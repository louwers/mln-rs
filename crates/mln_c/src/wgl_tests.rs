// OpenGL on Windows through WGL: a library device renders offscreen and
// presents to a Win32 window, whose pixel format it sets, and a device that
// wraps the host's context renders into the host's textures and presents to
// the host's device context. The host reads each frame back with GL itself.

use core::ffi::c_void;

use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::{GetDC, HDC, ReleaseDC};
use windows_sys::Win32::Graphics::OpenGL::{
    ChoosePixelFormat, GL_FRONT, GL_RGBA, GL_TEXTURE_2D, GL_UNSIGNED_BYTE, GetPixelFormat, HGLRC, PFD_DOUBLEBUFFER,
    PFD_DRAW_TO_WINDOW, PFD_MAIN_PLANE, PFD_SUPPORT_OPENGL, PFD_TYPE_RGBA, PIXELFORMATDESCRIPTOR, SetPixelFormat,
    glBindTexture, glDeleteTextures, glFinish, glGenTextures, glGetTexImage, glReadBuffer, glReadPixels, glTexImage2D,
    wglCreateContext, wglDeleteContext, wglGetProcAddress, wglMakeCurrent,
};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, MSG, PM_REMOVE, PeekMessageW, RegisterClassExW,
    SW_MINIMIZE, SW_SHOWNOACTIVATE, ShowWindow, TranslateMessage, WNDCLASSEXW, WS_POPUP,
};

use crate::acquired_frame::{
    mln_acquired_frame_get_open_gl_texture, mln_acquired_frame_release, mln_render_session_acquire_frame,
};
use crate::generated::types::{
    MLN_FRAME_DEMAND_FORCE, MLN_GPU_DEVICE_OPTION_BACKEND, MLN_GPU_DEVICE_OPTION_OPEN_GL_CLIENT_API,
    MLN_GPU_SYNC_CPU_COMPLETE, MLN_GPU_SYNC_KIND_FLAG_CPU_COMPLETE, MLN_OPEN_GL_CLIENT_API_GL,
    MLN_OPEN_GL_CLIENT_API_GLES, MLN_OPEN_GL_CONTEXT_PLATFORM_WGL, MLN_OPEN_GL_CONTEXT_PROVIDER_FLAG_WGL,
    MLN_RENDER_BACKEND_OPEN_GL, MLN_RENDER_RESULT_RENDERED, MLN_RENDER_RESULT_TARGET_NOT_READY,
    MLN_RENDER_SESSION_CAPABILITY_FRAME_ACQUISITION, MLN_RENDER_SESSION_CAPABILITY_PRESENTATION,
    MLN_RENDER_TARGET_KIND_FLAG_OFFSCREEN, MLN_RENDER_TARGET_KIND_FLAG_OPEN_GL_BORROWED_TEXTURES,
    MLN_RENDER_TARGET_KIND_FLAG_OPEN_GL_SURFACE, MLN_RENDER_TARGET_KIND_FLAG_WIN32_WINDOW,
    MLN_RENDER_TARGET_KIND_OPEN_GL_BORROWED_TEXTURES, MLN_RENDER_TARGET_KIND_OPEN_GL_SURFACE,
    MLN_RENDER_TARGET_KIND_WIN32_WINDOW, MLN_STATUS_OK, MLN_STATUS_UNSUPPORTED, mln_diagnostic, mln_gpu_device,
    mln_gpu_device_capabilities, mln_gpu_device_options, mln_gpu_sync, mln_logical_size, mln_map,
    mln_open_gl_borrowed_texture, mln_open_gl_borrowed_textures_target, mln_open_gl_context_descriptor,
    mln_open_gl_context_descriptor_data, mln_open_gl_surface_target, mln_open_gl_texture_frame, mln_render_session,
    mln_render_session_capabilities, mln_render_target, mln_render_target_data, mln_wgl_context_descriptor,
    mln_win32_window_target,
};
use crate::gpu_device::{
    mln_gpu_device_create, mln_gpu_device_get_capabilities, mln_gpu_device_release, mln_gpu_device_wrap_open_gl,
};
use crate::map::mln_map_set_style_json;
use crate::map::tests::{create_map, options, release_map, view};
use crate::render_session::mln_render_session_get_capabilities;
use crate::render_session::tests::{
    attach_options, demand, offscreen_target, read_pixel, release_session, sized, wait_for_result,
};
use crate::render_target::{mln_map_attach, mln_open_gl_supported_context_provider_mask};
use crate::runtime::tests::{Waiter, create, release};

