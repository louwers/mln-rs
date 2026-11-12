// Ports the VectorTile tests of test/tile/vector_tile.test.cpp (the
// VectorTileData ones are mln_tile's).

use std::rc::Rc;
use std::sync::{Arc, Mutex};

use mln_actor::{RunLoop, TaggedScheduler, scheduler};
use mln_map::transform::{NullTransformObserver, Transform};
use mln_map_types::{ConstrainMode, MapDebugOptions, MapMode, TileLodMode, ViewportMode};
use mln_renderer::query::SourceQueryOptions;
use mln_renderer::tile::geometry_tile_worker::{TileMessage, WorkerMessage};
use mln_renderer::tile::raster_dem_tile::{RasterDEMTileMessage, RasterDEMWorkerMessage};
use mln_renderer::tile::raster_tile::{RasterTileMessage, RasterWorkerMessage};
use mln_renderer::tile::tile::Tile;
use mln_renderer::tile::tile_loader::{LoaderTile, VectorTileEncoding};
use mln_renderer::tile::tile_worker_transport::{
    InProcessTransport, TileEndpoint, TileWorkerTransport, WorkerEndpoint, WorkerParameters,
};
use mln_renderer::tile::vector_tile::VectorTile;
use mln_renderer::tile_parameters::TileParameters;
use mln_storage::stub_file_source::{ResponseType, StubFileSource};
use mln_tile_id::OverscaledTileID;
use mln_util::error::Error;
use mln_util::identity::SimpleIdentity;
use mln_util::subdivision_granularity::SubdivisionGranularitySetting;
use mln_util::tileset::Tileset;

/// `VectorTileTest`.
struct VectorTileTest {
    run_loop: RunLoop,
    transform: Transform,
    thread_pool: TaggedScheduler,
    file_source: Arc<StubFileSource>,
    tileset: Tileset,
    transport: Rc<dyn TileWorkerTransport>,
}

impl VectorTileTest {
    fn new() -> Self {
        let thread_pool = TaggedScheduler::new(scheduler::get_background(), SimpleIdentity::new());
        VectorTileTest {
            run_loop: RunLoop::new(),
            transform: Transform::new(Rc::new(NullTransformObserver), ConstrainMode::HeightOnly, ViewportMode::Default),
            transport: Rc::new(InProcessTransport::new(thread_pool.clone())),
            thread_pool,
            // C++ uses a FakeFileSource whose requests never answer; the
            // asynchronous stub answers only when the run loop runs.
            file_source: Arc::new(StubFileSource::new(ResponseType::Asynchronous)),
            tileset: Tileset { tiles: vec!["https://example.com".to_owned()], ..Default::default() },
        }
    }

    fn tile_parameters(&self) -> TileParameters<'_> {
        let file_source: Arc<dyn mln_storage::FileSource> = self.file_source.clone();
        TileParameters {
            pixel_ratio: 1.0,
            debug_options: MapDebugOptions::NO_DEBUG,
            transform_state: self.transform.get_state(),
            file_source: Some(file_source),
            mode: MapMode::Continuous,
            prefetch_zoom_delta: 0,
            tile_lod_min_radius: 3.0,
            tile_lod_scale: 1.0,
            tile_lod_pitch_threshold: (60.0 / 180.0) * std::f64::consts::PI,
            tile_lod_zoom_shift: 0.0,
            tile_lod_mode: TileLodMode::Default,
            subdivision_granularity: SubdivisionGranularitySetting::none(),
            global_state: None,
            is_update_synchronous: false,
            capture_rendered_features: false,
            image_manager: mln_renderer::image_manager::ImageManager::create(),
            glyph_manager: mln_renderer::text::glyph_manager::GlyphManager::new(None),
            dynamic_texture_atlas: Arc::new(mln_renderer::gfx::dynamic_texture_atlas::DynamicTextureAtlas::new()),
            tile_worker_transport: self.transport.clone(),
        }
    }
}

fn mvt_tile(test: &VectorTileTest) -> VectorTile {
    VectorTile::new(
        OverscaledTileID::from_zxy(0, 0, 0),
        "source".to_owned(),
        &test.tile_parameters(),
        &test.tileset,
        None,
        VectorTileEncoding::Mvt,
    )
}

