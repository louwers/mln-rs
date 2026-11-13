// A resource reaches the device through the backend state, and the frame it
// is used in, the deletion queue and the rendering stats through a `Weak`
// context: the context's global uniform buffers hold resources, so an `Rc`
// back would keep the context alive. Without its context, a resource is
// destroyed when it is dropped.
//
// Differences from C++:
// - The memory comes from gpu-allocator, which keeps host-visible memory
//   mapped, in place of VulkanMemoryAllocator.
// - A failed allocation is logged and leaves the resource invalid, where C++
//   throws `std::bad_alloc`.
// - `clone()` and `shared()`, which C++ uses for the instance data of
//   symbols and fill extrusions, are not ported.
// - A uniform or storage buffer keeps its contents in CPU memory as well. An
//   update is compared with that copy and a frame's copy is written from it,
//   where C++ compares with and copies from the mapped memory: reading that
//   is slow when the driver maps it write-combined.
// - Other buffers are written without comparing.

use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};
use std::sync::atomic::{AtomicU64, Ordering};

use ash::vk;
use gpu_allocator::MemoryLocation;
use gpu_allocator::vulkan::{Allocation, AllocationCreateDesc, AllocationScheme};
use mln_gfx::context::Context as _;
use mln_util::logging::{Event, Log};

use crate::context::Context;
use crate::renderer_backend::BackendImpl;

static NEXT_ID: AtomicU64 = AtomicU64::new(1);

/// An id no other buffer, image or sampler has had. Vulkan can give a new
/// object the handle of a destroyed one, so what a descriptor points at is
/// told apart by these.
pub(crate) fn unique_id() -> u64 {
    NEXT_ID.fetch_add(1, Ordering::Relaxed)
}

