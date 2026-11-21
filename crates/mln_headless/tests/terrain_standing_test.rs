// Circles, fill extrusions and labels stand on the terrain's surface as it
// draws, where the camera sees their anchors at the surface's height.

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
/// (1 · 65536 + 154 · 256 + 40) · 0.1 − 10000 = 500 m.
const METERS_500: [u8; 3] = [1, 154, 40];
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
    let camera = |&zoom: &f64| CameraOptions {
        center: Some(scene.center),
        zoom: Some(zoom),
        pitch: Some(scene.pitch),
        ..Default::default()
    };
    render_views(&scene.style(), &zooms.iter().map(camera).collect::<Vec<_>>(), dem)
}

/// The still of the map of `style` seen by each of `cameras` in turn, as
/// [`render`] draws it: the last one, with the map that drew it and the DEM
/// tiles it asked for.
fn render_views(
    style: &str,
    cameras: &[CameraOptions],
    dem: impl Fn(&TileData) -> Vec<u8> + Send + Sync + 'static,
) -> Option<(PremultipliedImage, Map, Requests)> {
    render_with_frontend(style, cameras, dem).map(|(image, map, requests, _)| (image, map, requests))
}

/// [`render_views`], with the frontend that drew the still, which queries
/// it. The glyphs of every font are those of the test fixtures.
fn render_with_frontend(
    style: &str,
    cameras: &[CameraOptions],
    dem: impl Fn(&TileData) -> Vec<u8> + Send + Sync + 'static,
) -> Option<(PremultipliedImage, Map, Requests, HeadlessFrontend)> {
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
    file_source
        .set_response(Kind::Glyphs, |_| Some(crate::support::file_response("test/fixtures/resources/glyphs.pbf")));
    let options = MapOptions { map_mode: MapMode::Static, size, pixel_ratio: 1.0, ..Default::default() };
    let mut map = Map::with_file_source(file_source, options, &pool).unwrap();
    frontend.attach(&mut map);
    map.set_bounds(&BoundOptions { max_pitch: Some(85.0), ..Default::default() }).unwrap();
    map.load_style_json(style);
    let mut image = None;
    for camera in cameras {
        map.jump_to(camera);
        image = Some(frontend.render(&mut map).expect("the still image").image);
    }
    Some((image.expect("a camera"), map, requests, frontend))
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
/// the DEM tile of zoom 14 over it, which agrees with the surface's on a
/// plane slope, within 2 px of where the camera sees the surface, in a still,
/// whether the map drew a view at zoom 14 before or not.
#[test]
fn a_circle_of_a_source_tiled_coarser_than_the_dem_stands_on_the_surface() {
    let center = location_at(15, 16385.5, 16385.5);
    let x = 16385.5 * 256.0;
    for zooms in [&[17.0][..], &[14.0, 17.0]] {
        let Some((image, map, _)) = render(&Scene::new(center, 14, 15), zooms, move |tile| slope_png(tile, x, 2.0))
        else {
            return;
        };
        assert_circle_at(&image, &map, &center, 500.0);
    }
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

/// Two circles, blue in a source tile of zoom 14 and red in its east
/// neighbour, overlap across the edge between the tiles. Their overlap is
/// the same whether the view is drawn fresh or after a view north of them,
/// which drew the tiles in another order: the frame's order of the tiles
/// stacks them, not when their draws were made.
#[test]
fn circles_by_the_corner_of_source_tiles_stack_alike_however_the_view_is_reached() {
    // The source tiles of zoom 14 meet at x 8192, and the DEM tiles of zoom
    // 15 in them at y 16385. A pixel at zoom 17 is 1 / 2048 of a DEM tile.
    let corner = (16384.0, 16385.0);
    let off = 2.0 / 2048.0;
    let blue = location_at(15, corner.0 - off, corner.1 + off);
    let red = location_at(15, corner.0 + off, corner.1 - off);
    let point = |location: &LatLng, color: &str| {
        let (lng, lat) = (location.longitude(), location.latitude());
        format!(
            r#"{{"type": "Feature", "properties": {{"color": "{color}"}},
                "geometry": {{"type": "Point", "coordinates": [{lng}, {lat}]}}}}"#
        )
    };
    let (blue_point, red_point) = (point(&blue, "blue"), point(&red, "red"));
    let style = format!(
        r#"{{
            "version": 8,
            "terrain": {{"source": "dem"}},
            "sources": {{
                "dem": {{"type": "raster-dem", "tiles": ["dem/{{z}}/{{x}}/{{y}}.png"], "tileSize": 256, "maxzoom": 15}},
                "points": {{
                    "type": "geojson",
                    "maxzoom": 14,
                    "buffer": 0,
                    "data": {{"type": "FeatureCollection", "features": [{blue_point}, {red_point}]}}
                }}
            }},
            "layers": [
                {{"id": "background", "type": "background", "paint": {{"background-color": "white"}}}},
                {{
                    "id": "circles",
                    "type": "circle",
                    "source": "points",
                    "paint": {{"circle-color": ["get", "color"], "circle-radius": 6}}
                }}
            ]
        }}"#
    );
    let at_corner = location_at(15, corner.0, corner.1);
    let view = CameraOptions { center: Some(at_corner), zoom: Some(17.0), pitch: Some(60.0), ..Default::default() };
    let north = CameraOptions { center: Some(location_at(15, corner.0, corner.1 - 0.2)), ..view };
    let mut overlaps = Vec::new();
    for cameras in [&[view][..], &[north, view]] {
        let Some((image, map, _)) = render_views(&style, cameras, |_| dem_png(METERS_50)) else {
            return;
        };
        let [r, g, b, _] = pixel_at(&image, &map, &at_corner, 50.0);
        assert!(
            (r > 200 && g < 60 && b < 60) || (r < 60 && g < 60 && b > 200),
            "{} views: a circle at the overlap, not {:?}",
            cameras.len(),
            [r, g, b]
        );
        overlaps.push([r, g, b]);
    }
    assert_eq!(overlaps[0], overlaps[1], "the overlap fresh, then after the north halves");
}

