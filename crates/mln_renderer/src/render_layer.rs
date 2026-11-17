// C++ `RenderLayer` is an abstract class with data members; the data is
// `RenderLayerBase` and the virtual interface is the `RenderLayer` trait.
// The orchestrator owns the render layers, in the style's order, with what
// it keeps of each one's drawing (`LayerDraws`).

use std::cell::RefCell;
use std::sync::Arc;

use mln_gfx::ShaderRegistry;
use mln_gfx::projection_variant::ProjectionVariant;
use mln_map_types::{ProjectionData, TileProjector, TransformState};
use mln_style::expression::Dependency;
use mln_style::layer_type::{ErasedLayerStyle, LayerStyle, PaintSet};
use mln_style::{PropertyEvaluationParameters, TransitionParameters};
use mln_style_types::{TranslateAnchorType, VisibilityType};
use mln_tile_id::{OverscaledTileID, UnwrappedTileID};
use mln_util::Color;
use mln_util::mat4::Mat4;

use crate::bucket::BucketPtr;
use crate::draw_cache::{DrawGroup, LayerDraws};
use crate::gfx::context::Context;
use crate::layer_type::{ErasedEvaluatedLayer, EvaluatedLayer, LayerType};
use crate::line_atlas::LineAtlas;
use crate::paint_parameters::PaintParameters;
use crate::pattern_atlas::PatternAtlas;
use crate::render_pass::RenderPass;
use crate::render_source::RenderSource;
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

/// What placement reads of the render tile of a symbol bucket, as the frame
/// that prepared the bucket's placement data set it.
#[derive(Clone, Copy, Debug)]
pub(crate) struct PlacedTile {
    pub(crate) id: UnwrappedTileID,
    pub(crate) overscaled_id: OverscaledTileID,
    pub(crate) hold_for_fade: bool,
    /// The tile's projection; `matrix` is its main matrix.
    pub(crate) projection: ProjectionData,
    pub(crate) matrix: Mat4,
}

impl PlacedTile {
    #[must_use]
    pub(crate) fn new(tile: &RenderTile) -> PlacedTile {
        PlacedTile {
            id: tile.id,
            overscaled_id: *tile.get_overscaled_tile_id(),
            hold_for_fade: tile.hold_for_fade(),
            projection: tile.projection,
            matrix: tile.matrix,
        }
    }
}

/// symbols: a symbol bucket a layer places or draws, with its tile.
#[derive(Clone)]
pub(crate) struct BucketPlacementData {
    pub(crate) bucket: Arc<crate::buckets::symbol_bucket::SymbolBucket>,
    pub(crate) tile: PlacedTile,
    pub(crate) feature_index: Option<Arc<crate::geometry::feature_index::FeatureIndex>>,
    pub(crate) source_id: String,
    pub(crate) sort_key_range: Option<crate::buckets::symbol_bucket::SortKeyRange>,
    pub(crate) text_translate: SymbolTranslate,
    pub(crate) icon_translate: SymbolTranslate,
}

/// The tiles a layer draws in a frame: a selection of its source's render
/// tiles, in the order the layer draws them.
#[derive(Clone, Copy)]
pub(crate) struct LayerTiles<'a> {
    tiles: &'a [RenderTile],
    order: &'a [usize],
}

impl<'a> LayerTiles<'a> {
    /// The tiles of `tiles` at the indexes of `order`, in that order.
    #[must_use]
    pub(crate) fn new(tiles: &'a [RenderTile], order: &'a [usize]) -> LayerTiles<'a> {
        LayerTiles { tiles, order }
    }

    pub(crate) fn iter(self) -> impl Iterator<Item = &'a RenderTile> + 'a {
        self.order.iter().filter_map(|&i| self.tiles.get(i))
    }

    /// The tiles in order, each with its index in the source's tiles
    /// ([`source_tiles`](Self::source_tiles)).
    pub(crate) fn indexed(self) -> impl Iterator<Item = (usize, &'a RenderTile)> + 'a {
        self.order.iter().filter_map(|&i| Some((i, self.tiles.get(i)?)))
    }

    /// All the source's tiles, which the order indexes.
    #[must_use]
    pub(crate) fn source_tiles(self) -> &'a [RenderTile] {
        self.tiles
    }

    #[must_use]
    pub(crate) fn is_empty(self) -> bool {
        self.order.is_empty()
    }
}

