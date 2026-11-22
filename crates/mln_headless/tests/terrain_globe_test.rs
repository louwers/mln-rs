// Terrain on the globe: at zoom 14, where the sphere's curvature moves the
// ground by a fraction of a pixel, a view under `vertical-perspective` draws
// what the same view on Mercator draws, its drapes and the layers that stand
// on the ground alike.

use std::f64::consts::PI;
use std::sync::Arc;

use mln_headless::HeadlessFrontend;
use mln_map::{BoundOptions, Map, MapMode, MapOptions};
use mln_storage::resource::TileData;
use mln_storage::stub_file_source::{ResponseType, StubFileSource};
use mln_storage::{Kind, Resource, Response};
use mln_style::Edit;
use mln_style::image::Impl as StyleImage;
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
    render_over(style, ridge_png)
}

/// The still image of `style` over the DEM tiles that `dem` makes, with the
/// test fixtures' glyphs for every font and an image "square", a magenta
/// square 12 px across.
fn render_over(style: &str, dem: fn(&TileData) -> Vec<u8>) -> Option<PremultipliedImage> {
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
    file_source.set_response(Kind::Tile, move |resource: &Resource| {
        let mut response = Response::new();
        response.data = Some(Arc::new(dem(resource.tile_data.as_ref().expect("a tile"))));
        Some(response)
    });
    file_source
        .set_response(Kind::Glyphs, |_| Some(crate::support::file_response("test/fixtures/resources/glyphs.pbf")));
    let options = MapOptions { map_mode: MapMode::Static, size, pixel_ratio: 1.0, ..Default::default() };
    let mut map = Map::with_file_source(file_source, options, &pool).unwrap();
    frontend.attach(&mut map);
    map.set_bounds(&BoundOptions { max_pitch: Some(85.0), ..Default::default() }).unwrap();
    map.load_style_json(style);
    let mut square = PremultipliedImage::new(Size::new(12, 12));
    for pixel in square.data.as_chunks_mut::<4>().0 {
        *pixel = [255, 0, 255, 255];
    }
    let square = StyleImage::new("square".to_owned(), square, 1.0, false, Vec::new(), Vec::new(), None, None, None)
        .expect("an image");
    map.edit_style(|style| style.apply(Edit::AddImage(Arc::new(square)))).unwrap();
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

/// A map at zoom 14 pitched 55° over the ridge, on the globe if `globe`, of
/// a white background and `layer`, a layer of the source "feature" of the
/// GeoJSON `data`.
fn standing_style(globe: bool, data: &str, layer: &str) -> String {
    let projection = if globe { r#""projection": {"type": "vertical-perspective"},"# } else { "" };
    let (lon, lat) = CENTER;
    format!(
        r#"{{
            "version": 8,
            "center": [{lon}, {lat}],
            "zoom": 14,
            "pitch": 55,
            {projection}
            "glyphs": "glyphs/{{fontstack}}/{{range}}.pbf",
            "terrain": {{"source": "dem"}},
            "sources": {{
                "dem": {{"type": "raster-dem", "tiles": ["dem/{{z}}/{{x}}/{{y}}.png"], "tileSize": 256, "maxzoom": 15}},
                "feature": {{"type": "geojson", "data": {data}}}
            }},
            "layers": [
                {{"id": "background", "type": "background", "paint": {{"background-color": "white"}}}},
                {layer}
            ]
        }}"#
    )
}

