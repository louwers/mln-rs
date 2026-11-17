//! The installed fixture snapshot (`crate::fixtures`), as scenarios read it:
//! its files, the Liberty style over one of its tile sets, single tiles, and
//! a file source that resolves the style's `asset://` URLs in it.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use mln_actor::scheduler::SchedulerPtr;
use mln_storage::{FileSource, Loader};

use crate::fixtures::{CACHE_VARIABLE, Manifest, cache_root};
use crate::scenario::Bench;

/// The snapshot's tile sets.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Tiles {
    /// Central San Francisco, zooms 0 to 14.
    Street,
    /// The street tiles as MLT.
    #[expect(dead_code, reason = "no scenario reads the MLT tiles")]
    StreetMlt,
    /// The whole world, zooms 0 to 4.
    World,
    /// Central Tokyo, zooms 0 to 14, its names in Japanese script.
    Cjk,
}

impl Tiles {
    fn name(self) -> &'static str {
        match self {
            Tiles::Street => "street",
            Tiles::StreetMlt => "street-mlt",
            Tiles::World => "world",
            Tiles::Cjk => "cjk",
        }
    }
}

/// The installed snapshot.
pub(crate) struct Fixtures {
    dir: PathBuf,
    manifest: &'static Manifest,
}

impl Fixtures {
    /// The snapshot this harness's manifest names, which `bench`'s
    /// environment then records, so that runs on other fixtures get no
    /// verdicts against this one. Panics, failing the scenario, when the
    /// snapshot isn't installed.
    pub(crate) fn open(bench: &mut Bench) -> Fixtures {
        let fixtures = Fixtures::find().unwrap_or_else(|error| panic!("{error}"));
        bench.fixtures = Some(fixtures.manifest.hash.clone());
        fixtures
    }

    fn find() -> Result<Fixtures, String> {
        let manifest = Manifest::current();
        let root = cache_root()?;
        Fixtures::find_in(manifest, &root)
    }

    fn find_in(manifest: &'static Manifest, root: &Path) -> Result<Fixtures, String> {
        let dir = root.join(&manifest.hash);
        if !dir.is_dir() {
            return Err(format!(
                "the benchmark fixtures (snapshot {}) aren't installed in {}: run `mise run bench:fixtures` \
                 ({CACHE_VARIABLE} names another directory)",
                manifest.snapshot,
                root.display()
            ));
        }
        Ok(Fixtures { dir, manifest })
    }

    /// The directory the snapshot is installed in.
    #[expect(dead_code, reason = "no scenario reads the directory itself")]
    pub(crate) fn dir(&self) -> &Path {
        &self.dir
    }

    /// The file at `path` in the snapshot, such as
    /// `geojson/ne_10m_admin_0_countries.geojson`. Panics for a path the
    /// manifest doesn't name.
    pub(crate) fn path(&self, path: &str) -> PathBuf {
        assert!(self.manifest.file(path).is_some(), "the fixtures have no {path}");
        self.dir.join(path)
    }

    pub(crate) fn read(&self, path: &str) -> Vec<u8> {
        let path = self.path(path);
        std::fs::read(&path).unwrap_or_else(|error| panic!("{}: {error}", path.display()))
    }

    /// OpenFreeMap's Liberty style, its vector source on `tiles`, as JSON.
    /// Its URLs are `asset://` URLs that [`Fixtures::file_source`] resolves.
    pub(crate) fn style(&self, tiles: Tiles) -> String {
        self.style_value(tiles).to_string()
    }

    /// [`Fixtures::style`] as a JSON value, for a scenario to change.
    pub(crate) fn style_value(&self, tiles: Tiles) -> serde_json::Value {
        let mut style: serde_json::Value =
            serde_json::from_slice(&self.read("styles/liberty.json")).expect("the style is JSON");
        let url = format!("pmtiles://asset://{}", self.tile_set(tiles).archive);
        style["sources"]["openmaptiles"]["url"] = url.into();
        style
    }

    /// The manifest's description of a tile set: its archive, its zooms, and
    /// the area a camera can center on and see only tiles of the set.
    pub(crate) fn tile_set(&self, tiles: Tiles) -> &'static crate::fixtures::TileSet {
        &self.manifest.tile_sets[tiles.name()]
    }

    /// The tile `z`/`x`/`y` of `tiles`, decompressed: MVT or MLT bytes as a
    /// tile worker receives them. `None` for a tile the set lacks.
    #[expect(dead_code, reason = "no scenario reads single tiles")]
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
    pub(crate) fn file_source(&self, pool: SchedulerPtr) -> Arc<dyn FileSource> {
        let dir = self.dir.to_str().expect("the fixtures' directory is UTF-8");
        Arc::new(Loader::builder().asset_path(dir).pool(pool).build())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_missing_snapshot_names_the_command_that_installs_it() {
        let root = tempfile::tempdir().unwrap();
        let Err(error) = Fixtures::find_in(Manifest::current(), root.path()) else {
            panic!("an empty directory holds no snapshot");
        };
        assert!(error.contains("run `mise run bench:fixtures`"), "{error}");
        assert!(error.contains(&root.path().display().to_string()), "{error}");
    }
}
