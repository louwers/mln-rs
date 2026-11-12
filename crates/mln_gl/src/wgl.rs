// WGL, Windows' OpenGL: the desktop OpenGL contexts that share groups and
// window targets render with on Windows, where EGL is no system library.
//
// A context is made with wglCreateContextAttribsARB, as a core profile of
// OpenGL 3.3 or later, and shares its objects with the group's root. A
// context is current on a device context of the pixel format it was made
// with: each renderer's on a hidden window of its own, made on its thread
// (only that thread may destroy the window), and a window target's on the
// host's window, whose pixel format the target sets.

use std::any::Any;
use std::cell::Cell;
use std::ffi::{CString, c_void};
use std::sync::OnceLock;

use mln_util::error::Error;
use mln_util::logging::{Event, Log};
use windows_sys::Win32::Foundation::{GetLastError, HWND};
use windows_sys::Win32::Graphics::Gdi::{GetDC, HDC, ReleaseDC};
use windows_sys::Win32::Graphics::OpenGL::{
    ChoosePixelFormat, DescribePixelFormat, GetPixelFormat, HGLRC, PFD_DOUBLEBUFFER, PFD_DRAW_TO_WINDOW,
    PFD_MAIN_PLANE, PFD_SUPPORT_OPENGL, PFD_TYPE_RGBA, PIXELFORMATDESCRIPTOR, SetPixelFormat, SwapBuffers,
    wglCreateContext, wglDeleteContext, wglGetCurrentContext, wglGetCurrentDC, wglGetProcAddress, wglMakeCurrent,
};
use windows_sys::Win32::System::LibraryLoader::{
    GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS, GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT, GetModuleHandleExW,
    GetProcAddress, LoadLibraryW,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CS_OWNDC, CreateWindowExW, DefWindowProcW, DestroyWindow, RegisterClassExW, WNDCLASSEXW, WS_POPUP,
};

use crate::renderer_backend::PlatformContext;
use crate::types::Gl;

/// `WGL_ARB_create_context` and `WGL_ARB_create_context_profile`.
const CONTEXT_MAJOR_VERSION: i32 = 0x2091;
const CONTEXT_MINOR_VERSION: i32 = 0x2092;
const CONTEXT_PROFILE_MASK: i32 = 0x9126;
const CONTEXT_CORE_PROFILE_BIT: i32 = 0x1;

type CreateContextAttribs = unsafe extern "system" fn(HDC, HGLRC, *const i32) -> HGLRC;
type SwapInterval = unsafe extern "system" fn(i32) -> i32;

/// A host's `wglGetProcAddress`.
pub(crate) type Loader = unsafe extern "system" fn(*const u8) -> *const c_void;

/// The last Win32 error, after what failed.
fn last_error(what: &str) -> String {
    // SAFETY: a plain query of the thread's error.
    format!("{what} failed ({})", unsafe { GetLastError() })
}

/// The pixel format that the library's contexts render with: RGBA8, with a
/// depth and a stencil buffer, double-buffered for windows.
fn descriptor() -> PIXELFORMATDESCRIPTOR {
    PIXELFORMATDESCRIPTOR {
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
    }
}

/// The pixel format `format` of `dc`, or the library's when it is 0.
fn choose_pixel_format(dc: HDC, format: i32) -> Result<i32, String> {
    if format != 0 {
        return Ok(format);
    }
    let descriptor = descriptor();
    // SAFETY: a live device context and a valid descriptor.
    match unsafe { ChoosePixelFormat(dc, &raw const descriptor) } {
        0 => Err(last_error("ChoosePixelFormat")),
        format => Ok(format),
    }
}

/// Gives `dc`, which has no pixel format yet, the pixel format `format`, or
/// the library's when it is 0. Returns the format.
fn set_pixel_format(dc: HDC, format: i32) -> Result<i32, String> {
    let format = choose_pixel_format(dc, format)?;
    let mut chosen = descriptor();
    // SAFETY: a live device context, and the format's own descriptor.
    unsafe {
        DescribePixelFormat(dc, format, size_of::<PIXELFORMATDESCRIPTOR>() as u32, &raw mut chosen);
        if SetPixelFormat(dc, format, &raw const chosen) == 0 {
            return Err(last_error("SetPixelFormat"));
        }
    }
    Ok(format)
}

/// The module of this code, which owns the hidden windows' class whether
/// the library is a DLL or linked into the host's executable.
fn module() -> *mut c_void {
    let mut module = std::ptr::null_mut();
    // SAFETY: an address in this module, and a local output.
    unsafe {
        GetModuleHandleExW(
            GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS | GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
            (module as *const ()).cast::<u16>(),
            &raw mut module,
        );
    }
    module
}