pub(crate) struct LayerPrepareParameters<'a> {
    /// The layer's source, `None` for a layer without one or a source type
    /// the renderer has no render source for.
    pub(crate) source: Option<&'a dyn RenderSource>,
    /// What the renderer keeps of the layer's drawing, whose tiles become
    /// the frame's.
    pub(crate) draws: &'a mut LayerDraws,
    /// conformance: `ImageManager& imageManager` (background-pattern): the image store.
    pub(crate) images: &'a crate::image_store::ImageStore,
    pub(crate) pattern_atlas: &'a RefCell<PatternAtlas>,
    pub(crate) line_atlas: &'a RefCell<LineAtlas>,
}

impl<'a> LayerPrepareParameters<'a> {
    /// The source's tiles at the indexes of `order`; `None` without either.
    #[must_use]
    pub(crate) fn tiles<'b>(&self, order: Option<&'b [usize]>) -> Option<LayerTiles<'b>>
    where
        'a: 'b,
    {
        let source = self.source?;
        Some(LayerTiles::new(source.render_tiles(), order?))
    }
}

/// The data members of `RenderLayer`.
pub(crate) struct RenderLayerBase {
    pub(crate) evaluated_properties: Arc<dyn ErasedEvaluatedLayer>,
    pub(crate) style: Arc<dyn ErasedLayerStyle>,

    pub(crate) passes: RenderPass,
    pub(crate) style_dependencies: Dependency,
    /// Whether the frame draws the layer.
    pub(crate) is_renderable: bool,

    /// The projection variant this layer's draws are built with.
    pub(crate) projection_variant: ProjectionVariant,
}

impl RenderLayerBase {
    pub(crate) fn new(properties: Arc<dyn ErasedEvaluatedLayer>) -> Self {
        let style = properties.shared_style();
        RenderLayerBase {
            evaluated_properties: properties,
            style,
            passes: RenderPass::None,
            style_dependencies: Dependency::NONE,
            is_renderable: false,
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

    /// Makes `evaluated` the layer's values, drawing in `base`'s passes.
    pub(crate) fn set_evaluated(&mut self, base: &mut RenderLayerBase, mut evaluated: EvaluatedLayer<T>) {
        evaluated.render_passes = base.passes.underlying_type();
        self.evaluated = Arc::new(evaluated);
        base.evaluated_properties = Arc::<EvaluatedLayer<T>>::clone(&self.evaluated);
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

    /// Picks the tiles of the source the layer draws in the frame, and
    /// returns their order, as indexes into the source's render tiles
    /// (`prepare_base`); `None` for a layer without tiles. The frame's update
    /// gets them in that order.
    fn prepare(&mut self, parameters: &mut LayerPrepareParameters<'_>) -> Option<Vec<usize>> {
        prepare_base(self, parameters)
    }

    /// TODO: Only for background layers.
    fn get_solid_background(&self) -> Option<Color> {
        None
    }

    /// Makes the layer's draws of the frame in `draws`, from the frame's
    /// `tiles` of the layer, the order its `prepare` returned, and what
    /// `draws` keeps of them, with the values of their uniforms.
    #[expect(
        clippy::too_many_arguments,
        reason = "the shaders, context, camera and frame parameters a layer's draws are made with, with its tiles and what it keeps of them"
    )]
    fn update(
        &mut self,
        _shaders: &ShaderRegistry,
        _context: &Context,
        _state: &TransformState,
        _update_parameters: &Arc<UpdateParameters>,
        _paint_parameters: &PaintParameters<'_>,
        _tiles: Option<LayerTiles<'_>>,
        _draws: &mut LayerDraws,
    ) {
    }

    /// Adds what the frame draws of the layer, of `draws`, to `groups`, in
    /// drawing order.
    fn draw_groups<'a>(&'a mut self, draws: &'a mut LayerDraws, groups: &mut Vec<DrawGroup<'a>>) {
        if self.base().is_renderable {
            draws.draw_groups(groups);
        }
    }

    /// Uploads what draws into the layer's render targets, which draw before
    /// the frame's passes, with what the renderer keeps of its drawing.
    fn upload_render_targets(&mut self, _context: &Context, _draws: &mut LayerDraws) {}

