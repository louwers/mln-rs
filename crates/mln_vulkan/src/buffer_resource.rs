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
// - The copies of a uniform or storage buffer aren't tied to the frames in
//   flight. An update is written, without comparing, to a copy that no
//   unfinished frame reads, and a frame binds the copy written last
//   (`Windows`). C++ gives each frame a copy, compares an update with the
//   mapped memory and copies from one frame's copy to the next: reading
//   mapped memory is slow when the driver maps it write-combined.
// - An update that finds every copy read by an unfinished frame, because the
//   frame being recorded has bound the buffer, moves the buffer to a new
//   allocation, where C++ overwrites what the frame's earlier draws read.
// - Other buffers are written without comparing.

use std::cell::RefCell;
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

/// The copies ("windows") of a uniform or storage buffer: which one holds
/// its contents, and the frames that read each. A frame in flight reads the
/// copy it bound until it is done, so a copy is written only when the frames
/// that bound it are.
struct Windows {
    /// The copy that holds the contents.
    latest: usize,
    /// The frame that last bound each copy (`Context::frame_serial`); 0 for
    /// none. Empty for other buffers, which have no copies.
    bound: Vec<u64>,
}

impl Windows {
    fn new(count: usize) -> Self {
        Windows { latest: 0, bound: vec![0; count] }
    }

    /// The copy an update can write: the one holding the contents if the
    /// frame that bound it `is_done`, else another whose frame is. `None` if
    /// unfinished frames read every copy.
    fn writable(&self, is_done: impl Fn(u64) -> bool) -> Option<usize> {
        std::iter::once(self.latest).chain(0..self.bound.len()).find(|&window| is_done(self.bound[window]))
    }

    /// The copy the frame `serial` binds: the one holding the contents.
    fn bind(&mut self, serial: u64) -> usize {
        if let Some(bound) = self.bound.get_mut(self.latest) {
            *bound = serial;
        }
        self.latest
    }
}

pub struct BufferResource {
    context: Weak<Context>,
    size: usize,
    usage: vk::BufferUsageFlags,
    /// Replaced when an update finds no copy it can write.
    buffer_allocation: RefCell<Option<Rc<BufferAllocation>>>,
    /// The size of a copy of a uniform or storage buffer; 0 for other
    /// buffers, which have one copy.
    buffer_window_size: usize,
    windows: RefCell<Windows>,
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

/// A mapped buffer of `size` bytes, holding zeros if `zeroed`.
fn allocate(
    backend: &Rc<BackendImpl>,
    size: usize,
    usage: vk::BufferUsageFlags,
    zeroed: bool,
) -> Option<Rc<BufferAllocation>> {
    let allocation = BufferAllocation::create(backend, size, usage, MemoryLocation::CpuToGpu, "buffer")
        .filter(|allocation| !allocation.mapped_buffer().is_null())?;
    if zeroed {
        // SAFETY: the mapped memory is at least `size` bytes.
        unsafe { std::ptr::write_bytes(allocation.mapped_buffer(), 0, size) };
    }
    Some(Rc::new(allocation))
}

impl BufferResource {
    /// Creates a buffer of `size` bytes holding `data`. A uniform or storage
    /// buffer holds zeros past `data`, and the contents of other buffers are
    /// unspecified there.
    ///
    /// A uniform or storage buffer has a copy of its contents for each frame
    /// in flight, so that an update doesn't change what the GPU is still
    /// reading.
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
        let mut window_count = 0;

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
            window_count = backend.get_max_frames() as usize;
            total_size = buffer_window_size * window_count;
        }

        let buffer_allocation = allocate(backend, total_size, usage, window_count > 0);
        if buffer_allocation.is_none() {
            Log::error(Event::Render, "Vulkan buffer allocation failed");
        }

