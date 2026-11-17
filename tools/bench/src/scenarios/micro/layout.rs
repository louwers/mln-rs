//! A vector tile laid out into buckets by the layers of one type, as the job
//! that a tile worker runs: the layers' features selected and evaluated, and
//! their vertices, indices and paint data built, with the feature index.
//!
//! The layout is the renderer's, reached through its `bench` entry points
//! (`mln_renderer::bench`).

use std::sync::Arc;

use mln_renderer::bench::TileLayout;
use mln_style::{Edit, Style};

use super::{DENSE, SPARSE, StreetTile, parse_liberty, tile_bytes, tile_data};
use crate::scenario::{Bench, Kind, Scenario};
use crate::support::fixtures::{Fixtures, Tiles};

pub(crate) const SCENARIOS: &[Scenario] = &[
    Scenario {
        name: "micro/layout/fill/sparse",
        kind: Kind::Micro,
        question: "What does laying out a sparse tile's fill layers cost?",
        run: fill_sparse,
    },
    Scenario {
        name: "micro/layout/fill/dense",
        kind: Kind::Micro,
        question: "What does laying out a dense tile's fill layers cost?",
        run: fill_dense,
    },
    Scenario {
        name: "micro/layout/line/sparse",
        kind: Kind::Micro,
        question: "What does laying out a sparse tile's line layers cost?",
        run: line_sparse,
    },
    Scenario {
        name: "micro/layout/line/dense",
        kind: Kind::Micro,
        question: "What does laying out a dense tile's line layers cost?",
        run: line_dense,
    },
    Scenario {
        name: "micro/layout/fill-extrusion/sparse",
        kind: Kind::Micro,
        question: "What does laying out a sparse tile's extruded buildings cost?",
        run: fill_extrusion_sparse,
    },
    Scenario {
        name: "micro/layout/fill-extrusion/dense",
        kind: Kind::Micro,
        question: "What does laying out a dense tile's extruded buildings cost?",
        run: fill_extrusion_dense,
    },
    Scenario {
        name: "micro/layout/symbol/sparse",
        kind: Kind::Micro,
        question: "What does laying out a sparse tile's labels and icons cost?",
        run: symbol_sparse,
    },
    Scenario {
        name: "micro/layout/symbol/dense",
        kind: Kind::Micro,
        question: "What does laying out a dense tile's labels and icons cost?",
        run: symbol_dense,
    },
];

/// The sparse street tile ([`SPARSE`]) laid out by Liberty's 16 fill
/// layers: land use and cover, water, parks and buildings.
fn fill_sparse(bench: &mut Bench) {
    lay_out(bench, "fill", SPARSE);
}

/// [`fill_sparse`] for the dense tile ([`DENSE`]).
fn fill_dense(bench: &mut Bench) {
    lay_out(bench, "fill", DENSE);
}

/// The sparse street tile laid out by Liberty's 67 line layers: roads by
/// class, with casings, dashes and bridges, rails, waterways and borders.
fn line_sparse(bench: &mut Bench) {
    lay_out(bench, "line", SPARSE);
}

/// [`line_sparse`] for the dense tile.
fn line_dense(bench: &mut Bench) {
    lay_out(bench, "line", DENSE);
}

/// The sparse street tile laid out by Liberty's fill-extrusion layer, its
/// buildings in 3D.
fn fill_extrusion_sparse(bench: &mut Bench) {
    lay_out(bench, "fill-extrusion", SPARSE);
}

/// [`fill_extrusion_sparse`] for the dense tile.
fn fill_extrusion_dense(bench: &mut Bench) {
    lay_out(bench, "fill-extrusion", DENSE);
}

/// The sparse street tile laid out by Liberty's 25 symbol layers: the
/// labels of places, roads, water and points of interest, the icons of
/// points of interest and road shields, their text shaped with the
/// snapshot's glyphs and their icons from its sprite.
fn symbol_sparse(bench: &mut Bench) {
    lay_out(bench, "symbol", SPARSE);
}

/// [`symbol_sparse`] for the dense tile.
fn symbol_dense(bench: &mut Bench) {
    lay_out(bench, "symbol", DENSE);
}

fn lay_out(bench: &mut Bench, layer_type: &str, tile: StreetTile) {
    let fixtures = Fixtures::open(bench);
    let style = with_sprite(&fixtures, parse_liberty(&fixtures, 1));
    let bytes = tile_bytes(&fixtures, Tiles::Street, tile.id);
    let load = |url: &str| Some(fixtures.read(&asset_path(url)));
    let layout = TileLayout::new(
        &style,
        "openmaptiles",
        |type_| type_ == layer_type,
        tile.id,
        &tile_data(Tiles::Street, &bytes),
        load,
    )
    .unwrap_or_else(|error| panic!("{error}"));

    for _ in 0..tile.repeats {
        let laid_out = bench.measure(|| layout.run(tile_data(Tiles::Street, &Arc::clone(&bytes))));
        let laid_out = laid_out.unwrap_or_else(|error| panic!("{error}"));
        assert!(laid_out.buckets() > 0, "the tile has features for the {layer_type} layers");
    }
    bench.per("tile", tile.repeats);
}

/// `style` with the images of its sprite, as a map's style loader adds them.
fn with_sprite(fixtures: &Fixtures, mut style: Style) -> Style {
    let sprite = style.get_sprites().first().expect("Liberty has a sprite").sprite_url.clone();
    let path = asset_path(&sprite);
    let png = mln_image::decode_png(&fixtures.read(&format!("{path}.png"))).expect("the sprite's image");
    let index = fixtures.read(&format!("{path}.json"));
    let images = mln_sprite::parse_sprite("default", &png, &index).expect("the sprite's index");
    style.apply(Edit::AddImages(images)).expect("the sprite's images");
    style
}

/// The snapshot's path of an `asset://` URL of the style.
fn asset_path(url: &str) -> String {
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
