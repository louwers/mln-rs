// The share group that GL renderers render in: an EGL display and a root
// context that each renderer's context shares its objects with. GL contexts
// are current on one thread at a time, so each renderer has a context of its
// own in the group, on its own thread.
//
// The display is Mesa's surfaceless platform where the library has it, so
// that no window system is needed; elsewhere it is the default display, as
// in C++ (platform/linux/src/headless_backend_egl.cpp).

use std::ffi::c_void;
use std::ptr::NonNull;
use std::sync::{Arc, OnceLock};

use khronos_egl as egl;
use mln_util::error::Error;
use mln_util::logging::{Event, Log};

use crate::egl::{Egl, library};
use crate::renderer_backend::PlatformContext;
use crate::types::Gl;

/// `EGL_PLATFORM_SURFACELESS_MESA` (EGL_MESA_platform_surfaceless).
const PLATFORM_SURFACELESS_MESA: egl::Enum = 0x31DD;

/// The EGL library and its display (`EGLDisplayConfig`), which every
/// library device of the process shares. The display stays initialized.
struct EglDisplay {
    egl: &'static Egl,
    display: egl::Display,
    config: egl::Config,
}

// SAFETY: the display and the config are handles EGL lets every thread use;
// the contexts made on them are used on one thread at a time.
unsafe impl Send for EglDisplay {}
// SAFETY: as above.
unsafe impl Sync for EglDisplay {}

impl EglDisplay {
    fn get() -> Result<&'static EglDisplay, String> {
        static DISPLAY: OnceLock<Result<EglDisplay, String>> = OnceLock::new();
        DISPLAY.get_or_init(Self::open).as_ref().map_err(Clone::clone)
    }

    fn open() -> Result<EglDisplay, String> {
        let egl = library()?;
        let client_extensions =
            egl.query_string(None, egl::EXTENSIONS).ok().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        let surfaceless = client_extensions.split(' ').any(|e| e == "EGL_MESA_platform_surfaceless");
        // SAFETY: the default native display is valid on every platform.
        let display = unsafe {
            match egl.upcast::<egl::EGL1_5>() {
                Some(egl15) if surfaceless => egl15
                    .get_platform_display(PLATFORM_SURFACELESS_MESA, egl::DEFAULT_DISPLAY, &[egl::ATTRIB_NONE])
                    .ok(),
                _ => None,
            }
            .or_else(|| egl.get_display(egl::DEFAULT_DISPLAY))
        }
        .ok_or("Failed to obtain a valid EGL display.")?;
        egl.initialize(display).map_err(|e| format!("eglInitialize() failed: {e}"))?;
        egl.bind_api(egl::OPENGL_ES_API).map_err(|e| format!("Error setting the EGL rendering API: {e}"))?;
        // Note: we're choosing an arbitrary pixel format, since we're not
        // using the default surface anyway; all rendering will be directed to
        // framebuffer objects.
        let attributes = [
            egl::RENDERABLE_TYPE,
            egl::OPENGL_ES3_BIT,
            egl::SURFACE_TYPE,
            egl::PBUFFER_BIT,
            egl::RED_SIZE,
            8,
            egl::GREEN_SIZE,
            8,
            egl::BLUE_SIZE,
            8,
            egl::ALPHA_SIZE,
            8,
            egl::NONE,
        ];
        let config = egl
            .choose_first_config(display, &attributes)
            .map_err(|e| format!("Failed to choose an EGL config: {e}"))?
            .ok_or("Failed to choose an EGL config.")?;
        Ok(EglDisplay { egl, display, config })
    }

    /// An OpenGL ES 3.0 context that shares with `share`, if any.
    fn create_context(&self, share: Option<egl::Context>) -> Result<egl::Context, String> {
        self.egl
            .create_context(self.display, self.config, share, &[egl::CONTEXT_CLIENT_VERSION, 3, egl::NONE])
            .map_err(|e| format!("Error creating the EGL context object: {e}"))
    }
}

/// The root context of a library device's share group, which is never
/// current.
struct ShareGroup {
    display: &'static EglDisplay,
    root: egl::Context,
}

// SAFETY: the root context is never made current; EGL lets any thread make
// contexts that share with it.
unsafe impl Send for ShareGroup {}
// SAFETY: as above.
unsafe impl Sync for ShareGroup {}

impl Drop for ShareGroup {
    fn drop(&mut self) {
        let EglDisplay { egl, display, .. } = self.display;
        if let Err(e) = egl.destroy_context(*display, self.root) {
            Log::error(Event::GraphicsBackend, format!("Failed to destroy EGL context: {e}"));
        }
    }
}

/// The host's EGL display and context.
#[derive(Debug)]
#[allow(dead_code)]
struct HostContext {
    display: NonNull<c_void>,
    context: NonNull<c_void>,
}

