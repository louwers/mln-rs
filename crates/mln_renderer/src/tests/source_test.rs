use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

use geojson::GeoJson;
use mln_map::transform::Transform;
use mln_map_types::{
    CameraOptions, ConstrainMode, MapDebugOptions, MapMode, TileLodMode, TransformState, ViewportMode,
};
use mln_storage::response::{Reason, ResourceError};
use mln_storage::stub_file_source::{ResponseType, StubFileSource};
use mln_storage::{FileSource, FileSourceExt, Kind, RequestOrder, Resource, Responder, Response};
use mln_style::layer::Layer;
use mln_style::layer_type::LayerStyle;
use mln_style::layers::{Hillshade, Raster};
use mln_style::source_type::SourceData;
use mln_style::sources::custom_geometry_source::{self, Options as CustomGeometrySourceOptions};
use mln_style::sources::custom_vector_source::{self, Options as CustomVectorSourceOptions, TileDataFormat};
use mln_style::sources::geojson::{GeoJSONOptions, GeoJsonSourceType, create_geojson_data, geojson_source};
use mln_style::sources::{
    CustomGeometrySourceHandle, CustomVectorSourceHandle, VectorSourceType, raster_dem_source, raster_source,
    vector_source,
};
use mln_style::{Edit, Source, Style};
use mln_style_types::ProjectionDefinition;
use mln_tile_id::{CanonicalTileID, OverscaledTileID};
use mln_util::error::Error;
use mln_util::geo::LatLng;
use mln_util::logging::{Event, EventSeverity, Log, Observer as LogObserver};
use mln_util::pool::{Pool, TaskGroup};
use mln_util::queue::Queue;
use mln_util::size::Size;
use mln_util::subdivision_granularity::SubdivisionGranularitySetting;
use mln_util::tileset::{PartialTileset, Tileset};
use parking_lot::Mutex;

use crate::Registry;
use crate::layer_type::{ErasedEvaluatedLayer, EvaluatedLayer};
use crate::render_source::RenderSource;
use crate::tests::fixture_layer::Dot;
use crate::tile::source_tiles::SourceTiles;
use crate::tile::store::{StoreEvent, TileStore};
use crate::tile_parameters::TileParameters;

struct SourceTest {
    run_loop: Queue<()>,
    transform: Transform,
    thread_pool: TaskGroup,
    /// Set by `end_handle`'s calls.
    ended: Arc<AtomicBool>,
}

/// The tile parameters for `map_mode`, from a transform state the caller
/// owns.
fn tile_parameters(
    state: &TransformState,
    map_mode: MapMode,
    file_source: Option<Arc<dyn FileSource>>,
) -> TileParameters<'_> {
    TileParameters {
        pixel_ratio: 1.0,
        debug_options: MapDebugOptions::NO_DEBUG,
        view: state.view(),
        min_zoom: state.min_zoom(),
        max_zoom: state.limits().max_zoom,
        file_source,
        mode: map_mode,
        prefetch_zoom_delta: 0,
        tile_lod_min_radius: 3.0,
        tile_lod_scale: 1.0,
        tile_lod_pitch_threshold: (60.0 / 180.0) * std::f64::consts::PI,
        tile_lod_zoom_shift: 0.0,
        tile_lod_mode: TileLodMode::Default,
        subdivision_granularity: SubdivisionGranularitySetting::none(),
        global_state: None,
        is_update_synchronous: false,
        font_faces: None,
        glyph_pbfs: true,
        placement_commits: 0,
    }
}

impl SourceTest {
    fn new() -> Self {
        let mut transform = Transform::new(ConstrainMode::HeightOnly, ViewportMode::Default);
        transform.resize(Size::new(512, 512)).unwrap();
        transform.jump_to(&CameraOptions { center: Some(LatLng::default()), zoom: Some(0.0), ..Default::default() });
        SourceTest {
            run_loop: Queue::new(),
            transform,
            thread_pool: TaskGroup::new(&crate::tests::pool()),
            ended: Arc::default(),
        }
    }

    fn tile_parameters(&self, map_mode: MapMode) -> TileParameters<'_> {
        self.tile_parameters_with(map_mode, None)
    }

    fn tile_parameters_with(&self, map_mode: MapMode, file_source: Option<Arc<dyn FileSource>>) -> TileParameters<'_> {
        tile_parameters(self.transform.state(), map_mode, file_source)
    }

    /// A tile store whose jobs run on the thread pool.
    fn store(&self) -> TileStore {
        crate::tests::pool_store(&self.thread_pool)
    }

    /// Ends the test; usable from `'static` callbacks.
    fn end_handle(&self) -> impl Fn() + Clone + Send + Sync + 'static {
        let handle = self.run_loop.clone();
        let ended = Arc::clone(&self.ended);
        move || {
            ended.store(true, Ordering::SeqCst);
            handle.stop();
        }
    }

    /// Runs the loop, and `store`'s updates as the renderer's, until the
    /// test ends; `on_event` gets what the store reports.
    fn run_with(&self, store: &mut TileStore, mut on_event: impl FnMut(&mut TileStore, StoreEvent)) {
        let deadline = mln_util::time::Instant::now() + Duration::from_secs(30);
        while !self.ended.load(Ordering::SeqCst) {
            assert!(mln_util::time::Instant::now() < deadline, "the test didn't end");
            self.run_loop.run_once(&mut ());
            crate::tests::update(store);
            for event in store.take_events() {
                on_event(store, event);
            }
            std::thread::yield_now();
        }
    }

    /// As `run_with`, with `observer` observing the store.
    fn run_observed(&self, store: &mut TileStore, observer: &StubStoreObserver) {
        self.run_with(store, |_, event| observer.dispatch(event));
    }
}

impl Drop for SourceTest {
    fn drop(&mut self) {
        self.thread_pool.wait();
    }
}

type OnTileChanged = Box<dyn Fn()>;
type OnTileError = Box<dyn Fn(&str, &OverscaledTileID, Error)>;

/// Hears what a tile store reports of its sources' tiles.
#[derive(Default)]
struct StubStoreObserver {
    tile_changed: RefCell<Option<OnTileChanged>>,
    tile_error: RefCell<Option<OnTileError>>,
}

impl StubStoreObserver {
    /// Calls the callback for `event`.
    fn dispatch(&self, event: StoreEvent) {
        match event {
            StoreEvent::Changed => {
                if let Some(f) = &*self.tile_changed.borrow() {
                    f();
                }
            }
            StoreEvent::Error { source, tile, error } => {
                if let Some(f) = &*self.tile_error.borrow() {
                    f(&source, &tile, error);
                }
            }
            StoreEvent::Action { .. } => {}
        }
    }
}

/// `source` after `edit`, made through a style that holds it.
fn edited(source: &Source, edit: Edit) -> Source {
    let mut style = Style::new(Arc::clone(Registry::with_builtins().style()));
    style.apply(Edit::AddSource(source.clone())).unwrap();
    style.apply(edit).unwrap();
    style.get_source(source.id()).unwrap().clone()
}

/// `source`, a vector source with a loaded tileset, with `tiles` as that
/// tileset's tile URLs.
fn with_tiles(source: &Source, tiles: &[&str]) -> Source {
    let data = SourceData::<VectorSourceType>::recover(source).expect("a vector source");
    let tileset = data.description.clone().expect("a loaded tileset");
    let tileset = Tileset { tiles: tiles.iter().map(|&tile| tile.to_owned()).collect(), ..tileset };
    source.with_data(SourceData::<VectorSourceType> { description: Some(tileset), ..data.clone() })
}

