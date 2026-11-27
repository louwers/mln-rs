// Terrain on the globe: at zoom 14, where the sphere's curvature moves the
// ground by a fraction of a pixel, a view under `vertical-perspective` drapes
// its layers over the ground as the same view on Mercator does.

use std::f64::consts::PI;
use std::sync::Arc;

use mln_engine::camera::BoundOptions;
use mln_engine::exec::pool::Pool;
use mln_engine::exec::queue::Queue;
use mln_engine::map::{Map, MapMode, MapOptions};
use mln_engine::storage::resource::TileData;
use mln_engine::storage::stub_file_source::{ResponseType, StubFileSource};
use mln_engine::storage::{Kind, Resource, Response};
use mln_headless::HeadlessFrontend;
use mln_style::bitmap::PremultipliedImage;
use mln_style::error::ErrorKind;
use mln_style::size::Size;

const SIDE: u32 = 256;

/// The view's center, on level ground at sea level.
const CENTER: (f64, f64) = (10.0, 20.0);

/// Meters a degree of latitude spans on GL JS's sphere.
const METERS_PER_DEGREE: f64 = 6_371_008.8 * PI / 180.0;

/// The longitude and latitude at the middle of the pixel at `column` and
/// `row` of the tile `tile`, `size` pixels a side.
fn pixel_location(tile: &TileData, size: u32, column: u32, row: u32) -> (f64, f64) {
    let tiles = 2f64.powi(i32::from(tile.z));
    let size = f64::from(size);
    let x = (f64::from(tile.x) + (f64::from(column) + 0.5) / size) / tiles;
    let y = (f64::from(tile.y) + (f64::from(row) + 0.5) / size) / tiles;
    (x * 360.0 - 180.0, (PI * (1.0 - 2.0 * y)).sinh().atan().to_degrees())
}

/// How many meters north and east of [`CENTER`] the location `(lon, lat)`
/// is.
fn meters_from_center((lon, lat): (f64, f64)) -> (f64, f64) {
    let (center_lon, center_lat) = CENTER;
    let north = (lat - center_lat) * METERS_PER_DEGREE;
    let east = (lon - center_lon) * METERS_PER_DEGREE * center_lat.to_radians().cos();
    (north, east)
}

/// A PNG of `size` pixels a side of the tile `tile`, each pixel `pixel` of
/// the meters north and east of the center at its middle.
fn tile_png(tile: &TileData, size: u32, pixel: impl Fn(f64, f64) -> [u8; 4]) -> Vec<u8> {
    let mut image = PremultipliedImage::new(Size::new(size, size));
    for (index, texel) in image.data.as_chunks_mut::<4>().0.iter_mut().enumerate() {
        let (column, row) = (index as u32 % size, index as u32 / size);
        let (north, east) = meters_from_center(pixel_location(tile, size, column, row));
        *texel = pixel(north, east);
    }
    mln_engine::image::encode_png(&image)
}

/// The height of the ground `north` meters north of the center: a ridge
/// running east to west, 400 m high 180 m north, falling to sea level 150 m
/// either side of its crest, which the camera, about 1,000 m up, sees from
/// the south.
fn ridge_height(north: f64) -> f64 {
    let across = (north - 180.0) / 150.0;
    if across.abs() >= 1.0 { 0.0 } else { 400.0 * (across * PI / 2.0).cos().powi(2) }
}

/// The location `north` and `east` meters from [`CENTER`], as longitude and
/// latitude.
fn offset(north: f64, east: f64) -> (f64, f64) {
    let (lon, lat) = CENTER;
    (lon + east / (METERS_PER_DEGREE * lat.to_radians().cos()), lat + north / METERS_PER_DEGREE)
}

/// The DEM tile `tile` of the ridge, in the Mapbox encoding.
fn ridge_png(tile: &TileData) -> Vec<u8> {
    tile_png(tile, 256, |north, _| {
        let value = ((ridge_height(north) + 10000.0) * 10.0).round() as u32;
        [(value >> 16) as u8, (value >> 8) as u8, value as u8, 255]
    })
}

/// The DEM tile `_tile` of level ground at sea level, in the Mapbox
/// encoding.
fn sea_level_png(_tile: &TileData) -> Vec<u8> {
    let mut image = PremultipliedImage::new(Size::new(512, 512));
    for pixel in image.data.as_chunks_mut::<4>().0 {
        *pixel = [1, 134, 160, 255];
    }
    mln_engine::image::encode_png(&image)
}

