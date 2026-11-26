//! Entry points to the renderer's internal stages, as functions over data,
//! for the benchmarks (`tools/bench`) to measure. Built only with the
//! `bench` feature, and hidden from the documentation: they aren't API.
//!
//! Each entry point takes and returns public types only, so that the
//! stages' internals can change without changing what the benchmarks call.

use std::collections::BTreeSet;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use glam::{DMat4, DVec4};
use mln_style::Style;
use mln_style::constants::EXTENT;
use mln_style::error::Error;
use mln_style::property::evaluation::PropertyEvaluationParameters;
use mln_style::tile_id::CanonicalTileID;
use mln_style::types::VisibilityType;
use mln_vec_map::VecSet;
use parking_lot::Mutex;

use crate::Registry;
use crate::camera::View;
use crate::draw::dynamic_texture_atlas::DynamicTextureAtlas;
use crate::image::image_store::{ImageMap, ImageType};
use crate::layer::bucket::LayerFeatureStates;
use crate::layer::layer_type::ErasedEvaluatedLayer;
use crate::layer::render_layer::{LayerParts, RenderLayer};
use crate::layer::tile_layout::{Dependencies, LayoutParams, Resolved};
use crate::map::MapMode;
use crate::placement::{Placement, PlacementInputs};
use crate::render::paint_parameters::TransformParameters;
use crate::render::tile::RenderTile;
use crate::render::update_parameters::UpdateParameters;
use crate::storage::Resource;
use crate::symbol::cross_tile_index::CrossTileSymbolIndex;
use crate::symbol::layer::RenderSymbolLayer;
use crate::text::glyph::{GlyphMap, get_glyph_range, style_font_faces};
use crate::text::glyph_pbf::parse_glyph_pbf;
use crate::text::glyph_store::ShapeResults;
use crate::text::tagged_string::font_stack_hash;
use crate::tile::{OverscaledTileID, TileData, UnwrappedTileID};
use crate::tile_store::contents::LayoutResult;
use crate::tile_store::store::TileStore;
use crate::worker::job::{
    DataInput, DataKey, Job, JobId, JobResult, JobTimes, LayoutJob, Reply, Run, SourceKey, SourceStyle, TileKey,
};
use crate::worker::tile_workers::InlineWorkers;
use crate::worker::worker_codec::{Encoded, MapCodec, WorkerCodec, read_job_id, read_reply_header};

/// A vector tile's layout job, as a tile worker runs it once the tile has
/// every glyph and image it needs: the selection of each layer's features,
/// and the buckets and feature index built from them.
pub struct TileLayout {
    id: OverscaledTileID,
    style: Arc<SourceStyle>,
    available_images: Arc<VecSet<String>>,
    params: LayoutParams,
    resolved: Arc<Resolved>,
}

impl TileLayout {
    /// The layout of tile `z`/`x`/`y` of `source` in `style`, by those of
    /// the style's visible layers on `source` whose type `include` accepts,
    /// evaluated at the tile's zoom as the renderer evaluates them, for a
    /// continuous map at pixel ratio 1.
    ///
    /// It lays `data` out once to find what the layout needs, and resolves
    /// that as the renderer would: glyph ranges from the style's glyph URL,
    /// whose bytes `load` gives, and images from the style's images, packed
    /// into an atlas. Fails for a style without the source, a layer the
    /// renderer has no type for, or a layout that fails.
    pub fn new(
        style: &Style,
        source: &str,
        include: impl Fn(&str) -> bool,
        (z, x, y): (u8, u32, u32),
        data: &TileData,
        load: impl Fn(&str) -> Option<Vec<u8>>,
    ) -> Result<TileLayout, Error> {
        TileLayout::overscaled(style, source, include, (z, x, y), z, data, load)
    }

