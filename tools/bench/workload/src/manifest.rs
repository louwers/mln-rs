//! The fixture snapshot's manifest: the map data that the benchmarks read,
//! which `tools/bench/fixtures.json` names file by file with each one's size
//! and SHA-256, and where its installation lives, in a cache that every
//! checkout of the user shares.
//!
//! A snapshot is installed in `<cache>/<manifest hash>/`, where the hash is
//! the SHA-256 of `fixtures.json`. `mln-bench fixtures` installs it.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fmt::Write as _;
use std::path::{Component, Path, PathBuf};
use std::sync::OnceLock;

use serde::Deserialize;
use sha2::{Digest, Sha256};

/// The manifest of the snapshot that the benchmarks of this checkout read.
const MANIFEST: &[u8] = include_bytes!("../../fixtures.json");

/// The environment variable that names the directory to install snapshots
/// in, in place of the per-user cache.
pub const CACHE_VARIABLE: &str = "MLN_BENCH_FIXTURES";

#[derive(Debug, Deserialize)]
pub struct Manifest {
    /// The snapshot's id, the last segment of its base URL.
    pub snapshot: String,
    /// The URL under which the snapshot's files are published, ending in `/`.
    pub base_url: String,
    /// The tile sets, by name, as `Tiles` names them.
    pub tile_sets: BTreeMap<String, TileSet>,
    pub files: Vec<File>,
    /// The SHA-256 of the manifest's text, in hex: the name of the directory
    /// the snapshot is installed in.
    #[serde(skip)]
    pub hash: String,
}

/// A tile set's archive.
#[derive(Debug, Deserialize)]
pub struct TileSet {
    /// The archive's path in the snapshot.
    pub archive: String,
    /// West, south, east, north: a camera centered here sees only tiles the
    /// archive holds, when its view is at most two tiles across.
    pub area: [f64; 4],
}

/// A file of the snapshot.
#[derive(Debug, Deserialize)]
pub struct File {
    /// Relative to the snapshot's directory and base URL, with `/` between
    /// components.
    pub path: String,
    pub size: u64,
    /// In hex.
    pub sha256: String,
}

impl Manifest {
    /// The manifest this harness was built with.
    pub fn current() -> &'static Manifest {
        static MANIFEST_: OnceLock<Manifest> = OnceLock::new();
        MANIFEST_.get_or_init(|| Manifest::parse(MANIFEST).expect("tools/bench/fixtures.json is a manifest"))
    }

    /// Parses a manifest, refusing a path that could leave the snapshot's
    /// directory and a tile set whose archive it doesn't list.
    pub fn parse(text: &[u8]) -> Result<Manifest, String> {
        let mut manifest: Manifest = serde_json::from_slice(text).map_err(|error| format!("the manifest: {error}"))?;
        for file in &manifest.files {
            let path = Path::new(&file.path);
            let normal = path.components().all(|component| matches!(component, Component::Normal(_)));
            if file.path.is_empty() || file.path.contains('\\') || !normal {
                return Err(format!("the manifest names a file outside the snapshot: {:?}", file.path));
            }
        }
        for (name, set) in &manifest.tile_sets {
            if manifest.file(&set.archive).is_none() {
                return Err(format!("the manifest doesn't list {name}'s archive, {}", set.archive));
            }
        }
        if !manifest.base_url.ends_with('/') {
            return Err(format!("the manifest's base URL {} doesn't end in /", manifest.base_url));
        }
        manifest.hash = hex(&Sha256::digest(text));
        Ok(manifest)
    }

    #[must_use]
    pub fn file(&self, path: &str) -> Option<&File> {
        self.files.iter().find(|file| file.path == path)
    }
}

/// The directory that holds the installed snapshots: `$MLN_BENCH_FIXTURES`
/// if it's set, else `mln-rs/bench-fixtures` in the user's cache directory
/// (`~/Library/Caches` on macOS, `$XDG_CACHE_HOME` or `~/.cache` on Linux and
/// other Unix systems, `%LOCALAPPDATA%` on Windows).
pub fn cache_root() -> Result<PathBuf, String> {
    cache_root_from(|name| std::env::var_os(name).filter(|value| !value.is_empty()))
}