// SAFETY: EGL handles may be used from any thread; the host's contract for
// `from_raw` covers the context being current elsewhere.
unsafe impl Send for HostContext {}
// SAFETY: as above.
unsafe impl Sync for HostContext {}

enum Kind {
    Library(ShareGroup),
    Host(#[allow(dead_code)] HostContext),
}

/// A GL share group that renderers on any thread render in: the library's,
/// or the host's. Clones share it.
#[derive(Clone)]
pub struct Device {
    kind: Arc<Kind>,
}

impl std::fmt::Debug for Device {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Device").finish_non_exhaustive()
    }
}

impl Device {
    /// The library's share group, of OpenGL ES 3.0 contexts on EGL's
    /// display.
    ///
    /// Fails with [`ErrorKind::Unsupported`](mln_util::ErrorKind) when there
    /// is no EGL, or no display or config for it, and with
    /// [`ErrorKind::Native`](mln_util::ErrorKind) when EGL makes no
    /// context.
    pub fn new() -> Result<Device, Error> {
        let display = EglDisplay::get().map_err(Error::unsupported)?;
        let root = display.create_context(None).map_err(Error::native)?;
        Ok(Device { kind: Arc::new(Kind::Library(ShareGroup { display, root })) })
    }

    /// The host's share group, of `context` on `display`. A renderer on it
    /// fails with [`ErrorKind::Unsupported`](mln_util::ErrorKind): the
    /// library renders in share groups of its own only.
    ///
    /// # Safety
    ///
    /// `display` is an initialized `EGLDisplay` and `context` an
    /// `EGLContext` of it, and both outlive the device.
    pub unsafe fn from_raw(display: NonNull<c_void>, context: NonNull<c_void>) -> Device {
        Device { kind: Arc::new(Kind::Host(HostContext { display, context })) }
    }

    /// A context in the share group, current on a 1x1 pbuffer around a
    /// backend's frames, and the GL functions it calls.
    pub(crate) fn create_context(&self) -> Result<(Gl, Box<dyn PlatformContext>), Error> {
        let Kind::Library(group) = &*self.kind else {
            return Err(Error::unsupported("the library renders in GL share groups of its own only"));
        };
        let context = EglContext::new(self.clone(), group).map_err(Error::native)?;
        context.make_current();
        let egl = context.display.egl;
        // SAFETY: the context is current, and EGL gives the functions of its
        // API.
        let gl = unsafe {
            glow::Context::from_loader_function(|name| {
                egl.get_proc_address(name).map_or(std::ptr::null(), |function| function as *const _)
            })
        };
        context.done_current();
        Ok((gl, Box::new(context)))
    }
}

/// `EGLBackendImpl`: an OpenGL ES 3.0 context, and the 1x1 pbuffer it is
/// current on.
struct EglContext {
    display: &'static EglDisplay,
    context: egl::Context,
    surface: egl::Surface,
    /// The share group, which outlives its contexts.
    _device: Device,
}

impl EglContext {
    fn new(device: Device, group: &ShareGroup) -> Result<Self, String> {
        let display = group.display;
        let context = display.create_context(Some(group.root))?;
        let EglDisplay { egl, display: egl_display, config } = display;
        // Create a dummy pbuffer. We will render to framebuffers anyway, but
        // we need a pbuffer to activate the context.
        let surface =
            match egl.create_pbuffer_surface(*egl_display, *config, &[egl::WIDTH, 1, egl::HEIGHT, 1, egl::NONE]) {
                Ok(surface) => surface,
                Err(e) => {
                    let _ = egl.destroy_context(*egl_display, context);
                    return Err(format!("Could not create surface: {e}"));
                }
            };
        Ok(EglContext { display, context, surface, _device: device })
    }
}

impl PlatformContext for EglContext {
    fn make_current(&self) {
        let EglDisplay { egl, display, .. } = self.display;
        if let Err(e) = egl.make_current(*display, Some(self.surface), Some(self.surface), Some(self.context)) {
            Log::error(Event::GraphicsBackend, format!("Switching OpenGL context failed: {e}"));
        }
    }

    fn done_current(&self) {
        let EglDisplay { egl, display, .. } = self.display;
        if let Err(e) = egl.make_current(*display, None, None, None) {
            Log::error(Event::GraphicsBackend, format!("Removing OpenGL context failed: {e}"));
        }
    }
}

impl Drop for EglContext {
    fn drop(&mut self) {
        let EglDisplay { egl, display, .. } = self.display;
        if let Err(e) = egl.destroy_context(*display, self.context) {
            Log::error(Event::GraphicsBackend, format!("Failed to destroy EGL context: {e}"));
        }
        if let Err(e) = egl.destroy_surface(*display, self.surface) {
            Log::error(Event::GraphicsBackend, format!("Failed to destroy EGL surface: {e}"));
        }
    }
}
