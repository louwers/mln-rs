// The share group that GL renderers render in: an EGL display and a root
// context that each renderer's context shares its objects with. GL contexts
// are current on one thread at a time, so each renderer has a context of its
// own in the group, on its own thread. The root is the library's, which is
// never current, or the host's.
//
// A library device can instead have one context, which its renderers take
// turns in: each renderer waits for the turn, makes the context current on
// its own thread for one stretch of work, and hands it back released. GL
// objects that a context doesn't share (vertex arrays, framebuffers) are then
// in the one context too, each renderer's own, so they stay valid between its
// turns. What the context last had bound is the previous renderer's, so a
// renderer forgets its cached GL state when another one had the context
// since its last turn (`PlatformContext::take_state_lost`).
//
// Every context, and every turn in a single one, gives the thread back the
// context it had current when it is done: a host's thread that services a
// caller-driven session may have its own context current.
//
// The library's display is Mesa's surfaceless platform where the library has
// it, so that no window system is needed; elsewhere it is the default
// display, as in C++ (platform/linux/src/headless_backend_egl.cpp). A host's
// group is on the host's display and config.
//
// EGL binds a client API to each thread, and makes and releases the current
// context of the bound one, so a context binds its API around its use and
// puts back the thread's API after.
//
// On Windows, the library's group is WGL's (wgl.rs), and EGL's only a host's.

// Windows makes no library device on EGL.
#![cfg_attr(windows, allow(dead_code))]

use std::cell::Cell;
use std::ffi::c_void;
use std::ptr::NonNull;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};
use std::thread::ThreadId;

use khronos_egl as egl;
use mln_util::error::Error;
use mln_util::logging::{Event, Log};
use parking_lot::{Condvar, Mutex};

use crate::borrowed_target::SurfaceHandle;
use crate::egl::{Egl, library};
use crate::renderer_backend::PlatformContext;
use crate::types::Gl;

/// `EGL_PLATFORM_SURFACELESS_MESA` (`EGL_MESA_platform_surfaceless`).
const PLATFORM_SURFACELESS_MESA: egl::Enum = 0x31DD;

/// The OpenGL API that a device's contexts render with. By default the
/// platform's: desktop OpenGL on Windows, whose WGL makes no OpenGL ES
/// context, and OpenGL ES elsewhere.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Api {
    /// OpenGL ES 3.0 or later.
    Gles,
    /// Desktop OpenGL 3.3 or later, of the core profile where the library
    /// makes the context.
    Gl,
}

impl Default for Api {
    fn default() -> Api {
        if cfg!(windows) { Api::Gl } else { Api::Gles }
    }
}

impl Api {
    fn egl(self) -> egl::Enum {
        match self {
            Api::Gles => egl::OPENGL_ES_API,
            Api::Gl => egl::OPENGL_API,
        }
    }

    fn renderable_bit(self) -> egl::Int {
        match self {
            Api::Gles => egl::OPENGL_ES3_BIT,
            Api::Gl => egl::OPENGL_BIT,
        }
    }

    /// The token of `EGL_CLIENT_APIS` that names the API.
    fn client_api_name(self) -> &'static str {
        match self {
            Api::Gles => "OpenGL_ES",
            Api::Gl => "OpenGL",
        }
    }

    fn name(self) -> &'static str {
        match self {
            Api::Gles => "OpenGL ES 3",
            Api::Gl => "desktop OpenGL",
        }
    }

    /// The attributes of the library's contexts: OpenGL ES 3.0, or the core
    /// profile of OpenGL 3.3, which EGL makes the latest compatible version
    /// of.
    fn context_attributes(self) -> &'static [egl::Int] {
        match self {
            Api::Gles => &[egl::CONTEXT_CLIENT_VERSION, 3, egl::NONE],
            Api::Gl => &[
                egl::CONTEXT_MAJOR_VERSION,
                3,
                egl::CONTEXT_MINOR_VERSION,
                3,
                egl::CONTEXT_OPENGL_PROFILE_MASK,
                egl::CONTEXT_OPENGL_CORE_PROFILE_BIT,
                egl::NONE,
            ],
        }
    }
}

