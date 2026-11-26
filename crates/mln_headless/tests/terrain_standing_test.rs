// The layers that stand on terrain: circles, heatmap kernels and symbols
// stand on the ground at their points, and circles and symbols hide behind
// the ground in front of them.

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

/// The pixels of a DEM tile a side.
const DEM_SIDE: u32 = 256;

/// The texel of a height of `meters` in the Mapbox encoding:
/// `(r · 65536 + g · 256 + b) · 0.1 − 10000`.
fn mapbox(meters: f64) -> [u8; 4] {
    let value = ((meters + 10000.0) * 10.0).round() as u32;
    [(value >> 16) as u8, (value >> 8) as u8, value as u8, 255]
}

/// The DEM tile `z/x/y` of the ground `height`, the height in meters at a
/// longitude and latitude, sampled at its pixels' centers.
fn dem_tile(z: u8, x: u32, y: u32, height: &dyn Fn(f64, f64) -> f64) -> Vec<u8> {
    let mut image = PremultipliedImage::new(Size::new(DEM_SIDE, DEM_SIDE));
    let tiles = f64::from(1u32 << z);
    for (index, texel) in image.data.as_chunks_mut::<4>().0.iter_mut().enumerate() {
        let (px, py) = (f64::from(index as u32 % DEM_SIDE) + 0.5, f64::from(index as u32 / DEM_SIDE) + 0.5);
        let (wx, wy) =
            ((f64::from(x) + px / f64::from(DEM_SIDE)) / tiles, (f64::from(y) + py / f64::from(DEM_SIDE)) / tiles);
        let lng = wx * 360.0 - 180.0;
        let lat = (std::f64::consts::PI * (1.0 - 2.0 * wy)).sinh().atan().to_degrees();
        *texel = mapbox(height(lng, lat));
    }
    mln_image::encode_png(&image)
}

/// The `z`, `x` and `y` of a DEM tile's URL, `dem/{z}/{x}/{y}.png`.
fn tile_of(url: &str) -> Option<(u8, u32, u32)> {
    let mut parts = url.strip_prefix("dem/")?.strip_suffix(".png")?.split('/');
    Some((parts.next()?.parse().ok()?, parts.next()?.parse().ok()?, parts.next()?.parse().ok()?))
}

/// The members of a style seen from `camera`, the center's longitude and
/// latitude, the zoom and the pitch, in `projection`, with a terrain if
/// `terrain`, a white background, and `layers` of the GeoJSON source
/// `points`, whose points are `points`.
fn style(terrain: bool, projection: &str, camera: [f64; 4], points: &[[f64; 2]], layers: &str) -> String {
    let terrain = if terrain { r#""terrain": {"source": "dem"},"# } else { "" };
    let [lng, lat, zoom, pitch] = camera;
    let points: Vec<String> = points.iter().map(|[lng, lat]| format!("[{lng}, {lat}]")).collect();
    format!(
        r#"{{
            "version": 8,
            "center": [{lng}, {lat}],
            "zoom": {zoom},
            "pitch": {pitch},
            "projection": {{"type": "{projection}"}},
            {terrain}
            "sources": {{
                "dem": {{"type": "raster-dem", "tiles": ["dem/{{z}}/{{x}}/{{y}}.png"], "tileSize": 256, "maxzoom": 14}},
                "points": {{"type": "geojson", "data": {{"type": "MultiPoint", "coordinates": [{}]}}}}
            }},
            "layers": [
                {{"id": "background", "type": "background", "paint": {{"background-color": "white"}}}},
                {layers}
            ]
        }}"#,
        points.join(", ")
    )
}

const CIRCLES: &str =
    r#"{"id": "circles", "type": "circle", "source": "points", "paint": {"circle-color": "lime", "circle-radius": 6}}"#;

const HEATMAP: &str = r#"{"id": "heat", "type": "heatmap", "source": "points", "paint": {"heatmap-radius": 20}}"#;

/// Lime icons ([`LIME_ICON`]) that always show.
const ICONS: &str = r#"{"id": "icons", "type": "symbol", "source": "points",
    "layout": {"icon-image": "lime", "icon-allow-overlap": true, "icon-ignore-placement": true}}"#;

/// The style image `lime`: a lime square 12 px a side.
const LIME_ICON: &str = "lime";

/// The still image of `style` on the harness's device
/// ([`mln_headless::harness_device`]), over the ground `height`; `None`
/// where the machine has no such device.
fn render(style: &str, height: impl Fn(f64, f64) -> f64 + Send + Sync + 'static) -> Option<PremultipliedImage> {
    still(style, height).map(|(image, _)| image)
}

