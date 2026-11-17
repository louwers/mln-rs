//! The installed fixture snapshot, as a map reads it.

use std::path::{Path, PathBuf};

use mln_util::geo::LatLng;

use crate::manifest::{CACHE_VARIABLE, Manifest, TileSet, cache_root};

/// The snapshot's tile sets.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tiles {
    /// Central San Francisco, zooms 0 to 14.
    Street,
    /// The street tiles as MLT.
    StreetMlt,
    /// The whole world, zooms 0 to 4.
    World,
    /// Central Tokyo, zooms 0 to 14, its names in Japanese script.
    Cjk,
}

impl Tiles {
    /// The set's name in the manifest.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Tiles::Street => "street",
            Tiles::StreetMlt => "street-mlt",
            Tiles::World => "world",
            Tiles::Cjk => "cjk",
        }
    }
}

/// The installed snapshot of [`Manifest::current`].
pub struct Snapshot {
    dir: PathBuf,
    manifest: &'static Manifest,
}

impl Snapshot {
    /// The snapshot, where it's installed. Fails, naming the command that
    /// installs it, when it isn't.
    pub fn find() -> Result<Snapshot, String> {
        Snapshot::find_in(Manifest::current(), &cache_root()?)
    }

    fn find_in(manifest: &'static Manifest, root: &Path) -> Result<Snapshot, String> {
        let dir = root.join(&manifest.hash);
        if !dir.is_dir() {
            return Err(format!(
                "the benchmark fixtures (snapshot {}) aren't installed in {}: run `mise run bench:fixtures` \
                 ({CACHE_VARIABLE} names another directory)",
                manifest.snapshot,
                root.display()
            ));
        }
        Ok(Snapshot { dir, manifest })
    }

    #[must_use]
    pub fn manifest(&self) -> &'static Manifest {
        self.manifest
    }

    /// The directory the snapshot is installed in: the asset path that
    /// resolves the style's `asset://` URLs.
    #[must_use]
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// The file at `path` in the snapshot, such as
    /// `geojson/ne_10m_admin_0_countries.geojson`. Panics for a path the
    /// manifest doesn't name.
    #[must_use]
    pub fn path(&self, path: &str) -> PathBuf {
        assert!(self.manifest.file(path).is_some(), "the fixtures have no {path}");
        self.dir.join(path)
    }

    /// The file at `path` in the snapshot, read. Panics for a path the
    /// manifest doesn't name, or a file that can't be read.
    #[must_use]
    pub fn read(&self, path: &str) -> Vec<u8> {
        let path = self.path(path);
        std::fs::read(&path).unwrap_or_else(|error| panic!("{}: {error}", path.display()))
    }

    /// OpenFreeMap's Liberty style, its vector source on `tiles`, as JSON.
    /// Its URLs are `asset://` URLs of the snapshot.
    #[must_use]
    pub fn style(&self, tiles: Tiles) -> String {
        self.style_value(tiles).to_string()
    }

    /// [`Snapshot::style`] as a JSON value, to change.
    #[must_use]
    pub fn style_value(&self, tiles: Tiles) -> serde_json::Value {
        let mut style: serde_json::Value =
            serde_json::from_slice(&self.read("styles/liberty.json")).expect("the style is JSON");
        let url = format!("pmtiles://asset://{}", self.tile_set(tiles).archive);
        style["sources"]["openmaptiles"]["url"] = url.into();
        style
    }

    /// The style over the world's tiles, on the globe.
    #[must_use]
    pub fn globe_style(&self) -> String {
        let mut style = self.style_value(Tiles::World);
        style["projection"] = serde_json::json!({ "type": "globe" });
        style.to_string()
    }

    /// The manifest's description of a tile set: its archive, and the area a
    /// camera can center on and see only tiles of the set.
    #[must_use]
    pub fn tile_set(&self, tiles: Tiles) -> &'static TileSet {
        &self.manifest.tile_sets[tiles.name()]
    }

    /// The center of the area of `tiles`.
    #[must_use]
    pub fn area_center(&self, tiles: Tiles) -> LatLng {
        let [west, south, east, north] = self.tile_set(tiles).area;
        LatLng::new(f64::midpoint(south, north), f64::midpoint(west, east)).expect("an area's center")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_missing_snapshot_names_the_command_that_installs_it() {
        let root = std::env::temp_dir().join(format!("mln-bench-workload-test-{}", std::process::id()));
        let Err(error) = Snapshot::find_in(Manifest::current(), &root) else {
            panic!("an empty directory holds no snapshot");
        };
        assert!(error.contains("run `mise run bench:fixtures`"), "{error}");
        assert!(error.contains(&root.display().to_string()), "{error}");
    }
}