/// The three tiled source types that each case runs for.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Tiled {
    Raster,
    RasterDEM,
    Vector,
}

impl Tiled {
    /// A raster, hillshade or line (source-layer "water") layer "id" on
    /// "source", as evaluated values.
    fn layers(self) -> Vec<Arc<dyn ErasedEvaluatedLayer>> {
        let properties: Arc<dyn ErasedEvaluatedLayer> = match self {
            Tiled::Raster => {
                let layer = Layer::new(Registry::with_builtins().style(), "raster", "id", "source").unwrap();
                Arc::new(EvaluatedLayer::<Raster>::new(LayerStyle::recover(layer.style()).unwrap()))
            }
            Tiled::RasterDEM => {
                let layer = Layer::new(Registry::with_builtins().style(), "hillshade", "id", "source").unwrap();
                Arc::new(EvaluatedLayer::<Hillshade>::new(LayerStyle::recover(layer.style()).unwrap()))
            }
            Tiled::Vector => {
                let mut layer = Layer::new(Registry::with_builtins().style(), "dot", "id", "source").unwrap();
                layer.set_source_layer("water");
                Arc::new(EvaluatedLayer::<Dot>::new(LayerStyle::recover(layer.style()).unwrap()))
            }
        };
        vec![properties]
    }

    /// The source "source" for `url` and `tileset`: raster or raster-dem
    /// with tile size 512, or vector.
    fn source(self, url: Option<String>, tileset: PartialTileset) -> Source {
        match self {
            Tiled::Raster => raster_source("source", url, tileset, 512),
            Tiled::RasterDEM => raster_dem_source("source", url, tileset, 512),
            Tiled::Vector => vector_source("source", url, tileset),
        }
    }

    fn type_name(self) -> &'static str {
        match self {
            Tiled::Raster => "raster",
            Tiled::RasterDEM => "raster-dem",
            Tiled::Vector => "vector",
        }
    }
}

fn tiles_tileset() -> PartialTileset {
    PartialTileset { tiles: Some(vec!["tiles".to_owned()]), ..Default::default() }
}

/// The shared body of the `<Type>Tile<Case>` tests: a source of `kind`
/// whose tiles get `tile_response`, a render source observed by `observer`,
/// one update, then `test.run()`. Returns the name of the render source's
/// type.
fn run_tile_case(
    test: &SourceTest,
    kind: Tiled,
    file_source: Arc<StubFileSource>,
    observer: &StubStoreObserver,
) -> &'static str {
    let layers = kind.layers();
    let source = crate::tests::ready(kind.source(None, tiles_tileset()));

    let mut store = test.store();
    let mut render_source = crate::tests::create_render_source(&source, &mut store);
    let fs: Arc<dyn FileSource> = file_source;
    render_source.update(
        &mut store,
        source.clone(),
        &layers,
        true,
        true,
        &test.tile_parameters_with(MapMode::Continuous, Some(fs)),
    );

    test.run_observed(&mut store, observer);
    render_source.base().style_source.info().name
}

fn tile_empty(kind: Tiled) {
    let test = SourceTest::new();
    let file_source = StubFileSource::on_queue(&test.run_loop, ResponseType::Asynchronous);
    file_source.set_response(Kind::Tile, |_resource| {
        let mut response = Response::new();
        response.no_content = true;
        Some(response)
    });

    let observer = Rc::new(StubStoreObserver::default());
    let changed = Rc::new(RefCell::new(false));
    {
        let changed = Rc::clone(&changed);
        let end = test.end_handle();
        *observer.tile_changed.borrow_mut() = Some(Box::new(move || {
            *changed.borrow_mut() = true;
            end();
        }));
    }
    *observer.tile_error.borrow_mut() = Some(Box::new(|_, _, _| panic!("Should never be called")));

    run_tile_case(&test, kind, file_source, &observer);
    assert!(*changed.borrow());
}

fn tile_fail(kind: Tiled) {
    let test = SourceTest::new();
    let file_source = StubFileSource::on_queue(&test.run_loop, ResponseType::Asynchronous);
    file_source.set_response(Kind::Tile, |_resource| {
        let mut response = Response::new();
        response.error = Some(Box::new(ResourceError::new(Reason::Other, "Failed by the test case")));
        Some(response)
    });

    let observer = Rc::new(StubStoreObserver::default());
    let failed = Rc::new(RefCell::new(false));
    {
        let failed = Rc::clone(&failed);
        let end = test.end_handle();
        *observer.tile_error.borrow_mut() = Some(Box::new(move |source_id, tile_id, error| {
            assert_eq!("source", source_id);
            assert_eq!(OverscaledTileID::from_zxy(0, 0, 0), *tile_id);
            assert_eq!("Failed by the test case", error.to_string());
            *failed.borrow_mut() = true;
            end();
        }));
    }

    // The observer gets the source id, so the type is checked here.
    assert_eq!(kind.type_name(), run_tile_case(&test, kind, file_source, &observer));
    assert!(*failed.borrow());
}

fn tile_corrupt(kind: Tiled) {
    let test = SourceTest::new();
    let file_source = StubFileSource::on_queue(&test.run_loop, ResponseType::Asynchronous);
    file_source.set_response(Kind::Tile, |_resource| {
        let mut response = Response::new();
        response.data = Some(Arc::new(b"CORRUPTED".to_vec()));
        Some(response)
    });

    let observer = Rc::new(StubStoreObserver::default());
    let failed = Rc::new(RefCell::new(false));
    {
        let failed = Rc::clone(&failed);
        let end = test.end_handle();
        *observer.tile_error.borrow_mut() = Some(Box::new(move |source_id, tile_id, error| {
            assert_eq!("source", source_id);
            assert_eq!(OverscaledTileID::from_zxy(0, 0, 0), *tile_id);
            if kind == Tiled::Vector {
                assert_eq!(error.to_string(), "protobuf decode error: invalid wire type: 7");
            } else {
                // Not asserting on platform-specific error text.
                assert_ne!(error.to_string(), "");
            }
            *failed.borrow_mut() = true;
            end();
        }));
    }

    assert_eq!(kind.type_name(), run_tile_case(&test, kind, file_source, &observer));
    assert!(*failed.borrow());
}

fn tile_cancel(kind: Tiled) {
    let test = SourceTest::new();
    let file_source = StubFileSource::on_queue(&test.run_loop, ResponseType::Asynchronous);
    let requested = Arc::new(AtomicBool::new(false));
    {
        let requested = Arc::clone(&requested);
        let end = test.end_handle();
        file_source.set_response(Kind::Tile, move |_resource| {
            requested.store(true, Ordering::SeqCst);
            end();
            None
        });
    }

    let observer = Rc::new(StubStoreObserver::default());
    *observer.tile_changed.borrow_mut() = Some(Box::new(|| panic!("Should never be called")));
    *observer.tile_error.borrow_mut() = Some(Box::new(|_, _, _| panic!("Should never be called")));

    run_tile_case(&test, kind, file_source, &observer);
    assert!(requested.load(Ordering::SeqCst));
}

/// A file source whose tile requests never answer, as tiles in flight: it
/// records each request's tile and order, and each cancellation.
#[derive(Default)]
struct PendingTileSource {
    requested: Arc<Mutex<Vec<(i8, i32, i32)>>>,
    orders: Arc<Mutex<Vec<RequestOrder>>>,
    cancelled: Arc<Mutex<Vec<(i8, i32, i32)>>>,
}

