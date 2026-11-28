//! Layers that change after they drew: new properties, a new place in the
//! style, a new zoom, and a new viewport.

use mln_engine::TileOperation;
use mln_engine::camera::CameraOptions;
use mln_engine::map::MapEvent;
use mln_runtime::{RuntimeEventKind, RuntimeOptions};
use serde_json::json;

use crate::support::still::{StillMap, StillOptions, check_image, fixtures_root, ll, read_file};
use crate::support::wait;

/// A layer whose color comes to depend on the zoom draws the color of each
/// zoom that the map moves to after.
#[test]
fn a_layer_updated_to_depend_on_the_zoom_follows_it() {
    let map = StillMap::still();
    map.load_json(&read_file("test/fixtures/map/style_update_zoom_dependency/style.json"));
    map.jump(CameraOptions { zoom: Some(0.0), ..Default::default() });
    check_image("test/fixtures/map/style_update_zoom_dependency/initial_red", &map.render(), 0.0006, 0.1);

    let color = json!(["interpolate", ["linear"], ["zoom"], 0, "rgba(255,0,0,1)", 14, "rgba(0,255,0,1)"]);
    wait(map.map.set_style_layer_property("background", "background-color", color.to_string()));
    map.render();

    map.jump(CameraOptions { zoom: Some(14.0), ..Default::default() });
    check_image("test/fixtures/map/style_update_zoom_dependency/after_update_green", &map.render(), 0.0006, 0.1);
}

/// A style of the streets fixtures' water: a background, then the water as
/// a fill and as a wide line, in `order`.
fn water_layers_in(order: [&str; 2]) -> String {
    let layer = |id: &str| match id {
        "fill" => json!({"id": "fill", "type": "fill", "source": "mapbox", "source-layer": "water",
            "paint": {"fill-color": "blue"}}),
        _ => json!({"id": "line", "type": "line", "source": "mapbox", "source-layer": "water",
            "paint": {"line-color": "red", "line-width": 12}}),
    };
    json!({
        "version": 8,
        "sources": {"mapbox": {"type": "vector", "tiles": ["asset://streets/{z}-{x}-{y}.vector.pbf"]}},
        "layers": [{"id": "background", "type": "background", "paint": {"background-color": "white"}},
            layer(order[0]), layer(order[1])],
    })
    .to_string()
}

/// A still map of `style` over the streets fixtures, around them.
fn streets_map(style: &str) -> StillMap {
    let map = StillMap::new(StillOptions {
        runtime_options: RuntimeOptions {
            asset_path: Some(fixtures_root().join("test/fixtures/api/assets")),
            ..RuntimeOptions::default()
        },
        ..StillOptions::default()
    });
    map.load_json(style);
    map.jump(CameraOptions { center: Some(ll(37.8, -122.5)), zoom: Some(10.0), ..Default::default() });
    map
}

/// How many tiles the map started to parse after the first `from` of its
/// events.
fn parses_since(map: &StillMap, from: usize) -> usize {
    map.events.events()[from..]
        .iter()
        .filter(|event| {
            matches!(event.kind, RuntimeEventKind::Map(MapEvent::TileAction { op: TileOperation::StartParse, .. }))
        })
        .count()
}

/// A moved layer draws in its new place without laying its tiles out again,
/// as a style with the layers in that order draws them.
#[test]
fn a_moved_layer_draws_in_its_new_place_without_a_relayout() {
    let map = streets_map(&water_layers_in(["fill", "line"]));
    let before = map.render();
    assert!(parses_since(&map, 0) > 0);

    let from = map.events.events().len();
    wait(map.map.move_style_layer("line", Some("fill".to_owned())));
    let moved = map.render();
    assert_eq!(0, parses_since(&map, from), "moving a layer laid its tiles out again");
    assert!(before != moved, "the moved layer draws where it did");

    let reordered = streets_map(&water_layers_in(["line", "fill"]));
    assert!(reordered.render() == moved, "the moved layer draws otherwise than a layer in its place");
}

/// A dashed line whose dash array steps at a zoom between two draws of the
/// same tile shows the dashes of the zoom it draws at, as a map that starts
/// at that zoom does.
#[test]
fn a_line_shows_the_dashes_of_its_zoom_on_a_kept_tile() {
    let style = r#"{
      "version": 8,
      "sources": {"line": {"type": "geojson", "data": {"type": "LineString", "coordinates": [[-40, 0], [40, 0]]}}},
      "layers": [
        {"id": "background", "type": "background", "paint": {"background-color": "white"}},
        {"id": "line", "type": "line", "source": "line", "paint": {
          "line-width": 8,
          "line-dasharray": ["step", ["zoom"], ["literal", [1, 1]], 2.5, ["literal", [4, 1]]]
        }}
      ]
    }"#;
    let render_at = |map: &StillMap, zoom: f64| {
        map.jump(CameraOptions { center: Some(ll(0.0, 0.0)), zoom: Some(zoom), ..Default::default() });
        map.render()
    };

    let zoomed = StillMap::still();
    zoomed.load_json(style);
    render_at(&zoomed, 2.2);
    let zoomed = render_at(&zoomed, 2.8);

    let fresh = StillMap::still();
    fresh.load_json(style);
    let fresh = render_at(&fresh, 2.8);

    assert!(zoomed.data == fresh.data, "the zoomed map draws other dashes than a map that starts at its zoom");
}