/// The squares the raster draws: their colors and where they are, north
/// and east of the center, in meters, each 80 m across.
const SQUARES: [([u8; 3], f64, f64); 3] =
    [([255, 0, 0], 180.0, 0.0), ([0, 255, 0], -120.0, -150.0), ([0, 0, 255], 90.0, 160.0)];

/// The raster tile `tile` of the squares on white.
fn squares_png(tile: &TileData) -> Vec<u8> {
    tile_png(tile, 256, |north, east| {
        let inside = |&&(_, square_north, square_east): &&([u8; 3], f64, f64)| {
            (north - square_north).abs() < 40.0 && (east - square_east).abs() < 40.0
        };
        match SQUARES.iter().find(inside) {
            Some(&([r, g, b], ..)) => [r, g, b, 255],
            None => [255, 255, 255, 255],
        }
    })
}

/// A map at zoom 14 pitched 55° over the ridge, with a raster of squares
/// draped over it, on the globe if `globe`.
fn style(globe: bool) -> String {
    let projection = if globe { r#""projection": {"type": "vertical-perspective"},"# } else { "" };
    let (lon, lat) = CENTER;
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
                "squares": {{"type": "raster", "tiles": ["raster/{{z}}/{{x}}/{{y}}.png"], "tileSize": 256, "maxzoom": 16}}
            }},
            "layers": [
                {{"id": "background", "type": "background", "paint": {{"background-color": "white"}}}},
                {{"id": "squares", "type": "raster", "source": "squares", "paint": {{"raster-fade-duration": 0}}}}
            ]
        }}"#
    )
}

/// The still image of `style` on the harness's device
/// ([`mln_headless::harness_device`]), over the tiles that `tile` makes of
/// each tile's URL and coordinates; `None` where the machine has no such
/// device.
fn render(
    style: &str,
    tile: impl Fn(&str, &TileData) -> Vec<u8> + Send + Sync + 'static,
) -> Option<PremultipliedImage> {
    render_with_altitude(style, tile).map(|(image, _)| image)
}

/// [`render`]'s still image, with the altitude of the center it drew, in
/// meters.
fn render_with_altitude(
    style: &str,
    tile: impl Fn(&str, &TileData) -> Vec<u8> + Send + Sync + 'static,
) -> Option<(PremultipliedImage, f64)> {
    let (image, map, _) = still(style, tile)?;
    Some((image, map.transform_state().view().center_altitude()))
}

