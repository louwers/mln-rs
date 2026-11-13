// Both are shared through `Rc` (gfx::AttributeBinding keeps a clone) and
// updated in place, so the buffer sits behind a `RefCell`.

use std::cell::{Cell, Ref, RefCell};

use mln_gfx::vertex_buffer;
use mln_util::time::Instant;

use crate::buffer_resource::BufferResource;

pub struct VertexBufferResource {
    buffer: RefCell<BufferResource>,
    last_updated: Cell<Option<Instant>>,
}

impl VertexBufferResource {
    #[must_use]
    pub fn new(buffer: BufferResource) -> Self {
        if buffer.is_valid() {
            let mut stats = buffer.get_context().rendering_stats_mut();
            stats.num_vertex_buffers += 1;
            stats.mem_vertex_buffers += buffer.get_allocated_size() as i32;
        }
        VertexBufferResource { buffer: RefCell::new(buffer), last_updated: Cell::new(None) }
    }

    pub fn get_size_in_bytes(&self) -> usize {
        self.buffer.borrow().get_size_in_bytes()
    }
    pub fn get_buffer(&self) -> Ref<'_, BufferResource> {
        self.buffer.borrow()
    }
    /// When the buffer was last updated, `None` for never (C++'s zero
    /// duration), which is earlier than any `Instant`.
    pub fn get_last_updated(&self) -> Option<Instant> {
        self.last_updated.get()
    }
    pub fn set_last_updated(&self, time: Instant) {
        self.last_updated.set(Some(time));
    }
    pub fn update(&self, data: &[u8]) {
        let mut buffer = self.buffer.borrow_mut();
        buffer.update(data, 0);
        if buffer.is_valid() {
            let mut stats = buffer.get_context().rendering_stats_mut();
            stats.vertex_update_bytes = stats.vertex_update_bytes.wrapping_add(data.len());
        }
    }
}

impl Drop for VertexBufferResource {
    fn drop(&mut self) {
        let buffer = self.buffer.borrow();
        if buffer.is_valid() {
            let mut stats = buffer.get_context().rendering_stats_mut();
            stats.num_vertex_buffers -= 1;
            stats.mem_vertex_buffers -= buffer.get_allocated_size() as i32;
        }
    }
}

impl vertex_buffer::VertexBufferResource for VertexBufferResource {}

pub struct IndexBufferResource {
    buffer: RefCell<BufferResource>,
}

impl IndexBufferResource {
    #[must_use]
    pub fn new(buffer: BufferResource) -> Self {
        if buffer.is_valid() {
            let mut stats = buffer.get_context().rendering_stats_mut();
            stats.num_index_buffers += 1;
            stats.mem_index_buffers += buffer.get_allocated_size() as i32;
        }
        IndexBufferResource { buffer: RefCell::new(buffer) }
    }

    pub fn get_size_in_bytes(&self) -> usize {
        self.buffer.borrow().get_size_in_bytes()
    }
    pub fn get_buffer(&self) -> Ref<'_, BufferResource> {
        self.buffer.borrow()
    }
}

impl Drop for IndexBufferResource {
    fn drop(&mut self) {
        let buffer = self.buffer.borrow();
        if buffer.is_valid() {
            let mut stats = buffer.get_context().rendering_stats_mut();
            stats.num_index_buffers -= 1;
            stats.mem_index_buffers -= buffer.get_allocated_size() as i32;
        }
    }
}

impl vertex_buffer::IndexBufferResource for IndexBufferResource {}
