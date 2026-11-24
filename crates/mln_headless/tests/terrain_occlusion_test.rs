// Terrain hides the circles and labels that stand behind it, each whole, by
// the depth of the ground drawn in the same frame, and makes that depth only
// in a frame that needs it. It hides no heatmap kernel.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use mln_headless::HeadlessFrontend;
use mln_map::{BoundOptions, CameraOptions, Map, MapMode, MapOptions};
use mln_storage::resource::TileData;
use mln_storage::stub_file_source::{ResponseType, StubFileSource};
use mln_storage::{Kind, Resource, Response};
use mln_util::error::ErrorKind;
use mln_util::geo::{LatLng, ScreenCoordinate, WrapMode};
use mln_util::image::PremultipliedImage;
use mln_util::pool::Pool;
use mln_util::queue::Queue;
use mln_util::size::Size;

const SIDE: u32 = 256;

/// The meters a pixel of a DEM tile of zoom 15 spans at the equator.
const METERS_PER_PIXEL: f64 = 40_075_016.685_578_49 / (256.0 * 32768.0);

/// The middle of a tile of zoom 15 by the equator, where the tests stand.
const MIDDLE: f64 = 16384.5;

/// The location at `x`, `y` of the world at zoom 15, in tiles from its top
/// left.
fn location_at(x: f64, y: f64) -> LatLng {
    let tiles = 32768.0;
    let longitude = x / tiles * 360.0 - 180.0;
    let latitude = (std::f64::consts::PI * (1.0 - 2.0 * y / tiles)).sinh().atan().to_degrees();
    LatLng::new(latitude, longitude).unwrap()
}

/// The location `meters` north of the tests' middle.
fn north(meters: f64) -> LatLng {
    location_at(MIDDLE, MIDDLE - meters / METERS_PER_PIXEL / 256.0)
}

/// A DEM tile of 256 pixels a side, in the Mapbox encoding, whose ground is
/// `height` meters at each pixel's middle, in pixels of zoom 15 from the
/// world's top left.
fn dem_of(tile: &TileData, height: impl Fn(f64, f64) -> f64) -> Vec<u8> {
    let scale = 2f64.powi(15 - i32::from(tile.z));
    let mut image = PremultipliedImage::new(Size::new(256, 256));
    for (index, pixel) in image.data.as_chunks_mut::<4>().0.iter_mut().enumerate() {
        let column = (f64::from(tile.x * 256) + (index % 256) as f64 + 0.5) * scale;
        let row = (f64::from(tile.y * 256) + (index / 256) as f64 + 0.5) * scale;
        let value = ((height(column, row).clamp(0.0, 1000.0) + 10000.0) * 10.0).round() as u32;
        *pixel = [(value >> 16) as u8, (value >> 8) as u8, value as u8, 255];
    }
    mln_image::encode_png(&image)
}

/// How high the ridge of [`ridge_height`] is, and how far north of the
/// middle.
const RIDGE: (f64, f64) = (60.0, 120.0);

/// How far north of the middle a point behind the ridge is, which the ridge
/// hides from the tests' camera at the middle.
const BEHIND: f64 = 200.0;

/// A ridge [`RIDGE`] high running east and west, north of the middle, whose
/// sides fall to the ground at sea level 40 m to either side of it.
fn ridge_height(_column: f64, row: f64) -> f64 {
    let (height, north) = RIDGE;
    let ridge = MIDDLE * 256.0 - north / METERS_PER_PIXEL;
    height * (1.0 - (row - ridge).abs() * METERS_PER_PIXEL / 40.0).max(0.0)
}

