//! Buffers and images with their memory from gpu-allocator, which keeps
//! host-visible memory mapped. Their owner destroys each with the device
//! and the allocator that made it, once the commands that use it have
//! finished.

use ash::vk;
use gpu_allocator::vulkan::{Allocation, AllocationCreateDesc, AllocationScheme, Allocator, AllocatorCreateDesc};
use gpu_allocator::{AllocationSizes, AllocatorDebugSettings, MemoryLocation};
use mln_util::error::Error;

use crate::device::Device;

/// An allocator of `device`'s memory.
///
/// Fails with [`ErrorKind::Native`](mln_util::ErrorKind) when
/// gpu-allocator can't make one.
pub(crate) fn allocator(device: &Device) -> Result<Allocator, Error> {
    const MB: u64 = 1024 * 1024;
    let shared = &device.shared;
    Allocator::new(&AllocatorCreateDesc {
        instance: shared.instance().clone(),
        device: shared.device().clone(),
        physical_device: shared.physical_device(),
        debug_settings: AllocatorDebugSettings::default(),
        buffer_device_address: false,
        allocation_sizes: AllocationSizes::new(16 * MB, 8 * MB)
            .with_max_device_memblock_size(256 * MB)
            .with_max_host_memblock_size(64 * MB),
    })
    .map_err(|e| Error::native(format!("Vulkan allocator init failed: {e}")))
}

/// A buffer and its memory, which [`destroy`](Self::destroy) frees: in
/// debug builds, dropping one that isn't destroyed panics.
pub(crate) struct BufferAllocation {
    buffer: vk::Buffer,
    allocation: Allocation,
    mapped_buffer: *mut u8,
}

impl BufferAllocation {
    /// Creates a buffer of `size` bytes in device memory
    /// (`MemoryLocation::GpuOnly`) or in mapped memory that the CPU writes
    /// (`CpuToGpu`) or reads (`GpuToCpu`). Fails if the CPU's memory isn't
    /// mapped.
    pub(crate) fn create(
        device: &ash::Device,
        allocator: &mut Allocator,
        size: usize,
        usage: vk::BufferUsageFlags,
        location: MemoryLocation,
        name: &str,
    ) -> Option<BufferAllocation> {
        let buffer_info = vk::BufferCreateInfo::default()
            .size(size.max(1) as vk::DeviceSize)
            .usage(usage)
            .sharing_mode(vk::SharingMode::EXCLUSIVE);
        // SAFETY: the create info is valid for the device.
        let buffer = unsafe { device.create_buffer(&buffer_info, None) }.ok()?;
        // SAFETY: `buffer` is a buffer of the device.
        let requirements = unsafe { device.get_buffer_memory_requirements(buffer) };
        let allocation = allocator.allocate(&AllocationCreateDesc {
            name,
            requirements,
            location,
            linear: true,
            allocation_scheme: AllocationScheme::GpuAllocatorManaged,
        });
        let allocation = match allocation {
            // SAFETY: the memory was allocated for the buffer's requirements,
            // and neither is bound yet.
            Ok(allocation)
                if unsafe { device.bind_buffer_memory(buffer, allocation.memory(), allocation.offset()).is_ok() } =>
            {
                allocation
            }
            result => {
                if let Ok(allocation) = result {
                    let _ = allocator.free(allocation);
                }
                // SAFETY: nothing uses the buffer yet.
                unsafe { device.destroy_buffer(buffer, None) };
                return None;
            }
        };
        let mapped_buffer = allocation.mapped_ptr().map_or(std::ptr::null_mut(), |p| p.as_ptr().cast::<u8>());
        let allocation = BufferAllocation { buffer, allocation, mapped_buffer };
        if mapped_buffer.is_null() && location != MemoryLocation::GpuOnly {
            allocation.destroy(device, allocator);
            return None;
        }
        Some(allocation)
    }

    /// Destroys the buffer and frees its memory, which no command that
    /// hasn't finished uses.
    pub(crate) fn destroy(mut self, device: &ash::Device, allocator: &mut Allocator) {
        // SAFETY: the commands that use the buffer have finished.
        unsafe { device.destroy_buffer(self.buffer, None) };
        let allocation = std::mem::take(&mut self.allocation);
        let _ = allocator.free(allocation);
    }

    #[must_use]
    pub(crate) fn buffer(&self) -> vk::Buffer {
        self.buffer
    }

    /// The buffer's memory, or null if it is in device memory.
    #[must_use]
    pub(crate) fn mapped_buffer(&self) -> *mut u8 {
        self.mapped_buffer
    }
}

/// An image, its memory and its views, which [`destroy`](Self::destroy)
/// frees: in debug builds, dropping one that isn't destroyed panics.
pub(crate) struct ImageAllocation {
    image: vk::Image,
    allocation: Allocation,
    image_view: vk::ImageView,
    /// Of an image with mips, a view of level 0, which framebuffers take.
    level_view: vk::ImageView,
}