    /// [`TileLayout::new`] for the tile shown at `overscaled_z`, a zoom
    /// at or above its own, as a map shows the tiles of its source's highest
    /// zoom when it zooms in further: the layers evaluated at that zoom, and
    /// the tile's features scaled to it.
    pub fn overscaled(
        style: &Style,
        source: &str,
        include: impl Fn(&str) -> bool,
        (z, x, y): (u8, u32, u32),
        overscaled_z: u8,
        data: &TileData,
        load: impl Fn(&str) -> Option<Vec<u8>>,
    ) -> Result<TileLayout, Error> {
        if style.get_source(source).is_none() {
            return Err(Error::invalid_argument(format!("the style has no source {source}")));
        }
        if overscaled_z < z {
            return Err(Error::invalid_argument(format!("tile {z}/{x}/{y} shown at zoom {overscaled_z}")));
        }
        let registry = Registry::with_builtins();
        let parameters = PropertyEvaluationParameters::new(f32::from(overscaled_z));
        let mut layers: Vec<Arc<dyn ErasedEvaluatedLayer>> = Vec::new();
        for layer in style.get_layers() {
            let base = layer.style().base();
            if base.source != source || base.visibility == VisibilityType::None || !include(layer.info().name) {
                continue;
            }
            let mut render_layer = registry
                .layer_type(layer.info().name)
                .and_then(|layer_type| layer_type.create_render_layer(layer.style()))
                .ok_or_else(|| Error::invalid_argument(format!("no render layer for {}", layer.get_id())))?;
            render_layer.evaluate(&parameters);
            layers.push(Arc::clone(&render_layer.base().evaluated_properties));
        }

        let mut layout = TileLayout {
            id: OverscaledTileID::new(overscaled_z, 0, CanonicalTileID::new(z, x, y)),
            style: Arc::new(SourceStyle {
                version: 0,
                layers,
                global_state: Some(Arc::clone(style.get_global_state())),
                font_faces: style_font_faces(style.get_font_faces().map(|faces| &**faces), None),
                glyph_pbfs: !style.get_glyph_url().is_empty(),
            }),
            available_images: Arc::new(style.get_images().iter().map(|image| image.id.clone()).collect()),
            params: LayoutParams {
                mode: MapMode::Continuous,
                pixel_ratio: 1.0,
                subdivision_granularity: style.get_projection().get_subdivision_granularity(),
                first_load: true,
            },
            resolved: Arc::default(),
        };
        match layout.job(data.clone()) {
            JobResult::NeedsDeps(dependencies) => layout.resolved = Arc::new(resolve(&dependencies, style, &load)?),
            JobResult::Laid(_) => {}
            JobResult::Failed(error) => return Err(error),
            JobResult::NeedsData | JobResult::Raster(_) | JobResult::Dem(_) => {
                return Err(Error::native("a layout job answered as no layout does"));
            }
        }
        Ok(layout)
    }

    /// Lays `data` out, as the job that a tile worker runs once its glyphs
    /// and images are resolved: the result holds the tile's buckets.
    pub fn run(&self, data: TileData) -> Result<LaidOut, Error> {
        match self.job(data) {
            JobResult::Laid(output) => Ok(LaidOut { id: self.id, result: output.result }),
            JobResult::Failed(error) => Err(error),
            JobResult::NeedsDeps(_) | JobResult::NeedsData | JobResult::Raster(_) | JobResult::Dem(_) => {
                Err(Error::native("the layout needs what it didn't need before"))
            }
        }
    }

    /// Sets the feature state of `laid_out`'s buckets, a layout of this
    /// tile, to `states`, as a tile does when a map's feature state
    /// changes: each bucket whose paint depends on feature state evaluates
    /// that paint again for the features that have a state.
    pub fn set_feature_state(&self, laid_out: &mut LaidOut, states: &LayerFeatureStates) {
        laid_out.result.set_feature_state(states);
    }

    fn job(&self, data: TileData) -> JobResult {
        InlineWorkers::run(self.run_of(data, 1)).result
    }

    /// The job of `data`'s layout, as the tile's `generation`th.
    fn run_of(&self, data: TileData, generation: u64) -> Run {
        let tile = TileKey { source: SourceKey(1), id: self.id };
        let key = DataKey { source: SourceKey(1), tile: self.id.canonical, load: generation };
        let job = LayoutJob {
            style: Arc::clone(&self.style),
            available_images: Arc::clone(&self.available_images),
            data: DataInput::Full { key, data: Some(data) },
            params: self.params,
            resolved: Arc::clone(&self.resolved),
        };
        Run { id: JobId { tile, generation }, job: Job::Layout(job), superseded: Arc::default() }
    }
}

/// A tile's buckets and feature index, as its layout made them.
pub struct LaidOut {
    id: OverscaledTileID,
    result: LayoutResult,
}

impl LaidOut {
    /// The number of layers that got a bucket.
    #[must_use]
    pub fn buckets(&self) -> usize {
        self.result.layer_render_data.len()
    }
}

