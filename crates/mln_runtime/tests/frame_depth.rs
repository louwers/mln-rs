// The depth that frames hand the host, and the projection that they report:
// a fill extrusion's roof has the depth that the frame's projection gives
// its points, on every backend, and the ground in front of it a 2D layer's,
// between the projection's far depth and 1. A backend that this build or
// machine lacks skips its cases.

mod support;

use mln_map::{CameraOptions, CameraUpdate, MapMode};
use mln_runtime::{
    AcquiredFrame, AttachOptions, ClipDepthRange, DemandMode, Device, DeviceOptions, ErrorKind, FrameDemand,
    FrameDisposition, FrameProjection, FrameResult, Map, MapOptions, RenderSession, RenderTarget, Runtime, SyncKind,
    SyncKinds,
};
use mln_util::geo::{EdgeInsets, LatLng};
use mln_util::mat4::{Mat4, matrix};
use mln_util::size::Size;
use support::{eventually, runtime, wait};

const SIZE: Size = Size { width: 64, height: 48 };
/// The longitude of the extrusion's east wall: it covers the map west of it.
const WALL: f64 = -74.0;
/// The extrusion's height, in meters.
const HEIGHT: f64 = 4.0;

/// An opaque fill, the ground, under a fill extrusion that covers the map
/// west of `WALL`.
fn style() -> String {
    format!(
        r##"{{"version":8,"transition":{{"duration":0,"delay":0}},
        "sources":{{
          "block":{{"type":"geojson","data":{{"type":"Feature","properties":{{}},
            "geometry":{{"type":"Polygon","coordinates":[[[-74.1,40.6],[{WALL},40.6],[{WALL},40.8],[-74.1,40.8],[-74.1,40.6]]]}}}}}},
          "ground":{{"type":"geojson","data":{{"type":"Feature","properties":{{}},
            "geometry":{{"type":"Polygon","coordinates":[[[-75,40],[-73,40],[-73,41],[-75,41],[-75,40]]]}}}}}}}},
        "layers":[
            {{"id":"background","type":"background","paint":{{"background-color":"#0000ff"}}}},
            {{"id":"ground","type":"fill","source":"ground","paint":{{"fill-color":"#00ff00"}}}},
            {{"id":"block","type":"fill-extrusion","source":"block",
              "paint":{{"fill-extrusion-color":"#ff0000","fill-extrusion-height":{HEIGHT},"fill-extrusion-base":0}}}}]}}"##
    )
}

/// A map that looks at the wall from its east, with padding, roll, pitch
/// and bearing.
fn map_of(runtime: &Runtime) -> Map {
    let options = MapOptions {
        map: mln_map::MapOptions { size: SIZE, map_mode: MapMode::Continuous, ..Default::default() },
        ..Default::default()
    };
    let (map, created) = runtime.create_map(options).unwrap();
    created.wait().unwrap();
    wait(map.set_style_json(style()));
    let camera = CameraOptions {
        center: Some(LatLng::new(40.7, WALL + 0.0001).unwrap()),
        zoom: Some(17.0),
        bearing: Some(-70.0),
        pitch: Some(50.0),
        roll: Some(6.0),
        padding: Some(EdgeInsets::new(6.0, 10.0, 0.0, 0.0).unwrap()),
        ..Default::default()
    };
    wait(map.update_camera(CameraUpdate { camera, ..CameraUpdate::default() }));
    map
}

fn attach(map: &Map, target: RenderTarget, options: AttachOptions) -> RenderSession {
    let options = AttachOptions { producer_sync_kinds: SyncKinds::of(&[SyncKind::CpuComplete]), ..options };
    let (session, attached) = map.attach(target, options).unwrap();
    attached.wait().unwrap();
    session
}

/// Renders frames until one shows everything loaded, and acquires it. The
/// frames before it go back to the session.
fn loaded_frame(session: &RenderSession) -> (AcquiredFrame, FrameResult) {
    let mut loaded = None;
    assert!(eventually(|| {
        session.demand_frame(FrameDemand::new(DemandMode::Force, 1)).unwrap();
        let mut results = Vec::new();
        while results.is_empty() {
            results.extend(session.drain_frame_results().unwrap());
        }
        let Some(frame) = session.acquire_frame().unwrap() else { return false };
        let result = *frame.result();
        if result.disposition == FrameDisposition::Rendered && result.mode == mln_map::RenderMode::Full {
            loaded = Some((frame, result));
            return true;
        }
        drop(frame);
        false
    }));
    loaded.unwrap()
}

/// A point of the world, x and y in Web Mercator's 0 to 1, z in meters.
type Point = [f64; 3];

fn transform(matrix: &Mat4, point: [f64; 4]) -> [f64; 4] {
    let mut out = [0.0; 4];
    matrix::transform_mat4(&mut out, &point, matrix);
    out
}

