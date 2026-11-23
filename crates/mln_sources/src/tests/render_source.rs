//! The cases of test/style/source.test.cpp for the built-in source types:
//! their render sources, whose tiles cover a view, and the TileJSON that
//! describes a tileset source.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use geojson::GeoJson;
use mln_map_types::{ConstrainMode, MapMode, TransformState, ViewportMode};
use mln_render::testing::{Event, OneSource, View};
use mln_storage::response::Reason;
use mln_storage::stub_file_source::{ResponseType, StubFileSource};
use mln_storage::{FileSource, Kind, ResourceError, Response};
use mln_style::source_type::{Load, SourceData};
use mln_style::{Edit, Layer, Source, Style};
use mln_tile_id::{CanonicalTileID, OverscaledTileID};
use mln_util::error::Error;
use mln_util::geo::LatLng;
use mln_util::pool::{Pool, TaskGroup};
use mln_util::size::Size;
use mln_util::tileset::{PartialTileset, Tileset};

use super::{registry, style_registry};
use crate::custom::{geometry, vector};
use crate::geojson::{GeoJSONOptions, GeoJsonSourceType, create_geojson_data, geojson_source};
use crate::{
    CustomGeometrySourceHandle, CustomVectorSourceHandle, TileDataFormat, VectorSourceType, raster_dem_source,
    raster_source, vector_source,
};

/// A 512 by 512 view at `zoom` over the center of the world.
fn view_state(zoom: f64) -> TransformState {
    let mut state = TransformState::new(ConstrainMode::HeightOnly, ViewportMode::Default);
    state.set_size(Size::new(512, 512));
    state.set_lat_lng_zoom(&LatLng::default(), zoom);
    state
}

/// What a render source updates for: `state` in `mode`, with `file_source`.
fn view(state: &TransformState, mode: MapMode, file_source: Option<Arc<dyn FileSource>>) -> View<'_> {
    View { state, mode, file_source, needs_relayout: true }
}

/// A pool for the tiles' work, which the test waits for as it ends.
struct Workers(TaskGroup);

impl Workers {
    fn new() -> Self {
        Workers(TaskGroup::new(&Pool::new(4)))
    }
}

impl Drop for Workers {
    fn drop(&mut self) {
        self.0.wait();
    }
}

/// A layer `id` of the type `type_` on the source "source", reading its
/// source layer "water".
fn layer(type_: &str) -> Layer {
    let mut layer = Layer::new(registry().style(), type_, "id", "source").unwrap();
    layer.set_source_layer("water");
    layer
}

/// `source` after `edit`, made through a style that holds it.
fn edited(source: &Source, edit: Edit) -> Source {
    let mut style = Style::new(style_registry());
    style.apply(Edit::AddSource(source.clone())).unwrap();
    style.apply(edit).unwrap();
    style.get_source(source.id()).unwrap().clone()
}

/// `source` with the description that its type knows without a fetch, such
/// as the tileset a style lists, as the map's loader gives it.
fn ready(source: Source) -> Source {
    match source.data().plan_load() {
        Load::Ready(description) => {
            source.with_description(Some(description)).expect("a description of the source's type")
        }
        _ => source,
    }
}

/// Steps `source`'s tiles until `done` is true of an event they report.
///
/// # Panics
///
/// If no event does within 30 seconds.
fn run_until(source: &mut OneSource, mut done: impl FnMut(Event) -> bool) {
    let deadline = mln_util::time::Instant::now() + Duration::from_secs(30);
    loop {
        source.step();
        if source.take_events().into_iter().any(&mut done) {
            return;
        }
        assert!(mln_util::time::Instant::now() < deadline, "the test didn't end");
        std::thread::yield_now();
    }
}

/// A function that makes a raster or raster-dem source: its id, the URL of
/// its TileJSON, its inline tileset and its tile size.
type RasterSource = fn(&str, Option<String>, PartialTileset, u16) -> Source;