/// `"mln_gl_hidden"`, as UTF-16.
const CLASS: &[u16] = &[
    b'm' as u16,
    b'l' as u16,
    b'n' as u16,
    b'_' as u16,
    b'g' as u16,
    b'l' as u16,
    b'_' as u16,
    b'h' as u16,
    b'i' as u16,
    b'd' as u16,
    b'd' as u16,
    b'e' as u16,
    b'n' as u16,
    0,
];

/// A hidden window, whose device context has a pixel format for contexts to
/// be current on. Only the thread that made it destroys it.
pub(crate) struct HiddenWindow {
    window: HWND,
    dc: HDC,
}

impl HiddenWindow {
    /// A window of the pixel format `format`, or the library's when it is 0,
    /// and the format.
    pub(crate) fn new(format: i32) -> Result<(HiddenWindow, i32), String> {
        static REGISTERED: OnceLock<bool> = OnceLock::new();
        let registered = *REGISTERED.get_or_init(|| {
            let class = WNDCLASSEXW {
                cbSize: size_of::<WNDCLASSEXW>() as u32,
                // The class's windows own their device contexts, which need
                // no release and last as long as the window.
                style: CS_OWNDC,
                lpfnWndProc: Some(DefWindowProcW),
                hInstance: module(),
                lpszClassName: CLASS.as_ptr(),
                ..WNDCLASSEXW::default()
            };
            // SAFETY: a valid class, which stays registered.
            unsafe { RegisterClassExW(&raw const class) != 0 }
        });
        if !registered {
            return Err(last_error("Registering the hidden window's class"));
        }
        // SAFETY: the class is registered, and the window is new.
        let window = unsafe {
            CreateWindowExW(
                0,
                CLASS.as_ptr(),
                CLASS.as_ptr(),
                WS_POPUP,
                0,
                0,
                8,
                8,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                module(),
                std::ptr::null(),
            )
        };
        if window.is_null() {
            return Err(last_error("CreateWindowExW"));
        }
        // SAFETY: the window is live, and its class owns its device context.
        let hidden = HiddenWindow { window, dc: unsafe { GetDC(window) } };
        let format = set_pixel_format(hidden.dc, format)?;
        Ok((hidden, format))
    }

    pub(crate) fn dc(&self) -> HDC {
        self.dc
    }
}

impl Drop for HiddenWindow {
    fn drop(&mut self) {
        // SAFETY: the window is this one's, made on this thread.
        unsafe { DestroyWindow(self.window) };
    }
}

/// The function that makes contexts, which a context must be current to
/// look up: found once, with a context of the library's on a hidden window.
fn create_context_attribs() -> Result<CreateContextAttribs, String> {
    static FUNCTION: OnceLock<Result<usize, String>> = OnceLock::new();
    let function = FUNCTION.get_or_init(|| {
        // SAFETY: plain queries of the thread's current context.
        let current = unsafe { (wglGetCurrentDC(), wglGetCurrentContext()) };
        let found = || -> Result<usize, String> {
            let lookup = || proc_address(None, "wglCreateContextAttribsARB").map(|function| function as usize);
            if !current.1.is_null()
                && let Some(function) = lookup()
            {
                return Ok(function);
            }
            let (window, _) = HiddenWindow::new(0)?;
            // SAFETY: the window's device context has a pixel format.
            let context = unsafe { wglCreateContext(window.dc()) };
            if context.is_null() {
                return Err(last_error("wglCreateContext"));
            }
            // SAFETY: the new context, on the window it was made for.
            let function = unsafe {
                let function = (wglMakeCurrent(window.dc(), context) != 0).then(lookup).flatten();
                wglMakeCurrent(current.0, current.1);
                wglDeleteContext(context);
                function
            };
            function.ok_or_else(|| "the OpenGL driver has no WGL_ARB_create_context".to_owned())
        };
        found()
    });
    let function = function.clone()?;
    // SAFETY: the address of the extension's function, which has its type.
    Ok(unsafe { std::mem::transmute::<usize, CreateContextAttribs>(function) })
}