/// The symbols of laid-out tiles in a view, as a renderer finds which to
/// show: their render layers, each tile's matrices for the view, and the
/// view's update.
pub struct SymbolView {
    update: Arc<UpdateParameters>,
    /// The style's symbol layers that lead a bucket of the tiles, in the
    /// style's order.
    layers: Vec<RenderSymbolLayer>,
}

impl SymbolView {
    /// The view that `update` shows of `tiles`, laid out by the visible
    /// symbol layers on `source` in `update`'s style. Fails for a layer the
    /// renderer has no type for.
    ///
    /// The harness makes the update, since its fields are what a renderer
    /// takes from a map: a comparison builds this module against libraries
    /// whose updates have other fields.
    pub fn new(source: &str, tiles: &[&LaidOut], update: UpdateParameters) -> Result<SymbolView, Error> {
        let transform = TransformParameters::new(&update.view);
        let render_tiles: Rc<Vec<RenderTile>> = Rc::new(
            tiles
                .iter()
                .map(|laid_out| {
                    let mut render_tile = RenderTile::of_tile_id(laid_out.id.to_unwrapped(), laid_out.id, false);
                    render_tile.set_projection(&transform);
                    render_tile
                })
                .collect(),
        );

        let parameters = PropertyEvaluationParameters::new(update.view.zoom() as f32);
        let mut layers = Vec::new();
        for layer in update.style.get_layers() {
            let base = layer.style().base();
            if base.source != source || base.visibility == VisibilityType::None || layer.info().name != "symbol" {
                continue;
            }
            let style = mln_style::layer::LayerStyle::<mln_style::layers::Symbol>::recover(layer.style())
                .ok_or_else(|| Error::invalid_argument(format!("no render layer for {}", layer.get_id())))?;
            let mut render_layer = RenderSymbolLayer::new(style);
            render_layer.evaluate(&parameters);
            // As the layer's update adds its tiles' buckets.
            let translates = render_layer.symbol_translates();
            for (tile_index, laid_out) in tiles.iter().enumerate() {
                let Some(render_data) = laid_out.result.layer_render_data.get(layer.get_id()) else { continue };
                let Some(bucket) = crate::layer::bucket::recover(&render_data.bucket) else { continue };
                let feature_index = || Some(Arc::clone(&laid_out.result.feature_index));
                render_layer.add_placement_data(&render_tiles, tile_index, bucket, feature_index, translates);
            }
            if render_layer.has_placement_data() {
                layers.push(render_layer);
            }
        }

        Ok(SymbolView { update: Arc::new(update), layers })
    }

    /// Adds the view's symbol buckets to `index`, as a renderer does before
    /// it places them: each symbol takes the id of the same symbol in a
    /// tile of another zoom that `index` holds, or a new one, and `index`
    /// drops the buckets of each layer that the view no longer has. Returns
    /// how many layers added a bucket.
    pub fn index(&self, index: &mut SymbolIndex) -> Result<usize, Error> {
        let longitude = self.update.view.center(mln_style::geo::WrapMode::Unwrapped).longitude() as f32;
        let mut added = 0;
        for layer in self.layers.iter().rev() {
            let result = index.0.add_layer(layer, longitude, &self.update.view);
            if result.buckets_added {
                added += 1;
            }
        }
        Ok(added)
    }

    /// Places the view's symbols, topmost layer first, each where it
    /// collides with none placed before it, after a placement that showed
    /// none: what a renderer does when the view changes. Returns how many
    /// symbols it shows. Their ids come from [`SymbolView::index`]: a symbol
    /// without one shows nowhere.
    #[must_use]
    pub fn place(&self) -> usize {
        let previous = Placement::new_default();
        let layers: Vec<&dyn RenderLayer> = self.layers.iter().map(|layer| -> &dyn RenderLayer { layer }).collect();
        let mut buckets = crate::layer::render_layer::HeldBuckets::take();
        let inputs = PlacementInputs::new(&self.update, None, &layers, &buckets);
        let mut placement = Placement::create(Arc::new(inputs), self.update.time_point, Some(previous));
        placement.place(&mut buckets);
        buckets.restore();
        placement.shown()
    }
}

/// The renderer's cross-tile symbol index: the symbols of the tiles that
/// it shows, by layer, with the ids that keep a symbol the same across
/// tiles of different zooms.
#[derive(Default)]
pub struct SymbolIndex(CrossTileSymbolIndex);

