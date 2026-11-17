//! The installation of the fixture snapshot that `fixtures.json` names
//! (`mln_bench_workload::manifest`), in a cache that every checkout of the
//! user shares.
//!
//! Installing fills a temporary directory
//! beside it, checks every file, and renames the directory into place, so an
//! installed directory is always complete and nothing writes to it
//! afterwards. Sessions that install at once each fill their own directory,
//! and the first rename wins.

use std::fmt::Write as _;
use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use mln_bench_workload::manifest::{File, hex};
pub(crate) use mln_bench_workload::manifest::{Manifest, cache_root, user_cache};
use sha2::{Digest, Sha256};

/// The most downloads in flight at once.
const PARALLEL_DOWNLOADS: usize = 8;

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
    fn urls_encode_each_segment() {
        assert_eq!(encode_segment("Noto Sans Regular"), "Noto%20Sans%20Regular");
        assert_eq!(encode_segment("ofm@2x.png"), "ofm@2x.png");
    }
}
