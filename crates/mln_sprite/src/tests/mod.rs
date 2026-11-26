// Ported gtests: test/sprite/sprite_parser.test.cpp.

mod sprite_parser;

use std::cell::RefCell;
use std::path::PathBuf;
use std::sync::{Arc, Once};

use parking_lot::Mutex;

/// A path of the C++ repository, in the copy of its files these tests read
/// (testdata/).
pub(crate) fn cpp_path(relative: &str) -> PathBuf {
    std::path::Path::new(&std::env::var_os("CARGO_MANIFEST_DIR").unwrap()).join("testdata").join(relative)
}

pub(crate) fn read_file(relative: &str) -> Vec<u8> {
    let path = cpp_path(relative);
    std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

type Records = Arc<Mutex<Vec<((log::Level, String, String), bool)>>>;

thread_local! {
    /// The records of the thread's live `FixtureLog`, if any.
    static CAUGHT: RefCell<Option<Records>> = const { RefCell::new(None) };
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
            if let Some(records) = caught {
                let message = (record.level(), record.target().to_owned(), record.args().to_string());
                records.lock().push((message, false));
            }
        });
    }

    fn flush(&self) {}
}

/// Captures the log records of the calling thread while it lives.
pub(crate) struct FixtureLog {
    messages: Records,
}

impl FixtureLog {
    pub(crate) fn new() -> Self {
        static INSTALL: Once = Once::new();
        INSTALL.call_once(|| {
            log::set_logger(&Catcher).expect("the test binary has no other logger");
            log::set_max_level(log::LevelFilter::Info);
        });
        let messages = Records::default();
        CAUGHT.set(Some(Arc::clone(&messages)));
        FixtureLog { messages }
    }

    pub(crate) fn count(&self, level: log::Level, target: &str, msg: &str) -> usize {
        let mut count = 0;
        for (m, checked) in self.messages.lock().iter_mut() {
            if !*checked && m.0 == level && m.1 == target && m.2 == msg {
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
        CAUGHT.set(None);
    }
}
