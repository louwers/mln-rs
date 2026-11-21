// Circles stand on the terrain's ground, where the camera sees their
// centers at the ground's height.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use mln_headless::HeadlessFrontend;
use mln_map::{BoundOptions, CameraOptions, Map, MapMode, MapOptions};
use mln_storage::stub_file_source::{ResponseType, StubFileSource};
use mln_storage::{Kind, Resource, Response};
use mln_util::error::ErrorKind;
use mln_util::geo::LatLng;
use mln_util::image::PremultipliedImage;
use mln_util::pool::Pool;
use mln_util::queue::Queue;
use mln_util::size::Size;

const SIDE: u32 = 256;

/// A DEM tile of 256 pixels a side, each `rgb` in the Mapbox encoding.
fn dem_png(rgb: [u8; 3]) -> Vec<u8> {
    let mut image = PremultipliedImage::new(Size::new(256, 256));
    for pixel in image.data.as_chunks_mut::<4>().0 {
        *pixel = [rgb[0], rgb[1], rgb[2], 255];
    }
    mln_image::encode_png(&image)
}

/// (1 · 65536 + 146 · 256 + 88) · 0.1 − 10000 = 300 m.
const METERS_300: [u8; 3] = [1, 146, 88];
/// (1 · 65536 + 136 · 256 + 148) · 0.1 − 10000 = 50 m.
const METERS_50: [u8; 3] = [1, 136, 148];
/// (1 · 65536 + 134 · 256 + 160) · 0.1 − 10000 = 0 m.
const SEA_LEVEL: [u8; 3] = [1, 134, 160];

/// A white map with a red circle 12 px across at `center`, from a GeoJSON
/// source of the zoom levels up to `geojson_max_zoom`, over a terrain of DEM
/// tiles of 256 px up to `dem_max_zoom`.
fn style(center: LatLng, geojson_max_zoom: u8, dem_max_zoom: u8) -> String {
    let (lng, lat) = (center.longitude(), center.latitude());
    format!(
        r#"{{
            "version": 8,
            "terrain": {{"source": "dem"}},
            "sources": {{
                "dem": {{"type": "raster-dem", "tiles": ["dem/{{z}}/{{x}}/{{y}}.png"], "tileSize": 256, "maxzoom": {dem_max_zoom}}},
                "point": {{
                    "type": "geojson",
                    "maxzoom": {geojson_max_zoom},
                    "data": {{"type": "Point", "coordinates": [{lng}, {lat}]}}
                }}
            }},
            "layers": [
                {{"id": "background", "type": "background", "paint": {{"background-color": "white"}}}},
                {{"id": "circle", "type": "circle", "source": "point", "paint": {{"circle-color": "red", "circle-radius": 6}}}}
            ]
        }}"#
    )
}

/// The still of `style` with the camera at `center`, `zoom` and pitch 60, on
/// the harness's device ([`mln_headless::harness_device`]), whose DEM tiles
/// `dem` answers by their zoom, or leaves for a later poll; with the map
/// that drew it. `None` where the machine has no such device.
fn render(
    style: &str,
    center: LatLng,
    zoom: f64,
    dem: impl Fn(u8) -> Option<Vec<u8>> + Send + Sync + 'static,
) -> Option<(PremultipliedImage, Map)> {
    let size = Size::new(SIDE, SIDE);
    let queue = Queue::new();
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
        HeadlessFrontend::with_device(device, size, 1.0, Arc::default(), &queue, &pool).expect("a frontend");
    let file_source = StubFileSource::on_queue(&queue, ResponseType::Asynchronous);
    file_source.set_response(Kind::Tile, move |resource: &Resource| {
        let tile = resource.tile_data.as_ref().expect("a tile");
        let png = dem(tile.z as u8)?;
        let mut response = Response::new();
        response.data = Some(Arc::new(png));
        Some(response)
    });
    let options = MapOptions { map_mode: MapMode::Static, size, pixel_ratio: 1.0, ..Default::default() };
    let mut map = Map::with_file_source(file_source, options, &pool).unwrap();
    frontend.attach(&mut map);
    map.set_bounds(&BoundOptions { max_pitch: Some(85.0), ..Default::default() }).unwrap();
    map.load_style_json(style);
    map.jump_to(&CameraOptions { center: Some(center), zoom: Some(zoom), pitch: Some(60.0), ..Default::default() });
    let image = frontend.render(&mut map).expect("the still image").image;
    Some((image, map))
}

/// The pixel at `x`, `y` of `image`.
fn pixel(image: &PremultipliedImage, x: u32, y: u32) -> [u8; 4] {
    let start = ((y * image.size.width + x) * 4) as usize;
    image.data[start..start + 4].try_into().unwrap()
}

fn red(pixel: [u8; 4]) -> bool {
    let [r, g, b, _] = pixel;
    r > 200 && g < 50 && b < 50
}

/// The pixel of `image`, which `map` drew, where its camera sees
/// `location` `height` meters above sea level.
fn pixel_at(image: &PremultipliedImage, map: &Map, location: &LatLng, height: f64) -> [u8; 4] {
    let point = map.transform_state().view().lat_lng_to_screen_coordinate_at(location, height);
    let (x, y) = (point.x.round(), f64::from(SIDE) - point.y.round());
    assert!((0.0..f64::from(SIDE)).contains(&x) && (0.0..f64::from(SIDE)).contains(&y), "{location:?} at {height} m");
    pixel(image, x as u32, y as u32)
}

/// Over ground 300 m high, pitched 60°, a circle draws where the camera
/// sees its center 300 m up, and not where it sees it at sea level, some 50
/// px lower.
#[test]
fn a_circle_stands_on_the_ground() {
    let center = LatLng::new(0.0, 0.0).unwrap();
    let Some((image, map)) = render(&style(center, 18, 14), center, 14.0, |_| Some(dem_png(METERS_300))) else {
        return;
    };
    assert!(red(pixel_at(&image, &map, &center, 300.0)), "the circle on the ground");
    assert!(!red(pixel_at(&image, &map, &center, 0.0)), "no circle at sea level");
}

/// A GeoJSON source whose deepest zoom, 14, is coarser than the DEM tiles of
/// zoom 15 that the ground under it is drawn from at zoom 17: its circle
/// stands on the DEM tile of zoom 14 over it, which the still waits for,
/// though it comes later than the others. The coarser DEM tiles the terrain
/// loads, at sea level, would put it there.
#[test]
fn a_circle_of_a_source_tiled_coarser_than_the_dem_waits_for_its_ground() {
    let center = LatLng::new(0.0, 0.0).unwrap();
    let asked = Arc::new(AtomicUsize::new(0));
    let dem = move |z: u8| match z {
        // Ten polls, some 10 ms, after the others.
        14 if asked.fetch_add(1, Ordering::Relaxed) < 10 => None,
        14.. => Some(dem_png(METERS_50)),
        _ => Some(dem_png(SEA_LEVEL)),
    };
    let Some((image, map)) = render(&style(center, 14, 15), center, 17.0, dem) else {
        return;
    };
    assert!(red(pixel_at(&image, &map, &center, 50.0)), "the circle on the ground");
    assert!(!red(pixel_at(&image, &map, &center, 0.0)), "no circle at sea level");
}
