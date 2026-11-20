//! The style parser over the C++ repository's style parser fixtures, whose
//! styles have layers of every built-in type, so it parses against the
//! facade's built-in registrations.

use std::path::PathBuf;
use std::sync::Arc;

use mln_rs::builtin_registry;
use mln_style::parser::Parser;
use mln_util::logging::{Event, EventSeverity, Log, Observer};
use parking_lot::Mutex;

fn fixtures_dir() -> PathBuf {
    std::path::Path::new(&std::env::var_os("CARGO_MANIFEST_DIR").unwrap()).join("testdata/test/fixtures/style_parser")
}

/// A log message, and whether a count matched it.
type Logged = (EventSeverity, Event, String, bool);

/// The C++ `FixtureLog`: records this thread's log messages while alive.
struct FixtureLog(Arc<Mutex<Vec<Logged>>>);

struct Recorder(Arc<Mutex<Vec<Logged>>>);

impl Observer for Recorder {
    fn on_record(&self, severity: EventSeverity, event: Event, _code: i64, msg: &str) -> bool {
        self.0.lock().push((severity, event, msg.to_owned(), false));
        true
    }
}

impl FixtureLog {
    fn new() -> Self {
        let messages = Arc::new(Mutex::new(Vec::new()));
        let _ = Log::set_thread_observer(Some(Box::new(Recorder(Arc::clone(&messages)))));
        FixtureLog(messages)
    }

    /// The number of messages that match, which are then checked.
    fn count(&self, severity: EventSeverity, event: Event, msg: &str) -> usize {
        let mut count = 0;
        for (s, e, m, checked) in self.0.lock().iter_mut() {
            if *s == severity && *e == event && m == msg {
                *checked = true;
                count += 1;
            }
        }
        count
    }

    /// The messages no `count` call has matched.
    fn unchecked(&self) -> Vec<String> {
        self.0.lock().iter().filter(|logged| !logged.3).map(|(s, e, m, _)| format!("{s:?} {e:?}: {m}")).collect()
    }
}

impl Drop for FixtureLog {
    fn drop(&mut self) {
        let _ = Log::set_thread_observer(None);
    }
}

/// The name of every `<name>.info.json` in the fixture directory.
fn fixture_names() -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(fixtures_dir())
        .unwrap()
        .filter_map(|entry| {
            let name = entry.unwrap().file_name().into_string().unwrap();
            name.strip_suffix(".info.json").map(str::to_owned)
        })
        .collect();
    names.sort();
    names
}

/// One `StyleParserTest.ParseStyle` instance. Returns the failure, if any.
fn parse_style_fixture(name: &str) -> Result<(), String> {
    let base = fixtures_dir().join(name);
    let log = FixtureLog::new();

    let info_doc: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(base.with_extension("info.json")).unwrap()).unwrap();
    let style_json = std::fs::read_to_string(base.with_extension("style.json")).unwrap();

    let mut parser = Parser::new(builtin_registry().style());
    if let Err(error) = parser.parse(&style_json) {
        Log::error(Event::ParseStyle, format!("Failed to parse style: {error}"));
    }

    for (property_name, value) in info_doc.as_object().unwrap() {
        if let Some(js_log) = value.get("log") {
            for js_entry in js_log.as_array().unwrap() {
                let js_entry = js_entry.as_array().unwrap();
                let count = js_entry[0].as_u64().unwrap() as usize;
                let severity = js_entry[1].as_str().unwrap().parse::<EventSeverity>().unwrap();
                let event = js_entry[2].as_str().unwrap().parse::<Event>().unwrap();
                let msg = js_entry[3].as_str().unwrap();
                let observed = log.count(severity, event, msg);
                if count != observed {
                    return Err(format!("{name}/{property_name}: Message: {msg}: expected {count}, got {observed}"));
                }
            }
        }
        let unchecked = log.unchecked();
        if !unchecked.is_empty() {
            return Err(format!("Unchecked Log Messages ({name}/{property_name}): {unchecked:?}"));
        }
    }
    Ok(())
}

/// Runs the style parser test over every fixture.
// gtest: StyleParserTest.ParseStyle
#[test]
fn parse_style() {
    let names = fixture_names();
    assert_ne!(names, Vec::<String>::new());
    let mut failures = Vec::new();
    for name in &names {
        if let Err(failure) = parse_style_fixture(name) {
            failures.push(failure);
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