/// How the library makes a device.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct DeviceOptions {
    /// The API that the device's contexts render with. Only Linux's and
    /// FreeBSD's EGL renders desktop OpenGL.
    pub api: Api,
    /// Whether the device's renderers take turns in one context instead of
    /// having one each.
    pub single_context: bool,
}

/// Whether this platform's EGL may render desktop OpenGL: the EGL of
/// Android and `OpenHarmony` renders OpenGL ES only.
const DESKTOP_GL_PLATFORM: bool = cfg!(all(any(target_os = "linux", target_os = "freebsd"), not(target_env = "ohos")));

/// Runs `f` with `api` bound on the thread, which EGL makes contexts of, and
/// binds the thread's API again after.
fn with_api<T>(egl: &Egl, api: Api, f: impl FnOnce() -> Result<T, String>) -> Result<T, String> {
    let previous = egl.query_api();
    if previous != api.egl() {
        egl.bind_api(api.egl()).map_err(|e| format!("Error setting the EGL rendering API: {e}"))?;
    }
    let result = f();
    if previous != api.egl()
        && let Err(e) = egl.bind_api(previous)
    {
        Log::error(Event::GraphicsBackend, format!("Error setting the EGL rendering API back: {e}"));
    }
    result
}

/// A context of `api` on `config`, sharing objects with `share`, if any.
fn create_context(
    egl: &Egl,
    display: egl::Display,
    config: egl::Config,
    share: Option<egl::Context>,
    api: Api,
    attributes: &[egl::Int],
) -> Result<egl::Context, String> {
    with_api(egl, api, || {
        egl.create_context(display, config, share, attributes)
            .map_err(|e| format!("Error creating the EGL context object: {e}"))
    })
}

/// A 1x1 pbuffer of `config`, for a context to be current on.
fn create_pbuffer(egl: &Egl, display: egl::Display, config: egl::Config) -> Result<egl::Surface, String> {
    egl.create_pbuffer_surface(display, config, &[egl::WIDTH, 1, egl::HEIGHT, 1, egl::NONE])
        .map_err(|e| format!("Could not create surface: {e}"))
}

/// The EGL library and its display (`EGLDisplayConfig`), which every
/// library device of the process shares, and its config for each API. The
/// display stays initialized.
struct EglDisplay {
    egl: &'static Egl,
    display: egl::Display,
    /// The display's config of each API, found when first asked.
    configs: [OnceLock<Result<egl::Config, String>>; 2],
}

// SAFETY: the display and the configs are handles EGL lets every thread use;
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
        Ok(EglDisplay { egl, display, configs: [OnceLock::new(), OnceLock::new()] })
    }

    /// The display's config of `api`, or why it has none.
    fn config(&self, api: Api) -> Result<egl::Config, String> {
        let index = usize::from(api == Api::Gl);
        self.configs[index].get_or_init(|| self.choose_config(api)).clone()
    }

    fn choose_config(&self, api: Api) -> Result<egl::Config, String> {
        let EglDisplay { egl, display, .. } = *self;
        if api == Api::Gl && !DESKTOP_GL_PLATFORM {
            return Err("this platform's EGL renders OpenGL ES only, not desktop OpenGL".to_owned());
        }
        let client_apis = egl
            .query_string(Some(display), egl::CLIENT_APIS)
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        if !client_apis.split(' ').any(|name| name == api.client_api_name()) {
            return Err(format!("the EGL display renders no {}", api.name()));
        }
        if api == Api::Gl && !has_create_context(egl, display) {
            return Err("the EGL display makes no core profile context: it lacks EGL_KHR_create_context".to_owned());
        }
        // Note: we're choosing an arbitrary pixel format, since we're not
        // using the default surface anyway; all rendering will be directed to
        // framebuffer objects.
        let attributes = [
            egl::RENDERABLE_TYPE,
            api.renderable_bit(),
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
        egl.choose_first_config(display, &attributes)
            .map_err(|e| format!("Failed to choose an EGL config: {e}"))?
            .ok_or_else(|| format!("Failed to choose an EGL config of {}.", api.name()))
    }
}

/// Whether `display` takes the context attributes of EGL 1.5 or
/// `EGL_KHR_create_context`: a version and a profile.
fn has_create_context(egl: &Egl, display: egl::Display) -> bool {
    if egl.upcast::<egl::EGL1_5>().is_some() {
        return true;
    }
    egl.query_string(Some(display), egl::EXTENSIONS)
        .is_ok_and(|extensions| extensions.to_string_lossy().split(' ').any(|e| e == "EGL_KHR_create_context"))
}