/// What a TileJSON with `attribution` gives a tileset source of the kind
/// that `source` makes: the attribution, and new metadata.
fn tile_attribution(source: RasterSource, attribution: &str) {
    let source = source("source", Some("url".to_owned()), PartialTileset::default(), 512);

    // The map's loader fetches the TileJSON and describes it.
    let Load::Fetch(resource) = source.data().plan_load() else { panic!("a TileJSON") };
    assert_eq!("url", resource.url);
    let tilejson = format!(r#"{{ "tilejson": "2.1.0", "attribution": "{attribution}", "tiles": [ "tiles" ] }}"#);
    let (sender, receiver) = std::sync::mpsc::channel();
    source.data().describe_job(Some(Arc::new(tilejson.into_bytes())))(Box::new(move |result| {
        let _ = sender.send(result);
    }));
    let described = source.with_description(receiver.recv().unwrap().unwrap()).expect("a description");
    assert_eq!(Some(attribution.to_owned()), described.data().attribution());
    assert!(described.metadata_changed_from(&source));
}

// gtest: Source.RasterTileAttribution
#[test]
fn raster_tile_attribution() {
    let mapbox_osm = "<a href='https://www.mapbox.com/about/maps/' target='_blank'>&copy; Mapbox</a> \
                      <a href='http://www.openstreetmap.org/about/' target='_blank'>©️ OpenStreetMap</a>";
    assert!(mapbox_osm.contains("©️ OpenStreetMap"));
    tile_attribution(raster_source, mapbox_osm);
}

// gtest: Source.RasterDEMTileAttribution
#[test]
fn raster_dem_tile_attribution() {
    let mapbox = "<a href='https://www.mapbox.com/about/maps/' target='_blank'>&copy; Mapbox</a> ";
    tile_attribution(raster_dem_source, mapbox);
}

/// The three kinds of tileset sources.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Tiled {
    Raster,
    RasterDem,
    Vector,
}

/// The render source of a tileset source "source" of the kind `kind`, whose
/// tiles `file_source` answers, drawn by a layer of the type that draws such
/// tiles (raster, hillshade or circle), after an update over `state`.
fn tile_case(kind: Tiled, workers: &Workers, state: &TransformState, file_source: StubFileSource) -> OneSource {
    let tileset = PartialTileset { tiles: Some(vec!["tiles".to_owned()]), ..Default::default() };
    let (source, layer) = match kind {
        Tiled::Raster => (
            raster_source("source", None, tileset, 512),
            Layer::new(registry().style(), "raster", "id", "source").unwrap(),
        ),
        Tiled::RasterDem => (
            raster_dem_source("source", None, tileset, 512),
            Layer::new(registry().style(), "hillshade", "id", "source").unwrap(),
        ),
        Tiled::Vector => (vector_source("source", None, tileset), layer("circle")),
    };
    let source = ready(source);
    let mut render_source = OneSource::new(&registry(), &source, &[layer], &workers.0).unwrap();
    let file_source: Arc<dyn FileSource> = Arc::new(file_source);
    render_source.update(&source, &view(state, MapMode::Continuous, Some(file_source)));
    render_source
}

/// A file source that answers each tile's request at once with what
/// `response` makes.
fn answering(response: impl Fn() -> Option<Response> + Send + Sync + 'static) -> StubFileSource {
    let file_source = StubFileSource::new(ResponseType::Synchronous);
    file_source.set_response(Kind::Tile, move |_| response());
    file_source
}

/// A tile whose response has no content changes, empty, without an error.
fn tile_empty(kind: Tiled) {
    let (workers, state) = (Workers::new(), view_state(0.0));
    let file_source = answering(|| {
        let mut response = Response::new();
        response.no_content = true;
        Some(response)
    });
    let mut render_source = tile_case(kind, &workers, &state, file_source);
    run_until(&mut render_source, |event| match event {
        Event::Changed => true,
        Event::Error { error, .. } => panic!("{error}"),
        Event::Action(_) => false,
    });
}

