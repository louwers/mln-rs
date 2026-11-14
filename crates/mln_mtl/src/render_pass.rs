// Differences from C++:
// - C++ applies only the descriptor's clear color and leaves the depth and
//   stencil attachments at Metal's default load actions; the port also
//   clears them to the descriptor's values when it has them, and loads
//   every attachment it doesn't clear.
// - The pass keeps the attachments' pixel formats, which pipeline states
//   and depth-stencil states are made for (C++ reads them from the
//   draw target's descriptor where it needs them).
// - A cached binding is of a buffer of the backend's `Gpu`, whose writes
//   land before the frame's commands, so the same buffer isn't bound again
//   after it changes (C++ records version 0, so it binds again after any
//   update).

use std::any::Any;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

use mln_gfx::debug_group::DebugGroupTarget;
use mln_gfx::gpu::{BufferId, SamplerId, TextureId};
use mln_gfx::render_pass::{self, RenderPassDescriptor};
use mln_gfx::shaders::shader_defines::{ID_DRAWABLE_RESERVED_FRAGMENT_ONLY_UBO, ID_DRAWABLE_RESERVED_VERTEX_ONLY_UBO};
use mln_gfx::uniform_buffer::UniformBufferArray;
use mln_util::range::Range;
use objc2::Message;
use objc2::rc::Retained;
use objc2_foundation::{NSCopying, NSString};
use objc2_metal::{
    MTLClearColor, MTLCommandBuffer, MTLCommandEncoder, MTLCullMode, MTLLoadAction, MTLPixelFormat,
    MTLRenderCommandEncoder, MTLScissorRect, MTLStoreAction, MTLTexture, MTLViewport, MTLWinding,
};

use crate::command_encoder::CommandEncoder;
use crate::context::Context;
use crate::custom_layer::CustomLayerPass;
use crate::draw_target::{DrawTarget, as_mtl};
use crate::mtl_fwd::{
    MTLBufferPtr, MTLCommandBufferPtr, MTLDepthStencilStatePtr, MTLRenderCommandEncoderPtr, MTLRenderPipelineStatePtr,
    MTLSamplerStatePtr, MTLTexturePtr,
};

const MAX_BINDS: usize = 32;

/// The pixel formats of a pass's attachments.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct AttachmentFormats {
    pub color: MTLPixelFormat,
    pub depth: Option<MTLPixelFormat>,
    pub stencil: Option<MTLPixelFormat>,
}

struct State {
    current_depth_stencil_state: Option<MTLDepthStencilStatePtr>,
    current_pipeline_state: Option<MTLRenderPipelineStatePtr>,
    current_stencil_reference_value: i32,
    /// The buffer of the backend's `Gpu` and the byte offset into it bound
    /// at each index.
    vertex_binds: [Option<(BufferId, usize)>; MAX_BINDS],
    /// The buffer of the backend's `Gpu` bound whole at each index.
    fragment_binds: [Option<BufferId>; MAX_BINDS],
    fragment_texture_bindings: [Option<MTLTexturePtr>; MAX_BINDS],
    fragment_sampler_states: [Option<MTLSamplerStatePtr>; MAX_BINDS],
    current_cull_mode: MTLCullMode,
    current_winding: MTLWinding,
    current_scissor_rect: MTLScissorRect,
    current_depth_range: Option<Range<f32>>,
}

impl Default for State {
    fn default() -> Self {
        State {
            current_depth_stencil_state: None,
            current_pipeline_state: None,
            current_stencil_reference_value: 0,
            vertex_binds: [None; MAX_BINDS],
            fragment_binds: [None; MAX_BINDS],
            fragment_texture_bindings: Default::default(),
            fragment_sampler_states: Default::default(),
            current_cull_mode: MTLCullMode::None,
            current_winding: MTLWinding::Clockwise,
            current_scissor_rect: MTLScissorRect { x: 0, y: 0, width: 0, height: 0 },
            current_depth_range: None,
        }
    }
}

