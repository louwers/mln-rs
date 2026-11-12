// A ring of the target's own images whose memory it exports to the host:
// as dmabufs with a DRM format modifier, as opaque file descriptors for
// another API of the same driver, as Android hardware buffers, or on Windows
// as D3D11 textures. Frames draw into the images as into a borrowed ring's,
// and are queued for the host to acquire.
//
// Each image has memory of its own (a dedicated allocation), which the
// exported file descriptor names. A hardware buffer goes the other way: the
// target allocates it, with the usage bits that the host's reads need, which
// an export of Vulkan memory can't add, and imports it as the image's
// memory. So does a D3D11 texture's image, since Vulkan exports no memory
// that D3D11 opens as a texture: a D3D11 device on the GPU's adapter makes
// the texture and its NT handle.
//
// A resize makes a new generation of images; the frames that the host holds
// keep the old generation's images until they are given back.

use std::cell::RefCell;
#[cfg(unix)]
use std::os::fd::{AsRawFd as _, FromRawFd as _, OwnedFd};
use std::rc::Rc;

use ash::vk;
#[cfg(unix)]
use mln_gfx::DmaBufPlane;
use mln_gfx::command_encoder::CommandEncoder;
use mln_gfx::export::{
    DMA_BUF_MAX_PLANES, DRM_FORMAT_ABGR8888, DRM_FORMAT_ARGB8888, DRM_FORMAT_MOD_LINEAR, DmaBufChoice, Generations,
    choose_dma_buf_format,
};
use mln_gfx::frame::{self, FrameTarget, Handoff};
use mln_gfx::{ConsumerSync, DmaBufFormat, ExportKind, ExportKinds, ExportedImage, ExportedRing, SyncKind, SyncKinds};
use mln_util::error::Error;
use mln_util::logging::{Event, Log};
use mln_util::size::Size;

use crate::borrowed_target::{BorrowedImage, BorrowedImages, BorrowedTarget, ImageOwner};
use crate::device::Device;
use crate::renderer_backend::RendererBackend;

const DMA_BUF_USAGE: vk::ImageUsageFlags = vk::ImageUsageFlags::from_raw(
    vk::ImageUsageFlags::COLOR_ATTACHMENT.as_raw() | vk::ImageUsageFlags::SAMPLED.as_raw(),
);
const OPAQUE_FD_USAGE: vk::ImageUsageFlags = vk::ImageUsageFlags::from_raw(
    vk::ImageUsageFlags::COLOR_ATTACHMENT.as_raw()
        | vk::ImageUsageFlags::SAMPLED.as_raw()
        | vk::ImageUsageFlags::TRANSFER_SRC.as_raw(),
);
/// The format of an opaque file descriptor's image.
const OPAQUE_FD_FORMAT: vk::Format = vk::Format::R8G8B8A8_UNORM;
/// The usage of a hardware buffer's image: what
/// `AHARDWAREBUFFER_USAGE_GPU_COLOR_OUTPUT` and
/// `AHARDWAREBUFFER_USAGE_GPU_SAMPLED_IMAGE` allow.
#[cfg(target_os = "android")]
const HARDWARE_BUFFER_USAGE: vk::ImageUsageFlags = DMA_BUF_USAGE;
/// The format of a hardware buffer's image, which
/// `AHARDWAREBUFFER_FORMAT_R8G8B8A8_UNORM` is.
#[cfg(target_os = "android")]
const HARDWARE_BUFFER_FORMAT: vk::Format = vk::Format::R8G8B8A8_UNORM;
/// What a D3D11 texture's image renders and samples, as the texture binds as
/// a render target and a shader resource.
const DXGI_USAGE: vk::ImageUsageFlags = DMA_BUF_USAGE;
/// The format of a D3D11 texture's image, `DXGI_FORMAT_B8G8R8A8_UNORM`'s.
const DXGI_FORMAT: vk::Format = vk::Format::B8G8R8A8_UNORM;

fn dma_buf_vk_format(fourcc: u32) -> vk::Format {
    if fourcc == DRM_FORMAT_ARGB8888 { vk::Format::B8G8R8A8_UNORM } else { vk::Format::R8G8B8A8_UNORM }
}

/// What a ring's images are made with.
#[derive(Clone, Debug)]
enum Layout {
    /// A dmabuf of `fourcc`, with one of `modifiers`, each of which has the
    /// memory planes of the same index in `plane_counts`.
    DmaBuf {
        fourcc: u32,
        modifiers: Vec<u64>,
        plane_counts: Vec<u32>,
    },
    OpaqueFd,
    /// A hardware buffer with the extra `AHARDWAREBUFFER_USAGE_*` bits
    /// `usage`.
    #[cfg(target_os = "android")]
    AndroidHardwareBuffer {
        usage: u64,
    },
    /// A D3D11 texture, whose memory the image imports.
    Dxgi,
}

