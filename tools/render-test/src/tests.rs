// Tests of the runner port. The C++ runner has no unit tests of its own;
// these check the ported behaviour on the shared fixtures (testdata/, from
// the package directory `cargo test` runs them in) and on small generated
// suites with a mock TestRunnerImpl.

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use mln_engine::image::encode_png;
use mln_style::bitmap::PremultipliedImage;
use mln_style::size::Size;

use crate::manifest_parser::ManifestParser;
use crate::metadata::{MapMode, TerrainSkirtLength, TestMetadata};
use crate::parser::{parse_test_metadata, parse_test_operations};
use crate::render_test::{Arguments, RunResult, parse_arguments, run_render_tests_with};
use crate::runner::{ImplFactory, ImplOptions, MapError, ResourceOptions, TestContext, TestRunner, TestRunnerImpl};

const OUR_MANIFEST: &str = "manifests/linux-wgpu.json";
const CPP_MANIFEST: &str = "testdata/metrics/linux-opengl.json";
const INTEGRATION: &str = "testdata/metrics/integration";

const M1_FILTER: &str = "/render-tests/(background-color/(default|literal|function|colorSpace-hcl|colorSpace-lab|transition)|background-opacity/color|background-visibility/(none|visible))/style\\.json$";

/// Sets the working directory to the `render-test` package directory once, for
/// device runners (such as `xcrun simctl spawn`) that do not preserve Cargo's
/// working directory.
fn in_package_dir() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let dir = std::env::var_os("CARGO_MANIFEST_DIR")
            .map_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")), PathBuf::from);
        std::env::set_current_dir(&dir).unwrap_or_else(|e| panic!("chdir {}: {e}", dir.display()));
    });
}

fn style_paths(id: &str) -> crate::metadata::TestPaths {
    in_package_dir();
    let dir = format!("{INTEGRATION}/{id}/");
    crate::metadata::TestPaths::new(format!("{dir}style.json"), vec![dir.clone()], vec![dir])
}

/// A fresh directory in the system's temporary directory, which no other test run shares and
/// which is removed when it drops.
fn tmp_dir(name: &str) -> tempfile::TempDir {
    tempfile::Builder::new().prefix(&format!("render-test-{name}-")).tempdir().unwrap()
}

/// `path` in the generic format the runner returns paths in, `/` on Windows
/// too, as C++'s `generic_string()`.
fn generic(path: &Path) -> String {
    let path = path.to_str().unwrap();
    if cfg!(windows) { path.replace('\\', "/") } else { path.to_string() }
}

fn write(path: &Path, contents: impl AsRef<[u8]>) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, contents).unwrap();
}

fn solid(size: Size, rgba: [u8; 4]) -> PremultipliedImage {
    let mut image = PremultipliedImage::new(size);
    for px in image.data.as_chunks_mut::<4>().0 {
        px.copy_from_slice(&rgba);
    }
    image
}

// --- manifest_parser ---

#[test]
fn parses_our_manifest() {
    in_package_dir();
    let manifest = ManifestParser::parse_manifest(OUR_MANIFEST, String::new()).unwrap();
    assert_eq!(manifest.get_manifest_path(), "manifests");
    assert_eq!(manifest.get_result_path(), "manifests");
    assert_eq!(manifest.get_test_root_path(), INTEGRATION);
    assert_eq!(manifest.get_cache_path(), "manifests/../testdata/metrics/cache-style.db");
    assert_eq!(manifest.get_asset_path(), "testdata/metrics/integration/gljs/assets");
    assert!(manifest.get_probes().is_empty());

    // Ignore files in manifest order, members in document order.
    let ignores = manifest.get_ignores();
    assert_eq!(
        ignores[0],
        (
            "expression-tests/collator/accent-equals-de".to_string(),
            "Locale-specific behavior changes based on platform.".to_string()
        )
    );
    assert!(ignores.contains(&(
        "render-tests/background-color/transition".to_string(),
        "https://github.com/mapbox/mapbox-gl-native/issues/10619".to_string()
    )));
    // The port's own ignore file is read last (its entries come and go).
    assert!(ignores.iter().any(|(id, _)| id.starts_with("query-tests/") || id.starts_with("render-tests/")));
    // M1 tests are not ignored by the port.
    assert!(!ignores.iter().any(|(id, _)| id == "render-tests/background-color/default"));

    let default = manifest
        .get_test_paths()
        .iter()
        .find(|t| t.style_path.ends_with("/render-tests/background-color/default/style.json"))
        .unwrap();
    assert_eq!(default.style_path, format!("{INTEGRATION}/render-tests/background-color/default/style.json"));
    assert_eq!(
        default.expectations,
        vec![
            format!("{INTEGRATION}/render-tests/background-color/default/"),
            // platform-linux first, as in C++'s OpenGL manifest: the runner
            // checks the paths from the last one, so a platform-linux-opengl
            // baseline wins where both exist, and the port's own over both.
            "testdata/metrics/expectations/platform-linux/render-tests/background-color/default/".to_string(),
            "testdata/metrics/expectations/platform-linux-opengl/render-tests/background-color/default/".to_string(),
            "expectations/render-tests/background-color/default/".to_string(),
        ]
    );
    assert_eq!(
        default.expected_metrics,
        vec![
            format!("{INTEGRATION}/render-tests/background-color/default/"),
            "testdata/metrics/linux-gcc8-release/render-tests/background-color/default/".to_string(),
        ]
    );
}