/// [`render`]'s still image, and the frontend that drew it.
fn still(
    style: &str,
    height: impl Fn(f64, f64) -> f64 + Send + Sync + 'static,
) -> Option<(PremultipliedImage, HeadlessFrontend)> {
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
        let (z, x, y) = tile_of(&resource.url)?;
        let mut response = Response::new();
        response.data = Some(Arc::new(dem_tile(z, x, y, &height)));
        Some(response)
    });
    file_source
        .set_response(Kind::Glyphs, |_| Some(crate::support::file_response("test/fixtures/resources/glyphs.pbf")));
    let options = MapOptions { map_mode: MapMode::Static, size, pixel_ratio: 1.0, ..Default::default() };
    let mut map = Map::with_file_source(file_source, options, &pool).unwrap();
    frontend.attach(&mut map);
    map.load_style_json(style);
    let mut icon = PremultipliedImage::new(Size::new(12, 12));
    for texel in icon.data.as_chunks_mut::<4>().0 {
        *texel = [0, 255, 0, 255];
    }
    let icon = crate::gtest_api::style_image(LIME_ICON, icon, 1.0);
    map.edit_style(|style| style.apply(mln_style::Edit::AddImage(icon))).expect("the icon");
    let camera = mln_engine::map::style_camera(map.style());
    map.jump_to(&camera);
    let image = frontend.render(&mut map).expect("the still image").image;
    Some((image, frontend))
}

/// How many pixels of `a` and `b` differ, as the render tests count them.
fn differing(a: &PremultipliedImage, b: &PremultipliedImage) -> u64 {
    pixelmatch::pixelmatch(&a.data, &b.data, SIDE as usize, SIDE as usize, None, 0.1285, false)
}

/// How many pixels of `image` are lime, a circle's.
fn lime(image: &PremultipliedImage) -> usize {
    image.data.as_chunks::<4>().0.iter().filter(|[r, g, b, _]| *g > 192 && *r < 96 && *b < 96).count()
}

/// How many pixels of `image` aren't white: a heatmap's.
fn heat(image: &PremultipliedImage) -> usize {
    image.data.as_chunks::<4>().0.iter().filter(|&&[r, g, b, _]| r.min(g).min(b) < 224).count()
}

/// Circles over a plateau stand on it: the camera stands on it too, so the
/// map looks as it does without terrain, on Mercator and on the globe. At
/// sea level, they would show elsewhere, or hide under the plateau.
#[test]
fn circles_stand_on_a_plateau() {
    let cases = [("mercator", [0.0, 0.0, 12.0, 60.0], 0.01, 2000.0), ("globe", [0.0, 0.0, 6.0, 50.0], 0.5, 40000.0)];
    for (projection, camera, spread, plateau) in cases {
        let points = [[0.0, 0.0], [spread, spread], [-spread, spread * 0.5], [spread * 0.5, -spread * 0.5]];
        let (Some(standing), Some(flat)) = (
            render(&style(true, projection, camera, &points, CIRCLES), move |_, _| plateau),
            render(&style(false, projection, camera, &points, CIRCLES), |_, _| 0.0),
        ) else {
            return;
        };
        assert!(lime(&flat) > 4 * 64, "{projection}: the circles without terrain");
        let differing = differing(&standing, &flat);
        assert!(differing <= 64, "{projection}: {differing} pixels differ from the map without terrain");
    }
}

/// Circles of a source whose tiles go no deeper than zoom 5 stand on a
/// plateau under terrain tiles at zoom 9: they stand on the DEM tiles at
/// zoom 5 that hold their tiles, which the terrain also loads to cull its
/// tiles, so the map looks as it does without terrain.
#[test]
fn circles_of_a_coarse_source_stand_on_a_plateau() {
    let camera = [0.0, 0.0, 9.0, 0.0];
    let points = [[0.05, 0.05], [-0.05, 0.03], [0.03, -0.05]];
    let coarse = |terrain| {
        style(terrain, "mercator", camera, &points, CIRCLES).replacen(
            r#""type": "geojson", "data""#,
            r#""type": "geojson", "maxzoom": 5, "data""#,
            1,
        )
    };
    let (Some(standing), Some(flat)) = (render(&coarse(true), |_, _| 2000.0), render(&coarse(false), |_, _| 0.0))
    else {
        return;
    };
    assert!(lime(&flat) > 3 * 64, "the circles without terrain: {} px", lime(&flat));
    let differing = differing(&standing, &flat);
    assert!(differing <= 64, "{differing} pixels differ from the map without terrain");
}

/// Ground 2000 m high from 0.004° to 0.008° north, a ridge across the view
/// north of the center, which a camera pitched 75° looks over.
fn ridge(_: f64, lat: f64) -> f64 {
    if (0.004..0.008).contains(&lat) { 2000.0 } else { 0.0 }
}

