// wgpu vs. Dawn/wgpu-native: the readback maps the staging buffer and blocks
// with `Device::poll(PollType::Wait)` (C++: `wgpuInstanceWaitAny` on Dawn, a
// 1000 x 1 ms `wgpuDevicePoll` loop on wgpu-native).
//
// Port-only: the staging buffer is kept between reads, and after a read the
// next frame's own submission ends with the copy, which C++ submits
// separately on each read.
//
// There is no size-changed callback (`handleResize` / `setSizeListener`):
// an offscreen texture keeps its size, and a render target of another size
// makes a new one.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

use mln_gfx::command_encoder::CommandEncoder as GfxCommandEncoder;
use mln_gfx::context::Context as _;
use mln_gfx::texture2d::{SamplerState, Texture2DPtr, texture_format};
use mln_gfx::types::{TextureChannelDataType, TextureFilterType, TexturePixelType, TextureWrapType};
use mln_gfx::{frame, offscreen_texture};
use mln_util::image::PremultipliedImage;
use mln_util::logging::{Event, Log};
use mln_util::size::Size;
use mln_util::time::Instant;

use crate::command_encoder::CommandEncoder;
use crate::context::Context;
use crate::draw_target::{DrawTarget, TargetResource};

const BYTES_PER_ROW_ALIGNMENT: u32 = 256;

/// How long a readback waits again after waits that reported a timeout.
const WAIT_AGAIN_LIMIT: Duration = Duration::from_secs(1);

fn linear_clamp() -> SamplerState {
    SamplerState {
        filter: TextureFilterType::Linear,
        wrap_u: TextureWrapType::Clamp,
        wrap_v: TextureWrapType::Clamp,
        ..SamplerState::default()
    }
}

pub(crate) struct OffscreenTextureResource {
    context: Rc<Context>,
    size: Size,
    /// A texture of the backend's `Gpu`, which the renderer samples.
    color_texture: Texture2DPtr,
    /// The depth or depth-stencil attachment, and its view.
    depth_texture: Option<(wgpu::Texture, wgpu::TextureView)>,
    /// The buffer the color texture is copied into to be read back, kept
    /// for the next read.
    staging_buffer: RefCell<Option<wgpu::Buffer>>,
    /// The color texture was read back: the next frame copies it out.
    read_back: Cell<bool>,
    /// The submission (`Context::submissions`) whose copy the readback
    /// buffer holds.
    copied_at: Cell<Option<u64>>,
}

impl OffscreenTextureResource {
    pub(crate) fn new(
        context: Rc<Context>,
        size: Size,
        type_: TextureChannelDataType,
        depth: bool,
        stencil: bool,
    ) -> Self {
        debug_assert!(size.width != 0 && size.height != 0);
        let color_texture = context.create_texture_2d();
        {
            let mut texture = color_texture.borrow_mut();
            texture.set_size(size);
            texture.set_format(texture_format(TexturePixelType::RGBA, type_));
            texture.set_render_target();
            texture.set_sampler(linear_clamp());
        }
        // WebGPU requires a combined depth-stencil texture when both are
        // needed.
        let depth_texture = (depth || stencil).then(|| {
            let format = if depth && !stencil {
                wgpu::TextureFormat::Depth32Float
            } else {
                wgpu::TextureFormat::Depth24PlusStencil8
            };
            let texture = context.get_backend().device().create_texture(&wgpu::TextureDescriptor {
                label: Some("Offscreen depth"),
                size: wgpu::Extent3d { width: size.width, height: size.height, depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                view_formats: &[],
            });
            let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
            (texture, view)
        });
        // Port-only: the C++ WebGPU backend only decrements this count.
        context.rendering_stats_mut().num_frame_buffers += 1;
        OffscreenTextureResource {
            context,
            size,
            color_texture,
            depth_texture,
            staging_buffer: RefCell::default(),
            read_back: Cell::default(),
            copied_at: Cell::default(),
        }
    }

    /// The color texture's wgpu texture, made on first use.
    fn color(&self) -> Option<wgpu::Texture> {
        let created = self.color_texture.borrow_mut().create(&*self.context);
        created.ok().and_then(|texture| self.context.get_backend().native_texture(texture)).map(|(texture, _)| texture)
    }

    fn row_strides(&self) -> (u32, u32) {
        let row_stride = self.size.width * 4; // RGBA8 output
        (row_stride, row_stride.next_multiple_of(BYTES_PER_ROW_ALIGNMENT))
    }

    /// Records a copy of the color texture into the readback buffer, if
    /// the texture exists.
    fn encode_copy(&self, device: &wgpu::Device, encoder: &mut wgpu::CommandEncoder) -> bool {
        let Some(texture) = self.color() else {
            return false;
        };
        let size = self.size;
        let (_, aligned_row_stride) = self.row_strides();
        let mut staging = self.staging_buffer.borrow_mut();
        let staging = staging.get_or_insert_with(|| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("Readback Buffer"),
                size: u64::from(aligned_row_stride) * u64::from(size.height),
                usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                mapped_at_creation: false,
            })
        });
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: staging,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(aligned_row_stride),
                    rows_per_image: Some(size.height),
                },
            },
            wgpu::Extent3d { width: size.width, height: size.height, depth_or_array_layers: 1 },
        );
        true
    }

    /// Ends a frame with the copy a read asked for: a host reading frames
    /// back reads the next one too, and the copy then costs no submission
    /// of its own.
    pub(crate) fn encode_requested_copy(&self, encoder: &mut wgpu::CommandEncoder) {
        let device = self.context.get_backend().device();
        if self.read_back.replace(false) && self.encode_copy(device, encoder) {
            self.copied_at.set(Some(self.context.submissions()));
        }
    }

    /// The color texture's pixels. The copy is the one the last submission
    /// made (`encode_requested_copy`), or else one submitted now.
    pub(crate) fn read_still_image(&self) -> PremultipliedImage {
        let size = self.size;
        let (row_stride, aligned_row_stride) = self.row_strides();
        let mut data = vec![0u8; row_stride as usize * size.height as usize];
        self.read_back.set(true);

        let backend = self.context.get_backend();
        let (device, queue) = (backend.device(), backend.queue());

        // Any later submission may have drawn the texture again.
        if self.copied_at.take() != Some(self.context.submissions()) {
            let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
            if !self.encode_copy(device, &mut encoder) {
                Log::error(Event::Render, "WebGPU: Color texture is null");
                return PremultipliedImage::from_vec(size, data);
            }
            queue.submit([encoder.finish()]);
        }
        let staging = self.staging_buffer.borrow();
        let Some(staging_buffer) = staging.as_ref() else {
            return PremultipliedImage::from_vec(size, data);
        };

        // Map buffer and read data (blocking)
        let (tx, rx) = std::sync::mpsc::channel();
        staging_buffer.map_async(wgpu::MapMode::Read, .., move |result| {
            let _ = tx.send(result);
        });
        // A wait without a timeout can still report one: PowerVR's Vulkan
        // driver on Android ends the wait shortly before the fence value
        // that wgpu reads afterwards shows the submission finished. Waiting
        // again returns at once then, so this asks until the value caught up.
        let wait = || device.poll(wgpu::PollType::Wait { submission_index: None, timeout: None });
        let start = Instant::now();
        let mut waited = wait();
        while matches!(waited, Err(wgpu::PollError::Timeout)) && start.elapsed() < WAIT_AGAIN_LIMIT {
            waited = wait();
        }
        if let Err(e) = waited {
            Log::error(Event::Render, format!("WebGPU: Buffer mapping wait failed: {e}"));
        }
        match rx.try_recv() {
            Ok(Ok(())) => match staging_buffer.get_mapped_range(..) {
                Ok(mapped) => {
                    if aligned_row_stride == row_stride {
                        let n = data.len();
                        data.copy_from_slice(&mapped[..n]);
                    } else {
                        let rs = row_stride as usize;
                        let ars = aligned_row_stride as usize;
                        for row in 0..size.height as usize {
                            data[row * rs..(row + 1) * rs].copy_from_slice(&mapped[row * ars..row * ars + rs]);
                        }
                    }
                }
                Err(e) => Log::error(Event::Render, format!("WebGPU: Failed to get mapped range: {e}")),
            },
            Ok(Err(e)) => Log::error(Event::Render, format!("WebGPU: Buffer mapping failed with status: {e}")),
            Err(_) => Log::error(Event::Render, "WebGPU: Buffer mapping timeout after polling"),
        }
        staging_buffer.unmap();
        PremultipliedImage::from_vec(size, data)
    }

    pub(crate) fn get_texture(&self) -> Texture2DPtr {
        Rc::clone(&self.color_texture)
    }
}

