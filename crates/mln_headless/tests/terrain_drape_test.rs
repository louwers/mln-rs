// The layers that drape over terrain draw into the terrain tiles' drapes,
// which the surface shows: over flat ground, seen from above, the map looks
// as it does without terrain.

use std::sync::Arc;

use mln_headless::HeadlessFrontend;
use mln_map::{Map, MapMode, MapOptions, ViewportMode};
use mln_storage::stub_file_source::{ResponseType, StubFileSource};
use mln_storage::{Kind, Resource, Response};
use mln_util::error::ErrorKind;
use mln_util::image::PremultipliedImage;
use mln_util::pool::Pool;
use mln_util::queue::Queue;
use mln_util::size::Size;

const SIDE: u32 = 256;

/// How many pixels a draped map may differ by from the same map drawn flat:
/// the surface filters its drapes, so pixels on the images' edges differ, and
/// how many depends on the GPU.
const EDGE_PIXELS: u64 = 64;

/// A DEM tile of sea level everywhere, in the Mapbox encoding: (1 · 65536 +
/// 134 · 256 + 160) · 0.1 − 10000 = 0 m.
fn sea_level_png() -> Vec<u8> {
    solid_png([1, 134, 160, 255])
}

/// A PNG of 8 pixels a side, every pixel `pixel`.
fn solid_png(pixel: [u8; 4]) -> Vec<u8> {
    let mut image = PremultipliedImage::new(Size::new(8, 8));
    for texel in image.data.as_chunks_mut::<4>().0 {
        *texel = pixel;
    }
    mln_image::encode_png(&image)
}

/// The members of a style at zoom 2 over the center, seen from above, with a
/// terrain over flat ground if `terrain`, whose sources are `sources` and
/// the DEM's, and whose layers are `layers`.
fn style_of(terrain: bool, sources: &str, layers: &str) -> String {
    let terrain = if terrain { r#""terrain": {"source": "dem"},"# } else { "" };
    format!(
        r#"{{
            "version": 8,
            "center": [0, 0],
            "zoom": 2,
            {terrain}
            "sources": {{
                "dem": {{"type": "raster-dem", "tiles": ["dem/{{z}}/{{x}}/{{y}}.png"], "tileSize": 256, "maxzoom": 10}},
                {sources}
            }},
            "layers": [{layers}]
        }}"#
    )
}

/// An image source `id` of the image `id.png` from `west` to `east` and from
/// `south` to `north`, in degrees.
fn image_source(id: &str, [west, south, east, north]: [f64; 4]) -> String {
    format!(
        r#""{id}": {{
            "type": "image",
            "url": "{id}.png",
            "coordinates": [[{west}, {north}], [{east}, {north}], [{east}, {south}], [{west}, {south}]]
        }}"#
    )
}

/// A raster layer of the source `id`.
fn raster_layer(id: &str) -> String {
    format!(r#"{{"id": "{id}", "type": "raster", "source": "{id}", "paint": {{"raster-fade-duration": 0}}}}"#)
}

const BACKGROUND: &str = r#"{"id": "background", "type": "background", "paint": {"background-color": "white"}}"#;

/// A white map with a red image around the center and a blue image band
/// across it north of the center; with a terrain over flat ground if
/// `terrain`.
fn style(terrain: bool) -> String {
    let sources = [image_source("red", [-5.0, -5.0, 5.0, 5.0]), image_source("blue", [-60.0, 9.0, 60.0, 11.0])];
    let layers = [BACKGROUND.to_owned(), raster_layer("red"), raster_layer("blue")];
    style_of(terrain, &sources.join(","), &layers.join(","))
}

/// A white map with a red image around the center, a green circle at the
/// center, and a blue image over the circle's east half; with a terrain over
/// flat ground if `terrain`. The circle splits the draped layers into two
/// stacks.
fn stacked_style(terrain: bool) -> String {
    let circle = r#"{
        "id": "circle",
        "type": "circle",
        "source": "point",
        "paint": {"circle-color": "lime", "circle-radius": 15, "circle-blur": 0}
    }"#;
    let sources = [
        image_source("red", [-8.0, -8.0, 8.0, 8.0]),
        r#""point": {"type": "geojson", "data": {"type": "Point", "coordinates": [0, 0]}}"#.to_owned(),
        image_source("blue", [-1.0, -5.0, 5.0, 5.0]),
    ];
    let layers = [BACKGROUND.to_owned(), raster_layer("red"), circle.to_owned(), raster_layer("blue")];
    style_of(terrain, &sources.join(","), &layers.join(","))
}