/// [`render`]'s still image, with the map and the frontend that drew it.
fn still(
    style: &str,
    tile: impl Fn(&str, &TileData) -> Vec<u8> + Send + Sync + 'static,
) -> Option<(PremultipliedImage, Map, HeadlessFrontend)> {
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
    file_source.set_response(Kind::Image, |_: &Resource| {
        let mut image = PremultipliedImage::new(Size::new(8, 8));
        for pixel in image.data.as_chunks_mut::<4>().0 {
            *pixel = [0, 0, 255, 255];
        }
        let mut response = Response::new();
        response.data = Some(Arc::new(mln_engine::image::encode_png(&image)));
        Some(response)
    });
    let options = MapOptions { map_mode: MapMode::Static, size, pixel_ratio: 1.0, ..Default::default() };
    let mut map = Map::with_file_source(file_source, options, mln_builtins::registry(), &pool).unwrap();
    frontend.attach(&mut map);
    map.set_bounds(&BoundOptions { max_pitch: Some(85.0), ..Default::default() }).unwrap();
    map.load_style_json(style);
    let camera = mln_engine::map::style_camera(map.style());
    map.jump_to(&camera);
    let image = frontend.render(&mut map).expect("the still image").image;
    Some((image, map, frontend))
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
/// level drapes its squares alike on the globe and on Mercator: each square
/// lands within a pixel, and the pixels that differ visibly are few, along
/// edges. The square on the crest draws higher on screen than it would at
/// sea level, where the ridge's slope hides part of it.
#[test]
fn a_ridge_at_zoom_14_drapes_alike_on_the_globe_and_mercator() {
    let tiles = |url: &str, tile: &TileData| if url.starts_with("dem/") { ridge_png(tile) } else { squares_png(tile) };
    let (Some(globe), Some(mercator)) = (render(&style(true), tiles), render(&style(false), tiles)) else {
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

    // Over level ground, the square on the crest draws lower on screen.
    let flat =
        |url: &str, tile: &TileData| if url.starts_with("dem/") { sea_level_png(tile) } else { squares_png(tile) };
    let Some(level) = render(&style(true), flat) else {
        return;
    };
    let ((raised, _), (lowered, _)) = (centroid(&globe, SQUARES[0].0), centroid(&level, SQUARES[0].0));
    assert!(raised.1 + 5.0 < lowered.1, "the crest's square is at {raised:?} raised, {lowered:?} at sea level");
}

/// A raster tile of one color: red for the western half of the world,
/// blue for the eastern.
fn half_png(tile: &TileData) -> Vec<u8> {
    let east = f64::from(tile.x) + 0.5 >= 2f64.powi(i32::from(tile.z)) / 2.0;
    let mut image = PremultipliedImage::new(Size::new(512, 512));
    for pixel in image.data.as_chunks_mut::<4>().0 {
        *pixel = if east { [0, 0, 255, 255] } else { [255, 0, 0, 255] };
    }
    mln_engine::image::encode_png(&image)
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
        let Some(image) = render(&style(globe), tiles) else {
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

/// The DEM tile `_tile` of a plateau 4,000 m high everywhere, in the Mapbox
/// encoding.
fn plateau_png(_tile: &TileData) -> Vec<u8> {
    let value = ((PLATEAU + 10000.0) * 10.0).round() as u32;
    let mut image = PremultipliedImage::new(Size::new(512, 512));
    for pixel in image.data.as_chunks_mut::<4>().0 {
        *pixel = [(value >> 16) as u8, (value >> 8) as u8, value as u8, 255];
    }
    mln_engine::image::encode_png(&image)
}

/// The plateau's height, in meters: above the eye of a camera at zoom 13,
/// pitched 60° over 256 px, that looked at sea level.
const PLATEAU: f64 = 4000.0;

/// A raster tile of green.
fn green_png(_tile: &TileData) -> Vec<u8> {
    let mut image = PremultipliedImage::new(Size::new(256, 256));
    for pixel in image.data.as_chunks_mut::<4>().0 {
        *pixel = [0, 255, 0, 255];
    }
    mln_engine::image::encode_png(&image)
}

/// Under `vertical-perspective` at zoom 13, a camera pitched over a 4,000 m
/// plateau in the Alps stands on it at rest, as on Mercator, and draws the
/// ground draped green to the bottom of the view. Looking at sea level,
/// its eye would be inside the plateau, and the view would show nothing.
#[test]
fn a_globe_camera_at_zoom_13_stands_on_a_plateau() {
    let style = r#"{
        "version": 8,
        "center": [11.5, 47.3],
        "zoom": 13,
        "pitch": 60,
        "projection": {"type": "vertical-perspective"},
        "terrain": {"source": "dem"},
        "sources": {
            "dem": {"type": "raster-dem", "tiles": ["dem/{z}/{x}/{y}.png"], "tileSize": 512, "maxzoom": 12},
            "raster": {"type": "raster", "tiles": ["raster/{z}/{x}/{y}.png"], "tileSize": 256}
        },
        "layers": [
            {"id": "background", "type": "background", "paint": {"background-color": "white"}},
            {"id": "raster", "type": "raster", "source": "raster", "paint": {"raster-fade-duration": 0}}
        ]
    }"#;
    let tiles = |url: &str, tile: &TileData| if url.starts_with("dem/") { plateau_png(tile) } else { green_png(tile) };
    let Some((image, altitude)) = render_with_altitude(style, tiles) else {
        return;
    };
    for y in [SIDE / 2, SIDE - 1] {
        let start = ((y * SIDE + SIDE / 2) * 4) as usize;
        assert_eq!([0, 255, 0, 255], image.data[start..start + 4], "the ground at row {y}");
    }
    assert!((altitude - PLATEAU).abs() < 1.0, "the center stands {altitude} m up");
}

/// A map at zoom 14 pitched 55° over the ridge, on the globe if `globe`: a
/// red square 80 m across on the crest, a fill, which drapes over the
/// ground, and a blue circle south of the ridge, which draws flat, at sea
/// level.
fn query_style(globe: bool) -> String {
    let projection = if globe { r#""projection": {"type": "vertical-perspective"},"# } else { "" };
    let (lon, lat) = CENTER;
    let ring: Vec<String> = [(140.0, -40.0), (140.0, 40.0), (220.0, 40.0), (220.0, -40.0), (140.0, -40.0)]
        .iter()
        .map(|&(north, east)| {
            let (lon, lat) = offset(north, east);
            format!("[{lon}, {lat}]")
        })
        .collect();
    let (circle_lon, circle_lat) = offset(-150.0, 0.0);
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
                "square": {{"type": "geojson", "data": {{"type": "Polygon", "coordinates": [[{}]]}}}},
                "circle": {{"type": "geojson", "data": {{"type": "Point", "coordinates": [{circle_lon}, {circle_lat}]}}}}
            }},
            "layers": [
                {{"id": "background", "type": "background", "paint": {{"background-color": "white"}}}},
                {{"id": "square", "type": "fill", "source": "square", "paint": {{"fill-color": "rgb(255, 0, 0)"}}}},
                {{"id": "circle", "type": "circle", "source": "circle",
                  "paint": {{"circle-color": "rgb(0, 0, 255)", "circle-radius": 6}}}}
            ]
        }}"#,
        ring.join(", "),
    )
}

