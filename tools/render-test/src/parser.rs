// Ports render-test/parser.{hpp,cpp}
//
// rapidjson is replaced by serde_json (with `preserve_order`, so objects
// keep document order). Differences that matter:
// - parse error messages are serde_json's, not
//   `formatJSONParseError` ("<rapidjson message> at offset N");
// - rapidjson keeps duplicate object members, serde_json the last one; the
//   ignore files are read member by member (`read_json_object_members`).

use std::fmt;
use std::sync::LazyLock;
use std::time::Duration;

use feature::Value;
use mln_util::geo::LatLng;
use mln_util::size::Size;
use regex::Regex;
use serde::de::{Deserialize, Deserializer, MapAccess, Visitor};

use crate::file_source::ProxyFileSource;
use crate::filesystem;
use crate::log::{self, Event};
use crate::metadata::{
    FileSizeProbe, GfxProbe, GfxProbeMemory, MapDebugOptions, MapMode, MemoryProbe, NetworkProbe, TestMetadata,
    TestMetrics, TestPaths, TestStatistics,
};
use crate::runner::{CameraOptions, MapError, TestContext, TestOperation, TestOperations};

pub type ErrorMessage = String;

static NULL: serde_json::Value = serde_json::Value::Null;

const RESULTS_STYLE: &str = r#"
<meta charset="UTF-8">
<style>
    body { font: 18px/1.2 -apple-system, BlinkMacSystemFont, "Helvetica Neue", Helvetica, Arial, sans-serif; padding: 10px; }
    h1 { font-size: 32px; margin-bottom: 0; }
    button { vertical-align: middle; }
    h2 { font-size: 24px; font-weight: normal; margin: 10px 0 10px; line-height: 1; }
    img { margin: 0 10px 10px 0; border: 1px dotted #ccc; }
    .stats { margin-top: 10px; }
    .test { border-bottom: 1px dotted #bbb; padding-bottom: 5px; }
    .tests { border-top: 1px dotted #bbb; margin-top: 10px; }
    .diff { color: #777; }
    .test p, .test pre { margin: 0 0 10px; }
    .test pre { font-size: 14px; }
    .label { color: white; font-size: 18px; padding: 2px 6px 3px; border-radius: 3px; margin-right: 3px; vertical-align: bottom; display: inline-block; }
    .hide { display: none; }
</style>
"#;

const RESULTS_SCRIPT: &str = r#"
<script>
document.addEventListener('mouseover', handleHover);
document.addEventListener('mouseout', handleHover);

function handleHover(e) {
    var el = e.target;
    if (el.tagName === 'IMG' && el.dataset.altSrc) {
        var tmp = el.src;
        el.src = el.dataset.altSrc;
        el.dataset.altSrc = tmp;
    }
}

document.getElementById('toggle-passed').addEventListener('click', function (e) {
    for (const row of document.querySelectorAll('.test.passed')) {
        row.classList.toggle('hide');
    }
});
document.getElementById('toggle-ignored').addEventListener('click', function (e) {
    for (const row of document.querySelectorAll('.test.ignored')) {
        row.classList.toggle('hide');
    }
});
document.getElementById('toggle-sequence').addEventListener('click', function (e) {
    document.getElementById('test-sequence').classList.toggle('hide');
});
</script>
"#;

const RESULTS_HEADER_BUTTONS: &str = r#"
    <button id='toggle-sequence'>Toggle test sequence</button>
    <button id='toggle-passed'>Toggle passed tests</button>
    <button id='toggle-ignored'>Toggle ignored tests</button>
</h1>
"#;

// query: `toJSON` with `indent` 2 and `singleLine` false, the only form the
// runner uses. rapidjson's PrettyWriter and serde_json's pretty printer lay
// out JSON the same way (two-space indent, `"key": value`, `[]`/`{}` when
// empty); number spelling may differ (the text is only parsed back, written
// to actual.json and shown in the report).

/// `mln::Value` as a JSON value (object members sorted by key, as C++
/// writes them).
fn value_to_json(value: &Value) -> serde_json::Value {
    match value {
        Value::Null => serde_json::Value::Null,
        Value::Bool(b) => serde_json::Value::Bool(*b),
        Value::Uint(u) => serde_json::Value::from(*u),
        Value::Int(i) => serde_json::Value::from(*i),
        Value::Double(d) => serde_json::Number::from_f64(*d).map_or(serde_json::Value::Null, serde_json::Value::Number),
        Value::String(s) => serde_json::Value::String(s.clone()),
        Value::Array(values) => serde_json::Value::Array(values.iter().map(value_to_json).collect()),
        Value::Object(map) => {
            serde_json::Value::Object(map.iter().map(|(k, v)| (k.clone(), value_to_json(v))).collect())
        }
    }
}

/// `toJSON(const mln::Value&, 2, false)`.
pub fn to_json_value(value: &Value) -> String {
    serde_json::to_string_pretty(&value_to_json(value)).expect("JSON value serializes")
}

/// `toJSON(const std::vector<mln::Feature>&, 2, false)`.
pub fn to_json_features(features: &[mln_util::feature::Feature]) -> String {
    let mut array = Vec::with_capacity(features.len());
    for feature in features {
        // type, id, geometry, properties (mapbox::geojson::convert), then
        // source, sourceLayer (when not empty) and state.
        let serde_json::Value::Object(mut result) = geojson::to_json_feature(&feature.to_geojson_feature()) else {
            unreachable!("a feature converts to an object");
        };
        result.insert("source".to_owned(), serde_json::Value::String(feature.source.clone()));
        if !feature.source_layer.is_empty() {
            result.insert("sourceLayer".to_owned(), serde_json::Value::String(feature.source_layer.clone()));
        }
        result.insert("state".to_owned(), value_to_json(&Value::Object(feature.state.clone())));
        array.push(serde_json::Value::Object(result));
    }
    serde_json::to_string_pretty(&serde_json::Value::Array(array)).expect("JSON value serializes")
}

/// `readJson(path)`.
pub fn read_json(json_path: &str) -> Result<serde_json::Value, ErrorMessage> {
    let bytes = std::fs::read(json_path).map_err(|_| format!("Unable to open file {json_path}"))?;
    serde_json::from_slice(&bytes).map_err(|e| e.to_string())
}

/// `readJson(path)` keeping the text, for documents the runner hands on
/// unchanged (style.json).
pub fn read_json_with_text(json_path: &str) -> Result<(serde_json::Value, String), ErrorMessage> {
    let bytes = std::fs::read(json_path).map_err(|_| format!("Unable to open file {json_path}"))?;
    let value = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
    let text = String::from_utf8(bytes).map_err(|e| e.to_string())?;
    Ok((value, text))
}

/// The members of a JSON object in document order, duplicates kept
/// (rapidjson's `GetObject()` iteration).
struct ObjectMembers(Vec<(String, serde_json::Value)>);

impl<'de> Deserialize<'de> for ObjectMembers {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct MembersVisitor;
        impl<'de> Visitor<'de> for MembersVisitor {
            type Value = ObjectMembers;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a JSON object")
            }
            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<ObjectMembers, A::Error> {
                let mut members = Vec::new();
                while let Some(entry) = map.next_entry::<String, serde_json::Value>()? {
                    members.push(entry);
                }
                Ok(ObjectMembers(members))
            }
        }
        deserializer.deserialize_map(MembersVisitor)
    }
}

/// `readJson(path)` for a document whose top level is an object, as its
/// members in document order.
pub fn read_json_object_members(json_path: &str) -> Result<Vec<(String, serde_json::Value)>, ErrorMessage> {
    let bytes = std::fs::read(json_path).map_err(|_| format!("Unable to open file {json_path}"))?;
    serde_json::from_slice::<ObjectMembers>(&bytes).map(|members| members.0).map_err(|e| e.to_string())
}

/// `serializeJsonValue(value)`: compact JSON (rapidjson `Writer`).
pub fn serialize_json_value(value: &serde_json::Value) -> String {
    serde_json::to_string(value).expect("serde_json::Value serializes")
}

// TODO(port): serializeMetrics (rapidjson PrettyWriter output); only the
// update/rebaseline modes write metrics, and the port does not write
// baselines.

fn read_expected_entries(regex: &Regex, base: &str) -> Vec<String> {
    let mut expected_entries = Vec::new();
    // C++ throws from directory_iterator on an unreadable directory; callers
    // only pass existing directories.
    let Ok(entries) = filesystem::directory_entries(base) else {
        return expected_entries;
    };
    for path in entries {
        if filesystem::is_regular_file(&path) && regex.is_match(&path) {
            expected_entries.push(path);
        }
    }
    expected_entries
}

// `std::regex_match` (whole string) with ECMAScript syntax; the patterns are
// the same in the Rust regex dialect.
static EXPECTED_IMAGE_REGEX: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^(?:.*[/\\]expected.*.png)$").unwrap());
static EXPECTED_METRIC_REGEX: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^(?:.*[/\\]metrics.json)$").unwrap());
static EXPECTED_JSON_REGEX: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^(?:.*[/\\]expected.*.json)$").unwrap());

pub fn read_expected_image_entries(base: &str) -> Vec<String> {
    read_expected_entries(&EXPECTED_IMAGE_REGEX, base)
}

pub fn read_expected_metric_entries(base: &str) -> Vec<String> {
    read_expected_entries(&EXPECTED_METRIC_REGEX, base)
}

pub fn read_expected_json_entries(base: &str) -> Vec<String> {
    read_expected_entries(&EXPECTED_JSON_REGEX, base)
}

// rapidjson getters on values the C++ code `assert`s the type of. In a
// release build a wrong type reads garbage; the port reads 0 / "" instead.
fn get_u64(v: &serde_json::Value) -> u64 {
    v.as_u64().or_else(|| v.as_f64().map(|d| d as u64)).unwrap_or(0)
}
fn get_i32(v: &serde_json::Value) -> i32 {
    v.as_i64().or_else(|| v.as_f64().map(|d| d as i64)).unwrap_or(0) as i32
}
fn get_f64(v: &serde_json::Value) -> f64 {
    v.as_f64().unwrap_or(0.0)
}
fn get_f32(v: &serde_json::Value) -> f32 {
    get_f64(v) as f32
}
fn get_str(v: &serde_json::Value) -> &str {
    v.as_str().unwrap_or("")
}
fn get_bool(v: &serde_json::Value) -> bool {
    v.as_bool().unwrap_or(false)
}
fn index(v: &serde_json::Value, i: usize) -> &serde_json::Value {
    v.get(i).unwrap_or(&NULL)
}
fn member<'a>(v: &'a serde_json::Value, key: &str) -> &'a serde_json::Value {
    v.get(key).unwrap_or(&NULL)
}
fn array_size(v: &serde_json::Value) -> usize {
    v.as_array().map_or(0, Vec::len)
}

