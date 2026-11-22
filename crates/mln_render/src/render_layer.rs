// A render layer is the renderer's side of a style layer: a `Layer<T>` of
// its type `T`, which holds the layer's values, its values of the frame's
// tiles and what else the engine keeps of it, beside what its type keeps
// (`LayerType::State` and `LayerType::Drawing`). The engine does what is
// the same for every type around the type's hooks, and holds the layers as
// `dyn RenderLayer`. The orchestrator owns the render layers, in the
// style's order.

use std::any::Any;
use std::ops::Range;
use std::sync::Arc;

use mln_gfx::gpu::Gpu;
use mln_gfx::projection_variant::ProjectionVariant;
use mln_map_types::View;
use mln_style::expression::Dependency;
use mln_style::layer_type::{ErasedLayerStyle, LayerStyle, PaintSet, SourceKind};
use mln_style::{PropertyEvaluationParameters, TransitionParameters};
use mln_style_types::VisibilityType;
use mln_style_types::image::ImagePosition;
use mln_util::identity::SimpleIdentity;
use smallvec::SmallVec;

use crate::bucket::Bucket;
use crate::bucket_geometry::BucketGeometry;
use crate::erased_layer::{ErasedEvaluatedLayer, EvaluatedLayer};
use crate::geometry::feature_index::FeatureIndex;
use crate::gfx::context::Context;
use crate::layer::{DrawContext, EndedHost, EvaluateCx, Evaluation, HitTest, LayerType, LayerValues, UpdateContext};
use crate::layer_drawing::{DrawGroup, DrawOrder, LayerDrawing};
use crate::paint_parameters::PaintParameters;
use crate::placement::PlacingLayer;
use crate::render_source::{RenderSource, TileOrder};
use crate::render_tile::{FindView, FoundTile, FrameTile, RenderTile, TileContent, ViewPlace};
use crate::tile::contents::GeometryContents;
use crate::tile_render_data::TileAtlasTextures;
use crate::tile_values::{ChangedBuckets, TileMap};
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
    /// The bucket.
    pub bucket: &'a T::Bucket,
    /// The index of the bucket among the layout's buckets.
    pub(crate) bucket_index: usize,
    /// The values of the layer that the bucket was laid out with.
    pub layer: &'a EvaluatedLayer<T>,
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
            bucket: crate::erased_layer::recover_bucket::<T>(bucket)?,
            bucket_index,
            layer: layer.downcast_ref()?,
            contents,
        })
    }

    /// The geometry of the layout's buckets in the context's stores, which
    /// the draws bind.
    #[must_use]
    pub fn geometry(self) -> &'a BucketGeometry {
        &self.contents.bucket_geometry
    }

    /// The feature index of the layout.
    #[must_use]
    pub fn feature_index(self) -> &'a Arc<FeatureIndex> {
        &self.contents.result.feature_index
    }

    /// Where the pattern `pattern` is in the tile's icon atlas.
    #[must_use]
    pub fn pattern(self, pattern: &str) -> Option<ImagePosition> {
        self.contents.pattern_positions()?.get(pattern).cloned()
    }

    /// The textures of the tile's glyph and icon atlases.
    #[must_use]
    pub fn atlas_textures(self) -> &'a TileAtlasTextures {
        &self.contents.textures
    }
}

impl<'a, T: LayerType> FindView<'a> for LayerBucket<'a, T> {
    const SOURCE: SourceKind = SourceKind::Geometry;

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

    fn draws(self) -> bool {
        self.layer.draws
    }

    fn bucket_index(self) -> Option<usize> {
        Some(self.bucket_index)
    }

    fn data(self) -> Option<SimpleIdentity> {
        (self.layer.draws && self.bucket.has_data()).then(|| self.bucket.base().id())
    }
}

/// A source's render tiles in a frame, with what each of them holds, as
/// the engine sees them.
#[derive(Clone, Copy)]
pub(crate) struct FrameSourceTiles<'a> {
    tiles: &'a [RenderTile],
    /// What each of `tiles` holds, at its index.
    contents: &'a [Option<TileContent<'a>>],
    /// Whether the source is an image source.
    image: bool,
}

