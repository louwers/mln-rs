//! The style's global state, as the layers that read it draw it.

use std::sync::Arc;

use mln_engine::camera::CameraOptions;
use mln_engine::map::MapMode;
use mln_engine::storage::Response;
use mln_style::bitmap::PremultipliedImage;
use mln_style::geo::LatLng;
use mln_style::size::Size;

use crate::support::still::{StillMap, StillOptions, assert_near, center_pixel, serve};
use crate::support::wait;

fn continuous_map() -> StillMap {
    StillMap::new(StillOptions { mode: MapMode::Continuous, ..StillOptions::default() })
}

/// Draws a frame, and returns the RGB of the pixel at its center.
fn frame_center(map: &StillMap) -> [u8; 3] {
    map.frame();
    let [r, g, b, _] = center_pixel(&map.image());
    [r, g, b]
}

/// A change to the global state transitions the paint properties that read
/// it: the frame that takes the change in starts the transition at its own
/// time, so it still shows the old state, and the new one shows once the
/// transition ends.
#[test]
fn a_global_state_change_transitions_a_paint_property() {
    let map = continuous_map();
    map.load_json(
        r##"{
      "version": 8,
      "transition": {"duration": 100, "delay": 0},
      "state": {"color": {"default": "#ff0000"}},
      "sources": {},
      "layers": [{"id": "background", "type": "background", "paint": {"background-color": ["global-state", "color"]}}]
    }"##,
    );

    let initial = frame_center(&map);
    assert!(initial[0] > 240, "{initial:?}");
    assert!(initial[2] < 10, "{initial:?}");

    wait(map.map.set_global_state_property("color", r##""#0000ff""##));
    let changed = frame_center(&map);
    assert!(changed[0] > 200, "{changed:?}");
    assert!(changed[2] < 55, "{changed:?}");

    map.settle();
    let finished = frame_center(&map);
    assert!(finished[0] < 10, "{finished:?}");
    assert!(finished[2] > 240, "{finished:?}");
}

/// A layer added with a change to the global state that it reads never
/// showed the old state, so it shows the new one at once.
#[test]
fn a_layer_added_with_a_global_state_change_shows_the_new_state() {
    let map = continuous_map();
    map.load_json(
        r##"{
      "version": 8,
      "transition": {"duration": 100, "delay": 0},
      "state": {"color": {"default": "#0000ff"}},
      "sources": {},
      "layers": [{"id": "base", "type": "background", "paint": {"background-color": "#ff0000"}}]
    }"##,
    );

    let initial = frame_center(&map);
    assert!(initial[0] > 240, "{initial:?}");
    assert!(initial[1] < 10, "{initial:?}");

    wait(map.map.set_global_state_property("color", r##""#00ff00""##));
    let layer = r#"{"id": "added", "type": "background", "paint": {"background-color": ["global-state", "color"]}}"#;
    wait(map.map.add_style_layer_json(layer, None));

    let added = frame_center(&map);
    assert!(added[0] < 10, "{added:?}");
    assert!(added[1] > 240, "{added:?}");
    assert!(added[2] < 10, "{added:?}");
}

/// A color ramp shows the global state as it is after changes to state that
/// it doesn't read, for each layer type with a ramp: as the layer joins in
/// the same update as such a change, while the ramp reads no state, and as
/// the ramp reads the state again in the same update as another.
#[test]
fn a_color_ramp_shows_the_current_state_after_an_unrelated_change() {
    for layer_type in ["line", "heatmap", "color-relief"] {
        let ramp_property =
            if layer_type == "line" { "line-gradient".to_owned() } else { format!("{layer_type}-color") };
        let ramp = |color: &str| {
            let input = match layer_type {
                "line" => r#"["line-progress"], 0, "#,
                "heatmap" => r#"["heatmap-density"], 0, "#,
                _ => r#"["elevation"], -20000, "#,
            };
            let end = if layer_type == "color-relief" { ", 20000, " } else { ", 1, " };
            format!(r#"["interpolate", ["linear"], {input}{color}{end}{color}]"#)
        };
        let state_color = r#"["global-state", "color"]"#;

        let map = StillMap::still();
        // Opaque black decodes to an elevation of -10000 m.
        let mut dem = PremultipliedImage::new(Size::new(256, 256));
        for alpha in dem.data.iter_mut().skip(3).step_by(4) {
            *alpha = 255;
        }
        let dem = Arc::new(mln_engine::image::encode_png(&dem));
        serve(&map.runtime, move |_| Response { data: Some(Arc::clone(&dem)), ..Response::default() });
        map.load_json(
            r#"{
            "version": 8,
            "state": {"color": {"default": "red"}, "unrelated": {"default": 0}},
            "sources": {
                "line": {
                    "type": "geojson",
                    "lineMetrics": true,
                    "data": {"type": "LineString", "coordinates": [[-20, 0], [20, 0]]}
                },
                "heatmap": {"type": "geojson", "data": {"type": "Point", "coordinates": [0, 0]}},
                "color-relief": {"type": "raster-dem", "tiles": ["https://example.com/{z}/{x}/{y}.png"]}
            },
            "layers": [{"id": "background", "type": "background", "paint": {"background-color": "white"}}]
        }"#,
        );
        let center = LatLng::new(0.0, 0.0).expect("LatLng");
        map.jump(CameraOptions { center: Some(center), zoom: Some(2.0), ..Default::default() });
        map.render();

        let expect_center = |expected: [u8; 3]| {
            let pixel = center_pixel(&map.render());
            for channel in 0..3 {
                assert_near(f64::from(pixel[channel]), f64::from(expected[channel]), 2.0);
            }
        };
        let set_ramp = |color: &str| wait(map.map.set_style_layer_property("ramp", &ramp_property, ramp(color)));
        let set_state = |name: &str, json: &str| wait(map.map.set_global_state_property(name, json));

        let extra_paint = match layer_type {
            "line" => r#", "line-width": 16"#,
            "heatmap" => r#", "heatmap-radius": 64"#,
            _ => "",
        };
        let layer = format!(
            r#"{{"id": "ramp", "type": "{layer_type}", "source": "{layer_type}", "paint": {{"{ramp_property}": {}{extra_paint}}}}}"#,
            ramp(state_color)
        );

        set_state("unrelated", "1");
        wait(map.map.add_style_layer_json(layer, None));
        expect_center([255, 0, 0]);

        set_ramp(r#""lime""#);
        expect_center([0, 255, 0]);
        set_state("color", r#""blue""#);
        expect_center([0, 255, 0]);

        set_state("unrelated", "2");
        set_ramp(state_color);
        expect_center([0, 0, 255]);
    }
}

/// Clearing a session's data drops the renderer's layers, sources and
/// images, and keeps the light, sky and global state it drew with, so the
/// next still image is the same.
#[test]
fn clearing_the_data_keeps_the_light_sky_and_global_state() {
    let map = StillMap::still();
    map.load_json(
        r#"{"version": 8, "sources": {},
        "state": {"color": {"default": "rgba(0,0,255,1)"}},
        "light": {"intensity": 0.2},
        "sky": {"sky-color": "rgba(0,255,0,1)"},
        "layers": [{"id": "background", "type": "background",
            "paint": {"background-color": ["to-color", ["global-state", "color"]]}}]}"#,
    );
    wait(map.map.set_global_state_property("color", r#""rgba(255,0,0,1)""#));
    let before = map.render();
    assert_eq!(center_pixel(&before), [255, 0, 0, 255]);

    wait(map.session.clear_data());
    let after = map.render();
    assert!(before == after, "the image changed once the data was cleared");
}
