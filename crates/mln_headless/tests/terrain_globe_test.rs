// Terrain on the globe: at zoom 14, where the sphere's curvature moves the
// ground by a fraction of a pixel, a view under `vertical-perspective` draws
// what the same view on Mercator draws.

use std::f64::consts::PI;
use std::sync::Arc;

use mln_headless::HeadlessFrontend;
use mln_map::{BoundOptions, Map, MapMode, MapOptions};
use mln_storage::resource::TileData;
use mln_storage::stub_file_source::{ResponseType, StubFileSource};
use mln_storage::{Kind, Resource, Response};
use mln_util::error::ErrorKind;
use mln_util::image::PremultipliedImage;
use mln_util::pool::Pool;
use mln_util::queue::Queue;
use mln_util::size::Size;

const SIDE: u32 = 256;

/// The view's center, on level ground at sea level.
const CENTER: (f64, f64) = (10.0, 20.0);

/// Meters a degree of latitude spans on GL JS's sphere.
const METERS_PER_DEGREE: f64 = 6_371_008.8 * PI / 180.0;

/// The location `north` and `east` meters from [`CENTER`], as longitude and
/// latitude.
fn offset(north: f64, east: f64) -> (f64, f64) {
    let (lon, lat) = CENTER;
    (lon + east / (METERS_PER_DEGREE * lat.to_radians().cos()), lat + north / METERS_PER_DEGREE)
}

/// The height of the ground `north` meters north of the center: a ridge
/// running east to west, 400 m high 180 m north, falling to sea level 150 m
/// either side of its crest, which the camera, about 1,000 m up, sees from
/// the south.
fn ridge_height(north: f64) -> f64 {
    let across = (north - 180.0) / 150.0;
    if across.abs() >= 1.0 { 0.0 } else { 400.0 * (across * PI / 2.0).cos().powi(2) }
}

/// The DEM tile `tile` of the ridge, in the Mapbox encoding.
fn ridge_png(tile: &TileData) -> Vec<u8> {
    let tiles = 2f64.powi(i32::from(tile.z));
    let mut image = PremultipliedImage::new(Size::new(256, 256));
    for (index, pixel) in image.data.as_chunks_mut::<4>().0.iter_mut().enumerate() {
        let row = (index / 256) as f64 + 0.5;
        let y = (f64::from(tile.y) + row / 256.0) / tiles;
        let latitude = (PI * (1.0 - 2.0 * y)).sinh().atan().to_degrees();
        let height = ridge_height((latitude - CENTER.1) * METERS_PER_DEGREE);
        let value = ((height + 10000.0) * 10.0).round() as u32;
        *pixel = [(value >> 16) as u8, (value >> 8) as u8, value as u8, 255];
    }
    mln_image::encode_png(&image)
}

