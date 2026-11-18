// A render layer is the renderer's side of a style layer: its base, its
// values of its type and its drawing (`LayerParts`), and what its type does
// with them (`RenderLayer`). The orchestrator owns the render layers, in the
// style's order.

use std::cell::RefCell;
use std::sync::Arc;

use mln_gfx::gpu::Gpu;
use mln_gfx::projection_variant::ProjectionVariant;
use mln_map_types::{ProjectionData, TileProjector, TransformState};
use mln_style::expression::Dependency;
use mln_style::layer_type::{ErasedLayerStyle, LayerStyle, PaintSet};
use mln_style::{PropertyEvaluationParameters, TransitionParameters};
use mln_style_types::{TranslateAnchorType, VisibilityType};
use mln_tile_id::{OverscaledTileID, UnwrappedTileID};
use mln_util::Color;
use mln_util::mat4::Mat4;
use smallvec::SmallVec;

use crate::bucket::Bucket;
use crate::buckets::symbol_bucket::SymbolBucket;
use crate::draw_cache::{ChangedBuckets, DrawGroup, DrawOrder, DrawSlots, LayerDrawing};
use crate::gfx::context::Context;
use crate::layer_type::{ErasedEvaluatedLayer, EvaluatedLayer, LayerType};
use crate::line_atlas::LineAtlas;
use crate::paint_parameters::PaintParameters;
use crate::pattern_atlas::PatternAtlas;
use crate::render_pass::RenderPass;
use crate::render_source::RenderSource;
use crate::render_tile::{FrameTile, RenderTile};
use crate::sources::render_image_source::ImageSourceRenderData;
use crate::tile::contents::Contents;
use crate::tile::job::SourceKey;
use crate::update_parameters::UpdateParameters;

/// A layer's bucket in a layout, which the layers of its layout group
/// share, and the layer's values it was laid out with.
#[derive(Clone, Copy)]
pub(crate) struct LayerRenderData<'a> {
    pub(crate) bucket: &'a dyn Bucket,
    /// The index of the bucket among the layout's buckets.
    pub(crate) bucket_index: usize,
    /// The values of the layer that the bucket was laid out with.
    pub(crate) layer: &'a Arc<dyn ErasedEvaluatedLayer>,
}

impl<'a> LayerRenderData<'a> {
    /// The bucket, as a bucket of a `T` layer, or `None` for another type's.
    pub(crate) fn bucket<T: LayerType>(self) -> Option<&'a T::Bucket> {
        self.bucket.downcast_ref()
    }

    /// The values the bucket was laid out with, as a `T` layer's, or `None`
    /// for another type's.
    pub(crate) fn layer<T: LayerType>(self) -> Option<&'a EvaluatedLayer<T>> {
        let any: &dyn std::any::Any = &**self.layer;
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

/// symbols: where a symbol bucket is: its tile, in the tile store, with
/// where the tile is among its source's tiles as the frame's render tiles
/// found it, and its index among the tile's buckets.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct BucketKey {
    pub(crate) bucket: usize,
    pub(crate) slot: usize,
    pub(crate) tile: OverscaledTileID,
    pub(crate) source: SourceKey,
}

/// symbols: the symbol buckets of the frame's tiles, by key.
pub(crate) trait SymbolBuckets {
    fn get(&self, key: &BucketKey) -> Option<&SymbolBucket>;
    fn get_mut(&mut self, key: &BucketKey) -> Option<&mut SymbolBucket>;
}

/// symbols: a symbol bucket a layer places or draws, with its tile.
#[derive(Clone)]
pub(crate) struct BucketPlacementData {
    pub(crate) bucket: BucketKey,
    pub(crate) tile: PlacedTile,
    pub(crate) feature_index: Option<Arc<crate::geometry::feature_index::FeatureIndex>>,
    pub(crate) source_id: String,
    pub(crate) sort_key_range: Option<crate::buckets::symbol_bucket::SortKeyRange>,
    pub(crate) text_translate: SymbolTranslate,
    pub(crate) icon_translate: SymbolTranslate,
}

/// bench: the copies of symbol buckets that benchmarks' layers place, kept
/// where a tile store would keep the buckets, by their index, which is
/// their keys' `bucket`. They stay for the thread's life.
#[cfg(feature = "bench")]
pub(crate) struct HeldBuckets(Vec<SymbolBucket>);

#[cfg(feature = "bench")]
thread_local! {
    static HELD_BUCKETS: std::cell::Cell<Vec<SymbolBucket>> = const { std::cell::Cell::new(Vec::new()) };
}

