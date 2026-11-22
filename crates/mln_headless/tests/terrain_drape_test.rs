// The layers that drape over terrain draw into the terrain tiles' drapes,
// which the surface shows: over flat ground, seen from above, the map looks
// as it does without terrain.

use std::sync::Arc;

use mln_headless::HeadlessFrontend;
use mln_map::{BoundOptions, Map, MapMode, MapOptions, ViewportMode};
use mln_storage::resource::TileData;
use mln_storage::stub_file_source::{ResponseType, StubFileSource};
use mln_storage::{Kind, Resource, Response};
use mln_util::error::ErrorKind;
use mln_util::image::PremultipliedImage;
use mln_util::pool::Pool;
use mln_util::queue::Queue;
use mln_util::size::Size;

const SIDE: u32 = 256;

/// How many pixels a draped map may differ by from the same map drawn flat:
/// the surface filters its drapes, so pixels on the shapes' edges differ, and
/// how many depends on the GPU.
const EDGE_PIXELS: u64 = 64;

/// A DEM tile of sea level everywhere, in the Mapbox encoding: (1 · 65536 +
/// 134 · 256 + 160) · 0.1 − 10000 = 0 m.
fn sea_level_png() -> Vec<u8> {
    let mut image = PremultipliedImage::new(Size::new(256, 256));
    for pixel in image.data.as_chunks_mut::<4>().0 {
        pixel.copy_from_slice(&[1, 134, 160, 255]);
    }
    mln_image::encode_png(&image)
}

/// A white map with a red square around the center and a blue line across
/// it, at an integer zoom, seen from above; with a terrain over flat ground
/// if `terrain`.
fn style(terrain: bool) -> String {
    let terrain = if terrain { r#""terrain": {"source": "dem"},"# } else { "" };
    format!(
        r#"{{
            "version": 8,
            "center": [0, 0],
            "zoom": 2,
            {terrain}
            "sources": {{
                "dem": {{"type": "raster-dem", "tiles": ["dem/{{z}}/{{x}}/{{y}}.png"], "tileSize": 256, "maxzoom": 10}},
                "shapes": {{
                    "type": "geojson",
                    "data": {{
                        "type": "FeatureCollection",
                        "features": [
                            {{
                                "type": "Feature",
                                "properties": {{}},
                                "geometry": {{
                                    "type": "Polygon",
                                    "coordinates": [[[-5, -5], [5, -5], [5, 5], [-5, 5], [-5, -5]]]
                                }}
                            }},
                            {{
                                "type": "Feature",
                                "properties": {{}},
                                "geometry": {{"type": "LineString", "coordinates": [[-60, 10], [60, 10]]}}
                            }}
                        ]
                    }}
                }}
            }},
            "layers": [
                {{"id": "background", "type": "background", "paint": {{"background-color": "white"}}}},
                {{
                    "id": "fill",
                    "type": "fill",
                    "source": "shapes",
                    "filter": ["==", "$type", "Polygon"],
                    "paint": {{"fill-color": "red"}}
                }},
                {{
                    "id": "line",
                    "type": "line",
                    "source": "shapes",
                    "filter": ["==", "$type", "LineString"],
                    "paint": {{"line-color": "blue", "line-width": 6}}
                }}
            ]
        }}"#
    )
}

