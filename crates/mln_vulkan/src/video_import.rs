// Video frames in dmabufs as sampled images. Each plane becomes an image of
// a one- or two-channel format over the dmabuf's memory, laid out by the
// frame's DRM format modifier (VK_EXT_image_drm_format_modifier), as wlroots
// imports its clients' buffers: acquired from the foreign queue family that
// the decoder's work belongs to, from the GENERAL layout that a foreign
// queue leaves it in, which keeps what the decoder wrote. The decoder's
// implicit fence comes out of the dmabuf as a sync file, which the next
// submit waits for.

use std::os::fd::{AsRawFd as _, IntoRawFd as _, OwnedFd};

use ash::vk;
use mln_gfx::gpu::{GpuError, TextureFormat};
use mln_gfx::video::{DmaBufPlane, PlaneLayout};
use mln_util::size::Size;

use crate::device::Device;

const fn fourcc(code: [u8; 4]) -> u32 {
    u32::from_le_bytes(code)
}

const DRM_FORMAT_NV12: u32 = fourcc(*b"NV12");
const DRM_FORMAT_ARGB8888: u32 = fourcc(*b"AR24");
const DRM_FORMAT_XRGB8888: u32 = fourcc(*b"XR24");
const DRM_FORMAT_ABGR8888: u32 = fourcc(*b"AB24");
const DRM_FORMAT_XBGR8888: u32 = fourcc(*b"XB24");
/// `DRM_FORMAT_MOD_INVALID`: a layout only the driver that made the memory
/// knows, which Vulkan can't import.
const DRM_FORMAT_MOD_INVALID: u64 = 0x00ff_ffff_ffff_ffff;

/// A plane's format, as the backend names it and as Vulkan does, and the
/// divisor of its size.
type PlaneFormat = (TextureFormat, vk::Format, u32);

/// How a frame of `fourcc` lays out its planes, and each plane's format.
fn plane_formats(fourcc: u32) -> Option<(PlaneLayout, Vec<PlaneFormat>)> {
    Some(match fourcc {
        DRM_FORMAT_NV12 => (
            PlaneLayout::Nv12,
            vec![
                (TextureFormat::R8Unorm, vk::Format::R8_UNORM, 1),
                (TextureFormat::Rg8Unorm, vk::Format::R8G8_UNORM, 2),
            ],
        ),
        // The DRM formats name their channels from the most significant
        // byte: ARGB8888 has blue in the first byte, as B8G8R8A8 does.
        DRM_FORMAT_ARGB8888 | DRM_FORMAT_XRGB8888 => {
            (PlaneLayout::Rgba, vec![(TextureFormat::Bgra8Unorm, vk::Format::B8G8R8A8_UNORM, 1)])
        }
        DRM_FORMAT_ABGR8888 | DRM_FORMAT_XBGR8888 => {
            (PlaneLayout::Rgba, vec![(TextureFormat::Rgba8Unorm, vk::Format::R8G8B8A8_UNORM, 1)])
        }
        _ => return None,
    })
}

/// One plane of a dmabuf as an image, its memory and its view, which the
/// owner destroys once the GPU no longer reads it.
pub(crate) struct Plane {
    pub(crate) image: vk::Image,
    pub(crate) memory: vk::DeviceMemory,
    pub(crate) view: vk::ImageView,
    pub(crate) format: TextureFormat,
    pub(crate) vk_format: vk::Format,
    pub(crate) size: Size,
}

impl Plane {
    /// Destroys the plane's objects.
    ///
    /// # Safety
    ///
    /// No work that uses them is pending.
    pub(crate) unsafe fn destroy(&self, device: &ash::Device) {
        // SAFETY: the caller's contract; null handles are skipped.
        unsafe {
            if self.view != vk::ImageView::null() {
                device.destroy_image_view(self.view, None);
            }
            if self.image != vk::Image::null() {
                device.destroy_image(self.image, None);
            }
            if self.memory != vk::DeviceMemory::null() {
                device.free_memory(self.memory, None);
            }
        }
    }
}

