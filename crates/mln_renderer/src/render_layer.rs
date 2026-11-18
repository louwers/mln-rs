// A render layer is the renderer's side of a style layer: its base, its
// values of its type and its drawing. Its type implements `TypedLayer`,
// whose hooks see the frame's tiles and buckets of the type, and
// `RenderLayer`, the hooks the engine calls on any layer; `LayerParts` is
// what the engine does around the hooks, the same for every type. The
// orchestrator owns the render layers, in the style's order.

use std::cell::RefCell;
use std::marker::PhantomData;
use std::sync::Arc;

use mln_gfx::gpu::Gpu;
use mln_gfx::projection_variant::ProjectionVariant;
use mln_map_types::{ProjectionData, TileProjector, TransformState};
use mln_style::expression::Dependency;
use mln_style::layer_type::{ErasedLayerStyle, LayerStyle, PaintSet, SourceKind, StyleLayerType};
use mln_style::{PropertyEvaluationParameters, TransitionParameters};
use mln_style_types::{TranslateAnchorType, VisibilityType};
use mln_tile_id::{OverscaledTileID, UnwrappedTileID};
use mln_util::Color;
use mln_util::mat4::Mat4;

use crate::bucket::Bucket;
use crate::buckets::symbol_bucket::SymbolBucket;
use crate::gfx::context::Context;
use crate::layer_drawing::{ChangedBuckets, DrawGroup, DrawOrder, LayerDrawing};
use crate::layer_type::{ErasedEvaluatedLayer, EvaluatedLayer, LayerType};
use crate::line_atlas::LineAtlas;
use crate::paint_parameters::PaintParameters;
use crate::pattern_atlas::PatternAtlas;
use crate::render_pass::RenderPass;
use crate::render_source::{RenderSource, TileOrder};
use crate::render_tile::{FrameTile, RenderTile};
use crate::sources::render_image_source::ImageSourceRenderData;
use crate::tile::contents::Contents;
use crate::tile::job::SourceKey;
use crate::update_parameters::UpdateParameters;

/// A layer's bucket in a layout, which the layers of its layout group
/// share, with the layer's values it was laid out with, whichever the
/// layer's type.
#[derive(Clone, Copy)]
pub(crate) struct ErasedLayerBucket<'a> {
    pub(crate) bucket: &'a dyn Bucket,
    /// The index of the bucket among the layout's buckets.
    pub(crate) bucket_index: usize,
    /// The values of the layer that the bucket was laid out with.
    pub(crate) layer: &'a dyn ErasedEvaluatedLayer,
}

/// A bucket of a layer of the type `T` in a layout, with the layer's values
/// it was laid out with, as the layer type's glue recovers it
/// ([`ErasedLayerBucket::recover`]).
pub(crate) struct LayerBucket<'a, T: LayerType> {
    pub(crate) bucket: &'a T::Bucket,
    /// The index of the bucket among the layout's buckets.
    pub(crate) bucket_index: usize,
    /// The values of the layer that the bucket was laid out with.
    pub(crate) layer: &'a EvaluatedLayer<T>,
}

impl<T: LayerType> Clone for LayerBucket<'_, T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<T: LayerType> Copy for LayerBucket<'_, T> {}

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
/// found it ([`RenderTile::slot`]), and its index among the tile's buckets.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct BucketKey {
    pub(crate) bucket: usize,
    pub(crate) slot: Option<usize>,
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
/// their keys' `bucket`. The benchmarks' entry points pass no store, so the
/// copies are the thread's, and the layer that places one frees it
/// ([`HeldBuckets::release`]) when it drops.
#[cfg(feature = "bench")]
pub(crate) struct HeldBuckets(Vec<Option<SymbolBucket>>);

#[cfg(feature = "bench")]
thread_local! {
    static HELD_BUCKETS: std::cell::Cell<Vec<Option<SymbolBucket>>> = const { std::cell::Cell::new(Vec::new()) };
}

