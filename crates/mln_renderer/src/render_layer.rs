// Ports src/mln/renderer/render_layer.hpp and src/mln/renderer/render_layer.cpp
// (feature queries arrive with queries).
//
// C++ `RenderLayer` is an abstract class with data members; the data is
// `RenderLayerBase` and the virtual interface is the `RenderLayer` trait.
// Render layers are shared between the orchestrator, its ordered list and
// the render tree's items (C++: `unique_ptr` + `reference_wrapper`s), so
// they are `Rc<RefCell<dyn RenderLayer>>`.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::Arc;

use geometry::Point;
use mln_gfx::projection_variant::ProjectionVariant;
use mln_gfx::rendering_stats::{LayerFeaturesMap, NDCBound};
use mln_gfx::{Context, LayerGroupBasePtr, ShaderRegistry};
use mln_map_types::{ProjectedTilePoint, TileProjector, TransformState};
use mln_style::expression::Dependency;
use mln_style::layer::layer_type_info;
use mln_style::layer_properties::LayerProperties;
use mln_style::{PropertyEvaluationParameters, TransitionParameters, layer};
use mln_style_types::{TranslateAnchorType, VisibilityType};
use mln_tile_id::OverscaledTileID;
use mln_util::Color;
use mln_util::constants::EXTENT;
use mln_util::identity::SimpleIdentity;
use mln_util::interpolate::Interpolate;
use mln_util::mat4::{Mat4, matrix};
use mln_util::vectors::{Vec2, Vec3, Vec4};

use crate::bucket::BucketPtr;
use crate::change_request::{ChangeRequest, UniqueChangeRequestVec};
use crate::layer_tweaker::LayerTweakerPtr;
use crate::line_atlas::LineAtlas;
use crate::paint_parameters::PaintParameters;
use crate::pattern_atlas::PatternAtlas;
use crate::render_pass::RenderPass;
use crate::render_source::{RenderSource, RenderTiles};
use crate::render_tile::RenderTile;
use crate::render_tree::RenderTree;
use crate::update_parameters::UpdateParameters;

pub type RenderLayerPtr = Rc<RefCell<dyn RenderLayer>>;
pub type RenderLayerReferences = Vec<RenderLayerPtr>;

/// `LayerRenderData`.
#[derive(Clone)]
pub struct LayerRenderData {
    pub bucket: BucketPtr,
    pub layer_properties: Arc<dyn LayerProperties>,
}

// symbols: `BucketPlacementData`. C++ holds `reference_wrapper`s to the
// bucket and the render tile; here the bucket is shared and the tile is
// kept alive by the vector that owns it.
/// A symbol layer's `*-translate` in pixels, carried into placement the way
/// GL JS passes `translation`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SymbolTranslate {
    pub offset: [f32; 2],
    pub anchor: TranslateAnchorType,
}

impl Default for SymbolTranslate {
    fn default() -> Self {
        SymbolTranslate { offset: [0.0, 0.0], anchor: TranslateAnchorType::Map }
    }
}

/// `BucketPlacementData`.
#[derive(Clone)]
pub struct BucketPlacementData {
    pub bucket: BucketPtr,
    /// The `RenderTile` (`tile`): `tile_owner[tile_index]`.
    pub tile_owner: Rc<Vec<RenderTile>>,
    pub tile_index: usize,
    pub feature_index: Option<Arc<crate::geometry::feature_index::FeatureIndex>>,
    pub source_id: String,
    pub sort_key_range: Option<crate::buckets::symbol_bucket::SortKeyRange>,
    pub text_translate: SymbolTranslate,
    pub icon_translate: SymbolTranslate,
}

impl BucketPlacementData {
    /// `tile`.
    #[must_use]
    pub fn tile(&self) -> &RenderTile {
        &self.tile_owner[self.tile_index]
    }
}

/// `LayerPlacementData = std::list<BucketPlacementData>`.
pub type LayerPlacementData = Vec<BucketPlacementData>;

/// `LayerPrepareParameters`.
pub struct LayerPrepareParameters<'a> {
    pub source: Option<&'a dyn RenderSource>,
    /// conformance: `ImageManager& imageManager` (background-pattern).
    pub image_manager: &'a crate::image_manager::ImageManager,
    pub pattern_atlas: &'a RefCell<PatternAtlas>,
    pub line_atlas: &'a RefCell<LineAtlas>,
    pub state: &'a TransformState,
}