/// The GeoJSON point `north` and `east` meters from the view's center.
fn point(north: f64, east: f64) -> String {
    let (lon, lat) = offset(north, east);
    format!(r#"{{"type": "Point", "coordinates": [{lon}, {lat}]}}"#)
}

/// The centroid of what of `image` isn't white, each pixel weighted by how
/// far from white it is, and that weight in all.
fn ink(image: &PremultipliedImage) -> ((f64, f64), f64) {
    let (mut sum, mut total) = ((0.0, 0.0), 0.0);
    for (index, pixel) in image.data.as_chunks::<4>().0.iter().enumerate() {
        let weight = f64::from(255 - pixel[..3].iter().min().copied().unwrap_or(255)) / 255.0;
        let (x, y) = ((index % SIDE as usize) as f64 + 0.5, (index / SIDE as usize) as f64 + 0.5);
        sum = (sum.0 + x * weight, sum.1 + y * weight);
        total += weight;
    }
    ((sum.0 / total, sum.1 / total), total)
}

/// At zoom 14, a view pitched over a 400 m ridge north of a center at sea
/// level stands a circle, an icon, a label along a line and a fill
/// extrusion on the ridge alike on the globe and on Mercator: each lands
/// within a pixel of where Mercator draws it, where at sea level it would
/// draw tens of pixels lower. The extrusion floats 20 m above the crest:
/// where an extrusion meets the ground on the globe, the globe's depth
/// precision rags their edge by a pixel or two.
#[test]
fn standing_layers_at_zoom_14_stand_alike_on_the_globe_and_mercator() {
    let (west, east) = (offset(180.0, -600.0), offset(180.0, 600.0));
    let crest =
        format!(r#"{{"type": "LineString", "coordinates": [[{}, {}], [{}, {}]]}}"#, west.0, west.1, east.0, east.1);
    let cases = [
        (
            "circle",
            point(180.0, -100.0),
            r#"{"id": "circle", "type": "circle", "source": "feature", "paint": {"circle-color": "black", "circle-radius": 6}}"#,
        ),
        (
            "icon",
            point(150.0, 100.0),
            r#"{"id": "icon", "type": "symbol", "source": "feature", "layout": {"icon-image": "square", "icon-allow-overlap": true}}"#,
        ),
        (
            "line label",
            crest,
            r#"{"id": "label", "type": "symbol", "source": "feature",
                "layout": {"text-field": "HHHHHH", "text-font": ["Open Sans Regular"], "text-size": 16,
                           "symbol-placement": "line", "text-allow-overlap": true}}"#,
        ),
        (
            "fill extrusion",
            square(180.0, 60.0, 30.0),
            r#"{"id": "extrusion", "type": "fill-extrusion", "source": "feature",
                "paint": {"fill-extrusion-color": "black", "fill-extrusion-base": 20, "fill-extrusion-height": 60}}"#,
        ),
    ];
    for (name, data, layer) in cases {
        let (Some(globe), Some(mercator)) =
            (render(&standing_style(true, &data, layer)), render(&standing_style(false, &data, layer)))
        else {
            return;
        };
        let ((globe_at, globe_ink), (mercator_at, mercator_ink)) = (ink(&globe), ink(&mercator));
        assert!(mercator_ink > 20.0, "the {name} shows: {mercator_ink}");
        let moved = (globe_at.0 - mercator_at.0).hypot(globe_at.1 - mercator_at.1);
        assert!(
            moved <= 1.0,
            "the {name} is at {globe_at:?} ({globe_ink:.0}) on the globe, {mercator_at:?} ({mercator_ink:.0}) on Mercator"
        );
    }
}

/// The DEM tile `_tile` of level ground at sea level, in the Mapbox
/// encoding.
fn sea_level_png(_tile: &TileData) -> Vec<u8> {
    let mut image = PremultipliedImage::new(Size::new(256, 256));
    for pixel in image.data.as_chunks_mut::<4>().0 {
        // (1 · 65536 + 134 · 256 + 160) · 0.1 − 10000 = 0 m.
        *pixel = [1, 134, 160, 255];
    }
    mln_image::encode_png(&image)
}

/// A label behind the ridge, which hides it from the camera, hides on the
/// globe as on Mercator, and one in front of the ridge shows on both: the
/// share of each label that draws, against the same label over level
/// ground, is the same on both projections within 0.05.
#[test]
fn a_label_behind_a_ridge_hides_on_the_globe_as_on_mercator() {
    let label = r#"{"id": "label", "type": "symbol", "source": "feature",
        "layout": {"text-field": "HH", "text-font": ["Open Sans Regular"], "text-size": 24,
                   "text-allow-overlap": true, "text-ignore-placement": true}}"#;
    // From the camera, about 2 km up and 2.8 km south, the line of sight to
    // the label 450 m north passes the crest 160 m up, under the ridge's
    // 400 m; to the one 200 m south it passes nothing.
    for (north, shows) in [(450.0, false), (-200.0, true)] {
        let data = point(north, 0.0);
        let shown = |globe: bool| -> Option<f64> {
            let style = standing_style(globe, &data, label);
            let over_ridge = ink(&render_over(&style, ridge_png)?).1;
            let over_level = ink(&render_over(&style, sea_level_png)?).1;
            assert!(over_level > 20.0, "the label {north} m north shows over level ground: {over_level}");
            Some(over_ridge / over_level)
        };
        let (Some(globe), Some(mercator)) = (shown(true), shown(false)) else {
            return;
        };
        assert!(
            (globe - mercator).abs() <= 0.05,
            "{north} m north: {globe} shows on the globe, {mercator} on Mercator"
        );
        let expected = if shows { 1.0 } else { 0.0 };
        assert!((mercator - expected).abs() <= 0.05, "{north} m north: {mercator} shows on Mercator");
    }
}