/// The still image of `style` on the harness's device
/// ([`mln_headless::harness_device`]), over DEM tiles at sea level, with the
/// image `red.png` red and the image `blue.png` blue, and the bytes of the
/// textures that drew it; `None` where the machine has no such device.
fn render(style: &str, viewport_mode: ViewportMode) -> Option<(PremultipliedImage, i32)> {
    let size = Size::new(SIDE, SIDE);
    let run_loop = Queue::new();
    let pool = Pool::new(4);
    let device = match mln_headless::harness_device() {
        Ok(device) => device,
        Err(error) if error.kind == ErrorKind::Unsupported => {
            eprintln!("skipped: no device: {error}");
            return None;
        }
        Err(error) => panic!("a device: {error}"),
    };
    let mut frontend =
        HeadlessFrontend::with_device(device, size, 1.0, Arc::default(), &run_loop, &pool).expect("a frontend");
    let file_source = Arc::new(StubFileSource::new(ResponseType::Synchronous));
    let dem = Arc::new(sea_level_png());
    file_source.set_response(Kind::Tile, move |_: &Resource| {
        let mut response = Response::new();
        response.data = Some(Arc::clone(&dem));
        Some(response)
    });
    let (red, blue) = (Arc::new(solid_png([255, 0, 0, 255])), Arc::new(solid_png([0, 0, 255, 255])));
    file_source.set_response(Kind::Image, move |resource: &Resource| {
        let mut response = Response::new();
        let image = if resource.url.starts_with("red") { &red } else { &blue };
        response.data = Some(Arc::clone(image));
        Some(response)
    });
    let options = MapOptions { map_mode: MapMode::Static, size, pixel_ratio: 1.0, viewport_mode, ..Default::default() };
    let mut map = Map::with_file_source(file_source, options, &pool).unwrap();
    frontend.attach(&mut map);
    map.load_style_json(style);
    let camera = map.style().get_default_camera();
    map.jump_to(&camera);
    let result = frontend.render(&mut map).expect("the still image");
    Some((result.image, result.stats.mem_textures))
}

fn pixel(image: &PremultipliedImage, x: u32, y: u32) -> [u8; 4] {
    let start = ((y * image.size.width + x) * 4) as usize;
    image.data[start..start + 4].try_into().unwrap()
}

/// How many pixels of `a` and `b` differ, as the render tests count them.
fn differing(a: &PremultipliedImage, b: &PremultipliedImage) -> u64 {
    pixelmatch::pixelmatch(&a.data, &b.data, SIDE as usize, SIDE as usize, None, 0.1285, false)
}

#[test]
fn a_background_and_images_drape_over_flat_terrain() {
    let (Some((draped, draped_textures)), Some((flat, flat_textures))) =
        (render(&style(true), ViewportMode::Default), render(&style(false), ViewportMode::Default))
    else {
        return;
    };
    // At least one drape of 1024² texels for the terrain's 512 px tiles.
    assert!(draped_textures - flat_textures >= 1024 * 1024 * 4, "the drapes' textures");
    let center = SIDE / 2;
    assert_eq!([255, 0, 0, 255], pixel(&draped, center, center), "the red image");
    // 10° north is 57 px north of the equator in the 2048 px of the world
    // at zoom 2.
    assert_eq!([0, 0, 255, 255], pixel(&draped, center, center - 57), "the blue image");
    assert_eq!([255, 255, 255, 255], pixel(&draped, 4, 4), "the background");
    let differing = differing(&draped, &flat);
    assert!(differing <= EDGE_PIXELS, "{differing} pixels differ from the map without terrain");
}

/// A viewport flipped upside down, which turns the surface's triangles
/// round, still shows the surface, drawn upside down.
#[test]
fn a_flipped_viewport_shows_the_terrain() {
    let (Some((flipped, _)), Some((flat, _))) =
        (render(&style(true), ViewportMode::FlippedY), render(&style(false), ViewportMode::FlippedY))
    else {
        return;
    };
    let center = SIDE / 2;
    assert_eq!([0, 0, 255, 255], pixel(&flipped, center, center + 57), "the blue image, south of the center");
    let differing = differing(&flipped, &flat);
    assert!(differing <= EDGE_PIXELS, "{differing} pixels differ from the map without terrain");
}