/// The root context of a library device's share group, which is never
/// current.
struct ShareGroup {
    display: &'static EglDisplay,
    config: egl::Config,
    api: Api,
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

/// Who has the turn in a single context.
#[derive(Default)]
struct Turn {
    /// The thread that has the context current.
    holder: Option<ThreadId>,
    /// How many stretches of work on the holder's thread have it, nested.
    depth: u32,
    /// The renderer that last had the turn, or 0 for none known.
    last_user: u64,
    /// What the holder's thread had current before the turn, which it gets
    /// back after: a host's thread that services a session has its own
    /// context current.
    previous: Previous,
    /// The thread's client API before the turn, which it gets back after.
    previous_api: Option<egl::Enum>,
}

/// The one context that a single-context device's renderers take turns in,
/// and the pbuffer that it is current on.
struct SingleContext {
    display: &'static EglDisplay,
    api: Api,
    context: egl::Context,
    surface: egl::Surface,
    turn: Mutex<Turn>,
    free: Condvar,
    /// The last renderer's number. Renderers are numbered, not known by
    /// address, since a new one can take a dropped one's.
    users: AtomicU64,
}

// SAFETY: the context is current on the thread that has the turn only, and
// released before another thread gets it.
unsafe impl Send for SingleContext {}
// SAFETY: as above.
unsafe impl Sync for SingleContext {}

impl SingleContext {
    /// Waits for the turn and makes the context current on this thread, for
    /// `user`. Returns whether another renderer had it since `user` last
    /// did. A stretch of work nested in one on the same thread shares its
    /// turn, rather than waiting for itself.
    fn take(&self, user: u64) -> bool {
        let thread = std::thread::current().id();
        let mut turn = self.turn.lock();
        while turn.holder.is_some_and(|holder| holder != thread) {
            self.free.wait(&mut turn);
        }
        let lost = turn.last_user != user;
        turn.last_user = user;
        if turn.depth == 0 {
            let egl = self.display.egl;
            turn.previous = current_other_than(egl, self.context);
            let previous = egl.query_api();
            if previous != self.api.egl()
                && let Err(e) = egl.bind_api(self.api.egl())
            {
                Log::error(Event::GraphicsBackend, format!("Error setting the EGL rendering API: {e}"));
            }
            turn.previous_api = Some(previous);
            if let Err(e) =
                egl.make_current(self.display.display, Some(self.surface), Some(self.surface), Some(self.context))
            {
                Log::error(Event::GraphicsBackend, format!("Switching OpenGL context failed: {e}"));
            }
            turn.holder = Some(thread);
        }
        turn.depth += 1;
        lost
    }