impl FileSource for PendingTileSource {
    fn load(&self, resource: &Resource, responder: Responder) {
        let data = resource.tile_data.as_ref().expect("only tiles are requested");
        let tile = (data.z, data.x, data.y);
        self.requested.lock().push(tile);
        self.orders.lock().push(resource.order.clone());
        let cancelled = Arc::clone(&self.cancelled);
        responder.on_cancel(move || cancelled.lock().push(tile));
        // The request stays open until it's cancelled.
        std::mem::forget(responder);
    }

    fn can_request(&self, _resource: &Resource) -> bool {
        true
    }
}

/// A tile that leaves the view while it loads goes on loading, and is the
/// one the view gets back when it turns back, as a map rotated back and
/// forth does at its corners: its request is neither cancelled nor made
/// again.
#[test]
fn tile_out_of_view_keeps_loading() {
    let mut test = SourceTest::new();
    test.transform.resize(Size::new(1024, 128)).unwrap();
    let file_source = Arc::new(PendingTileSource::default());
    let (requested, cancelled) = (Arc::clone(&file_source.requested), Arc::clone(&file_source.cancelled));
    let layers = Tiled::Vector.layers();
    let source = crate::tests::ready(Tiled::Vector.source(None, tiles_tileset()));
    let mut store = test.store();
    let mut render_source = crate::tests::create_render_source(&source, &mut store);
    let fs: Arc<dyn FileSource> = file_source;
    // Requests made so far, after an update with the camera at `bearing`.
    let mut turn = |test: &mut SourceTest, bearing: f64, needs_relayout: bool| {
        test.transform.jump_to(&CameraOptions {
            center: Some(LatLng::new(20.0, 20.0).unwrap()),
            zoom: Some(4.0),
            bearing: Some(bearing),
            ..Default::default()
        });
        let parameters = test.tile_parameters_with(MapMode::Continuous, Some(Arc::clone(&fs)));
        render_source.update(&mut store, source.clone(), &layers, true, needs_relayout, &parameters);
        requested.lock().clone()
    };

    let unturned = turn(&mut test, 0.0, true);
    let turned = turn(&mut test, 90.0, false);
    assert!(turned.len() > unturned.len(), "turning shows other tiles");
    let back = turn(&mut test, 0.0, false);
    assert_eq!(turned, back, "no tile is requested again");
    assert!(cancelled.lock().is_empty(), "no request is cancelled");
}

/// The requests for a view's tiles stand in the order the view needs them:
/// the coarse cover's, then the ideal cover's from the centre outwards. A
/// tile that leaves the view while it loads, and goes on loading next to it,
/// stands behind them all until the view comes back to it.
#[test]
fn tile_requests_follow_the_view() {
    const UNWANTED: u32 = u32::MAX;
    let mut test = SourceTest::new();
    // One ideal tile wide and several tall, so that a move of one tile to
    // the side leaves every ideal tile behind, next to the new ones.
    test.transform.resize(Size::new(256, 1024)).unwrap();
    let file_source = Arc::new(PendingTileSource::default());
    let (requested, orders) = (Arc::clone(&file_source.requested), Arc::clone(&file_source.orders));
    let layers = Tiled::Vector.layers();
    let source = crate::tests::ready(Tiled::Vector.source(None, tiles_tileset()));
    let mut store = test.store();
    let mut render_source = crate::tests::create_render_source(&source, &mut store);
    let fs: Arc<dyn FileSource> = file_source;
    // The tiles requested so far and where their requests stand now, after
    // an update with the camera at `center`.
    let mut look_at = |test: &mut SourceTest, center: LatLng, needs_relayout: bool| {
        test.transform.jump_to(&CameraOptions { center: Some(center), zoom: Some(6.0), ..Default::default() });
        let mut parameters = test.tile_parameters_with(MapMode::Continuous, Some(Arc::clone(&fs)));
        parameters.prefetch_zoom_delta = 4;
        render_source.update(&mut store, source.clone(), &layers, true, needs_relayout, &parameters);
        let orders: Vec<u32> = orders.lock().iter().map(RequestOrder::get).collect();
        (requested.lock().clone(), orders)
    };

    // The middle of the zoom 6 tile column `x`.
    let column = |x: f64| LatLng::new(20.0, -180.0 + (x + 0.5) * 360.0 / 64.0).unwrap();
    let here = column(35.0);
    let (tiles, first) = look_at(&mut test, here, true);
    let coarse: Vec<u32> = tiles.iter().zip(&first).filter(|(tile, _)| tile.0 == 2).map(|(_, order)| *order).collect();
    let ideal: Vec<u32> = tiles.iter().zip(&first).filter(|(tile, _)| tile.0 == 6).map(|(_, order)| *order).collect();
    assert_eq!(coarse.len() + ideal.len(), tiles.len());
    assert!(!coarse.is_empty() && ideal.len() > 1, "the view has coarse tiles and several ideal ones");
    assert!(coarse.iter().all(|order| *order > 0), "a tile's request comes after the requests that set no order");
    assert!(coarse.iter().max() < ideal.iter().min(), "the coarse tiles come first");
    assert!(ideal.is_sorted_by(|a, b| a < b), "the ideal tiles come in the cover's order");
    assert!(ideal.iter().all(|order| *order < UNWANTED));

    let first_tiles = tiles;
    let (tiles, second) = look_at(&mut test, column(36.0), false);
    assert!(tiles.len() > first.len(), "the new view needs other tiles");
    for ((tile, before), after) in first_tiles.iter().zip(&first).zip(&second) {
        if tile.0 == 6 {
            assert_eq!(*after, UNWANTED, "the tiles left behind come last");
        } else {
            assert_eq!(after, before, "the coarse tile stays in view");
        }
    }
    assert!(second[first.len()..].iter().all(|order| *order < UNWANTED));

    let (back_tiles, back) = look_at(&mut test, here, false);
    assert_eq!(back_tiles, tiles, "no tile is requested again");
    assert_eq!(back[..first.len()], first, "the first view's tiles stand as they did");
    assert!(back[first.len()..].iter().all(|order| *order == UNWANTED));
}

// gtest: Source.RasterTileEmpty
#[test]
fn raster_tile_empty() {
    tile_empty(Tiled::Raster);
}

// gtest: Source.RasterDEMTileEmpty
#[test]
fn raster_dem_tile_empty() {
    tile_empty(Tiled::RasterDEM);
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
    tile_fail(Tiled::RasterDEM);
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
    tile_corrupt(Tiled::RasterDEM);
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
    tile_cancel(Tiled::RasterDEM);
}

// gtest: Source.VectorTileCancel
#[test]
fn vector_tile_cancel() {
    tile_cancel(Tiled::Vector);
}

