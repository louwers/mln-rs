// The tile debug overlays of `Map::set_debug` (tile borders, parse status,
// timestamps) and the rendering stats view, on the headless frontend.

mod support;

use mln_map::{CameraOptions, MapDebugOptions};
use mln_storage::{Kind, Resource};
use mln_util::chrono::Timestamp;
use mln_util::geo::LatLng;
use mln_util::image::PremultipliedImage;
use support::{MapTest, file_response};

/// A raster source of one solid tile, whose responses carry `modified` and
/// `expires` times.
const RASTER_STYLE: &str = r#"{
  "version": 8,
  "sources": {
    "raster": { "type": "raster", "tiles": [ "asset://tile.png" ], "tileSize": 256 }
  },
  "layers": [{ "id": "raster", "type": "raster", "source": "raster" }]
}"#;

/// Tile 1/1/1 filling the frontend's 256 px: 32 tile units to a pixel.
fn tile_camera() -> CameraOptions {
    CameraOptions::default().with_center(LatLng::new(-66.51326044311186, 90.0).ok()).with_zoom(Some(0.0))
}

fn raster_map() -> MapTest {
    let test = MapTest::new();
    test.file_source.set_response(Kind::Tile, |_: &Resource| {
        let mut response = file_response("test/fixtures/map/disabled_layers/tile.png");
        response.modified = Some(Timestamp::from_secs(784111777));
        response.expires = Some(Timestamp::from_secs(784115377));
        Some(response)
    });
    test.load_json(RASTER_STYLE);
    test.map().jump_to(&tile_camera());
    test
}

/// Renders a still image with `debug`, and writes it to
/// target/tmp/debug_options/<name>.png.
fn render(test: &MapTest, debug: MapDebugOptions, name: &str) -> PremultipliedImage {
    test.map().set_debug(debug);
    let image = test.render().image;
    let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("debug_options");
    if std::fs::create_dir_all(&dir).is_ok() {
        let _ = std::fs::write(dir.join(format!("{name}.png")), mln_image::encode_png(&image));
    }
    image
}

fn pixel(image: &PremultipliedImage, x: u32, y: u32) -> [u8; 4] {
    let offset = y as usize * image.stride() + x as usize * 4;
    image.data[offset..offset + 4].try_into().unwrap()
}

/// The pixels that differ between two images of the same size.
fn changed_pixels(a: &PremultipliedImage, b: &PremultipliedImage) -> Vec<(u32, u32)> {
    assert_eq!(a.size, b.size);
    let mut changed = Vec::new();
    for y in 0..a.size.height {
        for x in 0..a.size.width {
            if pixel(a, x, y) != pixel(b, x, y) {
                changed.push((x, y));
            }
        }
    }
    changed
}

/// The last row with a changed pixel.
fn bottom(changed: &[(u32, u32)]) -> u32 {
    changed.iter().map(|&(_, y)| y).max().expect("a changed pixel")
}

fn is_gray([r, g, b, _]: [u8; 4]) -> bool {
    r.abs_diff(g) <= 1 && g.abs_diff(b) <= 1
}

#[test]
fn parse_status_draws_black_text_on_white() {
    let test = raster_map();
    let plain = render(&test, MapDebugOptions::NO_DEBUG, "parse_status_plain");
    let status = render(&test, MapDebugOptions::PARSE_STATUS, "parse_status");

    // "1/1/1=>1 - complete raster", 50 units from the tile's left, on a
    // baseline 200 units from its top.
    let changed = changed_pixels(&plain, &status);
    assert!(changed.len() > 200, "{} pixels changed", changed.len());
    assert!(bottom(&changed) < 12, "text below the first line");
    assert!(changed.iter().any(|&(x, _)| x > 64), "the text is shorter than expected");
    // The text is black over a white outline.
    assert!(changed.iter().any(|&(x, y)| pixel(&status, x, y) == [0, 0, 0, 255]));
    assert!(changed.iter().any(|&(x, y)| pixel(&status, x, y) == [255, 255, 255, 255]));
    let gray = changed.iter().filter(|&&(x, y)| is_gray(pixel(&status, x, y))).count();
    assert!(gray * 2 > changed.len(), "{gray} of {} changed pixels are gray", changed.len());
}

