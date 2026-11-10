// Ported gtests of test/tile/*.test.cpp that need the renderer's tiles or
// mln_map's Transform (above mln_renderer), one module per C++ file. The
// other test/tile files are ported elsewhere: geojson_tile (geojson_tile_test),
// vector_tile (vector_tile_test, mln_tile), geometry_tile_data (mln_tile),
// tile_cache (mln_renderer::tile::tile_cache), tile_id (mln_tile_id).

mod custom_geometry_tile;
mod raster_dem_tile;
mod raster_tile;
mod tile_coordinate;
mod tile_lod;

use std::sync::Arc;

use mln_actor::{TaggedScheduler, scheduler};
use mln_map_types::{MapDebugOptions, MapMode, TileLodMode, TransformState};
use mln_renderer::tile_parameters::TileParameters;
use mln_storage::stub_file_source::{ResponseType, StubFileSource};
use mln_util::identity::SimpleIdentity;
use mln_util::range::Range;
use mln_util::tileset::Tileset;

/// The fixture shared by `RasterTileTest` and `RasterDEMTileTest`
/// (test/tile/raster_tile.test.cpp, raster_dem_tile.test.cpp).
pub struct RasterTileTest {
    // C++ uses a FakeFileSource whose requests never answer; the
    // asynchronous stub answers only when the run loop runs, and these tests
    // never run it.
    file_source: Arc<StubFileSource>,
    transform_state: TransformState,
    pub run_loop: mln_actor::RunLoop,
    thread_pool: TaggedScheduler,
    pub tileset: Tileset,
}

impl RasterTileTest {
    pub fn new() -> Self {
        RasterTileTest {
            file_source: Arc::new(StubFileSource::new(ResponseType::Asynchronous)),
            transform_state: TransformState::default(),
            run_loop: mln_actor::RunLoop::new(),
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

    pub fn tile_parameters(&self) -> TileParameters<'_> {
        let file_source: Arc<dyn mln_storage::FileSource> = self.file_source.clone();
        TileParameters {
            pixel_ratio: 1.0,
            debug_options: MapDebugOptions::NO_DEBUG,
            transform_state: &self.transform_state,
            file_source: Some(file_source),
            mode: MapMode::Continuous,
            prefetch_zoom_delta: 0,
            thread_pool: self.thread_pool.clone(),
            tile_lod_min_radius: 3.0,
            tile_lod_scale: 1.0,
            tile_lod_pitch_threshold: (60.0 / 180.0) * std::f64::consts::PI,
            tile_lod_zoom_shift: 0.0,
            tile_lod_mode: TileLodMode::Default,
            global_state: None,
            is_update_synchronous: false,
            capture_rendered_features: false,
            image_manager: mln_renderer::image_manager::ImageManager::create(),
            glyph_manager: mln_renderer::text::glyph_manager::GlyphManager::new(None),
            dynamic_texture_atlas: Arc::new(mln_renderer::gfx::dynamic_texture_atlas::DynamicTextureAtlas::new()),
            tile_worker_transport: std::rc::Rc::new(
                mln_renderer::tile::tile_worker_transport::InProcessTransport::new(self.thread_pool.clone()),
            ),
        }
    }
}

/// Port-only: a transport whose workers are in `count` worker instances on
/// threads, reached only through the worker codec.
pub fn serializing_transport(count: usize) -> std::rc::Rc<mln_renderer::tile::worker_codec::SerializingTransport> {
    use mln_renderer::tile::worker_codec::{SerializingTransport, ThreadWorkerInstances};
    SerializingTransport::new(|replies| Box::new(ThreadWorkerInstances::new(count, replies)))
}

/// Port-only: an opaque PNG whose pixels differ from their neighbours.
pub fn png(width: u32, height: u32) -> mln_storage::Data {
    let mut image = mln_util::image::PremultipliedImage::new(mln_util::size::Size::new(width, height));
    for (i, pixel) in image.data.chunks_exact_mut(4).enumerate() {
        pixel.copy_from_slice(&[(i % 251) as u8, (i / 7 % 253) as u8, (i * 3 % 255) as u8, 255]);
    }
    Arc::new(mln_image::encode_png(&image))
}

/// Port-only: an observer that records the tiles' errors.
#[derive(Default)]
pub struct ErrorObserver(pub std::cell::RefCell<Vec<String>>);

impl mln_renderer::tile::tile_observer::TileObserver for ErrorObserver {
    fn on_tile_error(&self, _tile: &mln_renderer::tile::TileBase, error: mln_util::exception::ExceptionPtr) {
        self.0.borrow_mut().push(error.to_string());
    }
}

impl Drop for RasterTileTest {
    fn drop(&mut self) {
        self.thread_pool.wait_for_empty();
    }
}
