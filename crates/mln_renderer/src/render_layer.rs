// C++ `RenderLayer` is an abstract class with data members; the data is
// `RenderLayerBase` and the virtual interface is the `RenderLayer` trait.
// The orchestrator owns the render layers, in the style's order, and each
// render layer owns the layer groups it draws, with their tweakers.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use mln_gfx::ShaderRegistry;
use mln_gfx::projection_variant::ProjectionVariant;
use mln_map_types::{TileProjector, TransformState};
use mln_style::expression::Dependency;
use mln_style::layer_type::{ErasedLayerStyle, LayerStyle, PaintSet};
use mln_style::{PropertyEvaluationParameters, TransitionParameters};
use mln_style_types::{TranslateAnchorType, VisibilityType};
use mln_tile_id::OverscaledTileID;
use mln_util::Color;
use mln_util::hash::HashMap;
use mln_util::identity::SimpleIdentity;

use crate::bucket::BucketPtr;
use crate::gfx::context::Context;
use crate::layer_group::LayerGroup;
use crate::layer_tweaker::LayerTweaker;
use crate::layer_type::{ErasedEvaluatedLayer, EvaluatedLayer, LayerType};
use crate::line_atlas::LineAtlas;
use crate::paint_parameters::PaintParameters;
use crate::pattern_atlas::PatternAtlas;
use crate::render_pass::RenderPass;
use crate::render_source::{RenderSource, RenderTiles};
use crate::render_tile::RenderTile;
use crate::update_parameters::UpdateParameters;

#[derive(Clone)]
pub(crate) struct LayerRenderData {
    pub(crate) bucket: BucketPtr,
    /// The values of the layer that the bucket was laid out with.
    pub(crate) layer: Arc<dyn ErasedEvaluatedLayer>,
}

impl LayerRenderData {
    /// The bucket, as a bucket of a `T` layer, or `None` for another type's.
    pub(crate) fn bucket<T: LayerType>(&self) -> Option<&T::Bucket> {
        (*self.bucket).downcast_ref()
    }

    /// The values the bucket was laid out with, as a `T` layer's, or `None`
    /// for another type's.
    pub(crate) fn layer<T: LayerType>(&self) -> Option<&EvaluatedLayer<T>> {
        let any: &dyn std::any::Any = &*self.layer;
        any.downcast_ref()
    }
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
    pub(crate) bucket: Arc<crate::buckets::symbol_bucket::SymbolBucket>,
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
    /// The group of the layer's drawables, which draws while the layer is
    /// renderable.
    pub(crate) layer_group: Option<LayerGroup>,
    /// Sets the uniforms of the group's drawables each frame.
    pub(crate) layer_tweaker: Option<Box<dyn LayerTweaker>>,
    pub(crate) style_dependencies: Dependency,
    /// Whether the frame draws the layer.
    pub(crate) is_renderable: bool,

    /// Stores current set of tiles to be rendered for this layer.
    pub(crate) render_tiles: Option<RenderTiles>,
    /// Retains ownership of tiles
    pub(crate) render_tiles_owner: Rc<Vec<RenderTile>>,
    /// The tile IDs in `render_tiles`, along with the identity of the
    /// bucket from which they were built (C++ `TinyUnorderedMap`; only
    /// looked up).
    pub(crate) render_tile_ids: HashMap<OverscaledTileID, Option<SimpleIdentity>>,
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
            style_dependencies: Dependency::NONE,
            is_renderable: false,
            render_tiles: None,
            render_tiles_owner: Rc::new(Vec::new()),
            render_tile_ids: HashMap::default(),
            projection_variant: ProjectionVariant::Mercator,
        }
    }
}

/// A layer's values of its type `T`: its style, its paint values in
/// transition, and their evaluation for the last frame.
pub(crate) struct LayerValues<T: LayerType> {
    pub(crate) style: Arc<LayerStyle<T>>,
    pub(crate) unevaluated: <T::PaintProperties as PaintSet>::Unevaluated,
    pub(crate) evaluated: Arc<EvaluatedLayer<T>>,
}

impl<T: LayerType> LayerValues<T> {
    /// The values of the layer `style`, before any transition, and the base
    /// of a render layer that holds them.
    pub(crate) fn new(style: Arc<LayerStyle<T>>) -> (RenderLayerBase, Self) {
        let unevaluated = style.paint.untransitioned();
        let evaluated = Arc::new(EvaluatedLayer::new(Arc::clone(&style)));
        let mut base = RenderLayerBase::new(Arc::<EvaluatedLayer<T>>::clone(&evaluated));
        base.style_dependencies = T::PaintProperties::dependencies(&unevaluated);
        (base, LayerValues { style, unevaluated, evaluated })
    }