/// bench: a copy of the bucket `bucket` points to, if it is a symbol
/// bucket, which `HeldBuckets` holds for a benchmark's layer to place, at
/// the index this returns, until the layer drops.
#[cfg(feature = "bench")]
pub(crate) fn hold_symbol_bucket(bucket: &impl std::ops::Deref<Target = dyn Bucket>) -> Option<usize> {
    let bucket = crate::layer_type::recover_bucket::<mln_style::layers::Symbol>(&**bucket)?;
    Some(HeldBuckets::hold(bucket.clone()))
}

#[cfg(feature = "bench")]
impl HeldBuckets {
    /// Keeps `bucket`, and returns its index.
    pub(crate) fn hold(bucket: SymbolBucket) -> usize {
        let mut buckets = Self::take();
        buckets.0.push(Some(bucket));
        let index = buckets.0.len() - 1;
        buckets.restore();
        index
    }

    /// Frees the bucket at `index`. The indexes start again once no bucket
    /// is held.
    pub(crate) fn release(index: usize) {
        let mut buckets = Self::take();
        if let Some(bucket) = buckets.0.get_mut(index) {
            *bucket = None;
        }
        if buckets.0.iter().all(Option::is_none) {
            buckets.0.clear();
        }
        buckets.restore();
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
        self.0.get(key.bucket)?.as_ref()
    }

    fn get_mut(&mut self, key: &BucketKey) -> Option<&mut SymbolBucket> {
        self.0.get_mut(key.bucket)?.as_mut()
    }
}

/// The layer type of the frame's tiles and sources as the engine sees them,
/// without the layers' types. The glue of a layer type hands its layer the
/// views of its own type (`LayerTiles<'_, T>`), whose buckets are the
/// type's.
pub(crate) enum Erased {}

/// What a layer draws in a frame from its source, as a layer of the type `T`
/// sees it.
pub(crate) enum LayerSource<'a, T = Erased> {
    /// A selection of the source's render tiles.
    Tiles(LayerTiles<'a, T>),
    /// An image source's image.
    Image(ImageSourceRenderData<'a>),
}

impl<T> Clone for LayerSource<'_, T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<T> Copy for LayerSource<'_, T> {}

impl<'a, T> LayerSource<'a, T> {
    /// The tiles, of a source with tiles.
    #[must_use]
    pub(crate) fn tiles(self) -> Option<LayerTiles<'a, T>> {
        match self {
            LayerSource::Tiles(tiles) => Some(tiles),
            LayerSource::Image(_) => None,
        }
    }

    /// The image, of an image source.
    #[must_use]
    pub(crate) fn image(self) -> Option<ImageSourceRenderData<'a>> {
        match self {
            LayerSource::Tiles(_) => None,
            LayerSource::Image(image) => Some(image),
        }
    }
}

impl<'a> LayerSource<'a> {
    /// The source as a layer of the type `T` sees it.
    fn typed<T: LayerType>(self) -> LayerSource<'a, T> {
        match self {
            LayerSource::Tiles(tiles) => LayerSource::Tiles(tiles.typed()),
            LayerSource::Image(image) => LayerSource::Image(image),
        }
    }
}

/// The tiles a layer draws in a frame: a selection of its source's render
/// tiles, in the order the layer draws them, with what their tiles show, as
/// a layer of the type `T` sees them.
pub(crate) struct LayerTiles<'a, T = Erased> {
    tiles: &'a [RenderTile],
    /// What each of `tiles` shows, at its index.
    contents: &'a [Option<&'a Contents>],
    order: &'a [usize],
    layer_type: PhantomData<fn() -> T>,
}

impl<T> Clone for LayerTiles<'_, T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<T> Copy for LayerTiles<'_, T> {}