fn same<T: Message + ?Sized>(a: Option<&Retained<T>>, b: Option<&Retained<T>>) -> bool {
    match (a, b) {
        (Some(a), Some(b)) => std::ptr::addr_eq(Retained::as_ptr(a), Retained::as_ptr(b)),
        (None, None) => true,
        _ => false,
    }
}

pub(crate) struct RenderPass {
    context: Rc<Context>,
    command_buffer: Option<MTLCommandBufferPtr>,
    encoder: RefCell<Option<MTLRenderCommandEncoderPtr>>,
    formats: AttachmentFormats,
    width: usize,
    height: usize,
    sample_count: usize,
    state: RefCell<State>,
    debug_groups: Cell<usize>,
}

impl RenderPass {
    pub(crate) fn new(command_encoder: &CommandEncoder, name: &str, descriptor: &RenderPassDescriptor<'_>) -> Self {
        let context = Rc::clone(command_encoder.get_context());
        let mut command_buffer = None;
        let mut encoder = None;
        let mut formats = AttachmentFormats { color: MTLPixelFormat::BGRA8Unorm, depth: None, stencil: None };
        let (mut width, mut height, mut sample_count) = (0, 0, 1);

        if let Some(resource) = as_mtl(descriptor.target).map(DrawTarget::resource) {
            resource.bind();
            if let (Some(buffer), Some(rpd)) = (resource.get_command_buffer(), resource.get_render_pass_descriptor()) {
                let rpd = rpd.copy();
                // SAFETY: index 0 is in the attachment array's bounds.
                let color = unsafe { rpd.colorAttachments().objectAtIndexedSubscript(0) };
                if let Some(c) = descriptor.clear_color {
                    color.setLoadAction(MTLLoadAction::Clear);
                    color.setClearColor(MTLClearColor {
                        red: f64::from(c.r),
                        green: f64::from(c.g),
                        blue: f64::from(c.b),
                        alpha: f64::from(c.a),
                    });
                } else {
                    color.setLoadAction(MTLLoadAction::Load);
                }
                color.setStoreAction(MTLStoreAction::Store);
                if let Some(texture) = color.texture() {
                    formats.color = texture.pixelFormat();
                    width = texture.width();
                    height = texture.height();
                    sample_count = texture.sampleCount();
                }
                let depth = rpd.depthAttachment();
                if let Some(texture) = depth.texture() {
                    formats.depth = Some(texture.pixelFormat());
                    match descriptor.clear_depth {
                        Some(value) => {
                            depth.setLoadAction(MTLLoadAction::Clear);
                            depth.setClearDepth(f64::from(value));
                        }
                        None => depth.setLoadAction(MTLLoadAction::Load),
                    }
                    depth.setStoreAction(MTLStoreAction::Store);
                }
                let stencil = rpd.stencilAttachment();
                if let Some(texture) = stencil.texture() {
                    formats.stencil = Some(texture.pixelFormat());
                    match descriptor.clear_stencil {
                        Some(value) => {
                            stencil.setLoadAction(MTLLoadAction::Clear);
                            stencil.setClearStencil(value as u32);
                        }
                        None => stencil.setLoadAction(MTLLoadAction::Load),
                    }
                    stencil.setStoreAction(MTLStoreAction::Store);
                }
                encoder = buffer.renderCommandEncoderWithDescriptor(&rpd);
                command_buffer = Some(buffer);
            }
        }
        debug_assert!(encoder.is_some());

        let pass = RenderPass {
            context,
            command_buffer,
            encoder: RefCell::new(encoder),
            formats,
            width,
            height,
            sample_count,
            state: RefCell::new(State::default()),
            debug_groups: Cell::new(0),
        };

        // Push the groups already accumulated by the encoder, then the one
        // for this pass's name.
        command_encoder.visit_debug_groups(|group| pass.push_debug_group(group));
        pass.push_debug_group(name);

        mln_gfx::context::Context::perform_cleanup(&*pass.context);
        pass
    }

    pub(crate) fn get_metal_encoder(&self) -> Option<MTLRenderCommandEncoderPtr> {
        self.encoder.borrow().clone()
    }

    pub(crate) fn get_attachment_formats(&self) -> AttachmentFormats {
        self.formats
    }

