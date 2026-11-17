use std::rc::Rc;

use mln_gfx::gpu::{ColorAttachment, DepthStencilAttachment, Gpu, GpuError, LoadOp, TargetFormats};
use mln_gfx::scissor_rect::ScissorRect;
use mln_gfx::texture2d::{SamplerState, texture_format};
use mln_gfx::types::{TextureChannelDataType, TextureFilterType, TexturePixelType, TextureWrapType};
use mln_util::Color;
use mln_util::logging::{Event, Log};
use mln_util::size::Size;

use crate::gfx::attachment::Attachment;
use crate::gfx::context::Context;
use crate::gfx::texture2d::Texture2DPtr;
use crate::paint_parameters::{OpenPass, PaintParameters};
use crate::render_pass::RenderPass;

/// A texture that a layer draws into ahead of the frame, which other draws
/// sample, with the depth texture the backend draws it with
/// (`Context::render_target_depth`). The layer that owns it draws it.
pub(crate) struct RenderTarget {
    texture: Texture2DPtr,
    /// Made with the first pass, of the texture's size.
    depth: Option<Attachment>,
    /// The channel type the texture was asked for.
    type_: TextureChannelDataType,
}

impl RenderTarget {
    /// A render target of `size` with channel type `type_`.
    pub(crate) fn new(context: &Context, size: Size, type_: TextureChannelDataType) -> Self {
        RenderTarget { texture: texture(context, size, type_), depth: None, type_ }
    }

    /// Get the render target texture
    #[must_use]
    pub(crate) fn get_texture(&self) -> Texture2DPtr {
        Rc::clone(&self.texture)
    }

    /// Draws into a texture of `size` from now on: a texture of another size
    /// replaces the current one, and [`get_texture`](Self::get_texture)
    /// returns the new one.
    pub(crate) fn set_size(&mut self, context: &Context, size: Size) {
        if self.texture.borrow().size() != size {
            self.texture = texture(context, size, self.type_);
            self.depth = None;
        }
    }

    /// Clears the texture and draws into it with `draw`, which draws what of
    /// the target is in `parameters`' pass into the open pass of the `Gpu`,
    /// the context's. It draws in the opaque pass, then the translucent one,
    /// both as the frame's first layer.
    pub(crate) fn render(
        &mut self,
        parameters: &mut PaintParameters<'_>,
        mut draw: impl FnMut(&mut dyn Gpu, &mut PaintParameters<'_>),
    ) {
        let num_sublayers = PaintParameters::NUM_SUBLAYERS as usize;
        let depth_epsilon = PaintParameters::DEPTH_EPSILON;

        let pass = match self.pass(parameters.context) {
            Ok(pass) => pass,
            Err(error) => {
                Log::error(Event::Render, format!("Beginning a render target's pass failed: {error}"));
                return;
            }
        };
        let context = parameters.context;
        let mut gpu = context.gpu();
        if let Err(error) = parameters.begin_pass(&mut *gpu, pass) {
            Log::error(Event::Render, format!("Beginning a render target's pass failed: {error}"));
            return;
        }

        let prev_scissor_rect = parameters.scissor_rect;
        parameters.scissor_rect = ScissorRect { x: 0, y: 0, width: pass.size.width, height: pass.size.height };

        let depth_range_size = 1.0 - (3 * num_sublayers) as f32 * depth_epsilon;
        for pass in [RenderPass::Opaque, RenderPass::Translucent] {
            parameters.pass = pass;
            parameters.depth_range_size = depth_range_size;
            parameters.current_layer = 0;
            draw(&mut *gpu, parameters);
        }

        if let Err(error) = parameters.end_pass(&mut *gpu) {
            Log::error(Event::Render, format!("Ending a render target's pass failed: {error}"));
        }
        parameters.scissor_rect = prev_scissor_rect;
    }

    /// The pass that clears the texture to opaque black and its depth to 1.
    fn pass(&mut self, context: &Context) -> Result<OpenPass, GpuError> {
        let mut texture = self.texture.borrow_mut();
        let (color, size, format) = (texture.create(context)?, texture.size(), texture.format());
        drop(texture);
        if self.depth.is_none()
            && let Some(depth_format) = context.render_target_depth()
        {
            self.depth = Some(Attachment::new(context, "render target depth", size, depth_format)?);
        }
        Ok(OpenPass {
            label: "render target",
            color: Some(ColorAttachment { texture: color, load: LoadOp::Clear(Color::new(0.0, 0.0, 0.0, 1.0)) }),
            depth_stencil: self.depth.as_ref().map(|depth| DepthStencilAttachment {
                texture: depth.id(),
                depth: LoadOp::Clear(1.0),
                stencil: LoadOp::Load,
            }),
            size,
            targets: TargetFormats { color: Some(format), depth_stencil: self.depth.as_ref().map(Attachment::format) },
        })
    }
}

/// The color texture of a render target of `size`, which draws draw into and
/// then sample, linear and clamped.
fn texture(context: &Context, size: Size, type_: TextureChannelDataType) -> Texture2DPtr {
    let texture = context.create_texture_2d();
    {
        let mut texture = texture.borrow_mut();
        texture.set_size(size);
        texture.set_format(texture_format(TexturePixelType::RGBA, context.render_target_type(type_)));
        texture.set_render_target();
        texture.set_sampler(SamplerState {
            filter: TextureFilterType::Linear,
            wrap_u: TextureWrapType::Clamp,
            wrap_v: TextureWrapType::Clamp,
            ..SamplerState::default()
        });
    }
    texture
}
