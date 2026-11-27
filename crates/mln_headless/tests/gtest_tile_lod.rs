use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use mln_engine::camera::CameraOptions;
use mln_engine::exec::pool::Pool;
use mln_engine::map::{Map, MapMode, MapOptions, TileSettings};
use mln_engine::storage::stub_file_source::{ResponseType, StubFileSource};
use mln_engine::storage::{Kind, Response};
use mln_headless::HeadlessFrontend;
use mln_style::Edit;
use mln_style::conversion::Convertible;
use mln_style::layer::Layer;
use mln_style::sources::vector_source;
use mln_style::tileset::PartialTileset;

use crate::support::*;

/// Changes the map's tile settings.
fn set_tile(map: &mut Map, change: impl FnOnce(&mut TileSettings)) {
    let mut tile = map.snapshot().tile;
    change(&mut tile);
    map.set_tile_settings(&tile);
}

/// A map whose transparent circles count the tiles it loads: the tile
/// requests of a vector source of zooms 0 to 18, as C++ counts a custom
/// geometry source's fetches. C++'s tests
/// also compare images of the tile borders, a debug overlay that mln-rs
/// doesn't draw, so these check the counts.
struct TileLODTest {
    _run_loop: TestLoop,
    frontend: HeadlessFrontend,
    map: Map,
    tile_count: Arc<AtomicU32>,
}

impl TileLODTest {
    fn new(camera_options: &CameraOptions) -> Self {
        let run_loop = TestLoop::new();
        let pool = Pool::new(4);
        let mut frontend =
            HeadlessFrontend::with_pixel_ratio(1.0, run_loop.queue(), &pool).expect("a headless frontend");
        let file_source = StubFileSource::on_queue(run_loop.queue(), ResponseType::Asynchronous);
        let tile_count = Arc::new(AtomicU32::new(0));
        let counted = Arc::clone(&tile_count);
        file_source.set_response(Kind::Tile, move |_| {
            counted.fetch_add(1, Ordering::SeqCst);
            let mut response = Response::new();
            response.no_content = true;
            Some(response)
        });
        let mut map = Map::with_file_source(
            file_source,
            MapOptions { map_mode: MapMode::Static, size: frontend.size(), ..Default::default() },
            mln_builtins::registry(),
            &pool,
        )
        .unwrap();
        frontend.attach(&mut map);
        map.jump_to(camera_options);

        map.load_style_json(&read_file("test/fixtures/api/empty.json"));

        let tileset = PartialTileset {
            tiles: Some(vec!["custom/{z}/{x}/{y}".to_owned()]),
            max_zoom: Some(18),
            ..PartialTileset::default()
        };
        let source = vector_source("custom", None, tileset);
        map.add_source(source).unwrap();

        let mut layer = Layer::new(mln_builtins::registry().style(), "circle", "circles", "custom").unwrap();
        let error = layer.set_property("circle-color", &Convertible::from(&serde_json::json!("rgba(0,0,255,0)")), None);
        assert!(error.is_none(), "{error:?}");
        map.edit_style(|style| style.apply(Edit::AddLayer { layer, before: None })).unwrap();

        TileLODTest { _run_loop: run_loop, frontend, map, tile_count }
    }

    fn render(&mut self) {
        self.frontend.render(&mut self.map).expect("render");
    }

    fn tile_count(&self) -> u32 {
        self.tile_count.load(Ordering::SeqCst)
    }
}

fn zoom(zoom: f64) -> CameraOptions {
    CameraOptions { zoom: Some(zoom), ..Default::default() }
}

fn pitch(pitch: f64) -> CameraOptions {
    CameraOptions { pitch: Some(pitch), ..Default::default() }
}

// gtest: TileLOD.disabled
#[test]
fn disabled() {
    let mut test = TileLODTest::new(&zoom(10.0));

    set_tile(&mut test.map, |tile| tile.lod_pitch_threshold = 180.0);

    test.render();
    assert_eq!(test.tile_count(), 4);
}

// gtest: TileLOD.zoomShift
#[test]
fn zoom_shift() {
    let mut test = TileLODTest::new(&zoom(12.0));

    set_tile(&mut test.map, |tile| tile.lod_zoom_shift = 4.0);
    test.render();
    assert_eq!(test.tile_count(), 64);

    test.tile_count.store(0, Ordering::SeqCst);

    set_tile(&mut test.map, |tile| tile.lod_zoom_shift = -4.0);
    test.render();
    assert_eq!(test.tile_count(), 4);
}

// gtest: TileLOD.pitchThreshold
#[test]
fn pitch_threshold() {
    let mut test = TileLODTest::new(&zoom(10.0));

    let threshold = 45.0;
    set_tile(&mut test.map, |tile| tile.lod_zoom_shift = 4.0);
    set_tile(&mut test.map, |tile| tile.lod_min_radius = 1.0);
    set_tile(&mut test.map, |tile| tile.lod_scale = 1.0);
    set_tile(&mut test.map, |tile| tile.lod_pitch_threshold = threshold);

    test.map.jump_to(&pitch(threshold - 1.0));
    test.render();
    assert_eq!(test.tile_count(), 140);
    test.tile_count.store(0, Ordering::SeqCst);

    test.map.jump_to(&pitch(threshold + 1.0));
    test.render();
    assert_eq!(test.tile_count(), 22);
}

// gtest: TileLOD.scale
#[test]
fn scale() {
    let mut test = TileLODTest::new(&zoom(0.0));

    set_tile(&mut test.map, |tile| tile.lod_min_radius = 1.0);
    set_tile(&mut test.map, |tile| tile.lod_scale = 0.4);
    set_tile(&mut test.map, |tile| tile.lod_pitch_threshold = mln_style::math::rad2deg(-1.0));

    set_tile(&mut test.map, |tile| tile.lod_zoom_shift = 5.0);

    test.render();
    assert_eq!(test.tile_count(), 112);
}

// gtest: TileLOD.radius
#[test]
fn radius() {
    let mut test = TileLODTest::new(&zoom(0.0));

    set_tile(&mut test.map, |tile| tile.lod_min_radius = 5.0);
    set_tile(&mut test.map, |tile| tile.lod_scale = 1.0);
    set_tile(&mut test.map, |tile| tile.lod_pitch_threshold = mln_style::math::rad2deg(-1.0));

    set_tile(&mut test.map, |tile| tile.lod_zoom_shift = 5.0);

    test.render();
    assert_eq!(test.tile_count(), 172);
}