/// The depth that `projection` gives `point`, under the default depth range.
fn depth_of(projection: &FrameProjection, point: Point) -> f64 {
    let clip = transform(&projection.matrix, [point[0], point[1], point[2], 1.0]);
    let z = clip[2] / clip[3];
    match projection.clip_depth_range {
        ClipDepthRange::ZeroToOne => z,
        ClipDepthRange::NegativeOneToOne => f64::midpoint(z, 1.0),
    }
}

/// Where the view ray through the center of pixel `(x, y)`, from the top
/// left, meets the plane of altitude `z`.
fn ray_at(projection: &FrameProjection, x: u32, y: u32, z: f64) -> Point {
    let mut inverse = [0.0; 16];
    // `invert` reports a singular matrix.
    assert!(!matrix::invert(&mut inverse, &projection.matrix));
    let ndc_x = (f64::from(x) + 0.5) / f64::from(SIZE.width) * 2.0 - 1.0;
    let ndc_y = 1.0 - (f64::from(y) + 0.5) / f64::from(SIZE.height) * 2.0;
    let unproject = |depth: f64| {
        let ndc_z = match projection.clip_depth_range {
            ClipDepthRange::ZeroToOne => depth,
            ClipDepthRange::NegativeOneToOne => 2.0 * depth - 1.0,
        };
        let world = transform(&inverse, [ndc_x, ndc_y, ndc_z, 1.0]);
        [world[0] / world[3], world[1] / world[3], world[2] / world[3]]
    };
    let (near, far) = (unproject(0.2), unproject(0.8));
    let t = (z - near[2]) / (far[2] - near[2]);
    [near[0] + t * (far[0] - near[0]), near[1] + t * (far[1] - near[1]), z]
}

/// Checks the depth that `depth_at` reads at each pixel, from the top left,
/// against `projection`: the roof's where the ray meets it west of the wall,
/// and a 2D layer's where it meets the ground east of the wall.
fn check_depth(backend: &str, projection: &FrameProjection, depth_at: impl Fn(u32, u32) -> f64) {
    assert!(!projection.globe);
    assert!(projection.far_depth > 0.99 && projection.far_depth < 1.0, "{backend}: {projection:?}");
    let wall = (WALL + 180.0) / 360.0;
    // Two pixels at zoom 17, from the wall.
    let margin = 2.0 / (512.0 * 2f64.powi(17));
    let (mut roof, mut ground) = (0, 0);
    for y in (1..SIZE.height).step_by(3) {
        for x in (1..SIZE.width).step_by(3) {
            let depth = depth_at(x, y);
            let on_roof = ray_at(projection, x, y, HEIGHT);
            let on_ground = ray_at(projection, x, y, 0.0);
            if on_roof[0] < wall - margin {
                let expected = depth_of(projection, on_roof);
                assert!(
                    (depth - expected).abs() < 2e-4,
                    "{backend}: the roof at ({x}, {y}) has depth {depth}, not {expected}"
                );
                roof += 1;
            } else if on_roof[0] > wall + margin && on_ground[0] > wall + margin {
                assert!(
                    depth > projection.far_depth && depth < 1.0,
                    "{backend}: the ground at ({x}, {y}) has depth {depth}, outside its layer's"
                );
                ground += 1;
            }
        }
    }
    assert!(roof > 10 && ground > 10, "{backend}: {roof} roof and {ground} ground pixels");
}

/// A device of `backend`, or `None` where this build or machine has none.
fn device_of(backend: mln_runtime::BackendType) -> Option<Device> {
    match Device::with_backend(backend, DeviceOptions::default()) {
        Ok(device) => Some(device),
        Err(error) if error.kind == ErrorKind::Unsupported => {
            eprintln!("skipped: no {backend:?} device: {error}");
            None
        }
        Err(error) => panic!("a {backend:?} device: {error}"),
    }
}

#[cfg(all(feature = "vulkan", not(any(target_vendor = "apple", target_family = "wasm"))))]
#[expect(unsafe_code, reason = "the tests make the host's images and read back their depth through ash")]
mod vulkan {
    use mln_runtime::{BackendType, BorrowedRing, ExportKind, ExportKinds, ExportedImage, ExportedRing, NativeTexture};
    use mln_vulkan::ash::vk::Handle as _;
    use mln_vulkan::ash::{self, vk};

    use super::*;

    const COLOR: vk::Format = vk::Format::R8G8B8A8_UNORM;

    /// The host's side of the library's Vulkan device: images, and copies of
    /// their depth to memory that it maps.
    struct Host {
        instance: ash::Instance,
        device: ash::Device,
        raw: mln_vulkan::RawDevice,
        pool: vk::CommandPool,
        images: Vec<(vk::Image, vk::ImageView, vk::DeviceMemory)>,
    }

