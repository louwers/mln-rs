//! Fails when the checked-in generated style code (in `crates/mln_style` and
//! `crates/mln_layers`) differs from what the generator writes for the style
//! spec in v8.json.

use std::path::{Path, PathBuf};

fn spec_path() -> PathBuf {
    Path::new(&std::env::var_os("CARGO_MANIFEST_DIR").unwrap()).join("v8.json")
}

#[test]
#[cfg_attr(
    any(target_os = "android", target_env = "ohos"),
    ignore = "reads crates/, which the device's copy of this package lacks"
)]
fn generated_code_is_up_to_date() {
    let spec_path = spec_path();
    let spec = std::fs::read_to_string(&spec_path).unwrap_or_else(|e| panic!("{}: {e}", spec_path.display()));
    let root = Path::new(&std::env::var_os("CARGO_MANIFEST_DIR").unwrap()).join("../../crates");
    let stale = style_codegen::check(&spec, &root).expect("generator");
    assert!(
        stale.is_empty(),
        "generated style code is out of date: {stale:?}\nRegenerate it with `mise run generate:style`."
    );
}

#[test]
fn every_layer_type_is_generated() {
    let spec_path = spec_path();
    let spec = std::fs::read_to_string(&spec_path).unwrap();
    let files = style_codegen::generate(&spec).unwrap();
    let paths: Vec<&str> = files.iter().map(|f| f.path.as_str()).collect();
    let spec: serde_json::Value = serde_json::from_str(&spec).unwrap();
    let types = spec["layer"]["type"]["values"].as_object().expect("layer.type.values");
    assert!(!types.is_empty());
    for t in types.keys() {
        let name = t.replace('-', "_");
        let path = if style_codegen::LAYERS_CRATE_TYPES.contains(&t.as_str()) {
            format!("mln_layers/src/{name}/properties.rs")
        } else {
            format!("mln_style/src/layers/{name}_layer_properties.rs")
        };
        assert!(paths.contains(&path.as_str()), "{t}");
    }
    assert!(paths.contains(&"mln_style/src/light_properties.rs"));
    assert!(paths.contains(&"mln_style/src/sky_properties.rs"));
    assert_eq!(files.len(), types.len() + 2);
}
