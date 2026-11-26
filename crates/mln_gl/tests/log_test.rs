#![cfg(not(target_arch = "wasm32"))]

// A library device logs the platform it is on, as maplibre-native-ffi's
// graphics devices do, and the process's first context logs the GPU, as C++'s
// does. A test binary of its own, since a context of any other test would
// have logged the GPU first.

use std::cell::RefCell;
use std::sync::Once;

use mln_gfx::RendererBackend as _;
use mln_gfx::gpu::Gpu as _;
use mln_gl::{Device, DeviceOptions, RendererBackend};

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

/// What a library device logs, by the platform it is on.
const PLATFORMS: [&str; 3] = [
    "graphics device uses the EGL surfaceless platform",
    "graphics device uses the EGL default platform",
    "graphics device uses WGL",
];

/// A library device, or none where there is no EGL or WGL to make one with,
/// as on macOS: the test is skipped there, unless `MLN_RENDER_BACKEND` asks
/// for OpenGL (`gl` or `gl-desktop`).
fn device(options: DeviceOptions) -> Option<Device> {
    match Device::with_options(options) {
        Ok(device) => Some(device),
        Err(error) if std::env::var("MLN_RENDER_BACKEND").is_ok_and(|backend| backend.starts_with("gl")) => {
            panic!("{error}")
        }
        Err(error) => {
            eprintln!("skipped: {error}");
            None
        }
    }
}

#[test]
fn each_device_logs_its_platform_and_the_first_context_its_gpu() {
    let (devices, records) = capture("mln_gl", || {
        let share_group = device(DeviceOptions::default())?;
        let mut devices = vec![share_group];
        // WGL makes no single context.
        if !cfg!(windows) {
            devices.push(
                Device::with_options(DeviceOptions { single_context: true, ..DeviceOptions::default() }).unwrap(),
            );
        }
        let mut renderer = String::new();
        for device in &devices {
            // A scope alone logs nothing; the first flush logs the GPU.
            let mut backend = RendererBackend::new(device).unwrap();
            drop(backend.gpu());
            backend.gpu().flush(true).unwrap();
            renderer = backend.get_renderer_name();
        }
        Some((devices.len(), renderer))
    });
    let Some((devices, renderer)) = devices else { return };

    let platform = records.first().map(|record| record.2.clone()).unwrap_or_default();
    assert!(PLATFORMS.contains(&platform.as_str()), "{records:?}");
    let mut expected = vec![(log::Level::Info, "mln_gl::render".to_owned(), platform); devices];
    expected.push((log::Level::Info, "mln_gl::gpu".to_owned(), format!("GPU Identifier: {renderer}")));
    assert_eq!(records, expected);
}