/// A worker instance's codec and the map side's, between which a tile's
/// job and its reply cross as frames of bytes
/// (`worker::worker_codec`), as in the browser build.
pub struct Codec {
    map: MapCodec,
    worker: WorkerCodec,
    generation: u64,
}

/// A tile's job as the worker instance decoded it.
pub struct WorkerJob(Run);

/// A worker instance's reply to a job, before it crosses back.
pub struct WorkerReply(Reply);

impl Codec {
    /// The two sides of a new worker instance.
    #[must_use]
    pub fn new() -> Codec {
        Codec {
            map: MapCodec::new(Registry::with_builtins()),
            worker: WorkerCodec::new(Registry::with_builtins()),
            generation: 0,
        }
    }

    /// Sends `layout`'s job for `data` to the worker: the map side encodes
    /// it, defining the style's values that the worker lacks, and the worker
    /// decodes it.
    pub fn send(&mut self, layout: &TileLayout, data: TileData) -> Result<WorkerJob, Error> {
        self.generation += 1;
        let frame = self.map.encode_run(&layout.run_of(data, self.generation)).map_err(Error::native)?;
        let (id, body) = read_job_id(&frame).ok_or_else(|| Error::native("a job frame without its id"))?;
        let job = self.worker.decode_job(body).map_err(Error::native)?;
        Ok(WorkerJob(Run { id, job, superseded: Arc::default() }))
    }

    /// Sends `reply` back: the worker encodes it, its arrays as payloads,
    /// and the map side decodes it.
    pub fn reply(&mut self, reply: &WorkerReply) -> Result<LaidOut, Error> {
        let encoded = self.worker.encode_reply(&reply.0, &JobTimes::default());
        let (_, _, body) =
            read_reply_header(&encoded.bytes).ok_or_else(|| Error::native("a reply frame without its header"))?;
        let body = Encoded { bytes: body.to_vec(), payloads: encoded.payloads };
        match self.map.decode_result(body) {
            JobResult::Laid(output) => Ok(LaidOut { id: reply.0.id.tile.id, result: output.result }),
            JobResult::Failed(error) => Err(error),
            JobResult::NeedsDeps(_) | JobResult::NeedsData | JobResult::Raster(_) | JobResult::Dem(_) => {
                Err(Error::native("the reply isn't a layout"))
            }
        }
    }
}

impl Default for Codec {
    fn default() -> Self {
        Codec::new()
    }
}

impl WorkerJob {
    /// Runs the job, as the worker instance does.
    #[must_use]
    pub fn run(self) -> WorkerReply {
        WorkerReply(InlineWorkers::run(self.0))
    }
}

/// What `dependencies` resolve to: the glyphs of the ranges that `load`
/// gives for the style's glyph URL, and the style's images, packed into an
/// atlas as the renderer packs them.
fn resolve(
    dependencies: &Dependencies,
    style: &Style,
    load: &impl Fn(&str) -> Option<Vec<u8>>,
) -> Result<Resolved, Error> {
    let mut glyphs = GlyphMap::new();
    for (font_stack, ids) in &dependencies.glyphs.glyphs {
        let font_glyphs = glyphs.entry(font_stack_hash(font_stack)).or_default();
        let ranges: BTreeSet<_> = ids.iter().map(|id| get_glyph_range(*id)).collect();
        for range in ranges {
            let url = Resource::glyphs(style.get_glyph_url(), font_stack, (range.first, range.second)).url;
            let Some(bytes) = load(&url) else { continue };
            let parsed = parse_glyph_pbf(&range, &bytes).map_err(|error| Error::native(format!("{url}: {error}")))?;
            for glyph in parsed {
                font_glyphs.insert(glyph.id, Some(Arc::new(glyph)));
            }
        }
        for id in ids {
            font_glyphs.entry(*id).or_insert(None);
        }
    }

    let (mut icons, mut patterns) = (ImageMap::new(), ImageMap::new());
    for (id, type_) in &dependencies.images {
        if let Some(image) = style.get_image(id) {
            let map = match type_ {
                ImageType::Icon => &mut icons,
                ImageType::Pattern => &mut patterns,
            };
            map.insert(id.clone(), Arc::clone(image));
        }
    }
    let missing_images = dependencies
        .images
        .keys()
        .filter(|id| !icons.contains_key(*id) && !patterns.contains_key(*id))
        .cloned()
        .collect();

    let mut atlas = DynamicTextureAtlas::new();
    let glyph_page = atlas.pack_glyphs(&glyphs);
    let image_page = atlas.pack_images(&icons, &patterns);
    Ok(Resolved {
        glyphs: glyph_page.positions,
        shaping: ShapeResults::new(),
        icons: image_page.icons,
        patterns: image_page.patterns,
        missing_images,
    })
}