const NO_DIAGNOSTIC: *mut mln_diagnostic = core::ptr::null_mut();
const RED: [u8; 4] = [255, 0, 0, 255];
const WIDTH: u32 = 64;
const HEIGHT: u32 = 48;

/// `"mln_c_test"`, as UTF-16.
const CLASS: &[u16] = &[109, 108, 110, 95, 99, 95, 116, 101, 115, 116, 0];

unsafe extern "system" fn window_procedure(window: HWND, message: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    // SAFETY: the system's arguments.
    unsafe { DefWindowProcW(window, message, w, l) }
}

/// A shown window of the host's, `WIDTH` by `HEIGHT` pixels.
struct HostWindow(HWND);

impl HostWindow {
    fn new() -> HostWindow {
        // SAFETY: the test's executable.
        let instance = unsafe { GetModuleHandleW(core::ptr::null()) };
        let class = WNDCLASSEXW {
            cbSize: size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(window_procedure),
            hInstance: instance,
            lpszClassName: CLASS.as_ptr(),
            ..WNDCLASSEXW::default()
        };
        // SAFETY: a valid class, which may already be registered.
        unsafe { RegisterClassExW(&raw const class) };
        // SAFETY: the class is registered.
        let window = unsafe {
            CreateWindowExW(
                0,
                CLASS.as_ptr(),
                CLASS.as_ptr(),
                WS_POPUP,
                0,
                0,
                WIDTH as i32,
                HEIGHT as i32,
                core::ptr::null_mut(),
                core::ptr::null_mut(),
                instance,
                core::ptr::null(),
            )
        };
        assert!(!window.is_null(), "a window");
        // SAFETY: the live window.
        unsafe { ShowWindow(window, SW_SHOWNOACTIVATE) };
        pump();
        HostWindow(window)
    }
}

impl Drop for HostWindow {
    fn drop(&mut self) {
        // SAFETY: the test's window, made on this thread.
        unsafe { DestroyWindow(self.0) };
        pump();
    }
}

/// Handles the thread's window messages.
fn pump() {
    // SAFETY: a local message.
    unsafe {
        let mut message: MSG = core::mem::zeroed();
        while PeekMessageW(&raw mut message, core::ptr::null_mut(), 0, 0, PM_REMOVE) != 0 {
            TranslateMessage(&raw const message);
            DispatchMessageW(&raw const message);
        }
    }
}

/// A context of the host's on `window`, whose pixel format it sets if the
/// window has none.
struct HostContext {
    window: HWND,
    dc: HDC,
    context: HGLRC,
}

impl HostContext {
    fn new(window: HWND) -> HostContext {
        // SAFETY: the live window.
        let dc = unsafe { GetDC(window) };
        // SAFETY: the window's device context.
        if unsafe { GetPixelFormat(dc) } == 0 {
            let descriptor = PIXELFORMATDESCRIPTOR {
                nSize: size_of::<PIXELFORMATDESCRIPTOR>() as u16,
                nVersion: 1,
                dwFlags: PFD_DRAW_TO_WINDOW | PFD_SUPPORT_OPENGL | PFD_DOUBLEBUFFER,
                iPixelType: PFD_TYPE_RGBA,
                cColorBits: 32,
                cAlphaBits: 8,
                cDepthBits: 24,
                cStencilBits: 8,
                iLayerType: PFD_MAIN_PLANE as u8,
                ..PIXELFORMATDESCRIPTOR::default()
            };
            // SAFETY: as above, with a valid descriptor.
            unsafe {
                assert_ne!(SetPixelFormat(dc, ChoosePixelFormat(dc, &raw const descriptor), &raw const descriptor), 0);
            };
        }
        // SAFETY: as above, with a pixel format.
        let context = unsafe { wglCreateContext(dc) };
        assert!(!context.is_null(), "a WGL context");
        HostContext { window, dc, context }
    }

    /// Runs `f` with the context current.
    fn with<T>(&self, f: impl FnOnce() -> T) -> T {
        // SAFETY: the context, on the device context it was made for.
        assert_ne!(unsafe { wglMakeCurrent(self.dc, self.context) }, 0);
        let result = f();
        // SAFETY: the context is current.
        unsafe { wglMakeCurrent(core::ptr::null_mut(), core::ptr::null_mut()) };
        result
    }

