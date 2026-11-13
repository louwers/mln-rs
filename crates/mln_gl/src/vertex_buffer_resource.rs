use std::cell::Cell;
use std::rc::Weak;

use mln_gfx::context::Context as _;
use mln_gfx::types::BufferUsageType;
use mln_gfx::vertex_buffer;
use mln_util::time::Instant;

use crate::context::Context;
use crate::enums;
use crate::types::BufferID;

pub struct VertexBufferResource {
    context: Weak<Context>,
    buffer: Option<BufferID>,
    byte_size: usize,
    last_updated: Cell<Option<Instant>>,
}

impl VertexBufferResource {
    /// A vertex buffer holding `data`.
    pub fn new(context: &Context, data: &[u8], usage: BufferUsageType) -> Self {
        let buffer = context.create_buffer(glow::ARRAY_BUFFER, data, enums::buffer_usage(usage));
        Self::of(context, buffer, data.len())
    }

    /// A vertex buffer holding `data`, written from the JS array it is in.
    #[cfg(target_family = "wasm")]
    pub fn new_from_host(context: &Context, data: &mln_util::host_bytes::HostBytes, usage: BufferUsageType) -> Self {
        let buffer = context.create_buffer_from_host(glow::ARRAY_BUFFER, data, enums::buffer_usage(usage));
        Self::of(context, buffer, data.len())
    }

    fn of(context: &Context, buffer: Option<BufferID>, byte_size: usize) -> Self {
        {
            let mut stats = context.rendering_stats();
            stats.num_vertex_buffers += 1;
            stats.mem_vertex_buffers += byte_size as i32;
        }
        VertexBufferResource { context: context.weak(), buffer, byte_size, last_updated: Cell::new(None) }
    }

    pub fn get_buffer(&self) -> Option<BufferID> {
        self.buffer
    }

    pub fn get_size_in_bytes(&self) -> usize {
        self.byte_size
    }

    pub fn get_last_updated(&self) -> Option<Instant> {
        self.last_updated.get()
    }

    pub fn set_last_updated(&self, time: Instant) {
        self.last_updated.set(Some(time));
    }

    /// Replaces the start of the buffer with `data`, which fits in it.
    pub fn update(&self, context: &Context, data: &[u8]) {
        debug_assert!(data.len() <= self.byte_size);
        if let Some(buffer) = self.buffer
            && data.len() <= self.byte_size
        {
            context.update_buffer(glow::ARRAY_BUFFER, buffer, data);
            context.rendering_stats().vertex_update_bytes += data.len();
        }
    }
}

impl Drop for VertexBufferResource {
    fn drop(&mut self) {
        if let Some(context) = self.context.upgrade() {
            {
                let mut stats = context.rendering_stats();
                stats.num_vertex_buffers -= 1;
                stats.mem_vertex_buffers -= self.byte_size as i32;
            }
            if let Some(buffer) = self.buffer {
                context.abandon_buffer(buffer, self.byte_size);
            }
        }
    }
}

impl vertex_buffer::VertexBufferResource for VertexBufferResource {}

pub struct IndexBufferResource {
    context: Weak<Context>,
    buffer: Option<BufferID>,
    byte_size: usize,
}

impl IndexBufferResource {
    /// An index buffer holding `data`.
    pub fn new(context: &Context, data: &[u8], usage: BufferUsageType) -> Self {
        let buffer = context.create_buffer(glow::ELEMENT_ARRAY_BUFFER, data, enums::buffer_usage(usage));
        Self::of(context, buffer, data.len())
    }

    /// An index buffer holding `data`, written from the JS array it is in.
    #[cfg(target_family = "wasm")]
    pub fn new_from_host(context: &Context, data: &mln_util::host_bytes::HostBytes, usage: BufferUsageType) -> Self {
        let buffer = context.create_buffer_from_host(glow::ELEMENT_ARRAY_BUFFER, data, enums::buffer_usage(usage));
        Self::of(context, buffer, data.len())
    }

    fn of(context: &Context, buffer: Option<BufferID>, byte_size: usize) -> Self {
        {
            let mut stats = context.rendering_stats();
            stats.num_index_buffers += 1;
            stats.mem_index_buffers += byte_size as i32;
        }
        IndexBufferResource { context: context.weak(), buffer, byte_size }
    }

    #[must_use]
    pub fn get_buffer(&self) -> Option<BufferID> {
        self.buffer
    }
}

impl Drop for IndexBufferResource {
    fn drop(&mut self) {
        if let Some(context) = self.context.upgrade() {
            {
                let mut stats = context.rendering_stats();
                stats.num_index_buffers -= 1;
                stats.mem_index_buffers -= self.byte_size as i32;
            }
            if let Some(buffer) = self.buffer {
                context.abandon_buffer(buffer, self.byte_size);
            }
        }
    }
}

impl vertex_buffer::IndexBufferResource for IndexBufferResource {}