/// The tiles of a layer of the type `T` in a frame, each with what it shows
/// the layer, which keeps up to 64 of them on the stack.
pub(crate) type FrameTiles<'a, T> = SmallVec<[FrameTile<'a, T>; 64]>;

impl<'a> FrameSourceTiles<'a> {
    /// The render tiles of `source`, which hold `contents`, at their
    /// indexes.
    #[must_use]
    pub(crate) fn new(source: &'a dyn RenderSource, contents: &'a [Option<TileContent<'a>>]) -> Self {
        FrameSourceTiles { tiles: source.render_tiles(), contents, image: source.is_image() }
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
        Some((FrameSourceTiles::new(source, self.contents), source.tile_order(order)?))
    }
}

/// A render layer of the layer type `T`: the layer's values, what the
/// engine keeps of it, and what its type keeps.
pub(crate) struct Layer<T: LayerType> {
    values: LayerValues<T>,
    state: T::State,
    drawing: T::Drawing,
    /// The layer's values of the frame's tiles: of its source's, or of
    /// those a layer without a source picked.
    tile_values: TileMap<T::Tile>,
    /// Whether the layer, which has no source, picked tiles in an update
    /// ([`UpdateContext::pick_tiles`]).
    picked_tiles: bool,
    /// What the last evaluation of the layer's values said.
    evaluation: Evaluation,
    /// Whether the layer draws: its evaluation says so, or since that
    /// evaluation one of its tiles had a bucket laid out with values that
    /// draw.
    draws: bool,
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
            tile_values: TileMap::default(),
            picked_tiles: false,
            evaluation: Evaluation::NONE,
            draws: false,
            renderable: false,
            projection_variant: ProjectionVariant::Mercator,
            found: 0..0,
        }
    }

    /// The layer as its type's placement pass sees it.
    pub(crate) fn placing(&self) -> PlacingLayer<'_, T> {
        PlacingLayer { layer: &self.values, state: &self.state }
    }

    /// Runs the type's [`LayerType::update`] with `tiles`, the tiles of the
    /// tile store's `source`, as a frame's update does, but without images
    /// or the source's max zoom: for the benchmarks, which make the
    /// tiles without a frame.
    #[cfg(feature = "bench")]
    pub(crate) fn update_tiles(
        &mut self,
        tiles: &[FrameTile<'_, T>],
        update_parameters: &UpdateParameters,
        source: crate::tile::job::SourceKey,
    ) {
        let mut cx = UpdateContext {
            tiles,
            layer: &self.values,
            view: &update_parameters.view,
            update_parameters,
            images: &crate::image_store::ImageStore::default(),
            source_tiles: Some(source),
            source_max_zoom: None,
            tile_values: &mut self.tile_values,
            picked_tiles: &mut self.picked_tiles,
        };
        T::update(&mut self.state, &mut self.drawing, &mut cx);
    }

    /// Starts the frame's draws of the layer, which has something to draw,
    /// and returns whether its evaluation draws. One that doesn't drops the
    /// frame's draws, and keeps what it keeps of its tiles.
    fn begin_frame(&mut self) -> bool {
        self.drawing.begin_frame();
        let draws = self.evaluation.draws;
        if !draws {
            self.drawing.clear_frame();
        }
        draws
    }

    /// Runs the type's [`LayerType::make_draws`] with `tiles`, those its
    /// update found, once it dropped the values of those that show the
    /// layer nothing to draw or other data than before.
    fn make_draws(&mut self, gpu: &mut dyn Gpu, paint: &mut PaintParameters<'_>, tiles: &[FrameTile<'_, T>]) {
        if !self.begin_frame() {
            return;
        }
        debug_assert!(tiles.is_empty() || tiles.len() == self.tile_values.len(), "tiles the update didn't find");
        for (index, tile) in tiles.iter().enumerate() {
            debug_assert_eq!(Some(tile.id()), self.tile_values.id(index), "a tile's value at another's index");
            self.tile_values.show(index, tile.view().and_then(FindView::data));
        }
        let mut cx = DrawContext {
            tiles,
            layer: &self.values,
            projection_variant: self.projection_variant,
            context: &mut *paint.context,
            frame: paint.frame,
        };
        T::make_draws(&mut self.state, &mut self.drawing, &mut self.tile_values, gpu, &mut cx);
    }
}

