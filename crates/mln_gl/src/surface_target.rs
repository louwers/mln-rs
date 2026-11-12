// The target of a native window: frames presented to an EGL window surface
// on a Wayland surface or an X11 window, from an OpenGL ES 3.0 context of
// the window's display. After the
// GLFW app's backend (platform/glfw/glfw_gl_backend.cpp), which gets its
// context and its buffer swaps from GLFW; here they are EGL's.
//
// Port-only, as on the other window backends: an overlay that draws over each
// frame before it is presented (an app's controls), and a copy of a
// presented frame.

use std::any::Any;
use std::cell::{Cell, RefCell};
use std::ffi::{c_int, c_ulong, c_void};
use std::rc::{Rc, Weak};
use std::sync::Arc;

use khronos_egl as egl;
use mln_gfx::command_encoder::CommandEncoder;
use mln_gfx::context::Context as GfxContext;
use mln_gfx::frame::{self, FrameTarget, Handoff};
use mln_gfx::stencil_mode::StencilMode;
use mln_gfx::window::raw_window_handle::{RawDisplayHandle, RawWindowHandle};
use mln_gfx::{BackendScope, ScopeType, Window};
use mln_util::error::Error;
use mln_util::image::PremultipliedImage;
use mln_util::logging::{Event, Log};
use mln_util::size::Size;

use crate::context::Context;
use crate::draw_target::{DrawTarget, TargetResource};
use crate::egl::{Egl, library};
use crate::renderer_backend::{PlatformContext, RendererBackend};
use crate::types::Gl;

/// The window to present to, as its windowing system names it.
#[derive(Clone, Copy)]
enum WindowHandle {
    /// A `wl_display` and a `wl_surface` of it.
    Wayland { display: *mut c_void, surface: *mut c_void },
    /// An Xlib `Display` and a window of it, with the ID of the window's
    /// visual, or 0 for one that isn't known.
    Xlib { display: *mut c_void, window: c_ulong, visual_id: c_ulong },
}

impl WindowHandle {
    fn of(window: &dyn Window) -> Result<WindowHandle, String> {
        let display = window.display_handle().map_err(|e| format!("The window has no display: {e}"))?;
        let handle = window.window_handle().map_err(|e| format!("The window has no handle: {e}"))?;
        match (display.as_raw(), handle.as_raw()) {
            (RawDisplayHandle::Wayland(display), RawWindowHandle::Wayland(window)) => {
                Ok(WindowHandle::Wayland { display: display.display.as_ptr(), surface: window.surface.as_ptr() })
            }
            (RawDisplayHandle::Xlib(display), RawWindowHandle::Xlib(window)) => Ok(WindowHandle::Xlib {
                display: display
                    .display
                    .ok_or("The window's X11 display is the default one, which EGL can't share.")?
                    .as_ptr(),
                window: window.window,
                visual_id: window.visual_id,
            }),
            _ => Err("The window is neither a Wayland surface nor an Xlib window.".to_owned()),
        }
    }
}

/// `EGL_PLATFORM_X11_KHR` and `EGL_PLATFORM_WAYLAND_KHR`.
const PLATFORM_X11_KHR: egl::Enum = 0x31D5;
const PLATFORM_WAYLAND_KHR: egl::Enum = 0x31D8;

type WlEglWindowCreate = unsafe extern "C" fn(*mut c_void, c_int, c_int) -> *mut c_void;
type WlEglWindowResize = unsafe extern "C" fn(*mut c_void, c_int, c_int, c_int, c_int);
type WlEglWindowDestroy = unsafe extern "C" fn(*mut c_void);

/// The context, and the window surface it is current on.
struct EglWindow {
    egl: &'static Egl,
    display: egl::Display,
    config: egl::Config,
    context: egl::Context,
    handle: WindowHandle,
    /// libwayland-egl, which makes the `wl_egl_window` EGL draws a Wayland
    /// surface through.
    wayland_egl: Option<libloading::Library>,
    size: Cell<Size>,
    swap_interval: egl::Int,
    /// Whether the window is this context's to present to: from the first
    /// frame on. While it isn't, the context is current without a surface.
    presenting: Cell<bool>,
    /// Made when the context is first made current to present.
    surface: Cell<Option<egl::Surface>>,
    wl_egl_window: Cell<*mut c_void>,
    /// What `handle` names, which stays open until the context is gone.
    _window: Arc<dyn Window>,
}

