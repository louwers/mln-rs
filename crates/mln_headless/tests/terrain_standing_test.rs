// Circles stand on the terrain's surface as it draws, where the camera sees
// their centers at the surface's height.

use std::collections::BTreeSet;
use std::sync::Arc;

use mln_headless::HeadlessFrontend;
use mln_map::{BoundOptions, CameraOptions, Map, MapMode, MapOptions};
use mln_storage::resource::TileData;
use mln_storage::stub_file_source::{ResponseType, StubFileSource};
use mln_storage::{Kind, Resource, Response};
use mln_util::error::ErrorKind;
use mln_util::geo::LatLng;
use mln_util::image::PremultipliedImage;
use mln_util::pool::Pool;
use mln_util::queue::Queue;
use mln_util::size::Size;
use parking_lot::Mutex;

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
/// (1 · 65536 + 137 · 256 + 192) · 0.1 − 10000 = 80 m.
const METERS_80: [u8; 3] = [1, 137, 192];
/// (1 · 65536 + 136 · 256 + 148) · 0.1 − 10000 = 50 m.
const METERS_50: [u8; 3] = [1, 136, 148];
/// (1 · 65536 + 134 · 256 + 160) · 0.1 − 10000 = 0 m.
const SEA_LEVEL: [u8; 3] = [1, 134, 160];

/// The map of a test: a white background and a red circle 12 px across at
/// a point, half opaque if `translucent`, from a GeoJSON source of the zoom
/// levels up to `geojson_max_zoom`, of the circle layer's zoom levels
/// `circle_zooms`, over a terrain of DEM tiles of 256 px of the zoom levels
/// `dem_zooms`, seen at a pitch of `pitch` degrees. With an `extrusion`
/// height, the source is a square 0.0002° from the point to each side, and
/// the layer a red fill extrusion of that height in meters.
struct Scene {
    center: LatLng,
    pitch: f64,
    geojson_max_zoom: u8,
    circle_zooms: (u8, u8),
    dem_zooms: (u8, u8),
    translucent: bool,
    extrusion: Option<f64>,
}

impl Scene {
    /// A red circle at `center` from a GeoJSON source of the zoom levels up
    /// to `geojson_max_zoom`, over DEM tiles of the zoom levels up to
    /// `dem_max_zoom`.
    fn new(center: LatLng, geojson_max_zoom: u8, dem_max_zoom: u8) -> Scene {
        Scene {
            center,
            pitch: 60.0,
            geojson_max_zoom,
            circle_zooms: (0, 24),
            dem_zooms: (0, dem_max_zoom),
            translucent: false,
            extrusion: None,
        }
    }

    fn style(&self) -> String {
        let (lng, lat) = (self.center.longitude(), self.center.latitude());
        let Scene { geojson_max_zoom, circle_zooms: (circle_min, circle_max), dem_zooms: (dem_min, dem_max), .. } =
            *self;
        let opacity = if self.translucent { 0.5 } else { 1.0 };
        let (data, layer) = match self.extrusion {
            None => (
                format!(r#"{{"type": "Point", "coordinates": [{lng}, {lat}]}}"#),
                format!(
                    r#"{{
                        "id": "circle",
                        "type": "circle",
                        "source": "point",
                        "minzoom": {circle_min},
                        "maxzoom": {circle_max},
                        "paint": {{"circle-color": "red", "circle-radius": 6, "circle-opacity": {opacity}}}
                    }}"#
                ),
            ),
            Some(height) => {
                let (west, east, south, north) = (lng - 0.0002, lng + 0.0002, lat - 0.0002, lat + 0.0002);
                (
                    format!(
                        r#"{{"type": "Polygon", "coordinates": [[
                            [{west}, {south}], [{east}, {south}], [{east}, {north}], [{west}, {north}], [{west}, {south}]
                        ]]}}"#
                    ),
                    format!(
                        r#"{{
                            "id": "extrusion",
                            "type": "fill-extrusion",
                            "source": "point",
                            "paint": {{"fill-extrusion-color": "red", "fill-extrusion-height": {height}}}
                        }}"#
                    ),
                )
            }
        };
        format!(
            r#"{{
                "version": 8,
                "terrain": {{"source": "dem"}},
                "sources": {{
                    "dem": {{
                        "type": "raster-dem",
                        "tiles": ["dem/{{z}}/{{x}}/{{y}}.png"],
                        "tileSize": 256,
                        "minzoom": {dem_min},
                        "maxzoom": {dem_max}
                    }},
                    "point": {{
                        "type": "geojson",
                        "maxzoom": {geojson_max_zoom},
                        "data": {data}
                    }}
                }},
                "layers": [
                    {{"id": "background", "type": "background", "paint": {{"background-color": "white"}}}},
                    {layer}
                ]
            }}"#
        )
    }
}

