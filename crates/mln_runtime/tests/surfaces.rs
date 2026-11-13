// Surface sessions without a display: what a session does when its window
// has no surface to give, and what an offscreen session refuses. A window
// that presents frames is in surfaces_window.rs, which needs a display.

mod support;

use mln_map::MapMode;
use mln_runtime::{
    AttachOptions, Driver, ErrorKind, Map, MapOptions, RenderSessionState, RenderTarget, Runtime, Surface,
};
use mln_util::size::Size;
use support::{runtime, wait};
use wgpu::rwh::{DisplayHandle, HandleError, HasDisplayHandle, HasWindowHandle, WindowHandle};

const SIZE: Size = Size { width: 64, height: 64 };

/// A window whose handles are gone, as after the platform destroyed it.
struct GoneWindow;

impl HasWindowHandle for GoneWindow {
    fn window_handle(&self) -> Result<WindowHandle<'_>, HandleError> {
        Err(HandleError::Unavailable)
    }
}

impl HasDisplayHandle for GoneWindow {
    fn display_handle(&self) -> Result<DisplayHandle<'_>, HandleError> {
        Err(HandleError::Unavailable)
    }
}

fn map(runtime: &Runtime) -> Map {
    let options = MapOptions {
        map: mln_map::MapOptions { size: Size::new(256, 256), map_mode: MapMode::Continuous, ..Default::default() },
        ..Default::default()
    };
    let (map, created) = runtime.create_map(options).unwrap();
    created.wait().unwrap();
    map
}

fn gone_window() -> RenderTarget {
    RenderTarget::Surface { size: SIZE, surface: Surface::window(GoneWindow), device: None }
}

#[test]
fn a_surface_session_without_a_surface_fails_and_frees_its_map() {
    let runtime = runtime();
    let map = map(&runtime);
    let (session, attached) = map.attach(gone_window(), AttachOptions::default()).unwrap();
    let capabilities = session.capabilities().unwrap();
    assert!(capabilities.presentation);
    assert!(!capabilities.readback);
    let error = attached.wait().unwrap_err();
    // Without a GPU the session can't get as far as the window.
    assert!(matches!(error.kind, ErrorKind::Native | ErrorKind::Unsupported), "{error:?}");
    let snapshot = session.snapshot().unwrap();
    assert_eq!(RenderSessionState::Failed, snapshot.state);
    assert_eq!(Some(error.clone()), snapshot.failure);
    assert!(!snapshot.has_target);
    assert_eq!(error, session.release_target().unwrap_err());

    wait(session.release());
    let (other, attached) = map.attach(gone_window(), AttachOptions::default()).unwrap();
    assert!(attached.wait().is_err());
    wait(other.release());
    wait(map.release());
}

#[test]
fn a_caller_driven_surface_session_fails_in_its_first_service() {
    let runtime = runtime();
    let map = map(&runtime);
    let options = AttachOptions { driver: Driver::CallerThread, ..AttachOptions::default() };
    let (session, attached) = map.attach(gone_window(), options).unwrap();
    let driver = session.take_driver_work().unwrap();
    assert_eq!(RenderSessionState::Attaching, session.snapshot().unwrap().state);
    driver.service().unwrap();
    assert!(attached.wait().is_err());
    assert_eq!(RenderSessionState::Failed, session.snapshot().unwrap().state);
    // A failed caller-driven session is abandoned when it is released.
    wait(session.release());
    assert_eq!(ErrorKind::InvalidState, driver.service().unwrap_err().kind);
    wait(map.release());
}

#[test]
fn an_offscreen_session_keeps_its_target() {
    let runtime = runtime();
    let map = map(&runtime);
    let (session, attached) =
        map.attach(RenderTarget::Offscreen { size: SIZE, device: None }, AttachOptions::default()).unwrap();
    attached.wait().unwrap();
    assert!(!session.capabilities().unwrap().presentation);
    assert_eq!(ErrorKind::Unsupported, session.release_target().unwrap_err().kind);
    let replaced = session.set_surface(SIZE, Surface::window(GoneWindow)).unwrap_err();
    assert_eq!(ErrorKind::Unsupported, replaced.kind);
    assert!(session.snapshot().unwrap().has_target);
    wait(session.release());
}

#[test]
fn attach_rejects_an_empty_surface() {
    let runtime = runtime();
    let map = map(&runtime);
    let target = RenderTarget::Surface { size: Size::new(0, 64), surface: Surface::window(GoneWindow), device: None };
    assert_eq!(ErrorKind::InvalidArgument, map.attach(target, AttachOptions::default()).unwrap_err().kind);
}