/// A white map with a red square around the center, a green circle at the
/// center, and a blue rectangle over the circle's east half, at an integer
/// zoom, seen from above; with a terrain over flat ground if `terrain`. The
/// circle splits the draped layers into two stacks.
fn stacked_style(terrain: bool) -> String {
    let terrain = if terrain { r#""terrain": {"source": "dem"},"# } else { "" };
    format!(
        r#"{{
            "version": 8,
            "center": [0, 0],
            "zoom": 2,
            {terrain}
            "sources": {{
                "dem": {{"type": "raster-dem", "tiles": ["dem/{{z}}/{{x}}/{{y}}.png"], "tileSize": 256, "maxzoom": 10}},
                "red": {{
                    "type": "geojson",
                    "data": {{"type": "Polygon", "coordinates": [[[-8, -8], [8, -8], [8, 8], [-8, 8], [-8, -8]]]}}
                }},
                "point": {{"type": "geojson", "data": {{"type": "Point", "coordinates": [0, 0]}}}},
                "blue": {{
                    "type": "geojson",
                    "data": {{"type": "Polygon", "coordinates": [[[-1, -5], [5, -5], [5, 5], [-1, 5], [-1, -5]]]}}
                }}
            }},
            "layers": [
                {{"id": "background", "type": "background", "paint": {{"background-color": "white"}}}},
                {{"id": "red", "type": "fill", "source": "red", "paint": {{"fill-color": "red"}}}},
                {{
                    "id": "circle",
                    "type": "circle",
                    "source": "point",
                    "paint": {{"circle-color": "lime", "circle-radius": 15, "circle-blur": 0}}
                }},
                {{"id": "blue", "type": "fill", "source": "blue", "paint": {{"fill-color": "blue"}}}}
            ]
        }}"#
    )
}

/// Where the blue fill of [`fogged_style`] is: in the upper stack, in the
/// lower one, or nowhere.
#[derive(Clone, Copy, PartialEq)]
enum Blue {
    Above,
    Below,
    None,
}

/// The sky of [`fogged_style`]: a magenta fog over the whole ground.
const MAGENTA_FOG: &str = r#""sky": {"fog-color": "magenta", "fog-ground-blend": 0},"#;

/// A white map over flat terrain pitched 75°, under [`MAGENTA_FOG`], with a circle near the camera and an opaque blue fill over
/// the ground west of 1° W, in the stack above the circle or below it.
fn fogged_style(blue: Blue) -> String {
    let fill = r#"{"id": "blue", "type": "fill", "source": "blue", "paint": {"fill-color": "blue"}}"#;
    let circle = r#"{"id": "circle", "type": "circle", "source": "point", "paint": {"circle-color": "lime"}}"#;
    let layers = match blue {
        Blue::Above => format!("{circle}, {fill}"),
        Blue::Below => format!("{fill}, {circle}"),
        Blue::None => circle.to_owned(),
    };
    format!(
        r#"{{
            "version": 8,
            "center": [0, 0],
            "zoom": 4,
            "pitch": 75,
            "terrain": {{"source": "dem"}},
            {MAGENTA_FOG}
            "sources": {{
                "dem": {{"type": "raster-dem", "tiles": ["dem/{{z}}/{{x}}/{{y}}.png"], "tileSize": 256, "maxzoom": 10}},
                "point": {{"type": "geojson", "data": {{"type": "Point", "coordinates": [0, -4]}}}},
                "blue": {{
                    "type": "geojson",
                    "data": {{"type": "Polygon", "coordinates": [[[-170, -80], [-1, -80], [-1, 80], [-170, 80], [-170, -80]]]}}
                }}
            }},
            "layers": [
                {{"id": "background", "type": "background", "paint": {{"background-color": "white"}}}},
                {layers}
            ]
        }}"#
    )
}

/// The still image of `style` on the harness's device
/// ([`mln_headless::harness_device`]), over DEM tiles at sea level, and the
/// bytes of the textures that drew it; `None` where the machine has no such
/// device.
fn render(style: &str, viewport_mode: ViewportMode) -> Option<(PremultipliedImage, i32)> {
    let dem = Arc::new(sea_level_png());
    render_over(style, viewport_mode, move |_| dem.to_vec())
}

/// [`render`] over the DEM tiles that `dem` makes.
fn render_over(
    style: &str,
    viewport_mode: ViewportMode,
    dem: impl Fn(&TileData) -> Vec<u8> + Send + Sync + 'static,
) -> Option<(PremultipliedImage, i32)> {
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
    let options = MapOptions { map_mode: MapMode::Static, size, pixel_ratio: 1.0, viewport_mode, ..Default::default() };
    let mut map = Map::with_file_source(file_source, options, &pool).unwrap();
    frontend.attach(&mut map);
    map.set_bounds(&BoundOptions { max_pitch: Some(85.0), ..Default::default() }).unwrap();
    map.load_style_json(style);
    let camera = map.style().get_default_camera();
    map.jump_to(&camera);
    let result = frontend.render(&mut map).expect("the still image");
    Some((result.image, result.stats.mem_textures))
}