/// Over a 400 m ridge, on the globe as on Mercator, a rendered query finds
/// a fill draped on the crest at the pixels where it draws, and nothing a
/// few pixels above its top edge, where the line of sight passes over it to
/// the ground beyond; at sea level under those pixels lie the slope beyond
/// the crest. A circle, which draws flat over the terrain, is found where it
/// draws too.
#[test]
fn a_rendered_query_over_terrain_finds_a_draped_fill_where_it_draws() {
    use mln_engine::geo::ScreenCoordinate;
    use mln_engine::query::RenderedQueryOptions;

    for globe in [true, false] {
        let Some((image, _, mut frontend)) = still(&query_style(globe), |_, tile| ridge_png(tile)) else {
            return;
        };
        let mut found = |layer: &str, x: f64, y: f64| {
            let options = RenderedQueryOptions::new(Some(vec![layer.to_owned()]), None);
            frontend.renderer().query_rendered_features_at_point(&ScreenCoordinate::new(x, y), &options).unwrap().len()
        };
        let ((x, y), count) = centroid(&image, [255, 0, 0]);
        assert!(count > 20, "globe {globe}: the square shows: {count} px");
        let top = (0..SIDE as usize)
            .find(|&row| {
                let pixel = image.data.as_chunks::<4>().0[row * SIDE as usize + x.round() as usize];
                pixel[0] > 200 && pixel[1] < 64 && pixel[2] < 64
            })
            .expect("the square's top edge") as f64;
        assert_eq!(1, found("square", x, y), "globe {globe}: the square where it draws, at {x}, {y}");
        assert_eq!(0, found("square", x, top - 4.0), "globe {globe}: nothing above the square's top edge at {top}");

        let ((x, y), count) = centroid(&image, [0, 0, 255]);
        assert!(count > 20, "globe {globe}: the circle shows: {count} px");
        assert_eq!(1, found("circle", x, y), "globe {globe}: the circle where it draws, at {x}, {y}");
    }
}

/// The DEM tile `tile` of sea level with a block 1000 m high in each corner
/// of the view, more than 300 m north or south and east or west of the
/// center, in the Mapbox encoding.
fn corner_blocks_png(tile: &TileData) -> Vec<u8> {
    tile_png(tile, 256, |north, east| {
        let height: f64 = if north.abs() > 300.0 && east.abs() > 300.0 { 1000.0 } else { 0.0 };
        let value = ((height + 10000.0) * 10.0).round() as u32;
        [(value >> 16) as u8, (value >> 8) as u8, value as u8, 255]
    })
}

