//! A map whose pixel ratio changes renders as a map made at the new ratio,
//! and keeps its tiles.

use std::sync::Arc;

use mln_engine::storage::{Kind, Response};
use mln_runtime::LogicalExtent;
use mln_style::bitmap::PremultipliedImage;
use mln_style::size::Size;

use crate::support::still::{Requests, StillMap, StillOptions, file_response, not_found, serve};
use crate::support::wait;

const STYLE: &str = r##"{
  "version": 8,
  "center": [0, 20],
  "zoom": 0.5,
  "sources": {
    "streets": { "type": "vector", "tiles": ["http://tiles/{z}-{x}-{y}.pbf"], "maxzoom": 0 }
  },
  "glyphs": "http://glyphs/{fontstack}/{range}.pbf",
  "sprite": "http://sprites/sprite",
  "layers": [
    { "id": "background", "type": "background", "paint": { "background-color": "white" } },
    { "id": "water", "type": "fill", "source": "streets", "source-layer": "water",
      "paint": { "fill-color": "#9cf" } },
    { "id": "admin", "type": "line", "source": "streets", "source-layer": "admin",
      "paint": { "line-color": "#333", "line-width": 1.5, "line-dasharray": [3, 2] } },
    { "id": "vertices", "type": "circle", "source": "streets", "source-layer": "admin",
      "paint": { "circle-radius": 1.5, "circle-color": "#c00" } },
    { "id": "icons", "type": "symbol", "source": "streets", "source-layer": "admin",
      "layout": { "icon-image": "checks", "icon-allow-overlap": true } },
    { "id": "labels", "type": "symbol", "source": "streets", "source-layer": "admin",
      "layout": {
        "symbol-placement": "line", "symbol-spacing": 120,
        "icon-image": "checks", "text-field": "Text", "text-font": ["Helvetica"],
        "text-size": 11, "icon-allow-overlap": true, "text-allow-overlap": true
      },
      "paint": { "text-color": "black", "text-halo-color": "white", "text-halo-width": 1 }
    }
  ]
}"##;

const SIZE: Size = Size::new(256, 256);

/// The sprite at `ratio`: an 8 logical pixel checkerboard of device pixels,
/// which shows whether the renderer samples it at the device's pixels.
fn sprite_image(ratio: u32) -> Vec<u8> {
    let side = 8 * ratio;
    let mut data = Vec::with_capacity((side * side * 4) as usize);
    for y in 0..side {
        for x in 0..side {
            let shade = if (x + y) % 2 == 0 { 0 } else { 255 };
            data.extend_from_slice(&[shade, shade / 2, 255 - shade, 255]);
        }
    }
    mln_engine::image::encode_png(&PremultipliedImage::from_vec(Size::new(side, side), data))
}

fn sprite_json(ratio: u32) -> Vec<u8> {
    let side = 8 * ratio;
    format!(r#"{{"checks": {{"x": 0, "y": 0, "width": {side}, "height": {side}, "pixelRatio": {ratio}}}}}"#)
        .into_bytes()
}

fn response(data: Vec<u8>) -> Response {
    Response { data: Some(Arc::new(data)), ..Response::default() }
}

/// A still map at `pixel_ratio` with the style, answering from the fixtures,
/// and the requests it made.
fn ratio_map(pixel_ratio: f32) -> (StillMap, Requests) {
    let map = StillMap::new(StillOptions { pixel_ratio, ..StillOptions::default() });
    let requests = serve(&map.runtime, |resource| {
        let ratio = if resource.url.contains("@2x") { 2 } else { 1 };
        match resource.kind {
            Kind::Tile => file_response("test/fixtures/map/offline/0-0-0.vector.pbf"),
            Kind::Glyphs => file_response("test/fixtures/map/offline/glyph.pbf"),
            Kind::SpriteJSON => response(sprite_json(ratio)),
            Kind::SpriteImage => response(sprite_image(ratio)),
            _ => not_found(),
        }
    });
    map.load_json(STYLE);
    (map, requests)
}

/// The URLs of the sprite's requests, in order.
fn sprites(requests: &Requests) -> Vec<String> {
    let all = requests.all();
    all.into_iter()
        .filter(|(kind, _)| matches!(kind, Kind::SpriteJSON | Kind::SpriteImage))
        .map(|(_, url)| url)
        .collect()
}

/// Sets the scale factor of `map`'s session to `ratio`, at the same size.
fn set_ratio(map: &StillMap, ratio: f64) {
    wait(map.session.resize(LogicalExtent { size: SIZE, scale_factor: ratio }));
}

fn assert_same(actual: &PremultipliedImage, expected: &PremultipliedImage) {
    assert_eq!(actual.size, expected.size);
    let differing = actual.data.chunks(4).zip(expected.data.chunks(4)).filter(|(a, b)| a != b).count();
    assert_eq!(differing, 0, "{differing} pixels differ");
}

/// The map at `pixel_ratio` from the start.
fn render_made_at(pixel_ratio: f32) -> PremultipliedImage {
    ratio_map(pixel_ratio).0.render()
}

/// A map whose session changes its scale factor renders as a map made at
/// that ratio, keeps its tile, and loads the sprite at the new resolution.
#[test]
fn a_map_whose_pixel_ratio_changes_renders_as_one_made_at_that_ratio() {
    let made_at_1 = render_made_at(1.0);
    let made_at_2 = render_made_at(2.0);

    let (map, requests) = ratio_map(1.0);
    assert_same(&map.render(), &made_at_1);
    assert_eq!(requests.count(Kind::Tile), 1);
    assert_eq!(sprites(&requests), ["http://sprites/sprite.json", "http://sprites/sprite.png"]);

    set_ratio(&map, 2.0);
    assert_same(&map.render(), &made_at_2);
    // The map kept its tile, and loaded the sprite at the new resolution.
    assert_eq!(requests.count(Kind::Tile), 1);
    assert_eq!(sprites(&requests)[2..], ["http://sprites/sprite@2x.json", "http://sprites/sprite@2x.png"]);

    set_ratio(&map, 1.0);
    assert_same(&map.render(), &made_at_1);
    assert_eq!(requests.count(Kind::Tile), 1);
}

/// Memory reduced between a ratio change and its frame keeps the shaders
/// that the frame before drew with: the old shaders go after the draws made
/// with them, which go with the next frame. A debug build's backend refuses
/// to destroy a shader that live bindings or geometry were made for.
#[test]
fn memory_reduced_between_a_ratio_change_and_its_frame_keeps_the_drawn_shaders() {
    let made_at_2 = render_made_at(2.0);
    let (map, _) = ratio_map(1.0);
    map.render();
    set_ratio(&map, 2.0);
    wait(map.session.reduce_memory_use());
    assert_same(&map.render(), &made_at_2);
    wait(map.session.reduce_memory_use());
}

/// A ratio that picks the same sprite as the ratio before renders at the new
/// ratio and loads nothing.
#[test]
fn a_ratio_that_picks_the_same_sprite_loads_nothing() {
    let (map, requests) = ratio_map(2.0);
    map.render();
    set_ratio(&map, 3.0);
    let image = map.render();
    assert_eq!(image.size, Size::new(768, 768));
    assert_eq!(requests.count(Kind::Tile), 1);
    assert_eq!(sprites(&requests).len(), 2);
    assert_eq!(map.map.snapshot().unwrap().view.extent.scale_factor, 3.0);
}