fn tile_attribution(kind: Tiled, attribution: &'static str) {
    let test = SourceTest::new();
    let layers = kind.layers();

    let file_source = StubFileSource::on_queue(&test.run_loop, ResponseType::Asynchronous);
    file_source.set_response(Kind::Tile, |_resource| {
        let mut response = Response::new();
        response.no_content = true;
        Some(response)
    });
    file_source.set_response(Kind::Source, move |resource| {
        assert_eq!("url", resource.url);
        let mut response = Response::new();
        response.data = Some(Arc::new(
            format!(r#"{{ "tilejson": "2.1.0", "attribution": "{attribution}", "tiles": [ "tiles" ] }}"#).into_bytes(),
        ));
        Some(response)
    });

    let source = kind.source(Some("url".to_owned()), PartialTileset::default());
    let mut store = test.store();
    let mut render_source = crate::tests::create_render_source(&source, &mut store);
    let fs: Arc<dyn FileSource> = Arc::<StubFileSource>::clone(&file_source);
    render_source.update(
        &mut store,
        source.clone(),
        &layers,
        true,
        true,
        &test.tile_parameters_with(MapMode::Continuous, Some(fs)),
    );

    // The map's loader fetches the TileJSON and describes it.
    let mln_style::source_type::Load::Fetch(resource) = source.data().plan_load() else { panic!("a TileJSON") };
    let (sender, responses) = std::sync::mpsc::channel();
    let _request = file_source.request(&resource, move |response: Response| {
        let _ = sender.send(response);
    });
    // The stub answers in the turns of the test's loop.
    let deadline = mln_util::time::Instant::now() + Duration::from_secs(30);
    let response = loop {
        if let Ok(response) = responses.try_recv() {
            break response;
        }
        assert!(mln_util::time::Instant::now() < deadline, "the TileJSON didn't load");
        test.run_loop.run_once(&mut ());
        crate::tests::update(&mut store);
        std::thread::yield_now();
    };
    let (sender, receiver) = std::sync::mpsc::channel();
    source.data().describe_job(response.data)(Box::new(move |result| {
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
    tile_attribution(Tiled::Raster, mapbox_osm);
}

// gtest: Source.RasterDEMTileAttribution
#[test]
fn raster_dem_tile_attribution() {
    let mapbox = "<a href='https://www.mapbox.com/about/maps/' target='_blank'>&copy; Mapbox</a> ";
    tile_attribution(Tiled::RasterDEM, mapbox);
}

/// A loaded custom geometry source "source", a line layer for it, and its
/// render source after one update of `store`.
fn custom_geometry_case(test: &SourceTest, store: &mut TileStore) -> (Source, Box<dyn RenderSource>) {
    let source =
        custom_geometry_source::create("source", CustomGeometrySourceOptions::default(), test.thread_pool.pool())
            .unwrap();

    let mut layer = Layer::new(Registry::with_builtins().style(), "dot", "id", "source").unwrap();
    layer.set_source_layer("water");
    let layer_properties: Arc<dyn ErasedEvaluatedLayer> =
        Arc::new(EvaluatedLayer::<Dot>::new(LayerStyle::recover(layer.style()).unwrap()));
    let layers = vec![layer_properties];

    let mut render_source = crate::tests::create_render_source(&source, store);
    render_source.update(store, source.clone(), &layers, true, true, &test.tile_parameters(MapMode::Continuous));
    (source, render_source)
}

// gtest: Source.CustomGeometrySourceSetTileData
#[test]
fn custom_geometry_source_set_tile_data() {
    let test = SourceTest::new();
    let observer = Rc::new(StubStoreObserver::default());
    {
        let end = test.end_handle();
        *observer.tile_changed.borrow_mut() = Some(Box::new(end));
    }
    *observer.tile_error.borrow_mut() = Some(Box::new(|_, _, _| panic!("Should never be called")));

    let mut store = test.store();
    let (source, _render_source) = custom_geometry_case(&test, &mut store);

    let handle = CustomGeometrySourceHandle::of(&source).unwrap();
    test.run_loop.post(move |()| {
        // Set Tile Data
        handle.set_tile_data(CanonicalTileID::new(0, 0, 0), GeoJson::from(Vec::<geojson::Feature>::new()));
    });

    test.run_observed(&mut store, &observer);
}

// gtest: Source.CustomGeometrySourceSetTileError
#[test]
fn custom_geometry_source_set_tile_error() {
    let test = SourceTest::new();
    let observer = Rc::new(StubStoreObserver::default());
    {
        let end = test.end_handle();
        *observer.tile_error.borrow_mut() = Some(Box::new(move |source_id, tile_id, error| {
            assert_eq!("source", source_id);
            assert_eq!(OverscaledTileID::from_zxy(0, 0, 0), *tile_id);
            assert_eq!("failed", error.to_string());
            end();
        }));
    }

    let mut store = test.store();
    let (source, _render_source) = custom_geometry_case(&test, &mut store);

    let handle = CustomGeometrySourceHandle::of(&source).unwrap();
    test.run_loop.post(move |()| {
        handle.set_tile_error(CanonicalTileID::new(0, 0, 0), Error::native("failed"));
    });

    test.run_observed(&mut store, &observer);
}

// Port-only: Source.CustomGeometrySourceSetTileData for a custom vector
// source.
#[test]
fn custom_vector_source_set_tile_data() {
    let test = SourceTest::new();
    let observer = Rc::new(StubStoreObserver::default());
    {
        let end = test.end_handle();
        *observer.tile_changed.borrow_mut() = Some(Box::new(end));
    }
    *observer.tile_error.borrow_mut() = Some(Box::new(|_, _, _| panic!("Should never be called")));

    let source = custom_vector_source::create("source", CustomVectorSourceOptions::default(), test.thread_pool.pool());

    let mut layer = Layer::new(Registry::with_builtins().style(), "dot", "id", "source").unwrap();
    layer.set_source_layer("water");
    let layer_properties: Arc<dyn ErasedEvaluatedLayer> =
        Arc::new(EvaluatedLayer::<Dot>::new(LayerStyle::recover(layer.style()).unwrap()));

    let mut store = test.store();
    let mut render_source = crate::tests::create_render_source(&source, &mut store);
    render_source.update(
        &mut store,
        source.clone(),
        &[layer_properties],
        true,
        true,
        &test.tile_parameters(MapMode::Continuous),
    );

    let handle = CustomVectorSourceHandle::of(&source).unwrap();
    test.run_loop.post(move |()| handle.set_tile_data(CanonicalTileID::new(0, 0, 0), None, TileDataFormat::Mvt));

    test.run_observed(&mut store, &observer);
}

// gtest: Source.EmptyTileset
#[test]
fn empty_tileset() {
    let test = SourceTest::new();
    let file_source = StubFileSource::on_queue(&test.run_loop, ResponseType::Asynchronous);
    let mut source = crate::tests::ready(vector_source("source", None, PartialTileset::default()));
    let mut store = test.store();
    let mut render_source = crate::tests::create_render_source(&source, &mut store);

    let layers = Tiled::Vector.layers();
    let fs: Arc<dyn FileSource> = file_source;
    let parameters = test.tile_parameters_with(MapMode::Continuous, Some(fs));

    render_source.update(&mut store, source.clone(), &layers, true, true, &parameters);
    assert!(render_source.is_loaded(&store));

    source = with_tiles(&source, &["tiles"]);
    render_source.update(&mut store, source.clone(), &layers, true, false, &parameters);
    assert!(!render_source.is_loaded(&store));

    source = with_tiles(&source, &[]);
    render_source.update(&mut store, source.clone(), &layers, true, false, &parameters);
    assert!(render_source.is_loaded(&store));

    source = with_tiles(&source, &["tiles"]);
    render_source.update(&mut store, source.clone(), &layers, true, false, &parameters);
    assert!(!render_source.is_loaded(&store));

    assert_eq!(0, tile_errors(&mut store));
}

// As Source.EmptyTileset, for the raster and raster-dem render sources.
#[test]
fn empty_raster_tilesets_make_no_tiles() {
    for kind in [Tiled::Raster, Tiled::RasterDEM] {
        let test = SourceTest::new();
        let file_source = StubFileSource::on_queue(&test.run_loop, ResponseType::Asynchronous);
        let source = crate::tests::ready(kind.source(None, PartialTileset::default()));
        let mut store = test.store();
        let mut render_source = crate::tests::create_render_source(&source, &mut store);

        let fs: Arc<dyn FileSource> = file_source;
        let parameters = test.tile_parameters_with(MapMode::Continuous, Some(fs));
        render_source.update(&mut store, source.clone(), &kind.layers(), true, true, &parameters);

        assert!(render_source.is_loaded(&store));
        assert_eq!(0, tile_errors(&mut store));
    }
}

/// Counts the tile errors `store` reported.
fn tile_errors(store: &mut TileStore) -> usize {
    store.take_events().into_iter().filter(|event| matches!(event, StoreEvent::Error { .. })).count()
}

// gtest: Source.GeoJSONSourceTilesAfterDataReset
#[test]
fn geojson_source_tiles_after_data_reset() {
    let test = SourceTest::new();
    let mut source = geojson_source("source", GeoJSONOptions::default_options());
    let geojson = r#"{"geometry": {"type": "Point", "coordinates": [1.1, 1.1]}, "type": "Feature", "properties": {}}"#
        .parse::<GeoJson>()
        .unwrap();
    let geojson_data = create_geojson_data(geojson, &GeoJSONOptions::default_options()).unwrap();
    source = edited(&source, GeoJsonSourceType::set_data("source", Some(Arc::clone(&geojson_data))));
    let mut store = test.store();
    let mut render_source = crate::tests::create_render_source(&source, &mut store);

    let layer = Layer::new(Registry::with_builtins().style(), "dot", "id", "source").unwrap();
    let layer_properties: Arc<dyn ErasedEvaluatedLayer> =
        Arc::new(EvaluatedLayer::<Dot>::new(LayerStyle::recover(layer.style()).unwrap()));
    let layers = vec![layer_properties];

    let continuous = test.tile_parameters(MapMode::Continuous);
    render_source.update(&mut store, source.clone(), &layers, true, true, &continuous);
    // loaded == false, means that the source contains pending tiles.
    assert!(!render_source.is_loaded(&store));

    source = edited(&source, GeoJsonSourceType::set_data("source", None));
    render_source.update(&mut store, source.clone(), &layers, true, true, &continuous);
    assert!(!render_source.is_loaded(&store)); // Tiles remain in continuous mode.

    source = edited(&source, GeoJsonSourceType::set_data("source", Some(geojson_data)));
    render_source.update(&mut store, source.clone(), &layers, true, true, &continuous);
    assert!(!render_source.is_loaded(&store));

    source = edited(&source, GeoJsonSourceType::set_data("source", None));
    render_source.update(&mut store, source.clone(), &layers, true, true, &test.tile_parameters(MapMode::Static));
    assert!(render_source.is_loaded(&store)); // Tiles are reset in static mode.
}

/// A dot layer "id" on "source", as evaluated values.
fn dot_layers() -> Vec<Arc<dyn ErasedEvaluatedLayer>> {
    let layer = Layer::new(Registry::with_builtins().style(), "dot", "id", "source").unwrap();
    vec![Arc::new(EvaluatedLayer::<Dot>::new(LayerStyle::recover(layer.style()).unwrap()))]
}

/// A GeoJSON source rendered with a dot layer. Its data is clustered,
/// which slices a tile within the call in either mode, so whether the tiles
/// complete within an update shows only how they lay out.
struct SynchronousUpdateSource {
    options: Arc<GeoJSONOptions>,
    source: Source,
    store: TileStore,
    render_source: Box<dyn RenderSource>,
    layers: Vec<Arc<dyn ErasedEvaluatedLayer>>,
}

impl SynchronousUpdateSource {
    fn new(test: &SourceTest, synchronous_update: bool) -> Self {
        let options: Arc<GeoJSONOptions> =
            Arc::new(GeoJSONOptions { cluster: true, synchronous_update, ..Default::default() });
        let source = geojson_source("source", Arc::clone(&options));
        let mut store = test.store();
        let render_source = crate::tests::create_render_source(&source, &mut store);
        SynchronousUpdateSource { options, source, store, render_source, layers: dot_layers() }
    }

    fn is_loaded(&self) -> bool {
        self.render_source.is_loaded(&self.store)
    }

    /// Sets new data and updates the render source, as a frame would.
    fn update(&mut self, test: &SourceTest) {
        let geojson = r#"{"type": "FeatureCollection", "features": [{"type": "Feature", "properties": {},
                "geometry": {"type": "Point", "coordinates": [1.1, 1.1]}}]}"#
            .parse::<GeoJson>()
            .unwrap();
        let data = Some(create_geojson_data(geojson, &self.options).unwrap());
        self.source = edited(&self.source, GeoJsonSourceType::set_data("source", data));
        let mut parameters = test.tile_parameters(MapMode::Continuous);
        parameters.is_update_synchronous = self.source.data().is_update_synchronous();
        self.render_source.update(&mut self.store, self.source.clone(), &self.layers, true, true, &parameters);
        // The end of the renderer's update.
        self.store.drive();
    }
}

/// The edit that sets the synchronous update of the GeoJSON source "source".
fn synchronous_update(synchronous: bool) -> Edit {
    GeoJsonSourceType::set_synchronous("source", synchronous)
}

// gtest: Source.GeoJSONSourceSynchronousUpdateOverrideAppliesToExistingTiles
#[test]
fn geojson_source_synchronous_update_override_applies_to_existing_tiles() {
    let test = SourceTest::new();

    let mut synchronous = SynchronousUpdateSource::new(&test, true);
    synchronous.update(&test);
    assert!(synchronous.is_loaded()); // The tiles complete within the update.
    synchronous.source = edited(&synchronous.source, synchronous_update(false));
    synchronous.update(&test);
    assert!(!synchronous.is_loaded()); // The tiles now lay out in the background.

    let mut asynchronous = SynchronousUpdateSource::new(&test, false);
    asynchronous.update(&test);
    assert!(!asynchronous.is_loaded());
    while !asynchronous.is_loaded() {
        test.run_loop.run_once(&mut ());
        crate::tests::update(&mut asynchronous.store);
        std::thread::yield_now();
    }
    asynchronous.source = edited(&asynchronous.source, synchronous_update(true));
    asynchronous.update(&test);
    assert!(asynchronous.is_loaded()); // The tiles now complete within the update.
}

// gtest: Source.GeoJSONSourceSlicesInlineWhileABackgroundSliceRuns
#[test]
fn geojson_source_slices_inline_while_a_background_slice_runs() {
    let mut test = SourceTest::new();
    // At zoom 3 the index slices its tiles on demand, which fills its tile cache.
    test.transform.jump_to(&CameraOptions { center: Some(LatLng::default()), zoom: Some(3.0), ..Default::default() });

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
    let mut store = crate::tests::pool_store(&TaskGroup::new(&pool));
    let mut render_source = crate::tests::create_render_source(&source, &mut store);
    let layers = dot_layers();
    let update = |render_source: &mut Box<dyn RenderSource>, store: &mut TileStore, source: &Source| {
        let mut parameters = test.tile_parameters(MapMode::Continuous);
        parameters.is_update_synchronous = source.data().is_update_synchronous();
        render_source.update(store, source.clone(), &layers, true, true, &parameters);
        store.drive();
    };

    update(&mut render_source, &mut store, &source);
    assert!(queued.load(Ordering::SeqCst));
    assert!(!render_source.is_loaded(&store));

    // The queued slices run in the background while the source turns
    // synchronous and recreates its tiles, which slice the same index inline.
    let source = edited(&source, synchronous_update(true));
    let background_pool = pool.clone();
    let background = std::thread::spawn(move || background_pool.run_all());
    update(&mut render_source, &mut store, &source);
    background.join().unwrap();
    assert!(render_source.is_loaded(&store)); // The new tiles complete within the update.

    // The background slices reply to tiles that no longer exist.
    crate::tests::update(&mut store);
    assert!(render_source.is_loaded(&store));
}

type Records = Arc<Mutex<Vec<((EventSeverity, Event, String), bool)>>>;

struct Recorder(Records);

impl LogObserver for Recorder {
    fn on_record(&self, severity: EventSeverity, event: Event, _code: i64, msg: &str) -> bool {
        self.0.lock().push(((severity, event, msg.to_owned()), false));
        true
    }
}

/// Captures this thread's log records.
struct FixtureLog {
    messages: Records,
}

impl FixtureLog {
    fn new() -> Self {
        let messages: Records = Arc::new(Mutex::new(Vec::new()));
        let _ = Log::set_thread_observer(Some(Box::new(Recorder(Arc::clone(&messages)))));
        FixtureLog { messages }
    }

    fn count(&self, severity: EventSeverity, event: Event, msg: &str) -> usize {
        let mut count = 0;
        for (m, checked) in self.messages.lock().iter_mut() {
            if m.0 == severity && m.1 == event && m.2 == msg {
                *checked = true;
                count += 1;
            }
        }
        count
    }

    fn unchecked_count(&self) -> usize {
        self.messages.lock().iter().filter(|(_, checked)| !checked).count()
    }
}

impl Drop for FixtureLog {
    fn drop(&mut self) {
        let _ = Log::set_thread_observer(None);
    }
}

// gtest: Source.SetMaxParentOverscaleFactor
#[test]
fn set_max_parent_overscale_factor() {
    let mut test = SourceTest::new();
    test.transform.jump_to(&CameraOptions { center: Some(LatLng::default()), zoom: Some(8.0), ..Default::default() });
    let log = FixtureLog::new();

    let file_source = StubFileSource::on_queue(&test.run_loop, ResponseType::Asynchronous);
    {
        let end = test.end_handle();
        file_source.set_response(Kind::Tile, move |res| {
            let z = res.tile_data.as_ref().unwrap().z;
            if z == 5 {
                let end = end.clone();
                std::thread::spawn(move || {
                    std::thread::sleep(Duration::from_millis(10));
                    end();
                });
            }
            // No tiles above zoom level 5 should be requested.
            assert!(5 <= i32::from(z), "requested z{z}");
            // The tileset has no tiles (HTTP 404), so the source requests parents in their place.
            // An empty raster tile (HTTP 204) would be drawn, empty, instead.
            let mut response = Response::new();
            response.no_content = true;
            response.tile_not_found = true;
            Some(response)
        });
    }

    let layers = Tiled::Raster.layers();
    let source = Tiled::Raster.source(None, tiles_tileset());
    let mut common = *source.common();
    assert_eq!(None, common.max_overscale_factor);
    common.max_overscale_factor = Some(3);
    let style_source = crate::tests::ready(source.with_common(common));

    let mut store = test.store();
    let mut render_source = crate::tests::create_render_source(&style_source, &mut store);
    let fs: Arc<dyn FileSource> = Arc::<StubFileSource>::clone(&file_source);
    let parameters = test.tile_parameters_with(MapMode::Continuous, Some(Arc::clone(&fs)));
    render_source.update(&mut store, style_source.clone(), &layers, true, true, &parameters);

    // Each tile that changes updates the source again.
    test.run_with(&mut store, |store, event| {
        if matches!(event, StoreEvent::Changed) {
            render_source.update(store, style_source.clone(), &layers, true, true, &parameters);
        }
    });

    assert_eq!(
        1,
        log.count(EventSeverity::Warning, Event::Style, "Parent tile overscale factor will cap prefetch delta to 3")
    );
    assert_eq!(0, log.unchecked_count());
}

// gtest: Source.RasterDEMTileRoundsCoveringZoom
#[test]
fn raster_dem_tile_rounds_covering_zoom() {
    let mut test = SourceTest::new();
    test.transform.jump_to(&CameraOptions { center: Some(LatLng::default()), zoom: Some(4.6), ..Default::default() });

    let requested_zooms = Arc::new(Mutex::new(Vec::<i8>::new()));
    let file_source = StubFileSource::on_queue(&test.run_loop, ResponseType::Asynchronous);
    {
        let requested_zooms = Arc::clone(&requested_zooms);
        file_source.set_response(Kind::Tile, move |res| {
            requested_zooms.lock().push(res.tile_data.as_ref().unwrap().z);
            let mut response = Response::new();
            response.no_content = true;
            Some(response)
        });
    }

    let layers = Tiled::RasterDEM.layers();
    let source = crate::tests::ready(Tiled::RasterDEM.source(None, tiles_tileset()));

    let mut store = test.store();
    let mut render_source = crate::tests::create_render_source(&source, &mut store);
    let fs: Arc<dyn FileSource> = Arc::<StubFileSource>::clone(&file_source);
    render_source.update(
        &mut store,
        source.clone(),
        &layers,
        true,
        true,
        &test.tile_parameters_with(MapMode::Continuous, Some(fs)),
    );

    let end = test.end_handle();
    test.run_with(&mut store, |_, event| {
        if matches!(event, StoreEvent::Changed) {
            end();
        }
    });

    assert!(!requested_zooms.lock().is_empty());
    for &z in requested_zooms.lock().iter() {
        assert_eq!(5, z);
    }
}

/// A vector source drawn with the camera of a map, whose tile requests are
/// answered only when the test says so.
struct TurningSource {
    test: SourceTest,
    file_source: Arc<StubFileSource>,
    requests: Arc<AtomicUsize>,
    source: Source,
    store: TileStore,
    render_source: Box<dyn RenderSource>,
    layers: Vec<Arc<dyn ErasedEvaluatedLayer>>,
    prefetch_zoom_delta: u8,
    latitude: f64,
    /// The placements committed before the next update.
    placement_commits: u64,
}

impl TurningSource {
    fn set_max_overscale_factor(&mut self, factor: Option<u8>) {
        let mut common = *self.source.common();
        common.max_overscale_factor = factor;
        self.source = self.source.with_common(common);
    }

    fn new(projection: &str, size: Size, min_zoom: u8) -> Self {
        let mut test = SourceTest::new();
        test.transform.resize(size).unwrap();
        test.transform.set_projection_definition(&ProjectionDefinition::from_name(projection));
        let file_source = Arc::new(StubFileSource::new(ResponseType::Manual));
        let requests = Arc::new(AtomicUsize::new(0));
        {
            let requests = Arc::clone(&requests);
            file_source.set_response(Kind::Tile, move |_| {
                requests.fetch_add(1, Ordering::SeqCst);
                let mut response = Response::new();
                response.no_content = true;
                Some(response)
            });
        }
        let source = Tiled::Vector.source(
            None,
            PartialTileset {
                tiles: Some(vec!["tiles".to_owned()]),
                min_zoom: Some(min_zoom),
                max_zoom: Some(14),
                ..Default::default()
            },
        );
        let source = crate::tests::ready(source);
        let mut store = crate::tests::tile_store(&test.thread_pool, Box::new(crate::tile::tile_workers::InlineWorkers));
        let render_source = crate::tests::create_render_source(&source, &mut store);
        TurningSource {
            test,
            file_source,
            requests,
            source,
            store,
            render_source,
            layers: Tiled::Vector.layers(),
            prefetch_zoom_delta: 0,
            latitude: 0.0,
            placement_commits: 0,
        }
    }

    fn pyramid(&self) -> &SourceTiles {
        self.render_source.tiles().and_then(|key| self.store.source(key)).expect("the source's tiles")
    }

    /// Moves the camera and updates the source, as a frame would.
    fn update(&mut self, longitude: f64, zoom: f64) {
        self.test.transform.jump_to(&CameraOptions {
            center: Some(LatLng::new(self.latitude, longitude).unwrap()),
            zoom: Some(zoom),
            ..Default::default()
        });
        self.update_rendering(true);
    }

    /// Updates the source at this camera, as a frame does in which its
    /// layers are visible, or (with `false`) hidden.
    fn update_rendering(&mut self, needs_rendering: bool) {
        let file_source: Arc<dyn FileSource> = Arc::<StubFileSource>::clone(&self.file_source);
        let mut parameters = self.test.tile_parameters_with(MapMode::Continuous, Some(file_source));
        parameters.prefetch_zoom_delta = self.prefetch_zoom_delta;
        parameters.placement_commits = self.placement_commits;
        let (store, source, layers) = (&mut self.store, self.source.clone(), &self.layers);
        self.render_source.update(store, source, layers, needs_rendering, false, &parameters);
    }

    /// How many tiles the source caches, each checked to wait behind the
    /// view's: its jobs and its request.
    fn cached_unwanted(&self) -> usize {
        let unwanted = crate::tile::tile::TileLoadOrder::UNWANTED;
        let cached: Vec<_> = self.pyramid().slots().filter(|slot| slot.place().is_cached()).collect();
        for slot in &cached {
            assert_eq!(slot.place().order(), unwanted, "tile {}'s jobs", slot.tile.id);
            assert_eq!(slot.tile.load.request_order(), Some(unwanted.value()), "tile {}'s request", slot.tile.id);
        }
        cached.len()
    }

    /// Updates the source at this camera until every tile it asks for has
    /// loaded.
    fn load(&mut self, longitude: f64, zoom: f64) {
        self.update(longitude, zoom);
        for _ in 0..100 {
            self.file_source.respond_to_all();
            while self.test.run_loop.run_once(&mut ()) {}
            self.test.thread_pool.wait();
            while self.test.run_loop.run_once(&mut ()) {}
            crate::tests::update(&mut self.store);
            self.update(longitude, zoom);
            if self.pyramid().is_loaded() {
                return;
            }
        }
        panic!("the tiles did not load");
    }

    /// The pyramid's tiles of zoom `z`.
    fn tiles_at(&self, z: u8) -> Vec<CanonicalTileID> {
        self.pyramid().cover().map(|tile| tile.id).filter(|id| id.canonical.z == z).map(|id| id.canonical).collect()
    }

    /// The tiles still loading that no rendered tile stands in for.
    fn bare_tiles(&self) -> Vec<OverscaledTileID> {
        let rendered: Vec<CanonicalTileID> = self.pyramid().rendered().map(|rendered| rendered.id.canonical).collect();
        self.pyramid()
            .cover()
            .filter(|tile| {
                !tile.is_renderable()
                    && !rendered
                        .iter()
                        .any(|parent| *parent == tile.id.canonical || tile.id.canonical.is_child_of(parent))
            })
            .map(|tile| tile.id)
            .collect()
    }
}

/// A tile that enters the cache, whether the view left it or the source's
/// layers were hidden, waits behind the view's tiles: its jobs, and its
/// request should it revalidate.
#[test]
fn cached_tiles_wait_behind_the_view() {
    let mut panned = TurningSource::new("mercator", Size::new(512, 512), 0);
    panned.load(0.0, 3.0);
    assert_eq!(0, panned.cached_unwanted());
    // The tiles the view left are held while their symbols fade out, for
    // two placements.
    for _ in 0..3 {
        panned.update(120.0, 3.0);
        panned.placement_commits += 1;
    }
    assert!(panned.cached_unwanted() > 0, "the tiles the view left");

    let mut hidden = TurningSource::new("mercator", Size::new(512, 512), 0);
    hidden.load(0.0, 3.0);
    let cover = hidden.pyramid().cover().count();
    hidden.update_rendering(false);
    assert_eq!(0, hidden.pyramid().cover().count());
    assert_eq!(cover, hidden.cached_unwanted(), "the hidden source's tiles");
}

/// A tile the view leaves is held for its symbols' fade from the update that
/// leaves it until two placements have committed since, however many
/// updates come in between, and the next update drops it.
#[test]
fn a_tile_the_view_leaves_is_held_for_two_placements() {
    let mut turned = TurningSource::new("mercator", Size::new(512, 512), 0);
    turned.placement_commits = 5;
    turned.load(0.0, 3.0);
    assert!(!turned.store.has_fading_tiles());
    turned.update(120.0, 3.0);
    assert!(turned.store.has_fading_tiles(), "held from the update that leaves it");
    turned.update(120.0, 3.0);
    assert!(turned.store.has_fading_tiles(), "held while no placement commits");
    turned.placement_commits += 1;
    turned.update(120.0, 3.0);
    assert!(turned.store.has_fading_tiles(), "held after one placement");
    turned.placement_commits += 1;
    turned.update(120.0, 3.0);
    assert!(!turned.store.has_fading_tiles(), "dropped after two");
}

/// A globe turned to its far side before any tile there has loaded draws the
/// source's first level in their place.
fn globe_far_side_falls_back_to_the_base_level(size: Size, min_zoom: u8, zoom: f64) {
    let mut turning = TurningSource::new("vertical-perspective", size, min_zoom);
    turning.prefetch_zoom_delta = 4;
    turning.load(60.0, zoom);
    assert_eq!(1 << (2 * min_zoom), turning.tiles_at(min_zoom).len(), "the whole first level is loaded");
    assert!(
        turning.pyramid().rendered().all(|rendered| rendered.id.canonical.z > min_zoom),
        "the loaded view draws none of it"
    );

    turning.update(240.0, zoom);
    let loading = turning.pyramid().cover().filter(|tile| !tile.is_renderable()).count();
    assert!(loading > 0, "the far side has tiles to load");
    assert_eq!(Vec::<OverscaledTileID>::new(), turning.bare_tiles());
    assert!(turning.pyramid().rendered().any(|rendered| rendered.id.canonical.z == min_zoom));
}

#[test]
fn globe_far_side_falls_back_to_zoom_zero() {
    globe_far_side_falls_back_to_the_base_level(Size::new(1920, 1080), 0, 5.0);
}

#[test]
fn globe_far_side_falls_back_to_zoom_one() {
    globe_far_side_falls_back_to_the_base_level(Size::new(512, 512), 1, 3.0);
}

/// The tiles past the antimeridian have another wrap than the zoom-0 tile,
/// which stands in for them all the same, and is drawn once.
#[test]
fn globe_fallback_crosses_the_antimeridian() {
    let mut turning = TurningSource::new("vertical-perspective", Size::new(512, 512), 0);
    turning.load(90.0, 2.0);
    assert!(turning.pyramid().rendered().all(|rendered| rendered.id.canonical.z == 2));

    turning.update(150.0, 2.0);
    let wraps = |renderable: bool| -> Vec<i16> {
        let mut wraps: Vec<i16> = turning
            .pyramid()
            .cover()
            .filter(|tile| tile.id.canonical.z == 2 && tile.is_renderable() == renderable)
            .map(|tile| tile.id.wrap)
            .collect();
        wraps.dedup();
        wraps
    };
    assert_eq!((vec![0], vec![1]), (wraps(true), wraps(false)));
    assert_eq!(Vec::<OverscaledTileID>::new(), turning.bare_tiles());
    assert_eq!(1, turning.pyramid().rendered().filter(|rendered| rendered.id.canonical.z == 0).count());
}

/// While the globe blends into Mercator, a tile's flat half is placed by its
/// wrap, so the tiles past the antimeridian don't take the zoom-0 tile of
/// the other wrap.
#[test]
fn globe_fallback_keeps_to_its_wrap_while_blending_into_mercator() {
    let mut turning = TurningSource::new("vertical-perspective", Size::new(512, 512), 0);
    turning.test.transform.set_projection_definition(&ProjectionDefinition::between_names(
        "vertical-perspective",
        "mercator",
        0.5,
    ));
    turning.load(90.0, 2.0);
    assert_eq!(1, turning.tiles_at(0).len());

    turning.update(150.0, 2.0);
    let bare = turning.bare_tiles();
    assert_ne!(Vec::<OverscaledTileID>::new(), bare);
    assert!(bare.iter().all(|id| id.wrap == 1 && id.canonical.z == 2));
    assert_eq!(0, turning.pyramid().rendered().filter(|rendered| rendered.id.canonical.z == 0).count());
}

/// The first level costs a source that starts at zoom 1 the tiles of it that
/// the view doesn't show, and a Mercator map nothing.
#[test]
fn globe_base_level_requests() {
    let mut globe = TurningSource::new("vertical-perspective", Size::new(512, 512), 1);
    globe.load(60.0, 3.0);
    let mut mercator = TurningSource::new("mercator", Size::new(512, 512), 1);
    mercator.load(60.0, 3.0);
    assert_eq!(Vec::<CanonicalTileID>::new(), mercator.tiles_at(1));
    assert_eq!(4, globe.tiles_at(1).len());
    let view = globe.pyramid().cover().count() - 4;
    assert_eq!(view + 4, globe.requests.load(Ordering::SeqCst));
}

#[test]
fn globe_base_level_is_released() {
    let mut turning = TurningSource::new("vertical-perspective", Size::new(512, 512), 1);
    turning.load(60.0, 3.0);
    assert_eq!(4, turning.tiles_at(1).len());

    // The planet's radius passes twice the viewport's side between these.
    turning.update(60.0, 3.6);
    assert_eq!(4, turning.tiles_at(1).len());
    turning.update(60.0, 3.7);
    assert_eq!(Vec::<CanonicalTileID>::new(), turning.tiles_at(1));

    turning.update(60.0, 3.0);
    assert_eq!(4, turning.tiles_at(1).len());
    turning.test.transform.set_projection_definition(&ProjectionDefinition::from_name("mercator"));
    turning.update(60.0, 3.0);
    assert_eq!(Vec::<CanonicalTileID>::new(), turning.tiles_at(1));
}

/// The globe is larger away from the equator at the same zoom, so the first
/// level is let go of at a lower zoom there.
#[test]
fn globe_base_level_is_released_sooner_away_from_the_equator() {
    let mut turning = TurningSource::new("vertical-perspective", Size::new(512, 512), 1);
    turning.latitude = 60.0;
    turning.load(60.0, 2.6);
    assert_eq!(4, turning.tiles_at(1).len());

    // At 60 degrees the planet is twice its size at the equator: its radius
    // passes twice the viewport's side a zoom lower.
    turning.update(60.0, 2.7);
    assert_eq!(Vec::<CanonicalTileID>::new(), turning.tiles_at(1));
}

/// A first level of sixteen tiles is not kept, nor one that the search for a
/// loaded parent would not reach.
#[test]
fn globe_base_level_is_not_kept_where_it_is_large_or_out_of_reach() {
    let mut large = TurningSource::new("vertical-perspective", Size::new(512, 512), 2);
    large.load(60.0, 3.0);
    assert_eq!(Vec::<CanonicalTileID>::new(), large.tiles_at(2));

    let mut out_of_reach = TurningSource::new("vertical-perspective", Size::new(512, 512), 1);
    out_of_reach.set_max_overscale_factor(Some(1));
    out_of_reach.load(60.0, 3.0);
    assert_eq!(Vec::<CanonicalTileID>::new(), out_of_reach.tiles_at(1));
    out_of_reach.set_max_overscale_factor(Some(2));
    out_of_reach.load(60.0, 3.0);
    assert_eq!(4, out_of_reach.tiles_at(1).len());
}

/// The DEM of `tile`, a raster-dem tile.
fn dem(tile: &crate::tile::tile::Tile) -> &crate::buckets::hillshade_bucket::HillshadeBucket {
    match tile.contents() {
        Some(crate::tile::contents::Contents::Dem(bucket)) => bucket,
        _ => panic!("a DEM"),
    }
}

/// Port-only: raster-dem tiles that meet at the antimeridian, in two copies
/// of the world, backfill each other's borders as neighbours in one copy
/// do.
#[test]
fn raster_dem_borders_across_a_world_wrap() {
    use crate::tests::tile::dem_png;

    let mut test = SourceTest::new();
    test.transform.jump_to(&CameraOptions {
        center: Some(LatLng::new(0.0, 180.0).unwrap()),
        zoom: Some(2.0),
        ..Default::default()
    });
    let file_source = StubFileSource::on_queue(&test.run_loop, ResponseType::Asynchronous);
    file_source.set_response(Kind::Tile, |resource| {
        let tile = resource.tile_data.as_ref()?;
        let mut response = Response::new();
        response.data = Some(dem_png(tile.x as u32));
        Some(response)
    });
    let layers = Tiled::RasterDEM.layers();
    let source = crate::tests::ready(Tiled::RasterDEM.source(None, tiles_tileset()));
    let mut store = test.store();
    let mut render_source = crate::tests::create_render_source(&source, &mut store);
    let fs: Arc<dyn FileSource> = file_source;
    let deadline = mln_util::time::Instant::now() + Duration::from_secs(30);
    let mut relayout = true;
    loop {
        // The store backfills the tiles as their DEMs arrive.
        let parameters = test.tile_parameters_with(MapMode::Continuous, Some(Arc::clone(&fs)));
        render_source.update(&mut store, source.clone(), &layers, true, relayout, &parameters);
        relayout = false;
        let key = render_source.tiles().expect("the source's tiles");
        if store.source(key).is_some_and(|source| source.slots().next().is_some()) && render_source.is_loaded(&store) {
            break;
        }
        assert!(mln_util::time::Instant::now() < deadline, "the tiles didn't load");
        test.run_loop.run_once(&mut ());
        crate::tests::update(&mut store);
        std::thread::yield_now();
    }

    let source = render_source.tiles().and_then(|key| store.source(key)).expect("the source's tiles");
    let tile = |x| {
        source
            .slots()
            .map(|slot| &slot.tile)
            .find(|tile| tile.id.canonical == CanonicalTileID::new(2, x, 1))
            .unwrap_or_else(|| panic!("no tile 2/{x}/1"))
    };
    // The east edge of a copy of the world, and the west edge of the copy
    // east of it.
    let (east, west) = (tile(3), tile(0));
    assert_eq!(west.id.wrap, east.id.wrap + 1);
    let (east, west) = (dem(east), dem(west));
    let (east, west) = (east.dem_data(), west.dem_data());
    for y in 0..16 {
        assert_eq!(east.get(16, y), west.get(0, y), "row {y} of the east tile's border");
        assert_eq!(west.get(-1, y), east.get(15, y), "row {y} of the west tile's border");
    }
}
