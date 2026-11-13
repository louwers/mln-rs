// `UniformBufferArrayGL` only adds `bind` to the gfx array; the port keeps
// the gfx struct, and the context binds what a drawable's shader reads
// (`Context::bind_uniform_blocks`).
//
// Port-only: the buffers the renderer makes for a layer with an element per
// drawable (C++: `MLN_UBO_CONSOLIDATION`, shader storage buffers indexed by
// `GlobalUBOIndex`, which the C++ OpenGL backend doesn't build). OpenGL ES
// 3.0 has no storage buffers, so such a buffer is a uniform buffer, and each
// drawable binds its element as the uniform block (`glBindBufferRange`). GL
// only binds offsets that are multiples of
// `GL_UNIFORM_BUFFER_OFFSET_ALIGNMENT`, which the element size need not be,
// so the bytes are kept here and uploaded with the elements that far apart.
// The element size comes from the shader that reads the buffer, so the
// upload waits for the first drawable that binds it.
//
// Not ported: the buffer allocator's shared buffers. Each uniform buffer is
// a GL buffer of its own.

use std::cell::{Cell, RefCell};
use std::rc::Weak;

use glow::HasContext;
use mln_gfx::context::Context as _;
use mln_gfx::uniform_buffer;
use mln_util::logging::{Event, Log};

use crate::context::Context;
use crate::types::BufferID;

/// The bytes of a layer's buffer of an element per drawable, and how they
/// were last uploaded.
struct Elements {
    data: Vec<u8>,
    /// Whether `data` changed since the upload.
    dirty: bool,
    /// The element size `data` was uploaded for; 0 before the first upload.
    element_size: usize,
    /// The size of the GL buffer's storage.
    uploaded_size: usize,
}

pub(crate) struct UniformBuffer {
    context: Weak<Context>,
    size: usize,
    /// The GL buffer; of a layer's buffer of elements, made by its first
    /// upload.
    buffer: Cell<Option<BufferID>>,
    /// A layer's buffer of an element per drawable.
    elements: Option<RefCell<Elements>>,
}

impl UniformBuffer {
    pub(crate) fn new(context: &Context, data: &[u8], elements: bool) -> Self {
        {
            let mut stats = context.rendering_stats();
            stats.num_uniform_buffers += 1;
            stats.mem_uniform_buffers += data.len() as i32;
        }
        if elements {
            // Counted as the buffer the renderer made, whatever storage its
            // elements get.
            context.count_buffer(data.len());
            return UniformBuffer {
                context: context.weak(),
                size: data.len(),
                buffer: Cell::new(None),
                elements: Some(RefCell::new(Elements {
                    data: data.to_vec(),
                    dirty: true,
                    element_size: 0,
                    uploaded_size: 0,
                })),
            };
        }
        let buffer = context.create_buffer(glow::UNIFORM_BUFFER, data, glow::DYNAMIC_DRAW);
        UniformBuffer { context: context.weak(), size: data.len(), buffer: Cell::new(buffer), elements: None }
    }

    /// The buffer range to bind for a uniform block of `block_size` bytes:
    /// the whole buffer, or the element `index` of a buffer of
    /// `element_size` byte elements.
    pub(crate) fn range(
        &self,
        context: &Context,
        index: u32,
        element_size: usize,
        block_size: usize,
    ) -> Option<(BufferID, usize, usize)> {
        let Some(elements) = &self.elements else {
            debug_assert!(block_size <= self.size, "uniform buffer smaller than its block");
            return Some((self.buffer.get()?, 0, self.size));
        };
        let mut elements = elements.borrow_mut();
        if element_size == 0 || elements.data.len() < element_size {
            debug_assert!(false, "a shader reads a buffer of elements as one block");
            return None;
        }
        let count = elements.data.len() / element_size;
        let stride = element_size.next_multiple_of(context.uniform_buffer_offset_alignment());
        if elements.dirty || elements.element_size != element_size {
            self.upload(context, &mut elements, element_size, stride, count);
        }
        debug_assert!((index as usize) < count, "drawable UBO index out of range");
        let index = (index as usize).min(count - 1);
        Some((self.buffer.get()?, index * stride, block_size.max(1)))
    }

    /// Uploads the elements `stride` bytes apart.
    fn upload(&self, context: &Context, elements: &mut Elements, element_size: usize, stride: usize, count: usize) {
        let spaced;
        let bytes: &[u8] = if stride == element_size {
            &elements.data[..count * element_size]
        } else {
            let mut data = vec![0u8; count * stride];
            for (target, source) in data.chunks_exact_mut(stride).zip(elements.data.chunks_exact(element_size)) {
                target[..element_size].copy_from_slice(source);
            }
            spaced = data;
            &spaced
        };
        match self.buffer.get() {
            Some(buffer) if elements.uploaded_size == bytes.len() => {
                context.update_buffer(glow::UNIFORM_BUFFER, buffer, bytes);
            }
            Some(buffer) => {
                // SAFETY: the buffer is alive; its storage is replaced.
                unsafe {
                    context.gl().bind_buffer(glow::UNIFORM_BUFFER, Some(buffer));
                    context.gl().buffer_data_u8_slice(glow::UNIFORM_BUFFER, bytes, glow::DYNAMIC_DRAW);
                }
            }
            None => {
                self.buffer.set(context.create_uncounted_buffer(glow::UNIFORM_BUFFER, bytes, glow::DYNAMIC_DRAW));
            }
        }
        elements.dirty = false;
        elements.element_size = element_size;
        elements.uploaded_size = bytes.len();
    }
}

impl Drop for UniformBuffer {
    fn drop(&mut self) {
        if let Some(context) = self.context.upgrade() {
            {
                let mut stats = context.rendering_stats();
                stats.num_uniform_buffers -= 1;
                stats.mem_uniform_buffers -= self.size as i32;
            }
            match (self.buffer.get(), &self.elements) {
                (buffer, Some(_)) => {
                    context.uncount_buffer(self.size);
                    if let Some(buffer) = buffer {
                        context.abandon_uncounted_buffer(buffer);
                    }
                }
                (Some(buffer), None) => context.abandon_buffer(buffer, self.size),
                (None, None) => {}
            }
        }
    }
}

impl uniform_buffer::UniformBuffer for UniformBuffer {
    fn update(&self, data: &[u8]) {
        debug_assert!(data.len() <= self.size);
        if data.len() > self.size {
            Log::error(
                Event::General,
                format!("Mismatched size given to UBO update, expected max {}, got {}", self.size, data.len()),
            );
            return;
        }
        let Some(context) = self.context.upgrade() else {
            return;
        };
        {
            let mut stats = context.rendering_stats();
            stats.num_uniform_updates += 1;
            stats.uniform_update_bytes += data.len();
        }
        match &self.elements {
            Some(elements) => {
                let mut elements = elements.borrow_mut();
                elements.data[..data.len()].copy_from_slice(data);
                elements.dirty = true;
            }
            None => {
                if let Some(buffer) = self.buffer.get() {
                    context.update_buffer(glow::UNIFORM_BUFFER, buffer, data);
                }
            }
        }
    }

    fn get_size(&self) -> usize {
        self.size
    }
}
