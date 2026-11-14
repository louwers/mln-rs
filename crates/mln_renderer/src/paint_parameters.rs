use std::any::Any;
use std::cell::RefCell;
use std::rc::Rc;

use mln_gfx::backend::Type as BackendType;
use mln_gfx::gpu::{ColorAttachment, DepthStencilAttachment, GpuError, LoadOp, Pass, RenderCommand, TargetFormats};
use mln_gfx::stencil_mode::Test;
use mln_gfx::types::{DepthFunctionType, StencilOpType};
use mln_gfx::{Context, DepthMaskType, DepthMode, RendererBackend, ScissorRect, StencilMode};
use mln_map_types::{MapDebugOptions, ProjectionData, TransformState, ViewportMode};
use mln_tile_id::UnwrappedTileID;
use mln_util::mat4::Mat4;
use mln_util::range::Range;
use mln_util::size::Size;
use mln_vec_map::VecMap;

use crate::clip_masks::ClipMasks;
use crate::line_atlas::LineAtlas;
use crate::pattern_atlas::PatternAtlas;
use crate::render_pass::RenderPass;
use crate::render_static_data::RenderStaticData;

pub(crate) struct TransformParameters {
    pub(crate) proj_matrix: Mat4,
    pub(crate) aligned_proj_matrix: Mat4,
    pub(crate) near_clipped_proj_matrix: Mat4,
    /// The near plane of `near_clipped_proj_matrix`, in pixels.
    pub(crate) near_clipped_near_z: f64,
    pub(crate) state: TransformState,
}

impl TransformParameters {
    pub(crate) fn new(state: &TransformState) -> Self {
        let state = state.clone();
        // Update the default matrices to the current viewport dimensions.
        let proj_matrix = state.get_proj_matrix(1, false);

        // Also compute a projection matrix that aligns with the current pixel
        // grid, taking into account odd viewport sizes.
        let aligned_proj_matrix = state.get_proj_matrix(1, true);

        // Calculate a second projection matrix with the near plane moved
        // further, to a tenth of the far value, so as not to waste depth
        // buffer precision on very close empty space, for layer types
        // (fill-extrusion) that use the depth buffer to emulate real-world
        // space.
        let near_z = (0.1 * f64::from(state.get_camera_to_center_distance())) as u16;
        let near_clipped_proj_matrix = state.get_proj_matrix(near_z, false);

        TransformParameters {
            proj_matrix,
            aligned_proj_matrix,
            near_clipped_proj_matrix,
            near_clipped_near_z: f64::from(near_z),
            state,
        }
    }
}

/// A pass of the backend's `Gpu` that drawables record into, and its
/// attachments' size and formats.
pub(crate) struct RecordedPass {
    pub(crate) pass: Pass,
    pub(crate) size: Size,
    pub(crate) targets: TargetFormats,
}

impl RecordedPass {
    /// A pass with no commands over the same attachments that loads them,
    /// with the stencil as `stencil` says.
    fn loading(&self, stencil: LoadOp<u32>) -> RecordedPass {
        let pass = Pass {
            label: self.pass.label.clone(),
            color: self.pass.color.map(|color| ColorAttachment { load: LoadOp::Load, ..color }),
            depth_stencil: self.pass.depth_stencil.map(|depth_stencil| DepthStencilAttachment {
                depth: LoadOp::Load,
                stencil,
                ..depth_stencil
            }),
            commands: Vec::new(),
        };
        RecordedPass { pass, size: self.size, targets: self.targets }
    }
}