/// That the roof of the extrusion of `image`, which `map` drew, its red,
/// lit pixels, is centered within 2 px of where the camera sees `location`
/// `height` meters above sea level. Its walls are darker.
fn assert_roof_at(image: &PremultipliedImage, map: &Map, location: &LatLng, height: f64) {
    let ((x, y), count) = red_center(image);
    let (seen_x, seen_y) = seen_at(map, location, height);
    let off = (x - seen_x).hypot(y - seen_y);
    assert!(off <= 2.0, "the roof at {x}, {y}, {off} px off where it is seen at {height} m");
    assert!(count > 100, "{count} red pixels");
}

/// The still of a red fill extrusion 400 m tall at `center`, from a GeoJSON
/// source of the zoom levels up to 14, over DEM tiles of `dem_zooms` that
/// `dem` answers, seen from straight above a point 0.0006° west of it at
/// zoom 16, after a view at each of `before`: where the surface's DEM tiles
/// are of zoom 15, finer than the source's tile. `None` where the machine
/// has no device.
fn tall_extrusion(
    center: LatLng,
    dem_zooms: (u8, u8),
    before: &[f64],
    dem: impl Fn(&TileData) -> Vec<u8> + Send + Sync + 'static,
) -> Option<(PremultipliedImage, Map, Requests)> {
    let scene = Scene { dem_zooms, extrusion: Some(400.0), ..Scene::new(center, 14, 15) };
    let west = LatLng::new(center.latitude(), center.longitude() - 0.0006).unwrap();
    let camera =
        |zoom: f64| CameraOptions { center: Some(west), zoom: Some(zoom), pitch: Some(0.0), ..Default::default() };
    let cameras: Vec<CameraOptions> = before.iter().copied().chain([16.0]).map(camera).collect();
    render_views(&scene.style(), &cameras, dem)
}

/// Over a DEM whose lowest zoom, 15, is deeper than an extrusion's source
/// tile of zoom 14, no DEM tile holds the source tile, which stands at sea
/// level: its roof, 400 m up, rises above the surface at 300 m, where the
/// camera sees it 400 m up and not as it would on the surface, 700 m up.
#[test]
fn an_extrusion_over_a_dem_that_starts_deeper_than_its_tile_stands_at_sea_level() {
    let center = location_at(15, 16385.5, 16385.5);
    let Some((image, map, _)) = tall_extrusion(center, (15, 15), &[], |_| dem_png(METERS_300)) else {
        return;
    };
    assert_roof_at(&image, &map, &center, 400.0);
    let (sea, ground) = (seen_at(&map, &center, 400.0), seen_at(&map, &center, 700.0));
    assert!((sea.0 - ground.0).hypot(sea.1 - ground.1) > 20.0, "the two heights differ on screen");
}

