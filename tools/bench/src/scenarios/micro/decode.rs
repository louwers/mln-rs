//! Vector tiles decoded, MVT and MLT, as a layout reads them: each source
//! layer that the style's layers draw, its features, the filters that select
//! them, and the geometry of those selected.

use std::hint::black_box;
use std::sync::Arc;

use mln_engine::tile::TileData;
use mln_style::expression::EvaluationContext;
use mln_style::filter::Filter;
use mln_style::types::VisibilityType;

use super::{DENSE, SPARSE, StreetTile, parse_liberty, tile_bytes, tile_data};
use crate::scenario::{Bench, Kind, Scenario};
use crate::support::fixtures::{Fixtures, Tiles};

pub(crate) const SCENARIOS: &[Scenario] = &[
    Scenario {
        name: "micro/decode/mvt/sparse",
        kind: Kind::Micro,
        question: "What does decoding a sparse MVT tile cost, its features selected by a style's filters?",
        run: mvt_sparse,
    },
    Scenario {
        name: "micro/decode/mvt/dense",
        kind: Kind::Micro,
        question: "What does decoding a dense MVT tile cost, its features selected by a style's filters?",
        run: mvt_dense,
    },
    Scenario {
        name: "micro/decode/mlt/sparse",
        kind: Kind::Micro,
        question: "What does decoding a sparse MLT tile cost, its features selected by a style's filters?",
        run: mlt_sparse,
    },
    Scenario {
        name: "micro/decode/mlt/dense",
        kind: Kind::Micro,
        question: "What does decoding a dense MLT tile cost, its features selected by a style's filters?",
        run: mlt_dense,
    },
];

/// The sparse street tile ([`SPARSE`]) as MVT, decoded from its bytes: for
/// each source layer that Liberty's visible layers of zoom 14 draw, every
/// feature, the filters of those layers evaluated on it, and the geometry
/// of each feature that one selects.
fn mvt_sparse(bench: &mut Bench) {
    decode(bench, Tiles::Street, SPARSE);
}

/// [`mvt_sparse`] for the dense tile ([`DENSE`]).
fn mvt_dense(bench: &mut Bench) {
    decode(bench, Tiles::Street, DENSE);
}

/// [`mvt_sparse`] for the same tile as MLT.
fn mlt_sparse(bench: &mut Bench) {
    decode(bench, Tiles::StreetMlt, SPARSE);
}

/// [`mvt_dense`] for the same tile as MLT.
fn mlt_dense(bench: &mut Bench) {
    decode(bench, Tiles::StreetMlt, DENSE);
}

fn decode(bench: &mut Bench, tiles: Tiles, tile: StreetTile) {
    let fixtures = Fixtures::open(bench);
    let bytes = tile_bytes(&fixtures, tiles, tile.id);
    let zoom = f32::from(tile.id.0);
    let style = parse_liberty(&fixtures, 1);
    // Each source layer the style draws at the tile's zoom, with the
    // filters of its layers.
    let mut source_layers: Vec<(String, Vec<Filter>)> = Vec::new();
    for layer in style.get_layers() {
        let drawn = !layer.get_source_layer().is_empty()
            && layer.get_visibility() != VisibilityType::None
            && zoom >= layer.get_min_zoom().floor()
            && zoom < layer.get_max_zoom().ceil();
        if !drawn {
            continue;
        }
        let filter = layer.get_filter();
        match source_layers.iter_mut().find(|(name, _)| name == layer.get_source_layer()) {
            Some((_, filters)) => filters.push(filter),
            None => source_layers.push((layer.get_source_layer().to_owned(), vec![filter])),
        }
    }

    let mut selected = 0;
    for _ in 0..tile.repeats {
        selected = bench.measure(|| select(&tile_data(tiles, &Arc::clone(&bytes)), &source_layers, zoom));
    }
    assert!(selected > 0, "the filters select some of the tile's features");
    bench.per("tile", tile.repeats);
}

/// The features of `data` that any of their source layer's filters selects,
/// each with its geometry decoded.
fn select(data: &TileData, source_layers: &[(String, Vec<Filter>)], zoom: f32) -> usize {
    let mut selected = 0;
    for (name, filters) in source_layers {
        let Some(layer) = data.layer(name).expect("a tile that decodes") else { continue };
        for index in 0..layer.len() {
            let feature = layer.feature(index).expect("a feature that decodes");
            let context = EvaluationContext::with_zoom_feature(zoom, Some(&feature));
            if filters.iter().any(|filter| filter.evaluate(&context)) {
                black_box(feature.geometry().expect("a geometry that decodes"));
                selected += 1;
            }
        }
    }
    selected
}
