// A direct check of the M1 background render tests
// (metrics/integration/render-tests/background-*) through Map ->
// HeadlessFrontend, without the render-test runner: load style.json, jump to
// the style's default camera, render a still and compare with expected.png
// using the runner's metric (pixelmatch threshold 0.1285, `allowed` default
// 0.00015). Operations are not run, so tests with operations are skipped;
// `//tools/render-test:m1_render_tests` is the authoritative run.

use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;

use mln_actor::RunLoop;
use mln_gfx::ContextMode;
use mln_headless::HeadlessFrontend;
use mln_image::decode_image;
use mln_map::{Map, MapMode, MapOptions, NullMapObserver};
use mln_util::size::Size;
use mln_webgpu::HeadlessBackend;

fn render_tests_dir() -> PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata/metrics/integration/render-tests")
}

/// Returns (difference, allowed).
fn run(test: &str) -> (f64, f64) {
    run_on(test, HeadlessFrontend::new)
}

/// `run` with the frontend `frontend` makes for the size and pixel ratio.
fn run_on(test: &str, frontend: impl FnOnce(Size, f32) -> HeadlessFrontend) -> (f64, f64) {
    let dir = render_tests_dir().join(test);
    let json = std::fs::read_to_string(dir.join("style.json")).expect("style.json");
    let document: serde_json::Value = serde_json::from_str(&json).expect("json");
    let test_metadata = &document["metadata"]["test"];
    assert!(test_metadata.get("operations").is_none(), "{test} has operations");
    let width = test_metadata["width"].as_u64().unwrap_or(512) as u32;
    let height = test_metadata["height"].as_u64().unwrap_or(512) as u32;
    let pixel_ratio = test_metadata["pixelRatio"].as_f64().unwrap_or(1.0) as f32;
    let allowed = test_metadata["allowed"].as_f64().unwrap_or(0.00015);
    let size = Size::new(width, height);

    let _run_loop = RunLoop::new();
    let mut frontend = frontend(size, pixel_ratio);
    let mut map = Map::new(
        &frontend,
        Rc::new(NullMapObserver),
        MapOptions::new().with_map_mode(MapMode::Static).with_size(size).with_pixel_ratio(pixel_ratio),
    )
    .unwrap();
    map.get_style_mut().load_json(&json);
    let camera = map.get_style().get_default_camera();
    map.jump_to(&camera);
    let actual = frontend.render(&mut map).expect("render").image;

    let expected = decode_image(&std::fs::read(dir.join("expected.png")).expect("expected.png")).expect("decode");
    assert_eq!(actual.size, expected.size, "{test}: size");
    let pixels = pixelmatch::pixelmatch(
        &actual.data,
        &expected.data,
        expected.size.width as usize,
        expected.size.height as usize,
        None,
        0.1285,
        false,
    );
    let difference = pixels as f64 / expected.size.area() as f64;
    eprintln!(
        "{test}: difference {difference} (allowed {allowed}); actual[0] = {:?}, expected[0] = {:?}",
        &actual.data[..4],
        &expected.data[..4]
    );
    (difference, allowed)
}

fn check(test: &str) {
    let (difference, allowed) = run(test);
    assert!(difference <= allowed, "{test}: difference {difference} > allowed {allowed}");
}

#[test]
fn background_color_default() {
    check("background-color/default");
}

#[test]
fn background_color_literal() {
    check("background-color/literal");
}

#[test]
fn background_color_function() {
    check("background-color/function");
}

#[test]
fn background_color_color_space_lab() {
    check("background-color/colorSpace-lab");
}

/// Ignored by the C++ metrics/ignores/platform-all.json; C++ fails it too.
#[test]
fn background_color_color_space_hcl() {
    let (difference, allowed) = run("background-color/colorSpace-hcl");
    eprintln!("colorSpace-hcl (C++ fails it too): difference {difference}, allowed {allowed}");
}

/// Needs the render-test runner's ProxyFileSource: the style's sprite
/// (`local://sprites/sprite`) is served cache-only from
/// metrics/cache-style.db as `http://sprites/sprite.*`. With a plain Map the
/// cache misses and the request falls through to the network (C++: an
/// online request that fails; the port has no online file source yet, so
/// nothing answers); `//tools/render-test:m1_render_tests` covers it.
#[test]
#[ignore = "needs the render-test ProxyFileSource (see //tools/render-test:m1_render_tests)"]
fn background_opacity_color() {
    check("background-opacity/color");
}

#[test]
fn background_visibility_none() {
    check("background-visibility/none");
}

#[test]
fn background_visibility_visible() {
    check("background-visibility/visible");
}

/// maplibre-native-ffi's offscreen device: maps whose frontends render on
/// one device, in turn.
#[test]
fn frontends_on_one_device_render_in_turn() {
    let (device, queue) = HeadlessBackend::request_device(None).expect("device");
    let on_shared_device = |size, pixel_ratio| {
        let backend = HeadlessBackend::with_device(device.clone(), queue.clone(), Size::default(), ContextMode::Unique);
        HeadlessFrontend::with_backend(backend, size, pixel_ratio, true, Arc::default())
    };
    for test in ["background-color/literal", "background-color/function", "background-color/literal"] {
        let (difference, allowed) = run_on(test, on_shared_device);
        assert!(difference <= allowed, "{test}: difference {difference} > allowed {allowed}");
    }
}
