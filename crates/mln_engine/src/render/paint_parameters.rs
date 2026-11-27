use std::any::Any;
use std::rc::Rc;

use glam::{DMat4, DVec4};
use mln_gfx::cull_face_mode::CullFaceMode;
use mln_gfx::gpu::{ColorAttachment, DepthStencilAttachment, Gpu, GpuError, LoadOp, PassDesc, Scissor, TargetFormats};
use mln_gfx::shaders::layer_ubo::GlobalPaintParamsUBO;
use mln_gfx::stencil_mode::Test;
use mln_gfx::types::{CullFaceWindingType, DepthFunctionType, StencilOpType};
use mln_gfx::{DepthMaskType, DepthMode, ScissorRect, Size, StencilMode};
use mln_style::constants::EXTENT;
use mln_style::tile_id::CanonicalTileID;
use mln_vec_map::VecMap;
use smallvec::SmallVec;

use crate::camera::{ProjectionData, View, ViewportMode};
use crate::draw::context::Context;
use crate::draw::identity::SimpleIdentity;
use crate::render::clip_masks::ClipMasks;
use crate::render::frame::Frame;
use crate::render::layer_composite::LayerComposite;
use crate::render::pass::RenderPass;
use crate::render::static_data::RenderStaticData;
use crate::render::terrain::FrameGround;
use crate::tile::UnwrappedTileID;

/// The tiles whose masks clear the stencil buffer: one per world copy in
/// view, so usually one, and held inline, since every frame makes them.
pub(crate) type StencilClear = SmallVec<[UnwrappedTileID; 2]>;

/// A draw group drawing into the layer composite's texture, from
/// [`PaintParameters::begin_composited`] to
/// [`PaintParameters::end_composited`], which every group that begins must
/// reach.
#[must_use = "a composited draw group ends with `PaintParameters::end_composited`"]
pub(crate) struct Composited {
    group: SimpleIdentity,
    /// The frame's pass, which resumes after the group's draws.
    frame: OpenPass,
    /// Whether the texture's pass began.
    began: bool,
    /// The frame's tile masks and next stencil value, where the group's
    /// draws don't share them: they draw with the texture's own stencil.
    frame_stencil: Option<(VecMap<UnwrappedTileID, i32>, i32)>,
    unended: Unended,
}

/// Checks, in debug builds, that a [`Composited`] reaches
/// [`PaintParameters::end_composited`], which forgets it: dropping a group
/// that didn't would leave the frame's pass ended and its stencil lost.
struct Unended;

impl Drop for Unended {
    fn drop(&mut self) {
        debug_assert!(std::thread::panicking(), "a composited draw group ended without `end_composited`");
    }
}

impl Composited {
    /// Whether the group draws: false where the texture's pass didn't
    /// begin.
    #[must_use]
    pub(crate) fn draws(&self) -> bool {
        self.began
    }
}

pub(crate) struct TransformParameters {
    pub(crate) proj_matrix: DMat4,
    pub(crate) aligned_proj_matrix: DMat4,
    pub(crate) near_clipped_proj_matrix: DMat4,
    /// The near plane of `near_clipped_proj_matrix`, in pixels.
    pub(crate) near_clipped_near_z: f64,
    pub(crate) view: View,
}

impl TransformParameters {
    pub(crate) fn new(view: &View) -> Self {
        let state = view.clone();
        // Update the default matrices to the current viewport dimensions.
        let proj_matrix = state.projection_matrix_with(state.near_z(), false);

        // Also compute a projection matrix that aligns with the current pixel
        // grid, taking into account odd viewport sizes.
        let aligned_proj_matrix = state.projection_matrix_with(state.near_z(), true);

        // Calculate a second projection matrix with the near plane moved
        // further, to a tenth of the far value, so as not to waste depth
        // buffer precision on very close empty space, for layer types
        // (fill-extrusion) that use the depth buffer to emulate real-world
        // space.
        let near_clipped_near_z = f64::from((0.1 * state.camera_to_center_distance()) as u16);
        let near_clipped_proj_matrix = state.projection_matrix_with(near_clipped_near_z, false);

        TransformParameters {
            proj_matrix,
            aligned_proj_matrix,
            near_clipped_proj_matrix,
            near_clipped_near_z,
            view: state,
        }
    }