    impl Host {
        fn new(vulkan: &mln_vulkan::Device) -> Host {
            let raw = vulkan.raw();
            // SAFETY: the device's own loader function and handles.
            let (instance, device) = unsafe {
                let entry =
                    ash::Entry::from_static_fn(ash::StaticFn { get_instance_proc_addr: raw.get_instance_proc_addr });
                let instance = ash::Instance::load(entry.static_fn(), raw.instance);
                let device = ash::Device::load(instance.fp_v1_0(), raw.device);
                (instance, device)
            };
            let info = vk::CommandPoolCreateInfo::default().queue_family_index(raw.queue_family_index);
            // SAFETY: a valid create info.
            let pool = unsafe { device.create_command_pool(&info, None) }.unwrap();
            Host { instance, device, raw, pool, images: Vec::new() }
        }

        fn memory(&self, requirements: vk::MemoryRequirements, flags: vk::MemoryPropertyFlags) -> vk::DeviceMemory {
            // SAFETY: a plain query of the GPU.
            let properties = unsafe { self.instance.get_physical_device_memory_properties(self.raw.physical_device) };
            let index = (0..properties.memory_type_count)
                .find(|&index| {
                    requirements.memory_type_bits & (1 << index) != 0
                        && properties.memory_types[index as usize].property_flags.contains(flags)
                })
                .unwrap();
            let info = vk::MemoryAllocateInfo::default().allocation_size(requirements.size).memory_type_index(index);
            // SAFETY: a valid allocate info.
            unsafe { self.device.allocate_memory(&info, None) }.unwrap()
        }

        /// An image of the ring's size, of `format`, with a view of `aspect`.
        fn image(
            &mut self,
            format: vk::Format,
            usage: vk::ImageUsageFlags,
            aspect: vk::ImageAspectFlags,
        ) -> mln_vulkan::BorrowedImage {
            let info = vk::ImageCreateInfo::default()
                .image_type(vk::ImageType::TYPE_2D)
                .format(format)
                .extent(vk::Extent3D { width: SIZE.width, height: SIZE.height, depth: 1 })
                .mip_levels(1)
                .array_layers(1)
                .samples(vk::SampleCountFlags::TYPE_1)
                .tiling(vk::ImageTiling::OPTIMAL)
                .usage(usage | vk::ImageUsageFlags::TRANSFER_SRC)
                .initial_layout(vk::ImageLayout::UNDEFINED);
            // SAFETY: valid create infos, and memory of the image's
            // requirements.
            unsafe {
                let image = self.device.create_image(&info, None).unwrap();
                let memory =
                    self.memory(self.device.get_image_memory_requirements(image), vk::MemoryPropertyFlags::empty());
                self.device.bind_image_memory(image, memory, 0).unwrap();
                let view_info = vk::ImageViewCreateInfo::default()
                    .image(image)
                    .view_type(vk::ImageViewType::TYPE_2D)
                    .format(format)
                    .subresource_range(vk::ImageSubresourceRange {
                        aspect_mask: aspect,
                        base_mip_level: 0,
                        level_count: 1,
                        base_array_layer: 0,
                        layer_count: 1,
                    });
                let view = self.device.create_image_view(&view_info, None).unwrap();
                self.images.push((image, view, memory));
                mln_vulkan::BorrowedImage { image, view }
            }
        }