/// What a frame showed of the view, as shares of the view's area: the
/// area of the tiles that the view wants (its ideal cover, inside each
/// source's bounds), split by what drew there. Each tile counts by its area
/// on the screen, so a tilted view's far tiles count little. A view on the
/// globe isn't measured.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Coverage {
    /// Drawn by the tiles the view wants.
    pub ideal: f64,
    /// Drawn by a parent or children of a tile that hasn't loaded.
    pub stand_in: f64,
    /// Where nothing drew.
    pub blank: f64,
}

/// Whether [`measure_coverage`] was called.
static MEASURING_COVERAGE: AtomicBool = AtomicBool::new(false);

/// The coverage of the last update.
static COVERAGE: Mutex<Option<Coverage>> = Mutex::new(None);

/// Measures each update's [`Coverage`] from now on, for [`take_coverage`].
pub fn measure_coverage() {
    MEASURING_COVERAGE.store(true, Ordering::Relaxed);
}

/// The [`Coverage`] of the renderer's last update since the last call, the
/// average of the sources that had a cover, or `None` when none had: the
/// style or its sources hadn't loaded, or the view is on the globe.
pub fn take_coverage() -> Option<Coverage> {
    COVERAGE.lock().take()
}

/// Records the coverage of the sources' covers after an update in `view`
/// (`RenderOrchestrator::update`).
pub(crate) fn observe_cover(view: &View, store: &TileStore) {
    if !MEASURING_COVERAGE.load(Ordering::Relaxed) {
        return;
    }
    let coverage = if view.is_globe() {
        None
    } else {
        let projection = view.projection_matrix();
        let area = |id: &UnwrappedTileID| screen_share(&(projection * view.tile_matrix(id)));
        let mut sum = Coverage::default();
        let mut sources = 0u32;
        for source in store.sources().filter(|source| !source.ideal.is_empty()) {
            let rendered: Vec<_> =
                source.rendered().filter(|tile| !tile.hold_for_fade).map(|tile| (*tile.id, tile.tile.id)).collect();
            let coverage = cover_coverage(&source.ideal, &rendered, area);
            sum.ideal += coverage.ideal;
            sum.stand_in += coverage.stand_in;
            sum.blank += coverage.blank;
            sources += 1;
        }
        (sources > 0).then(|| {
            let sources = f64::from(sources);
            Coverage { ideal: sum.ideal / sources, stand_in: sum.stand_in / sources, blank: sum.blank / sources }
        })
    };
    *COVERAGE.lock() = coverage;
}

/// The coverage of a source whose ideal cover is `ideal` and which draws
/// `rendered` (each tile where it draws, and which tile it is), where `area`
/// gives a tile's share of the view.
fn cover_coverage(
    ideal: &[OverscaledTileID],
    rendered: &[(UnwrappedTileID, OverscaledTileID)],
    area: impl Fn(&UnwrappedTileID) -> f64,
) -> Coverage {
    let mut coverage = Coverage::default();
    for wanted in ideal {
        let at = wanted.to_unwrapped();
        let share = area(&at);
        if let Some((_, key)) = rendered.iter().find(|(id, _)| *id == at) {
            if key.overscaled_z == wanted.overscaled_z {
                coverage.ideal += share;
            } else {
                coverage.stand_in += share;
            }
        } else if rendered.iter().any(|(id, _)| id.canonical.z < at.canonical.z && at.is_child_of(id)) {
            coverage.stand_in += share;
        } else {
            let children: f64 = rendered
                .iter()
                .filter(|(id, _)| id.canonical.z > at.canonical.z && id.is_child_of(&at))
                .map(|(id, _)| area(id))
                .sum();
            let children = children.min(share);
            coverage.stand_in += children;
            coverage.blank += share - children;
        }
    }
    coverage
}