impl<'a> LayerTiles<'a> {
    /// The tiles of `tiles` at the indexes of `order`, in that order, which
    /// show `contents`, at their indexes.
    #[must_use]
    pub(crate) fn new(
        tiles: &'a [RenderTile],
        contents: &'a [Option<&'a Contents>],
        order: &'a [usize],
    ) -> LayerTiles<'a> {
        LayerTiles { tiles, contents, order, layer_type: PhantomData }
    }

    /// The tiles as a layer of the type `T` sees them.
    fn typed<T: LayerType>(self) -> LayerTiles<'a, T> {
        let LayerTiles { tiles, contents, order, layer_type: _ } = self;
        LayerTiles { tiles, contents, order, layer_type: PhantomData }
    }
}

impl<'a, T: 'a> LayerTiles<'a, T> {
    /// The tile at `index` of the source's tiles.
    fn tile(self, index: usize) -> Option<FrameTile<'a, T>> {
        let tile = self.tiles.get(index)?;
        Some(FrameTile::new(tile, self.contents.get(index).copied().flatten()))
    }

    pub(crate) fn iter(self) -> impl Iterator<Item = FrameTile<'a, T>> + 'a {
        self.order.iter().filter_map(move |&i| self.tile(i))
    }

    /// The tiles in order, each with its position in the order, which
    /// [`get`](Self::get) takes.
    pub(crate) fn iter_indexed(self) -> impl Iterator<Item = (usize, FrameTile<'a, T>)> + 'a {
        self.order.iter().enumerate().filter_map(move |(position, &i)| Some((position, self.tile(i)?)))
    }

    /// The tile at `position` in the order.
    pub(crate) fn get(self, position: usize) -> Option<FrameTile<'a, T>> {
        self.tile(*self.order.get(position)?)
    }

    /// The render tiles in order, without what they show.
    pub(crate) fn render_tiles(self) -> impl Iterator<Item = &'a RenderTile> + 'a {
        self.order.iter().filter_map(move |&i| self.tiles.get(i))
    }

    /// The ids of the tiles, in order.
    pub(crate) fn tile_ids(self) -> impl Iterator<Item = OverscaledTileID> + 'a {
        self.render_tiles().map(|tile| *tile.get_overscaled_tile_id())
    }

    #[must_use]
    pub(crate) fn is_empty(self) -> bool {
        self.order.is_empty()
    }
}

/// What a layer reads in the frame's update, besides its tiles.
pub(crate) struct LayerUpdateParameters<'a> {
    /// The layer's source, `None` for a layer without one or a source type
    /// the renderer has no render source for.
    pub(crate) source: Option<&'a dyn RenderSource>,
    /// What each of the source's render tiles shows, at its index.
    pub(crate) contents: &'a [Option<&'a Contents>],
    /// The images of the style.
    pub(crate) images: &'a crate::image_store::ImageStore,
    pub(crate) pattern_atlas: &'a RefCell<PatternAtlas>,
    pub(crate) line_atlas: &'a RefCell<LineAtlas>,
}

impl<'a> LayerUpdateParameters<'a> {
    /// The source's tiles in `order`; `None` without either.
    #[must_use]
    fn tiles(&self, order: Option<TileOrder>) -> Option<LayerTiles<'a>> {
        let source = self.source?;
        Some(LayerTiles::new(source.render_tiles(), self.contents, source.tile_order(order?)?))
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
    pub(crate) drawing: &'a L::Drawing,
}

/// The parts of a render layer of a [`TypedLayer`] `L`, to change.
pub(crate) struct PartsMut<'a, L: TypedLayer + ?Sized> {
    pub(crate) base: &'a mut RenderLayerBase,
    pub(crate) values: &'a mut LayerValues<L::Type>,
    pub(crate) drawing: &'a mut L::Drawing,
}

/// A render layer whose values are of the layer type `Type`, and that keeps
/// its drawing, the GPU objects of its draws and its draw groups of the
/// frame, in a `Drawing` of its own type. Its parts give it [`LayerParts`],
/// which runs its hooks here with the tiles and buckets of its type.
pub(crate) trait TypedLayer {
    type Type: LayerType;
    type Drawing: LayerDrawing;