        /// The depth of `image`, of `format` and in `layout`, at each pixel,
        /// row by row from the top. With `external`, the copy first acquires
        /// the image from `VK_QUEUE_FAMILY_EXTERNAL`, which an exported ring
        /// releases its images to.
        fn read_depth(
            &self,
            image: vk::Image,
            format: vk::Format,
            layout: vk::ImageLayout,
            external: bool,
        ) -> Vec<f64> {
            let size = u64::from(SIZE.width * SIZE.height * 4);
            // SAFETY: valid create and allocate infos; the command buffer
            // records one copy of the image's depth, and the host waits for
            // it before it maps the buffer.
            unsafe {
                let info = vk::BufferCreateInfo::default().size(size).usage(vk::BufferUsageFlags::TRANSFER_DST);
                let buffer = self.device.create_buffer(&info, None).unwrap();
                let flags = vk::MemoryPropertyFlags::HOST_VISIBLE | vk::MemoryPropertyFlags::HOST_COHERENT;
                let memory = self.memory(self.device.get_buffer_memory_requirements(buffer), flags);
                self.device.bind_buffer_memory(buffer, memory, 0).unwrap();
                let info = vk::CommandBufferAllocateInfo::default()
                    .command_pool(self.pool)
                    .level(vk::CommandBufferLevel::PRIMARY)
                    .command_buffer_count(1);
                let command_buffer = self.device.allocate_command_buffers(&info).unwrap()[0];
                self.device.begin_command_buffer(command_buffer, &vk::CommandBufferBeginInfo::default()).unwrap();
                let range = vk::ImageSubresourceRange {
                    aspect_mask: vk::ImageAspectFlags::DEPTH | vk::ImageAspectFlags::STENCIL,
                    base_mip_level: 0,
                    level_count: 1,
                    base_array_layer: 0,
                    layer_count: 1,
                };
                let (source, destination) = if external {
                    (vk::QUEUE_FAMILY_EXTERNAL, self.raw.queue_family_index)
                } else {
                    (vk::QUEUE_FAMILY_IGNORED, vk::QUEUE_FAMILY_IGNORED)
                };
                let barrier = vk::ImageMemoryBarrier::default()
                    .src_access_mask(vk::AccessFlags::empty())
                    .dst_access_mask(vk::AccessFlags::TRANSFER_READ)
                    .old_layout(layout)
                    .new_layout(layout)
                    .src_queue_family_index(source)
                    .dst_queue_family_index(destination)
                    .image(image)
                    .subresource_range(range);
                self.device.cmd_pipeline_barrier(
                    command_buffer,
                    vk::PipelineStageFlags::TOP_OF_PIPE,
                    vk::PipelineStageFlags::TRANSFER,
                    vk::DependencyFlags::empty(),
                    &[],
                    &[],
                    &[barrier],
                );
                let region = vk::BufferImageCopy::default()
                    .image_subresource(vk::ImageSubresourceLayers {
                        aspect_mask: vk::ImageAspectFlags::DEPTH,
                        mip_level: 0,
                        base_array_layer: 0,
                        layer_count: 1,
                    })
                    .image_extent(vk::Extent3D { width: SIZE.width, height: SIZE.height, depth: 1 });
                self.device.cmd_copy_image_to_buffer(command_buffer, image, layout, buffer, &[region]);
                self.device.end_command_buffer(command_buffer).unwrap();
                let fence = self.device.create_fence(&vk::FenceCreateInfo::default(), None).unwrap();
                let command_buffers = [command_buffer];
                let submit = vk::SubmitInfo::default().command_buffers(&command_buffers);
                self.device.queue_submit(self.raw.queue, &[submit], fence).unwrap();
                self.device.wait_for_fences(&[fence], true, u64::MAX).unwrap();
                let mapped = self.device.map_memory(memory, 0, size, vk::MemoryMapFlags::empty()).unwrap();
                let words = std::slice::from_raw_parts(mapped.cast::<u32>(), (SIZE.width * SIZE.height) as usize);
                let depths = words
                    .iter()
                    .map(|&word| match format {
                        vk::Format::D32_SFLOAT_S8_UINT => f64::from(f32::from_bits(word)),
                        // The depth is the low 24 bits of the word.
                        _ => f64::from(word & 0x00ff_ffff) / f64::from(0x00ff_ffffu32),
                    })
                    .collect();
                self.device.unmap_memory(memory);
                self.device.destroy_fence(fence, None);
                self.device.free_command_buffers(self.pool, &command_buffers);
                self.device.destroy_buffer(buffer, None);
                self.device.free_memory(memory, None);
                depths
            }
        }

        fn supports(&self, format: vk::Format) -> bool {
            // SAFETY: a plain query of the GPU.
            let properties =
                unsafe { self.instance.get_physical_device_format_properties(self.raw.physical_device, format) };
            properties.optimal_tiling_features.contains(vk::FormatFeatureFlags::DEPTH_STENCIL_ATTACHMENT)
        }
    }

    impl Drop for Host {
        fn drop(&mut self) {
            // SAFETY: the host's objects, which no session uses any more.
            unsafe {
                let _ = self.device.device_wait_idle();
                self.device.destroy_command_pool(self.pool, None);
                for (image, view, memory) in self.images.drain(..) {
                    self.device.destroy_image_view(view, None);
                    self.device.destroy_image(image, None);
                    self.device.free_memory(memory, None);
                }
            }
        }
    }

    fn vulkan_device() -> Option<(Device, mln_vulkan::Device)> {
        let device = device_of(BackendType::Vulkan)?;
        let Device::Vulkan(vulkan) = &device else { unreachable!() };
        let vulkan = vulkan.clone();
        Some((device, vulkan))
    }