    pub(crate) fn end_encoding(&self) {
        for _ in 0..self.debug_groups.replace(0) {
            if let Some(encoder) = self.encoder.borrow().as_ref() {
                encoder.popDebugGroup();
            }
        }
        if let Some(encoder) = self.encoder.borrow_mut().take() {
            encoder.endEncoding();
        }
        self.reset_state();
    }

    /// Resets the cached states and bindings, for when they were changed by
    /// calling Metal directly (a custom layer). The state that the cache
    /// starts from is set on the encoder too, since a draw sets only what
    /// differs from the cache.
    pub(crate) fn reset_state(&self) {
        let state = State::default();
        if let Some(encoder) = self.encoder.borrow().as_ref() {
            encoder.setCullMode(state.current_cull_mode);
            encoder.setFrontFacingWinding(state.current_winding);
            encoder.setStencilReferenceValue(state.current_stencil_reference_value as u32);
        }
        *self.state.borrow_mut() = state;
    }

    /// Applies the given depth/stencil state, if different from the current
    /// one.
    pub(crate) fn set_depth_stencil_state(&self, state: Option<&MTLDepthStencilStatePtr>) {
        let mut s = self.state.borrow_mut();
        if !same(state, s.current_depth_stencil_state.as_ref()) {
            s.current_depth_stencil_state = state.cloned();
            if let Some(encoder) = self.encoder.borrow().as_ref() {
                encoder.setDepthStencilState(state.map(|s| &**s));
            }
        }
    }

    /// Applies the given stencil reference value, if different from the
    /// current one.
    pub(crate) fn set_stencil_reference(&self, reference_value: i32) {
        let mut s = self.state.borrow_mut();
        if reference_value != s.current_stencil_reference_value {
            s.current_stencil_reference_value = reference_value;
            if let Some(encoder) = self.encoder.borrow().as_ref() {
                encoder.setStencilReferenceValue(reference_value as u32);
            }
        }
    }

    /// Binds a texture to the fragment location.
    pub(crate) fn set_fragment_texture(&self, texture: Option<&MTLTexturePtr>, location: usize) {
        debug_assert!(location < MAX_BINDS);
        if location >= MAX_BINDS {
            return;
        }
        let mut s = self.state.borrow_mut();
        if !same(texture, s.fragment_texture_bindings[location].as_ref()) {
            s.fragment_texture_bindings[location] = texture.cloned();
            if let Some(encoder) = self.encoder.borrow().as_ref() {
                // SAFETY: the command buffer retains the texture; `location`
                // is below the 31 texture slots of every Metal GPU.
                unsafe { encoder.setFragmentTexture_atIndex(texture.map(|t| &**t), location) };
            }
        }
    }

    /// Sets the sampler for a texture binding.
    pub(crate) fn set_fragment_sampler_state(&self, sampler: Option<&MTLSamplerStatePtr>, location: usize) {
        debug_assert!(location < MAX_BINDS);
        if location >= MAX_BINDS {
            return;
        }
        let mut s = self.state.borrow_mut();
        if !same(sampler, s.fragment_sampler_states[location].as_ref()) {
            s.fragment_sampler_states[location] = sampler.cloned();
            if let Some(encoder) = self.encoder.borrow().as_ref() {
                // SAFETY: as in `set_fragment_texture` (16 sampler slots;
                // shaders use fewer).
                unsafe { encoder.setFragmentSamplerState_atIndex(sampler.map(|s| &**s), location) };
            }
        }
    }

    pub(crate) fn set_render_pipeline_state(&self, pipeline_state: &MTLRenderPipelineStatePtr) {
        let mut s = self.state.borrow_mut();
        if !same(Some(pipeline_state), s.current_pipeline_state.as_ref()) {
            s.current_pipeline_state = Some(pipeline_state.clone());
            if let Some(encoder) = self.encoder.borrow().as_ref() {
                encoder.setRenderPipelineState(pipeline_state);
            }
        }
    }