/// A tile whose request fails reports the failure as its error.
fn tile_fail(kind: Tiled) {
    let (workers, state) = (Workers::new(), view_state(0.0));
    let file_source = answering(|| {
        let mut response = Response::new();
        response.error = Some(Box::new(ResourceError::new(Reason::Other, "Failed by the test case")));
        Some(response)
    });
    let mut render_source = tile_case(kind, &workers, &state, file_source);
    run_until(&mut render_source, |event| match event {
        Event::Error { tile, error } => {
            assert_eq!(OverscaledTileID::from_zxy(0, 0, 0), tile);
            assert_eq!("Failed by the test case", error.to_string());
            true
        }
        Event::Changed | Event::Action(_) => false,
    });
}

/// A tile whose data doesn't decode reports why as its error.
fn tile_corrupt(kind: Tiled) {
    let (workers, state) = (Workers::new(), view_state(0.0));
    let file_source = answering(|| {
        let mut response = Response::new();
        response.data = Some(Arc::new(b"CORRUPTED".to_vec()));
        Some(response)
    });
    let mut render_source = tile_case(kind, &workers, &state, file_source);
    run_until(&mut render_source, |event| match event {
        Event::Error { tile, error } => {
            assert_eq!(OverscaledTileID::from_zxy(0, 0, 0), tile);
            if kind == Tiled::Vector {
                assert_eq!(error.to_string(), "protobuf decode error: invalid wire type: 7");
            } else {
                // Not asserting on platform-specific error text.
                assert_ne!(error.to_string(), "");
            }
            true
        }
        Event::Changed | Event::Action(_) => false,
    });
}

/// A tile whose request isn't answered neither changes nor fails.
fn tile_cancel(kind: Tiled) {
    let (workers, state) = (Workers::new(), view_state(0.0));
    let requested = Arc::new(AtomicBool::new(false));
    let asked = Arc::clone(&requested);
    let file_source = answering(move || {
        asked.store(true, Ordering::SeqCst);
        None
    });
    let mut render_source = tile_case(kind, &workers, &state, file_source);
    render_source.step();
    assert!(requested.load(Ordering::SeqCst));
    for event in render_source.take_events() {
        assert!(matches!(event, Event::Action(_)), "{event:?}");
    }
}

// gtest: Source.RasterTileEmpty
#[test]
fn raster_tile_empty() {
    tile_empty(Tiled::Raster);
}

// gtest: Source.RasterDEMTileEmpty
#[test]
fn raster_dem_tile_empty() {
    tile_empty(Tiled::RasterDem);
}

// gtest: Source.VectorTileEmpty
#[test]
fn vector_tile_empty() {
    tile_empty(Tiled::Vector);
}

// gtest: Source.RasterTileFail
#[test]
fn raster_tile_fail() {
    tile_fail(Tiled::Raster);
}

// gtest: Source.RasterDEMTileFail
#[test]
fn raster_dem_tile_fail() {
    tile_fail(Tiled::RasterDem);
}

// gtest: Source.VectorTileFail
#[test]
fn vector_tile_fail() {
    tile_fail(Tiled::Vector);
}

// gtest: Source.RasterTileCorrupt
#[test]
fn raster_tile_corrupt() {
    tile_corrupt(Tiled::Raster);
}

// gtest: Source.RasterDEMTileCorrupt
#[test]
fn raster_dem_tile_corrupt() {
    tile_corrupt(Tiled::RasterDem);
}

// gtest: Source.VectorTileCorrupt
#[test]
fn vector_tile_corrupt() {
    tile_corrupt(Tiled::Vector);
}

// gtest: Source.RasterTileCancel
#[test]
fn raster_tile_cancel() {
    tile_cancel(Tiled::Raster);
}

// gtest: Source.RasterDEMTileCancel
#[test]
fn raster_dem_tile_cancel() {
    tile_cancel(Tiled::RasterDem);
}