#[cfg(feature = "bench")]
impl HeldBuckets {
    /// Keeps `bucket`, and returns its index.
    pub(crate) fn hold(bucket: SymbolBucket) -> usize {
        let mut buckets = Self::take();
        buckets.0.push(bucket);
        let index = buckets.0.len() - 1;
        buckets.restore();
        index
    }

    /// The buckets, lent until `restore`.
    pub(crate) fn take() -> Self {
        HeldBuckets(HELD_BUCKETS.take())
    }

    pub(crate) fn restore(self) {
        HELD_BUCKETS.set(self.0);
    }
}

#[cfg(feature = "bench")]
impl SymbolBuckets for HeldBuckets {
    fn get(&self, key: &BucketKey) -> Option<&SymbolBucket> {
        self.0.get(key.bucket)
    }

    fn get_mut(&mut self, key: &BucketKey) -> Option<&mut SymbolBucket> {
        self.0.get_mut(key.bucket)
    }
}

/// The tiles a layer draws in a frame: a selection of its source's render
/// tiles, in the order the layer draws them, with what their tiles show;
/// or an image source's image.
#[derive(Clone, Copy)]
pub(crate) struct LayerTiles<'a> {
    tiles: &'a [RenderTile],
    /// What each of `tiles` shows, at its index.
    contents: &'a [Option<&'a Contents>],
    order: &'a [usize],
    image: Option<ImageSourceRenderData<'a>>,
}

impl<'a> LayerTiles<'a> {
    /// The tiles of `tiles` at the indexes of `order`, in that order, which
    /// show `contents`, at their indexes.
    #[must_use]
    pub(crate) fn new(
        tiles: &'a [RenderTile],
        contents: &'a [Option<&'a Contents>],
        order: &'a [usize],
    ) -> LayerTiles<'a> {
        LayerTiles { tiles, contents, order, image: None }
    }

    /// An image source's image, without tiles.
    #[must_use]
    pub(crate) fn of_image(image: ImageSourceRenderData<'a>) -> LayerTiles<'a> {
        LayerTiles { tiles: &[], contents: &[], order: &[], image: Some(image) }
    }

    /// The tile at `index` of the source's tiles.
    fn tile(self, index: usize) -> Option<FrameTile<'a>> {
        let tile = self.tiles.get(index)?;
        Some(FrameTile::new(tile, self.contents.get(index).copied().flatten()))
    }

    pub(crate) fn iter(self) -> impl Iterator<Item = FrameTile<'a>> + 'a {
        self.order.iter().filter_map(move |&i| self.tile(i))
    }

    /// The render tiles in order, without what they show.
    pub(crate) fn render_tiles(self) -> impl Iterator<Item = &'a RenderTile> + 'a {
        self.order.iter().filter_map(move |&i| self.tiles.get(i))
    }

    /// An image source's image.
    #[must_use]
    pub(crate) fn image(self) -> Option<ImageSourceRenderData<'a>> {
        self.image
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
    /// What each of the source's render tiles shows, at its index.
    pub(crate) contents: &'a [Option<&'a Contents>],
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
        Some(LayerTiles::new(source.render_tiles(), self.contents, order?))
    }
}

/// What every render layer has, whatever its type.
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

/// The parts of a render layer of a [`TypedLayer`] `L`.
pub(crate) struct Parts<'a, L: TypedLayer + ?Sized> {
    pub(crate) base: &'a RenderLayerBase,
    pub(crate) values: &'a LayerValues<L::Type>,
    pub(crate) drawing: &'a LayerDrawing<L::Tile, L::Shared>,
}

/// The parts of a render layer of a [`TypedLayer`] `L`, to change.
pub(crate) struct PartsMut<'a, L: TypedLayer + ?Sized> {
    pub(crate) base: &'a mut RenderLayerBase,
    pub(crate) values: &'a mut LayerValues<L::Type>,
    pub(crate) drawing: &'a mut LayerDrawing<L::Tile, L::Shared>,
}

/// A render layer whose values are of the layer type `Type`, and that keeps
/// the GPU objects of its draws in caches of `Tile` for each tile and of
/// `Shared` for the draws no tile holds. Its parts give it [`LayerParts`].
pub(crate) trait TypedLayer {
    type Type: LayerType;
    type Tile: DrawSlots;
    type Shared: DrawSlots + Default;

    fn parts(&self) -> Parts<'_, Self>;
    fn parts_mut(&mut self) -> PartsMut<'_, Self>;
}

/// What a render layer does with its parts, whatever its type: its base,
/// its values, which this transitions without knowing their type, and its
/// drawing, whose caches this keeps for its tiles and whose draws this
/// issues.
pub(crate) trait LayerParts {
    fn base(&self) -> &RenderLayerBase;
    fn base_mut(&mut self) -> &mut RenderLayerBase;