/// `readExpectedMetrics(path)`.
pub fn read_expected_metrics(path: &str) -> TestMetrics {
    let mut result = TestMetrics::default();

    let Ok(document) = read_json(path) else {
        return result;
    };

    if let Some(file_size_value) = document.get("file-size") {
        for probe_value in file_size_value.as_array().into_iter().flatten() {
            let mark = get_str(index(probe_value, 0)).to_string();
            let file_path = get_str(index(probe_value, 1)).to_string();
            result.file_size.entry(mark).or_insert(FileSizeProbe::new(file_path, get_u64(index(probe_value, 2)), 0.0));
        }
    }

    if let Some(memory_value) = document.get("memory") {
        for probe_value in memory_value.as_array().into_iter().flatten() {
            let mark = get_str(index(probe_value, 0)).to_string();
            result.memory.entry(mark).or_insert(MemoryProbe::new(
                get_u64(index(probe_value, 1)) as usize,
                get_u64(index(probe_value, 2)) as usize,
            ));
        }
    }

    if let Some(network_value) = document.get("network") {
        for probe_value in network_value.as_array().into_iter().flatten() {
            let mark = get_str(index(probe_value, 0)).to_string();
            result.network.entry(mark).or_insert(NetworkProbe::new(
                get_u64(index(probe_value, 1)) as usize,
                get_u64(index(probe_value, 2)) as usize,
            ));
        }
    }

    if let Some(fps_value) = document.get("fps") {
        for probe_value in fps_value.as_array().into_iter().flatten() {
            let mark = get_str(index(probe_value, 0)).to_string();
            result.fps.entry(mark).or_insert(crate::metadata::FpsProbe {
                average: get_f32(index(probe_value, 1)),
                min_one_pc: get_f32(index(probe_value, 2)),
                tolerance: get_f32(index(probe_value, 3)),
            });
        }
    }

    if let Some(gfx_value) = document.get("gfx") {
        for probe_value in gfx_value.as_array().into_iter().flatten() {
            let mark = get_str(index(probe_value, 0)).to_string();
            let memory = |i: usize| GfxProbeMemory {
                allocated: get_i32(index(index(probe_value, i), 0)),
                peak: get_i32(index(index(probe_value, i), 1)),
            };
            let probe = GfxProbe {
                num_draw_calls: get_i32(index(probe_value, 1)),
                num_textures: get_i32(index(probe_value, 2)),
                num_buffers: get_i32(index(probe_value, 3)),
                num_frame_buffers: get_i32(index(probe_value, 4)),
                mem_textures: memory(5),
                mem_index_buffers: memory(6),
                mem_vertex_buffers: memory(7),
            };
            result.gfx.entry(mark).or_insert(probe);
        }
    }

    result
}