    /// The paint values evaluated for a frame, copying from the last
    /// evaluation each value that can't have changed, with the frame's
    /// cross-fade for a type that has one and no render passes.
    pub(crate) fn evaluate(&mut self, parameters: &PropertyEvaluationParameters) -> EvaluatedLayer<T> {
        let paint =
            T::PaintProperties::evaluate_with_previous(&mut self.unevaluated, parameters, &self.evaluated.paint);
        let mut evaluated = EvaluatedLayer::with_paint(Arc::clone(&self.style), paint);
        if T::CROSSFADES {
            evaluated.crossfade = parameters.get_crossfade_parameters();
        }
        evaluated
    }

    /// Makes `evaluated` the layer's values, drawing in `base`'s passes,
    /// and gives them to the layer's tweaker.
    pub(crate) fn set_evaluated(&mut self, base: &mut RenderLayerBase, mut evaluated: EvaluatedLayer<T>) {
        evaluated.render_passes = base.passes.underlying_type();
        self.evaluated = Arc::new(evaluated);
        base.evaluated_properties = Arc::<EvaluatedLayer<T>>::clone(&self.evaluated);
        if let Some(layer_tweaker) = &mut base.layer_tweaker {
            layer_tweaker.update_properties(Arc::clone(&base.evaluated_properties));
        }
    }
}

/// A render layer whose values are of the layer type `Type`. Its base and
/// values give it [`LayerParts`].
pub(crate) trait TypedLayer {
    type Type: LayerType;

    fn parts(&self) -> (&RenderLayerBase, &LayerValues<Self::Type>);
    fn parts_mut(&mut self) -> (&mut RenderLayerBase, &mut LayerValues<Self::Type>);
}

/// What a render layer holds whatever its type: its base, and its values,
/// which this transitions without knowing their type.
pub(crate) trait LayerParts {
    fn base(&self) -> &RenderLayerBase;
    fn base_mut(&mut self) -> &mut RenderLayerBase;

    /// Makes `style`, the changed style of the layer, the layer's, if it is
    /// a layer of its type.
    fn set_style(&mut self, style: &Arc<dyn ErasedLayerStyle>);

    /// Transitions the layer's paint values to those of its style.
    fn transition_values(&mut self, parameters: &TransitionParameters);

    /// Whether a paint value of the layer is still in transition.
    fn values_in_transition(&self) -> bool;

    /// Whether the layer's evaluation is in a cross-fade.
    fn values_crossfading(&self) -> bool;
}

impl<L: TypedLayer> LayerParts for L {
    fn base(&self) -> &RenderLayerBase {
        self.parts().0
    }

    fn base_mut(&mut self) -> &mut RenderLayerBase {
        self.parts_mut().0
    }

    fn set_style(&mut self, style: &Arc<dyn ErasedLayerStyle>) {
        let (base, values) = self.parts_mut();
        if let Some(recovered) = LayerStyle::recover(style) {
            values.style = recovered;
            base.style = Arc::clone(style);
        }
    }

    fn transition_values(&mut self, parameters: &TransitionParameters) {
        let (base, values) = self.parts_mut();
        let prior = std::mem::take(&mut values.unevaluated);
        values.unevaluated = values.style.paint.transitioned(parameters, prior);
        base.style_dependencies =
            <L::Type as mln_style::layer_type::StyleLayerType>::PaintProperties::dependencies(&values.unevaluated);
    }

    fn values_in_transition(&self) -> bool {
        <L::Type as mln_style::layer_type::StyleLayerType>::PaintProperties::has_transition(&self.parts().1.unevaluated)
    }

    fn values_crossfading(&self) -> bool {
        L::Type::CROSSFADES && self.parts().1.evaluated.crossfade.t != 1.0
    }
}

pub(crate) trait RenderLayer: LayerParts {
    /// Begin transitions for any properties that have changed since the last
    /// frame.
    fn transition(&mut self, parameters: &TransitionParameters) {
        self.transition_values(parameters);
    }

    /// Fully evaluate possibly-transitioning paint properties based on a
    /// zoom and some evaluated layout properties.
    fn evaluate(&mut self, parameters: &PropertyEvaluationParameters);

    /// Returns true if any paint properties have active transitions.
    fn has_transition(&self) -> bool {
        self.values_in_transition()
    }

    /// Returns true if the layer has a pattern property and is actively
    /// crossfading.
    fn has_crossfade(&self) -> bool {
        self.values_crossfading()
    }

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
    fn update(
        &mut self,
        _shaders: &ShaderRegistry,
        _context: &Context,
        _state: &TransformState,
        _update_parameters: &Arc<UpdateParameters>,
        _paint_parameters: &PaintParameters<'_>,
    ) {
    }

    /// Runs the layer's tweakers on its layer groups that draw, before they
    /// upload.
    fn tweak(&mut self, parameters: &PaintParameters<'_>) {
        let base = self.base_mut();
        if base.is_renderable
            && let (Some(tweaker), Some(group)) = (&mut base.layer_tweaker, &mut base.layer_group)
        {
            tweaker.execute(group, parameters);
        }
    }

