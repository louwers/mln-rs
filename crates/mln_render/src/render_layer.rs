// A render layer is the renderer's side of a style layer: a `Hosted<L>` of
// its layer type `L`, which holds the layer, its values, its values of the
// frame's tiles and its draws. The engine runs the layer's hooks with what
// is the same for every type around them, and holds the layers as
// `dyn RenderLayer`. The orchestrator owns the render layers, in the
// style's order.

use std::any::Any;
use std::ops::Range;
use std::sync::Arc;

use mln_gfx::gpu::Gpu;
use mln_gfx::projection_variant::ProjectionVariant;
use mln_map_types::View;
use mln_map_types::tile_cover::{TileCoverParameters, tile_cover};
use mln_style::expression::Dependency;
use mln_style::layer_type::{ErasedLayerStyle, LayerStyle, PaintSet, SourceKind, StyleLayerType};
use mln_style::{PropertyEvaluationParameters, TransitionParameters};
use mln_style_types::VisibilityType;
use mln_tile_id::OverscaledTileID;
use mln_util::geo::ScreenCoordinate;
use mln_util::range::Range as ZoomRange;
use smallvec::SmallVec;

use crate::bucket::ErasedBucket;
use crate::encode::{DrawOrder, Draws, FrameDraws};
use crate::erased_layer::{ErasedEvaluatedLayer, EvaluatedLayer, entry};
use crate::gfx::context::Resources;
use crate::image_store::ImageStore;
use crate::layer::{
    EvaluateCx, Evaluation, Found, FrameQuery, FrameTile, Hit, HitTest, Layer, LayerValues, NewCx, PrepareCx, TileCx, Tiles, UpdateCx,
    input,
};
use crate::paint_parameters::PaintParameters;
use crate::render_source::{RenderSource, TileOrder};
use crate::render_tile::{FoundTile, RenderTile, TileContent};
use crate::tile::contents::GeometryContents;
use crate::tile_values::TileMap;
use crate::update_parameters::UpdateParameters;

/// A layer's bucket in a layout, which the layers of its layout group
/// share, with the layer's values it was laid out with, whichever the
/// layer's type.
#[derive(Clone, Copy)]
pub(crate) struct ErasedLayerBucket<'a> {
    pub(crate) bucket: &'a dyn ErasedBucket,
    /// The values of the layer that the bucket was laid out with.
    pub(crate) layer: &'a dyn ErasedEvaluatedLayer,
}

/// A source's render tiles in a frame, with what each of them holds, as
/// the engine sees them.
#[derive(Clone, Copy)]
pub(crate) struct FrameSourceTiles<'a> {
    tiles: &'a [RenderTile],
    /// What each of `tiles` holds, at its index.
    contents: &'a [Option<TileContent<'a>>],
}

