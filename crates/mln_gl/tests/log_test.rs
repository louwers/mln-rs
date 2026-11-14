#![cfg(not(target_arch = "wasm32"))]

// A library device logs the platform it is on, as maplibre-native-ffi's
// graphics devices do, and the process's first context logs the GPU, as C++'s
// does. A test binary of its own, since a context of any other test would
// have logged the GPU first.

use std::sync::mpsc::{Sender, channel};

use mln_gfx::renderer_backend::RendererBackend as _;
use mln_gfx::{BackendScope, ScopeType};
use mln_gl::{Device, DeviceOptions, RendererBackend};
use mln_util::logging::{Event, EventSeverity, Log, Observer};

struct Recorder(Sender<(EventSeverity, Event, i64, String)>);

impl Observer for Recorder {
    fn on_record(&self, severity: EventSeverity, event: Event, code: i64, msg: &str) -> bool {
        let _ = self.0.send((severity, event, code, msg.to_owned()));
        true
    }
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
    let (sender, records) = channel();
    let previous = Log::set_thread_observer(Some(Box::new(Recorder(sender))));
    let Some(share_group) = device(DeviceOptions::default()) else { return };
    let mut devices = vec![share_group];
    // WGL makes no single context.
    if !cfg!(windows) {
        devices.push(Device::with_options(DeviceOptions { single_context: true, ..DeviceOptions::default() }).unwrap());
    }
    let mut renderer = String::new();
    for device in &devices {
        let backend = RendererBackend::new(device).unwrap();
        let _scope = BackendScope::new(&backend, ScopeType::Explicit);
        backend.get_context();
        renderer = backend.get_renderer_name();
    }
    drop(Log::set_thread_observer(previous));

    let records: Vec<_> = records.try_iter().collect();
    let platform = records.first().map(|record| record.3.clone()).unwrap_or_default();
    assert!(PLATFORMS.contains(&platform.as_str()), "{records:?}");
    let mut expected = vec![(EventSeverity::Info, Event::Render, -1, platform); devices.len()];
    expected.push((EventSeverity::Info, Event::General, -1, format!("GPU Identifier: {renderer}")));
    assert_eq!(records, expected);
}