/// A core profile context of OpenGL 3.3 or later, of the pixel format of
/// `dc`, that shares its objects with `share`, if any.
pub(crate) fn create_context(dc: HDC, share: HGLRC) -> Result<HGLRC, String> {
    let create = create_context_attribs()?;
    let attributes =
        [CONTEXT_MAJOR_VERSION, 3, CONTEXT_MINOR_VERSION, 3, CONTEXT_PROFILE_MASK, CONTEXT_CORE_PROFILE_BIT, 0];
    // SAFETY: a device context with a pixel format, a live context or none,
    // and attributes that end with 0.
    let context = unsafe { create(dc, share, attributes.as_ptr()) };
    if context.is_null() {
        return Err(last_error("Creating an OpenGL 3.3 core context with wglCreateContextAttribsARB"));
    }
    Ok(context)
}

/// A context of the pixel format of `dc` that shares its objects with the
/// host's `share`: of the core profile of OpenGL 3.3 or later, or where the
/// driver shares no such context with the host's, of the default profile and
/// version, which the caller checks.
fn create_shared_context(dc: HDC, share: HGLRC) -> Result<HGLRC, String> {
    create_context(dc, share).or_else(|error| {
        let create = create_context_attribs()?;
        let attributes = [0];
        // SAFETY: as in `create_context`.
        let context = unsafe { create(dc, share, attributes.as_ptr()) };
        if context.is_null() { Err(error) } else { Ok(context) }
    })
}

/// The root of a share group and how its contexts are made: the library's
/// root, which is never current, or the host's.
pub(crate) struct Group {
    root: HGLRC,
    /// Whether the root is the library's, which the group deletes.
    owns_root: bool,
    /// The pixel format of the root, which the group's contexts are made
    /// with.
    format: i32,
    /// The host's `wglGetProcAddress`.
    loader: Option<Loader>,
}

// SAFETY: the root is never made current by the group; WGL lets any thread
// make contexts that share with it, and delete it.
unsafe impl Send for Group {}
// SAFETY: as above.
unsafe impl Sync for Group {}

impl Group {
    /// The library's group: a root of the core profile of OpenGL 3.3 or
    /// later, of the library's pixel format.
    ///
    /// Fails with [`ErrorKind::Unsupported`](mln_util::ErrorKind) when the
    /// driver makes no such context, and with
    /// [`ErrorKind::Native`](mln_util::ErrorKind) when Windows makes no
    /// window for it.
    pub(crate) fn library() -> Result<Group, Error> {
        let (window, format) = HiddenWindow::new(0).map_err(Error::native)?;
        let root = create_context(window.dc(), std::ptr::null_mut()).map_err(Error::unsupported)?;
        Ok(Group { root, owns_root: true, format, loader: None })
    }

    /// The host's group, whose root is `context`, of the pixel format of
    /// `dc`, and whose functions `loader` gives.
    ///
    /// Fails with [`ErrorKind::InvalidArgument`](mln_util::ErrorKind) for a
    /// device context without a pixel format.
    ///
    /// # Safety
    ///
    /// `dc` is a live device context with the pixel format of `context`, a
    /// live context, which outlives the group, and `loader` is null or a
    /// `wglGetProcAddress`.
    pub(crate) unsafe fn host(dc: HDC, context: HGLRC, loader: *mut c_void) -> Result<Group, Error> {
        let format = pixel_format(dc);
        if format == 0 {
            return Err(Error::invalid_argument("the host's device context has no pixel format"));
        }
        // SAFETY: the caller's contract.
        let loader = (!loader.is_null()).then(|| unsafe { std::mem::transmute::<*mut c_void, Loader>(loader) });
        Ok(Group { root: context, owns_root: false, format, loader })
    }

    pub(crate) fn is_host(&self) -> bool {
        !self.owns_root
    }

    /// A context in the group, current on a hidden window of its own, made
    /// on this thread, around a backend's frames; or with `dc`, the host's
    /// device context of the group's pixel format, current on it. A context
    /// in the host's group makes what the thread had current again when it
    /// is done. `keep` is what the context keeps alive, the device.
    pub(crate) fn create_context(
        &self,
        dc: Option<HDC>,
        keep: Box<dyn Any>,
    ) -> Result<(Gl, Box<dyn PlatformContext>), Error> {
        let (window, dc) = if let Some(dc) = dc {
            (None, dc)
        } else {
            let (window, _) = HiddenWindow::new(self.format).map_err(Error::native)?;
            let dc = window.dc();
            (Some(window), dc)
        };
        let context = if self.owns_root {
            create_context(dc, self.root).map_err(Error::native)?
        } else {
            create_shared_context(dc, self.root)
                .map_err(|e| Error::native(format!("Error creating a WGL context that shares the host's: {e}")))?
        };
        let context = GroupContext {
            dc,
            context,
            _window: window,
            previous: Cell::new(None),
            restores: !self.owns_root,
            _keep: keep,
        };
        context.make_current();
        let gl = functions(self.loader);
        context.done_current();
        Ok((gl?, Box::new(context)))
    }
}

