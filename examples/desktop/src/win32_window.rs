// The window as the render sessions take it on Windows: winit hands out a
// window's handles only on the thread that runs its event loop, and the
// render thread makes the window's surfaces. So the window's thread takes the
// handles, and the render thread lends them out, while the window lives.

#![expect(unsafe_code, reason = "lending the window's raw handles is the FFI here")]

use std::sync::Arc;

use mln_gfx::window::raw_window_handle::{
    DisplayHandle, HandleError, HasDisplayHandle, HasWindowHandle, RawDisplayHandle, RawWindowHandle, WindowHandle,
};

/// A window and its handles, which the window's thread took.
pub(crate) struct SurfaceWindow {
    /// Kept alive with its handles.
    _window: Arc<winit::window::Window>,
    handles: Result<(RawWindowHandle, RawDisplayHandle), HandleError>,
}

// SAFETY: a Win32 window's handles name it from any thread; the window lives
// as long as `self`.
unsafe impl Send for SurfaceWindow {}
// SAFETY: as above; nothing here changes the handles.
unsafe impl Sync for SurfaceWindow {}

impl SurfaceWindow {
    /// The window, with its handles. Call it on the window's thread.
    pub(crate) fn new(window: Arc<winit::window::Window>) -> SurfaceWindow {
        let handles = window
            .window_handle()
            .map(|handle| handle.as_raw())
            .and_then(|handle| window.display_handle().map(|display| (handle, display.as_raw())));
        SurfaceWindow { _window: window, handles }
    }
}

impl HasWindowHandle for SurfaceWindow {
    fn window_handle(&self) -> Result<WindowHandle<'_>, HandleError> {
        let (handle, _) = self.handles.as_ref().map_err(|_| HandleError::Unavailable)?;
        // SAFETY: the handle names the window, which lives as long as `self`.
        Ok(unsafe { WindowHandle::borrow_raw(*handle) })
    }
}

impl HasDisplayHandle for SurfaceWindow {
    fn display_handle(&self) -> Result<DisplayHandle<'_>, HandleError> {
        let (_, display) = self.handles.as_ref().map_err(|_| HandleError::Unavailable)?;
        // SAFETY: as above.
        Ok(unsafe { DisplayHandle::borrow_raw(*display) })
    }
}
