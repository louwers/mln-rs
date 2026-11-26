// The target of a Win32 window: frames presented to the window from a WGL
// context of its own, current on the window's device context, whose pixel
// format the target sets. Windows lets no pixel format change, so the window
// keeps the library's after the target is gone, and takes a target of the
// library's again.
//
// As on the other window backends, an overlay can draw over each frame
// before it is presented (an app's controls).

use std::sync::Arc;

use mln_gfx::frame::{FrameImage, FrameTarget, Handoff};
use mln_gfx::window::raw_window_handle::RawWindowHandle;
use mln_gfx::{Error, Size, Window};
use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::Graphics::Gdi::HDC;
use windows_sys::Win32::Graphics::OpenGL::HGLRC;
use windows_sys::Win32::UI::WindowsAndMessaging::IsIconic;

use crate::frame_image::{FrameImports, default_framebuffer};
use crate::gpu::GlGpu;
use crate::overlay::{self, Overlay};
use crate::renderer_backend::{PlatformContext, RendererBackend};
use crate::{log_target, wgl};

/// The window's context, current on the window's device context, which the
/// backend's platform context owns.
struct WglWindow {
    window: HWND,
    dc: wgl::WindowDc,
    context: HGLRC,
    /// What the window handle names, which stays open until the context is
    /// gone.
    _window: Arc<dyn Window>,
}

/// The Win32 window of `window`.
fn hwnd(window: &dyn Window) -> Result<HWND, Error> {
    let handle =
        window.window_handle().map_err(|e| Error::invalid_argument(format!("The window has no handle: {e}")))?;
    let RawWindowHandle::Win32(handle) = handle.as_raw() else {
        return Err(Error::invalid_argument("The window is no Win32 window."));
    };
    Ok(handle.hwnd.get() as HWND)
}

fn other_pixel_format() -> Error {
    Error::invalid_argument("the window has a pixel format of its own, which Windows doesn't let change")
}