/// The camera at the equator at zoom 12, pitched 75°, looking north.
const RIDGE_CAMERA: [f64; 4] = [0.0, 0.0, 12.0, 75.0];

/// A circle north of a ridge hides behind it, where without the ridge it
/// shows; one south of it, in front, shows either way.
#[test]
fn a_circle_hides_behind_a_ridge() {
    let behind = [[0.0, 0.016]];
    let in_front = [[0.0, -0.002]];
    let (Some(hidden), Some(shown), Some(front)) = (
        render(&style(true, "mercator", RIDGE_CAMERA, &behind, CIRCLES), ridge),
        render(&style(true, "mercator", RIDGE_CAMERA, &behind, CIRCLES), |_, _| 0.0),
        render(&style(true, "mercator", RIDGE_CAMERA, &in_front, CIRCLES), ridge),
    ) else {
        return;
    };
    assert!(lime(&shown) > 16, "the circle north of flat ground: {} px", lime(&shown));
    assert_eq!(0, lime(&hidden), "the circle behind the ridge");
    assert!(lime(&front) > 16, "the circle in front of the ridge: {} px", lime(&front));
}

/// A circle a terrain tile north of a ridge hides behind it, as it does in
/// the ridge's own tile: the terrain tiles draw the ground's depth nearest
/// the view's center first, the ridge's tile before the circle's, and the
/// ridge stays in front where both draw.
#[test]
fn a_circle_hides_behind_a_ridge_in_a_nearer_tile() {
    // A ridge north of the view's center, the circle in the terrain tile north of
    // its tile at zoom 12, which ends at 0.0879° north.
    let ridge = |_: f64, lat: f64| if (0.04..0.05).contains(&lat) { 2000.0 } else { 0.0 };
    let camera = [0.0, 0.02, 12.0, 75.0];
    let behind = [[0.0, 0.1]];
    let (Some(hidden), Some(shown)) = (
        render(&style(true, "mercator", camera, &behind, CIRCLES), ridge),
        render(&style(true, "mercator", camera, &behind, CIRCLES), |_, _| 0.0),
    ) else {
        return;
    };
    assert!(lime(&shown) > 16, "the circle north of flat ground: {} px", lime(&shown));
    assert_eq!(0, lime(&hidden), "the circle behind the ridge");
}

/// A heatmap's kernel north of a ridge shows over it, as GL JS's does: a
/// heatmap's density adds up on the screen, which the ground doesn't hide.
#[test]
fn a_heatmap_kernel_shows_behind_a_ridge() {
    let behind = [[0.0, 0.016]];
    let Some(image) = render(&style(true, "mercator", RIDGE_CAMERA, &behind, HEATMAP), ridge) else {
        return;
    };
    assert!(heat(&image) > 16, "the kernel behind the ridge: {} px", heat(&image));
}

/// The longitude of the edge between two terrain tiles at zoom 13, inside a
/// tile at zoom 10.
const EDGE: f64 = 360.0 / 8192.0;

/// The members of a style of two circles either side of the edge between
/// two terrain tiles at zoom 13, seen from above: a red one west of it,
/// sorted above a blue one east of it, which it overlaps. Their source's
/// tiles go no deeper than zoom 10, so that one holds both, and the terrain
/// tiles, which read DEM tiles of their own.
fn sorted_circles(terrain: bool) -> String {
    let terrain = if terrain { r#""terrain": {"source": "dem"},"# } else { "" };
    let circle = |lng: f64, key: u32, color: &str| {
        format!(
            r#"{{"type": "Feature", "geometry": {{"type": "Point", "coordinates": [{lng}, -0.001]}},
                "properties": {{"key": {key}, "color": "{color}"}}}}"#
        )
    };
    format!(
        r#"{{
            "version": 8,
            "center": [0.04, -0.001],
            "zoom": 13,
            {terrain}
            "sources": {{
                "dem": {{"type": "raster-dem", "tiles": ["dem/{{z}}/{{x}}/{{y}}.png"], "tileSize": 256, "maxzoom": 14}},
                "points": {{"type": "geojson", "maxzoom": 10, "data": {{"type": "FeatureCollection", "features": [{}, {}]}}}}
            }},
            "layers": [
                {{"id": "background", "type": "background", "paint": {{"background-color": "white"}}}},
                {{"id": "circles", "type": "circle", "source": "points", "layout": {{"circle-sort-key": ["get", "key"]}},
                  "paint": {{"circle-color": ["get", "color"], "circle-radius": 16}}}}
            ]
        }}"#,
        circle(EDGE - 0.0008, 2, "red"),
        circle(EDGE + 0.0008, 1, "blue"),
    )
}