/// What the engine reads of a render layer to run a frame and answer
/// queries, kept as plain data beside it ([`RecordedLayer`]), so that
/// reading a fact takes no dyn call.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct LayerRecord {
    /// The kind of source the layer's type draws, `None` for a type without
    /// one.
    pub(crate) input: SourceKind,
    /// The id of the layer's source.
    pub(crate) source: String,
    /// Whether the layer's visibility isn't `none`.
    pub(crate) visible: bool,
    /// The layer's zoom range, from its minimum zoom to before its maximum.
    pub(crate) min_zoom: f32,
    pub(crate) max_zoom: f32,
    /// What the last evaluation of the layer's values said.
    pub(crate) evaluation: Evaluation,
    /// Whether the layer draws: its evaluation says so, or since that
    /// evaluation one of its tiles had a bucket laid out with values that
    /// draw.
    pub(crate) draws: bool,
    /// See [`LayerType::VOLUME`].
    pub(crate) volume: bool,
    /// Whether the layer's type places its layers ([`LayerType::PLACEMENT`]).
    pub(crate) places: bool,
    /// The order the layer draws its source's tiles in.
    pub(crate) tile_order: TileOrder,
}

impl LayerRecord {
    /// Whether the layer draws at `zoom`. A minimum zoom of 0 is no
    /// minimum: the globe zooms below 0 toward the poles.
    pub(crate) fn supports_zoom(&self, zoom: f32) -> bool {
        let zoom_or_zero = if zoom < 0.0 { 0.0 } else { zoom };
        self.min_zoom <= zoom_or_zero && zoom < self.max_zoom
    }

    /// Whether the layer draws and is visible.
    pub(crate) fn needs_rendering(&self) -> bool {
        self.draws && self.visible
    }
}

/// A render layer with its [`LayerRecord`]. Its own
/// [`restyle`](Self::restyle), [`evaluate`](Self::evaluate) and
/// [`update`](Self::update), the layer's calls that change what the record
/// holds, take the place of the layer's and keep the record up to date. It
/// derefs to the layer for the rest of what the engine does with it.
pub(crate) struct RecordedLayer {
    record: LayerRecord,
    layer: Box<dyn RenderLayer>,
}

impl RecordedLayer {
    pub(crate) fn new(layer: Box<dyn RenderLayer>) -> RecordedLayer {
        RecordedLayer { record: layer.record(), layer }
    }

    /// What the engine reads of the layer.
    pub(crate) fn record(&self) -> &LayerRecord {
        &self.record
    }

    /// See [`RenderLayer::restyle`].
    pub(crate) fn restyle(
        &mut self,
        parameters: &TransitionParameters,
        style: &Arc<dyn ErasedLayerStyle>,
        ended: &mut EndedHosts,
    ) {
        self.layer.restyle(parameters, style, ended);
        self.record = self.layer.record();
    }

    /// See [`RenderLayer::evaluate`].
    pub(crate) fn evaluate(&mut self, parameters: &PropertyEvaluationParameters) {
        let evaluation = self.layer.evaluate(parameters);
        self.record.evaluation = evaluation;
        self.record.draws = evaluation.draws;
    }

    /// See [`RenderLayer::update`].
    pub(crate) fn update(&mut self, update: &mut LayerUpdate<'_>) {
        self.record.draws = self.layer.update(update);
        debug_assert_eq!(self.record, self.layer.record(), "a layer's record out of step with the layer");
    }
}

impl std::ops::Deref for RecordedLayer {
    type Target = dyn RenderLayer;

    fn deref(&self) -> &(dyn RenderLayer + 'static) {
        &*self.layer
    }
}

impl std::ops::DerefMut for RecordedLayer {
    fn deref_mut(&mut self) -> &mut (dyn RenderLayer + 'static) {
        &mut *self.layer
    }
}

/// What the engine does with a render layer, whatever its type: its values,
/// which this transitions without knowing their type, and its drawing,
/// which keeps what the layer keeps of its tiles and uploads and draws its
/// draw groups. It runs the hooks of the layer's [`LayerType`] with what is
/// common to every layer type around them.
pub(crate) trait RenderLayer: Any {
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

