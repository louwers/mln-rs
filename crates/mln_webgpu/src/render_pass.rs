// Ports include/mln/webgpu/render_pass.hpp and src/mln/webgpu/render_pass.cpp
//
// The C++ pass keeps the descriptor (with a `Renderable&`); drawables read
// `getDescriptor().renderable` for its size (and pass it, unused, to
// `getRenderPipeline`). The port keeps the size and an identity of the
// draw target instead of the reference.
//
// Drawables encode through a `StateFilter`, which drops the commands that set
// state the pass already has, as the Metal backend's render pass does.

use std::any::Any;
use std::cell::{Cell, Ref, RefCell, RefMut};
use std::ops::Range;
use std::rc::Rc;
use std::sync::OnceLock;

use mln_gfx::context::Context as _;
use mln_gfx::debug_group::DebugGroupTarget;
use mln_gfx::render_pass::{self, RenderPassDescriptor};
use mln_gfx::uniform_buffer::UniformBufferArray;
use mln_util::size::Size;

use crate::command_encoder::CommandEncoder;
use crate::context::Context;
use crate::draw_target::as_webgpu;

pub struct RenderPass {
    context: Rc<Context>,
    renderable_size: Size,
    /// Address of the renderable's resource (`clipMaskRenderable` compares it).
    renderable_id: usize,
    encoder: RefCell<Option<wgpu::RenderPass<'static>>>,
    /// The state `encoder` has, as far as [`StateFilter`] knows it.
    state: RefCell<PassState>,
    global_uniform_buffers_bound: Cell<bool>,
    previous_color_format: Option<wgpu::TextureFormat>,
    previous_depth_stencil_format: Option<wgpu::TextureFormat>,
    color_format_updated: bool,
    depth_format_updated: bool,
}

fn debug_groups_enabled() -> bool {
    // Debug groups on render pass encoders can cause issues in some Dawn
    // versions; enable with an environment variable if needed.
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var_os("WEBGPU_ENABLE_DEBUG_GROUPS").is_some())
}

