// Ports include/mln/mtl/render_pass.hpp and src/mln/mtl/render_pass.cpp
//
// Differences from C++:
// - C++ applies only the descriptor's clear color and leaves the depth and
//   stencil attachments at Metal's default load actions; the port also
//   clears them to the descriptor's values when it has them, and loads
//   every attachment it doesn't clear.
// - The pass keeps the attachments' pixel formats, which pipeline states
//   and depth-stencil states are made for (C++ reads them from the
//   draw target's descriptor where it needs them).
// - A cached binding remembers the buffer's version, so the same buffer is
//   bound again only after it changes (C++ records version 0, so it binds
//   again after any update).

use std::any::Any;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

use mln_gfx::debug_group::DebugGroupTarget;
use mln_gfx::render_pass::{self, RenderPassDescriptor};
use mln_util::range::Range;
use objc2::rc::Retained;
use objc2_foundation::{NSCopying, NSString};
use objc2_metal::{
    MTLClearColor, MTLCommandBuffer, MTLCommandEncoder, MTLCullMode, MTLLoadAction, MTLPixelFormat,
    MTLRenderCommandEncoder, MTLScissorRect, MTLStoreAction, MTLTexture, MTLViewport, MTLWinding,
};

use crate::buffer_resource::{BufferResource, VersionType};
use crate::command_encoder::CommandEncoder;
use crate::context::Context;
use crate::draw_target::as_mtl;
use crate::mtl_fwd::{
    MTLBufferPtr, MTLCommandBufferPtr, MTLDepthStencilStatePtr, MTLRenderCommandEncoderPtr, MTLRenderPipelineStatePtr,
    MTLSamplerStatePtr, MTLTexturePtr,
};

const MAX_BINDS: usize = 32;

#[derive(Clone, Copy)]
struct BindInfo {
    buffer_id: u64,
    offset: usize,
    version: VersionType,
}

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
    vertex_binds: [Option<BindInfo>; MAX_BINDS],
    fragment_binds: [Option<BindInfo>; MAX_BINDS],
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

fn same<T: objc2::Message + ?Sized>(a: Option<&Retained<T>>, b: Option<&Retained<T>>) -> bool {
    match (a, b) {
        (Some(a), Some(b)) => std::ptr::addr_eq(Retained::as_ptr(a), Retained::as_ptr(b)),
        (None, None) => true,
        _ => false,
    }
}

pub struct RenderPass {
    context: Rc<Context>,
    command_buffer: Option<MTLCommandBufferPtr>,
    encoder: RefCell<Option<MTLRenderCommandEncoderPtr>>,
    formats: AttachmentFormats,
    width: usize,
    height: usize,
    state: RefCell<State>,
    debug_groups: Cell<usize>,
}

