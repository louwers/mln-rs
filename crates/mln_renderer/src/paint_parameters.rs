// Ports src/mln/renderer/paint_parameters.hpp and src/mln/renderer/paint_parameters.cpp
// (the Metal and WebGPU branches, which are the same, and the Vulkan one).

use std::cell::RefCell;
use std::rc::Rc;

use mln_gfx::backend::Type as BackendType;
use mln_gfx::color_mode::{BlendFunction, LinearBlend, Mask};
use mln_gfx::globe_clip_mask::GlobeClipMask;
use mln_gfx::paint_parameters::PaintParameters as GfxPaintParameters;
use mln_gfx::render_pass::RenderPass as GfxRenderPass;
use mln_gfx::shaders::clipping_mask_ubo::ClipUBO;
use mln_gfx::stencil_mode::Test;
use mln_gfx::types::{ColorBlendFactorType, DepthFunctionType, StencilOpType};
use mln_gfx::{
    ColorMode, CommandEncoder, Context, DepthMaskType, DepthMode, RendererBackend, ScissorRect, ShaderRegistry,
    StencilMode,
};
use mln_map_types::{MapDebugOptions, MapMode, ProjectionData, TileLodMode, TransformState, ViewportMode};
use mln_tile_id::{CanonicalTileID, UnwrappedTileID};
use mln_util::Color;
use mln_util::chrono::TimePoint;
use mln_util::mat4::Mat4;
use mln_util::range::Range;
use mln_util::size::Size;
use mln_util::subdivision_granularity::SubdivisionGranularitySetting;
use mln_vec_map::VecMap;

use crate::globe_tile_mesh::raw_globe_tile_mesh;
use crate::layer_tweaker::to_projection_ubo;
use crate::line_atlas::LineAtlas;
use crate::pattern_atlas::PatternAtlas;
use crate::render_pass::RenderPass;
use crate::render_static_data::RenderStaticData;

/// `TransformParameters`.
pub struct TransformParameters {
    pub proj_matrix: Mat4,
    pub aligned_proj_matrix: Mat4,
    pub near_clipped_proj_matrix: Mat4,
    /// The near plane of `near_clipped_proj_matrix`, in pixels.
    pub near_clipped_near_z: f64,
    pub state: TransformState,
}

impl TransformParameters {
    pub fn new(state: &TransformState) -> Self {
        let state = state.clone();
        // C++ leaves the matrices uninitialized; getProjMatrix fills them
        // unless the size is empty.
        let mut proj_matrix: Mat4 = [0.0; 16];
        let mut aligned_proj_matrix: Mat4 = [0.0; 16];
        let mut near_clipped_proj_matrix: Mat4 = [0.0; 16];

        // Update the default matrices to the current viewport dimensions.
        state.get_proj_matrix(&mut proj_matrix, 1, false);

        // Also compute a projection matrix that aligns with the current pixel
        // grid, taking into account odd viewport sizes.
        state.get_proj_matrix(&mut aligned_proj_matrix, 1, true);

        // Calculate a second projection matrix with the near plane moved
        // further, to a tenth of the far value, so as not to waste depth
        // buffer precision on very close empty space, for layer types
        // (fill-extrusion) that use the depth buffer to emulate real-world
        // space.
        let near_z = (0.1 * f64::from(state.get_camera_to_center_distance())) as u16;
        state.get_proj_matrix(&mut near_clipped_proj_matrix, near_z, false);

        TransformParameters {
            proj_matrix,
            aligned_proj_matrix,
            near_clipped_proj_matrix,
            near_clipped_near_z: f64::from(near_z),
            state,
        }
    }
}

