// A render layer is the renderer's side of a style layer: a `Hosted<L>` of
// its layer type `L`, which holds the layer, its values, its values of the
// frame's tiles and its draws. The engine runs the layer's hooks with what
// is the same for every type around them, and holds the layers as
// `dyn RenderLayer`. The orchestrator owns the render layers, in the
// style's order.

use std::any::Any;
use std::sync::Arc;

use mln_gfx::gpu::Gpu;
use mln_map_types::View;
use mln_style::expression::Dependency;
use mln_style::layer_type::{ErasedLayerStyle, LayerStyle, PaintSet, SourceKind, StyleLayerType};
use mln_style::{PropertyEvaluationParameters, Style, TransitionParameters};
use mln_style_types::VisibilityType;
use mln_util::geo::ScreenCoordinate;
use smallvec::SmallVec;

use crate::bucket::ErasedBucket;
use crate::encode::{DrawOrder, Draws, FrameDraws};
use crate::erased_layer::{ErasedEvaluatedLayer, ErasedLayerType, EvaluatedLayer};
use crate::gfx::context::Resources;
use crate::image_store::ImageStore;
use crate::layer::{
    EvaluateCx, Evaluation, Found, FrameQuery, Hit, HitTest, Layer, LayerValues, LendAt, Lent, NewCx, PrepareCx,
    TileCx, Tiles, UpdateCx, input,
};
use crate::named_target::Routes;
use crate::paint_parameters::PaintParameters;
use crate::render_pass::Pass;
use crate::render_source::RenderSource;
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
    pub(crate) tiles: &'a [RenderTile],
    /// What each of `tiles` holds, at its index.
    pub(crate) contents: &'a [Option<TileContent<'a>>],
    /// See [`RenderSource::max_zoom`].
    pub(crate) max_zoom: Option<u8>,
}

impl<'a> FrameSourceTiles<'a> {
    /// The render tiles of `source`, which hold `contents`, at their
    /// indexes.
    #[must_use]
    pub(crate) fn new(source: &'a RenderSource, contents: &'a [Option<TileContent<'a>>]) -> Self {
        FrameSourceTiles { tiles: source.render_tiles(), contents, max_zoom: source.max_zoom() }
    }
}

/// What the engine reads of the frame in a layer's update, besides the
/// layer.
pub(crate) struct LayerUpdate<'a> {
    pub(crate) view: &'a View,
    pub(crate) update_parameters: &'a UpdateParameters,
    /// The layer's source, `None` for a layer without one or a source type
    /// the renderer has no render source for.
    pub(crate) source: Option<&'a RenderSource>,
    /// What each of the source's render tiles holds, at its index.
    pub(crate) contents: &'a [Option<TileContent<'a>>],
    /// The tiles of the frame's layers, as their updates find them, which
    /// the update adds the layer's to: those the encode draws.
    pub(crate) found: &'a mut Vec<FoundTile>,
    /// Whether the layer places: its tiles are all its source's, those held
    /// while placement fades them out included, in placement's order
    /// ([`placement::tile_order`](crate::placement::tile_order)).
    pub(crate) places: bool,
}

/// A render layer of the layer type `L`: the layer, its values, and what
/// the engine keeps of it.
pub(crate) struct Hosted<L: Layer> {
    /// The layer's type, as the registry holds it.
    entry: &'static dyn ErasedLayerType,
    values: LayerValues<L::Style>,
    layer: L,
    /// The layer's values of the frame's tiles.
    tile_values: TileMap<L::Tile>,
    /// What the layer's update found of its frame tiles.
    frame_tiles: <L::Input as input::Find<L::Style>>::Frame,
    style_dependencies: Dependency,
    /// Whether the frame draws the layer.
    renderable: bool,
    /// Whether the layer's tile values are made for the globe.
    globe: bool,
    /// The layer's draws of the frame.
    frame_draws: Draws,
    /// What the engine reads of the layer.
    record: LayerRecord,
    /// Where the participant of the layer's type is among the renderer's,
    /// for a type whose layers place.
    participant: Option<usize>,
}

impl<L: Layer> Hosted<L> {
    /// The layer and its values.
    pub(crate) fn parts(&self) -> (&L, &LayerValues<L::Style>) {
        (&self.layer, &self.values)
    }

