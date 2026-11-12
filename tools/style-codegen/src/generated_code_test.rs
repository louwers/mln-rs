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
        "generated style code is out of date: {stale:?}\nRegenerate it with `just generate-style-code`."
    );
}

#[test]
fn every_layer_type_is_generated() {
    let spec_path = spec_path();
    let spec = std::fs::read_to_string(&spec_path).unwrap();
    let files = style_codegen::generate(&spec).unwrap();
    let paths: Vec<&str> = files.iter().map(|f| f.path.as_str()).collect();
    for t in [
        "fill",
        "line",
        "symbol",
        "circle",
        "heatmap",
        "fill_extrusion",
        "raster",
        "hillshade",
        "color_relief",
        "background",
        "location_indicator",
    ] {
        assert!(paths.contains(&format!("layers/{t}_layer.rs").as_str()), "{t}");
        assert!(paths.contains(&format!("layers/{t}_layer_properties.rs").as_str()), "{t}");
    }
    assert!(paths.contains(&"light.rs"));
    assert!(paths.contains(&"sky.rs"));
    assert_eq!(files.len(), 24);
}
