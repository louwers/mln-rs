//! The fixture snapshot: the map data that scenarios read, which
//! `fixtures.json` names file by file with each one's size and SHA-256, and
//! its installation in a cache that every checkout of the user shares.
//!
//! A snapshot is installed in `<cache>/<manifest hash>/`, where the hash is
//! the SHA-256 of `fixtures.json`. Installing fills a temporary directory
//! beside it, checks every file, and renames the directory into place, so an
//! installed directory is always complete and nothing writes to it
//! afterwards. Sessions that install at once each fill their own directory,
//! and the first rename wins.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fmt::Write as _;
use std::io::Read as _;
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::OnceLock;

use serde::Deserialize;
use sha2::{Digest, Sha256};

/// The manifest of the snapshot that this harness's scenarios read.
const MANIFEST: &[u8] = include_bytes!("../fixtures.json");

/// The environment variable that names the directory to install snapshots
/// in, in place of the per-user cache.
pub(crate) const CACHE_VARIABLE: &str = "MLN_BENCH_FIXTURES";

/// The most downloads in flight at once.
const PARALLEL_DOWNLOADS: usize = 8;

#[derive(Debug, Deserialize)]
pub(crate) struct Manifest {
    /// The snapshot's id, the last segment of its base URL.
    pub(crate) snapshot: String,
    /// The URL under which the snapshot's files are published, ending in `/`.
    pub(crate) base_url: String,
    /// The tile sets, by name, as `Tiles` names them.
    pub(crate) tile_sets: BTreeMap<String, TileSet>,
    pub(crate) files: Vec<File>,
    /// The SHA-256 of the manifest's text, in hex: the name of the directory
    /// the snapshot is installed in.
    #[serde(skip)]
    pub(crate) hash: String,
}

/// A tile set's archive.
#[derive(Debug, Deserialize)]
pub(crate) struct TileSet {
    /// The archive's path in the snapshot.
    pub(crate) archive: String,
    /// West, south, east, north: a camera centered here sees only tiles the
    /// archive holds, when its view is at most two tiles across.
    pub(crate) area: [f64; 4],
    /// The archive's first and last zoom.
    #[expect(dead_code, reason = "no scenario reads the fixtures")]
    pub(crate) zooms: [u8; 2],
}

/// A file of the snapshot.
#[derive(Debug, Deserialize)]
pub(crate) struct File {
    /// Relative to the snapshot's directory and base URL, with `/` between
    /// components.
    pub(crate) path: String,
    pub(crate) size: u64,
    /// In hex.
    pub(crate) sha256: String,
}

impl Manifest {
    /// The manifest this harness was built with.
    pub(crate) fn current() -> &'static Manifest {
        static MANIFEST_: OnceLock<Manifest> = OnceLock::new();
        MANIFEST_.get_or_init(|| Manifest::parse(MANIFEST).expect("tools/bench/fixtures.json is a manifest"))
    }

    /// Parses a manifest, refusing a path that could leave the snapshot's
    /// directory and a tile set whose archive it doesn't list.
    pub(crate) fn parse(text: &[u8]) -> Result<Manifest, String> {
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

    pub(crate) fn file(&self, path: &str) -> Option<&File> {
        self.files.iter().find(|file| file.path == path)
    }
}

/// The directory that holds the installed snapshots: `$MLN_BENCH_FIXTURES`
/// if it's set, else `mln-rs/bench-fixtures` in the user's cache directory
/// (`~/Library/Caches` on macOS, `$XDG_CACHE_HOME` or `~/.cache` on Linux and
/// other Unix systems, `%LOCALAPPDATA%` on Windows).
pub(crate) fn cache_root() -> Result<PathBuf, String> {
    cache_root_from(|name| std::env::var_os(name).filter(|value| !value.is_empty()))
}

fn cache_root_from(var: impl Fn(&str) -> Option<OsString>) -> Result<PathBuf, String> {
    if let Some(dir) = var(CACHE_VARIABLE) {
        return Ok(dir.into());
    }
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
    Ok(cache.join("mln-rs").join("bench-fixtures"))
}

/// Where a snapshot's files come from.
pub(crate) enum Origin<'a> {
    /// The manifest's base URL, through curl.
    Url,
    /// A directory laid out as the base URL is, such as one that
    /// `mise run bench:snapshot` staged.
    Dir(&'a Path),
}

