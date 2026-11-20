// A render layer is the renderer's side of a style layer: a `Layer<T>` of
// its type `T`, which holds the layer's values and what the engine keeps of
// it, beside what its type keeps (`LayerType::State` and
// `LayerType::Drawing`). The engine does what is the same for every type
// around the type's hooks, and holds the layers as `dyn RenderLayer`. The
// orchestrator owns the render layers, in the style's order.

use std::any::Any;
use std::ops::Range;
use std::sync::Arc;

use glam::DMat4;
use mln_gfx::gpu::Gpu;
use mln_gfx::projection_variant::ProjectionVariant;
use mln_map_types::{ProjectionData, View};
use mln_style::expression::Dependency;
use mln_style::layer_type::{ErasedLayerStyle, LayerStyle, PaintSet, SourceKind};
use mln_style::{PropertyEvaluationParameters, TransitionParameters};
use mln_style_types::image::ImagePosition;
use mln_style_types::{TranslateAnchorType, VisibilityType};
use mln_tile_id::{OverscaledTileID, UnwrappedTileID};
use mln_util::Color;
use smallvec::SmallVec;

use crate::bucket::Bucket;
use crate::bucket_geometry::BucketGeometry;
use crate::buckets::symbol_bucket::SymbolBucket;
use crate::geometry::feature_index::FeatureIndex;
use crate::gfx::context::Context;
use crate::layer::{DrawContext, EndedHost, HitTest, LayerType, LayerValues, UpdateContext};
use crate::layer_drawing::{ChangedBuckets, DrawGroup, DrawOrder, LayerDrawing};
use crate::layer_type::{ErasedEvaluatedLayer, EvaluatedLayer};
use crate::line_atlas::LineAtlas;
use crate::paint_parameters::PaintParameters;
use crate::pattern_atlas::PatternAtlas;
use crate::render_pass::RenderPass;
use crate::render_source::{RenderSource, TileOrder};
use crate::render_tile::{FindView, FoundTile, FrameTile, RenderTile, TileContent, ViewPlace};
use crate::tile::contents::GeometryContents;
use crate::tile::job::SourceKey;
use crate::tile_render_data::TileAtlasTextures;
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

/// A bucket of a layer of the type `T` in a tile's layout, with the layer's
/// values it was laid out with and the rest of what the layout's contents
/// hold for the tile's draws.
pub struct LayerBucket<'a, T: LayerType> {
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
            bucket: crate::layer_type::recover_bucket::<T>(bucket)?,
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

impl<'a, T: LayerType> FindView<'a> for LayerBucket<'a, T> {
    fn find(content: TileContent<'a>, layer_id: &str) -> Option<(Self, ViewPlace)> {
        let TileContent::Tile(contents) = content else { return None };
        let contents = contents.geometry()?;
        let index = contents.result.layer_render_data.find(layer_id)?;
        Some((LayerBucket::laid(contents, index)?, ViewPlace::Laid(index)))
    }

    fn at(content: TileContent<'a>, place: ViewPlace) -> Option<Self> {
        let ViewPlace::Laid(index) = place else {
            debug_assert!(false, "a tile's own data's place for a laid-out layer");
            return None;
        };
        let TileContent::Tile(contents) = content else { return None };
        LayerBucket::laid(contents.geometry()?, index)
    }

    fn is_for(&self, layer_id: &str) -> bool {
        self.layer.style.id == layer_id
    }

    fn passes(self) -> RenderPass {
        self.layer.passes()
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
pub struct BucketPlacementData {
    pub(crate) bucket: BucketKey,
    pub(crate) tile: PlacedTile,
    pub(crate) feature_index: Option<Arc<FeatureIndex>>,
    pub(crate) source_id: Arc<str>,
    pub(crate) sort_key_range: Option<crate::buckets::symbol_bucket::SortKeyRange>,
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

/// A source's render tiles in a frame, with what each of them holds, as
/// the engine sees them.
#[derive(Clone, Copy)]
pub(crate) struct FrameSourceTiles<'a> {
    tiles: &'a [RenderTile],
    /// What each of `tiles` holds, at its index.
    contents: &'a [Option<TileContent<'a>>],
}

/// The tiles of a layer of the type `T` in a frame, each with what it shows
/// the layer, which keeps up to 64 of them on the stack.
pub(crate) type FrameTiles<'a, T> = SmallVec<[FrameTile<'a, T>; 64]>;