/// A buffer and its memory, mapped.
pub(crate) struct BufferAllocation {
    id: u64,
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
        Some(BufferAllocation {
            id: unique_id(),
            backend: Rc::clone(backend),
            buffer,
            allocation: Some(allocation),
            mapped_buffer,
        })
    }

    #[must_use]
    pub(crate) fn id(&self) -> u64 {
        self.id
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

pub(crate) type VersionType = u16;

pub struct BufferResource {
    context: Weak<Context>,
    size: usize,
    version: Cell<VersionType>,
    buffer_allocation: Option<Rc<BufferAllocation>>,
    /// The size of a frame's copy of a uniform or storage buffer; 0 for
    /// other buffers, which have one copy.
    buffer_window_size: usize,
    /// The version of `contents` each frame's copy holds.
    buffer_window_versions: RefCell<Vec<VersionType>>,
    /// The contents of a uniform or storage buffer, `size` bytes; empty for
    /// other buffers.
    contents: RefCell<Vec<u8>>,
}

fn align_up(size: usize, align: usize) -> usize {
    if align <= 1 { size } else { size.div_ceil(align) * align }
}

fn lcm(a: usize, b: usize) -> usize {
    fn gcd(a: usize, b: usize) -> usize {
        if b == 0 { a } else { gcd(b, a % b) }
    }
    if a == 0 || b == 0 { a.max(b) } else { a / gcd(a, b) * b }
}

impl BufferResource {
    /// Creates a buffer of `size` bytes holding `data`. Without `data` a
    /// uniform or storage buffer holds zeros, and the contents of other
    /// buffers are unspecified.
    ///
    /// A uniform or storage buffer has a copy of its contents for each frame
    /// in flight, so that writing the next frame's doesn't change what the
    /// GPU is still reading.
    pub(crate) fn new(
        context: &Context,
        data: Option<&[u8]>,
        size: usize,
        usage: vk::BufferUsageFlags,
        _persistent: bool,
    ) -> Self {
        let backend = context.get_backend();
        let mut total_size = size;
        let mut buffer_window_size = 0;
        let mut buffer_window_versions = Vec::new();
        let mut contents = Vec::new();
        let mut version = 0;

        // TODO -> check avg minUniformBufferOffsetAlignment vs individual buffers
        if !usage.contains(vk::BufferUsageFlags::VERTEX_BUFFER)
            && usage.intersects(vk::BufferUsageFlags::UNIFORM_BUFFER | vk::BufferUsageFlags::STORAGE_BUFFER)
        {
            let limits = &backend.get_device_properties().limits;
            let mut align = 0;
            if usage.contains(vk::BufferUsageFlags::UNIFORM_BUFFER) {
                align = limits.min_uniform_buffer_offset_alignment as usize;
            }
            if usage.contains(vk::BufferUsageFlags::STORAGE_BUFFER) {
                align = lcm(align, limits.min_storage_buffer_offset_alignment as usize);
            }
            buffer_window_size = align_up(size.max(1), align);
            let frame_count = backend.get_max_frames() as usize;
            total_size = buffer_window_size * frame_count;
            buffer_window_versions = vec![0; frame_count];
            contents = vec![0; size];
            // No frame's copy holds the contents yet.
            version = 1;
        }

        let buffer_allocation =
            BufferAllocation::create(backend, total_size, usage, MemoryLocation::CpuToGpu, "buffer").map(Rc::new);
        if buffer_allocation.as_ref().is_none_or(|allocation| allocation.mapped_buffer().is_null()) {
            Log::error(Event::Render, "Vulkan buffer allocation failed");
        }

        let resource = BufferResource {
            context: context.weak(),
            size,
            version: Cell::new(version),
            buffer_allocation: buffer_allocation.filter(|allocation| !allocation.mapped_buffer().is_null()),
            buffer_window_size,
            buffer_window_versions: RefCell::new(buffer_window_versions),
            contents: RefCell::new(contents),
        };
        if let Some(data) = data {
            resource.update(&data[..size.min(data.len())], 0);
        }
        if resource.is_valid() {
            let mut stats = context.rendering_stats();
            stats.num_buffers += 1;
            stats.mem_buffers += resource.allocated_size() as i32;
            stats.total_buffers += 1;
            stats.total_buffer_objs += 1;
        }
        resource
    }

    /// The context, unless it is gone.
    pub(crate) fn context(&self) -> Option<Rc<Context>> {
        self.context.upgrade()
    }

    pub(crate) fn get_size_in_bytes(&self) -> usize {
        self.size
    }

    /// The bytes allocated: every frame's copy.
    fn allocated_size(&self) -> usize {
        if self.buffer_window_size > 0 {
            self.buffer_window_size * self.buffer_window_versions.borrow().len()
        } else {
            self.size
        }
    }

    /// The buffer, null if its allocation failed.
    pub fn get_vulkan_buffer(&self) -> vk::Buffer {
        self.buffer_allocation.as_ref().map_or(vk::Buffer::null(), |allocation| allocation.buffer())
    }

    /// The id of the buffer's allocation (`unique_id`), 0 without one.
    pub(crate) fn allocation_id(&self) -> u64 {
        self.buffer_allocation.as_ref().map_or(0, |allocation| allocation.id())
    }

    /// The frame being recorded.
    fn current_frame(&self) -> u8 {
        self.context.upgrade().map_or(0, |context| context.get_current_frame_resource_index())
    }

    /// Where the current frame's copy starts.
    pub(crate) fn get_vulkan_buffer_offset(&self) -> usize {
        self.get_vulkan_buffer_offset_of(self.current_frame())
    }

    pub(crate) fn get_vulkan_buffer_offset_of(&self, resource_index: u8) -> usize {
        usize::from(resource_index) * self.buffer_window_size
    }

    pub fn is_valid(&self) -> bool {
        self.buffer_allocation.is_some()
    }

    /// Writes `data` at `offset` of the buffer. The current frame's copy of
    /// a uniform or storage buffer holds the new contents afterwards, and
    /// the other frames' copies when they are next brought up to date
    /// (`update_vulkan_buffer`).
    pub(crate) fn update(&self, data: &[u8], offset: usize) {
        debug_assert!(data.len() + offset <= self.size);
        let update_size = data.len().min(self.size.saturating_sub(offset));
        let Some(allocation) = &self.buffer_allocation else {
            return;
        };
        if update_size == 0 {
            return;
        }
        let data = &data[..update_size];

        if self.buffer_window_size > 0 {
            let changed = {
                let mut contents = self.contents.borrow_mut();
                let range = &mut contents[offset..offset + update_size];
                let changed = *range != *data;
                if changed {
                    range.copy_from_slice(data);
                }
                changed
            };
            if changed {
                self.contents_changed(update_size);
            }
            self.update_vulkan_buffer(self.current_frame());
            return;
        }

        // SAFETY: the mapped memory is `size` bytes, `offset + update_size`
        // is within it, and `data` is not part of it.
        unsafe { std::ptr::copy_nonoverlapping(data.as_ptr(), allocation.mapped_buffer().add(offset), update_size) };
        self.contents_changed(update_size);
    }

    /// Counts an update of `update_size` bytes and gives the contents a new
    /// version.
    fn contents_changed(&self, update_size: usize) {
        if let Some(context) = self.context.upgrade() {
            let mut stats = context.rendering_stats();
            stats.buffer_update_bytes += update_size;
            stats.buffer_updates += 1;
            stats.buffer_obj_updates += 1;
        }
        let mut version = self.version.get().wrapping_add(1);
        if version == VersionType::MAX {
            version = 1;
            self.buffer_window_versions.borrow_mut().fill(0);
        }
        self.version.set(version);
    }

    /// Brings the copy of the frame `frame` up to date with the buffer's
    /// contents.
    pub(crate) fn update_vulkan_buffer(&self, frame: u8) {
        if self.buffer_window_size == 0 {
            return;
        }
        let Some(allocation) = &self.buffer_allocation else {
            return;
        };
        let mut window_versions = self.buffer_window_versions.borrow_mut();
        let Some(window_version) = window_versions.get_mut(usize::from(frame)) else {
            return;
        };
        if *window_version == self.version.get() {
            return;
        }
        let contents = self.contents.borrow();
        // SAFETY: the mapped memory holds a window of at least `size` bytes
        // for each frame, and `contents` is `size` bytes outside it.
        unsafe {
            std::ptr::copy_nonoverlapping(
                contents.as_ptr(),
                allocation.mapped_buffer().add(self.get_vulkan_buffer_offset_of(frame)),
                contents.len(),
            );
        }
        *window_version = self.version.get();
    }
}

impl Drop for BufferResource {
    /// `destroy(deferred = true)`: the frames in flight may still use the
    /// buffer, so it goes when its frame has been waited for.
    fn drop(&mut self) {
        let Some(allocation) = self.buffer_allocation.take() else {
            return;
        };
        let size = self.allocated_size();
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
