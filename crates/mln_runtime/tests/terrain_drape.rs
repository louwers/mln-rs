//! The layers that drape over terrain draw into the terrain tiles' drapes,
//! which the surface shows: over flat ground, seen from above, the map looks
//! as it does without terrain.

use std::sync::Arc;

use mln_engine::camera::{BoundOptions, ViewportMode};
use mln_engine::storage::{Kind, Response};
use mln_style::bitmap::PremultipliedImage;
use mln_style::size::Size;

use crate::support::still::{StillMap, StillOptions, differing, not_found, pixel, serve};
use crate::support::wait;

const SIDE: u32 = 256;

/// How many pixels a draped map may differ by from the same map drawn flat:
/// the surface filters its drapes, so pixels on the images' edges differ, and
/// how many depends on the GPU. `OpenHarmony`'s emulator, with Mesa 22.2.5's
/// llvmpipe, filters them more softly than newer drivers do.
const EDGE_PIXELS: u64 = if cfg!(target_env = "ohos") { 256 } else { 64 };

/// A DEM tile of sea level everywhere, in the Mapbox encoding: (1 · 65536 +
/// 134 · 256 + 160) · 0.1 − 10000 = 0 m.
fn sea_level_png() -> Vec<u8> {
    solid_png([1, 134, 160, 255])
}

/// A PNG of 8 pixels a side, every pixel `pixel`.
fn solid_png(pixel: [u8; 4]) -> Vec<u8> {
    let mut image = PremultipliedImage::new(Size::new(8, 8));
    for texel in image.data.as_chunks_mut::<4>().0 {
        *texel = pixel;
    }
    mln_engine::image::encode_png(&image)
}

/// The members of a style at zoom 2 over the center, seen from above, with a
/// terrain over flat ground if `terrain`, whose sources are `sources` and
/// the DEM's, and whose layers are `layers`.
fn style_of(terrain: bool, sources: &str, layers: &str) -> String {
    let terrain = if terrain { r#""terrain": {"source": "dem"},"# } else { "" };
    format!(
        r#"{{
            "version": 8,
            "center": [0, 0],
            "zoom": 2,
            {terrain}
            "sources": {{
                "dem": {{"type": "raster-dem", "tiles": ["dem/{{z}}/{{x}}/{{y}}.png"], "tileSize": 256, "maxzoom": 10}},
                {sources}
            }},
            "layers": [{layers}]
        }}"#
    )
}

/// An image source `id` of the image `id.png` from `west` to `east` and from
/// `south` to `north`, in degrees.
fn image_source(id: &str, [west, south, east, north]: [f64; 4]) -> String {
    format!(
        r#""{id}": {{
            "type": "image",
            "url": "{id}.png",
            "coordinates": [[{west}, {north}], [{east}, {north}], [{east}, {south}], [{west}, {south}]]
        }}"#
    )
}

/// A raster layer of the source `id`.
fn raster_layer(id: &str) -> String {
    format!(r#"{{"id": "{id}", "type": "raster", "source": "{id}", "paint": {{"raster-fade-duration": 0}}}}"#)
}

const BACKGROUND: &str = r#"{"id": "background", "type": "background", "paint": {"background-color": "white"}}"#;

/// A white map with a red image around the center and a blue image band
/// across it north of the center; with a terrain over flat ground if
/// `terrain`.
fn style(terrain: bool) -> String {
    let sources = [image_source("red", [-5.0, -5.0, 5.0, 5.0]), image_source("blue", [-60.0, 9.0, 60.0, 11.0])];
    let layers = [BACKGROUND.to_owned(), raster_layer("red"), raster_layer("blue")];
    style_of(terrain, &sources.join(","), &layers.join(","))
}

/// A white map with a red image around the center, a green circle at the
/// center, and a blue image over the circle's east half; with a terrain over
/// flat ground if `terrain`. The circle splits the draped layers into two
/// stacks.
fn stacked_style(terrain: bool) -> String {
    let circle = r#"{
        "id": "circle",
        "type": "circle",
        "source": "point",
        "paint": {"circle-color": "lime", "circle-radius": 15, "circle-blur": 0}
    }"#;
    let sources = [
        image_source("red", [-8.0, -8.0, 8.0, 8.0]),
        r#""point": {"type": "geojson", "data": {"type": "Point", "coordinates": [0, 0]}}"#.to_owned(),
        image_source("blue", [-1.0, -5.0, 5.0, 5.0]),
    ];
    let layers = [BACKGROUND.to_owned(), raster_layer("red"), circle.to_owned(), raster_layer("blue")];
    style_of(terrain, &sources.join(","), &layers.join(","))
}

