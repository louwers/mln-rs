//! A frame with more clipped tiles than stencil values clears the stencil
//! buffer and starts again: in every world copy it shows, not only the
//! central one.

use mln_engine::storage::Kind;
use mln_style::bitmap::PremultipliedImage;
use mln_style::size::Size;

use crate::support::still::{StillMap, StillOptions, file_response, not_found, serve};

/// Two worlds wide at zoom 2: the east half of world copy -1, copy 0 and
/// the west half of copy 1.
const WIDTH: u32 = 4096;
const HEIGHT: u32 = 512;
const WORLD: usize = 2048;

/// A pink world under `pairs` pairs of blue water fills, which alternate
/// between two vector sources limited to the middle of the world, with
/// different tiles, so that each fill clips to new tile masks. Past the
/// sources' bounds no tile clips the water, which reaches beyond its tiles'
/// edges.
fn style(pairs: usize) -> String {
    let mut layers = vec![
        r#"{"id": "background", "type": "background", "paint": {"background-color": "white"}}"#.to_owned(),
        r#"{"id": "world", "type": "fill", "source": "world", "paint": {"fill-color": "pink"}}"#.to_owned(),
    ];
    for i in 0..pairs {
        for source in ["middle", "north"] {
            layers.push(format!(
                r#"{{"id": "{source}{i}", "type": "fill", "source": "{source}", "source-layer": "water", "paint": {{"fill-color": "blue"}}}}"#
            ));
        }
    }
    format!(
        r#"{{
            "version": 8,
            "center": [0, 0],
            "zoom": 2,
            "sources": {{
                "middle": {{"type": "vector", "maxzoom": 0, "bounds": [-80, -60, 80, 60], "tiles": ["middle/{{z}}-{{x}}-{{y}}.pbf"]}},
                "north": {{"type": "vector", "maxzoom": 0, "bounds": [-80, 10, 80, 60], "tiles": ["north/{{z}}-{{x}}-{{y}}.pbf"]}},
                "world": {{
                    "type": "geojson",
                    "maxzoom": 0,
                    "data": {{"type": "Polygon", "coordinates": [[[-180, -85], [180, -85], [180, 85], [-180, 85], [-180, -85]]]}}
                }}
            }},
            "layers": [{}]
        }}"#,
        layers.join(",")
    )
}

/// The still image of the map drawn with `style`.
fn render(style: &str) -> PremultipliedImage {
    let map = StillMap::new(StillOptions { size: Size::new(WIDTH, HEIGHT), ..StillOptions::default() });
    serve(&map.runtime, |resource| match resource.kind {
        Kind::Tile => file_response("test/fixtures/map/offline/0-0-0.vector.pbf"),
        _ => not_found(),
    });
    map.load_json(style);
    map.render()
}

/// The columns from `left` of `image`, a world copy's half wide.
fn columns(image: &PremultipliedImage, left: usize) -> Vec<u8> {
    let row = WIDTH as usize * 4;
    let (from, to) = (left * 4, (left + WORLD / 2) * 4);
    image.data.chunks_exact(row).flat_map(|pixels| &pixels[from..to]).copied().collect()
}

/// Each world copy beside the central one draws as the central one does
/// over the same longitudes, in a frame with more clipped tiles than the
/// stencil buffer has values.
#[test]
fn the_world_copies_beside_the_central_one_draw_alike_after_the_stencil_overflows() {
    let image = render(&style(42));
    let differing = |copy: usize, central: usize| {
        let (copy, central) = (columns(&image, copy), columns(&image, central));
        image_diff::differing_pixels(&copy, &central, WORLD as u32 / 2, HEIGHT, 0.1, None)
    };
    assert_eq!(0, differing(0, WORLD), "the east half of copy -1");
    assert_eq!(0, differing(3 * WORLD / 2, WORLD / 2), "the west half of copy 1");
}