/// A circle layer of red circles at `red` and blue ones at `blue`, 12 px
/// across, over a white background and a terrain with no `terrain`.
fn circles_style(red: &[LatLng], blue: &[LatLng], terrain: bool) -> String {
    let points = |locations: &[LatLng]| {
        let coordinates: Vec<String> =
            locations.iter().map(|at| format!("[{}, {}]", at.longitude(), at.latitude())).collect();
        format!(r#"{{"type": "MultiPoint", "coordinates": [{}]}}"#, coordinates.join(", "))
    };
    let (red, blue) = (points(red), points(blue));
    let terrain = if terrain { r#""terrain": {"source": "dem"},"# } else { "" };
    format!(
        r#"{{
            "version": 8,
            {terrain}
            "sources": {{
                "dem": {{"type": "raster-dem", "tiles": ["dem/{{z}}/{{x}}/{{y}}.png"], "tileSize": 256, "maxzoom": 15}},
                "red": {{"type": "geojson", "data": {red}}},
                "blue": {{"type": "geojson", "data": {blue}}}
            }},
            "layers": [
                {{"id": "background", "type": "background", "paint": {{"background-color": "white"}}}},
                {{"id": "red", "type": "circle", "source": "red",
                  "paint": {{"circle-color": "red", "circle-radius": 6}}}},
                {{"id": "blue", "type": "circle", "source": "blue",
                  "paint": {{"circle-color": "blue", "circle-radius": 6}}}}
            ]
        }}"#
    )
}

/// A heatmap layer of one kernel at `at`, opaque blue wherever its density
/// shows, over a white background and a terrain.
fn heatmap_style(at: &LatLng) -> String {
    format!(
        r#"{{
            "version": 8,
            "terrain": {{"source": "dem"}},
            "sources": {{
                "dem": {{"type": "raster-dem", "tiles": ["dem/{{z}}/{{x}}/{{y}}.png"], "tileSize": 256, "maxzoom": 15}},
                "point": {{"type": "geojson", "data": {{"type": "Point", "coordinates": [{}, {}]}}}}
            }},
            "layers": [
                {{"id": "background", "type": "background", "paint": {{"background-color": "white"}}}},
                {{"id": "heatmap", "type": "heatmap", "source": "point",
                  "paint": {{"heatmap-radius": 10,
                            "heatmap-color": ["step", ["heatmap-density"], "rgba(0, 0, 0, 0)", 0.05, "blue"]}}}}
            ]
        }}"#,
        at.longitude(),
        at.latitude()
    )
}

/// The camera of the tests: at `center`, zoom 16 and a pitch of 70°, looking
/// along `bearing`.
fn camera(center: LatLng, bearing: f64) -> CameraOptions {
    CameraOptions {
        center: Some(center),
        zoom: Some(16.0),
        pitch: Some(70.0),
        bearing: Some(bearing),
        ..Default::default()
    }
}

/// A frontend on the harness's device ([`mln_headless::harness_device`]) of
/// `size`, with a map of `mode` and that size, which draws `style` over the
/// DEM tiles that `dem` answers, when it answers; the glyphs of every font
/// are the test fixtures'. `None` where the machine has no such device.
fn map(
    style: &str,
    size: Size,
    mode: MapMode,
    dem: impl Fn(&TileData) -> Option<Vec<u8>> + Send + Sync + 'static,
) -> Option<(HeadlessFrontend, Map, Queue<()>)> {
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
        let mut response = Response::new();
        response.data = Some(Arc::new(dem(resource.tile_data.as_ref().expect("a tile"))?));
        Some(response)
    });
    file_source
        .set_response(Kind::Glyphs, |_| Some(crate::support::file_response("test/fixtures/resources/glyphs.pbf")));
    let options = MapOptions { map_mode: mode, size, pixel_ratio: 1.0, ..Default::default() };
    let mut map = Map::with_file_source(file_source, options, &pool).unwrap();
    frontend.attach(&mut map);
    map.set_bounds(&BoundOptions { max_pitch: Some(85.0), ..Default::default() }).unwrap();
    map.load_style_json(style);
    Some((frontend, map, queue))
}

/// The still of `style` at `size`, seen by `camera`, over the DEM tiles of
/// `dem`, with the map that drew it and its frontend.
fn still(
    style: &str,
    size: Size,
    camera: &CameraOptions,
    dem: impl Fn(&TileData) -> Vec<u8> + Send + Sync + 'static,
) -> Option<(PremultipliedImage, Map, HeadlessFrontend)> {
    let (mut frontend, mut map, _) = map(style, size, MapMode::Static, move |tile| Some(dem(tile)))?;
    map.jump_to(camera);
    let image = frontend.render(&mut map).expect("the still image").image;
    Some((image, map, frontend))
}

