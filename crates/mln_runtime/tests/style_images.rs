//! The style's images, as icons and patterns: added, replaced and removed
//! before and after the tiles that use them laid out.

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use mln_engine::TileOperation;
use mln_engine::camera::CameraOptions;
use mln_engine::map::{MapEvent, MapMode};
use mln_engine::storage::Kind;
use mln_runtime::{RuntimeEventKind, TileOptions};
use mln_style::bitmap::PremultipliedImage;
use mln_style::image::Impl as StyleImage;
use mln_style::size::Size;

use crate::support::still::{StillMap, StillOptions, center_pixel, file_response, ll, not_found, pixel, serve};
use crate::support::wait;

const RED: [u8; 4] = [255, 0, 0, 255];
const GREEN: [u8; 4] = [0, 255, 0, 255];
const BLUE: [u8; 4] = [0, 0, 255, 255];

/// An image `id` of `size`, every pixel `color`, at pixel ratio 1.
fn solid_image(id: &str, size: Size, color: [u8; 4]) -> StyleImage {
    let pixels = PremultipliedImage::from_vec(size, color.repeat((size.width * size.height) as usize));
    StyleImage::new(id.to_owned(), pixels, 1.0, false, Vec::new(), Vec::new(), None, None, None).expect("a style image")
}

fn add_image(map: &StillMap, image: StyleImage) {
    wait(map.map.set_style_image(image));
}

fn same_color(actual: [u8; 4], expected: [u8; 4]) -> bool {
    actual.iter().zip(expected).all(|(a, b)| a.abs_diff(b) <= 2)
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

/// A background switches between its color and a pattern, and back.
#[test]
fn a_background_switches_between_solid_and_patterned() {
    let map = StillMap::still();
    // The underlay keeps the tested background out of the clear color.
    map.load_json(
        r#"{
        "version": 8,
        "sources": {},
        "layers": [
            {"id": "underlay", "type": "background", "paint": {"background-color": "white"}},
            {"id": "background", "type": "background", "paint": {"background-color": "red"}}
        ]
    }"#,
    );
    add_image(&map, solid_image("blue", Size::new(1, 1), BLUE));
    let set_pattern = |json: &str| wait(map.map.set_style_layer_property("background", "background-pattern", json));

    assert_eq!(center_pixel(&map.render()), RED);
    set_pattern(r#""blue""#);
    assert_eq!(center_pixel(&map.render()), BLUE);
    set_pattern("null");
    assert_eq!(center_pixel(&map.render()), RED);
}

/// A 512 by 512 still map with a white background, whose glyphs come from
/// test/fixtures/resources/glyphs.pbf, and the count of its glyph requests.
fn geojson_symbol_map() -> (StillMap, Arc<AtomicU32>) {
    let map = StillMap::new(StillOptions { size: Size::new(512, 512), ..StillOptions::default() });
    let glyph_requests = Arc::new(AtomicU32::new(0));
    {
        let glyph_requests = Arc::clone(&glyph_requests);
        serve(&map.runtime, move |resource| {
            if resource.kind == Kind::Glyphs {
                glyph_requests.fetch_add(1, Ordering::SeqCst);
                file_response("test/fixtures/resources/glyphs.pbf")
            } else {
                not_found()
            }
        });
    }
    map.load_json(
        r#"{
        "version": 8,
        "glyphs": "https://example.com/{fontstack}/{range}.pbf",
        "sources": {},
        "layers": [{"id": "background", "type": "background", "paint": {"background-color": "white"}}]
    }"#,
    );
    (map, glyph_requests)
}

/// How a GeoJSON source of a symbol builds its tiles, and whether the
/// symbol has a label.
#[derive(Clone, Copy, Debug)]
struct SymbolCase {
    synchronous: bool,
    with_text: bool,
}

const SYMBOL_CASES: [SymbolCase; 4] = [
    SymbolCase { synchronous: false, with_text: false },
    SymbolCase { synchronous: false, with_text: true },
    SymbolCase { synchronous: true, with_text: false },
    SymbolCase { synchronous: true, with_text: true },
];