impl Layout {
    fn format(&self) -> vk::Format {
        match self {
            Layout::DmaBuf { fourcc, .. } => dma_buf_vk_format(*fourcc),
            Layout::OpaqueFd => OPAQUE_FD_FORMAT,
            #[cfg(target_os = "android")]
            Layout::AndroidHardwareBuffer { .. } => HARDWARE_BUFFER_FORMAT,
            Layout::Dxgi => DXGI_FORMAT,
        }
    }

    fn handle_type(&self) -> vk::ExternalMemoryHandleTypeFlags {
        match self {
            Layout::DmaBuf { .. } => vk::ExternalMemoryHandleTypeFlags::DMA_BUF_EXT,
            Layout::OpaqueFd => vk::ExternalMemoryHandleTypeFlags::OPAQUE_FD,
            #[cfg(target_os = "android")]
            Layout::AndroidHardwareBuffer { .. } => vk::ExternalMemoryHandleTypeFlags::ANDROID_HARDWARE_BUFFER_ANDROID,
            Layout::Dxgi => vk::ExternalMemoryHandleTypeFlags::D3D11_TEXTURE,
        }
    }

    fn usage(&self) -> vk::ImageUsageFlags {
        match self {
            Layout::DmaBuf { .. } => DMA_BUF_USAGE,
            Layout::OpaqueFd => OPAQUE_FD_USAGE,
            #[cfg(target_os = "android")]
            Layout::AndroidHardwareBuffer { .. } => HARDWARE_BUFFER_USAGE,
            Layout::Dxgi => DXGI_USAGE,
        }
    }
}

/// The modifiers of `format` that the device renders into and samples, in
/// at most as many memory planes as a dmabuf has.
fn device_modifiers(device: &Device, format: vk::Format) -> Vec<vk::DrmFormatModifierPropertiesEXT> {
    let instance = device.shared.instance();
    let mut list = vk::DrmFormatModifierPropertiesListEXT::default();
    let mut properties = vk::FormatProperties2::default().push_next(&mut list);
    // SAFETY: a plain query of the GPU, which is of Vulkan 1.1.
    unsafe {
        instance.get_physical_device_format_properties2(device.shared.physical_device(), format, &mut properties);
    };
    let count = list.drm_format_modifier_count as usize;
    let mut modifiers = vec![vk::DrmFormatModifierPropertiesEXT::default(); count];
    let mut list = vk::DrmFormatModifierPropertiesListEXT::default().drm_format_modifier_properties(&mut modifiers);
    let mut properties = vk::FormatProperties2::default().push_next(&mut list);
    // SAFETY: as above, into an array of the count it reported.
    unsafe {
        instance.get_physical_device_format_properties2(device.shared.physical_device(), format, &mut properties);
    };
    let written = list.drm_format_modifier_count as usize;
    modifiers.truncate(written);
    let features = vk::FormatFeatureFlags::COLOR_ATTACHMENT | vk::FormatFeatureFlags::SAMPLED_IMAGE;
    modifiers.retain(|modifier| {
        (1..=DMA_BUF_MAX_PLANES as u32).contains(&modifier.drm_format_modifier_plane_count)
            && modifier.drm_format_modifier_tiling_features.contains(features)
    });
    modifiers
}

/// Whether the device exports an image of `format` and `usage` in memory
/// of `handle_type`, with `modifier` for a dmabuf.
fn exports(
    device: &Device,
    format: vk::Format,
    usage: vk::ImageUsageFlags,
    handle_type: vk::ExternalMemoryHandleTypeFlags,
    modifier: Option<u64>,
) -> bool {
    shares(device, format, usage, handle_type, modifier, vk::ExternalMemoryFeatureFlags::EXPORTABLE)
}

/// Whether the device shares an image as [`exports`] says, in the way that
/// `feature` names: exporting its memory, or importing it.
fn shares(
    device: &Device,
    format: vk::Format,
    usage: vk::ImageUsageFlags,
    handle_type: vk::ExternalMemoryHandleTypeFlags,
    modifier: Option<u64>,
    feature: vk::ExternalMemoryFeatureFlags,
) -> bool {
    let mut modifier_info = vk::PhysicalDeviceImageDrmFormatModifierInfoEXT::default()
        .drm_format_modifier(modifier.unwrap_or(0))
        .sharing_mode(vk::SharingMode::EXCLUSIVE);
    let mut external_info = vk::PhysicalDeviceExternalImageFormatInfo::default().handle_type(handle_type);
    let mut info = vk::PhysicalDeviceImageFormatInfo2::default()
        .format(format)
        .ty(vk::ImageType::TYPE_2D)
        .tiling(if modifier.is_some() { vk::ImageTiling::DRM_FORMAT_MODIFIER_EXT } else { vk::ImageTiling::OPTIMAL })
        .usage(usage)
        .push_next(&mut external_info);
    if modifier.is_some() {
        info = info.push_next(&mut modifier_info);
    }
    let mut external = vk::ExternalImageFormatProperties::default();
    let mut properties = vk::ImageFormatProperties2::default().push_next(&mut external);
    // SAFETY: a plain query of the GPU, which is of Vulkan 1.1.
    let supported = unsafe {
        device.shared.instance().get_physical_device_image_format_properties2(
            device.shared.physical_device(),
            &info,
            &mut properties,
        )
    };
    supported.is_ok() && external.external_memory_properties.external_memory_features.contains(feature)
}