/// The DEM tiles a map asked for.
type Requests = Arc<Mutex<BTreeSet<(i8, i32, i32)>>>;

/// The still of `scene` with the camera at its center and its pitch, at each
/// of `zooms` in turn, on the harness's device
/// ([`mln_headless::harness_device`]), whose DEM tiles `dem` answers: the
/// last one, with the map that drew it and the DEM tiles it asked for.
/// `None` where the machine has no such device.
fn render(
    scene: &Scene,
    zooms: &[f64],
    dem: impl Fn(&TileData) -> Vec<u8> + Send + Sync + 'static,
) -> Option<(PremultipliedImage, Map, Requests)> {
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
    let requests = Requests::default();
    let asked = Arc::clone(&requests);
    file_source.set_response(Kind::Tile, move |resource: &Resource| {
        let tile = resource.tile_data.as_ref().expect("a tile");
        asked.lock().insert((tile.z, tile.x, tile.y));
        let mut response = Response::new();
        response.data = Some(Arc::new(dem(tile)));
        Some(response)
    });
    let options = MapOptions { map_mode: MapMode::Static, size, pixel_ratio: 1.0, ..Default::default() };
    let mut map = Map::with_file_source(file_source, options, &pool).unwrap();
    frontend.attach(&mut map);
    map.set_bounds(&BoundOptions { max_pitch: Some(85.0), ..Default::default() }).unwrap();
    map.load_style_json(&scene.style());
    let mut image = None;
    for &zoom in zooms {
        let camera = CameraOptions {
            center: Some(scene.center),
            zoom: Some(zoom),
            pitch: Some(scene.pitch),
            ..Default::default()
        };
        map.jump_to(&camera);
        image = Some(frontend.render(&mut map).expect("the still image").image);
    }
    Some((image.expect("a zoom"), map, requests))
}

/// The pixel at `x`, `y` of `image`.
fn pixel(image: &PremultipliedImage, x: u32, y: u32) -> [u8; 4] {
    let start = ((y * image.size.width + x) * 4) as usize;
    image.data[start..start + 4].try_into().unwrap()
}

/// Whether `pixel` is of the circle: red, opaque or half so, over white.
fn red(pixel: [u8; 4]) -> bool {
    let [r, g, b, _] = pixel;
    r > 200 && g < 200 && b < 200 && g.abs_diff(b) < 16
}

/// Where in `image`, which `map` drew, its camera sees `location` `height`
/// meters above sea level, in pixels from the top left.
fn seen_at(map: &Map, location: &LatLng, height: f64) -> (f64, f64) {
    let point = map.transform_state().view().lat_lng_to_screen_coordinate_at(location, height);
    (point.x, f64::from(SIDE) - point.y)
}

/// The pixel of `image`, which `map` drew, where its camera sees `location`
/// `height` meters above sea level.
fn pixel_at(image: &PremultipliedImage, map: &Map, location: &LatLng, height: f64) -> [u8; 4] {
    let (x, y) = seen_at(map, location, height);
    let (x, y) = (x.round(), y.round());
    assert!((0.0..f64::from(SIDE)).contains(&x) && (0.0..f64::from(SIDE)).contains(&y), "{location:?} at {height} m");
    pixel(image, x as u32, y as u32)
}

/// The center of the red pixels of `image`, and how many there are.
fn red_center(image: &PremultipliedImage) -> ((f64, f64), usize) {
    let (mut sum, mut count) = ((0.0, 0.0), 0);
    for y in 0..image.size.height {
        for x in 0..image.size.width {
            if red(pixel(image, x, y)) {
                sum = (sum.0 + f64::from(x) + 0.5, sum.1 + f64::from(y) + 0.5);
                count += 1;
            }
        }
    }
    assert!(count > 0, "no circle");
    ((sum.0 / count as f64, sum.1 / count as f64), count)
}