/// Circles keep the order their sort keys give them over terrain, on a
/// plateau the camera stands on, where they stand on terrain tiles that
/// read different DEM tiles: the map looks as it does without terrain.
#[test]
fn circles_keep_their_order_across_terrain_tiles() {
    let (Some(standing), Some(flat)) =
        (render(&sorted_circles(true), |_, _| 2000.0), render(&sorted_circles(false), |_, _| 0.0))
    else {
        return;
    };
    let red = |image: &PremultipliedImage| {
        image.data.as_chunks::<4>().0.iter().filter(|[r, g, b, _]| *r > 192 && *g < 96 && *b < 96).count()
    };
    assert!(red(&flat) > 400, "the red circle, whole, on top: {} px", red(&flat));
    let differing = differing(&standing, &flat);
    assert!(differing <= 64, "{differing} pixels differ from the map without terrain");
}

/// Icons over a plateau stand on it, placed where they draw: the map looks
/// as it does without terrain, on Mercator and on the globe. At sea level,
/// they would show elsewhere, or hide under the plateau.
#[test]
fn icons_stand_on_a_plateau() {
    let cases = [("mercator", [0.0, 0.0, 12.0, 60.0], 0.01, 2000.0), ("globe", [0.0, 0.0, 6.0, 50.0], 0.5, 40000.0)];
    for (projection, camera, spread, plateau) in cases {
        let points = [[0.0, 0.0], [spread, spread], [-spread, spread * 0.5], [spread * 0.5, -spread * 0.5]];
        let (Some(standing), Some(flat)) = (
            render(&style(true, projection, camera, &points, ICONS), move |_, _| plateau),
            render(&style(false, projection, camera, &points, ICONS), |_, _| 0.0),
        ) else {
            return;
        };
        assert!(lime(&flat) > 4 * 64, "{projection}: the icons without terrain: {} px", lime(&flat));
        let differing = differing(&standing, &flat);
        assert!(differing <= 64, "{projection}: {differing} pixels differ from the map without terrain");
    }
}

/// An icon north of a ridge hides behind it, where without the ridge it
/// shows; one south of it, in front, shows either way.
#[test]
fn an_icon_hides_behind_a_ridge() {
    let behind = [[0.0, 0.016]];
    let in_front = [[0.0, -0.002]];
    let (Some(hidden), Some(shown), Some(front)) = (
        render(&style(true, "mercator", RIDGE_CAMERA, &behind, ICONS), ridge),
        render(&style(true, "mercator", RIDGE_CAMERA, &behind, ICONS), |_, _| 0.0),
        render(&style(true, "mercator", RIDGE_CAMERA, &in_front, ICONS), ridge),
    ) else {
        return;
    };
    assert!(lime(&shown) > 16, "the icon north of flat ground: {} px", lime(&shown));
    assert_eq!(0, lime(&hidden), "the icon behind the ridge");
    assert!(lime(&front) > 16, "the icon in front of the ridge: {} px", lime(&front));
}

/// The lime pixels of `image`, and their center, `None` for none.
fn lime_center(image: &PremultipliedImage) -> (usize, Option<(f64, f64)>) {
    let lime: Vec<(usize, usize)> = (0..SIDE as usize * SIDE as usize)
        .map(|index| (index % SIDE as usize, index / SIDE as usize))
        .filter(|&(x, y)| {
            let [r, g, b, _] = image.data.as_chunks::<4>().0[y * SIDE as usize + x];
            g > 192 && r < 96 && b < 96
        })
        .collect();
    let (x, y) = lime.iter().fold((0.0, 0.0), |(x, y), &(px, py)| (x + px as f64, y + py as f64));
    let count = lime.len();
    (count, (count > 0).then(|| (x / count as f64, y / count as f64)))
}

