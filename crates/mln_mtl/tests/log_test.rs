#![cfg(target_vendor = "apple")]

// A library device logs the GPU it is on, as maplibre-native-ffi's graphics
// devices do.

use std::cell::RefCell;
use std::sync::Once;

use mln_mtl::Device;

/// A record that the test logger caught: its level, target and message.
type Caught = (log::Level, String, String);

thread_local! {
    /// The records of `capture`'s crate that this thread logs during its
    /// run, or `None` outside one.
    static CAUGHT: RefCell<Option<(&'static str, Vec<Caught>)>> = const { RefCell::new(None) };
}

/// The test binary's logger. It keeps each thread's records apart, so tests
/// that run on threads of one process don't see each other's.
struct Catcher;

impl log::Log for Catcher {
    fn enabled(&self, _: &log::Metadata<'_>) -> bool {
        true
    }

    fn log(&self, record: &log::Record<'_>) {
        CAUGHT.with_borrow_mut(|caught| {
            if let Some((krate, records)) = caught
                && record.target().split("::").next() == Some(*krate)
            {
                records.push((record.level(), record.target().to_owned(), record.args().to_string()));
            }
        });
    }

    fn flush(&self) {}
}

/// Runs `f` and returns its result with the records of `krate` that it
/// logged on this thread.
fn capture<T>(krate: &'static str, f: impl FnOnce() -> T) -> (T, Vec<Caught>) {
    static INSTALL: Once = Once::new();
    INSTALL.call_once(|| {
        log::set_logger(&Catcher).expect("the test binary has no other logger");
        log::set_max_level(log::LevelFilter::Trace);
    });
    CAUGHT.set(Some((krate, Vec::new())));
    let result = f();
    let records = CAUGHT.take().map(|(_, records)| records).unwrap_or_default();
    (result, records)
}

#[test]
fn a_device_logs_its_gpu() {
    let (device, records) = capture("mln_mtl", || Device::new(None).expect("Metal device"));

    let message = format!("graphics device uses Metal device {}", device.name());
    assert_eq!(records, [(log::Level::Info, "mln_mtl::render".to_owned(), message)]);
}
