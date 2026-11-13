// The gfx-facing part of `mln::PaintParameters` (src/mln/renderer/paint_parameters.hpp).
//
// Crate cut: `PaintParameters` is a renderer type (it holds the transform
// state, atlases, static data), but gfx and the backends call into it from
// `Drawable::draw`, `LayerGroupBase::render` and `DrawableTweaker::execute`.
// The renderer's struct implements this trait; the backends only see it.

use mln_tile_id::UnwrappedTileID;

use crate::command_encoder::CommandEncoder;
use crate::context::Context;
use crate::depth_mode::DepthMode;
use crate::render_pass::RenderPass as GfxRenderPass;
use crate::renderer::render_pass::RenderPass;
use crate::scissor_rect::ScissorRect;
use crate::stencil_mode::StencilMode;
use crate::types::DepthMaskType;

pub trait PaintParameters {
    fn context(&self) -> &dyn Context;
    fn encoder(&self) -> Option<&dyn CommandEncoder>;
    fn render_pass(&self) -> Option<&dyn GfxRenderPass>;
    /// The pass being drawn (`Opaque`, `Translucent` or `Pass3D`).
    fn pass(&self) -> RenderPass;
    fn scissor_rect(&self) -> ScissorRect;

    fn depth_mode_for_sublayer(&self, n: u8, mask: DepthMaskType) -> DepthMode;
    fn depth_mode_for_3d(&self, mask: DepthMaskType) -> DepthMode;
    /// The depth that 2D layers write at the layer being drawn, as
    /// `depth_mode_for_sublayer(0, ..)` gives it where 2D layers write depth.
    fn layer_depth(&self) -> f32 {
        1.0
    }

    /// Draws the clipping masks of the tiles in `tile_ids`. C++ passes the
    /// render tiles, but only their ids are used. Returns true when the
    /// clipping mask state is ready.
    fn render_tile_clipping_masks(&mut self, tile_ids: &[UnwrappedTileID]) -> bool;
    /// Get a stencil mode for rendering constrained to the specified tile ID.
    fn stencil_mode_for_clipping(&self, tile_id: &UnwrappedTileID) -> StencilMode;
    /// Initialize a stencil mode for 3D rendering (each value is unique).
    fn stencil_mode_for_3d(&mut self) -> StencilMode;
    /// Gives each drawable a 3D stencil mode of its own, clearing the stencil
    /// buffer first if they would not all fit, so no value handed out before
    /// the clear is drawn after it. Returns false, handing out none, when there
    /// are more drawables than the buffer holds values: the caller then takes
    /// each one's value with `stencil_mode_for_3d` as it draws it, so a clear
    /// falls between two draws. C++ keys the modes by drawable; only their
    /// number matters, so this takes the modes alone.
    fn stencil_modes_for_3d(&mut self, modes: &mut [StencilMode]) -> bool;

    /// conformance: `parameters.state.getPitch()` and
    /// `parameters.state.isChanging()`, which `gfx::DrawableAtlasesTweaker`
    /// reads (the transform state is a map type, above `mln_gfx`).
    fn state_pitch(&self) -> f64 {
        0.0
    }
    fn state_is_changing(&self) -> bool {
        false
    }
}
