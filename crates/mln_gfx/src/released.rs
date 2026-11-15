use std::cell::Cell;

use crate::gpu::{BufferId, BufferUsage, Gpu, ResourceId, TextureId};
use crate::rendering_stats::RenderingStats;

/// The resources the renderer dropped, which it destroys in a batch where
/// its context reaches the backend. The rendering stats stop counting a
/// resource when it is released.
#[derive(Default)]
pub struct Released {
    resources: Cell<Vec<ResourceId>>,
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
    pub fn buffer(&self, id: BufferId, usage: BufferUsage, bytes: usize) {
        push(&self.resources, id.into());
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

    pub fn texture(&self, id: TextureId, bytes: usize) {
        push(&self.resources, id.into());
        let mut uncounted = self.uncounted.get();
        uncounted.textures += 1;
        uncounted.texture_bytes += bytes as i32;
        self.uncounted.set(uncounted);
    }

    /// Releases an attachment, which the stats don't count.
    pub fn attachment(&self, id: TextureId) {
        push(&self.resources, id.into());
    }

    /// Releases a resource that the stats don't count, such as bindings or
    /// geometry.
    pub fn resource(&self, id: impl Into<ResourceId>) {
        push(&self.resources, id.into());
    }

    /// Takes the resources released since the last call out of `stats`.
    pub fn uncount(&self, stats: &mut RenderingStats) {
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
    pub fn destroy(&self, gpu: &mut dyn Gpu) {
        for id in self.resources.take() {
            gpu.destroy(id);
        }
    }
}

fn push<T>(list: &Cell<Vec<T>>, item: T) {
    let mut items = list.take();
    items.push(item);
    list.set(items);
}