/// The layout of dmabufs of `choice`'s fourcc with its modifiers that the
/// device renders and exports.
fn dma_buf_layout(device: &Device, choice: DmaBufChoice) -> Layout {
    let supported = device_modifiers(device, dma_buf_vk_format(choice.fourcc));
    let plane_counts = choice
        .modifiers
        .iter()
        .map(|modifier| {
            supported
                .iter()
                .find(|properties| properties.drm_format_modifier == *modifier)
                .map_or(1, |properties| properties.drm_format_modifier_plane_count)
        })
        .collect();
    Layout::DmaBuf { fourcc: choice.fourcc, modifiers: choice.modifiers, plane_counts }
}

/// The host's formats of the dmabufs that the device renders and exports.
fn dma_buf_choice(device: &Device, formats: &[DmaBufFormat]) -> Option<DmaBufChoice> {
    if !device.features().dma_buf {
        return None;
    }
    choose_dma_buf_format(formats, |candidate| {
        let format = dma_buf_vk_format(candidate.fourcc);
        device_modifiers(device, format).iter().any(|modifier| modifier.drm_format_modifier == candidate.modifier)
            && exports(
                device,
                format,
                DMA_BUF_USAGE,
                vk::ExternalMemoryHandleTypeFlags::DMA_BUF_EXT,
                Some(candidate.modifier),
            )
    })
}

/// The kinds that `device` exports, found by exporting an image of each
/// that it says it exports.
pub(crate) fn probe(device: &Device) -> ExportKinds {
    let mut kinds = ExportKinds::NONE;
    let extent = vk::Extent2D { width: 1, height: 1 };
    if device.features().external_memory_fd
        && exports(device, OPAQUE_FD_FORMAT, OPAQUE_FD_USAGE, vk::ExternalMemoryHandleTypeFlags::OPAQUE_FD, None)
    {
        match ExportedMemory::new(device, extent, &Layout::OpaqueFd) {
            Ok(_) => kinds = kinds.with(ExportKind::OpaqueFd),
            Err(error) => Log::info(Event::Render, format!("The Vulkan device exports no opaque memory: {error}")),
        }
    }
    let modifiers = |fourcc| {
        device_modifiers(device, dma_buf_vk_format(fourcc))
            .into_iter()
            .map(|modifier| DmaBufFormat { fourcc, modifier: modifier.drm_format_modifier })
            .collect::<Vec<_>>()
    };
    if device.features().dma_buf
        && let Some(choice) = dma_buf_choice(device, &modifiers(DRM_FORMAT_ABGR8888))
    {
        match ExportedMemory::new(device, extent, &dma_buf_layout(device, choice)) {
            Ok(_) => kinds = kinds.with(ExportKind::DmaBuf),
            Err(error) => Log::info(Event::Render, format!("The Vulkan device exports no dmabufs: {error}")),
        }
    }
    #[cfg(target_os = "android")]
    if imports_hardware_buffers(device) {
        match ExportedMemory::new(device, extent, &Layout::AndroidHardwareBuffer { usage: 0 }) {
            Ok(_) => kinds = kinds.with(ExportKind::AndroidHardwareBuffer),
            Err(error) => Log::info(Event::Render, format!("The Vulkan device exports no hardware buffers: {error}")),
        }
    }
    let d3d11 = vk::ExternalMemoryHandleTypeFlags::D3D11_TEXTURE;
    if device.features().external_memory_win32
        && shares(device, DXGI_FORMAT, DXGI_USAGE, d3d11, None, vk::ExternalMemoryFeatureFlags::IMPORTABLE)
    {
        match ExportedMemory::new(device, extent, &Layout::Dxgi) {
            Ok(_) => kinds = kinds.with(ExportKind::DxgiSharedHandle),
            Err(error) => Log::info(Event::Render, format!("The Vulkan device shares no D3D11 textures: {error}")),
        }
    }
    kinds
}

/// Whether the device imports hardware buffers as the images of a ring.
#[cfg(target_os = "android")]
fn imports_hardware_buffers(device: &Device) -> bool {
    device.features().android_hardware_buffer
        && shares(
            device,
            HARDWARE_BUFFER_FORMAT,
            HARDWARE_BUFFER_USAGE,
            vk::ExternalMemoryHandleTypeFlags::ANDROID_HARDWARE_BUFFER_ANDROID,
            None,
            vk::ExternalMemoryFeatureFlags::IMPORTABLE,
        )
}