    /// The pixel at the center of the window's front buffer.
    fn front_center(&self) -> [u8; 4] {
        self.with(|| {
            let mut pixel = [0u8; 4];
            // SAFETY: the context is current, and the pixel holds one.
            unsafe {
                glReadBuffer(GL_FRONT);
                glReadPixels(
                    WIDTH as i32 / 2,
                    HEIGHT as i32 / 2,
                    1,
                    1,
                    GL_RGBA,
                    GL_UNSIGNED_BYTE,
                    pixel.as_mut_ptr().cast(),
                );
            }
            pixel
        })
    }
}

impl Drop for HostContext {
    fn drop(&mut self) {
        // SAFETY: the context is current nowhere, and the device context is
        // this one's, got on this thread.
        unsafe {
            wglDeleteContext(self.context);
            ReleaseDC(self.window, self.dc);
        }
    }
}

fn red_map(runtime: crate::generated::types::mln_runtime) -> mln_map {
    let map = create_map(runtime, &options(1.0));
    let styled = Waiter::new();
    let style =
        r#"{"version":8,"sources":{},"layers":[{"id":"bg","type":"background","paint":{"background-color":"red"}}]}"#;
    // SAFETY: local arguments.
    unsafe { mln_map_set_style_json(map, view(style), &raw const styled.descriptor, NO_DIAGNOSTIC) };
    assert_eq!(styled.status(), MLN_STATUS_OK);
    map
}

fn attach(map: mln_map, target: &mln_render_target, device: mln_gpu_device) -> mln_render_session {
    let options = crate::generated::types::mln_render_session_attach_options {
        producer_sync_kinds: MLN_GPU_SYNC_KIND_FLAG_CPU_COMPLETE,
        ..attach_options()
    };
    let (mut session, attached) = (0, Waiter::new());
    // SAFETY: local arguments.
    let status = unsafe {
        mln_map_attach(
            map,
            target,
            device,
            &raw const options,
            &raw mut session,
            &raw const attached.descriptor,
            NO_DIAGNOSTIC,
        )
    };
    assert_eq!((status, attached.status()), (MLN_STATUS_OK, MLN_STATUS_OK));
    session
}

fn capabilities(session: mln_render_session) -> mln_render_session_capabilities {
    let mut capabilities = sized::<mln_render_session_capabilities>();
    // SAFETY: local capabilities.
    let status = unsafe { mln_render_session_get_capabilities(session, &raw mut capabilities, NO_DIAGNOSTIC) };
    assert_eq!(status, MLN_STATUS_OK);
    capabilities
}

fn device_capabilities(device: mln_gpu_device) -> mln_gpu_device_capabilities {
    let mut capabilities = sized::<mln_gpu_device_capabilities>();
    // SAFETY: local capabilities.
    let status = unsafe { mln_gpu_device_get_capabilities(device, &raw mut capabilities, NO_DIAGNOSTIC) };
    assert_eq!(status, MLN_STATUS_OK);
    capabilities
}

fn rendered(session: mln_render_session, token: u64) -> u32 {
    assert_eq!(demand(session, MLN_FRAME_DEMAND_FORCE, token), MLN_STATUS_OK);
    let disposition = wait_for_result(session).disposition;
    pump();
    disposition
}