fn pixel(image: &PremultipliedImage, x: u32, y: u32) -> [u8; 4] {
    let start = ((y * image.size.width + x) * 4) as usize;
    image.data[start..start + 4].try_into().unwrap()
}

/// Whether `pixel` is red, opaque or partly so, over white.
fn red(pixel: [u8; 4]) -> bool {
    let [r, g, b, _] = pixel;
    r > 200 && g < 200 && b < 200 && g.abs_diff(b) < 16
}

/// Whether `pixel` is blue, opaque or partly so, over white.
fn blue(pixel: [u8; 4]) -> bool {
    let [r, g, b, _] = pixel;
    b > 200 && r < 200 && g < 200 && r.abs_diff(g) < 16
}

/// How many pixels of `image` are `kind`.
fn count(image: &PremultipliedImage, kind: fn([u8; 4]) -> bool) -> usize {
    image.data.as_chunks::<4>().0.iter().filter(|&&pixel| kind(pixel)).count()
}

/// Where in an image of `map`, its camera sees `location` `height` meters
/// above sea level, in pixels from the top left, if it is in the image.
fn seen_at(map: &Map, location: &LatLng, height: f64) -> Option<(u32, u32)> {
    let point = map.transform_state().view().lat_lng_to_screen_coordinate_at(location, height);
    let size = map.transform_state().view().size();
    let (x, y) = (point.x.round(), (f64::from(size.height) - point.y).round());
    let inside = (0.0..f64::from(size.width)).contains(&x) && (0.0..f64::from(size.height)).contains(&y);
    inside.then_some((x as u32, y as u32))
}

/// A ridge 60 m high, 120 m north of a red circle at the camera's center,
/// hides a blue circle 80 m north of it, at sea level, from a camera pitched
/// 70° looking north, and doesn't hide the red one. Over the same ground at
/// sea level, the blue circle shows. A heatmap's kernel where the blue circle
/// was shows behind the ridge: the ground hides no kernel.
#[test]
fn a_ridge_hides_a_circle_behind_it_and_not_one_before_it() {
    let (front, behind) = (north(0.0), north(BEHIND));
    let style = circles_style(&[front], &[behind], true);
    let view = camera(front, 0.0);
    let size = Size::new(SIDE, SIDE);
    let Some((image, map, _)) = still(&style, size, &view, |tile| dem_of(tile, ridge_height)) else {
        return;
    };
    let (x, y) = seen_at(&map, &front, 0.0).expect("the red circle in view");
    assert!(red(pixel(&image, x, y)), "the red circle before the ridge");
    let hidden = seen_at(&map, &behind, 0.0).expect("the blue circle in view, behind the ridge");
    assert_eq!(0, count(&image, blue), "the blue circle behind the ridge, seen at {hidden:?}");

    let Some((level, level_map, _)) = still(&style, size, &view, |tile| dem_of(tile, |_, _| 0.0)) else {
        return;
    };
    let (x, y) = seen_at(&level_map, &behind, 0.0).expect("the blue circle in view");
    assert!(blue(pixel(&level, x, y)), "the blue circle over level ground");

    let Some((kernel, kernel_map, _)) = still(&heatmap_style(&behind), size, &view, |tile| dem_of(tile, ridge_height))
    else {
        return;
    };
    let (x, y) = seen_at(&kernel_map, &behind, 0.0).expect("the kernel in view, behind the ridge");
    assert!(blue(pixel(&kernel, x, y)), "the heatmap's kernel behind the ridge");
}

/// [`circles_style`] over terrain, with a fill square under its circles, 20 m
/// to each side of `at`.
fn circles_and_fill_style(red: &[LatLng], blue: &[LatLng], at: &LatLng) -> String {
    // 20 m in degrees at the equator.
    let half = 20.0 / METERS_PER_PIXEL / 256.0 / 32768.0 * 360.0;
    let (w, e, s, n) = (at.longitude() - half, at.longitude() + half, at.latitude() - half, at.latitude() + half);
    let fill = format!(
        r#""fill": {{"type": "geojson", "data": {{"type": "Polygon", "coordinates": [[
            [{w}, {s}], [{e}, {s}], [{e}, {n}], [{w}, {n}], [{w}, {s}]
        ]]}}}},
        "red":"#
    );
    circles_style(red, blue, true).replacen(r#""red":"#, &fill, 1).replacen(
        r#"{"id": "red""#,
        r#"{"id": "fill", "type": "fill", "source": "fill", "paint": {"fill-color": "lime"}},
                {"id": "red""#,
        1,
    )
}