/// The still image of `style` in `viewport_mode`, pitched as it says up to
/// 85°, over DEM tiles at sea level, with the image `red.png` red and the
/// image `blue.png` blue.
fn render(style: &str, viewport_mode: ViewportMode) -> PremultipliedImage {
    let mut options = StillOptions::default();
    options.map.map.viewport_mode = viewport_mode;
    let map = StillMap::new(options);
    let dem = Arc::new(sea_level_png());
    let (red, blue) = (Arc::new(solid_png([255, 0, 0, 255])), Arc::new(solid_png([0, 0, 255, 255])));
    serve(&map.runtime, move |resource| {
        let data = match resource.kind {
            Kind::Tile => &dem,
            Kind::Image if resource.url.starts_with("red") => &red,
            Kind::Image => &blue,
            _ => return not_found(),
        };
        Response { data: Some(Arc::clone(data)), ..Response::default() }
    });
    wait(map.map.set_bounds(BoundOptions { max_pitch: Some(85.0), ..Default::default() }));
    map.load_json(style);
    map.render()
}

/// A viewport flipped upside down, which turns the surface's triangles
/// round, still shows the surface, drawn upside down.
#[test]
fn a_flipped_viewport_shows_the_terrain() {
    let flipped = render(&style(true), ViewportMode::FlippedY);
    let flat = render(&style(false), ViewportMode::FlippedY);
    let center = SIDE / 2;
    assert_eq!([0, 0, 255, 255], pixel(&flipped, center, center + 57), "the blue image, south of the center");
    let differing = differing(&flipped, &flat);
    assert!(differing <= EDGE_PIXELS, "{differing} pixels differ from the map without terrain");
}

/// A circle between two images stands between their stacks: the image below
/// shows under it, and the image above covers it, as without terrain.
#[test]
fn a_circle_stands_between_two_stacks_of_draped_layers() {
    let draped = render(&stacked_style(true), ViewportMode::Default);
    let flat = render(&stacked_style(false), ViewportMode::Default);
    let center = SIDE / 2;
    assert_eq!([0, 0, 255, 255], pixel(&draped, center, center), "the blue image over the circle's center");
    // 1° is 5.7 px in the 2048 px of the world at zoom 2: the blue image
    // starts 6 px west of the center, and the circle reaches 15 px.
    assert_eq!([0, 255, 0, 255], pixel(&draped, center - 11, center), "the circle west of the blue image");
    assert_eq!([255, 0, 0, 255], pixel(&draped, center - 30, center), "the red image around the circle");
    let differing = differing(&draped, &flat);
    assert!(differing <= EDGE_PIXELS, "{differing} pixels differ from the map without terrain");
}

/// A GeoJSON source `id` of a polygon for each of `boxes`, each from west
/// to east and from south to north, in degrees.
fn polygon_source(id: &str, boxes: &[[f64; 4]]) -> String {
    let features: Vec<String> = boxes
        .iter()
        .map(|[west, south, east, north]| {
            format!(
                r#"{{"type": "Feature", "properties": {{}}, "geometry": {{"type": "Polygon", "coordinates": [[[{west}, {north}], [{east}, {north}], [{east}, {south}], [{west}, {south}], [{west}, {north}]]]}}}}"#
            )
        })
        .collect();
    format!(
        r#""{id}": {{"type": "geojson", "data": {{"type": "FeatureCollection", "features": [{}]}}}}"#,
        features.join(",")
    )
}