/// The center of the window's front buffer once a frame of the session's
/// shows there: the system maps a new window after a while, and a frame
/// that presents before shows nowhere.
fn presented(session: mln_render_session, host: &HostContext, token: u64) -> [u8; 4] {
    for token in token..token + 50 {
        let pixel = host.front_center();
        if pixel == RED {
            return pixel;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
        assert_eq!(rendered(session, token), MLN_RENDER_RESULT_RENDERED);
    }
    host.front_center()
}

/// A library device of OpenGL with `client_api`, or the status of its
/// creation.
fn gl_device(client_api: Option<u32>) -> Result<mln_gpu_device, crate::generated::types::mln_status> {
    let mut options = mln_gpu_device_options {
        fields: MLN_GPU_DEVICE_OPTION_BACKEND,
        backend: MLN_RENDER_BACKEND_OPEN_GL,
        ..sized()
    };
    if let Some(client_api) = client_api {
        options.fields |= MLN_GPU_DEVICE_OPTION_OPEN_GL_CLIENT_API;
        options.open_gl_client_api = client_api;
    }
    let mut device = 0;
    // SAFETY: local arguments.
    match unsafe { mln_gpu_device_create(&raw const options, &raw mut device, NO_DIAGNOSTIC) } {
        MLN_STATUS_OK => Ok(device),
        status => Err(status),
    }
}

fn window_target(window: HWND) -> mln_render_target {
    let data = mln_win32_window_target {
        logical_size: mln_logical_size { width: WIDTH, height: HEIGHT },
        window: window.cast(),
    };
    mln_render_target {
        kind: MLN_RENDER_TARGET_KIND_WIN32_WINDOW,
        data: mln_render_target_data { win32_window: data },
        ..sized()
    }
}

// WGL is the provider here, and a library device renders desktop OpenGL by
// default, and no OpenGL ES.
#[test]
fn a_wgl_device_renders_desktop_open_gl_offscreen() {
    assert_eq!(mln_open_gl_supported_context_provider_mask(), MLN_OPEN_GL_CONTEXT_PROVIDER_FLAG_WGL);
    assert_eq!(gl_device(Some(MLN_OPEN_GL_CLIENT_API_GLES)), Err(MLN_STATUS_UNSUPPORTED));
    let explicit = gl_device(Some(MLN_OPEN_GL_CLIENT_API_GL)).expect("a desktop OpenGL device");
    mln_gpu_device_release(explicit);
    let device = gl_device(None).expect("a WGL device");
    let capabilities = device_capabilities(device);
    assert_eq!(capabilities.backend, MLN_RENDER_BACKEND_OPEN_GL);
    let kinds = MLN_RENDER_TARGET_KIND_FLAG_OFFSCREEN | MLN_RENDER_TARGET_KIND_FLAG_WIN32_WINDOW;
    assert_eq!(capabilities.target_kinds & kinds, kinds);

    let runtime = create();
    let map = red_map(runtime);
    let session = attach(map, &offscreen_target(WIDTH, HEIGHT), device);
    assert_eq!(rendered(session, 1), MLN_RENDER_RESULT_RENDERED);
    assert_eq!(read_pixel(session), (MLN_STATUS_OK, Some((WIDTH, HEIGHT, RED))));
    release_session(session);
    mln_gpu_device_release(device);
    release_map(map);
    release(runtime);
}

// A Win32 window target on a WGL device sets the window's pixel format and
// presents to it, which the host reads back, and finishes its frames
// unrendered while the window is minimized.
#[test]
fn a_win32_window_target_presents_on_a_wgl_device() {
    let device = gl_device(None).expect("a WGL device");
    let runtime = create();
    let map = red_map(runtime);
    let window = HostWindow::new();
    let session = attach(map, &window_target(window.0), device);
    assert_eq!(capabilities(session).flags, MLN_RENDER_SESSION_CAPABILITY_PRESENTATION);
    assert_eq!(rendered(session, 1), MLN_RENDER_RESULT_RENDERED);
    // The host's context reads the window's front buffer, in the pixel
    // format that the session set.
    let host = HostContext::new(window.0);
    // SAFETY: the window's device context.
    assert_ne!(unsafe { GetPixelFormat(host.dc) }, 0);
    assert_eq!(presented(session, &host, 10), RED);

    // SAFETY: the live window.
    unsafe { ShowWindow(window.0, SW_MINIMIZE) };
    pump();
    assert_eq!(rendered(session, 2), MLN_RENDER_RESULT_TARGET_NOT_READY);
    // SAFETY: as above.
    unsafe { ShowWindow(window.0, SW_SHOWNOACTIVATE) };
    pump();
    assert_eq!(rendered(session, 3), MLN_RENDER_RESULT_RENDERED);
    release_session(session);
    drop(host);
    drop(window);
    mln_gpu_device_release(device);
    release_map(map);
    release(runtime);
}

// A device that wraps the host's WGL context renders into the host's
// textures, which the host reads, and presents to the host's device
// context.
#[test]
fn a_wrapped_wgl_context_renders_into_host_textures_and_surfaces() {
    let window = HostWindow::new();
    let host = HostContext::new(window.0);
    let descriptor = mln_open_gl_context_descriptor {
        platform: MLN_OPEN_GL_CONTEXT_PLATFORM_WGL,
        data: mln_open_gl_context_descriptor_data {
            wgl: mln_wgl_context_descriptor {
                device_context: host.dc,
                share_context: host.context,
                get_proc_address: wglGetProcAddress as *mut c_void,
            },
        },
    };
    let mut device = 0;
    // SAFETY: local arguments, and the host's live context.
    let status = unsafe { mln_gpu_device_wrap_open_gl(&raw const descriptor, &raw mut device, NO_DIAGNOSTIC) };
    assert_eq!(status, MLN_STATUS_OK);
    let kinds = MLN_RENDER_TARGET_KIND_FLAG_OPEN_GL_SURFACE | MLN_RENDER_TARGET_KIND_FLAG_OPEN_GL_BORROWED_TEXTURES;
    assert_eq!(device_capabilities(device).target_kinds & kinds, kinds);
    let runtime = create();
    let map = red_map(runtime);

    // Two textures of the host's, which the session renders into.
    let mut textures = [0u32; 2];
    host.with(|| {
        // SAFETY: the context is current.
        unsafe {
            glGenTextures(2, textures.as_mut_ptr());
            for &texture in &textures {
                glBindTexture(GL_TEXTURE_2D, texture);
                // GL_RGBA8.
                glTexImage2D(
                    GL_TEXTURE_2D,
                    0,
                    0x8058,
                    WIDTH as i32,
                    HEIGHT as i32,
                    0,
                    GL_RGBA,
                    GL_UNSIGNED_BYTE,
                    core::ptr::null(),
                );
            }
            glBindTexture(GL_TEXTURE_2D, 0);
            glFinish();
        }
    });
    let borrowed = textures.map(|texture| mln_open_gl_borrowed_texture { texture });
    let ring = mln_open_gl_borrowed_textures_target {
        logical_size: mln_logical_size { width: WIDTH, height: HEIGHT },
        physical_width: WIDTH,
        physical_height: HEIGHT,
        textures: borrowed.as_ptr(),
        texture_count: borrowed.len(),
        target: GL_TEXTURE_2D,
    };
    let target = mln_render_target {
        kind: MLN_RENDER_TARGET_KIND_OPEN_GL_BORROWED_TEXTURES,
        data: mln_render_target_data { open_gl_borrowed_textures: ring },
        ..sized()
    };
    let session = attach(map, &target, device);
    assert_eq!(capabilities(session).flags, MLN_RENDER_SESSION_CAPABILITY_FRAME_ACQUISITION);
    for token in 1..=2 {
        assert_eq!(rendered(session, token), MLN_RENDER_RESULT_RENDERED);
        let mut frame = 0;
        // SAFETY: a local output.
        assert_eq!(unsafe { mln_render_session_acquire_frame(session, &raw mut frame, NO_DIAGNOSTIC) }, MLN_STATUS_OK);
        let mut texture = sized::<mln_open_gl_texture_frame>();
        // SAFETY: as above.
        let status = unsafe { mln_acquired_frame_get_open_gl_texture(frame, &raw mut texture, NO_DIAGNOSTIC) };
        assert_eq!(status, MLN_STATUS_OK);
        assert_eq!((texture.width, texture.height, texture.target), (WIDTH, HEIGHT, GL_TEXTURE_2D));
        let pixels = host.with(|| {
            let mut pixels = vec![0u8; (WIDTH * HEIGHT * 4) as usize];
            // SAFETY: the context is current, and the buffer holds the
            // texture.
            unsafe {
                glBindTexture(GL_TEXTURE_2D, texture.texture);
                glGetTexImage(GL_TEXTURE_2D, 0, GL_RGBA, GL_UNSIGNED_BYTE, pixels.as_mut_ptr().cast());
                glBindTexture(GL_TEXTURE_2D, 0);
            }
            pixels
        });
        let center = ((HEIGHT / 2 * WIDTH + WIDTH / 2) * 4) as usize;
        assert_eq!(pixels[center..center + 4], RED, "frame {token}");
        let done = mln_gpu_sync { kind: MLN_GPU_SYNC_CPU_COMPLETE, ..sized() };
        // SAFETY: a local sync.
        assert_eq!(unsafe { mln_acquired_frame_release(frame, &raw const done, NO_DIAGNOSTIC) }, MLN_STATUS_OK);
    }
    release_session(session);

    // The host's device context, which the session presents to.
    let surface = mln_open_gl_surface_target {
        logical_size: mln_logical_size { width: WIDTH, height: HEIGHT },
        surface: host.dc,
    };
    let target = mln_render_target {
        kind: MLN_RENDER_TARGET_KIND_OPEN_GL_SURFACE,
        data: mln_render_target_data { open_gl_surface: surface },
        ..sized()
    };
    let session = attach(map, &target, device);
    assert_eq!(capabilities(session).flags, MLN_RENDER_SESSION_CAPABILITY_PRESENTATION);
    assert_eq!(rendered(session, 3), MLN_RENDER_RESULT_RENDERED);
    assert_eq!(presented(session, &host, 4), RED);
    release_session(session);

    host.with(|| {
        // SAFETY: the context is current, and the textures are its.
        unsafe { glDeleteTextures(2, textures.as_ptr()) };
    });
    mln_gpu_device_release(device);
    release_map(map);
    release(runtime);
    drop(host);
    drop(window);
}