    /// Binds `buffer`, a buffer of the backend's `Gpu`, from byte `offset`
    /// to the vertex functions at `index`.
    pub(crate) fn bind_vertex_buffer(&self, buffer: BufferId, offset: usize, index: usize) {
        debug_assert!(index < MAX_BINDS);
        let encoder = self.encoder.borrow();
        let Some(encoder) = encoder.as_ref() else {
            return;
        };
        let gpu = self.context.get_backend().gpu().borrow();
        let Some(native) = gpu.native_buffer(buffer) else {
            return;
        };
        if let Some(bound) = self.state.borrow_mut().vertex_binds.get_mut(index) {
            if *bound == Some((buffer, offset)) {
                return;
            }
            *bound = Some((buffer, offset));
        }
        // SAFETY: the command buffer retains the buffer it encodes.
        unsafe { encoder.setVertexBuffer_offset_atIndex(Some(native), offset, index) };
    }

    pub(crate) fn unbind_vertex(&self, index: usize) {
        if index < MAX_BINDS {
            self.state.borrow_mut().vertex_binds[index] = None;
        }
    }

    /// Binds `texture` of the backend's `Gpu`, sampled with `sampler`, to
    /// the fragment functions at `location`.
    pub(crate) fn bind_texture(&self, texture: TextureId, sampler: SamplerId, location: usize) {
        let (texture, sampler) = {
            let gpu = self.context.get_backend().gpu().borrow();
            (gpu.native_texture(texture).map(Message::retain), gpu.native_sampler(sampler).map(Message::retain))
        };
        self.set_fragment_texture(texture.as_ref(), location);
        self.set_fragment_sampler_state(sampler.as_ref(), location);
    }

    /// Binds each buffer of `array` at its id, to the vertex and fragment
    /// functions both except at the reserved vertex-only and fragment-only
    /// ids.
    pub(crate) fn bind_uniform_buffers(&self, array: &UniformBufferArray) {
        let encoder = self.encoder.borrow();
        let Some(encoder) = encoder.as_ref() else {
            return;
        };
        let gpu = self.context.get_backend().gpu().borrow();
        let mut s = self.state.borrow_mut();
        for index in 0..array.allocated_size().min(MAX_BINDS) {
            let Some(buffer) = array.get(index) else {
                continue;
            };
            let Some(native) = gpu.native_buffer(buffer.id()) else {
                continue;
            };
            let bound = Some(buffer.id());
            if index != ID_DRAWABLE_RESERVED_FRAGMENT_ONLY_UBO && s.vertex_binds[index] != Some((buffer.id(), 0)) {
                s.vertex_binds[index] = Some((buffer.id(), 0));
                // SAFETY: the command buffer retains the buffer it encodes.
                unsafe { encoder.setVertexBuffer_offset_atIndex(Some(native), 0, index) };
            }
            if index != ID_DRAWABLE_RESERVED_VERTEX_ONLY_UBO && s.fragment_binds[index] != bound {
                s.fragment_binds[index] = bound;
                // SAFETY: as above.
                unsafe { encoder.setFragmentBuffer_offset_atIndex(Some(native), 0, index) };
            }
        }
    }

    pub(crate) fn unbind_fragment(&self, index: usize) {
        if index < MAX_BINDS {
            self.state.borrow_mut().fragment_binds[index] = None;
        }
    }

    /// Port-only: sets `bytes` for the vertex and fragment functions at
    /// `index`, replacing what is bound there.
    pub(crate) fn set_bytes(&self, bytes: &[u8], index: usize) {
        self.unbind_vertex(index);
        self.unbind_fragment(index);
        if let Some(encoder) = self.encoder.borrow().as_ref() {
            let ptr = std::ptr::NonNull::from(bytes).cast();
            // SAFETY: Metal copies `bytes`; the slot is below Metal's 31.
            unsafe {
                encoder.setVertexBytes_length_atIndex(ptr, bytes.len(), index);
                encoder.setFragmentBytes_length_atIndex(ptr, bytes.len(), index);
            }
        }
    }

