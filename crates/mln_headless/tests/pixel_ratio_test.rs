// A map whose pixel ratio changes renders as a map made at the new ratio,
// and keeps its tiles.

mod support;

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;

use mln_map::MapMode;
use mln_storage::{Kind, Response};
use mln_util::ErrorKind;
use mln_util::image::PremultipliedImage;
use mln_util::size::Size;
use support::*;

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
    mln_image::encode_png(&PremultipliedImage::from_vec(Size::new(side, side), data))
}

fn sprite_json(ratio: u32) -> Vec<u8> {
    let side = 8 * ratio;
    format!(r#"{{"checks": {{"x": 0, "y": 0, "width": {side}, "height": {side}, "pixelRatio": {ratio}}}}}"#)
        .into_bytes()
}

fn response(data: Vec<u8>) -> Response {
    let mut response = Response::new();
    response.data = Some(Arc::new(data));
    response
}

/// What the map requested.
#[derive(Default)]
struct Requests {
    tiles: Cell<usize>,
    sprites: RefCell<Vec<String>>,
}

/// A still map at `pixel_ratio` with the style, answering from the fixtures.
fn map_test(pixel_ratio: f32) -> (MapTest, Rc<Requests>) {
    let test = MapTest::with_mode(pixel_ratio, MapMode::Static);
    let requests = Rc::new(Requests::default());
    let fs = &test.file_source;
    let r = requests.clone();
    fs.set_response(Kind::Tile, move |_| {
        r.tiles.set(r.tiles.get() + 1);
        Some(file_response("test/fixtures/map/offline/0-0-0.vector.pbf"))
    });
    fs.set_response(Kind::Glyphs, |_| Some(file_response("test/fixtures/map/offline/glyph.pbf")));
    let r = requests.clone();
    fs.set_response(Kind::SpriteJSON, move |resource| {
        r.sprites.borrow_mut().push(resource.url.clone());
        Some(response(sprite_json(if resource.url.contains("@2x") { 2 } else { 1 })))
    });
    let r = requests.clone();
    fs.set_response(Kind::SpriteImage, move |resource| {
        r.sprites.borrow_mut().push(resource.url.clone());
        Some(response(sprite_image(if resource.url.contains("@2x") { 2 } else { 1 })))
    });
    test.load_json(STYLE);
    (test, requests)
}

fn assert_same(actual: &PremultipliedImage, expected: &PremultipliedImage) {
    assert_eq!(actual.size, expected.size);
    let differing = actual.data.chunks(4).zip(expected.data.chunks(4)).filter(|(a, b)| a != b).count();
    assert_eq!(differing, 0, "{differing} pixels differ");
}

/// The map at `pixel_ratio` from the start.
fn render_made_at(pixel_ratio: f32) -> PremultipliedImage {
    let (test, _) = map_test(pixel_ratio);
    test.render().image
}

#[test]
fn a_map_whose_pixel_ratio_changes_renders_as_one_made_at_that_ratio() {
    // One map at a time: each has the thread's run loop.
    let made_at_1 = render_made_at(1.0);
    let made_at_2 = render_made_at(2.0);

    let (test, requests) = map_test(1.0);
    assert_same(&test.render().image, &made_at_1);
    assert_eq!(requests.tiles.get(), 1);
    assert_eq!(*requests.sprites.borrow(), ["http://sprites/sprite.json", "http://sprites/sprite.png"]);

    test.map().set_pixel_ratio(2.0).unwrap();
    test.frontend.clone().set_pixel_ratio(2.0);
    assert_same(&test.render().image, &made_at_2);
    // The map kept its tile, and loaded the sprite at the new resolution.
    assert_eq!(requests.tiles.get(), 1);
    assert_eq!(requests.sprites.borrow()[2..], ["http://sprites/sprite@2x.json", "http://sprites/sprite@2x.png"]);

    test.map().set_pixel_ratio(1.0).unwrap();
    test.frontend.clone().set_pixel_ratio(1.0);
    assert_same(&test.render().image, &made_at_1);
    assert_eq!(requests.tiles.get(), 1);
}

#[test]
fn a_ratio_that_picks_the_same_sprite_loads_nothing() {
    let (test, requests) = map_test(2.0);
    test.render();
    test.map().set_pixel_ratio(3.0).unwrap();
    test.frontend.clone().set_pixel_ratio(3.0);
    let image = test.render().image;
    assert_eq!(image.size, Size::new(768, 768));
    assert_eq!(requests.tiles.get(), 1);
    assert_eq!(requests.sprites.borrow().len(), 2);
    assert_eq!(test.map().get_map_options().pixel_ratio, 3.0);
}

#[test]
fn set_pixel_ratio_rejects_a_ratio_that_isnt_finite_and_positive() {
    let test = MapTest::new();
    for ratio in [0.0, -1.0, f32::NAN, f32::INFINITY] {
        let error = test.map().set_pixel_ratio(ratio).unwrap_err();
        assert_eq!(error.kind, ErrorKind::InvalidArgument);
    }
    assert_eq!(test.map().get_pixel_ratio(), 1.0);
}