impl<'a> FrameSourceTiles<'a> {
    /// The render tiles `tiles`, which hold `contents`, at their indexes.
    #[must_use]
    pub(crate) fn new(tiles: &'a [RenderTile], contents: &'a [Option<TileContent<'a>>]) -> Self {
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
    fn at<T: LayerType>(self, found: &[FoundTile], layer_id: &str) -> FrameTiles<'a, T> {
        found
            .iter()
            .filter_map(|&FoundTile { tile, view }| {
                let render_tile = self.tiles.get(tile);
                debug_assert!(render_tile.is_some(), "a tile found past the frame's render tiles");
                let content = self.contents.get(tile).copied().flatten();
                Some(FrameTile::at(render_tile?, content, layer_id, view))
            })
            .collect()
    }
}

/// The atlases that the layers' draws share: the line atlas of the dash
/// patterns of `line-dasharray`s that don't differ by feature, and the
/// pattern atlas of `background-pattern`s. A frame's update adds what its
/// layers draw, and its encode uploads them and binds their textures.
#[derive(Default)]
pub struct Atlases {
    pub(crate) line: LineAtlas,
    pub pattern: PatternAtlas,
}

/// The hosts that left the renderer after they drew through its layers
/// ([`LayerHost`](crate::layer::LayerHost)), until [`end_hosts`] tells
/// them that it stopped.
pub(crate) type EndedHosts = Vec<Box<dyn EndedHost>>;

/// Tells `hosts`, which left the renderer after drawing with `gpu`, that it
/// stopped, once `gpu` finished their work, so that they free what they drew
/// with.
pub(crate) fn end_hosts(gpu: &mut dyn Gpu, hosts: EndedHosts) {
    if hosts.is_empty() {
        return;
    }
    if let Err(error) = gpu.flush(true) {
        mln_util::logging::Log::error(
            mln_util::logging::Event::Render,
            format!("Waiting for the GPU before ending the layers' hosts failed: {error}"),
        );
    }
    for host in hosts {
        host.end();
    }
}

/// What the engine reads of the frame in a layer's update, besides the
/// layer.
pub(crate) struct LayerUpdate<'a> {
    pub(crate) view: &'a View,
    pub(crate) update_parameters: &'a UpdateParameters,
    /// The layer's source, `None` for a layer without one or a source type
    /// the renderer has no render source for.
    pub(crate) source: Option<&'a dyn RenderSource>,
    /// What each of the source's render tiles holds, at its index.
    pub(crate) contents: &'a [Option<TileContent<'a>>],
    pub(crate) images: &'a crate::image_store::ImageStore,
    pub(crate) atlases: &'a mut Atlases,
    /// The tiles of the frame's layers, as their updates find them, which
    /// the update adds the layer's to: those the encode draws.
    pub(crate) found: &'a mut Vec<FoundTile>,
}

impl<'a> LayerUpdate<'a> {
    /// The source's tiles, and their indexes in `order`; `None` for a layer
    /// without a source or a source without tiles.
    #[must_use]
    fn tiles(&self, order: TileOrder) -> Option<(FrameSourceTiles<'a>, &'a [usize])> {
        let source = self.source?;
        Some((FrameSourceTiles::new(source.render_tiles(), self.contents), source.tile_order(order)?))
    }
}

/// A render layer of the layer type `T`: the layer's values, what the
/// engine keeps of it, and what its type keeps.
pub(crate) struct Layer<T: LayerType> {
    values: LayerValues<T>,
    state: T::State,
    drawing: T::Drawing,
    passes: RenderPass,
    style_dependencies: Dependency,
    /// Whether the frame draws the layer.
    renderable: bool,
    /// The projection variant the layer's draws are built with.
    projection_variant: ProjectionVariant,
    /// Where the tiles the layer's update found are among the frame's
    /// ([`LayerUpdate::found`]).
    found: Range<usize>,
}

impl<T: LayerType> Layer<T> {
    /// The render layer of the layer `style`, before its values transition.
    pub(crate) fn new(style: Arc<LayerStyle<T>>) -> Self {
        let values = LayerValues::new(style);
        let (state, drawing) = T::new_layer(&values);
        Layer {
            style_dependencies: T::PaintProperties::dependencies(&values.unevaluated),
            values,
            state,
            drawing,
            passes: RenderPass::None,
            renderable: false,
            projection_variant: ProjectionVariant::Mercator,
            found: 0..0,
        }
    }

    /// What the layer's type keeps of it besides its drawing.
    #[cfg(feature = "bench")]
    pub(crate) fn state(&self) -> &T::State {
        &self.state
    }

    /// [`state`](Self::state), to change.
    #[cfg(feature = "bench")]
    pub(crate) fn state_mut(&mut self) -> &mut T::State {
        &mut self.state
    }