impl<'a> FrameSourceTiles<'a> {
    /// The render tiles of `source`, which hold `contents`, at their
    /// indexes.
    #[must_use]
    pub(crate) fn new(source: &'a dyn RenderSource, contents: &'a [Option<TileContent<'a>>]) -> Self {
        FrameSourceTiles { tiles: source.render_tiles(), contents }
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
    /// The tiles of the frame's layers, as their updates find them, which
    /// the update adds the layer's to: those the encode draws.
    pub(crate) found: &'a mut Vec<FoundTile>,
}

/// A render layer of the layer type `L`: the layer, its values, and what
/// the engine keeps of it.
pub(crate) struct Hosted<L: Layer> {
    values: LayerValues<L::Style>,
    layer: L,
    /// The layer's values of the frame's tiles.
    tile_values: TileMap<L::Tile>,
    /// The tiles that cover the view, for a layer of [`ViewCover`](crate::layer::ViewCover).
    cover: Vec<OverscaledTileID>,
    style_dependencies: Dependency,
    /// Whether the frame draws the layer.
    renderable: bool,
    /// The projection the layer's tile values are made for.
    projection_variant: ProjectionVariant,
    /// Where the tiles the layer's update found are among the frame's
    /// ([`LayerUpdate::found`]).
    found: Range<usize>,
    /// The deepest zoom with tiles of their own in the layer's source.
    source_max_zoom: Option<u8>,
    /// The layer's draws of the frame.
    frame_draws: Draws,
}

impl<L: Layer> Hosted<L> {
    /// The layer as its type's placement pass sees it.
    pub(crate) fn placing(&self) -> crate::placement::PlacingLayer<'_, L> {
        crate::placement::PlacingLayer { layer: &self.values, state: &self.layer }
    }

    /// The layer and its values.
    pub(crate) fn parts(&self) -> (&L, &LayerValues<L::Style>) {
        (&self.layer, &self.values)
    }

    /// The render layer of the layer `style`, before its values transition,
    /// which joins the renderer of `res`.
    pub(crate) fn new(style: Arc<LayerStyle<L::Style>>, res: &mut Resources) -> Self {
        let unevaluated = style.paint.untransitioned();
        let evaluated = Arc::new(EvaluatedLayer::with_paint(entry::<L>(), Arc::clone(&style), Default::default()));
        let values = LayerValues { style, unevaluated, evaluated };
        let layer = L::new(&mut NewCx { layer: &values, res });
        Hosted {
            style_dependencies: <L::Style as StyleLayerType>::PaintProperties::dependencies(&values.unevaluated),
            values,
            layer,
            tile_values: TileMap::default(),
            cover: Vec::new(),
            renderable: false,
            projection_variant: ProjectionVariant::Mercator,
            found: 0..0,
            source_max_zoom: None,
            frame_draws: Draws::new(),
        }
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
    /// See [`Layer::VOLUME`].
    pub(crate) volume: bool,
    /// Whether the layer's type places its layers.
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

/// A render layer with its [`LayerRecord`], which alone holds what the
/// layer's evaluation says and whether it draws. Its own
/// [`restyle`](Self::restyle), [`evaluate`](Self::evaluate),
/// [`update`](Self::update) and [`update_draws`](Self::update_draws) take
/// the place of the layer's, and keep and read the record. It derefs to the
/// layer for the rest of what the engine does with it.
pub(crate) struct RecordedLayer {
    record: LayerRecord,
    layer: Box<dyn RenderLayer>,
}

impl RecordedLayer {
    pub(crate) fn new(layer: Box<dyn RenderLayer>) -> RecordedLayer {
        RecordedLayer { record: layer.record(Evaluation::NONE, false), layer }
    }

    /// What the engine reads of the layer.
    pub(crate) fn record(&self) -> &LayerRecord {
        &self.record
    }

    /// See [`RenderLayer::restyle`].
    pub(crate) fn restyle(&mut self, parameters: &TransitionParameters, style: &Arc<dyn ErasedLayerStyle>) {
        self.layer.restyle(parameters, style);
        self.record = self.layer.record(self.record.evaluation, self.record.draws);
    }

    /// See [`RenderLayer::evaluate`].
    pub(crate) fn evaluate(&mut self, parameters: &PropertyEvaluationParameters) {
        let evaluation = self.layer.evaluate(parameters);
        self.record.evaluation = evaluation;
        self.record.draws = evaluation.draws;
    }

    /// See [`RenderLayer::update`]. The layer draws from then on, until
    /// the next evaluation, where a tile has something for it to draw.
    pub(crate) fn update(&mut self, update: &mut LayerUpdate<'_>) {
        self.record.draws |= self.layer.update(update);
    }

    /// See [`RenderLayer::update_draws`].
    pub(crate) fn update_draws(
        &mut self,
        gpu: &mut dyn Gpu,
        paint: &mut PaintParameters<'_>,
        images: &ImageStore,
        source: Option<FrameSourceTiles<'_>>,
        found: &[FoundTile],
    ) {
        self.layer.update_draws(gpu, paint, self.record.evaluation, images, source, found);
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
/// which this transitions without knowing their type, and its frames, in
/// which it runs the layer's hooks and draws what they push.
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

    /// The facts about the layer that the engine keeps beside it, with
    /// `evaluation` and `draws` as the record holds them.
    fn record(&self, evaluation: Evaluation, draws: bool) -> LayerRecord;

    /// Makes `style`, the changed style of the layer (of the same id and
    /// type), the layer's, drops its tile values and draws, which were made
    /// for the previous style's values, and begins transitions to the new
    /// values. A style of another type leaves the layer's.
    fn restyle(&mut self, parameters: &TransitionParameters, style: &Arc<dyn ErasedLayerStyle>);

    /// Makes `style`, the layer's style with other
    /// [`Data`](StyleLayerType::Data) alone, the layer's. Its tile values
    /// and transitions stay.
    fn set_data(&mut self, style: &Arc<dyn ErasedLayerStyle>);

    /// Begins transitions for the paint values that changed since the last
    /// frame, to those of the layer's style.
    fn transition(&mut self, parameters: &TransitionParameters);

    /// Evaluates the layer's paint values for a frame of `parameters`, and
    /// returns what the layer says of them ([`Layer::evaluate`]).
    fn evaluate(&mut self, parameters: &PropertyEvaluationParameters) -> Evaluation;

    /// Whether a paint value of the layer is still in transition.
    fn has_transition(&self) -> bool;

    /// Whether the layer has a pattern property and is in a cross-fade.
    fn has_crossfade(&self) -> bool;

    /// The frame's update of the layer: finds the tiles of `update`'s source
    /// the layer draws, which it adds to `update.found`, or the tiles that
    /// cover the view for a layer of no source. Returns whether one of
    /// their buckets was laid out with values that draw.
    fn update(&mut self, update: &mut LayerUpdate<'_>) -> bool;

    /// The encode's run of the layer: drops its tile values where the
    /// frame's projection changed, and where `evaluation` draws, gives the
    /// tiles of `source` that its update found in `found` their data, runs
    /// [`Layer::update`], [`Layer::tile`] for the tiles without a value and
    /// [`Layer::prepare`], and readies the draws it pushed.
    fn update_draws(
        &mut self,
        gpu: &mut dyn Gpu,
        paint: &mut PaintParameters<'_>,
        evaluation: Evaluation,
        images: &ImageStore,
        source: Option<FrameSourceTiles<'_>>,
        found: &[FoundTile],
    );

    /// Moves the layer's value of each tile to the tile `wrap_delta` world
    /// copies east of it: the tiles are the same ones under new ids once the
    /// map's center has crossed the antimeridian, and their values stay.
    fn handle_wrap_jump(&mut self, wrap_delta: i16);

    /// Includes the layer in the frame's drawing or excludes it. A layer
    /// excluded drops its draws and its tiles with their values, and hears
    /// that it stopped ([`Layer::stopped`]).
    fn set_renderable(&mut self, renderable: bool);

    /// Whether the frame draws the layer.
    fn is_renderable(&self) -> bool;

    /// What the layer's paint values depend on.
    fn style_dependencies(&self) -> Dependency;

    /// Drops the frame's draws of the layer and its values of its tiles.
    /// The tiles stay.
    fn remove_all_draws(&mut self);

    /// Visits the layer's draw groups of the frame, in drawing order: none
    /// unless the frame draws the layer.
    fn visit_draw_groups<'a>(&'a self, visit: &mut dyn FnMut(&'a FrameDraws));

    /// Uploads the meshes of the layer's draw groups, before the frame draws
    /// them.
    fn upload(&mut self, gpu: &mut dyn Gpu, context: &mut Resources);

    /// Draws the pass's draws of the layer's draw groups into the open pass
    /// of `gpu`, the context's, in `order`, each at its depth.
    fn render(&mut self, gpu: &mut dyn Gpu, parameters: &mut PaintParameters<'_>, order: DrawOrder);

    /// The query radius of the layer's features in `contents`, a tile's
    /// layout ([`Layer::query_radius`]).
    fn query_radius(&self, contents: &GeometryContents) -> f32;

    /// Whether `test`'s feature, as the layer draws it, is hit by the
    /// query's geometry ([`Layer::hit`]).
    fn hit(&self, test: &ErasedHitTest<'_>) -> Option<f32>;

    /// Adds to `hits` what `geometry`, in `camera`'s pixels, hits of what
    /// the layer draws without tile features ([`Layer::hit_frame`]).
    fn hit_frame(&self, camera: &View, geometry: &[ScreenCoordinate], hits: &mut Vec<Hit>);
}

/// A [`HitTest`] of a layer of any type.
pub(crate) struct ErasedHitTest<'a> {
    pub(crate) query_geometry: &'a mln_tile_id::geometry_tile_data::GeometryCoordinates,
    pub(crate) feature: &'a dyn mln_tile_id::geometry_tile_data::GeometryTileFeature,
    pub(crate) geometry: &'a mln_tile_id::geometry_tile_data::GeometryCollection,
    pub(crate) feature_state: &'a mln_style::expression::expr::FeatureState,
    pub(crate) zoom: f32,
    pub(crate) camera: &'a View,
    pub(crate) pixels_to_tile_units: f32,
    pub(crate) projector: &'a mln_map_types::TileProjector<'a>,
}

impl<L: Layer> RenderLayer for Hosted<L> {
    fn style(&self) -> &dyn ErasedLayerStyle {
        &*self.values.style
    }

    fn shared_style(&self) -> Arc<dyn ErasedLayerStyle> {
        Arc::<LayerStyle<L::Style>>::clone(&self.values.style)
    }

    fn evaluated(&self) -> Arc<dyn ErasedEvaluatedLayer> {
        Arc::<EvaluatedLayer<L::Style>>::clone(&self.values.evaluated)
    }

    fn record(&self, evaluation: Evaluation, draws: bool) -> LayerRecord {
        let base = self.values.style.base();
        LayerRecord {
            input: crate::layer::input_source::<L>(),
            source: base.source.clone(),
            visible: base.visibility != VisibilityType::None,
            min_zoom: base.min_zoom,
            max_zoom: base.max_zoom,
            evaluation,
            draws,
            volume: L::VOLUME,
            places: false,
            tile_order: TileOrder::ById,
        }
    }

    fn restyle(&mut self, parameters: &TransitionParameters, style: &Arc<dyn ErasedLayerStyle>) {
        self.remove_all_draws();
        if let Some(recovered) = LayerStyle::recover(style) {
            self.values.style = recovered;
        }
        self.transition(parameters);
    }

    fn set_data(&mut self, style: &Arc<dyn ErasedLayerStyle>) {
        if let Some(recovered) = LayerStyle::recover(style) {
            self.values.style = recovered;
        }
    }

    fn transition(&mut self, parameters: &TransitionParameters) {
        let values = &mut self.values;
        let prior = std::mem::take(&mut values.unevaluated);
        values.unevaluated = values.style.paint.transitioned(parameters, prior);
        self.style_dependencies = <L::Style as StyleLayerType>::PaintProperties::dependencies(&values.unevaluated);
    }

    fn evaluate(&mut self, parameters: &PropertyEvaluationParameters) -> Evaluation {
        type Paint<L> = <<L as Layer>::Style as StyleLayerType>::PaintProperties;
        let values = &mut self.values;
        let paint = Paint::<L>::evaluate_with_previous(&mut values.unevaluated, parameters, &values.evaluated.paint);
        let mut evaluated = EvaluatedLayer::with_paint(entry::<L>(), Arc::clone(&values.style), paint);
        if Paint::<L>::CROSSFADES {
            evaluated.crossfade = parameters.get_crossfade_parameters();
        }
        let evaluation = self.layer.evaluate(EvaluateCx { layer: values, parameters, evaluated: &mut evaluated });
        evaluated.draws = evaluation.draws;
        values.evaluated = Arc::new(evaluated);
        evaluation
    }

    fn has_transition(&self) -> bool {
        <L::Style as StyleLayerType>::PaintProperties::has_transition(&self.values.unevaluated)
    }

    fn has_crossfade(&self) -> bool {
        <L::Style as StyleLayerType>::PaintProperties::CROSSFADES && self.values.evaluated.crossfade.t != 1.0
    }

    fn update(&mut self, update: &mut LayerUpdate<'_>) -> bool {
        let start = update.found.len();
        let mut draws = false;
        match crate::layer::input_source::<L>() {
            SourceKind::None => {
                let view = update.view;
                let parameters = update.update_parameters;
                let zoom = view.integer_zoom();
                self.cover = tile_cover(
                    &TileCoverParameters {
                        view: view.clone(),
                        tile_lod_min_radius: parameters.tile_lod_min_radius,
                        tile_lod_scale: parameters.tile_lod_scale,
                        tile_lod_pitch_threshold: parameters.tile_lod_pitch_threshold,
                        tile_lod_mode: parameters.tile_lod_mode,
                        requested_zoom: None,
                        round_zoom: false,
                        max_content_elevation: 0.0,
                    },
                    zoom,
                    ZoomRange::new(0u8, zoom),
                    None,
                );
                self.tile_values.set_tiles(self.cover.iter().copied());
            }
            _ => {
                let mut ids: SmallVec<[OverscaledTileID; 64]> = SmallVec::new();
                if let Some(source) = update.source
                    && let Some(order) = source.tile_order(TileOrder::ById)
                {
                    let tiles = source.render_tiles();
                    for &index in order {
                        let Some(tile) = tiles.get(index) else { continue };
                        let content = update.contents.get(index).copied().flatten();
                        let found = content.and_then(|content| {
                            <L::Input as input::Find<L::Style>>::find(content, &self.values.style.id)
                        });
                        draws |= found
                            .is_some_and(|(data, _)| <L::Input as input::Find<L::Style>>::identity(data).is_some());
                        update.found.push(FoundTile { tile: index, view: found.map(|(_, place)| place) });
                        ids.push(*tile.overscaled_tile_id());
                    }
                }
                self.tile_values.set_tiles(ids);
                self.source_max_zoom = update.source.and_then(RenderSource::max_zoom);
            }
        }
        self.found = start..update.found.len();
        draws
    }

    fn update_draws(
        &mut self,
        gpu: &mut dyn Gpu,
        paint: &mut PaintParameters<'_>,
        evaluation: Evaluation,
        images: &ImageStore,
        source: Option<FrameSourceTiles<'_>>,
        found: &[FoundTile],
    ) {
        let variant =
            if paint.frame.camera.is_globe() { ProjectionVariant::Globe } else { ProjectionVariant::Mercator };
        if variant != self.projection_variant {
            self.projection_variant = variant;
            self.remove_all_draws();
        }
        paint.context.set_projection_variant(variant);
        self.frame_draws.begin(evaluation.opaque, paint.frame.composites_with_frame_depth);
        if !evaluation.draws {
            self.frame_draws.clear();
            return;
        }

        let mut tiles: SmallVec<[Found<'_>; 64]> = SmallVec::new();
        match crate::layer::input_source::<L>() {
            SourceKind::None => {
                for (index, &id) in self.cover.iter().enumerate() {
                    tiles.push(Found { tile: FrameTile { index, id, unwrapped: id.to_unwrapped() }, at: None });
                }
            }
            _ => {
                let found = found.get(self.found.clone()).unwrap_or_default();
                for (index, &FoundTile { tile, view }) in found.iter().enumerate() {
                    let Some(source) = source else { break };
                    let Some(render_tile) = source.tiles.get(tile) else { continue };
                    let content = source.contents.get(tile).copied().flatten();
                    let at = content.zip(view);
                    let held = at.and_then(|(content, place)| <L::Input as input::Find<L::Style>>::at(content, place));
                    self.tile_values.show(index, held.and_then(<L::Input as input::Find<L::Style>>::identity));
                    let id = *render_tile.overscaled_tile_id();
                    tiles.push(Found { tile: FrameTile { index, id, unwrapped: render_tile.id }, at });
                }
            }
        }

        let frame = paint.frame;
        let res = &mut *paint.context;
        let Hosted { values, layer, tile_values, frame_draws, .. } = self;
        layer.update(&mut UpdateCx {
            layer: values,
            frame: &frame,
            images,
            tiles: Tiles { found: &tiles, values: tile_values },
            res,
        });
        for found in &tiles {
            let Some(held) = found.data::<L>() else { continue };
            let index = found.tile.index;
            if tile_values.get(index).is_none() {
                let cx = &mut TileCx { layer: values, frame: &frame, res };
                *tile_values.get_mut(index) = layer.tile(cx, found.tile, held);
            }
        }
        layer
            .prepare(&PrepareCx { layer: values, frame: &frame, res, found: &tiles, values: tile_values }, frame_draws);
        frame_draws.finish(gpu, res, tiles.iter().map(|found| found.tile.unwrapped));
    }

    fn handle_wrap_jump(&mut self, wrap_delta: i16) {
        self.tile_values.shift_wraps(wrap_delta);
    }

    fn set_renderable(&mut self, renderable: bool) {
        self.renderable = renderable;
        if !renderable {
            self.remove_all_draws();
            self.tile_values.clear_tiles();
            self.layer.stopped();
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
        self.frame_draws.clear();
    }

    fn visit_draw_groups<'a>(&'a self, visit: &mut dyn FnMut(&'a FrameDraws)) {
        if self.renderable {
            for group in self.frame_draws.groups() {
                visit(&group.frame);
            }
        }
    }

    fn upload(&mut self, gpu: &mut dyn Gpu, context: &mut Resources) {
        if self.renderable {
            for group in self.frame_draws.groups() {
                group.upload(gpu, context);
            }
        }
    }

    fn render(&mut self, gpu: &mut dyn Gpu, parameters: &mut PaintParameters<'_>, order: DrawOrder) {
        if self.renderable {
            for group in self.frame_draws.groups() {
                group.render(gpu, parameters);
                order.step(parameters);
            }
        }
    }

    fn query_radius(&self, contents: &GeometryContents) -> f32 {
        let laid = <L::Input as input::Find<L::Style>>::laid(contents, &self.values.style.id);
        laid.map_or(0.0, |data| self.layer.query_radius(&self.values, data))
    }

    fn hit(&self, test: &ErasedHitTest<'_>) -> Option<f32> {
        self.layer.hit(&HitTest {
            layer: &self.values,
            query_geometry: test.query_geometry,
            feature: test.feature,
            geometry: test.geometry,
            feature_state: test.feature_state,
            zoom: test.zoom,
            camera: test.camera,
            pixels_to_tile_units: test.pixels_to_tile_units,
            projector: test.projector,
        })
    }

    fn hit_frame(&self, camera: &View, geometry: &[ScreenCoordinate], hits: &mut Vec<Hit>) {
        if self.renderable {
            self.layer.hit_frame(&FrameQuery { layer: &self.values, camera, geometry }, hits);
        }
    }
}