    /// Makes `style`, the changed style of the layer, the layer's, if it is
    /// a layer of its type.
    fn set_style(&mut self, style: &Arc<dyn ErasedLayerStyle>);

    /// Begins transitions for the paint values that changed since the last
    /// frame, to those of the layer's style.
    fn transition(&mut self, parameters: &TransitionParameters);

    /// Whether a paint value of the layer is still in transition.
    fn has_transition(&self) -> bool;

    /// Whether the layer has a pattern property and is in a cross-fade.
    fn has_crossfade(&self) -> bool;

    /// Makes `tiles`, the frame's tiles of the layer, those the layer keeps
    /// entries of, with what it keeps of the tiles it had; none for `None`.
    fn set_tiles(&mut self, tiles: Option<LayerTiles<'_>>);

    /// Records that the buckets in `changed` changed, for the tiles whose
    /// draws were built from them.
    fn mark_changed(&mut self, changed: &ChangedBuckets);

    /// Moves what the layer keeps for each tile to the tile `wrap_delta`
    /// world copies east of it: the tiles are the same ones under new ids
    /// once the map's center has crossed the antimeridian, and their draws
    /// stay rather than being built again.
    fn handle_wrap_jump(&mut self, wrap_delta: i16);

    /// Drops the frame's draws of the layer and the caches of their GPU
    /// objects. The tiles' entries stay.
    fn remove_all_draws(&mut self);

    /// The layer's draw groups of the frame, in drawing order: none unless
    /// the frame draws the layer.
    fn draw_groups(&self) -> DrawGroups<'_>;

    /// Uploads the GPU objects of the layer's draw groups, before the frame
    /// draws them.
    fn upload(&mut self, context: &Context);

    /// Draws the pass's draws of the layer's draw groups into the open pass
    /// of `gpu`, the context's, in `order`, each at its depth.
    fn render(&mut self, gpu: &mut dyn Gpu, parameters: &mut PaintParameters<'_>, order: DrawOrder);
}

impl<L: TypedLayer> LayerParts for L {
    fn base(&self) -> &RenderLayerBase {
        self.parts().base
    }

    fn base_mut(&mut self) -> &mut RenderLayerBase {
        self.parts_mut().base
    }

    fn set_style(&mut self, style: &Arc<dyn ErasedLayerStyle>) {
        let PartsMut { base, values, .. } = self.parts_mut();
        if let Some(recovered) = LayerStyle::recover(style) {
            values.style = recovered;
            base.style = Arc::clone(style);
        }
    }

    fn transition(&mut self, parameters: &TransitionParameters) {
        let PartsMut { base, values, .. } = self.parts_mut();
        let prior = std::mem::take(&mut values.unevaluated);
        values.unevaluated = values.style.paint.transitioned(parameters, prior);
        base.style_dependencies =
            <L::Type as mln_style::layer_type::StyleLayerType>::PaintProperties::dependencies(&values.unevaluated);
    }

    fn has_transition(&self) -> bool {
        <L::Type as mln_style::layer_type::StyleLayerType>::PaintProperties::has_transition(
            &self.parts().values.unevaluated,
        )
    }

    fn has_crossfade(&self) -> bool {
        L::Type::CROSSFADES && self.parts().values.evaluated.crossfade.t != 1.0
    }

    fn set_tiles(&mut self, tiles: Option<LayerTiles<'_>>) {
        let PartsMut { drawing, .. } = self.parts_mut();
        let caches = &mut drawing.caches;
        match tiles {
            Some(tiles) => caches.set_tiles(tiles.render_tiles().map(|tile| *tile.get_overscaled_tile_id())),
            None => caches.clear_tiles(),
        }
    }

    fn mark_changed(&mut self, changed: &ChangedBuckets) {
        self.parts_mut().drawing.caches.mark_changed(changed);
    }

    fn handle_wrap_jump(&mut self, wrap_delta: i16) {
        self.parts_mut().drawing.caches.shift_wraps(wrap_delta);
    }

    fn remove_all_draws(&mut self) {
        self.parts_mut().drawing.clear();
    }

    fn draw_groups(&self) -> DrawGroups<'_> {
        let Parts { base, drawing, .. } = self.parts();
        if !base.is_renderable {
            return DrawGroups::new();
        }
        drawing.groups().map(DrawGroup::Draws).collect()
    }

    fn upload(&mut self, context: &Context) {
        let PartsMut { base, drawing, .. } = self.parts_mut();
        if base.is_renderable {
            drawing.upload(context);
        }
    }

    fn render(&mut self, gpu: &mut dyn Gpu, parameters: &mut PaintParameters<'_>, order: DrawOrder) {
        let PartsMut { base, drawing, .. } = self.parts_mut();
        if base.is_renderable {
            drawing.render(gpu, parameters, order);
        }
    }
}

/// A layer's draw groups of a frame, in drawing order.
pub(crate) type DrawGroups<'a> = SmallVec<[DrawGroup<'a>; 2]>;