impl RenderPass {
    pub fn new(command_encoder: &CommandEncoder, name: &str, descriptor: &RenderPassDescriptor<'_>) -> Self {
        let context = command_encoder.get_context().clone();
        let backend = context.get_backend().clone();
        let mut pass = RenderPass {
            context: context.clone(),
            renderable_size: descriptor.target.size(),
            renderable_id: 0,
            encoder: RefCell::new(None),
            state: RefCell::new(PassState::default()),
            global_uniform_buffers_bound: Cell::new(false),
            previous_color_format: backend.get_color_format(),
            previous_depth_stencil_format: backend.get_depth_stencil_format(),
            color_format_updated: false,
            depth_format_updated: false,
        };

        let mut encoder_guard = command_encoder.get_encoder();
        let Some(command_encoder) = encoder_guard.as_mut() else {
            eprintln!("WebGPU: Command encoder unavailable");
            return pass;
        };

        // Access the draw target associated with this pass
        let Some(target) = as_webgpu(descriptor.target) else {
            eprintln!("WebGPU: The pass's draw target is not a WebGPU one");
            return pass;
        };
        pass.renderable_id = std::ptr::from_ref(target) as usize;
        let renderable_resource = target.resource();
        renderable_resource.bind();

        // Resolve the color attachment view.
        let Some(color_view) = renderable_resource.get_color_texture_view() else {
            eprintln!("WebGPU: Failed to acquire color attachment view");
            return pass;
        };
        if let Some(color_format) = renderable_resource.get_color_texture_format() {
            backend.set_color_format(Some(color_format));
            pass.color_format_updated = true;
        }

        let color_ops = wgpu::Operations {
            load: match descriptor.clear_color {
                Some(c) => wgpu::LoadOp::Clear(wgpu::Color {
                    r: f64::from(c.r),
                    g: f64::from(c.g),
                    b: f64::from(c.b),
                    a: f64::from(c.a),
                }),
                None => wgpu::LoadOp::Load,
            },
            store: wgpu::StoreOp::Store,
        };

        let depth_view = renderable_resource.get_depth_stencil_texture_view();
        if depth_view.is_some()
            && let Some(depth_format) = renderable_resource.get_depth_stencil_texture_format()
        {
            backend.set_depth_stencil_format(Some(depth_format));
            pass.depth_format_updated = true;
        }
        let depth_stencil_attachment = depth_view.as_ref().map(|view| wgpu::RenderPassDepthStencilAttachment {
            view,
            depth_ops: Some(wgpu::Operations {
                load: match descriptor.clear_depth {
                    Some(d) => wgpu::LoadOp::Clear(d),
                    None => wgpu::LoadOp::Load,
                },
                store: wgpu::StoreOp::Store,
            }),
            // C++ uses Undefined load/store ops without a stencil clear; in
            // wgpu `None` (read-only stencil aspect) is the equivalent.
            stencil_ops: descriptor
                .clear_stencil
                .map(|s| wgpu::Operations { load: wgpu::LoadOp::Clear(s as u32), store: wgpu::StoreOp::Store }),
        });

        let encoder = command_encoder
            .begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some(name),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &color_view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: color_ops,
                })],
                depth_stencil_attachment,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            })
            .forget_lifetime();
        drop(encoder_guard);

        context.perform_cleanup();
        *pass.encoder.borrow_mut() = Some(encoder);
        if let Some(mut encoder) = pass.state_filter() {
            let size = pass.renderable_size;
            encoder.set_viewport(0.0, 0.0, size.width as f32, size.height as f32, 0.0, 1.0);
            encoder.set_scissor_rect(0, 0, size.width, size.height);
        }
        pass
    }

    /// `getEncoder()`: the wgpu render pass, `None` if it failed to begin.
    /// Its user may set any state, so [`state_filter`](Self::state_filter)
    /// forgets the state it knew.
    pub fn get_encoder(&self) -> RefMut<'_, Option<wgpu::RenderPass<'static>>> {
        *self.state.borrow_mut() = PassState::default();
        self.encoder.borrow_mut()
    }

    /// The wgpu render pass behind a filter that drops the commands setting
    /// state the pass already has, `None` if the pass failed to begin.
    pub fn state_filter(&self) -> Option<StateFilter<'_>> {
        let encoder = RefMut::filter_map(self.encoder.borrow_mut(), Option::as_mut).ok()?;
        Some(StateFilter { encoder, state: self.state.borrow_mut() })
    }

    pub fn has_encoder(&self) -> bool {
        self.encoder.borrow().is_some()
    }

    /// `getDescriptor().renderable.getSize()`
    pub fn get_renderable_size(&self) -> Size {
        self.renderable_size
    }

    pub fn get_renderable_id(&self) -> usize {
        self.renderable_id
    }

    pub fn get_context(&self) -> &Rc<Context> {
        &self.context
    }

    /// `setGlobalUniformBuffers(ptr)`: bound or not; the array is the
    /// context's (`Context::bindGlobalUniformBuffers`).
    pub fn set_global_uniform_buffers(&self, bound: bool) {
        self.global_uniform_buffers_bound.set(bound);
    }

    pub fn get_global_uniform_buffers(&self) -> Option<Ref<'_, UniformBufferArray>> {
        self.global_uniform_buffers_bound.get().then(|| self.context.get_global_uniform_buffers())
    }
}

impl Drop for RenderPass {
    fn drop(&mut self) {
        // End the render pass
        self.encoder.borrow_mut().take();
        let backend = self.context.get_backend();
        if self.color_format_updated {
            backend.set_color_format(self.previous_color_format);
        }
        if self.depth_format_updated {
            backend.set_depth_stencil_format(self.previous_depth_stencil_format);
        }
    }
}

impl DebugGroupTarget for RenderPass {
    fn push_debug_group(&self, name: &str) {
        if debug_groups_enabled()
            && let Some(encoder) = self.encoder.borrow_mut().as_mut()
        {
            encoder.push_debug_group(name);
        }
    }

    fn pop_debug_group(&self) {
        if debug_groups_enabled()
            && let Some(encoder) = self.encoder.borrow_mut().as_mut()
        {
            encoder.pop_debug_group();
        }
    }
}