/// On the globe, a tall fill extrusion that draws before the draped layers,
/// on the far side of the planet in a tile that reaches over the horizon,
/// stays hidden behind the planet, which a half transparent image covers
/// there, as without terrain. On the near side it shows.
#[test]
fn the_globe_hides_an_extrusion_behind_it_before_the_drapes() {
    let style = |terrain: bool, [west, east]: [f64; 2]| {
        let terrain = if terrain { r#""terrain": {"source": "dem"},"# } else { "" };
        let sources = [
            polygon_source("extrusion", &[[west, -20.0, east, 20.0]]),
            image_source("red", [-85.0, -60.0, 85.0, 60.0]),
        ];
        format!(
            r#"{{
                "version": 8,
                "center": [0, 0],
                "zoom": 0.5,
                "projection": {{"type": "vertical-perspective"}},
                {terrain}
                "sources": {{
                    "dem": {{"type": "raster-dem", "tiles": ["dem/{{z}}/{{x}}/{{y}}.png"], "tileSize": 256, "maxzoom": 10}},
                    {}
                }},
                "layers": [
                    {{"id": "extrusion", "type": "fill-extrusion", "source": "extrusion",
                      "paint": {{"fill-extrusion-color": "lime", "fill-extrusion-height": 300000}}}},
                    {{"id": "red", "type": "raster", "source": "red",
                      "paint": {{"raster-fade-duration": 0, "raster-opacity": 0.5}}}}
                ]
            }}"#,
            sources.join(",")
        )
    };
    // Lime, alone or under half the red.
    let lime = |image: &PremultipliedImage| {
        let pixels = image.data.as_chunks::<4>().0.iter();
        pixels.filter(|[_, g, b, _]| *g > 64 && *b < 32).count()
    };
    for terrain in [false, true] {
        let near = render(&style(terrain, [20.0, 50.0]), ViewportMode::Default);
        let far = render(&style(terrain, [120.0, 150.0]), ViewportMode::Default);
        assert_ne!(0, lime(&near), "the extrusion on the near side, with terrain: {terrain}");
        assert_eq!(0, lime(&far), "the extrusion on the far side, with terrain: {terrain}");
    }
}

/// Where the blue fill of [`fogged_style`] is: in the stack above the
/// circle, in the one below it, or nowhere.
#[derive(Clone, Copy)]
enum Blue {
    Above,
    Below,
    None,
}

/// A white map over flat terrain pitched 75° under the sky `sky`, the
/// members of a style's sky object, with a circle near the camera, which
/// splits the draped layers into two stacks, and an opaque blue fill over
/// the ground west of 1° W, in the stack `blue` says.
fn fogged_style(sky: &str, blue: Blue) -> String {
    let fill = r#"{"id": "blue", "type": "fill", "source": "blue", "paint": {"fill-color": "blue"}}"#;
    let circle = r#"{"id": "circle", "type": "circle", "source": "point", "paint": {"circle-color": "lime"}}"#;
    let layers = match blue {
        Blue::Above => format!("{BACKGROUND}, {circle}, {fill}"),
        Blue::Below => format!("{BACKGROUND}, {fill}, {circle}"),
        Blue::None => format!("{BACKGROUND}, {circle}"),
    };
    format!(
        r#"{{
            "version": 8,
            "center": [0, 0],
            "zoom": 4,
            "pitch": 75,
            "terrain": {{"source": "dem"}},
            "sky": {{{sky}}},
            "sources": {{
                "dem": {{"type": "raster-dem", "tiles": ["dem/{{z}}/{{x}}/{{y}}.png"], "tileSize": 256, "maxzoom": 10}},
                "point": {{"type": "geojson", "data": {{"type": "Point", "coordinates": [0, -4]}}}},
                "blue": {{
                    "type": "geojson",
                    "data": {{"type": "Polygon", "coordinates": [[[-170, -80], [-1, -80], [-1, 80], [-170, 80], [-170, -80]]]}}
                }}
            }},
            "layers": [{layers}]
        }}"#
    )
}