/// Installs `manifest`'s snapshot in `root` from `origin`, unless it's there,
/// and returns its directory.
pub(crate) fn install(manifest: &Manifest, root: &Path, origin: &Origin<'_>) -> Result<PathBuf, String> {
    let target = root.join(&manifest.hash);
    if target.is_dir() {
        return Ok(target);
    }
    std::fs::create_dir_all(root).map_err(|error| format!("{}: {error}", root.display()))?;
    // Removed on every way out but the rename.
    let partial = tempfile::Builder::new()
        .prefix(&format!(".{}-", &manifest.hash[..16]))
        .tempdir_in(root)
        .map_err(|error| format!("{}: {error}", root.display()))?;
    match origin {
        Origin::Url => download(manifest, partial.path())?,
        Origin::Dir(dir) => copy(manifest, dir, partial.path())?,
    }
    for file in &manifest.files {
        check(file, &partial.path().join(&file.path))?;
    }
    if let Err(error) = std::fs::rename(partial.path(), &target) {
        // Another installation got there first; its files are the same.
        if !target.is_dir() {
            return Err(format!("{}: {error}", target.display()));
        }
    }
    Ok(target)
}

fn copy(manifest: &Manifest, from: &Path, to: &Path) -> Result<(), String> {
    for file in &manifest.files {
        let (source, target) = (from.join(&file.path), to.join(&file.path));
        create_parent(&target)?;
        std::fs::copy(&source, &target).map_err(|error| format!("{}: {error}", source.display()))?;
    }
    Ok(())
}

/// Downloads every file with curl, which every system the benchmarks run on
/// has, so that the harness links no HTTP or TLS stack.
fn download(manifest: &Manifest, to: &Path) -> Result<(), String> {
    let mut config = String::new();
    for file in &manifest.files {
        let url =
            format!("{}{}", manifest.base_url, file.path.split('/').map(encode_segment).collect::<Vec<_>>().join("/"));
        let output = to.join(&file.path);
        create_parent(&output)?;
        writeln!(config, "url = \"{}\"\noutput = \"{}\"", quote(&url), quote(&output.to_string_lossy()))
            .expect("writing to a string");
    }
    let config_file = tempfile::NamedTempFile::new().map_err(|error| format!("a temporary file: {error}"))?;
    std::fs::write(config_file.path(), config).map_err(|error| format!("curl's configuration: {error}"))?;
    eprintln!("mln-bench: downloading {} files from {}", manifest.files.len(), manifest.base_url);
    let status = Command::new("curl")
        .args(["--parallel", "--parallel-max", &PARALLEL_DOWNLOADS.to_string()])
        .args(["--fail", "--silent", "--show-error", "--location", "--retry", "3"])
        .args(["--user-agent", "mln-rs-bench-fixtures"])
        .arg("--config")
        .arg(config_file.path())
        .stdin(Stdio::null())
        .status()
        .map_err(|error| format!("curl: {error}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("downloading the snapshot from {} failed ({status})", manifest.base_url))
    }
}

/// Checks that the file at `path` is `file`.
fn check(file: &File, path: &Path) -> Result<(), String> {
    let mut reader = std::fs::File::open(path).map_err(|error| format!("{}: {error}", file.path))?;
    let mut hasher = Sha256::new();
    let mut size = 0;
    let mut buffer = vec![0; 1 << 16];
    loop {
        let read = reader.read(&mut buffer).map_err(|error| format!("{}: {error}", file.path))?;
        if read == 0 {
            break;
        }
        size += read as u64;
        hasher.update(&buffer[..read]);
    }
    let sha256 = hex(&hasher.finalize());
    if size != file.size || sha256 != file.sha256 {
        return Err(format!(
            "{} has {size} bytes with SHA-256 {sha256}, but the manifest says {} bytes with SHA-256 {}",
            file.path, file.size, file.sha256
        ));
    }
    Ok(())
}

fn create_parent(path: &Path) -> Result<(), String> {
    let parent = path.parent().expect("a file's directory");
    std::fs::create_dir_all(parent).map_err(|error| format!("{}: {error}", parent.display()))
}

fn hex(bytes: &[u8]) -> String {
    let mut text = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        write!(text, "{byte:02x}").expect("writing to a string");
    }
    text
}

