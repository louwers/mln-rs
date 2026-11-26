//! A vector tile laid out into buckets by the layers of one type, as the job
//! that a tile worker runs: the layers' features selected and evaluated, and
//! their vertices, indices and paint data built, with the feature index.
//!
//! The layout is the renderer's, reached through its `bench` entry points
//! (`mln_engine::bench`).

use super::{DENSE, SPARSE, StreetLayout, StreetTile, loaded_liberty};
use crate::scenario::{Bench, Kind, Scenario};
use crate::support::fixtures::Fixtures;

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
    let layout = StreetLayout::new(&fixtures, &loaded_liberty(&fixtures), |type_| type_ == layer_type, tile.id);
    for _ in 0..tile.repeats {
        let laid_out = bench.measure(|| layout.job.run(layout.data()));
        let laid_out = laid_out.unwrap_or_else(|error| panic!("{error}"));
        assert!(laid_out.buckets() > 0, "the tile has features for the {layer_type} layers");
    }
    bench.per("tile", tile.repeats);
}