/// How many features of the layer `layer` the frontend's renderer finds at
/// `x`, `y`, in pixels from the top left.
fn query_at(frontend: &mut HeadlessFrontend, layer: &str, (x, y): (u32, u32)) -> usize {
    use mln_renderer::query::RenderedQueryOptions;

    let options = RenderedQueryOptions::new(Some(vec![layer.to_owned()]), None);
    let point = ScreenCoordinate::new(f64::from(x), f64::from(y));
    frontend.renderer().query_rendered_features_at_point(&point, &options).unwrap().len()
}

/// The ridge of [`a_ridge_hides_a_circle_behind_it_and_not_one_before_it`]
/// hides a blue circle and a fill square behind it from queries where the
/// camera sees them at sea level: there, the line of sight meets the ridge
/// first. The red circle before it is found. Over the same ground at sea
/// level, the blue circle and the square are found there.
#[test]
fn a_ridge_hides_a_circle_and_a_fill_behind_it_from_queries() {
    let (front, behind) = (north(0.0), north(BEHIND));
    let style = circles_and_fill_style(&[front], &[behind], &behind);
    let view = camera(front, 0.0);
    let size = Size::new(SIDE, SIDE);
    let Some((_, map, mut frontend)) = still(&style, size, &view, |tile| dem_of(tile, ridge_height)) else {
        return;
    };
    let red_at = seen_at(&map, &front, 0.0).expect("the red circle in view");
    let hidden = seen_at(&map, &behind, 0.0).expect("the blue circle in view, behind the ridge");
    assert_eq!(1, query_at(&mut frontend, "red", red_at), "the red circle before the ridge");
    assert_eq!(0, query_at(&mut frontend, "blue", hidden), "the blue circle behind the ridge");
    assert_eq!(0, query_at(&mut frontend, "fill", hidden), "the fill behind the ridge");

    let Some((_, level_map, mut level)) = still(&style, size, &view, |tile| dem_of(tile, |_, _| 0.0)) else {
        return;
    };
    let shown = seen_at(&level_map, &behind, 0.0).expect("the blue circle in view");
    assert_eq!(1, query_at(&mut level, "blue", shown), "the blue circle over level ground");
    assert_eq!(1, query_at(&mut level, "fill", shown), "the fill over level ground");
}

/// A red label on the ground, on a slope that falls 30° to the east, seen
/// from a camera pitched 45° at each of eight bearings, so that the camera
/// sees the slope at 15° or more from grazing, shows at full opacity: the
/// ground under it doesn't hide it. Seen at a grazing angle, as looking down
/// this slope from a pitch of 55° or more, a label standing on the ground can
/// draw part-faded, as in GL JS: the ground's depth is read at the texel
/// nearest the label's anchor, which there can lie well in front of it.
#[test]
fn a_label_on_a_slope_shows_whole_at_an_ordinary_angle() {
    let top = location_at(MIDDLE, MIDDLE);
    let style = format!(
        r#"{{
            "version": 8,
            "glyphs": "glyphs/{{fontstack}}/{{range}}.pbf",
            "terrain": {{"source": "dem"}},
            "sources": {{
                "dem": {{"type": "raster-dem", "tiles": ["dem/{{z}}/{{x}}/{{y}}.png"], "tileSize": 256, "maxzoom": 15}},
                "point": {{"type": "geojson", "data": {{"type": "Point", "coordinates": [{}, {}]}}}}
            }},
            "layers": [
                {{"id": "background", "type": "background", "paint": {{"background-color": "white"}}}},
                {{"id": "label", "type": "symbol", "source": "point",
                  "layout": {{"text-field": "HH", "text-font": ["Open Sans Regular"], "text-size": 24}},
                  "paint": {{"text-color": "red"}}}}
            ]
        }}"#,
        top.longitude(),
        top.latitude()
    );
    let slope = 30f64.to_radians().tan();
    let ground = move |column: f64, _row: f64| 500.0 - slope * (column - MIDDLE * 256.0) * METERS_PER_PIXEL;
    let Some((mut frontend, mut map, _)) =
        map(&style, Size::new(SIDE, SIDE), MapMode::Static, move |tile| Some(dem_of(tile, ground)))
    else {
        return;
    };
    // Opaque red: a label that fades shows pink over the white ground.
    let opaque = |pixel: [u8; 4]| pixel[0] > 200 && pixel[1] < 40 && pixel[2] < 40;
    for bearing in (0..8).map(|step| f64::from(step) * 45.0) {
        map.jump_to(&CameraOptions { pitch: Some(45.0), ..camera(top, bearing) });
        let image = frontend.render(&mut map).expect("the still image").image;
        let shown = count(&image, opaque);
        assert!(shown >= 40, "at a bearing of {bearing}°, {shown} opaque red pixels of the label");
    }
}