/// A rendered query over terrain finds an icon that stands on a block of
/// high ground north of the center, where it draws, and nothing where it
/// would draw at sea level, on Mercator and on the globe. The camera stands
/// on the low ground at the center, so the block raises the icon in the
/// view.
#[test]
fn a_rendered_query_finds_a_standing_icon_where_it_draws() {
    use mln_engine::query::RenderedQueryOptions;
    use mln_util::geo::ScreenCoordinate;

    let cases = [("mercator", [0.0, 0.0, 12.0, 60.0], 0.01, 1000.0), ("globe", [0.0, 0.0, 6.0, 50.0], 0.5, 20000.0)];
    for (projection, camera, north, block) in cases {
        let points = [[0.0, north]];
        let ground = move |lng: f64, lat: f64| {
            let near = lng.abs() < north / 2.0 && (lat - north).abs() < north / 2.0;
            if near { block } else { 0.0 }
        };
        let (Some((image, mut frontend)), Some(flat)) = (
            still(&style(true, projection, camera, &points, ICONS), ground),
            render(&style(true, projection, camera, &points, ICONS), |_, _| 0.0),
        ) else {
            return;
        };
        let (count, Some((x, y))) = lime_center(&image) else {
            panic!("{projection}: the icon shows");
        };
        assert!(count > 16, "{projection}: the icon shows: {count} px");
        let options = RenderedQueryOptions::new(Some(vec!["icons".to_owned()]), None);
        let mut query = |x: f64, y: f64| {
            frontend.renderer().query_rendered_features_at_point(&ScreenCoordinate::new(x, y), &options).unwrap().len()
        };
        assert_eq!(1, query(x, y), "{projection}: the icon where it draws, at {x}, {y}");
        let (_, Some((sea_x, sea_y))) = lime_center(&flat) else {
            panic!("{projection}: the icon shows on flat ground");
        };
        assert!(sea_y - y > 12.0, "{projection}: the block raises the icon from {sea_y} to {y}");
        assert_eq!(0, query(sea_x, sea_y), "{projection}: no icon where it would draw at sea level");
    }
}

/// A style of `style`'s members whose source `points` holds a point at
/// each longitude and latitude of `points`, its property `k` its index, and
/// whose lime icons have the layout properties `layout` too.
fn keyed_icons_style(camera: [f64; 4], points: &[[f64; 2]], layout: &str) -> String {
    let features: Vec<String> = points
        .iter()
        .enumerate()
        .map(|(k, [lng, lat])| {
            format!(
                r#"{{"type": "Feature", "properties": {{"k": {k}}},
                    "geometry": {{"type": "Point", "coordinates": [{lng}, {lat}]}}}}"#
            )
        })
        .collect();
    let layer = format!(
        r#"{{"id": "icons", "type": "symbol", "source": "keyed",
            "layout": {{"icon-image": "lime", "icon-allow-overlap": true, "icon-ignore-placement": true{layout}}}}}"#
    );
    let source = format!(
        r#""sources": {{
            "keyed": {{"type": "geojson", "data": {{"type": "FeatureCollection", "features": [{}]}}}},"#,
        features.join(", ")
    );
    style(true, "mercator", camera, &[], &layer).replacen(r#""sources": {"#, &source, 1)
}

/// Ground 2000 m high around 0.01° north, from 0.006° to 0.014° north and
/// 0.004° either side of the meridian, and sea level elsewhere.
fn block(lng: f64, lat: f64) -> f64 {
    if lng.abs() < 0.004 && (0.006..0.014).contains(&lat) { 2000.0 } else { 0.0 }
}

