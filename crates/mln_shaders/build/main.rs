//! Links every program for every target, with the variant flags kept, into
//! the text and tables the crate embeds (`emit`), and registers the WESL
//! modules: every `.wesl` file under `shaders/`, by its module path, and the
//! program of each `BuiltIn` that `shaders/programs/<BuiltIn>.wesl` names.

mod analyze;
mod cond;
mod emit;
mod normalize;

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::{env, fs};

use wesl::resolver::VirtualResolver;
use wesl::{CompileOptions, Compiler, Feature, Features, ManglerKind};

/// Each target's feature flag, the constant of its programs in the
/// generated code, and the crate feature that embeds them.
const TARGETS: [(&str, &str, &str); 5] = [
    ("vulkan", "VULKAN", "spirv"),
    ("d3d12", "D3D12", "hlsl"),
    ("gl", "GL", "glsl"),
    ("metal", "METAL", "msl"),
    ("webgpu", "WEBGPU", "wgsl"),
];

fn main() {
    let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("set by Cargo"));
    let root = manifest.join("shaders");
    let out = PathBuf::from(env::var("OUT_DIR").expect("set by Cargo"));
    println!("cargo:rerun-if-changed={}", root.display());
    let mut files = Vec::new();
    collect(&root, &mut files);
    files.sort();

    let mut modules = Vec::new();
    let mut generated = String::from(
        "/// Every WESL module, by its path in the package.\npub(crate) const SOURCES: &[(&str, &str)] = &[\n",
    );
    for file in &files {
        println!("cargo:rerun-if-changed={}", file.display());
        let relative = file.strip_prefix(&root).expect("under the root").with_extension("");
        let path: Vec<&str> = relative.iter().map(|part| part.to_str().expect("a UTF-8 file name")).collect();
        let module = path.join("::");
        let _ = writeln!(generated, "    ({module:?}, include_str!({:?})),", file.display().to_string());
        let program = match path[..] {
            ["programs", name] => Some(name.to_owned()),
            _ => None,
        };
        let source = fs::read_to_string(file).expect("shaders/ is readable");
        modules.push((module, source, program));
    }
    generated.push_str("];\n");
    generated.push_str(
        "/// Each program that has a WESL source, and its module.\npub(crate) const PROGRAMS: &[(BuiltIn, &str)] = &[\n",
    );
    let programs: Vec<(&str, &str)> =
        modules.iter().filter_map(|(module, _, program)| Some((program.as_deref()?, module.as_str()))).collect();
    for (name, module) in &programs {
        let _ = writeln!(generated, "    (BuiltIn::{name}, {module:?}),");
    }
    generated.push_str("];\n");

    let normalized: Vec<(String, String)> = modules
        .iter()
        .map(|(module, source, _)| {
            let normalized = normalize::normalize(module, source).unwrap_or_else(|error| panic!("{error}"));
            (module.clone(), normalized)
        })
        .collect();

    let tables: Vec<String> = std::thread::scope(|scope| {
        let threads: Vec<_> = TARGETS
            .iter()
            .map(|&(target, constant, feature)| {
                let (normalized, programs, out) = (&normalized, &programs, &out);
                scope.spawn(move || {
                    let mut table = format!(
                        "/// Each program of `PROGRAMS` linked for {target}, if the crate embeds them.\n\
                         #[cfg(not(any(test, feature = {feature:?})))]\n\
                         const {constant}: &[Built] = &[];\n\
                         #[cfg(any(test, feature = {feature:?}))]\n\
                         const {constant}: &[Built] = &[\n"
                    );
                    for (name, module) in programs {
                        let unit = link(normalized, module, target).unwrap_or_else(|error| panic!("{error}"));
                        let emitted = emit::emit(unit, target == "gl")
                            .unwrap_or_else(|error| panic!("{module} ({target}): {error}"));
                        let file = out.join(format!("{name}.{target}.wgsl"));
                        fs::write(&file, &emitted.text).expect("OUT_DIR is writable");
                        write_built(&mut table, &file, &emitted);
                    }
                    table.push_str("];\n");
                    table
                })
            })
            .collect();
        threads.into_iter().map(|thread| thread.join().expect("no panic")).collect()
    });
    for table in tables {
        generated.push_str(&table);
    }
    fs::write(out.join("built.rs"), generated).expect("OUT_DIR is writable");
}

/// Links the program `module` for `target` with every other flag kept.
fn link(normalized: &[(String, String)], module: &str, target: &str) -> Result<wesl::syntax::TranslationUnit, String> {
    let mut resolver = VirtualResolver::new();
    for (name, source) in normalized {
        let path = format!("package::{name}").parse().map_err(|error| format!("{name}: {error:?}"))?;
        resolver.add_module(path, source.clone().into());
    }
    let mut features = Features { default: Feature::Keep, ..Features::default() };
    for (other, _, _) in TARGETS {
        features.set(other, other == target);
    }
    let options = CompileOptions { features, mangler: ManglerKind::Escape, ..CompileOptions::default() };
    let main = format!("package::{module}").parse().map_err(|error| format!("{module}: {error:?}"))?;
    let mut result = Compiler::new_with_resolver(options, resolver)
        .compile_module(&main)
        .map_err(|error| format!("{module} ({target}): WESL: {error}"))?;
    mangle_once(&mut result.syntax);
    Ok(result.syntax)
}

/// Mangles once each name that WESL mangled more than once.
///
/// Declarations of one name whose `@if`s WESL keeps share their identifier,
/// which WESL mangles once for each of them; linked for a variant, the one
/// declaration is mangled once.
fn mangle_once(unit: &mut wesl::syntax::TranslationUnit) {
    use wesl::Mangler as _;
    use wesl::mangler::EscapeMangler;

    let idents: Vec<wesl::syntax::Ident> =
        unit.global_declarations.iter().filter_map(wgsl_parse::SyntaxNode::ident).collect();
    for (index, ident) in idents.iter().enumerate() {
        let count = idents.iter().filter(|other| *other == ident).count();
        if count < 2 || idents[..index].contains(ident) {
            continue;
        }
        let mut name = ident.name().clone();
        for _ in 1..count {
            match EscapeMangler.unmangle(&name) {
                Some((path, item)) if EscapeMangler.mangle(&path, &item) == name => name = item,
                _ => break,
            }
        }
        ident.clone().rename(name);
    }
}

/// Writes the `Built` of `emitted`, whose text is at `file`.
fn write_built(out: &mut String, file: &Path, emitted: &emit::Emitted) {
    let _ = writeln!(
        out,
        "    Built {{\n        text: include_str!({:?}),\n        resources: &[",
        file.display().to_string()
    );
    for r in &emitted.resources {
        let _ = writeln!(
            out,
            "            BuiltResource {{ name: {:?}, kind: DeclaredKind::{}, group: {}, binding: {}, size: {}, vertex: {:?}, fragment: {:?}, filterable: {:?} }},",
            r.name,
            r.kind,
            r.group,
            r.binding,
            r.size,
            r.vertex.to_string(),
            r.fragment.to_string(),
            r.filterable.to_string(),
        );
    }
    out.push_str("        ],\n        inputs: &[\n");
    for input in &emitted.inputs {
        let _ = writeln!(
            out,
            "            BuiltInput {{ location: {}, scalar: ScalarKind::{}, components: {}, cond: {:?} }},",
            input.location,
            input.scalar,
            input.components,
            input.cond.to_string(),
        );
    }
    out.push_str("        ],\n    },\n");
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