impl EglWindow {
    fn new(window: Arc<dyn Window>, size: Size, cap_frame_rate: bool) -> Result<Self, String> {
        let handle = WindowHandle::of(&*window)?;
        let egl = library()?;
        let (platform, native_display, wayland) = match handle {
            WindowHandle::Wayland { display, .. } => (PLATFORM_WAYLAND_KHR, display, true),
            WindowHandle::Xlib { display, .. } => (PLATFORM_X11_KHR, display, false),
        };
        // SAFETY: the native display is the live one of `window`.
        let display = unsafe {
            egl.upcast::<egl::EGL1_5>()
                .and_then(|egl15| egl15.get_platform_display(platform, native_display, &[egl::ATTRIB_NONE]).ok())
                .or_else(|| egl.get_display(native_display))
        }
        .ok_or("Failed to obtain an EGL display for the window.")?;
        egl.initialize(display).map_err(|e| format!("eglInitialize() failed: {e}"))?;
        egl.bind_api(egl::OPENGL_ES_API).map_err(|e| format!("Error setting the EGL rendering API: {e}"))?;
        let attributes = [
            egl::RENDERABLE_TYPE,
            egl::OPENGL_ES3_BIT,
            egl::SURFACE_TYPE,
            egl::WINDOW_BIT,
            egl::RED_SIZE,
            8,
            egl::GREEN_SIZE,
            8,
            egl::BLUE_SIZE,
            8,
            egl::DEPTH_SIZE,
            24,
            egl::STENCIL_SIZE,
            8,
            egl::NONE,
        ];
        let count = egl
            .matching_config_count(display, &attributes)
            .map_err(|e| format!("Failed to choose an EGL config: {e}"))?;
        let mut configs = Vec::with_capacity(count);
        egl.choose_config(display, &attributes, &mut configs)
            .map_err(|e| format!("Failed to choose an EGL config: {e}"))?;
        let attribute = |config, attribute| egl.get_config_attrib(display, config, attribute).ok();
        // An X11 window takes the surfaces of its visual only. Elsewhere, a
        // config without alpha: the window is opaque, as the map's frames
        // are, and a compositor would blend what alpha they have left.
        let of_visual = match handle {
            WindowHandle::Xlib { visual_id, .. } if visual_id != 0 => configs
                .iter()
                .find(|&&config| attribute(config, egl::NATIVE_VISUAL_ID) == egl::Int::try_from(visual_id).ok()),
            _ => None,
        };
        let opaque = || configs.iter().find(|&&config| attribute(config, egl::ALPHA_SIZE) == Some(0));
        let config = *of_visual
            .or_else(opaque)
            .or(configs.first())
            .ok_or("No EGL config for a window with a depth and a stencil buffer.")?;
        let context = egl
            .create_context(display, config, None, &[egl::CONTEXT_CLIENT_VERSION, 3, egl::NONE])
            .map_err(|e| format!("Error creating the EGL context object: {e}"))?;
        let wayland_egl = if wayland {
            // SAFETY: libwayland-egl has no initializers with requirements.
            match unsafe { libloading::Library::new("libwayland-egl.so.1") } {
                Ok(library) => Some(library),
                Err(e) => {
                    let _ = egl.destroy_context(display, context);
                    return Err(format!("Failed to load libwayland-egl: {e}"));
                }
            }
        } else {
            None
        };
        Ok(EglWindow {
            egl,
            display,
            config,
            context,
            handle,
            wayland_egl,
            size: Cell::new(size),
            // A Wayland compositor shows whole frames whenever they come,
            // and a swap that waits for it would wait forever on a hidden
            // window.
            swap_interval: egl::Int::from(cap_frame_rate && !wayland),
            presenting: Cell::new(false),
            surface: Cell::new(None),
            wl_egl_window: Cell::new(std::ptr::null_mut()),
            _window: window,
        })
    }

    /// The window surface, made on first use.
    fn surface(&self) -> Option<egl::Surface> {
        if let Some(surface) = self.surface.get() {
            return Some(surface);
        }
        let size = self.size.get();
        let native_window = match self.handle {
            WindowHandle::Wayland { surface, .. } => {
                let library = self.wayland_egl.as_ref()?;
                // SAFETY: the symbol has this signature, and the surface is
                // the live one of `_window`.
                let window = unsafe {
                    let create = library.get::<WlEglWindowCreate>(b"wl_egl_window_create\0").ok()?;
                    create(surface, size.width.max(1) as c_int, size.height.max(1) as c_int)
                };
                self.wl_egl_window.set(window);
                window
            }
            WindowHandle::Xlib { window, .. } => window as *mut c_void,
        };
        // SAFETY: the native window is the live one of `_window`, or the
        // `wl_egl_window` just made of it.
        match unsafe { self.egl.create_window_surface(self.display, self.config, native_window, None) } {
            Ok(surface) => {
                self.surface.set(Some(surface));
                Some(surface)
            }
            Err(e) => {
                Log::error(Event::GraphicsBackend, format!("Could not create a window surface: {e}"));
                None
            }
        }
    }