#[test]
fn timestamps_follow_the_parse_status() {
    let test = raster_map();
    let plain = render(&test, MapDebugOptions::NO_DEBUG, "timestamps_plain");
    let status = render(&test, MapDebugOptions::PARSE_STATUS, "timestamps_status");
    let timestamps = render(&test, MapDebugOptions::TIMESTAMPS, "timestamps_alone");
    let both = render(&test, MapDebugOptions::PARSE_STATUS | MapDebugOptions::TIMESTAMPS, "timestamps_both");

    // One line, two lines, three lines.
    let status_bottom = bottom(&changed_pixels(&plain, &status));
    let timestamps_bottom = bottom(&changed_pixels(&plain, &timestamps));
    let both_bottom = bottom(&changed_pixels(&plain, &both));
    assert!(status_bottom < timestamps_bottom, "{status_bottom} >= {timestamps_bottom}");
    assert!(timestamps_bottom < both_bottom, "{timestamps_bottom} >= {both_bottom}");
    assert!(both_bottom < 26, "{both_bottom}");
}

#[test]
fn text_follows_a_change_of_options() {
    // The renderer keeps its debug drawables from frame to frame: they must
    // still change when the tile's text does.
    let test = raster_map();
    let status = render(&test, MapDebugOptions::PARSE_STATUS, "change_status");
    let both = render(&test, MapDebugOptions::PARSE_STATUS | MapDebugOptions::TIMESTAMPS, "change_both");
    let status_again = render(&test, MapDebugOptions::PARSE_STATUS, "change_status_again");

    assert!(bottom(&changed_pixels(&status, &both)) > 12, "no timestamps");
    assert!(changed_pixels(&status, &status_again).is_empty(), "the timestamps stayed");
}

#[test]
fn tile_borders_follow_the_camera() {
    // The same four tiles before and after the move.
    let camera = |lng: f64| CameraOptions::default().with_center(LatLng::new(0.0, lng).ok()).with_zoom(Some(2.0));

    let moved = raster_map();
    moved.map().jump_to(&camera(0.0));
    render(&moved, MapDebugOptions::TILE_BORDERS, "borders_before_move");
    moved.map().jump_to(&camera(10.0));
    let after_move = render(&moved, MapDebugOptions::TILE_BORDERS, "borders_after_move");

    let fresh = raster_map();
    fresh.map().jump_to(&camera(10.0));
    let first_frame = render(&fresh, MapDebugOptions::TILE_BORDERS, "borders_first_frame");

    let differences = changed_pixels(&after_move, &first_frame);
    assert!(differences.is_empty(), "{} pixels differ", differences.len());
}

const GLYPHS_STYLE: &str = r#"{
  "version": 8,
  "glyphs": "asset://{fontstack}/{range}.pbf",
  "sources": {},
  "layers": [{ "id": "background", "type": "background", "paint": { "background-color": "white" } }]
}"#;

fn has_stats_view(test: &MapTest) -> (bool, bool) {
    let map = test.map();
    let style = map.get_style();
    (style.get_layer("rendering-stats").is_some(), style.get_source("rendering-stats-source").is_some())
}

#[test]
fn rendering_stats_view_shows_the_stats_in_the_top_right() {
    let test = MapTest::new();
    test.file_source
        .set_response(Kind::Glyphs, |_: &Resource| Some(file_response("test/fixtures/resources/glyphs.pbf")));
    test.load_json(GLYPHS_STYLE);
    let plain = render(&test, MapDebugOptions::NO_DEBUG, "stats_plain");

    assert!(!test.map().is_rendering_stats_view_enabled());
    test.map().enable_rendering_stats_view(true);
    assert!(test.map().is_rendering_stats_view_enabled());
    assert_eq!(has_stats_view(&test), (true, true));

    // The first frame sets the text, the second shows it.
    render(&test, MapDebugOptions::NO_DEBUG, "stats_first");
    let stats = render(&test, MapDebugOptions::NO_DEBUG, "stats");
    let changed = changed_pixels(&plain, &stats);
    // Red text, right-aligned to the top right corner.
    assert!(changed.len() > 1000, "{} pixels changed", changed.len());
    assert!(changed.iter().any(|&(x, y)| x >= 240 && y < 16), "no text in the top right corner");
    assert!(changed.iter().all(|&(x, _)| x >= 16), "text at the left edge");
    assert!(changed.iter().all(|&(x, y)| {
        let [r, g, b, _] = pixel(&stats, x, y);
        r == 255 && g == b
    }));

    // A new style gets the view again.
    test.load_json(GLYPHS_STYLE);
    assert_eq!(has_stats_view(&test), (false, false));
    render(&test, MapDebugOptions::NO_DEBUG, "stats_reloaded");
    assert_eq!(has_stats_view(&test), (true, true));

    test.map().enable_rendering_stats_view(false);
    assert!(!test.map().is_rendering_stats_view_enabled());
    assert_eq!(has_stats_view(&test), (false, false));
    assert!(changed_pixels(&plain, &render(&test, MapDebugOptions::NO_DEBUG, "stats_disabled")).is_empty());
}