    /// The order the layer draws its source's tiles in.
    const TILE_ORDER: TileOrder = TileOrder::ById;

    fn parts(&self) -> Parts<'_, Self>;
    fn parts_mut(&mut self) -> PartsMut<'_, Self>;

    /// Called in the frame's update once the layer has `tiles`, the frame's
    /// tiles of its source in the order it draws them: `None` without a
    /// source, or for an image source. The layer collects here what the
    /// update needs of its tiles' buckets, such as their dash patterns or
    /// their symbols.
    fn update(&mut self, _parameters: &LayerUpdateParameters<'_>, _tiles: Option<LayerTiles<'_, Self::Type>>) {}

    /// Makes the layer's draws of the frame, in the frame's encode, from
    /// what it draws of its `source`: the frame's tiles of the layer, in the
    /// order the update gave them, or an image source's image; and what it
    /// keeps of them, with the values of their uniforms.
    fn update_draws(
        &mut self,
        _parameters: &PaintParameters<'_>,
        _update_parameters: &UpdateParameters,
        _source: Option<LayerSource<'_, Self::Type>>,
    ) {
    }

    /// Drops what the layer keeps for its draws besides their GPU objects
    /// and its tiles' entries, when it stops rendering.
    fn release(&mut self) {}

    /// Called when the layer's style changed to another of the same id and
    /// type, once it is the layer's and before its values transition.
    fn style_changed(&mut self) {}
}

/// What the engine does with a render layer's parts, whatever its type: its
/// base, its values, which this transitions without knowing their type, and
/// its drawing, which keeps what the layer keeps of its tiles and uploads
/// and draws its draw groups. It runs the layer's [`TypedLayer`] hooks with
/// what is common to every layer type around them.
pub(crate) trait LayerParts {
    fn base(&self) -> &RenderLayerBase;
    fn base_mut(&mut self) -> &mut RenderLayerBase;

    /// The layer's id.
    fn id(&self) -> &str {
        &self.base().style.base().id
    }

    /// The order the layer draws its source's tiles in.
    fn tile_order(&self) -> TileOrder;

    /// Makes `style`, the changed style of the layer (of the same id and
    /// type), the layer's, drops the draws, which were built for the
    /// previous style's values, and begins transitions to the new values.
    /// A style of another type leaves the layer's. The next encode builds
    /// the draws again from the tiles' buckets.
    fn restyle(&mut self, parameters: &TransitionParameters, style: &Arc<dyn ErasedLayerStyle>);

    /// Begins transitions for the paint values that changed since the last
    /// frame, to those of the layer's style.
    fn transition(&mut self, parameters: &TransitionParameters);

    /// Whether a paint value of the layer is still in transition.
    fn has_transition(&self) -> bool;

    /// Whether the layer has a pattern property and is in a cross-fade.
    fn has_crossfade(&self) -> bool;