    /// Adds the layer's groups that draw to `groups`, in drawing order.
    fn layer_groups<'a>(&'a mut self, groups: &mut Vec<&'a mut LayerGroup>) {
        let base = self.base_mut();
        if base.is_renderable
            && let Some(group) = &mut base.layer_group
        {
            groups.push(group);
        }
    }

    /// Uploads the drawables of the layer's render targets, which draw
    /// before the frame's passes.
    fn upload_render_targets(&mut self, _context: &Context) {}

    /// Draws the layer's render targets, ahead of the frame's passes, which
    /// sample them.
    fn render_targets(&mut self, _parameters: &mut PaintParameters<'_>) {}

    /// Runs a custom layer's prerender, after the layer groups upload and
    /// before the frame's first pass, which draws into `target`.
    fn prerender(&mut self, _parameters: &PaintParameters<'_>) {}

    /// Tells a custom layer's host that the renderer's graphics objects are
    /// lost, and lets go of it.
    fn lose_context(&mut self) {}

    /// Called when the style layer is replaced (same ID and type), and the
    /// render layer is reused.
    fn layer_changed(&mut self, _parameters: &TransitionParameters, _new_layer: &Arc<dyn ErasedLayerStyle>) {
        // When a layer changes, the bucket won't be replaced until the new
        // source(s) load. If we remove the drawables here, they will just be
        // re-created based on the current data.

        // Detach the layer tweaker, if any. This keeps the existing tiles
        // up-to-date with the most recent evaluated properties, while a new
        // one will be created along with new drawables when tiles are loaded
        // and new buckets are available.
        self.base_mut().layer_tweaker = None;
    }

    /// Called when the layer is included or excluded from rendering.
    fn mark_layer_renderable(&mut self, will_render: bool) {
        mark_layer_renderable_base(self, will_render);
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
        if let Some(group) = &mut self.base_mut().layer_group {
            group.clear_drawables();
        }
    }

    /// Moves what the layer keeps for each tile to the tile `wrap_delta`
    /// world copies east of it: the tiles are the same ones under new ids
    /// once the map's center has crossed the antimeridian, and their
    /// drawables stay rather than being built again.
    fn handle_wrap_jump(&mut self, wrap_delta: i16) {
        handle_wrap_jump_base(self, wrap_delta);
    }

    /// Adds the groups that hold the drawables of the layer's tiles to
    /// `groups`.
    fn tile_layer_groups<'a>(&'a mut self, groups: &mut Vec<&'a mut LayerGroup>) {
        groups.extend(self.base_mut().layer_group.as_mut());
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
    /// transitions to it. A style of another type leaves the layer's.
    fn restyle(&mut self, parameters: &TransitionParameters, style: &Arc<dyn ErasedLayerStyle>) {
        self.set_style(style);
        self.transition(parameters);
    }

    /// Returns true if the layer has to be placed.
    fn needs_placement(&self) -> bool {
        !self.get_placement_data().is_empty()
    }

    /// symbols: the buckets the layer places, which only a symbol layer
    /// has.
    fn get_placement_data(&self) -> &[BucketPlacementData] {
        &[]
    }

    /// symbols: the buckets that another layer places and this layer draws,
    /// at its own translation.
    fn get_follower_data(&self) -> &[BucketPlacementData] {
        &[]
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
}

/// The default `RenderLayer::mark_layer_renderable`, callable from
/// overrides: sets the renderable flag, and releases the render data of a
/// layer that stops rendering.
pub(crate) fn mark_layer_renderable_base<L: RenderLayer + ?Sized>(layer: &mut L, will_render: bool) {
    layer.base_mut().is_renderable = will_render;

    if !will_render {
        layer.release_render_data();
    }
}

/// The default `RenderLayer::handle_wrap_jump`, callable from overrides.
pub(crate) fn handle_wrap_jump_base<L: RenderLayer + ?Sized>(layer: &mut L, wrap_delta: i16) {
    let base = layer.base_mut();
    base.render_tile_ids = std::mem::take(&mut base.render_tile_ids)
        .into_iter()
        .map(|(id, bucket)| (id.unwrap_to(id.wrap.wrapping_add(wrap_delta)), bucket))
        .collect();
    let mut groups = Vec::new();
    layer.tile_layer_groups(&mut groups);
    for group in groups {
        group.shift_wraps(wrap_delta);
    }
}

/// The default `RenderLayer::release_render_data`, callable from overrides.
pub(crate) fn release_render_data_base<L: RenderLayer + ?Sized>(layer: &mut L) {
    layer.remove_all_drawables();
    let base = layer.base_mut();
    base.render_tiles = None;
    base.render_tiles_owner = Rc::new(Vec::new());
    base.render_tile_ids.clear();
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
