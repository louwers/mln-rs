// Ports the VectorTile tests of test/tile/vector_tile.test.cpp (the
// VectorTileData ones are mln_tile's).

use std::rc::Rc;
use std::sync::Arc;

use mln_actor::{RunLoop, TaggedScheduler, scheduler};
use mln_map::transform::{NullTransformObserver, Transform};
use mln_map_types::{ConstrainMode, MapDebugOptions, MapMode, TileLodMode, ViewportMode};
use mln_renderer::query::SourceQueryOptions;
use mln_renderer::tile::tile::Tile;
use mln_renderer::tile::tile_loader::{LoaderTile, VectorTileEncoding};
use mln_renderer::tile::vector_tile::VectorTile;
use mln_renderer::tile_parameters::TileParameters;
use mln_storage::stub_file_source::{ResponseType, StubFileSource};
use mln_tile_id::OverscaledTileID;
use mln_util::exception::RuntimeError;
use mln_util::identity::SimpleIdentity;
use mln_util::subdivision_granularity::SubdivisionGranularitySetting;
use mln_util::tileset::Tileset;

/// `VectorTileTest`.
struct VectorTileTest {
    _run_loop: RunLoop,
    transform: Transform,
    thread_pool: TaggedScheduler,
    file_source: Arc<StubFileSource>,
    tileset: Tileset,
}

impl VectorTileTest {
    fn new() -> Self {
        VectorTileTest {
            _run_loop: RunLoop::new(),
            transform: Transform::new(Rc::new(NullTransformObserver), ConstrainMode::HeightOnly, ViewportMode::Default),
            thread_pool: TaggedScheduler::new(scheduler::get_background(), SimpleIdentity::new()),
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
            thread_pool: self.thread_pool.clone(),
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
    LoaderTile::set_error(&**tile.geometry_tile().inner(), RuntimeError::ptr("test"));
    assert!(!tile.is_renderable());
    assert!(tile.is_loaded());
    assert!(tile.is_complete());
}

// gtest: VectorTile.onError
#[test]
fn on_error() {
    let test = VectorTileTest::new();
    let tile = mvt_tile(&test);
    tile.geometry_tile().inner().on_error(RuntimeError::ptr("test"), 0);

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
    tile.query_source_features(&mut result, &SourceQueryOptions::new(Some(vec!["layer".to_owned()]), None), None);
}