/// `RenderLayer::Stats`.
#[derive(Clone, Debug, Default)]
pub struct Stats {
    /// The features this layer rendered in the current frame, filled by
    /// `update` when `UpdateParameters::capture_rendered_features` is set.
    pub rendered_features: LayerFeaturesMap,
}

impl Stats {
    /// `addRenderedFeature`: add an entry or merge with an existing one.
    pub fn add_rendered_feature(&mut self, feature_id: &str, bound: &NDCBound, tile_ids: HashSet<OverscaledTileID>) {
        mln_gfx::rendering_stats::add_rendered_feature(&mut self.rendered_features, feature_id, bound, tile_ids);
    }
}

/// `RenderLayer::unpack_mix_alpha`: the alpha of a packed pair of colors
/// (as the paint property binders store them), interpolated by `t`.
#[must_use]
pub fn unpack_mix_alpha(packed: &[f32; 4], t: f32) -> f32 {
    fn decode_alpha(encoded: f32) -> f32 {
        let packed_int_value = encoded as i32;
        (packed_int_value - (packed_int_value / 256) * 256) as f32 / 255.0
    }
    decode_alpha(packed[1]).interpolate_f32(&decode_alpha(packed[3]), t)
}

/// The zoom-interpolated value of a data-driven opacity at a vertex (the
/// first two floats of `PaintPropertyBinder::get_vertex_value`).
#[must_use]
pub fn vertex_opacity(vertex: &[f32], zoom_fraction: f64) -> Option<f32> {
    match vertex {
        [a, b, ..] => Some(a.interpolate(b, zoom_fraction)),
        _ => None,
    }
}

/// The zoom-interpolated alpha of a data-driven color at a vertex (the four
/// floats of `PaintPropertyBinder::get_vertex_value`).
#[must_use]
pub fn vertex_color_alpha(vertex: &[f32], zoom_fraction: f64) -> Option<f32> {
    let packed: &[f32; 4] = vertex.get(..4)?.try_into().ok()?;
    Some(unpack_mix_alpha(packed, zoom_fraction as f32))
}

/// Where a tile's features are drawn, for the rendered-feature capture: through
/// the tile matrix on Mercator, through the tile's projection on the globe.
pub enum CaptureView<'a> {
    /// The tile matrix, translated as the layer draws.
    Matrix(Mat4),
    /// The tile's projection, and the layer's translation in tile units.
    Globe { projector: Box<TileProjector<'a>>, translation: Vec2 },
}

impl<'a> CaptureView<'a> {
    /// The view of `tile_id` translated by `translation`, as a layer draws it.
    pub fn new(
        tile_id: &OverscaledTileID,
        state: &'a TransformState,
        transform_params: &crate::paint_parameters::TransformParameters,
        translation: &[f32; 2],
        anchor: mln_style_types::TranslateAnchorType,
        near_clipped: bool,
    ) -> Self {
        let unwrapped = tile_id.to_unwrapped();
        if state.is_globe_rendering() {
            let translate = RenderTile::tile_unit_translation(&unwrapped, translation, anchor, state);
            return CaptureView::Globe {
                projector: Box::new(TileProjector::new(state, &unwrapped)),
                translation: [f64::from(translate[0]), f64::from(translate[1])],
            };
        }
        CaptureView::Matrix(crate::layer_tweaker::get_tile_matrix_for(
            &unwrapped,
            state,
            transform_params,
            translation,
            anchor,
            /*origin=*/ None,
            near_clipped,
            /*in_viewport_pixel_units=*/ false,
            /*aligned=*/ false,
        ))
    }

    /// The NDC bound of `vertex_count` vertices in tile units (z in meters
    /// above the ground) from `get_vertex`, as the view draws them.
    pub fn tile_space_bound(
        &self,
        vertex_count: usize,
        get_vertex: impl FnMut(usize) -> Option<Vec3>,
    ) -> Option<NDCBound> {
        match self {
            CaptureView::Matrix(tile_matrix) => compute_feature_ndc_bound(vertex_count, tile_matrix, false, get_vertex),
            CaptureView::Globe { projector, translation } => {
                compute_globe_feature_ndc_bound(vertex_count, projector, translation, get_vertex)
            }
        }
    }
}

