//! Writes the declarations of `mln-rs-style`'s properties from the style
//! spec (v8.json): for each layer type, the light and the sky, each
//! property's name, kind, type and default, as `properties!` sets and a
//! property table. The macros in `mln-rs-style` write the code from those.
//! The output is checked in; the `generated_code_test` fails when it differs
//! from fresh generator output.

mod defaults;
mod emit_properties;
mod emit_root_properties;
mod emit_table;
mod json;
mod spec;
mod types;
mod writer;

/// One output file, relative to `crates/mln_style/src`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GeneratedFile {
    pub path: String,
    pub content: String,
}

/// Every generated file: each layer type's properties, then the light's and
/// the sky's.
pub fn generate(spec_text: &str) -> Result<Vec<GeneratedFile>, String> {
    let spec = spec::load(spec_text)?;
    let mut files = Vec::new();
    for layer in spec::layers(&spec)? {
        let name = writer::layer_file_name(&layer.type_);
        files.push(GeneratedFile {
            path: format!("layers/{name}_layer_properties.rs"),
            content: emit_properties::emit(&layer),
        });
    }
    let light = emit_root_properties::emit("light", &spec::light_properties(&spec)?);
    files.push(GeneratedFile { path: "light_properties.rs".into(), content: light });
    let sky = emit_root_properties::emit("sky", &spec::sky_properties(&spec)?);
    files.push(GeneratedFile { path: "sky_properties.rs".into(), content: sky });
    Ok(files)
}

/// Compares fresh output with the files under `root` (`crates/mln_style/src`);
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
