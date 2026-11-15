// Ported gtests of test/tile/*.test.cpp that need the renderer's tiles or
// mln_map's Transform (above mln_renderer), one module per C++ file. The
// other test/tile files are ported elsewhere: geojson_tile (geojson_tile_test),
// vector_tile (vector_tile_test, mln_tile), geometry_tile_data (mln_tile),
// tile_cache (crate::tile::tile_cache), tile_id (mln_tile_id),
// tile_lod (render_test, which has the HeadlessFrontend).

mod custom_geometry_tile;
mod custom_vector_tile;
mod raster_dem_tile;
mod raster_tile;
mod tile_coordinate;

use std::sync::Arc;

use mln_actor::{TaggedScheduler, scheduler};
use mln_map_types::{MapDebugOptions, MapMode, TileLodMode, TransformState};
use mln_storage::stub_file_source::{ResponseType, StubFileSource};
use mln_util::identity::SimpleIdentity;
use mln_util::range::Range;
use mln_util::subdivision_granularity::SubdivisionGranularitySetting;
use mln_util::tileset::Tileset;

use crate::tile_parameters::TileParameters;

/// The fixture shared by `RasterTileTest` and `RasterDEMTileTest`
/// (`test/tile/raster_tile.test.cpp`, `raster_dem_tile.test.cpp`).
pub(crate) struct RasterTileTest {
    // C++ uses a FakeFileSource whose requests never answer; the
    // asynchronous stub answers only when the run loop runs, and these tests
    // never run it.
    file_source: Arc<StubFileSource>,
    transform_state: TransformState,
    pub(crate) run_loop: mln_actor::RunLoop,
    thread_pool: TaggedScheduler,
    pub(crate) tileset: Tileset,
}

impl RasterTileTest {
    #[must_use]
    pub(crate) fn new() -> Self {
        let run_loop = mln_actor::RunLoop::new();
        RasterTileTest {
            file_source: Arc::new(StubFileSource::new(&run_loop, ResponseType::Asynchronous)),
            transform_state: TransformState::default(),
            run_loop,
            thread_pool: TaggedScheduler::new(scheduler::get_background(), SimpleIdentity::new()),
            // `Tileset tileset{{"https://example.com"}, {0, 22}, "none"}`.
            tileset: Tileset {
                tiles: vec!["https://example.com".to_owned()],
                zoom_range: Range::new(0, 22),
                attribution: "none".to_owned(),
                ..Default::default()
            },
        }
    }

    pub(crate) fn tile_parameters(&self) -> TileParameters<'_> {
        let file_source: Arc<dyn mln_storage::FileSource> = Arc::<StubFileSource>::clone(&self.file_source);
        TileParameters {
            pixel_ratio: 1.0,
            debug_options: MapDebugOptions::NO_DEBUG,
            transform_state: &self.transform_state,
            file_source: Some(file_source),
            scheduler: self.run_loop.scheduler(),
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
            source: crate::tests::test_source(),
            jobs: None,
            available_images: Arc::default(),
            font_faces: None,
            thread_pool: self.thread_pool.clone(),
        }
    }
}

impl Default for RasterTileTest {
    fn default() -> Self {
        Self::new()
    }
}

impl RasterTileTest {
    /// A raster tile of `kind` at `id` whose errors go to `observer`.
    pub(crate) fn tile(
        &self,
        id: mln_tile_id::OverscaledTileID,
        kind: crate::tile::raster_tile::RasterKind,
        observer: Option<std::rc::Rc<dyn crate::tile::tile_observer::TileObserver>>,
    ) -> crate::tile::raster_tile::RasterTile {
        crate::tile::raster_tile::RasterTile::new(
            id,
            "testSource".to_owned(),
            kind,
            &self.tile_parameters(),
            &self.tileset,
            observer,
        )
    }

    /// A driver of tiles whose jobs run on the thread pool.
    pub(crate) fn pool_driver(&self) -> crate::tests::TileDriver {
        crate::tests::TileDriver::pool(&self.run_loop, &self.thread_pool)
    }

    /// A driver of tiles whose jobs run in `count` worker instances on
    /// threads, reached only through the worker codec.
    pub(crate) fn remote_driver(&self, count: usize) -> crate::tests::TileDriver {
        let workers = remote_workers(&self.run_loop, count);
        crate::tests::TileDriver::new(&self.run_loop, Box::new(crate::tile::worker_codec::RemoteJobs(workers)))
    }

    /// Runs the loop and updates `tiles` with `driver` until they are all
    /// complete.
    pub(crate) fn settle(&self, driver: &mut crate::tests::TileDriver, tiles: &[&dyn crate::tile::tile::Tile]) {
        let deadline = mln_util::time::Instant::now() + std::time::Duration::from_secs(30);
        loop {
            self.run_loop.run_once();
            driver.update(tiles);
            if tiles.iter().all(|tile| tile.is_complete()) {
                return;
            }
            assert!(mln_util::time::Instant::now() < deadline, "the tiles didn't complete");
            std::thread::yield_now();
        }
    }
}

/// Port-only: workers in `count` worker instances on threads, reached only
/// through the worker codec.
#[must_use]
pub(crate) fn remote_workers(
    run_loop: &mln_actor::RunLoop,
    count: usize,
) -> std::rc::Rc<crate::tile::worker_codec::RemoteWorkers> {
    use crate::tile::worker_codec::{RemoteWorkers, ThreadWorkerInstances};
    RemoteWorkers::new(&run_loop.scheduler(), |replies| Box::new(ThreadWorkerInstances::new(count, &replies)))
}

/// Port-only: an opaque PNG whose pixels differ from their neighbours.
#[must_use]
pub(crate) fn png(width: u32, height: u32) -> mln_storage::Data {
    let mut image = mln_util::image::PremultipliedImage::new(mln_util::size::Size::new(width, height));
    for (i, pixel) in image.data.as_chunks_mut::<4>().0.iter_mut().enumerate() {
        pixel.copy_from_slice(&[(i % 251) as u8, (i / 7 % 253) as u8, (i * 3 % 255) as u8, 255]);
    }
    Arc::new(mln_image::encode_png(&image))
}

/// Port-only: an observer that records the tiles' errors.
#[derive(Default)]
pub(crate) struct ErrorObserver(pub(crate) std::cell::RefCell<Vec<String>>);

impl crate::tile::tile_observer::TileObserver for ErrorObserver {
    fn on_tile_error(&self, _tile: &crate::tile::tile::TileBase, error: mln_util::error::Error) {
        self.0.borrow_mut().push(error.to_string());
    }
}

impl Drop for RasterTileTest {
    fn drop(&mut self) {
        self.thread_pool.wait_for_empty();
    }
}