impl Drop for Group {
    fn drop(&mut self) {
        if self.owns_root {
            delete_context(self.root);
        }
    }
}

/// A context of a share group, and the hidden window it is current on, or
/// the host's device context.
struct GroupContext {
    dc: HDC,
    context: HGLRC,
    /// Before what it keeps: the window goes after the context.
    _window: Option<HiddenWindow>,
    /// What the host's thread had current, which the context makes current
    /// again when it is done.
    previous: Cell<Option<(HDC, HGLRC)>>,
    restores: bool,
    _keep: Box<dyn Any>,
}

impl PlatformContext for GroupContext {
    fn make_current(&self) {
        if self.restores {
            self.previous.set(current().filter(|(_, context)| *context != self.context));
        }
        if let Err(e) = make_current(self.dc, self.context) {
            Log::error(Event::GraphicsBackend, format!("Switching OpenGL context failed: {e}"));
        }
    }

    fn done_current(&self) {
        let (dc, context) = self.previous.take().unwrap_or((std::ptr::null_mut(), std::ptr::null_mut()));
        if let Err(e) = make_current(dc, context) {
            Log::error(Event::GraphicsBackend, format!("Removing OpenGL context failed: {e}"));
        }
    }
}

impl Drop for GroupContext {
    fn drop(&mut self) {
        if current().is_some_and(|(_, context)| context == self.context) {
            let _ = make_current(std::ptr::null_mut(), std::ptr::null_mut());
        }
        delete_context(self.context);
    }
}

/// Deletes `context`, which is current on no thread.
pub(crate) fn delete_context(context: HGLRC) {
    // SAFETY: the context is the caller's to delete.
    unsafe { wglDeleteContext(context) };
}

/// The function of a context current on this thread called `name`: from
/// `loader`, the host's, or WGL, or `opengl32.dll` for OpenGL 1.1's, which
/// WGL doesn't give.
pub(crate) fn proc_address(loader: Option<Loader>, name: &str) -> Option<*const c_void> {
    static OPENGL32: OnceLock<usize> = OnceLock::new();
    let name = CString::new(name).ok()?;
    // wglGetProcAddress returns 1, 2, 3 or -1 for some functions it lacks.
    let valid = |function: *const c_void| !function.is_null() && !matches!(function as isize, 1..=3 | -1);
    if let Some(loader) = loader {
        // SAFETY: the host's loader, of a null-terminated name.
        let function = unsafe { loader(name.as_ptr().cast()) };
        if valid(function) {
            return Some(function);
        }
    }
    // SAFETY: as above, of WGL's.
    if let Some(function) = unsafe { wglGetProcAddress(name.as_ptr().cast()) }.map(|function| function as *const c_void)
        && valid(function)
    {
        return Some(function);
    }
    let opengl32 = *OPENGL32.get_or_init(|| {
        let name: Vec<u16> = "opengl32.dll".encode_utf16().chain([0]).collect();
        // SAFETY: a system library, which stays loaded.
        unsafe { LoadLibraryW(name.as_ptr()) as usize }
    });
    if opengl32 == 0 {
        return None;
    }
    // SAFETY: the loaded library, and a null-terminated name.
    unsafe { GetProcAddress(opengl32 as *mut c_void, name.as_ptr().cast()) }.map(|function| function as *const c_void)
}

/// The GL functions of the context current on this thread, from `loader` or
/// WGL.
///
/// Fails with [`ErrorKind::Unsupported`](mln_util::ErrorKind) for a context
/// older than OpenGL 3.3.
pub(crate) fn functions(loader: Option<Loader>) -> Result<Gl, Error> {
    // SAFETY: a context is current (the caller's), and WGL gives its
    // functions.
    let gl =
        unsafe { glow::Context::from_loader_function(|name| proc_address(loader, name).unwrap_or(std::ptr::null())) };
    let version = glow::HasContext::version(&gl);
    if version.is_embedded || (version.major, version.minor) < (3, 3) {
        return Err(Error::unsupported(format!(
            "the library renders with OpenGL 3.3 or later, and the context is of {}.{}",
            version.major, version.minor
        )));
    }
    Ok(gl)
}

/// The device context and context current on this thread, if any.
pub(crate) fn current() -> Option<(HDC, HGLRC)> {
    // SAFETY: plain queries of the thread's current context.
    let (dc, context) = unsafe { (wglGetCurrentDC(), wglGetCurrentContext()) };
    (!context.is_null()).then_some((dc, context))
}

