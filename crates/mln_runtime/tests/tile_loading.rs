//! The tiles that a map loads for its view, and what it draws while they
//! load.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use mln_engine::TileOperation;
use mln_engine::camera::CameraOptions;
use mln_engine::map::{MapEvent, MapMode};
use mln_engine::storage::{Kind, Response};
use mln_runtime::{ResourceRequest, RuntimeEventKind, RuntimeOptions, TileOptions};
use mln_style::size::Size;
use mln_style::tile_id::CanonicalTileID;
use parking_lot::Mutex;

use crate::support::still::{
    StillMap, StillOptions, check_image, file_response, fixtures_root, ll, not_found, read_bytes, serve,
};
use crate::support::{COMPLETION_TIMEOUT, wait};

fn zoom(zoom: f64) -> CameraOptions {
    CameraOptions { zoom: Some(zoom), ..Default::default() }
}

fn no_content() -> Response {
    Response { no_content: true, ..Response::default() }
}

/// A layer below its minzoom draws nothing, though its source's tiles load
/// for another layer.
#[test]
fn disabled_sources() {
    // Every raster tile is the same image.
    let assets = fixtures_root().join("test/fixtures/map/disabled_layers");
    let runtime_options = RuntimeOptions { asset_path: Some(assets), ..RuntimeOptions::default() };
    let map = StillMap::new(StillOptions { runtime_options, ..StillOptions::default() });
    map.jump(zoom(1.0));

    // The style has two raster layers, one that starts at zoom 1, the
    // other at zoom 0. At zoom 1 both show, at an opacity of 0.5; at zoom
    // 0.5 only "raster1" does. They differ in color through
    // "raster-hue-rotate".
    map.load_json(
        r#"
{
  "version": 8,
  "name": "Test",
  "sources": {
    "raster": {
      "type": "raster",
      "tiles": [ "asset://tile.png" ],
      "tileSize": 256
    }
  },
  "layers": [{
    "id": "background",
    "type": "background",
    "paint": {
      "background-color": "white"
    }
  }, {
    "id": "raster1",
    "type": "raster",
    "source": "raster",
    "minzoom": 0
  }, {
    "id": "raster2",
    "type": "raster",
    "source": "raster",
    "minzoom": 1,
    "paint": {
      "raster-hue-rotate": 180
    }
  }]
}
"#,
    );

    check_image("test/fixtures/map/disabled_layers/first", &map.render(), 0.0, 0.0);
    map.jump(zoom(0.5));
    check_image("test/fixtures/map/disabled_layers/second", &map.render(), 0.0, 0.0);
}

/// A map loads tiles only at the zooms where a layer of their source shows.
#[test]
fn dont_load_unneeded_tiles() {
    let map = StillMap::still();
    map.load_json(
        r#"{
  "sources": {
    "a": { "type": "vector", "tiles": [ "a/{z}/{x}/{y}" ] }
  },
  "layers": [{
    "id": "a",
    "type": "fill",
    "source": "a",
    "source-layer": "a",
    "minzoom": 0.3,
    "maxzoom": 1.6
  }]
}"#,
    );
    let requests = serve(&map.runtime, |_| no_content());

    let expected = |z: f64| -> BTreeSet<String> {
        let tiles: &[&str] = if z == 0.3 {
            // The layer's minzoom is 0.3: no tiles load below it.
            &["a/0/0/0"]
        } else if z == 1.0 {
            &["a/1/1/0", "a/1/0/1", "a/1/0/0", "a/1/1/1"]
        } else {
            // The layer's maxzoom is 1.6: no z2 or z3 tiles load.
            &[]
        };
        tiles.iter().map(ToString::to_string).collect()
    };

    // Each tenth of a zoom level from 0 to 3, computed from an integer so it
    // compares exactly.
    for tenths in 0..=30u32 {
        let z = f64::from(tenths) / 10.0;
        requests.clear();
        map.jump(zoom(z));
        map.render();
        let loaded: BTreeSet<String> = requests.urls(Kind::Tile).into_iter().collect();
        assert_eq!(expected(z), loaded, "zoom level {z}");
    }
}