pub struct PaintParameters<'a> {
    pub(crate) context: &'a dyn Context,
    pub(crate) backend: &'a dyn RendererBackend,
    /// The pass of the backend's `Gpu` that drawables record into, while
    /// one is begun.
    pub(crate) recorded: RefCell<Option<RecordedPass>>,
    /// The frame's passes recorded before the one begun, which the next
    /// submit runs first.
    finished: RefCell<Vec<Pass>>,

    pub(crate) transform_params: &'a TransformParameters,
    pub(crate) state: &'a TransformState,
    /// conformance: `const EvaluatedLight& evaluatedLight` (a copy; the
    /// renderer sets it from the render tree's parameters).
    pub(crate) evaluated_light: crate::render_light::EvaluatedLight,
    pub(crate) static_data: &'a RenderStaticData,
    /// The orchestrator's line atlas. `new` starts with an empty one, and the
    /// renderer assigns the orchestrator's.
    pub(crate) line_atlas: Rc<RefCell<LineAtlas>>,
    pub(crate) pattern_atlas: Rc<RefCell<PatternAtlas>>,

    pub(crate) pass: RenderPass,
    pub(crate) debug_options: MapDebugOptions,

    pub(crate) pixel_ratio: f32,
    pub(crate) renderable_size: Size,
    pub(crate) pixels_to_gl_units: [f32; 2],

    /// The masks of the tiles that the frame's layer groups can clip to.
    pub(crate) clip_masks: ClipMasks,
    /// The stencil value of each tile whose mask the stencil buffer holds.
    tile_clipping_mask_ids: VecMap<UnwrappedTileID, i32>,
    next_stencil_id: i32,

    pub(crate) current_layer: u32,
    pub(crate) depth_range_size: f32,
    pub(crate) opaque_pass_cutoff: u32,
    pub(crate) symbol_fade_change: f32,

    pub(crate) scissor_rect: ScissorRect,
    /// The frame that custom layers draw into, set before their prerender.
    pub(crate) custom_layer_frame: Option<mln_style::layers::CustomLayerFrame>,
}

impl<'a> PaintParameters<'a> {
    pub(crate) const NUM_SUBLAYERS: i32 = 3;
    pub(crate) const DEPTH_EPSILON: f32 = 1.0 / (1 << 16) as f32;
    pub(crate) const MAX_STENCIL_VALUE: i32 = 255;

    #[expect(clippy::too_many_arguments, reason = "each argument is a field of the paint parameters")]
    pub(crate) fn new(
        context: &'a dyn Context,
        pixel_ratio: f32,
        backend: &'a dyn RendererBackend,
        debug_options: MapDebugOptions,
        transform_params: &'a TransformParameters,
        static_data: &'a RenderStaticData,
        pattern_atlas: Rc<RefCell<PatternAtlas>>,
        renderable_size: Size,
        scissor_rect: ScissorRect,
    ) -> Self {
        let state = &transform_params.state;
        let mut pixels_to_gl_units = [2.0f32 / state.get_size().width as f32, -2.0f32 / state.get_size().height as f32];

        if state.get_viewport_mode() == ViewportMode::FlippedY {
            pixels_to_gl_units[1] *= -1.0;
        }

        PaintParameters {
            context,
            backend,
            recorded: RefCell::new(None),
            finished: RefCell::new(Vec::new()),
            transform_params,
            state,
            static_data,
            evaluated_light: crate::render_light::EvaluatedLight::default(),
            line_atlas: Rc::new(RefCell::new(LineAtlas::new())),
            pattern_atlas,
            pass: RenderPass::Opaque,
            debug_options,
            pixel_ratio,
            renderable_size,
            pixels_to_gl_units,
            clip_masks: ClipMasks::default(),
            tile_clipping_mask_ids: VecMap::new(),
            next_stencil_id: 1,
            // The renderer sets these before use.
            current_layer: 0,
            depth_range_size: 1.0,
            opaque_pass_cutoff: 0,
            symbol_fade_change: 0.0,
            scissor_rect,
            custom_layer_frame: None,
        }
    }

    /// The size and formats of the attachments that drawables draw into
    /// now, `None` outside a pass.
    pub(crate) fn pass_target(&self) -> Option<(Size, TargetFormats)> {
        self.recorded.borrow().as_ref().map(|recorded| (recorded.size, recorded.targets))
    }

    /// Records `commands`, a drawable's, into the pass begun.
    pub(crate) fn record(&self, commands: &[RenderCommand]) {
        if let Some(recorded) = &mut *self.recorded.borrow_mut() {
            recorded.pass.commands.extend_from_slice(commands);
        }
    }

    /// Ends the pass begun, and begins one over its attachments that loads
    /// them, with the stencil as `stencil` says.
    fn restart_pass(&self, stencil: LoadOp<u32>) {
        if let Some(recorded) = &mut *self.recorded.borrow_mut() {
            let next = recorded.loading(stencil);
            self.finished.borrow_mut().push(std::mem::replace(recorded, next).pass);
        }
    }

    /// Runs `f` with the backend's commands inside the pass begun, after
    /// what the frame recorded so far, as a custom layer draws: `f` gets the
    /// backend's `CustomLayerPass`. The pass then goes on in one that loads
    /// its attachments. Returns whether `f` ran.
    pub(crate) fn lend_pass(&self, f: &mut dyn FnMut(&mut dyn Any)) -> Result<bool, GpuError> {
        let Some(recorded) = self.recorded.borrow_mut().take() else {
            return Ok(false);
        };
        let next = recorded.loading(LoadOp::Load);
        let finished = std::mem::take(&mut *self.finished.borrow_mut());
        let mut gpu = self.context.gpu();
        let lent = gpu.submit(&finished).and_then(|()| gpu.lend_to_custom_layer(Some(&recorded.pass), f));
        drop(gpu);
        *self.recorded.borrow_mut() = Some(next);
        lent.map(|()| true)
    }

