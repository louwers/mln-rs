// Surface sessions that present to X11 windows: the frames they render,
// releasing the surface, and moving to another window. It needs an X server
// that the GPU can present to, so it is ignored by default: run
// `cargo test -p mln-rs-runtime --test surfaces_window -- --ignored` in a
// desktop session, or in a headless compositor such as
// `kwin_wayland --virtual --xwayland --exit-with-session <script>`. Xvfb has
// no DRI3, which Mesa's Vulkan drivers need to present.

#![cfg(target_os = "linux")]

mod support;

use std::sync::Arc;

use mln_map::MapMode;
use mln_runtime::{
    AttachOptions, DemandMode, Driver, DriverWork, FrameDemand, FrameDisposition, FrameResult, Map, MapOptions,
    RenderSession, RenderTarget, Runtime, Surface,
};
use mln_util::size::Size;
use support::{TIMEOUT, eventually, runtime, wait};
use winit::dpi::PhysicalSize;
use winit::event_loop::EventLoop;
use winit::platform::x11::EventLoopBuilderExtX11;
use winit::window::Window;

const STYLE: &str =
    r##"{"version":8,"sources":{},"layers":[{"id":"bg","type":"background","paint":{"background-color":"#00ff00"}}]}"##;

fn map(runtime: &Runtime) -> Map {
    let options = MapOptions {
        map: mln_map::MapOptions { size: Size::new(256, 256), map_mode: MapMode::Continuous, ..Default::default() },
        ..Default::default()
    };
    let (map, created) = runtime.create_map(options).unwrap();
    created.wait().unwrap();
    wait(map.set_style_json(STYLE));
    map
}

/// Demands a forced frame, and takes its result, servicing `driver` while
/// it waits when the session has one.
fn frame(session: &RenderSession, driver: Option<&DriverWork>, token: u64) -> FrameResult {
    session.demand_frame(FrameDemand::new(DemandMode::Force, token)).unwrap();
    let mut results = Vec::new();
    let deadline = std::time::Instant::now() + TIMEOUT;
    while results.is_empty() {
        assert!(std::time::Instant::now() < deadline, "no frame result");
        if let Some(driver) = driver {
            driver.service().unwrap();
        }
        results.extend(session.drain_frame_results().unwrap());
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    assert_eq!(1, results.len(), "{results:?}");
    assert_eq!(Some(token), results[0].token);
    results[0]
}

/// Waits for a completion, servicing `driver` meanwhile.
fn done<T: Send + 'static>(completion: mln_runtime::Completion<T>, driver: Option<&DriverWork>) -> T {
    if let Some(driver) = driver {
        assert!(eventually(|| {
            driver.service().unwrap();
            completion.is_complete()
        }));
    }
    completion.wait().unwrap()
}

/// Renders to `first`, releases the surface, then moves to `second`.
fn present(map: &Map, driver_kind: Driver, first: &Arc<Window>, second: &Arc<Window>) {
    let target =
        RenderTarget::Surface { size: Size::new(64, 48), surface: Surface::window(Arc::clone(first)), device: None };
    let (session, attached) = map.attach(target, AttachOptions { driver: driver_kind, ..Default::default() }).unwrap();
    let driver = (driver_kind == Driver::CallerThread).then(|| session.take_driver_work().unwrap());
    let driver = driver.as_ref();
    done(attached, driver);
    assert!(session.capabilities().unwrap().presentation);
    assert!(session.snapshot().unwrap().has_target);

    let shown = frame(&session, driver, 1);
    assert_eq!(FrameDisposition::Rendered, shown.disposition);
    assert_eq!(Size::new(64, 48), shown.view.unwrap().extent.size);

    done(session.release_target().unwrap(), driver);
    assert!(!session.snapshot().unwrap().has_target);
    assert_eq!(FrameDisposition::TargetNotReady, frame(&session, driver, 2).disposition);

    done(session.set_surface(Size::new(80, 40), Surface::window(Arc::clone(second))).unwrap(), driver);
    let snapshot = session.snapshot().unwrap();
    assert!(snapshot.has_target);
    assert_eq!(Size::new(80, 40), snapshot.extent.size);
    let moved = frame(&session, driver, 3);
    assert_eq!(FrameDisposition::Rendered, moved.disposition);
    assert_eq!(Size::new(80, 40), moved.view.unwrap().extent.size);

    done(session.release().unwrap(), driver);
}

#[test]
#[ignore = "needs an X server"]
fn surface_sessions_present_release_and_move_between_windows() {
    let event_loop = EventLoop::builder().with_x11().with_any_thread(true).build().unwrap();
    let window = |width, height| {
        #[expect(deprecated, reason = "the test runs no event loop to create windows from")]
        let window = event_loop
            .create_window(Window::default_attributes().with_inner_size(PhysicalSize::new(width, height)))
            .unwrap();
        Arc::new(window)
    };
    let first = window(64, 48);
    let second = window(80, 40);
    let runtime = runtime();
    let map = map(&runtime);
    present(&map, Driver::CoreWorker, &first, &second);
    present(&map, Driver::CallerThread, &first, &second);
}
