use std::cell::Cell;

use mln_gfx::context::Context as _;
use mln_gfx::vertex_buffer;
use mln_util::time::Instant;

use crate::buffer_resource::BufferResource;

pub(crate) struct VertexBufferResource {
    buffer: BufferResource,
    last_updated: Cell<Option<Instant>>,
}

impl VertexBufferResource {
    pub(crate) fn new(buffer: BufferResource) -> Self {
        if buffer.is_valid()
            && let Some(context) = buffer.context()
        {
            let mut stats = context.rendering_stats();
            stats.num_vertex_buffers += 1;
            stats.mem_vertex_buffers += buffer.get_size_in_bytes() as i32;
        }
        VertexBufferResource { buffer, last_updated: Cell::new(None) }
    }

    pub(crate) fn get(&self) -> &BufferResource {
        &self.buffer
    }

    pub(crate) fn get_size_in_bytes(&self) -> usize {
        self.buffer.get_size_in_bytes()
    }

    pub(crate) fn get_last_updated(&self) -> Option<Instant> {
        self.last_updated.get()
    }

    pub(crate) fn set_last_updated(&self, time: Instant) {
        self.last_updated.set(Some(time));
    }
}

impl Drop for VertexBufferResource {
    fn drop(&mut self) {
        if self.buffer.is_valid()
            && let Some(context) = self.buffer.context()
        {
            let mut stats = context.rendering_stats();
            stats.num_vertex_buffers -= 1;
            stats.mem_vertex_buffers -= self.buffer.get_size_in_bytes() as i32;
        }
    }
}

impl vertex_buffer::VertexBufferResource for VertexBufferResource {}

pub(crate) struct IndexBufferResource {
    buffer: BufferResource,
}

impl IndexBufferResource {
    pub(crate) fn new(buffer: BufferResource) -> Self {
        if buffer.is_valid()
            && let Some(context) = buffer.context()
        {
            let mut stats = context.rendering_stats();
            stats.num_index_buffers += 1;
            stats.mem_index_buffers += buffer.get_size_in_bytes() as i32;
        }
        IndexBufferResource { buffer }
    }

    pub(crate) fn get(&self) -> &BufferResource {
        &self.buffer
    }
}

impl Drop for IndexBufferResource {
    fn drop(&mut self) {
        if self.buffer.is_valid()
            && let Some(context) = self.buffer.context()
        {
            let mut stats = context.rendering_stats();
            stats.num_index_buffers -= 1;
            stats.mem_index_buffers -= self.buffer.get_size_in_bytes() as i32;
        }
    }
}

impl vertex_buffer::IndexBufferResource for IndexBufferResource {}
