// A render layer is the renderer's side of a style layer: its base, its
// values of its type and its drawing. Its type implements `TypedLayer`,
// whose hooks see the frame's tiles and buckets of the type, and
// `RenderLayer`, the hooks the engine calls on any layer; `LayerParts` is
// what the engine does around the hooks, the same for every type. The
// orchestrator owns the render layers, in the style's order.

use std::any::Any;
use std::ops::Range;
use std::sync::Arc;

use glam::DMat4;
use mln_gfx::gpu::Gpu;
use mln_gfx::projection_variant::ProjectionVariant;
use mln_style::color::Color;
use mln_style::expression::Dependency;
use mln_style::layer::{ErasedLayerStyle, LayerStyle, PaintSet, SourceKind, StyleLayerType};
use mln_style::types::{LayerBlendType, TranslateAnchorType, VisibilityType};
use mln_style::{PropertyEvaluationParameters, TransitionParameters};
use smallvec::SmallVec;

use crate::camera::{ProjectionData, View};
use crate::draw::context::Context;
use crate::draw::tile_render_data::TileAtlasTextures;
use crate::layer::bucket::Bucket;
use crate::layer::bucket_geometry::BucketGeometry;
use crate::layer::layer_drawing::{ChangedBuckets, DrapeDrawing, DrawGroup, DrawOrder, LayerDrawing};
use crate::layer::layer_type::{ErasedEvaluatedLayer, EvaluatedLayer, LayerType};
use crate::layers::custom::EndedHosts;
use crate::query::feature_index::{FeatureIndex, QueryRegion};
use crate::render::paint_parameters::PaintParameters;
use crate::render::pass::RenderPass;
use crate::render::terrain::SurfaceLight;
use crate::render::tile::{FoundTile, FrameTile, RenderTile, TileStamp, TileView, ViewPlace};
use crate::render::update_parameters::UpdateParameters;
use crate::source::render_image_source::ImageSourceRenderData;
use crate::source::render_source::{RenderSource, TileOrder};
use crate::symbol::bucket::SymbolBucket;
use crate::text::atlas::ImagePosition;
use crate::tile::{OverscaledTileID, UnwrappedTileID};
use crate::tile_store::contents::{Contents, GeometryContents};
use crate::worker::job::SourceKey;

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

/// A bucket of a layer of the type `T` in a tile's layout, with the layer's
/// values it was laid out with and the rest of what the layout's contents
/// hold for the tile's draws.
pub(crate) struct LayerBucket<'a, T: LayerType> {
    pub(crate) bucket: &'a T::Bucket,
    /// The index of the bucket among the layout's buckets.
    pub(crate) bucket_index: usize,
    /// The values of the layer that the bucket was laid out with.
    pub(crate) layer: &'a EvaluatedLayer<T>,
    contents: &'a GeometryContents,
}

impl<T: LayerType> Clone for LayerBucket<'_, T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<T: LayerType> Copy for LayerBucket<'_, T> {}

impl<'a, T: LayerType> LayerBucket<'a, T> {
    /// The bucket of the layer at `index` among those `contents` laid out,
    /// if the layout laid it out as a layer of the type `T`.
    fn laid(contents: &'a GeometryContents, index: usize) -> Option<Self> {
        let ErasedLayerBucket { bucket, bucket_index, layer } = contents.result.layer_render_data.laid(index)?;
        let layer: &dyn Any = layer;
        Some(LayerBucket {
            bucket: crate::layer::layer_type::recover_bucket::<T>(bucket)?,
            bucket_index,
            layer: layer.downcast_ref()?,
            contents,
        })
    }

    /// The geometry of the layout's buckets in the context's stores, which
    /// the draws bind.
    #[must_use]
    pub(crate) fn geometry(self) -> &'a BucketGeometry {
        &self.contents.bucket_geometry
    }

    /// The feature index of the layout.
    #[must_use]
    pub(crate) fn feature_index(self) -> &'a Arc<FeatureIndex> {
        &self.contents.result.feature_index
    }

    /// Where the pattern `pattern` is in the tile's icon atlas.
    #[must_use]
    pub(crate) fn pattern(self, pattern: &str) -> Option<ImagePosition> {
        self.contents.pattern_positions()?.get(pattern).cloned()
    }

    /// The textures of the tile's glyph and icon atlases.
    #[must_use]
    pub(crate) fn atlas_textures(self) -> &'a TileAtlasTextures {
        &self.contents.textures
    }
}

impl<'a, T: LayerType> TileView<'a> for LayerBucket<'a, T> {
    fn find(contents: &'a Contents, layer_id: &str) -> Option<(Self, ViewPlace)> {
        let contents = contents.geometry()?;
        let index = contents.result.layer_render_data.find(layer_id)?;
        Some((LayerBucket::laid(contents, index)?, ViewPlace::Laid(index)))
    }

    fn at(contents: &'a Contents, place: ViewPlace) -> Option<Self> {
        let ViewPlace::Laid(index) = place else {
            debug_assert!(false, "a tile's own contents' place for a laid-out layer");
            return None;
        };
        LayerBucket::laid(contents.geometry()?, index)
    }

    fn is_for(self, layer_id: &str) -> bool {
        self.layer.style.id == layer_id
    }

    fn passes(self) -> RenderPass {
        RenderPass(self.layer.render_passes)
    }
}

