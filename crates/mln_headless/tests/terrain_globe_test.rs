// Terrain on the globe: at zoom 14, where the sphere's curvature moves the
// ground by a fraction of a pixel, a view under `vertical-perspective` draws
// what the same view on Mercator draws, its drapes and the layers that stand
// on the ground alike.

use std::f64::consts::PI;
use std::sync::Arc;

use mln_headless::HeadlessFrontend;
use mln_map::{BoundOptions, CameraOptions, Map, MapMode, MapOptions};
use mln_map_types::View;
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
    dem_png(tile, ridge_height)
}

/// The DEM tile `tile` of ground whose height `height` gives by how many
/// meters north of the center it is, in the Mapbox encoding.
fn dem_png(tile: &TileData, height: impl Fn(f64) -> f64) -> Vec<u8> {
    let tiles = 2f64.powi(i32::from(tile.z));
    let mut image = PremultipliedImage::new(Size::new(256, 256));
    for (index, pixel) in image.data.as_chunks_mut::<4>().0.iter_mut().enumerate() {
        let row = (index / 256) as f64 + 0.5;
        let y = (f64::from(tile.y) + row / 256.0) / tiles;
        let latitude = (PI * (1.0 - 2.0 * y)).sinh().atan().to_degrees();
        let height = height((latitude - CENTER.1) * METERS_PER_DEGREE);
        let value = ((height + 10000.0) * 10.0).round() as u32;
        *pixel = [(value >> 16) as u8, (value >> 8) as u8, value as u8, 255];
    }
    mln_image::encode_png(&image)
}

/// The height of a ridge running east to west through the center, 1,500 m
/// high along its crest and falling to sea level `half_width` meters either
/// side of it, `north` meters north of the center.
fn crest_height(north: f64, half_width: f64) -> f64 {
    let across = north / half_width;
    if across.abs() >= 1.0 { 0.0 } else { 1500.0 * (across * PI / 2.0).cos().powi(2) }
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
    render_tiles(style, move |_, tile| dem(tile))
}

/// The still image of `style` over the tiles that `tile` makes of each
/// tile's URL and coordinates, with the glyphs and image of
/// [`render_over`].
fn render_tiles(
    style: &str,
    tile: impl Fn(&str, &TileData) -> Vec<u8> + Send + Sync + 'static,
) -> Option<PremultipliedImage> {
    still(style, tile, false, &[]).map(|(mut stills, ..)| stills.remove(0).0)
}

/// The still image of `style` over the tiles that `tile` makes, as
/// [`render_tiles`] draws it, with the center standing on the ground if
/// `clamped`, seen from the style's camera and then at each of `zooms` in
/// turn: each still with the view that drew it, and the map and the
/// frontend that drew the last.
fn still(
    style: &str,
    tile: impl Fn(&str, &TileData) -> Vec<u8> + Send + Sync + 'static,
    clamped: bool,
    zooms: &[f64],
) -> Option<(Vec<(PremultipliedImage, View)>, Map, HeadlessFrontend)> {
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
        response.data = Some(Arc::new(tile(&resource.url, resource.tile_data.as_ref().expect("a tile"))));
        Some(response)
    });
    file_source
        .set_response(Kind::Glyphs, |_| Some(crate::support::file_response("test/fixtures/resources/glyphs.pbf")));
    let options = MapOptions { map_mode: MapMode::Static, size, pixel_ratio: 1.0, ..Default::default() };
    let mut map = Map::with_file_source(file_source, options, &pool).unwrap();
    frontend.attach(&mut map);
    map.set_bounds(&BoundOptions { max_pitch: Some(85.0), ..Default::default() }).unwrap();
    map.set_center_clamped_to_ground(clamped);
    map.load_style_json(style);
    let mut square = PremultipliedImage::new(Size::new(12, 12));
    for pixel in square.data.as_chunks_mut::<4>().0 {
        *pixel = [255, 0, 255, 255];
    }
    let square = StyleImage::new("square".to_owned(), square, 1.0, false, Vec::new(), Vec::new(), None, None, None)
        .expect("an image");
    map.edit_style(|style| style.apply(Edit::AddImage(Arc::new(square)))).unwrap();
    let mut cameras = vec![map.style().get_default_camera()];
    cameras.extend(zooms.iter().map(|&zoom| CameraOptions { zoom: Some(zoom), ..Default::default() }));
    let stills = cameras
        .iter()
        .map(|camera| {
            map.jump_to(camera);
            let image = frontend.render(&mut map).expect("the still image").image;
            (image, map.transform_state().view().clone())
        })
        .collect();
    Some((stills, map, frontend))
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
/// where an extrusion meets the ground on the globe, the ground cuts its
/// foot a pixel or two differently, within the precision of the globe's
/// vertex positions, which the shaders place in f32 to a few meters.
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