/// Imports the planes of a dmabuf frame of `fourcc` with `modifier`, of
/// `size` pixels. Fails with [`GpuError::Unsupported`] for a device without
/// dmabuf imports, a format or modifier it doesn't sample, or an implicit
/// layout.
pub(crate) fn import(
    device: &Device,
    fourcc: u32,
    modifier: u64,
    size: Size,
    planes: &[DmaBufPlane],
) -> Result<(PlaneLayout, Vec<Plane>), GpuError> {
    let features = device.features();
    let memory_fd = device.shared.memory_fd().filter(|_| features.dma_buf);
    let Some(memory_fd) = memory_fd else {
        return Err(GpuError::Unsupported("dmabufs on this Vulkan device".to_owned()));
    };
    if modifier == DRM_FORMAT_MOD_INVALID {
        return Err(GpuError::Unsupported("dmabufs of an implicit layout on Vulkan".to_owned()));
    }
    let (layout, formats) =
        plane_formats(fourcc).ok_or_else(|| GpuError::Unsupported(format!("dmabufs of the fourcc {fourcc:#x}")))?;
    if planes.len() < formats.len() {
        return Err(GpuError::Invalid(format!("a dmabuf of {} planes for {fourcc:#x}", planes.len())));
    }
    let mut imported = Vec::with_capacity(formats.len());
    for (plane, &(format, vk_format, divisor)) in planes.iter().zip(&formats) {
        let plane_size = Size::new(size.width.div_ceil(divisor), size.height.div_ceil(divisor));
        match import_plane(device, memory_fd, plane, vk_format, modifier, plane_size) {
            Ok((image, memory, view)) => {
                imported.push(Plane { image, memory, view, format, vk_format, size: plane_size });
            }
            Err(error) => {
                for plane in &imported {
                    // SAFETY: no work uses the planes yet.
                    unsafe { plane.destroy(device.shared.device()) };
                }
                return Err(error);
            }
        }
    }
    Ok((layout, imported))
}

fn import_plane(
    device: &Device,
    memory_fd: &ash::khr::external_memory_fd::Device,
    plane: &DmaBufPlane,
    format: vk::Format,
    modifier: u64,
    size: Size,
) -> Result<(vk::Image, vk::DeviceMemory, vk::ImageView), GpuError> {
    let shared = &device.shared;
    let (instance, vk_device) = (shared.instance(), shared.device());
    let unsupported = |what: &str| {
        GpuError::Unsupported(format!("a {format:?} dmabuf plane with the modifier {modifier:#x}: {what}"))
    };
    let usage = vk::ImageUsageFlags::SAMPLED;
    let handle = vk::ExternalMemoryHandleTypeFlags::DMA_BUF_EXT;

    // Whether the device samples the format with the modifier, imported.
    let mut modifier_info = vk::PhysicalDeviceImageDrmFormatModifierInfoEXT::default()
        .drm_format_modifier(modifier)
        .sharing_mode(vk::SharingMode::EXCLUSIVE);
    let mut external_info = vk::PhysicalDeviceExternalImageFormatInfo::default().handle_type(handle);
    let format_info = vk::PhysicalDeviceImageFormatInfo2::default()
        .format(format)
        .ty(vk::ImageType::TYPE_2D)
        .tiling(vk::ImageTiling::DRM_FORMAT_MODIFIER_EXT)
        .usage(usage)
        .push_next(&mut external_info)
        .push_next(&mut modifier_info);
    let mut external = vk::ExternalImageFormatProperties::default();
    let mut properties = vk::ImageFormatProperties2::default().push_next(&mut external);
    // SAFETY: a plain query of the GPU with valid structures.
    unsafe {
        instance.get_physical_device_image_format_properties2(shared.physical_device(), &format_info, &mut properties)
    }
    .map_err(|_| unsupported("the device doesn't sample it"))?;
    if !external
        .external_memory_properties
        .external_memory_features
        .contains(vk::ExternalMemoryFeatureFlags::IMPORTABLE)
    {
        return Err(unsupported("the device doesn't import it"));
    }

    let layouts = [vk::SubresourceLayout {
        offset: u64::from(plane.offset),
        size: 0,
        row_pitch: u64::from(plane.stride),
        array_pitch: 0,
        depth_pitch: 0,
    }];
    let mut explicit = vk::ImageDrmFormatModifierExplicitCreateInfoEXT::default()
        .drm_format_modifier(modifier)
        .plane_layouts(&layouts);
    let mut external_create = vk::ExternalMemoryImageCreateInfo::default().handle_types(handle);
    let image_info = vk::ImageCreateInfo::default()
        .image_type(vk::ImageType::TYPE_2D)
        .format(format)
        .extent(vk::Extent3D { width: size.width, height: size.height, depth: 1 })
        .mip_levels(1)
        .array_layers(1)
        .samples(vk::SampleCountFlags::TYPE_1)
        .tiling(vk::ImageTiling::DRM_FORMAT_MODIFIER_EXT)
        .usage(usage)
        .sharing_mode(vk::SharingMode::EXCLUSIVE)
        .initial_layout(vk::ImageLayout::UNDEFINED)
        .push_next(&mut external_create)
        .push_next(&mut explicit);
    // SAFETY: the create info is valid for the device, which samples the
    // format with the modifier.
    let image = unsafe { vk_device.create_image(&image_info, None) }.map_err(|e| unsupported(&e.to_string()))?;
    let result = bind(device, memory_fd, plane, image, format);
    match result {
        Ok((memory, view)) => Ok((image, memory, view)),
        Err(error) => {
            // SAFETY: the image is new, and nothing uses it.
            unsafe { vk_device.destroy_image(image, None) };
            Err(error)
        }
    }
}