/// `parseTestMetadata(paths)`.
pub fn parse_test_metadata(paths: &TestPaths) -> TestMetadata {
    let mut metadata = TestMetadata { paths: paths.clone(), ..TestMetadata::default() };

    let (document, text) = match read_json_with_text(&paths.style_path) {
        Ok(parsed) => parsed,
        Err(_) => {
            metadata.error_message = format!("Unable to parse: {}", metadata.paths.style_path);
            return metadata;
        }
    };
    metadata.document = document;
    metadata.document_json = text;

    let Some(metadata_value) = metadata.document.get("metadata") else {
        log::warning(Event::ParseStyle, &format!("Style has no 'metadata': {}", paths.style_path));
        return metadata;
    };

    let Some(test_value) = metadata_value.get("test").cloned() else {
        log::warning(Event::ParseStyle, &format!("Style has no 'metadata.test': {}", paths.style_path));
        return metadata;
    };
    let test_value = &test_value;
    let has = |key: &str| test_value.get(key).is_some();

    if has("mapMode") {
        metadata.outputs_image = true;
        let map_mode_str = get_str(member(test_value, "mapMode"));
        if map_mode_str == "tile" {
            metadata.map_mode = MapMode::Tile;
            // In the tile mode, map is showing exactly one tile.
            let tile_size = mln_util::constants::TILE_SIZE as u32;
            metadata.size = Size::new(tile_size, tile_size);
        } else if map_mode_str == "continuous" {
            metadata.map_mode = MapMode::Continuous;
            metadata.outputs_image = false;
        } else if map_mode_str == "static" {
            metadata.map_mode = MapMode::Static;
        } else {
            log::warning(Event::ParseStyle, &format!("Unknown map mode: {map_mode_str}. Falling back to static mode"));
            metadata.map_mode = MapMode::Static;
        }
    }

    if has("width") {
        if metadata.map_mode == MapMode::Tile {
            log::warning(Event::ParseStyle, "The 'width' metadata field is ignored in tile map mode");
        } else {
            // `GetInt()` assigned to `uint32_t`.
            metadata.size.width = get_i32(member(test_value, "width")) as u32;
        }
    }

    if has("height") {
        if metadata.map_mode == MapMode::Tile {
            log::warning(Event::ParseStyle, "The 'height' metadata field is ignored in tile map mode");
        } else {
            metadata.size.height = get_i32(member(test_value, "height")) as u32;
        }
    }

    if has("maxPitch") {
        metadata.max_pitch = Some(get_f32(member(test_value, "maxPitch")));
    }

    if has("pixelRatio") {
        metadata.pixel_ratio = get_f32(member(test_value, "pixelRatio"));
    }

    if has("allowed") {
        metadata.allowed = get_f64(member(test_value, "allowed"));
    }

    if has("description") {
        metadata.description = get_str(member(test_value, "description")).to_string();
    }

    // Test operations handled in runner.cpp.

    if has("debug") {
        metadata.debug |= MapDebugOptions::TILE_BORDERS;
    }

    if has("collisionDebug") {
        metadata.debug |= MapDebugOptions::COLLISION;
    }

    if has("showOverdrawInspector") {
        metadata.debug |= MapDebugOptions::OVERDRAW;
    }

    if has("crossSourceCollisions") {
        metadata.cross_source_collisions = get_bool(member(test_value, "crossSourceCollisions"));
    }

    if has("axonometric") {
        metadata.axonometric = get_bool(member(test_value, "axonometric"));
    }

    if has("skew") {
        let skew = member(test_value, "skew");
        metadata.x_skew = get_f64(index(skew, 0));
        metadata.y_skew = get_f64(index(skew, 1));
    }

    if has("queryGeometry") {
        let query_geometry = member(test_value, "queryGeometry");
        let (first, second) = (index(query_geometry, 0), index(query_geometry, 1));
        if first.is_number() && second.is_number() {
            metadata.query_geometry.x = get_f64(first);
            metadata.query_geometry.y = get_f64(second);
        } else if first.is_array() && second.is_array() {
            metadata.query_geometry_box.min.x = get_f64(index(first, 0));
            metadata.query_geometry_box.min.y = get_f64(index(first, 1));
            metadata.query_geometry_box.max.x = get_f64(index(second, 0));
            metadata.query_geometry_box.max.y = get_f64(index(second, 1));
        }
        metadata.render_test = false;
    }

    if has("ignoreProbing") {
        if let Some(ignore_probing) = member(test_value, "ignoreProbing").as_bool() {
            metadata.ignore_probing = ignore_probing;
        }
    }

    if has("queryOptions") {
        let query_options = member(test_value, "queryOptions");

        if let Some(layers) = query_options.get("layers") {
            let layers_vec = layers.as_array().into_iter().flatten().map(|layer| get_str(layer).to_string()).collect();
            metadata.query_options.layer_ids = Some(layers_vec);
        }

        if let Some(filter) = query_options.get("filter") {
            // TODO(port): `convert<Filter>(filterVal, error)` (mln_style).
            metadata.query_options.filter = Some(filter.clone());
        }
    }

    // TODO: fadeDuration
    // TODO: addFakeCanvas

    metadata
}

/// `mln::TileLodMode` (include/mln/map/mode.hpp) with the names the runner's
/// `MBGL_DEFINE_ENUM` gives it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TileLodMode {
    Default,
    Distance,
}

impl TileLodMode {
    pub fn to_enum(s: &str) -> Option<TileLodMode> {
        match s {
            "default" => Some(TileLodMode::Default),
            "distance" => Some(TileLodMode::Distance),
            _ => None,
        }
    }
}

