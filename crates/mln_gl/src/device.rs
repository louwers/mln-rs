// The share group that GL renderers render in: an EGL display and a root
// context that each renderer's context shares its objects with. GL contexts
// are current on one thread at a time, so each renderer has a context of its
// own in the group, on its own thread. The root is the library's, which is
// never current, or the host's.
//
// The library's display is Mesa's surfaceless platform where the library has
// it, so that no window system is needed; elsewhere it is the default
// display, as in C++ (platform/linux/src/headless_backend_egl.cpp). A host's
// group is on the host's display and config, and a renderer's context in it
// makes the host's context current again when it is done, since it may be
// current on the host's thread.

use std::cell::Cell;
use std::ffi::c_void;
use std::ptr::NonNull;
use std::sync::{Arc, OnceLock};

use khronos_egl as egl;
use mln_util::error::Error;
use mln_util::logging::{Event, Log};

use crate::egl::{Egl, library};
use crate::renderer_backend::PlatformContext;
use crate::types::Gl;

/// `EGL_PLATFORM_SURFACELESS_MESA` (`EGL_MESA_platform_surfaceless`).
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

/// The host's EGL display, config and context, the root of a share group.
struct HostGroup {
    egl: &'static Egl,
    display: egl::Display,
    config: egl::Config,
    root: egl::Context,
    /// Whether a context is made current without a surface, where the config
    /// has no pbuffers.
    surfaceless: bool,
}

// SAFETY: EGL handles may be used from any thread; the host's contract for
// `from_raw` covers its context being current elsewhere, and the root is
// only shared with, never made current.
unsafe impl Send for HostGroup {}
// SAFETY: as above.
unsafe impl Sync for HostGroup {}

enum Kind {
    Library(ShareGroup),
    Host(HostGroup),
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

    /// The host's share group, whose root is `context`, of `config` on
    /// `display`: each renderer's context shares its objects with it.
    ///
    /// Fails with [`ErrorKind::Unsupported`](mln_util::ErrorKind) without
    /// EGL, or for a context that isn't of OpenGL ES 3, the API that the
    /// renderers draw with, and with [`ErrorKind::Native`](mln_util::ErrorKind)
    /// when EGL can't answer for the context.
    ///
    /// # Safety
    ///
    /// `display` is an initialized `EGLDisplay`, `config` an `EGLConfig` of
    /// it and `context` an `EGLContext` of it on `config`, and they outlive
    /// the device and its renderers.
    pub unsafe fn from_raw(
        display: NonNull<c_void>,
        config: NonNull<c_void>,
        context: NonNull<c_void>,
    ) -> Result<Device, Error> {
        let egl = library().map_err(Error::unsupported)?;
        // SAFETY: the caller's contract.
        let (display, config, root) = unsafe {
            (
                egl::Display::from_ptr(display.as_ptr()),
                egl::Config::from_ptr(config.as_ptr()),
                egl::Context::from_ptr(context.as_ptr()),
            )
        };
        let client_type = egl
            .query_context(display, root, egl::CONTEXT_CLIENT_TYPE)
            .map_err(|e| Error::native(format!("The host's EGL context can't be queried: {e}")))?;
        let version = egl.query_context(display, root, egl::CONTEXT_CLIENT_VERSION).unwrap_or(0);
        if client_type != egl::OPENGL_ES_API as egl::Int || version < 3 {
            return Err(Error::unsupported(
                "the library's OpenGL renderers share objects with OpenGL ES 3 contexts only",
            ));
        }
        let pbuffers =
            egl.get_config_attrib(display, config, egl::SURFACE_TYPE).is_ok_and(|types| types & egl::PBUFFER_BIT != 0);
        let extensions = egl
            .query_string(Some(display), egl::EXTENSIONS)
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        let surfaceless = !pbuffers && extensions.split(' ').any(|e| e == "EGL_KHR_surfaceless_context");
        Ok(Device { kind: Arc::new(Kind::Host(HostGroup { egl, display, config, root, surfaceless })) })
    }

    /// Whether the group's root is the host's context.
    #[must_use]
    pub fn is_host(&self) -> bool {
        matches!(*self.kind, Kind::Host(_))
    }

    /// A context in the share group, current on a 1x1 pbuffer around a
    /// backend's frames, and the GL functions it calls.
    pub(crate) fn create_context(&self) -> Result<(Gl, Box<dyn PlatformContext>), Error> {
        let context = match &*self.kind {
            Kind::Library(group) => EglContext::new(self.clone(), group).map_err(Error::native)?,
            Kind::Host(group) => EglContext::in_host_group(self.clone(), group, None).map_err(Error::native)?,
        };
        Ok(Self::functions(context))
    }

    /// A context in the host's share group, current on the host's `surface`
    /// around a backend's frames, which it presents to.
    pub(crate) fn create_surface_context(
        &self,
        surface: egl::Surface,
    ) -> Result<(Gl, Box<dyn PlatformContext>, SurfaceSwap), Error> {
        let Kind::Host(group) = &*self.kind else {
            return Err(Error::unsupported("only a device of the host's EGL context presents to its surfaces"));
        };
        let context = EglContext::in_host_group(self.clone(), group, Some(surface)).map_err(Error::native)?;
        let swap = SurfaceSwap { egl: group.egl, display: group.display, surface };
        let (gl, platform) = Self::functions(context);
        Ok((gl, platform, swap))
    }