/// An image with memory of its own, exported to the host, and a view of it.
struct ExportedMemory {
    device: ash::Device,
    image: vk::Image,
    view: vk::ImageView,
    memory: vk::DeviceMemory,
    /// The exported file descriptor, which `export` names.
    #[cfg(unix)]
    _fd: Option<OwnedFd>,
    /// The imported hardware buffer, which `export` names. It goes after
    /// the memory, which `drop` frees.
    #[cfg(target_os = "android")]
    _buffer: Option<crate::android_hardware_buffer::HardwareBuffer>,
    /// The D3D11 texture whose memory the image imported, and whose NT
    /// handle `export` names. It goes after the image and its memory.
    #[cfg(windows)]
    _texture: Option<crate::win32::SharedTexture>,
    export: ExportedImage,
}

impl ExportedMemory {
    fn new(device: &Device, extent: vk::Extent2D, layout: &Layout) -> Result<ExportedMemory, Error> {
        fn native(what: &'static str) -> impl Fn(vk::Result) -> Error {
            move |e| Error::native(format!("{what} failed: {e}"))
        }
        let vk_device = device.shared.device().clone();
        let mut this = ExportedMemory {
            device: vk_device.clone(),
            image: vk::Image::null(),
            view: vk::ImageView::null(),
            memory: vk::DeviceMemory::null(),
            #[cfg(unix)]
            _fd: None,
            #[cfg(target_os = "android")]
            _buffer: None,
            #[cfg(windows)]
            _texture: None,
            export: ExportedImage::OpaqueFd { fd: -1, allocation_size: 0, dedicated: true, format: 0 },
        };
        let format = layout.format();
        let mut external = vk::ExternalMemoryImageCreateInfo::default().handle_types(layout.handle_type());
        let modifiers: &[u64] = if let Layout::DmaBuf { modifiers, .. } = layout { modifiers } else { &[] };
        let mut modifier_list = vk::ImageDrmFormatModifierListCreateInfoEXT::default().drm_format_modifiers(modifiers);
        let mut info = vk::ImageCreateInfo::default()
            .image_type(vk::ImageType::TYPE_2D)
            .format(format)
            .extent(vk::Extent3D { width: extent.width, height: extent.height, depth: 1 })
            .mip_levels(1)
            .array_layers(1)
            .samples(vk::SampleCountFlags::TYPE_1)
            .tiling(if matches!(layout, Layout::DmaBuf { .. }) {
                vk::ImageTiling::DRM_FORMAT_MODIFIER_EXT
            } else {
                vk::ImageTiling::OPTIMAL
            })
            .usage(layout.usage())
            .sharing_mode(vk::SharingMode::EXCLUSIVE)
            .initial_layout(vk::ImageLayout::UNDEFINED)
            .push_next(&mut external);
        if matches!(layout, Layout::DmaBuf { .. }) {
            info = info.push_next(&mut modifier_list);
        }
        // SAFETY: the create info is valid for the device, which exports
        // images of the layout (the caller checked).
        this.image = unsafe { vk_device.create_image(&info, None) }.map_err(native("Creating an exported image"))?;
        #[cfg(target_os = "android")]
        if let Layout::AndroidHardwareBuffer { usage } = layout {
            this.import_hardware_buffer(device, extent, *usage)?;
            this.create_view(format)?;
            return Ok(this);
        }

        let mut planes = Vec::new();
        let mut modifier = 0;
        if let Layout::DmaBuf { modifiers, plane_counts, .. } = layout {
            let loader = device
                .shared
                .drm_format_modifier()
                .ok_or_else(|| Error::unsupported("the Vulkan device has no DRM format modifiers"))?;
            let mut chosen = vk::ImageDrmFormatModifierPropertiesEXT::default();
            // SAFETY: the image is the device's, of DRM modifier tiling.
            unsafe { loader.get_image_drm_format_modifier_properties(this.image, &mut chosen) }
                .map_err(native("Reading the dmabuf's modifier"))?;
            modifier = chosen.drm_format_modifier;
            let index = modifiers
                .iter()
                .position(|candidate| *candidate == modifier)
                .ok_or_else(|| Error::native("Vulkan chose a modifier that the ring lacks"))?;
            let aspects = [
                vk::ImageAspectFlags::MEMORY_PLANE_0_EXT,
                vk::ImageAspectFlags::MEMORY_PLANE_1_EXT,
                vk::ImageAspectFlags::MEMORY_PLANE_2_EXT,
                vk::ImageAspectFlags::MEMORY_PLANE_3_EXT,
            ];
            for aspect in aspects.iter().take(plane_counts[index] as usize) {
                // SAFETY: the plane is one of the image's modifier's.
                let plane = unsafe {
                    vk_device.get_image_subresource_layout(
                        this.image,
                        vk::ImageSubresource { aspect_mask: *aspect, mip_level: 0, array_layer: 0 },
                    )
                };
                planes.push((plane.offset as u32, plane.row_pitch as u32));
            }
        }

        // SAFETY: the image is the device's.
        let requirements = unsafe { vk_device.get_image_memory_requirements(this.image) };
        if let Layout::Dxgi = layout {
            this.import_dxgi(device, extent, requirements)?;
        } else {
            // SAFETY: a plain query of the GPU.
            let types = unsafe {
                device.shared.instance().get_physical_device_memory_properties(device.shared.physical_device())
            };
            let linear = matches!(layout, Layout::DmaBuf { .. }) && modifier == DRM_FORMAT_MOD_LINEAR;
            let memory_type = memory_type(requirements.memory_type_bits, &types, linear)
                .ok_or_else(|| Error::native("No memory type takes the exported image"))?;
            // The image's memory is its own, so that the descriptor names it
            // alone.
            let mut dedicated = vk::MemoryDedicatedAllocateInfo::default().image(this.image);
            let mut export = vk::ExportMemoryAllocateInfo::default().handle_types(layout.handle_type());
            let allocate = vk::MemoryAllocateInfo::default()
                .allocation_size(requirements.size)
                .memory_type_index(memory_type)
                .push_next(&mut export)
                .push_next(&mut dedicated);
            // SAFETY: the allocate info is valid for the device.
            this.memory =
                unsafe { vk_device.allocate_memory(&allocate, None) }.map_err(native("Allocating exported memory"))?;
            // SAFETY: the memory was made for the image, which has none.
            unsafe { vk_device.bind_image_memory(this.image, this.memory, 0) }
                .map_err(native("Binding exported memory"))?;
            this.export_fd(device, layout, requirements.size, modifier, &planes)?;
        }
        this.create_view(format)?;
        Ok(this)
    }

