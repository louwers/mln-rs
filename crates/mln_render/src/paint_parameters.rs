use glam::DMat4;
use mln_gfx::gpu::{
    ColorAttachment, DepthState, DepthStencilAttachment, Gpu, GpuError, LoadOp, PassDesc, Scissor, StencilState,
    StencilTest, TargetFormats,
};
use mln_gfx::types::{DepthFunctionType, StencilOpType};
use mln_map_types::{ClipDepthRange, FrameProjection, MapDebugOptions, ProjectionData, View, ViewportMode};
use mln_tile_id::UnwrappedTileID;
use mln_util::identity::SimpleIdentity;
use mln_util::logging::{Event, Log};
use mln_util::size::Size;
use mln_vec_map::VecMap;

use crate::clip_masks::ClipMasks;
use crate::frame::FramePlan;
use crate::gfx::context::Resources;
use crate::layer_composite::LayerComposite;
use crate::render_light::EvaluatedLight;
use crate::render_pass::Pass;

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
        let proj_matrix = state.projection_matrix_with(1, false);

        // Also compute a projection matrix that aligns with the current pixel
        // grid, taking into account odd viewport sizes.
        let aligned_proj_matrix = state.projection_matrix_with(1, true);

        // Calculate a second projection matrix with the near plane moved
        // further, to a tenth of the far value, so as not to waste depth
        // buffer precision on very close empty space, for layer types that
        // use the depth buffer to emulate real-world space.
        let near_z = (0.1 * state.camera_to_center_distance()) as u16;
        let near_clipped_proj_matrix = state.projection_matrix_with(near_z, false);

        TransformParameters {
            proj_matrix,
            aligned_proj_matrix,
            near_clipped_proj_matrix,
            near_clipped_near_z: f64::from(near_z),
            view: state,
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

/// The frame a layer draws in: its camera, the target it draws into and
/// its light.
#[derive(Clone, Copy)]
pub struct Frame<'a> {
    /// The frame's camera.
    pub camera: &'a View,
    /// Whether a gesture or an animation moves the camera: draws favor speed
    /// over crispness then.
    pub camera_moving: bool,
    /// The device pixels of the target per logical pixel of the camera.
    pub pixel_ratio: f32,
    /// The size of the target, in device pixels.
    pub target_size: Size,
    /// The clip space units that a logical pixel spans, in x and y, where
    /// y's sign follows the viewport's orientation.
    pub pixels_to_clip: [f32; 2],
    /// The style's light, evaluated for the frame.
    pub light: &'a EvaluatedLight,
    /// Whether a group drawn as a whole ([`Composite`](crate::draw::Composite))
    /// can draw with the frame's depth and stencil: the frame has them, and
    /// a pass can draw into them together with the layer composite's
    /// texture.
    pub(crate) composites_with_frame_depth: bool,
    /// The clip depth of the backend's API.
    pub(crate) clip_depth: ClipDepthRange,
    /// How far the fade of the current placement has come: from 0 when it
    /// was committed to 1 once its fade is done, and past 1 after that; 1
    /// where placements don't fade.
    pub(crate) placement_fade: f32,
    /// The frame's projection matrices.
    pub(crate) transform: &'a TransformParameters,
}

impl Frame<'_> {
    /// The projection of the tile `tile_id` in the frame, pixel-aligned if
    /// `aligned`.
    #[must_use]
    pub(crate) fn projection_data_for_tile(&self, tile_id: &UnwrappedTileID, aligned: bool) -> ProjectionData {
        self.transform.projection_data_for_tile(tile_id, aligned)
    }
}

/// The frame's encode, which a layer's drawing and its offscreen pass draw
/// through and pass on to the draw services. Outside the engine, it says
/// which pass draws, at which depth, and where the frame draws the map.
pub(crate) struct PaintParameters<'a> {
    /// The renderer's GPU state, which the encode changes.
    pub(crate) context: &'a mut Resources,
    /// The pass of the backend's `Gpu` that draws draw into, while one
    /// is open.
    open_pass: Option<OpenPass>,

    /// The frame's camera and target.
    pub(crate) frame: Frame<'a>,

    pub(crate) pass: Pass,
    pub(crate) debug_options: MapDebugOptions,

    /// The masks of the tiles that the frame's draw groups can clip to.
    pub(crate) clip_masks: &'a mut ClipMasks,
    /// What the frame's layers drawn as a whole draw into and composite with.
    pub(crate) layer_composite: &'a mut LayerComposite,
    /// The stencil value of each tile whose mask the stencil buffer holds.
    tile_clipping_mask_ids: VecMap<UnwrappedTileID, i32>,
    next_stencil_id: i32,

    pub(crate) current_layer: u32,
    pub(crate) depth_range_size: f32,
    pub(crate) opaque_pass_cutoff: u32,

    /// The area of the frame's target that its draws draw in.
    pub(crate) scissor: Scissor,
    /// Where the frame draws the map, and the depth it writes there, once
    /// the frame's draw groups are known, before the offscreen passes.
    pub(crate) frame_projection: Option<FrameProjection>,
}

