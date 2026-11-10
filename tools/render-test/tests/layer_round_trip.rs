// Not a C++ test: every layer in every render- and query-test style crosses
// through serde (its `Layer::Impl`, as a tile worker in another wasm
// instance gets its layers) to one that encodes the same, its expressions as
// the JSON they serialize to (mln_style::serialization). A layer that
// doesn't cross couldn't be laid out there, and one that crosses as another
// layer would be laid out differently.

use std::fs;
use std::path::{Path, PathBuf};

use mln_conversion::{Convertible, Error};
use mln_style::conversion::convert_layer;
use mln_style::layer_impl::Impl;
use mln_util::immutable::Immutable;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
struct Crossing(#[serde(with = "mln_style::serialization::shared_layer")] Immutable<dyn Impl>);

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

#[test]
fn every_layer_crosses_through_serde() {
    let integration = Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata/metrics/integration");
    let mut paths = Vec::new();
    styles(&integration.join("render-tests"), &mut paths);
    styles(&integration.join("query-tests"), &mut paths);
    paths.sort();

    let mut layers = 0;
    let mut failures = Vec::new();
    for path in &paths {
        let style: serde_json::Value = match serde_json::from_str(&fs::read_to_string(path).unwrap()) {
            Ok(style) => style,
            Err(_) => continue,
        };
        let Some(style_layers) = style.get("layers").and_then(|layers| layers.as_array()) else { continue };
        for json in style_layers {
            // Layers that don't parse in the first place (tests of invalid
            // styles) have nothing to send.
            let Some(layer) = convert_layer(&Convertible::from(json), &mut Error::default()) else { continue };
            layers += 1;
            let encoded = postcard::to_allocvec(&Crossing(layer.base_impl())).unwrap();
            match postcard::from_bytes::<Crossing>(&encoded) {
                Ok(decoded) => {
                    if postcard::to_allocvec(&decoded).unwrap() != encoded {
                        failures.push(format!(
                            "{}: crosses as another layer\n  {:?}",
                            path.display(),
                            layer.serialize()
                        ));
                    }
                }
                Err(error) => failures.push(format!("{}: {error}\n  {:?}", path.display(), layer.serialize())),
            }
        }
    }
    assert!(layers > 1000, "only {layers} layers found");
    assert!(failures.is_empty(), "{} of {layers} layers don't cross:\n{}", failures.len(), failures.join("\n"));
}
