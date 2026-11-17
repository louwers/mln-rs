// C++ `RenderLayer` is an abstract class with data members; the data is
// `RenderLayerBase` and the virtual interface is the `RenderLayer` trait.
// Render layers are shared between the orchestrator, its ordered list and
// the render tree's items (C++: `unique_ptr` + `reference_wrapper`s), so
// they are `Rc<RefCell<dyn RenderLayer>>`.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use mln_gfx::ShaderRegistry;
use mln_gfx::projection_variant::ProjectionVariant;
use mln_map_types::{TileProjector, TransformState};
use mln_style::expression::Dependency;
use mln_style::layer_type::{ErasedLayerStyle, LayerStyle, StyleLayerType};
use mln_style::{PropertyEvaluationParameters, TransitionParameters};
use mln_style_types::{TranslateAnchorType, VisibilityType};
use mln_tile_id::OverscaledTileID;
use mln_util::Color;
use mln_util::hash::HashMap;
use mln_util::identity::SimpleIdentity;

use crate::bucket::BucketPtr;
use crate::change_request::{ChangeRequest, UniqueChangeRequestVec};
use crate::gfx::context::Context;
use crate::layer_group::LayerGroupPtr;
use crate::layer_tweaker::LayerTweakerPtr;
use crate::layer_type::ErasedEvaluatedLayer;
use crate::line_atlas::LineAtlas;
use crate::paint_parameters::PaintParameters;
use crate::pattern_atlas::PatternAtlas;
use crate::render_pass::RenderPass;
use crate::render_source::{RenderSource, RenderTiles};
use crate::render_tile::RenderTile;
use crate::render_tree::RenderTree;
use crate::update_parameters::UpdateParameters;

pub(crate) type RenderLayerPtr = Rc<RefCell<dyn RenderLayer>>;

#[derive(Clone)]
pub(crate) struct LayerRenderData {
    pub(crate) bucket: BucketPtr,
    /// The values of the layer that the bucket was laid out with.
    pub(crate) layer: Arc<dyn ErasedEvaluatedLayer>,
}

// symbols: `BucketPlacementData`. C++ holds `reference_wrapper`s to the
// bucket and the render tile; here the bucket is shared and the tile is
// kept alive by the vector that owns it.
/// A symbol layer's `*-translate` in pixels, carried into placement the way
/// GL JS passes `translation`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct SymbolTranslate {
    pub(crate) offset: [f32; 2],
    pub(crate) anchor: TranslateAnchorType,
}

impl Default for SymbolTranslate {
    fn default() -> Self {
        SymbolTranslate { offset: [0.0, 0.0], anchor: TranslateAnchorType::Map }
    }
}

#[derive(Clone)]
pub(crate) struct BucketPlacementData {
    pub(crate) bucket: BucketPtr,
    /// The `RenderTile` (`tile`): `tile_owner[tile_index]`.
    pub(crate) tile_owner: Rc<Vec<RenderTile>>,
    pub(crate) tile_index: usize,
    pub(crate) feature_index: Option<Arc<crate::geometry::feature_index::FeatureIndex>>,
    pub(crate) source_id: String,
    pub(crate) sort_key_range: Option<crate::buckets::symbol_bucket::SortKeyRange>,
    pub(crate) text_translate: SymbolTranslate,
    pub(crate) icon_translate: SymbolTranslate,
}

impl BucketPlacementData {
    /// `tile`.
    #[must_use]
    pub(crate) fn tile(&self) -> &RenderTile {
        &self.tile_owner[self.tile_index]
    }
}

pub(crate) type LayerPlacementData = Vec<BucketPlacementData>;

pub(crate) struct LayerPrepareParameters<'a> {
    pub(crate) source: Option<&'a dyn RenderSource>,
    /// conformance: `ImageManager& imageManager` (background-pattern): the image store.
    pub(crate) images: &'a crate::image_store::ImageStore,
    pub(crate) pattern_atlas: &'a RefCell<PatternAtlas>,
    pub(crate) line_atlas: &'a RefCell<LineAtlas>,
}

/// The data members of `RenderLayer`.
pub(crate) struct RenderLayerBase {
    pub(crate) evaluated_properties: Arc<dyn ErasedEvaluatedLayer>,
    pub(crate) style: Arc<dyn ErasedLayerStyle>,

    pub(crate) passes: RenderPass,
    pub(crate) layer_group: Option<LayerGroupPtr>,
    pub(crate) layer_tweaker: Option<LayerTweakerPtr>,
    pub(crate) layer_index: i32,
    pub(crate) style_dependencies: Dependency,
    pub(crate) is_renderable: bool,

    /// Stores current set of tiles to be rendered for this layer.
    pub(crate) render_tiles: Option<RenderTiles>,
    /// Retains ownership of tiles
    pub(crate) render_tiles_owner: Rc<Vec<RenderTile>>,
    /// The tile IDs in `render_tiles`, along with the identity of the
    /// bucket from which they were built (C++ `TinyUnorderedMap`; only
    /// looked up).
    pub(crate) render_tile_ids: HashMap<OverscaledTileID, Option<SimpleIdentity>>,
    // symbols: `placementData`.
    pub(crate) placement_data: LayerPlacementData,
    // symbols: `followerData`.
    pub(crate) follower_data: LayerPlacementData,
    /// The projection variant this layer's drawables are built with.
    pub(crate) projection_variant: ProjectionVariant,
}