/// A red label on the level ground far behind a ridge, whose anchor the
/// camera sees a quarter of a pixel above the ridge's silhouette, shows at
/// full opacity: the ground's depth is read at the texel nearest the anchor,
/// which holds the ground the label stands on, though the row of texels just
/// below holds the ridge, much nearer the camera. The ridge is drawn lime,
/// draped, to find its silhouette.
#[test]
fn a_label_just_above_a_ridges_silhouette_shows_whole() {
    let view = camera(north(0.0), 0.0);
    let size = Size::new(SIDE, SIDE);
    let style = |label: Option<&LatLng>| {
        let (near, far) = (north(RIDGE.1 - 50.0), north(RIDGE.1 + 50.0));
        let (west, east) = (near.longitude() - 0.02, near.longitude() + 0.02);
        let (s, n) = (near.latitude(), far.latitude());
        let point = label.map_or([0.0, 0.0], |at| [at.longitude(), at.latitude()]);
        let label = if label.is_some() {
            r#",
                {"id": "label", "type": "symbol", "source": "label",
                  "layout": {"text-field": "HH", "text-font": ["Open Sans Regular"], "text-size": 24,
                             "text-allow-overlap": true, "text-ignore-placement": true},
                  "paint": {"text-color": "red"}}"#
        } else {
            ""
        };
        format!(
            r#"{{
                "version": 8,
                "glyphs": "glyphs/{{fontstack}}/{{range}}.pbf",
                "terrain": {{"source": "dem"}},
                "sources": {{
                    "dem": {{"type": "raster-dem", "tiles": ["dem/{{z}}/{{x}}/{{y}}.png"], "tileSize": 256, "maxzoom": 15}},
                    "ridge": {{"type": "geojson", "data": {{"type": "Polygon", "coordinates": [[
                        [{west}, {s}], [{east}, {s}], [{east}, {n}], [{west}, {n}], [{west}, {s}]
                    ]]}}}},
                    "label": {{"type": "geojson", "data": {{"type": "Point", "coordinates": [{}, {}]}}}}
                }},
                "layers": [
                    {{"id": "background", "type": "background", "paint": {{"background-color": "white"}}}},
                    {{"id": "ridge", "type": "fill", "source": "ridge", "paint": {{"fill-color": "lime"}}}}{label}
                ]
            }}"#,
            point[0], point[1]
        )
    };
    let Some((ridge, ridge_map, _)) = still(&style(None), size, &view, |tile| dem_of(tile, ridge_height)) else {
        return;
    };
    let green = |pixel: [u8; 4]| pixel[1] > 200 && pixel[0] < 100 && pixel[2] < 100;
    let top = |column: u32| (0..SIDE).find(|&row| green(pixel(&ridge, column, row))).expect("the ridge in view");
    let row = top(128);
    assert_eq!(row, top(127), "the ridge's silhouette across the label's anchor");
    // A quarter of a pixel above the top of the ridge's first row, in the
    // row above it, and on the boundary of two columns that both show it.
    let anchor = ScreenCoordinate::new(f64::from(SIDE / 2), f64::from(SIDE - row) + 0.25);
    let at = ridge_map.transform_state().view().screen_coordinate_to_lat_lng(&anchor, WrapMode::Unwrapped).unwrap();
    let Some((image, _, _)) = still(&style(Some(&at)), size, &view, |tile| dem_of(tile, ridge_height)) else {
        return;
    };
    let opaque = |pixel: [u8; 4]| pixel[0] > 200 && pixel[1] < 40 && pixel[2] < 40;
    let shown = count(&image, opaque);
    assert!(shown >= 40, "{shown} opaque red pixels of the label");
}