/// Icons that sort by `symbol-sort-key` into segments of their own, one on
/// a block of high ground and one beside it, each stand on the ground under
/// their own anchor: the map looks as it does with the icons in one
/// segment.
#[test]
fn icons_in_segments_of_their_own_stand_on_their_own_ground() {
    let camera = [0.0, 0.0, 12.0, 60.0];
    // Both in the tile north-east of the center, at zoom 12.
    let points = [[0.002, 0.01], [0.008, 0.002]];
    let (Some(sorted), Some(unsorted), Some(flat)) = (
        render(&keyed_icons_style(camera, &points, r#", "symbol-sort-key": ["-", ["get", "k"]]"#), block),
        render(&keyed_icons_style(camera, &points, ""), block),
        render(&keyed_icons_style(camera, &points, ""), |_, _| 0.0),
    ) else {
        return;
    };
    assert_eq!(2, lime_blobs(&unsorted), "both icons show");
    assert!(differing(&unsorted, &flat) > 64, "the block raises an icon");
    let differing = differing(&sorted, &unsorted);
    assert!(differing <= 8, "{differing} pixels differ from the icons in one segment");
}

/// How many separate runs of lime rows `image` has, top to bottom: the
/// icons of [`icons_in_segments_of_their_own_stand_on_their_own_ground`],
/// which stand at different heights in the view.
fn lime_blobs(image: &PremultipliedImage) -> usize {
    let side = SIDE as usize;
    let rows: Vec<bool> = (0..side)
        .map(|y| {
            image.data.as_chunks::<4>().0[y * side..(y + 1) * side]
                .iter()
                .any(|[r, g, b, _]| *g > 192 && *r < 96 && *b < 96)
        })
        .collect();
    rows.windows(2).filter(|pair| pair[1] && !pair[0]).count() + usize::from(rows[0])
}

/// An icon on a block of high ground, moved by `icon-translate` off the
/// block in the view, stands at the height of the ground under its anchor,
/// as GL JS draws it: a rendered query finds it where it draws.
#[test]
fn a_rendered_query_finds_a_translated_icon_where_it_draws() {
    use mln_engine::query::RenderedQueryOptions;
    use mln_util::geo::ScreenCoordinate;

    let camera = [0.0, 0.0, 12.0, 60.0];
    let style = keyed_icons_style(camera, &[[0.0, 0.01]], r#", "icon-translate": [0, 60]"#);
    let Some((image, mut frontend)) = still(&style, block) else {
        return;
    };
    let (count, Some((x, y))) = lime_center(&image) else {
        panic!("the icon shows");
    };
    assert!(count > 16, "the icon shows: {count} px");
    let options = RenderedQueryOptions::new(Some(vec!["icons".to_owned()]), None);
    let found = frontend.renderer().query_rendered_features_at_point(&ScreenCoordinate::new(x, y), &options);
    assert_eq!(1, found.unwrap().len(), "the icon where it draws, at {x}, {y}");
}

/// A label at a variable anchor, aligned with the viewport and turned with
/// the map, on ground that climbs steeply east through its anchor, shifts
/// along the east the view shows at its anchor's height, as the shaders turn
/// it and placement turns its box: a rendered query finds it where it draws.
#[test]
fn a_rendered_query_finds_a_label_at_a_variable_anchor_on_a_slope_where_it_draws() {
    use mln_engine::query::RenderedQueryOptions;
    use mln_util::geo::ScreenCoordinate;

    let (lng, lat) = (0.0105, 0.004);
    let slope = move |x: f64, _: f64| ((x - lng) * 500_000.0).clamp(-1000.0, 1000.0);
    let layer = r#"{"id": "labels", "type": "symbol", "source": "points",
        "layout": {"text-field": "WWWW", "text-size": 24, "text-variable-anchor": ["left"], "text-radial-offset": 2,
            "text-rotation-alignment": "map", "text-pitch-alignment": "viewport",
            "text-allow-overlap": true, "text-ignore-placement": true},
        "paint": {"text-color": "lime"}}"#;
    let style = style(true, "mercator", [lng, lat, 12.0, 60.0], &[[lng, lat]], layer).replacen(
        r#""sources": {"#,
        r#""glyphs": "glyphs/{fontstack}/{range}.pbf", "sources": {"#,
        1,
    );
    let Some((image, mut frontend)) = still(&style, slope) else {
        return;
    };
    let (count, Some((x, y))) = lime_center(&image) else {
        panic!("the label shows");
    };
    assert!(count > 16, "the label shows: {count} px");
    let options = RenderedQueryOptions::new(Some(vec!["labels".to_owned()]), None);
    let found = frontend.renderer().query_rendered_features_at_point(&ScreenCoordinate::new(x, y), &options);
    assert_eq!(1, found.unwrap().len(), "the label where it draws, at {x}, {y}");
    assert!(x > 128.0 + 24.0 && (y - 128.0).abs() < 24.0, "the label shifts east of its anchor, to {x}, {y}");
}

/// A rendered query over terrain finds a circle that stands on a plateau
/// where it draws, on Mercator and on the globe.
#[test]
fn a_rendered_query_finds_a_standing_circle_where_it_draws() {
    use mln_engine::query::RenderedQueryOptions;
    use mln_util::geo::ScreenCoordinate;

    let cases = [("mercator", [0.0, 0.0, 12.0, 60.0], 2000.0), ("globe", [0.0, 0.0, 6.0, 50.0], 40000.0)];
    for (projection, camera, plateau) in cases {
        let style = style(true, projection, camera, &[[0.0, 0.0]], CIRCLES);
        let Some((image, mut frontend)) = still(&style, move |_, _| plateau) else {
            return;
        };
        let lime: Vec<(usize, usize)> = (0..SIDE as usize * SIDE as usize)
            .map(|index| (index % SIDE as usize, index / SIDE as usize))
            .filter(|&(x, y)| {
                let [r, g, b, _] = image.data.as_chunks::<4>().0[y * SIDE as usize + x];
                g > 192 && r < 96 && b < 96
            })
            .collect();
        assert!(lime.len() > 16, "{projection}: the circle shows: {} px", lime.len());
        let (x, y) = lime.iter().fold((0.0, 0.0), |(x, y), &(px, py)| (x + px as f64, y + py as f64));
        let (x, y) = (x / lime.len() as f64, y / lime.len() as f64);
        let options = RenderedQueryOptions::new(Some(vec!["circles".to_owned()]), None);
        let found = frontend.renderer().query_rendered_features_at_point(&ScreenCoordinate::new(x, y), &options);
        assert_eq!(1, found.unwrap().len(), "{projection}: the circle where it draws, at {x}, {y}");
    }
}

/// A rendered query over a plateau finds a circle aligned with and scaled
/// by the viewport within its radius of its center on the screen, and not
/// past it: its radius is in screen pixels where it stands, not at sea
/// level, farther from the camera.
#[test]
fn a_rendered_query_finds_a_standing_circle_within_its_radius() {
    use mln_engine::query::RenderedQueryOptions;
    use mln_util::geo::ScreenCoordinate;

    const RADIUS: f64 = 20.0;
    let layer = format!(
        r#"{{"id": "circles", "type": "circle", "source": "points", "paint": {{"circle-color": "lime",
            "circle-radius": {RADIUS}, "circle-pitch-alignment": "viewport", "circle-pitch-scale": "viewport"}}}}"#
    );
    let style = style(true, "mercator", [0.0, 0.0, 12.0, 0.0], &[[0.0, 0.0]], &layer);
    let Some((image, mut frontend)) = still(&style, |_, _| 2000.0) else {
        return;
    };
    let center = f64::from(SIDE) / 2.0;
    let [r, g, b, _] = image.data.as_chunks::<4>().0[(SIDE / 2 * SIDE + SIDE / 2) as usize];
    assert!(g > 192 && r < 96 && b < 96, "the circle at the view's center");
    let options = RenderedQueryOptions::new(Some(vec!["circles".to_owned()]), None);
    let mut found = |dx: f64| {
        let point = ScreenCoordinate::new(center + dx, center);
        frontend.renderer().query_rendered_features_at_point(&point, &options).unwrap().len()
    };
    assert_eq!(1, found(RADIUS - 4.0), "inside the circle");
    assert_eq!(0, found(RADIUS + 4.0), "outside the circle");
}

