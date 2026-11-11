//! A Rust port of the MapLibre Native style code generator
//! (scripts/generate-style-code.mjs + scripts/style-code.mjs +
//! scripts/style-spec.mjs and the .ejs templates). It reads the same
//! `scripts/style-spec-reference/v8.json` and makes the same decisions
//! (property order, data-driven / cross-faded / color-ramp kinds, type
//! mapping, default values, enum names, the per-layer overrides) and emits
//! Rust shaped like the C++ output into crates/mln_style/src. The output is
//! checked in; the generated_code_test fails when it differs from fresh
//! generator output.

pub mod defaults;
pub mod difftest;
pub mod emit_layer;
pub mod emit_properties;
pub mod emit_root_properties;
pub mod json;
pub mod spec;
pub mod types;
pub mod writer;

/// One output file, relative to `crates/mln_style/src`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GeneratedFile {
    pub path: String,
    pub content: String,
}

/// Everything the C++ generator writes, in its order (per layer:
/// properties, then the layer; then the light and the sky).
pub fn generate(spec_text: &str) -> Result<Vec<GeneratedFile>, String> {
    let spec = spec::load(spec_text)?;
    let mut files = Vec::new();
    for layer in spec::layers(&spec)? {
        let name = writer::layer_file_name(&layer.type_);
        files.push(GeneratedFile {
            path: format!("layers/{name}_layer_properties.rs"),
            content: emit_properties::emit(&layer),
        });
        files.push(GeneratedFile { path: format!("layers/{name}_layer.rs"), content: emit_layer::emit(&layer) });
    }
    let light = emit_root_properties::emit("Light", "light", &spec::light_properties(&spec)?);
    files.push(GeneratedFile { path: "light.rs".into(), content: light });
    let sky = emit_root_properties::emit("Sky", "sky", &spec::sky_properties(&spec)?);
    files.push(GeneratedFile { path: "sky.rs".into(), content: sky });
    Ok(files)
}

/// Compares fresh output with the files under `root` (crates/mln_style/src);
/// returns the paths that differ or are missing.
pub fn check(spec_text: &str, root: &std::path::Path) -> Result<Vec<String>, String> {
    let mut stale = Vec::new();
    for file in generate(spec_text)? {
        match std::fs::read_to_string(root.join(&file.path)) {
            Ok(existing) if existing == file.content => {}
            _ => stale.push(file.path),
        }
    }
    Ok(stale)
}
