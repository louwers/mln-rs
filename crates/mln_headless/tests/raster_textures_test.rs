// The textures that raster, hillshade and color-relief layers keep of their
// tiles, through Map -> HeadlessFrontend over raster tiles that the test
// makes.

use std::sync::Arc;

use mln_engine::camera::CameraOptions;
use mln_engine::map::{Map, MapMode, MapOptions};
use mln_gfx::rendering_stats::RenderingStats;
use mln_headless::HeadlessFrontend;
use mln_storage::stub_file_source::{ResponseType, StubFileSource};
use mln_storage::{Kind, Resource, Response};
use mln_style::bitmap::PremultipliedImage;
use mln_style::error::ErrorKind;
use mln_style::geo::LatLng;
use mln_style::size::Size;
use mln_util::pool::Pool;
use mln_util::queue::Queue;

/// The layer types, each with the paint property that hides it at 0.
const LAYERS: [(&str, &str); 3] =
    [("raster", "raster-opacity"), ("hillshade", "hillshade-exaggeration"), ("color-relief", "color-relief-opacity")];

/// Longitudes of views in the source's bounds, on tiles none of the others
/// show, and of one out of them, at zoom 12.
const IN_BOUNDS: [f64; 2] = [0.0, 0.3];
const OUT_OF_BOUNDS: f64 = 20.0;

/// A tile of 256 pixels a side, in the Mapbox DEM encoding, of a plane
/// that rises to the north and the east.
fn tile_png(z: i8, y: i32) -> Vec<u8> {
    let tiles = f64::from(1u32 << z);
    let mut image = PremultipliedImage::new(Size::new(256, 256));
    for (index, pixel) in image.data.as_chunks_mut::<4>().0.iter_mut().enumerate() {
        let y = (f64::from(y) + ((index / 256) as f64 + 0.5) / 256.0) / tiles;
        let height = (0.5 - y) * 5_000_000.0 + (index % 256) as f64 * 3.0;
        let value = ((height + 10000.0) * 10.0).round() as u32;
        *pixel = [(value >> 16) as u8, (value >> 8) as u8, value as u8, 255];
    }
    mln_image::encode_png(&image)
}

/// A map of a layer of `layer_type` over a source bounded to a few degrees
/// around the origin, or `None` without a device.
fn map(layer_type: &str, run_loop: &Queue<()>, pool: &Pool) -> Option<(HeadlessFrontend, Map)> {
    let device = match mln_headless::harness_device() {
        Ok(device) => device,
        Err(error) if error.kind == ErrorKind::Unsupported => {
            eprintln!("skipped: no device: {error}");
            return None;
        }
        Err(error) => panic!("a device: {error}"),
    };
    let size = Size::new(512, 512);
    let mut frontend =
        HeadlessFrontend::with_device(device, size, 1.0, Arc::default(), run_loop, pool).expect("a frontend");
    let file_source = Arc::new(StubFileSource::new(ResponseType::Synchronous));
    file_source.set_response(Kind::Tile, |resource: &Resource| {
        let tile = resource.tile_data.as_ref().expect("a tile");
        let mut response = Response::new();
        response.data = Some(Arc::new(tile_png(tile.z, tile.y)));
        Some(response)
    });
    let options = MapOptions { map_mode: MapMode::Static, size, ..Default::default() };
    let mut map = Map::with_file_source(file_source, options, pool).unwrap();
    frontend.attach(&mut map);
    let source_type = if layer_type == "raster" { "raster" } else { "raster-dem" };
    let paint = if layer_type == "color-relief" {
        r#"{"color-relief-color": ["interpolate", ["linear"], ["elevation"], 0, "blue", 3000, "red"]}"#
    } else {
        "{}"
    };
    map.load_style_json(&format!(
        r#"{{
        "version": 8,
        "sources": {{
            "s": {{"type": "{source_type}", "tiles": ["{{z}}/{{x}}/{{y}}.png"], "tileSize": 256, "bounds": [-3, -3, 3, 3]}}
        }},
        "layers": [{{"id": "layer", "type": "{layer_type}", "source": "s", "paint": {paint}}}]
    }}"#
    ));
    Some((frontend, map))
}

/// The stats after a still of the view at `longitude`, at zoom 12.
fn still_at(frontend: &mut HeadlessFrontend, map: &mut Map, longitude: f64) -> RenderingStats {
    let center = LatLng::new(0.5, longitude).unwrap();
    map.jump_to(&CameraOptions { center: Some(center), zoom: Some(12.0), ..Default::default() });
    frontend.render(map).expect("the still image").stats
}

fn set_paint(map: &mut Map, name: &str, value: &str) {
    map.edit_loaded_style(|style| style.set_layer_property_json("layer", name, value)).expect("the paint property");
}

/// A layer keeps the textures of the tiles in view, and lets go of the
/// others while it draws none: with no tiles in view, or hidden by its
/// paint while the view moves.
#[test]
fn a_layer_keeps_the_textures_of_the_tiles_in_view_only() {
    let (run_loop, pool) = (Queue::new(), Pool::new(4));
    for (layer_type, hiding_property) in LAYERS {
        let Some((mut frontend, mut map)) = map(layer_type, &run_loop, &pool) else {
            return;
        };
        let in_view = still_at(&mut frontend, &mut map, IN_BOUNDS[0]).num_active_textures;
        let none = still_at(&mut frontend, &mut map, OUT_OF_BOUNDS).num_active_textures;
        assert!(none < in_view, "{layer_type}: {in_view} textures with tiles in view, {none} without");

        still_at(&mut frontend, &mut map, IN_BOUNDS[0]);
        set_paint(&mut map, hiding_property, "0");
        let hidden = still_at(&mut frontend, &mut map, IN_BOUNDS[1]).num_active_textures;
        assert_eq!(hidden, none, "{layer_type}: textures hidden in another view");
    }
}

/// A layer hidden by its paint keeps the textures of the tiles in view, and
/// shows them again without making them again.
#[test]
fn a_layer_hidden_in_place_keeps_its_textures() {
    let (run_loop, pool) = (Queue::new(), Pool::new(4));
    // A color relief makes its color ramps' textures again with any change
    // to its paint.
    for (layer_type, hiding_property) in &LAYERS[..2] {
        let Some((mut frontend, mut map)) = map(layer_type, &run_loop, &pool) else {
            return;
        };
        let shown = still_at(&mut frontend, &mut map, IN_BOUNDS[0]);
        set_paint(&mut map, hiding_property, "0");
        let hidden = still_at(&mut frontend, &mut map, IN_BOUNDS[0]);
        set_paint(&mut map, hiding_property, "1");
        let shown_again = still_at(&mut frontend, &mut map, IN_BOUNDS[0]);
        for stats in [&hidden, &shown_again] {
            assert_eq!(stats.num_active_textures, shown.num_active_textures, "{layer_type}");
            assert_eq!(stats.num_created_textures, shown.num_created_textures, "{layer_type}");
        }
    }
}
