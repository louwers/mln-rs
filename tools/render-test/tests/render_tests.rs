// Every render and query test of the shared suite on the Rust renderer, and
// those of the extension layer types that the runner's registry adds
// (src/map.rs). The port's ignore file (ignores/rust-port.json) lists what
// of the shared suite is expected to fail.

use crate::suite;

#[test]
fn render_tests() {
    assert_eq!(suite::run("render_tests", &[], Some(render_test::map::factory())), 0);
}

/// The same tests with the tile workers in two worker instances that the map
/// reaches only through the worker codec, as in the browser build: every
/// message, bucket and tile crosses as bytes.
#[test]
fn render_tests_through_the_codec() {
    let factory = Some(render_test::map::factory());
    assert_eq!(suite::run("render_tests_through_the_codec", &["--tileWorkerCodec", "2"], factory), 0);
}

/// The manifests of the extension layer types' own tests, which expect the
/// same images on every backend.
const EXTENSIONS: [(&str, &str); 2] = [
    ("ngon", "../../crates/mln_ngon/render-tests/manifest.json"),
    ("location_puck", "../../crates/mln_location_puck/render-tests/manifest.json"),
];

#[test]
fn extension_render_tests() {
    for (name, manifest) in EXTENSIONS {
        let factory = Some(render_test::map::factory());
        assert_eq!(suite::run_manifest(&format!("{name}_render_tests"), manifest, &[], factory), 0, "{name}");
        let factory = Some(render_test::map::factory());
        let codec = ["--tileWorkerCodec", "2"];
        let name = format!("{name}_render_tests_through_the_codec");
        assert_eq!(suite::run_manifest(&name, manifest, &codec, factory), 0, "{name}");
    }
}