    /// Makes the view of the image, which is bound to its memory.
    fn create_view(&mut self, format: vk::Format) -> Result<(), Error> {
        let view_info = vk::ImageViewCreateInfo::default()
            .image(self.image)
            .view_type(vk::ImageViewType::TYPE_2D)
            .format(format)
            .subresource_range(vk::ImageSubresourceRange {
                aspect_mask: vk::ImageAspectFlags::COLOR,
                base_mip_level: 0,
                level_count: 1,
                base_array_layer: 0,
                layer_count: 1,
            });
        // SAFETY: the image is bound to its memory.
        self.view = unsafe { self.device.create_image_view(&view_info, None) }
            .map_err(|e| Error::native(format!("Creating a view failed: {e}")))?;
        Ok(())
    }

    /// Allocates a hardware buffer of `extent` with the extra `usage` bits,
    /// and binds the image, made for one, to it.
    #[cfg(target_os = "android")]
    #[expect(clippy::used_underscore_binding, reason = "the image keeps the buffer only to release it")]
    fn import_hardware_buffer(&mut self, device: &Device, extent: vk::Extent2D, usage: u64) -> Result<(), Error> {
        use crate::android_hardware_buffer::HardwareBuffer;
        let loader = device
            .shared
            .android_hardware_buffer()
            .ok_or_else(|| Error::unsupported("the Vulkan device imports no hardware buffers"))?;
        let buffer = HardwareBuffer::allocate(extent.width, extent.height, usage)?;
        let mut properties = vk::AndroidHardwareBufferPropertiesANDROID::default();
        // SAFETY: the buffer is live, and the device imports hardware
        // buffers.
        unsafe { loader.get_android_hardware_buffer_properties(buffer.as_ptr(), &mut properties) }
            .map_err(|e| Error::native(format!("Reading the hardware buffer's properties failed: {e}")))?;
        // SAFETY: a plain query of the GPU.
        let types =
            unsafe { device.shared.instance().get_physical_device_memory_properties(device.shared.physical_device()) };
        let memory_type = memory_type(properties.memory_type_bits, &types, false)
            .ok_or_else(|| Error::native("No memory type takes the hardware buffer"))?;
        // A hardware buffer's image must have the buffer as its own memory.
        let mut import = vk::ImportAndroidHardwareBufferInfoANDROID::default().buffer(buffer.as_ptr());
        let mut dedicated = vk::MemoryDedicatedAllocateInfo::default().image(self.image);
        let allocate = vk::MemoryAllocateInfo::default()
            .allocation_size(properties.allocation_size)
            .memory_type_index(memory_type)
            .push_next(&mut import)
            .push_next(&mut dedicated);
        // SAFETY: the allocate info is valid for the device, and the buffer
        // live; the import takes a reference of its own.
        self.memory = unsafe { self.device.allocate_memory(&allocate, None) }
            .map_err(|e| Error::native(format!("Importing the hardware buffer failed: {e}")))?;
        // SAFETY: the memory was imported for the image, which has none.
        unsafe { self.device.bind_image_memory(self.image, self.memory, 0) }
            .map_err(|e| Error::native(format!("Binding the hardware buffer failed: {e}")))?;
        self.export = ExportedImage::AndroidHardwareBuffer { buffer: buffer.as_ptr() as u64 };
        self._buffer = Some(buffer);
        Ok(())
    }