impl RenderLayerBase {
    pub(crate) fn new(properties: Arc<dyn ErasedEvaluatedLayer>) -> Self {
        let style = properties.shared_style();
        RenderLayerBase {
            evaluated_properties: properties,
            style,
            passes: RenderPass::None,
            layer_group: None,
            layer_tweaker: None,
            layer_index: 0,
            style_dependencies: Dependency::NONE,
            is_renderable: false,
            render_tiles: None,
            render_tiles_owner: Rc::new(Vec::new()),
            render_tile_ids: HashMap::default(),
            placement_data: Vec::new(), // symbols
            follower_data: Vec::new(),  // symbols
            projection_variant: ProjectionVariant::Mercator,
        }
    }
}

pub(crate) trait RenderLayer {
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

    /// Reads the source's render tiles (`prepare_base`).
    fn prepare(&mut self, parameters: &mut LayerPrepareParameters<'_>) {
        prepare_base(self, parameters);
    }

    /// TODO: Only for background layers.
    fn get_solid_background(&self) -> Option<Color> {
        None
    }

    /// Update a layer with its render data (drawables).
    #[expect(
        clippy::too_many_arguments,
        reason = "the shaders, context, camera, update and paint parameters, render tree and change list that a layer's drawables depend on"
    )]
    fn update(
        &mut self,
        _shaders: &ShaderRegistry,
        _context: &Context,
        _state: &TransformState,
        _update_parameters: &Arc<UpdateParameters>,
        _paint_parameters: &PaintParameters<'_>,
        _render_tree: &RenderTree,
        _changes: &mut UniqueChangeRequestVec,
    ) {
    }

    /// Runs a custom layer's prerender, after the layer groups upload and
    /// before the frame's first pass, which draws into `target`.
    fn prerender(&mut self, _parameters: &PaintParameters<'_>) {}

    /// Tells a custom layer's host that the renderer's graphics objects are
    /// lost, and lets go of it.
    fn lose_context(&mut self) {}

    /// Called when the style layer is replaced (same ID and type), and the
    /// render layer is reused.
    fn layer_changed(
        &mut self,
        _parameters: &TransitionParameters,
        _new_layer: &Arc<dyn ErasedLayerStyle>,
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
        activate_layer_group(group.as_ref(), false, changes);
    }

    /// Called when the layer index changes.
    fn layer_index_changed(&mut self, new_layer_index: i32, changes: &mut UniqueChangeRequestVec) {
        self.base_mut().layer_index = new_layer_index;

        // Submit a change request to update the layer index of our tile layer
        // group
        let group = self.base().layer_group.clone();
        change_layer_index(group.as_ref(), new_layer_index, changes);
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

    fn add_render_passes_from_tiles(&mut self) {
        let base = self.base_mut();
        let render_tiles = base.render_tiles.clone().expect("renderTiles");
        for tile in render_tiles.iter() {
            if let Some(render_data) = tile.get_layer_render_data(&*base.style) {
                base.passes |= RenderPass(render_data.layer.render_passes());
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

        let new_render_tile_ids: HashMap<OverscaledTileID, Option<SimpleIdentity>> = render_tiles
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
    fn get_render_tile_bucket_id(&self, tile_id: &OverscaledTileID) -> Option<SimpleIdentity> {
        get_render_tile_bucket_id(self.base(), tile_id)
    }

    /// Set the bucket ID from which a given tile was built. Returns true if
    /// updated, false if the tile ID is not present in the set of tiles to
    /// be rendered or the ID is unchanged.
    fn set_render_tile_bucket_id(&mut self, tile_id: &OverscaledTileID, bucket_id: SimpleIdentity) -> bool {
        match self.base_mut().render_tile_ids.get_mut(tile_id) {
            Some(id) if *id != Some(bucket_id) => {
                *id = Some(bucket_id);
                true
            }
            _ => false,
        }
    }

    fn get_id(&self) -> &str {
        &self.base().style.base().id
    }

    /// Makes `style`, the changed style of the layer, the layer's, and
    /// transitions to it. Through [`set_style`], a style of another type
    /// leaves the layer's.
    fn restyle(&mut self, parameters: &TransitionParameters, style: &Arc<dyn ErasedLayerStyle>);

    /// Returns true if the layer has to be placed.
    fn needs_placement(&self) -> bool {
        self.base().style.info().cross_tile_index && !self.base().placement_data.is_empty()
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

    /// Checks whether this layer needs to be rendered in the given render
    /// pass.
    fn needs_rendering(&self) -> bool {
        self.base().passes != RenderPass::None && self.base().style.base().visibility != VisibilityType::None
    }

    /// Checks whether this layer can be rendered.
    fn supports_zoom(&self, zoom: f32) -> bool {
        // TODO: shall we use rounding or epsilon comparisons?
        // A minimum of 0 is "no minimum": the globe zooms below 0 toward the
        // poles. (`std::max(zoom, 0.0f)`.)
        let zoom_or_zero = if zoom < 0.0 { 0.0 } else { zoom };
        self.base().style.base().min_zoom <= zoom_or_zero && zoom < self.base().style.base().max_zoom
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
    #[expect(
        clippy::too_many_arguments,
        reason = "the query geometry, the feature and its state, and the zoom, camera and projector that test them"
    )]
    fn query_intersects_feature(
        &self,
        _query_geometry: &mln_tile_id::geometry_tile_data::GeometryCoordinates,
        _feature: &dyn mln_tile_id::geometry_tile_data::GeometryTileFeature,
        _geometry: &mln_tile_id::geometry_tile_data::GeometryCollection,
        _zoom: f32,
        _transform_state: &TransformState,
        _pixels_to_tile_units: f32,
        _projector: &TileProjector<'_>,
        _feature_state: &mln_style::expression::expression::FeatureState,
    ) -> bool {
        false
    }

    fn set_layer_group(&mut self, layer_group: Option<LayerGroupPtr>, changes: &mut UniqueChangeRequestVec) {
        // Remove the active layer group, if any, before replacing it.
        let old = self.base().layer_group.clone();
        activate_layer_group(old.as_ref(), false, changes);

        self.base_mut().layer_group = layer_group;

        // Add the new layer group, if we're currently renderable.
        let group = self.base().layer_group.clone();
        let renderable = self.base().is_renderable;
        activate_layer_group(group.as_ref(), renderable, changes);
    }
}

/// Makes `style` the style of the render layer whose members are `base` and
/// whose style is `typed`, if it is a `T` layer's.
pub(crate) fn set_style<T: StyleLayerType>(
    base: &mut RenderLayerBase,
    typed: &mut Arc<LayerStyle<T>>,
    style: &Arc<dyn ErasedLayerStyle>,
) {
    if let Some(recovered) = LayerStyle::recover(style) {
        *typed = recovered;
        base.style = Arc::clone(style);
    }
}

/// The default `RenderLayer::mark_layer_renderable`, callable from
/// overrides: sets the renderable flag, (un-)registers the layer group, and
/// releases the render data of a layer that stops rendering.
pub(crate) fn mark_layer_renderable_base<L: RenderLayer + ?Sized>(
    layer: &mut L,
    will_render: bool,
    changes: &mut UniqueChangeRequestVec,
) {
    layer.base_mut().is_renderable = will_render;

    // This layer is either being freshly included in the renderable set or
    // excluded
    let group = layer.base().layer_group.clone();
    activate_layer_group(group.as_ref(), will_render, changes);

    if !will_render {
        layer.release_render_data();
    }
}

/// The default `RenderLayer::release_render_data`, callable from overrides.
pub(crate) fn release_render_data_base<L: RenderLayer + ?Sized>(layer: &mut L) {
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
pub(crate) fn prepare_base<L: RenderLayer + ?Sized>(layer: &mut L, parameters: &mut LayerPrepareParameters<'_>) {
    let Some(source) = parameters.source else {
        let base = layer.base_mut();
        base.render_tiles = None;
        base.render_tile_ids.clear();
        return;
    };
    debug_assert!(source.is_enabled());

    let render_tiles = source.get_render_tiles();
    let render_tiles_owner = source.get_raw_render_tiles();
    let base = layer.base_mut();
    base.render_tiles = render_tiles;
    base.render_tiles_owner = render_tiles_owner;

    layer.add_render_passes_from_tiles();

    layer.update_render_tile_ids();
}

/// The bucket id of the render tile `tile_id`, or `None` if there is none.
fn get_render_tile_bucket_id(base: &RenderLayerBase, tile_id: &OverscaledTileID) -> Option<SimpleIdentity> {
    base.render_tile_ids.get(tile_id).copied().flatten()
}

/// (Un-)Register the layer group with the orchestrator.
pub(crate) fn activate_layer_group(
    layer_group: Option<&LayerGroupPtr>,
    activate: bool,
    changes: &mut UniqueChangeRequestVec,
) {
    if let Some(layer_group) = layer_group {
        if activate {
            // The RenderTree has determined this layer should be included in
            // the renderable set for a frame
            changes.push(ChangeRequest::AddLayerGroup(Rc::clone(layer_group)));
        } else {
            // The RenderTree is informing us we should not render anything
            changes.push(ChangeRequest::RemoveLayerGroup(Rc::clone(layer_group)));
        }
    }
}

pub(crate) fn change_layer_index(group: Option<&LayerGroupPtr>, new_index: i32, changes: &mut UniqueChangeRequestVec) {
    if let Some(group) = group
        && group.borrow().layer_index != new_index
    {
        changes.push(ChangeRequest::UpdateLayerGroupIndex(Rc::clone(group), new_index));
    }
}