/// In a continuous map, a ridge whose DEM tile of zoom 15 arrives after the
/// map drew its ground from the level DEM tiles of zoom 14 hides the red
/// label behind it in the frame that first draws the ridge: the frame whose
/// blue circle on the ridge's top stands on it.
#[test]
fn a_ridge_hides_a_label_behind_it_in_the_frame_its_dem_arrives() {
    let (front, top, behind) = (north(0.0), north(RIDGE.1), north(BEHIND));
    let style = format!(
        r#"{{
            "version": 8,
            "glyphs": "glyphs/{{fontstack}}/{{range}}.pbf",
            "terrain": {{"source": "dem"}},
            "sources": {{
                "dem": {{"type": "raster-dem", "tiles": ["dem/{{z}}/{{x}}/{{y}}.png"], "tileSize": 256, "maxzoom": 15}},
                "label": {{"type": "geojson", "data": {{"type": "Point", "coordinates": [{}, {}]}}}},
                "top": {{"type": "geojson", "data": {{"type": "Point", "coordinates": [{}, {}]}}}}
            }},
            "layers": [
                {{"id": "background", "type": "background", "paint": {{"background-color": "white"}}}},
                {{"id": "top", "type": "circle", "source": "top",
                  "paint": {{"circle-color": "blue", "circle-radius": 6}}}},
                {{"id": "label", "type": "symbol", "source": "label",
                  "layout": {{"text-field": "HH", "text-font": ["Open Sans Regular"], "text-size": 24,
                              "text-allow-overlap": true, "text-ignore-placement": true}},
                  "paint": {{"text-color": "red"}}}}
            ]
        }}"#,
        behind.longitude(),
        behind.latitude(),
        top.longitude(),
        top.latitude()
    );
    let arrived = Arc::new(AtomicBool::new(false));
    let dem = {
        let arrived = Arc::clone(&arrived);
        move |tile: &TileData| match tile.z {
            15 if !arrived.load(Ordering::SeqCst) => None,
            15 => Some(dem_of(tile, ridge_height)),
            _ => Some(dem_of(tile, |_, _| 0.0)),
        }
    };
    let Some((mut frontend, mut map, queue)) = map(&style, Size::new(SIDE, SIDE), MapMode::Continuous, dem) else {
        return;
    };
    map.jump_to(&camera(front, 0.0));

    // Draws the map's frames until `done` says one is the last, and returns
    // that one's image and the map's camera then.
    let mut frames = |done: &mut dyn FnMut(&PremultipliedImage, &Map) -> bool| {
        let start = Instant::now();
        loop {
            assert!(start.elapsed() < Duration::from_secs(30), "no such frame");
            map.process_events();
            queue.run_once(&mut ());
            map.trigger_repaint();
            map.process_events();
            if frontend.render_update() {
                map.process_events();
                let image = frontend.read_still_image();
                if done(&image, &map) {
                    return image;
                }
            } else {
                std::thread::sleep(Duration::from_millis(1));
            }
        }
    };
    // Over the level ground, the label shows, faded in, and the circle on
    // the ridge's top stands at sea level.
    let opaque = |pixel: [u8; 4]| pixel[0] > 200 && pixel[1] < 40 && pixel[2] < 40;
    frames(&mut |image, map| {
        let (x, y) = seen_at(map, &top, 0.0).expect("the circle at sea level in view");
        blue(pixel(image, x, y)) && count(image, opaque) >= 40
    });

    arrived.store(true, Ordering::SeqCst);
    let image = frames(&mut |image, map| {
        let (x, y) = seen_at(map, &top, RIDGE.0).expect("the circle on the ridge in view");
        blue(pixel(image, x, y))
    });
    assert_eq!(0, count(&image, red), "the label behind the ridge, in the frame that draws the ridge");
}