impl render_pass::RenderPass for RenderPass {
    fn add_debug_signpost(&self, name: &str) {
        if let Some(encoder) = self.encoder.borrow_mut().as_mut() {
            encoder.insert_debug_marker(name);
        }
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// The vertex buffer and bind group slots whose state [`PassState`] tracks.
/// Commands for higher slots pass through unfiltered.
const TRACKED_SLOTS: usize = 8;

/// The state a render pass has been given, `None` where it is unknown. It
/// holds handles rather than addresses, so a resource dropped mid-pass can't
/// be mistaken for a new one at the same address.
#[derive(Default)]
struct PassState {
    pipeline: Option<wgpu::RenderPipeline>,
    /// x, y, width, height, min and max depth, as bits.
    viewport: Option<[u32; 6]>,
    scissor_rect: Option<[u32; 4]>,
    stencil_reference: Option<u32>,
    /// The buffer and byte range bound at each slot.
    vertex_buffers: [Option<(wgpu::Buffer, Range<u64>)>; TRACKED_SLOTS],
    index_buffer: Option<(wgpu::Buffer, Range<u64>, wgpu::IndexFormat)>,
    bind_groups: [Option<wgpu::BindGroup>; TRACKED_SLOTS],
}

/// Whether a command must set the state `is_set` accepts, which is when
/// `tracked` holds other or unknown state, or is `None` (untracked). Records
/// the state `make` returns when it does.
fn changes<T>(tracked: Option<&mut Option<T>>, is_set: impl FnOnce(&T) -> bool, make: impl FnOnce() -> T) -> bool {
    let Some(tracked) = tracked else {
        return true;
    };
    if tracked.as_ref().is_some_and(is_set) {
        return false;
    }
    *tracked = Some(make());
    true
}

/// A render pass encoder that drops the commands setting state the pass
/// already has; WebGPU keeps all of it from one draw to the next. What it
/// knows is the pass's, so it carries over to the pass's next filter.
pub struct StateFilter<'a> {
    encoder: RefMut<'a, wgpu::RenderPass<'static>>,
    state: RefMut<'a, PassState>,
}

impl StateFilter<'_> {
    pub fn set_pipeline(&mut self, pipeline: &wgpu::RenderPipeline) {
        if changes(Some(&mut self.state.pipeline), |set| set == pipeline, || pipeline.clone()) {
            self.encoder.set_pipeline(pipeline);
        }
    }

    pub fn set_viewport(&mut self, x: f32, y: f32, width: f32, height: f32, min_depth: f32, max_depth: f32) {
        let viewport = [x, y, width, height, min_depth, max_depth].map(f32::to_bits);
        if changes(Some(&mut self.state.viewport), |set| *set == viewport, || viewport) {
            self.encoder.set_viewport(x, y, width, height, min_depth, max_depth);
        }
    }

    pub fn set_scissor_rect(&mut self, x: u32, y: u32, width: u32, height: u32) {
        let rect = [x, y, width, height];
        if changes(Some(&mut self.state.scissor_rect), |set| *set == rect, || rect) {
            self.encoder.set_scissor_rect(x, y, width, height);
        }
    }

    pub fn set_stencil_reference(&mut self, reference: u32) {
        if changes(Some(&mut self.state.stencil_reference), |set| *set == reference, || reference) {
            self.encoder.set_stencil_reference(reference);
        }
    }

    /// Binds the bytes `range` of `buffer` to vertex buffer `slot`.
    pub fn set_vertex_buffer(&mut self, slot: u32, buffer: &wgpu::Buffer, range: Range<u64>) {
        let tracked = self.state.vertex_buffers.get_mut(slot as usize);
        if changes(tracked, |(b, r)| b == buffer && *r == range, || (buffer.clone(), range.clone())) {
            self.encoder.set_vertex_buffer(slot, buffer.slice(range));
        }
    }

    /// Binds the bytes `range` of `buffer` as the index buffer.
    pub fn set_index_buffer(&mut self, buffer: &wgpu::Buffer, range: Range<u64>, format: wgpu::IndexFormat) {
        let tracked = Some(&mut self.state.index_buffer);
        if changes(
            tracked,
            |(b, r, f)| b == buffer && *r == range && *f == format,
            || (buffer.clone(), range.clone(), format),
        ) {
            self.encoder.set_index_buffer(buffer.slice(range), format);
        }
    }

    /// Binds `bind_group`, which has no dynamic offsets, at `index`.
    pub fn set_bind_group(&mut self, index: u32, bind_group: &wgpu::BindGroup) {
        let tracked = self.state.bind_groups.get_mut(index as usize);
        if changes(tracked, |set| set == bind_group, || bind_group.clone()) {
            self.encoder.set_bind_group(index, bind_group, &[]);
        }
    }

    pub fn draw_indexed(&mut self, indices: Range<u32>, base_vertex: i32, instances: Range<u32>) {
        self.encoder.draw_indexed(indices, base_vertex, instances);
    }
}

#[cfg(test)]
mod tests {
    use super::changes;

    #[test]
    fn only_new_or_untracked_state_changes() {
        let mut tracked = None;
        assert!(changes(Some(&mut tracked), |set| *set == 1, || 1), "unknown state");
        assert!(!changes(Some(&mut tracked), |set| *set == 1, || 1), "the state already set");
        assert!(changes(Some(&mut tracked), |set| *set == 2, || 2), "other state");
        assert_eq!(tracked, Some(2));
        assert!(changes(None, |set: &i32| *set == 2, || 2), "an untracked slot");
    }
}