pub mod test_operation_names {
    pub const WAIT_OP: &str = "wait";
    pub const SLEEP_OP: &str = "sleep";
    pub const ADD_IMAGE_OP: &str = "addImage";
    pub const UPDATE_IMAGE_OP: &str = "updateImage";
    pub const REMOVE_IMAGE_OP: &str = "removeImage";
    pub const SET_STYLE_OP: &str = "setStyle";
    pub const SET_CENTER_OP: &str = "setCenter";
    pub const SET_ZOOM_OP: &str = "setZoom";
    pub const SET_BEARING_OP: &str = "setBearing";
    pub const SET_PITCH_OP: &str = "setPitch";
    pub const SET_ROLL_OP: &str = "setRoll";
    pub const SET_CENTER_ALTITUDE_OP: &str = "setCenterElevation";
    pub const SET_FILTER_OP: &str = "setFilter";
    pub const SET_LAYER_ZOOM_RANGE_OP: &str = "setLayerZoomRange";
    pub const SET_LIGHT_OP: &str = "setLight";
    pub const ADD_LAYER_OP: &str = "addLayer";
    pub const REMOVE_LAYER_OP: &str = "removeLayer";
    pub const ADD_SOURCE_OP: &str = "addSource";
    pub const REMOVE_SOURCE_OP: &str = "removeSource";
    pub const SET_PAINT_PROPERTY_OP: &str = "setPaintProperty";
    pub const SET_LAYOUT_PROPERTY_OP: &str = "setLayoutProperty";
    pub const SET_GLOBAL_STATE_PROPERTY_OP: &str = "setGlobalStateProperty";
    pub const FILE_SIZE_PROBE_OP: &str = "probeFileSize";
    pub const MEMORY_PROBE_OP: &str = "probeMemory";
    pub const MEMORY_PROBE_START_OP: &str = "probeMemoryStart";
    pub const MEMORY_PROBE_END_OP: &str = "probeMemoryEnd";
    pub const NETWORK_PROBE_OP: &str = "probeNetwork";
    pub const NETWORK_PROBE_START_OP: &str = "probeNetworkStart";
    pub const NETWORK_PROBE_END_OP: &str = "probeNetworkEnd";
    pub const SET_FEATURE_STATE_OP: &str = "setFeatureState";
    pub const GET_FEATURE_STATE_OP: &str = "getFeatureState";
    pub const REMOVE_FEATURE_STATE_OP: &str = "removeFeatureState";
    pub const PAN_GESTURE_OP: &str = "panGesture";
    pub const GFX_PROBE_OP: &str = "probeGFX";
    pub const GFX_PROBE_START_OP: &str = "probeGFXStart";
    pub const GFX_PROBE_END_OP: &str = "probeGFXEnd";
    pub const SET_TILE_LOD_MIN_RADIUS_OP: &str = "setTileLodMinRadius";
    pub const SET_TILE_LOD_SCALE_OP: &str = "setTileLodScale";
    pub const SET_TILE_LOD_PITCH_THRESHOLD_OP: &str = "setTileLodPitchThreshold";
    pub const SET_TILE_LOD_ZOOM_SHIFT_OP: &str = "setTileLodZoomShift";
    pub const SET_TILE_LOD_MODE_OP: &str = "setTileLodMode";
}

use test_operation_names::*;

/// `parseGlobalStateValue(value)`.
fn parse_global_state_value(value: &serde_json::Value) -> Value {
    match value {
        serde_json::Value::Null => Value::Null,
        serde_json::Value::Bool(b) => Value::Bool(*b),
        serde_json::Value::String(s) => Value::String(s.clone()),
        serde_json::Value::Number(n) => {
            if let Some(u) = n.as_u64() {
                Value::Uint(u)
            } else if let Some(i) = n.as_i64() {
                Value::Int(i)
            } else {
                Value::Double(n.as_f64().unwrap_or(0.0))
            }
        }
        serde_json::Value::Array(elements) => Value::Array(elements.iter().map(parse_global_state_value).collect()),
        serde_json::Value::Object(members) => {
            Value::Object(members.iter().map(|(k, v)| (k.clone(), parse_global_state_value(v))).collect())
        }
    }
}

/// `ConversionTraits<const JSValue*>::toValue`: null and false are
/// `false`, arrays and objects have no value.
fn json_to_value(value: &serde_json::Value) -> Option<Value> {
    match value {
        serde_json::Value::Null | serde_json::Value::Bool(false) => Some(Value::Bool(false)),
        serde_json::Value::Bool(true) => Some(Value::Bool(true)),
        serde_json::Value::String(s) => Some(Value::String(s.clone())),
        serde_json::Value::Number(n) => Some(if let Some(u) = n.as_u64() {
            Value::Uint(u)
        } else if let Some(i) = n.as_i64() {
            Value::Int(i)
        } else {
            Value::Double(n.as_f64().unwrap_or(0.0))
        }),
        _ => None,
    }
}

/// The state conversion of the setFeatureState operation (parser.cpp
/// `convertFn` over `eachMember(state, ...)`), with its quirks: an array
/// value becomes `{key: [...]}` but is not stored, an object recurses and
/// stores the last nested value under the outer key too.
fn parse_feature_state(state: &serde_json::Value, metadata: &mut TestMetadata) -> feature::PropertyMap {
    struct Parse {
        state_value: Value,
        value_parsed: bool,
        parsed_state: feature::PropertyMap,
    }
    fn convert(p: &mut Parse, k: &str, v: &serde_json::Value, metadata: &mut TestMetadata) {
        if let Some(value) = json_to_value(v) {
            p.state_value = value;
            p.value_parsed = true;
        } else if let serde_json::Value::Array(elements) = v {
            let array: Vec<Value> = elements.iter().filter_map(json_to_value).collect();
            let mut values = feature::ValueObject::new();
            values.insert(k.to_string(), Value::Array(array));
            p.state_value = Value::Object(values);
            p.value_parsed = true;
            return;
        } else if let serde_json::Value::Object(members) = v {
            for (mk, mv) in members {
                convert(p, mk, mv, metadata);
            }
        }

        if !p.value_parsed {
            metadata.error_message = format!("Could not get feature state value, state key: {k}");
            return;
        }
        p.parsed_state.insert(k.to_string(), p.state_value.clone());
    }
    let mut p = Parse { state_value: Value::Null, value_parsed: false, parsed_state: feature::PropertyMap::new() };
    if let serde_json::Value::Object(members) = state {
        for (k, v) in members {
            convert(&mut p, k, v, metadata);
        }
    }
    p.parsed_state
}

