use std::cell::Cell;

use crate::gpu::{BindGroupId, BufferId, BufferUsage, Gpu, TextureId};
use crate::rendering_stats::RenderingStats;

/// The buffers, textures and bind groups the renderer dropped. A backend destroys them
/// with [`Context::destroy_released`](crate::Context::destroy_released) once
/// it has submitted the frame that last used them, so a resource dropped
/// while a frame is recorded outlives that frame's commands. The rendering
/// stats stop counting a resource when it is released.
#[derive(Default)]
pub struct Released {
    buffers: Cell<Vec<BufferId>>,
    textures: Cell<Vec<TextureId>>,
    bind_groups: Cell<Vec<BindGroupId>>,
    /// What the resources released since the stats were last read took from
    /// them.
    uncounted: Cell<Uncounted>,
}

#[derive(Clone, Copy, Default)]
struct Uncounted {
    vertex_buffers: (i32, i32),
    index_buffers: (i32, i32),
    uniform_buffers: (i32, i32),
    textures: i32,
    texture_bytes: i32,
}

impl Released {
    pub(crate) fn buffer(&self, id: BufferId, usage: BufferUsage, bytes: usize) {
        push(&self.buffers, id);
        let mut uncounted = self.uncounted.get();
        let (count, total) = match usage {
            BufferUsage::Vertex => &mut uncounted.vertex_buffers,
            BufferUsage::Index => &mut uncounted.index_buffers,
            BufferUsage::Uniform | BufferUsage::Storage => &mut uncounted.uniform_buffers,
        };
        *count += 1;
        *total += bytes as i32;
        self.uncounted.set(uncounted);
    }

    pub(crate) fn texture(&self, id: TextureId, bytes: usize) {
        push(&self.textures, id);
        let mut uncounted = self.uncounted.get();
        uncounted.textures += 1;
        uncounted.texture_bytes += bytes as i32;
        self.uncounted.set(uncounted);
    }

    /// Releases an attachment, which the stats don't count.
    pub(crate) fn attachment(&self, id: TextureId) {
        push(&self.textures, id);
    }

    pub(crate) fn bind_group(&self, id: BindGroupId) {
        push(&self.bind_groups, id);
    }

    /// Takes the resources released since the last call out of `stats`.
    pub(crate) fn uncount(&self, stats: &mut RenderingStats) {
        let uncounted = self.uncounted.take();
        for ((count, bytes), (num, mem)) in [
            (uncounted.vertex_buffers, (&mut stats.num_vertex_buffers, &mut stats.mem_vertex_buffers)),
            (uncounted.index_buffers, (&mut stats.num_index_buffers, &mut stats.mem_index_buffers)),
            (uncounted.uniform_buffers, (&mut stats.num_uniform_buffers, &mut stats.mem_uniform_buffers)),
        ] {
            *num -= count;
            *mem -= bytes;
            stats.num_buffers -= count;
            stats.mem_buffers -= bytes;
        }
        stats.num_active_textures -= uncounted.textures;
        stats.mem_textures -= uncounted.texture_bytes;
    }

    /// Destroys the resources released so far.
    pub(crate) fn destroy(&self, gpu: &mut dyn Gpu) {
        for id in self.buffers.take() {
            gpu.destroy_buffer(id);
        }
        for id in self.textures.take() {
            gpu.destroy_texture(id);
        }
        for id in self.bind_groups.take() {
            gpu.destroy_bind_group(id);
        }
    }
}

fn push<T>(list: &Cell<Vec<T>>, item: T) {
    let mut items = list.take();
    items.push(item);
    list.set(items);
}
