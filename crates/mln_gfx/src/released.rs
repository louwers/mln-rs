use std::cell::Cell;

use crate::gpu::{BufferId, Gpu, TextureId};
use crate::rendering_stats::RenderingStats;

/// The buffers and textures the renderer dropped. A backend destroys them
/// with [`Context::destroy_released`](crate::Context::destroy_released) once
/// it has submitted the frame that last used them, so a resource dropped
/// while a frame is recorded outlives that frame's commands. The rendering
/// stats stop counting a resource when it is released.
#[derive(Default)]
pub struct Released {
    buffers: Cell<Vec<BufferId>>,
    textures: Cell<Vec<TextureId>>,
    /// What the resources released since the stats were last read took from
    /// them.
    uncounted: Cell<Uncounted>,
}

#[derive(Clone, Copy, Default)]
struct Uncounted {
    buffers: i32,
    buffer_bytes: i32,
    textures: i32,
    texture_bytes: i32,
}

impl Released {
    pub(crate) fn buffer(&self, id: BufferId, bytes: usize) {
        push(&self.buffers, id);
        let mut uncounted = self.uncounted.get();
        uncounted.buffers += 1;
        uncounted.buffer_bytes += bytes as i32;
        self.uncounted.set(uncounted);
    }

    pub(crate) fn texture(&self, id: TextureId, bytes: usize) {
        push(&self.textures, id);
        let mut uncounted = self.uncounted.get();
        uncounted.textures += 1;
        uncounted.texture_bytes += bytes as i32;
        self.uncounted.set(uncounted);
    }

    /// Takes the resources released since the last call out of `stats`.
    pub(crate) fn uncount(&self, stats: &mut RenderingStats) {
        let uncounted = self.uncounted.take();
        stats.num_buffers -= uncounted.buffers;
        stats.mem_buffers -= uncounted.buffer_bytes;
        stats.num_uniform_buffers -= uncounted.buffers;
        stats.mem_uniform_buffers -= uncounted.buffer_bytes;
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
    }
}

fn push<T>(list: &Cell<Vec<T>>, item: T) {
    let mut items = list.take();
    items.push(item);
    list.set(items);
}