    // A borrowed ring's depth images get the frame's depth, which the
    // frame's projection predicts.
    #[test]
    fn a_borrowed_rings_depth_images_hold_the_frames_depth() {
        let Some((device, vulkan)) = vulkan_device() else { return };
        let mut host = Host::new(&vulkan);
        let format = vk::Format::D32_SFLOAT_S8_UINT;
        if !host.supports(format) {
            eprintln!("skipped: the Vulkan device draws no depth into {format:?}");
            return;
        }
        let depth_aspects = vk::ImageAspectFlags::DEPTH | vk::ImageAspectFlags::STENCIL;
        let colors: Vec<_> = (0..2)
            .map(|_| host.image(COLOR, vk::ImageUsageFlags::COLOR_ATTACHMENT, vk::ImageAspectFlags::COLOR))
            .collect();
        let depths: Vec<_> =
            (0..2).map(|_| host.image(format, vk::ImageUsageFlags::DEPTH_STENCIL_ATTACHMENT, depth_aspects)).collect();
        let ring = BorrowedRing::vulkan(mln_vulkan::BorrowedImages {
            images: colors,
            size: SIZE,
            format: COLOR,
            final_layout: vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
            owner: mln_vulkan::ImageOwner::GraphicsQueue,
            depth: Some(mln_vulkan::BorrowedDepth {
                images: depths.clone(),
                format,
                final_layout: vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
            }),
        });
        let runtime = runtime();
        let map = map_of(&runtime);
        let target = RenderTarget::Borrowed { size: SIZE, ring, device };
        let session = attach(&map, target, AttachOptions::default());
        let (frame, result) = loaded_frame(&session);
        let projection = result.projection.expect("a rendered frame has its projection");
        assert_eq!(ClipDepthRange::ZeroToOne, projection.clip_depth_range);
        let texture = frame.texture().unwrap();
        let Some(NativeTexture::Vulkan { image, .. }) = texture.depth.as_ref().map(|depth| depth.texture) else {
            panic!("the frame has no depth image: {texture:?}");
        };
        assert_eq!(depths[frame.slot()].image.as_raw(), image);
        let depth = host.read_depth(depths[frame.slot()].image, format, vk::ImageLayout::TRANSFER_SRC_OPTIMAL, false);
        check_depth("a Vulkan ring", &projection, |x, y| depth[(y * SIZE.width + x) as usize]);
        drop(frame);
        wait(session.detach());
    }

    // An exported ring that keeps depth hands out each frame's depth image
    // on the device, and its memory as an opaque file descriptor.
    #[test]
    fn an_exported_ring_keeps_the_frames_depth() {
        let Some((device, vulkan)) = vulkan_device() else { return };
        if !vulkan.depth_export_kinds().contains(ExportKind::OpaqueFd) {
            eprintln!("skipped: the Vulkan device exports no depth images");
            return;
        }
        let host = Host::new(&vulkan);
        let ring = ExportedRing {
            depth: 2,
            kinds: ExportKinds::of(&[ExportKind::OpaqueFd]),
            dma_buf_formats: Vec::new(),
            android_hardware_buffer_usage: 0,
            keep_depth: true,
        };
        let runtime = runtime();
        let map = map_of(&runtime);
        let target = RenderTarget::Exported { size: SIZE, ring, device: Some(device) };
        let session = attach(&map, target, AttachOptions::default());
        let (frame, result) = loaded_frame(&session);
        let projection = result.projection.unwrap();
        let texture = frame.texture().unwrap();
        let depth = texture.depth.clone().expect("the frame keeps its depth");
        let Some(ExportedImage::OpaqueFd { fd, allocation_size, dedicated: true, .. }) = depth.export else {
            panic!("the frame shares no depth memory: {depth:?}");
        };
        assert!(fd >= 0 && allocation_size > 0);
        let NativeTexture::Vulkan { image, format, layout, .. } = depth.texture else {
            panic!("the frame has no depth image on the device: {depth:?}");
        };
        assert_eq!(vk::ImageLayout::GENERAL.as_raw(), layout);
        let depth =
            host.read_depth(vk::Image::from_raw(image), vk::Format::from_raw(format), vk::ImageLayout::GENERAL, true);
        check_depth("a Vulkan exported ring", &projection, |x, y| depth[(y * SIZE.width + x) as usize]);
        drop(frame);
        wait(session.detach());
    }

    // An exported ring keeps no depth unless it is asked to.
    #[test]
    fn an_exported_ring_keeps_no_depth_by_default() {
        let Some((device, vulkan)) = vulkan_device() else { return };
        if !vulkan.export_kinds().contains(ExportKind::OpaqueFd) {
            eprintln!("skipped: the Vulkan device exports no opaque memory");
            return;
        }
        let ring = ExportedRing {
            depth: 1,
            kinds: ExportKinds::of(&[ExportKind::OpaqueFd]),
            dma_buf_formats: Vec::new(),
            android_hardware_buffer_usage: 0,
            keep_depth: false,
        };
        let runtime = runtime();
        let map = map_of(&runtime);
        let target = RenderTarget::Exported { size: SIZE, ring, device: Some(device) };
        let session = attach(&map, target, AttachOptions::default());
        let (frame, _) = loaded_frame(&session);
        let texture = frame.texture().unwrap();
        assert_eq!(None, texture.depth);
        assert!(matches!(texture.native_kind(), Some(mln_runtime::FrameTextureKind::Vulkan { .. })));
        drop(frame);
        wait(session.detach());
    }
}