// gtest: Source.VectorTileCancel
#[test]
fn vector_tile_cancel() {
    tile_cancel(Tiled::Vector);
}

/// The loaded custom geometry source "source", whose provider runs on
/// `workers`, and its render source after one update.
fn custom_geometry_case(workers: &Workers, state: &TransformState) -> (Source, OneSource) {
    let source = geometry::create("source", geometry::Options::default(), workers.0.pool()).unwrap();
    let mut render_source = OneSource::new(&registry(), &source, &[layer("circle")], &workers.0).unwrap();
    render_source.update(&source, &view(state, MapMode::Continuous, None));
    (source, render_source)
}

// gtest: Source.CustomGeometrySourceSetTileData
#[test]
fn custom_geometry_source_set_tile_data() {
    let (workers, state) = (Workers::new(), view_state(0.0));
    let (source, mut render_source) = custom_geometry_case(&workers, &state);

    let handle = CustomGeometrySourceHandle::of(&source).unwrap();
    handle.set_tile_data(CanonicalTileID::new(0, 0, 0), GeoJson::from(Vec::<geojson::Feature>::new()));

    run_until(&mut render_source, |event| match event {
        Event::Changed => true,
        Event::Error { error, .. } => panic!("{error}"),
        Event::Action(_) => false,
    });
}

// gtest: Source.CustomGeometrySourceSetTileError
#[test]
fn custom_geometry_source_set_tile_error() {
    let (workers, state) = (Workers::new(), view_state(0.0));
    let (source, mut render_source) = custom_geometry_case(&workers, &state);

    let handle = CustomGeometrySourceHandle::of(&source).unwrap();
    handle.set_tile_error(CanonicalTileID::new(0, 0, 0), Error::native("failed"));

    run_until(&mut render_source, |event| match event {
        Event::Error { tile, error } => {
            assert_eq!(OverscaledTileID::from_zxy(0, 0, 0), tile);
            assert_eq!("failed", error.to_string());
            true
        }
        Event::Changed | Event::Action(_) => false,
    });
}

// Port-only: Source.CustomGeometrySourceSetTileData for a custom vector
// source.
#[test]
fn custom_vector_source_set_tile_data() {
    let (workers, state) = (Workers::new(), view_state(0.0));
    let source = vector::create("source", vector::Options::default(), workers.0.pool());
    let mut render_source = OneSource::new(&registry(), &source, &[layer("circle")], &workers.0).unwrap();
    render_source.update(&source, &view(&state, MapMode::Continuous, None));

    let handle = CustomVectorSourceHandle::of(&source).unwrap();
    handle.set_tile_data(CanonicalTileID::new(0, 0, 0), None, TileDataFormat::Mvt);

    run_until(&mut render_source, |event| match event {
        Event::Changed => true,
        Event::Error { error, .. } => panic!("{error}"),
        Event::Action(_) => false,
    });
}

/// `source`, a vector source with a loaded tileset, with `tiles` as that
/// tileset's tile URLs.
fn with_tiles(source: &Source, tiles: &[&str]) -> Source {
    let data = SourceData::<VectorSourceType>::recover(source).expect("a vector source");
    let tileset = data.description.clone().expect("a loaded tileset");
    let tileset = Tileset { tiles: tiles.iter().map(|&tile| tile.to_owned()).collect(), ..tileset };
    source.with_data(SourceData::<VectorSourceType> { description: Some(tileset), ..data.clone() })
}

/// Counts the tile errors that `source` reported.
fn tile_errors(source: &mut OneSource) -> usize {
    source.take_events().into_iter().filter(|event| matches!(event, Event::Error { .. })).count()
}

/// A file source that never answers.
fn unanswered() -> Arc<dyn FileSource> {
    Arc::new(StubFileSource::new(ResponseType::Manual))
}