/// That the circle of `image`, which `map` drew, is one circle whose center
/// is within 2 px of where the camera sees `location` `height` meters above
/// sea level.
fn assert_circle_at(image: &PremultipliedImage, map: &Map, location: &LatLng, height: f64) {
    let ((x, y), count) = red_center(image);
    let (seen_x, seen_y) = seen_at(map, location, height);
    let off = (x - seen_x).hypot(y - seen_y);
    assert!(off <= 2.0, "the circle at {x}, {y}, {off} px off where its center is seen at {height} m");
    // A disc 12 px across, with its edge antialiased.
    assert!((60..=180).contains(&count), "{count} red pixels: not one circle");
}

/// The location at `x`, `y` of the world at zoom `z`, in tiles from its top
/// left.
fn location_at(z: u8, x: f64, y: f64) -> LatLng {
    let tiles = f64::from(1u32 << z);
    let longitude = x / tiles * 360.0 - 180.0;
    let latitude = (std::f64::consts::PI * (1.0 - 2.0 * y / tiles)).sinh().atan().to_degrees();
    LatLng::new(latitude, longitude).unwrap()
}

/// Over ground 300 m high, pitched 60°, a circle draws where the camera
/// sees its center 300 m up, and not where it sees it at sea level, some 50
/// px lower.
#[test]
fn a_circle_stands_on_the_ground() {
    let center = LatLng::new(0.0, 0.0).unwrap();
    let Some((image, map, _)) = render(&Scene::new(center, 18, 14), &[14.0], |_| dem_png(METERS_300)) else {
        return;
    };
    assert!(red(pixel_at(&image, &map, &center, 300.0)), "the circle on the ground");
    assert!(!red(pixel_at(&image, &map, &center, 0.0)), "no circle at sea level");
    assert_circle_at(&image, &map, &center, 300.0);
}

/// The DEM tiles of a ridge's zoom 15 at 300 m, over DEM tiles of zoom 14
/// that average it down to 50 m, and coarser ones at sea level.
fn ridge(tile: &TileData) -> Vec<u8> {
    dem_png(match tile.z {
        15.. => METERS_300,
        14 => METERS_50,
        _ => SEA_LEVEL,
    })
}

/// A GeoJSON source whose deepest zoom, 14, is coarser than the DEM tiles of
/// zoom 15 that the surface under it reads at zoom 17: its circle stands on
/// the surface, 300 m up, where the DEM tile of zoom 14 over it would put it
/// at 50 m, off the screen. So it does whatever the map loaded before: here
/// a view at zoom 14, which reads the DEM tile of zoom 14 and its
/// neighbours.
#[test]
fn a_circle_of_a_source_tiled_coarser_than_the_dem_stands_on_the_surface() {
    // Just inside the north-east of the four source tiles that meet at 0, 0:
    // the south ones, under the camera, are culled by the range of the DEM
    // tile of zoom 14, 50 m, below the surface at 300 m.
    let center = LatLng::new(0.0001, 0.0001).unwrap();
    for zooms in [&[17.0][..], &[14.0, 17.0]] {
        let Some((image, map, _)) = render(&Scene::new(center, 14, 15), zooms, ridge) else {
            return;
        };
        assert_circle_at(&image, &map, &center, 300.0);
        let (ground, coarse) = (seen_at(&map, &center, 300.0), seen_at(&map, &center, 50.0));
        assert!((ground.0 - coarse.0).hypot(ground.1 - coarse.1) > 20.0, "{zooms:?}: the zooms' DEMs differ on screen");
    }
}

/// A circle half opaque, 3 DEM pixels east of the edge between two DEM
/// tiles of zoom 15 in the middle of its source tile of zoom 14, the west
/// one 80 m high and the east one 50 m: it draws once, as half opaque as it
/// is, on the east one.
#[test]
fn a_circle_by_an_edge_between_dem_tiles_draws_once() {
    // The DEM tiles of zoom 15 meet at x 16385, inside the tile of zoom 14
    // at x 8192.
    let center = location_at(15, 16385.0 + 3.0 / 256.0, 16385.5);
    let scene = Scene { translucent: true, ..Scene::new(center, 14, 15) };
    let dem = |tile: &TileData| match tile.z {
        15.. if tile.x % 2 == 0 => dem_png(METERS_80),
        15.. => dem_png(METERS_50),
        _ => dem_png(SEA_LEVEL),
    };
    let Some((image, map, _)) = render(&scene, &[17.0], dem) else {
        return;
    };
    assert_circle_at(&image, &map, &center, 50.0);
    let [r, g, b, _] = pixel_at(&image, &map, &center, 50.0);
    // Half of red over white is (255, 128, 128); twice over, (255, 64, 64).
    assert!(r > 240 && (100..160).contains(&g) && (100..160).contains(&b), "drawn once: {:?}", [r, g, b]);
}

