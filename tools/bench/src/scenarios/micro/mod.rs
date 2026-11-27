//! One scenario per pipeline stage, alone: a function over data, with no
//! map, GPU or threads, or for drawing, one layer type over loaded data.

pub(crate) mod decode;
pub(crate) mod draw;
pub(crate) mod expression;
pub(crate) mod feature_state;
pub(crate) mod geojson;
pub(crate) mod layout;
pub(crate) mod queue;
pub(crate) mod style;
pub(crate) mod symbols;
pub(crate) mod text;
pub(crate) mod worker_codec;

use std::sync::Arc;

use mln_engine::bench::{LaidOut, TileLayout};
use mln_engine::tile::{MltTile, MvtTile, TileData};
use mln_style::{Edit, Style, StyleRegistry};

use crate::support::fixtures::{Fixtures, Tiles};

/// A street tile that the scenarios of tiles work on, and how many times a
/// run works on it: enough that a run counts closely, as a sparse tile
/// takes little work.
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

/// Tile `id` of `tiles`: MVT bytes, or MLT for `Tiles::StreetMlt`.
pub(crate) fn tile_bytes(fixtures: &Fixtures, tiles: Tiles, (z, x, y): (u8, u32, u32)) -> Arc<Vec<u8>> {
    Arc::new(fixtures.tile(tiles, z, x, y).unwrap_or_else(|| panic!("the fixtures have no tile {z}/{x}/{y}")))
}

/// Tile data of `bytes`, read as a map's tile loader reads them: MLT
/// without FastPFOR, which a map reads only when it enables it.
pub(crate) fn tile_data(tiles: Tiles, bytes: &Arc<Vec<u8>>) -> TileData {
    match tiles {
        Tiles::StreetMlt => TileData::Mlt(MltTile::new(Arc::clone(bytes), false)),
        Tiles::Street | Tiles::World | Tiles::Cjk | Tiles::Flight => TileData::Mvt(MvtTile::new(Arc::clone(bytes))),
        Tiles::Terrain => panic!("the terrain tiles are images, not vector tiles"),
    }
}

/// What a map's styles parse against.
pub(crate) fn registry() -> Arc<StyleRegistry> {
    Arc::clone(mln_builtins::registry().style())
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
    Style::from_json(&registry(), &liberty(fixtures, copies)).expect("Liberty parses")
}

/// Liberty as a map holds it once it has loaded: [`parse_liberty`], with
/// the images of its sprite.
pub(crate) fn loaded_liberty(fixtures: &Fixtures) -> Style {
    with_sprite(fixtures, parse_liberty(fixtures, 1))
}

/// `style` with the images of its sprite, as a map's style loader adds them.
pub(crate) fn with_sprite(fixtures: &Fixtures, mut style: Style) -> Style {
    let sprite = style.get_sprites().first().expect("Liberty has a sprite").sprite_url.clone();
    let path = asset_path(&sprite);
    let png = mln_engine::image::decode_png(&fixtures.read(&format!("{path}.png"))).expect("the sprite's image");
    let index = fixtures.read(&format!("{path}.json"));
    let images = mln_engine::image::sprite::parse_sprite("default", &png, &index).expect("the sprite's index");
    style.apply(Edit::AddImages(images)).expect("the sprite's images");
    style
}

/// A street tile's layout job, as a tile worker gets it, and the tile's
/// bytes, which each run of the job reads afresh.
pub(crate) struct StreetLayout {
    pub(crate) job: TileLayout,
    bytes: Arc<Vec<u8>>,
}

impl StreetLayout {
    /// The layout of the street tile `id` by those of `style`'s layers on
    /// its vector source whose type `include` accepts, with the glyphs and
    /// images it needs resolved from the snapshot.
    pub(crate) fn new(fixtures: &Fixtures, style: &Style, include: impl Fn(&str) -> bool, id: (u8, u32, u32)) -> Self {
        StreetLayout::shown_at(fixtures, style, include, id, id.0)
    }

    /// [`StreetLayout::new`] for the tile shown at zoom `zoom`, at or above
    /// its own, as a map shows the street tiles' highest zoom when it zooms
    /// in further.
    pub(crate) fn shown_at(
        fixtures: &Fixtures,
        style: &Style,
        include: impl Fn(&str) -> bool,
        id: (u8, u32, u32),
        zoom: u8,
    ) -> Self {
        let bytes = tile_bytes(fixtures, Tiles::Street, id);
        let load = |url: &str| Some(fixtures.read(&asset_path(url)));
        let job =
            TileLayout::overscaled(style, "openmaptiles", include, id, zoom, &tile_data(Tiles::Street, &bytes), load)
                .unwrap_or_else(|error| panic!("{error}"));
        StreetLayout { job, bytes }
    }

    /// The tile's data, not yet decoded.
    pub(crate) fn data(&self) -> TileData {
        tile_data(Tiles::Street, &self.bytes)
    }

    /// The tile, laid out.
    pub(crate) fn run(&self) -> LaidOut {
        self.job.run(self.data()).unwrap_or_else(|error| panic!("{error}"))
    }
}

/// The snapshot's path of an `asset://` URL of the style.
pub(crate) fn asset_path(url: &str) -> String {
    let path = url.strip_prefix("asset://").unwrap_or_else(|| panic!("{url} isn't in the snapshot"));
    percent_decode(path)
}

/// `text` with each `%XX` turned into the byte it stands for.
fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        let hex =
            bytes.get(index + 1..index + 3).and_then(|hex| u8::from_str_radix(std::str::from_utf8(hex).ok()?, 16).ok());
        match (bytes[index], hex) {
            (b'%', Some(byte)) => {
                decoded.push(byte);
                index += 3;
            }
            (byte, _) => {
                decoded.push(byte);
                index += 1;
            }
        }
    }
    String::from_utf8(decoded).expect("a URL of UTF-8")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn asset_urls_name_snapshot_paths() {
        assert_eq!(asset_path("asset://glyphs/Noto%20Sans%20Regular/0-255.pbf"), "glyphs/Noto Sans Regular/0-255.pbf");
        assert_eq!(asset_path("asset://sprites/ofm_f384/ofm"), "sprites/ofm_f384/ofm");
    }
}