/// Makes `context` current on `dc` on this thread, or none.
pub(crate) fn make_current(dc: HDC, context: HGLRC) -> Result<(), String> {
    // SAFETY: a live device context of the context's pixel format, or none.
    if unsafe { wglMakeCurrent(dc, context) } == 0 {
        return Err(last_error("wglMakeCurrent"));
    }
    Ok(())
}

/// The pixel format of `dc`, or 0 for none.
pub(crate) fn pixel_format(dc: HDC) -> i32 {
    // SAFETY: a plain query of a live device context.
    unsafe { GetPixelFormat(dc) }
}

/// Presents the back buffer of `dc`'s window.
pub(crate) fn swap_buffers(dc: HDC) -> Result<(), String> {
    // SAFETY: a live device context of a double-buffered pixel format.
    if unsafe { SwapBuffers(dc) } == 0 {
        return Err(last_error("SwapBuffers"));
    }
    Ok(())
}

/// Has the current context's swaps wait for the display, or not, where the
/// driver has `WGL_EXT_swap_control`.
pub(crate) fn set_swap_interval(loader: Option<Loader>, interval: i32) {
    if let Some(function) = proc_address(loader, "wglSwapIntervalEXT") {
        // SAFETY: the extension's function, which has this type, with the
        // context current.
        unsafe { std::mem::transmute::<*const c_void, SwapInterval>(function)(interval) };
    }
}

/// A device context of a host's window, whose pixel format the library set.
/// Only the thread that got it releases it.
pub(crate) struct WindowDc {
    window: HWND,
    dc: HDC,
}

impl WindowDc {
    /// `window`'s device context with the pixel format `format`, or the
    /// library's when it is 0, which it sets on a window that has none.
    /// Returns `None` for a window of another pixel format, which Windows
    /// doesn't let change.
    ///
    /// # Safety
    ///
    /// `window` is a live window.
    pub(crate) unsafe fn new(window: HWND, format: i32) -> Result<Option<WindowDc>, String> {
        // SAFETY: the caller's contract.
        let dc = unsafe { GetDC(window) };
        if dc.is_null() {
            return Err(last_error("GetDC of the window"));
        }
        let this = WindowDc { window, dc };
        let format = choose_pixel_format(dc, format)?;
        match pixel_format(dc) {
            0 => {
                set_pixel_format(dc, format)?;
            }
            current if current == format => {}
            _ => return Ok(None),
        }
        Ok(Some(this))
    }

    pub(crate) fn dc(&self) -> HDC {
        self.dc
    }
}

impl Drop for WindowDc {
    fn drop(&mut self) {
        // SAFETY: the device context is this one's, got on this thread.
        unsafe { ReleaseDC(self.window, self.dc) };
    }
}

#[cfg(test)]
mod tests {
    use glow::HasContext;

    use super::*;

    // Two contexts of a share group on hidden windows: the second sees the
    // first's texture, and both are core profiles of OpenGL 3.3 or later.
    #[test]
    fn contexts_share_objects_on_hidden_windows() {
        let (first, format) = HiddenWindow::new(0).unwrap();
        let root = create_context(first.dc(), std::ptr::null_mut()).unwrap();
        let (second, same) = HiddenWindow::new(format).unwrap();
        assert_eq!(format, same);
        let shared = create_context(second.dc(), root).unwrap();
        let load = |dc, context| {
            make_current(dc, context).unwrap();
            // SAFETY: the context is current.
            unsafe { glow::Context::from_loader_function(|name| proc_address(None, name).unwrap_or(std::ptr::null())) }
        };
        let gl = load(first.dc(), root);
        let version = gl.version().clone();
        assert!(!version.is_embedded && (version.major, version.minor) >= (3, 3), "{version:?}");
        // SAFETY: the context is current.
        let texture = unsafe { gl.create_texture() }.unwrap();
        // SAFETY: as above.
        unsafe {
            gl.bind_texture(glow::TEXTURE_2D, Some(texture));
            gl.finish();
        }
        let gl = load(second.dc(), shared);
        // SAFETY: the shared context is current.
        assert!(unsafe { gl.is_texture(texture) });
        assert_eq!(current().map(|(_, context)| context), Some(shared));
        make_current(std::ptr::null_mut(), std::ptr::null_mut()).unwrap();
        assert!(current().is_none());
        delete_context(shared);
        delete_context(root);
    }
}