/// A box query over uneven terrain finds a fill drawn inside the box, on the
/// globe as on Mercator: seen from straight above, raised blocks under the
/// box's corners meet its corners' lines of sight nearer the center than
/// the sea level beside them, where a small fill near the middle of the
/// box's east edge draws.
#[test]
fn a_box_query_over_uneven_terrain_finds_a_fill_inside_it() {
    use mln_engine::geo::ScreenCoordinate;
    use mln_engine::query::RenderedQueryOptions;

    for globe in [true, false] {
        let ring: Vec<String> = [(-20.0, 480.0), (-20.0, 520.0), (20.0, 520.0), (20.0, 480.0), (-20.0, 480.0)]
            .iter()
            .map(|&(north, east)| {
                let (lon, lat) = offset(north, east);
                format!("[{lon}, {lat}]")
            })
            .collect();
        let projection = if globe { r#""projection": {"type": "vertical-perspective"},"# } else { "" };
        let (lon, lat) = CENTER;
        let style = format!(
            r#"{{
                "version": 8,
                "center": [{lon}, {lat}],
                "zoom": 14,
                {projection}
                "terrain": {{"source": "dem"}},
                "sources": {{
                    "dem": {{"type": "raster-dem", "tiles": ["dem/{{z}}/{{x}}/{{y}}.png"], "tileSize": 256, "maxzoom": 15}},
                    "square": {{"type": "geojson", "data": {{"type": "Polygon", "coordinates": [[{}]]}}}}
                }},
                "layers": [
                    {{"id": "background", "type": "background", "paint": {{"background-color": "white"}}}},
                    {{"id": "square", "type": "fill", "source": "square", "paint": {{"fill-color": "rgb(255, 0, 0)"}}}}
                ]
            }}"#,
            ring.join(", "),
        );
        let Some((image, _, mut frontend)) = still(&style, |_, tile| corner_blocks_png(tile)) else {
            return;
        };
        let ((x, y), count) = centroid(&image, [255, 0, 0]);
        assert!(count > 10, "globe {globe}: the fill shows: {count} px");
        let (min, max) = (8.0, f64::from(SIDE) - 8.0);
        assert!(x < max && y > min && y < max, "globe {globe}: the fill draws inside the box, at {x}, {y}");
        let options = RenderedQueryOptions::new(Some(vec!["square".to_owned()]), None);
        let found = frontend
            .renderer()
            .query_rendered_features_in_box(
                &ScreenCoordinate::new(min, min),
                &ScreenCoordinate::new(max, max),
                &options,
            )
            .unwrap();
        assert_eq!(1, found.len(), "globe {globe}: the box finds the fill at {x}, {y}");
    }
}

/// On the globe, an image whose quad crosses the antimeridian drapes over
/// the terrain on both sides of it, as it draws without terrain.
#[test]
fn an_image_drapes_on_both_sides_of_the_antimeridian() {
    let style = |terrain: bool, center: f64| {
        let terrain = if terrain { r#""terrain": {"source": "dem"},"# } else { "" };
        format!(
            r#"{{
                "version": 8,
                "center": [{center}, 0],
                "zoom": 1,
                "projection": {{"type": "vertical-perspective"}},
                {terrain}
                "sources": {{
                    "dem": {{"type": "raster-dem", "tiles": ["dem/{{z}}/{{x}}/{{y}}.png"], "tileSize": 512, "maxzoom": 15}},
                    "image": {{"type": "image", "url": "image.png",
                        "coordinates": [[-210, 40], [-100, 40], [-100, -40], [-210, -40]]}}
                }},
                "layers": [
                    {{"id": "background", "type": "background", "paint": {{"background-color": "white"}}}},
                    {{"id": "image", "type": "raster", "source": "image", "paint": {{"raster-fade-duration": 0}}}}
                ]
            }}"#
        )
    };
    let tiles = |_: &str, tile: &TileData| sea_level_png(tile);
    let blue = |image: &PremultipliedImage| {
        image.data.as_chunks::<4>().0.iter().filter(|[r, _, b, _]| *b > 200 && *r < 64).count()
    };
    // Centers on either side of the antimeridian, where the drapes of the
    // tiles west of it are in another world copy than those east of it.
    for center in [180.0, 155.0, -155.0, 120.0] {
        let (Some(draped), Some(flat)) = (render(&style(true, center), tiles), render(&style(false, center), tiles))
        else {
            return;
        };
        // The surface filters its drapes, so the image's edges differ, by
        // about 1% on a software rasterizer. Without its copy in the next
        // world, the image loses about 40%.
        let (draped, flat) = (blue(&draped), blue(&flat));
        assert!(
            flat > 0 && draped.abs_diff(flat) * 20 <= flat,
            "{draped} blue pixels draped, {flat} without terrain, at {center}"
        );
    }
}