/// What the fill, line, circle and fill-extrusion layers'
/// `captureRenderedFeatures` share: the features `bucket` retained, bounded
/// in NDC.
///
/// `visible(vertex_offset)` is `false` for a feature whose data-driven
/// paint values at its first vertex make it invisible.
/// `bound(view, vertex_offset, vertex_count)` is the NDC bound of the
/// feature whose vertices those are, drawn through `view`: the tile matrix
/// of `tile_id` translated by `translation`, or on the globe the tile's
/// projection.
#[allow(clippy::too_many_arguments)]
pub fn capture_retained_features<'a>(
    stats: &mut Stats,
    bucket: &dyn crate::bucket::Bucket,
    tile_id: &OverscaledTileID,
    state: &'a TransformState,
    transform_params: &crate::paint_parameters::TransformParameters,
    translation: &[f32; 2],
    anchor: mln_style_types::TranslateAnchorType,
    mut visible: impl FnMut(usize) -> bool,
    mut bound: impl FnMut(&CaptureView<'_>, usize, usize) -> Option<NDCBound>,
) {
    let mut view: Option<CaptureView<'a>> = None;
    let features = bucket.get_retained_features();
    stats.rendered_features.reserve(features.len());

    for feature in features {
        debug_assert_ne!(feature.feature_id, "");

        // Consider only the value for the first vertex of this feature, for now.
        let vertex_offset = feature.vertex_offset;
        if !visible(vertex_offset) {
            continue;
        }

        // Compute the view once
        let view = view.get_or_insert_with(|| {
            CaptureView::new(tile_id, state, transform_params, translation, anchor, /*near_clipped=*/ false)
        });

        if let Some(bound) = bound(view, vertex_offset, feature.vertex_count) {
            stats.add_rendered_feature(&feature.feature_id, &bound, HashSet::from([*tile_id]));
        }
    }
}

fn minmax(range: (f64, f64), value: f64) -> (f64, f64) {
    (range.0.min(value), range.1.max(value))
}

/// `RenderLayer::computeFeatureNDCBound`: the NDC bounding box of
/// `vertex_count` vertices from `get_vertex`, or `None` when they are
/// degenerate or entirely outside the viewport.
///
/// The vertices' bounding box is projected by its corners through
/// `tile_matrix`, unless `pre_transformed`, in which case the vertices are
/// already in NDC.
pub fn compute_feature_ndc_bound(
    vertex_count: usize,
    tile_matrix: &Mat4,
    pre_transformed: bool,
    mut get_vertex: impl FnMut(usize) -> Option<Vec3>,
) -> Option<NDCBound> {
    // Compute the feature's bounding box in tile space and project the
    // corners into NDC space. Faster than projecting every vertex, and good
    // enough for our purposes.
    let init_range = (f64::MAX, f64::MIN);
    let mut range_x = init_range;
    let mut range_y = init_range;
    let mut range_z = init_range;
    for i in 0..vertex_count {
        let vertex = get_vertex(i)?;
        range_x = minmax(range_x, vertex[0]);
        range_y = minmax(range_y, vertex[1]);
        range_z = minmax(range_z, vertex[2]);
    }

    if range_x.1 <= range_x.0 || range_y.1 <= range_y.0 || range_z.1 < range_z.0 {
        return None;
    }

    let corners: [Vec4; 8] = [
        [range_x.0, range_y.0, range_z.0, 1.0],
        [range_x.1, range_y.0, range_z.0, 1.0],
        [range_x.1, range_y.1, range_z.0, 1.0],
        [range_x.0, range_y.1, range_z.0, 1.0],
        [range_x.0, range_y.0, range_z.1, 1.0],
        [range_x.1, range_y.0, range_z.1, 1.0],
        [range_x.1, range_y.1, range_z.1, 1.0],
        [range_x.0, range_y.1, range_z.1, 1.0],
    ];
    let corners_count = if range_z.0 < range_z.1 { 8 } else { 4 };

    let mut ndc_range_x = init_range;
    let mut ndc_range_y = init_range;
    let mut ndc_range_z = init_range;
    for view_pos in &corners[..corners_count] {
        let clip_pos = if pre_transformed { *view_pos } else { matrix::transform_mat4_value(view_pos, tile_matrix) };
        ndc_range_x = minmax(ndc_range_x, clip_pos[0] / clip_pos[3]);
        ndc_range_y = minmax(ndc_range_y, clip_pos[1] / clip_pos[3]);
        ndc_range_z = minmax(ndc_range_z, clip_pos[2] / clip_pos[3]);
    }

    if ndc_range_x.0 < ndc_range_x.1
        && ndc_range_y.0 < ndc_range_y.1
        && ndc_range_z.0 <= ndc_range_z.1
        && -1.0 <= ndc_range_x.1
        && ndc_range_x.0 <= 1.0
        && -1.0 <= ndc_range_y.1
        && ndc_range_y.0 <= 1.0
    {
        return Some(NDCBound {
            min_x: ndc_range_x.0,
            max_x: ndc_range_x.1,
            min_y: ndc_range_y.0,
            max_y: ndc_range_y.1,
        });
    }
    None
}