// gtest: Source.EmptyTileset
#[test]
fn empty_tileset() {
    let (workers, state) = (Workers::new(), view_state(0.0));
    let mut source = ready(vector_source("source", None, PartialTileset::default()));
    let mut render_source = OneSource::new(&registry(), &source, &[layer("circle")], &workers.0).unwrap();
    let mut view = view(&state, MapMode::Continuous, Some(unanswered()));

    render_source.update(&source, &view);
    assert!(render_source.is_loaded());

    view.needs_relayout = false;
    source = with_tiles(&source, &["tiles"]);
    render_source.update(&source, &view);
    assert!(!render_source.is_loaded());

    source = with_tiles(&source, &[]);
    render_source.update(&source, &view);
    assert!(render_source.is_loaded());

    source = with_tiles(&source, &["tiles"]);
    render_source.update(&source, &view);
    assert!(!render_source.is_loaded());

    assert_eq!(0, tile_errors(&mut render_source));
}

// As Source.EmptyTileset, for the raster and raster-dem render sources.
#[test]
fn empty_raster_tilesets_make_no_tiles() {
    let raster: [(RasterSource, &str); 2] = [(raster_source, "raster"), (raster_dem_source, "hillshade")];
    for (make, layer_type) in raster {
        let (workers, state) = (Workers::new(), view_state(0.0));
        let source = ready(make("source", None, PartialTileset::default(), 512));
        let layer = Layer::new(registry().style(), layer_type, "id", "source").unwrap();
        let mut render_source = OneSource::new(&registry(), &source, &[layer], &workers.0).unwrap();
        render_source.update(&source, &view(&state, MapMode::Continuous, Some(unanswered())));

        assert!(render_source.is_loaded());
        assert_eq!(0, tile_errors(&mut render_source));
    }
}

// gtest: Source.GeoJSONSourceTilesAfterDataReset
#[test]
fn geojson_source_tiles_after_data_reset() {
    let (workers, state) = (Workers::new(), view_state(0.0));
    let mut source = geojson_source("source", GeoJSONOptions::default_options());
    let geojson = r#"{"geometry": {"type": "Point", "coordinates": [1.1, 1.1]}, "type": "Feature", "properties": {}}"#
        .parse::<GeoJson>()
        .unwrap();
    let geojson_data = create_geojson_data(geojson, &GeoJSONOptions::default_options()).unwrap();
    source = edited(&source, GeoJsonSourceType::set_data("source", Some(Arc::clone(&geojson_data))));
    let mut render_source = OneSource::new(&registry(), &source, &[layer("circle")], &workers.0).unwrap();

    let continuous = view(&state, MapMode::Continuous, None);
    render_source.update(&source, &continuous);
    // loaded == false, means that the source contains pending tiles.
    assert!(!render_source.is_loaded());

    source = edited(&source, GeoJsonSourceType::set_data("source", None));
    render_source.update(&source, &continuous);
    assert!(!render_source.is_loaded()); // Tiles remain in continuous mode.

    source = edited(&source, GeoJsonSourceType::set_data("source", Some(geojson_data)));
    render_source.update(&source, &continuous);
    assert!(!render_source.is_loaded());

    source = edited(&source, GeoJsonSourceType::set_data("source", None));
    render_source.update(&source, &view(&state, MapMode::Static, None));
    assert!(render_source.is_loaded()); // Tiles are reset in static mode.
}

/// A GeoJSON source rendered with a circle layer. Its data is clustered,
/// which slices a tile within the call in either mode, so whether the tiles
/// complete within an update shows only how they lay out.
struct SynchronousUpdateSource {
    options: Arc<GeoJSONOptions>,
    source: Source,
    render_source: OneSource,
}

impl SynchronousUpdateSource {
    fn new(workers: &Workers, synchronous_update: bool) -> Self {
        let options: Arc<GeoJSONOptions> =
            Arc::new(GeoJSONOptions { cluster: true, synchronous_update, ..Default::default() });
        let source = geojson_source("source", Arc::clone(&options));
        let render_source = OneSource::new(&registry(), &source, &[layer("circle")], &workers.0).unwrap();
        SynchronousUpdateSource { options, source, render_source }
    }