    /// The layer's values.
    #[cfg(feature = "bench")]
    pub(crate) fn values(&self) -> &LayerValues<T> {
        &self.values
    }

    /// Starts the frame's draws of the layer, which has something to draw,
    /// and returns whether it draws in a pass with its values of the frame.
    /// One that doesn't drops the frame's draws, and keeps what it keeps of
    /// its tiles.
    fn begin_frame(&mut self) -> bool {
        self.drawing.begin_frame();
        let draws = self.values.evaluated.render_passes != RenderPass::None.underlying_type();
        if !draws {
            self.drawing.clear_frame();
        }
        draws
    }

    /// Runs the type's [`LayerType::make_draws`] with `tiles`.
    fn make_draws(
        &mut self,
        gpu: &mut dyn Gpu,
        paint: &mut PaintParameters<'_>,
        atlases: &Atlases,
        tiles: &[FrameTile<'_, T>],
    ) {
        if self.begin_frame() {
            let mut cx = DrawContext {
                tiles,
                layer: &self.values,
                passes: self.passes,
                projection_variant: self.projection_variant,
                atlases,
                paint,
            };
            T::make_draws(&mut self.state, &mut self.drawing, gpu, &mut cx);
        }
    }
}

/// What the engine does with a render layer, whatever its type: its values,
/// which this transitions without knowing their type, and its drawing,
/// which keeps what the layer keeps of its tiles and uploads and draws its
/// draw groups. It runs the hooks of the layer's [`LayerType`] with what is
/// common to every layer type around them.
pub(crate) trait RenderLayer {
    /// The layer's style.
    fn style(&self) -> &dyn ErasedLayerStyle;

    /// [`style`](Self::style), shared.
    fn shared_style(&self) -> Arc<dyn ErasedLayerStyle>;

    /// The layer's values of the last frame.
    fn evaluated(&self) -> Arc<dyn ErasedEvaluatedLayer>;