pub struct PaintParameters<'a> {
    pub context: &'a dyn Context,
    pub backend: &'a dyn RendererBackend,
    pub encoder: Option<Box<dyn CommandEncoder>>,
    pub render_pass: Option<Box<dyn GfxRenderPass>>,

    pub transform_params: &'a TransformParameters,
    pub state: &'a TransformState,
    /// conformance: `const EvaluatedLight& evaluatedLight` (a copy; the
    /// renderer sets it from the render tree's parameters).
    pub evaluated_light: crate::render_light::EvaluatedLight,
    pub static_data: &'a RenderStaticData,
    /// `LineAtlas& lineAtlas`: the orchestrator's atlas (`RenderTree::
    /// getLineAtlas()`); `new` starts with an empty one, the renderer
    /// assigns the orchestrator's.
    pub line_atlas: Rc<RefCell<LineAtlas>>,
    pub pattern_atlas: Rc<RefCell<PatternAtlas>>,

    pub pass: RenderPass,
    pub map_mode: MapMode,
    pub debug_options: MapDebugOptions,
    pub time_point: TimePoint,

    pub pixel_ratio: f32,
    pub renderable_size: Size,
    pub pixels_to_gl_units: [f32; 2],

    pub shaders: &'a ShaderRegistry,

    tile_clipping_mask_ids: VecMap<UnwrappedTileID, i32>,
    next_stencil_id: i32,

    pub current_layer: u32,
    pub depth_range_size: f32,
    pub opaque_pass_cutoff: u32,
    pub symbol_fade_change: f32,
    pub frame_count: u64,

    pub tile_lod_min_radius: f64,
    pub tile_lod_scale: f64,
    pub tile_lod_pitch_threshold: f64,
    pub tile_lod_mode: TileLodMode,

    pub scissor_rect: ScissorRect,
    pub capture_rendered_features: bool,
}

/// The WebGPU context's `rawGlobeTileMesh(tile, borders,
/// SubdivisionGranularitySetting::globe().stencil)`, in the form the gfx trait
/// takes it (see `Context::render_globe_tile_clipping_masks`).
fn globe_clip_mesh(tile: &CanonicalTileID, borders: bool) -> (Vec<u8>, Vec<u16>) {
    let mesh = raw_globe_tile_mesh(tile, borders, &SubdivisionGranularitySetting::globe().stencil);
    (mesh.vertices, mesh.indices)
}

impl<'a> PaintParameters<'a> {
    pub const NUM_SUBLAYERS: i32 = 3;
    pub const DEPTH_EPSILON: f32 = 1.0 / (1 << 16) as f32;
    pub const MAX_STENCIL_VALUE: i32 = 255;

    #[allow(clippy::too_many_arguments)]
    pub fn new(
        context: &'a dyn Context,
        pixel_ratio: f32,
        backend: &'a dyn RendererBackend,
        map_mode: MapMode,
        debug_options: MapDebugOptions,
        time_point: TimePoint,
        transform_params: &'a TransformParameters,
        static_data: &'a RenderStaticData,
        pattern_atlas: Rc<RefCell<PatternAtlas>>,
        frame_count: u64,
        tile_lod_min_radius: f64,
        tile_lod_scale: f64,
        tile_lod_pitch_threshold: f64,
        tile_lod_mode: TileLodMode,
        renderable_size: Size,
        scissor_rect: ScissorRect,
        capture_rendered_features: bool,
    ) -> Self {
        let state = &transform_params.state;
        let mut pixels_to_gl_units = [2.0f32 / state.get_size().width as f32, -2.0f32 / state.get_size().height as f32];

        if state.get_viewport_mode() == ViewportMode::FlippedY {
            pixels_to_gl_units[1] *= -1.0;
        }

        PaintParameters {
            context,
            backend,
            encoder: Some(context.create_command_encoder()),
            render_pass: None,
            transform_params,
            state,
            static_data,
            evaluated_light: crate::render_light::EvaluatedLight::default(),
            line_atlas: Rc::new(RefCell::new(LineAtlas::new())),
            pattern_atlas,
            pass: RenderPass::Opaque,
            map_mode,
            debug_options,
            time_point,
            pixel_ratio,
            renderable_size,
            pixels_to_gl_units,
            shaders: &static_data.shaders,
            tile_clipping_mask_ids: VecMap::new(),
            next_stencil_id: 1,
            // C++ leaves these uninitialized; the renderer sets them before
            // use.
            current_layer: 0,
            depth_range_size: 1.0,
            opaque_pass_cutoff: 0,
            symbol_fade_change: 0.0,
            frame_count,
            tile_lod_min_radius,
            tile_lod_scale,
            tile_lod_pitch_threshold,
            tile_lod_mode,
            scissor_rect,
            capture_rendered_features,
        }
    }

    /// `matrixForTile(const UnwrappedTileID&, bool aligned = false)`.
    #[must_use]
    pub fn matrix_for_tile(&self, tile_id: &UnwrappedTileID, aligned: bool) -> Mat4 {
        self.projection_data_for_tile(tile_id, aligned).main_matrix
    }