/// Adds a GeoJSON source with a point at `longitude` on the equator, and a
/// symbol layer that draws the "marker" icon above it and, with text, a
/// label below it.
fn add_geojson_symbol(map: &StillMap, index: usize, longitude: f64, case: SymbolCase) {
    let source_id = format!("point-{index}");
    let source = format!(r#"{{"type": "geojson", "data": {{"type": "Point", "coordinates": [{longitude}, 0]}}}}"#);
    wait(map.map.add_style_source_json(&source_id, source));
    wait(map.map.set_geojson_source_synchronous_tiling(&source_id, case.synchronous));
    let text = if case.with_text { "New place" } else { "" };
    let layer = format!(
        r#"{{
            "id": "symbol-{index}", "type": "symbol", "source": "{source_id}",
            "layout": {{
                "icon-image": "marker", "icon-anchor": "bottom",
                "icon-allow-overlap": true, "icon-ignore-placement": true,
                "text-field": "{text}", "text-font": ["Open Sans Regular"], "text-anchor": "top",
                "text-allow-overlap": true, "text-ignore-placement": true
            }}
        }}"#
    );
    wait(map.map.add_style_layer_json(layer, None));
}

/// GeoJSON symbols keep their icons, and their labels, across zooms that
/// lay their tiles out again, which load their glyphs once.
#[test]
fn geojson_symbols_keep_their_icons_and_text_across_zooms() {
    for case in SYMBOL_CASES {
        let (map, glyph_requests) = geojson_symbol_map();
        add_image(&map, solid_image("marker", Size::new(40, 48), RED));
        for i in 0..3 {
            add_geojson_symbol(&map, i, (i as f64 - 1.0) * 0.01, case);
        }

        for zoom in [14.0, 13.5, 13.0, 12.5, 14.0] {
            map.jump(CameraOptions { center: Some(ll(0.0, 0.0)), zoom: Some(zoom), ..Default::default() });
            let image = map.render();
            let mut red_pixels = 0;
            let mut black_pixels = 0;
            for pixel in image.data.as_chunks::<4>().0 {
                red_pixels += usize::from(pixel[0] > 200 && pixel[1] < 50 && pixel[2] < 50);
                black_pixels += usize::from(pixel[0] < 100 && pixel[1] < 100 && pixel[2] < 100);
            }
            assert!(red_pixels >= 5000, "{case:?} at zoom {zoom}: {red_pixels} red pixels");
            if case.with_text {
                assert!(black_pixels > 100, "{case:?} at zoom {zoom}: {black_pixels} black pixels");
                assert_eq!(glyph_requests.load(Ordering::SeqCst), 1, "{case:?} at zoom {zoom}");
            }
        }
    }
}

/// An image that a symbol's tile laid out without shows once the style has
/// it, though neither the camera nor the layers change.
#[test]
fn a_style_image_added_after_layout_shows() {
    for case in SYMBOL_CASES {
        let (map, _) = geojson_symbol_map();
        add_geojson_symbol(&map, 0, 0.0, case);

        let icon_size = Size::new(32, 32);
        let before = map.render();
        let x = before.size.width / 2;
        let y = before.size.height / 2 - icon_size.height / 2;
        assert_eq!(pixel(&before, x, y)[..3], [255, 255, 255], "{case:?}");

        add_image(&map, solid_image("marker", icon_size, RED));
        assert_eq!(pixel(&map.render(), x, y), RED, "{case:?}");
    }
}

/// An image that no layer uses lays out no tile again, neither of those in
/// view nor of those in the cache.
#[test]
fn an_unrelated_image_keeps_the_tiles_layout_and_cache() {
    let map = StillMap::new(StillOptions { size: Size::new(128, 128), ..StillOptions::default() });
    wait(map.map.set_tile_options(TileOptions { prefetch_zoom_delta: Some(0), ..TileOptions::default() }));
    map.load_json(
        r#"{
        "version": 8,
        "sources": {"point": {"type": "geojson", "data": {"type": "Point", "coordinates": [-90, 40]}}},
        "layers": [{"id": "symbol", "type": "symbol", "source": "point", "layout": {"icon-image": "marker"}}]
    }"#,
    );
    map.jump(CameraOptions { center: Some(ll(40.0, -90.0)), zoom: Some(3.0), ..Default::default() });
    let from = map.events.events().len();
    map.render();
    assert!(parses_since(&map, from) > 0);

    // Move the first tiles into the cache, and finish laying out others.
    map.jump(CameraOptions { center: Some(ll(40.0, 0.0)), zoom: Some(3.0), ..Default::default() });
    map.render();
    let from = map.events.events().len();
    add_image(&map, solid_image("unrelated", Size::new(16, 16), RED));
    map.render();
    assert_eq!(parses_since(&map, from), 0);

    map.jump(CameraOptions { center: Some(ll(40.0, -90.0)), zoom: Some(3.0), ..Default::default() });
    map.render();
    assert_eq!(parses_since(&map, from), 0);
}