/// `segment` as one segment of a URL's path.
fn encode_segment(segment: &str) -> String {
    let mut encoded = String::new();
    for byte in segment.bytes() {
        if byte.is_ascii_alphanumeric() || b"-._~@".contains(&byte) {
            encoded.push(byte as char);
        } else {
            write!(encoded, "%{byte:02X}").expect("writing to a string");
        }
    }
    encoded
}

/// `text` inside the double quotes of a curl configuration file.
fn quote(text: &str) -> String {
    text.replace('\\', "\\\\").replace('"', "\\\"")
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    /// A manifest of `files`, and a directory that holds them.
    fn snapshot(files: &[(&str, &[u8])]) -> (Manifest, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let mut entries = Vec::new();
        for (path, data) in files {
            let file = dir.path().join(path);
            create_parent(&file).unwrap();
            std::fs::write(&file, data).unwrap();
            entries.push(json!({ "path": path, "size": data.len(), "sha256": hex(&Sha256::digest(data)) }));
        }
        let text = json!({
            "snapshot": "test",
            "base_url": "https://example.com/test/",
            "tile_sets": {},
            "files": entries,
        });
        (Manifest::parse(text.to_string().as_bytes()).unwrap(), dir)
    }

    fn entries(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> =
            std::fs::read_dir(dir).unwrap().map(|entry| entry.unwrap().file_name().into_string().unwrap()).collect();
        names.sort();
        names
    }

    const FILES: &[(&str, &[u8])] = &[("a.json", b"{}"), ("glyphs/Noto Sans Regular/0-255.pbf", &[1, 2, 3])];

    #[test]
    fn installs_a_snapshot_once_complete_and_then_leaves_it() {
        let (manifest, source) = snapshot(FILES);
        let root = tempfile::tempdir().unwrap();
        let installed = install(&manifest, root.path(), &Origin::Dir(source.path())).unwrap();
        assert_eq!(installed, root.path().join(&manifest.hash));
        assert_eq!(std::fs::read(installed.join("glyphs/Noto Sans Regular/0-255.pbf")).unwrap(), [1, 2, 3]);
        // No temporary directory stays behind.
        assert_eq!(entries(root.path()), std::slice::from_ref(&manifest.hash));

        // An installed snapshot is never read from its origin again.
        std::fs::remove_file(source.path().join("a.json")).unwrap();
        assert_eq!(install(&manifest, root.path(), &Origin::Dir(source.path())).unwrap(), installed);
    }

    #[test]
    fn a_file_that_fails_its_checksum_installs_nothing() {
        let (manifest, source) = snapshot(FILES);
        std::fs::write(source.path().join("a.json"), b"[]").unwrap();
        let root = tempfile::tempdir().unwrap();
        let error = install(&manifest, root.path(), &Origin::Dir(source.path())).unwrap_err();
        assert!(error.starts_with("a.json has 2 bytes with SHA-256 "), "{error}");
        assert_eq!(entries(root.path()), Vec::<String>::new());
    }

    #[test]
    fn a_missing_file_installs_nothing() {
        let (manifest, source) = snapshot(FILES);
        std::fs::remove_file(source.path().join("a.json")).unwrap();
        let root = tempfile::tempdir().unwrap();
        assert!(install(&manifest, root.path(), &Origin::Dir(source.path())).is_err());
        assert_eq!(entries(root.path()), Vec::<String>::new());
    }

    #[test]
    fn installations_at_once_agree_on_one_directory() {
        let (manifest, source) = snapshot(FILES);
        let root = tempfile::tempdir().unwrap();
        let installed: Vec<PathBuf> = std::thread::scope(|scope| {
            let threads: Vec<_> = (0..8)
                .map(|_| scope.spawn(|| install(&manifest, root.path(), &Origin::Dir(source.path())).unwrap()))
                .collect();
            threads.into_iter().map(|thread| thread.join().unwrap()).collect()
        });
        assert!(installed.iter().all(|dir| *dir == root.path().join(&manifest.hash)));
        assert_eq!(entries(root.path()), std::slice::from_ref(&manifest.hash));
        assert_eq!(std::fs::read(installed[0].join("a.json")).unwrap(), b"{}");
    }

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

    #[test]
    fn urls_encode_each_segment() {
        assert_eq!(encode_segment("Noto Sans Regular"), "Noto%20Sans%20Regular");
        assert_eq!(encode_segment("ofm@2x.png"), "ofm@2x.png");
    }
}
