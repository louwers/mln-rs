//! Textures of a layer's own that its draws draw into ahead of a frame and
//! that its other draws sample: [`Target`]s.
//!
//! A layer makes a target of a size ([`Resources::target`]) and keeps it. In a frame it pushes the
//! draws into the target into the target's group of its draws
//! ([`Draws::target`](crate::draw::draws::Draws::target)), and binds the
//! target's [`texture`](Target::texture) to the draws that sample it. The
//! engine makes the target's texture, and the depth it draws with, of its
//! size; clears it and draws the group into it before the frame, or the
//! drape, whose draws sample it; and keeps whether it holds what was drawn
//! into it ([`Target::holds`]), so that a layer whose target holds the same
//! from frame to frame draws into it once.

use mln_gfx::gpu::{ColorAttachment, DepthStencilAttachment, Gpu, GpuError, LoadOp, TargetFormats};
use mln_gfx::scissor_rect::ScissorRect;
use mln_gfx::texture2d::{SamplerState, texture_format};
use mln_gfx::types::{TextureChannelDataType, TextureFilterType, TexturePixelType, TextureWrapType};
use mln_gfx::{Rgba, Size};

use crate::draw::context::Context;
use crate::draw::resources::{GpuTexture, Resources};
use crate::draw::store::{Shared, SharedTexture, TargetKey};
use crate::render::paint_parameters::{OpenPass, PaintParameters};
use crate::render::pass::RenderPass;

/// A texture that a layer's draws draw into ahead of the frame, which its
/// other draws sample: a handle of the target in the context's store, which
/// stays while a clone of the handle does.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Target {
    key: Shared<TargetKey>,
    texture: SharedTexture,
}

/// What the context keeps of a [`Target`].
pub(crate) struct TargetObjects {
    size: Size,
    /// The depth the target's draws draw with, of its size, made with its
    /// first pass where the backend draws with one
    /// ([`Context::render_target_depth`]).
    depth: Option<GpuTexture>,
    /// Whether the target holds what the draws into it drew, as
    /// [`Target::holds`] says.
    holds: bool,
    /// Whether the frame drew into the target without failing.
    drawn: bool,
}

impl Target {
    /// See [`Resources::target`]: of the type the backend draws into for
    /// `type_` ([`Context::render_target_type`]).
    pub(crate) fn new(context: &mut Context, size: Size, type_: TextureChannelDataType) -> Target {
        let format = texture_format(TexturePixelType::RGBA, context.render_target_type(type_));
        let texture = context.add_texture_with(|texture| {
            texture.set_size(size);
            texture.set_format(format);
            texture.set_render_target();
            texture.set_sampler(SamplerState {
                filter: TextureFilterType::Linear,
                wrap_u: TextureWrapType::Clamp,
                wrap_v: TextureWrapType::Clamp,
                ..SamplerState::default()
            });
        });
        let key = context.add_target(TargetObjects { size, depth: None, holds: false, drawn: false });
        Target { key, texture }
    }

    pub(crate) fn key(&self) -> &Shared<TargetKey> {
        &self.key
    }

    /// The texture the target's draws draw into, which draws sample. It is
    /// the same through a change of the target's size.
    #[must_use]
    pub fn texture(&self) -> &SharedTexture {
        &self.texture
    }

    /// Makes the target of `size` from now on: of another size, it then
    /// holds nothing.
    pub fn set_size(&self, res: &mut Resources<'_>, size: Size) {
        let context = &mut *res.context;
        let Some(objects) = context.target_mut(self).filter(|objects| objects.size != size) else {
            return;
        };
        *objects = TargetObjects { size, depth: None, holds: false, drawn: false };
        if let Some(mut texture) = context.texture_mut(&self.texture) {
            texture.set_size(size);
        }
    }

    /// Whether the target holds what draws drew into it: from the frame
    /// whose draws into it were written, until a frame fails to draw them,
    /// or the target changes size.
    #[must_use]
    pub fn holds(&self, res: &Resources<'_>) -> bool {
        res.context.target(self).is_some_and(|objects| objects.holds)
    }