    /// The render layer of the layer `style`, of the type `entry`, before
    /// its values transition, which joins the renderer of `res`, and places
    /// through the renderer's participant at `participant`, if its type has
    /// one.
    pub(crate) fn new(
        entry: &'static dyn ErasedLayerType,
        style: Arc<LayerStyle<L::Style>>,
        res: &mut Resources,
        participant: Option<usize>,
    ) -> Self {
        let unevaluated = style.paint.untransitioned();
        let evaluated = Arc::new(EvaluatedLayer::with_paint(entry, Arc::clone(&style), Default::default()));
        let values = LayerValues { style, unevaluated, evaluated };
        let after_gpu = res.after_gpu.handle().clone();
        let layer = L::new(&mut NewCx { layer: &values, res, after_gpu: &after_gpu });
        Hosted {
            record: LayerRecord::new::<L>(entry, &values.style, Evaluation::NONE, false),
            participant,
            entry,
            style_dependencies: <L::Style as StyleLayerType>::PaintProperties::dependencies(&values.unevaluated),
            values,
            layer,
            tile_values: TileMap::default(),
            frame_tiles: Default::default(),
            renderable: false,
            globe: false,
            frame_draws: Draws::new(),
        }
    }
}

/// What the engine reads of a render layer to run a frame and answer
/// queries ([`RenderLayer::record`]). The layer's restyle, evaluation and
/// update keep it.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct LayerRecord {
    /// The kind of source the layer's type draws, `None` for a type without
    /// one.
    pub(crate) input: SourceKind,
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
    /// Whether the layer places ([`Placement::places`](crate::placement::Placement::places)).
    pub(crate) places: bool,
}

impl LayerRecord {
    /// The record of a layer of the type `L`, of the style `style` and the
    /// type `entry`, with `evaluation` and `draws`.
    fn new<L: Layer>(
        entry: &dyn ErasedLayerType,
        style: &LayerStyle<L::Style>,
        evaluation: Evaluation,
        draws: bool,
    ) -> Self {
        let base = style.base();
        LayerRecord {
            input: crate::layer::input_source::<L>(),
            visible: base.visibility != VisibilityType::None,
            min_zoom: base.min_zoom,
            max_zoom: base.max_zoom,
            evaluation,
            draws,
            volume: L::VOLUME,
            places: entry.places(style),
        }
    }

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

    /// What the engine reads of the layer.
    fn record(&self) -> &LayerRecord;

    /// The renderer's participant that places the layer, if it places.
    fn placing(&self) -> Option<usize>;

    /// Makes `style`, the changed style of the layer (of the same id and
    /// type), the layer's, drops its draws and, unless its type keeps them
    /// ([`Layer::RESTYLE_KEEPS_TILES`]), its tile values, which were made for
    /// the previous style's values, and begins transitions to the new
    /// values. A style of another type leaves the layer's. The record is
    /// made again, with its evaluation and whether it draws.
    fn restyle(&mut self, parameters: &TransitionParameters, style: &Arc<dyn ErasedLayerStyle>);

    /// Makes `style`, the layer's style with other
    /// [`Data`](StyleLayerType::Data) alone, the layer's. Its tile values
    /// and transitions stay.
    fn set_data(&mut self, style: &Arc<dyn ErasedLayerStyle>);

    /// Begins transitions for the paint values that changed since the last
    /// frame, to those of the layer's style.
    fn transition(&mut self, parameters: &TransitionParameters);

    /// Evaluates the layer's paint values for a frame of `parameters`, and
    /// records what the layer says of them ([`Layer::evaluate`]), which
    /// says whether it draws until the next evaluation.
    fn evaluate(&mut self, parameters: &PropertyEvaluationParameters);

    /// Whether a paint value of the layer is still in transition.
    fn has_transition(&self) -> bool;

    /// Whether the layer has a pattern property and is in a cross-fade.
    fn has_crossfade(&self) -> bool;