/// A circle layer out of its zoom levels, of a source tiled coarser than the
/// DEM, which a fill layer draws too, asks for no DEM tile of its own: the
/// still waits for the surface's DEM tiles alone, which, looking straight
/// down at zoom 17, are of zoom 15 and the coarse ones that cull the
/// surface. In its zoom levels, it asks for the DEM tile of its source tile,
/// of zoom 14.
#[test]
fn a_circle_layer_out_of_its_zooms_asks_for_no_dem_tile() {
    let center = LatLng::new(0.0001, 0.0001).unwrap();
    let fill = r#"{"id": "fill", "type": "fill", "source": "point"},"#;
    for (circle_zooms, asks) in [((0, 10), false), ((0, 24), true)] {
        let scene = Scene { pitch: 0.0, circle_zooms, ..Scene::new(center, 14, 15) };
        let style = scene.style().replacen(r#"{"id": "background""#, &format!(r#"{fill}{{"id": "background""#), 1);
        let camera = CameraOptions { center: Some(center), zoom: Some(17.0), pitch: Some(0.0), ..Default::default() };
        let Some((image, _, requests)) = render_views(&style, &[camera], ridge) else {
            return;
        };
        let zooms: BTreeSet<i8> = requests.lock().iter().map(|&(z, ..)| z).collect();
        assert_eq!(asks, zooms.contains(&14), "layer zooms {circle_zooms:?}: DEM tiles of zooms {zooms:?}");
        if !asks {
            assert!((0..SIDE).all(|y| (0..SIDE).all(|x| !red(pixel(&image, x, y)))), "no circle");
        }
    }
}

/// A symbol layer that reads no ground, of a source tiled coarser than the
/// DEM, asks for no DEM tile of its own: one drawn over the view
/// (`symbol-screen-space`), and one of point labels measured from sea level
/// (`symbol-height-anchor: absolute`), here 400 m up, over ground 300 m high.
/// Looking straight down at zoom 17, the still waits for the surface's DEM
/// tiles alone, of zoom 15 and the coarse ones that cull the surface. Labels
/// measured from the ground ask for the DEM tile of their source tile, of
/// zoom 14. Each draws.
#[test]
fn a_symbol_layer_that_reads_no_ground_asks_for_no_dem_tile() {
    let center = LatLng::new(0.0001, 0.0001).unwrap();
    for (layout, asks) in [
        (r#""symbol-screen-space": true"#, false),
        (r#""symbol-height-anchor": "absolute", "symbol-height-offset": 400"#, false),
        (r#""symbol-height-anchor": "ground""#, true),
    ] {
        let style = format!(
            r#"{{
                "version": 8,
                "glyphs": "glyphs/{{fontstack}}/{{range}}.pbf",
                "terrain": {{"source": "dem"}},
                "sources": {{
                    "dem": {{"type": "raster-dem", "tiles": ["dem/{{z}}/{{x}}/{{y}}.png"], "tileSize": 256, "maxzoom": 15}},
                    "point": {{
                        "type": "geojson",
                        "maxzoom": 14,
                        "data": {{"type": "Point", "coordinates": [0.0001, 0.0001]}}
                    }}
                }},
                "layers": [
                    {{"id": "background", "type": "background", "paint": {{"background-color": "white"}}}},
                    {{
                        "id": "label",
                        "type": "symbol",
                        "source": "point",
                        "layout": {{"text-field": "HH", "text-font": ["Open Sans Regular"], {layout}}},
                        "paint": {{"text-color": "red"}}
                    }}
                ]
            }}"#
        );
        let camera = CameraOptions { center: Some(center), zoom: Some(17.0), pitch: Some(0.0), ..Default::default() };
        let Some((image, _, requests)) = render_views(&style, &[camera], |_| dem_png(METERS_300)) else {
            return;
        };
        let zooms: BTreeSet<i8> = requests.lock().iter().map(|&(z, ..)| z).collect();
        assert!(zooms.contains(&15), "{layout}: the surface's DEM tiles, of zooms {zooms:?}");
        assert_eq!(asks, zooms.contains(&14), "{layout}: DEM tiles of zooms {zooms:?}");
        let (_, count) = red_center(&image);
        assert!(count > 20, "{layout}: {count} red pixels: no label");
    }
}

/// The known limit: an extrusion of a source whose deepest zoom, 14, is
/// coarser than the DEM tiles of zoom 15 that the surface under it reads
/// stands on the DEM tile of zoom 14 over it, which the ridge's DEM tiles
/// disagree with: at 50 m, with its roof 400 m up at 450 m, and not on the
/// surface at 300 m, with its roof at 700 m. It does so in a still whether
/// the map drew a view at zoom 14 before or not.
#[test]
fn an_extrusion_of_a_source_tiled_coarser_than_the_dem_stands_on_its_own_dem_tile() {
    // In the south-east DEM tile of zoom 15 of its tile of zoom 14.
    let center = location_at(15, 16385.5, 16385.5);
    for before in [&[][..], &[14.0]] {
        let Some((image, map, _)) = tall_extrusion(center, (0, 15), before, ridge) else {
            return;
        };
        assert_roof_at(&image, &map, &center, 450.0);
        let (own, surface) = (seen_at(&map, &center, 450.0), seen_at(&map, &center, 700.0));
        assert!((own.0 - surface.0).hypot(own.1 - surface.1) > 20.0, "the two heights differ on screen");
    }
}

/// A DEM tile of 256 pixels a side over a slope down to the east, which
/// falls `drop` meters a pixel of zoom 15 from 500 m at `x`, in pixels of
/// zoom 15 from the world's west, and lies between 0 and 1000 m.
fn slope_png(tile: &TileData, x: f64, drop: f64) -> Vec<u8> {
    let scale = 2f64.powi(15 - i32::from(tile.z));
    let mut image = PremultipliedImage::new(Size::new(256, 256));
    for (index, pixel) in image.data.as_chunks_mut::<4>().0.iter_mut().enumerate() {
        let column = f64::from(tile.x * 256) + (index % 256) as f64 + 0.5;
        let height = (500.0 - drop * (column * scale - x)).clamp(0.0, 1000.0);
        let value = ((height + 10000.0) * 10.0).round() as u32;
        *pixel = [(value >> 16) as u8, (value >> 8) as u8, value as u8, 255];
    }
    mln_image::encode_png(&image)
}

/// Whether `pixel` is of the extrusion: a lit or shaded red, not the white
/// ground.
fn extrusion(pixel: [u8; 4]) -> bool {
    let [r, g, b, _] = pixel;
    r > 40 && g < r / 2 && b < r / 2
}

/// A building 150 m tall on the ground, on a slope down to the east that
/// falls 5 m a DEM pixel: the ground under its east wall is 23 m below the
/// ground under its centroid, 500 m. Seen from the east, the wall reaches
/// down to the ground, with no gap above it that the 10 m that a wall's base
/// sinks would leave, and the wall shades as the same building's does on
/// level ground at 500 m, where the two show.
#[test]
fn a_wall_downhill_reaches_the_ground() {
    let center = location_at(15, 16384.5, 16384.5);
    let x = 16384.5 * 256.0;
    let scene = Scene { extrusion: Some(150.0), ..Scene::new(center, 18, 15) };
    let camera = CameraOptions {
        center: Some(center),
        zoom: Some(16.0),
        pitch: Some(60.0),
        bearing: Some(270.0),
        ..Default::default()
    };
    let Some((image, map, _)) = render_views(&scene.style(), &[camera], move |tile| slope_png(tile, x, 5.0)) else {
        return;
    };
    let east = LatLng::new(center.latitude(), center.longitude() + 0.0002).unwrap();
    // The wall's end is 0.0002° east of the centroid, 4.66 pixels of zoom 15.
    let ground = 500.0 - 5.0 * 0.0002 / 360.0 * 2f64.powi(15) * 256.0;
    let base = 500.0 - 10.0;
    assert!(base - ground > 10.0, "a gap of {} m under the base", base - ground);
    for height in (ground.ceil() as i32 + 1..base as i32).map(f64::from) {
        let pixel = pixel_at(&image, &map, &east, height);
        assert!(extrusion(pixel), "the wall {height} m up, {:.1} m above the ground: {pixel:?}", height - ground);
    }

    let Some((level, level_map, _)) = render_views(&scene.style(), &[camera], |_| dem_png(METERS_500)) else {
        return;
    };
    // Above the level ground, the wall's gradient runs from its base to its
    // top, as it does on the slope.
    for height in [505.0, 520.0, 560.0, 600.0, 640.0] {
        let (pixel, level) = (pixel_at(&image, &map, &east, height), pixel_at(&level, &level_map, &east, height));
        assert!(extrusion(pixel), "the wall {height} m up: {pixel:?}");
        assert!(
            pixel.iter().zip(level).all(|(&a, b)| a.abs_diff(b) <= 1),
            "the wall {height} m up on the slope, {pixel:?}, and on level ground, {level:?}"
        );
    }
}

/// Over ground 300 m high, pitched 60°, a red label whose text takes a
/// variable anchor draws where the camera sees its anchor 300 m up, shifted
/// below it, and collides there: a query at the middle of what it draws finds
/// it, and one where it would draw at sea level finds nothing.
#[test]
fn a_label_at_a_variable_anchor_collides_where_it_stands_on_the_ground() {
    use mln_renderer::query::RenderedQueryOptions;
    use mln_util::geo::ScreenCoordinate;

    let center = LatLng::new(0.0, 0.0).unwrap();
    let style = r#"{
        "version": 8,
        "glyphs": "glyphs/{fontstack}/{range}.pbf",
        "terrain": {"source": "dem"},
        "sources": {
            "dem": {"type": "raster-dem", "tiles": ["dem/{z}/{x}/{y}.png"], "tileSize": 256, "maxzoom": 14},
            "point": {"type": "geojson", "data": {"type": "Point", "coordinates": [0, 0]}}
        },
        "layers": [
            {"id": "background", "type": "background", "paint": {"background-color": "white"}},
            {
                "id": "label",
                "type": "symbol",
                "source": "point",
                "layout": {
                    "text-field": "HH",
                    "text-font": ["Open Sans Regular"],
                    "text-size": 24,
                    "text-variable-anchor": ["top"],
                    "text-radial-offset": 1
                },
                "paint": {"text-color": "red"}
            }
        ]
    }"#;
    let camera = CameraOptions { center: Some(center), zoom: Some(14.0), pitch: Some(60.0), ..Default::default() };
    let Some((image, map, _, mut frontend)) = render_with_frontend(style, &[camera], |_| dem_png(METERS_300)) else {
        return;
    };
    let ((x, y), count) = red_center(&image);
    assert!(count > 50, "{count} red pixels: no label");
    let (anchor_x, anchor_y) = seen_at(&map, &center, 300.0);
    assert!(
        (x - anchor_x).abs() < 2.0 && y > anchor_y + 10.0,
        "the label at {x}, {y}, below its anchor at {anchor_x}, {anchor_y}"
    );

    let mut query = |x: f64, y: f64| {
        let options = RenderedQueryOptions::new(Some(vec!["label".to_owned()]), None);
        frontend.renderer().query_rendered_features_at_point(&ScreenCoordinate::new(x, y), &options).unwrap().len()
    };
    assert_eq!(1, query(x, y), "the label where it draws");
    let (sea_x, sea_y) = seen_at(&map, &center, 0.0);
    let (lower_x, lower_y) = (x + sea_x - anchor_x, y + sea_y - anchor_y);
    assert!(lower_y - y > 30.0, "the label at sea level {} px lower", lower_y - y);
    assert_eq!(0, query(lower_x, lower_y), "nothing where the label would draw at sea level");
}

/// Over terrain, with the collision debug option on, a map with a label
/// draws its frame, collision boxes and circles included.
#[test]
fn collision_boxes_draw_over_terrain() {
    use mln_map::MapDebugOptions;

    let center = LatLng::new(0.0, 0.0).unwrap();
    let style = r#"{
        "version": 8,
        "glyphs": "glyphs/{fontstack}/{range}.pbf",
        "terrain": {"source": "dem"},
        "sources": {
            "dem": {"type": "raster-dem", "tiles": ["dem/{z}/{x}/{y}.png"], "tileSize": 256, "maxzoom": 14},
            "points": {"type": "geojson", "data": {"type": "LineString", "coordinates": [[-0.01, 0], [0.01, 0]]}}
        },
        "layers": [
            {"id": "background", "type": "background", "paint": {"background-color": "white"}},
            {
                "id": "point",
                "type": "symbol",
                "source": "points",
                "layout": {"text-field": "HH", "text-font": ["Open Sans Regular"], "symbol-placement": "point"},
                "paint": {"text-color": "red"}
            },
            {
                "id": "line",
                "type": "symbol",
                "source": "points",
                "layout": {"text-field": "HH", "text-font": ["Open Sans Regular"], "symbol-placement": "line"},
                "paint": {"text-color": "red"}
            }
        ]
    }"#;
    let camera = CameraOptions { center: Some(center), zoom: Some(14.0), pitch: Some(60.0), ..Default::default() };
    let Some((plain, mut map, _, mut frontend)) = render_with_frontend(style, &[camera], |_| dem_png(METERS_300))
    else {
        return;
    };
    map.set_debug(MapDebugOptions::COLLISION);
    let debug = frontend.render(&mut map).expect("a frame with the collision boxes").image;
    assert_ne!(plain.data, debug.data, "no collision box drew");
}