    /// Makes the context current: on the window's surface while the window
    /// is the context's to present to, and without a surface while it isn't
    /// (which needs EGL_KHR_surfaceless_context).
    fn make_current(&self) -> Result<(), egl::Error> {
        if !self.presenting.get() {
            return self.egl.make_current(self.display, None, None, Some(self.context));
        }
        let first = self.surface.get().is_none();
        let surface = self.surface();
        self.egl.make_current(self.display, surface, surface, Some(self.context))?;
        if first && surface.is_some() {
            let _ = self.egl.swap_interval(self.display, self.swap_interval);
        }
        Ok(())
    }

    fn resize(&self, size: Size) {
        self.size.set(size);
        let window = self.wl_egl_window.get();
        if !window.is_null()
            && let Some(library) = &self.wayland_egl
        {
            // SAFETY: the symbol has this signature, and the window is live.
            unsafe {
                if let Ok(resize) = library.get::<WlEglWindowResize>(b"wl_egl_window_resize\0") {
                    resize(window, size.width.max(1) as c_int, size.height.max(1) as c_int, 0, 0);
                }
            }
        }
    }

    fn swap(&self) {
        if let Some(surface) = self.surface.get()
            && let Err(e) = self.egl.swap_buffers(self.display, surface)
        {
            Log::error(Event::GraphicsBackend, format!("eglSwapBuffers() failed: {e}"));
        }
    }

    /// Stops presenting to the window: destroys the surface.
    fn release_surface(&self) {
        self.presenting.set(false);
        let _ = self.egl.make_current(self.display, None, None, None);
        if let Some(surface) = self.surface.take() {
            let _ = self.egl.destroy_surface(self.display, surface);
        }
        let window = self.wl_egl_window.replace(std::ptr::null_mut());
        if !window.is_null()
            && let Some(library) = &self.wayland_egl
        {
            // SAFETY: the symbol has this signature, and nothing uses the
            // window any more.
            unsafe {
                if let Ok(destroy) = library.get::<WlEglWindowDestroy>(b"wl_egl_window_destroy\0") {
                    destroy(window);
                }
            }
        }
    }
}

impl Drop for EglWindow {
    fn drop(&mut self) {
        self.release_surface();
        let _ = self.egl.destroy_context(self.display, self.context);
    }
}

/// The backend's share of the window's context.
struct WindowContext(Rc<EglWindow>);

impl PlatformContext for WindowContext {
    fn make_current(&self) {
        if let Err(e) = self.0.make_current() {
            Log::error(Event::GraphicsBackend, format!("Switching OpenGL context failed: {e}"));
        }
    }

    fn done_current(&self) {
        let window = &self.0;
        if let Err(e) = window.egl.make_current(window.display, None, None, None) {
            Log::error(Event::GraphicsBackend, format!("Removing OpenGL context failed: {e}"));
        }
    }
}

/// What an overlay draws into: the window's framebuffer, bound, through the
/// GL functions of the backend's context.
pub struct OverlayTarget<'a> {
    pub gl: &'a Arc<Gl>,
    /// The size of the framebuffer, in pixels.
    pub size: Size,
}

