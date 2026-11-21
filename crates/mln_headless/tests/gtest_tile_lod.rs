use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, OnceLock};

use geojson::{Feature, Geometry};
use mln_headless::HeadlessFrontend;
use mln_runtime::map::{CameraOptions, Map, MapDebugOptions, MapMode, MapOptions, TileSettings};
use mln_sources::CustomGeometrySourceHandle;
use mln_sources::custom::geometry::{self, Options as CustomGeometrySourceOptions};
use mln_storage::ResourceOptions;
use mln_style::Edit;
use mln_style::conversion::Convertible;
use mln_style::layer::Layer;
use mln_util::geo::LatLngBounds;
use mln_util::pool::Pool;

use crate::support::*;

/// Changes the map's tile settings.
fn set_tile(map: &mut Map, change: impl FnOnce(&mut TileSettings)) {
    let mut tile = map.snapshot().tile;
    change(&mut tile);
    map.set_tile_settings(&tile);
}

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
        let mut map = Map::with_resource_options(
            MapOptions { map_mode: MapMode::Static, size: frontend.size(), ..Default::default() },
            &ResourceOptions {
                asset_path: cpp_root().join("test/fixtures/api/assets").to_str().unwrap().to_owned(),
                ..ResourceOptions::default()
            },
            registry(),
            &pool,
        )
        .unwrap();
        frontend.attach(&mut map);
        map.jump_to(camera_options);

        map.set_debug(MapDebugOptions::TILE_BORDERS);
        map.load_style_json(&read_file("test/fixtures/api/empty.json"));

        let tile_count = Arc::new(AtomicU32::new(0));
        let handle = Arc::new(OnceLock::new());
        let counted = Arc::clone(&tile_count);
        let options = CustomGeometrySourceOptions {
            fetch_tile_function: Some(answer_with(&handle, move |tile_id| {
                let center = LatLngBounds::from_canonical_tile(tile_id.z, tile_id.x, tile_id.y).center();
                counted.fetch_add(1, Ordering::SeqCst);
                vec![Feature::from(Geometry::new_point([center.longitude(), center.latitude()]))]
            })),
            ..Default::default()
        };
        let source = geometry::create("custom", options, &pool).unwrap();
        handle.set(CustomGeometrySourceHandle::of(&source).unwrap()).unwrap();
        map.add_source(source).unwrap();

        let mut layer = Layer::new(mln_rs::builtin_registry().style(), "circle", "circles", "custom").unwrap();
        let error = layer.set_property("circle-color", &Convertible::from(&serde_json::json!("rgba(0,0,255,0)")), None);
        assert!(error.is_none(), "{error:?}");
        map.edit_style(|style| style.apply(Edit::AddLayer { layer, before: None })).unwrap();

        TileLODTest { _run_loop: run_loop, frontend, map, tile_count }
    }

    fn render(&mut self) -> mln_headless::RenderResult {
        self.frontend.render(&mut self.map).expect("render")
    }

    fn tile_count(&self) -> u32 {
        self.tile_count.load(Ordering::SeqCst)
    }

    #[track_caller]
    fn check_image(&mut self, image: &str, expected_tile_count: u32) {
        self.check_images(&[image], expected_tile_count, 0.0002);
    }

    #[track_caller]
    fn check_images(&mut self, images: &[&str], expected_tile_count: u32, image_threshold: f64) {
        let result = self.render();

        // check this after frontend.render()
        assert_eq!(self.tile_count(), expected_tile_count);

        let bases: Vec<String> = images.iter().map(|image| format!("test/fixtures/tile_lod/{image}")).collect();
        let bases: Vec<&str> = bases.iter().map(String::as_str).collect();
        check_images(&bases, &result.image, image_threshold, 0.01);
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

    test.check_image("disabled", 4);
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
    test.check_images(&["pitchThreshold-line", "pitchThreshold-polyline"], 22, 0.0015);
}

// gtest: TileLOD.scale
#[test]
fn scale() {
    let mut test = TileLODTest::new(&zoom(0.0));

    set_tile(&mut test.map, |tile| tile.lod_min_radius = 1.0);
    set_tile(&mut test.map, |tile| tile.lod_scale = 0.4);
    set_tile(&mut test.map, |tile| tile.lod_pitch_threshold = mln_math::rad2deg(-1.0));

    set_tile(&mut test.map, |tile| tile.lod_zoom_shift = 5.0);

    test.check_image("scale", 112);
}

// gtest: TileLOD.radius
#[test]
fn radius() {
    let mut test = TileLODTest::new(&zoom(0.0));

    set_tile(&mut test.map, |tile| tile.lod_min_radius = 5.0);
    set_tile(&mut test.map, |tile| tile.lod_scale = 1.0);
    set_tile(&mut test.map, |tile| tile.lod_pitch_threshold = mln_math::rad2deg(-1.0));

    set_tile(&mut test.map, |tile| tile.lod_zoom_shift = 5.0);

    test.check_image("radius", 172);
}
