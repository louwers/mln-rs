// Surface sessions that present to Win32 windows, which take no session and
// no frame while they are minimized. Each window has a thread of its own
// that processes its messages, as a host's window does. A backend without a
// device that presents to Win32 windows is skipped.

#![cfg(windows)]
#![expect(unsafe_code, reason = "the tests make Win32 windows to present to")]

mod support;

use std::sync::{Arc, mpsc};
use std::thread::JoinHandle;
use std::time::Instant;

use mln_gfx::window::raw_window_handle::{
    DisplayHandle, HandleError, HasDisplayHandle, HasWindowHandle, RawDisplayHandle, RawWindowHandle,
    Win32WindowHandle, WindowHandle, WindowsDisplayHandle,
};
use mln_map::MapMode;
use mln_runtime::{
    AttachOptions, BackendType, DemandMode, Device, DeviceOptions, ErrorKind, FrameDemand, FrameDisposition,
    FrameResult, Map, MapOptions, RenderSession, RenderTarget, Runtime, Surface,
};
use mln_util::size::Size;
use support::{TIMEOUT, eventually, runtime, wait};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DispatchMessageW, GetMessageW, IsIconic, MSG, PostMessageW, PostQuitMessage,
    RegisterClassW, SW_SHOWMINNOACTIVE, SW_SHOWNOACTIVATE, ShowWindow, TranslateMessage, WM_CLOSE, WM_DESTROY,
    WNDCLASSW, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_POPUP,
};
use windows::core::w;

const SIZE: Size = Size { width: 64, height: 64 };

const STYLE: &str =
    r##"{"version":8,"sources":{},"layers":[{"id":"bg","type":"background","paint":{"background-color":"#00ff00"}}]}"##;

/// A shown pop-up window, whose client area is the whole window, on a
/// thread that processes its messages until the window closes on drop.
struct TestWindow {
    hwnd: isize,
    thread: Option<JoinHandle<()>>,
}

extern "system" fn window_procedure(window: HWND, message: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if message == WM_DESTROY {
        // SAFETY: the window's own thread ends its message loop.
        unsafe { PostQuitMessage(0) };
        return LRESULT(0);
    }
    // SAFETY: the window's message, which the default procedure handles.
    unsafe { DefWindowProcW(window, message, wparam, lparam) }
}

impl TestWindow {
    fn new() -> Arc<TestWindow> {
        let (created, made) = mpsc::channel();
        let thread = std::thread::spawn(move || {
            // SAFETY: the window's thread makes and shows it, and processes
            // its messages until it is destroyed.
            unsafe {
                let instance = GetModuleHandleW(None).expect("the module").into();
                let class = WNDCLASSW {
                    lpfnWndProc: Some(window_procedure),
                    hInstance: instance,
                    lpszClassName: w!("MlnRuntimeTestWin32Window"),
                    ..WNDCLASSW::default()
                };
                // A second registration fails, and keeps the first.
                RegisterClassW(&raw const class);
                let window = CreateWindowExW(
                    WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW,
                    class.lpszClassName,
                    class.lpszClassName,
                    WS_POPUP,
                    0,
                    0,
                    SIZE.width as i32,
                    SIZE.height as i32,
                    None,
                    None,
                    Some(instance),
                    None,
                )
                .expect("a window");
                let _ = ShowWindow(window, SW_SHOWNOACTIVATE);
                created.send(window.0 as isize).unwrap();
                let mut message = MSG::default();
                while GetMessageW(&raw mut message, None, 0, 0).0 > 0 {
                    let _ = TranslateMessage(&raw const message);
                    DispatchMessageW(&raw const message);
                }
            }
        });
        let hwnd = made.recv().expect("the window's thread makes it");
        Arc::new(TestWindow { hwnd, thread: Some(thread) })
    }

    fn handle(&self) -> HWND {
        HWND(self.hwnd as *mut core::ffi::c_void)
    }

    fn is_minimized(&self) -> bool {
        // SAFETY: a query of the live window.
        unsafe { IsIconic(self.handle()) }.as_bool()
    }

    /// Minimizes or restores the window, and waits until it is.
    fn set_minimized(&self, minimized: bool) {
        let command = if minimized { SW_SHOWMINNOACTIVE } else { SW_SHOWNOACTIVATE };
        // SAFETY: the live window, whose thread handles the messages.
        let _ = unsafe { ShowWindow(self.handle(), command) };
        assert!(eventually(|| self.is_minimized() == minimized), "the window minimized: {minimized}");
    }
}

impl Drop for TestWindow {
    fn drop(&mut self) {
        // SAFETY: the window closes, which ends its thread.
        let _ = unsafe { PostMessageW(Some(self.handle()), WM_CLOSE, WPARAM(0), LPARAM(0)) };
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl HasWindowHandle for TestWindow {
    fn window_handle(&self) -> Result<WindowHandle<'_>, HandleError> {
        let hwnd = core::num::NonZeroIsize::new(self.hwnd).ok_or(HandleError::Unavailable)?;
        // SAFETY: the window lives as long as `self`.
        Ok(unsafe { WindowHandle::borrow_raw(RawWindowHandle::Win32(Win32WindowHandle::new(hwnd))) })
    }
}

impl HasDisplayHandle for TestWindow {
    fn display_handle(&self) -> Result<DisplayHandle<'_>, HandleError> {
        // SAFETY: Windows' display handle names nothing.
        Ok(unsafe { DisplayHandle::borrow_raw(RawDisplayHandle::Windows(WindowsDisplayHandle::new())) })
    }
}

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