pub(crate) trait RenderLayer: LayerParts {
    /// Fully evaluate possibly-transitioning paint properties based on a
    /// zoom and some evaluated layout properties.
    fn evaluate(&mut self, parameters: &PropertyEvaluationParameters);

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
    fn prepare(&mut self, parameters: &LayerPrepareParameters<'_>) -> Option<Vec<usize>> {
        prepare_base(self, parameters)
    }

    /// TODO: Only for background layers.
    fn get_solid_background(&self) -> Option<Color> {
        None
    }

    /// Makes the layer's draws of the frame, from the frame's `tiles` of the
    /// layer, in the order its `prepare` returned, and what it keeps of
    /// them, with the values of their uniforms.
    fn update(
        &mut self,
        _parameters: &PaintParameters<'_>,
        _update_parameters: &UpdateParameters,
        _tiles: Option<LayerTiles<'_>>,
    ) {
    }

    /// Uploads what draws into the layer's render targets, which draw before
    /// the frame's passes.
    fn upload_render_targets(&mut self, _context: &Context) {}

    /// Draws the layer's render targets, ahead of the frame's passes, which
    /// sample them.
    fn render_targets(&mut self, _parameters: &mut PaintParameters<'_>) {}

    /// Runs a custom layer's prerender, after the layers' draws upload and
    /// before the frame's first pass, which draws into `target`.
    fn prerender(&mut self, _parameters: &PaintParameters<'_>) {}

    /// Tells a custom layer's host that the renderer's graphics objects are
    /// lost, and lets go of it.
    fn lose_context(&mut self) {}

    /// Called when the style layer is replaced (same ID and type), and the
    /// render layer is reused: drops the draws, which were built for the
    /// previous style's values. The next update builds them again from the
    /// tiles' buckets.
    fn layer_changed(&mut self, _parameters: &TransitionParameters, _new_layer: &Arc<dyn ErasedLayerStyle>) {
        self.remove_all_draws();
    }

    /// Called when the layer is included or excluded from rendering.
    fn mark_layer_renderable(&mut self, will_render: bool) {
        mark_layer_renderable_base(self, will_render);
    }

    /// Drops what the last `prepare` and `update` built: the draws, the
    /// tiles' entries, and the placement data that keeps the tiles' buckets
    /// alive.
    /// Called when the layer stops rendering, since neither runs again until
    /// it renders, and they then rebuild everything from the source's tiles.
    fn release_render_data(&mut self) {
        release_render_data_base(self);
    }

    /// Picks the shader variant for the state's projection; returns true and
    /// drops the draws when it changed.
    fn update_projection_variant(&mut self, state: &TransformState) -> bool {
        let variant = if state.is_globe_rendering() { ProjectionVariant::Globe } else { ProjectionVariant::Mercator };
        if variant == self.base().projection_variant {
            return false;
        }
        self.base_mut().projection_variant = variant;
        self.remove_all_draws();
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
    // tile's projector in place of its matrix; false unless a layer type
    // overrides it.
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

/// The default `RenderLayer::release_render_data`, callable from overrides.
pub(crate) fn release_render_data_base<L: RenderLayer + ?Sized>(layer: &mut L) {
    layer.remove_all_draws();
    layer.set_tiles(None);
}

/// Makes the source's tiles at the indexes of `order` those the layer keeps
/// entries of, none for `None`, and returns `order`.
pub(crate) fn set_layer_tiles<L: RenderLayer + ?Sized>(
    layer: &mut L,
    parameters: &LayerPrepareParameters<'_>,
    order: Option<Vec<usize>>,
) -> Option<Vec<usize>> {
    layer.set_tiles(parameters.tiles(order.as_deref()));
    order
}

/// The default `RenderLayer::prepare`, callable from overrides: the layer
/// draws its source's tiles in [`RenderSource::tile_order`], in the passes
/// their buckets were laid out for. Without a source (a source type the
/// port has no render source for), it gets no tiles.
pub(crate) fn prepare_base<L: RenderLayer + ?Sized>(
    layer: &mut L,
    parameters: &LayerPrepareParameters<'_>,
) -> Option<Vec<usize>> {
    let order = parameters.source.and_then(|source| {
        debug_assert!(source.is_enabled());
        source.tile_order()
    });
    if let Some(tiles) = parameters.tiles(order.as_deref()) {
        layer.add_render_passes_from_tiles(tiles);
    }
    set_layer_tiles(layer, parameters, order)
}