#[test]
fn filters_tests_by_regex_search_on_the_path() {
    in_package_dir();
    let manifest = ManifestParser::parse_manifest(OUR_MANIFEST, M1_FILTER.to_string()).unwrap();
    let mut ids: Vec<&str> =
        manifest.get_test_paths().iter().map(|t| t.style_path.strip_prefix(INTEGRATION).unwrap()).collect();
    ids.sort_unstable();
    assert_eq!(
        ids,
        vec![
            "/render-tests/background-color/colorSpace-hcl/style.json",
            "/render-tests/background-color/colorSpace-lab/style.json",
            "/render-tests/background-color/default/style.json",
            "/render-tests/background-color/function/style.json",
            "/render-tests/background-color/literal/style.json",
            "/render-tests/background-color/transition/style.json",
            "/render-tests/background-opacity/color/style.json",
            "/render-tests/background-visibility/none/style.json",
            "/render-tests/background-visibility/visible/style.json",
        ]
    );
}

#[test]
fn shuffle_order_depends_only_on_the_seed() {
    in_package_dir();
    let manifest = ManifestParser::parse_manifest(OUR_MANIFEST, String::new()).unwrap();
    let ids = |m: &crate::manifest_parser::Manifest| -> Vec<String> {
        m.get_test_paths().iter().map(|t| t.style_path.clone()).collect()
    };
    let shuffled = |seed| {
        let mut m = manifest.clone();
        m.do_shuffle(seed);
        ids(&m)
    };
    let original = ids(&manifest);
    let order = shuffled(1);
    assert_eq!(order, shuffled(1));
    assert_ne!(order, original);
    assert_ne!(order, shuffled(2));
    let (mut sorted, mut original_sorted) = (order, original);
    sorted.sort_unstable();
    original_sorted.sort_unstable();
    assert_eq!(sorted, original_sorted);
}

#[test]
fn parses_the_cpp_linux_manifest() {
    in_package_dir();
    let manifest = ManifestParser::parse_manifest(CPP_MANIFEST, "background-color".to_string()).unwrap();
    assert_eq!(manifest.get_manifest_path(), "testdata/metrics");
    assert_eq!(manifest.get_test_root_path(), INTEGRATION);
    assert_eq!(manifest.get_cache_path(), "testdata/metrics/cache-style.db");
    assert_eq!(manifest.get_probes().iter().collect::<Vec<_>>(), vec!["probeNetwork"]);
    assert_eq!(manifest.get_test_paths().len(), 7);
}