/// The GeoJSON polygon of a square `side` meters across, its center `north`
/// and `east` meters from the view's center.
fn square(north: f64, east: f64, side: f64) -> String {
    let half = side / 2.0;
    let corners = [(-half, -half), (-half, half), (half, half), (half, -half), (-half, -half)];
    let ring: Vec<String> = corners
        .iter()
        .map(|&(n, e)| {
            let (lon, lat) = offset(north + n, east + e);
            format!("[{lon}, {lat}]")
        })
        .collect();
    format!(r#"{{"type": "Polygon", "coordinates": [[{}]]}}"#, ring.join(", "))
}

/// The squares the style drapes: their colors and where they are, north
/// and east of the center, in meters.
const SQUARES: [([u8; 3], f64, f64); 3] =
    [([255, 0, 0], 180.0, 0.0), ([0, 255, 0], -120.0, -150.0), ([0, 0, 255], 90.0, 160.0)];

/// A map at zoom 14 pitched 55° over the ridge, with draped squares and a
/// line along the ridge's south slope, on the globe if `globe`.
fn style(globe: bool) -> String {
    let projection = if globe { r#""projection": {"type": "vertical-perspective"},"# } else { "" };
    let (lon, lat) = CENTER;
    let layers: Vec<String> = SQUARES
        .iter()
        .enumerate()
        .map(|(index, &([r, g, b], ..))| {
            format!(
                r#"{{"id": "square{index}", "type": "fill", "source": "square{index}", "paint": {{"fill-color": "rgb({r}, {g}, {b})"}}}}"#
            )
        })
        .collect();
    let sources: Vec<String> = SQUARES
        .iter()
        .enumerate()
        .map(|(index, &(_, north, east))| {
            format!(r#""square{index}": {{"type": "geojson", "data": {}}}"#, square(north, east, 80.0))
        })
        .collect();
    let (west, east) = (offset(60.0, -1000.0), offset(60.0, 1000.0));
    format!(
        r#"{{
            "version": 8,
            "center": [{lon}, {lat}],
            "zoom": 14,
            "pitch": 55,
            {projection}
            "terrain": {{"source": "dem"}},
            "sources": {{
                "dem": {{"type": "raster-dem", "tiles": ["dem/{{z}}/{{x}}/{{y}}.png"], "tileSize": 256, "maxzoom": 15}},
                "line": {{
                    "type": "geojson",
                    "data": {{"type": "LineString", "coordinates": [[{}, {}], [{}, {}]]}}
                }},
                {}
            }},
            "layers": [
                {{"id": "background", "type": "background", "paint": {{"background-color": "white"}}}},
                {},
                {{"id": "line", "type": "line", "source": "line", "paint": {{"line-color": "black", "line-width": 4}}}}
            ]
        }}"#,
        west.0,
        west.1,
        east.0,
        east.1,
        sources.join(",\n"),
        layers.join(",\n"),
    )
}

/// The still image of `style` on the harness's device
/// ([`mln_headless::harness_device`]), over the ridge's DEM tiles; `None`
/// where the machine has no such device.
fn render(style: &str) -> Option<PremultipliedImage> {
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
    file_source.set_response(Kind::Tile, |resource: &Resource| {
        let mut response = Response::new();
        response.data = Some(Arc::new(ridge_png(resource.tile_data.as_ref().expect("a tile"))));
        Some(response)
    });
    let options = MapOptions { map_mode: MapMode::Static, size, pixel_ratio: 1.0, ..Default::default() };
    let mut map = Map::with_file_source(file_source, options, &pool).unwrap();
    frontend.attach(&mut map);
    map.set_bounds(&BoundOptions { max_pitch: Some(85.0), ..Default::default() }).unwrap();
    map.load_style_json(style);
    let camera = map.style().get_default_camera();
    map.jump_to(&camera);
    Some(frontend.render(&mut map).expect("the still image").image)
}

/// The centroid of the pixels of `image` near `rgb`, and how many there are.
fn centroid(image: &PremultipliedImage, rgb: [u8; 3]) -> ((f64, f64), usize) {
    let (mut sum, mut count) = ((0.0, 0.0), 0);
    for (index, pixel) in image.data.as_chunks::<4>().0.iter().enumerate() {
        if (0..3).all(|channel| pixel[channel].abs_diff(rgb[channel]) < 64) {
            let (x, y) = ((index % SIDE as usize) as f64, (index / SIDE as usize) as f64);
            sum = (sum.0 + x, sum.1 + y);
            count += 1;
        }
    }
    ((sum.0 / count as f64, sum.1 / count as f64), count)
}

/// At zoom 14, a view pitched over a 400 m ridge north of a center at sea
/// level drapes its squares and line alike on the globe and on Mercator:
/// each square lands within a pixel, and the pixels that differ visibly are
/// few, along edges.
#[test]
fn a_ridge_at_zoom_14_drapes_alike_on_the_globe_and_mercator() {
    let (Some(globe), Some(mercator)) = (render(&style(true)), render(&style(false))) else {
        return;
    };
    for (rgb, ..) in SQUARES {
        let ((globe_at, globe_count), (mercator_at, mercator_count)) =
            (centroid(&globe, rgb), centroid(&mercator, rgb));
        assert!(mercator_count > 100, "the square of {rgb:?} shows: {mercator_count} px");
        let moved = (globe_at.0 - mercator_at.0).hypot(globe_at.1 - mercator_at.1);
        assert!(
            moved <= 1.0,
            "the square of {rgb:?} is at {globe_at:?} ({globe_count} px) on the globe, {mercator_at:?} ({mercator_count} px) on Mercator"
        );
    }
    let differing = (globe.data.as_chunks::<4>().0.iter().zip(mercator.data.as_chunks::<4>().0))
        .filter(|(a, b)| a.iter().zip(b.iter()).any(|(a, b)| a.abs_diff(*b) > 32))
        .count();
    let allowed = (SIDE * SIDE) as usize / 50;
    assert!(differing <= allowed, "{differing} pixels differ by more than 32");
}