    /// `projectionDataForTile(const UnwrappedTileID&, bool aligned = false)`.
    #[must_use]
    pub fn projection_data_for_tile(&self, tile_id: &UnwrappedTileID, aligned: bool) -> ProjectionData {
        self.state.get_projection_data_with(
            tile_id,
            if aligned { &self.transform_params.aligned_proj_matrix } else { &self.transform_params.proj_matrix },
        )
    }

    #[must_use]
    pub fn depth_mode_for_sublayer(&self, n: u8, mask: DepthMaskType) -> DepthMode {
        // On the globe, 2D layers are ordered by draw order alone; their Z is
        // the back-hemisphere clip.
        if self.current_layer < self.opaque_pass_cutoff || self.state.is_globe_rendering() {
            return DepthMode::disabled();
        }

        // C++: `((1 + currentLayer) * numSublayers + n)`, unsigned arithmetic, times a float.
        let steps =
            (1u32.wrapping_add(self.current_layer)).wrapping_mul(Self::NUM_SUBLAYERS as u32).wrapping_add(u32::from(n));
        let depth = self.depth_range_size + steps as f32 * Self::DEPTH_EPSILON;
        DepthMode { func: DepthFunctionType::LessEqual, mask, range: Range::new(depth, depth) }
    }

    #[must_use]
    pub fn depth_mode_for_3d(&self, mask: DepthMaskType) -> DepthMode {
        DepthMode { func: DepthFunctionType::LessEqual, mask, range: Range::new(0.0, self.depth_range_size) }
    }

    #[must_use]
    pub fn color_mode_for_render_pass(&self) -> ColorMode {
        if self.debug_options.contains(MapDebugOptions::OVERDRAW) {
            let overdraw: f32 = 1.0 / 8.0;
            ColorMode {
                blend_function: BlendFunction::Add(LinearBlend {
                    src_factor: ColorBlendFactorType::ConstantColor,
                    dst_factor: ColorBlendFactorType::One,
                }),
                blend_color: Color::new_unchecked(overdraw, overdraw, overdraw, 0.0),
                mask: Mask { r: true, g: true, b: true, a: true },
            }
        } else if self.pass == RenderPass::Translucent {
            ColorMode::alpha_blended()
        } else {
            ColorMode::unblended()
        }
    }

    pub fn clear_stencil(&mut self) {
        self.next_stencil_id = 1;
        self.tile_clipping_mask_ids.clear();

        let render_pass = self.render_pass.as_deref().expect("render pass");
        if self.context.backend_type() == BackendType::Vulkan {
            // MLN_RENDER_BACKEND_VULKAN
            render_pass.clear_stencil(0);
        } else if self.state.is_globe_rendering() {
            // The render pass clears the stencil only where it begins;
            // within it, draw zero over the zoom 0 tile, as Metal does.
            let masks = [GlobeClipMask {
                projection: to_projection_ubo(&self.projection_data_for_tile(&UnwrappedTileID::new(0, 0, 0), false)),
                stencil_ref: 0,
                tile: CanonicalTileID::new(0, 0, 0),
            }];
            self.context.render_globe_tile_clipping_masks(render_pass, self.shaders, &masks, &globe_clip_mesh);
        } else {
            let matrix = self.matrix_for_tile(&UnwrappedTileID::new(0, 0, 0), false);
            let tile_ubo = [ClipUBO {
                matrix: std::array::from_fn(|i| matrix[i] as f32),
                stencil_ref: 0,
                pad1: 0.0,
                pad2: 0.0,
                pad3: 0.0,
            }];
            self.context.render_tile_clipping_masks(render_pass, self.shaders, &tile_ubo);
        }
        self.context.rendering_stats().stencil_clears += 1;
    }

