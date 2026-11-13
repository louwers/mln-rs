// The resource reaches the device through the backend state and the
// rendering stats through a `Weak` context: the context's global uniform
// buffers hold resources, so an `Rc` back would keep the context alive.
//
// C++ render passes recognize a bound resource by its address; the port
// gives each resource an id instead, which a later resource can't reuse.

use std::cell::{Cell, RefCell};
use std::ffi::c_void;
use std::ptr::NonNull;
use std::rc::{Rc, Weak};

use mln_gfx::context::Context as _;
use mln_util::logging::{Event, Log};
use objc2::runtime::ProtocolObject;
use objc2_metal::{MTLBuffer, MTLDevice, MTLRenderCommandEncoder, MTLResourceOptions};

use crate::context::Context;
use crate::mtl_fwd::MTLBufferPtr;
use crate::renderer_backend::BackendImpl;

/// The size below which data is bound with `setVertexBytes:length:atIndex:`,
/// the best option for a very small amount (less than 4 KB) of dynamic buffer
/// data, which avoids the overhead of creating an intermediary `MTLBuffer`.
const BUFFER_THRESHOLD: usize = 4096;

thread_local! {
    static NEXT_ID: Cell<u64> = const { Cell::new(1) };
}

pub type VersionType = u16;

pub struct BufferResource {
    id: u64,
    backend: Rc<BackendImpl>,
    context: Weak<Context>,
    buffer: RefCell<Option<MTLBufferPtr>>,
    raw: RefCell<Vec<u8>>,
    size: usize,
    usage: MTLResourceOptions,
    version: Cell<VersionType>,
    using_vertex_buffer: Cell<bool>,
}

/// Copies `data` into a new buffer of `size` bytes, or allocates an empty
/// one when there is no data.
pub(crate) fn new_metal_buffer(
    device: &ProtocolObject<dyn objc2_metal::MTLDevice>,
    data: Option<&[u8]>,
    size: usize,
    usage: MTLResourceOptions,
) -> Option<MTLBufferPtr> {
    match data.filter(|d| !d.is_empty() && size != 0) {
        Some(data) => {
            debug_assert!(data.len() >= size);
            // SAFETY: `data` holds at least `size` bytes, which Metal copies.
            unsafe {
                device.newBufferWithBytes_length_options(
                    NonNull::new_unchecked(data.as_ptr().cast_mut().cast::<c_void>()),
                    size,
                    usage,
                )
            }
        }
        None => device.newBufferWithLength_options(size.max(1), usage),
    }
}

impl BufferResource {
    /// Creates a buffer of `size` bytes holding `data` (zeros without it).
    ///
    /// Small buffers that aren't for indexes and aren't marked persistent
    /// stay in memory and are bound with `setVertexBytes`; the others are
    /// `MTLBuffer`s. Metal draws indexed primitives only from an
    /// `MTLBuffer`. `persistent` hints that the buffer is reused many times.
    pub fn new(
        context: &Context,
        data: Option<&[u8]>,
        size: usize,
        usage: MTLResourceOptions,
        is_index_buffer: bool,
        persistent: bool,
    ) -> Self {
        let mut raw = Vec::new();
        let mut buffer = None;
        if size < BUFFER_THRESHOLD && !is_index_buffer && !persistent {
            match data {
                Some(data) => raw.extend_from_slice(&data[..size]),
                None => raw.resize(size, 0),
            }
        } else {
            buffer = new_metal_buffer(context.get_backend().get_device(), data, size, usage);
            if buffer.is_none() {
                Log::error(Event::Render, format!("Failed to allocate a Metal buffer of {size} bytes"));
            }
        }
        let resource = BufferResource {
            id: NEXT_ID.with(|id| {
                let value = id.get();
                id.set(value + 1);
                value
            }),
            backend: context.get_backend().clone(),
            context: context.weak(),
            buffer: RefCell::new(buffer),
            raw: RefCell::new(raw),
            size,
            usage,
            version: Cell::new(0),
            using_vertex_buffer: Cell::new(false),
        };
        if resource.is_valid() {
            let mut stats = context.rendering_stats();
            stats.num_buffers += 1;
            stats.mem_buffers += size as i32;
            stats.total_buffers += 1;
            if resource.buffer.borrow().is_some() {
                stats.total_buffer_objs += 1;
            }
        }
        resource
    }

    /// The context, unless it is gone.
    pub fn context(&self) -> Option<Rc<Context>> {
        self.context.upgrade()
    }

    pub fn id(&self) -> u64 {
        self.id
    }

    pub fn get_size_in_bytes(&self) -> usize {
        self.size
    }

    /// A copy of the contents.
    pub fn contents(&self) -> Vec<u8> {
        if let Some(buffer) = self.buffer.borrow().as_ref() {
            // SAFETY: the buffer is shared memory of `size` bytes, and no
            // command reading it writes to it.
            unsafe { std::slice::from_raw_parts(buffer.contents().as_ptr().cast::<u8>(), self.size) }.to_vec()
        } else {
            self.raw.borrow().clone()
        }
    }

    pub fn get_metal_buffer(&self) -> Option<MTLBufferPtr> {
        self.buffer.borrow().clone()
    }

    pub fn is_valid(&self) -> bool {
        self.buffer.borrow().is_some() || !self.raw.borrow().is_empty()
    }

    pub fn get_version(&self) -> VersionType {
        self.version.get()
    }