/// A symbol layer's `*-translate` in pixels, which placement moves the
/// layer's symbols' collision geometry by.
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

/// What placement reads of the render tile of a symbol bucket, as the
/// frame's update that collected the bucket's placement data found it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct PlacedTile {
    pub(crate) id: UnwrappedTileID,
    pub(crate) overscaled_id: OverscaledTileID,
    pub(crate) hold_for_fade: bool,
    /// The tile's projection; `matrix` is its main matrix.
    pub(crate) projection: ProjectionData,
    pub(crate) matrix: DMat4,
}

impl PlacedTile {
    #[must_use]
    pub(crate) fn new(tile: &RenderTile) -> PlacedTile {
        PlacedTile {
            id: tile.id,
            overscaled_id: *tile.overscaled_tile_id(),
            hold_for_fade: tile.hold_for_fade(),
            projection: tile.projection,
            matrix: tile.matrix,
        }
    }
}

/// Where a symbol bucket is: its tile, in the tile store, with
/// where the tile is among its source's tiles as the frame's render tiles
/// found it ([`RenderTile::slot`]), and its index among the tile's buckets.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct BucketKey {
    pub(crate) bucket: usize,
    pub(crate) slot: Option<usize>,
    pub(crate) tile: OverscaledTileID,
    pub(crate) source: SourceKey,
}

/// The symbol buckets of the frame's tiles, by key.
pub(crate) trait SymbolBuckets {
    fn get(&self, key: &BucketKey) -> Option<&SymbolBucket>;
    fn get_mut(&mut self, key: &BucketKey) -> Option<&mut SymbolBucket>;
}

/// A symbol bucket a layer places or draws, with its tile.
#[derive(Clone)]
pub(crate) struct BucketPlacementData {
    pub(crate) bucket: BucketKey,
    pub(crate) tile: PlacedTile,
    pub(crate) feature_index: Option<Arc<FeatureIndex>>,
    pub(crate) source_id: Arc<str>,
    pub(crate) sort_key_range: Option<crate::symbol::bucket::SortKeyRange>,
    pub(crate) text_translate: SymbolTranslate,
    pub(crate) icon_translate: SymbolTranslate,
}

impl PartialEq for BucketPlacementData {
    fn eq(&self, other: &Self) -> bool {
        let BucketPlacementData {
            bucket,
            tile,
            feature_index,
            source_id,
            sort_key_range,
            text_translate,
            icon_translate,
        } = self;
        *bucket == other.bucket
            && *tile == other.tile
            && match (feature_index, &other.feature_index) {
                (Some(a), Some(b)) => Arc::ptr_eq(a, b),
                (None, None) => true,
                _ => false,
            }
            && *source_id == other.source_id
            && *sort_key_range == other.sort_key_range
            && *text_translate == other.text_translate
            && *icon_translate == other.icon_translate
    }
}

/// The copies of symbol buckets that benchmarks' layers place, kept
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

/// A copy of the bucket `bucket` points to, if it is a symbol
/// bucket, which `HeldBuckets` holds for a benchmark's layer to place, at
/// the index this returns, until the layer drops.
#[cfg(feature = "bench")]
pub(crate) fn hold_symbol_bucket(bucket: &impl std::ops::Deref<Target = dyn Bucket>) -> Option<usize> {
    let bucket = crate::layer::layer_type::recover_bucket::<mln_style::layers::Symbol>(&**bucket)?;
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

/// What a layer draws in a frame from its source, as the engine sees it,
/// without the layer's type.
#[derive(Clone, Copy)]
pub(crate) enum LayerSource<'a> {
    /// The source's render tiles, and the tiles that the frame's updates of
    /// the layers found ([`LayerUpdateParameters::found`]), among which the
    /// layer draws those its update found.
    Tiles(FrameSourceTiles<'a>, &'a [FoundTile]),
    /// An image source's image.
    Image(ImageSourceRenderData<'a>),
}

/// A source's render tiles in a frame, with what each of them shows, as
/// the engine sees them.
#[derive(Clone, Copy)]
pub(crate) struct FrameSourceTiles<'a> {
    tiles: &'a [RenderTile],
    /// What each of `tiles` shows, at its index.
    contents: &'a [Option<&'a Contents>],
}

/// The tiles of a layer of the type `T` in a frame, each with what it shows
/// the layer, which keeps up to 64 of them on the stack.
pub(crate) type FrameTiles<'a, T> = SmallVec<[FrameTile<'a, T>; 64]>;

