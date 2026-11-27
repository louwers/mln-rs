//! Test helpers: the C++ `FixtureLog`, over the `log` crate and per thread,
//! and `convertJSON`.

use std::borrow::Cow;
use std::cell::RefCell;
use std::sync::{Arc, Once};

use parking_lot::Mutex;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct LogMessage {
    pub(crate) level: log::Level,
    pub(crate) target: String,
    pub(crate) msg: String,
}

type Messages = Arc<Mutex<Vec<(LogMessage, bool)>>>;

thread_local! {
    /// The messages of the thread's live `FixtureLog`, if any.
    static CAUGHT: RefCell<Option<Messages>> = const { RefCell::new(None) };
}

/// The test binary's `log` logger. It hands each of the library's records to
/// the `FixtureLog` of the thread that logged it, so tests that run on
/// threads of one process don't see each other's.
struct Catcher;

impl log::Log for Catcher {
    fn enabled(&self, metadata: &log::Metadata<'_>) -> bool {
        metadata.target().starts_with("mln_")
    }

    fn log(&self, record: &log::Record<'_>) {
        if !self.enabled(record.metadata()) {
            return;
        }
        CAUGHT.with_borrow(|caught| {
            if let Some(messages) = caught {
                let message = LogMessage {
                    level: record.level(),
                    target: record.target().to_owned(),
                    msg: record.args().to_string(),
                };
                messages.lock().push((message, false));
            }
        });
    }

    fn flush(&self) {}
}

/// Records this thread's log messages while alive.
pub(crate) struct FixtureLog {
    messages: Messages,
}

impl FixtureLog {
    pub(crate) fn new() -> Self {
        static INSTALL: Once = Once::new();
        INSTALL.call_once(|| {
            log::set_logger(&Catcher).expect("the test binary has no other logger");
            log::set_max_level(log::LevelFilter::Info);
        });
        let messages = Messages::default();
        CAUGHT.set(Some(Arc::clone(&messages)));
        FixtureLog { messages }
    }

    /// `count(message)`: the number of matching messages, which are then
    /// checked.
    pub(crate) fn count(&self, level: log::Level, target: &str, msg: &str) -> usize {
        let mut messages = self.messages.lock();
        let mut count = 0;
        for (m, checked) in messages.iter_mut() {
            if m.level == level && m.target == target && m.msg == msg {
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
        CAUGHT.set(None);
    }
}

/// Reads a file of testdata/, the copy of the C++ repository's files that
/// these tests read, by its path relative to the repository.
pub(crate) fn read_file(relative: &str) -> String {
    let path = std::path::Path::new(&std::env::var_os("CARGO_MANIFEST_DIR").unwrap()).join("testdata").join(relative);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

pub(crate) fn parse_json(json: &str) -> serde_json::Value {
    serde_json::from_str(json).expect("test JSON must parse")
}

pub(super) use crate::conversion::value_to_json;

pub(crate) struct StubGeometryTileFeature {
    pub(crate) properties: crate::feature::PropertyMap,
    pub(crate) id: crate::feature::Identifier,
    pub(crate) type_: crate::geometry::FeatureType,
    pub(crate) geometry: crate::feature::GeometryCollection,
}

impl StubGeometryTileFeature {
    pub(crate) fn new(properties: crate::feature::PropertyMap) -> Self {
        StubGeometryTileFeature {
            properties,
            id: crate::feature::Identifier::Null,
            type_: crate::geometry::FeatureType::Point,
            geometry: crate::feature::GeometryCollection::default(),
        }
    }

    pub(crate) fn with_geometry(
        type_: crate::geometry::FeatureType,
        geometry: crate::feature::GeometryCollection,
    ) -> Self {
        StubGeometryTileFeature {
            properties: crate::feature::PropertyMap::default(),
            id: crate::feature::Identifier::Null,
            type_,
            geometry,
        }
    }

    /// A feature with an id, a type and properties, and no geometry.
    pub(crate) fn with_id_type(
        id: crate::feature::Identifier,
        type_: crate::geometry::FeatureType,
        properties: crate::feature::PropertyMap,
    ) -> Self {
        StubGeometryTileFeature { properties, id, type_, geometry: crate::feature::GeometryCollection::default() }
    }
}

impl crate::feature::GeometryTileFeature for StubGeometryTileFeature {
    fn get_type(&self) -> crate::geometry::FeatureType {
        self.type_
    }

    fn get_value(&self, key: &str) -> Option<crate::feature::Value> {
        self.properties.get(key).cloned()
    }

    fn get_properties(&self) -> Cow<'_, crate::feature::PropertyMap> {
        Cow::Borrowed(&self.properties)
    }

    fn get_id(&self) -> crate::feature::Identifier {
        self.id.clone()
    }

    fn get_geometries(&self) -> Result<Cow<'_, crate::feature::GeometryCollection>, crate::error::Error> {
        Ok(Cow::Borrowed(&self.geometry))
    }
}

pub(crate) fn property_map(entries: &[(&str, crate::feature::Value)]) -> crate::feature::PropertyMap {
    entries.iter().map(|(k, v)| ((*k).to_owned(), v.clone())).collect()
}

/// A layer of the built-in type `type_`.
pub(crate) fn new_layer(type_: &str, id: &str, source: &str) -> crate::layer::Layer {
    crate::layer::Layer::new(&crate::StyleRegistry::with_builtins(), type_, id, source).unwrap()
}

/// The style data of `layer`, a `T` layer.
pub(crate) fn style_of<T: crate::layer::StyleLayerType>(layer: &crate::layer::Layer) -> &crate::layer::LayerStyle<T> {
    crate::layer::LayerStyle::recover_ref(&**layer.style()).unwrap()
}

impl crate::Style {
    /// Replaces the style with the one `json` describes, as a map's loader
    /// does; a document that doesn't parse leaves the style as it was.
    pub(crate) fn load_json(&mut self, json: &str) {
        if let Ok(parsed) = Self::from_json(&Arc::clone(self.registry()), json) {
            *self = parsed;
        }
    }
}

impl crate::Style {
    /// Records the source that `json` describes, as the map does when it
    /// adds one.
    pub(crate) fn add_source_json(&mut self, id: &str, json: &str) {
        let value: serde_json::Value = serde_json::from_str(json).expect("JSON");
        let mut error = crate::conversion::Error::default();
        let registry = Arc::clone(self.registry());
        let source =
            crate::source::convert_source(&crate::conversion::Convertible::from(&value), &mut error, id, &registry)
                .expect("a source");
        let _ = self.apply(crate::style::edit::Edit::AddSource(source));
    }
}
