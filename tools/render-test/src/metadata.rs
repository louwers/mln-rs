use std::collections::BTreeMap;

use geometry::Point;
use mln_util::size::Size;

/// The map's mode. The runner keeps its own copy of the map's enum so that
/// it does not depend on the map crate; `TestRunnerImpl` implementations
/// convert.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[repr(u32)]
pub(crate) enum MapMode {
    /// continually updating map
    Continuous = 0,
    /// a once-off still image of an arbitrary viewport
    #[default]
    Static = 1,
    /// a once-off still image of a single tile
    Tile = 2,
}

/// The map's debug overlays, a bit set.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub(crate) struct MapDebugOptions(pub(crate) u32);

impl MapDebugOptions {
    pub(crate) const NO_DEBUG: MapDebugOptions = MapDebugOptions(0);
    pub(crate) const TILE_BORDERS: MapDebugOptions = MapDebugOptions(1 << 1);
    pub(crate) const COLLISION: MapDebugOptions = MapDebugOptions(1 << 4);
    pub(crate) const OVERDRAW: MapDebugOptions = MapDebugOptions(1 << 5);
}

impl std::ops::BitOrAssign for MapDebugOptions {
    fn bitor_assign(&mut self, rhs: Self) {
        self.0 |= rhs.0;
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ScreenBox {
    pub(crate) min: Point<f64>,
    pub(crate) max: Point<f64>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct RenderedQueryOptions {
    pub(crate) layer_ids: Option<Vec<String>>,
    // The raw JSON; the map converts it to a filter.
    pub(crate) filter: Option<serde_json::Value>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TestStatistics {
    pub(crate) ignore_failed_tests: u32,
    pub ignore_passed_tests: u32,
    pub errored_tests: u32,
    pub(crate) failed_tests: u32,
    pub passed_tests: u32,
}

/// Paths are strings: the runner derives test ids from them by offset.
/// Serialized for the workers of `--jobs` (jobs.rs).
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) struct TestPaths {
    pub(crate) style_path: String,
    pub(crate) expectations: Vec<String>,
    pub(crate) expected_metrics: Vec<String>,
}

impl TestPaths {
    #[must_use]
    pub(crate) fn new(style_path: String, expectations: Vec<String>, expected_metrics: Vec<String>) -> Self {
        TestPaths { style_path, expectations, expected_metrics }
    }

    #[must_use]
    pub(crate) fn default_expectations(&self) -> &str {
        assert_ne!(self.expectations, Vec::<String>::new());
        &self.expectations[0]
    }
}

#[must_use]
pub(crate) fn check_value(expected: f32, actual: f32, tolerance: f32) -> (bool, f32) {
    let delta = expected * tolerance;
    debug_assert!(delta >= 0.0);
    ((expected - actual).abs() <= delta, delta)
}

#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct FileSizeProbe {
    pub(crate) path: String,
    pub(crate) size: u64,
    pub(crate) tolerance: f32,
}

impl FileSizeProbe {
    #[must_use]
    pub(crate) fn new(path: String, size: u64, tolerance: f32) -> Self {
        FileSizeProbe { path, size, tolerance }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct MemoryProbe {
    pub(crate) peak: usize,
    pub(crate) allocations: usize,
    pub(crate) tolerance: f32,
}

impl MemoryProbe {
    #[must_use]
    pub(crate) fn new(peak: usize, allocations: usize) -> Self {
        MemoryProbe { peak, allocations, tolerance: 0.0 }
    }

    #[must_use]
    pub(crate) fn check_peak(expected: &MemoryProbe, actual: &MemoryProbe) -> (bool, f32) {
        check_value(expected.peak as f32, actual.peak as f32, actual.tolerance)
    }

    #[must_use]
    pub(crate) fn check_allocations(expected: &MemoryProbe, actual: &MemoryProbe) -> (bool, f32) {
        check_value(expected.allocations as f32, actual.allocations as f32, actual.tolerance)
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct FpsProbe {
    pub(crate) average: f32,
    pub(crate) min_one_pc: f32,
    pub(crate) tolerance: f32,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct NetworkProbe {
    pub(crate) requests: usize,
    pub(crate) transferred: usize,
}

impl NetworkProbe {
    #[must_use]
    pub(crate) fn new(requests: usize, transferred: usize) -> Self {
        NetworkProbe { requests, transferred }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct GfxProbeMemory {
    pub(crate) allocated: i32,
    pub(crate) peak: i32,
}

/// Graphics resource counts. The runner never fills them, since
/// `probeGFX` is unsupported.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct GfxProbe {
    pub(crate) num_buffers: i32,
    pub(crate) num_draw_calls: i32,
    pub(crate) num_frame_buffers: i32,
    pub(crate) num_textures: i32,
    pub(crate) mem_index_buffers: GfxProbeMemory,
    pub(crate) mem_vertex_buffers: GfxProbeMemory,
    pub(crate) mem_textures: GfxProbeMemory,
}

/// A test's metrics, by probe kind and name. The maps are `BTreeMap`s to
/// keep the key order of C++'s `std::map`.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct TestMetrics {
    pub(crate) file_size: BTreeMap<String, FileSizeProbe>,
    pub(crate) memory: BTreeMap<String, MemoryProbe>,
    pub(crate) network: BTreeMap<String, NetworkProbe>,
    pub(crate) fps: BTreeMap<String, FpsProbe>,
    pub(crate) gfx: BTreeMap<String, GfxProbe>,
}

impl TestMetrics {
    #[must_use]
    pub(crate) fn is_empty(&self) -> bool {
        self.file_size.is_empty()
            && self.memory.is_empty()
            && self.network.is_empty()
            && self.fps.is_empty()
            && self.gfx.is_empty()
    }
}

#[derive(Clone, Debug)]
pub struct TestMetadata {
    pub(crate) paths: TestPaths,
    /// The parsed style.json (`mln::JSDocument`; `Null` when it did not parse).
    pub(crate) document: serde_json::Value,
    /// The style JSON handed to `Style::loadJSON`. C++ re-serializes
    /// `document`; the port passes the file text, which parses to the same
    /// values in the same member order (and keeps duplicate members, as
    /// rapidjson does). Re-serialized from `document` when the runner edits
    /// it (`source-replace`).
    pub(crate) document_json: String,
    pub(crate) render_test: bool,
    pub(crate) outputs_image: bool,
    pub(crate) ignored_test: bool,
    /// If unit test hasn't metric.json, the unit test will end with error
    /// message: "Failed to find expectations for...", to prevent unit test
    /// error by missing metric.json, can turn on 'ignoreProbing' to prevent
    /// the unit test fail, and just verify the render result.
    pub(crate) ignore_probing: bool,

    pub(crate) size: Size,
    pub(crate) max_pitch: Option<f32>,
    pub(crate) pixel_ratio: f32,
    /// diff
    pub(crate) allowed: f64,
    pub(crate) description: String,
    pub(crate) map_mode: MapMode,
    pub(crate) debug: MapDebugOptions,
    pub(crate) cross_source_collisions: bool,
    pub(crate) axonometric: bool,
    pub(crate) x_skew: f64,
    pub(crate) y_skew: f64,
    pub(crate) query_geometry: Point<f64>,
    pub(crate) query_geometry_box: ScreenBox,
    pub(crate) query_options: RenderedQueryOptions,

    // HTML
    pub id: String,
    pub status: String,
    pub(crate) color: String,

    /// PNG bytes (C++ keeps binary data in `std::string`).
    pub(crate) actual: Vec<u8>,
    pub(crate) actual_json: String,
    /// PNG bytes.
    pub(crate) expected: Vec<u8>,
    /// PNG bytes for render tests; the HTML diff text for query tests.
    pub(crate) diff: Vec<u8>,

    pub(crate) metrics: TestMetrics,
    pub(crate) expected_metrics: TestMetrics,

    // Results
    pub(crate) metrics_errored: u32,
    pub(crate) metrics_failed: u32,
    pub(crate) render_errored: u32,
    pub(crate) render_failed: u32,
    pub(crate) label_cut_off_found: u32,
    pub(crate) duplications_count: u32,

    pub error_message: String,
    pub(crate) difference: f64,
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
    /// Whether the document has no members. A document that did not parse is
    /// `Null` here (a zero-member value in rapidjson), and counts as empty.
    #[must_use]
    pub(crate) fn document_object_empty(&self) -> bool {
        match &self.document {
            serde_json::Value::Object(map) => map.is_empty(),
            _ => true,
        }
    }

    /// `document["metadata"]["test"]`, if present.
    #[must_use]
    pub(crate) fn test_value(&self) -> Option<&serde_json::Value> {
        self.document.get("metadata")?.get("test")
    }
}