/// The share of the view that a tile covers, drawn with `matrix` (from the
/// tile's units to clip space): its square clipped to the view, and to the
/// near side of the camera.
fn screen_share(matrix: &DMat4) -> f64 {
    let extent = f64::from(EXTENT);
    let mut polygon = Polygon::default();
    for (x, y) in [(0.0, 0.0), (extent, 0.0), (extent, extent), (0.0, extent)] {
        polygon.push(*matrix * DVec4::new(x, y, 0.0, 1.0));
    }
    // In front of the camera, then inside each edge of the view: w > 0 and
    // -w <= x, y <= w.
    let planes: [fn(DVec4) -> f64; 5] = [|p| p.w - 1e-9, |p| p.w - p.x, |p| p.w + p.x, |p| p.w - p.y, |p| p.w + p.y];
    for plane in planes {
        polygon = polygon.clip(plane);
        if polygon.points().is_empty() {
            return 0.0;
        }
    }
    let points = polygon.points();
    let twice: f64 = points
        .iter()
        .zip(points.iter().cycle().skip(1))
        .map(|(a, b)| (a.x / a.w) * (b.y / b.w) - (b.x / b.w) * (a.y / a.w))
        .sum();
    // The view is 2 by 2 in normalized device coordinates.
    (twice.abs() / 2.0 / 4.0).min(1.0)
}

/// A convex polygon of a tile's square clipped by the view's five planes:
/// each clip adds at most one point to the square's four.
#[derive(Clone, Copy, Default)]
struct Polygon {
    points: [DVec4; 9],
    len: usize,
}

impl Polygon {
    fn points(&self) -> &[DVec4] {
        &self.points[..self.len]
    }

    fn push(&mut self, point: DVec4) {
        self.points[self.len] = point;
        self.len += 1;
    }

    /// The part where `inside` isn't negative (Sutherland-Hodgman).
    fn clip(&self, inside: fn(DVec4) -> f64) -> Polygon {
        let points = self.points();
        let mut clipped = Polygon::default();
        for (i, &point) in points.iter().enumerate() {
            let previous = points[(i + points.len() - 1) % points.len()];
            let (d0, d1) = (inside(previous), inside(point));
            if (d0 >= 0.0) != (d1 >= 0.0) {
                clipped.push(previous + (point - previous) * (d0 / (d0 - d1)));
            }
            if d1 >= 0.0 {
                clipped.push(point);
            }
        }
        clipped
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use feature::{Feature, Identifier, PropertyMap, Value};
    use geo_types::{Geometry, LineString, Point};
    use mln_style::geo::LatLng;
    use mln_style::size::Size;
    use mln_style::time::Instant;

    use super::*;
    use crate::camera::TransformState;
    use crate::map::TerrainOptions;
    use crate::tile_store::TileLodMode;

    fn feature(id: u64, geometry: Geometry<i16>) -> Feature<i16, Arc<PropertyMap>> {
        let mut properties = PropertyMap::new();
        properties.insert("name".to_owned(), Value::String("ab".to_owned()));
        Feature { geometry: Some(geometry), properties: Arc::new(properties), id: Identifier::Uint(id) }
    }

    /// A style of a line layer whose color depends on feature state, and a
    /// symbol layer, over source `s`.
    fn style() -> Style {
        Style::from_json(
            Registry::with_builtins().style(),
            r#"{
                "version": 8,
                "glyphs": "local://glyphs/{fontstack}/{range}.pbf",
                "sources": {"s": {"type": "vector", "tiles": ["local://tiles/{z}-{x}-{y}.mvt"]}},
                "layers": [
                    {"id": "roads", "type": "line", "source": "s", "source-layer": "l",
                     "paint": {"line-color": ["case", ["boolean", ["feature-state", "on"], false], "red", "blue"]}},
                    {"id": "names", "type": "symbol", "source": "s", "source-layer": "l",
                     "layout": {"text-field": ["get", "name"], "text-font": ["Test"]}}
                ]
            }"#,
        )
        .unwrap()
    }

