// A hillshade over 3D terrain, lit from the viewport, keeps its light where
// the viewport puts it as the map turns, and shades each slope as a
// hillshade lit from the map at the same direction does.

use std::sync::Arc;

use mln_engine::map::{Map, MapMode, MapOptions};
use mln_headless::HeadlessFrontend;
use mln_storage::stub_file_source::{ResponseType, StubFileSource};
use mln_storage::{Kind, Resource, Response};
use mln_style::bitmap::PremultipliedImage;
use mln_style::error::ErrorKind;
use mln_style::size::Size;
use mln_util::pool::Pool;
use mln_util::queue::Queue;

const SIDE: u32 = 256;

/// A DEM tile of 256 pixels a side, in the Mapbox encoding, of a round hill
/// at 0° N 0° E, 1500 m high and about 2 km in radius, over flat ground at
/// sea level.
fn hill_png(z: i8, x: i32, y: i32) -> Vec<u8> {
    const CIRCUMFERENCE: f64 = 40_075_016.685_578_49;
    let tiles = f64::from(1u32 << z);
    let mut image = PremultipliedImage::new(Size::new(256, 256));
    for (index, pixel) in image.data.as_chunks_mut::<4>().0.iter_mut().enumerate() {
        let east = ((f64::from(x) + ((index % 256) as f64 + 0.5) / 256.0) / tiles - 0.5) * CIRCUMFERENCE;
        let south = ((f64::from(y) + ((index / 256) as f64 + 0.5) / 256.0) / tiles - 0.5) * CIRCUMFERENCE;
        let height = 1500.0 * (-(east * east + south * south) / (2000.0 * 2000.0)).exp();
        let value = ((height + 10000.0) * 10.0).round() as u32;
        *pixel = [(value >> 16) as u8, (value >> 8) as u8, value as u8, 255];
    }
    mln_image::encode_png(&image)
}

/// The still image, seen from above at `bearing`, of the hill over terrain
/// under a hillshade whose paint values are `paint`; `None` where the
/// machine has no device ([`mln_headless::harness_device`]).
fn render(bearing: f64, paint: &str) -> Option<PremultipliedImage> {
    let style = format!(
        r#"{{
            "version": 8,
            "center": [0, 0],
            "zoom": 11,
            "bearing": {bearing},
            "terrain": {{"source": "dem"}},
            "sources": {{
                "dem": {{"type": "raster-dem", "tiles": ["dem/{{z}}/{{x}}/{{y}}.png"], "tileSize": 256, "maxzoom": 11}}
            }},
            "layers": [
                {{"id": "background", "type": "background", "paint": {{"background-color": "white"}}}},
                {{"id": "hillshade", "type": "hillshade", "source": "dem", "paint": {paint}}}
            ]
        }}"#
    );
    let device = match mln_headless::harness_device() {
        Ok(device) => device,
        Err(error) if error.kind == ErrorKind::Unsupported => {
            eprintln!("skipped: no device: {error}");
            return None;
        }
        Err(error) => panic!("a device: {error}"),
    };
    let size = Size::new(SIDE, SIDE);
    let (run_loop, pool) = (Queue::new(), Pool::new(4));
    let mut frontend =
        HeadlessFrontend::with_device(device, size, 1.0, Arc::default(), &run_loop, &pool).expect("a frontend");
    let file_source = Arc::new(StubFileSource::new(ResponseType::Synchronous));
    file_source.set_response(Kind::Tile, |resource: &Resource| {
        let tile = resource.tile_data.as_ref().expect("a tile");
        let mut response = Response::new();
        response.data = Some(Arc::new(hill_png(tile.z, tile.x, tile.y)));
        Some(response)
    });
    let options = MapOptions { map_mode: MapMode::Static, size, ..Default::default() };
    let mut map = Map::with_file_source(file_source, options, &pool).unwrap();
    frontend.attach(&mut map);
    map.load_style_json(&style);
    let camera = mln_engine::map::style_camera(map.style());
    map.jump_to(&camera);
    Some(frontend.render(&mut map).expect("the still image").image)
}

/// How many pixels of `a` and `b` differ, as the render tests count them.
fn differing(a: &PremultipliedImage, b: &PremultipliedImage) -> u64 {
    pixelmatch::pixelmatch(&a.data, &b.data, SIDE as usize, SIDE as usize, None, 0.1285, false)
}

/// How many pixels the hill's shading differs by between ways of drawing
/// it: its pixels sample the DEM, the slopes and the drapes at other places.
const SAMPLED_PIXELS: u64 = 64;

/// The light of a hillshade lit from the viewport stays at the top left of
/// the viewport: a round hill looks the same at every bearing.
#[test]
fn a_viewport_light_stays_with_the_viewport_as_the_map_turns() {
    let paint = r#"{"hillshade-illumination-anchor": "viewport", "hillshade-exaggeration": 1}"#;
    let Some(north_up) = render(0.0, paint) else { return };
    for bearing in [90.0, 135.0] {
        let turned = render(bearing, paint).expect("an image");
        let differ = differing(&north_up, &turned);
        assert!(differ <= SAMPLED_PIXELS, "{differ} pixels differ at a bearing of {bearing}");
    }
    // The hill is shaded: lit from the map, its shading turns with it.
    let map_lit = r#"{"hillshade-illumination-anchor": "map", "hillshade-exaggeration": 1}"#;
    let differ = differing(&render(0.0, map_lit).expect("an image"), &render(90.0, map_lit).expect("an image"));
    assert!(differ > 1000, "only {differ} pixels differ as a hill lit from the map turns");
}

/// A hillshade lit from the viewport, whose drapes hold its slopes that the
/// surface shades, looks as one lit from the map at the direction the
/// viewport's light has on the map, whose drapes hold its colors.
#[test]
fn a_viewport_light_shades_as_the_map_light_it_points_as() {
    let methods = ["standard", "basic", "combined", "igor", "multidirectional"];
    for method in methods {
        let viewport = format!(
            r#"{{"hillshade-illumination-anchor": "viewport", "hillshade-illumination-direction": 300,
                "hillshade-exaggeration": 1, "hillshade-method": "{method}"}}"#
        );
        let Some(lit_on_surface) = render(100.0, &viewport) else { return };
        let map = format!(
            r#"{{"hillshade-illumination-anchor": "map", "hillshade-illumination-direction": 40,
                "hillshade-exaggeration": 1, "hillshade-method": "{method}"}}"#
        );
        let lit_in_drape = render(100.0, &map).expect("an image");
        let differ = differing(&lit_on_surface, &lit_in_drape);
        assert!(differ <= SAMPLED_PIXELS, "{differ} pixels differ for the {method} method");
    }
}