/// A raster tile of one color: red for the western half of the world,
/// blue for the eastern.
fn half_png(tile: &TileData) -> Vec<u8> {
    let east = f64::from(tile.x) + 0.5 >= 2f64.powi(i32::from(tile.z)) / 2.0;
    let mut image = PremultipliedImage::new(Size::new(512, 512));
    for pixel in image.data.as_chunks_mut::<4>().0 {
        *pixel = if east { [0, 0, 255, 255] } else { [255, 0, 0, 255] };
    }
    mln_image::encode_png(&image)
}

/// Near the antimeridian at zoom 1, a raster drapes on both sides of it, on
/// the globe as on Mercator: the zoom 0 terrain tile's drape, in the world
/// copy of the center, holds the western raster tile that the globe's cover
/// puts in the next copy, as well as the eastern one.
#[test]
fn a_raster_drapes_on_both_sides_of_the_antimeridian() {
    let style = |globe: bool| {
        let projection = if globe { r#""projection": {"type": "vertical-perspective"},"# } else { "" };
        format!(
            r#"{{
                "version": 8,
                "center": [179, 0],
                "zoom": 1,
                {projection}
                "terrain": {{"source": "dem"}},
                "sources": {{
                    "dem": {{"type": "raster-dem", "tiles": ["dem/{{z}}/{{x}}/{{y}}.png"], "tileSize": 512, "maxzoom": 15}},
                    "raster": {{"type": "raster", "tiles": ["raster/{{z}}/{{x}}/{{y}}.png"], "tileSize": 512}}
                }},
                "layers": [
                    {{"id": "background", "type": "background", "paint": {{"background-color": "white"}}}},
                    {{"id": "raster", "type": "raster", "source": "raster", "paint": {{"raster-fade-duration": 0}}}}
                ]
            }}"#
        )
    };
    let tiles = |url: &str, tile: &TileData| if url.starts_with("dem/") { sea_level_png(tile) } else { half_png(tile) };
    for globe in [true, false] {
        let Some(image) = render_tiles(&style(globe), tiles) else {
            return;
        };
        let pixel = |x: u32| {
            let start = ((SIDE / 2 * SIDE + x) * 4) as usize;
            <[u8; 4]>::try_from(&image.data[start..start + 4]).unwrap()
        };
        // The antimeridian is a degree east of the center, a few pixels
        // right of the view's middle.
        let (west, east) = (pixel(SIDE / 2 - 40), pixel(SIDE / 2 + 40));
        assert_eq!([0, 0, 255, 255], west, "east of the meridian, west of the view's middle, on the globe: {globe}");
        assert_eq!([255, 0, 0, 255], east, "west of the meridian, east of the view's middle, on the globe: {globe}");
    }
}