impl WglWindow {
    fn new(window: Arc<dyn Window>) -> Result<WglWindow, Error> {
        let hwnd = hwnd(&*window)?;
        // SAFETY: the window is the live one of `window`.
        let dc = unsafe { wgl::WindowDc::new(hwnd, 0) }.map_err(Error::native)?.ok_or_else(other_pixel_format)?;
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

/// The backend's platform context: the window's context.
struct WindowContext(WglWindow);

impl PlatformContext for WindowContext {
    fn make_current(&mut self) {
        if let Err(e) = self.0.make_current() {
            log::error!(target: log_target::GRAPHICS_BACKEND, "Switching OpenGL context failed: {e}");
        }
    }

    fn done_current(&mut self) {
        if let Err(e) = wgl::make_current(std::ptr::null_mut(), std::ptr::null_mut()) {
            log::error!(target: log_target::GRAPHICS_BACKEND, "Removing OpenGL context failed: {e}");
        }
    }
}

/// Frames presented to a Win32 window, from a WGL context of the window's
/// own, which the backend's `GlGpu` owns.
///
/// The target's tear-down drops the overlay with the context current. A
/// target dropped without it drops the overlay without.
pub struct SurfaceTarget {
    /// The window, which the backend's context keeps open.
    window: HWND,
    /// The window's device context, which the backend's context keeps.
    dc: HDC,
    size: Size,
    /// The frame's default framebuffer, imported into the backend's `GlGpu`.
    image: FrameImports,
    /// What draws over each frame.
    overlay: Option<Overlay>,
}

impl SurfaceTarget {
    /// Checks, on any thread and without a context, that
    /// [`with_backend`](Self::with_backend) can give `window` the library's
    /// pixel format: that it has none yet, or that one. Sets none.
    ///
    /// Fails with [`ErrorKind::InvalidArgument`](mln_gfx::ErrorKind) for a
    /// window that isn't a Win32 window or that has a pixel format of its
    /// own, and with [`ErrorKind::Native`](mln_gfx::ErrorKind) when the
    /// query fails.
    pub fn check_window(window: &dyn Window) -> Result<(), Error> {
        let hwnd = hwnd(window)?;
        // SAFETY: the window that `window` names, which the caller keeps open.
        if unsafe { wgl::takes_library_pixel_format(hwnd) }.map_err(Error::native)? {
            Ok(())
        } else {
            Err(other_pixel_format())
        }
    }

    /// A backend on a context of `window`, a Win32 window, and a target that
    /// presents frames of `framebuffer_size` pixels to it, which it keeps.
    /// It gives the window the library's pixel format. With
    /// `cap_frame_rate`, presenting waits for the display.
    ///
    /// Fails with [`ErrorKind::InvalidArgument`](mln_gfx::ErrorKind) for a
    /// window with a pixel format of its own, with
    /// [`ErrorKind::Unsupported`](mln_gfx::ErrorKind) when the driver makes
    /// no OpenGL 3.3 core context, and with
    /// [`ErrorKind::Native`](mln_gfx::ErrorKind) when WGL fails otherwise.
    pub fn with_backend(
        window: Arc<dyn Window>,
        framebuffer_size: Size,
        cap_frame_rate: bool,
    ) -> Result<(RendererBackend, SurfaceTarget), Error> {
        let window = WglWindow::new(window)?;
        window.make_current().map_err(|e| Error::native(format!("Switching OpenGL context failed: {e}")))?;
        wgl::set_swap_interval(None, i32::from(cap_frame_rate));
        let gl = wgl::functions(None);
        let _ = wgl::make_current(std::ptr::null_mut(), std::ptr::null_mut());
        let gl = gl?;
        let (hwnd, dc) = (window.window, window.dc.dc());
        let backend = RendererBackend::from_parts(gl, Box::new(WindowContext(window)));
        let target =
            SurfaceTarget { window: hwnd, dc, size: framebuffer_size, image: FrameImports::default(), overlay: None };
        Ok((backend, target))
    }

    /// The size of the frames, in physical pixels.
    #[must_use]
    pub fn size(&self) -> Size {
        self.size
    }

    /// Draws `overlay` over each frame from now on, or nothing. The overlay
    /// it had goes with the context of `gpu`, the backend's, current.
    pub fn set_overlay(&mut self, gpu: &mut GlGpu, overlay: Option<Overlay>) {
        let _current = gpu.enter();
        self.overlay = overlay;
    }
}

impl FrameTarget for SurfaceTarget {
    type Gpu = GlGpu;

    /// The window has `size` pixels from now on.
    fn resize(&mut self, _gpu: &mut GlGpu, size: Size) {
        self.size = size;
    }

    /// A frame begins unless the window is minimized, or has no pixels.
    fn begin_frame(&mut self, gpu: &mut GlGpu) -> Option<FrameImage> {
        let size = self.size;
        // SAFETY: a plain query of the live window.
        if size.width == 0 || size.height == 0 || unsafe { IsIconic(self.window) } != 0 {
            return None;
        }
        self.image.get_or_import(|| default_framebuffer(gpu, size))
    }

    /// Presents the frame, with the overlay over it.
    fn end_frame(&mut self, gpu: &mut GlGpu) -> Handoff {
        let Some(image) = self.image.get() else {
            return Handoff::Dropped;
        };
        let mut current = gpu.enter();
        let gpu = &mut *current;
        if let Some(overlay) = self.overlay.as_mut() {
            overlay::draw(overlay, gpu.context_mut(), image.size);
        }
        self.image.release(gpu);
        if let Err(e) = wgl::swap_buffers(self.dc) {
            log::error!(target: log_target::GRAPHICS_BACKEND, "{e}");
        }
        Handoff::Presented
    }

    /// The overlay goes with the context current.
    fn tear_down(&mut self, gpu: &mut GlGpu) {
        self.image.release(gpu);
        let _current = gpu.enter();
        self.overlay = None;
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::num::NonZeroIsize;
    use std::rc::Rc;
    use std::sync::Arc;

    use glow::HasContext as _;
    use mln_gfx::frame::{FrameTarget as _, Handoff};
    use mln_gfx::gpu::{ColorAttachment, Gpu as _, LoadOp, PassDesc};
    use mln_gfx::window::raw_window_handle::{
        DisplayHandle, HandleError, HasDisplayHandle, HasWindowHandle, RawDisplayHandle, RawWindowHandle,
        Win32WindowHandle, WindowHandle, WindowsDisplayHandle,
    };
    use mln_gfx::{RendererBackend as _, Rgba, Size};

    use super::SurfaceTarget;
    use crate::wgl::HiddenWindow;

    /// The handle of a window that the test keeps open.
    struct Handle(NonZeroIsize);

    impl HasWindowHandle for Handle {
        fn window_handle(&self) -> Result<WindowHandle<'_>, HandleError> {
            // SAFETY: the window stays open until the target is gone.
            Ok(unsafe { WindowHandle::borrow_raw(RawWindowHandle::Win32(Win32WindowHandle::new(self.0))) })
        }
    }

    impl HasDisplayHandle for Handle {
        fn display_handle(&self) -> Result<DisplayHandle<'_>, HandleError> {
            // SAFETY: Windows' display handle names nothing.
            Ok(unsafe { DisplayHandle::borrow_raw(RawDisplayHandle::Windows(WindowsDisplayHandle::new())) })
        }
    }

    /// Clears a frame of `target` red, and presents it.
    fn red_frame(backend: &mut crate::RendererBackend, target: &mut SurfaceTarget) -> Handoff {
        let mut gpu = backend.gpu();
        let image = target.begin_frame(&mut gpu).unwrap();
        let clear = LoadOp::Clear(Rgba::new(1.0, 0.0, 0.0, 1.0));
        let pass = PassDesc {
            label: "frame",
            color: Some(ColorAttachment { texture: image.color, load: clear }),
            depth_stencil: None,
        };
        gpu.begin_pass(&pass).unwrap();
        gpu.end_pass().unwrap();
        gpu.flush(false).unwrap();
        target.end_frame(&mut gpu)
    }

    // The overlay draws over each frame into the window's framebuffer, with
    // the frame drawn into it, until it is taken away.
    #[test]
    fn an_overlay_draws_over_each_frame() {
        let (window, _) = HiddenWindow::new(0).unwrap();
        let handle = Arc::new(Handle(NonZeroIsize::new(window.window() as isize).unwrap()));
        let size = Size::new(8, 8);
        let (mut backend, mut target) = SurfaceTarget::with_backend(handle, size, false).unwrap();
        let pixels = Rc::new(Cell::new(Vec::new()));
        let seen = Rc::clone(&pixels);
        target.set_overlay(
            &mut backend.gpu(),
            Some(Box::new(move |target| {
                assert_eq!(size, target.size);
                let mut pixel = [0u8; 4];
                // SAFETY: the context is current, with the window's framebuffer
                // bound, and the pixel holds the one RGBA texel read.
                unsafe {
                    target.gl.read_pixels(
                        4,
                        4,
                        1,
                        1,
                        glow::RGBA,
                        glow::UNSIGNED_BYTE,
                        glow::PixelPackData::Slice(Some(&mut pixel)),
                    );
                }
                let mut pixels = seen.take();
                pixels.push(pixel);
                seen.set(pixels);
            })),
        );
        assert_eq!(Handoff::Presented, red_frame(&mut backend, &mut target));
        assert_eq!(Handoff::Presented, red_frame(&mut backend, &mut target));
        assert_eq!(vec![[255, 0, 0, 255]; 2], pixels.take());
        target.set_overlay(&mut backend.gpu(), None);
        assert_eq!(Handoff::Presented, red_frame(&mut backend, &mut target));
        assert_eq!(Vec::<[u8; 4]>::new(), pixels.take());
        target.tear_down(&mut backend.gpu());
        drop(target);
        drop(backend);
    }
}