fn pixel(image: &PremultipliedImage, x: u32, y: u32) -> [u8; 4] {
    let start = ((y * image.size.width + x) * 4) as usize;
    image.data[start..start + 4].try_into().unwrap()
}

#[test]
fn a_fill_and_a_line_drape_over_flat_terrain() {
    let (Some((draped, draped_textures)), Some((flat, flat_textures))) =
        (render(&style(true), ViewportMode::Default), render(&style(false), ViewportMode::Default))
    else {
        return;
    };
    // At least one drape of 1024² texels for the terrain's 512 px tiles.
    assert!(draped_textures - flat_textures >= 1024 * 1024 * 4, "the drapes' textures");
    let center = SIDE / 2;
    assert_eq!([255, 0, 0, 255], pixel(&draped, center, center), "the fill");
    // 10° north is 57 px north of the equator in the 2048 px of the world
    // at zoom 2.
    assert_eq!([0, 0, 255, 255], pixel(&draped, center, center - 57), "the line");
    assert_eq!([255, 255, 255, 255], pixel(&draped, 4, 4), "the background");
    let differing = pixelmatch::pixelmatch(&draped.data, &flat.data, SIDE as usize, SIDE as usize, None, 0.1285, false);
    assert!(differing <= EDGE_PIXELS, "{differing} pixels differ from the map without terrain");
}

/// A viewport flipped upside down, which turns the surface's triangles
/// round, still shows the surface, drawn upside down.
#[test]
fn a_flipped_viewport_shows_the_terrain() {
    let (Some((flipped, _)), Some((flat, _))) =
        (render(&style(true), ViewportMode::FlippedY), render(&style(false), ViewportMode::FlippedY))
    else {
        return;
    };
    let center = SIDE / 2;
    assert_eq!([0, 0, 255, 255], pixel(&flipped, center, center + 57), "the line, south of the center");
    let differing =
        pixelmatch::pixelmatch(&flipped.data, &flat.data, SIDE as usize, SIDE as usize, None, 0.1285, false);
    assert!(differing <= EDGE_PIXELS, "{differing} pixels differ from the map without terrain");
}

/// A circle between two fills stands between their stacks: the fill below
/// shows under it, and the fill above covers it, as without terrain.
#[test]
fn a_circle_stands_between_two_stacks_of_draped_layers() {
    let (Some((draped, _)), Some((flat, _))) =
        (render(&stacked_style(true), ViewportMode::Default), render(&stacked_style(false), ViewportMode::Default))
    else {
        return;
    };
    let center = SIDE / 2;
    assert_eq!([0, 0, 255, 255], pixel(&draped, center, center), "the blue fill over the circle's center");
    // 1° is 5.7 px in the 2048 px of the world at zoom 2: the blue fill
    // starts 6 px west of the center, and the circle reaches 15 px.
    assert_eq!([0, 255, 0, 255], pixel(&draped, center - 11, center), "the circle west of the blue fill");
    assert_eq!([255, 0, 0, 255], pixel(&draped, center - 30, center), "the red fill around the circle");
    let differing = pixelmatch::pixelmatch(&draped.data, &flat.data, SIDE as usize, SIDE as usize, None, 0.1285, false);
    assert!(differing <= EDGE_PIXELS, "{differing} pixels differ from the map without terrain");
}