    /// Port-only: binds a Metal buffer of the context's own (its empty
    /// vertex buffer) to the vertex functions at `index`.
    pub(crate) fn set_vertex_buffer(&self, buffer: &MTLBufferPtr, offset: usize, index: usize) {
        self.unbind_vertex(index);
        if let Some(encoder) = self.encoder.borrow().as_ref() {
            // SAFETY: the command buffer retains the buffer; the slot is
            // below Metal's 31.
            unsafe { encoder.setVertexBuffer_offset_atIndex(Some(buffer), offset, index) };
        }
    }

    pub(crate) fn set_cull_mode(&self, mode: MTLCullMode) {
        let mut s = self.state.borrow_mut();
        if mode != s.current_cull_mode {
            if let Some(encoder) = self.encoder.borrow().as_ref() {
                encoder.setCullMode(mode);
            }
            s.current_cull_mode = mode;
        }
    }

    pub(crate) fn set_front_facing_winding(&self, winding: MTLWinding) {
        let mut s = self.state.borrow_mut();
        if winding != s.current_winding {
            if let Some(encoder) = self.encoder.borrow().as_ref() {
                encoder.setFrontFacingWinding(winding);
            }
            s.current_winding = winding;
        }
    }

    /// Sets the scissor rectangle, clipped to the attachments.
    pub(crate) fn set_scissor_rect(&self, mut rect: MTLScissorRect) {
        let mut s = self.state.borrow_mut();
        let current = s.current_scissor_rect;
        if rect.x != current.x || rect.y != current.y || rect.width != current.width || rect.height != current.height {
            if rect.width + rect.x > self.width {
                rect.width = self.width.saturating_sub(rect.x);
            }
            if rect.height + rect.y > self.height {
                rect.height = self.height.saturating_sub(rect.y);
            }
            if let Some(encoder) = self.encoder.borrow().as_ref() {
                encoder.setScissorRect(rect);
            }
            s.current_scissor_rect = rect;
        }
    }

    /// Sets the viewport's depth range.
    pub(crate) fn set_depth_range(&self, range: Range<f32>) {
        let mut s = self.state.borrow_mut();
        if Some(range) != s.current_depth_range {
            if let Some(encoder) = self.encoder.borrow().as_ref() {
                encoder.setViewport(MTLViewport {
                    originX: 0.0,
                    originY: 0.0,
                    width: self.width as f64,
                    height: self.height as f64,
                    znear: f64::from(range.min),
                    zfar: f64::from(range.max),
                });
            }
            s.current_depth_range = Some(range);
        }
    }
}

impl Drop for RenderPass {
    fn drop(&mut self) {
        self.end_encoding();
    }
}

impl DebugGroupTarget for RenderPass {
    fn push_debug_group(&self, name: &str) {
        if let Some(encoder) = self.encoder.borrow().as_ref() {
            encoder.pushDebugGroup(&NSString::from_str(name));
            self.debug_groups.set(self.debug_groups.get() + 1);
        }
    }
    fn pop_debug_group(&self) {
        if self.debug_groups.get() == 0 {
            return;
        }
        if let Some(encoder) = self.encoder.borrow().as_ref() {
            encoder.popDebugGroup();
            self.debug_groups.set(self.debug_groups.get() - 1);
        }
    }
}

impl render_pass::RenderPass for RenderPass {
    /// Lends the pass's encoder, then resets the state that the pass knew and
    /// binds the global uniform buffers again.
    fn lend_to_custom_layer(&self, f: &mut dyn FnMut(&mut dyn Any)) -> bool {
        let (Some(encoder), Some(command_buffer)) = (self.get_metal_encoder(), self.command_buffer.clone()) else {
            return false;
        };
        let mut pass =
            CustomLayerPass { encoder, command_buffer, formats: self.formats, sample_count: self.sample_count };
        f(&mut pass);
        drop(pass);
        self.reset_state();
        mln_gfx::context::Context::bind_global_uniform_buffers(&*self.context, self);
        true
    }

    fn add_debug_signpost(&self, name: &str) {
        if let Some(encoder) = self.encoder.borrow().as_ref() {
            encoder.insertDebugSignpost(&NSString::from_str(name));
        }
    }
}