/// An image added after layout, which a `coalesce` skipped as missing, lays
/// its tile out again, which then draws it in place of the fallback.
#[test]
fn a_coalesced_image_added_after_layout_replaces_the_fallback() {
    let map = StillMap::new(StillOptions { size: Size::new(128, 128), ..StillOptions::default() });
    map.load_json(
        r#"{
        "version": 8,
        "sources": {"point": {"type": "geojson", "data": {"type": "Point", "coordinates": [0, 0]}}},
        "layers": [
            {"id": "background", "type": "background", "paint": {"background-color": "white"}},
            {"id": "symbol", "type": "symbol", "source": "point", "layout": {
                "icon-image": ["coalesce", ["image", "late"], ["image", "fallback"]],
                "icon-allow-overlap": true, "icon-ignore-placement": true
            }}
        ]
    }"#,
    );
    add_image(&map, solid_image("fallback", Size::new(32, 32), BLUE));
    assert_eq!(pixel(&map.render(), 64, 64), BLUE);

    let from = map.events.events().len();
    add_image(&map, solid_image("late", Size::new(32, 32), RED));
    let after = map.render();
    assert!(parses_since(&map, from) > 0);
    assert_eq!(pixel(&after, 64, 64), RED);
}

const ICON_STYLE: &str = r#"{
  "version": 8,
  "sources": {"point": {"type": "geojson", "data": {"type": "Point", "coordinates": [0, 0]}}},
  "layers": [
    {"id": "background", "type": "background", "paint": {"background-color": "white"}},
    {"id": "icon", "type": "symbol", "source": "point",
     "layout": {"icon-image": "icon", "icon-size": 4, "icon-allow-overlap": true, "icon-ignore-placement": true}}
  ]
}"#;

fn continuous_map() -> StillMap {
    StillMap::new(StillOptions { mode: MapMode::Continuous, ..StillOptions::default() })
}

fn settled_center(map: &StillMap) -> [u8; 4] {
    map.settle();
    center_pixel(&map.image())
}

/// An image replaced at a new size, and replaced again before its tile is
/// laid out again, shows its last pixels: the old layout gets no pixels of
/// a size it has no room for.
#[test]
fn an_image_updated_again_before_its_resize_relayout_shows_its_last_pixels() {
    let map = continuous_map();
    map.load_json(ICON_STYLE);
    add_image(&map, solid_image("icon", Size::new(10, 10), RED));
    map.settle();

    add_image(&map, solid_image("icon", Size::new(20, 20), BLUE));
    map.frame();
    add_image(&map, solid_image("icon", Size::new(20, 20), GREEN));
    map.frame();

    let pixel = settled_center(&map);
    assert!(same_color(pixel, GREEN), "{pixel:?}");
}

/// An image removed and added again before its tile is laid out again shows
/// its new pixels, though the old layout still holds the texture of the
/// removed image when the new layout reserves its own.
#[test]
fn an_image_added_again_before_the_relayout_shows_its_new_pixels() {
    let map = continuous_map();
    map.load_json(ICON_STYLE);
    add_image(&map, solid_image("icon", Size::new(10, 10), RED));
    map.settle();

    wait(map.map.remove_style_image("icon"));
    map.frame();
    add_image(&map, solid_image("icon", Size::new(10, 10), BLUE));
    map.frame();

    let pixel = settled_center(&map);
    assert!(same_color(pixel, BLUE), "{pixel:?}");
}

/// A background pattern shows its image's new pixels once the style
/// replaces the image, at the same size too, and nothing once it removes it.
#[test]
fn a_background_pattern_follows_its_image() {
    let map = continuous_map();
    map.load_json(
        r#"{
      "version": 8,
      "sources": {},
      "layers": [{"id": "background", "type": "background", "paint": {"background-pattern": "icon"}}]
    }"#,
    );
    add_image(&map, solid_image("icon", Size::new(8, 8), RED));
    let pixel = settled_center(&map);
    assert!(same_color(pixel, RED), "{pixel:?}");

    add_image(&map, solid_image("icon", Size::new(8, 8), BLUE));
    let pixel = settled_center(&map);
    assert!(same_color(pixel, BLUE), "{pixel:?}");

    wait(map.map.remove_style_image("icon"));
    let pixel = settled_center(&map);
    assert!(same_color(pixel, [0, 0, 0, 0]), "{pixel:?}");
}
