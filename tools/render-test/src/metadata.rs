// Ports render-test/metadata.hpp

use std::collections::BTreeMap;

use geometry::Point;
use mln_util::size::Size;

/// `mln::MapMode` (include/mln/map/mode.hpp). The runner keeps its own copy
/// so that it does not depend on the map crate; `TestRunnerImpl`
/// implementations convert.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[repr(u32)]
pub enum MapMode {
    /// continually updating map
    Continuous = 0,
    /// a once-off still image of an arbitrary viewport
    #[default]
    Static = 1,
    /// a once-off still image of a single tile
    Tile = 2,
}

/// `mln::MapDebugOptions` (include/mln/map/mode.hpp), a bit set.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct MapDebugOptions(pub u32);

impl MapDebugOptions {
    pub const NO_DEBUG: MapDebugOptions = MapDebugOptions(0);
    pub const TILE_BORDERS: MapDebugOptions = MapDebugOptions(1 << 1);
    pub const PARSE_STATUS: MapDebugOptions = MapDebugOptions(1 << 2);
    pub const TIMESTAMPS: MapDebugOptions = MapDebugOptions(1 << 3);
    pub const COLLISION: MapDebugOptions = MapDebugOptions(1 << 4);
    pub const OVERDRAW: MapDebugOptions = MapDebugOptions(1 << 5);
    pub const STENCIL_CLIP: MapDebugOptions = MapDebugOptions(1 << 6);
    pub const DEPTH_BUFFER: MapDebugOptions = MapDebugOptions(1 << 7);
}

impl std::ops::BitOrAssign for MapDebugOptions {
    fn bitor_assign(&mut self, rhs: Self) {
        self.0 |= rhs.0;
    }
}

/// `mln::ScreenBox` (`mapbox::geometry::box<double>`).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ScreenBox {
    pub min: Point<f64>,
    pub max: Point<f64>,
}

/// `mln::RenderedQueryOptions` (include/mln/renderer/query.hpp).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RenderedQueryOptions {
    pub layer_ids: Option<Vec<String>>,
    // TODO(port): `std::optional<style::Filter>`; converting it needs
    // mln_style's filter conversion. Kept as the raw JSON until then.
    pub filter: Option<serde_json::Value>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TestStatistics {
    pub ignore_failed_tests: u32,
    pub ignore_passed_tests: u32,
    pub errored_tests: u32,
    pub failed_tests: u32,
    pub passed_tests: u32,
}

/// Paths are strings: the runner derives test ids from them by offset.
/// Serialized for the workers of `--jobs` (jobs.rs).
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct TestPaths {
    pub style_path: String,
    pub expectations: Vec<String>,
    pub expected_metrics: Vec<String>,
}

impl TestPaths {
    #[must_use]
    pub fn new(style_path: String, expectations: Vec<String>, expected_metrics: Vec<String>) -> Self {
        TestPaths { style_path, expectations, expected_metrics }
    }

    #[must_use]
    pub fn default_expectations(&self) -> &str {
        assert_ne!(self.expectations, Vec::<String>::new());
        &self.expectations[0]
    }
}