/// A continuous map whose style switches to one without layers, past its
/// tiles' zooms, stops repainting: the fading tiles of the style before it
/// kept it repainting (mapbox/mapbox-gl-native#15342).
#[test]
fn issue15342() {
    let map = StillMap::continuous();
    serve(&map.runtime, |_| file_response("test/fixtures/map/issue12432/0-0-0.mvt"));
    map.jump(zoom(3.0));
    map.load_json(
        r#"{
      "version": 8,
      "sources": {
        "mapbox": {
          "type": "vector",
          "tiles": ["http://example.com/{z}-{x}-{y}.vector.pbf"]
        }
      },
      "layers": [{
        "id": "water",
        "type": "fill",
        "source": "mapbox",
        "source-layer": "water"
      }]
    }"#,
    );
    map.settle();

    map.load_json(
        r#"{
      "version": 8,
      "sources": {
        "mapbox": {
          "type": "vector",
          "tiles": ["http://example.com/{z}-{x}-{y}.vector.pbf"]
        }
      },
      "layers": []
    }"#,
    );
    map.jump(zoom(20.0));
    map.settle();
}

/// The map-wide prefetch zoom delta decides how many zoom levels lower a
/// map also loads tiles for: none at 0, and the parent tiles 4 levels up at
/// 4.
#[test]
fn prefetch_delta_override() {
    let map = StillMap::continuous();
    let requests = serve(&map.runtime, |_| no_content());
    wait(map.map.set_tile_options(TileOptions { prefetch_zoom_delta: Some(0), ..TileOptions::default() }));
    map.load_json(
        r#"{
                "layers": [{
                    "id": "vector",
                    "type": "fill",
                    "source": "vector",
                    "minzoom": 0,
                    "maxzoom": 24
                },
                {
                    "id": "custom",
                    "type": "fill",
                    "source": "custom",
                    "minzoom": 0,
                    "maxzoom": 24
                }]
                }"#,
    );
    wait(map.map.add_style_source_json("vector", r#"{"type": "vector", "tiles": ["a/{z}/{x}/{y}"]}"#));
    wait(map.map.add_style_source_json("custom", r#"{"type": "vector", "tiles": ["b/{z}/{x}/{y}"]}"#));

    map.jump(zoom(16.0));
    map.settle();
    // 2 sources x 4 tiles.
    assert_eq!(requests.count(Kind::Tile), 8, "{:?}", requests.all());

    requests.clear();
    wait(map.map.set_tile_options(TileOptions { prefetch_zoom_delta: Some(4), ..TileOptions::default() }));
    map.settle();
    // Each source loads the 4 tiles at z12 over the view.
    assert_eq!(requests.count(Kind::Tile), 8, "{:?}", requests.all());
}

/// On the globe, a map also loads the tiles of coarser zooms, down by the
/// prefetch zoom delta.
#[test]
fn globe_prefetch_requests_coarser_tiles() {
    let map = StillMap::continuous();
    let zooms: Arc<Mutex<BTreeSet<i8>>> = Arc::default();
    serve(&map.runtime, {
        let zooms = Arc::clone(&zooms);
        move |resource| {
            if let Some(tile) = &resource.tile_data {
                zooms.lock().insert(tile.z);
            }
            no_content()
        }
    });
    map.load_json(
        r#"{"version": 8, "projection": {"type": "vertical-perspective"}, "sources": {},
                 "layers": [{"id": "vector", "type": "fill", "source": "vector"}]}"#,
    );
    wait(map.map.add_style_source_json("vector", r#"{"type": "vector", "tiles": ["a/{z}/{x}/{y}"]}"#));
    map.jump(CameraOptions { center: Some(ll(40.0, 10.0)), zoom: Some(8.0), ..Default::default() });
    map.settle();

    let zooms = zooms.lock();
    assert!(zooms.contains(&4), "{zooms:?}");
    assert!(zooms.contains(&8), "{zooms:?}");
}

/// On the globe, a tile drawn in place of children that never arrive meets
/// its finer neighbor without a gap, though it draws their shared edge as one
/// chord and the neighbor as two.
#[test]
fn globe_tile_drawn_for_its_children_meets_finer_tiles_without_a_gap() {
    let map =
        StillMap::new(StillOptions { size: Size::new(512, 512), mode: MapMode::Continuous, ..StillOptions::default() });
    let ocean = Arc::new(read_bytes("test/fixtures/map/globe_tile_masks/ocean.mvt"));
    let hold_children = Arc::new(AtomicBool::new(false));
    let served: Arc<Mutex<BTreeSet<CanonicalTileID>>> = Arc::default();
    let held: Arc<Mutex<Vec<ResourceRequest>>> = Arc::default();
    {
        let (hold_children, served, held) = (Arc::clone(&hold_children), Arc::clone(&served), Arc::clone(&held));
        wait(map.runtime.set_resource_provider(move |request: ResourceRequest| {
            let Some(tile) = request.resource().tile_data.clone() else {
                let _ = request.respond(not_found());
                return;
            };
            // The children of tile 7/20/49.
            if hold_children.load(Ordering::SeqCst) && tile.z == 8 && tile.x / 2 == 20 && tile.y / 2 == 49 {
                held.lock().push(request);
                return;
            }
            served.lock().insert(CanonicalTileID::new(tile.z as u8, tile.x as u32, tile.y as u32));
            let _ = request.respond(Response { data: Some(Arc::clone(&ocean)), ..Response::default() });
        }));
    }
    map.load_json(
        r#"{"version": 8, "projection": {"type": "vertical-perspective"},
                 "sources": {"ocean": {"type": "vector", "tiles": ["a/{z}/{x}/{y}"]}},
                 "layers": [{"id": "background", "type": "background", "paint": {"background-color": "red"}},
                            {"id": "ocean", "type": "fill", "source": "ocean", "source-layer": "water",
                             "paint": {"fill-color": "blue"}}]}"#,
    );

    // The view's center is the corner of tile 7/20/49.
    map.jump(CameraOptions { center: Some(ll(36.6026, -120.9375)), zoom: Some(7.6), ..Default::default() });
    map.settle();

    hold_children.store(true, Ordering::SeqCst);
    served.lock().clear();
    let events_before = map.events.events().len();
    map.jump(zoom(8.2));
    // Frames until one draws after every tile served has parsed.
    let deadline = Instant::now() + COMPLETION_TIMEOUT;
    let mut all_parsed = false;
    while !all_parsed {
        map.frame();
        let parsed: BTreeSet<CanonicalTileID> = map.events.events()[events_before..]
            .iter()
            .filter_map(|event| match &event.kind {
                RuntimeEventKind::Map(MapEvent::TileAction { op: TileOperation::EndParse, id, .. }) => {
                    Some(id.canonical)
                }
                _ => None,
            })
            .collect();
        let served = served.lock();
        all_parsed = !served.is_empty() && served.is_subset(&parsed);
        assert!(Instant::now() < deadline, "served {served:?}, parsed {parsed:?}");
    }
    let image = map.image();

    assert!(!held.lock().is_empty());
    let background = image.data.as_chunks::<4>().0.iter().filter(|pixel| pixel[0] > 128 && pixel[2] < 128).count();
    assert_eq!(0, background);
}

/// The tiles of a map's view load once while its render data stays, and
/// again for each image after the render data is cleared.
#[test]
fn keep_render_data() {
    let map = StillMap::still();
    let requests = serve(&map.runtime, |resource| match resource.kind {
        Kind::Tile => file_response("test/fixtures/resources/vector.tile"),
        Kind::Glyphs => file_response("test/fixtures/resources/glyphs.pbf"),
        Kind::Style => file_response("test/fixtures/resources/style_vector.json"),
        Kind::Source => file_response("test/fixtures/resources/source_vector.json"),
        Kind::SpriteJSON => file_response("test/fixtures/resources/sprite.json"),
        Kind::SpriteImage => file_response("test/fixtures/resources/sprite.png"),
        _ => not_found(),
    });
    let loads = || requests.count(Kind::Tile) + requests.count(Kind::Glyphs);

    map.jump(zoom(10.0));
    map.load_url("https://example.com/maps/streets");
    let iterations = 3;
    // The view's tiles.
    let resources = 4;

    requests.clear();
    for _ in 1..=iterations {
        map.render();
        assert_eq!(resources, loads(), "{:?}", requests.all());
    }

    requests.clear();
    for i in 1..=iterations {
        wait(map.session.clear_data());
        map.render();
        assert_eq!(resources * i, loads(), "{:?}", requests.all());
    }
}