impl RenderPass {
    pub fn new(command_encoder: &CommandEncoder, name: &str, descriptor: &RenderPassDescriptor<'_>) -> Self {
        let context = command_encoder.get_context().clone();
        let mut command_buffer = None;
        let mut encoder = None;
        let mut formats = AttachmentFormats { color: MTLPixelFormat::BGRA8Unorm, depth: None, stencil: None };
        let (mut width, mut height) = (0, 0);

        if let Some(resource) = as_mtl(descriptor.target).map(|target| target.resource()) {
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

    pub fn get_context(&self) -> &Rc<Context> {
        &self.context
    }

    pub fn get_command_buffer(&self) -> Option<&MTLCommandBufferPtr> {
        self.command_buffer.as_ref()
    }

    /// `getMetalEncoder()`
    pub fn get_metal_encoder(&self) -> Option<MTLRenderCommandEncoderPtr> {
        self.encoder.borrow().clone()
    }

    pub fn get_attachment_formats(&self) -> AttachmentFormats {
        self.formats
    }

    pub fn end_encoding(&self) {
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
    /// calling Metal directly (a custom layer).
    pub fn reset_state(&self) {
        *self.state.borrow_mut() = State::default();
    }

    /// Applies the given depth/stencil state, if different from the current
    /// one.
    pub fn set_depth_stencil_state(&self, state: Option<&MTLDepthStencilStatePtr>) {
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
    pub fn set_stencil_reference(&self, reference_value: i32) {
        let mut s = self.state.borrow_mut();
        if reference_value != s.current_stencil_reference_value {
            s.current_stencil_reference_value = reference_value;
            if let Some(encoder) = self.encoder.borrow().as_ref() {
                encoder.setStencilReferenceValue(reference_value as u32);
            }
        }
    }

    /// Binds a texture to the fragment location.
    pub fn set_fragment_texture(&self, texture: Option<&MTLTexturePtr>, location: usize) {
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
    pub fn set_fragment_sampler_state(&self, sampler: Option<&MTLSamplerStatePtr>, location: usize) {
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

    pub fn set_render_pipeline_state(&self, pipeline_state: &MTLRenderPipelineStatePtr) {
        let mut s = self.state.borrow_mut();
        if !same(Some(pipeline_state), s.current_pipeline_state.as_ref()) {
            s.current_pipeline_state = Some(pipeline_state.clone());
            if let Some(encoder) = self.encoder.borrow().as_ref() {
                encoder.setRenderPipelineState(pipeline_state);
            }
        }
    }

    /// Binds `size` bytes of `buffer` from `offset` (the rest of it if
    /// `size` is 0) to the vertex functions at `index`.
    pub fn bind_vertex(&self, buffer: &BufferResource, offset: usize, index: usize, size: usize) {
        let actual_size = if size != 0 { size } else { buffer.get_size_in_bytes() - offset };
        debug_assert!(actual_size <= buffer.get_size_in_bytes());
        debug_assert!(index < MAX_BINDS);
        let encoder = self.encoder.borrow();
        let Some(encoder) = encoder.as_ref() else {
            return;
        };
        if index < MAX_BINDS {
            let mut s = self.state.borrow_mut();
            if let Some(bind) = s.vertex_binds[index].as_mut()
                && bind.buffer_id == buffer.id()
                && !buffer.need_rebind(bind.version)
            {
                // The same buffer is bound here; move only the offset.
                if bind.offset != offset {
                    buffer.update_vertex_bind_offset(encoder, offset, index, actual_size);
                    bind.offset = offset;
                }
                return;
            }
            s.vertex_binds[index] = Some(BindInfo { buffer_id: buffer.id(), offset, version: buffer.get_version() });
        }
        buffer.bind_vertex(encoder, offset, index, actual_size);
    }

    pub fn unbind_vertex(&self, index: usize) {
        if index < MAX_BINDS {
            self.state.borrow_mut().vertex_binds[index] = None;
        }
    }

    /// `bind_vertex` for the fragment functions.
    pub fn bind_fragment(&self, buffer: &BufferResource, offset: usize, index: usize, size: usize) {
        let actual_size = if size != 0 { size } else { buffer.get_size_in_bytes() - offset };
        debug_assert!(actual_size <= buffer.get_size_in_bytes());
        debug_assert!(index < MAX_BINDS);
        let encoder = self.encoder.borrow();
        let Some(encoder) = encoder.as_ref() else {
            return;
        };
        if index < MAX_BINDS {
            let mut s = self.state.borrow_mut();
            if let Some(bind) = s.fragment_binds[index].as_mut()
                && bind.buffer_id == buffer.id()
                && !buffer.need_rebind(bind.version)
            {
                if bind.offset != offset {
                    buffer.update_fragment_bind_offset(encoder, offset, index, actual_size);
                    bind.offset = offset;
                }
                return;
            }
            s.fragment_binds[index] = Some(BindInfo { buffer_id: buffer.id(), offset, version: buffer.get_version() });
        }
        buffer.bind_fragment(encoder, offset, index, actual_size);
    }

    pub fn unbind_fragment(&self, index: usize) {
        if index < MAX_BINDS {
            self.state.borrow_mut().fragment_binds[index] = None;
        }
    }

    /// Port-only: sets `bytes` for the vertex and fragment functions at
    /// `index`, replacing what is bound there.
    pub fn set_bytes(&self, bytes: &[u8], index: usize) {
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

    /// Port-only: `set_bytes` for the vertex functions only.
    pub fn set_vertex_bytes(&self, bytes: &[u8], index: usize) {
        self.unbind_vertex(index);
        if let Some(encoder) = self.encoder.borrow().as_ref() {
            // SAFETY: as in `set_bytes`.
            unsafe { encoder.setVertexBytes_length_atIndex(std::ptr::NonNull::from(bytes).cast(), bytes.len(), index) };
        }
    }

    /// Port-only: binds a Metal buffer that has no `BufferResource` (the
    /// context's cached buffers) to the vertex functions at `index`.
    pub fn set_vertex_buffer(&self, buffer: &MTLBufferPtr, offset: usize, index: usize) {
        self.unbind_vertex(index);
        if let Some(encoder) = self.encoder.borrow().as_ref() {
            // SAFETY: the command buffer retains the buffer; the slot is
            // below Metal's 31.
            unsafe { encoder.setVertexBuffer_offset_atIndex(Some(buffer), offset, index) };
        }
    }

    pub fn set_cull_mode(&self, mode: MTLCullMode) {
        let mut s = self.state.borrow_mut();
        if mode != s.current_cull_mode {
            if let Some(encoder) = self.encoder.borrow().as_ref() {
                encoder.setCullMode(mode);
            }
            s.current_cull_mode = mode;
        }
    }

    pub fn set_front_facing_winding(&self, winding: MTLWinding) {
        let mut s = self.state.borrow_mut();
        if winding != s.current_winding {
            if let Some(encoder) = self.encoder.borrow().as_ref() {
                encoder.setFrontFacingWinding(winding);
            }
            s.current_winding = winding;
        }
    }

    /// Sets the scissor rectangle, clipped to the attachments.
    pub fn set_scissor_rect(&self, mut rect: MTLScissorRect) {
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
    pub fn set_depth_range(&self, range: Range<f32>) {
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
    fn add_debug_signpost(&self, name: &str) {
        if let Some(encoder) = self.encoder.borrow().as_ref() {
            encoder.insertDebugSignpost(&NSString::from_str(name));
        }
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}
