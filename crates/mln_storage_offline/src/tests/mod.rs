// Ported gtests of test/storage/offline_download.test.cpp and their helpers,
// and tests of DatabaseFileSource's offline region API.

mod database_file_source;
mod offline_download;

use std::path::PathBuf;

/// A path of the C++ repository, in the copy of its files these tests read
/// (testdata/).
pub fn cpp_path(relative: &str) -> PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata").join(relative)
}

/// A new empty temporary directory, deleted when dropped.
pub struct TempDir(PathBuf);

impl TempDir {
    pub fn new() -> Self {
        static COUNTER: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!("mln-storage-offline-test-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap_or_else(|e| panic!("Cannot create {}: {e}", dir.display()));
        TempDir(dir)
    }

    /// The path of `name` in the directory.
    pub fn path(&self, name: &str) -> String {
        self.0.join(name).to_string_lossy().into_owned()
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