// A WebGPU ring's depth textures get the frame's depth.
#[cfg(all(feature = "webgpu", not(target_family = "wasm")))]
#[test]
fn a_webgpu_rings_depth_textures_hold_the_frames_depth() {
    let Some(device) = device_of(mln_runtime::BackendType::WebGPU) else { return };
    let Device::WebGpu(webgpu) = &device else { unreachable!() };
    let (wgpu_device, queue) = (webgpu.device().clone(), webgpu.queue().clone());
    if !wgpu_device.features().contains(wgpu::Features::DEPTH32FLOAT_STENCIL8) {
        eprintln!("skipped: the WebGPU device has no Depth32FloatStencil8");
        return;
    }
    let texture = |format| {
        wgpu_device.create_texture(&wgpu::TextureDescriptor {
            label: None,
            size: wgpu::Extent3d { width: SIZE.width, height: SIZE.height, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        })
    };
    let textures = vec![texture(wgpu::TextureFormat::Rgba8Unorm), texture(wgpu::TextureFormat::Rgba8Unorm)];
    let depths =
        vec![texture(wgpu::TextureFormat::Depth32FloatStencil8), texture(wgpu::TextureFormat::Depth32FloatStencil8)];
    let ring = mln_runtime::BorrowedRing::webgpu(mln_webgpu::BorrowedTextures {
        textures,
        size: SIZE,
        depth_textures: depths.clone(),
    });
    let runtime = runtime();
    let map = map_of(&runtime);
    let target = RenderTarget::Borrowed { size: SIZE, ring, device: device.clone() };
    let session = attach(&map, target, AttachOptions::default());
    let (frame, result) = loaded_frame(&session);
    let projection = result.projection.unwrap();
    assert_eq!(ClipDepthRange::ZeroToOne, projection.clip_depth_range);
    assert!(frame.texture().unwrap().depth.is_some());
    // 64 pixels of 4 bytes, a row of the 256 bytes that copies align to.
    let bytes_per_row = SIZE.width * 4;
    let buffer = wgpu_device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: u64::from(bytes_per_row * SIZE.height),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = wgpu_device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
    let depth = &depths[frame.slot()];
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture: depth,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::DepthOnly,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &buffer,
            layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(bytes_per_row), rows_per_image: None },
        },
        depth.size(),
    );
    queue.submit([encoder.finish()]);
    buffer.slice(..).map_async(wgpu::MapMode::Read, |_| {});
    wgpu_device.poll(wgpu::PollType::Wait { submission_index: None, timeout: None }).unwrap();
    let data = buffer.slice(..).get_mapped_range().unwrap();
    let depth: Vec<f64> = data.as_chunks::<4>().0.iter().map(|bytes| f64::from(f32::from_le_bytes(*bytes))).collect();
    drop(data);
    check_depth("a WebGPU ring", &projection, |x, y| depth[(y * SIZE.width + x) as usize]);
    drop(frame);
    wait(session.detach());
}

#[cfg(all(feature = "gl", target_os = "linux"))]
#[expect(unsafe_code, reason = "the tests make the host's context and read back its depth through EGL and OpenGL")]
mod gl {
    use std::ptr::NonNull;

    use glow::HasContext as _;
    use khronos_egl as egl;

    use super::*;

    type Egl = egl::DynamicInstance<egl::EGL1_4>;

    /// Reads a depth texture by drawing its depth, as 24 bits, into the
    /// bytes of a color texture.
    const VERTEX: &str = "#version 300 es
        void main() {
            vec2 corner = vec2(float((gl_VertexID & 1) << 2), float((gl_VertexID & 2) << 1)) - 1.0;
            gl_Position = vec4(corner, 0.0, 1.0);
        }";
    const FRAGMENT: &str = "#version 300 es
        precision highp float;
        precision highp int;
        uniform highp sampler2D depth;
        out vec4 color;
        void main() {
            uint value = uint(texelFetch(depth, ivec2(gl_FragCoord.xy), 0).r * 16777215.0 + 0.5);
            color = vec4(uvec4(value & 255u, (value >> 8) & 255u, (value >> 16) & 255u, 255u)) / 255.0;
        }";

    /// The host's OpenGL ES 3 context, current on a pbuffer, in whose share
    /// group a device renders.
    struct Host {
        egl: Egl,
        display: egl::Display,
        config: egl::Config,
        context: egl::Context,
        pbuffer: egl::Surface,
        gl: glow::Context,
    }