impl<'a> PaintParameters<'a> {
    pub(crate) const NUM_SUBLAYERS: i32 = 3;
    pub(crate) const DEPTH_EPSILON: f32 = 1.0 / (1 << 16) as f32;
    pub(crate) const MAX_STENCIL_VALUE: i32 = 255;

    /// The parameters of the encode of `frame`, into a target of
    /// `renderable_size` device pixels at `pixel_ratio`, drawn within
    /// `scissor`, which clips with `clip_masks` and composites with
    /// `layer_composite`, with the frame's depth where
    /// `composites_with_frame_depth` says it can.
    #[expect(clippy::too_many_arguments, reason = "the renderer's GPU state, the frame and where it draws")]
    pub(crate) fn new(
        context: &'a mut Resources,
        clip_masks: &'a mut ClipMasks,
        layer_composite: &'a mut LayerComposite,
        composites_with_frame_depth: bool,
        frame: &'a FramePlan,
        pixel_ratio: f32,
        renderable_size: Size,
        scissor: Scissor,
    ) -> Self {
        let transform_params = &frame.transform;
        let state = &transform_params.view;
        let mut pixels_to_gl_units = [2.0f32 / state.size().width as f32, -2.0f32 / state.size().height as f32];

        if state.viewport_mode() == ViewportMode::FlippedY {
            pixels_to_gl_units[1] *= -1.0;
        }

        let clip_depth = context.caps().clip_depth;
        PaintParameters {
            context,
            open_pass: None,
            frame: Frame {
                camera: state,
                camera_moving: frame.camera_moving,
                pixel_ratio,
                target_size: renderable_size,
                pixels_to_clip: pixels_to_gl_units,
                light: &frame.light,
                composites_with_frame_depth,
                clip_depth,
                placement_fade: frame.placement_fade,
                transform: transform_params,
            },
            pass: Pass::Opaque,
            debug_options: frame.debug_options,
            clip_masks,
            layer_composite,
            tile_clipping_mask_ids: VecMap::new(),
            next_stencil_id: 1,
            // The renderer sets these before use.
            current_layer: 0,
            depth_range_size: 1.0,
            opaque_pass_cutoff: frame.opaque_pass_cutoff,
            scissor,
            frame_projection: None,
        }
    }

    /// The size and formats of the attachments that draws draw into
    /// now, `None` outside a pass.
    pub(crate) fn pass_target(&self) -> Option<(Size, TargetFormats)> {
        self.open_pass.map(|pass| (pass.size, pass.targets))
    }

