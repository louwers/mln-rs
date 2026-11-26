//! The installed fixture snapshot, as scenarios read it: what
//! `mln_bench_workload::Snapshot` reads, single tiles, and a file source that
//! resolves the style's `asset://` URLs in it.

use std::ops::Deref;
use std::sync::Arc;

use mln_bench_workload::Snapshot;
pub(crate) use mln_bench_workload::Tiles;
use mln_engine::storage::{FileSource, Loader};
use mln_util::pool::Pool;

use crate::scenario::Bench;

/// The installed snapshot, which scenarios read through [`Snapshot`]'s
/// methods.
pub(crate) struct Fixtures(Snapshot);

impl Deref for Fixtures {
    type Target = Snapshot;

    fn deref(&self) -> &Snapshot {
        &self.0
    }
}

impl Fixtures {
    /// The snapshot this harness's manifest names, which `bench`'s
    /// environment then records, so that runs on other fixtures get no
    /// comparison with this one. Panics, failing the scenario, when the
    /// snapshot isn't installed.
    pub(crate) fn open(bench: &mut Bench) -> Fixtures {
        let snapshot = Snapshot::find().unwrap_or_else(|error| panic!("{error}"));
        bench.fixtures = Some(snapshot.manifest().hash.clone());
        Fixtures(snapshot)
    }

    /// The tile `z`/`x`/`y` of `tiles`, decompressed: MVT or MLT bytes as a
    /// tile worker receives them. `None` for a tile the set lacks.
    pub(crate) fn tile(&self, tiles: Tiles, z: u8, x: u32, y: u32) -> Option<Vec<u8>> {
        let path = self.path(&self.tile_set(tiles).archive);
        let reader =
            pmtiles::PmTilesReader::new_with_path(&path).unwrap_or_else(|error| panic!("{}: {error}", path.display()));
        let coord = pmtiles::TileCoord::new(z, x, y).expect("a tile in the world");
        let tile = reader.get_tile_decompressed(coord).unwrap_or_else(|error| panic!("{}: {error}", path.display()));
        tile.map(|bytes| bytes.to_vec())
    }

    /// A file source that loads `asset://` URLs from the snapshot and local
    /// files on `pool`, and nothing from the network.
    pub(crate) fn file_source(&self, pool: &Pool) -> Arc<dyn FileSource> {
        let dir = self.dir().to_str().expect("the fixtures' directory is UTF-8");
        Arc::new(Loader::builder(pool).asset_path(dir).build())
    }
}