    impl Host {
        fn new() -> Option<Host> {
            const PLATFORM_SURFACELESS_MESA: egl::Enum = 0x31DD;
            // SAFETY: loading the system's EGL.
            let egl = unsafe { Egl::load_required() }.ok()?;
            // SAFETY: the default native display is valid on every platform.
            let display = unsafe {
                egl.upcast::<egl::EGL1_5>()
                    .and_then(|egl| {
                        egl.get_platform_display(PLATFORM_SURFACELESS_MESA, egl::DEFAULT_DISPLAY, &[egl::ATTRIB_NONE])
                            .ok()
                    })
                    .or_else(|| egl.get_display(egl::DEFAULT_DISPLAY))
            }?;
            egl.initialize(display).ok()?;
            egl.bind_api(egl::OPENGL_ES_API).ok()?;
            let attributes = [
                egl::RENDERABLE_TYPE,
                egl::OPENGL_ES3_BIT,
                egl::SURFACE_TYPE,
                egl::PBUFFER_BIT,
                egl::RED_SIZE,
                8,
                egl::GREEN_SIZE,
                8,
                egl::BLUE_SIZE,
                8,
                egl::ALPHA_SIZE,
                8,
                egl::NONE,
            ];
            let config = egl.choose_first_config(display, &attributes).ok()??;
            let context =
                egl.create_context(display, config, None, &[egl::CONTEXT_CLIENT_VERSION, 3, egl::NONE]).ok()?;
            let size = [egl::WIDTH, SIZE.width as i32, egl::HEIGHT, SIZE.height as i32, egl::NONE];
            let pbuffer = egl.create_pbuffer_surface(display, config, &size).ok()?;
            egl.make_current(display, Some(pbuffer), Some(pbuffer), Some(context)).ok()?;
            // SAFETY: the context is current, and EGL gives its functions.
            let gl = unsafe {
                glow::Context::from_loader_function(|name| {
                    egl.get_proc_address(name).map_or(std::ptr::null(), |function| function as *const _)
                })
            };
            egl.make_current(display, None, None, None).ok()?;
            Some(Host { egl, display, config, context, pbuffer, gl })
        }

        fn current(&self) {
            self.egl.make_current(self.display, Some(self.pbuffer), Some(self.pbuffer), Some(self.context)).unwrap();
        }

        fn done(&self) {
            self.egl.make_current(self.display, None, None, None).unwrap();
        }

        fn device(&self) -> Device {
            let pointer = |pointer: *mut std::ffi::c_void| NonNull::new(pointer).unwrap();
            // SAFETY: the host's live display, config and context, which
            // outlive the device's sessions.
            let device = unsafe {
                mln_gl::Device::from_raw(
                    pointer(self.display.as_ptr()),
                    pointer(self.config.as_ptr()),
                    pointer(self.context.as_ptr()),
                )
            };
            Device::Gl(device.unwrap())
        }