/// An operation the port cannot execute yet: the test errors (see
/// `TestContext::fail_not_ported`).
fn not_ported(what: String) -> TestOperation {
    Box::new(move |ctx: &mut TestContext<'_>| ctx.fail_not_ported(&what))
}

/// Handles a `TestRunnerImpl` call made by an operation that, in C++,
/// cannot fail (it returns `true`).
fn infallible(ctx: &mut TestContext<'_>, result: Result<(), MapError>) -> bool {
    match result {
        Ok(()) => true,
        Err(MapError::NotPorted(what)) => ctx.fail_not_ported(&what),
        Err(MapError::Exception(what)) => ctx.fail_exception(&what),
        Err(_) => true,
    }
}

/// `parseTestOperations(metadata)`.
pub fn parse_test_operations(metadata: &mut TestMetadata) -> TestOperations {
    let mut result: TestOperations = Vec::new();
    let Some(operations) = metadata.test_value().and_then(|t| t.get("operations")).cloned() else {
        return result;
    };

    let operations_array = operations.as_array().cloned().unwrap_or_default();
    if operations_array.is_empty() {
        return result;
    }
    for operation_array in &operations_array {
        let arg = |i: usize| index(operation_array, i);
        let size = array_size(operation_array);
        let name = get_str(arg(0));

        if name == WAIT_OP {
            // wait
            result.push(Box::new(|ctx: &mut TestContext<'_>| ctx.get_map().render().is_ok()));
        } else if name == SLEEP_OP {
            // sleep
            let mut duration = Duration::from_secs(3);
            if size >= 2 {
                duration = Duration::from_millis(u64::from(get_u64(arg(1)) as u32));
            }
            result.push(Box::new(move |ctx: &mut TestContext<'_>| {
                // C++ runs the run loop until a util::Timer fires.
                let result = ctx.get_map().sleep(duration);
                infallible(ctx, result)
            }));
        } else if name == ADD_IMAGE_OP || name == UPDATE_IMAGE_OP {
            // addImage | updateImage
            let mut pixel_ratio = 1.0f32;
            let mut sdf = false;

            if size == 4 {
                let image_options = arg(3);
                if let Some(value) = image_options.get("pixelRatio") {
                    pixel_ratio = get_f32(value);
                }
                if let Some(value) = image_options.get("sdf") {
                    sdf = get_bool(value);
                }
            }

            let image_name: String = get_str(arg(1)).chars().filter(|&c| c != '"').collect();
            let image_path = get_str(arg(2)).to_string();

            result.push(Box::new(move |ctx: &mut TestContext<'_>| {
                // C++ requests "mapbox://render-tests/" + imagePath through
                // the runner's file source, decodes it and calls
                // Style::addImage.
                match ctx.get_map().add_image(&image_name, &image_path, pixel_ratio, sdf) {
                    Ok(()) => true,
                    Err(MapError::NotPorted(what)) => ctx.fail_not_ported(&what),
                    Err(MapError::Exception(what)) => ctx.fail_exception(&what),
                    Err(_) => {
                        ctx.get_metadata().error_message += &format!("Failed to load expected image {image_path}");
                        false
                    }
                }
            }));
        } else if name == REMOVE_IMAGE_OP {
            // removeImage
            let image_name = get_str(arg(1)).to_string();
            result.push(Box::new(move |ctx: &mut TestContext<'_>| {
                let result = ctx.get_map().remove_image(&image_name);
                infallible(ctx, result)
            }));
        } else if name == SET_STYLE_OP {
            // setStyle
            if let Some(url) = arg(1).as_str() {
                let url = url.to_string();
                result.push(Box::new(move |ctx: &mut TestContext<'_>| {
                    let result = ctx.get_map().load_style_url(&url);
                    infallible(ctx, result)
                }));
            } else {
                let json = serialize_json_value(arg(1));
                result.push(Box::new(move |ctx: &mut TestContext<'_>| {
                    let result = ctx.get_map().load_style_json(&json);
                    infallible(ctx, result)
                }));
            }
        } else if name == SET_CENTER_OP {
            // setCenter
            let center_array = arg(1);
            // C++ `LatLng` throws std::domain_error on an invalid latitude or
            // longitude, which the runner does not catch.
            match LatLng::new(get_f64(index(center_array, 1)), get_f64(index(center_array, 0))) {
                Ok(center) => {
                    result.push(camera_operation(CameraOptions { center: Some(center), ..CameraOptions::default() }))
                }
                Err(e) => {
                    metadata.error_message = format!("Invalid setCenter operation: {e}");
                    return Vec::new();
                }
            }
        } else if name == SET_ZOOM_OP {
            // setZoom
            let zoom = get_f64(arg(1));
            result.push(camera_operation(CameraOptions { zoom: Some(zoom), ..Default::default() }));
        } else if name == SET_BEARING_OP {
            // setBearing
            let bearing = get_f64(arg(1));
            result.push(camera_operation(CameraOptions { bearing: Some(bearing), ..Default::default() }));
        } else if name == SET_PITCH_OP {
            // setPitch
            let pitch = get_f64(arg(1));
            result.push(camera_operation(CameraOptions { pitch: Some(pitch), ..Default::default() }));
        } else if name == SET_ROLL_OP {
            // setRoll
            let roll = get_f64(arg(1));
            result.push(camera_operation(CameraOptions { roll: Some(roll), ..Default::default() }));
        } else if name == SET_CENTER_ALTITUDE_OP {
            // setCenterAltitude
            let center_altitude = get_f64(arg(1));
            result
                .push(camera_operation(CameraOptions { center_altitude: Some(center_altitude), ..Default::default() }));
        } else if name == SET_FILTER_OP {
            // setFilter
            let layer_name = get_str(arg(1)).to_string();
            // C++ converts the filter here and reports a conversion error
            // when the operation runs; the conversion lives in mln_style, so
            // the TestRunnerImpl converts (conversion error first, then
            // "Layer not found").
            let filter = arg(2).clone();
            result.push(Box::new(move |ctx: &mut TestContext<'_>| {
                match ctx.get_map().set_layer_filter(&layer_name, &filter) {
                    Ok(()) => true,
                    Err(MapError::Conversion(message)) => {
                        ctx.get_metadata().error_message = format!("Unable to convert filter: {message}");
                        false
                    }
                    Err(MapError::LayerNotFound) => {
                        ctx.get_metadata().error_message = format!("Layer not found: {layer_name}");
                        false
                    }
                    Err(MapError::NotPorted(what)) => ctx.fail_not_ported(&what),
                    Err(MapError::Exception(what)) => ctx.fail_exception(&what),
                    Err(MapError::LoadFailed) => false,
                }
            }));
        } else if name == SET_LAYER_ZOOM_RANGE_OP {
            // setLayerZoomRange
            let layer_name = get_str(arg(1)).to_string();
            let min_zoom = get_f32(arg(2));
            let max_zoom = get_f32(arg(3));
            result.push(Box::new(move |ctx: &mut TestContext<'_>| {
                match ctx.get_map().set_layer_zoom_range(&layer_name, min_zoom, max_zoom) {
                    Ok(()) => true,
                    Err(MapError::LayerNotFound) => {
                        ctx.get_metadata().error_message = format!("Layer not found: {layer_name}");
                        false
                    }
                    Err(MapError::NotPorted(what)) => ctx.fail_not_ported(&what),
                    Err(MapError::Exception(what)) => ctx.fail_exception(&what),
                    Err(_) => true,
                }
            }));
        } else if name == SET_LIGHT_OP {
            // setLight (conformance). C++ converts the light while parsing
            // and, on error, sets "Unable to convert light: ..." and returns
            // no operations at all; the conversion needs mln_style, so here it
            // happens when the operation runs, with the same message.
            let json = serialize_json_value(arg(1));
            result.push(Box::new(move |ctx: &mut TestContext<'_>| match ctx.get_map().set_light(&json) {
                Ok(()) => true,
                Err(MapError::Conversion(message)) => {
                    ctx.get_metadata().error_message = format!("Unable to convert light: {message}");
                    false
                }
                Err(MapError::NotPorted(what)) => ctx.fail_not_ported(&what),
                Err(MapError::Exception(what)) => ctx.fail_exception(&what),
                Err(_) => true,
            }));
        } else if name == ADD_LAYER_OP {
            // addLayer
            let json = serialize_json_value(arg(1));
            result.push(Box::new(move |ctx: &mut TestContext<'_>| match ctx.get_map().add_layer(&json) {
                Ok(()) => true,
                Err(MapError::Conversion(message)) => {
                    ctx.get_metadata().error_message = format!("Unable to convert layer: {message}");
                    false
                }
                Err(MapError::NotPorted(what)) => ctx.fail_not_ported(&what),
                Err(MapError::Exception(what)) => ctx.fail_exception(&what),
                Err(_) => true,
            }));
        } else if name == REMOVE_LAYER_OP {
            // removeLayer
            let layer_name = get_str(arg(1)).to_string();
            result.push(Box::new(move |ctx: &mut TestContext<'_>| match ctx.get_map().remove_layer(&layer_name) {
                Ok(removed) => removed,
                Err(MapError::NotPorted(what)) => ctx.fail_not_ported(&what),
                Err(MapError::Exception(what)) => ctx.fail_exception(&what),
                Err(_) => false,
            }));
        } else if name == ADD_SOURCE_OP {
            // addSource
            let source_name = get_str(arg(1)).to_string();
            let json = serialize_json_value(arg(2));
            result.push(Box::new(move |ctx: &mut TestContext<'_>| {
                match ctx.get_map().add_source(&source_name, &json) {
                    Ok(()) => true,
                    Err(MapError::Conversion(message)) => {
                        ctx.get_metadata().error_message = format!("Unable to convert source: {message}");
                        false
                    }
                    Err(MapError::NotPorted(what)) => ctx.fail_not_ported(&what),
                    Err(MapError::Exception(what)) => ctx.fail_exception(&what),
                    Err(_) => true,
                }
            }));
        } else if name == REMOVE_SOURCE_OP {
            // removeSource
            let source_name = get_str(arg(1)).to_string();
            result.push(Box::new(move |ctx: &mut TestContext<'_>| match ctx.get_map().remove_source(&source_name) {
                Ok(removed) => removed,
                Err(MapError::NotPorted(what)) => ctx.fail_not_ported(&what),
                Err(MapError::Exception(what)) => ctx.fail_exception(&what),
                Err(_) => false,
            }));
        } else if name == SET_LAYOUT_PROPERTY_OP || name == SET_PAINT_PROPERTY_OP {
            // set{Paint|Layout}Property
            let layer_name = get_str(arg(1)).to_string();
            let property_name = get_str(arg(2)).to_string();
            // C++ serializes the value here and re-parses it when the
            // operation runs; a clone is the same value.
            let value = arg(3).clone();
            result.push(Box::new(move |ctx: &mut TestContext<'_>| {
                match ctx.get_map().set_layer_property(&layer_name, &property_name, &value) {
                    // `Layer::setProperty` returns an optional<Error> the
                    // runner ignores.
                    Ok(()) | Err(MapError::Conversion(_)) => true,
                    Err(MapError::LayerNotFound) => {
                        ctx.get_metadata().error_message = format!("Layer not found: {layer_name}");
                        false
                    }
                    Err(MapError::NotPorted(what)) => ctx.fail_not_ported(&what),
                    Err(MapError::Exception(what)) => ctx.fail_exception(&what),
                    Err(MapError::LoadFailed) => true,
                }
            }));
        } else if name == SET_GLOBAL_STATE_PROPERTY_OP {
            // setGlobalStateProperty
            let property_name = get_str(arg(1)).to_string();
            let value = parse_global_state_value(arg(2));
            result.push(Box::new(move |ctx: &mut TestContext<'_>| {
                let result = ctx.get_map().set_global_state_property(&property_name, &value);
                infallible(ctx, result)
            }));
        } else if name == FILE_SIZE_PROBE_OP {
            // probeFileSize
            let mark = get_str(arg(1)).to_string();
            let path = get_str(arg(2)).to_string();
            let tolerance = get_f64(arg(3)) as f32;

            let compressed = size == 5;

            let mut file_path = path.clone();
            if filesystem::is_relative(&file_path) {
                file_path = filesystem::join(metadata.paths.default_expectations(), &file_path);
            }
            result.push(Box::new(move |ctx: &mut TestContext<'_>| {
                if !filesystem::exists(&file_path) {
                    ctx.get_metadata().error_message = format!("File not found: {path}");
                    return false;
                }
                let size = if compressed {
                    let raw = std::fs::read(&file_path).unwrap_or_default();
                    mln_util::compression::compress(&raw).map(|c| c.len() as u64).unwrap_or(0)
                } else {
                    std::fs::metadata(&file_path).map(|m| m.len()).unwrap_or(0)
                };

                ctx.get_metadata().metrics.file_size.entry(mark.clone()).or_insert(FileSizeProbe::new(
                    path.clone(),
                    size,
                    tolerance,
                ));
                true
            }));
        } else if name == MEMORY_PROBE_START_OP || name == MEMORY_PROBE_OP || name == MEMORY_PROBE_END_OP {
            // TODO(port): probeMemoryStart / probeMemory / probeMemoryEnd need
            // render-test/allocation_index.cpp (a counting global allocator).
            result.push(not_ported(format!("{name} operation (AllocationIndex)")));
        } else if name == NETWORK_PROBE_START_OP {
            // probeNetworkStart
            result.push(Box::new(|_ctx: &mut TestContext<'_>| {
                ProxyFileSource::set_tracking_active(true);
                true
            }));
        } else if name == NETWORK_PROBE_OP {
            // probeNetwork
            let mark = get_str(arg(1)).to_string();
            result.push(Box::new(move |ctx: &mut TestContext<'_>| {
                ctx.get_metadata().metrics.network.entry(mark.clone()).or_insert(NetworkProbe::new(
                    ProxyFileSource::get_request_count(),
                    ProxyFileSource::get_transferred_size(),
                ));
                true
            }));
        } else if name == NETWORK_PROBE_END_OP {
            // probeNetworkEnd
            result.push(Box::new(|_ctx: &mut TestContext<'_>| {
                ProxyFileSource::set_tracking_active(false);
                true
            }));
        } else if name == SET_FEATURE_STATE_OP || name == GET_FEATURE_STATE_OP || name == REMOVE_FEATURE_STATE_OP {
            let feature_options = arg(1);
            let source_id = feature_options.get("source").map(|v| get_str(v).to_string()).unwrap_or_default();
            let source_layer: Option<String> = feature_options.get("sourceLayer").map(|v| get_str(v).to_string());
            let feature_id = match feature_options.get("id") {
                Some(serde_json::Value::String(id)) => id.clone(),
                // `util::toString(GetUint64())`
                Some(serde_json::Value::Number(n)) => n.as_u64().unwrap_or(0).to_string(),
                _ => String::new(),
            };
            let finish = |ctx: &mut TestContext<'_>, result: Result<bool, MapError>| match result {
                Ok(ok) => ok,
                Err(MapError::NotPorted(what)) => ctx.fail_not_ported(&what),
                Err(MapError::Exception(what)) => ctx.fail_exception(&what),
                Err(_) => false,
            };
            if name == SET_FEATURE_STATE_OP {
                // setFeatureState
                let parsed_state = parse_feature_state(arg(2), metadata);
                result.push(Box::new(move |ctx: &mut TestContext<'_>| {
                    let result = ctx.get_map().set_feature_state(
                        &source_id,
                        source_layer.as_deref(),
                        &feature_id,
                        &parsed_state,
                    );
                    finish(ctx, result)
                }));
            } else if name == GET_FEATURE_STATE_OP {
                // getFeatureState
                result.push(Box::new(move |ctx: &mut TestContext<'_>| {
                    let result = ctx.get_map().get_feature_state(&source_id, source_layer.as_deref(), &feature_id);
                    finish(ctx, result)
                }));
            } else {
                // removeFeatureState: featureID is always passed (a
                // std::string converts to a non-empty optional).
                let state_key: Option<String> = if size >= 3 { Some(get_str(arg(2)).to_string()) } else { None };
                result.push(Box::new(move |ctx: &mut TestContext<'_>| {
                    let result = ctx.get_map().remove_feature_state(
                        &source_id,
                        source_layer.as_deref(),
                        Some(&feature_id),
                        state_key.as_deref(),
                    );
                    finish(ctx, result)
                }));
            }
        } else if name == PAN_GESTURE_OP {
            // benchmarkPanGesture
            if metadata.map_mode != MapMode::Continuous {
                metadata.error_message = format!("Map mode must be Continuous for {PAN_GESTURE_OP} operation");
                return Vec::new();
            }
            // TODO(port): flyTo animation with frame-time sampling
            // (HeadlessFrontend::renderOnce, getFrameTime).
            result.push(not_ported(format!("{name} operation (flyTo, renderOnce)")));
        } else if name == GFX_PROBE_START_OP {
            // probeGFXStart
            result.push(Box::new(|ctx: &mut TestContext<'_>| {
                ctx.gfx_probe_active = true;
                ctx.baseline_gfx_probe = ctx.active_gfx_probe;
                true
            }));
        } else if name == GFX_PROBE_END_OP {
            // probeGFXEnd
            result.push(Box::new(|ctx: &mut TestContext<'_>| {
                debug_assert!(ctx.gfx_probe_active);
                ctx.gfx_probe_active = false;
                true
            }));
        } else if name == GFX_PROBE_OP {
            // TODO(port): probeGFX needs RenderResult::stats
            // (gfx::RenderingStats) from the headless frontend.
            result.push(not_ported(format!("{name} operation (gfx::RenderingStats)")));
        } else if name == SET_TILE_LOD_MIN_RADIUS_OP {
            // setTileLodMinRadius
            let min_radius = get_f64(arg(1));
            result.push(Box::new(move |ctx: &mut TestContext<'_>| {
                let result = ctx.get_map().set_tile_lod_min_radius(min_radius);
                infallible(ctx, result)
            }));
        } else if name == SET_TILE_LOD_SCALE_OP {
            // setTileLodScale
            let scale = get_f64(arg(1));
            result.push(Box::new(move |ctx: &mut TestContext<'_>| {
                let result = ctx.get_map().set_tile_lod_scale(scale);
                infallible(ctx, result)
            }));
        } else if name == SET_TILE_LOD_PITCH_THRESHOLD_OP {
            // setTileLodPitchThreshold
            let pitch_threshold = get_f64(arg(1));
            result.push(Box::new(move |ctx: &mut TestContext<'_>| {
                let result = ctx.get_map().set_tile_lod_pitch_threshold(pitch_threshold);
                infallible(ctx, result)
            }));
        } else if name == SET_TILE_LOD_ZOOM_SHIFT_OP {
            // setTileLodZoomShift
            let zoom_shift = get_f64(arg(1));
            result.push(Box::new(move |ctx: &mut TestContext<'_>| {
                let result = ctx.get_map().set_tile_lod_zoom_shift(zoom_shift);
                infallible(ctx, result)
            }));
        } else if name == SET_TILE_LOD_MODE_OP {
            // setTileLodMode
            // C++ calls `.value()` on the optional: an unknown name throws
            // std::bad_optional_access, which the runner does not catch.
            let Some(mode) = TileLodMode::to_enum(get_str(arg(1))) else {
                metadata.error_message = format!("Unknown tile LOD mode: {}", get_str(arg(1)));
                return Vec::new();
            };
            result.push(Box::new(move |ctx: &mut TestContext<'_>| {
                let result = ctx.get_map().set_tile_lod_mode(mode);
                infallible(ctx, result)
            }));
        } else {
            metadata.error_message = format!("Unsupported operation: {name}");
            return Vec::new();
        }
    }

    result
}