    /// `renderTileClippingMasks(const RenderTiles&)`, with the tile ids.
    pub fn render_tile_clipping_masks_for(&mut self, tile_ids: &[UnwrappedTileID]) -> bool {
        // We can avoid updating the mask if it already contains the same set
        // of tiles.
        let covered = self.tile_clipping_mask_ids.len() == tile_ids.len()
            && self.tile_clipping_mask_ids.keys().zip(tile_ids.iter()).all(|(a, b)| a == b);
        if covered {
            return true;
        }

        if self.render_pass.is_none() {
            return false;
        }

        self.tile_clipping_mask_ids.clear();

        // If the stencil value will overflow, clear the target to ensure
        // ensure that none of the new values remain set somewhere in it.
        // Otherwise we can continue to overwrite it incrementally.
        let count = tile_ids.len();
        if self.next_stencil_id as usize + count > Self::MAX_STENCIL_VALUE as usize {
            self.clear_stencil();
        }
        if !self.state.is_globe_rendering() {
            self.context.release_globe_clip_masks();
        }

        // MLN_RENDER_BACKEND_METAL, MLN_RENDER_BACKEND_VULKAN and
        // MLN_RENDER_BACKEND_WEBGPU, the same.
        let globe = self.state.is_globe_rendering();
        let mut tile_ubos: Vec<ClipUBO> = Vec::new();
        let mut globe_masks: Vec<GlobeClipMask> = Vec::new();
        for tile_id in tile_ids {
            let stencil_id = self.next_stencil_id;
            if self.tile_clipping_mask_ids.contains_key(tile_id) {
                continue;
            }
            self.tile_clipping_mask_ids.insert(*tile_id, stencil_id);
            self.next_stencil_id += 1;

            if globe {
                globe_masks.push(GlobeClipMask {
                    projection: to_projection_ubo(&self.projection_data_for_tile(tile_id, false)),
                    stencil_ref: stencil_id as u32,
                    tile: tile_id.canonical,
                });
                continue;
            }

            if tile_ubos.is_empty() {
                tile_ubos.reserve(count);
            }

            let matrix = self.matrix_for_tile(tile_id, false);
            tile_ubos.push(ClipUBO {
                matrix: std::array::from_fn(|i| matrix[i] as f32),
                stencil_ref: stencil_id as u32,
                pad1: 0.0,
                pad2: 0.0,
                pad3: 0.0,
            });
        }

        if !tile_ubos.is_empty() || !globe_masks.is_empty() {
            let render_pass = self.render_pass.as_deref().expect("render pass");
            if globe {
                self.context.render_globe_tile_clipping_masks(
                    render_pass,
                    self.shaders,
                    &globe_masks,
                    &globe_clip_mesh,
                );
            } else {
                self.context.render_tile_clipping_masks(render_pass, self.shaders, &tile_ubos);
            }
            self.context.rendering_stats().stencil_updates += 1;
        }

        true
    }

    #[must_use]
    pub fn stencil_mode_for_clipping(&self, tile_id: &UnwrappedTileID) -> StencilMode {
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

    pub fn stencil_mode_for_3d(&mut self) -> StencilMode {
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
    /// clear falls between two draws. (C++ takes the modes keyed by drawable;
    /// see the gfx trait.)
    pub fn stencil_modes_for_3d(&mut self, modes: &mut [StencilMode]) -> bool {
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

impl GfxPaintParameters for PaintParameters<'_> {
    fn context(&self) -> &dyn Context {
        self.context
    }

    fn encoder(&self) -> Option<&dyn CommandEncoder> {
        self.encoder.as_deref()
    }

    fn render_pass(&self) -> Option<&dyn GfxRenderPass> {
        self.render_pass.as_deref()
    }

    fn pass(&self) -> RenderPass {
        self.pass
    }

    fn scissor_rect(&self) -> ScissorRect {
        self.scissor_rect
    }

    fn depth_mode_for_sublayer(&self, n: u8, mask: DepthMaskType) -> DepthMode {
        PaintParameters::depth_mode_for_sublayer(self, n, mask)
    }

    fn depth_mode_for_3d(&self, mask: DepthMaskType) -> DepthMode {
        PaintParameters::depth_mode_for_3d(self, mask)
    }

    fn render_tile_clipping_masks(&mut self, tile_ids: &[UnwrappedTileID]) -> bool {
        self.render_tile_clipping_masks_for(tile_ids)
    }

    fn stencil_mode_for_clipping(&self, tile_id: &UnwrappedTileID) -> StencilMode {
        PaintParameters::stencil_mode_for_clipping(self, tile_id)
    }

    fn stencil_mode_for_3d(&mut self) -> StencilMode {
        PaintParameters::stencil_mode_for_3d(self)
    }

    fn stencil_modes_for_3d(&mut self, modes: &mut [StencilMode]) -> bool {
        PaintParameters::stencil_modes_for_3d(self, modes)
    }

    // conformance: for DrawableAtlasesTweaker.
    fn state_pitch(&self) -> f64 {
        self.state.get_pitch()
    }

    fn state_is_changing(&self) -> bool {
        self.state.is_changing()
    }
}
