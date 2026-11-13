//! Test helpers: the C++ `FixtureLog` (per thread, see `mln_util::logging`)
//! and `convertJSON`.

use std::sync::Arc;

use mln_util::logging::{Event, EventSeverity, Log, Observer};
use parking_lot::Mutex;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct LogMessage {
    pub(crate) severity: EventSeverity,
    pub(crate) event: Event,
    pub(crate) code: i64,
    pub(crate) msg: String,
}

struct Recorder(Arc<Mutex<Vec<(LogMessage, bool)>>>);

impl Observer for Recorder {
    fn on_record(&self, severity: EventSeverity, event: Event, code: i64, msg: &str) -> bool {
        self.0.lock().push((LogMessage { severity, event, code, msg: msg.to_owned() }, false));
        true
    }
}

/// Records this thread's log messages while alive.
pub(crate) struct FixtureLog {
    messages: Arc<Mutex<Vec<(LogMessage, bool)>>>,
}

impl FixtureLog {
    pub(crate) fn new() -> Self {
        let messages = Arc::new(Mutex::new(Vec::new()));
        let _ = Log::set_thread_observer(Some(Box::new(Recorder(messages.clone()))));
        FixtureLog { messages }
    }

    /// `count(message)`: the number of matching messages, which are then
    /// checked (C++ ignores the code when it is -1 in the pattern).
    pub(crate) fn count(&self, severity: EventSeverity, event: Event, msg: &str) -> usize {
        let mut messages = self.messages.lock();
        let mut count = 0;
        for (m, checked) in messages.iter_mut() {
            if m.severity == severity && m.event == event && m.msg == msg {
                *checked = true;
                count += 1;
            }
        }
        count
    }

    /// The messages no `count` call has matched.
    pub(crate) fn unchecked(&self) -> Vec<LogMessage> {
        self.messages.lock().iter().filter(|(_, checked)| !checked).map(|(m, _)| m.clone()).collect()
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.messages.lock().is_empty()
    }

    pub(crate) fn messages(&self) -> Vec<LogMessage> {
        self.messages.lock().iter().map(|(m, _)| m.clone()).collect()
    }
}

impl Drop for FixtureLog {
    fn drop(&mut self) {
        let _ = Log::set_thread_observer(None);
    }
}

/// Reads a file of testdata/, the copy of the C++ repository's files that
/// these tests read, by its path relative to the repository.
pub(crate) fn read_file(relative: &str) -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata").join(relative);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

/// Reads a file of testdata/ as bytes.
pub(crate) fn read_file_bytes(relative: &str) -> Vec<u8> {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata").join(relative);
    std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

pub(crate) fn parse_json(json: &str) -> serde_json::Value {
    serde_json::from_str(json).expect("test JSON must parse")
}

pub(super) use mln_conversion::value_to_json;

pub(crate) struct StubGeometryTileFeature {
    pub(crate) properties: feature::PropertyMap,
    pub(crate) id: feature::Identifier,
    pub(crate) type_: mln_util::geometry::FeatureType,
    pub(crate) geometry: mln_tile_id::geometry_tile_data::GeometryCollection,
}

impl StubGeometryTileFeature {
    pub(crate) fn new(properties: feature::PropertyMap) -> Self {
        StubGeometryTileFeature {
            properties,
            id: feature::Identifier::Null,
            type_: mln_util::geometry::FeatureType::Point,
            geometry: mln_tile_id::geometry_tile_data::GeometryCollection::default(),
        }
    }

    pub(crate) fn with_geometry(
        type_: mln_util::geometry::FeatureType,
        geometry: mln_tile_id::geometry_tile_data::GeometryCollection,
    ) -> Self {
        StubGeometryTileFeature {
            properties: feature::PropertyMap::default(),
            id: feature::Identifier::Null,
            type_,
            geometry,
        }
    }

    /// A feature with an id, a type and properties, and no geometry.
    pub(crate) fn with_id_type(
        id: feature::Identifier,
        type_: mln_util::geometry::FeatureType,
        properties: feature::PropertyMap,
    ) -> Self {
        StubGeometryTileFeature {
            properties,
            id,
            type_,
            geometry: mln_tile_id::geometry_tile_data::GeometryCollection::default(),
        }
    }
}

impl mln_tile_id::geometry_tile_data::GeometryTileFeature for StubGeometryTileFeature {
    fn get_type(&self) -> mln_util::geometry::FeatureType {
        self.type_
    }

    fn get_value(&self, key: &str) -> Option<feature::Value> {
        self.properties.get(key).cloned()
    }

    fn get_properties(&self) -> &feature::PropertyMap {
        &self.properties
    }

    fn get_id(&self) -> feature::Identifier {
        self.id.clone()
    }

    fn get_geometries(&self) -> &mln_tile_id::geometry_tile_data::GeometryCollection {
        &self.geometry
    }
}

pub(crate) fn property_map(entries: &[(&str, feature::Value)]) -> feature::PropertyMap {
    entries.iter().map(|(k, v)| ((*k).to_owned(), v.clone())).collect()
}