// gtest: VectorTile.setError
#[test]
fn set_error() {
    let test = VectorTileTest::new();
    let tile = mvt_tile(&test);
    LoaderTile::set_error(&**tile.geometry_tile().inner(), Error::native("test"));
    assert!(!tile.is_renderable());
    assert!(tile.is_loaded());
    assert!(tile.is_complete());
}

// gtest: VectorTile.onError
#[test]
fn on_error() {
    let test = VectorTileTest::new();
    let tile = mvt_tile(&test);
    tile.geometry_tile().inner().on_error(Error::native("test"), 0);

    assert!(!tile.is_renderable());
    assert!(tile.is_loaded());
    assert!(tile.is_complete());
}

// gtest: VectorTile.Issue8542
#[test]
fn issue_8542() {
    let test = VectorTileTest::new();
    let tile = mvt_tile(&test);

    // Query before data is set
    let mut result = Vec::new();
    tile.query_source_features(&mut result, &SourceQueryOptions::new(Some(vec!["layer".to_owned()]), None), None)
        .unwrap();
}

/// Port-only: an in-process transport that records the messages between a
/// tile and its worker.
struct RecordingTransport {
    inner: InProcessTransport,
    log: Arc<Mutex<Vec<&'static str>>>,
}

struct RecordingWorker {
    inner: Box<dyn WorkerEndpoint>,
    log: Arc<Mutex<Vec<&'static str>>>,
}

struct RecordingTile {
    inner: Box<dyn TileEndpoint>,
    log: Arc<Mutex<Vec<&'static str>>>,
}

impl TileWorkerTransport for RecordingTransport {
    fn create(&self, parameters: WorkerParameters, tile: Box<dyn TileEndpoint>) -> Box<dyn WorkerEndpoint> {
        self.log.lock().unwrap().push("create");
        let tile = Box::new(RecordingTile { inner: tile, log: self.log.clone() });
        Box::new(RecordingWorker { inner: self.inner.create(parameters, tile), log: self.log.clone() })
    }

    fn create_raster(
        &self,
        tile: Box<dyn TileEndpoint<RasterTileMessage>>,
    ) -> Box<dyn WorkerEndpoint<RasterWorkerMessage>> {
        self.inner.create_raster(tile)
    }

    fn create_raster_dem(
        &self,
        tile: Box<dyn TileEndpoint<RasterDEMTileMessage>>,
    ) -> Box<dyn WorkerEndpoint<RasterDEMWorkerMessage>> {
        self.inner.create_raster_dem(tile)
    }
}

impl WorkerEndpoint for RecordingWorker {
    fn send(&self, message: WorkerMessage) {
        self.log.lock().unwrap().push(match &message {
            WorkerMessage::SetData { .. } => "setData",
            WorkerMessage::SetLayers { .. } => "setLayers",
            _ => "other",
        });
        self.inner.send(message);
    }
}

impl TileEndpoint for RecordingTile {
    fn send(&self, message: TileMessage) {
        self.log.lock().unwrap().push(match &message {
            TileMessage::Layout { .. } => "onLayout",
            _ => "other",
        });
        self.inner.send(message);
    }
}

// Port-only: the tile and its worker talk only through the transport.
#[test]
fn tile_and_worker_talk_through_the_transport() {
    let mut test = VectorTileTest::new();
    let log = Arc::new(Mutex::new(Vec::new()));
    test.transport =
        Rc::new(RecordingTransport { inner: InProcessTransport::new(test.thread_pool.clone()), log: log.clone() });
    let tile = mvt_tile(&test);
    tile.geometry_tile().set_data(None);
    tile.geometry_tile().set_layers(&[], None);

    let deadline = mln_util::time::Instant::now() + std::time::Duration::from_secs(10);
    while !tile.is_complete() {
        assert!(mln_util::time::Instant::now() < deadline, "the worker did not reply");
        test.run_loop.run_once();
        std::thread::yield_now();
    }
    assert!(tile.is_renderable());
    assert_eq!(*log.lock().unwrap(), ["create", "setData", "setLayers", "onLayout"]);
}