    /// The transform of a drape of the tile `tile` in `view`'s map, which
    /// draws the map's layers into the tile's texture: an orthographic
    /// projection of the tile's square of the world, in pixels at the
    /// view's scale, onto the whole target, the tile's north edge at clip
    /// y = 1, every depth at clip z = 0. The view stays `view`, so zoom and
    /// bearing are its own.
    pub(crate) fn for_drape(view: &View, tile: &UnwrappedTileID) -> Self {
        // The tile's square of the world: its corner, and its side.
        let tile_matrix = view.tile_matrix(tile);
        let (x, y) = (tile_matrix.w_axis.x, tile_matrix.w_axis.y);
        let side = tile_matrix.x_axis.x * f64::from(EXTENT);
        let proj_matrix = DMat4::from_cols(
            DVec4::new(2.0 / side, 0.0, 0.0, 0.0),
            DVec4::new(0.0, -2.0 / side, 0.0, 0.0),
            DVec4::ZERO,
            DVec4::new(-1.0 - 2.0 * x / side, 1.0 + 2.0 * y / side, 0.0, 1.0),
        );
        TransformParameters {
            proj_matrix,
            aligned_proj_matrix: proj_matrix,
            near_clipped_proj_matrix: proj_matrix,
            near_clipped_near_z: 0.0,
            view: view.clone(),
        }
    }

    /// The projection of the tile `tile_id`, with the projection matrix
    /// aligned to the pixel grid or not.
    #[must_use]
    pub(crate) fn projection_data_for_tile(&self, tile_id: &UnwrappedTileID, aligned: bool) -> ProjectionData {
        self.view.projection_data_with(tile_id, if aligned { &self.aligned_proj_matrix } else { &self.proj_matrix })
    }
}

/// Where a draw draws in the open pass: the pass's attachments' size and
/// formats, and the scissor rect within them, `None` where nothing of the
/// frame's scissor rect is.
#[derive(Clone, Copy)]
pub(crate) struct DrawTarget {
    pub(crate) size: Size,
    pub(crate) targets: TargetFormats,
    pub(crate) scissor: Option<Scissor>,
}

/// A pass of the backend's `Gpu` that draws draw into, and its
/// attachments' size and formats.
#[derive(Clone, Copy)]
pub(crate) struct OpenPass {
    pub(crate) label: &'static str,
    pub(crate) color: Option<ColorAttachment>,
    pub(crate) depth_stencil: Option<DepthStencilAttachment>,
    pub(crate) size: Size,
    pub(crate) targets: TargetFormats,
}

impl OpenPass {
    /// The pass over the same attachments that loads them, with the
    /// stencil as `stencil` says.
    fn loading(&self, stencil: LoadOp<u32>) -> OpenPass {
        OpenPass {
            color: self.color.map(|color| ColorAttachment { load: LoadOp::Load, ..color }),
            depth_stencil: self.depth_stencil.map(|depth_stencil| DepthStencilAttachment {
                depth: LoadOp::Load,
                stencil,
                ..depth_stencil
            }),
            ..*self
        }
    }

    fn desc(&self) -> PassDesc<'static> {
        PassDesc { label: self.label, color: self.color, depth_stencil: self.depth_stencil }
    }
}

