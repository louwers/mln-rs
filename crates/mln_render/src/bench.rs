//! Entry points to the renderer's internal stages, as functions over data,
//! for the benchmarks (`tools/bench`) to measure. Built only with the
//! `bench` feature, and hidden from the documentation: they aren't API.
//!
//! Each entry point takes and returns public types only, so that the
//! stages' internals can change without changing what the benchmarks call.

use std::collections::BTreeSet;
use std::sync::Arc;

use mln_map_types::{MapDebugOptions, MapMode, TileLodMode, TransformState};
use mln_storage::Resource;
use mln_style::Style;
use mln_style::layer_type::{LayerStyle, StyleLayerType};
use mln_style::renderer::property_evaluation_parameters::PropertyEvaluationParameters;
use mln_style_types::VisibilityType;
use mln_style_types::image::{ImageMap, ImageType};
use mln_text::glyph::{GlyphMap, get_glyph_range};
use mln_text::glyph_pbf::parse_glyph_pbf;
use mln_text::tagged_string::font_stack_hash;
use mln_tile::TileData;
use mln_tile_id::{CanonicalTileID, OverscaledTileID};
use mln_util::error::Error;
use mln_util::geo::LatLng;
use mln_util::size::Size;
use mln_util::time::Instant;
use mln_vec_map::VecSet;