fn cache_root_from(var: impl Fn(&str) -> Option<OsString>) -> Result<PathBuf, String> {
    if let Some(dir) = var(CACHE_VARIABLE) {
        return Ok(dir.into());
    }
    Ok(user_cache_from(var)?.join("bench-fixtures"))
}

/// `mln-rs` in the user's cache directory, which every checkout of the user
/// shares.
pub fn user_cache() -> Result<PathBuf, String> {
    user_cache_from(|name| std::env::var_os(name).filter(|value| !value.is_empty()))
}

fn user_cache_from(var: impl Fn(&str) -> Option<OsString>) -> Result<PathBuf, String> {
    let home = || var("HOME").map(PathBuf::from).ok_or("HOME isn't set");
    let cache = if cfg!(target_os = "macos") {
        home()?.join("Library/Caches")
    } else if cfg!(windows) {
        var("LOCALAPPDATA").map(PathBuf::from).ok_or("LOCALAPPDATA isn't set")?
    } else {
        match var("XDG_CACHE_HOME") {
            Some(dir) => dir.into(),
            None => home()?.join(".cache"),
        }
    };
    Ok(cache.join("mln-rs"))
}

/// `bytes` in lowercase hex.
#[must_use]
pub fn hex(bytes: &[u8]) -> String {
    let mut text = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        write!(text, "{byte:02x}").expect("writing to a string");
    }
    text
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn a_path_outside_the_snapshot_is_refused() {
        for path in ["../a", "/a", "a/../../b", "a\\b", ""] {
            let text = json!({
                "snapshot": "test",
                "base_url": "https://example.com/",
                "tile_sets": {},
                "files": [{ "path": path, "size": 0, "sha256": "" }],
            });
            assert!(Manifest::parse(text.to_string().as_bytes()).is_err(), "{path}");
        }
    }

    #[test]
    fn the_checked_in_manifest_names_a_complete_snapshot() {
        let manifest = Manifest::current();
        assert_eq!(manifest.hash, hex(&Sha256::digest(MANIFEST)));
        assert!(manifest.base_url.ends_with(&format!("/{}/", manifest.snapshot)));
        for name in ["street", "street-mlt", "world", "cjk"] {
            let set = &manifest.tile_sets[name];
            assert!(manifest.file(&set.archive).is_some(), "{name}'s archive");
        }
        assert!(manifest.file("styles/liberty.json").is_some());
    }

    #[test]
    fn the_cache_is_per_user_unless_the_variable_names_one() {
        let var = |vars: &'static [(&'static str, &'static str)]| {
            move |name: &str| vars.iter().find(|(key, _)| *key == name).map(|(_, value)| OsString::from(value))
        };
        assert_eq!(
            cache_root_from(var(&[(CACHE_VARIABLE, "/fixtures"), ("HOME", "/home/a")])).unwrap(),
            PathBuf::from("/fixtures")
        );
        let home = cache_root_from(var(&[("HOME", "/home/a"), ("LOCALAPPDATA", "C:\\Local")])).unwrap();
        if cfg!(target_os = "macos") {
            assert_eq!(home, PathBuf::from("/home/a/Library/Caches/mln-rs/bench-fixtures"));
        } else if cfg!(windows) {
            assert_eq!(home, PathBuf::from("C:\\Local").join("mln-rs").join("bench-fixtures"));
        } else {
            assert_eq!(home, PathBuf::from("/home/a/.cache/mln-rs/bench-fixtures"));
            let xdg = cache_root_from(var(&[("HOME", "/home/a"), ("XDG_CACHE_HOME", "/xdg")])).unwrap();
            assert_eq!(xdg, PathBuf::from("/xdg/mln-rs/bench-fixtures"));
        }
    }
}