    /// The layer's id.
    fn id(&self) -> &str {
        &self.style().base().id
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
    /// which the layer completes ([`LayerType::complete_evaluation`]), and
    /// sets the passes the layer draws in with them ([`LayerType::passes`]).
    fn evaluate(&mut self, parameters: &PropertyEvaluationParameters);

    /// Whether a paint value of the layer is still in transition.
    fn has_transition(&self) -> bool;

    /// Whether the layer has a pattern property and is in a cross-fade.
    fn has_crossfade(&self) -> bool;

    /// The frame's update of the layer: finds the tiles of `update`'s source
    /// the layer draws, which it adds to `update.found`, adds the passes
    /// their buckets were laid out for to the layer's, keeps entries for the
    /// tiles, and runs the type's [`LayerType::update`].
    fn update(&mut self, update: &mut LayerUpdate<'_>);

    /// The encode's update of the layer's draws: picks the shader variant
    /// for the frame's projection, dropping the draws when it changed; drops
    /// the draws of a layer that has nothing to draw, and otherwise starts
    /// its draws of the frame ([`LayerDrawing::begin_frame`]); drops them
    /// again for a layer that draws in no pass, and otherwise runs the
    /// type's [`LayerType::make_draws`] with the tiles of `source` that its
    /// update found in `found`.
    fn update_draws(
        &mut self,
        gpu: &mut dyn Gpu,
        paint: &mut PaintParameters<'_>,
        atlases: &Atlases,
        source: Option<FrameSourceTiles<'_>>,
        found: &[FoundTile],
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
    /// for them ([`LayerType::release`]): its updates don't run until it is
    /// included again, and they then rebuild everything from the source's
    /// tiles.
    fn set_renderable(&mut self, renderable: bool);

    /// Whether the frame draws the layer.
    fn is_renderable(&self) -> bool;

    /// What the layer's paint values depend on.
    fn style_dependencies(&self) -> Dependency;

    /// Whether the layer draws in a pass and is visible.
    fn needs_rendering(&self) -> bool;

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

    /// Whether the layer draws at `zoom`. A minimum zoom of 0 is no
    /// minimum: the globe zooms below 0 toward the poles.
    fn supports_zoom(&self, zoom: f32) -> bool {
        let zoom_or_zero = if zoom < 0.0 { 0.0 } else { zoom };
        self.style().base().min_zoom <= zoom_or_zero && zoom < self.style().base().max_zoom
    }

    /// See [`LayerType::IS_3D`].
    fn is_3d(&self) -> bool;

    /// See [`LayerType::solid_background`].
    fn solid_background(&self) -> Option<Color>;

    /// Uploads what the layer draws in its [`OffscreenPass`](crate::layer::OffscreenPass),
    /// if the frame draws it.
    fn upload_offscreen(&mut self, gpu: &mut dyn Gpu, context: &mut Context);

    /// Draws the layer's [`OffscreenPass`](crate::layer::OffscreenPass), if
    /// the frame draws it.
    fn render_offscreen(&mut self, gpu: &mut dyn Gpu, parameters: &mut PaintParameters<'_>);

    /// Lets go of the layer's host ([`LayerHost::leave`](crate::layer::LayerHost::leave))
    /// as the layer leaves the renderer: one that drew goes to `ended`. The
    /// orchestrator calls it before it drops the layer.
    fn leave(&mut self, ended: &mut EndedHosts);

    /// See [`LayerHost::lose_context`](crate::layer::LayerHost::lose_context).
    fn lose_context(&mut self);

    /// See [`LayerType::PLACES_SYMBOLS`].
    fn places_symbols(&self) -> bool;

    /// Returns true if the layer has to be placed.
    fn needs_placement(&self) -> bool {
        !self.placement_data().is_empty()
    }

    /// See [`LayerType::placement_data`].
    fn placement_data(&self) -> &[BucketPlacementData];

    /// See [`LayerType::follower_data`].
    fn follower_data(&self) -> &[BucketPlacementData];

    /// Whether `test`'s feature, as the layer draws it, intersects the
    /// query's geometry ([`FeatureQuery::intersects`](crate::layer::FeatureQuery::intersects));
    /// false for a type whose features no query hits.
    fn query_intersects_feature(&self, test: &HitTest<'_>) -> bool;
}

impl<T: LayerType> RenderLayer for Layer<T> {
    fn style(&self) -> &dyn ErasedLayerStyle {
        &*self.values.style
    }

    fn shared_style(&self) -> Arc<dyn ErasedLayerStyle> {
        Arc::<LayerStyle<T>>::clone(&self.values.style)
    }

    fn evaluated(&self) -> Arc<dyn ErasedEvaluatedLayer> {
        Arc::<EvaluatedLayer<T>>::clone(&self.values.evaluated)
    }

    fn tile_order(&self) -> TileOrder {
        T::TILE_ORDER
    }

    fn restyle(
        &mut self,
        parameters: &TransitionParameters,
        style: &Arc<dyn ErasedLayerStyle>,
        ended: &mut EndedHosts,
    ) {
        self.remove_all_draws();
        if let Some(recovered) = LayerStyle::recover(style) {
            self.values.style = recovered;
        }
        if let Some(host) = T::host(&mut self.drawing) {
            ended.extend(host.restyled(&self.values));
        }
        self.transition(parameters);
    }

    fn transition(&mut self, parameters: &TransitionParameters) {
        let values = &mut self.values;
        let prior = std::mem::take(&mut values.unevaluated);
        values.unevaluated = values.style.paint.transitioned(parameters, prior);
        self.style_dependencies = T::PaintProperties::dependencies(&values.unevaluated);
    }

    fn evaluate(&mut self, parameters: &PropertyEvaluationParameters) {
        let mut evaluated = self.values.evaluate(parameters);
        T::complete_evaluation(&mut self.state, &self.values, parameters, &mut evaluated);
        self.passes = T::passes(&self.values, &evaluated);
        evaluated.render_passes = self.passes.underlying_type();
        self.values.evaluated = Arc::new(evaluated);
    }

    fn has_transition(&self) -> bool {
        T::PaintProperties::has_transition(&self.values.unevaluated)
    }

    fn has_crossfade(&self) -> bool {
        T::CROSSFADES && self.values.evaluated.crossfade.t != 1.0
    }

    fn update(&mut self, update: &mut LayerUpdate<'_>) {
        let start = update.found.len();
        let tiles = match update.tiles(T::TILE_ORDER) {
            Some((tiles, order)) => tiles.find::<T>(order, &self.values.style.id, update.found),
            None => FrameTiles::new(),
        };
        self.found = start..update.found.len();
        for bucket in tiles.iter().filter_map(|tile| tile.bucket()) {
            self.passes |= bucket.passes();
        }
        if T::INFO.source != SourceKind::None
            && let Some(kept) = self.drawing.tiles()
        {
            kept.set_tiles(tiles.iter().map(|tile| tile.id()));
        }
        let mut cx = UpdateContext {
            tiles: &tiles,
            layer: &self.values,
            view: update.view,
            update_parameters: update.update_parameters,
            images: update.images,
            atlases: update.atlases,
            source: update.source,
        };
        T::update(&mut self.state, &mut self.drawing, &mut cx);
    }

    fn update_draws(
        &mut self,
        gpu: &mut dyn Gpu,
        paint: &mut PaintParameters<'_>,
        atlases: &Atlases,
        source: Option<FrameSourceTiles<'_>>,
        found: &[FoundTile],
    ) {
        let variant = if paint.view.is_globe() { ProjectionVariant::Globe } else { ProjectionVariant::Mercator };
        if variant != self.projection_variant {
            self.projection_variant = variant;
            self.remove_all_draws();
            T::projection_changed(&mut self.state, variant);
        }
        if T::INFO.source == SourceKind::None {
            // A layer that keeps tiles draws those its update picked.
            if self.drawing.tiles().is_some_and(|tiles| tiles.is_empty()) {
                self.remove_all_draws();
            } else {
                self.make_draws(gpu, paint, atlases, &[]);
            }
            return;
        }
        match source {
            Some(source) if let Some(found) = found.get(self.found.clone()).filter(|found| !found.is_empty()) => {
                let tiles = source.at::<T>(found, &self.values.style.id);
                self.make_draws(gpu, paint, atlases, &tiles);
            }
            _ => {
                self.remove_all_draws();
                T::nothing_to_draw(&mut self.state);
            }
        }
    }

    fn mark_changed(&mut self, changed: &ChangedBuckets) {
        if let Some(tiles) = self.drawing.tiles() {
            tiles.mark_changed(changed);
        }
    }

    fn handle_wrap_jump(&mut self, wrap_delta: i16) {
        if let Some(tiles) = self.drawing.tiles() {
            tiles.shift_wraps(wrap_delta);
        }
    }

    fn set_renderable(&mut self, renderable: bool) {
        self.renderable = renderable;
        if !renderable {
            self.remove_all_draws();
            if let Some(kept) = self.drawing.tiles() {
                kept.clear_tiles();
            }
            T::release(&mut self.state);
        }
    }

    fn is_renderable(&self) -> bool {
        self.renderable
    }

    fn style_dependencies(&self) -> Dependency {
        self.style_dependencies
    }

    fn needs_rendering(&self) -> bool {
        self.passes != RenderPass::None && self.values.style.visibility != VisibilityType::None
    }

    fn remove_all_draws(&mut self) {
        self.drawing.clear();
    }

    fn visit_draw_groups<'a>(&'a self, visit: &mut dyn FnMut(DrawGroup<'a>)) {
        if self.renderable {
            self.drawing.visit_groups(visit);
        }
    }

    fn upload(&mut self, gpu: &mut dyn Gpu, context: &mut Context) {
        if self.renderable {
            self.drawing.upload(gpu, context);
        }
    }

    fn render(&mut self, gpu: &mut dyn Gpu, parameters: &mut PaintParameters<'_>, order: DrawOrder) {
        if self.renderable {
            self.drawing.render(gpu, parameters, order);
        }
    }

    fn is_3d(&self) -> bool {
        T::IS_3D
    }

    fn solid_background(&self) -> Option<Color> {
        T::solid_background(&self.values.evaluated)
    }

    fn upload_offscreen(&mut self, gpu: &mut dyn Gpu, context: &mut Context) {
        if self.renderable
            && let Some(pass) = T::offscreen_pass(&mut self.state, &mut self.drawing)
        {
            pass.upload(gpu, context);
        }
    }

    fn render_offscreen(&mut self, gpu: &mut dyn Gpu, parameters: &mut PaintParameters<'_>) {
        if self.renderable
            && let Some(pass) = T::offscreen_pass(&mut self.state, &mut self.drawing)
        {
            pass.render(gpu, parameters);
        }
    }

    fn leave(&mut self, ended: &mut EndedHosts) {
        if let Some(host) = T::host(&mut self.drawing) {
            ended.extend(host.leave());
        }
    }

    fn lose_context(&mut self) {
        if let Some(host) = T::host(&mut self.drawing) {
            host.lose_context();
        }
    }

    fn places_symbols(&self) -> bool {
        T::PLACES_SYMBOLS
    }

    fn placement_data(&self) -> &[BucketPlacementData] {
        T::placement_data(&self.state)
    }

    fn follower_data(&self) -> &[BucketPlacementData] {
        T::follower_data(&self.state)
    }

    fn query_intersects_feature(&self, test: &HitTest<'_>) -> bool {
        T::feature_query().is_some_and(|query| query.intersects(&self.values.evaluated, test))
    }
}
