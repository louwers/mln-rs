// Ports src/mln/renderer/render_layer.hpp and src/mln/renderer/render_layer.cpp
// (feature queries arrive with queries).
//
// C++ `RenderLayer` is an abstract class with data members; the data is
// `RenderLayerBase` and the virtual interface is the `RenderLayer` trait.
// Render layers are shared between the orchestrator, its ordered list and
// the render tree's items (C++: `unique_ptr` + `reference_wrapper`s), so
// they are `Rc<RefCell<dyn RenderLayer>>`.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

use mln_gfx::projection_variant::ProjectionVariant;
use mln_gfx::{Context, LayerGroupBasePtr, ShaderRegistry, UploadPass};
use mln_map_types::TransformState;
use mln_style::expression::Dependency;
use mln_style::layer::layer_type_info;
use mln_style::layer_properties::LayerProperties;
use mln_style::{PropertyEvaluationParameters, TransitionParameters, layer};
use mln_style_types::{TranslateAnchorType, VisibilityType};
use mln_tile_id::OverscaledTileID;
use mln_util::Color;
use mln_util::identity::SimpleIdentity;
use mln_util::immutable::Immutable;

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
    pub layer_properties: Immutable<dyn LayerProperties>,
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
    pub drawables_added: usize,
    pub drawables_removed: usize,
    // TODO(port): renderedFeatures (rendered-feature capture).
}

/// The data members of `RenderLayer`.
pub struct RenderLayerBase {
    pub evaluated_properties: Immutable<dyn LayerProperties>,
    pub base_impl: Immutable<dyn layer::Impl>,

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
    pub fn new(properties: Immutable<dyn LayerProperties>) -> Self {
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
    /// PaintParameters::depthModeFor3D().
    fn is_3d(&self) -> bool {
        false
    }

    /// Returns true if the layer may draw depth-tested 3D geometry, which the
    /// planet's depth hides behind the horizon.
    fn needs_globe_depth(&self) -> bool {
        self.is_3d()
    }

    fn upload(&mut self, _upload_pass: &dyn UploadPass) {}

    fn render(&mut self, _parameters: &mut PaintParameters<'_>) {}

    /// `prepare(const LayerPrepareParameters&)`: reads the source's render
    /// tiles (`prepare_base`).
    fn prepare(&mut self, parameters: &mut LayerPrepareParameters<'_>) {
        prepare_base(self, parameters);
    }

    fn mark_context_destroyed(&mut self) {
        // no-op
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

    /// Called when the style layer is replaced (same ID and type), and the
    /// render layer is reused.
    fn layer_changed(
        &mut self,
        _parameters: &TransitionParameters,
        _new_layer: &Immutable<dyn layer::Impl>,
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
        self.base_mut().is_renderable = will_render;

        // This layer is either being freshly included in the renderable set
        // or excluded
        let group = self.base().layer_group.clone();
        activate_layer_group(&group, will_render, changes);
    }

    /// Remove all drawables for the tile from the layer group.
    fn remove_all_drawables(&mut self) -> usize {
        let base = self.base_mut();
        if let Some(group) = &base.layer_group {
            let count = group.borrow().get_drawable_count();
            base.stats.drawables_removed += count;
            group.borrow_mut().clear_drawables();
            return count;
        }
        0
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

    /// `getRenderDataForPass(tile, pass)`.
    fn get_render_data_for_pass<'t>(&self, tile: &'t RenderTile, pass: RenderPass) -> Option<&'t LayerRenderData> {
        let render_data = tile.get_layer_render_data(&*self.base().base_impl)?;
        if (RenderPass(render_data.layer_properties.render_passes) & pass) != RenderPass::None {
            Some(render_data)
        } else {
            None
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

    /// Whether a given tile ID is present in the current cover set
    /// (`renderTiles`)
    fn has_render_tile(&self, tile_id: &OverscaledTileID) -> bool {
        self.base().render_tile_ids.contains_key(tile_id)
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
    fn transition_with_impl(&mut self, parameters: &TransitionParameters, new_impl: Immutable<dyn layer::Impl>) {
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

    /// Checks whether the given render pass is used by this layer.
    fn has_render_pass(&self, pass: RenderPass) -> bool {
        (self.base().passes & pass) != RenderPass::None
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
        self.base().base_impl.min_zoom <= zoom_or_zero && self.base().base_impl.max_zoom >= zoom
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
    if let Some(group) = group {
        if group.borrow().get_layer_index() != new_index {
            changes.push(ChangeRequest::UpdateLayerGroupIndex(group.clone(), new_index));
        }
    }
}