/// `RenderLayer::computeFeatureNDCBound` on the globe: the NDC bound of
/// `vertex_count` vertices in tile units (z in meters above the ground)
/// from `get_vertex`, moved by `translation` and drawn through `projector`,
/// or `None` when none of it is drawn inside the viewport.
///
/// The box bends with the sphere and the horizon can cut through it, so its
/// corners don't bound it: it is sampled every few degrees, keeping what the
/// horizon leaves visible.
pub fn compute_globe_feature_ndc_bound(
    vertex_count: usize,
    projector: &TileProjector<'_>,
    translation: &Vec2,
    mut get_vertex: impl FnMut(usize) -> Option<Vec3>,
) -> Option<NDCBound> {
    const MAX_STEP_DEGREES: f64 = 10.0;
    const MAX_STEPS: f64 = 32.0;

    let init_range = (f64::MAX, f64::MIN);
    let mut range_x = init_range;
    let mut range_y = init_range;
    let mut range_z = init_range;
    for i in 0..vertex_count {
        let vertex = get_vertex(i)?;
        range_x = minmax(range_x, vertex[0]);
        range_y = minmax(range_y, vertex[1]);
        range_z = minmax(range_z, vertex[2]);
    }

    if range_x.1 <= range_x.0 || range_y.1 <= range_y.0 || range_z.1 < range_z.0 {
        return None;
    }

    let tile_id = projector.get_tile_id();
    let degrees_per_tile_unit = 360.0 / f64::from(EXTENT) / 2f64.powi(i32::from(tile_id.canonical.z));
    let steps_for = |range: (f64, f64)| -> usize {
        let degrees = (range.1 - range.0) * degrees_per_tile_unit;
        (degrees / MAX_STEP_DEGREES).ceil().clamp(1.0, MAX_STEPS) as usize
    };
    let steps_x = steps_for(range_x);
    let steps_y = steps_for(range_y);
    let extruded = range_z.0 < range_z.1;

    let mut ndc_range_x = init_range;
    let mut ndc_range_y = init_range;
    let mut include = |projected: &ProjectedTilePoint| {
        if !projected.occluded && projected.signed_distance_from_camera > 0.0 {
            ndc_range_x = minmax(ndc_range_x, projected.point.x);
            ndc_range_y = minmax(ndc_range_y, projected.point.y);
        }
    };
    for iy in 0..=steps_y {
        let y = range_y.0.interpolate(&range_y.1, iy as f64 / steps_y as f64);
        for ix in 0..=steps_x {
            let x = range_x.0.interpolate(&range_x.1, ix as f64 / steps_x as f64);
            let point = Point::new(x + translation[0], y + translation[1]);
            let base = projector.project_as_drawn(&point, range_z.0);
            include(&base);
            if !extruded {
                continue;
            }
            let top = projector.project_as_drawn(&point, range_z.1);
            include(&top);
            if base.occluded != top.occluded {
                // The wall comes into view part way up, where it is drawn at
                // the limb: find that height.
                let (mut hidden, mut shown) =
                    if base.occluded { (range_z.0, range_z.1) } else { (range_z.1, range_z.0) };
                for _ in 0..16 {
                    let mid = f64::midpoint(hidden, shown);
                    if projector.project_as_drawn(&point, mid).occluded {
                        hidden = mid;
                    } else {
                        shown = mid;
                    }
                }
                include(&projector.project_as_drawn(&point, shown));
            }
        }
    }

    // Close up, the visible part of the planet can be smaller than the
    // sampling grid: the surface under the screen's corners, edges and center
    // is drawn wherever it falls inside the box.
    let state = projector.get_transform_state();
    let size = state.get_size();
    for screen_x in [0.0, 0.5, 1.0] {
        for screen_y in [0.0, 0.5, 1.0] {
            let screen = Point::new(screen_x * f64::from(size.width), screen_y * f64::from(size.height));
            let tile = state.screen_coordinate_to_tile_coordinate(&screen, tile_id.canonical.z).p;
            // The globe draws a tile on the same part of the sphere at any wrap.
            let point = Point::new(
                (tile.x - f64::from(tile_id.canonical.x)) * f64::from(EXTENT),
                (tile.y - f64::from(tile_id.canonical.y)) * f64::from(EXTENT),
            );
            let x = point.x - translation[0];
            let y = point.y - translation[1];
            if range_x.0 <= x && x <= range_x.1 && range_y.0 <= y && y <= range_y.1 {
                include(&projector.project_as_drawn(&point, range_z.0));
            }
        }
    }

    if ndc_range_x.0 < ndc_range_x.1
        && ndc_range_y.0 < ndc_range_y.1
        && -1.0 <= ndc_range_x.1
        && ndc_range_x.0 <= 1.0
        && -1.0 <= ndc_range_y.1
        && ndc_range_y.0 <= 1.0
    {
        return Some(NDCBound {
            min_x: ndc_range_x.0,
            max_x: ndc_range_x.1,
            min_y: ndc_range_y.0,
            max_y: ndc_range_y.1,
        });
    }
    None
}