use crate::Registry;
use crate::bucket::{ErasedBucket, LayerFeatureStates};
use crate::erased_layer::{ErasedEvaluatedLayer, NoPlacement};
use crate::gfx::context::Resources;
use crate::gfx::dynamic_texture_atlas::DynamicTextureAtlas;
use crate::layer::Layer;
use crate::layout::tile_layout::{Dependencies, LayoutParams, Resolved};
use crate::paint_parameters::TransformParameters;
use crate::placement::{BucketKey, LaidBuckets, PlacementBuckets, PlacementContext, PlacingLayer, PlacingTile};
use crate::render_layer::{Hosted, RenderLayer as _};
use crate::render_tile::RenderTile;
use crate::text::glyph_store::ShapeResults;
use crate::tile::contents::LayoutResult;
use crate::tile::job::{
    DataInput, DataKey, Job, JobId, JobResult, JobTimes, LayoutJob, Reply, Run, SourceKey, SourceStyle, TileKey,
};
use crate::tile::tile_workers::InlineWorkers;
use crate::tile::worker_codec::{Encoded, MapCodec, WorkerCodec, read_job_id, read_reply_header};
use crate::update_parameters::UpdateParameters;

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
    /// evaluated at the tile's zoom as the renderer evaluates them with the
    /// layer types of `registry`, for a continuous map at pixel ratio 1.
    ///
    /// It lays `data` out once to find what the layout needs, and resolves
    /// that as the renderer would: glyph ranges from the style's glyph URL,
    /// whose bytes `load` gives, and images from the style's images, packed
    /// into an atlas. Fails for a style without the source, a layer whose
    /// type `registry` lacks, or a layout that fails.
    pub fn new(
        registry: &Registry,
        style: &Style,
        source: &str,
        include: impl Fn(&str) -> bool,
        (z, x, y): (u8, u32, u32),
        data: &TileData,
        load: impl Fn(&str) -> Option<Vec<u8>>,
    ) -> Result<TileLayout, Error> {
        TileLayout::overscaled(registry, style, source, include, (z, x, y), z, data, load)
    }

    /// [`TileLayout::new`] for the tile shown at `overscaled_z`, a zoom
    /// at or above its own, as a map shows the tiles of its source's highest
    /// zoom when it zooms in further: the layers evaluated at that zoom, and
    /// the tile's features scaled to it.
    #[expect(clippy::too_many_arguments, reason = "the arguments of `new`, and the zoom")]
    pub fn overscaled(
        registry: &Registry,
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
        let parameters = PropertyEvaluationParameters::new(f32::from(overscaled_z));
        let mut layers: Vec<Arc<dyn ErasedEvaluatedLayer>> = Vec::new();
        for layer in style.get_layers() {
            let base = layer.style().base();
            if base.source != source || base.visibility == VisibilityType::None || !include(layer.info().name) {
                continue;
            }
            let mut render_layer = registry
                .layer_type(layer.info().name)
                .and_then(|layer_type| layer_type.create_render_layer(layer.style(), &mut Resources::detached()))
                .ok_or_else(|| Error::invalid_argument(format!("no render layer for {}", layer.get_id())))?;
            render_layer.evaluate(&parameters);
            layers.push(render_layer.evaluated());
        }

        let mut layout = TileLayout {
            id: OverscaledTileID::new(overscaled_z, 0, CanonicalTileID::new(z, x, y)),
            style: Arc::new(SourceStyle {
                version: 0,
                layers,
                global_state: Some(Arc::clone(style.get_global_state())),
                font_faces: style.get_font_faces().cloned(),
                glyph_pbfs: !style.get_glyph_url().is_empty(),
            }),
            available_images: Arc::new(style.get_images().iter().map(|image| image.id.clone()).collect()),
            params: LayoutParams {
                mode: MapMode::Continuous,
                pixel_ratio: 1.0,
                show_collision_boxes: false,
                cpu_rasterizer: false,
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

    /// The tile's id, and the wire forms of its buckets and its feature
    /// index's grid, which are equal for equal layouts. Fails for a bucket
    /// without a wire form.
    pub fn encoded(&self) -> Result<(OverscaledTileID, [Vec<u8>; 2]), Error> {
        let buckets = crate::tile::worker_codec::fields::encode_buckets(&self.result).map_err(Error::native)?;
        let grid = self.result.feature_index.encode_grid().map_err(Error::native)?;
        Ok((self.id, [buckets, grid]))
    }
}

/// The source key of the tiles that a [`PlacementFrame`] shows.
const FRAME_SOURCE: SourceKey = SourceKey(0);

/// Laid-out tiles in a view, as a renderer's frame shows them to its
/// placement pass for the layers `L`: the style's visible layers of `L`'s
/// type on a source, each evaluated at the view's zoom with its tiles that
/// hold a bucket of it, in the order it places them, and the tiles'
/// buckets, which placement reads and changes.
pub struct PlacementFrame<L: Layer> {
    update: UpdateParameters,
    /// Each tile's id and buckets.
    tiles: Vec<(OverscaledTileID, LayoutResult)>,
    /// The layers, bottom to top, each with its tiles.
    layers: Vec<(Hosted<L>, Vec<PlacingTile>)>,
}

impl<L: Layer> PlacementFrame<L> {
    /// A view of `size` pixels centered on `center` at `zoom`, of copies of
    /// `tiles` laid out by the visible layers of `L`'s type on `source` in
    /// `style`, for a continuous map at pixel ratio 1. Fails for a layer of
    /// the type's name that isn't of the type.
    pub fn new(
        style: &Style,
        source: &str,
        tiles: &[&LaidOut],
        center: LatLng,
        zoom: f64,
        (width, height): (u32, u32),
    ) -> Result<PlacementFrame<L>, Error> {
        let mut state = TransformState::default();
        state.set_size(Size::new(width, height));
        state.set_lat_lng_zoom(&center, zoom);
        let transform = TransformParameters::new(state.view());
        let render_tiles: Vec<RenderTile> = tiles
            .iter()
            .map(|laid_out| {
                let mut render_tile = RenderTile::of_tile_id(laid_out.id.to_unwrapped(), laid_out.id, false);
                render_tile.set_projection(&transform);
                render_tile
            })
            .collect();
        let tiles: Vec<(OverscaledTileID, LayoutResult)> = tiles
            .iter()
            .map(|laid_out| {
                let buckets = laid_out.result.layer_render_data.copy_where(|_| true);
                (laid_out.id, LayoutResult::new(buckets, Arc::clone(&laid_out.result.feature_index)))
            })
            .collect();
        let order = crate::placement::tile_order(&render_tiles, state.view().angle() as f32);

        let update = UpdateParameters {
            style_loaded: true,
            mode: MapMode::Continuous,
            pixel_ratio: 1.0,
            debug_options: MapDebugOptions::NO_DEBUG,
            time_point: Some(Instant::now()),
            view: state.view().clone(),
            min_zoom: state.min_zoom(),
            max_zoom: state.limits().max_zoom,
            camera_moving: false,
            style: Arc::new(style.clone()),
            sprite_loaded: true,
            feature_states: Arc::default(),
            file_source: None,
            prefetch_zoom_delta: 0,
            still_image_request: false,
            still_image_request_id: 0,
            cross_source_collisions: true,
            fast_pfor_enabled: false,
            tile_lod_min_radius: 3.0,
            tile_lod_scale: 1.0,
            tile_lod_pitch_threshold: mln_math::deg2rad(60.0),
            tile_lod_zoom_shift: 0.0,
            tile_lod_mode: TileLodMode::Default,
            subdivision_granularity: style.get_projection().get_subdivision_granularity(),
        };

        let parameters = PropertyEvaluationParameters::new(zoom as f32);
        let mut layers = Vec::new();
        for layer in style.get_layers() {
            let base = layer.style().base();
            let name = <L::Style as StyleLayerType>::INFO.name;
            if base.source != source || base.visibility == VisibilityType::None || layer.info().name != name {
                continue;
            }
            let style = LayerStyle::<L::Style>::recover(layer.style())
                .ok_or_else(|| Error::invalid_argument(format!("no render layer for {}", layer.get_id())))?;
            let entry = crate::erased_layer::entry::<L, NoPlacement>();
            let mut render_layer = Hosted::<L>::new(entry, style, &mut Resources::detached());
            render_layer.evaluate(&parameters);
            let placing = order
                .iter()
                .filter_map(|&index| {
                    let (render_tile, (id, result)) = (&render_tiles[index], &tiles[index]);
                    let laid = result.layer_render_data.find(layer.get_id())?;
                    Some(PlacingTile {
                        bucket: BucketKey {
                            bucket: result.layer_render_data.bucket_index(laid)?,
                            slot: None,
                            tile: *id,
                            source: FRAME_SOURCE,
                        },
                        id: render_tile.id,
                        overscaled_id: *id,
                        held_for_fade: false,
                        projection: render_tile.projection,
                        feature_index: Arc::clone(&result.feature_index),
                    })
                })
                .collect();
            layers.push((render_layer, placing));
        }
        Ok(PlacementFrame { update, tiles, layers })
    }

    /// Runs `hook` with the frame's placement context: its layers, its
    /// update and its tiles' buckets, as a renderer runs its placement pass.
    pub fn with_context<R>(&mut self, hook: impl FnOnce(&mut PlacementContext<'_, L>) -> R) -> R {
        let layers: Vec<PlacingLayer<'_, L>> =
            self.layers.iter().map(|(layer, tiles)| PlacingLayer { layer: layer.parts().1, tiles }).collect();
        hook(&mut PlacementContext {
            layers: &layers,
            update_parameters: &self.update,
            buckets: PlacementBuckets::new(&mut FrameBuckets(&mut self.tiles)),
        })
    }
}

/// The buckets of a [`PlacementFrame`]'s tiles, by their keys.
struct FrameBuckets<'a>(&'a mut [(OverscaledTileID, LayoutResult)]);

impl FrameBuckets<'_> {
    /// The buckets of the tile that holds `key`'s bucket.
    fn tile(&self, key: &BucketKey) -> Option<usize> {
        (key.source == FRAME_SOURCE).then(|| self.0.iter().position(|(id, _)| *id == key.tile)).flatten()
    }
}

impl LaidBuckets for FrameBuckets<'_> {
    fn bucket(&self, key: &BucketKey) -> Option<&dyn ErasedBucket> {
        self.0.get(self.tile(key)?)?.1.layer_render_data.bucket(key.bucket)
    }

    fn bucket_mut(&mut self, key: &BucketKey) -> Option<&mut dyn ErasedBucket> {
        let tile = self.tile(key)?;
        self.0.get_mut(tile)?.1.layer_render_data.bucket_mut(key.bucket)
    }
}