    /// Submits the passes the frame recorded, ending the one begun.
    pub(crate) fn submit(&self) -> Result<(), GpuError> {
        let mut passes = std::mem::take(&mut *self.finished.borrow_mut());
        passes.extend(self.recorded.borrow_mut().take().map(|recorded| recorded.pass));
        self.context.gpu().submit(&passes)
    }

    #[must_use]
    pub(crate) fn projection_data_for_tile(&self, tile_id: &UnwrappedTileID, aligned: bool) -> ProjectionData {
        self.state.get_projection_data_with(
            tile_id,
            if aligned { &self.transform_params.aligned_proj_matrix } else { &self.transform_params.proj_matrix },
        )
    }

    #[must_use]
    pub(crate) fn depth_mode_for_sublayer(&self, n: u8, mask: DepthMaskType) -> DepthMode {
        // On the globe, 2D layers are ordered by draw order alone; their Z is
        // the back-hemisphere clip.
        if self.current_layer < self.opaque_pass_cutoff || self.state.is_globe_rendering() {
            return DepthMode::disabled();
        }

        // Wrapping unsigned arithmetic, then a float product.
        let steps =
            (1u32.wrapping_add(self.current_layer)).wrapping_mul(Self::NUM_SUBLAYERS as u32).wrapping_add(u32::from(n));
        let depth = self.depth_range_size + steps as f32 * Self::DEPTH_EPSILON;
        DepthMode { func: DepthFunctionType::LessEqual, mask, range: Range::new(depth, depth) }
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
        DepthMode { func: DepthFunctionType::LessEqual, mask, range: Range::new(0.0, self.depth_range_size) }
    }

    pub(crate) fn clear_stencil(&mut self) {
        self.next_stencil_id = 1;
        self.tile_clipping_mask_ids.clear();

        match self.stencil_clear_tile() {
            Some(tile) => self.draw_clip_masks(&[(tile, 0)]),
            None => self.restart_pass(LoadOp::Clear(0)),
        }
        self.context.rendering_stats().stencil_clears += 1;
    }

    /// The tile whose mask `clear_stencil` draws zero with, where the render
    /// pass clears the stencil buffer only where it begins.
    #[must_use]
    pub(crate) fn stencil_clear_tile(&self) -> Option<UnwrappedTileID> {
        (self.context.backend_type() != BackendType::Vulkan).then(|| UnwrappedTileID::new(0, 0, 0))
    }

    fn draw_clip_masks(&mut self, masks: &[(UnwrappedTileID, i32)]) {
        let mut clip_masks = std::mem::take(&mut self.clip_masks);
        clip_masks.draw(masks, self);
        self.clip_masks = clip_masks;
    }

    /// Renders the clipping masks of `tile_ids`, unless the stencil buffer
    /// already holds exactly theirs.
    pub(crate) fn render_tile_clipping_masks(&mut self, tile_ids: &[UnwrappedTileID]) {
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
            self.clear_stencil();
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
            self.draw_clip_masks(&masks);
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

    pub(crate) fn stencil_mode_for_3d(&mut self) -> StencilMode {
        if self.next_stencil_id + 1 > Self::MAX_STENCIL_VALUE {
            self.clear_stencil();
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

    /// Give each drawable a 3D stencil mode of its own, clearing the stencil
    /// buffer first if they would not all fit, so no value handed out before
    /// the clear is drawn after it. Returns false, handing out none, when
    /// there are more drawables than the buffer holds values: the caller then
    /// takes each one's value with `stencil_mode_for_3d` as it draws it, so a
    /// clear falls between two draws.
    pub(crate) fn stencil_modes_for_3d(&mut self, modes: &mut [StencilMode]) -> bool {
        if modes.len() >= Self::MAX_STENCIL_VALUE as usize {
            return false;
        }
        if self.next_stencil_id as usize + modes.len() > Self::MAX_STENCIL_VALUE as usize {
            self.clear_stencil();
        }
        for mode in modes.iter_mut() {
            *mode = self.stencil_mode_for_3d();
        }
        true
    }
}