    /// Exports the image's memory as a file descriptor, which the image
    /// keeps, and names it in `export`: a dmabuf of `modifier`, whose planes
    /// have the offsets and strides of `planes`, or opaque memory of
    /// `allocation_size` bytes.
    #[cfg(unix)]
    #[expect(clippy::used_underscore_binding, reason = "the image keeps the descriptor only to close it")]
    fn export_fd(
        &mut self,
        device: &Device,
        layout: &Layout,
        allocation_size: u64,
        modifier: u64,
        planes: &[(u32, u32)],
    ) -> Result<(), Error> {
        let loader =
            device.shared.memory_fd().ok_or_else(|| Error::unsupported("the Vulkan device exports no memory"))?;
        let get = vk::MemoryGetFdInfoKHR::default().memory(self.memory).handle_type(layout.handle_type());
        // SAFETY: the memory was allocated for export as the handle type.
        let fd = unsafe { loader.get_memory_fd(&get) }
            .map_err(|e| Error::native(format!("Exporting the image's memory failed: {e}")))?;
        // SAFETY: the export made the descriptor, which is the image's.
        let fd = unsafe { OwnedFd::from_raw_fd(fd) };
        let raw = fd.as_raw_fd();
        self._fd = Some(fd);
        self.export = if let Layout::DmaBuf { fourcc, .. } = layout {
            ExportedImage::DmaBuf {
                fourcc: *fourcc,
                modifier,
                planes: planes.iter().map(|&(offset, stride)| DmaBufPlane { fd: raw, offset, stride }).collect(),
            }
        } else {
            ExportedImage::OpaqueFd {
                fd: raw,
                allocation_size,
                dedicated: true,
                format: layout.format().as_raw() as u32,
            }
        };
        Ok(())
    }

    #[cfg(not(unix))]
    #[expect(clippy::unused_self, reason = "the stand-in for a method of another platform")]
    fn export_fd(&mut self, _: &Device, _: &Layout, _: u64, _: u64, _: &[(u32, u32)]) -> Result<(), Error> {
        Err(Error::unsupported("this platform shares no memory as file descriptors"))
    }

    /// Binds the image to the memory of a D3D11 texture of `extent` that the
    /// device's D3D11 device makes, which the image keeps, and names the
    /// texture's NT handle in `export`. `requirements` are the image's.
    #[cfg(windows)]
    fn import_dxgi(
        &mut self,
        device: &Device,
        extent: vk::Extent2D,
        requirements: vk::MemoryRequirements,
    ) -> Result<(), Error> {
        let handle_type = vk::ExternalMemoryHandleTypeFlags::D3D11_TEXTURE;
        let loader = device
            .shared
            .memory_win32()
            .ok_or_else(|| Error::unsupported("the Vulkan device imports no D3D11 textures"))?;
        let texture = device.shared.dxgi()?.texture(extent.width, extent.height)?;
        let mut properties = vk::MemoryWin32HandlePropertiesKHR::default();
        // SAFETY: the texture's live NT handle, of a handle type that isn't
        // opaque.
        unsafe { loader.get_memory_win32_handle_properties(handle_type, texture.handle(), &mut properties) }
            .map_err(|e| Error::native(format!("Reading the D3D11 texture's memory types failed: {e}")))?;
        // SAFETY: a plain query of the GPU.
        let types =
            unsafe { device.shared.instance().get_physical_device_memory_properties(device.shared.physical_device()) };
        let memory_type = memory_type(requirements.memory_type_bits & properties.memory_type_bits, &types, false)
            .ok_or_else(|| Error::native("No memory type imports the D3D11 texture"))?;
        // Vulkan imports a D3D11 texture only into memory of the image's own.
        let mut dedicated = vk::MemoryDedicatedAllocateInfo::default().image(self.image);
        let mut import =
            vk::ImportMemoryWin32HandleInfoKHR::default().handle_type(handle_type).handle(texture.handle());
        let allocate = vk::MemoryAllocateInfo::default()
            .allocation_size(requirements.size)
            .memory_type_index(memory_type)
            .push_next(&mut import)
            .push_next(&mut dedicated);
        // SAFETY: the allocate info is valid for the device, and an import of
        // an NT handle leaves the handle to the texture.
        self.memory = unsafe { self.device.allocate_memory(&allocate, None) }
            .map_err(|e| Error::native(format!("Importing the D3D11 texture's memory failed: {e}")))?;
        // SAFETY: the memory was imported for the image, which has none.
        unsafe { self.device.bind_image_memory(self.image, self.memory, 0) }
            .map_err(|e| Error::native(format!("Binding the D3D11 texture's memory failed: {e}")))?;
        self.export =
            ExportedImage::DxgiSharedTexture { handle: texture.handle() as u64, format: crate::win32::FORMAT };
        self._texture = Some(texture);
        Ok(())
    }

    #[cfg(not(windows))]
    #[expect(clippy::unused_self, reason = "the stand-in for a method of another platform")]
    fn import_dxgi(&mut self, _: &Device, _: vk::Extent2D, _: vk::MemoryRequirements) -> Result<(), Error> {
        Err(Error::unsupported("this platform has no D3D11 textures"))
    }
}