/// A worker instance's codec and the map side's, between which a tile's
/// job and its reply cross as frames of bytes
/// (`tile::worker_codec`), as in the browser build.
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
    /// The two sides of a new worker instance, with `registry`.
    #[must_use]
    pub fn new(registry: Arc<Registry>) -> Codec {
        Codec { map: MapCodec::new(Arc::clone(&registry)), worker: WorkerCodec::new(registry), generation: 0 }
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

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use feature::{Feature, Identifier, PropertyMap, Value};
    use geo_types::{Geometry, LineString, Point};
    use mln_style::edit::Edit;
    use mln_util::tileset::Tileset;

    use super::*;
    use crate::tests::fixture_source::{self, Geometry as Features};

    fn feature(id: u64, geometry: Geometry<i16>) -> Feature<i16, Arc<PropertyMap>> {
        let mut properties = PropertyMap::new();
        properties.insert("name".to_owned(), Value::String("ab".to_owned()));
        Feature { geometry: Some(geometry), properties: Arc::new(properties), id: Identifier::Uint(id) }
    }

    /// A style of a `dot` layer whose color depends on feature state, and
    /// one of labels, over the fixture source `s`.
    fn style() -> Style {
        let mut style = Style::from_json(
            Registry::with_builtins().style(),
            r#"{"version": 8, "glyphs": "local://glyphs/{fontstack}/{range}.pbf", "sources": {}, "layers": []}"#,
            None,
        )
        .unwrap();
        let tileset = Tileset { tiles: vec!["local://tiles/{z}-{x}-{y}.mvt".to_owned()], ..Default::default() };
        style.apply(Edit::AddSource(fixture_source::source::<Features>("s", tileset, 512))).unwrap();
        style
            .add_layer_json(
                r#"{"id": "roads", "type": "dot", "source": "s", "source-layer": "l",
                    "paint": {"dot-color": ["case", ["boolean", ["feature-state", "on"], false], "red", "blue"]}}"#,
                None,
            )
            .unwrap();
        style
            .add_layer_json(
                r#"{"id": "names", "type": "dot", "source": "s", "source-layer": "l",
                    "layout": {"dot-label": ["get", "name"], "dot-font": ["Test"]}}"#,
                None,
            )
            .unwrap();
        style
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

        let layout =
            TileLayout::new(&Registry::with_builtins(), &style, "s", |_| true, (0, 0, 0), &data, load).unwrap();
        assert_eq!(*requested.borrow(), ["local://glyphs/Test/0-255.pbf"]);
        assert_eq!(layout.run(data).unwrap().buckets(), 2);
    }

    #[test]
    fn a_layout_crosses_the_codec_as_it_was_laid_out() {
        let (style, data, glyphs) = (style(), data(), glyphs());
        let layout = TileLayout::new(
            &Registry::with_builtins(),
            &style,
            "s",
            |_| true,
            (0, 0, 0),
            &data,
            |_| Some(glyphs.clone()),
        )
        .unwrap();
        let direct = layout.run(data.clone()).unwrap();
        let mut codec = Codec::new(Registry::with_builtins());
        for _ in 0..2 {
            let reply = codec.send(&layout, data.clone()).unwrap().run();
            let crossed = codec.reply(&reply).unwrap();
            assert_eq!(crossed.encoded().unwrap(), direct.encoded().unwrap());
        }
    }
}