    /// The facts about the layer that the engine keeps beside it.
    fn record(&self) -> LayerRecord;

    /// Makes `style`, the changed style of the layer (of the same id and
    /// type), the layer's, drops the draws, which were built for the
    /// previous style's values, and begins transitions to the new values.
    /// A style of another type leaves the layer's. The next encode builds
    /// the draws again from the tiles' buckets. A host the layer draws
    /// through ([`LayerHost`](crate::layer::LayerHost)) that drew and that the style replaced goes to `ended`.
    fn restyle(&mut self, parameters: &TransitionParameters, style: &Arc<dyn ErasedLayerStyle>, ended: &mut EndedHosts);

    /// Begins transitions for the paint values that changed since the last
    /// frame, to those of the layer's style.
    fn transition(&mut self, parameters: &TransitionParameters);

    /// Evaluates the layer's paint values for a frame of `parameters`, and
    /// keeps and returns what the layer says of them
    /// ([`LayerType::evaluate`]).
    fn evaluate(&mut self, parameters: &PropertyEvaluationParameters) -> Evaluation;

    /// Whether a paint value of the layer is still in transition.
    fn has_transition(&self) -> bool;

    /// Whether the layer has a pattern property and is in a cross-fade.
    fn has_crossfade(&self) -> bool;

