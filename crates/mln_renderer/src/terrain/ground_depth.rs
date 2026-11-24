//! The depth of the frame's drawn ground: a texture of the eye distance of
//! each logical pixel's nearest ground, which the standing layers that
//! terrain hides read at their anchor.

use mln_gfx::gpu::{ColorAttachment, DepthStencilAttachment, Gpu, GpuError, LoadOp, TargetFormats, TextureFormat};
use mln_gfx::scissor_rect::ScissorRect;
use mln_gfx::texture2d::SamplerState;
use mln_gfx::types::{TextureFilterType, TextureWrapType};
use mln_util::Color;
use mln_util::size::Size;

use crate::gfx::context::Context;
use crate::gfx::resources::{GpuTexture, attachment_bytes};
use crate::gfx::store::SharedTexture;
use crate::paint_parameters::{OpenPass, PaintParameters};
use crate::render_pass::RenderPass;

/// The texel format of the depth: the eye distance's f32 bits, least
/// significant byte first, which every backend draws into, where a float
/// format isn't sure to be.
const FORMAT: TextureFormat = TextureFormat::Rgba8Unorm;
/// The format of the depth test that keeps each pixel's nearest ground.
const DEPTH_STENCIL: TextureFormat = TextureFormat::Depth24PlusStencil8;

/// The ground's depth at the frame's logical pixels, and the depth test it is
/// drawn with. It keeps its texture while the frame's size stays, so the
/// draws that bind it keep their bindings.
pub(crate) struct GroundDepth {
    texture: SharedTexture,
    size: Size,
    /// The depth test's attachment, made with the first pass.
    depth_stencil: Option<GpuTexture>,
}

impl GroundDepth {
    /// The ground depth of a frame of `size` device pixels at `pixel_ratio`.
    pub(crate) fn new(context: &mut Context, size: Size, pixel_ratio: f32) -> GroundDepth {
        let size = logical(size, pixel_ratio);
        let texture = context.add_texture_with(|texture| {
            texture.set_size(size);
            texture.set_format(FORMAT);
            texture.set_render_target();
            texture.set_sampler(SamplerState {
                filter: TextureFilterType::Nearest,
                wrap_u: TextureWrapType::Clamp,
                wrap_v: TextureWrapType::Clamp,
                ..SamplerState::default()
            });
        });
        GroundDepth { texture, size, depth_stencil: None }
    }

    /// Whether it is the ground depth of a frame of `size` device pixels at
    /// `pixel_ratio`.
    pub(crate) fn fits(&self, size: Size, pixel_ratio: f32) -> bool {
        self.size == logical(size, pixel_ratio)
    }

    /// The texture of the depth.
    pub(crate) fn texture(&self) -> &SharedTexture {
        &self.texture
    }

    /// The bytes of its texture and its depth test's.
    pub(crate) fn bytes(&self) -> u64 {
        attachment_bytes(self.size, FORMAT) + attachment_bytes(self.size, DEPTH_STENCIL)
    }

    /// Begins the pass that draws the depth through `gpu`, the context's,
    /// cleared to no ground, and has `parameters` draw volumes into the
    /// whole of it; [`end`](Self::end) ends it. Returns the frame's scissor
    /// rect, which `end` gives back; `None` where the pass didn't begin,
    /// which fails the frame.
    pub(crate) fn begin(&mut self, gpu: &mut dyn Gpu, parameters: &mut PaintParameters<'_>) -> Option<ScissorRect> {
        let pass = match self.pass(gpu, parameters.context) {
            Ok(pass) => pass,
            Err(error) => {
                parameters.context.fail(format!("Beginning the ground's depth pass failed: {error}"));
                return None;
            }
        };
        if let Err(error) = parameters.begin_pass(gpu, pass) {
            parameters.context.fail(format!("Beginning the ground's depth pass failed: {error}"));
            return None;
        }
        let frame_scissor = parameters.scissor_rect;
        parameters.scissor_rect = ScissorRect { x: 0, y: 0, width: self.size.width, height: self.size.height };
        parameters.pass = RenderPass::Translucent;
        parameters.depth_range_size = 1.0;
        Some(frame_scissor)
    }

    /// Ends the pass [`begin`](Self::begin) began, and gives `parameters`
    /// back the frame's scissor rect, `frame_scissor`.
    pub(crate) fn end(gpu: &mut dyn Gpu, parameters: &mut PaintParameters<'_>, frame_scissor: ScissorRect) {
        if let Err(error) = parameters.end_pass(gpu) {
            parameters.context.fail(format!("Ending the ground's depth pass failed: {error}"));
        }
        parameters.scissor_rect = frame_scissor;
    }

    /// The pass that clears the texture to 0, no ground, and the depth to 1,
    /// making them through `gpu`, the context's.
    fn pass(&mut self, gpu: &mut dyn Gpu, context: &mut Context) -> Result<OpenPass, GpuError> {
        let color = context.create_texture(gpu, &self.texture)?;
        let depth_stencil = match &self.depth_stencil {
            Some(depth_stencil) => depth_stencil,
            None => self.depth_stencil.insert(context.device().attachment(
                gpu,
                "ground depth test",
                self.size,
                DEPTH_STENCIL,
            )?),
        };
        Ok(OpenPass {
            label: "ground depth",
            color: Some(ColorAttachment { texture: color, load: LoadOp::Clear(Color::new(0.0, 0.0, 0.0, 0.0)) }),
            depth_stencil: Some(DepthStencilAttachment {
                texture: depth_stencil.id(),
                depth: LoadOp::Clear(1.0),
                stencil: LoadOp::Clear(0),
            }),
            size: self.size,
            targets: TargetFormats { color: Some(FORMAT), depth_stencil: Some(DEPTH_STENCIL) },
        })
    }
}

/// The logical pixels of a frame of `size` device pixels at `pixel_ratio`,
/// at least one each way.
fn logical(size: Size, pixel_ratio: f32) -> Size {
    let side = |pixels: u32| (f64::from(pixels) / f64::from(pixel_ratio.max(f32::MIN_POSITIVE))).ceil().max(1.0) as u32;
    Size::new(side(size.width), side(size.height))
}