/// A map resized after a still draws its next still as a map made at the new
/// size does: the ground's depth takes the new size, at a logical pixel a
/// texel, and the draws bind the new one.
#[test]
fn a_resized_map_hides_what_a_map_made_at_its_size_hides() {
    let (front, behind) = (north(0.0), north(BEHIND));
    let style = circles_style(&[front], &[behind], true);
    let view = camera(front, 0.0);
    let (first, second) = (Size::new(SIDE, SIDE), Size::new(200, 320));
    let Some((expected, _, _)) = still(&style, second, &view, |tile| dem_of(tile, ridge_height)) else {
        return;
    };
    let Some((_, mut map, mut frontend)) = still(&style, first, &view, |tile| dem_of(tile, ridge_height)) else {
        return;
    };
    frontend.set_size(second);
    map.set_size(second).unwrap();
    let result = frontend.render(&mut map).expect("the still image");
    assert_eq!(expected.size, result.image.size);
    let differing = result.image.data.chunks(4).zip(expected.data.chunks(4)).filter(|(a, b)| a != b).count();
    assert_eq!(0, differing, "pixels that differ from the map made at {second:?}");
    let texel = 8;
    assert_eq!((200 * 320 * texel) as usize, result.stats.terrain.mem_ground_depth, "the ground's depth's bytes");
    assert!(result.stats.terrain.ground_depth_draw_calls > 0, "no draws of the ground's depth");
}

/// The ground's depth is drawn in a frame with terrain and a layer it hides,
/// and in no other: not without terrain, nor over terrain with only layers
/// that drape, that the depth test hides, such as fill extrusions, or that
/// it doesn't hide, such as heatmaps.
#[test]
fn only_a_frame_with_terrain_and_a_layer_it_hides_draws_the_ground_depth() {
    let at = north(0.0);
    let view = camera(at, 0.0);
    let size = Size::new(SIDE, SIDE);
    let extrusion = format!(
        r#"{{
            "version": 8,
            "terrain": {{"source": "dem"}},
            "sources": {{
                "dem": {{"type": "raster-dem", "tiles": ["dem/{{z}}/{{x}}/{{y}}.png"], "tileSize": 256, "maxzoom": 15}},
                "square": {{"type": "geojson", "data": {{"type": "Polygon", "coordinates": [[
                    [{w}, {s}], [{e}, {s}], [{e}, {n}], [{w}, {n}], [{w}, {s}]
                ]]}}}}
            }},
            "layers": [
                {{"id": "background", "type": "background", "paint": {{"background-color": "white"}}}},
                {{"id": "extrusion", "type": "fill-extrusion", "source": "square",
                  "paint": {{"fill-extrusion-color": "red", "fill-extrusion-height": 50}}}}
            ]
        }}"#,
        w = at.longitude() - 0.0002,
        e = at.longitude() + 0.0002,
        s = at.latitude() - 0.0002,
        n = at.latitude() + 0.0002,
    );
    let cases = [
        ("circles without terrain", circles_style(&[at], &[], false), false),
        ("fill extrusions over terrain", extrusion, false),
        ("a heatmap over terrain", heatmap_style(&at), false),
        ("circles over terrain", circles_style(&[at], &[], true), true),
    ];
    for (name, style, draws) in cases {
        let Some((_, mut map, mut frontend)) = still(&style, size, &view, |tile| dem_of(tile, ridge_height)) else {
            return;
        };
        let stats = frontend.render(&mut map).expect("the still image").stats.terrain;
        assert_eq!(draws, stats.ground_depth_draw_calls > 0, "{name}: draws of the ground's depth");
        let bytes = if draws { (SIDE * SIDE * 8) as usize } else { 0 };
        assert_eq!(bytes, stats.mem_ground_depth, "{name}: the ground's depth's bytes");
    }
}