impl<'a> FrameSourceTiles<'a> {
    /// The render tiles `tiles`, which show `contents`, at their indexes.
    #[must_use]
    pub(crate) fn new(tiles: &'a [RenderTile], contents: &'a [Option<&'a Contents>]) -> Self {
        FrameSourceTiles { tiles, contents }
    }

    /// The tiles at the indexes of `order`, in that order, as the layer
    /// `layer_id` of the type `T` sees them, each with what it shows the
    /// layer, found by the layer's id. `found` keeps where.
    fn find<T: LayerType>(self, order: &[usize], layer_id: &str, found: &mut Vec<FoundTile>) -> FrameTiles<'a, T> {
        let mut tiles = FrameTiles::new();
        for &index in order {
            let Some(tile) = self.tiles.get(index) else { continue };
            let (frame_tile, view) = FrameTile::find(tile, self.contents.get(index).copied().flatten(), layer_id);
            tiles.push(frame_tile);
            found.push(FoundTile { tile: index, view });
        }
        tiles
    }

    /// The tiles of `found`, as [`find`](Self::find) found them for the
    /// layer `layer_id` of the type `T` in the same frame.
    fn at<'f, T: LayerType>(self, found: impl IntoIterator<Item = &'f FoundTile>, layer_id: &str) -> FrameTiles<'a, T> {
        found
            .into_iter()
            .filter_map(|&FoundTile { tile, view }| {
                let render_tile = self.tiles.get(tile);
                debug_assert!(render_tile.is_some(), "a tile found past the frame's render tiles");
                let contents = self.contents.get(tile).copied().flatten();
                Some(FrameTile::at(render_tile?, contents, layer_id, view))
            })
            .collect()
    }

    /// The tiles of `found`, as [`at`](Self::at) gives them, that overlap
    /// `tile`: it, its ancestors and its descendants, in its world copy, or
    /// on the `globe`, which has no copies, whatever their copy. A drape of
    /// `tile` draws each in `tile`'s copy ([`FrameTile::draw_id`]).
    pub(crate) fn overlapping<T: LayerType>(
        self,
        found: &[FoundTile],
        layer_id: &str,
        tile: UnwrappedTileID,
        globe: bool,
    ) -> FrameTiles<'a, T> {
        self.at(self.overlaps(found, tile, globe).map(|(found, _)| found), layer_id)
    }

    /// What a layer draws of the tiles of `found` that
    /// [`overlapping`](Self::overlapping) gives, in its order, under the ids
    /// it draws them in.
    pub(crate) fn stamps(
        self,
        found: &[FoundTile],
        tile: UnwrappedTileID,
        globe: bool,
    ) -> impl Iterator<Item = TileStamp> {
        self.overlaps(found, tile, globe).map(move |(&FoundTile { tile: index, view }, render_tile)| {
            let contents = self.contents.get(index).copied().flatten();
            let stamp = contents.zip(view).and_then(|(contents, place)| contents.stamp(place));
            TileStamp {
                tile: OverscaledTileID { wrap: tile.wrap, ..*render_tile.overscaled_tile_id() },
                bucket: stamp.map(|(bucket, _)| bucket),
                generations: stamp.map_or([0, 0], |(_, generations)| generations),
            }
        })
    }

    /// The tiles of `found` that overlap `tile`, with their render tiles:
    /// in its world copy, or, on the `globe`, in any.
    fn overlaps<'f>(
        self,
        found: &'f [FoundTile],
        tile: UnwrappedTileID,
        globe: bool,
    ) -> impl Iterator<Item = (&'f FoundTile, &'a RenderTile)> {
        found.iter().filter_map(move |found| {
            let render_tile = self.tiles.get(found.tile)?;
            let id = render_tile.id;
            let tile = if globe { UnwrappedTileID { wrap: id.wrap, ..tile } } else { tile };
            (id == tile || id.is_child_of(&tile) || tile.is_child_of(&id)).then_some((found, render_tile))
        })
    }
}

/// What a layer reads in the frame's update, besides its tiles.
pub(crate) struct LayerUpdateParameters<'a> {
    /// The frame's camera.
    pub(crate) view: &'a View,
    /// The update the frame draws.
    pub(crate) update_parameters: &'a UpdateParameters,
    /// The layer's source, `None` for a layer without one or a source type
    /// the renderer has no render source for.
    pub(crate) source: Option<&'a dyn RenderSource>,
    /// What each of the source's render tiles shows, at its index.
    pub(crate) contents: &'a [Option<&'a Contents>],
    /// The images of the style.
    pub(crate) images: &'a crate::image::image_store::ImageStore,
    /// The tiles of the frame's layers, as their updates find them, which
    /// the update adds the layer's to: those the encode draws.
    pub(crate) found: &'a mut Vec<FoundTile>,
}

impl<'a> LayerUpdateParameters<'a> {
    /// The source's tiles, and their indexes in `order`; `None` for a layer
    /// without a source or a source without tiles.
    #[must_use]
    fn tiles(&self, order: TileOrder) -> Option<(FrameSourceTiles<'a>, &'a [usize])> {
        let source = self.source?;
        Some((FrameSourceTiles::new(source.render_tiles(), self.contents), source.tile_order(order)?))
    }
}

/// What every render layer has, whatever its type.
pub(crate) struct RenderLayerBase {
    pub(crate) evaluated_properties: Arc<dyn ErasedEvaluatedLayer>,
    pub(crate) style: Arc<dyn ErasedLayerStyle>,