/// The data members of `RenderLayer`.
pub struct RenderLayerBase {
    pub evaluated_properties: Arc<dyn LayerProperties>,
    pub base_impl: Arc<dyn layer::Impl>,

    pub passes: RenderPass,
    pub layer_group: Option<LayerGroupBasePtr>,
    pub layer_tweaker: Option<LayerTweakerPtr>,
    pub layer_index: i32,
    pub style_dependencies: Dependency,
    pub is_renderable: bool,

    pub stats: Stats,

    /// Stores current set of tiles to be rendered for this layer.
    pub render_tiles: Option<RenderTiles>,
    /// Retains ownership of tiles
    pub render_tiles_owner: Rc<Vec<RenderTile>>,
    /// The tile IDs in `render_tiles`, along with the identity of the
    /// bucket from which they were built (C++ `TinyUnorderedMap`; only
    /// looked up).
    pub render_tile_ids: HashMap<OverscaledTileID, SimpleIdentity>,
    // symbols: `placementData`.
    pub placement_data: LayerPlacementData,
    // symbols: `followerData`.
    pub follower_data: LayerPlacementData,
    /// The projection variant this layer's drawables are built with.
    pub projection_variant: ProjectionVariant,
    // TODO(port): sourceForState (feature state), hasRenderFailures.
}

impl RenderLayerBase {
    /// `RenderLayer(Immutable<style::LayerProperties>)`.
    pub fn new(properties: Arc<dyn LayerProperties>) -> Self {
        let base_impl = properties.base_impl.clone();
        RenderLayerBase {
            evaluated_properties: properties,
            base_impl,
            passes: RenderPass::None,
            layer_group: None,
            layer_tweaker: None,
            layer_index: 0,
            style_dependencies: Dependency::NONE,
            is_renderable: false,
            stats: Stats::default(),
            render_tiles: None,
            render_tiles_owner: Rc::new(Vec::new()),
            render_tile_ids: HashMap::new(),
            placement_data: Vec::new(), // symbols
            follower_data: Vec::new(),  // symbols
            projection_variant: ProjectionVariant::Mercator,
        }
    }
}

pub trait RenderLayer {
    fn base(&self) -> &RenderLayerBase;
    fn base_mut(&mut self) -> &mut RenderLayerBase;

    /// Begin transitions for any properties that have changed since the last
    /// frame.
    fn transition(&mut self, parameters: &TransitionParameters);

    /// Fully evaluate possibly-transitioning paint properties based on a
    /// zoom and some evaluated layout properties.
    fn evaluate(&mut self, parameters: &PropertyEvaluationParameters);

    /// Returns true if any paint properties have active transitions.
    fn has_transition(&self) -> bool;

    /// Returns true if the layer has a pattern property and is actively
    /// crossfading.
    fn has_crossfade(&self) -> bool;

    /// Returns true if layer writes to depth buffer by drawing using
    /// `PaintParameters::depthModeFor3D()`.
    fn is_3d(&self) -> bool {
        false
    }