/// A rendered query over a slope finds a circle aligned with the map within
/// its edges on the screen, and not past them: the circle lies flat at the
/// height of the ground under its center, where the query's lines of sight
/// meet its plane, not the slope.
#[test]
fn a_rendered_query_finds_a_circle_on_a_slope_within_its_edges() {
    use mln_engine::query::RenderedQueryOptions;
    use mln_util::geo::ScreenCoordinate;

    let layer = r#"{"id": "circles", "type": "circle", "source": "points", "paint": {"circle-color": "lime",
        "circle-radius": 30, "circle-pitch-alignment": "map", "circle-pitch-scale": "map"}}"#;
    let style = style(true, "mercator", [0.0, 0.0, 14.0, 60.0], &[[0.0, 0.0]], layer);
    // Rising 200 m north every 0.01°, around the view.
    let slope = |_: f64, lat: f64| 1000.0 + lat.clamp(-0.02, 0.02) * 20_000.0;
    let Some((image, mut frontend)) = still(&style, slope) else {
        return;
    };
    let side = SIDE as usize;
    let column = side / 2;
    let rows: Vec<usize> = (0..side)
        .filter(|&y| {
            let [r, g, b, _] = image.data.as_chunks::<4>().0[y * side + column];
            g > 192 && r < 96 && b < 96
        })
        .collect();
    let (Some(&top), Some(&bottom)) = (rows.first(), rows.last()) else {
        panic!("the circle shows at the view's center column");
    };
    let options = RenderedQueryOptions::new(Some(vec!["circles".to_owned()]), None);
    let mut found = |y: usize| {
        let point = ScreenCoordinate::new(column as f64, y as f64);
        frontend.renderer().query_rendered_features_at_point(&point, &options).unwrap().len()
    };
    assert_eq!(1, found(top + 3), "inside the circle's top edge, at row {top}");
    assert_eq!(1, found(bottom - 3), "inside the circle's bottom edge, at row {bottom}");
    assert_eq!(0, found(top - 3), "past the circle's top edge, at row {top}");
    assert_eq!(0, found(bottom + 3), "past the circle's bottom edge, at row {bottom}");
}

/// A style of `style`'s members with a lime extrusion `height` m tall of the
/// square of `side` degrees a side around `center`, the longitude and
/// latitude, over terrain if `terrain`.
fn extrusion_style(
    terrain: bool,
    projection: &str,
    camera: [f64; 4],
    center: [f64; 2],
    side: f64,
    height: f64,
) -> String {
    let [lng, lat] = center;
    let (w, e, s, n) = (lng - side / 2.0, lng + side / 2.0, lat - side / 2.0, lat + side / 2.0);
    let layer = format!(
        r#"{{"id": "box", "type": "fill-extrusion", "source": "box",
            "paint": {{"fill-extrusion-color": "lime", "fill-extrusion-height": {height}}}}}"#
    );
    let source = format!(
        r#""sources": {{
            "box": {{"type": "geojson", "data": {{"type": "Polygon",
                "coordinates": [[[{w}, {s}], [{e}, {s}], [{e}, {n}], [{w}, {n}], [{w}, {s}]]]}}}},"#
    );
    style(terrain, projection, camera, &[], &layer).replacen(r#""sources": {"#, &source, 1)
}

