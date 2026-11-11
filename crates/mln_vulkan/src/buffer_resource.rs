// Ports include/mln/vulkan/buffer_resource.hpp and src/mln/vulkan/buffer_resource.cpp
//
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

/// `BufferAllocation`: a buffer and its memory, mapped.
pub struct BufferAllocation {
    id: u64,
    backend: Rc<BackendImpl>,
    buffer: vk::Buffer,
    allocation: Option<Allocation>,
    mapped_buffer: *mut u8,
}

impl BufferAllocation {
    /// `create(allocInfo, bufferInfo)`: a buffer of `size` bytes in
    /// host-visible memory, written (`MemoryLocation::CpuToGpu`) or read
    /// (`GpuToCpu`) by the CPU.
    pub fn create(
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
            backend: backend.clone(),
            buffer,
            allocation: Some(allocation),
            mapped_buffer,
        })
    }

    pub fn id(&self) -> u64 {
        self.id
    }

    pub fn buffer(&self) -> vk::Buffer {
        self.buffer
    }

    /// The buffer's memory, or null if the allocator couldn't map it.
    pub fn mapped_buffer(&self) -> *mut u8 {
        self.mapped_buffer
    }
}

impl Drop for BufferAllocation {
    /// `destroy()`
    fn drop(&mut self) {
        // SAFETY: the owner of the allocation waited for the commands that
        // use the buffer (the context's deletion queue), or never used it.
        unsafe { self.backend.get_device().destroy_buffer(self.buffer, None) };
        if let Some(allocation) = self.allocation.take() {
            let _ = self.backend.with_allocator(|allocator| allocator.free(allocation));
        }
    }
}

pub type VersionType = u16;

pub struct BufferResource {
    context: Weak<Context>,
    size: usize,
    version: Cell<VersionType>,
    buffer_allocation: Option<Rc<BufferAllocation>>,
    /// The size of a frame's copy of a uniform or storage buffer; 0 for
    /// other buffers, which have one copy.
    buffer_window_size: usize,
    buffer_window_versions: RefCell<Vec<VersionType>>,
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
    /// Creates a buffer of `size` bytes holding `data` (zeros without it).
    ///
    /// A uniform or storage buffer has a copy of its contents for each frame
    /// in flight, so that writing the next frame's doesn't change what the
    /// GPU is still reading.
    pub fn new(
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
        }

        let buffer_allocation =
            BufferAllocation::create(backend, total_size, usage, MemoryLocation::CpuToGpu, "buffer").map(Rc::new);
        if buffer_allocation.as_ref().is_none_or(|allocation| allocation.mapped_buffer().is_null()) {
            Log::error(Event::Render, "Vulkan buffer allocation failed");
        }

        let resource = BufferResource {
            context: context.weak(),
            size,
            version: Cell::new(0),
            buffer_allocation: buffer_allocation.filter(|allocation| !allocation.mapped_buffer().is_null()),
            buffer_window_size,
            buffer_window_versions: RefCell::new(buffer_window_versions),
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
    pub fn context(&self) -> Option<Rc<Context>> {
        self.context.upgrade()
    }

    pub fn get_size_in_bytes(&self) -> usize {
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

    /// `getVulkanBuffer()`: the buffer, null if its allocation failed.
    pub fn get_vulkan_buffer(&self) -> vk::Buffer {
        self.buffer_allocation.as_ref().map_or(vk::Buffer::null(), |allocation| allocation.buffer())
    }

    /// The id of the buffer's allocation (`unique_id`), 0 without one.
    pub fn allocation_id(&self) -> u64 {
        self.buffer_allocation.as_ref().map_or(0, |allocation| allocation.id())
    }

    /// The frame being recorded.
    fn current_frame(&self) -> u8 {
        self.context.upgrade().map_or(0, |context| context.get_current_frame_resource_index())
    }

    /// `getVulkanBufferOffset()`: where the current frame's copy starts.
    pub fn get_vulkan_buffer_offset(&self) -> usize {
        self.get_vulkan_buffer_offset_of(self.current_frame())
    }

    /// `getVulkanBufferOffset(resourceIndex)`
    pub fn get_vulkan_buffer_offset_of(&self, resource_index: u8) -> usize {
        usize::from(resource_index) * self.buffer_window_size
    }

    pub fn is_valid(&self) -> bool {
        self.buffer_allocation.is_some()
    }

    /// Used to detect whether buffer contents have changed.
    pub fn get_version(&self) -> VersionType {
        self.version.get()
    }

    /// Writes `data` at `offset` of the current frame's copy.
    pub fn update(&self, data: &[u8], offset: usize) {
        debug_assert!(data.len() + offset <= self.size);
        let update_size = data.len().min(self.size.saturating_sub(offset));
        let Some(allocation) = &self.buffer_allocation else {
            return;
        };
        if update_size == 0 {
            return;
        }
        let frame = self.current_frame();
        // SAFETY: the mapped memory holds every frame's copy of `size`
        // bytes, `offset + update_size` is within one, and this thread is
        // the only one that reads or writes it.
        let contents = unsafe {
            std::slice::from_raw_parts_mut(
                allocation.mapped_buffer().add(self.get_vulkan_buffer_offset_of(frame) + offset),
                update_size,
            )
        };
        let mut window_versions = self.buffer_window_versions.borrow_mut();
        if *contents == data[..update_size] {
            if let Some(version) = window_versions.get_mut(usize::from(frame)) {
                *version = self.version.get();
            }
            return;
        }
        contents.copy_from_slice(&data[..update_size]);

        if let Some(context) = self.context.upgrade() {
            let mut stats = context.rendering_stats();
            stats.buffer_update_bytes += update_size;
            stats.buffer_updates += 1;
            stats.buffer_obj_updates += 1;
        }
        let mut version = self.version.get().wrapping_add(1);
        if version == VersionType::MAX {
            version = 1;
            window_versions.fill(0);
        }
        self.version.set(version);
        if let Some(window_version) = window_versions.get_mut(usize::from(frame)) {
            *window_version = version;
        }
    }

    /// `updateVulkanBuffer(destination, source)`: brings the frame
    /// `destination`'s copy up to date with the frame `source`'s, if that
    /// one is newer.
    pub fn update_vulkan_buffer(&self, destination: u8, source: u8) {
        if self.buffer_window_size == 0 || destination == source {
            return;
        }
        let Some(allocation) = &self.buffer_allocation else {
            return;
        };
        let mut window_versions = self.buffer_window_versions.borrow_mut();
        let (destination, source) = (usize::from(destination), usize::from(source));
        if destination >= window_versions.len() || source >= window_versions.len() {
            return;
        }
        if window_versions[destination] < window_versions[source] {
            // SAFETY: the two frames' copies are `size` bytes each, apart.
            unsafe {
                std::ptr::copy_nonoverlapping(
                    allocation.mapped_buffer().add(self.buffer_window_size * source),
                    allocation.mapped_buffer().add(self.buffer_window_size * destination),
                    self.size,
                );
            }
            window_versions[destination] = window_versions[source];
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
