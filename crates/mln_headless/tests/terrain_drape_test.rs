// The layers that drape over terrain draw into the terrain tiles' drapes,
// which the surface shows: over flat ground, seen from above, the map looks
// as it does without terrain.

use std::sync::Arc;

use mln_headless::HeadlessFrontend;
use mln_map::{BoundOptions, Map, MapMode, MapOptions, ViewportMode};
use mln_storage::stub_file_source::{ResponseType, StubFileSource};
use mln_storage::{Kind, Resource, Response};
use mln_util::error::ErrorKind;
use mln_util::image::PremultipliedImage;
use mln_util::pool::Pool;
use mln_util::queue::Queue;
use mln_util::size::Size;

const SIDE: u32 = 256;

/// How many pixels a draped map may differ by from the same map drawn flat:
/// the surface filters its drapes, so pixels on the shapes' edges differ, and
/// how many depends on the GPU.
const EDGE_PIXELS: u64 = 64;

/// A DEM tile of sea level everywhere, in the Mapbox encoding: (1 · 65536 +
/// 134 · 256 + 160) · 0.1 − 10000 = 0 m.
fn sea_level_png() -> Vec<u8> {
    let mut image = PremultipliedImage::new(Size::new(256, 256));
    for pixel in image.data.as_chunks_mut::<4>().0 {
        pixel.copy_from_slice(&[1, 134, 160, 255]);
    }
    mln_image::encode_png(&image)
}

/// A white map with a red square around the center and a blue line across
/// it, at an integer zoom, seen from above; with a terrain over flat ground
/// if `terrain`.
fn style(terrain: bool) -> String {
    let terrain = if terrain { r#""terrain": {"source": "dem"},"# } else { "" };
    format!(
        r#"{{
            "version": 8,
            "center": [0, 0],
            "zoom": 2,
            {terrain}
            "sources": {{
                "dem": {{"type": "raster-dem", "tiles": ["dem/{{z}}/{{x}}/{{y}}.png"], "tileSize": 256, "maxzoom": 10}},
                "shapes": {{
                    "type": "geojson",
                    "data": {{
                        "type": "FeatureCollection",
                        "features": [
                            {{
                                "type": "Feature",
                                "properties": {{}},
                                "geometry": {{
                                    "type": "Polygon",
                                    "coordinates": [[[-5, -5], [5, -5], [5, 5], [-5, 5], [-5, -5]]]
                                }}
                            }},
                            {{
                                "type": "Feature",
                                "properties": {{}},
                                "geometry": {{"type": "LineString", "coordinates": [[-60, 10], [60, 10]]}}
                            }}
                        ]
                    }}
                }}
            }},
            "layers": [
                {{"id": "background", "type": "background", "paint": {{"background-color": "white"}}}},
                {{
                    "id": "fill",
                    "type": "fill",
                    "source": "shapes",
                    "filter": ["==", "$type", "Polygon"],
                    "paint": {{"fill-color": "red"}}
                }},
                {{
                    "id": "line",
                    "type": "line",
                    "source": "shapes",
                    "filter": ["==", "$type", "LineString"],
                    "paint": {{"line-color": "blue", "line-width": 6}}
                }}
            ]
        }}"#
    )
}

/// A white map with a red square around the center, a green circle at the
/// center, and a blue rectangle over the circle's east half, at an integer
/// zoom, seen from above; with a terrain over flat ground if `terrain`. The
/// circle splits the draped layers into two stacks.
fn stacked_style(terrain: bool) -> String {
    let terrain = if terrain { r#""terrain": {"source": "dem"},"# } else { "" };
    format!(
        r#"{{
            "version": 8,
            "center": [0, 0],
            "zoom": 2,
            {terrain}
            "sources": {{
                "dem": {{"type": "raster-dem", "tiles": ["dem/{{z}}/{{x}}/{{y}}.png"], "tileSize": 256, "maxzoom": 10}},
                "red": {{
                    "type": "geojson",
                    "data": {{"type": "Polygon", "coordinates": [[[-8, -8], [8, -8], [8, 8], [-8, 8], [-8, -8]]]}}
                }},
                "point": {{"type": "geojson", "data": {{"type": "Point", "coordinates": [0, 0]}}}},
                "blue": {{
                    "type": "geojson",
                    "data": {{"type": "Polygon", "coordinates": [[[-1, -5], [5, -5], [5, 5], [-1, 5], [-1, -5]]]}}
                }}
            }},
            "layers": [
                {{"id": "background", "type": "background", "paint": {{"background-color": "white"}}}},
                {{"id": "red", "type": "fill", "source": "red", "paint": {{"fill-color": "red"}}}},
                {{
                    "id": "circle",
                    "type": "circle",
                    "source": "point",
                    "paint": {{"circle-color": "lime", "circle-radius": 15, "circle-blur": 0}}
                }},
                {{"id": "blue", "type": "fill", "source": "blue", "paint": {{"fill-color": "blue"}}}}
            ]
        }}"#
    )
}

/// Where the blue fill of [`fogged_style`] is: in the upper stack, in the
/// lower one, or nowhere.
#[derive(Clone, Copy, PartialEq)]
enum Blue {
    Above,
    Below,
    None,
}

/// The sky of [`fogged_style`]: a magenta fog over the whole ground.
const MAGENTA_FOG: &str = r#""sky": {"fog-color": "magenta", "fog-ground-blend": 0},"#;

