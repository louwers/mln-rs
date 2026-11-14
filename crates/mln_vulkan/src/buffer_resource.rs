// A resource reaches the device through the backend state, and the deletion
// queue and the rendering stats through a `Weak` context, which the context's
// own resources would otherwise keep alive. Without its context, a resource
// is destroyed when it is dropped.
//
// Differences from C++:
// - The memory comes from gpu-allocator, which keeps host-visible memory
//   mapped, in place of VulkanMemoryAllocator.
// - A failed allocation is logged and leaves the resource invalid, where C++
//   throws `std::bad_alloc`.
// - `clone()` and `shared()`, which C++ uses for the instance data of
//   symbols and fill extrusions, are not ported.

use std::rc::{Rc, Weak};

use ash::vk;
use gpu_allocator::MemoryLocation;
use gpu_allocator::vulkan::{Allocation, AllocationCreateDesc, AllocationScheme};
use mln_gfx::context::Context as _;
use mln_util::logging::{Event, Log};

use crate::context::Context;
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
        // use the buffer (the context's deletion queue), or never used it.
        unsafe { self.backend.get_device().destroy_buffer(self.buffer, None) };
        if let Some(allocation) = self.allocation.take() {
            let _ = self.backend.with_allocator(|allocator| allocator.free(allocation));
        }
    }
}

pub struct BufferResource {
    context: Weak<Context>,
    size: usize,
    buffer_allocation: Option<Rc<BufferAllocation>>,
}

impl BufferResource {
    /// Creates a buffer of `size` bytes holding `data`. Without `data` its
    /// contents are unspecified.
    pub(crate) fn new(context: &Context, data: Option<&[u8]>, size: usize, usage: vk::BufferUsageFlags) -> Self {
        let buffer_allocation =
            BufferAllocation::create(context.get_backend(), size, usage, MemoryLocation::CpuToGpu, "buffer")
                .map(Rc::new);
        if buffer_allocation.as_ref().is_none_or(|allocation| allocation.mapped_buffer().is_null()) {
            Log::error(Event::Render, "Vulkan buffer allocation failed");
        }

        let resource = BufferResource {
            context: context.weak(),
            size,
            buffer_allocation: buffer_allocation.filter(|allocation| !allocation.mapped_buffer().is_null()),
        };
        if let Some(data) = data {
            resource.update(&data[..size.min(data.len())], 0);
        }
        if resource.is_valid() {
            let mut stats = context.rendering_stats();
            stats.num_buffers += 1;
            stats.mem_buffers += size as i32;
            stats.total_buffers += 1;
            stats.total_buffer_objs += 1;
        }
        resource
    }

    /// The buffer, null if its allocation failed.
    pub fn get_vulkan_buffer(&self) -> vk::Buffer {
        self.buffer_allocation.as_ref().map_or(vk::Buffer::null(), |allocation| allocation.buffer())
    }

    pub fn is_valid(&self) -> bool {
        self.buffer_allocation.is_some()
    }

    /// Writes `data` at `offset` of the buffer.
    pub(crate) fn update(&self, data: &[u8], offset: usize) {
        debug_assert!(data.len() + offset <= self.size);
        let update_size = data.len().min(self.size.saturating_sub(offset));
        let Some(allocation) = &self.buffer_allocation else {
            return;
        };
        if update_size == 0 {
            return;
        }
        // SAFETY: the mapped memory is `size` bytes, `offset + update_size`
        // is within it, and `data` is not part of it.
        unsafe { std::ptr::copy_nonoverlapping(data.as_ptr(), allocation.mapped_buffer().add(offset), update_size) };
        if let Some(context) = self.context.upgrade() {
            let mut stats = context.rendering_stats();
            stats.buffer_update_bytes += update_size;
            stats.buffer_updates += 1;
            stats.buffer_obj_updates += 1;
        }
    }
}

impl Drop for BufferResource {
    /// `destroy(deferred = true)`: the frames in flight may still use the
    /// buffer, so it goes when its frame has been waited for.
    fn drop(&mut self) {
        let Some(allocation) = self.buffer_allocation.take() else {
            return;
        };
        let size = self.size;
        if let Some(context) = self.context.upgrade() {
            context.enqueue_deletion(Box::new(move |context| {
                let mut stats = context.rendering_stats();
                stats.num_buffers -= 1;
                stats.mem_buffers -= size as i32;
                drop(stats);
                drop(allocation);
            }));
        }
    }
}