    /// Ends `user`'s stretch of work: the last one on the thread releases
    /// the context, which flushes it, and hands the turn on. After a stretch
    /// nested in another, no renderer is known to have had the context last,
    /// so every renderer's next turn finds its state lost. The outer stretch
    /// goes on with the state that the nested one left.
    fn give(&self, user: u64) {
        let mut turn = self.turn.lock();
        if turn.depth == 0 {
            return;
        }
        turn.depth -= 1;
        if turn.depth > 0 {
            if turn.last_user == user {
                turn.last_user = 0;
            }
            return;
        }
        let egl = self.display.egl;
        if let Err(e) = restore(egl, self.display.display, turn.previous.take()) {
            Log::error(Event::GraphicsBackend, format!("Removing OpenGL context failed: {e}"));
        }
        if let Some(previous) = turn.previous_api.take()
            && previous != self.api.egl()
            && let Err(e) = egl.bind_api(previous)
        {
            Log::error(Event::GraphicsBackend, format!("Error setting the EGL rendering API back: {e}"));
        }
        turn.holder = None;
        drop(turn);
        self.free.notify_one();
    }
}

impl Drop for SingleContext {
    fn drop(&mut self) {
        // Each renderer holds the context, so none has the turn any more.
        let EglDisplay { egl, display, .. } = self.display;
        if let Err(e) = egl.destroy_surface(*display, self.surface) {
            Log::error(Event::GraphicsBackend, format!("Failed to destroy EGL surface: {e}"));
        }
        if let Err(e) = egl.destroy_context(*display, self.context) {
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
    api: Api,
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
    Single(Arc<SingleContext>),
    Host(HostGroup),
    /// A WGL share group, the library's or the host's.
    #[cfg(windows)]
    Wgl(crate::wgl::Group),
}

/// A GL share group that renderers on any thread render in: the library's,
/// or the host's, or the one context that the library's renderers take
/// turns in. Clones share it.
#[derive(Clone)]
pub struct Device {
    kind: Arc<Kind>,
    /// What exports the group's shared images, found once, when first asked.
    #[cfg(all(unix, not(target_vendor = "apple")))]
    exporter: Arc<OnceLock<Option<Arc<crate::exported_target::Exporter>>>>,
}

impl std::fmt::Debug for Device {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Device").finish_non_exhaustive()
    }
}

impl Device {
    /// The library's device on EGL's display: a share group of contexts of
    /// `options.api`, or one such context. On Windows, a share group of WGL's
    /// desktop OpenGL contexts.
    ///
    /// Fails with [`ErrorKind::Unsupported`](mln_util::ErrorKind) when there
    /// is no EGL, or no display or config of the API for it, and on Windows
    /// for OpenGL ES, a single context, or a driver without OpenGL 3.3 core
    /// contexts. Fails with [`ErrorKind::Native`](mln_util::ErrorKind) when
    /// EGL makes no context.
    pub fn with_options(options: DeviceOptions) -> Result<Device, Error> {
        let DeviceOptions { api, single_context } = options;
        #[cfg(windows)]
        {
            if api == Api::Gles {
                return Err(Error::unsupported("a WGL device renders with desktop OpenGL, not OpenGL ES"));
            }
            if single_context {
                return Err(Error::unsupported("the sessions of a WGL device have a context each"));
            }
            crate::wgl::Group::library().map(|group| Device::of(Kind::Wgl(group)))
        }
        #[cfg(not(windows))]
        Self::egl_library(api, single_context)
    }

    #[cfg(not(windows))]
    fn egl_library(api: Api, single_context: bool) -> Result<Device, Error> {
        let display = EglDisplay::get().map_err(Error::unsupported)?;
        let config = display.config(api).map_err(Error::unsupported)?;
        let EglDisplay { egl, display: egl_display, .. } = *display;
        let context =
            create_context(egl, egl_display, config, None, api, api.context_attributes()).map_err(Error::native)?;
        if !single_context {
            return Ok(Device::of(Kind::Library(ShareGroup { display, config, api, root: context })));
        }
        let surface = match create_pbuffer(egl, egl_display, config) {
            Ok(surface) => surface,
            Err(e) => {
                let _ = egl.destroy_context(egl_display, context);
                return Err(Error::native(e));
            }
        };
        Ok(Device::of(Kind::Single(Arc::new(SingleContext {
            display,
            api,
            context,
            surface,
            turn: Mutex::default(),
            free: Condvar::new(),
            users: AtomicU64::new(0),
        }))))
    }

    /// The host's share group, whose root is `context`, of `config` on
    /// `display`: each renderer's context shares its objects with it, and
    /// renders with its API.
    ///
    /// Fails with [`ErrorKind::Unsupported`](mln_util::ErrorKind) without
    /// EGL, or for a context of an API that the renderers don't draw with:
    /// OpenGL ES 3 or later, or desktop OpenGL where the platform's EGL
    /// renders it. Fails with [`ErrorKind::Native`](mln_util::ErrorKind)
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
        let api = if client_type == egl::OPENGL_API as egl::Int && DESKTOP_GL_PLATFORM {
            // EGL tells no desktop context's version: a renderer's context
            // checks for 3.3 once it is current.
            Api::Gl
        } else if client_type == egl::OPENGL_ES_API as egl::Int
            && egl.query_context(display, root, egl::CONTEXT_CLIENT_VERSION).unwrap_or(0) >= 3
        {
            Api::Gles
        } else {
            let desktop = if DESKTOP_GL_PLATFORM { " or desktop OpenGL" } else { "" };
            return Err(Error::unsupported(format!(
                "the library's OpenGL renderers share objects with OpenGL ES 3{desktop} contexts only",
            )));
        };
        let pbuffers =
            egl.get_config_attrib(display, config, egl::SURFACE_TYPE).is_ok_and(|types| types & egl::PBUFFER_BIT != 0);
        let extensions = egl
            .query_string(Some(display), egl::EXTENSIONS)
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        let surfaceless = !pbuffers && extensions.split(' ').any(|e| e == "EGL_KHR_surfaceless_context");
        Ok(Device::of(Kind::Host(HostGroup { egl, display, config, root, api, surfaceless })))
    }