    pub(crate) passes: RenderPass,
    /// What the layer's last evaluation said of how it draws.
    pub(crate) evaluation: Evaluation,
    pub(crate) style_dependencies: Dependency,
    /// Whether the frame draws the layer.
    pub(crate) is_renderable: bool,

    /// The projection variant this layer's draws are built with.
    pub(crate) projection_variant: ProjectionVariant,

    /// Where the tiles the layer's update found are among the frame's
    /// ([`LayerUpdateParameters::found`]).
    found: Range<usize>,
}

impl RenderLayerBase {
    pub(crate) fn new(properties: Arc<dyn ErasedEvaluatedLayer>) -> Self {
        let style = properties.shared_style();
        RenderLayerBase {
            evaluated_properties: properties,
            style,
            passes: RenderPass::None,
            evaluation: Evaluation::NONE,
            style_dependencies: Dependency::NONE,
            is_renderable: false,
            projection_variant: ProjectionVariant::Mercator,
            found: 0..0,
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
    /// cross-fade for a type that has one, and no render passes yet.
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

/// What a layer's evaluation says of how it draws with its values.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct Evaluation {
    /// Whether the layer draws anything.
    pub(crate) draws: bool,
    /// Whether the layer may draw in the opaque pass, which the frame draws
    /// front to back before it blends the rest.
    pub(crate) opaque: bool,
    /// The one solid color the layer is, which the frame clears to instead
    /// of drawing the layer when it is the style's bottom layer.
    pub(crate) solid: Option<Color>,
    /// Whether the layer draws a drape by the map's bearing: something of
    /// it is anchored to the viewport.
    pub(crate) drapes_by_bearing: bool,
}

impl Evaluation {
    /// The layer draws nothing.
    pub(crate) const NONE: Evaluation =
        Evaluation { draws: false, opaque: false, solid: None, drapes_by_bearing: false };
    /// The layer draws, translucent.
    pub(crate) const TRANSLUCENT: Evaluation = Evaluation { draws: true, ..Evaluation::NONE };
    /// The layer draws, and may draw in the opaque pass.
    pub(crate) const OPAQUE: Evaluation = Evaluation { opaque: true, ..Evaluation::TRANSLUCENT };

    /// The passes a layer of this evaluation draws in.
    fn passes(self) -> RenderPass {
        match self {
            Evaluation { draws: false, .. } => RenderPass::None,
            Evaluation { opaque: true, .. } => RenderPass::Opaque | RenderPass::Translucent,
            Evaluation { opaque: false, .. } => RenderPass::Translucent,
        }
    }
}

/// What a layer's [`TypedLayer::evaluate`] reads and completes.
pub(crate) struct EvaluateCx<'a, L: TypedLayer + ?Sized> {
    /// What the values were evaluated for.
    pub(crate) parameters: &'a PropertyEvaluationParameters,
    /// The new evaluation of the values, which becomes the layer's once the
    /// hook returns.
    pub(crate) evaluated: &'a mut EvaluatedLayer<L::Type>,
}

/// How a layer type meets terrain.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum OnTerrain {
    /// The layer draws flat, at sea level.
    Flat,
    /// The layer drapes over terrain: it draws flat into the terrain tiles'
    /// textures ([`LayerParts::update_drape_draws`]) rather than onto the
    /// frame.
    Drapes,
    /// The layer stands on terrain: over terrain, its draws are of the
    /// terrain variant ([`ProjectionVariant::stands`]), and their points
    /// stand on the frame's ground ([`PaintParameters::ground`]). One that
    /// `hides_behind_ground` hides its points behind the ground in front of
    /// them, which reads the ground's depth ([`FrameGround::depth`]).
    ///
    /// [`FrameGround::depth`]: crate::render::terrain::FrameGround::depth
    Stands { hides_behind_ground: bool },
}

impl OnTerrain {
    /// Whether the layer drapes over terrain.
    pub(crate) fn drapes(self) -> bool {
        self == OnTerrain::Drapes
    }

    /// Whether the layer stands on terrain.
    pub(crate) fn stands(self) -> bool {
        matches!(self, OnTerrain::Stands { .. })
    }