/// The state of a frame's encode: the renderer's GPU state, which the
/// encode changes, the frame's camera and values, and where its passes are.
pub(crate) struct PaintParameters<'a> {
    pub(crate) context: &'a mut Context,
    /// The pass of the backend's `Gpu` that draws draw into, while one
    /// is open.
    open_pass: Option<OpenPass>,

    pub(crate) transform_params: &'a TransformParameters,
    pub(crate) view: &'a View,
    /// Whether a gesture or an animation moves the camera: draws favor speed
    /// over crispness then.
    pub(crate) camera_moving: bool,
    /// The light of the frame.
    pub(crate) light: &'a crate::render::light::EvaluatedLight,
    pub(crate) static_data: &'a RenderStaticData,

    pub(crate) pass: RenderPass,

    pub(crate) pixel_ratio: f32,
    pub(crate) renderable_size: Size,
    pub(crate) pixels_to_gl_units: [f32; 2],

    /// The masks of the tiles that the frame's draw groups can clip to.
    pub(crate) clip_masks: &'a mut ClipMasks,
    /// What the frame's layers drawn as a whole draw into and composite with.
    pub(crate) layer_composite: &'a mut LayerComposite,
    /// Whether a layer drawn as a whole can draw with the frame's depth and
    /// stencil: the frame has them, and a pass can draw into them together
    /// with the layer composite's texture.
    pub(crate) composites_with_frame_depth: bool,
    /// The stencil value of each tile whose mask the stencil buffer holds.
    tile_clipping_mask_ids: VecMap<UnwrappedTileID, i32>,
    next_stencil_id: i32,
    /// The tile of the drape that the draws go into, whose mask covers the
    /// whole target; `None` for the frame.
    drape_tile: Option<UnwrappedTileID>,
    /// Whether the draws go into a drape whose shading the terrain's surface
    /// adds ([`SurfaceLight`](crate::render::terrain::SurfaceLight)): a hillshade
    /// draws its slopes there, unlit.
    pub(crate) drape_lit: bool,
    /// The tiles whose masks `clear_stencil` draws zero with in the frame,
    /// from [`stencil_clear_tiles`](Self::stencil_clear_tiles); a drape
    /// draws its own tile's.
    pub(crate) stencil_clear: StencilClear,

    pub(crate) current_layer: u32,
    pub(crate) depth_range_size: f32,
    /// Whether the frame draws terrain, under which 2D layers draw without
    /// depth.
    pub(crate) terrain: bool,
    /// The ground that the layers that stand on terrain stand on, where the
    /// frame has terrain and such layers.
    pub(crate) ground: Option<Rc<FrameGround>>,
    /// The number of the frame's draw groups above its lowest 3D layer's,
    /// which draw without a depth test
    /// ([`RenderOrchestrator::opaque_pass_cutoff`](crate::render::orchestrator::RenderOrchestrator::opaque_pass_cutoff)).
    pub(crate) opaque_pass_cutoff: u32,
    pub(crate) symbol_fade_change: f32,

    pub(crate) scissor_rect: ScissorRect,
    /// The frame that custom layers draw into, set before their prerender.
    pub(crate) custom_layer_frame: Option<crate::layers::custom::CustomLayerFrame>,
}

impl<'a> PaintParameters<'a> {
    pub(crate) const NUM_SUBLAYERS: i32 = 3;
    pub(crate) const DEPTH_EPSILON: f32 = 1.0 / (1 << 16) as f32;
    pub(crate) const MAX_STENCIL_VALUE: i32 = 255;

    /// The parameters of the encode of `frame`, into a target of
    /// `renderable_size` device pixels at `pixel_ratio`, drawn within
    /// `scissor_rect`, which clips with `clip_masks` and composites with
    /// `layer_composite`, with the frame's depth where
    /// `composites_with_frame_depth` says it can.
    #[expect(clippy::too_many_arguments, reason = "the renderer's GPU state, the frame and where it draws")]
    pub(crate) fn new(
        context: &'a mut Context,
        clip_masks: &'a mut ClipMasks,
        layer_composite: &'a mut LayerComposite,
        composites_with_frame_depth: bool,
        frame: &'a Frame,
        static_data: &'a RenderStaticData,
        pixel_ratio: f32,
        renderable_size: Size,
        scissor_rect: ScissorRect,
    ) -> Self {
        let transform_params = &frame.transform;
        let state = &transform_params.view;
        let mut pixels_to_gl_units = [2.0f32 / state.size().width as f32, -2.0f32 / state.size().height as f32];

        if state.viewport_mode() == ViewportMode::FlippedY {
            pixels_to_gl_units[1] *= -1.0;
        }

        PaintParameters {
            context,
            open_pass: None,
            transform_params,
            view: state,
            camera_moving: frame.camera_moving,
            static_data,
            light: &frame.light,
            pass: RenderPass::Opaque,
            pixel_ratio,
            renderable_size,
            pixels_to_gl_units,
            clip_masks,
            layer_composite,
            composites_with_frame_depth,
            tile_clipping_mask_ids: VecMap::new(),
            next_stencil_id: 1,
            drape_tile: None,
            drape_lit: false,
            stencil_clear: StencilClear::new(),
            // The renderer sets these before use.
            current_layer: 0,
            depth_range_size: 1.0,
            terrain: frame.terrain,
            ground: None,
            opaque_pass_cutoff: 0,
            symbol_fade_change: frame.symbol_fade_change,
            scissor_rect,
            custom_layer_frame: None,
        }
    }