    /// The share group of the host's WGL context `context`, of the pixel
    /// format of `dc`, whose functions `get_proc_address`, the host's
    /// `wglGetProcAddress` or null, gives: each renderer's context shares
    /// its objects with it.
    ///
    /// Fails with [`ErrorKind::InvalidArgument`](mln_util::ErrorKind) for a
    /// device context without a pixel format.
    ///
    /// # Safety
    ///
    /// `dc` is a live `HDC` with the pixel format of `context`, a live
    /// `HGLRC`, and they outlive the device and its renderers.
    #[cfg(windows)]
    pub unsafe fn from_wgl(
        dc: NonNull<c_void>,
        context: NonNull<c_void>,
        get_proc_address: *mut c_void,
    ) -> Result<Device, Error> {
        // SAFETY: the caller's contract.
        let group = unsafe { crate::wgl::Group::host(dc.as_ptr(), context.as_ptr(), get_proc_address) }?;
        Ok(Device::of(Kind::Wgl(group)))
    }

    fn of(kind: Kind) -> Device {
        Device {
            kind: Arc::new(kind),
            #[cfg(all(unix, not(target_vendor = "apple")))]
            exporter: Arc::default(),
        }
    }

    /// The EGL library and the display of the group.
    #[cfg(all(unix, not(target_vendor = "apple")))]
    pub(crate) fn display(&self) -> (&'static Egl, egl::Display) {
        match &*self.kind {
            Kind::Library(group) => (group.display.egl, group.display.display),
            Kind::Single(single) => (single.display.egl, single.display.display),
            Kind::Host(group) => (group.egl, group.display),
        }
    }

    /// What exports the group's shared images, or `None` where nothing does.
    #[cfg(all(unix, not(target_vendor = "apple")))]
    pub(crate) fn exporter(&self) -> Option<Arc<crate::exported_target::Exporter>> {
        self.exporter.get_or_init(|| crate::exported_target::Exporter::probe(self).map(Arc::new)).clone()
    }

    /// Whether the group's root is the host's context.
    #[must_use]
    pub fn is_host(&self) -> bool {
        match &*self.kind {
            Kind::Library(_) | Kind::Single(_) => false,
            Kind::Host(_) => true,
            #[cfg(windows)]
            Kind::Wgl(group) => group.is_host(),
        }
    }

    /// A context in the share group, current on a 1x1 pbuffer around a
    /// backend's frames, or a turn in the device's one context, and the GL
    /// functions it calls.
    pub(crate) fn create_context(&self) -> Result<(Gl, Box<dyn PlatformContext>), Error> {
        match &*self.kind {
            Kind::Library(group) => {
                let context = EglContext::new(self.clone(), group).map_err(Error::native)?;
                Self::functions(Box::new(context))
            }
            Kind::Single(single) => {
                let user = single.users.fetch_add(1, Ordering::Relaxed) + 1;
                let turns = SingleContextTurns { single: single.clone(), user, lost: Cell::new(false) };
                Self::functions(Box::new(turns))
            }
            Kind::Host(group) => {
                let context = EglContext::in_host_group(self.clone(), group, None).map_err(Error::native)?;
                Self::functions(Box::new(context))
            }
            #[cfg(windows)]
            Kind::Wgl(group) => group.create_context(None, Box::new(self.clone())),
        }
    }

