//! `test/src/mln/test/fixture_log_observer.hpp`'s `FixtureLog`, over the
//! `log` crate and for the calling thread.

use std::cell::RefCell;
use std::sync::{Arc, Once};

use parking_lot::Mutex;

/// A caught record: its level, target and message, and whether a `count`
/// matched it.
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

/// Captures the calling thread's log records while it lives.
pub(crate) struct FixtureLog {
    records: Records,
}

impl FixtureLog {
    pub(crate) fn new() -> Self {
        static INSTALL: Once = Once::new();
        INSTALL.call_once(|| {
            log::set_logger(&Catcher).expect("the test binary has no other logger");
            log::set_max_level(log::LevelFilter::Info);
        });
        let records = Records::default();
        CAUGHT.set(Some(Arc::clone(&records)));
        FixtureLog { records }
    }

    /// Counts the matching records, and marks them checked.
    pub(crate) fn count(&self, level: log::Level, target: &str, msg: &str) -> usize {
        let mut count = 0;
        for ((l, t, m), checked) in self.records.lock().iter_mut() {
            if *l == level && t == target && m == msg {
                *checked = true;
                count += 1;
            }
        }
        count
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.records.lock().is_empty()
    }
}

impl Drop for FixtureLog {
    fn drop(&mut self) {
        CAUGHT.set(None);
    }
}