/// At zoom 14, a view pitched 76° over level ground fogs it alike on the
/// globe and on Mercator: the same tint at the same distance. Below the
/// horizon, which the globe's curvature lowers by a few pixels, at most 1% of
/// the pixels differ visibly, where the fog tints the farthest ground most of
/// the way to its color. The globe's atmosphere is off: it draws over the
/// ground, as GL JS's does.
#[test]
fn the_fog_tints_the_ground_alike_on_the_globe_and_mercator() {
    // The ground is a green fill 40 km across, which drapes, where a
    // background would clear the frame.
    let fogged = |globe: bool| {
        let sky = r#""pitch": 76,
            "sky": {"fog-color": "rgb(255, 0, 255)", "fog-ground-blend": 0, "horizon-fog-blend": 1, "atmosphere-blend": 0},"#;
        let ground =
            format!(r#""sources": {{"ground": {{"type": "geojson", "data": {}}},"#, square(0.0, 0.0, 40_000.0));
        let fill =
            r#"{"id": "ground", "type": "fill", "source": "ground", "paint": {"fill-color": "rgb(0, 128, 0)"}},"#;
        style(globe).replace(r#""pitch": 55,"#, sky).replacen(r#""sources": {"#, &ground, 1).replacen(
            r#"{"id": "background", "type": "background", "paint": {"background-color": "white"}},"#,
            fill,
            1,
        )
    };
    let (Some(globe), Some(mercator)) =
        (render_over(&fogged(true), sea_level_png), render_over(&fogged(false), sea_level_png))
    else {
        return;
    };
    // The sky shows in the top 37 rows on Mercator.
    let ground = 48 * SIDE as usize..(SIDE * SIDE) as usize;
    let pixels = |image: &PremultipliedImage| image.data.as_chunks::<4>().0[ground.clone()].to_vec();
    let (globe, mercator) = (pixels(&globe), pixels(&mercator));
    let far = &mercator[..SIDE as usize];
    let red = far.iter().map(|pixel| u32::from(pixel[0])).sum::<u32>() / SIDE;
    assert!(red > 200, "the fog tints the farthest ground: red {red}");
    let differing =
        (globe.iter().zip(&mercator)).filter(|(a, b)| a.iter().zip(b.iter()).any(|(a, b)| a.abs_diff(*b) > 32)).count();
    assert!(differing <= globe.len() / 100, "{differing} of {} pixels differ by more than 32", globe.len());
}

/// A map at `zoom` pitched 55° over a ridge through the center, under
/// `projection` (a style's `projection.type`, or Mercator for `None`): a
/// white background, a square `side` meters across of each of `squares`,
/// draped, and with `standing`, a cyan circle 100 m south and 150 m east of
/// the center and a magenta icon 100 m north and 100 m west of it.
fn crest_style(
    projection: Option<&str>,
    zoom: f64,
    squares: &[([u8; 3], f64, f64)],
    side: f64,
    standing: bool,
) -> String {
    let projection = projection.map_or_else(String::new, |type_| format!(r#""projection": {{"type": "{type_}"}},"#));
    let (lon, lat) = CENTER;
    let mut sources = vec![
        r#""dem": {"type": "raster-dem", "tiles": ["dem/{z}/{x}/{y}.png"], "tileSize": 256, "maxzoom": 15}"#.to_owned(),
    ];
    let mut layers =
        vec![r#"{"id": "background", "type": "background", "paint": {"background-color": "white"}}"#.to_owned()];
    for (index, &([r, g, b], north, east)) in squares.iter().enumerate() {
        sources.push(format!(r#""square{index}": {{"type": "geojson", "data": {}}}"#, square(north, east, side)));
        layers.push(format!(
            r#"{{"id": "square{index}", "type": "fill", "source": "square{index}", "paint": {{"fill-color": "rgb({r}, {g}, {b})"}}}}"#
        ));
    }
    if standing {
        sources.push(format!(r#""circle": {{"type": "geojson", "data": {}}}"#, point(-100.0, 150.0)));
        sources.push(format!(r#""icon": {{"type": "geojson", "data": {}}}"#, point(100.0, -100.0)));
        layers.push(
            r#"{"id": "circle", "type": "circle", "source": "circle", "paint": {"circle-color": "rgb(0, 255, 255)", "circle-radius": 6}}"#
                .to_owned(),
        );
        layers.push(
            r#"{"id": "icon", "type": "symbol", "source": "icon", "layout": {"icon-image": "square", "icon-allow-overlap": true}}"#
                .to_owned(),
        );
    }
    format!(
        r#"{{
            "version": 8,
            "center": [{lon}, {lat}],
            "zoom": {zoom},
            "pitch": 55,
            {projection}
            "terrain": {{"source": "dem"}},
            "sources": {{{}}},
            "layers": [{}]
        }}"#,
        sources.join(",\n"),
        layers.join(",\n"),
    )
}

/// The squares on the ridge at zoom 14, 80 m across: one on the crest at
/// the center, one down each side.
const CREST_SQUARES: [([u8; 3], f64, f64); 3] =
    [([255, 0, 0], 0.0, 0.0), ([0, 255, 0], -200.0, -150.0), ([0, 0, 255], 150.0, 160.0)];

/// The colors of the standing layers of [`crest_style`]: the circle's and
/// the icon's.
const STANDING: [[u8; 3]; 2] = [[0, 255, 255], [255, 0, 255]];

/// How many pixels of `a` and `b` differ by more than 32 in a channel.
fn differing(a: &PremultipliedImage, b: &PremultipliedImage) -> usize {
    (a.data.as_chunks::<4>().0.iter().zip(b.data.as_chunks::<4>().0))
        .filter(|(a, b)| a.iter().zip(b.iter()).any(|(a, b)| a.abs_diff(*b) > 32))
        .count()
}

/// The still of [`crest_style`] at zoom 14 with the standing layers, the
/// center standing on the crest, on the globe if `globe`, with the map and
/// the frontend that drew it.
fn on_the_crest(globe: bool) -> Option<(PremultipliedImage, View, HeadlessFrontend)> {
    let style = crest_style(globe.then_some("vertical-perspective"), 14.0, &CREST_SQUARES, 80.0, true);
    let (mut stills, _, frontend) =
        still(&style, |_, tile| dem_png(tile, |north| crest_height(north, 1500.0)), true, &[])?;
    let (image, view) = stills.remove(0);
    Some((image, view, frontend))
}

/// At zoom 14, a camera whose center stands on a 1,500 m ridge sees alike
/// on the globe and on Mercator: the center stands as high on both, each
/// draped square and each standing layer lands within a pixel of where
/// Mercator draws it, and the pixels that differ visibly are few, along
/// edges.
#[test]
fn a_camera_on_a_ridge_at_zoom_14_sees_alike_on_the_globe_and_mercator() {
    let (Some((globe, globe_view, _)), Some((mercator, mercator_view, _))) = (on_the_crest(true), on_the_crest(false))
    else {
        return;
    };
    let (globe_altitude, mercator_altitude) = (globe_view.center_altitude(), mercator_view.center_altitude());
    assert!(mercator_altitude > 1450.0, "the center stands {mercator_altitude} m up on Mercator");
    assert!(
        (globe_altitude - mercator_altitude).abs() < 0.5,
        "the center stands {globe_altitude} m up on the globe, {mercator_altitude} m on Mercator"
    );
    for rgb in CREST_SQUARES.map(|(rgb, ..)| rgb).into_iter().chain(STANDING) {
        let ((globe_at, globe_count), (mercator_at, mercator_count)) =
            (centroid(&globe, rgb), centroid(&mercator, rgb));
        assert!(mercator_count > 20, "{rgb:?} shows: {mercator_count} px");
        let moved = (globe_at.0 - mercator_at.0).hypot(globe_at.1 - mercator_at.1);
        assert!(
            moved <= 1.0,
            "{rgb:?} is at {globe_at:?} ({globe_count} px) on the globe, {mercator_at:?} ({mercator_count} px) on Mercator"
        );
    }
    let differing = differing(&globe, &mercator);
    let allowed = (SIDE * SIDE) as usize / 50;
    assert!(differing <= allowed, "{differing} pixels differ by more than 32");
}

/// A rendered query over the ridge on the globe finds the draped square on
/// the crest at the pixel where it draws, and nothing just above it, where
/// the line of sight passes over it to the ground beyond.
#[test]
fn a_rendered_query_on_the_globe_finds_the_draped_fill_where_it_draws() {
    use mln_renderer::query::RenderedQueryOptions;
    use mln_util::geo::ScreenCoordinate;

    let Some((image, _, mut frontend)) = on_the_crest(true) else {
        return;
    };
    let ((x, y), count) = centroid(&image, CREST_SQUARES[0].0);
    assert!(count > 20, "the square shows: {count} px");
    let top = (0..SIDE as usize)
        .find(|&row| {
            let pixel = image.data.as_chunks::<4>().0[row * SIDE as usize + x.round() as usize];
            pixel[0] > 200 && pixel[1] < 64 && pixel[2] < 64
        })
        .expect("the square's top edge") as f64;
    let mut query = |x: f64, y: f64| {
        let options = RenderedQueryOptions::new(Some(vec!["square0".to_owned()]), None);
        frontend.renderer().query_rendered_features_at_point(&ScreenCoordinate::new(x, y), &options).unwrap().len()
    };
    assert_eq!(1, query(x, y), "the square where it draws, at {x}, {y}");
    assert_eq!(0, query(x, top - 4.0), "nothing above the square's top edge at {top}");
}

/// Squares 300 m across on a ridge 3 km either side of its crest: one on
/// the crest at the center, two down its side that faces the camera.
const HANDOVER_SQUARES: [([u8; 3], f64, f64); 3] =
    [([255, 0, 0], 0.0, 0.0), ([0, 255, 0], -1500.0, -1200.0), ([0, 0, 255], -700.0, 1300.0)];

/// A map under the `globe` projection, which hands the globe over to
/// Mercator by zoom 12, steps across that zoom, from 11.99 to 12.01, with
/// its center standing on a ridge, as a map on Mercator steps across it:
/// each square moves within a pixel of how far it moves on Mercator, the
/// step changes about as many pixels, and the center stands as high on
/// both sides and both projections. The style has no sky, so no
/// atmosphere draws.
#[test]
fn the_globe_hands_over_to_mercator_without_a_jump() {
    // The stills at zoom 11.99 and 12.01, with their views.
    let step = |projection: Option<&str>| {
        let style = crest_style(projection, 11.99, &HANDOVER_SQUARES, 300.0, false);
        let ridge = |_: &str, tile: &TileData| dem_png(tile, |north| crest_height(north, 3000.0));
        let (mut stills, ..) = still(&style, ridge, true, &[12.01])?;
        let after = stills.remove(1);
        Some((stills.remove(0), after))
    };
    let (Some(((globe_before, globe_view), (globe_after, globe_after_view))), Some((mercator_before, mercator_after))) =
        (step(Some("globe")), step(None))
    else {
        return;
    };
    assert!(globe_view.is_globe() && !globe_after_view.is_globe(), "the globe at zoom 11.99, Mercator at 12.01");
    let altitude = mercator_before.1.center_altitude();
    assert!(altitude > 1450.0, "the center stands {altitude} m up");
    let altitudes = [
        ("on the globe", globe_view.center_altitude()),
        ("after the globe", globe_after_view.center_altitude()),
        ("on Mercator after", mercator_after.1.center_altitude()),
    ];
    for (what, other) in altitudes {
        assert!((other - altitude).abs() < 0.5, "the center stands {other} m up {what}, {altitude} m on Mercator");
    }
    let (globe_before, globe_after) = (&globe_before, &globe_after);
    let (mercator_before, mercator_after) = (&mercator_before.0, &mercator_after.0);
    for (rgb, ..) in HANDOVER_SQUARES {
        let moved = |before: &PremultipliedImage, after: &PremultipliedImage| {
            let ((before, before_count), (after, after_count)) = (centroid(before, rgb), centroid(after, rgb));
            assert!(before_count > 20 && after_count > 20, "{rgb:?} shows: {before_count} and {after_count} px");
            (after.0 - before.0, after.1 - before.1)
        };
        let (globe_step, mercator_step) = (moved(globe_before, globe_after), moved(mercator_before, mercator_after));
        let jump = (globe_step.0 - mercator_step.0).hypot(globe_step.1 - mercator_step.1);
        assert!(jump <= 1.0, "{rgb:?} moves {globe_step:?} on the globe, {mercator_step:?} on Mercator");
    }
    let (globe_changed, mercator_changed) =
        (differing(globe_before, globe_after), differing(mercator_before, mercator_after));
    let allowed = mercator_changed + (SIDE * SIDE) as usize / 100;
    assert!(globe_changed <= allowed, "{globe_changed} pixels change on the globe, {mercator_changed} on Mercator");
}
