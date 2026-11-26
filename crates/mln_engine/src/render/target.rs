use mln_gfx::gpu::{ColorAttachment, DepthStencilAttachment, Gpu, GpuError, LoadOp, TargetFormats};
use mln_gfx::scissor_rect::ScissorRect;
use mln_gfx::texture2d::{SamplerState, texture_format};
use mln_gfx::types::{TextureChannelDataType, TextureFilterType, TexturePixelType, TextureWrapType};
use mln_gfx::{Rgba, Size};

use crate::draw::context::Context;
use crate::draw::resources::GpuTexture;
use crate::draw::store::SharedTexture;
use crate::render::paint_parameters::{OpenPass, PaintParameters};
use crate::render::pass::RenderPass;

/// A texture that a layer draws into ahead of the frame, which other draws
/// sample, with the depth texture the backend draws it with
/// (`Context::render_target_depth`). The layer that owns it draws it.
pub(crate) struct RenderTarget {
    texture: SharedTexture,
    size: Size,
    /// Made with the first pass, of the texture's size.
    depth: Option<GpuTexture>,
    /// The channel type the texture was asked for.
    type_: TextureChannelDataType,
}

impl RenderTarget {
    /// A render target of `size` with channel type `type_`.
    pub(crate) fn new(context: &mut Context, size: Size, type_: TextureChannelDataType) -> Self {
        RenderTarget { texture: texture(context, size, type_), size, depth: None, type_ }
    }

    /// The texture the target draws into, which draws sample.
    #[must_use]
    pub(crate) fn texture(&self) -> &SharedTexture {
        &self.texture
    }

    /// Draws into a texture of `size` from now on: a texture of another size
    /// replaces the current one, and [`texture`](Self::texture) returns the
    /// new one.
    pub(crate) fn set_size(&mut self, context: &mut Context, size: Size) {
        if self.size != size {
            self.texture = texture(context, size, self.type_);
            self.size = size;
            self.depth = None;
        }
    }

    /// Clears the texture and begins drawing into it through `gpu`, the
    /// context's, in the passes that [`TargetPasses::next`] moves
    /// `parameters` to; [`TargetPasses::end`] ends what this begins. `None`
    /// where the pass didn't begin.
    pub(crate) fn begin(&mut self, gpu: &mut dyn Gpu, parameters: &mut PaintParameters<'_>) -> Option<TargetPasses> {
        let pass = match self.pass(gpu, parameters.context) {
            Ok(pass) => pass,
            Err(error) => {
                parameters.context.fail(format!("Beginning a render target's pass failed: {error}"));
                return None;
            }
        };
        if let Err(error) = parameters.begin_pass(gpu, pass) {
            parameters.context.fail(format!("Beginning a render target's pass failed: {error}"));
            return None;
        }
        let prev_scissor_rect = parameters.scissor_rect;
        parameters.scissor_rect = ScissorRect { x: 0, y: 0, width: pass.size.width, height: pass.size.height };
        Some(TargetPasses { passes: [RenderPass::Opaque, RenderPass::Translucent].into_iter(), prev_scissor_rect })
    }

    /// The pass that clears the texture to opaque black and its depth to 1,
    /// making them through `gpu`, the context's.
    fn pass(&mut self, gpu: &mut dyn Gpu, context: &mut Context) -> Result<OpenPass, GpuError> {
        let color = context.create_texture(gpu, &self.texture)?;
        let format = context.texture(&self.texture).ok_or(GpuError::UnknownId("texture"))?.format();
        let size = self.size;
        if self.depth.is_none()
            && let Some(depth_format) = context.render_target_depth()
        {
            self.depth = Some(context.device().attachment(gpu, "render target depth", size, depth_format)?);
        }
        Ok(OpenPass {
            label: "render target",
            color: Some(ColorAttachment { texture: color, load: LoadOp::Clear(Rgba::new(0.0, 0.0, 0.0, 1.0)) }),
            depth_stencil: self.depth.as_ref().map(|depth| DepthStencilAttachment {
                texture: depth.id(),
                depth: LoadOp::Clear(1.0),
                stencil: LoadOp::Load,
            }),
            size,
            targets: TargetFormats {
                color: Some(format),
                depth_stencil: self.depth.as_ref().and(context.render_target_depth()),
            },
        })
    }
}

/// The color texture of a render target of `size`, which draws draw into and
/// then sample, linear and clamped.
fn texture(context: &mut Context, size: Size, type_: TextureChannelDataType) -> SharedTexture {
    let format = texture_format(TexturePixelType::RGBA, context.render_target_type(type_));
    context.add_texture_with(|texture| {
        texture.set_size(size);
        texture.set_format(format);
        texture.set_render_target();
        texture.set_sampler(SamplerState {
            filter: TextureFilterType::Linear,
            wrap_u: TextureWrapType::Clamp,
            wrap_v: TextureWrapType::Clamp,
            ..SamplerState::default()
        });
    })
}

/// The passes of the draws into a [`RenderTarget`], from
/// [`RenderTarget::begin`] to [`TargetPasses::end`], which every one that
/// begins must reach.
#[must_use = "the draws into a render target end with `TargetPasses::end`"]
pub(crate) struct TargetPasses {
    /// The passes not yet drawn.
    passes: std::array::IntoIter<RenderPass, 2>,
    /// The frame's scissor rect, which `end` restores.
    prev_scissor_rect: ScissorRect,
}

impl TargetPasses {
    /// Moves `parameters` to the next pass, the opaque one, then the
    /// translucent one, both as the frame's first layer; false after the
    /// last.
    pub(crate) fn next(&mut self, parameters: &mut PaintParameters<'_>) -> bool {
        let Some(pass) = self.passes.next() else {
            return false;
        };
        let num_sublayers = PaintParameters::NUM_SUBLAYERS as usize;
        parameters.pass = pass;
        parameters.depth_range_size = 1.0 - (3 * num_sublayers) as f32 * PaintParameters::DEPTH_EPSILON;
        parameters.current_layer = 0;
        true
    }

    /// Ends the draws into the render target, through `gpu`, the context's.
    /// Returns whether the pass ended.
    pub(crate) fn end(self, gpu: &mut dyn Gpu, parameters: &mut PaintParameters<'_>) -> bool {
        let ended = parameters.end_pass(gpu);
        if let Err(error) = &ended {
            parameters.context.fail(format!("Ending a render target's pass failed: {error}"));
        }
        parameters.scissor_rect = self.prev_scissor_rect;
        ended.is_ok()
    }
}