    /// Culling of the faces turned away from the camera, whose triangles
    /// wind clockwise on screen, or counterclockwise where a viewport
    /// flipped upside down turns every triangle round. The globe's
    /// projection draws the right way up in either mode.
    pub(crate) fn cull_back_faces(&self) -> CullFaceMode {
        let turned = self.view.viewport_mode() == ViewportMode::FlippedY && !self.view.is_globe();
        let winding = if turned { CullFaceWindingType::Clockwise } else { CullFaceWindingType::CounterClockwise };
        CullFaceMode { winding, ..CullFaceMode::back_ccw() }
    }

    /// The parameters of the encode of a drape, which draws the frame's
    /// layers into a texture of `size` over its tile `tile` with `transform`
    /// ([`TransformParameters::for_drape`]): with the transform's view, the
    /// frame's GPU state and values, but its own clip masks and stencil
    /// values, its layers drawn as a whole into and composited by
    /// `layer_composite`, of `size`, with their own depth, and the whole
    /// target as the scissor. The layers draw with depth, front to back in
    /// the opaque pass. A map pixel is a pixel of the projection, so
    /// [`pixels_to_gl_units`](Self::pixels_to_gl_units) are its scale.
    pub(crate) fn drape<'b>(
        &'b mut self,
        transform: &'b TransformParameters,
        clip_masks: &'b mut ClipMasks,
        layer_composite: &'b mut LayerComposite,
        size: Size,
        tile: UnwrappedTileID,
    ) -> PaintParameters<'b> {
        let proj_matrix = &transform.proj_matrix;
        PaintParameters {
            context: &mut *self.context,
            open_pass: None,
            transform_params: transform,
            view: &transform.view,
            camera_moving: self.camera_moving,
            light: self.light,
            static_data: self.static_data,
            pass: RenderPass::Opaque,
            pixel_ratio: self.pixel_ratio,
            renderable_size: size,
            pixels_to_gl_units: [proj_matrix.x_axis.x as f32, proj_matrix.y_axis.y as f32],
            clip_masks,
            layer_composite,
            composites_with_frame_depth: false,
            tile_clipping_mask_ids: VecMap::new(),
            next_stencil_id: 1,
            drape_tile: Some(tile),
            drape_lit: false,
            stencil_clear: StencilClear::new(),
            current_layer: 0,
            depth_range_size: 1.0,
            terrain: false,
            ground: None,
            opaque_pass_cutoff: 0,
            // A drape draws no symbols, and keeps no fade of theirs.
            symbol_fade_change: 1.0,
            scissor_rect: ScissorRect { x: 0, y: 0, width: size.width, height: size.height },
            custom_layer_frame: None,
        }
    }

    /// What every draw of the encode reads of its frame: the target's size
    /// and the pixels' scale, the camera and the frame's values.
    #[must_use]
    pub(crate) fn global_paint_params(&self) -> GlobalPaintParamsUBO {
        GlobalPaintParamsUBO {
            units_to_pixels: [1.0 / self.pixels_to_gl_units[0], 1.0 / self.pixels_to_gl_units[1]],
            world_size: [self.renderable_size.width as f32, self.renderable_size.height as f32],
            camera_to_center_distance: self.view.camera_to_center_distance() as f32,
            symbol_fade_change: self.symbol_fade_change,
            aspect_ratio: self.view.size().aspect_ratio(),
            pixel_ratio: self.pixel_ratio,
            map_zoom: self.view.zoom() as f32,
            pad1: 0.0,
            pad2: 0.0,
            pad3: 0.0,
        }
    }

    /// The paint parameters that the frame's draws read instead of the
    /// context's global ones, which the renderer writes for the map's: a
    /// drape's own.
    #[must_use]
    pub(crate) fn own_paint_params(&self) -> Option<GlobalPaintParamsUBO> {
        self.drape_tile.is_some().then(|| self.global_paint_params())
    }

    /// The size and formats of the attachments that draws draw into
    /// now, `None` outside a pass.
    pub(crate) fn pass_target(&self) -> Option<(Size, TargetFormats)> {
        self.open_pass.map(|pass| (pass.size, pass.targets))
    }

    /// Where draws draw now, `None` outside a pass.
    pub(crate) fn draw_target(&self) -> Option<DrawTarget> {
        let (size, targets) = self.pass_target()?;
        Some(DrawTarget { size, targets, scissor: clamped_scissor(self.scissor_rect, size) })
    }

    /// Begins `pass` on `gpu`, the context's, which draws then draw
    /// into.
    pub(crate) fn begin_pass(&mut self, gpu: &mut dyn Gpu, pass: OpenPass) -> Result<(), GpuError> {
        gpu.begin_pass(&pass.desc())?;
        self.open_pass = Some(pass);
        Ok(())
    }

    /// Ends the pass begun on `gpu`, the context's, if any.
    pub(crate) fn end_pass(&mut self, gpu: &mut dyn Gpu) -> Result<(), GpuError> {
        match self.open_pass.take() {
            Some(_) => gpu.end_pass(),
            None => Ok(()),
        }
    }

    /// Ends the pass begun on `gpu`, the context's, and begins one over its
    /// attachments that loads them, with the stencil as `stencil` says.
    fn restart_pass(&mut self, gpu: &mut dyn Gpu, stencil: LoadOp<u32>) {
        let Some(pass) = self.open_pass else {
            return;
        };
        let restarted = self.end_pass(gpu).and_then(|()| self.begin_pass(gpu, pass.loading(stencil)));
        if let Err(error) = restarted {
            self.context.fail(format!("Restarting the {} pass failed: {error}", pass.label));
        }
    }

    /// Begins drawing the draw group `group` into the layer composite's
    /// texture, rather than into the pass begun on `gpu`, the context's,
    /// where the frame composites `group`; `None` where it doesn't, and the
    /// group draws into the pass begun. The group draws only where
    /// [`Composited::draws`] says, and
    /// [`end_composited`](Self::end_composited) then composites the texture
    /// onto the pass begun. A group that draws with the texture's own
    /// stencil starts with it cleared and leaves the frame's as it was; one
    /// that draws with the frame's depth and stencil goes on with them.
    pub(crate) fn begin_composited(&mut self, gpu: &mut dyn Gpu, group: SimpleIdentity) -> Option<Composited> {
        let frame = self.open_pass.filter(|_| self.layer_composite.composites(group))?;
        let composite = self.layer_composite.pass(group, &frame)?;
        let own_stencil = !self.layer_composite.draws_with_frame_depth(group);
        let mut drawn = self.end_pass(gpu);
        if drawn.is_ok()
            && let Some(backdrop) = self.layer_composite.backdrop_pass(group)
        {
            drawn = self.begin_pass(gpu, backdrop);
            if drawn.is_ok() {
                if let Err(error) = self.copy_frame(gpu) {
                    self.context.fail(format!("Copying the frame failed: {error}"));
                }
                drawn = self.end_pass(gpu);
            }
        }
        drawn = drawn.and_then(|()| self.begin_pass(gpu, composite));
        let began = drawn.is_ok();
        if let Err(error) = drawn {
            self.context.fail(format!("Beginning the layer composite's pass failed: {error}"));
        }
        let frame_stencil = (began && own_stencil).then(|| {
            (std::mem::take(&mut self.tile_clipping_mask_ids), std::mem::replace(&mut self.next_stencil_id, 1))
        });
        Some(Composited { group, frame, began, frame_stencil, unended: Unended })
    }

    /// Copies the layer composite's frame texture onto the open pass.
    fn copy_frame(&mut self, gpu: &mut dyn Gpu) -> Result<(), GpuError> {
        match self.draw_target() {
            Some(target) => self.layer_composite.copy_frame(gpu, target, self.context),
            None => Ok(()),
        }
    }

    /// Ends the draws `begin_composited` began, and composites what they drew
    /// onto the frame's pass, which this begins again.
    pub(crate) fn end_composited(&mut self, gpu: &mut dyn Gpu, composited: Composited) {
        let Composited { group, frame, began, frame_stencil, unended } = composited;
        std::mem::forget(unended);
        if let Some((tile_clipping_mask_ids, next_stencil_id)) = frame_stencil {
            self.tile_clipping_mask_ids = tile_clipping_mask_ids;
            self.next_stencil_id = next_stencil_id;
        }
        if began && let Err(error) = self.end_pass(gpu) {
            self.context.fail(format!("Ending the layer composite's pass failed: {error}"));
        }
        if let Err(error) = self.begin_pass(gpu, frame.loading(LoadOp::Load)) {
            self.context.fail(format!("Resuming the {} pass failed: {error}", frame.label));
        } else if began && let Some(target) = self.draw_target() {
            // What drew with the frame's depth tested it as it drew, and
            // left its own there, in front of the layer's. The others
            // composite at the depth of the current layer: layers above
            // that drew into the depth buffer hide them.
            let depth = if self.layer_composite.draws_with_frame_depth(group) {
                DepthMode::disabled()
            } else {
                self.depth_mode_for_sublayer(0, DepthMaskType::ReadOnly)
            };
            self.layer_composite.draw(gpu, target, depth, self.context, group);
        }
    }

    /// Ends the frame's pass, which drew into the layer composite's frame
    /// texture, and begins `present`, which the texture is copied into.
    /// Does nothing where the frame drew into its target.
    pub(crate) fn present_frame(&mut self, gpu: &mut dyn Gpu, present: OpenPass) -> Result<(), GpuError> {
        if self.layer_composite.frame_texture().is_none() {
            return Ok(());
        }
        self.end_pass(gpu)?;
        self.begin_pass(gpu, present)?;
        self.copy_frame(gpu)
    }

    /// Runs `f` with `gpu`'s, the context's, commands inside the pass
    /// begun, after what drew there so far, as a custom layer draws: `f`
    /// gets the backend's `CustomLayerPass`. Returns whether `f` ran.
    pub(crate) fn lend_pass(&self, gpu: &mut dyn Gpu, f: &mut dyn FnMut(&mut dyn Any)) -> Result<bool, GpuError> {
        if self.open_pass.is_none() {
            return Ok(false);
        }
        gpu.lend(f).map(|()| true)
    }

    #[must_use]
    pub(crate) fn projection_data_for_tile(&self, tile_id: &UnwrappedTileID, aligned: bool) -> ProjectionData {
        self.transform_params.projection_data_for_tile(tile_id, aligned)
    }

    /// Whether the current layer draws in the opaque pass: front to back,
    /// with depth. On the globe, 2D layers are ordered by draw order alone;
    /// their Z is the back-hemisphere clip. Over terrain they draw flat, on
    /// top of it.
    #[must_use]
    pub(crate) fn opaque_pass_enabled_for_layer(&self) -> bool {
        self.current_layer >= self.opaque_pass_cutoff && !self.view.is_globe() && !self.terrain
    }

    #[must_use]
    pub(crate) fn depth_mode_for_sublayer(&self, n: u8, mask: DepthMaskType) -> DepthMode {
        if !self.opaque_pass_enabled_for_layer() {
            return DepthMode::disabled();
        }

        // Wrapping unsigned arithmetic, then a float product.
        let steps =
            (1u32.wrapping_add(self.current_layer)).wrapping_mul(Self::NUM_SUBLAYERS as u32).wrapping_add(u32::from(n));
        let depth = self.depth_range_size + steps as f32 * Self::DEPTH_EPSILON;
        DepthMode { func: DepthFunctionType::LessEqual, mask, range: (depth, depth) }
    }

    /// The depth of `depth_mode_for_sublayer(0, ..)` where 2D layers write
    /// depth, whether or not the current layer does.
    #[must_use]
    pub(crate) fn layer_depth(&self) -> f32 {
        let steps = (1u32.wrapping_add(self.current_layer)).wrapping_mul(Self::NUM_SUBLAYERS as u32);
        self.depth_range_size + steps as f32 * Self::DEPTH_EPSILON
    }

    #[must_use]
    pub(crate) fn depth_mode_for_3d(&self, mask: DepthMaskType) -> DepthMode {
        DepthMode { func: DepthFunctionType::LessEqual, mask, range: (0.0, self.depth_range_size) }
    }

    pub(crate) fn clear_stencil(&mut self, gpu: &mut dyn Gpu) {
        self.next_stencil_id = 1;
        self.tile_clipping_mask_ids.clear();

        if self.context.caps().clear_stencil_with_pass {
            self.restart_pass(gpu, LoadOp::Clear(0));
        } else {
            let masks: SmallVec<[_; 2]> = match self.drape_tile {
                Some(tile) => SmallVec::from_elem((tile, 0), 1),
                None => self.stencil_clear.iter().map(|tile| (*tile, 0)).collect(),
            };
            self.draw_clip_masks(gpu, &masks);
        }
        self.context.rendering_stats().stencil_clears += 1;
    }

    /// The tiles whose masks `clear_stencil` draws zero with, where the
    /// render pass clears the stencil buffer only where it begins, when the
    /// draws clip to the masks of `clip_tiles`: those that cover every mask,
    /// the drape's own tile in a drape and the world in each world copy of
    /// `clip_tiles` in the frame.
    #[must_use]
    pub(crate) fn stencil_clear_tiles(&self, clip_tiles: &[UnwrappedTileID]) -> StencilClear {
        let mut tiles = StencilClear::new();
        if self.context.caps().clear_stencil_with_pass || clip_tiles.is_empty() {
            return tiles;
        }
        if let Some(tile) = self.drape_tile {
            tiles.push(tile);
            return tiles;
        }
        for tile in clip_tiles {
            if !tiles.iter().any(|world| world.wrap == tile.wrap) {
                tiles.push(UnwrappedTileID::with_wrap(tile.wrap, CanonicalTileID::new(0, 0, 0)));
            }
        }
        tiles.sort_unstable();
        tiles
    }

    fn draw_clip_masks(&mut self, gpu: &mut dyn Gpu, masks: &[(UnwrappedTileID, i32)]) {
        if let Some(target) = self.draw_target() {
            self.clip_masks.draw(gpu, masks, target, self.context);
        }
    }

    /// Renders the clipping masks of `tile_ids`, unless the stencil buffer
    /// already holds exactly theirs.
    pub(crate) fn render_tile_clipping_masks(&mut self, gpu: &mut dyn Gpu, tile_ids: &[UnwrappedTileID]) {
        let covered = self.tile_clipping_mask_ids.len() == tile_ids.len()
            && self.tile_clipping_mask_ids.keys().zip(tile_ids.iter()).all(|(a, b)| a == b);
        if covered || self.pass_target().is_none() {
            return;
        }

        self.tile_clipping_mask_ids.clear();

        // If the stencil value would overflow, clear the stencil buffer so
        // that none of the new values remain set somewhere in it. Otherwise
        // the masks overwrite it incrementally.
        if self.next_stencil_id as usize + tile_ids.len() > Self::MAX_STENCIL_VALUE as usize {
            self.clear_stencil(gpu);
        }

        let mut masks = Vec::with_capacity(tile_ids.len());
        for tile_id in tile_ids {
            if self.tile_clipping_mask_ids.contains_key(tile_id) {
                continue;
            }
            self.tile_clipping_mask_ids.insert(*tile_id, self.next_stencil_id);
            masks.push((*tile_id, self.next_stencil_id));
            self.next_stencil_id += 1;
        }

        if !masks.is_empty() {
            self.draw_clip_masks(gpu, &masks);
            self.context.rendering_stats().stencil_updates += 1;
        }
    }

    #[must_use]
    pub(crate) fn stencil_mode_for_clipping(&self, tile_id: &UnwrappedTileID) -> StencilMode {
        let it = self.tile_clipping_mask_ids.get(tile_id);
        debug_assert!(it.is_some());
        let id = it.copied().unwrap_or(0b0000_0000);
        StencilMode {
            test: Test::Equal { mask: 0b1111_1111 },
            ref_: id,
            mask: 0b0000_0000,
            fail: StencilOpType::Keep,
            depth_fail: StencilOpType::Keep,
            pass: StencilOpType::Replace,
        }
    }

    pub(crate) fn stencil_mode_for_3d(&mut self, gpu: &mut dyn Gpu) -> StencilMode {
        if self.next_stencil_id + 1 > Self::MAX_STENCIL_VALUE {
            self.clear_stencil(gpu);
        }

        // We're potentially destroying the stencil clipping mask in this
        // pass. That means we'll have to recreate it for the next source if
        // any.
        self.tile_clipping_mask_ids.clear();

        let id = self.next_stencil_id;
        self.next_stencil_id += 1;
        StencilMode {
            test: Test::NotEqual { mask: 0b1111_1111 },
            ref_: id,
            mask: 0b1111_1111,
            fail: StencilOpType::Keep,
            depth_fail: StencilOpType::Keep,
            pass: StencilOpType::Replace,
        }
    }
}

/// `rect` within a target of `size`, `None` where nothing of it is.
fn clamped_scissor(rect: ScissorRect, size: Size) -> Option<Scissor> {
    let (x, y) = (rect.x as u32, rect.y as u32);
    let width = if x < size.width { rect.width.min(size.width - x) } else { 0 };
    let height = if y < size.height { rect.height.min(size.height - y) } else { 0 };
    (width > 0 && height > 0).then_some(Scissor { x, y, width, height })
}