/// A circle between two images stands between their stacks: the image below
/// shows under it, and the image above covers it, as without terrain.
#[test]
fn a_circle_stands_between_two_stacks_of_draped_layers() {
    let (Some((draped, _)), Some((flat, _))) =
        (render(&stacked_style(true), ViewportMode::Default), render(&stacked_style(false), ViewportMode::Default))
    else {
        return;
    };
    let center = SIDE / 2;
    assert_eq!([0, 0, 255, 255], pixel(&draped, center, center), "the blue image over the circle's center");
    // 1° is 5.7 px in the 2048 px of the world at zoom 2: the blue image
    // starts 6 px west of the center, and the circle reaches 15 px.
    assert_eq!([0, 255, 0, 255], pixel(&draped, center - 11, center), "the circle west of the blue image");
    assert_eq!([255, 0, 0, 255], pixel(&draped, center - 30, center), "the red image around the circle");
    let differing = differing(&draped, &flat);
    assert!(differing <= EDGE_PIXELS, "{differing} pixels differ from the map without terrain");
}

/// A GeoJSON source `id` of a polygon for each of `boxes`, each from west
/// to east and from south to north, in degrees.
fn polygon_source(id: &str, boxes: &[[f64; 4]]) -> String {
    let features: Vec<String> = boxes
        .iter()
        .map(|[west, south, east, north]| {
            format!(
                r#"{{"type": "Feature", "properties": {{}}, "geometry": {{"type": "Polygon", "coordinates": [[[{west}, {north}], [{east}, {north}], [{east}, {south}], [{west}, {south}], [{west}, {north}]]]}}}}"#
            )
        })
        .collect();
    format!(
        r#""{id}": {{"type": "geojson", "data": {{"type": "FeatureCollection", "features": [{}]}}}}"#,
        features.join(",")
    )
}

/// A map without a background, of a red fill around the center, a blue
/// line across it north of the center and two green fills that overlap,
/// half transparent as a whole, over the red fill's east half and the line;
/// with a terrain over flat ground if `terrain`. Its layers all drape.
fn shapes_style(terrain: bool) -> String {
    let sources = [
        polygon_source("red", &[[-5.0, -5.0, 5.0, 5.0]]),
        r#""line": {"type": "geojson", "data": {"type": "LineString", "coordinates": [[-60, 10], [60, 10]]}}"#
            .to_owned(),
        polygon_source("green", &[[0.0, -20.0, 20.0, 20.0], [0.0, -2.0, 30.0, 2.0]]),
    ];
    let layers = [
        r#"{"id": "red", "type": "fill", "source": "red", "paint": {"fill-color": "red"}}"#,
        r#"{"id": "line", "type": "line", "source": "line", "paint": {"line-color": "blue", "line-width": 6}}"#,
        r#"{"id": "green", "type": "fill", "source": "green", "paint": {"fill-color": "lime", "fill-layer-opacity": 0.5}}"#,
    ];
    style_of(terrain, &sources.join(","), &layers.join(","))
}

/// Fills and lines drape, and a layer that is half transparent as a whole
/// draws so inside the drapes, where its fills overlap too: over flat
/// ground seen from above, the map looks as it does without terrain.
#[test]
fn fills_and_lines_drape_over_flat_terrain() {
    let (Some((draped, draped_textures)), Some((flat, flat_textures))) =
        (render(&shapes_style(true), ViewportMode::Default), render(&shapes_style(false), ViewportMode::Default))
    else {
        return;
    };
    // At least one drape of 1024² texels for the terrain's 512 px tiles:
    // without a layer that drapes, there would be none.
    assert!(draped_textures - flat_textures >= 1024 * 1024 * 4, "the drapes' textures");
    let center = SIDE / 2;
    assert_eq!([255, 0, 0, 255], pixel(&draped, center - 10, center), "the red fill");
    // 10° north is 57 px north of the equator in the 2048 px of the world
    // at zoom 2.
    assert_eq!([0, 0, 255, 255], pixel(&draped, center - 60, center - 57), "the blue line");
    // Half the green over the red, and half the green alone over nothing.
    let [r, g, b, a] = pixel(&draped, center + 10, center);
    assert!(r.abs_diff(128) <= 2 && g.abs_diff(128) <= 2 && b == 0 && a == 255, "green over red: {:?}", [r, g, b, a]);
    let [r, g, b, a] = pixel(&draped, center + 60, center);
    assert!(r == 0 && g.abs_diff(128) <= 2 && b == 0 && a.abs_diff(128) <= 2, "green alone: {:?}", [r, g, b, a]);
    let differing = differing(&draped, &flat);
    assert!(differing <= EDGE_PIXELS, "{differing} pixels differ from the map without terrain");
}