    /// A context in the host's share group, current on the host's `surface`
    /// around a backend's frames, which it presents to.
    pub(crate) fn create_surface_context(
        &self,
        surface: SurfaceHandle,
    ) -> Result<(Gl, Box<dyn PlatformContext>, SurfaceSwap), Error> {
        match (&*self.kind, surface) {
            (Kind::Host(group), SurfaceHandle::Egl(surface)) => {
                let context = EglContext::in_host_group(self.clone(), group, Some(surface)).map_err(Error::native)?;
                let swap = SurfaceSwap::Egl { egl: group.egl, display: group.display, surface };
                let (gl, platform) = Self::functions(Box::new(context))?;
                Ok((gl, platform, swap))
            }
            #[cfg(windows)]
            (Kind::Wgl(group), SurfaceHandle::Wgl(dc)) if group.is_host() => {
                let (gl, platform) = group.create_context(Some(dc), Box::new(self.clone()))?;
                Ok((gl, platform, SurfaceSwap::Wgl(dc)))
            }
            _ => Err(Error::unsupported("only a device of the host's context presents to the host's surfaces")),
        }
    }

    /// The GL functions of `context`, made current to look them up. Fails
    /// with [`ErrorKind::Unsupported`](mln_util::ErrorKind) for a desktop
    /// context older than OpenGL 3.3.
    fn functions(context: Box<dyn PlatformContext>) -> Result<(Gl, Box<dyn PlatformContext>), Error> {
        let egl = library().map_err(Error::unsupported)?;
        context.make_current();
        // SAFETY: the context is current, and EGL gives the functions of its
        // API.
        let gl = unsafe {
            glow::Context::from_loader_function(|name| {
                egl.get_proc_address(name).map_or(std::ptr::null(), |function| function as *const _)
            })
        };
        context.done_current();
        let version = glow::HasContext::version(&gl);
        if !version.is_embedded && (version.major, version.minor) < (3, 3) {
            return Err(Error::unsupported(format!(
                "the library renders with OpenGL 3.3 or later, and the context is of {}.{}",
                version.major, version.minor
            )));
        }
        Ok((gl, context))
    }
}

/// Presents a frame to the host's surface that a context is current on.
pub(crate) enum SurfaceSwap {
    Egl {
        egl: &'static Egl,
        display: egl::Display,
        surface: egl::Surface,
    },
    /// The host's device context.
    #[cfg(windows)]
    Wgl(windows_sys::Win32::Graphics::Gdi::HDC),
}

impl SurfaceSwap {
    /// Presents the frame, and returns whether the surface took it.
    pub(crate) fn swap(&self) -> bool {
        match self {
            SurfaceSwap::Egl { egl, display, surface } => match egl.swap_buffers(*display, *surface) {
                Ok(()) => true,
                Err(e) => {
                    Log::error(Event::GraphicsBackend, format!("eglSwapBuffers() failed: {e}"));
                    false
                }
            },
            #[cfg(windows)]
            SurfaceSwap::Wgl(dc) => match crate::wgl::swap_buffers(*dc) {
                Ok(()) => true,
                Err(e) => {
                    Log::error(Event::GraphicsBackend, e);
                    false
                }
            },
        }
    }
}

/// A renderer's turns in the one context of a single-context device.
struct SingleContextTurns {
    /// The device's context, which outlives the renderer.
    single: Arc<SingleContext>,
    /// The renderer's number among the context's.
    user: u64,
    /// Whether another renderer had the context since this one's last turn.
    lost: Cell<bool>,
}

impl PlatformContext for SingleContextTurns {
    fn make_current(&self) {
        if self.single.take(self.user) {
            self.lost.set(true);
        }
    }

    fn done_current(&self) {
        self.single.give(self.user);
    }