    /// Whether the layer hides behind the ground in front of it.
    pub(crate) fn hides_behind_ground(self) -> bool {
        self == OnTerrain::Stands { hides_behind_ground: true }
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
    type Drawing: LayerDrawing + Default + 'static;

    /// The order the layer draws its source's tiles in.
    const TILE_ORDER: TileOrder = TileOrder::ById;

    /// How the layer meets terrain.
    const TERRAIN: OnTerrain = OnTerrain::Flat;

    /// Whether the layer draws 3D geometry, whose depth it writes in the
    /// range of [`PaintParameters::depth_mode_for_3d`]. The layers above the
    /// lowest such layer that draws draw without a depth test, and on the
    /// globe the planet's depth hides its geometry behind the horizon.
    const VOLUME: bool = false;

    fn parts(&self) -> Parts<'_, Self>;
    fn parts_mut(&mut self) -> PartsMut<'_, Self>;

    /// Called once the layer's paint values are evaluated for a frame,
    /// before they become the layer's: the layer derives the values of
    /// `cx.evaluated` that its type derives from others, remakes what it
    /// makes of them, such as a color ramp, and says how it draws with them.
    fn evaluate(&mut self, _cx: EvaluateCx<'_, Self>) -> Evaluation {
        Evaluation::TRANSLUCENT
    }

    /// The light that the terrain's surface can shade the layer's drapes
    /// with in the frame of `parameters`, whose draws then put in them what
    /// it shades ([`PaintParameters::drape_lit`]); `None` for a layer whose
    /// drapes hold its colors.
    fn surface_light(&mut self, _parameters: &PaintParameters<'_>) -> Option<SurfaceLight> {
        None
    }

    /// Called in the frame's update of a layer with a source with `tiles`,
    /// the frame's tiles of the source in the order the layer draws them,
    /// each with what it shows the layer: none for an image source. The
    /// layer collects here what the update needs of its tiles' buckets,
    /// such as their dash patterns or their symbols.
    fn update(&mut self, _parameters: &mut LayerUpdateParameters<'_>, _tiles: &[FrameTile<'_, Self::Type>]) {}