/// `checkValue(float expected, float actual, float tolerance)`.
#[must_use]
pub fn check_value(expected: f32, actual: f32, tolerance: f32) -> (bool, f32) {
    let delta = expected * tolerance;
    debug_assert!(delta >= 0.0);
    ((expected - actual).abs() <= delta, delta)
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct FileSizeProbe {
    pub path: String,
    pub size: u64,
    pub tolerance: f32,
}

impl FileSizeProbe {
    #[must_use]
    pub fn new(path: String, size: u64, tolerance: f32) -> Self {
        FileSizeProbe { path, size, tolerance }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct MemoryProbe {
    pub peak: usize,
    pub allocations: usize,
    pub tolerance: f32,
}

impl MemoryProbe {
    #[must_use]
    pub fn new(peak: usize, allocations: usize) -> Self {
        MemoryProbe { peak, allocations, tolerance: 0.0 }
    }

    #[must_use]
    pub fn check_peak(expected: &MemoryProbe, actual: &MemoryProbe) -> (bool, f32) {
        check_value(expected.peak as f32, actual.peak as f32, actual.tolerance)
    }

    #[must_use]
    pub fn check_allocations(expected: &MemoryProbe, actual: &MemoryProbe) -> (bool, f32) {
        check_value(expected.allocations as f32, actual.allocations as f32, actual.tolerance)
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct FpsProbe {
    pub average: f32,
    pub min_one_pc: f32,
    pub tolerance: f32,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct NetworkProbe {
    pub requests: usize,
    pub transferred: usize,
}

impl NetworkProbe {
    #[must_use]
    pub fn new(requests: usize, transferred: usize) -> Self {
        NetworkProbe { requests, transferred }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct GfxProbeMemory {
    pub allocated: i32,
    pub peak: i32,
}

/// `GfxProbe`. TODO(port): `GfxProbe(const gfx::RenderingStats&, const GfxProbe&)`
/// needs `RenderResult::stats` from the headless frontend.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct GfxProbe {
    pub num_buffers: i32,
    pub num_draw_calls: i32,
    pub num_frame_buffers: i32,
    pub num_textures: i32,
    pub mem_index_buffers: GfxProbeMemory,
    pub mem_vertex_buffers: GfxProbeMemory,
    pub mem_textures: GfxProbeMemory,
}

/// `TestMetrics`; `std::map` → `BTreeMap` (same key order).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TestMetrics {
    pub file_size: BTreeMap<String, FileSizeProbe>,
    pub memory: BTreeMap<String, MemoryProbe>,
    pub network: BTreeMap<String, NetworkProbe>,
    pub fps: BTreeMap<String, FpsProbe>,
    pub gfx: BTreeMap<String, GfxProbe>,
}

impl TestMetrics {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.file_size.is_empty()
            && self.memory.is_empty()
            && self.network.is_empty()
            && self.fps.is_empty()
            && self.gfx.is_empty()
    }
}

/// `TestMetadata`.
#[derive(Clone, Debug)]
pub struct TestMetadata {
    pub paths: TestPaths,
    /// The parsed style.json (`mln::JSDocument`; `Null` when it did not parse).
    pub document: serde_json::Value,
    /// The style JSON handed to `Style::loadJSON`. C++ re-serializes
    /// `document`; the port passes the file text, which parses to the same
    /// values in the same member order (and keeps duplicate members, as
    /// rapidjson does). Re-serialized from `document` when the runner edits
    /// it (`source-replace`).
    pub document_json: String,
    pub render_test: bool,
    pub outputs_image: bool,
    pub ignored_test: bool,
    /// If unit test hasn't metric.json, the unit test will end with error
    /// message: "Failed to find expectations for...", to prevent unit test
    /// error by missing metric.json, can turn on 'ignoreProbing' to prevent
    /// the unit test fail, and just verify the render result.
    pub ignore_probing: bool,

    pub size: Size,
    pub max_pitch: Option<f32>,
    pub pixel_ratio: f32,
    /// diff
    pub allowed: f64,
    pub description: String,
    pub map_mode: MapMode,
    pub debug: MapDebugOptions,
    pub cross_source_collisions: bool,
    pub axonometric: bool,
    pub x_skew: f64,
    pub y_skew: f64,
    pub query_geometry: Point<f64>,
    pub query_geometry_box: ScreenBox,
    pub query_options: RenderedQueryOptions,

    // TODO
    pub fade_duration: u32,
    pub add_fake_canvas: bool,

    // HTML
    pub id: String,
    pub status: String,
    pub color: String,

    /// PNG bytes (C++ keeps binary data in `std::string`).
    pub actual: Vec<u8>,
    pub actual_json: String,
    /// PNG bytes.
    pub expected: Vec<u8>,
    /// PNG bytes for render tests; the HTML diff text for query tests.
    pub diff: Vec<u8>,

    pub metrics: TestMetrics,
    pub expected_metrics: TestMetrics,

    // Results
    pub metrics_errored: u32,
    pub metrics_failed: u32,
    pub render_errored: u32,
    pub render_failed: u32,
    pub label_cut_off_found: u32,
    pub duplications_count: u32,

    pub error_message: String,
    pub difference: f64,
}

impl Default for TestMetadata {
    fn default() -> Self {
        TestMetadata {
            paths: TestPaths::default(),
            document: serde_json::Value::Null,
            document_json: String::new(),
            render_test: true,
            outputs_image: true,
            ignored_test: false,
            ignore_probing: true,
            size: Size::new(512, 512),
            max_pitch: None,
            pixel_ratio: 1.0,
            allowed: 0.00015,
            description: String::new(),
            map_mode: MapMode::Static,
            debug: MapDebugOptions::NO_DEBUG,
            cross_source_collisions: true,
            axonometric: false,
            x_skew: 0.0,
            y_skew: 1.0,
            query_geometry: Point::new(0.0, 0.0),
            query_geometry_box: ScreenBox::default(),
            query_options: RenderedQueryOptions::default(),
            fade_duration: 0,
            add_fake_canvas: false,
            id: String::new(),
            status: String::new(),
            color: String::new(),
            actual: Vec::new(),
            actual_json: String::new(),
            expected: Vec::new(),
            diff: Vec::new(),
            metrics: TestMetrics::default(),
            expected_metrics: TestMetrics::default(),
            metrics_errored: 0,
            metrics_failed: 0,
            render_errored: 0,
            render_failed: 0,
            label_cut_off_found: 0,
            duplications_count: 0,
            error_message: String::new(),
            difference: 0.0,
        }
    }
}

impl TestMetadata {
    /// `metadata.document.ObjectEmpty()`; a document that did not parse is
    /// `Null` in the port (a zero-member value in rapidjson).
    #[must_use]
    pub fn document_object_empty(&self) -> bool {
        match &self.document {
            serde_json::Value::Object(map) => map.is_empty(),
            _ => true,
        }
    }

    /// `document["metadata"]["test"]`, if present.
    #[must_use]
    pub fn test_value(&self) -> Option<&serde_json::Value> {
        self.document.get("metadata")?.get("test")
    }
}
