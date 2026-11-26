//! The tile worker codec: a tile's layout job and the laid-out tile that
//! answers it, each encoded into bytes on one side and decoded on the
//! other, as they cross between the map and a worker instance in the
//! browser build, where each instance has a memory of its own.
//!
//! The codec is the renderer's, reached through its `bench` entry points
//! (`mln_engine::bench`).

use mln_engine::bench::Codec;

use super::{DENSE, SPARSE, StreetLayout, StreetTile, loaded_liberty};
use crate::scenario::{Bench, Kind, Scenario};
use crate::support::fixtures::Fixtures;

pub(crate) const SCENARIOS: &[Scenario] = &[
    Scenario {
        name: "micro/worker-codec/tile/sparse",
        kind: Kind::Micro,
        question: "What does a sparse tile's job and its laid-out reply cost to cross between the map and a worker instance?",
        run: tile_sparse,
    },
    Scenario {
        name: "micro/worker-codec/tile/dense",
        kind: Kind::Micro,
        question: "What does a dense tile's job and its laid-out reply cost to cross between the map and a worker instance?",
        run: tile_dense,
    },
];

/// The sparse street tile ([`SPARSE`]) with all of Liberty's layers: its
/// job (the tile's data, and keys of the style's layers, which an earlier
/// job of the instance defined) encoded on the map side and decoded on the
/// worker's, then the worker's reply (the tile's buckets, their vertices and
/// indices as payloads beside the bytes, and its feature index) encoded on
/// the worker's side and decoded on the map's. The layout between the two
/// isn't measured.
fn tile_sparse(bench: &mut Bench) {
    cross(bench, SPARSE);
}

/// [`tile_sparse`] for the dense tile ([`DENSE`]).
fn tile_dense(bench: &mut Bench) {
    cross(bench, DENSE);
}

fn cross(bench: &mut Bench, tile: StreetTile) {
    let fixtures = Fixtures::open(bench);
    let layout = StreetLayout::new(&fixtures, &loaded_liberty(&fixtures), |_| true, tile.id);
    let mut codec = Codec::new();
    // The instance's first job defines the style's layers there, once.
    let job = codec.send(&layout.job, layout.data()).unwrap_or_else(|error| panic!("{error}"));
    let reply = job.run();
    let buckets = codec.reply(&reply).unwrap_or_else(|error| panic!("{error}")).buckets();
    assert!(buckets > 0, "the tile has buckets");

    for _ in 0..tile.repeats {
        let job = bench.measure(|| codec.send(&layout.job, layout.data()));
        drop(job.unwrap_or_else(|error| panic!("{error}")));
        let laid_out = bench.measure(|| codec.reply(&reply));
        assert_eq!(laid_out.unwrap_or_else(|error| panic!("{error}")).buckets(), buckets);
    }
    bench.per("tile", tile.repeats);
}
