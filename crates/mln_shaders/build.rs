//! The registry of WESL modules: every `.wesl` file under `shaders/`, by its
//! module path, and the program of each `BuiltIn` that
//! `shaders/programs/<BuiltIn>.wesl` names.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::{env, fs};

fn main() {
    let root = Path::new(&env::var("CARGO_MANIFEST_DIR").expect("set by Cargo")).join("shaders");
    println!("cargo:rerun-if-changed={}", root.display());
    let mut files = Vec::new();
    collect(&root, &mut files);
    files.sort();

    let mut sources = String::from(
        "/// Every WESL module, by its path in the package.\npub(crate) const SOURCES: &[(&str, &str)] = &[\n",
    );
    let mut programs = String::from(
        "/// Each program that has a WESL source, and its module.\npub(crate) const PROGRAMS: &[(BuiltIn, &str)] = &[\n",
    );
    for file in &files {
        println!("cargo:rerun-if-changed={}", file.display());
        let relative = file.strip_prefix(&root).expect("under the root").with_extension("");
        let path: Vec<&str> = relative.iter().map(|part| part.to_str().expect("a UTF-8 file name")).collect();
        let module = path.join("::");
        let _ = writeln!(sources, "    ({module:?}, include_str!({:?})),", file.display().to_string());
        if let ["programs", name] = path[..] {
            let _ = writeln!(programs, "    (BuiltIn::{name}, {module:?}),");
        }
    }
    sources.push_str("];\n");
    programs.push_str("];\n");
    let out = PathBuf::from(env::var("OUT_DIR").expect("set by Cargo")).join("registry.rs");
    fs::write(out, sources + &programs).expect("OUT_DIR is writable");
}

/// The `.wesl` files under `dir`, recursively.
fn collect(dir: &Path, files: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(dir).expect("shaders/ is readable") {
        let path = entry.expect("shaders/ is readable").path();
        if path.is_dir() {
            println!("cargo:rerun-if-changed={}", path.display());
            collect(&path, files);
        } else if path.extension().is_some_and(|extension| extension == "wesl") {
            files.push(path);
        }
    }
}