    /// The frame's update of the layer: picks the tiles of `parameters`'
    /// source the layer draws, adds the passes their buckets were laid out
    /// for to the layer's, runs the layer's [`TypedLayer::update`], and
    /// keeps entries for the tiles. Returns the order the layer draws the
    /// tiles in, `None` for a layer without a source.
    fn update(&mut self, parameters: &LayerUpdateParameters<'_>) -> Option<TileOrder>;

    /// Runs the layer's [`TypedLayer::update_draws`] with `source`.
    fn update_draws(
        &mut self,
        parameters: &PaintParameters<'_>,
        update_parameters: &UpdateParameters,
        source: Option<LayerSource<'_>>,
    );

    /// Records that the buckets in `changed` changed, for the tiles whose
    /// draws were built from them.
    fn mark_changed(&mut self, changed: &ChangedBuckets);

    /// Moves what the layer keeps for each tile to the tile `wrap_delta`
    /// world copies east of it: the tiles are the same ones under new ids
    /// once the map's center has crossed the antimeridian, and their draws
    /// stay rather than being built again.
    fn handle_wrap_jump(&mut self, wrap_delta: i16);

    /// Includes the layer in the frame's drawing or excludes it. A layer
    /// excluded drops its draws, its tiles' entries and what its type keeps
    /// for them ([`TypedLayer::release`]): its updates don't run until it is
    /// included again, and they then rebuild everything from the source's
    /// tiles.
    fn set_renderable(&mut self, renderable: bool);

    /// Drops the frame's draws of the layer and the caches of their GPU
    /// objects. The tiles' entries stay.
    fn remove_all_draws(&mut self);

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

    /// Visits the layer's draw groups of the frame, in drawing order: none
    /// unless the frame draws the layer. They are the groups `upload` and
    /// `render` upload and draw.
    fn visit_draw_groups<'a>(&'a self, visit: &mut dyn FnMut(DrawGroup<'a>));

    /// Uploads the GPU objects of the layer's draw groups, before the frame
    /// draws them.
    fn upload(&mut self, context: &Context);

    /// Draws the pass's draws of the layer's draw groups into the open pass
    /// of `gpu`, the context's, in `order`, each at its depth.
    fn render(&mut self, gpu: &mut dyn Gpu, parameters: &mut PaintParameters<'_>, order: DrawOrder);

    /// Whether the layer draws in a pass and is visible.
    fn needs_rendering(&self) -> bool {
        self.base().passes != RenderPass::None && self.base().style.base().visibility != VisibilityType::None
    }

    /// Whether the layer draws at `zoom`. A minimum zoom of 0 is no
    /// minimum: the globe zooms below 0 toward the poles.
    fn supports_zoom(&self, zoom: f32) -> bool {
        let zoom_or_zero = if zoom < 0.0 { 0.0 } else { zoom };
        self.base().style.base().min_zoom <= zoom_or_zero && zoom < self.base().style.base().max_zoom
    }

    /// Whether the frame draws the layer.
    fn is_renderable(&self) -> bool {
        self.base().is_renderable
    }

    /// What the layer's paint values depend on.
    fn style_dependencies(&self) -> Dependency {
        self.base().style_dependencies
    }
}

impl<L: TypedLayer> LayerParts for L {
    fn base(&self) -> &RenderLayerBase {
        self.parts().base
    }

    fn base_mut(&mut self) -> &mut RenderLayerBase {
        self.parts_mut().base
    }

    fn tile_order(&self) -> TileOrder {
        L::TILE_ORDER
    }

    fn restyle(&mut self, parameters: &TransitionParameters, style: &Arc<dyn ErasedLayerStyle>) {
        self.remove_all_draws();
        let PartsMut { base, values, .. } = self.parts_mut();
        if let Some(recovered) = LayerStyle::recover(style) {
            values.style = recovered;
            base.style = Arc::clone(style);
        }
        self.style_changed();
        self.transition(parameters);
    }

    fn transition(&mut self, parameters: &TransitionParameters) {
        let PartsMut { base, values, .. } = self.parts_mut();
        let prior = std::mem::take(&mut values.unevaluated);
        values.unevaluated = values.style.paint.transitioned(parameters, prior);
        base.style_dependencies = <L::Type as StyleLayerType>::PaintProperties::dependencies(&values.unevaluated);
    }

    fn has_transition(&self) -> bool {
        <L::Type as StyleLayerType>::PaintProperties::has_transition(&self.parts().values.unevaluated)
    }

    fn has_crossfade(&self) -> bool {
        L::Type::CROSSFADES && self.parts().values.evaluated.crossfade.t != 1.0
    }

    fn update(&mut self, parameters: &LayerUpdateParameters<'_>) -> Option<TileOrder> {
        let order = parameters.source.map(|source| {
            debug_assert!(source.is_enabled());
            L::TILE_ORDER
        });
        let tiles = parameters.tiles(order).map(LayerTiles::typed::<L::Type>);
        if let Some(tiles) = tiles {
            let base = self.parts_mut().base;
            for tile in tiles.iter() {
                if let Some(bucket) = tile.bucket(&*base.style) {
                    base.passes |= RenderPass(bucket.layer.render_passes);
                }
            }
        }
        TypedLayer::update(self, parameters, tiles);
        // A layer without a source keeps the tiles it picks itself.
        if <L::Type as StyleLayerType>::INFO.source != SourceKind::None {
            set_tiles(self, tiles);
        }
        order
    }

    fn update_draws(
        &mut self,
        parameters: &PaintParameters<'_>,
        update_parameters: &UpdateParameters,
        source: Option<LayerSource<'_>>,
    ) {
        TypedLayer::update_draws(self, parameters, update_parameters, source.map(LayerSource::typed));
    }

    fn mark_changed(&mut self, changed: &ChangedBuckets) {
        if let Some(tiles) = self.parts_mut().drawing.tiles() {
            tiles.mark_changed(changed);
        }
    }

    fn handle_wrap_jump(&mut self, wrap_delta: i16) {
        if let Some(tiles) = self.parts_mut().drawing.tiles() {
            tiles.shift_wraps(wrap_delta);
        }
    }

    fn set_renderable(&mut self, renderable: bool) {
        self.base_mut().is_renderable = renderable;
        if !renderable {
            self.remove_all_draws();
            set_tiles::<L, Erased>(self, None);
            self.release();
        }
    }

    fn remove_all_draws(&mut self) {
        self.parts_mut().drawing.clear();
    }

    fn visit_draw_groups<'a>(&'a self, visit: &mut dyn FnMut(DrawGroup<'a>)) {
        let Parts { base, drawing, .. } = self.parts();
        if base.is_renderable {
            drawing.visit_groups(visit);
        }
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

/// Makes `tiles`, the frame's tiles of `layer`, those the layer keeps
/// entries of, with what it keeps of the tiles it had; none for `None`.
fn set_tiles<L: TypedLayer, T>(layer: &mut L, tiles: Option<LayerTiles<'_, T>>) {
    if let Some(tiles_kept) = layer.parts_mut().drawing.tiles() {
        match tiles {
            Some(tiles) => tiles_kept.set_tiles(tiles.tile_ids()),
            None => tiles_kept.clear_tiles(),
        }
    }
}

/// What a layer type does that the engine can't do for it, whatever the
/// type: the hooks the engine calls on the layers it holds.
pub(crate) trait RenderLayer: LayerParts {
    /// Fully evaluate possibly-transitioning paint properties based on a
    /// zoom and some evaluated layout properties.
    fn evaluate(&mut self, parameters: &PropertyEvaluationParameters);

    /// Whether the layer draws 3D geometry, whose depth it writes in the
    /// range of [`PaintParameters::depth_mode_for_3d`].
    fn is_3d(&self) -> bool {
        false
    }

    /// Returns true if the layer may draw depth-tested 3D geometry, which the
    /// planet's depth hides behind the horizon.
    fn needs_globe_depth(&self) -> bool {
        self.is_3d()
    }

    /// A background layer's color, where it is a solid color the frame can
    /// clear to instead of drawing the layer.
    fn solid_background(&self) -> Option<Color> {
        None
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

    /// Returns true if the layer has to be placed.
    fn needs_placement(&self) -> bool {
        !self.placement_data().is_empty()
    }

    /// symbols: the buckets the layer places, which only a symbol layer
    /// has.
    fn placement_data(&self) -> &[BucketPlacementData] {
        &[]
    }

    /// symbols: the buckets that another layer places and this layer draws,
    /// at its own translation.
    fn follower_data(&self) -> &[BucketPlacementData] {
        &[]
    }

    /// Whether the feature `feature` with `geometry`, in a tile whose
    /// `projector` projects it, intersects `query_geometry` as the layer
    /// draws it at `zoom`; false unless a layer type overrides it.
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
