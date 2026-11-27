//! Whether a rendered-feature query finds a layer where it draws: a scene
//! of one layer drawn in one color over a gray map, queried at points of a
//! grid, each of whose answers must match whether the still image shows the
//! layer there.

use std::sync::Arc;

use mln_engine::Registry;
use mln_engine::exec::pool::Pool;
use mln_engine::exec::queue::Queue;
use mln_engine::geo::ScreenCoordinate;
use mln_engine::map::{Map, MapMode, MapOptions};
use mln_engine::query::RenderedQueryOptions;
use mln_engine::storage::stub_file_source::{ResponseType, StubFileSource};
use mln_engine::storage::{Kind, Resource, Response};
use mln_headless::HeadlessFrontend;
use mln_style::bitmap::PremultipliedImage;
use mln_style::error::ErrorKind;
use mln_style::size::Size;

/// The scenes' side, in pixels.
pub const SIDE: u32 = 256;

/// The color a scene's layer draws in, which tells its pixels from the
/// rest: magenta, which neither the gray map nor space has.
pub const COLOR: &str = "#ff00ff";

/// The style image `mark`: a [`COLOR`] square 16 pixels a side, which a
/// scene's icons draw.
pub const MARK: &str = "mark";

/// The height of the ground, in meters, at a longitude and latitude.
pub type Ground = Arc<dyn Fn(f64, f64) -> f64 + Send + Sync>;

/// A map that a scene draws and queries.
pub struct Scene {
    /// Its style, whose layer `layer` draws in [`COLOR`] over a gray
    /// background.
    pub style: String,
    /// The id of the layer that the queries look for.
    pub layer: String,
    /// The ground of the style's terrain, whose raster-dem source is `dem`
    /// ([`style`]); `None` for a style without terrain.
    pub ground: Option<Ground>,
    /// Whether the layer shows somewhere and somewhere not: false for a
    /// scene that hides it, where queries must find it nowhere.
    pub shows: bool,
}

/// The grid points whose answers didn't match the image.
pub struct Coverage {
    /// The still image of the scene.
    pub image: PremultipliedImage,
    /// The points tested: those at least [`EDGE`] pixels from where the
    /// layer's pixels begin or end.
    pub tested: usize,
    /// Of those, the points where the layer shows.
    pub shown: usize,
    /// The points where the layer shows that the query didn't find it.
    pub missed: Vec<ScreenCoordinate>,
    /// The points where the query found the layer that it doesn't show.
    pub extra: Vec<ScreenCoordinate>,
}

impl Coverage {
    /// Whether every answer matched the image.
    #[must_use]
    pub fn matches(&self) -> bool {
        self.missed.is_empty() && self.extra.is_empty()
    }
}

/// How far, in pixels, a tested point is from the layer's edge: a pixel
/// within it of a point that differs from it leaves the point untested.
pub const EDGE: i32 = 2;

/// The spacing of the grid, in pixels.
const STEP: usize = 4;

/// A camera: its center's longitude and latitude, its zoom, its pitch and
/// its bearing.
pub type Camera = [f64; 5];

/// A camera straight down.
pub const FLAT: Camera = [0.0, 0.0, 10.0, 0.0, 0.0];
/// A pitched and turned camera.
pub const PITCHED: Camera = [0.0, 0.0, 10.0, 60.0, 30.0];
/// A globe whose center is far south of features about 60° N, where a
/// pixel spans about two thirds of the ground it does at the center.
pub const GLOBE: Camera = [0.0, 45.0, 1.5, 0.0, 0.0];
/// A globe whose eastern horizon is about 20° E.
pub const HORIZON: Camera = [-62.0, 0.0, 0.8, 0.0, 0.0];
/// A camera south of the hill of [`hill`], looking north at it.
pub const TERRAIN: Camera = [0.0, -0.012, 13.0, 65.0, 0.0];

/// The ground of the terrain scenes: a hill 600 m high at 0, 0, about a
/// hundredth of a degree across, which hides what is right behind it from
/// [`TERRAIN`].
#[must_use]
pub fn hill() -> Ground {
    Arc::new(|lng: f64, lat: f64| {
        let d = (lng / 0.004).powi(2) + (lat / 0.004).powi(2);
        600.0 * (-d).exp()
    })
}