        /// A new texture of the ring's size and `format`.
        fn texture(&self, format: u32) -> glow::Texture {
            self.current();
            // SAFETY: the host's context is current.
            let texture = unsafe {
                let texture = self.gl.create_texture().unwrap();
                self.gl.bind_texture(glow::TEXTURE_2D, Some(texture));
                self.gl.tex_storage_2d(glow::TEXTURE_2D, 1, format, SIZE.width as i32, SIZE.height as i32);
                self.gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MIN_FILTER, glow::NEAREST as i32);
                self.gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MAG_FILTER, glow::NEAREST as i32);
                self.gl.bind_texture(glow::TEXTURE_2D, None);
                self.gl.finish();
                texture
            };
            self.done();
            texture
        }

        /// The depth of `texture` at each pixel, row by row from OpenGL's
        /// bottom.
        fn read_depth(&self, texture: glow::Texture) -> Vec<f64> {
            let target = self.texture(glow::RGBA8);
            self.current();
            let mut bytes = vec![0u8; (SIZE.width * SIZE.height * 4) as usize];
            // SAFETY: the host's context is current; the program, the vertex
            // array and the framebuffer are new, and the textures its.
            unsafe {
                let gl = &self.gl;
                let program = gl.create_program().unwrap();
                for (kind, source) in [(glow::VERTEX_SHADER, VERTEX), (glow::FRAGMENT_SHADER, FRAGMENT)] {
                    let shader = gl.create_shader(kind).unwrap();
                    gl.shader_source(shader, source);
                    gl.compile_shader(shader);
                    assert!(gl.get_shader_compile_status(shader), "{}", gl.get_shader_info_log(shader));
                    gl.attach_shader(program, shader);
                }
                gl.link_program(program);
                assert!(gl.get_program_link_status(program), "{}", gl.get_program_info_log(program));
                let framebuffer = gl.create_framebuffer().unwrap();
                gl.bind_framebuffer(glow::FRAMEBUFFER, Some(framebuffer));
                gl.framebuffer_texture_2d(
                    glow::FRAMEBUFFER,
                    glow::COLOR_ATTACHMENT0,
                    glow::TEXTURE_2D,
                    Some(target),
                    0,
                );
                let vertex_array = gl.create_vertex_array().unwrap();
                gl.bind_vertex_array(Some(vertex_array));
                gl.viewport(0, 0, SIZE.width as i32, SIZE.height as i32);
                gl.use_program(Some(program));
                gl.active_texture(glow::TEXTURE0);
                gl.bind_texture(glow::TEXTURE_2D, Some(texture));
                gl.uniform_1_i32(gl.get_uniform_location(program, "depth").as_ref(), 0);
                gl.draw_arrays(glow::TRIANGLES, 0, 3);
                gl.read_pixels(
                    0,
                    0,
                    SIZE.width as i32,
                    SIZE.height as i32,
                    glow::RGBA,
                    glow::UNSIGNED_BYTE,
                    glow::PixelPackData::Slice(Some(&mut bytes)),
                );
                gl.bind_framebuffer(glow::FRAMEBUFFER, None);
                gl.delete_framebuffer(framebuffer);
                gl.delete_vertex_array(vertex_array);
                gl.delete_program(program);
                gl.delete_texture(target);
            }
            self.done();
            bytes
                .as_chunks::<4>()
                .0
                .iter()
                .map(|pixel| {
                    let value = u32::from(pixel[0]) | (u32::from(pixel[1]) << 8) | (u32::from(pixel[2]) << 16);
                    f64::from(value) / f64::from(0x00ff_ffffu32)
                })
                .collect()
        }
    }

    impl Drop for Host {
        fn drop(&mut self) {
            let _ = self.egl.destroy_surface(self.display, self.pbuffer);
            let _ = self.egl.destroy_context(self.display, self.context);
        }
    }

    // An OpenGL ring's depth textures get the frame's depth, which the
    // frame's projection predicts in OpenGL's clip space: the same depth
    // that the other backends write.
    #[test]
    fn an_open_gl_rings_depth_textures_hold_the_frames_depth() {
        let Some(host) = Host::new() else {
            eprintln!("skipped: no EGL context of OpenGL ES 3 with pbuffers");
            return;
        };
        let colors = [host.texture(glow::RGBA8), host.texture(glow::RGBA8)];
        let depths = [host.texture(glow::DEPTH32F_STENCIL8), host.texture(glow::DEPTH32F_STENCIL8)];
        let ring = mln_runtime::BorrowedRing::gl(mln_gl::BorrowedTextures {
            textures: colors.iter().map(|texture| texture.0.get()).collect(),
            size: SIZE,
            depth_textures: depths.iter().map(|texture| texture.0.get()).collect(),
        });
        let runtime = runtime();
        let map = map_of(&runtime);
        let target = RenderTarget::Borrowed { size: SIZE, ring, device: host.device() };
        let session = attach(&map, target, AttachOptions::default());
        let (frame, result) = loaded_frame(&session);
        let projection = result.projection.unwrap();
        assert_eq!(ClipDepthRange::NegativeOneToOne, projection.clip_depth_range);
        // The host samples the texture in its own context, as a host that
        // reads the frame's depth does.
        let depth = host.read_depth(depths[frame.slot()]);
        // OpenGL's rows start at the bottom.
        check_depth("an OpenGL ring", &projection, |x, y| depth[((SIZE.height - 1 - y) * SIZE.width + x) as usize]);
        drop(frame);
        wait(session.detach());
    }

    // An OpenGL device's exported ring that keeps depth copies each frame's
    // depth beside its dmabuf, top row first as the dmabuf holds its
    // frame, into a texture of the device's share group.
    #[test]
    fn an_open_gl_exported_ring_keeps_the_frames_depth() {
        use mln_runtime::{DmaBufFormat, ExportKind, ExportKinds, ExportedRing, NativeTexture};

        let Some(host) = Host::new() else {
            eprintln!("skipped: no EGL context of OpenGL ES 3 with pbuffers");
            return;
        };
        let device = host.device();
        if !device.export_kinds().contains(ExportKind::DmaBuf) {
            eprintln!("skipped: the OpenGL device exports no dmabufs");
            return;
        }
        let ring = ExportedRing {
            depth: 2,
            kinds: ExportKinds::of(&[ExportKind::DmaBuf]),
            dma_buf_formats: vec![DmaBufFormat { fourcc: u32::from_le_bytes(*b"AB24"), modifier: 0 }],
            android_hardware_buffer_usage: 0,
            keep_depth: true,
        };
        let runtime = runtime();
        let map = map_of(&runtime);
        let target = RenderTarget::Exported { size: SIZE, ring, device: Some(device) };
        let session = attach(&map, target, AttachOptions::default());
        let (frame, result) = loaded_frame(&session);
        let projection = result.projection.unwrap();
        let texture = frame.texture().unwrap();
        assert!(matches!(texture.native_kind(), Some(mln_runtime::FrameTextureKind::OpenGl { .. })));
        let Some(NativeTexture::OpenGl { texture: name, internal_format, .. }) =
            texture.depth.as_ref().map(|depth| depth.texture)
        else {
            panic!("the frame keeps no depth texture: {texture:?}");
        };
        assert_eq!(glow::DEPTH24_STENCIL8, internal_format);
        let depth = host.read_depth(glow::NativeTexture(std::num::NonZeroU32::new(name).unwrap()));
        check_depth("an OpenGL exported ring", &projection, |x, y| depth[(y * SIZE.width + x) as usize]);
        drop(frame);
        wait(session.detach());
    }
}