    /// Sets new data and updates the render source, as a frame would.
    fn update(&mut self, state: &TransformState) {
        let geojson = r#"{"type": "FeatureCollection", "features": [{"type": "Feature", "properties": {},
                "geometry": {"type": "Point", "coordinates": [1.1, 1.1]}}]}"#
            .parse::<GeoJson>()
            .unwrap();
        let data = Some(create_geojson_data(geojson, &self.options).unwrap());
        self.source = edited(&self.source, GeoJsonSourceType::set_data("source", data));
        self.render_source.update(&self.source, &view(state, MapMode::Continuous, None));
    }
}

/// The edit that sets the synchronous update of the GeoJSON source "source".
fn synchronous_update(synchronous: bool) -> Edit {
    GeoJsonSourceType::set_synchronous("source", synchronous)
}

// gtest: Source.GeoJSONSourceSynchronousUpdateOverrideAppliesToExistingTiles
#[test]
fn geojson_source_synchronous_update_override_applies_to_existing_tiles() {
    let (workers, state) = (Workers::new(), view_state(0.0));

    let mut synchronous = SynchronousUpdateSource::new(&workers, true);
    synchronous.update(&state);
    assert!(synchronous.render_source.is_loaded()); // The tiles complete within the update.
    synchronous.source = edited(&synchronous.source, synchronous_update(false));
    synchronous.update(&state);
    assert!(!synchronous.render_source.is_loaded()); // The tiles now lay out in the background.

    let mut asynchronous = SynchronousUpdateSource::new(&workers, false);
    asynchronous.update(&state);
    assert!(!asynchronous.render_source.is_loaded());
    while !asynchronous.render_source.is_loaded() {
        asynchronous.render_source.step();
        std::thread::yield_now();
    }
    asynchronous.source = edited(&asynchronous.source, synchronous_update(true));
    asynchronous.update(&state);
    assert!(asynchronous.render_source.is_loaded()); // The tiles now complete within the update.
}

// gtest: Source.GeoJSONSourceSlicesInlineWhileABackgroundSliceRuns
#[test]
fn geojson_source_slices_inline_while_a_background_slice_runs() {
    // At zoom 3 the index slices its tiles on demand, which fills its tile cache.
    let state = view_state(3.0);

    // Unclustered data slices on the store's pool while the source is
    // asynchronous: a pool that holds its tasks until the test runs them.
    let pool = Pool::local();
    let queued = Arc::new(AtomicBool::new(false));
    let woken = Arc::clone(&queued);
    pool.set_waker(Some(Arc::new(move || woken.store(true, Ordering::SeqCst))));
    let mut source = geojson_source("source", GeoJSONOptions::default_options());
    let geojson = r#"{"type": "Polygon", "coordinates": [[[-10, -10], [10, -10], [10, 10], [-10, 10], [-10, -10]]]}"#
        .parse::<GeoJson>()
        .unwrap();
    let data = create_geojson_data(geojson, &GeoJSONOptions::default_options()).unwrap();
    source = edited(&source, GeoJsonSourceType::set_data("source", Some(data)));
    let mut render_source = OneSource::new(&registry(), &source, &[layer("circle")], &TaskGroup::new(&pool)).unwrap();
    let continuous = view(&state, MapMode::Continuous, None);

    render_source.update(&source, &continuous);
    assert!(queued.load(Ordering::SeqCst));
    assert!(!render_source.is_loaded());

    // The queued slices run in the background while the source turns
    // synchronous and recreates its tiles, which slice the same index inline.
    let source = edited(&source, synchronous_update(true));
    let background_pool = pool.clone();
    let background = std::thread::spawn(move || background_pool.run_all());
    render_source.update(&source, &continuous);
    background.join().unwrap();
    assert!(render_source.is_loaded()); // The new tiles complete within the update.

    // The background slices reply to tiles that no longer exist.
    render_source.step();
    assert!(render_source.is_loaded());
}