/// An extrusion on a plateau stands on it, its base and roof raised by it:
/// the camera stands on the plateau too, so the map looks as it does without
/// terrain, on Mercator and on the globe. At sea level, the plateau would
/// hide it.
#[test]
fn an_extrusion_stands_on_a_plateau() {
    let cases = [
        ("mercator", [0.0, 0.0, 15.0, 60.0], 0.0005, 30.0, 2000.0),
        ("globe", [0.0, 0.0, 6.0, 50.0], 0.5, 20000.0, 40000.0),
    ];
    for (projection, camera, side, height, plateau) in cases {
        let (Some(standing), Some(flat)) = (
            render(&extrusion_style(true, projection, camera, [0.0, 0.0], side, height), move |_, _| plateau),
            render(&extrusion_style(false, projection, camera, [0.0, 0.0], side, height), |_, _| 0.0),
        ) else {
            return;
        };
        assert!(lime_ish(&flat) > 64, "{projection}: the extrusion without terrain");
        let differing = differing(&standing, &flat);
        assert!(differing <= 64, "{projection}: {differing} pixels differ from the map without terrain");
    }
}

/// An extrusion north of a ridge hides behind it, where without the ridge it
/// shows.
#[test]
fn an_extrusion_hides_behind_a_ridge() {
    let style = extrusion_style(true, "mercator", RIDGE_CAMERA, [0.0, 0.016], 0.002, 50.0);
    let (Some(hidden), Some(shown)) = (render(&style, ridge), render(&style, |_, _| 0.0)) else {
        return;
    };
    assert!(lime_ish(&shown) > 16, "the extrusion north of flat ground: {} px", lime_ish(&shown));
    assert_eq!(0, lime_ish(&hidden), "the extrusion behind the ridge");
}

/// How many pixels of `image` are green, as a lit lime extrusion is.
fn lime_ish(image: &PremultipliedImage) -> usize {
    image
        .data
        .as_chunks::<4>()
        .0
        .iter()
        .filter(|[r, g, b, _]| *g > 96 && *g > r.saturating_add(48) && *g > b.saturating_add(48))
        .count()
}

/// A rendered query finds a circle on a ridge where it shows past the
/// ridge's silhouette, though the query's line of sight there meets the
/// lower ground beyond the ridge, far from the circle: a circle stands
/// raised, so where the query meets the ground doesn't bound where it finds
/// one.
#[test]
fn a_rendered_query_finds_a_circle_where_it_shows_past_a_ridge() {
    use mln_engine::query::RenderedQueryOptions;
    use mln_util::geo::ScreenCoordinate;

    let layer = r#"{"id": "circles", "type": "circle", "source": "points",
        "paint": {"circle-color": "lime", "circle-radius": 20}}"#;
    // On the ridge, near its far edge, so that the circle's upper half shows
    // above the ridge's silhouette.
    let style = style(true, "mercator", [0.0, 0.0, 13.0, 70.0], &[[0.0, 0.0075]], layer);
    let low_ridge = |_: f64, lat: f64| if (0.004..0.008).contains(&lat) { 300.0 } else { 0.0 };
    let Some((image, mut frontend)) = still(&style, low_ridge) else {
        return;
    };
    let side = SIDE as usize;
    let column = side / 2;
    let rows: Vec<usize> = (0..side)
        .filter(|&y| {
            let [r, g, b, _] = image.data.as_chunks::<4>().0[y * side + column];
            g > 192 && r < 96 && b < 96
        })
        .collect();
    let (Some(&top), Some(&bottom)) = (rows.first(), rows.last()) else {
        panic!("the circle shows at the view's center column");
    };
    assert!(top > 3, "the circle's top edge, at row {top}, is in view");
    let options = RenderedQueryOptions::new(Some(vec!["circles".to_owned()]), None);
    let mut found = |y: usize| {
        let point = ScreenCoordinate::new(column as f64, y as f64);
        frontend.renderer().query_rendered_features_at_point(&point, &options).unwrap().len()
    };
    for y in (top + 3)..=(bottom - 3) {
        assert_eq!(1, found(y), "the circle at row {y}, between rows {top} and {bottom}");
    }
    assert_eq!(0, found(top - 3), "past the circle's top edge, at row {top}");
}