impl Drop for ExportedMemory {
    fn drop(&mut self) {
        // SAFETY: the objects are this image's, and its generation's target
        // waited for the frames that drew into it.
        unsafe {
            if self.view != vk::ImageView::null() {
                self.device.destroy_image_view(self.view, None);
            }
            if self.image != vk::Image::null() {
                self.device.destroy_image(self.image, None);
            }
            if self.memory != vk::DeviceMemory::null() {
                self.device.free_memory(self.memory, None);
            }
        }
    }
}

/// The memory type that `bits` allows for an exported image: local to the
/// device, and for a linear dmabuf one the CPU maps too, so that a reader
/// can map it.
fn memory_type(bits: u32, types: &vk::PhysicalDeviceMemoryProperties, linear: bool) -> Option<u32> {
    let local = vk::MemoryPropertyFlags::DEVICE_LOCAL;
    let mappable = vk::MemoryPropertyFlags::HOST_VISIBLE;
    let preferred: &[vk::MemoryPropertyFlags] = if linear { &[local | mappable, mappable, local] } else { &[local] };
    let allowed = |index: u32| bits & (1 << index) != 0;
    preferred
        .iter()
        .find_map(|flags| {
            (0..types.memory_type_count)
                .find(|&index| allowed(index) && types.memory_types[index as usize].property_flags.contains(*flags))
        })
        .or_else(|| (0..types.memory_type_count).find(|&index| allowed(index)))
}

/// The kind of `ring` that `device` shares its images in: the first of the
/// host's kinds, by [`ExportKind::BY_PREFERENCE`], that the device exports,
/// for a dmabuf one of the host's formats, and for a hardware buffer one
/// that Android allocates with the host's usage bits.
///
/// Fails with [`ErrorKind::Unsupported`](mln_util::ErrorKind) when the
/// device exports none of them.
pub fn plan(device: &Device, ring: &ExportedRing) -> Result<ExportKind, Error> {
    let exported = device.export_kinds().intersection(ring.kinds);
    exported
        .iter()
        .find(|kind| match kind {
            ExportKind::DmaBuf => dma_buf_choice(device, &ring.dma_buf_formats).is_some(),
            // Android may refuse the host's usage bits.
            #[cfg(target_os = "android")]
            ExportKind::AndroidHardwareBuffer => {
                crate::android_hardware_buffer::HardwareBuffer::allocate(1, 1, ring.android_hardware_buffer_usage)
                    .is_ok()
            }
            _ => true,
        })
        .ok_or_else(|| Error::unsupported("the Vulkan device exports none of the ring's kinds of image"))
}

/// The kinds of sync with which the host waits for an exported image's
/// frame (the producer's), and the target for the host's reads (the
/// consumer's): a finished frame, or the sync that crosses APIs on the
/// platform: a sync file, or on Windows, a D3D12 fence.
#[must_use]
pub fn sync_kinds(device: &Device) -> (SyncKinds, SyncKinds) {
    let (producer, consumer) = device.sync_kinds();
    let crossing = SyncKinds::of(&[SyncKind::CpuComplete, SyncKind::SyncFd, SyncKind::D3d12Fence]);
    let keep = |kinds: SyncKinds| {
        crossing.iter().filter(|kind| kinds.contains(*kind)).fold(SyncKinds::NONE, mln_gfx::SyncKinds::with)
    };
    (keep(producer), keep(consumer))
}

/// One generation of a ring's images, of one size, and the borrowed ring
/// target that draws into them.
struct Generation {
    // Before the images: dropping it waits for the frames that drew into
    // them.
    target: BorrowedTarget,
    images: Vec<ExportedMemory>,
    size: Size,
}

impl Generation {
    fn new(
        backend: &Rc<RendererBackend>,
        layout: &Layout,
        depth: usize,
        size: Size,
        producer: SyncKind,
    ) -> Result<Generation, Error> {
        let device = backend.backend_impl().device();
        let extent = vk::Extent2D { width: size.width, height: size.height };
        let images = (0..depth).map(|_| ExportedMemory::new(device, extent, layout)).collect::<Result<Vec<_>, _>>()?;
        // A dmabuf's or hardware buffer's readers are outside the device,
        // for which the foreign queue family stands; another API of the
        // driver is external. A device that imports hardware buffers has the
        // foreign queue family.
        let owner = match layout {
            Layout::DmaBuf { .. } if device.features().foreign_queue_family => ImageOwner::Foreign,
            #[cfg(target_os = "android")]
            Layout::AndroidHardwareBuffer { .. } => ImageOwner::Foreign,
            _ => ImageOwner::External,
        };
        let borrowed = BorrowedImages {
            images: images.iter().map(|image| BorrowedImage { image: image.image, view: image.view }).collect(),
            size,
            format: layout.format(),
            final_layout: vk::ImageLayout::GENERAL,
            owner,
        };
        let target = BorrowedTarget::new(backend.clone(), borrowed, producer)?;
        Ok(Generation { target, images, size })
    }
}

/// Frames drawn into images of the target's own, which it exports, and
/// queued for the host to acquire.
pub struct ExportedTarget {
    backend: Rc<RendererBackend>,
    kind: ExportKind,
    layout: Layout,
    depth: usize,
    producer: SyncKind,
    generations: RefCell<Generations<Generation>>,
}