fn camera_operation(camera: CameraOptions) -> TestOperation {
    Box::new(move |ctx: &mut TestContext<'_>| {
        let result = ctx.get_map().jump_to(&camera);
        infallible(ctx, result)
    })
}

/// `encodeBase64(data)`: boost's
/// `insert_linebreaks<base64_from_binary<transform_width<const char*, 6, 8>>, 72>`,
/// which emits the last partial sextet zero-padded and no `=` padding, with
/// a `\n` after every 72 characters (none at the end).
pub fn encode_base64(data: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len() * 4 / 3 + data.len() / 54 + 4);
    let mut count = 0usize;
    let mut push = |c: u8, out: &mut String| {
        if count == 72 {
            out.push('\n');
            count = 0;
        }
        out.push(c as char);
        count += 1;
    };
    for chunk in data.chunks(3) {
        let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        let sextets = (chunk.len() * 8).div_ceil(6);
        for i in 0..sextets {
            push(ALPHABET[((n >> (18 - 6 * i)) & 0x3f) as usize], &mut out);
        }
    }
    out
}

fn to_string_u32(v: u32) -> String {
    v.to_string()
}

/// `createResultItem(metadata, hasFailedTests)`.
pub fn create_result_item(metadata: &TestMetadata, has_failed_tests: bool) -> String {
    let should_hide = (has_failed_tests && metadata.status == "passed") || metadata.status.contains("ignored");

    let mut html = String::new();
    html.push_str(&format!("<div class=\"test {}{}\">\n", metadata.status, if should_hide { " hide" } else { "" }));
    html.push_str(&format!(
        r#"<h2><span class="label" style="background: {}">{}</span> {}</h2>"#,
        metadata.color, metadata.status, metadata.id
    ));
    html.push('\n');

    if metadata.render_errored == 0 {
        if metadata.outputs_image {
            let width = to_string_u32(metadata.size.width);
            let height = to_string_u32(metadata.size.height);
            if metadata.render_test {
                html.push_str(&format!("<img width={width}"));
                html.push_str(&format!(" height={height}"));
                html.push_str(&format!(" src=\"data:image/png;base64,{}\"", encode_base64(&metadata.actual)));
                html.push_str(&format!(
                    " data-alt-src=\"data:image/png;base64,{}\">\n",
                    encode_base64(&metadata.expected)
                ));

                html.push_str(&format!("<img width={width}"));
                html.push_str(&format!(" height={height}"));
                html.push_str(&format!(" src=\"data:image/png;base64,{}\">\n", encode_base64(&metadata.diff)));
            } else {
                html.push_str(&format!("<img width={width}"));
                html.push_str(&format!(" height={height}"));
                html.push_str(&format!(" src=\"data:image/png;base64,{}\">\n", encode_base64(&metadata.actual)));
            }
        }
    } else {
        // FIXME: there are several places that errorMessage is not filled
        // comment out assert(!metadata.errorMessage.empty());
        html.push_str(&format!("<p style=\"color: red\"><strong>Error:</strong> {}</p>\n", metadata.error_message));
    }

    if metadata.metrics_failed != 0 || metadata.metrics_errored != 0 || metadata.label_cut_off_found != 0 {
        html.push_str(&format!(
            "<p style=\"color: red\"><strong>Error:</strong> {}</p>\n",
            metadata.error_message.replace('\n', "<br>")
        ));
    }

    if metadata.difference != 0.0 {
        if metadata.render_test {
            html.push_str(&format!(
                "<p class=\"diff\"><strong>Diff:</strong> {}</p>\n",
                mln_util::string::to_string_double(metadata.difference)
            ));
        } else {
            html.push_str(&format!(
                "<p class=\"diff\"><strong>Diff:</strong> {}</p>\n",
                String::from_utf8_lossy(&metadata.diff)
            ));
        }
    }
    html.push_str("</div>\n");

    html
}

