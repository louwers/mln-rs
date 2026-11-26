//! Tiles rendered as a tile server renders them: one map in tile mode and
//! its renderer, reused for each tile, drawing tiles it hasn't drawn
//! before as still images and reading them back.

use mln_engine::camera::CameraOptions;
use mln_engine::map::MapMode;
use mln_style::size::Size;

use crate::scenario::{Bench, Kind, Scenario};
use crate::support::fixtures::{Fixtures, Tiles};
use crate::support::map::{HeadlessMap, save_png, tile_at, tile_center};

pub(crate) const SCENARIOS: &[Scenario] = &[Scenario {
    name: "macro/tile-server/street",
    kind: Kind::Macro,
    question: "What does a tile server's renderer spend on each new tile of a city, from zoom 7 to 14?",
    run: street,
}];

/// A tile mode map shows exactly one 512-pixel tile.
const TILE: Size = Size { width: 512, height: 512 };

/// The tiles each zoom renders, as offsets from the tile holding the street
/// tiles' center: a 2 x 2 block at the last zoom, where a server gets the
/// most requests, two at the two before, and one at each other zoom.
const TILES: &[(u8, &[(u32, u32)])] = &[
    (7, &[(0, 0)]),
    (8, &[(0, 0)]),
    (9, &[(0, 0)]),
    (10, &[(0, 0)]),
    (11, &[(0, 0)]),
    (12, &[(0, 0), (1, 0)]),
    (13, &[(0, 0), (1, 0)]),
    (14, &[(0, 0), (1, 0), (0, 1), (1, 1)]),
];

/// The question: what does rendering one new tile of central San Francisco
/// cost a renderer that has the style loaded and has drawn other tiles?
/// One measured unit is one tile: moving the map to it, laying out its
/// source tile, placing its symbols, drawing it and reading its pixels
/// back. Thirteen tiles of zooms 7 to 14 run; the style, its sprite and a
/// first tile load before.
fn street(bench: &mut Bench) {
    let fixtures = Fixtures::open(bench);
    let center = fixtures.area_center(Tiles::Street);
    let camera = |z: u8, x: u32, y: u32| CameraOptions {
        center: Some(tile_center(z, x, y)),
        zoom: Some(f64::from(z)),
        ..Default::default()
    };
    let tiles: Vec<(u8, u32, u32)> = TILES
        .iter()
        .flat_map(|&(z, offsets)| {
            let (x, y) = tile_at(z, center);
            offsets.iter().map(move |&(dx, dy)| (z, x + dx, y + dy))
        })
        .collect();

    // A server's renderer has its style loaded: a tile of zoom 6, which
    // the measured tiles don't reuse.
    let (x, y) = tile_at(6, center);
    let style = fixtures.style(Tiles::Street);
    let mut map = HeadlessMap::open(bench, &fixtures, TILE, MapMode::Tile, &style, &camera(6, x, y));
    map.render_still();

    let mut image = None;
    for &(z, x, y) in &tiles {
        image = Some(bench.measure(|| {
            map.jump(&camera(z, x, y));
            map.render_still()
        }));
    }
    bench.per_part("tile");
    bench.bytes("gpu_buffers", map.gpu_buffer_bytes());
    if let (Some(path), Some(image)) = (&bench.image, image) {
        save_png(&image, path);
    }
}
