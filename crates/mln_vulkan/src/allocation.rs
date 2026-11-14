//! Buffers and images with their memory from gpu-allocator, which keeps
//! host-visible memory mapped. Dropping one destroys it at once: its owner
//! waits for the commands that use it first.

use std::rc::Rc;

use ash::vk;
use gpu_allocator::MemoryLocation;
use gpu_allocator::vulkan::{Allocation, AllocationCreateDesc, AllocationScheme};

use crate::renderer_backend::BackendImpl;

/// A buffer and its memory, mapped.
pub(crate) struct BufferAllocation {
    backend: Rc<BackendImpl>,
    buffer: vk::Buffer,
    allocation: Option<Allocation>,
    mapped_buffer: *mut u8,
}

impl BufferAllocation {
    /// Creates a buffer of `size` bytes in host-visible memory, written
    /// (`MemoryLocation::CpuToGpu`) or read (`GpuToCpu`) by the CPU.
    pub(crate) fn create(
        backend: &Rc<BackendImpl>,
        size: usize,
        usage: vk::BufferUsageFlags,
        location: MemoryLocation,
        name: &str,
    ) -> Option<BufferAllocation> {
        let device = backend.get_device();
        let buffer_info = vk::BufferCreateInfo::default()
            .size(size.max(1) as vk::DeviceSize)
            .usage(usage)
            .sharing_mode(vk::SharingMode::EXCLUSIVE);
        // SAFETY: the create info is valid for the device.
        let buffer = unsafe { device.create_buffer(&buffer_info, None) }.ok()?;
        // SAFETY: `buffer` is a buffer of the device.
        let requirements = unsafe { device.get_buffer_memory_requirements(buffer) };
        let allocation = backend.with_allocator(|allocator| {
            allocator.allocate(&AllocationCreateDesc {
                name,
                requirements,
                location,
                linear: true,
                allocation_scheme: AllocationScheme::GpuAllocatorManaged,
            })
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
                    let _ = backend.with_allocator(|allocator| allocator.free(allocation));
                }
                // SAFETY: nothing uses the buffer yet.
                unsafe { device.destroy_buffer(buffer, None) };
                return None;
            }
        };
        let mapped_buffer = allocation.mapped_ptr().map_or(std::ptr::null_mut(), |p| p.as_ptr().cast::<u8>());
        Some(BufferAllocation { backend: Rc::clone(backend), buffer, allocation: Some(allocation), mapped_buffer })
    }

    #[must_use]
    pub(crate) fn buffer(&self) -> vk::Buffer {
        self.buffer
    }

    /// The buffer's memory, or null if the allocator couldn't map it.
    #[must_use]
    pub(crate) fn mapped_buffer(&self) -> *mut u8 {
        self.mapped_buffer
    }
}

impl Drop for BufferAllocation {
    fn drop(&mut self) {
        // SAFETY: the owner of the allocation waited for the commands that
        // use the buffer, if any.
        unsafe { self.backend.get_device().destroy_buffer(self.buffer, None) };
        if let Some(allocation) = self.allocation.take() {
            let _ = self.backend.with_allocator(|allocator| allocator.free(allocation));
        }
    }
}

/// An image, its memory and its view.
pub(crate) struct ImageAllocation {
    backend: Rc<BackendImpl>,
    image: vk::Image,
    allocation: Option<Allocation>,
    image_view: vk::ImageView,
}

impl ImageAllocation {
    /// Creates an image in device memory (`MemoryLocation::GpuOnly`) or in
    /// memory the CPU reads (`GpuToCpu`).
    pub(crate) fn create(
        backend: &Rc<BackendImpl>,
        image_info: &vk::ImageCreateInfo<'_>,
        location: MemoryLocation,
        name: &str,
    ) -> Option<ImageAllocation> {
        let device = backend.get_device();
        // SAFETY: the create info is valid for the device.
        let image = unsafe { device.create_image(image_info, None) }.ok()?;
        // SAFETY: `image` is an image of the device.
        let requirements = unsafe { device.get_image_memory_requirements(image) };
        let allocation = backend.with_allocator(|allocator| {
            allocator.allocate(&AllocationCreateDesc {
                name,
                requirements,
                location,
                linear: image_info.tiling == vk::ImageTiling::LINEAR,
                allocation_scheme: AllocationScheme::GpuAllocatorManaged,
            })
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
                    let _ = backend.with_allocator(|allocator| allocator.free(allocation));
                }
                // SAFETY: nothing uses the image yet.
                unsafe { device.destroy_image(image, None) };
                return None;
            }
        };
        Some(ImageAllocation {
            backend: Rc::clone(backend),
            image,
            allocation: Some(allocation),
            image_view: vk::ImageView::null(),
        })
    }

    /// Creates the image's view, of every mip level of `aspect`.
    pub(crate) fn create_view(
        &mut self,
        format: vk::Format,
        components: vk::ComponentMapping,
        aspect: vk::ImageAspectFlags,
        mip_levels: u32,
    ) -> bool {
        let create_info = vk::ImageViewCreateInfo::default()
            .image(self.image)
            .view_type(vk::ImageViewType::TYPE_2D)
            .format(format)
            .components(components)
            .subresource_range(vk::ImageSubresourceRange {
                aspect_mask: aspect,
                base_mip_level: 0,
                level_count: mip_levels,
                base_array_layer: 0,
                layer_count: 1,
            });
        // SAFETY: the image is live and of `format`.
        match unsafe { self.backend.get_device().create_image_view(&create_info, None) } {
            Ok(view) => {
                self.image_view = view;
                true
            }
            Err(_) => false,
        }
    }

    #[must_use]
    pub(crate) fn image(&self) -> vk::Image {
        self.image
    }

    #[must_use]
    pub(crate) fn image_view(&self) -> vk::ImageView {
        self.image_view
    }
}

impl Drop for ImageAllocation {
    fn drop(&mut self) {
        let device = self.backend.get_device();
        // SAFETY: the owner of the allocation waited for the commands that
        // use the image, if any.
        unsafe {
            if self.image_view != vk::ImageView::null() {
                device.destroy_image_view(self.image_view, None);
            }
            device.destroy_image(self.image, None);
        }
        if let Some(allocation) = self.allocation.take() {
            let _ = self.backend.with_allocator(|allocator| allocator.free(allocation));
        }
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