/// `createResultPage(stats, metadatas, shuffle, seed)`.
pub fn create_result_page(stats: &TestStatistics, metadatas: &[TestMetadata], shuffle: bool, seed: u32) -> String {
    let unsuccessful = stats.errored_tests + stats.failed_tests;
    let mut results_page = String::new();

    // Style
    results_page.push_str(RESULTS_STYLE);

    // Header
    if unsuccessful != 0 {
        results_page.push_str(r#"<h1 style="color: red;">"#);
        results_page.push_str(&format!("{unsuccessful} tests failed."));
    } else {
        results_page.push_str(r#"<h1 style="color: green;">"#);
        results_page.push_str("All tests passed!");
    }
    results_page.push_str(RESULTS_HEADER_BUTTONS);

    // stats
    results_page.push_str(r#"<p class="stats">"#);
    if stats.ignore_failed_tests != 0 {
        results_page.push_str(&format!("{} ignored failed, ", stats.ignore_failed_tests));
    }
    if stats.ignore_passed_tests != 0 {
        results_page.push_str(&format!("{} ignored passed, ", stats.ignore_passed_tests));
    }
    if stats.errored_tests != 0 {
        results_page.push_str(&format!("{} errored, ", stats.errored_tests));
    }
    if stats.failed_tests != 0 {
        results_page.push_str(&format!("{} failed, ", stats.failed_tests));
    }
    results_page.push_str(&format!("{} passed.\n", stats.passed_tests));
    results_page.push_str("</p>\n");

    // Test sequence
    {
        results_page.push_str("<div id='test-sequence' class='hide'>\n");

        // Failed tests
        if unsuccessful != 0 {
            results_page.push_str("<p><strong>Failed tests:</strong>");
            for metadata in metadatas {
                if metadata.status == "failed" || metadata.status == "errored" {
                    results_page.push_str(&format!("{} ", metadata.id));
                }
            }
            results_page.push_str("</p>\n");
        }

        // Test sequence
        results_page.push_str("<p><strong>Test sequence:</strong>");
        for metadata in metadatas {
            results_page.push_str(&format!("{} ", metadata.id));
        }
        results_page.push_str("</p>\n");

        // Shuffle
        if shuffle {
            results_page.push_str(&format!("<p><strong>Shuffle seed</strong>: {seed}</p>\n"));
        }

        results_page.push_str("</div>\n");
    }

    // Script
    results_page.push_str(RESULTS_SCRIPT);

    // Tests
    results_page.push_str("<div class=\"tests\">\n");
    for metadata in metadatas {
        results_page.push_str(&create_result_item(metadata, unsuccessful != 0));
    }
    results_page.push_str("</div>\n");

    results_page
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_like_boost() {
        assert_eq!(encode_base64(b""), "");
        assert_eq!(encode_base64(b"f"), "Zg");
        assert_eq!(encode_base64(b"fo"), "Zm8");
        assert_eq!(encode_base64(b"foo"), "Zm9v");
        assert_eq!(encode_base64(b"foobar"), "Zm9vYmFy");
        // 54 bytes -> exactly 72 characters, no line break.
        let line = encode_base64(&[0u8; 54]);
        assert_eq!(line.len(), 72);
        assert!(!line.contains('\n'));
        // 55 bytes -> a break after 72 characters, then the 2 characters of
        // the last byte.
        let two = encode_base64(&[0u8; 55]);
        assert_eq!(two.find('\n'), Some(72));
        assert_eq!(two.len(), 72 + 1 + 2);
    }

    #[test]
    fn global_state_value() {
        let v: serde_json::Value = serde_json::from_str(r#"[null, true, 1, -1, 1.5, "s", {"a": [2]}]"#).unwrap();
        let mut object = feature::ValueObject::new();
        object.insert("a".to_string(), Value::Array(vec![Value::Uint(2)]));
        assert_eq!(
            parse_global_state_value(&v),
            Value::Array(vec![
                Value::Null,
                Value::Bool(true),
                Value::Uint(1),
                Value::Int(-1),
                Value::Double(1.5),
                Value::String("s".to_string()),
                Value::Object(object),
            ])
        );
    }
}