/// A circle half opaque, 3 DEM pixels east of the edge between two source
/// tiles of zoom 14, both drawn, over level DEM tiles of zoom 15, in a
/// still: the west tile's bucket holds it too, in its buffer, but it draws
/// once, as half opaque as it is.
#[test]
fn a_circle_by_an_edge_between_source_tiles_draws_once() {
    // The source tiles of zoom 14 meet at x 8192, where the DEM tiles of
    // zoom 15 meet at x 16384.
    let center = location_at(15, 16384.0 + 3.0 / 256.0, 16384.5);
    let scene = Scene { translucent: true, ..Scene::new(center, 14, 15) };
    let Some((image, map, _)) = render(&scene, &[17.0], |_| dem_png(METERS_50)) else {
        return;
    };
    assert_circle_at(&image, &map, &center, 50.0);
    let [r, g, b, _] = pixel_at(&image, &map, &center, 50.0);
    // Half of red over white is (255, 128, 128); twice over, (255, 64, 64).
    assert!(r > 240 && (100..160).contains(&g) && (100..160).contains(&b), "drawn once: {:?}", [r, g, b]);
}

/// Over a DEM whose lowest zoom, 15, is deeper than the circle's source
/// tile of zoom 14, the circle stands on the surface's DEM tile of zoom 15,
/// not at sea level.
#[test]
fn a_circle_over_a_dem_that_starts_deeper_than_its_tile_stands_on_the_surface() {
    let center = LatLng::new(0.0, 0.0).unwrap();
    let scene = Scene { dem_zooms: (15, 15), ..Scene::new(center, 14, 15) };
    let Some((image, map, _)) = render(&scene, &[17.0], |_| dem_png(METERS_300)) else {
        return;
    };
    assert_circle_at(&image, &map, &center, 300.0);
}

/// A circle layer out of its zoom levels, of a source tiled coarser than the
/// DEM, asks for no DEM tile of its own: the still waits for the surface's
/// DEM tiles alone, which, looking straight down at zoom 17, are of zoom 15
/// and the coarse ones that cull the surface.
#[test]
fn a_circle_layer_out_of_its_zooms_asks_for_no_dem_tile() {
    let center = LatLng::new(0.0, 0.0).unwrap();
    let scene = Scene { pitch: 0.0, circle_zooms: (0, 10), ..Scene::new(center, 14, 15) };
    let Some((image, _, requests)) = render(&scene, &[17.0], ridge) else {
        return;
    };
    let zooms: BTreeSet<i8> = requests.lock().iter().map(|&(z, ..)| z).collect();
    assert!(!zooms.contains(&14), "DEM tiles of zoom 14 asked for: {zooms:?}");
    assert!((0..SIDE).all(|y| (0..SIDE).all(|x| !red(pixel(&image, x, y)))), "no circle");
}

/// An extrusion of a source whose deepest zoom, 14, is coarser than the DEM
/// tiles of zoom 15 that the surface under it reads at zoom 17 stands on the
/// surface, 300 m up, though its tile of zoom 14 draws in a part for each of
/// the DEM tiles of zoom 15 under it: its roof is where the camera sees it
/// 300 m and its height up, and not where the DEM tile of zoom 14 would put
/// it, at 50 m, off the screen.
#[test]
fn an_extrusion_of_a_source_tiled_coarser_than_the_dem_stands_on_the_surface() {
    // In the south-east DEM tile of zoom 15 of its tile of zoom 14.
    let center = location_at(15, 16385.5, 16385.5);
    let scene = Scene { extrusion: Some(20.0), ..Scene::new(center, 14, 15) };
    let Some((image, map, _)) = render(&scene, &[17.0], ridge) else {
        return;
    };
    assert!(red(pixel_at(&image, &map, &center, 320.0)), "the roof on the ground");
    let (ground, coarse) = (seen_at(&map, &center, 320.0), seen_at(&map, &center, 70.0));
    assert!((ground.0 - coarse.0).hypot(ground.1 - coarse.1) > 20.0, "the zooms' DEMs differ on screen");
}
