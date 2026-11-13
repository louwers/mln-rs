// Ported gtests: test/sprite/sprite_parser.test.cpp and
// test/sprite/sprite_loader.test.cpp.

mod sprite_loader;
mod sprite_parser;

use std::path::PathBuf;
use std::sync::Arc;

use mln_util::logging::{Event, EventSeverity, Log, Observer};
use parking_lot::Mutex;

/// A path of the C++ repository, in the copy of its files these tests read
/// (testdata/).
pub(crate) fn cpp_path(relative: &str) -> PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata").join(relative)
}

pub(crate) fn read_file(relative: &str) -> Vec<u8> {
    let path = cpp_path(relative);
    std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

type Records = Arc<Mutex<Vec<((EventSeverity, Event, i64, String), bool)>>>;

struct Recorder(Records);

impl Observer for Recorder {
    fn on_record(&self, severity: EventSeverity, event: Event, code: i64, msg: &str) -> bool {
        self.0.lock().push(((severity, event, code, msg.to_owned()), false));
        true
    }
}

/// Captures the log records of the calling thread while it lives.
pub(crate) struct FixtureLog {
    messages: Records,
}

impl FixtureLog {
    pub(crate) fn new() -> Self {
        let messages: Records = Arc::new(Mutex::new(Vec::new()));
        let _ = Log::set_thread_observer(Some(Box::new(Recorder(messages.clone()))));
        FixtureLog { messages }
    }

    pub(crate) fn count(&self, severity: EventSeverity, event: Event, code: i64, msg: &str) -> usize {
        let mut count = 0;
        for (m, checked) in self.messages.lock().iter_mut() {
            if !*checked && m.0 == severity && m.1 == event && m.2 == code && m.3 == msg {
                *checked = true;
                count += 1;
            }
        }
        count
    }

    pub(crate) fn unchecked_count(&self) -> usize {
        self.messages.lock().iter().filter(|(_, checked)| !checked).count()
    }
}

impl Drop for FixtureLog {
    fn drop(&mut self) {
        let _ = Log::set_thread_observer(None);
    }
}