    /// Returns true if the layer may draw depth-tested 3D geometry, which the
    /// planet's depth hides behind the horizon.
    fn needs_globe_depth(&self) -> bool {
        self.is_3d()
    }

    /// `prepare(const LayerPrepareParameters&)`: reads the source's render
    /// tiles (`prepare_base`).
    fn prepare(&mut self, parameters: &mut LayerPrepareParameters<'_>) {
        prepare_base(self, parameters);
    }

    /// TODO: Only for background layers.
    fn get_solid_background(&self) -> Option<Color> {
        None
    }

    /// Update a layer with its render data (drawables).
    #[allow(clippy::too_many_arguments)]
    fn update(
        &mut self,
        _shaders: &ShaderRegistry,
        _context: &dyn Context,
        _state: &TransformState,
        _update_parameters: &Arc<UpdateParameters>,
        _paint_parameters: &PaintParameters<'_>,
        _render_tree: &RenderTree,
        _changes: &mut UniqueChangeRequestVec,
    ) {
    }

    /// Runs a custom layer's prerender, after the layer groups upload and
    /// before the frame's first pass, which draws into `target`.
    fn prerender(&mut self, _parameters: &PaintParameters<'_>, _target: &dyn mln_gfx::DrawTarget) {}

    /// Tells a custom layer's host that the renderer's graphics objects are
    /// lost, and lets go of it.
    fn lose_context(&mut self) {}

    /// Called when the style layer is replaced (same ID and type), and the
    /// render layer is reused.
    fn layer_changed(
        &mut self,
        _parameters: &TransitionParameters,
        _new_layer: &Arc<dyn layer::Impl>,
        _changes: &mut UniqueChangeRequestVec,
    ) {
        // When a layer changes, the bucket won't be replaced until the new
        // source(s) load. If we remove the drawables here, they will just be
        // re-created based on the current data.

        // Detach the layer tweaker, if any. This keeps the existing tiles
        // up-to-date with the most recent evaluated properties, while a new
        // one will be created along with new drawables when tiles are loaded
        // and new buckets are available.
        self.base_mut().layer_tweaker = None;
    }

    /// Called when the style layer is removed.
    fn layer_removed(&mut self, changes: &mut UniqueChangeRequestVec) {
        self.remove_all_drawables();
        let group = self.base().layer_group.clone();
        activate_layer_group(&group, false, changes);
    }

    /// Called when the layer index changes.
    fn layer_index_changed(&mut self, new_layer_index: i32, changes: &mut UniqueChangeRequestVec) {
        self.base_mut().layer_index = new_layer_index;

        // Submit a change request to update the layer index of our tile layer
        // group
        let group = self.base().layer_group.clone();
        change_layer_index(&group, new_layer_index, changes);
    }

    /// Called when the layer is included or excluded from rendering.
    fn mark_layer_renderable(&mut self, will_render: bool, changes: &mut UniqueChangeRequestVec) {
        mark_layer_renderable_base(self, will_render, changes);
    }

    /// Drops what the last `prepare` and `update` built: the drawables, and
    /// the tiles and placement data that keep the tiles' buckets alive.
    /// Called when the layer stops rendering, since neither runs again until
    /// it renders, and they then rebuild everything from the source's tiles.
    fn release_render_data(&mut self) {
        release_render_data_base(self);
    }

    /// Remove all drawables for the tile from the layer group.
    fn remove_all_drawables(&mut self) {
        if let Some(group) = &self.base().layer_group {
            group.borrow_mut().clear_drawables();
        }
    }

    // Non-virtual members of the C++ class.

    /// Picks the shader variant for the state's projection; returns true and
    /// drops the drawables when it changed.
    fn update_projection_variant(&mut self, state: &TransformState) -> bool {
        let variant = if state.is_globe_rendering() { ProjectionVariant::Globe } else { ProjectionVariant::Mercator };
        if variant == self.base().projection_variant {
            return false;
        }
        self.base_mut().projection_variant = variant;
        self.remove_all_drawables();
        true
    }

    /// `addRenderPassesFromTiles()`.
    fn add_render_passes_from_tiles(&mut self) {
        let base = self.base_mut();
        let render_tiles = base.render_tiles.clone().expect("renderTiles");
        for tile in render_tiles.iter() {
            if let Some(render_data) = tile.get_layer_render_data(&*base.base_impl) {
                base.passes |= RenderPass(render_data.layer_properties.render_passes);
            }
        }
    }

    /// Update `renderTileIDs` from `renderTiles`.
    fn update_render_tile_ids(&mut self) {
        let base = self.base_mut();
        let Some(render_tiles) = base.render_tiles.clone().filter(|tiles| !tiles.is_empty()) else {
            base.render_tile_ids.clear();
            return;
        };

        let new_render_tile_ids: HashMap<OverscaledTileID, SimpleIdentity> = render_tiles
            .iter()
            .map(|tile| {
                let tile_id = *tile.get_overscaled_tile_id();
                (tile_id, get_render_tile_bucket_id(base, &tile_id))
            })
            .collect();

        base.render_tile_ids = new_render_tile_ids;
    }

    /// Get the bucket ID from which a given tile was built.
    ///
    /// When a new style is loaded and contains a layer with the same ID,
    /// `layerChanged` will be called during style parsing, but the `Bucket`
    /// in a tile's `RenderData` will only be replaced when the asynchronous
    /// load for the tile is complete, at which point the drawable for the
    /// tile may need to be updated or replaced.
    fn get_render_tile_bucket_id(&self, tile_id: &OverscaledTileID) -> SimpleIdentity {
        get_render_tile_bucket_id(self.base(), tile_id)
    }

    /// Set the bucket ID from which a given tile was built. Returns true if
    /// updated, false if the tile ID is not present in the set of tiles to
    /// be rendered or the ID is unchanged.
    fn set_render_tile_bucket_id(&mut self, tile_id: &OverscaledTileID, bucket_id: SimpleIdentity) -> bool {
        match self.base_mut().render_tile_ids.get_mut(tile_id) {
            Some(id) if *id != bucket_id => {
                *id = bucket_id;
                true
            }
            _ => false,
        }
    }

    fn get_id(&self) -> &str {
        &self.base().base_impl.id
    }

    /// `transition(const TransitionParameters&, Immutable<style::Layer::Impl> newImpl)`.
    fn transition_with_impl(&mut self, parameters: &TransitionParameters, new_impl: Arc<dyn layer::Impl>) {
        self.base_mut().base_impl = new_impl;
        self.transition(parameters);
    }

    /// Returns true if the layer has to be placed.
    fn needs_placement(&self) -> bool {
        self.base().base_impl.get_type_info().cross_tile_index == layer_type_info::CrossTileIndex::Required
            && !self.base().placement_data.is_empty()
    }

    // symbols: `getPlacementData()`.
    fn get_placement_data(&self) -> &LayerPlacementData {
        &self.base().placement_data
    }

    /// symbols: `getFollowerData()`: buckets that another layer places and
    /// this layer draws, at its own translation.
    fn get_follower_data(&self) -> &LayerPlacementData {
        &self.base().follower_data
    }

    fn get_layer_index(&self) -> i32 {
        self.base().layer_index
    }

    /// Checks whether this layer needs to be rendered in the given render
    /// pass.
    fn needs_rendering(&self) -> bool {
        self.base().passes != RenderPass::None && self.base().base_impl.visibility != VisibilityType::None
    }

    /// Checks whether this layer can be rendered.
    fn supports_zoom(&self, zoom: f32) -> bool {
        // TODO: shall we use rounding or epsilon comparisons?
        // A minimum of 0 is "no minimum": the globe zooms below 0 toward the
        // poles. (`std::max(zoom, 0.0f)`.)
        let zoom_or_zero = if zoom < 0.0 { 0.0 } else { zoom };
        self.base().base_impl.min_zoom <= zoom_or_zero && zoom < self.base().base_impl.max_zoom
    }

    fn is_layer_renderable(&self) -> bool {
        self.base().is_renderable
    }

    fn get_style_dependencies(&self) -> Dependency {
        self.base().style_dependencies
    }

    // query: `queryIntersectsFeature(queryGeometry, feature, zoom,
    // transformState, pixelsToTileUnits, projector, featureState)`, with the
    // tile's projector in place of its matrix; C++ returns false unless a
    // layer type overrides it.
    #[allow(clippy::too_many_arguments)]
    fn query_intersects_feature(
        &self,
        _query_geometry: &mln_tile_id::geometry_tile_data::GeometryCoordinates,
        _feature: &dyn mln_tile_id::geometry_tile_data::GeometryTileFeature,
        _zoom: f32,
        _transform_state: &TransformState,
        _pixels_to_tile_units: f32,
        _projector: &mln_map_types::TileProjector<'_>,
        _feature_state: &mln_style::expression::expression::FeatureState,
    ) -> bool {
        false
    }

    /// `setLayerGroup(LayerGroupBasePtr, UniqueChangeRequestVec&)`.
    fn set_layer_group(&mut self, layer_group: Option<LayerGroupBasePtr>, changes: &mut UniqueChangeRequestVec) {
        // Remove the active layer group, if any, before replacing it.
        let old = self.base().layer_group.clone();
        activate_layer_group(&old, false, changes);

        self.base_mut().layer_group = layer_group;

        // Add the new layer group, if we're currently renderable.
        let group = self.base().layer_group.clone();
        let renderable = self.base().is_renderable;
        activate_layer_group(&group, renderable, changes);
    }
}

/// The default `RenderLayer::mark_layer_renderable`, callable from
/// overrides: sets the renderable flag, (un-)registers the layer group, and
/// releases the render data of a layer that stops rendering.
pub fn mark_layer_renderable_base<L: RenderLayer + ?Sized>(
    layer: &mut L,
    will_render: bool,
    changes: &mut UniqueChangeRequestVec,
) {
    layer.base_mut().is_renderable = will_render;

    // This layer is either being freshly included in the renderable set or
    // excluded
    let group = layer.base().layer_group.clone();
    activate_layer_group(&group, will_render, changes);

    if !will_render {
        layer.release_render_data();
    }
}

/// The default `RenderLayer::release_render_data`, callable from overrides.
pub fn release_render_data_base<L: RenderLayer + ?Sized>(layer: &mut L) {
    layer.remove_all_drawables();
    let base = layer.base_mut();
    base.render_tiles = None;
    base.render_tiles_owner = Rc::new(Vec::new());
    base.render_tile_ids.clear();
    base.placement_data.clear();
}

/// The default `RenderLayer::prepare(params)`, callable from overrides (C++
/// `RenderLayer::prepare(params)`). C++ asserts a source; without one (a
/// source type the port has no render source for) the layer gets no tiles.
pub fn prepare_base<L: RenderLayer + ?Sized>(layer: &mut L, parameters: &mut LayerPrepareParameters<'_>) {
    let Some(source) = parameters.source else {
        let base = layer.base_mut();
        base.render_tiles = None;
        base.render_tile_ids.clear();
        return;
    };
    debug_assert!(source.is_enabled());

    // TODO(port): sourceForState = params.source (feature state).
    let render_tiles = source.get_render_tiles();
    let render_tiles_owner = source.get_raw_render_tiles();
    let base = layer.base_mut();
    base.render_tiles = render_tiles;
    base.render_tiles_owner = render_tiles_owner;

    layer.add_render_passes_from_tiles();

    layer.update_render_tile_ids();
}

/// `getRenderTileBucketID(tileID)` on the data members.
fn get_render_tile_bucket_id(base: &RenderLayerBase, tile_id: &OverscaledTileID) -> SimpleIdentity {
    base.render_tile_ids.get(tile_id).copied().unwrap_or(SimpleIdentity::EMPTY)
}

/// (Un-)Register the layer group with the orchestrator.
pub fn activate_layer_group(
    layer_group: &Option<LayerGroupBasePtr>,
    activate: bool,
    changes: &mut UniqueChangeRequestVec,
) {
    if let Some(layer_group) = layer_group {
        if activate {
            // The RenderTree has determined this layer should be included in
            // the renderable set for a frame
            changes.push(ChangeRequest::AddLayerGroup(layer_group.clone()));
        } else {
            // The RenderTree is informing us we should not render anything
            changes.push(ChangeRequest::RemoveLayerGroup(layer_group.clone()));
        }
    }
}

/// `changeLayerIndex(const LayerGroupBasePtr&, int32_t, UniqueChangeRequestVec&)`.
pub fn change_layer_index(group: &Option<LayerGroupBasePtr>, new_index: i32, changes: &mut UniqueChangeRequestVec) {
    if let Some(group) = group
        && group.borrow().get_layer_index() != new_index
    {
        changes.push(ChangeRequest::UpdateLayerGroupIndex(group.clone(), new_index));
    }
}
