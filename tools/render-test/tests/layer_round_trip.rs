// Every layer in every render- and query-test style crosses (its style data,
// as a tile worker in another wasm instance gets its layers,
// mln_style::wire) to one that encodes the same. A layer that doesn't
// cross couldn't be laid out there, and one that crosses as another layer
// would be laid out differently. Every layer also serializes to style JSON
// that parses back to the same layer, as `Style::layer_json` promises.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use mln_engine::Registry;
use mln_style::StyleRegistry;
use mln_style::conversion::{Convertible, Error};
use mln_style::layer::{ErasedLayerStyle, Layer, convert_layer};
use mln_style::wire::SharedLayer;
use mln_wire::shared;

#[derive(mln_wire::Wire)]
struct Crossing(#[wire(with = "shared::ByKey::<SharedLayer>")] Arc<dyn ErasedLayerStyle>);

fn styles(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(dir).unwrap_or_else(|e| panic!("{}: {e}", dir.display())) {
        let path = entry.unwrap().path();
        if path.is_dir() {
            styles(&path, out);
        } else if path.file_name().is_some_and(|name| name == "style.json") {
            out.push(path);
        }
    }
}

fn every_style() -> Vec<PathBuf> {
    let integration = Path::new(&std::env::var_os("CARGO_MANIFEST_DIR").unwrap()).join("testdata/metrics/integration");
    let mut paths = Vec::new();
    styles(&integration.join("render-tests"), &mut paths);
    styles(&integration.join("query-tests"), &mut paths);
    paths.sort();
    paths
}

/// The layers of every style, with their style's path. Layers that don't
/// parse in the first place (tests of invalid styles) are left out.
fn every_layer(registry: &StyleRegistry) -> Vec<(PathBuf, Layer)> {
    let mut layers = Vec::new();
    for path in every_style() {
        let style: serde_json::Value = match serde_json::from_str(&fs::read_to_string(&path).unwrap()) {
            Ok(style) => style,
            Err(_) => continue,
        };
        let Some(style_layers) = style.get("layers").and_then(|layers| layers.as_array()) else { continue };
        for json in style_layers {
            if let Some(layer) = convert_layer(registry, &Convertible::from(json), &mut Error::default()) {
                layers.push((path.clone(), layer));
            }
        }
    }
    assert!(layers.len() > 1000, "only {} layers found", layers.len());
    layers
}

#[test]
fn every_layer_crosses() {
    let registry = Arc::clone(Registry::with_builtins().style());
    let layers = every_layer(&registry);
    let mut failures = Vec::new();
    for (path, layer) in &layers {
        let encoded = mln_wire::to_vec(&Crossing(Arc::clone(layer.style()))).unwrap();
        match mln_wire::from_slice_in::<Crossing>(&encoded, &*registry) {
            Ok(decoded) => {
                if mln_wire::to_vec(&decoded).unwrap() != encoded {
                    failures.push(format!("{}: crosses as another layer\n  {:?}", path.display(), layer.serialize()));
                }
            }
            Err(error) => failures.push(format!("{}: {error}\n  {:?}", path.display(), layer.serialize())),
        }
    }
    assert!(failures.is_empty(), "{} of {} layers don't cross:\n{}", failures.len(), layers.len(), failures.join("\n"));
}

/// Styles whose layers don't parse back. A zoom function of a color array
/// property serializes as an expression of single colors, and an expression
/// can't make a color array from a single color: the expression types have no
/// `colorArray`.
const NOT_REPARSED: [&str; 2] = [
    "render-tests/hillshade-highlight-color/zoom-function/style.json",
    "render-tests/hillshade-shadow-color/zoom-function/style.json",
];

#[test]
fn every_layer_reparses() {
    let registry = Arc::clone(Registry::with_builtins().style());
    let layers = every_layer(&registry);
    let mut failures = Vec::new();
    for (path, layer) in &layers {
        if NOT_REPARSED.iter().any(|style| path.ends_with(style)) {
            continue;
        }
        let serialized = layer.serialize();
        let mut error = Error::default();
        match convert_layer(&registry, &Convertible::from(&serialized), &mut error) {
            // Compared as text, so that NaN equals itself.
            Some(reparsed) if format!("{:?}", reparsed.serialize()) == format!("{serialized:?}") => {}
            Some(reparsed) => failures.push(format!(
                "{}: parses back as another layer\n  {serialized:?}\n  {:?}",
                path.display(),
                reparsed.serialize()
            )),
            None => failures.push(format!("{}: {}\n  {serialized:?}", path.display(), error.message)),
        }
    }
    assert!(
        failures.is_empty(),
        "{} of {} layers don't parse back:\n{}",
        failures.len(),
        layers.len(),
        failures.join("\n")
    );
}
