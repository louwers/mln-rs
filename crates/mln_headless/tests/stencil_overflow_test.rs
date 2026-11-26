// A frame with more clipped tiles than stencil values clears the stencil
// buffer and starts again: in every world copy it shows, not only the
// central one.

use std::sync::Arc;

use mln_engine::map::{Map, MapMode, MapOptions};
use mln_engine::storage::stub_file_source::{ResponseType, StubFileSource};
use mln_engine::storage::{Kind, Resource};
use mln_headless::HeadlessFrontend;
use mln_style::bitmap::PremultipliedImage;
use mln_style::error::ErrorKind;
use mln_style::size::Size;
use mln_util::pool::Pool;
use mln_util::queue::Queue;

use crate::support::file_response;

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

/// The map drawn with `style`, and how many times its frame cleared the
/// stencil buffer; `None` without a device.
fn render(style: &str) -> Option<(PremultipliedImage, i32)> {
    let size = Size::new(WIDTH, HEIGHT);
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
    file_source
        .set_response(Kind::Tile, |_: &Resource| Some(file_response("test/fixtures/map/offline/0-0-0.vector.pbf")));
    let options = MapOptions { map_mode: MapMode::Static, size, pixel_ratio: 1.0, ..Default::default() };
    let mut map = Map::with_file_source(file_source, options, &pool).unwrap();
    frontend.attach(&mut map);
    map.load_style_json(style);
    let camera = mln_engine::map::style_camera(map.style());
    map.jump_to(&camera);
    let result = frontend.render(&mut map).expect("the still image");
    Some((result.image, result.stats.stencil_clears))
}

/// The columns from `left` of `image`, a world copy's half wide.
fn columns(image: &PremultipliedImage, left: usize) -> Vec<u8> {
    let row = WIDTH as usize * 4;
    let (from, to) = (left * 4, (left + WORLD / 2) * 4);
    image.data.chunks_exact(row).flat_map(|pixels| &pixels[from..to]).copied().collect()
}

/// Each world copy beside the central one draws as the central one does
/// over the same longitudes, after the stencil values overflowed.
#[test]
fn the_world_copies_beside_the_central_one_draw_alike_after_the_stencil_overflows() {
    let Some((image, clears)) = render(&style(42)) else {
        return;
    };
    assert!(clears > 0, "the stencil values overflowed");
    let differing = |copy: usize, central: usize| {
        let (copy, central) = (columns(&image, copy), columns(&image, central));
        pixelmatch::pixelmatch(&copy, &central, WORLD / 2, HEIGHT as usize, None, 0.1, false)
    };
    assert_eq!(0, differing(0, WORLD), "the east half of copy -1");
    assert_eq!(0, differing(3 * WORLD / 2, WORLD / 2), "the west half of copy 1");
}