impl ImageAllocation {
    /// Creates an image in device memory (`MemoryLocation::GpuOnly`) or in
    /// memory the CPU reads (`GpuToCpu`).
    pub(crate) fn create(
        device: &ash::Device,
        allocator: &mut Allocator,
        image_info: &vk::ImageCreateInfo<'_>,
        location: MemoryLocation,
        name: &str,
    ) -> Option<ImageAllocation> {
        // SAFETY: the create info is valid for the device.
        let image = unsafe { device.create_image(image_info, None) }.ok()?;
        // SAFETY: `image` is an image of the device.
        let requirements = unsafe { device.get_image_memory_requirements(image) };
        let allocation = allocator.allocate(&AllocationCreateDesc {
            name,
            requirements,
            location,
            linear: image_info.tiling == vk::ImageTiling::LINEAR,
            allocation_scheme: AllocationScheme::GpuAllocatorManaged,
        });
        let allocation = match allocation {
            // SAFETY: the memory was allocated for the image's requirements,
            // and neither is bound yet.
            Ok(allocation)
                if unsafe { device.bind_image_memory(image, allocation.memory(), allocation.offset()).is_ok() } =>
            {
                allocation
            }
            result => {
                if let Ok(allocation) = result {
                    let _ = allocator.free(allocation);
                }
                // SAFETY: nothing uses the image yet.
                unsafe { device.destroy_image(image, None) };
                return None;
            }
        };
        Some(ImageAllocation {
            image,
            allocation,
            image_view: vk::ImageView::null(),
            level_view: vk::ImageView::null(),
        })
    }

    /// Creates the image's view, of its `mip_levels` mip levels of
    /// `aspect`, and with more than one level, the view of level 0 that
    /// framebuffers take.
    pub(crate) fn create_view(
        &mut self,
        device: &ash::Device,
        format: vk::Format,
        components: vk::ComponentMapping,
        aspect: vk::ImageAspectFlags,
        mip_levels: u32,
    ) -> bool {
        let view = |components, level_count| {
            let create_info = vk::ImageViewCreateInfo::default()
                .image(self.image)
                .view_type(vk::ImageViewType::TYPE_2D)
                .format(format)
                .components(components)
                .subresource_range(vk::ImageSubresourceRange {
                    aspect_mask: aspect,
                    base_mip_level: 0,
                    level_count,
                    base_array_layer: 0,
                    layer_count: 1,
                });
            // SAFETY: the image is live and of `format`, with `level_count`
            // levels or more.
            unsafe { device.create_image_view(&create_info, None) }.ok()
        };
        let Some(image_view) = view(components, mip_levels) else { return false };
        self.image_view = image_view;
        if mip_levels > 1 {
            let Some(level_view) = view(vk::ComponentMapping::default(), 1) else { return false };
            self.level_view = level_view;
        }
        true
    }

    #[must_use]
    pub(crate) fn image(&self) -> vk::Image {
        self.image
    }

    #[must_use]
    pub(crate) fn image_view(&self) -> vk::ImageView {
        self.image_view
    }

    /// The view of level 0, which framebuffers take.
    #[must_use]
    pub(crate) fn level_view(&self) -> vk::ImageView {
        if self.level_view == vk::ImageView::null() { self.image_view } else { self.level_view }
    }

    /// Destroys the view and the image and frees its memory, which no
    /// command that hasn't finished uses.
    pub(crate) fn destroy(mut self, device: &ash::Device, allocator: &mut Allocator) {
        // SAFETY: the commands that use the image have finished.
        unsafe {
            for view in [self.image_view, self.level_view] {
                if view != vk::ImageView::null() {
                    device.destroy_image_view(view, None);
                }
            }
            device.destroy_image(self.image, None);
        }
        let allocation = std::mem::take(&mut self.allocation);
        let _ = allocator.free(allocation);
    }
}

impl Drop for BufferAllocation {
    fn drop(&mut self) {
        debug_assert!(
            self.allocation.is_null() || std::thread::panicking(),
            "a Vulkan buffer was dropped without being destroyed"
        );
    }
}

impl Drop for ImageAllocation {
    fn drop(&mut self) {
        debug_assert!(
            self.allocation.is_null() || std::thread::panicking(),
            "a Vulkan image was dropped without being destroyed"
        );
    }
}

/// Records a layout transition of `image`'s color `range`.
pub(crate) fn transition_image(
    device: &ash::Device,
    command_buffer: vk::CommandBuffer,
    image: vk::Image,
    range: vk::ImageSubresourceRange,
    (old_layout, new_layout): (vk::ImageLayout, vk::ImageLayout),
    (src_access, dst_access): (vk::AccessFlags, vk::AccessFlags),
    (src_stage, dst_stage): (vk::PipelineStageFlags, vk::PipelineStageFlags),
) {
    let barrier = vk::ImageMemoryBarrier::default()
        .image(image)
        .old_layout(old_layout)
        .new_layout(new_layout)
        .src_access_mask(src_access)
        .dst_access_mask(dst_access)
        .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
        .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
        .subresource_range(range);
    // SAFETY: `command_buffer` is recording outside a render pass, and
    // `image` is in `old_layout` when the command runs.
    unsafe {
        device.cmd_pipeline_barrier(
            command_buffer,
            src_stage,
            dst_stage,
            vk::DependencyFlags::empty(),
            &[],
            &[],
            &[barrier],
        );
    }
}