    /// Writes `data` at `offset`.
    ///
    /// An `MTLBuffer` may still be in use by the previous frame, so it is
    /// replaced by a new buffer rather than written to.
    pub fn update(&self, data: &[u8], offset: usize) {
        debug_assert!(data.len() + offset <= self.size);
        let update_size = data.len().min(self.size.saturating_sub(offset));
        if update_size == 0 {
            return;
        }
        let data = &data[..update_size];
        let context = self.context.upgrade();
        let mut buffer = self.buffer.borrow_mut();
        if let Some(current) = buffer.as_ref() {
            let old = current.contents().as_ptr().cast::<u8>();
            let update_is_entire_buffer = offset == 0 && update_size == self.size;
            let new_buffer = if update_is_entire_buffer {
                // SAFETY: as in `contents`.
                if unsafe { std::slice::from_raw_parts(old, self.size) } == data {
                    return;
                }
                new_metal_buffer(self.backend.get_device(), Some(data), self.size, self.usage)
            } else {
                // SAFETY: as in `contents`.
                let mut merged = unsafe { std::slice::from_raw_parts(old, self.size) }.to_vec();
                merged[offset..offset + update_size].copy_from_slice(data);
                new_metal_buffer(self.backend.get_device(), Some(&merged), self.size, self.usage)
            };
            let Some(new_buffer) = new_buffer else {
                Log::error(Event::Render, format!("Failed to allocate a Metal buffer of {} bytes", self.size));
                return;
            };
            *buffer = Some(new_buffer);
            if let Some(context) = &context {
                let mut stats = context.rendering_stats();
                stats.total_buffers += 1;
                stats.total_buffer_objs += 1;
                stats.buffer_obj_updates += 1;
                stats.buffer_update_bytes += update_size;
            }
        } else {
            let mut raw = self.raw.borrow_mut();
            if raw.is_empty() {
                return;
            }
            raw[offset..offset + update_size].copy_from_slice(data);
            if let Some(context) = &context {
                context.rendering_stats().buffer_update_bytes += update_size;
            }
        }
        if let Some(context) = &context {
            context.rendering_stats().buffer_updates += 1;
        }
        self.version.set(self.version.get().wrapping_add(1));
    }

    /// Whether a binding made at `version` is stale.
    pub fn need_rebind(&self, version: VersionType) -> bool {
        self.version.get() != version
    }

    pub fn bind_vertex(
        &self,
        encoder: &ProtocolObject<dyn MTLRenderCommandEncoder>,
        offset: usize,
        index: usize,
        size: usize,
    ) {
        debug_assert!(offset + size <= self.size);
        if let Some(buffer) = self.buffer.borrow().as_ref() {
            // SAFETY: the command buffer retains the buffer it encodes.
            unsafe { encoder.setVertexBuffer_offset_atIndex(Some(buffer), offset, index) };
            self.using_vertex_buffer.set(true);
        } else {
            let raw = self.raw.borrow();
            if !raw.is_empty() {
                let size = if size != 0 { size.min(self.size - offset) } else { self.size - offset };
                // SAFETY: `raw` holds `offset + size` bytes, which Metal copies.
                unsafe { encoder.setVertexBytes_length_atIndex(bytes_at(&raw, offset), size, index) };
                self.using_vertex_buffer.set(false);
            }
        }
    }

    pub fn bind_fragment(
        &self,
        encoder: &ProtocolObject<dyn MTLRenderCommandEncoder>,
        offset: usize,
        index: usize,
        size: usize,
    ) {
        debug_assert!(offset + size <= self.size);
        if let Some(buffer) = self.buffer.borrow().as_ref() {
            // SAFETY: as in `bind_vertex`.
            unsafe { encoder.setFragmentBuffer_offset_atIndex(Some(buffer), offset, index) };
        } else {
            let raw = self.raw.borrow();
            if !raw.is_empty() {
                let size = if size != 0 { size.min(self.size - offset) } else { self.size - offset };
                // SAFETY: as in `bind_vertex`.
                unsafe { encoder.setFragmentBytes_length_atIndex(bytes_at(&raw, offset), size, index) };
            }
        }
    }

    /// Moves the offset of this buffer, already bound at `index`.
    pub fn update_vertex_bind_offset(
        &self,
        encoder: &ProtocolObject<dyn MTLRenderCommandEncoder>,
        offset: usize,
        index: usize,
        size: usize,
    ) {
        // `setVertexBufferOffset` is documented to work for bytes set with
        // `setVertexBytes` too, but fails validation for them: "Set Vertex
        // Buffer Offset Validation index(1) must have an existing buffer."
        if self.buffer.borrow().is_some() && self.using_vertex_buffer.get() {
            // SAFETY: the buffer bound at `index` holds `offset`.
            unsafe { encoder.setVertexBufferOffset_atIndex(offset, index) };
        } else {
            self.bind_vertex(encoder, offset, index, size);
        }
    }

    pub fn update_fragment_bind_offset(
        &self,
        encoder: &ProtocolObject<dyn MTLRenderCommandEncoder>,
        offset: usize,
        index: usize,
        size: usize,
    ) {
        if self.buffer.borrow().is_some() {
            // SAFETY: as in `update_vertex_bind_offset`.
            unsafe { encoder.setFragmentBufferOffset_atIndex(offset, index) };
        } else {
            self.bind_fragment(encoder, offset, index, size);
        }
    }
}

/// The address of `raw[offset..]`; `offset` is in bounds.
fn bytes_at(raw: &[u8], offset: usize) -> NonNull<c_void> {
    NonNull::from(&raw[offset..]).cast()
}

impl Drop for BufferResource {
    fn drop(&mut self) {
        if self.is_valid()
            && let Some(context) = self.context.upgrade()
        {
            let mut stats = context.rendering_stats();
            stats.num_buffers -= 1;
            stats.mem_buffers -= self.size as i32;
        }
    }
}
