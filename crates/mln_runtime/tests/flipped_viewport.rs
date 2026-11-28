//! A viewport flipped upside down draws the map upside down: the image,
//! turned back over, is the map drawn the right way up.

use mln_engine::camera::{BoundOptions, ViewportMode};
use mln_style::bitmap::PremultipliedImage;

use crate::support::still::{StillMap, StillOptions, differing};
use crate::support::wait;

/// The still image of the map drawn with `style` in `viewport_mode`, pitched
/// as the style says, up to 85°.
fn render(style: &str, viewport_mode: ViewportMode) -> PremultipliedImage {
    let mut options = StillOptions::default();
    options.map.map.viewport_mode = viewport_mode;
    let map = StillMap::new(options);
    wait(map.map.set_bounds(BoundOptions { max_pitch: Some(85.0), ..Default::default() }));
    map.load_json(style);
    map.render()
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

/// How many pixels of the map drawn with `style` in a flipped viewport,
/// turned back over, differ from the map drawn the right way up.
fn differing_when_flipped(style: &str) -> u64 {
    let upright = render(style, ViewportMode::Default);
    let upside_down = render(style, ViewportMode::FlippedY);
    differing(&flipped(&upside_down), &upright)
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

/// A flipped viewport shows a box's outer walls, as the upright one does.
#[test]
fn a_flipped_viewport_shows_a_fill_extrusions_outer_walls() {
    let differing = differing_when_flipped(EXTRUSION);
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
    let upright = render(GLOBE_EXTRUSION, ViewportMode::Default);
    let upside_down = render(GLOBE_EXTRUSION, ViewportMode::FlippedY);
    let differing = differing(&upside_down, &upright).min(differing(&flipped(&upside_down), &upright));
    assert!(differing <= 16, "{differing} pixels differ from the extrusion drawn the right way up");
}