    /// The frame's update of the layer: finds the tiles of `update`'s source
    /// the layer draws, which it adds to `update.found`, draws where one of
    /// their buckets was laid out with values that draw, moves the layer's
    /// values of its tiles to them, and runs the type's
    /// [`LayerType::update`]. Returns whether the layer draws.
    fn update(&mut self, update: &mut LayerUpdate<'_>) -> bool;

    /// The encode's update of the layer's draws: picks the shader variant
    /// for the frame's projection, dropping the draws when it changed; drops
    /// the draws of a layer that has nothing to draw (still starting the
    /// frame of one on an image source), and otherwise starts its draws of
    /// the frame ([`LayerDrawing::begin_frame`]); drops them again for a
    /// layer whose evaluation draws nothing, and otherwise drops the values
    /// of the tiles that show the layer nothing to draw or other data than
    /// they were made from, and runs the type's [`LayerType::make_draws`]
    /// with the tiles of `source` that its update found in `found`.
    fn update_draws(
        &mut self,
        gpu: &mut dyn Gpu,
        paint: &mut PaintParameters<'_>,
        source: Option<FrameSourceTiles<'_>>,
        found: &[FoundTile],
    );

    /// Records that the buckets in `changed` changed, for the tiles whose
    /// values were made from them.
    fn mark_changed(&mut self, changed: &ChangedBuckets);

    /// Moves the layer's value of each tile to the tile `wrap_delta` world
    /// copies east of it: the tiles are the same ones under new ids once the
    /// map's center has crossed the antimeridian, and their draws stay
    /// rather than being built again.
    fn handle_wrap_jump(&mut self, wrap_delta: i16);

    /// Includes the layer in the frame's drawing or excludes it. A layer
    /// excluded drops its draws, its tiles with their values and what its
    /// type keeps for them ([`LayerType::release`]): its updates don't run
    /// until it is included again, and they then rebuild everything from
    /// the source's tiles.
    fn set_renderable(&mut self, renderable: bool);

    /// Whether the frame draws the layer.
    fn is_renderable(&self) -> bool;

    /// What the layer's paint values depend on.
    fn style_dependencies(&self) -> Dependency;

    /// Drops the frame's draws of the layer, its values of its tiles and the
    /// other caches of their GPU objects. The tiles stay.
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

    fn record(&self) -> LayerRecord {
        let base = self.values.style.base();
        LayerRecord {
            input: T::INFO.source,
            source: base.source.clone(),
            visible: base.visibility != VisibilityType::None,
            min_zoom: base.min_zoom,
            max_zoom: base.max_zoom,
            evaluation: self.evaluation,
            draws: self.draws,
            volume: T::VOLUME,
            places: T::PLACEMENT.is_some(),
            tile_order: T::TILE_ORDER,
        }
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

    fn evaluate(&mut self, parameters: &PropertyEvaluationParameters) -> Evaluation {
        let mut evaluated = self.values.evaluate(parameters);
        let cx = EvaluateCx { layer: &self.values, parameters, evaluated: &mut evaluated };
        self.evaluation = T::evaluate(&mut self.state, cx);
        self.draws = self.evaluation.draws;
        evaluated.draws = self.evaluation.draws;
        self.values.evaluated = Arc::new(evaluated);
        self.evaluation
    }

    fn has_transition(&self) -> bool {
        T::PaintProperties::has_transition(&self.values.unevaluated)
    }

    fn has_crossfade(&self) -> bool {
        T::PaintProperties::CROSSFADES && self.values.evaluated.crossfade.t != 1.0
    }

    fn update(&mut self, update: &mut LayerUpdate<'_>) -> bool {
        let start = update.found.len();
        let tiles = match update.tiles(T::TILE_ORDER) {
            Some((tiles, order)) => tiles.find::<T>(order, &self.values.style.id, update.found),
            None => FrameTiles::new(),
        };
        self.found = start..update.found.len();
        self.draws |= tiles.iter().filter_map(|tile| tile.view()).any(FindView::draws);
        if T::INFO.source != SourceKind::None {
            self.tile_values.set_tiles(tiles.iter().map(|tile| tile.id()));
        }
        let mut cx = UpdateContext {
            tiles: &tiles,
            layer: &self.values,
            view: update.view,
            update_parameters: update.update_parameters,
            images: update.images,
            source_tiles: update.source.and_then(RenderSource::tiles),
            source_max_zoom: update.source.and_then(RenderSource::max_zoom),
            tile_values: &mut self.tile_values,
            picked_tiles: &mut self.picked_tiles,
        };
        T::update(&mut self.state, &mut self.drawing, &mut cx);
        self.draws
    }

    fn update_draws(
        &mut self,
        gpu: &mut dyn Gpu,
        paint: &mut PaintParameters<'_>,
        source: Option<FrameSourceTiles<'_>>,
        found: &[FoundTile],
    ) {
        let variant = if paint.frame.view.is_globe() { ProjectionVariant::Globe } else { ProjectionVariant::Mercator };
        if variant != self.projection_variant {
            self.projection_variant = variant;
            self.remove_all_draws();
            T::projection_changed(&mut self.state, variant);
        }
        if T::INFO.source == SourceKind::None {
            // A layer that picks tiles draws those its update picked.
            if self.picked_tiles && self.tile_values.is_empty() {
                self.remove_all_draws();
            } else {
                self.make_draws(gpu, paint, &[]);
            }
            return;
        }
        match source {
            Some(source) if let Some(found) = found.get(self.found.clone()).filter(|found| !found.is_empty()) => {
                let tiles = source.at::<T>(found, &self.values.style.id);
                self.make_draws(gpu, paint, &tiles);
            }
            // A layer of an image source takes its place in the layers'
            // order and depth from its first frame, whether or not the image
            // has tiles to draw in yet.
            Some(source) if source.image => {
                self.remove_all_draws();
                self.begin_frame();
            }
            _ => self.remove_all_draws(),
        }
    }

    fn mark_changed(&mut self, changed: &ChangedBuckets) {
        self.tile_values.mark_changed(changed);
    }

    fn handle_wrap_jump(&mut self, wrap_delta: i16) {
        self.tile_values.shift_wraps(wrap_delta);
    }

    fn set_renderable(&mut self, renderable: bool) {
        self.renderable = renderable;
        if !renderable {
            self.remove_all_draws();
            self.tile_values.clear_tiles();
            T::release(&mut self.state);
        }
    }

    fn is_renderable(&self) -> bool {
        self.renderable
    }

    fn style_dependencies(&self) -> Dependency {
        self.style_dependencies
    }

    fn remove_all_draws(&mut self) {
        self.tile_values.clear();
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

    fn query_intersects_feature(&self, test: &HitTest<'_>) -> bool {
        T::feature_query().is_some_and(|query| query.intersects(&self.values.evaluated, test))
    }
}
