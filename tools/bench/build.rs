//! Gives the binary its harness id, `MLN_BENCH_HARNESS`: a hash of every
//! file of this package, of the libraries' `bench` modules
//! (`crates/<library>/src/bench.rs`), whose entry points the scenarios
//! call, and of the compiler's version. `mln-bench compare` gives verdicts
//! only between two runs whose harness ids match, so that both sides ran the
//! same scenario code built the same way.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

fn main() {
    let root = PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    println!("cargo::rerun-if-changed={}", root.display());
    let mut files = Vec::new();
    collect(&root, &root, &mut files);
    files.sort();
    let mut hash = Fnv::new();
    for relative in &files {
        hash_file(&mut hash, &root, relative);
    }
    let workspace = root.join("../..");
    let mut modules = Vec::new();
    for entry in std::fs::read_dir(workspace.join("crates")).expect("the libraries") {
        let module = Path::new("crates").join(entry.expect("a directory entry").file_name()).join("src/bench.rs");
        if workspace.join(&module).is_file() {
            println!("cargo::rerun-if-changed={}", workspace.join(&module).display());
            modules.push(module);
        }
    }
    modules.sort();
    for module in &modules {
        hash_file(&mut hash, &workspace, module);
    }
    let rustc = std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into());
    let version = std::process::Command::new(rustc).arg("-vV").output().expect("rustc -vV").stdout;
    hash.write(&version);
    println!("cargo::rustc-env=MLN_BENCH_HARNESS={:016x}", hash.0);
    backend_timers(&workspace);
    terrain_update(&workspace);
    layout_probe();
}

/// Sets `mln_bench_vulkan`, `mln_bench_gl`, `mln_bench_d3d12` and
/// `mln_bench_mtl` where the native backends'
/// `bench` modules, which time the GPU's work (`meter::gpu_time`), are
/// built: where the runtime's `bench` feature turns on theirs. A base of
/// `mln-bench compare` from before they had them builds without them, and
/// reports no GPU time on those backends.
fn backend_timers(workspace: &Path) {
    let manifest = workspace.join("crates/mln_runtime/Cargo.toml");
    println!("cargo::rerun-if-changed={}", manifest.display());
    println!("cargo::rustc-check-cfg=cfg(mln_bench_vulkan, mln_bench_gl, mln_bench_d3d12, mln_bench_mtl)");
    let manifest = std::fs::read_to_string(&manifest).expect("the runtime's manifest");
    for (library, cfg) in [
        ("mln_vulkan", "mln_bench_vulkan"),
        ("mln_gl", "mln_bench_gl"),
        ("mln_d3d12", "mln_bench_d3d12"),
        ("mln_mtl", "mln_bench_mtl"),
    ] {
        // Only the runtime's `bench` feature names a library's.
        if manifest.contains(&format!("\"{library}?/bench\"")) {
            println!("cargo::rustc-cfg={cfg}");
        }
    }
}

/// Sets `mln_bench_terrain` where the renderer's `UpdateParameters` has the
/// terrain's fields, which the symbol scenarios' update must name, and
/// `mln_bench_terrain_skirts` where it has the skirts' length. A base of
/// `mln-bench compare` from before the libraries drew terrain, or before
/// they took the skirts' length, builds the update without them.
fn terrain_update(workspace: &Path) {
    let source = workspace.join("crates/mln_renderer/src/update_parameters.rs");
    println!("cargo::rerun-if-changed={}", source.display());
    println!("cargo::rustc-check-cfg=cfg(mln_bench_terrain)");
    println!("cargo::rustc-check-cfg=cfg(mln_bench_terrain_skirts)");
    let source = std::fs::read_to_string(&source).expect("the renderer's update parameters");
    if source.contains("pub elevation:") {
        println!("cargo::rustc-cfg=mln_bench_terrain");
    }
    if source.contains("pub terrain_skirt_length:") {
        println!("cargo::rustc-cfg=mln_bench_terrain_skirts");
    }
}

/// With the `layout-probe` feature, writes `layout_probe.rs` to `OUT_DIR`: a
/// function of many arms that the binary keeps but never calls, so that the
/// code after it lies at other addresses than without the feature. The
/// instructions a scenario retires can change with where its code lies,
/// and `mln-bench compare` measures by how much with this build of the base.
fn layout_probe() {
    if std::env::var_os("CARGO_FEATURE_LAYOUT_PROBE").is_none() {
        return;
    }
    let mut code = String::from(
        "/// Never called (build.rs).\n#[inline(never)]\npub(crate) fn layout_probe(x: u64) -> u64 {\n    match x % 512 {\n",
    );
    for arm in 0..512u64 {
        // black_box keeps each arm's code, which a table would replace.
        let _ = writeln!(
            code,
            "        {arm} => std::hint::black_box(x.wrapping_mul({})).wrapping_add({}),",
            2 * arm + 3,
            arm + 1
        );
    }
    code.push_str("        _ => 0,\n    }\n}\n");
    let out = PathBuf::from(std::env::var_os("OUT_DIR").expect("OUT_DIR"));
    std::fs::write(out.join("layout_probe.rs"), code).expect("the layout probe");
}

/// Adds the file `relative` of `root`, by its path and contents.
fn hash_file(hash: &mut Fnv, root: &Path, relative: &Path) {
    // `/` on every platform, so that the id doesn't depend on it.
    hash.write(relative.to_string_lossy().replace('\\', "/").as_bytes());
    hash.write(&[0]);
    hash.write(&std::fs::read(root.join(relative)).expect("a harness file"));
    hash.write(&[0]);
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