    /// Where draws draw now, `None` outside a pass.
    pub(crate) fn draw_target(&self) -> Option<DrawTarget> {
        let (size, targets) = self.pass_target()?;
        Some(DrawTarget { size, targets, scissor: clamped_scissor(self.scissor, size) })
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
            Log::error(Event::Render, format!("Restarting the {} pass failed: {error}", pass.label));
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
                self.copy_frame(gpu);
                drawn = self.end_pass(gpu);
            }
        }
        drawn = drawn.and_then(|()| self.begin_pass(gpu, composite));
        let began = drawn.is_ok();
        if let Err(error) = drawn {
            Log::error(Event::Render, format!("Beginning the layer composite's pass failed: {error}"));
        }
        let frame_stencil = (began && own_stencil).then(|| {
            (std::mem::take(&mut self.tile_clipping_mask_ids), std::mem::replace(&mut self.next_stencil_id, 1))
        });
        Some(Composited { group, frame, began, frame_stencil, unended: Unended })
    }

    /// Copies the layer composite's frame texture onto the open pass.
    fn copy_frame(&mut self, gpu: &mut dyn Gpu) {
        if let Some(target) = self.draw_target() {
            self.layer_composite.copy_frame(gpu, target, self.context);
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
            Log::error(Event::Render, format!("Ending the layer composite's pass failed: {error}"));
        }
        if let Err(error) = self.begin_pass(gpu, frame.loading(LoadOp::Load)) {
            Log::error(Event::Render, format!("Resuming the {} pass failed: {error}", frame.label));
        } else if began && let Some(target) = self.draw_target() {
            // What drew with the frame's depth tested it as it drew, and
            // left its own there, in front of the layer's. The others
            // composite at the depth of the current layer: layers above
            // that drew into the depth buffer hide them.
            let depth = if self.layer_composite.draws_with_frame_depth(group) {
                DepthTest::DISABLED
            } else {
                self.depth_for_sublayer(0, false)
            };
            self.layer_composite.draw(gpu, target, depth, self.context, group);
        }
    }

    /// Ends the frame's pass, which drew into the layer composite's frame
    /// texture, and begins `present`, which the texture is copied into.
    /// Does nothing where the frame drew into its target.
    pub(crate) fn present_frame(&mut self, gpu: &mut dyn Gpu, present: OpenPass) {
        if self.layer_composite.frame_texture().is_none() {
            return;
        }
        if let Err(error) = self.end_pass(gpu).and_then(|()| self.begin_pass(gpu, present)) {
            Log::error(Event::Render, format!("Beginning the frame's copy failed: {error}"));
            return;
        }
        self.copy_frame(gpu);
    }

    /// Whether the current layer draws in the opaque pass: front to back,
    /// with depth. On the globe, 2D layers are ordered by draw order alone;
    /// their Z is the back-hemisphere clip.
    #[must_use]
    pub(crate) fn opaque_pass_enabled_for_layer(&self) -> bool {
        self.current_layer >= self.opaque_pass_cutoff && !self.frame.camera.is_globe()
    }

    #[must_use]
    pub(crate) fn depth_for_sublayer(&self, n: u8, write: bool) -> DepthTest {
        if !self.opaque_pass_enabled_for_layer() {
            return DepthTest::DISABLED;
        }

        // Wrapping unsigned arithmetic, then a float product.
        let steps =
            (1u32.wrapping_add(self.current_layer)).wrapping_mul(Self::NUM_SUBLAYERS as u32).wrapping_add(u32::from(n));
        let depth = self.depth_range_size + steps as f32 * Self::DEPTH_EPSILON;
        DepthTest::less_equal(write, (depth, depth))
    }

    #[must_use]
    pub(crate) fn depth_for_3d(&self, write: bool) -> DepthTest {
        DepthTest::less_equal(write, (0.0, self.depth_range_size))
    }

    pub(crate) fn clear_stencil(&mut self, gpu: &mut dyn Gpu) {
        self.next_stencil_id = 1;
        self.tile_clipping_mask_ids.clear();

        match self.stencil_clear_tile() {
            Some(tile) => self.draw_clip_masks(gpu, &[(tile, 0)]),
            None => self.restart_pass(gpu, LoadOp::Clear(0)),
        }
        self.context.rendering_stats().stencil_clears += 1;
    }

    /// The tile whose mask `clear_stencil` draws zero with, where the render
    /// pass clears the stencil buffer only where it begins.
    #[must_use]
    pub(crate) fn stencil_clear_tile(&self) -> Option<UnwrappedTileID> {
        (!self.context.caps().clear_stencil_with_pass).then(|| UnwrappedTileID::new(0, 0, 0))
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
    pub(crate) fn stencil_for_clipping(&self, tile_id: &UnwrappedTileID) -> Stencil {
        let it = self.tile_clipping_mask_ids.get(tile_id);
        debug_assert!(it.is_some());
        let id = it.copied().unwrap_or(0b0000_0000);
        Stencil {
            state: StencilState {
                test: StencilTest::Equal { mask: 0b1111_1111 },
                write_mask: 0b0000_0000,
                fail: StencilOpType::Keep,
                depth_fail: StencilOpType::Keep,
                pass: StencilOpType::Replace,
            },
            reference: id as u32,
        }
    }

    pub(crate) fn stencil_for_3d(&mut self, gpu: &mut dyn Gpu) -> Stencil {
        if self.next_stencil_id + 1 > Self::MAX_STENCIL_VALUE {
            self.clear_stencil(gpu);
        }

        // We're potentially destroying the stencil clipping mask in this
        // pass. That means we'll have to recreate it for the next source if
        // any.
        self.tile_clipping_mask_ids.clear();

        let id = self.next_stencil_id;
        self.next_stencil_id += 1;
        Stencil {
            state: StencilState {
                test: StencilTest::NotEqual { mask: 0b1111_1111 },
                write_mask: 0b1111_1111,
                fail: StencilOpType::Keep,
                depth_fail: StencilOpType::Keep,
                pass: StencilOpType::Replace,
            },
            reference: id as u32,
        }
    }
}

/// A draw's depth test and write, and the depths that clip z maps to.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct DepthTest {
    pub(crate) state: DepthState,
    /// From near to far.
    pub(crate) range: (f32, f32),
}

impl DepthTest {
    /// No test and no write, over the whole range.
    pub(crate) const DISABLED: DepthTest = DepthTest { state: DepthState::DISABLED, range: (0.0, 1.0) };

    /// A `LessEqual` test that writes where `write`, over `range`.
    fn less_equal(write: bool, range: (f32, f32)) -> DepthTest {
        let state = DepthState { compare: DepthFunctionType::LessEqual, write };
        DepthTest { state, range }
    }
}

/// A draw's stencil test and operations, and the reference value it tests
/// and writes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Stencil {
    pub(crate) state: StencilState,
    pub(crate) reference: u32,
}

impl Stencil {
    /// The stencil left alone.
    pub(crate) const DISABLED: Stencil = Stencil { state: StencilState::DISABLED, reference: 0 };
}

/// `rect` within a target of `size`, `None` where nothing of it is.
fn clamped_scissor(rect: Scissor, size: Size) -> Option<Scissor> {
    let Scissor { x, y, .. } = rect;
    let width = if x < size.width { rect.width.min(size.width - x) } else { 0 };
    let height = if y < size.height { rect.height.min(size.height - y) } else { 0 };
    (width > 0 && height > 0).then_some(Scissor { x, y, width, height })
}
