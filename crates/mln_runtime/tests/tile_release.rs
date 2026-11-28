//! What layers draw and hold as they leave and return to their zoom range,
//! as frames and their rendering stats show it.

use std::sync::Arc;

use mln_engine::camera::{CameraOptions, CameraUpdate, CameraUpdateMode};
use mln_engine::map::{MapEvent, MapMode, RenderFrameStatus};
use mln_engine::storage::{Kind, Response};
use mln_gfx::rendering_stats::RenderingStats;
use mln_runtime::{FrameResult, MapOptions, RuntimeEventKind, TileOptions};
use mln_style::bitmap::PremultipliedImage;
use mln_style::geo::{LatLng, LatLngBounds};
use mln_style::size::Size;
use mln_style::tile_id::CanonicalTileID;

use crate::support::still::{StillMap, StillOptions, center_pixel, file_response, ll, not_found, read_bytes, serve};
use crate::support::{eventually, wait};

/// The report of the frame that `result` finished, once it reached the
/// map's events.
#[track_caller]
fn frame_status(map: &StillMap, result: &FrameResult) -> RenderFrameStatus {
    let reports = || -> Vec<RenderFrameStatus> {
        map.events
            .events()
            .into_iter()
            .filter_map(|event| match event.kind {
                RuntimeEventKind::Map(MapEvent::RenderFrameFinished(status)) => Some(status),
                _ => None,
            })
            .collect()
    };
    let drawn = usize::try_from(result.frame_generation).expect("a frame count");
    assert!(drawn > 0, "the frame didn't render");
    assert!(eventually(|| reports().len() >= drawn), "the frame's report never reached the map");
    reports().swap_remove(drawn - 1)
}

const CIRCLE_STYLE: &str = r#"{"version":8,"sources":{"points":{"type":"geojson","data":{
    "type":"Point","coordinates":[0,0]}}},"layers":[{"id":"circle","type":"circle","source":"points"}]}"#;