/// A white map over flat terrain pitched 75°, under [`MAGENTA_FOG`], with a circle near the camera and an opaque blue fill over
/// the ground west of 1° W, in the stack above the circle or below it.
fn fogged_style(blue: Blue) -> String {
    let fill = r#"{"id": "blue", "type": "fill", "source": "blue", "paint": {"fill-color": "blue"}}"#;
    let circle = r#"{"id": "circle", "type": "circle", "source": "point", "paint": {"circle-color": "lime"}}"#;
    let layers = match blue {
        Blue::Above => format!("{circle}, {fill}"),
        Blue::Below => format!("{fill}, {circle}"),
        Blue::None => circle.to_owned(),
    };
    format!(
        r#"{{
            "version": 8,
            "center": [0, 0],
            "zoom": 4,
            "pitch": 75,
            "terrain": {{"source": "dem"}},
            {MAGENTA_FOG}
            "sources": {{
                "dem": {{"type": "raster-dem", "tiles": ["dem/{{z}}/{{x}}/{{y}}.png"], "tileSize": 256, "maxzoom": 10}},
                "point": {{"type": "geojson", "data": {{"type": "Point", "coordinates": [0, -4]}}}},
                "blue": {{
                    "type": "geojson",
                    "data": {{"type": "Polygon", "coordinates": [[[-170, -80], [-1, -80], [-1, 80], [-170, 80], [-170, -80]]]}}
                }}
            }},
            "layers": [
                {{"id": "background", "type": "background", "paint": {{"background-color": "white"}}}},
                {layers}
            ]
        }}"#
    )
}

/// The still image of `style` on the harness's device
/// ([`mln_headless::harness_device`]), and the bytes of the textures that
/// drew it; `None` where the machine has no such device.
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
    let options = MapOptions { map_mode: MapMode::Static, size, pixel_ratio: 1.0, viewport_mode, ..Default::default() };
    let mut map = Map::with_file_source(file_source, options, &pool).unwrap();
    frontend.attach(&mut map);
    map.set_bounds(&BoundOptions { max_pitch: Some(85.0), ..Default::default() }).unwrap();
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

#[test]
fn a_fill_and_a_line_drape_over_flat_terrain() {
    let (Some((draped, draped_textures)), Some((flat, flat_textures))) =
        (render(&style(true), ViewportMode::Default), render(&style(false), ViewportMode::Default))
    else {
        return;
    };
    // At least one drape of 1024² texels for the terrain's 512 px tiles.
    assert!(draped_textures - flat_textures >= 1024 * 1024 * 4, "the drapes' textures");
    let center = SIDE / 2;
    assert_eq!([255, 0, 0, 255], pixel(&draped, center, center), "the fill");
    // 10° north is 57 px north of the equator in the 2048 px of the world
    // at zoom 2.
    assert_eq!([0, 0, 255, 255], pixel(&draped, center, center - 57), "the line");
    assert_eq!([255, 255, 255, 255], pixel(&draped, 4, 4), "the background");
    let differing = pixelmatch::pixelmatch(&draped.data, &flat.data, SIDE as usize, SIDE as usize, None, 0.1285, false);
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
    assert_eq!([0, 0, 255, 255], pixel(&flipped, center, center + 57), "the line, south of the center");
    let differing =
        pixelmatch::pixelmatch(&flipped.data, &flat.data, SIDE as usize, SIDE as usize, None, 0.1285, false);
    assert!(differing <= EDGE_PIXELS, "{differing} pixels differ from the map without terrain");
}

/// A circle between two fills stands between their stacks: the fill below
/// shows under it, and the fill above covers it, as without terrain.
#[test]
fn a_circle_stands_between_two_stacks_of_draped_layers() {
    let (Some((draped, _)), Some((flat, _))) =
        (render(&stacked_style(true), ViewportMode::Default), render(&stacked_style(false), ViewportMode::Default))
    else {
        return;
    };
    let center = SIDE / 2;
    assert_eq!([0, 0, 255, 255], pixel(&draped, center, center), "the blue fill over the circle's center");
    // 1° is 5.7 px in the 2048 px of the world at zoom 2: the blue fill
    // starts 6 px west of the center, and the circle reaches 15 px.
    assert_eq!([0, 255, 0, 255], pixel(&draped, center - 11, center), "the circle west of the blue fill");
    assert_eq!([255, 0, 0, 255], pixel(&draped, center - 30, center), "the red fill around the circle");
    let differing = pixelmatch::pixelmatch(&draped.data, &flat.data, SIDE as usize, SIDE as usize, None, 0.1285, false);
    assert!(differing <= EDGE_PIXELS, "{differing} pixels differ from the map without terrain");
}

/// The fog lands once on each pixel, however many stacks drape the surface:
/// where the stack above draws nothing, the ground shows as it does without
/// that stack, and inside its opaque fill, as it does with the fill in the
/// stack below. The far ground it compares is fogged.
#[test]
fn fog_lands_once_where_stacks_overlap() {
    let (Some((above, _)), Some((none, _)), Some((below, _)), Some((clear, _))) = (
        render(&fogged_style(Blue::Above), ViewportMode::Default),
        render(&fogged_style(Blue::None), ViewportMode::Default),
        render(&fogged_style(Blue::Below), ViewportMode::Default),
        render(&fogged_style(Blue::None).replace(MAGENTA_FOG, ""), ViewportMode::Default),
    ) else {
        return;
    };
    // Far ground east of the fill, where the stack above draws nothing.
    for (x, y) in [(210, 70), (200, 60)] {
        assert_ne!(pixel(&clear, x, y), pixel(&none, x, y), "the fog at {x}, {y}");
        assert_eq!(pixel(&none, x, y), pixel(&above, x, y), "the ground at {x}, {y}");
    }
    // Far ground inside the fill.
    for (x, y) in [(40, 70), (30, 100)] {
        assert_ne!(pixel(&none, x, y), pixel(&above, x, y), "the fill at {x}, {y}");
        assert_eq!(pixel(&below, x, y), pixel(&above, x, y), "the fill at {x}, {y}");
    }
}
