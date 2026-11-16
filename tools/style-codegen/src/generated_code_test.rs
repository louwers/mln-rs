//! Fails when the checked-in generated style code (`crates/mln_style/src`)
//! differs from what the generator writes for the style spec in v8.json.

use std::path::{Path, PathBuf};

fn spec_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("v8.json")
}

#[test]
fn generated_code_is_up_to_date() {
    let spec_path = spec_path();
    let spec = std::fs::read_to_string(&spec_path).unwrap_or_else(|e| panic!("{}: {e}", spec_path.display()));
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../crates/mln_style/src");
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
        let path = format!("layers/{}_layer_properties.rs", t.replace('-', "_"));
        assert!(paths.contains(&path.as_str()), "{t}");
    }
    assert!(paths.contains(&"light_properties.rs"));
    assert!(paths.contains(&"sky_properties.rs"));
    assert_eq!(files.len(), types.len() + 2);
}