    /// The frame's update of the layer: finds the tiles of `update`'s source
    /// the layer draws, which it adds to `update.found`, or the tiles that
    /// cover the view for a layer of no source. Where one of their buckets
    /// was laid out with values that draw, the layer draws from then on,
    /// until the next evaluation.
    fn update(&mut self, update: &mut LayerUpdate<'_>);

    /// The encode's run of the layer, the frame's layer at `index`: drops
    /// its tile values where the frame's projection changed, and where its
    /// evaluation draws, gives the tiles of `source` that its update found
    /// in `found` their data, runs [`Layer::update`] with the root keys of
    /// `style`, the frame's version, routes the layer
    /// through `routes` with the named targets it read, runs [`Layer::tile`]
    /// for the tiles without a value and [`Layer::prepare`], and readies the
    /// draws it pushed.
    #[expect(clippy::too_many_arguments, reason = "the layer's frame, each from another part of the renderer")]
    fn update_draws(
        &mut self,
        gpu: &mut dyn Gpu,
        paint: &mut PaintParameters<'_>,
        images: &ImageStore,
        source: Option<FrameSourceTiles<'_>>,
        found: &[FoundTile],
        index: usize,
        style: &Style,
        routes: &mut Routes,
    );

    /// Moves the layer's value of each tile to the tile `wrap_delta` world
    /// copies east of it: the tiles are the same ones under new ids once the
    /// map's center has crossed the antimeridian, and their values stay.
    fn handle_wrap_jump(&mut self, wrap_delta: i16);

    /// Includes the layer in the frame's drawing or excludes it. A layer
    /// excluded drops its draws and its tiles with their values.
    fn set_renderable(&mut self, renderable: bool);

    /// Whether the frame draws the layer.
    fn is_renderable(&self) -> bool;

    /// What the layer's paint values depend on.
    fn style_dependencies(&self) -> Dependency;

    /// Drops the frame's draws of the layer and its values of its tiles.
    /// The tiles stay.
    fn remove_all_draws(&mut self);

    /// Visits the layer's draw groups of the frame, in drawing order, each
    /// with whether it draws into a named target: none unless the frame
    /// draws the layer.
    fn visit_draw_groups<'a>(&'a self, visit: &mut dyn FnMut(&'a FrameDraws, bool));

    /// Uploads the meshes of the layer's draw groups and of its draws into
    /// its targets, before the frame draws them.
    fn upload(&mut self, gpu: &mut dyn Gpu, context: &mut Resources);

    /// Draws the layer's draws into its targets through `gpu`, the
    /// context's, before the frame's passes.
    fn render_targets(&mut self, gpu: &mut dyn Gpu, parameters: &mut PaintParameters<'_>);

    /// Draws the pass's draws of the layer's draw groups into the open pass
    /// of `gpu`, the context's, in `order`, each at its depth, where the
    /// frame draws them `into` the pass: the named target of that index, or
    /// with `None` the map's.
    fn render(
        &mut self,
        gpu: &mut dyn Gpu,
        parameters: &mut PaintParameters<'_>,
        order: DrawOrder,
        into: Option<usize>,
    );

    /// The query radius of the layer's features in `contents`, a tile's
    /// layout, seen by `camera` ([`Layer::query_radius`]).
    fn query_radius(&self, contents: &GeometryContents, camera: &View) -> f32;

    /// Whether `test`'s feature, as the layer draws it, is hit by the
    /// query's geometry ([`Layer::hit`]).
    fn hit(&self, test: &ErasedHitTest<'_>) -> Option<f32>;

    /// Adds to `hits` what `geometry`, in `camera`'s pixels, hits of what
    /// the layer draws without tile features ([`Layer::hit_frame`]).
    fn hit_frame(&self, camera: &View, geometry: &[ScreenCoordinate], hits: &mut Vec<Hit>);
}

/// Lends `layer` the backend's commands of `gpu` `at` the frame's time,
/// where the frame has them to lend: in a pass, the open one. A lend that
/// fails fails the frame.
fn lend<L: Layer>(layer: &mut L, gpu: &mut dyn Gpu, parameters: &mut PaintParameters<'_>, at: LendAt) {
    let (Some(projection), frame) = (parameters.frame_projection, parameters.frame) else { return };
    let layer_depth = match at {
        LendAt::BeforePasses => None,
        LendAt::Pass if parameters.pass_target().is_some() => Some(f64::from(parameters.layer_depth())),
        LendAt::Pass => return,
    };
    let lent = gpu.lend(&mut |raw| layer.lend(at, Lent { raw, frame: &frame, projection: &projection, layer_depth }));
    if let Err(error) = lent {
        parameters.context.fail(format!("Lending a layer the backend's commands failed: {error}"));
    }
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

    fn record(&self) -> &LayerRecord {
        &self.record
    }

    fn placing(&self) -> Option<usize> {
        self.participant.filter(|_| self.record.places)
    }

    fn restyle(&mut self, parameters: &TransitionParameters, style: &Arc<dyn ErasedLayerStyle>) {
        if L::RESTYLE_KEEPS_TILES {
            self.frame_draws.clear();
        } else {
            self.remove_all_draws();
        }
        if let Some(recovered) = LayerStyle::recover(style) {
            self.values.style = recovered;
        }
        self.transition(parameters);
        let LayerRecord { evaluation, draws, .. } = self.record;
        self.record = LayerRecord::new::<L>(self.entry, &self.values.style, evaluation, draws);
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

    fn evaluate(&mut self, parameters: &PropertyEvaluationParameters) {
        type Paint<L> = <<L as Layer>::Style as StyleLayerType>::PaintProperties;
        let values = &mut self.values;
        let paint = Paint::<L>::evaluate_with_previous(&mut values.unevaluated, parameters, &values.evaluated.paint);
        let mut evaluated = EvaluatedLayer::with_paint(self.entry, Arc::clone(&values.style), paint);
        if Paint::<L>::CROSSFADES {
            evaluated.crossfade = parameters.get_crossfade_parameters();
        }
        let evaluation = self.layer.evaluate(EvaluateCx { layer: values, parameters, evaluated: &mut evaluated });
        evaluated.draws = evaluation.draws;
        values.evaluated = Arc::new(evaluated);
        self.record.evaluation = evaluation;
        self.record.draws = evaluation.draws;
    }

    fn has_transition(&self) -> bool {
        <L::Style as StyleLayerType>::PaintProperties::has_transition(&self.values.unevaluated)
    }

    fn has_crossfade(&self) -> bool {
        <L::Style as StyleLayerType>::PaintProperties::CROSSFADES && self.values.evaluated.crossfade.t != 1.0
    }

    fn update(&mut self, update: &mut LayerUpdate<'_>) {
        self.record.draws |= <L::Input as input::Find<L::Style>>::update(
            &mut self.frame_tiles,
            update,
            &self.values.style.id,
            &mut self.tile_values,
        );
    }

    fn update_draws(
        &mut self,
        gpu: &mut dyn Gpu,
        paint: &mut PaintParameters<'_>,
        images: &ImageStore,
        source: Option<FrameSourceTiles<'_>>,
        found: &[FoundTile],
        index: usize,
        style: &Style,
        routes: &mut Routes,
    ) {
        let (frame, evaluation) = (paint.frame, self.record.evaluation);
        // The tiles of a layer without a source have other geometry on the
        // globe.
        if frame.camera.is_globe() != self.globe {
            self.globe = frame.camera.is_globe();
            self.remove_all_draws();
        }
        self.frame_draws.begin(frame.camera.zoom() as f32, evaluation.opaque, frame.composites_with_frame_depth);
        if !evaluation.draws {
            self.frame_draws.clear();
            return;
        }

        let mut tiles: SmallVec<[Found<'_>; 64]> = SmallVec::new();
        <L::Input as input::Find<L::Style>>::tiles(&self.frame_tiles, found, source, &mut self.tile_values, &mut tiles);

        let res = &mut *paint.context;
        let Hosted { values, layer, tile_values, frame_draws, .. } = self;
        layer.update(&mut UpdateCx {
            layer: values,
            frame: &frame,
            images,
            source_max_zoom: source.and_then(|source| source.max_zoom),
            tiles: Tiles { found: &tiles, values: tile_values },
            res,
            style,
            targets: routes.reads(),
        });
        let route = routes.route(index, &values.style.id, values.style.extra(), L::VOLUME);
        if let Some(target) = route {
            frame_draws.route_into(target);
        }
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
        frame_draws.finish(gpu, res, &values.evaluated.paint, tiles.iter().map(|found| found.tile.unwrapped));
        if route.is_some() {
            routes.check_routed(&values.style.id, frame_draws);
        }
    }

    fn handle_wrap_jump(&mut self, wrap_delta: i16) {
        self.tile_values.shift_wraps(wrap_delta);
    }

    fn set_renderable(&mut self, renderable: bool) {
        self.renderable = renderable;
        if !renderable {
            self.remove_all_draws();
            self.tile_values.clear_tiles();
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

    fn visit_draw_groups<'a>(&'a self, visit: &mut dyn FnMut(&'a FrameDraws, bool)) {
        if self.renderable {
            for group in self.frame_draws.groups() {
                visit(&group.frame, self.frame_draws.route().is_some());
            }
        }
    }

    fn upload(&mut self, gpu: &mut dyn Gpu, context: &mut Resources) {
        if self.renderable {
            for group in self.frame_draws.target_groups().chain(self.frame_draws.groups()) {
                group.upload(gpu, context);
            }
        }
    }

    fn render_targets(&mut self, gpu: &mut dyn Gpu, parameters: &mut PaintParameters<'_>) {
        if self.renderable {
            self.frame_draws.render_targets(gpu, parameters);
            if self.frame_draws.lends(LendAt::BeforePasses) {
                lend(&mut self.layer, gpu, parameters, LendAt::BeforePasses);
            }
        }
    }

    fn render(
        &mut self,
        gpu: &mut dyn Gpu,
        parameters: &mut PaintParameters<'_>,
        order: DrawOrder,
        into: Option<usize>,
    ) {
        if !self.renderable || self.frame_draws.route() != into {
            return;
        }
        let lends = into.is_none() && self.frame_draws.lends(LendAt::Pass) && parameters.pass == Pass::Translucent;
        let Hosted { layer, frame_draws, .. } = self;
        let main = frame_draws.main_group();
        let mut render = |group: &crate::encode::Group| {
            group.render(gpu, parameters);
            if lends && std::ptr::eq(group, main) {
                lend(layer, gpu, parameters, LendAt::Pass);
            }
            order.step(parameters);
        };
        match order {
            DrawOrder::BottomUp => frame_draws.groups().for_each(&mut render),
            DrawOrder::TopDown => frame_draws.groups().rev().for_each(&mut render),
        }
    }

    fn query_radius(&self, contents: &GeometryContents, camera: &View) -> f32 {
        let laid = <L::Input as input::Find<L::Style>>::laid(contents, &self.values.style.id);
        laid.map_or(0.0, |data| self.layer.query_radius(&self.values, data, camera))
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

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use mln_style::TransitionParameters;
    use mln_style::layer_type::{
        ErasedLayerStyle, LayerStyle, LayerTypeInfo, NoProperties, SourceKind, StyleLayerType,
    };
    use mln_tile_id::OverscaledTileID;

    use super::{Hosted, RenderLayer};
    use crate::draw::Draws;
    use crate::gfx::context::Resources;
    use crate::layer::{DemTiles, Layer, NewCx, PrepareCx};
    use crate::tests::fixture_layer::Shade;

    /// A layer type of a raster-dem source whose tile values a restyle
    /// keeps, drawing nothing.
    struct Kept;

    impl StyleLayerType for Kept {
        const INFO: LayerTypeInfo = LayerTypeInfo { name: "kept", source: SourceKind::RasterDem, from_json: true };

        type LayoutProperties = NoProperties;
        type PaintProperties = NoProperties;
        type Data = ();
    }

    impl Layer for Kept {
        type Style = Kept;
        type Input = DemTiles;
        type Tile = ();

        const RESTYLE_KEEPS_TILES: bool = true;

        fn new(_cx: &mut NewCx<'_, Self>) -> Self {
            Kept
        }

        fn prepare(&self, _cx: &PrepareCx<'_, Self>, _out: &mut Draws) {}
    }

    /// Whether a layer of the type `L` still has the value it made of a
    /// tile after a restyle.
    fn keeps_its_tile_after_a_restyle<L: Layer<Style = L, Tile = ()> + StyleLayerType>() -> bool {
        let style = |source: &str| Arc::new(LayerStyle::<L>::new("layer".to_owned(), source.to_owned()));
        let entry = crate::erased_layer::entry::<L, ()>();
        let mut layer = Hosted::<L>::new(entry, style("dem"), &mut Resources::detached(), None);
        layer.tile_values.set_tiles(&[OverscaledTileID::with_zxy(1, 0, 1, 0, 0)]);
        *layer.tile_values.get_mut(0) = Some(());
        let restyled: Arc<dyn ErasedLayerStyle> = style("other");
        layer.restyle(&TransitionParameters::default(), &restyled);
        layer.tile_values.get(0).is_some()
    }

    #[test]
    fn a_restyle_drops_the_tile_values_unless_the_type_keeps_them() {
        assert!(!keeps_its_tile_after_a_restyle::<Shade>(), "made from the previous style's values");
        assert!(keeps_its_tile_after_a_restyle::<Kept>());
    }
}