#[test]
fn manifest_errors() {
    in_package_dir();
    let tmp = tmp_dir("manifest_errors");
    let dir = tmp.path().to_owned();
    // Absolute, without the `\\?\` prefix that canonicalize gives on Windows.
    let integration = std::path::absolute(INTEGRATION).unwrap();
    // A JSON string, quotes included: Windows paths have backslashes to escape.
    let integration = serde_json::to_string(integration.to_str().unwrap()).unwrap();
    let parse = |name: &str, contents: &str| {
        let path = dir.join(name);
        write(&path, contents);
        ManifestParser::parse_manifest(path.to_str().unwrap(), String::new())
    };
    assert!(parse("not-json.json", "{").is_none());
    assert!(parse("result-path.json", r#"{"result_path": 1}"#).is_none());
    assert!(parse("missing-base.json", r#"{"base_test_path": "does-not-exist"}"#).is_none());
    assert!(parse("no-base.json", "{}").is_none());
    assert!(
        parse(
            "bad-expectation.json",
            &format!(r#"{{"base_test_path": {integration}, "expectation_paths": ["nope"]}}"#)
        )
        .is_none()
    );
    assert!(
        parse("bad-ignores.json", &format!(r#"{{"base_test_path": {integration}, "ignore_paths": "x"}}"#)).is_none()
    );
    assert!(parse("bad-probe.json", &format!(r#"{{"base_test_path": {integration}, "probes": [1]}}"#)).is_none());

    // The manifest's own filter applies when none is given; a given one wins.
    let manifest = parse(
        "filter.json",
        &format!(r#"{{"base_test_path": {integration}, "filter": "background-visibility/none/", "result_path": "."}}"#),
    )
    .unwrap();
    assert_eq!(manifest.get_test_paths().len(), 1);
    assert_eq!(manifest.get_result_path(), generic(&dir));
    let path = dir.join("filter.json");
    let manifest = ManifestParser::parse_manifest(path.to_str().unwrap(), "background-color/".to_string()).unwrap();
    assert_eq!(manifest.get_test_paths().len(), 7);
}

// --- parser: metadata ---

#[test]
fn parses_test_metadata() {
    let metadata = parse_test_metadata(&style_paths("render-tests/background-color/default"));
    assert_eq!(metadata.size, Size::new(64, 64));
    assert_eq!(metadata.pixel_ratio, 1.0);
    assert_eq!(metadata.allowed, 0.00015);
    assert_eq!(metadata.map_mode, MapMode::Static);
    assert!(metadata.render_test && metadata.outputs_image && metadata.ignore_probing);
    assert_eq!(metadata.error_message, "");
    assert_eq!(
        metadata.document_json,
        std::fs::read_to_string(format!("{INTEGRATION}/render-tests/background-color/default/style.json")).unwrap()
    );

    let overlay = parse_test_metadata(&style_paths("render-tests/background-opacity/overlay"));
    assert_eq!(overlay.size, Size::new(512, 256));

    let tile = parse_test_metadata(&style_paths("render-tests/symbol-placement/line-center-tile-map-mode"));
    assert_eq!(tile.map_mode, MapMode::Tile);
    assert_eq!(tile.size, Size::new(512, 512));

    let retina = parse_test_metadata(&style_paths("render-tests/runtime-styling/image-add-1x-image-2x-screen"));
    assert_eq!(retina.pixel_ratio, 2.0);

    let allowed = parse_test_metadata(&style_paths("render-tests/text-pitch-alignment/viewport-text-depthtest"));
    assert_eq!(allowed.allowed, 0.00025);
    assert_eq!(allowed.threshold, 0.1285);
    assert_eq!(allowed.size, Size::new(512, 256));

    let threshold =
        parse_test_metadata(&style_paths("render-tests/fill-extrusion-rounded-corner-distance/smooth-shading"));
    assert_eq!(threshold.threshold, 0.02);

    let skew = parse_test_metadata(&style_paths("render-tests/projection/skew"));
    assert!(skew.axonometric);
    assert_eq!((skew.x_skew, skew.y_skew), (-1.0, -1.0));

    let query = parse_test_metadata(&style_paths("query-tests/circle-radius-features-in/inside"));
    assert!(!query.render_test);
    assert_eq!(query.query_geometry_box.min.x, 1.0);
    assert_eq!(query.query_geometry_box.max.y, 120.0);

    // GL JS's fixtures, as the importer copies them.
    let imported = parse_test_metadata(&style_paths("gljs/render-tests/image/default"));
    assert_eq!(imported.allowed, 0.00025);
    assert_eq!(imported.terrain_skirt_length, TerrainSkirtLength::Auto);
    let skirts = |id| parse_test_metadata(&style_paths(id)).terrain_skirt_length;
    assert_eq!(skirts("gljs/render-tests/terrain/skirts-auto"), TerrainSkirtLength::Auto);
    assert_eq!(skirts("gljs/render-tests/terrain/skirts-none"), TerrainSkirtLength::None);

    let missing = parse_test_metadata(&style_paths("render-tests/does-not-exist"));
    assert!(missing.document_object_empty());
    assert!(missing.error_message.starts_with("Unable to parse: "));
}

// --- parser + runner: operations, with a mock map ---

/// Records the calls; "renders" a solid image of `color` at the test's
/// physical size; knows one layer, "background", whose `background-color`
/// "white" turns the color white.
struct MockMap {
    calls: Rc<RefCell<Vec<String>>>,
    size: Size,
    color: [u8; 4],
}

impl MockMap {
    fn new(calls: Rc<RefCell<Vec<String>>>, color: [u8; 4]) -> Self {
        MockMap { calls, size: Size::default(), color }
    }
    fn log(&self, call: String) {
        self.calls.borrow_mut().push(call);
    }
}

impl TestRunnerImpl for MockMap {
    fn reset_context(&mut self, metadata: &TestMetadata) -> Result<(), MapError> {
        self.size = Size::new(
            (metadata.size.width as f32 * metadata.pixel_ratio) as u32,
            (metadata.size.height as f32 * metadata.pixel_ratio) as u32,
        );
        let style: serde_json::Value = serde_json::from_str(&metadata.document_json).unwrap();
        if let Some(color) = style.get("mock-color").and_then(|c| c.as_array()) {
            for (i, c) in color.iter().enumerate() {
                self.color[i] = c.as_u64().unwrap() as u8;
            }
        }
        self.log("reset".to_string());
        Ok(())
    }
    fn jump_to_default_camera(&mut self) -> Result<(), MapError> {
        self.log("camera".to_string());
        Ok(())
    }
    fn render(&mut self) -> Result<PremultipliedImage, String> {
        self.log("render".to_string());
        if self.color == [1, 2, 3, 4] {
            return Err("mock render failure".to_string());
        }
        Ok(solid(self.size, self.color))
    }
    fn set_terrain(&mut self, json: &str) -> Result<(), MapError> {
        self.log(format!("terrain {json}"));
        Ok(())
    }
    fn set_center_clamped_to_ground(&mut self, clamped: bool) -> Result<(), MapError> {
        self.log(format!("clamped {clamped}"));
        Ok(())
    }
    fn set_layer_property(&mut self, layer: &str, name: &str, value: &serde_json::Value) -> Result<(), MapError> {
        self.log(format!("set {layer} {name} {value}"));
        if layer != "background" {
            return Err(MapError::LayerNotFound);
        }
        if name == "background-color" && value == "white" {
            self.color = [255, 255, 255, 255];
        }
        Ok(())
    }
}

fn run_operations(id: &str, map: &mut MockMap) -> (TestMetadata, bool) {
    let mut metadata = parse_test_metadata(&style_paths(id));
    let operations = parse_test_operations(&metadata).unwrap();
    let mut ok = true;
    {
        let mut ctx = TestContext::with_impl(&mut metadata, map);
        for operation in &operations {
            if !operation(&mut ctx) {
                ok = false;
                break;
            }
        }
    }
    (metadata, ok)
}

#[test]
fn transition_operations_drive_the_map() {
    let calls = Rc::new(RefCell::new(Vec::new()));
    let mut map = MockMap::new(Rc::clone(&calls), [255, 0, 0, 255]);
    let (metadata, ok) = run_operations("render-tests/background-color/transition", &mut map);
    assert!(ok);
    assert_eq!(metadata.error_message, "");
    assert_eq!(*calls.borrow(), vec!["set background background-color \"white\"", "render", "render"]);
    assert_eq!(map.color, [255, 255, 255, 255]);
}

#[test]
fn terrain_operations_drive_the_map() {
    let mut metadata = TestMetadata {
        document: serde_json::json!({"metadata": {"test": {"operations": [
            ["wait"],
            ["setTerrain"],
            ["idle"],
            ["setTerrain", {"source": "dem", "exaggeration": 1}],
            ["setCenterClampedToGround", false],
        ]}}}),
        ..TestMetadata::default()
    };
    let operations = parse_test_operations(&metadata).unwrap();
    let calls = Rc::new(RefCell::new(Vec::new()));
    let mut map = MockMap::new(Rc::clone(&calls), [0, 0, 0, 255]);
    let mut ctx = TestContext::with_impl(&mut metadata, &mut map);
    assert!(operations.iter().all(|operation| operation(&mut ctx)));
    assert_eq!(
        *calls.borrow(),
        vec!["render", "terrain null", "render", r#"terrain {"source":"dem","exaggeration":1}"#, "clamped false"]
    );
}

#[test]
fn unsupported_operations_error_the_test() {
    let calls = Rc::new(RefCell::new(Vec::new()));
    let mut map = MockMap::new(Rc::clone(&calls), [0, 0, 0, 255]);
    // addImage, addLayer, wait: addImage is not implemented by the mock.
    let (metadata, ok) = run_operations("render-tests/runtime-styling/image-add-1x-image-2x-screen", &mut map);
    assert!(!ok);
    assert_eq!(metadata.render_errored, 1);
    assert!(metadata.error_message.starts_with("Unsupported: "), "{}", metadata.error_message);
}

#[test]
fn known_unsupported_operations_error_the_test() {
    for name in ["probeMemoryStart", "probeMemory", "probeMemoryEnd", "probeGFX"] {
        let mut metadata = TestMetadata {
            document: serde_json::json!({"metadata": {"test": {"operations": [[name]]}}}),
            ..TestMetadata::default()
        };
        let operations = parse_test_operations(&metadata).unwrap();
        assert_eq!(operations.len(), 1, "{name}");
        let calls = Rc::new(RefCell::new(Vec::new()));
        let mut map = MockMap::new(calls, [0, 0, 0, 255]);
        let mut ctx = TestContext::with_impl(&mut metadata, &mut map);
        assert!(!operations[0](&mut ctx), "{name}");
        assert_eq!(metadata.render_errored, 1, "{name}");
        assert!(
            metadata.error_message.starts_with(&format!("Unsupported: {name} operation (")),
            "{}",
            metadata.error_message
        );
    }
}

#[test]
fn operations_that_fail_to_parse_are_an_error() {
    let error = |operations: serde_json::Value| {
        let metadata = TestMetadata {
            document: serde_json::json!({"metadata": {"test": {"operations": operations}}}),
            ..TestMetadata::default()
        };
        parse_test_operations(&metadata).err().unwrap_or_else(|| panic!("{operations} parsed"))
    };
    assert_eq!(error(serde_json::json!([["wait"], ["frobnicate"]])), "Unsupported operation: frobnicate");
    assert_eq!(
        error(serde_json::json!([["panGesture", "x", 1, [0, 0, 1], [1, 1, 2]]])),
        "Map mode must be Continuous for panGesture operation"
    );
    assert!(error(serde_json::json!([["setCenter", [0, 91]]])).starts_with("Invalid setCenter operation: "));
    assert_eq!(error(serde_json::json!([["setTileLodMode", "nearest"]])), "Unknown tile LOD mode: nearest");
    assert_eq!(
        error(serde_json::json!([["setFeatureState", {"source": "s", "id": 1}, {"hover": {}}]])),
        "Could not get feature state value, state key: hover"
    );
    assert_eq!(error(serde_json::json!({"wait": true})), r#"Operations must be an array, not {"wait":true}"#);
}

#[test]
fn idle_renders_like_wait() {
    let mut metadata = TestMetadata {
        document: serde_json::json!({"metadata": {"test": {"operations": [["idle"]]}}}),
        ..TestMetadata::default()
    };
    let operations = parse_test_operations(&metadata).unwrap();
    let calls = Rc::new(RefCell::new(Vec::new()));
    let mut map = MockMap::new(Rc::clone(&calls), [0, 0, 0, 255]);
    let mut ctx = TestContext::with_impl(&mut metadata, &mut map);
    assert!(operations[0](&mut ctx));
    assert_eq!(*calls.borrow(), vec!["render"]);
}

// --- runner: checkRenderTestResults ---

fn check(dir: &Path, actual: &PremultipliedImage, expectations: Vec<String>) -> TestMetadata {
    check_at(dir, actual, expectations, TestMetadata::default().threshold)
}

/// As [`check`], with the test's per-pixel `threshold`.
fn check_at(dir: &Path, actual: &PremultipliedImage, expectations: Vec<String>, threshold: f64) -> TestMetadata {
    in_package_dir();
    let output = dir.join("out");
    let manifest = ManifestParser::parse_manifest(OUR_MANIFEST, "background-color/default/".to_string()).unwrap();
    let runner = TestRunner::new(manifest, None, false, output);
    let mut metadata = TestMetadata {
        id: "check".to_string(),
        paths: crate::metadata::TestPaths::new("style.json".to_string(), expectations, vec![]),
        threshold,
        ..TestMetadata::default()
    };
    runner.check_render_test_results(actual, &mut metadata);
    metadata
}

#[test]
fn compares_against_expectations_like_cpp() {
    let tmp = tmp_dir("compare");
    let dir = tmp.path().to_owned();
    let size = Size::new(8, 8);
    let red = solid(size, [255, 0, 0, 255]);
    let blue = solid(size, [0, 0, 255, 255]);
    let default_dir = dir.join("default/");
    let platform_dir = dir.join("platform/");
    write(&default_dir.join("expected.png"), encode_png(&red));
    let d = |p: &Path| format!("{}/", p.display());

    // Identical: passes, difference 0, diff.png and actual.png written.
    let m = check(&dir, &red, vec![d(&default_dir)]);
    assert_eq!((m.render_failed, m.render_errored), (0, 0));
    assert_eq!(m.difference, 0.0);
    assert!(dir.join("out/check/actual.png").exists());
    assert!(dir.join("out/check/diff.png").exists());

    // Every pixel differs: fails with difference 1.
    let m = check(&dir, &blue, vec![d(&default_dir)]);
    assert_eq!((m.render_failed, m.render_errored), (1, 0));
    assert_eq!(m.difference, 1.0);

    // One pixel of 64 differs: 1/64 > 0.00015 fails, unless `allowed` says so.
    let mut one_off = red.clone();
    one_off.data[0..4].copy_from_slice(&[0, 0, 255, 255]);
    let m = check(&dir, &one_off, vec![d(&default_dir)]);
    assert_eq!(m.difference, 1.0 / 64.0);
    assert_eq!(m.render_failed, 1);

    // A pixel slightly off matches at GL JS's default threshold, but not at
    // a stricter one the test sets.
    let mut faint = red.clone();
    faint.data[0..4].copy_from_slice(&[235, 0, 0, 255]);
    assert_eq!(check(&dir, &faint, vec![d(&default_dir)]).difference, 0.0);
    assert_eq!(check_at(&dir, &faint, vec![d(&default_dir)], 0.02).difference, 1.0 / 64.0);

    // An alternate expected-*.png in the same directory can match.
    write(&default_dir.join("expected-blue.png"), encode_png(&blue));
    let m = check(&dir, &blue, vec![d(&default_dir)]);
    assert_eq!((m.render_failed, m.render_errored), (0, 0));

    // A platform directory with expectations shadows the default one.
    write(&platform_dir.join("expected.png"), encode_png(&blue));
    let m = check(&dir, &red, vec![d(&default_dir), d(&platform_dir)]);
    assert_eq!(m.render_failed, 1);
    let m = check(&dir, &blue, vec![d(&default_dir), d(&platform_dir)]);
    assert_eq!(m.render_failed, 0);
    // A missing platform directory falls back to the default one.
    let m = check(&dir, &red, vec![d(&default_dir), d(&dir.join("nope/"))]);
    assert_eq!(m.render_failed, 0);

    // No expectations at all.
    let m = check(&dir, &red, vec![d(&dir.join("nope/"))]);
    assert_eq!(m.render_errored, 1);
    assert_eq!(m.error_message, "Failed to find expectations for: style.json");

    // An empty actual image.
    let m = check(&dir, &PremultipliedImage::default(), vec![d(&default_dir)]);
    assert_eq!(m.render_errored, 1);
    assert_eq!(m.error_message, "Invalid size for actual image");

    // An expected image larger than the actual one (undefined behaviour in
    // C++) fails instead of reading out of bounds.
    let small_dir = dir.join("small/");
    write(&small_dir.join("expected.png"), encode_png(&solid(Size::new(16, 16), [255, 0, 0, 255])));
    let m = check(&dir, &red, vec![d(&small_dir)]);
    assert_eq!((m.render_failed, m.difference), (1, 1.0));
}

// --- render_test: statuses and return codes over a generated suite ---

struct Suite {
    dir: PathBuf,
    _tmp: tempfile::TempDir,
}

impl Suite {
    fn new(name: &str) -> Self {
        let tmp = tmp_dir(name);
        Suite { dir: tmp.path().to_owned(), _tmp: tmp }
    }

    /// A test drawing `color` (see `MockMap`) with `expected` images in the
    /// test directory.
    fn test(&self, id: &str, color: [u8; 4], extra: &str, expected: &[(&str, [u8; 4])]) {
        let dir = self.dir.join("integration").join(id);
        let style = format!(
            r#"{{"version": 8, "mock-color": {color:?}, "metadata": {{"test": {{"width": 4, "height": 4{extra}}}}}, "sources": {{}}, "layers": [{{"id": "background", "type": "background"}}]}}"#
        );
        write(&dir.join("style.json"), style);
        for (name, rgba) in expected {
            write(&dir.join(name), encode_png(&solid(Size::new(4, 4), *rgba)));
        }
    }

    fn run(&self, ignores: &str, filter: &str) -> RunResult {
        write(&self.dir.join("ignores.json"), ignores);
        let integration = self.dir.join("integration");
        let manifest = serde_json::json!({
            "base_test_path": integration,
            "expectation_paths": [self.dir.join("platform")],
            "ignore_paths": [self.dir.join("ignores.json")],
        })
        .to_string();
        std::fs::create_dir_all(self.dir.join("platform")).unwrap();
        let manifest_path = self.dir.join("suite.json");
        write(&manifest_path, manifest);
        let args: Vec<String> = [
            "render-test",
            "-p",
            manifest_path.to_str().unwrap(),
            "--filter",
            filter,
            "--output",
            self.dir.join("out").to_str().unwrap(),
        ]
        .iter()
        .map(ToString::to_string)
        .collect();
        let arguments: Arguments = parse_arguments(&args).unwrap();
        run_render_tests_with(&arguments, Some(mock_factory()))
    }
}

fn mock_factory() -> ImplFactory {
    Box::new(
        |_metadata: &TestMetadata,
         _options: &ResourceOptions,
         _impl_options: &ImplOptions,
         _: &mln_engine::exec::queue::Queue<()>,
         _: &mln_engine::exec::pool::Pool| {
            Ok(Box::new(MockMap::new(Rc::new(RefCell::new(Vec::new())), [0, 0, 0, 0])))
        },
    )
}

fn status<'a>(result: &'a RunResult, id: &str) -> &'a str {
    &result.metadatas.iter().find(|m| m.id == id).unwrap_or_else(|| panic!("{id} did not run")).status
}

const RED: [u8; 4] = [255, 0, 0, 255];
const BLUE: [u8; 4] = [0, 0, 255, 255];

#[test]
fn a_test_whose_operations_fail_to_parse_errors() {
    let suite = Suite::new("unparsable");
    // Without its operations, the test would draw its expected red.
    suite.test("unparsable", RED, r#", "operations": [["frobnicate"]]"#, &[("expected.png", RED)]);
    suite.test("idle", RED, r#", "operations": [["idle"]]"#, &[("expected.png", RED)]);
    let result = suite.run("{}", "");

    assert_eq!(status(&result, "unparsable"), "errored");
    let unparsable = result.metadatas.iter().find(|m| m.id == "unparsable").unwrap();
    assert_eq!(unparsable.error_message, "Unsupported operation: frobnicate");
    assert_eq!(status(&result, "idle"), "passed");
}

#[test]
fn statuses_and_summary() {
    let suite = Suite::new("statuses");
    suite.test("pass", RED, "", &[("expected.png", RED)]);
    suite.test("fail", RED, "", &[("expected.png", BLUE)]);
    suite.test("alternate", RED, "", &[("expected.png", BLUE), ("expected-red.png", RED)]);
    suite.test("noexpected", RED, "", &[]);
    suite.test("render-error", [1, 2, 3, 4], "", &[("expected.png", RED)]);
    suite.test(
        "op-layer-not-found",
        RED,
        r#", "operations": [["setPaintProperty", "nope", "background-color", "red"]]"#,
        &[("expected.png", RED)],
    );
    suite.test(
        "op-transition",
        RED,
        r#", "operations": [["setPaintProperty", "background", "background-color", "white"], ["wait"]]"#,
        &[("expected.png", [255, 255, 255, 255])],
    );
    suite.test("op-todo", RED, r#", "operations": [["probeMemoryStart"]]"#, &[("expected.png", RED)]);
    suite.test("tile", RED, r#", "mapMode": "tile""#, &[("expected.png", RED)]);
    suite.test("ignored-pass", RED, "", &[("expected.png", RED)]);
    suite.test("ignored-fail", RED, "", &[("expected.png", BLUE)]);
    suite.test("skipped", RED, "", &[("expected.png", BLUE)]);
    write(&suite.dir.join("integration/bad-json/style.json"), "{");
    let ignores = r#"{"ignored-pass": "reason a", "ignored-fail": "reason b", "skipped": "skip - reason c"}"#;
    let result = suite.run(ignores, "");

    assert_eq!(status(&result, "pass"), "passed");
    assert_eq!(status(&result, "fail"), "failed");
    assert_eq!(status(&result, "alternate"), "passed");
    assert_eq!(status(&result, "noexpected"), "errored");
    assert_eq!(status(&result, "render-error"), "errored");
    assert_eq!(status(&result, "op-layer-not-found"), "errored");
    assert_eq!(status(&result, "op-transition"), "passed");
    assert_eq!(status(&result, "op-todo"), "errored");
    assert_eq!(status(&result, "tile"), "errored");
    assert_eq!(status(&result, "bad-json"), "errored");
    assert_eq!(status(&result, "ignored-pass"), "ignored passed");
    assert_eq!(status(&result, "ignored-fail"), "ignored failed");
    assert!(!result.metadatas.iter().any(|m| m.id == "skipped"));

    let render_error = result.metadatas.iter().find(|m| m.id == "render-error").unwrap();
    assert_eq!(render_error.error_message, "Rendering failed: mock render failure");

    let stats = result.stats;
    assert_eq!(
        (
            stats.passed_tests,
            stats.failed_tests,
            stats.errored_tests,
            stats.ignore_passed_tests,
            stats.ignore_failed_tests
        ),
        (3, 1, 6, 1, 1)
    );
    // The last non-passing, non-ignored test decides: 2 (errored) or 3 (failed).
    assert!(result.return_code == 2 || result.return_code == 3);
    let html = std::fs::read_to_string(suite.dir.join("out/suite.html")).unwrap();
    assert!(html.contains("7 tests failed."));
    assert!(html.contains("1 ignored failed, 1 ignored passed, 6 errored, 1 failed, 3 passed."));
    assert!(html.contains("Test sequence:</strong>") && html.contains("ignored-pass "));
    // Entries for the failed and errored tests only, the failed one with its
    // images and the errored ones with their errors.
    let has_entry = |id: &str| html.contains(&format!("</span> {id}</h2>"));
    for id in ["fail", "noexpected", "render-error", "op-layer-not-found", "op-todo", "tile", "bad-json"] {
        assert!(has_entry(id), "{id}");
    }
    for id in ["pass", "alternate", "op-transition", "ignored-pass", "ignored-fail"] {
        assert!(!has_entry(id), "{id}");
    }
    assert_eq!(html.matches("<div class=\"test ").count(), 7);
    assert!(html.contains(" data-alt-src=\"data:image/png;base64,"));
    assert!(html.contains("<strong>Error:</strong> Rendering failed: mock render failure"));
    // A passed test's images are written to disk, though the report leaves
    // them out.
    assert!(suite.dir.join("out/pass/actual.png").exists());
}

#[test]
fn return_codes() {
    let suite = Suite::new("return_codes");
    suite.test("a-pass", RED, "", &[("expected.png", RED)]);
    suite.test("b-fail", RED, "", &[("expected.png", BLUE)]);
    suite.test("c-error", RED, "", &[]);
    suite.test("d-ignored", RED, "", &[("expected.png", BLUE)]);
    let ignores = r#"{"d-ignored": "reason"}"#;
    assert_eq!(suite.run(ignores, "a-pass/").return_code, 0);
    assert_eq!(suite.run(ignores, "b-fail/").return_code, 3);
    assert_eq!(suite.run(ignores, "c-error/").return_code, 2);
    // Ignored failures do not fail the run.
    assert_eq!(suite.run(ignores, "(a-pass|d-ignored)/").return_code, 0);
}

#[test]
fn source_replace_reruns_with_swapped_sources() {
    let suite = Suite::new("source_replace");
    let dir = suite.dir.join("integration/replace");
    // The mock reads "mock-color" from the style it is given; the swap is
    // visible in the JSON it receives.
    write(
        &dir.join("style.json"),
        r#"{"version": 8, "metadata": {"test": {"width": 4, "height": 4, "source-replace": [{"replace": "a", "with": "b"}]}}, "sources": {"a": {"type": "geojson"}, "b": {"type": "vector"}}, "layers": []}"#,
    );
    write(&dir.join("expected.png"), encode_png(&solid(Size::new(4, 4), [0, 0, 0, 0])));
    let result = suite.run("{}", "");
    assert_eq!(status(&result, "replace"), "passed");
    let metadata = &result.metadatas[0];
    // Restored afterwards, in document order.
    assert_eq!(metadata.document["sources"]["a"]["type"], "geojson");
    let keys: Vec<&String> = metadata.document["sources"].as_object().unwrap().keys().collect();
    assert_eq!(keys, ["a", "b"]);
    assert!(metadata.document_json.contains(r#""a": {"type": "geojson"}"#));
}

#[test]
fn argument_parsing() {
    in_package_dir();
    let args = |list: &[&str]| -> Vec<String> {
        std::iter::once("render-test").chain(list.iter().copied()).map(String::from).collect()
    };
    let parsed = parse_arguments(&args(&["-p", OUR_MANIFEST, "-f", "x", "-r", "--seed=7"])).unwrap();
    assert_eq!(parsed.manifest_path, OUR_MANIFEST);
    assert_eq!(parsed.test_filter, "x");
    assert!(parsed.recycle_map && !parsed.shuffle && !parsed.self_check);
    assert_eq!(parsed.seed, 7);
    let parsed = parse_arguments(&args(&[&format!("--manifestPath={OUR_MANIFEST}"), "--self-check"])).unwrap();
    assert!(parsed.self_check);
    assert_eq!(parse_arguments(&args(&["-h"])), Err(0));
    assert_eq!(parse_arguments(&args(&["--bogus"])), Err(1));
    assert_eq!(parse_arguments(&args(&[])), Err(2));
    assert_eq!(parse_arguments(&args(&["-p", "nope.json"])), Err(3));
    assert_eq!(parse_arguments(&args(&["-p", OUR_MANIFEST, "-u", "platform"])), Err(1));
    assert_eq!(parse_arguments(&args(&["-p", OUR_MANIFEST, "--update=platform"])), Err(1));
    assert!(parse_arguments(&args(&["-p", OUR_MANIFEST, "-s"])).unwrap().shuffle);
    // --jobs (jobs.rs).
    assert_eq!(parse_arguments(&args(&["-p", OUR_MANIFEST])).unwrap().jobs, 1);
    assert_eq!(parse_arguments(&args(&["-p", OUR_MANIFEST, "--jobs=4"])).unwrap().jobs, 4);
    assert_eq!(parse_arguments(&args(&["-p", OUR_MANIFEST, "--jobs", "0"])).unwrap().jobs, 0);
    assert_eq!(parse_arguments(&args(&["-p", OUR_MANIFEST, "--jobs=many"])), Err(1));
}

// A test that times out reports the proxy's requests that are neither
// answered nor cancelled (runner::waiting_for), so a hang says what it
// waited for.
#[test]
fn pending_requests_leave_when_answered_or_cancelled() {
    use mln_engine::storage::{FileSource, FileSourceExt, Resource, Responder};

    use crate::file_source::{ProxyFileSource, answered, track};
    /// Never answers.
    struct Unanswered;
    impl FileSource for Unanswered {
        fn load(&self, _resource: &Resource, responder: Responder) {
            std::mem::forget(responder);
        }

        fn can_request(&self, _resource: &Resource) -> bool {
            true
        }
    }
    let is_pending = |url: &str| ProxyFileSource::pending_requests().iter().any(|(pending, _)| pending == url);

    let number = track("http://tiles/answered.mvt");
    assert!(is_pending("http://tiles/answered.mvt"));
    answered(number);
    assert!(!is_pending("http://tiles/answered.mvt"));

    let proxy = ProxyFileSource { default_resource_loader: std::sync::Arc::new(Unanswered), assets: None };
    let request = proxy.request(&Resource::style("http://tiles/cancelled.mvt"), |_| {});
    assert!(is_pending("http://tiles/cancelled.mvt"));
    drop(request);
    assert!(!is_pending("http://tiles/cancelled.mvt"));
}

#[test]
fn timeout_message_lists_the_pending_requests() {
    use std::time::Duration;

    use crate::runner::waiting_for;
    assert_eq!(waiting_for(&[]), "no resource request was outstanding");
    assert_eq!(
        waiting_for(&[("http://tiles/14-8802-5373.mvt".to_string(), Duration::from_millis(59_940))]),
        "waiting for 1 request: http://tiles/14-8802-5373.mvt (59.9 s)"
    );
    let many: Vec<_> = (0..7).map(|i| (format!("http://tiles/{i}.mvt"), Duration::from_secs(1))).collect();
    assert!(waiting_for(&many).starts_with("waiting for 7 requests: http://tiles/0.mvt (1.0 s), "));
    assert!(waiting_for(&many).ends_with("http://tiles/4.mvt (1.0 s), and 2 more"));
}

// --- GL JS's fixtures ---

/// The ids, relative to the integration directory, of the tests in `dir`.
fn fixture_ids(dir: &Path) -> Vec<String> {
    let root = Path::new(&std::env::var_os("CARGO_MANIFEST_DIR").unwrap()).join(INTEGRATION);
    let mut ids = Vec::new();
    let mut pending = vec![root.join(dir)];
    while let Some(dir) = pending.pop() {
        for entry in std::fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                pending.push(path);
            } else if path.file_name().is_some_and(|name| name == "style.json") {
                ids.push(generic(path.parent().unwrap().strip_prefix(&root).unwrap()));
            }
        }
    }
    ids.sort();
    ids
}

#[test]
fn gljs_fixtures_parse_and_no_ignore_is_stale() {
    in_package_dir();
    let ids = fixture_ids(Path::new("gljs/render-tests"));
    assert!(ids.len() > 40, "{ids:?}");

    let manifest = ManifestParser::parse_manifest(OUR_MANIFEST, String::new()).unwrap();
    let ignored: Vec<&str> =
        manifest.get_ignores().iter().map(|(id, _)| id.as_str()).filter(|id| id.starts_with("gljs/")).collect();
    for id in &ids {
        let metadata = parse_test_metadata(&style_paths(id));
        assert_eq!(metadata.error_message, "", "{id}");
        if let Err(error) = parse_test_operations(&metadata) {
            panic!("{id}: {error}");
        }
    }
    for id in ignored {
        assert!(ids.iter().any(|fixture| fixture == id), "{id} isn't a fixture");
    }
}

/// Every image in the port's `expectations/` stands for a test's, at the
/// test's path.
#[test]
fn the_ports_expectations_are_of_tests() {
    let package = Path::new(&std::env::var_os("CARGO_MANIFEST_DIR").unwrap()).to_path_buf();
    let root = package.join("expectations");
    let mut pending = vec![root.clone()];
    while let Some(dir) = pending.pop() {
        for entry in std::fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                pending.push(path);
            } else {
                let test = path.parent().unwrap().strip_prefix(&root).unwrap();
                let style = package.join(INTEGRATION).join(test).join("style.json");
                assert!(style.exists(), "{} isn't of a test", path.display());
            }
        }
    }
}

/// A file source that records the URLs it is asked for.
struct Recording(std::sync::Arc<parking_lot::Mutex<Vec<String>>>);

impl mln_engine::storage::FileSource for Recording {
    fn load(&self, resource: &mln_engine::storage::Resource, responder: mln_engine::storage::Responder) {
        self.0.lock().push(resource.url.clone());
        std::mem::forget(responder);
    }

    fn can_request(&self, _resource: &mln_engine::storage::Resource) -> bool {
        true
    }
}

#[test]
fn asset_files_stand_in_for_the_caches_local_resources() {
    use mln_engine::storage::{FileSourceExt, Resource};

    use crate::file_source::{AssetDir, ProxyFileSource};

    let assets = tmp_dir("assets");
    write(&assets.path().join("tiles/a.png"), b"a");
    let load = |assets: Option<AssetDir>, url: &str| {
        let urls = std::sync::Arc::default();
        let loader = std::sync::Arc::new(Recording(std::sync::Arc::clone(&urls)));
        let proxy = ProxyFileSource { default_resource_loader: loader, assets };
        let _request = proxy.request(&Resource::style(url), |_| {});
        let urls = urls.lock();
        urls.clone()
    };
    let assets = |only| Some(AssetDir { path: assets.path().to_path_buf(), only });
    assert_eq!(load(assets(false), "local://tiles/a.png"), ["asset://tiles/a.png"]);
    assert_eq!(load(assets(false), "http://localhost:2900/tiles/a.png"), ["asset://tiles/a.png"]);
    // What isn't there is the cache's.
    assert_eq!(load(assets(false), "local://tiles/b.png"), ["http://tiles/b.png"]);
    assert_eq!(load(assets(false), "http://example.com/tiles/a.png"), ["http://example.com/tiles/a.png"]);
    assert_eq!(load(None, "local://tiles/a.png"), ["http://tiles/a.png"]);
}

#[test]
fn a_suite_whose_assets_hold_a_directory_loads_it_only_from_them() {
    use mln_engine::storage::{FileSourceExt, Resource};

    use crate::file_source::{AssetDir, ProxyFileSource};

    let assets = tmp_dir("assets-only");
    write(&assets.path().join("tiles/a.png"), b"a");
    let load = |only: bool, url: &str| {
        let urls = std::sync::Arc::default();
        let loader = std::sync::Arc::new(Recording(std::sync::Arc::clone(&urls)));
        let assets = Some(AssetDir { path: assets.path().to_path_buf(), only });
        let proxy = ProxyFileSource { default_resource_loader: loader, assets };
        let _request = proxy.request(&Resource::style(url), |_| {});
        let urls = urls.lock();
        urls.clone()
    };
    // A file the assets lack is not found there, not the cache's.
    assert_eq!(load(true, "local://tiles/a.png"), ["asset://tiles/a.png"]);
    assert_eq!(load(true, "local://tiles/b.png"), ["asset://tiles/b.png"]);
    assert_eq!(load(true, "http://localhost:2900/tiles/b.png"), ["asset://tiles/b.png"]);
    // A directory they lack, and other hosts, are still the cache's.
    assert_eq!(load(true, "local://glyphs/a.pbf"), ["http://glyphs/a.pbf"]);
    assert_eq!(load(true, "http://example.com/tiles/b.png"), ["http://example.com/tiles/b.png"]);
    // A native suite's test falls through to the cache.
    assert_eq!(load(false, "local://tiles/b.png"), ["http://tiles/b.png"]);
}

#[test]
fn a_test_is_in_the_suite_of_the_assets_beside_its_directory() {
    use crate::runner::in_assets_suite;

    let assets = "../testdata/metrics/integration/gljs/assets";
    assert!(in_assets_suite("../testdata/metrics/integration/gljs/render-tests/image/a/style.json", assets));
    assert!(!in_assets_suite("../testdata/metrics/integration/render-tests/image/a/style.json", assets));
    assert!(!in_assets_suite("../testdata/metrics/integration/gljs2/render-tests/a/style.json", assets));
    // A bare directory name has no parent that would hold every style.
    assert!(!in_assets_suite("render-tests/a/style.json", "assets"));
}