    /// The GL functions of `context`, made current to look them up.
    fn functions(context: EglContext) -> (Gl, Box<dyn PlatformContext>) {
        context.make_current();
        let egl = context.egl;
        // SAFETY: the context is current, and EGL gives the functions of its
        // API.
        let gl = unsafe {
            glow::Context::from_loader_function(|name| {
                egl.get_proc_address(name).map_or(std::ptr::null(), |function| function as *const _)
            })
        };
        context.done_current();
        (gl, Box::new(context))
    }
}

/// Presents a frame to the host's surface that a context is current on.
pub(crate) struct SurfaceSwap {
    egl: &'static Egl,
    display: egl::Display,
    surface: egl::Surface,
}

impl SurfaceSwap {
    pub(crate) fn swap(&self) {
        if let Err(e) = self.egl.swap_buffers(self.display, self.surface) {
            Log::error(Event::GraphicsBackend, format!("eglSwapBuffers() failed: {e}"));
        }
    }
}

/// The context that was current on the thread before a renderer's.
type Previous = Option<(egl::Display, Option<egl::Surface>, Option<egl::Surface>, egl::Context)>;

/// `EGLBackendImpl`: an OpenGL ES 3.0 context, and the 1x1 pbuffer it is
/// current on, or the host's surface it presents to, or none.
struct EglContext {
    egl: &'static Egl,
    display: egl::Display,
    context: egl::Context,
    surface: Option<egl::Surface>,
    /// Whether the surface is the context's own, which it destroys.
    owns_surface: bool,
    /// What a host's thread had current, which the context makes current
    /// again when it is done.
    previous: Cell<Previous>,
    restores: bool,
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
        Ok(EglContext {
            egl,
            display: *egl_display,
            context,
            surface: Some(surface),
            owns_surface: true,
            previous: Cell::new(None),
            restores: false,
            _device: device,
        })
    }

    /// A context in the host's group: current on `surface`, the host's,
    /// where it presents, and otherwise on a pbuffer of its own or none.
    fn in_host_group(device: Device, group: &HostGroup, surface: Option<egl::Surface>) -> Result<Self, String> {
        let HostGroup { egl, display, config, root, surfaceless } = *group;
        egl.bind_api(egl::OPENGL_ES_API).map_err(|e| format!("Error setting the EGL rendering API: {e}"))?;
        let context = egl
            .create_context(display, config, Some(root), &[egl::CONTEXT_CLIENT_VERSION, 3, egl::NONE])
            .map_err(|e| format!("Error creating an EGL context that shares the host's: {e}"))?;
        let (surface, owns_surface) = match surface {
            Some(surface) => (Some(surface), false),
            None if surfaceless => (None, false),
            None => match egl.create_pbuffer_surface(display, config, &[egl::WIDTH, 1, egl::HEIGHT, 1, egl::NONE]) {
                Ok(surface) => (Some(surface), true),
                Err(e) => {
                    let _ = egl.destroy_context(display, context);
                    return Err(format!("Could not create surface: {e}"));
                }
            },
        };
        Ok(EglContext {
            egl,
            display,
            context,
            surface,
            owns_surface,
            previous: Cell::new(None),
            restores: true,
            _device: device,
        })
    }
}

impl PlatformContext for EglContext {
    fn make_current(&self) {
        let egl = self.egl;
        if self.restores {
            let previous = egl.get_current_context().filter(|context| *context != self.context).and_then(|context| {
                Some((
                    egl.get_current_display()?,
                    egl.get_current_surface(egl::DRAW),
                    egl.get_current_surface(egl::READ),
                    context,
                ))
            });
            self.previous.set(previous);
        }
        if let Err(e) = egl.make_current(self.display, self.surface, self.surface, Some(self.context)) {
            Log::error(Event::GraphicsBackend, format!("Switching OpenGL context failed: {e}"));
        }
    }

    fn done_current(&self) {
        let egl = self.egl;
        let result = match self.previous.take() {
            Some((display, draw, read, context)) => egl.make_current(display, draw, read, Some(context)),
            None => egl.make_current(self.display, None, None, None),
        };
        if let Err(e) = result {
            Log::error(Event::GraphicsBackend, format!("Removing OpenGL context failed: {e}"));
        }
    }
}

impl Drop for EglContext {
    fn drop(&mut self) {
        let egl = self.egl;
        if let Err(e) = egl.destroy_context(self.display, self.context) {
            Log::error(Event::GraphicsBackend, format!("Failed to destroy EGL context: {e}"));
        }
        if let Some(surface) = self.surface.filter(|_| self.owns_surface)
            && let Err(e) = egl.destroy_surface(self.display, surface)
        {
            Log::error(Event::GraphicsBackend, format!("Failed to destroy EGL surface: {e}"));
        }
    }
}