    /// The update of a continuous map of `style` at pixel ratio 1, of
    /// `width` × `height` pixels centered on 0, 0 at `zoom`.
    fn update(style: &Style, zoom: f64, (width, height): (u32, u32)) -> UpdateParameters {
        let mut state = TransformState::default();
        state.set_size(Size::new(width, height));
        state.set_lat_lng_zoom(&LatLng::new(0.0, 0.0).unwrap(), zoom);
        UpdateParameters {
            style_loaded: true,
            mode: MapMode::Continuous,
            pixel_ratio: 1.0,
            time_point: Some(Instant::now()),
            view: state.view().clone(),
            min_zoom: state.min_zoom(),
            max_zoom: state.limits().max_zoom,
            camera_moving: false,
            style: Arc::new(style.clone()),
            font_faces: None,
            sprite_loaded: true,
            feature_states: Arc::default(),
            file_source: None,
            prefetch_zoom_delta: 0,
            still_image_request: false,
            still_image_request_id: 0,
            update_id: 0,
            cross_source_collisions: true,
            fast_pfor_enabled: false,
            terrain: TerrainOptions::default(),
            elevation: None,
            tile_lod_min_radius: 3.0,
            tile_lod_scale: 1.0,
            tile_lod_pitch_threshold: mln_style::math::deg2rad(60.0),
            tile_lod_zoom_shift: 0.0,
            tile_lod_mode: TileLodMode::Default,
            subdivision_granularity: style.get_projection().get_subdivision_granularity(),
        }
    }

    /// A line and a point, both named.
    fn data() -> TileData {
        TileData::Features(Arc::new(vec![
            feature(1, Geometry::LineString(LineString::from(vec![(100, 100), (4000, 4000)]))),
            feature(2, Geometry::Point(Point::new(2000, 1000))),
        ]))
    }

    fn glyphs() -> Vec<u8> {
        let path = std::path::Path::new(&std::env::var_os("CARGO_MANIFEST_DIR").unwrap())
            .join("testdata/test/fixtures/resources/glyphs.pbf");
        std::fs::read(path).unwrap()
    }

    #[test]
    fn lays_out_with_the_glyphs_it_loads() {
        let (style, data, glyphs) = (style(), data(), glyphs());
        let requested = RefCell::new(Vec::new());
        let load = |url: &str| {
            requested.borrow_mut().push(url.to_owned());
            Some(glyphs.clone())
        };

        let layout = TileLayout::new(&style, "s", |_| true, (0, 0, 0), &data, load).unwrap();
        assert_eq!(*requested.borrow(), ["local://glyphs/Test/0-255.pbf"]);
        assert_eq!(layout.run(data.clone()).unwrap().buckets(), 2);

        let lines = TileLayout::new(&style, "s", |type_| type_ == "line", (0, 0, 0), &data, |_| None).unwrap();
        assert_eq!(lines.run(data).unwrap().buckets(), 1);
    }

    #[test]
    fn places_the_symbols_it_indexes() {
        let (style, data, glyphs) = (style(), data(), glyphs());
        let symbols = |zoom| {
            let layout = TileLayout::overscaled(
                &style,
                "s",
                |type_| type_ == "symbol",
                (0, 0, 0),
                zoom,
                &data,
                |_| Some(glyphs.clone()),
            )
            .unwrap();
            layout.run(data.clone()).unwrap()
        };
        let (before, after) = (symbols(0), symbols(1));
        let zoomed_out = SymbolView::new("s", &[&before], update(&style, 0.0, (512, 512))).unwrap();
        let zoomed_in = SymbolView::new("s", &[&after], update(&style, 1.0, (1024, 1024))).unwrap();

        // Unindexed symbols show nowhere.
        assert_eq!(zoomed_in.place(), 0);
        let mut index = SymbolIndex::default();
        assert_eq!(zoomed_out.index(&mut index).unwrap(), 1);
        assert_eq!(zoomed_in.index(&mut index).unwrap(), 1);
        // Indexing a view again adds nothing.
        assert_eq!(zoomed_in.index(&mut index).unwrap(), 0);
        assert_eq!(zoomed_in.place(), 2);
    }

    #[test]
    fn feature_state_rewrites_the_buckets_that_depend_on_it() {
        let (style, data) = (style(), data());
        let layout = TileLayout::new(&style, "s", |type_| type_ == "line", (0, 0, 0), &data, |_| None).unwrap();
        let mut laid_out = layout.run(data).unwrap();
        let mut states = LayerFeatureStates::default();
        assert!(laid_out.result.set_feature_state(&states).is_empty(), "no state, no change");
        let mut on = mln_style::feature::FeatureState::new();
        on.insert("on".to_owned(), Value::Bool(true));
        states.entry("l".to_owned()).or_default().insert("1".to_owned(), on);
        let roads = laid_out.result.layer_render_data.get("roads").unwrap().bucket.id();
        assert_eq!(laid_out.result.set_feature_state(&states), [roads]);
    }