impl Drop for OffscreenTextureResource {
    fn drop(&mut self) {
        self.context.rendering_stats_mut().num_frame_buffers -= 1;
    }
}

impl TargetResource for OffscreenTextureResource {
    fn finish_frame(&self, encoder: &mut wgpu::CommandEncoder) {
        self.encode_requested_copy(encoder);
    }

    fn get_color_texture_view(&self) -> Option<wgpu::TextureView> {
        let created = self.color_texture.borrow_mut().create(&*self.context);
        created.ok().and_then(|texture| self.context.get_backend().native_texture(texture)).map(|(_, view)| view)
    }

    fn get_color_texture_format(&self) -> Option<wgpu::TextureFormat> {
        self.color().map(|texture| texture.format())
    }

    fn get_depth_stencil_texture_view(&self) -> Option<wgpu::TextureView> {
        self.depth_texture.as_ref().map(|(_, view)| view.clone())
    }

    fn get_depth_stencil_texture_format(&self) -> Option<wgpu::TextureFormat> {
        self.depth_texture.as_ref().map(|(texture, _)| texture.format())
    }
}

pub(crate) struct OffscreenTexture {
    target: DrawTarget,
    resource: Rc<OffscreenTextureResource>,
}

impl OffscreenTexture {
    pub(crate) fn new(
        context: Rc<Context>,
        size: Size,
        type_: TextureChannelDataType,
        depth: bool,
        stencil: bool,
    ) -> Self {
        let resource = Rc::new(OffscreenTextureResource::new(context, size, type_, depth, stencil));
        OffscreenTexture { target: DrawTarget::new(size, Rc::<OffscreenTextureResource>::clone(&resource)), resource }
    }

    fn resource(&self) -> &OffscreenTextureResource {
        &self.resource
    }
}

impl offscreen_texture::OffscreenTexture for OffscreenTexture {
    fn draw_target(&self) -> &dyn frame::DrawTarget {
        &self.target
    }

    /// Check if the texture resource is valid and can be rendered to
    fn is_renderable(&self) -> bool {
        self.resource().color().is_some()
    }

    fn read_still_image(&self) -> PremultipliedImage {
        self.resource.read_still_image()
    }

    fn get_texture(&self) -> Texture2DPtr {
        self.resource().get_texture()
    }

    fn submit(&self, encoder: &dyn GfxCommandEncoder) {
        if let Some(encoder) = encoder.downcast_ref::<CommandEncoder>() {
            encoder.submit_frame(Some(self.target.resource()));
        }
    }
}