/// [`fogged_style`] with no blue fill under `sky`, the members of a style's
/// sky object, in place of [`MAGENTA_FOG`].
fn sky_style(sky: &str) -> String {
    fogged_style(Blue::None).replace(MAGENTA_FOG, &format!(r#""sky": {{{sky}}},"#))
}

/// The far ground of [`fogged_style`], where the fog is strong.
const FAR_GROUND: [(u32, u32); 2] = [(210, 70), (200, 60)];

/// The fog lands once on each pixel, however many stacks drape the surface:
/// where the stack above draws nothing, the ground shows as it does without
/// that stack, and inside its opaque fill, as it does with the fill in the
/// stack below. The far ground it compares is fogged.
#[test]
fn fog_lands_once_where_stacks_overlap() {
    let (Some((above, _)), Some((none, _)), Some((below, _)), Some((clear, _))) = (
        render(&fogged_style(Blue::Above), ViewportMode::Default),
        render(&fogged_style(Blue::None), ViewportMode::Default),
        render(&fogged_style(Blue::Below), ViewportMode::Default),
        render(&fogged_style(Blue::None).replace(MAGENTA_FOG, ""), ViewportMode::Default),
    ) else {
        return;
    };
    // Far ground east of the fill, where the stack above draws nothing.
    for (x, y) in FAR_GROUND {
        assert_ne!(pixel(&clear, x, y), pixel(&none, x, y), "the fog at {x}, {y}");
        assert_eq!(pixel(&none, x, y), pixel(&above, x, y), "the ground at {x}, {y}");
    }
    // Far ground inside the fill.
    for (x, y) in [(40, 70), (30, 100)] {
        assert_ne!(pixel(&none, x, y), pixel(&above, x, y), "the fill at {x}, {y}");
        assert_eq!(pixel(&below, x, y), pixel(&above, x, y), "the fill at {x}, {y}");
    }
}

/// Fog tints what is drawn and never changes how much of a pixel the ground
/// covers: a fog color's alpha is the tint's strength, so a transparent fog
/// leaves opaque ground as it is, and a translucent one tints it less than
/// an opaque one does, with the ground still opaque.
#[test]
fn fog_tints_the_ground_without_uncovering_it() {
    let (Some((clear, _)), Some((transparent, _)), Some((translucent, _)), Some((opaque, _))) = (
        render(&fogged_style(Blue::None).replace(MAGENTA_FOG, ""), ViewportMode::Default),
        render(
            &sky_style(r#""fog-color": "transparent", "horizon-fog-blend": 1, "fog-ground-blend": 0"#),
            ViewportMode::Default,
        ),
        render(
            &sky_style(r#""fog-color": "rgba(255, 0, 255, 0.5)", "horizon-fog-blend": 1, "fog-ground-blend": 0"#),
            ViewportMode::Default,
        ),
        render(
            &sky_style(r#""fog-color": "magenta", "horizon-fog-blend": 1, "fog-ground-blend": 0"#),
            ViewportMode::Default,
        ),
    ) else {
        return;
    };
    for (x, y) in FAR_GROUND {
        assert_eq!(pixel(&clear, x, y), pixel(&transparent, x, y), "a transparent fog at {x}, {y}");
        let [_, clear_green, ..] = pixel(&clear, x, y);
        let [_, green, _, alpha] = pixel(&translucent, x, y);
        let [_, opaque_green, _, opaque_alpha] = pixel(&opaque, x, y);
        assert_eq!((255, 255), (alpha, opaque_alpha), "the ground's coverage at {x}, {y}");
        assert!(opaque_green < green && green < clear_green, "a translucent fog at {x}, {y}: {green}");
    }
}

/// Fog tints only what the stacks draw: ground that a style's draped layers
/// leave transparent stays transparent, as it is where no layer drapes.
#[test]
fn fog_leaves_transparent_ground_transparent() {
    let background = r#"{"id": "background", "type": "background", "paint": {"background-color": "white"}},"#;
    let style = fogged_style(Blue::None);
    let (Some((transparent, _)), Some((undraped, _))) = (
        render(
            &style.replace(r#""background-color": "white""#, r#""background-color": "transparent""#),
            ViewportMode::Default,
        ),
        render(&style.replace(background, ""), ViewportMode::Default),
    ) else {
        return;
    };
    for (x, y) in FAR_GROUND {
        assert_eq!([0; 4], pixel(&undraped, x, y), "no ground at {x}, {y}");
        assert_eq!([0; 4], pixel(&transparent, x, y), "the transparent ground at {x}, {y}");
    }
}

/// A DEM tile of 256 pixels a side, in the Mapbox encoding, of a plane that
/// rises to the north, 1 m for every 4 m of the world's width at the equator,
/// from 1000 m at the equator.
fn northward_slope_png(tile: &TileData) -> Vec<u8> {
    const EQUATOR_TO_POLE: f64 = 40_075_016.685_578_49 / 2.0;
    let tiles = f64::from(1u32 << tile.z);
    let mut image = PremultipliedImage::new(Size::new(256, 256));
    for (index, pixel) in image.data.as_chunks_mut::<4>().0.iter_mut().enumerate() {
        let y = (f64::from(tile.y) + ((index / 256) as f64 + 0.5) / 256.0) / tiles;
        let height = 1000.0 + (0.5 - y) * EQUATOR_TO_POLE * 2.0 / 4.0;
        let value = ((height + 10000.0) * 10.0).round() as u32;
        *pixel = [(value >> 16) as u8, (value >> 8) as u8, value as u8, 255];
    }
    mln_image::encode_png(&image)
}

/// A hillshade over terrain, seen past its DEM's deepest zoom, shades a plane
/// slope evenly: no line shows where two of its DEM tiles meet.
#[test]
fn a_hillshade_over_terrain_shows_no_seam_where_its_tiles_meet() {
    let style = r#"{
        "version": 8,
        "center": [0.001, 0.001],
        "zoom": 16,
        "pitch": 70,
        "terrain": {"source": "dem"},
        "sources": {
            "dem": {"type": "raster-dem", "tiles": ["dem/{z}/{x}/{y}.png"], "tileSize": 256, "minzoom": 13, "maxzoom": 13}
        },
        "layers": [
            {"id": "background", "type": "background", "paint": {"background-color": "white"}},
            {"id": "hillshade", "type": "hillshade", "source": "dem"}
        ]
    }"#;
    let Some((image, _)) = render_over(style, ViewportMode::Default, northward_slope_png) else {
        return;
    };
    // The slope rises toward the horizon and fills the image.
    let shade = pixel(&image, SIDE / 2, SIDE - 1);
    let uneven: Vec<(u32, u32)> = (0..SIDE)
        .flat_map(|y| (0..SIDE).map(move |x| (x, y)))
        .filter(|&(x, y)| pixel(&image, x, y).iter().zip(shade).any(|(a, b)| a.abs_diff(b) > 2))
        .collect();
    assert!(
        uneven.is_empty(),
        "{} pixels shade the slope otherwise than {shade:?}, as at {:?}",
        uneven.len(),
        uneven.first()
    );
}

/// A hillshade lit from the viewport, as its lights are by default, shades a
/// plane slope over terrain as it does on a flat map, at any bearing: the
/// terrain's surface lights its slopes with the lights of the frame's
/// bearing, which turn with the map.
#[test]
fn a_viewport_lit_hillshade_over_terrain_is_lit_as_on_a_flat_map() {
    let style = |terrain: bool, bearing: u32| {
        let terrain = if terrain { r#""terrain": {"source": "dem"},"# } else { "" };
        format!(
            r#"{{
                "version": 8,
                "center": [0.001, 0.001],
                "zoom": 16,
                "bearing": {bearing},
                {terrain}
                "sources": {{
                    "dem": {{"type": "raster-dem", "tiles": ["dem/{{z}}/{{x}}/{{y}}.png"], "tileSize": 256, "minzoom": 13, "maxzoom": 13}}
                }},
                "layers": [
                    {{"id": "background", "type": "background", "paint": {{"background-color": "white"}}}},
                    {{"id": "hillshade", "type": "hillshade", "source": "dem"}}
                ]
            }}"#
        )
    };
    let shade = |terrain: bool, bearing: u32| {
        let (image, _) = render_over(&style(terrain, bearing), ViewportMode::Default, northward_slope_png)?;
        Some(pixel(&image, SIDE / 2, SIDE / 2))
    };
    let mut shades = Vec::new();
    for bearing in [0, 120] {
        let (Some(draped), Some(flat)) = (shade(true, bearing), shade(false, bearing)) else {
            return;
        };
        let near = draped.iter().zip(flat).all(|(a, b)| a.abs_diff(b) <= 2);
        assert!(near, "at bearing {bearing}, {draped:?} over terrain and {flat:?} on a flat map");
        shades.push(flat);
    }
    let turned = shades[0].iter().zip(shades[1]).any(|(a, b)| a.abs_diff(b) > 16);
    assert!(turned, "the lights didn't turn with the map: {shades:?}");
}