/// A magenta fog over the whole ground, which turns to no other color.
const MAGENTA_FOG: &str = r#""fog-color": "magenta", "fog-ground-blend": 0, "horizon-fog-blend": 1"#;

/// Far ground of [`fogged_style`] east of the fill, where the fog is strong.
const FAR_GROUND: [(u32, u32); 2] = [(210, 70), (200, 60)];

/// Far ground of [`fogged_style`] inside the fill.
const FAR_FILL: [(u32, u32); 2] = [(40, 70), (30, 100)];

/// The fog lands once on each pixel, however many stacks drape the surface:
/// where the stack above draws nothing, the ground shows as it does without
/// that stack, and inside its opaque fill, as it does with the fill in the
/// stack below.
#[test]
fn fog_lands_once_where_stacks_overlap() {
    let above = render(&fogged_style(MAGENTA_FOG, Blue::Above), ViewportMode::Default);
    let none = render(&fogged_style(MAGENTA_FOG, Blue::None), ViewportMode::Default);
    let below = render(&fogged_style(MAGENTA_FOG, Blue::Below), ViewportMode::Default);
    let clear = render(&fogged_style(r#""fog-ground-blend": 1"#, Blue::None), ViewportMode::Default);
    for (x, y) in FAR_GROUND {
        assert_ne!(pixel(&clear, x, y), pixel(&none, x, y), "the fog at {x}, {y}");
        assert_eq!(pixel(&none, x, y), pixel(&above, x, y), "the ground at {x}, {y}");
    }
    for (x, y) in FAR_FILL {
        assert_ne!(pixel(&none, x, y), pixel(&above, x, y), "the fill at {x}, {y}");
        assert_eq!(pixel(&below, x, y), pixel(&above, x, y), "the fill at {x}, {y}");
    }
}

/// The fog tints the ground and never uncovers it: a fog color's alpha is
/// how strongly it tints, so a transparent fog leaves the ground as it is,
/// and a translucent one tints it less than an opaque one, the ground
/// opaque under both. Ground that the draped layers leave transparent stays
/// transparent.
#[test]
fn fog_tints_the_ground_without_uncovering_it() {
    let fog = |color: &str| format!(r#""fog-color": "{color}", "fog-ground-blend": 0, "horizon-fog-blend": 1"#);
    let without_background = |style: String| style.replace(&format!("{BACKGROUND}, "), "");
    let clear = render(&fogged_style(r#""fog-ground-blend": 1"#, Blue::None), ViewportMode::Default);
    let transparent = render(&fogged_style(&fog("transparent"), Blue::None), ViewportMode::Default);
    let translucent = render(&fogged_style(&fog("rgba(255, 0, 255, 0.5)"), Blue::None), ViewportMode::Default);
    let opaque = render(&fogged_style(&fog("magenta"), Blue::None), ViewportMode::Default);
    let undraped = render(&without_background(fogged_style(&fog("magenta"), Blue::None)), ViewportMode::Default);
    for (x, y) in FAR_GROUND {
        assert_eq!(pixel(&clear, x, y), pixel(&transparent, x, y), "a transparent fog at {x}, {y}");
        let [_, clear_green, ..] = pixel(&clear, x, y);
        let [_, green, _, alpha] = pixel(&translucent, x, y);
        let [_, opaque_green, _, opaque_alpha] = pixel(&opaque, x, y);
        assert_eq!((255, 255), (alpha, opaque_alpha), "the ground's coverage at {x}, {y}");
        assert!(opaque_green < green && green < clear_green, "a translucent fog at {x}, {y}: {green}");
        assert_eq!([0; 4], pixel(&undraped, x, y), "no ground at {x}, {y}");
    }
}