    /// Draws the layer's render targets, ahead of the frame's passes, which
    /// sample them, with what the renderer keeps of its drawing.
    fn render_targets(&mut self, _parameters: &mut PaintParameters<'_>, _draws: &mut LayerDraws) {}

    /// Runs a custom layer's prerender, after the layers' draws upload and
    /// before the frame's first pass, which draws into `target`.
    fn prerender(&mut self, _parameters: &PaintParameters<'_>) {}

    /// Tells a custom layer's host that the renderer's graphics objects are
    /// lost, and lets go of it.
    fn lose_context(&mut self) {}

    /// Called when the style layer is replaced (same ID and type), and the
    /// render layer is reused, with what the renderer keeps of its drawing:
    /// drops the draws, which were built for the previous style's values.
    /// The next update builds them again from the tiles' buckets.
    fn layer_changed(
        &mut self,
        _parameters: &TransitionParameters,
        _new_layer: &Arc<dyn ErasedLayerStyle>,
        draws: &mut LayerDraws,
    ) {
        self.remove_all_draws(draws);
    }

    /// Called when the layer is included or excluded from rendering.
    fn mark_layer_renderable(&mut self, will_render: bool, draws: &mut LayerDraws) {
        mark_layer_renderable_base(self, will_render, draws);
    }

    /// Drops what the last `prepare` and `update` built: the draws, the
    /// tiles' entries, and the placement data that keeps the tiles' buckets
    /// alive.
    /// Called when the layer stops rendering, since neither runs again until
    /// it renders, and they then rebuild everything from the source's tiles.
    fn release_render_data(&mut self, draws: &mut LayerDraws) {
        release_render_data_base(self, draws);
    }

    /// Removes the layer's draws, its own and those of `draws`.
    fn remove_all_draws(&mut self, draws: &mut LayerDraws) {
        draws.clear_draws();
    }

    /// Moves what the layer keeps for each tile to the tile `wrap_delta`
    /// world copies east of it: the tiles are the same ones under new ids
    /// once the map's center has crossed the antimeridian, and their
    /// draws stay rather than being built again.
    fn handle_wrap_jump(&mut self, wrap_delta: i16, draws: &mut LayerDraws) {
        draws.shift_wraps(wrap_delta);
    }

    // Non-virtual members of the C++ class.

    /// Picks the shader variant for the state's projection; returns true and
    /// drops the draws when it changed.
    fn update_projection_variant(&mut self, state: &TransformState, draws: &mut LayerDraws) -> bool {
        let variant = if state.is_globe_rendering() { ProjectionVariant::Globe } else { ProjectionVariant::Mercator };
        if variant == self.base().projection_variant {
            return false;
        }
        self.base_mut().projection_variant = variant;
        self.remove_all_draws(draws);
        true
    }

    /// Adds the passes the layer's buckets of `tiles` were laid out to draw
    /// in to the layer's.
    fn add_render_passes_from_tiles(&mut self, tiles: LayerTiles<'_>) {
        let base = self.base_mut();
        for tile in tiles.iter() {
            if let Some(render_data) = tile.get_layer_render_data(&*base.style) {
                base.passes |= RenderPass(render_data.layer.render_passes());
            }
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
pub(crate) fn mark_layer_renderable_base<L: RenderLayer + ?Sized>(
    layer: &mut L,
    will_render: bool,
    draws: &mut LayerDraws,
) {
    layer.base_mut().is_renderable = will_render;

    if !will_render {
        layer.release_render_data(draws);
    }
}

/// The default `RenderLayer::release_render_data`, callable from overrides.
pub(crate) fn release_render_data_base<L: RenderLayer + ?Sized>(layer: &mut L, draws: &mut LayerDraws) {
    layer.remove_all_draws(draws);
    draws.clear_tiles();
}

/// Makes the source's tiles at the indexes of `order` those
/// `parameters.draws` has entries for, none for `None`, and returns `order`.
pub(crate) fn set_layer_tiles(
    parameters: &mut LayerPrepareParameters<'_>,
    order: Option<Vec<usize>>,
) -> Option<Vec<usize>> {
    match parameters.tiles(order.as_deref()) {
        Some(tiles) => parameters.draws.set_tiles(tiles.iter().map(|tile| *tile.get_overscaled_tile_id())),
        None => parameters.draws.clear_tiles(),
    }
    order
}

/// The default `RenderLayer::prepare`, callable from overrides: the layer
/// draws its source's tiles in [`RenderSource::tile_order`], in the passes
/// their buckets were laid out for. Without a source (a source type the
/// port has no render source for), it gets no tiles.
pub(crate) fn prepare_base<L: RenderLayer + ?Sized>(
    layer: &mut L,
    parameters: &mut LayerPrepareParameters<'_>,
) -> Option<Vec<usize>> {
    let order = parameters.source.and_then(|source| {
        debug_assert!(source.is_enabled());
        source.tile_order()
    });
    if let Some(tiles) = parameters.tiles(order.as_deref()) {
        layer.add_render_passes_from_tiles(tiles);
    }
    set_layer_tiles(parameters, order)
}