/// Imports the dmabuf's memory for `image`, binds it, and makes the image's
/// view.
fn bind(
    device: &Device,
    memory_fd: &ash::khr::external_memory_fd::Device,
    plane: &DmaBufPlane,
    image: vk::Image,
    format: vk::Format,
) -> Result<(vk::DeviceMemory, vk::ImageView), GpuError> {
    let shared = &device.shared;
    let vk_device = shared.device();
    let handle = vk::ExternalMemoryHandleTypeFlags::DMA_BUF_EXT;
    let failed = |what: &str| GpuError::Unsupported(format!("importing a dmabuf plane's memory: {what}"));
    let fd = plane.fd.try_clone().map_err(|e| failed(&e.to_string()))?;
    let mut fd_properties = vk::MemoryFdPropertiesKHR::default();
    // SAFETY: the descriptor is an open dmabuf.
    unsafe { memory_fd.get_memory_fd_properties(handle, fd.as_raw_fd(), &mut fd_properties) }
        .map_err(|e| failed(&e.to_string()))?;
    // SAFETY: the image is the device's.
    let requirements = unsafe { vk_device.get_image_memory_requirements(image) };
    let types = requirements.memory_type_bits & fd_properties.memory_type_bits;
    let type_index = (0..32).find(|index| types & (1 << index) != 0).ok_or_else(|| failed("no memory type"))?;
    // The import takes the descriptor when it succeeds.
    let mut import = vk::ImportMemoryFdInfoKHR::default().handle_type(handle).fd(fd.as_raw_fd());
    let mut dedicated = vk::MemoryDedicatedAllocateInfo::default().image(image);
    let allocate = vk::MemoryAllocateInfo::default()
        .allocation_size(requirements.size)
        .memory_type_index(type_index)
        .push_next(&mut import)
        .push_next(&mut dedicated);
    // SAFETY: the info imports an open dmabuf for the image alone.
    let memory = unsafe { vk_device.allocate_memory(&allocate, None) }.map_err(|e| failed(&e.to_string()))?;
    let _ = fd.into_raw_fd();
    // SAFETY: the memory was imported for the image, which has none.
    if let Err(error) = unsafe { vk_device.bind_image_memory(image, memory, 0) } {
        // SAFETY: the memory is new, and bound to nothing.
        unsafe { vk_device.free_memory(memory, None) };
        return Err(failed(&error.to_string()));
    }
    let view_info = vk::ImageViewCreateInfo::default()
        .image(image)
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
    match unsafe { vk_device.create_image_view(&view_info, None) } {
        Ok(view) => Ok((memory, view)),
        Err(error) => {
            // SAFETY: the memory is bound to the image alone, which nothing
            // uses; the caller destroys the image.
            unsafe { vk_device.free_memory(memory, None) };
            Err(failed(&error.to_string()))
        }
    }
}

/// The fences of the work that writes `plane`'s dmabuf, as a sync file, or
/// `None` where the kernel can't export them (before Linux 6.0).
#[cfg(target_os = "linux")]
pub(crate) fn writes_sync_file(plane: &DmaBufPlane) -> Option<OwnedFd> {
    use std::os::fd::FromRawFd as _;

    #[repr(C)]
    struct ExportSyncFile {
        flags: u32,
        fd: i32,
    }
    // _IOWR('b', 2, struct dma_buf_export_sync_file).
    const DMA_BUF_IOCTL_EXPORT_SYNC_FILE: libc::c_ulong = 0xc008_6202;
    // The fences a reader waits for: the writers'.
    const DMA_BUF_SYNC_READ: u32 = 1;
    let mut export = ExportSyncFile { flags: DMA_BUF_SYNC_READ, fd: -1 };
    // SAFETY: the ioctl reads and writes a dma_buf_export_sync_file.
    #[cfg_attr(
        target_env = "gnu",
        expect(
            trivial_numeric_casts,
            reason = "glibc's ioctl takes a `c_ulong` request, and other C libraries' a `c_int`"
        )
    )]
    let result = unsafe { libc::ioctl(plane.fd.as_raw_fd(), DMA_BUF_IOCTL_EXPORT_SYNC_FILE as _, &mut export) };
    // SAFETY: the ioctl made the descriptor.
    (result == 0 && export.fd >= 0).then(|| unsafe { OwnedFd::from_raw_fd(export.fd) })
}

#[cfg(not(target_os = "linux"))]
pub(crate) fn writes_sync_file(_plane: &DmaBufPlane) -> Option<OwnedFd> {
    None
}
