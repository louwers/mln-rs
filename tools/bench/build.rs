//! Gives the binary its harness id, `MLN_BENCH_HARNESS`: a hash of every
//! file of this package and of the compiler's version. `mln-bench compare`
//! gives verdicts only between two runs whose harness ids match, so that
//! both sides ran the same scenario code built the same way.

use std::path::{Path, PathBuf};

fn main() {
    let root = PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    println!("cargo::rerun-if-changed={}", root.display());
    let mut files = Vec::new();
    collect(&root, &root, &mut files);
    files.sort();
    let mut hash = Fnv::new();
    for relative in &files {
        // `/` on every platform, so that the id doesn't depend on it.
        hash.write(relative.to_string_lossy().replace('\\', "/").as_bytes());
        hash.write(&[0]);
        hash.write(&std::fs::read(root.join(relative)).expect("a harness file"));
        hash.write(&[0]);
    }
    let rustc = std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into());
    let version = std::process::Command::new(rustc).arg("-vV").output().expect("rustc -vV").stdout;
    hash.write(&version);
    println!("cargo::rustc-env=MLN_BENCH_HARNESS={:016x}", hash.0);
}

/// The files under `dir`, relative to `root`, leaving out build output.
fn collect(root: &Path, dir: &Path, files: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).expect("a harness directory") {
        let path = entry.expect("a directory entry").path();
        let name = path.file_name().expect("a file name");
        if name == "target" || name.to_string_lossy().starts_with('.') {
            continue;
        }
        if path.is_dir() {
            collect(root, &path, files);
        } else {
            files.push(path.strip_prefix(root).expect("under the root").to_path_buf());
        }
    }
}

/// 64-bit FNV-1a, which is the same on every toolchain.
struct Fnv(u64);

impl Fnv {
    fn new() -> Fnv {
        Fnv(0xcbf2_9ce4_8422_2325)
    }

    fn write(&mut self, bytes: &[u8]) {
        for &byte in bytes {
            self.0 = (self.0 ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3);
        }
    }
}