    /// [`update`](Self::update) of a layer of a type without a source. One
    /// that keeps tiles ([`LayerDrawing::tiles`]) picks them here, and draws
    /// nothing in a frame it picks none in.
    fn update_without_source(&mut self, _parameters: &mut LayerUpdateParameters<'_>) {}

    /// Called in the encode when the frame draws in another projection than
    /// the last, once the engine dropped the layer's draws, for the layer to
    /// drop what else it made for the last projection.
    fn projection_changed(&mut self) {}

    /// Makes the layer's draws of the frame, in the frame's encode, once the
    /// engine started them ([`LayerDrawing::begin_frame`]), of a layer that
    /// draws in a pass and whose source has tiles to draw: from `tiles`, the
    /// frame's tiles of the layer, in the order the update gave them, each
    /// with what it shows the layer. The layer keeps what it makes of them,
    /// with the values of their uniforms.
    fn update_draws(
        &mut self,
        _gpu: &mut dyn Gpu,
        _parameters: &mut PaintParameters<'_>,
        _tiles: &[FrameTile<'_, Self::Type>],
    ) {
    }

    /// [`update_draws`](Self::update_draws) of a layer whose source is an
    /// image source, from its image.
    fn update_image_draws(
        &mut self,
        _gpu: &mut dyn Gpu,
        _parameters: &mut PaintParameters<'_>,
        _image: ImageSourceRenderData<'_>,
    ) {
    }

    /// [`update_draws`](Self::update_draws) of a layer of a type without a
    /// source.
    fn update_draws_without_source(&mut self, _gpu: &mut dyn Gpu, _parameters: &mut PaintParameters<'_>) {}

    /// Drops what the layer keeps for its draws besides their GPU objects
    /// and its tiles' entries, when it stops rendering.
    fn release(&mut self) {}

    /// Called when the layer's style changed to another of the same id and
    /// type, once it is the layer's and before its values transition. A
    /// custom layer whose host the style replaced puts the old host that
    /// drew in `ended`.
    fn style_changed(&mut self, _ended: &mut EndedHosts) {}
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
    /// the draws again from the tiles' buckets. A custom layer's host that
    /// drew and that the style replaced goes to `ended`.
    fn restyle(&mut self, parameters: &TransitionParameters, style: &Arc<dyn ErasedLayerStyle>, ended: &mut EndedHosts);

    /// Begins transitions for the paint values that changed since the last
    /// frame, to those of the layer's style.
    fn transition(&mut self, parameters: &TransitionParameters);

    /// Evaluates the layer's paint values for a frame of `parameters`,
    /// which the layer completes ([`TypedLayer::evaluate`]), and sets the
    /// passes the layer draws in with them.
    fn evaluate(&mut self, parameters: &PropertyEvaluationParameters);

    /// Whether a paint value of the layer is still in transition.
    fn has_transition(&self) -> bool;

    /// Whether the layer has a pattern property and is in a cross-fade.
    fn has_crossfade(&self) -> bool;

    /// The frame's update of the layer: finds the tiles of `parameters`'
    /// source the layer draws, which it adds to `parameters.found`, adds the
    /// passes their buckets were laid out for to the layer's, runs the
    /// layer's [`TypedLayer::update`], and keeps entries for the tiles; or
    /// runs the [`TypedLayer::update_without_source`] of a layer without a
    /// source.
    fn update(&mut self, parameters: &mut LayerUpdateParameters<'_>);

    /// The encode's update of the layer's draws: picks the shader variant
    /// for the frame's projection, standing on the frame's ground where the
    /// layer stands and the frame has one, dropping the draws when it
    /// changed; drops
    /// the draws of a layer that has nothing to draw, and otherwise starts
    /// its draws of the frame ([`LayerDrawing::begin_frame`]); drops them
    /// again for a layer that draws in no pass, and otherwise runs the
    /// layer's [`TypedLayer::update_draws`] with the tiles of its type that
    /// its update found in `source` (its [`LayerSource::Tiles`]), its
    /// [`TypedLayer::update_image_draws`] with an image source's image, or
    /// its [`TypedLayer::update_draws_without_source`].
    fn update_draws(
        &mut self,
        gpu: &mut dyn Gpu,
        parameters: &mut PaintParameters<'_>,
        source: Option<LayerSource<'_>>,
    );

    /// How the layer meets terrain ([`TypedLayer::TERRAIN`]).
    fn terrain(&self) -> OnTerrain;

    /// Whether the layer draws 3D geometry ([`TypedLayer::VOLUME`]).
    fn volume(&self) -> bool;

    /// Whether the layer blends with what is under it otherwise than by
    /// covering it, with its values of the frame: a layer blend
    /// ([`StyleLayerType::layer_blend`]) other than `normal`.
    fn blends(&self) -> bool;

    /// The light the terrain's surface can shade the layer's drapes with
    /// ([`TypedLayer::surface_light`]).
    fn surface_light(&mut self, parameters: &PaintParameters<'_>) -> Option<SurfaceLight>;

    /// An empty drawing of the layer's type, which a drape keeps of the
    /// layer.
    fn new_drape_drawing(&self) -> Option<DrapeDrawing>;

    /// The encode's update of the layer's draws into the drape of the
    /// terrain tile `drape`, with whether the frame is on the globe, which
    /// keeps them in `drawing`, one of the layer's
    /// ([`new_drape_drawing`](Self::new_drape_drawing)): as
    /// [`update_draws`](Self::update_draws) makes the frame's, with the
    /// drape's `parameters`, from the tiles of `source` the layer's update
    /// found that overlap the drape's tile
    /// ([`FrameSourceTiles::overlapping`]), from the copies of an image
    /// source's image that overlap it ([`ImageSourceRenderData::overlaps`]),
    /// or from the drape's tile itself for a layer without a source, in the
    /// Mercator projection, a drape's.
    fn update_drape_draws(
        &mut self,
        gpu: &mut dyn Gpu,
        parameters: &mut PaintParameters<'_>,
        source: Option<LayerSource<'_>>,
        drape: (OverscaledTileID, bool),
        drawing: &mut DrapeDrawing,
    );

    /// Adds to `stamps` what the layer draws of each tile that
    /// [`update_drape_draws`](Self::update_drape_draws) makes its draws in
    /// `drape` from: for an image source, its bucket and geometry in each
    /// copy of the image that overlaps the drape; none for a layer without
    /// a source.
    fn drape_stamps(
        &self,
        source: Option<LayerSource<'_>>,
        drape: (OverscaledTileID, bool),
        stamps: &mut Vec<TileStamp>,
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

    /// Visits the layer's draw groups of the frame, in drawing order: none
    /// unless the frame draws the layer. They are the groups `upload` and
    /// `render` upload and draw.
    fn visit_draw_groups<'a>(&'a self, visit: &mut dyn FnMut(DrawGroup<'a>));

    /// Uploads the GPU objects of the layer's draw groups, before the frame
    /// draws them.
    fn upload(&mut self, gpu: &mut dyn Gpu, context: &mut Context);

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

    fn restyle(
        &mut self,
        parameters: &TransitionParameters,
        style: &Arc<dyn ErasedLayerStyle>,
        ended: &mut EndedHosts,
    ) {
        self.remove_all_draws();
        let PartsMut { base, values, .. } = self.parts_mut();
        if let Some(recovered) = LayerStyle::recover(style) {
            values.style = recovered;
            base.style = Arc::clone(style);
        }
        self.style_changed(ended);
        self.transition(parameters);
    }

    fn transition(&mut self, parameters: &TransitionParameters) {
        let PartsMut { base, values, .. } = self.parts_mut();
        let prior = std::mem::take(&mut values.unevaluated);
        values.unevaluated = values.style.paint.transitioned(parameters, prior);
        base.style_dependencies = <L::Type as StyleLayerType>::PaintProperties::dependencies(&values.unevaluated);
    }

    fn evaluate(&mut self, parameters: &PropertyEvaluationParameters) {
        let mut evaluated = self.parts_mut().values.evaluate(parameters);
        let evaluation = TypedLayer::evaluate(self, EvaluateCx { parameters, evaluated: &mut evaluated });
        let PartsMut { base, values, .. } = self.parts_mut();
        base.passes = evaluation.passes();
        base.evaluation = evaluation;
        values.set_evaluated(base, evaluated);
    }

    fn has_transition(&self) -> bool {
        <L::Type as StyleLayerType>::PaintProperties::has_transition(&self.parts().values.unevaluated)
    }

    fn has_crossfade(&self) -> bool {
        L::Type::CROSSFADES && self.parts().values.evaluated.crossfade.t != 1.0
    }

    fn update(&mut self, parameters: &mut LayerUpdateParameters<'_>) {
        if <L::Type as StyleLayerType>::INFO.source == SourceKind::None {
            TypedLayer::update_without_source(self, parameters);
            return;
        }
        let base = self.base_mut();
        let start = parameters.found.len();
        let tiles = match parameters.tiles(L::TILE_ORDER) {
            Some((tiles, order)) => tiles.find::<L::Type>(order, &base.style.base().id, parameters.found),
            None => FrameTiles::new(),
        };
        base.found = start..parameters.found.len();
        for bucket in tiles.iter().filter_map(|tile| tile.bucket()) {
            base.passes |= bucket.passes();
        }
        TypedLayer::update(self, parameters, &tiles);
        if let Some(kept) = self.parts_mut().drawing.tiles() {
            kept.set_tiles(tiles.iter().map(|tile| tile.id()));
        }
    }

    fn update_draws(
        &mut self,
        gpu: &mut dyn Gpu,
        parameters: &mut PaintParameters<'_>,
        source: Option<LayerSource<'_>>,
    ) {
        let stands = L::TERRAIN.stands() && parameters.ground.is_some();
        use_projection_variant(self, ProjectionVariant::new(parameters.view.is_globe(), stands));
        if <L::Type as StyleLayerType>::INFO.source == SourceKind::None {
            // A layer that keeps tiles draws those its update picked.
            if self.parts_mut().drawing.tiles().is_some_and(|tiles| tiles.is_empty()) {
                self.remove_all_draws();
            } else if begin_frame(self) {
                TypedLayer::update_draws_without_source(self, gpu, parameters);
            }
            return;
        }
        match source {
            Some(LayerSource::Tiles(source, found))
                if let Some(found) = found.get(self.base().found.clone()).filter(|found| !found.is_empty()) =>
            {
                let tiles = source.at::<L::Type>(found, self.id());
                if begin_frame(self) {
                    TypedLayer::update_draws(self, gpu, parameters, &tiles);
                }
            }
            Some(LayerSource::Image(image)) => {
                if begin_frame(self) {
                    TypedLayer::update_image_draws(self, gpu, parameters, image);
                }
            }
            _ => self.remove_all_draws(),
        }
    }

    fn terrain(&self) -> OnTerrain {
        L::TERRAIN
    }

    fn volume(&self) -> bool {
        L::VOLUME
    }

    fn blends(&self) -> bool {
        L::Type::layer_blend(&self.parts().values.evaluated.paint) != LayerBlendType::Normal
    }

    fn surface_light(&mut self, parameters: &PaintParameters<'_>) -> Option<SurfaceLight> {
        TypedLayer::surface_light(self, parameters)
    }

    fn new_drape_drawing(&self) -> Option<DrapeDrawing> {
        // Only a layer that drapes builds the code of a drawing in a drape.
        L::TERRAIN.drapes().then(|| DrapeDrawing::new(L::Drawing::default()))
    }

    fn update_drape_draws(
        &mut self,
        gpu: &mut dyn Gpu,
        parameters: &mut PaintParameters<'_>,
        source: Option<LayerSource<'_>>,
        drape: (OverscaledTileID, bool),
        drawing: &mut DrapeDrawing,
    ) {
        let Some(drawing) = drawing.downcast_mut::<L::Drawing>().filter(|_| L::TERRAIN.drapes()) else {
            debug_assert!(false, "a drape's drawing of another layer's type");
            return;
        };
        // The drape's drawing is the layer's while the layer's hooks make its
        // draws.
        std::mem::swap(self.parts_mut().drawing, drawing);
        update_drape(self, gpu, parameters, source, drape);
        std::mem::swap(self.parts_mut().drawing, drawing);
    }

    fn drape_stamps(
        &self,
        source: Option<LayerSource<'_>>,
        (drape_tile, globe): (OverscaledTileID, bool),
        stamps: &mut Vec<TileStamp>,
    ) {
        let drape = drape_tile.to_unwrapped();
        match source {
            Some(LayerSource::Tiles(source, found)) => {
                let found = found.get(self.base().found.clone()).unwrap_or_default();
                stamps.extend(source.stamps(found, drape, globe));
            }
            Some(LayerSource::Image(image)) => {
                let bucket = image.bucket.map(|bucket| (bucket.id(), bucket.generation()));
                stamps.extend(image.copies_over(&drape, globe).map(|copy| TileStamp {
                    tile: copy.overscale_to(copy.canonical.z),
                    bucket: bucket.map(|(id, _)| id),
                    generations: [bucket.map_or(0, |(_, generation)| generation), 0],
                }));
            }
            None => {}
        }
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
            if let Some(kept) = self.parts_mut().drawing.tiles() {
                kept.clear_tiles();
            }
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

    fn upload(&mut self, gpu: &mut dyn Gpu, context: &mut Context) {
        let PartsMut { base, drawing, .. } = self.parts_mut();
        if base.is_renderable {
            drawing.upload(gpu, context);
        }
    }

    fn render(&mut self, gpu: &mut dyn Gpu, parameters: &mut PaintParameters<'_>, order: DrawOrder) {
        let PartsMut { base, drawing, .. } = self.parts_mut();
        if base.is_renderable {
            drawing.render(gpu, parameters, order);
        }
    }
}

/// [`LayerParts::update_drape_draws`] of `layer`, whose drawing is the
/// drape's.
fn update_drape<L: TypedLayer>(
    layer: &mut L,
    gpu: &mut dyn Gpu,
    parameters: &mut PaintParameters<'_>,
    source: Option<LayerSource<'_>>,
    (drape_tile, globe): (OverscaledTileID, bool),
) {
    // A drape is flat: the layer's draws are Mercator's.
    use_projection_variant(layer, ProjectionVariant::Mercator);
    if <L::Type as StyleLayerType>::INFO.source == SourceKind::None {
        if let Some(kept) = layer.parts_mut().drawing.tiles() {
            kept.set_tiles([drape_tile]);
        }
        if begin_frame(layer) {
            TypedLayer::update_draws_without_source(layer, gpu, parameters);
        }
        return;
    }
    let drape = drape_tile.to_unwrapped();
    let tiles = match source {
        Some(LayerSource::Tiles(source, found)) => {
            let found = found.get(layer.base().found.clone()).unwrap_or_default();
            source.overlapping::<L::Type>(found, layer.id(), drape, globe)
        }
        Some(LayerSource::Image(image)) => {
            let copies: SmallVec<[UnwrappedTileID; 4]> = image.copies_over(&drape, globe).collect();
            if copies.is_empty() {
                layer.remove_all_draws();
            } else if begin_frame(layer) {
                let image = ImageSourceRenderData { tile_ids: &copies, ..image };
                TypedLayer::update_image_draws(layer, gpu, parameters, image);
            }
            return;
        }
        None => FrameTiles::new(),
    };
    if let Some(kept) = layer.parts_mut().drawing.tiles() {
        kept.set_tiles(tiles.iter().map(|tile| tile.draw_id(parameters)));
    }
    if tiles.is_empty() {
        layer.remove_all_draws();
    } else if begin_frame(layer) {
        TypedLayer::update_draws(layer, gpu, parameters, &tiles);
    }
}

/// Makes `layer` draw in the projection `variant`: where its draws were of
/// another, drops them and what the layer made for that one.
fn use_projection_variant<L: TypedLayer>(layer: &mut L, variant: ProjectionVariant) {
    if variant != layer.base().projection_variant {
        layer.base_mut().projection_variant = variant;
        layer.remove_all_draws();
        layer.projection_changed();
    }
}

/// Starts the frame's draws of `layer`, which has something to draw, and
/// returns whether it draws in a pass with its values of the frame. One that
/// doesn't drops the frame's draws, and keeps what it keeps of its tiles.
fn begin_frame<L: TypedLayer + ?Sized>(layer: &mut L) -> bool {
    let PartsMut { values, drawing, .. } = layer.parts_mut();
    drawing.begin_frame();
    let draws = values.evaluated.render_passes != RenderPass::None.underlying_type();
    if !draws {
        drawing.clear_frame();
    }
    draws
}

/// What a layer type does that the engine can't do for it, whatever the
/// type: the hooks the engine calls on the layers it holds.
pub(crate) trait RenderLayer: LayerParts {
    /// Uploads what draws into the layer's render targets, which draw before
    /// the frame's passes.
    fn upload_render_targets(&mut self, _gpu: &mut dyn Gpu, _context: &mut Context) {}

    /// Draws the layer's render targets through `gpu`, the context's, ahead
    /// of the frame's passes, which sample them.
    fn render_targets(&mut self, _gpu: &mut dyn Gpu, _parameters: &mut PaintParameters<'_>) {}

    /// Prepares again, in a later frame, the kept render targets that the
    /// last [`render_targets`](Self::render_targets) prepared: its frame's
    /// submission failed, so they hold nothing.
    fn prepare_again(&mut self) {}

    /// Runs a custom layer's prerender through `gpu`, the context's, after
    /// the layers' draws upload and before the frame's first pass.
    fn prerender(&mut self, _gpu: &mut dyn Gpu, _parameters: &mut PaintParameters<'_>) {}

    /// Lets go of a custom layer's host as the layer leaves the renderer: a
    /// host that drew goes to `ended`, to hear that the renderer stopped.
    /// The orchestrator calls it before it drops the layer.
    fn leave(&mut self, _ended: &mut EndedHosts) {}

    /// Tells a custom layer's host that the renderer's graphics objects are
    /// lost, and lets go of it.
    fn lose_context(&mut self) {}

    /// Returns true if the layer has to be placed.
    fn needs_placement(&self) -> bool {
        !self.placement_data().is_empty()
    }

    /// The buckets the layer places, which only a symbol layer has.
    fn placement_data(&self) -> &[BucketPlacementData] {
        &[]
    }

    /// The buckets that another layer places and this layer draws,
    /// at its own translation.
    fn follower_data(&self) -> &[BucketPlacementData] {
        &[]
    }

    /// Whether the feature `feature` with `geometry`, with `feature_state`,
    /// intersects `region` of its tile as the layer draws it at the view's
    /// zoom; false unless a layer type overrides it.
    fn query_intersects_feature(
        &self,
        _region: &QueryRegion<'_>,
        _feature: &dyn mln_style::feature::GeometryTileFeature,
        _geometry: &mln_style::feature::GeometryCollection,
        _feature_state: &mln_style::feature::FeatureState,
    ) -> bool {
        false
    }
}