    /// Clears the target to opaque black and its depth to 1, and runs
    /// `draw` with `parameters` into the target in the opaque pass and then
    /// the translucent one, each as the first layer of a frame of its own,
    /// outside any other pass, through `gpu`, the context's. `draw` returns
    /// whether its draws didn't fail. A target that draws without failing
    /// holds what it drew once the frame is done
    /// ([`Context::end_targets`]).
    pub(crate) fn render(
        &self,
        gpu: &mut dyn Gpu,
        parameters: &mut PaintParameters<'_>,
        mut draw: impl FnMut(&mut dyn Gpu, &mut PaintParameters<'_>) -> bool,
    ) {
        let begun = self.pass(gpu, parameters.context).and_then(|pass| parameters.begin_pass(gpu, pass).map(|()| pass));
        let pass = match begun {
            Ok(pass) => pass,
            Err(error) => {
                parameters.context.fail(format!("Beginning a render target's pass failed: {error}"));
                return;
            }
        };
        let frame = (parameters.scissor_rect, parameters.pass, parameters.depth_range_size, parameters.current_layer);
        parameters.scissor_rect = ScissorRect { x: 0, y: 0, width: pass.size.width, height: pass.size.height };
        let num_sublayers = PaintParameters::NUM_SUBLAYERS as usize;
        let mut drawn = true;
        for pass in [RenderPass::Opaque, RenderPass::Translucent] {
            parameters.pass = pass;
            parameters.depth_range_size = 1.0 - (3 * num_sublayers) as f32 * PaintParameters::DEPTH_EPSILON;
            parameters.current_layer = 0;
            drawn &= draw(gpu, parameters);
        }
        let ended = parameters.end_pass(gpu);
        if let Err(error) = &ended {
            parameters.context.fail(format!("Ending a render target's pass failed: {error}"));
        }
        (parameters.scissor_rect, parameters.pass, parameters.depth_range_size, parameters.current_layer) = frame;
        if let Some(objects) = parameters.context.target_mut(self) {
            objects.drawn = ended.is_ok() && drawn;
        }
    }

    /// The pass that clears the target to opaque black and its depth to 1,
    /// making them through `gpu`, the context's.
    fn pass(&self, gpu: &mut dyn Gpu, context: &mut Context) -> Result<OpenPass, GpuError> {
        let color = context.create_texture(gpu, &self.texture)?;
        let format = context.texture(&self.texture).ok_or(GpuError::UnknownId("texture"))?.format();
        let depth_format = context.render_target_depth();
        let objects = context.target(self).ok_or(GpuError::UnknownId("render target"))?;
        let size = objects.size;
        if objects.depth.is_none()
            && let Some(depth_format) = depth_format
        {
            let depth = context.device().attachment(gpu, "render target depth", size, depth_format)?;
            if let Some(objects) = context.target_mut(self) {
                objects.depth = Some(depth);
            }
        }
        let depth = context.target(self).and_then(|objects| objects.depth.as_ref()).map(GpuTexture::id);
        Ok(OpenPass {
            label: "render target",
            color: Some(ColorAttachment { texture: color, load: LoadOp::Clear(Rgba::new(0.0, 0.0, 0.0, 1.0)) }),
            depth_stencil: depth.map(|texture| DepthStencilAttachment {
                texture,
                depth: LoadOp::Clear(1.0),
                stencil: LoadOp::Load,
            }),
            size,
            targets: TargetFormats { color: Some(format), depth_stencil: depth.and(depth_format) },
        })
    }
}

impl TargetObjects {
    /// Marks the target as holding what the frame's draws into it draw,
    /// until the frame ends without drawing them.
    pub(crate) fn declare(&mut self) {
        self.holds = true;
        self.drawn = false;
    }

    /// Ends the frame of the target: it holds what the frame drew into it
    /// if the frame drew it, and `submitted`, its work reached the GPU.
    pub(crate) fn end(&mut self, submitted: bool) {
        self.holds = self.drawn && submitted;
        self.drawn = false;
    }
}
