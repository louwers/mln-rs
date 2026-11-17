//! One scenario per pipeline stage: a function over data, with no map, GPU
//! or threads.

pub(crate) mod decode;
pub(crate) mod expression;
pub(crate) mod geojson;
pub(crate) mod layout;
pub(crate) mod style;

use std::sync::Arc;

use mln_style::{Style, StyleRegistry};
use mln_tile::{MltTile, MvtTile, TileData};

use crate::support::fixtures::{Fixtures, Tiles};

/// A street tile that the decode and layout scenarios work on, and how many
/// times a run works on it: enough that a run counts closely, as a sparse
/// tile takes little work.
#[derive(Clone, Copy)]
pub(crate) struct StreetTile {
    pub(crate) id: (u8, u32, u32),
    pub(crate) repeats: u64,
}

/// The street tile with the most features: 14/2621/6332, San Francisco's
/// South of Market, 6,919 features, most of them points of interest, with
/// a thousand road segments and a hundred and fifty buildings.
pub(crate) const DENSE: StreetTile = StreetTile { id: (14, 2621, 6332), repeats: 10 };

/// A street tile of the same zoom with every kind of feature, but few:
/// 14/2622/6336, Candlestick Point on the bay's shore, 186 features.
pub(crate) const SPARSE: StreetTile = StreetTile { id: (14, 2622, 6336), repeats: 100 };

/// The tile `id` of the street tiles: MVT bytes, or MLT for `Tiles::StreetMlt`.
pub(crate) fn tile_bytes(fixtures: &Fixtures, tiles: Tiles, (z, x, y): (u8, u32, u32)) -> Arc<Vec<u8>> {
    Arc::new(fixtures.tile(tiles, z, x, y).unwrap_or_else(|| panic!("the fixtures have no tile {z}/{x}/{y}")))
}

/// Tile data of `bytes`, read as a map's tile loader reads them: MLT
/// without FastPFOR, which a map reads only when it enables it.
pub(crate) fn tile_data(tiles: Tiles, bytes: &Arc<Vec<u8>>) -> TileData {
    match tiles {
        Tiles::StreetMlt => TileData::Mlt(MltTile::new(Arc::clone(bytes), false)),
        Tiles::Street | Tiles::World | Tiles::Cjk => TileData::Mvt(MvtTile::new(Arc::clone(bytes))),
    }
}

/// What a map's styles parse against.
pub(crate) fn registry() -> Arc<StyleRegistry> {
    Arc::clone(mln_renderer::Registry::with_builtins().style())
}

/// The Liberty style over the street tiles, as JSON, with its layers
/// `copies` times over: each copy after the first has the layers' ids with
/// `-<copy>` after them, in the same order above the ones before. Each copy
/// draws what the first does, as a style of many layers does.
pub(crate) fn liberty(fixtures: &Fixtures, copies: usize) -> String {
    let mut style = fixtures.style_value(Tiles::Street);
    let layers = style["layers"].as_array().expect("a style's layers").clone();
    let mut all = layers.clone();
    for copy in 1..copies {
        for layer in &layers {
            let mut layer = layer.clone();
            let id = format!("{}-{copy}", layer["id"].as_str().expect("a layer's id"));
            layer["id"] = id.into();
            all.push(layer);
        }
    }
    style["layers"] = all.into();
    style.to_string()
}

/// [`liberty`], parsed.
pub(crate) fn parse_liberty(fixtures: &Fixtures, copies: usize) -> Style {
    Style::from_json(&registry(), &liberty(fixtures, copies), None).expect("Liberty parses")
}