    fn take_state_lost(&self) -> bool {
        self.lost.replace(false)
    }
}

/// The context that was current on the thread before a renderer's.
type Previous = Option<(egl::Display, Option<egl::Surface>, Option<egl::Surface>, egl::Context)>;

/// The context current on the thread, with its display and surfaces, unless
/// it is `own`. EGL answers for the thread's bound client API.
fn current_other_than(egl: &Egl, own: egl::Context) -> Previous {
    let context = egl.get_current_context().filter(|context| *context != own)?;
    Some((egl.get_current_display()?, egl.get_current_surface(egl::DRAW), egl.get_current_surface(egl::READ), context))
}

/// Makes `previous` current on the thread again, or, with none, releases
/// the context current on `display`.
fn restore(egl: &Egl, display: egl::Display, previous: Previous) -> Result<(), egl::Error> {
    match previous {
        Some((display, draw, read, context)) => egl.make_current(display, draw, read, Some(context)),
        None => egl.make_current(display, None, None, None),
    }
}

/// An OpenGL ES 3.0 or desktop OpenGL context, and the 1x1 pbuffer it is
/// current on, or the host's surface it presents to, or none.
struct EglContext {
    egl: &'static Egl,
    display: egl::Display,
    context: egl::Context,
    api: Api,
    surface: Option<egl::Surface>,
    /// Whether the surface is the context's own, which it destroys.
    owns_surface: bool,
    /// What the thread had current, which the context makes current again
    /// when it is done: a host's thread that services a session has its own
    /// context current.
    previous: Cell<Previous>,
    /// The thread's client API before the context was made current.
    previous_api: Cell<Option<egl::Enum>>,
    /// The share group, which outlives its contexts.
    _device: Device,
}

impl EglContext {
    fn new(device: Device, group: &ShareGroup) -> Result<Self, String> {
        let ShareGroup { display, config, api, root } = *group;
        let EglDisplay { egl, display: egl_display, .. } = *display;
        let context = create_context(egl, egl_display, config, Some(root), api, api.context_attributes())?;
        // Create a dummy pbuffer. We will render to framebuffers anyway, but
        // we need a pbuffer to activate the context.
        let surface = match create_pbuffer(egl, egl_display, config) {
            Ok(surface) => surface,
            Err(e) => {
                let _ = egl.destroy_context(egl_display, context);
                return Err(e);
            }
        };
        Ok(EglContext {
            egl,
            display: egl_display,
            context,
            api,
            surface: Some(surface),
            owns_surface: true,
            previous: Cell::new(None),
            previous_api: Cell::new(None),
            _device: device,
        })
    }

    /// A context in the host's group: current on `surface`, the host's,
    /// where it presents, and otherwise on a pbuffer of its own or none.
    fn in_host_group(device: Device, group: &HostGroup, surface: Option<egl::Surface>) -> Result<Self, String> {
        let HostGroup { egl, display, config, root, api, surfaceless } = *group;
        let context = match api {
            Api::Gles => create_context(egl, display, config, Some(root), api, api.context_attributes()),
            // A context shares with one of the same profile: the host's may
            // be of the compatibility profile, whose version EGL picks.
            Api::Gl => create_context(egl, display, config, Some(root), api, api.context_attributes())
                .or_else(|_| create_context(egl, display, config, Some(root), api, &[egl::NONE])),
        }
        .map_err(|e| format!("Error creating an EGL context that shares the host's: {e}"))?;
        let (surface, owns_surface) = match surface {
            Some(surface) => (Some(surface), false),
            None if surfaceless => (None, false),
            None => match create_pbuffer(egl, display, config) {
                Ok(surface) => (Some(surface), true),
                Err(e) => {
                    let _ = egl.destroy_context(display, context);
                    return Err(e);
                }
            },
        };
        Ok(EglContext {
            egl,
            display,
            context,
            api,
            surface,
            owns_surface,
            previous: Cell::new(None),
            previous_api: Cell::new(None),
            _device: device,
        })
    }
}

impl PlatformContext for EglContext {
    fn make_current(&self) {
        let egl = self.egl;
        // What the thread had current, asked before the API changes, since
        // EGL answers for the bound one.
        self.previous.set(current_other_than(egl, self.context));
        let previous_api = egl.query_api();
        if previous_api != self.api.egl()
            && let Err(e) = egl.bind_api(self.api.egl())
        {
            Log::error(Event::GraphicsBackend, format!("Error setting the EGL rendering API: {e}"));
        }
        self.previous_api.set(Some(previous_api));
        if let Err(e) = egl.make_current(self.display, self.surface, self.surface, Some(self.context)) {
            Log::error(Event::GraphicsBackend, format!("Switching OpenGL context failed: {e}"));
        }
    }

    fn done_current(&self) {
        let egl = self.egl;
        if let Err(e) = restore(egl, self.display, self.previous.take()) {
            Log::error(Event::GraphicsBackend, format!("Removing OpenGL context failed: {e}"));
        }
        if let Some(previous) = self.previous_api.take()
            && previous != self.api.egl()
            && let Err(e) = egl.bind_api(previous)
        {
            Log::error(Event::GraphicsBackend, format!("Error setting the EGL rendering API back: {e}"));
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