    #[test]
    fn a_layout_crosses_the_codec_as_it_was_laid_out() {
        let (style, data, glyphs) = (style(), data(), glyphs());
        let layout = TileLayout::new(&style, "s", |_| true, (0, 0, 0), &data, |_| Some(glyphs.clone())).unwrap();
        let direct = layout.run(data.clone()).unwrap();
        let mut codec = Codec::new();
        for _ in 0..2 {
            let reply = codec.send(&layout, data.clone()).unwrap().run();
            let crossed = codec.reply(&reply).unwrap();
            assert_eq!(crossed.id, direct.id);
            let comparable = |laid_out: &LaidOut| {
                [
                    crate::worker::worker_codec::fields::encode_buckets(&laid_out.result),
                    laid_out.result.feature_index.encode_grid(),
                ]
            };
            assert_eq!(comparable(&crossed), comparable(&direct));
        }
    }

    /// The matrix that draws a tile's square over the view's `[x0, x1]` by
    /// `[y0, y1]` in normalized device coordinates.
    fn square(x0: f64, x1: f64, y0: f64, y1: f64) -> DMat4 {
        let scale = glam::DVec3::new((x1 - x0) / f64::from(EXTENT), (y1 - y0) / f64::from(EXTENT), 1.0);
        DMat4::from_translation(glam::DVec3::new(x0, y0, 0.0)) * DMat4::from_scale(scale)
    }

    #[test]
    fn a_tile_counts_by_its_area_inside_the_view() {
        let near = |share: f64, expected: f64| (share - expected).abs() < 1e-9;
        assert!(near(screen_share(&square(-1.0, 1.0, -1.0, 1.0)), 1.0));
        assert!(near(screen_share(&square(0.0, 1.0, 0.0, 1.0)), 0.25));
        assert!(near(screen_share(&square(-3.0, 3.0, -3.0, 3.0)), 1.0), "clipped to the view");
        assert!(near(screen_share(&square(0.5, 2.5, -1.0, 1.0)), 0.25), "half of it outside");
        assert!(near(screen_share(&square(2.0, 3.0, 2.0, 3.0)), 0.0), "outside");
        let mut behind = square(-1.0, 1.0, -1.0, 1.0);
        behind.w_axis *= -1.0;
        assert!(near(screen_share(&behind), 0.0), "behind the camera: w is negative");
    }

    #[test]
    fn a_cover_splits_into_ideal_stand_in_and_blank() {
        let tile = |z: u8, x: u32, y: u32| OverscaledTileID::new(z, 0, CanonicalTileID::new(z, x, y));
        // Four tiles of zoom 2 fill the view, each a quarter of it.
        let ideal = [tile(2, 0, 0), tile(2, 1, 0), tile(2, 0, 1), tile(2, 1, 1)];
        let area = |id: &UnwrappedTileID| 0.25 / f64::from(1u32 << (2 * (id.canonical.z - 2)));
        let drawn = |id: OverscaledTileID| (id.to_unwrapped(), id);
        let rendered = [
            // The first, at its own zoom.
            drawn(tile(2, 0, 0)),
            // The second's parent, also the third's and the fourth's.
            drawn(tile(1, 0, 0)),
        ];
        let coverage = cover_coverage(&ideal, &rendered, area);
        assert_eq!(coverage, Coverage { ideal: 0.25, stand_in: 0.75, blank: 0.0 });

        // Without the parent: two of the fourth's children stand in for
        // half of it, and the second and third are blank.
        let rendered = [drawn(tile(2, 0, 0)), drawn(tile(3, 2, 2)), drawn(tile(3, 3, 3))];
        let coverage = cover_coverage(&ideal, &rendered, area);
        assert_eq!(coverage, Coverage { ideal: 0.25, stand_in: 0.125, blank: 0.625 });

        // A tile of the right place drawn at another zoom, as an overscaled
        // tile is while its own loads, stands in.
        let overscaled = OverscaledTileID::new(3, 0, CanonicalTileID::new(2, 0, 0));
        let coverage = cover_coverage(&ideal[..1], &[drawn(overscaled)], area);
        assert_eq!(coverage, Coverage { ideal: 0.0, stand_in: 0.25, blank: 0.0 });
    }
}