/// Each frame's rendering stats count its draws, and the running counts
/// grow by them.
#[test]
fn rendering_stats_count_the_draws_of_each_frame() {
    let map = StillMap::new(StillOptions { mode: MapMode::Continuous, ..StillOptions::default() });
    map.load_json(CIRCLE_STYLE);
    map.settle();
    let first = frame_status(&map, &map.frame()).rendering_stats;
    assert!(first.num_frames > 0);
    assert!(first.num_draw_calls > 0);
    assert!(first.total_draw_calls >= first.num_draw_calls);

    let second = frame_status(&map, &map.frame()).rendering_stats;
    assert_eq!(first.num_frames + 1, second.num_frames);
    assert!(second.num_draw_calls > 0);
    assert_eq!(first.total_draw_calls + second.num_draw_calls, second.total_draw_calls);

    let hidden = wait(map.map.set_style_layer_property("circle", "visibility", r#""none""#));
    let frame = map.frame();
    assert!(frame.map_update_generation >= hidden.render_update_generation);
    let empty = frame_status(&map, &frame).rendering_stats;
    assert_eq!(second.num_frames + 1, empty.num_frames);
    assert_eq!(0, empty.num_draw_calls);
    assert_eq!(second.total_draw_calls, empty.total_draw_calls);
}

/// Layers without a source draw what they drew before once they return to
/// their zoom range. A second background, which isn't drawn as the clear
/// color, stands in for the C++ test's location indicator layer, a type
/// that mln-rs doesn't have.
#[test]
fn sourceless_layers_draw_again_once_they_return_to_their_zoom_range() {
    let map = StillMap::still();
    map.load_json(
        r#"{
      "version": 8,
      "sources": {},
      "layers": [
        {"id": "background", "type": "background", "minzoom": 10, "paint": {"background-color": "red"}},
        {"id": "tint", "type": "background", "minzoom": 10,
         "paint": {"background-color": "blue", "background-opacity": 0.5}}
      ]
    }"#,
    );
    let render = |zoom: f64| {
        map.jump(CameraOptions { center: Some(ll(0.0, 0.0)), zoom: Some(zoom), ..Default::default() });
        map.render()
    };

    let image = render(16.0);
    let center = center_pixel(&image);
    assert!(center[0] > 0 && center[2] > 0 && center[3] == 255, "{center:?}");
    assert!(render(5.0).data.iter().all(|&v| v == 0));
    assert!(render(16.0) == image, "the layers draw otherwise once back in their zoom range");
}

/// At zoom 16.1 to 16.45, a 256 pixel view shows one raster tile at the
/// small center, and four at the large one.
const SMALL_TILE: CanonicalTileID = CanonicalTileID { z: 15, x: 16484, y: 16283 };

fn small_center() -> LatLng {
    LatLngBounds::from_canonical_tile(SMALL_TILE.z, SMALL_TILE.x, SMALL_TILE.y).center()
}

fn large_center() -> LatLng {
    ll(0.0, 0.0)
}

/// A few features at the small center, and many at the large one.
fn features() -> String {
    let mut features: Vec<String> = Vec::new();
    let mut feature = |geometry: String| {
        features.push(format!(r#"{{"type":"Feature","properties":{{}},"geometry":{geometry}}}"#));
    };
    let point = |center: &LatLng, dx: f64, dy: f64| format!("[{},{}]", center.longitude() + dx, center.latitude() + dy);

    let small = small_center();
    feature(format!(
        r#"{{"type":"Polygon","coordinates":[[{},{},{},{}]]}}"#,
        point(&small, 0.0, 0.0),
        point(&small, 0.001, 0.0),
        point(&small, 0.0, 0.001),
        point(&small, 0.0, 0.0)
    ));
    feature(format!(
        r#"{{"type":"LineString","coordinates":[{},{}]}}"#,
        point(&small, 0.0, 0.0),
        point(&small, 0.001, 0.001)
    ));
    feature(format!(r#"{{"type":"Point","coordinates":{}}}"#, point(&small, 0.0, 0.0)));

    let large = large_center();
    let ring: Vec<String> = (0..=256)
        .map(|i| {
            let angle = std::f64::consts::TAU * f64::from(i % 256) / 256.0;
            point(&large, 0.002 * angle.cos(), 0.002 * angle.sin())
        })
        .collect();
    feature(format!(r#"{{"type":"Polygon","coordinates":[[{}]]}}"#, ring.join(",")));
    for row in 0..32 {
        let mut line = Vec::new();
        for column in 0..32 {
            let coordinates =
                point(&large, -0.002 + f64::from(column) * 0.000_125, -0.002 + f64::from(row) * 0.000_125);
            feature(format!(r#"{{"type":"Point","coordinates":{coordinates}}}"#));
            line.push(coordinates);
        }
        feature(format!(r#"{{"type":"LineString","coordinates":[{}]}}"#, line.join(",")));
    }
    format!(r#"{{"type":"FeatureCollection","features":[{}]}}"#, features.join(","))
}

/// The GPU resources of the stats that the test compares.
fn resources(stats: &RenderingStats) -> [i32; 5] {
    [
        stats.num_vertex_buffers,
        stats.mem_vertex_buffers,
        stats.num_index_buffers,
        stats.mem_index_buffers,
        stats.num_active_textures,
    ]
}

const NO_FEATURES: &str = r#"{"type":"circle","filter":["==",["get","missing"],1]}"#;
const DEM_KEEPER: &str = r#"{"type":"color-relief","paint":{"color-relief-opacity":0}}"#;

/// A layer outside its zoom range, and a source whose layers all are,
/// release the tiles they last drew, for each layer type: the GPU resources
/// after a layer with little data leaves are those after one with much data
/// leaves, and the layer draws the same once it returns.
#[test]
fn layers_release_their_tiles_outside_their_zoom_range() {
    // The source, a layer object without "id", "source", "minzoom" or
    // "maxzoom", and a layer of the same source that draws nothing.
    let cases = [
        ("geojson", r#"{"type":"fill","paint":{"fill-color":"red"}}"#, NO_FEATURES),
        ("geojson", r#"{"type":"line","paint":{"line-width":4}}"#, NO_FEATURES),
        ("geojson", r#"{"type":"circle"}"#, NO_FEATURES),
        ("geojson", r#"{"type":"fill-extrusion","paint":{"fill-extrusion-height":100}}"#, NO_FEATURES),
        (
            "geojson",
            r#"{"type":"symbol","layout":{"text-field":"x","text-font":["Open Sans Regular"],"text-allow-overlap":true}}"#,
            NO_FEATURES,
        ),
        ("geojson", r#"{"type":"heatmap"}"#, NO_FEATURES),
        ("raster", r#"{"type":"raster"}"#, r#"{"type":"raster","paint":{"raster-opacity":0}}"#),
        ("raster-dem", r#"{"type":"hillshade"}"#, DEM_KEEPER),
        (
            "raster-dem",
            r#"{"type":"color-relief","paint":{"color-relief-color":["interpolate",["linear"],["elevation"],-10000,"black",10000,"white"]}}"#,
            DEM_KEEPER,
        ),
    ];
    for (source, layer, keeper) in cases {
        releases_tiles_outside_zoom_range(source, layer, keeper);
    }
}

fn releases_tiles_outside_zoom_range(source: &str, layer: &str, keeper: &str) {
    let map = StillMap::new(StillOptions {
        mode: MapMode::Continuous,
        map: MapOptions { tile_cache_enabled: false, ..StillOptions::default().map },
        ..StillOptions::default()
    });
    wait(map.map.set_tile_options(TileOptions { prefetch_zoom_delta: Some(0), ..TileOptions::default() }));
    let small_tile = Arc::new(mln_engine::image::encode_png(&PremultipliedImage::new(Size::new(4, 4))));
    let large_tile = Arc::new(read_bytes("test/fixtures/resources/raster.tile"));
    serve(&map.runtime, move |resource| match (resource.kind, &resource.tile_data) {
        (Kind::Glyphs, _) => file_response("test/fixtures/resources/glyphs.pbf"),
        (Kind::Tile, Some(tile)) => {
            if tile.z == 15 && tile.x == SMALL_TILE.x as i32 && tile.y == SMALL_TILE.y as i32 {
                Response { data: Some(Arc::clone(&small_tile)), ..Response::default() }
            } else if tile.z == 15 && (tile.x == 16383 || tile.x == 16384) && (tile.y == 16383 || tile.y == 16384) {
                Response { data: Some(Arc::clone(&large_tile)), ..Response::default() }
            } else {
                // The tileset has no other tiles (HTTP 404), so a parent is
                // drawn in their place. An empty raster tile (HTTP 204)
                // would draw nothing, as in GL JS.
                Response { no_content: true, tile_not_found: true, ..Response::default() }
            }
        }
        _ => not_found(),
    });

    let source_json = if source == "geojson" {
        format!(r#"{{"type":"geojson","data":{}}}"#, features())
    } else {
        format!(r#"{{"type":"{source}","tiles":["https://example.com/{{z}}/{{x}}/{{y}}.png"]}}"#)
    };
    let layer_json =
        |id: &str, zoom: &str, json: &str| format!(r#"{{"id":"{id}","source":"data",{zoom},{}"#, &json[1..]);
    map.load_json(&format!(
        r#"{{"version":8,"glyphs":"https://example.com/{{fontstack}}/{{range}}.pbf","sources":{{"data":{source_json}}},"layers":[{},{}]}}"#,
        layer_json("layer", r#""minzoom":16.4"#, layer),
        layer_json("keeper", r#""minzoom":16.2,"maxzoom":16.35"#, keeper),
    ));

    // Tiles that a source drops are released on the render thread during
    // the frame, so the frame's stats already count them out.
    let render = |center: LatLng, zoom: f64| -> (PremultipliedImage, Arc<RenderingStats>) {
        let camera = CameraOptions { center: Some(center), zoom: Some(zoom), ..Default::default() };
        wait(map.map.update_camera(CameraUpdate { mode: CameraUpdateMode::Jump, camera, ..CameraUpdate::default() }));
        let settled = map.settle();
        (map.image(), frame_status(&map, &settled).rendering_stats)
    };
    let leave_zoom_range = |center: LatLng| {
        // The layer leaves its zoom range while the keeper still draws the
        // source.
        render(center, 16.3);
        // The keeper leaves its zoom range too, so the source stops
        // drawing.
        render(center, 16.1).1
    };

    // Draw the layer with the large data first: the context's buffers of
    // default attribute values grow to the largest drawable they serve, and
    // stay.
    render(large_center(), 16.45);
    leave_zoom_range(large_center());
    // Draw the layer once with little data, so it makes whatever it keeps
    // between frames.
    assert!(render(small_center(), 16.45).1.num_draw_calls > 0, "{layer}");
    let released = leave_zoom_range(small_center());

    let (image, rendered) = render(large_center(), 16.45);
    assert!(image.data.iter().any(|&v| v != 0), "{layer}");
    assert_eq!(resources(&released), resources(&leave_zoom_range(large_center())), "{layer}: released");

    let (image_again, rendered_again) = render(large_center(), 16.45);
    assert_eq!(resources(&rendered), resources(&rendered_again), "{layer}: drawn again");
    assert!(image == image_again, "{layer}: drawn again");
}
