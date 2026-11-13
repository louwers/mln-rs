// The target of a Win32 window: frames presented to the window from a WGL
// context of its own, current on the window's device context, whose pixel
// format the target sets. Windows lets no pixel format change, so the window
// keeps the library's after the target is gone, and takes a target of the
// library's again.

use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};
use std::sync::Arc;

use mln_gfx::command_encoder::CommandEncoder;
use mln_gfx::frame::{self, FrameTarget, Handoff};
use mln_gfx::window::raw_window_handle::RawWindowHandle;
use mln_gfx::{BackendScope, ScopeType, Window};
use mln_util::error::Error;
use mln_util::logging::{Event, Log};
use mln_util::size::Size;
use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::Graphics::OpenGL::HGLRC;
use windows_sys::Win32::UI::WindowsAndMessaging::IsIconic;

use crate::context::Context;
use crate::draw_target::{DrawTarget, TargetResource};
use crate::renderer_backend::{PlatformContext, RendererBackend};
use crate::wgl;

/// The window's context, current on the window's device context.
struct WglWindow {
    window: HWND,
    dc: wgl::WindowDc,
    context: HGLRC,
    /// What the window handle names, which stays open until the context is
    /// gone.
    _window: Arc<dyn Window>,
}

impl WglWindow {
    fn new(window: Arc<dyn Window>) -> Result<WglWindow, Error> {
        let handle =
            window.window_handle().map_err(|e| Error::invalid_argument(format!("The window has no handle: {e}")))?;
        let RawWindowHandle::Win32(handle) = handle.as_raw() else {
            return Err(Error::invalid_argument("The window is no Win32 window."));
        };
        let hwnd = handle.hwnd.get() as HWND;
        // SAFETY: the window is the live one of `window`.
        let dc = unsafe { wgl::WindowDc::new(hwnd, 0) }.map_err(Error::native)?.ok_or_else(|| {
            Error::invalid_argument("the window has a pixel format of its own, which Windows doesn't let change")
        })?;
        let context = wgl::create_context(dc.dc(), std::ptr::null_mut()).map_err(Error::unsupported)?;
        Ok(WglWindow { window: hwnd, dc, context, _window: window })
    }

    fn make_current(&self) -> Result<(), String> {
        wgl::make_current(self.dc.dc(), self.context)
    }
}

impl Drop for WglWindow {
    fn drop(&mut self) {
        if wgl::current().is_some_and(|(_, context)| context == self.context) {
            let _ = wgl::make_current(std::ptr::null_mut(), std::ptr::null_mut());
        }
        wgl::delete_context(self.context);
    }
}

/// The backend's share of the window's context.
struct WindowContext(Rc<WglWindow>);

impl PlatformContext for WindowContext {
    fn make_current(&self) {
        if let Err(e) = self.0.make_current() {
            Log::error(Event::GraphicsBackend, format!("Switching OpenGL context failed: {e}"));
        }
    }

    fn done_current(&self) {
        if let Err(e) = wgl::make_current(std::ptr::null_mut(), std::ptr::null_mut()) {
            Log::error(Event::GraphicsBackend, format!("Removing OpenGL context failed: {e}"));
        }
    }
}

/// The window's default framebuffer.
struct WindowResource {
    context: Weak<Context>,
    size: Rc<Cell<Size>>,
}

impl TargetResource for WindowResource {
    fn bind(&self) {
        if let Some(context) = self.context.upgrade() {
            context.bind_framebuffer(None, self.size.get(), true);
        }
    }
}

/// Frames presented to a Win32 window, from a WGL context of the window's
/// own.
pub struct SurfaceTarget {
    backend: Rc<RendererBackend>,
    window: Rc<WglWindow>,
    size: Rc<Cell<Size>>,
    /// Made with the first frame.
    image: RefCell<Option<Rc<DrawTarget>>>,
}

impl SurfaceTarget {
    /// A backend on a context of `window`, a Win32 window, and a target that
    /// presents frames of `framebuffer_size` pixels to it, which it keeps.
    /// It gives the window the library's pixel format. With
    /// `cap_frame_rate`, presenting waits for the display.
    ///
    /// Fails with [`ErrorKind::InvalidArgument`](mln_util::ErrorKind) for a
    /// window with a pixel format of its own, with
    /// [`ErrorKind::Unsupported`](mln_util::ErrorKind) when the driver makes
    /// no OpenGL 3.3 core context, and with
    /// [`ErrorKind::Native`](mln_util::ErrorKind) when WGL fails otherwise.
    pub fn with_backend(
        window: Arc<dyn Window>,
        framebuffer_size: Size,
        cap_frame_rate: bool,
    ) -> Result<(Rc<RendererBackend>, SurfaceTarget), Error> {
        let window = Rc::new(WglWindow::new(window)?);
        window.make_current().map_err(|e| Error::native(format!("Switching OpenGL context failed: {e}")))?;
        wgl::set_swap_interval(None, i32::from(cap_frame_rate));
        let gl = wgl::functions(None);
        let _ = wgl::make_current(std::ptr::null_mut(), std::ptr::null_mut());
        let gl = gl?;
        let backend = Rc::new(RendererBackend::from_parts(gl, Box::new(WindowContext(window.clone()))));
        let target = SurfaceTarget {
            backend: backend.clone(),
            window,
            size: Rc::new(Cell::new(framebuffer_size)),
            image: RefCell::new(None),
        };
        Ok((backend, target))
    }

    /// The size of the frames, in physical pixels.
    pub fn size(&self) -> Size {
        self.size.get()
    }

    /// The window has `size` pixels from now on.
    pub fn resize(&self, size: Size) {
        self.size.set(size);
    }
}

impl FrameTarget for SurfaceTarget {
    /// A frame begins unless the window is minimized, or has no pixels.
    fn begin_frame(&self) -> Option<Rc<dyn frame::DrawTarget>> {
        let size = self.size.get();
        // SAFETY: a plain query of the live window.
        if size.width == 0 || size.height == 0 || unsafe { IsIconic(self.window.window) } != 0 {
            return None;
        }
        let mut image = self.image.borrow_mut();
        if image.is_none() {
            let resource = WindowResource { context: self.backend.gl_context()?.weak(), size: self.size.clone() };
            *image = Some(Rc::new(DrawTarget::new(size, Rc::new(resource))));
        }
        let image = image.clone()?;
        image.set_size(size);
        Some(image)
    }

    fn end_frame(&self, _encoder: &dyn CommandEncoder) -> Handoff {
        if self.image.borrow().is_none() {
            return Handoff::Dropped;
        }
        if let Err(e) = wgl::swap_buffers(self.window.dc.dc()) {
            Log::error(Event::GraphicsBackend, e);
        }
        Handoff::Presented
    }
}

impl Drop for SurfaceTarget {
    fn drop(&mut self) {
        let _scope = BackendScope::new(&*self.backend, ScopeType::Explicit);
        self.image.borrow_mut().take();
    }
}