impl ExportedTarget {
    /// A target on `backend` that draws into `ring.depth` images of `size`
    /// pixels, exported as `kind`, which [`plan`] picked, and hands each
    /// frame over with sync of the kind `producer`, one of
    /// [`sync_kinds`]'s.
    ///
    /// Fails with [`ErrorKind::Unsupported`](mln_util::ErrorKind) when the
    /// device can't export `kind` or `producer`, and with
    /// [`ErrorKind::Native`](mln_util::ErrorKind) when a Vulkan call fails.
    pub fn new(
        backend: Rc<RendererBackend>,
        ring: &ExportedRing,
        kind: ExportKind,
        size: Size,
        producer: SyncKind,
    ) -> Result<ExportedTarget, Error> {
        let device = backend.backend_impl().device().clone();
        if !device.export_kinds().contains(kind) {
            return Err(Error::unsupported(format!("the Vulkan device exports no {kind:?}")));
        }
        if !sync_kinds(&device).0.contains(producer) {
            return Err(Error::unsupported(format!("an exported image hands no frame over with {producer:?}")));
        }
        let layout = match kind {
            ExportKind::DmaBuf => dma_buf_layout(
                &device,
                dma_buf_choice(&device, &ring.dma_buf_formats)
                    .ok_or_else(|| Error::unsupported("the Vulkan device exports a dmabuf of none of the formats"))?,
            ),
            ExportKind::OpaqueFd => Layout::OpaqueFd,
            #[cfg(target_os = "android")]
            ExportKind::AndroidHardwareBuffer => {
                Layout::AndroidHardwareBuffer { usage: ring.android_hardware_buffer_usage }
            }
            ExportKind::DxgiSharedHandle => Layout::Dxgi,
            _ => return Err(Error::unsupported(format!("the Vulkan device exports no {kind:?}"))),
        };
        let current = Generation::new(&backend, &layout, ring.depth, size, producer)?;
        Ok(ExportedTarget {
            backend,
            kind,
            layout,
            depth: ring.depth,
            producer,
            generations: RefCell::new(Generations::new(current)),
        })
    }

    /// How the target shares its images.
    pub fn kind(&self) -> ExportKind {
        self.kind
    }

    /// Has the next frame draw into the image at `slot`.
    pub fn select(&self, slot: usize) {
        self.generations.borrow().current().target.select(slot);
    }

    /// The host gave the frame at `slot` back, once its reads reach
    /// `consumer`. A frame of an older generation lets go of its image, which
    /// no frame draws into again.
    ///
    /// Fails as [`BorrowedTarget::release`] does.
    pub fn release(&self, slot: usize, consumer: ConsumerSync) -> Result<(), Error> {
        let mut generations = self.generations.borrow_mut();
        if generations.give_back(slot) {
            return Ok(());
        }
        generations.current().target.release(slot, consumer)
    }

    /// The host gave the frame at `slot` up without saying when its reads
    /// end.
    pub fn discard(&self, slot: usize) {
        let mut generations = self.generations.borrow_mut();
        if !generations.give_back(slot) {
            generations.current().target.discard(slot);
        }
    }

    /// Draws into images of `size` pixels from now on. The frames that the
    /// host holds, at `held` slots, keep their images. Returns whether the
    /// ring has new images, which it doesn't when they have `size` already.
    ///
    /// Fails with [`ErrorKind::Native`](mln_util::ErrorKind) when Vulkan
    /// can't make the images, and the target then keeps its own.
    pub fn resize(&self, size: Size, held: &[usize]) -> Result<bool, Error> {
        if self.size() == size {
            return Ok(false);
        }
        let next = Generation::new(&self.backend, &self.layout, self.depth, size, self.producer)?;
        self.generations.borrow_mut().replace(next, held);
        Ok(true)
    }

    /// The size of the images that frames draw into.
    pub fn size(&self) -> Size {
        self.generations.borrow().current().size
    }

    /// The image of the frame at `slot`, and its size: of the generation
    /// that the host holds it in.
    pub fn image(&self, slot: usize) -> Option<(Size, ExportedImage)> {
        let generations = self.generations.borrow();
        let generation = generations.retired(slot).unwrap_or(generations.current());
        generation.images.get(slot).map(|image| (generation.size, image.export.clone()))
    }
}

impl FrameTarget for ExportedTarget {
    fn begin_frame(&self) -> Option<Rc<dyn frame::DrawTarget>> {
        self.generations.borrow().current().target.begin_frame()
    }

    fn end_frame(&self, encoder: &dyn CommandEncoder) -> Handoff {
        match self.generations.borrow().current().target.end_frame(encoder) {
            Handoff::GivenBack { slot, sync } => Handoff::Queued { slot, sync },
            other => other,
        }
    }
}

impl Drop for ExportedTarget {
    fn drop(&mut self) {
        self.generations.get_mut().clear_retired();
    }
}