        let resource = BufferResource {
            context: context.weak(),
            size,
            usage,
            buffer_allocation: RefCell::new(buffer_allocation),
            buffer_window_size,
            windows: RefCell::new(Windows::new(window_count)),
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

    /// The bytes allocated: every copy.
    fn allocated_size(&self) -> usize {
        if self.buffer_window_size > 0 {
            self.buffer_window_size * self.windows.borrow().bound.len()
        } else {
            self.size
        }
    }

    /// `getVulkanBuffer()`: the buffer, null if its allocation failed.
    pub fn get_vulkan_buffer(&self) -> vk::Buffer {
        self.buffer_allocation.borrow().as_ref().map_or(vk::Buffer::null(), |allocation| allocation.buffer())
    }

    /// The id of the buffer's allocation (`unique_id`), 0 without one.
    pub fn allocation_id(&self) -> u64 {
        self.buffer_allocation.borrow().as_ref().map_or(0, |allocation| allocation.id())
    }

    /// `getVulkanBufferOffset()`: where the buffer's contents start, for the
    /// frame being recorded to bind. The contents stay there until the frame
    /// is done.
    pub fn get_vulkan_buffer_offset(&self) -> usize {
        let serial = self.context.upgrade().map_or(0, |context| context.frame_serial());
        self.windows.borrow_mut().bind(serial) * self.buffer_window_size
    }

    pub fn is_valid(&self) -> bool {
        self.buffer_allocation.borrow().is_some()
    }

    /// Writes `data` at `offset` of the buffer.
    ///
    /// A uniform or storage buffer is written from its start, and its bytes
    /// past `data` are unspecified afterwards: the update goes to a copy
    /// that no unfinished frame reads, which may be another one than the
    /// last update's.
    pub fn update(&self, data: &[u8], offset: usize) {
        debug_assert!(data.len() + offset <= self.size);
        let update_size = data.len().min(self.size.saturating_sub(offset));
        if update_size == 0 || !self.is_valid() {
            return;
        }
        let mut offset = offset;
        if self.buffer_window_size > 0 {
            debug_assert_eq!(offset, 0, "a uniform or storage buffer is written from its start");
            offset += self.writable_window() * self.buffer_window_size;
        }
        let allocation = self.buffer_allocation.borrow();
        let Some(allocation) = allocation.as_ref() else {
            return;
        };
        // SAFETY: the mapped memory holds `size` bytes from the start of
        // each copy, `offset + update_size` is within them, and `data` is
        // not part of it.
        unsafe {
            std::ptr::copy_nonoverlapping(data.as_ptr(), allocation.mapped_buffer().add(offset), update_size);
        }
        if let Some(context) = self.context.upgrade() {
            let mut stats = context.rendering_stats();
            stats.buffer_update_bytes += update_size;
            stats.buffer_updates += 1;
            stats.buffer_obj_updates += 1;
        }
    }

    /// The copy of a uniform or storage buffer an update writes, which holds
    /// the contents from then on. If unfinished frames read every copy (the
    /// frame being recorded bound the buffer before the update), the buffer
    /// moves to a new allocation, and the old one goes when its frames are
    /// done.
    fn writable_window(&self) -> usize {
        let mut windows = self.windows.borrow_mut();
        // Nothing is drawn without the context.
        let Some(context) = self.context.upgrade() else {
            return windows.latest;
        };
        if let Some(window) = windows.writable(|serial| context.frame_is_done(serial)) {
            windows.latest = window;
            return window;
        }
        let count = windows.bound.len();
        let Some(allocation) = allocate(context.get_backend(), self.buffer_window_size * count, self.usage, true)
        else {
            Log::error(Event::Render, "Vulkan buffer allocation failed");
            return windows.latest;
        };
        let replaced = self.buffer_allocation.replace(Some(allocation));
        context.enqueue_deletion(Box::new(move |_| drop(replaced)));
        context.rendering_stats().total_buffer_objs += 1;
        *windows = Windows::new(count);
        windows.latest
    }
}

impl Drop for BufferResource {
    /// `destroy(deferred = true)`: the frames in flight may still use the
    /// buffer, so it goes when its frame has been waited for.
    fn drop(&mut self) {
        let Some(allocation) = self.buffer_allocation.get_mut().take() else {
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

#[cfg(test)]
mod tests {
    use super::Windows;

    /// The frames in flight with `frames` frame resources while `serial` is
    /// recorded: it and the `frames - 1` before it.
    fn done(serial: u64, frames: u64) -> impl Fn(u64) -> bool {
        move |bound| bound == 0 || bound + frames <= serial
    }

    #[test]
    fn one_copy_is_written_in_place_between_frames() {
        let mut windows = Windows::new(1);
        for serial in 1..5 {
            assert_eq!(windows.writable(done(serial, 1)), Some(0));
            assert_eq!(windows.bind(serial), 0);
        }
    }

    #[test]
    fn two_copies_alternate_when_every_frame_updates() {
        let mut windows = Windows::new(2);
        for serial in 1..7 {
            let window = windows.writable(done(serial, 2)).expect("a copy to write");
            // Not the copy the frame before, still in flight, reads.
            assert_eq!(window, ((serial - 1) % 2) as usize);
            windows.latest = window;
            assert_eq!(windows.bind(serial), window);
        }
    }

    #[test]
    fn a_frame_without_an_update_binds_the_copy_written_last() {
        let mut windows = Windows::new(2);
        windows.latest = windows.writable(done(1, 2)).unwrap();
        assert_eq!(windows.bind(1), 0);
        // Frames 2 and 3 write nothing, and read what frame 1 wrote.
        assert_eq!(windows.bind(2), 0);
        assert_eq!(windows.bind(3), 0);
        // Frame 4 can't write the copy frame 3 reads.
        assert_eq!(windows.writable(done(4, 2)), Some(1));
    }

    #[test]
    fn an_update_between_binds_of_a_frame_has_no_copy_to_write() {
        let mut windows = Windows::new(1);
        windows.bind(1);
        assert_eq!(windows.writable(done(1, 1)), None);

        let mut windows = Windows::new(2);
        windows.bind(1);
        // The other copy first, and then none.
        assert_eq!(windows.writable(done(1, 2)), Some(1));
        windows.latest = 1;
        windows.bind(1);
        assert_eq!(windows.writable(done(1, 2)), None);
    }

    #[test]
    fn a_copy_never_bound_is_writable() {
        let windows = Windows::new(2);
        assert_eq!(windows.writable(|bound| bound == 0), Some(0));
    }
}