/// Draws over a frame before it is presented. It may leave any GL state
/// behind but the framebuffer binding: the context forgets what it had set.
pub type Overlay = Box<dyn FnMut(&OverlayTarget<'_>)>;

/// What the target shares with its draw target.
#[derive(Default)]
struct Presentation {
    overlay: RefCell<Option<Overlay>>,
    readback_requested: Cell<bool>,
    readback: RefCell<Option<PremultipliedImage>>,
}

/// The window's framebuffer.
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

    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// Frames presented to a window, from a GL context of the window's own.
pub struct SurfaceTarget {
    backend: Rc<RendererBackend>,
    window: Rc<EglWindow>,
    presentation: Presentation,
    size: Rc<Cell<Size>>,
    /// Made with the first frame.
    image: RefCell<Option<Rc<DrawTarget>>>,
}

impl SurfaceTarget {
    /// A backend on a context of `window`'s display, and a target that
    /// presents frames of `framebuffer_size` pixels to `window`, a Wayland
    /// surface or an Xlib window, which it keeps. It takes the window over
    /// with its first frame, not before: until then another target may
    /// present to it. With `cap_frame_rate`, presenting to an X11 window
    /// waits for the display.
    ///
    /// Fails with [`ErrorKind::Native`](mln_util::ErrorKind) when EGL has no
    /// OpenGL ES 3.0 context for the window.
    pub fn with_backend(
        window: Arc<dyn Window>,
        framebuffer_size: Size,
        cap_frame_rate: bool,
    ) -> Result<(Rc<RendererBackend>, SurfaceTarget), Error> {
        let window = Rc::new(EglWindow::new(window, framebuffer_size, cap_frame_rate).map_err(Error::native)?);
        // glow asks the context what it is, so it has to be current: without
        // a surface where EGL can, which leaves the window alone.
        if window.make_current().is_err() {
            window.presenting.set(true);
            window.make_current().map_err(|e| Error::native(format!("Switching OpenGL context failed: {e}")))?;
        }
        // SAFETY: the context is current, and EGL gives the functions of its
        // API.
        let gl = unsafe {
            glow::Context::from_loader_function(|name| {
                window.egl.get_proc_address(name).map_or(std::ptr::null(), |function| function as *const _)
            })
        };
        let _ = window.egl.make_current(window.display, None, None, None);
        let backend = Rc::new(RendererBackend::from_parts(gl, Box::new(WindowContext(window.clone()))));
        let target = SurfaceTarget {
            backend: backend.clone(),
            window,
            presentation: Presentation::default(),
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
        if size != self.size.replace(size) {
            self.window.resize(size);
        }
    }

    /// Draws `overlay` over each frame from now on, or nothing.
    pub fn set_overlay(&self, overlay: Option<Overlay>) {
        let _scope = BackendScope::new(&*self.backend, ScopeType::Explicit);
        *self.presentation.overlay.borrow_mut() = overlay;
    }

    /// Copies the next frame presented (see `take_readback`).
    pub fn request_readback(&self) {
        self.presentation.readback_requested.set(true);
    }

    /// The frame copied after `request_readback`, once presented.
    pub fn take_readback(&self) -> Option<PremultipliedImage> {
        self.presentation.readback.borrow_mut().take()
    }
}

impl FrameTarget for SurfaceTarget {
    /// Takes the window over. Its framebuffer is always there to draw into.
    fn begin_frame(&self) -> Option<Rc<dyn frame::DrawTarget>> {
        if !self.window.presenting.replace(true) {
            // The context is current without a surface: make it current on
            // the window's.
            self.backend.activate();
        }
        let mut image = self.image.borrow_mut();
        if image.is_none() {
            let resource = WindowResource { context: self.backend.gl_context()?.weak(), size: self.size.clone() };
            *image = Some(Rc::new(DrawTarget::new(self.size.get(), resource)));
        }
        let image = image.clone()?;
        image.set_size(self.size.get());
        Some(image)
    }

    /// Presents the frame, with the overlay over it.
    fn end_frame(&self, _encoder: &dyn CommandEncoder) -> Handoff {
        let Some(context) = self.backend.gl_context() else {
            return Handoff::Dropped;
        };
        let size = self.size.get();
        if let Some(overlay) = self.presentation.overlay.borrow_mut().as_mut() {
            context.bind_framebuffer(None, size, true);
            context.set_stencil_mode(&StencilMode::disabled());
            overlay(&OverlayTarget { gl: context.get_backend().gl_shared(), size });
            GfxContext::set_dirty_state(&*context);
        }
        if self.presentation.readback_requested.replace(false) {
            context.bind_framebuffer(None, size, true);
            let mut image = PremultipliedImage::new(size);
            image.data = context.read_framebuffer(size);
            *self.presentation.readback.borrow_mut() = Some(image);
        }
        self.window.swap();
        Handoff::Presented
    }
}

impl Drop for SurfaceTarget {
    fn drop(&mut self) {
        // The overlay's GL objects go with the context current: without a
        // surface, unless the window has this context's still.
        self.window.presenting.set(self.window.surface.get().is_some());
        {
            let _scope = BackendScope::new(&*self.backend, ScopeType::Explicit);
            self.presentation.overlay.borrow_mut().take();
            self.image.borrow_mut().take();
        }
        // The backend's context stays, current without a surface from now on.
        self.window.release_surface();
    }
}