/// A scene of `layer`, a layer whose id is `layer`, of the GeoJSON source
/// `data` holding `geojson`, seen by `camera` in `projection` ("mercator"
/// or "globe"), over `ground` where it has some.
#[must_use]
pub fn scene(camera: Camera, projection: &str, ground: Option<Ground>, geojson: &str, layer: &str) -> Scene {
    let style = style(camera, projection, ground.is_some(), geojson, layer);
    Scene { style, layer: "layer".to_owned(), ground, shows: true }
}

/// A square polygon around `lng`, `lat`, `half` degrees either way.
#[must_use]
pub fn square(lng: f64, lat: f64, half: f64) -> String {
    let (w, e, s, n) = (lng - half, lng + half, lat - half, lat + half);
    format!(r#"{{"type": "Polygon", "coordinates": [[[{w}, {s}], [{e}, {s}], [{e}, {n}], [{w}, {n}], [{w}, {s}]]]}}"#)
}

/// Points at the longitudes and latitudes `points`.
#[must_use]
pub fn points(points: &[[f64; 2]]) -> String {
    format!(r#"{{"type": "MultiPoint", "coordinates": [{}]}}"#, coordinates(points))
}

/// A line through the longitudes and latitudes `points`.
#[must_use]
pub fn line(points: &[[f64; 2]]) -> String {
    format!(r#"{{"type": "LineString", "coordinates": [{}]}}"#, coordinates(points))
}

fn coordinates(points: &[[f64; 2]]) -> String {
    points.iter().map(|[lng, lat]| format!("[{lng}, {lat}]")).collect::<Vec<_>>().join(", ")
}

/// A layer `layer` of the type `kind`, of the source `data`, with the
/// members `paint` and `layout` of its paint and layout properties.
#[must_use]
pub fn layer(kind: &str, paint: &str, layout: &str) -> String {
    format!(r#"{{"id": "layer", "type": "{kind}", "source": "data", "paint": {{{paint}}}, "layout": {{{layout}}}}}"#)
}

/// The members of a style that sees `camera` in `projection`, over terrain
/// where `terrain`, with the GeoJSON source `data` holding `geojson`, a
/// gray background and `layers`.
#[must_use]
pub fn style(camera: Camera, projection: &str, terrain: bool, geojson: &str, layers: &str) -> String {
    let [lng, lat, zoom, pitch, bearing] = camera;
    let terrain = if terrain { r#""terrain": {"source": "dem"},"# } else { "" };
    format!(
        r##"{{
            "version": 8,
            "center": [{lng}, {lat}],
            "zoom": {zoom},
            "pitch": {pitch},
            "bearing": {bearing},
            "projection": {{"type": "{projection}"}},
            {terrain}
            "sources": {{
                "dem": {{"type": "raster-dem", "tiles": ["dem/{{z}}/{{x}}/{{y}}.png"], "tileSize": 256, "maxzoom": 14}},
                "data": {{"type": "geojson", "data": {geojson}}}
            }},
            "layers": [
                {{"id": "background", "type": "background", "paint": {{"background-color": "#d8d8d8"}}}},
                {layers}
            ]
        }}"##
    )
}

/// Whether a pixel is [`COLOR`], as the layer draws it, shaded or lit: a
/// red and a blue alike, well above its green.
fn colored([r, g, b, _]: [u8; 4]) -> bool {
    let (r, g, b) = (i32::from(r), i32::from(g), i32::from(b));
    r > 60 && (r - b).abs() < 48 && g * 3 < r
}

/// Draws `scene` with the layer types of `registry` on the harness's
/// device ([`mln_headless::harness_device`]) and queries it at every
/// grid point; `None` where the machine has no such device.
///
/// # Panics
///
/// Where the map can't be drawn or queried.
#[must_use]
pub fn check(registry: Arc<Registry>, scene: &Scene) -> Option<Coverage> {
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
    if let Some(ground) = scene.ground.clone() {
        file_source.set_response(Kind::Tile, move |resource: &Resource| {
            let (z, x, y) = dem_tile_of(&resource.url)?;
            let mut response = Response::new();
            response.data = Some(Arc::new(dem_tile(z, x, y, &*ground)));
            Some(response)
        });
    }
    let options = MapOptions { map_mode: MapMode::Static, size, pixel_ratio: 1.0, ..Default::default() };
    let mut map = Map::with_file_source(file_source, options, registry, &pool).expect("a map");
    frontend.attach(&mut map);
    map.load_style_json(&scene.style);
    let mut icon = PremultipliedImage::new(Size::new(16, 16));
    for texel in icon.data.as_chunks_mut::<4>().0 {
        *texel = [255, 0, 255, 255];
    }
    let icon = mln_style::image::Impl::new(MARK.to_owned(), icon, 1.0, false, Vec::new(), Vec::new(), None, None, None)
        .expect("the icon");
    map.edit_style(|style| style.apply(mln_style::Edit::AddImage(Arc::new(icon)))).expect("the icon");
    let camera = mln_engine::map::style_camera(map.style());
    map.jump_to(&camera);
    let image = frontend.render(&mut map).expect("the still image").image;

    let side = SIDE as i32;
    let pixels = image.data.as_chunks::<4>().0;
    let shows = |x: i32, y: i32| colored(pixels[(y * side + x) as usize]);
    let options = RenderedQueryOptions::new(Some(vec![scene.layer.clone()]), None);
    let mut coverage =
        Coverage { image: PremultipliedImage::new(size), tested: 0, shown: 0, missed: Vec::new(), extra: Vec::new() };
    for y in (EDGE..side - EDGE).step_by(STEP) {
        for x in (EDGE..side - EDGE).step_by(STEP) {
            let shown = shows(x, y);
            let near = (-EDGE..=EDGE).flat_map(|dy| (-EDGE..=EDGE).map(move |dx| (x + dx, y + dy)));
            if near.into_iter().any(|(nx, ny)| shows(nx, ny) != shown) {
                continue;
            }
            let point = ScreenCoordinate::new(f64::from(x) + 0.5, f64::from(y) + 0.5);
            let found =
                !frontend.renderer().query_rendered_features_at_point(&point, &options).expect("a query").is_empty();
            coverage.tested += 1;
            coverage.shown += usize::from(shown);
            match (shown, found) {
                (true, false) => coverage.missed.push(point),
                (false, true) => coverage.extra.push(point),
                _ => {}
            }
        }
    }
    coverage.image = image;
    Some(coverage)
}

/// The texel of a height of `meters` in the Mapbox encoding:
/// `(r · 65536 + g · 256 + b) · 0.1 − 10000`.
fn mapbox(meters: f64) -> [u8; 4] {
    let value = ((meters + 10000.0) * 10.0).round() as u32;
    [(value >> 16) as u8, (value >> 8) as u8, value as u8, 255]
}

/// The `z`, `x` and `y` of a DEM tile's URL, `dem/{z}/{x}/{y}.png`.
fn dem_tile_of(url: &str) -> Option<(u8, u32, u32)> {
    let mut parts = url.strip_prefix("dem/")?.strip_suffix(".png")?.split('/');
    Some((parts.next()?.parse().ok()?, parts.next()?.parse().ok()?, parts.next()?.parse().ok()?))
}

/// The DEM tile `z/x/y`, 256 pixels a side, of the ground `height`,
/// sampled at its pixels' centers.
fn dem_tile(z: u8, x: u32, y: u32, height: &dyn Fn(f64, f64) -> f64) -> Vec<u8> {
    const DEM_SIDE: u32 = 256;
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
    mln_engine::image::encode_png(&image)
}

/// Checks each of `cases`, a scene and its name, with the layer types of
/// `registry`, and writes the images of those whose queries didn't match
/// them to `output`.
///
/// # Panics
///
/// Where a scene's queries didn't match its image, saying where, or the
/// scene shows its layer at none or all of its tested points where it
/// should show it ([`Scene::shows`]), or at some where it shouldn't.
pub fn assert_matches(registry: &Arc<Registry>, output: &std::path::Path, cases: Vec<(&str, Scene)>) {
    let mut failed = Vec::new();
    for (name, scene) in cases {
        let Some(coverage) = check(Arc::clone(registry), &scene) else { return };
        let shows = coverage.shown > 0 && coverage.shown < coverage.tested;
        if coverage.matches() && shows == scene.shows {
            continue;
        }
        std::fs::create_dir_all(output).expect("the output directory");
        let png = mln_engine::image::encode_png(&coverage.image);
        std::fs::write(output.join(format!("{name}.png")), png).expect("the image");
        let first = |points: &[ScreenCoordinate]| {
            points.first().map(|p| format!(" (first at {}, {})", p.x, p.y)).unwrap_or_default()
        };
        failed.push(format!(
            "{name}: {} of {} shown points missed{}, {} of {} others found{}",
            coverage.missed.len(),
            coverage.shown,
            first(&coverage.missed),
            coverage.extra.len(),
            coverage.tested - coverage.shown,
            first(&coverage.extra),
        ));
    }
    assert!(failed.is_empty(), "queries that didn't match the image:\n{}", failed.join("\n"));
}
