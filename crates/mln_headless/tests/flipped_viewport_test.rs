// A viewport flipped upside down draws the map upside down: the image,
// turned back over, is the map drawn the right way up.

use std::sync::Arc;

use mln_headless::HeadlessFrontend;
use mln_map::{BoundOptions, Map, MapMode, MapOptions, ViewportMode};
use mln_storage::stub_file_source::{ResponseType, StubFileSource};
use mln_style::bitmap::PremultipliedImage;
use mln_style::error::ErrorKind;
use mln_style::size::Size;
use mln_util::pool::Pool;
use mln_util::queue::Queue;

const SIDE: u32 = 256;

/// The map drawn with `style` in `viewport_mode`, or `None` without a
/// device.
fn render(style: &str, viewport_mode: ViewportMode) -> Option<PremultipliedImage> {
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
    let options = MapOptions { map_mode: MapMode::Static, size, pixel_ratio: 1.0, viewport_mode, ..Default::default() };
    let mut map = Map::with_file_source(file_source, options, &pool).unwrap();
    frontend.attach(&mut map);
    map.set_bounds(&BoundOptions { max_pitch: Some(85.0), ..Default::default() }).unwrap();
    map.load_style_json(style);
    let camera = mln_map::style_camera(map.style());
    map.jump_to(&camera);
    Some(frontend.render(&mut map).expect("the still image").image)
}

/// `image` turned upside down.
fn flipped(image: &PremultipliedImage) -> PremultipliedImage {
    let row = (image.size.width * 4) as usize;
    let mut turned = PremultipliedImage::new(image.size);
    for (to, from) in turned.data.chunks_exact_mut(row).zip(image.data.chunks_exact(row).rev()) {
        to.copy_from_slice(from);
    }
    turned
}

fn differing(image: &PremultipliedImage, other: &PremultipliedImage) -> u64 {
    pixelmatch::pixelmatch(&image.data, &other.data, SIDE as usize, SIDE as usize, None, 0.1285, false)
}

/// How many pixels of the map drawn with `style` in a flipped viewport,
/// turned back over, differ from the map drawn the right way up.
fn differing_when_flipped(style: &str) -> Option<u64> {
    let upright = render(style, ViewportMode::Default)?;
    let upside_down = render(style, ViewportMode::FlippedY)?;
    Some(differing(&flipped(&upside_down), &upright))
}

/// A tall box, pitched and turned so that two of its walls and its roof
/// face the camera.
const EXTRUSION: &str = r#"{
    "version": 8,
    "center": [0, 0],
    "zoom": 15,
    "pitch": 60,
    "bearing": 30,
    "sources": {
        "box": {
            "type": "geojson",
            "data": {
                "type": "Polygon",
                "coordinates": [[[-0.001, -0.001], [0.001, -0.001], [0.001, 0.001], [-0.001, 0.001], [-0.001, -0.001]]]
            }
        }
    },
    "layers": [
        {"id": "background", "type": "background", "paint": {"background-color": "white"}},
        {
            "id": "box",
            "type": "fill-extrusion",
            "source": "box",
            "paint": {"fill-extrusion-color": "red", "fill-extrusion-height": 200}
        }
    ]
}"#;

#[test]
fn a_flipped_viewport_shows_a_fill_extrusions_outer_walls() {
    let Some(differing) = differing_when_flipped(EXTRUSION) else {
        return;
    };
    assert!(differing <= 16, "{differing} pixels differ from the extrusion drawn the right way up");
}

/// A tall box on the globe, pitched and turned as [`EXTRUSION`].
const GLOBE_EXTRUSION: &str = r#"{
    "version": 8,
    "projection": {"type": "globe"},
    "center": [0, 0],
    "zoom": 3,
    "pitch": 50,
    "bearing": 30,
    "sources": {
        "box": {
            "type": "geojson",
            "data": {"type": "Polygon", "coordinates": [[[-5, -5], [5, -5], [5, 5], [-5, 5], [-5, -5]]]}
        }
    },
    "layers": [
        {"id": "background", "type": "background", "paint": {"background-color": "white"}},
        {
            "id": "box",
            "type": "fill-extrusion",
            "source": "box",
            "paint": {"fill-extrusion-color": "red", "fill-extrusion-height": 1000000}
        }
    ]
}"#;

/// On the globe too, a flipped viewport shows the box's outer walls,
/// whichever way up the globe's projection draws it.
#[test]
fn a_flipped_viewport_shows_a_fill_extrusions_outer_walls_on_the_globe() {
    let (Some(upright), Some(upside_down)) =
        (render(GLOBE_EXTRUSION, ViewportMode::Default), render(GLOBE_EXTRUSION, ViewportMode::FlippedY))
    else {
        return;
    };
    let differing = differing(&upside_down, &upright).min(differing(&flipped(&upside_down), &upright));
    assert!(differing <= 16, "{differing} pixels differ from the extrusion drawn the right way up");
}