/// Demands a forced frame, and takes its result.
fn frame(session: &RenderSession, token: u64) -> FrameResult {
    session.demand_frame(FrameDemand::new(DemandMode::Force, token)).unwrap();
    let mut results = Vec::new();
    let deadline = Instant::now() + TIMEOUT;
    while results.is_empty() {
        assert!(Instant::now() < deadline, "no frame result");
        results.extend(session.drain_frame_results().unwrap());
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    assert_eq!(1, results.len(), "{results:?}");
    assert_eq!(Some(token), results[0].token);
    results[0]
}

fn device_of(backend: BackendType) -> Option<Device> {
    match Device::with_backend(backend, DeviceOptions::default()) {
        Ok(device) if device.presents_to(mln_runtime::SurfaceType::Win32) => Some(device),
        Ok(_) => {
            eprintln!("skipped: the {backend:?} device presents to no Win32 window");
            None
        }
        Err(error) => {
            eprintln!("skipped: no {backend:?} device: {error}");
            None
        }
    }
}

fn surface(window: &Arc<TestWindow>) -> Surface {
    Surface::window(window.clone())
}

// A minimized window takes no session and no replacement, and a session's
// window takes no frame while it is minimized, and the next one once it is
// restored.
#[test]
fn a_minimized_window_takes_no_session_and_no_frames_on_each_backend() {
    let runtime = runtime();
    let map = map(&runtime);
    for backend in [BackendType::Vulkan, BackendType::WebGPU] {
        let Some(device) = device_of(backend) else { continue };
        let (minimized, shown) = (TestWindow::new(), TestWindow::new());
        minimized.set_minimized(true);
        let target = RenderTarget::Surface { size: SIZE, surface: surface(&minimized), device: Some(device.clone()) };
        let refused = map.attach(target, AttachOptions::default()).unwrap_err();
        assert_eq!(ErrorKind::InvalidState, refused.kind, "{backend:?}: {refused:?}");

        let target = RenderTarget::Surface { size: SIZE, surface: surface(&shown), device: Some(device) };
        let (session, attached) = map.attach(target, AttachOptions::default()).unwrap();
        attached.wait().unwrap();
        assert_eq!(FrameDisposition::Rendered, frame(&session, 1).disposition, "{backend:?}");
        let refused = session.set_surface(SIZE, surface(&minimized)).unwrap_err();
        assert_eq!(ErrorKind::InvalidState, refused.kind, "{backend:?}: {refused:?}");

        shown.set_minimized(true);
        assert_eq!(FrameDisposition::TargetNotReady, frame(&session, 2).disposition, "{backend:?}");
        assert_eq!(FrameDisposition::TargetNotReady, frame(&session, 3).disposition, "{backend:?}");
        shown.set_minimized(false);
        assert_eq!(FrameDisposition::Rendered, frame(&session, 4).disposition, "{backend:?}");
        session.release().unwrap().wait().unwrap();
    }
}

/// Draws a frame that clears `target`, a Vulkan target on `backend`, as a
/// renderer would: where it went, or `None` when the target took no frame.
#[cfg(feature = "vulkan")]
fn vulkan_frame(backend: &mln_vulkan::RendererBackend, target: &mln_vulkan::SurfaceTarget) -> Option<mln_gfx::Handoff> {
    use mln_gfx::FrameTarget as _;
    use mln_gfx::render_pass::RenderPassDescriptor;
    use mln_gfx::renderer_backend::RendererBackend as _;

    let _scope = mln_gfx::BackendScope::new(backend, mln_gfx::ScopeType::Explicit);
    let image = target.begin_frame()?;
    let context = backend.get_context();
    context.begin_frame();
    let encoder = context.create_command_encoder();
    let descriptor = RenderPassDescriptor {
        target: &*image,
        clear_color: Some(mln_util::Color::new(1.0, 0.0, 0.0, 1.0)),
        clear_depth: Some(1.0),
        clear_stencil: Some(0),
    };
    drop(encoder.create_render_pass("frame", &descriptor));
    let handoff = target.end_frame(&*encoder);
    context.end_frame();
    Some(handoff)
}

// A Vulkan target finds a minimized window out through its surface alone,
// without the window's state: no target is made on a surface of an empty
// extent, and a target whose window is minimized finds its swapchain out of
// date, can't make it again, and takes no frame until the window is restored.
// The driver may first report the swapchain as suboptimal, as Wine does,
// which has the target rebuild it a few frames later, so some frames may
// present before the target finds out.
#[cfg(feature = "vulkan")]
#[test]
fn a_vulkan_target_takes_no_frame_once_its_window_is_minimized() {
    use mln_gfx::Handoff;

    let Some(Device::Vulkan(device)) = device_of(BackendType::Vulkan) else { return };
    let window = TestWindow::new();
    let (backend, target) = mln_vulkan::SurfaceTarget::with_backend(&device, window.clone(), SIZE, true).unwrap();
    assert_eq!(Some(Handoff::Presented), vulkan_frame(&backend, &target));
    window.set_minimized(true);
    let refused = mln_vulkan::SurfaceTarget::new(backend.clone(), window.clone(), SIZE, true).err().unwrap();
    assert_eq!(ErrorKind::InvalidState, refused.kind, "{refused:?}");
    let frames: Vec<_> = (0..12).map(|_| vulkan_frame(&backend, &target)).collect();
    let presented = frames.iter().take_while(|frame| **frame == Some(Handoff::Presented)).count();
    let rest = &frames[presented..];
    assert!(
        !rest.is_empty() && rest.iter().skip_while(|frame| **frame == Some(Handoff::Dropped)).all(Option::is_none),
        "the frames to the minimized window: {frames:?}"
    );
    assert_eq!(None, *frames.last().unwrap(), "the frames to the minimized window: {frames:?}");
    window.set_minimized(false);
    assert_eq!(Some(Handoff::Presented), vulkan_frame(&backend, &target));
    drop(target);
    drop(backend);
}
