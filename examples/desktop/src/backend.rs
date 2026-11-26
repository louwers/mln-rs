// The window's render session: Metal on macOS, Vulkan elsewhere than on
// Apple's platforms, D3D12 on Windows, OpenGL ES on Linux, and WebGPU
// (through wgpu) everywhere, picked with --backend and switched in the
// controls. Without --backend, the session renders on the device that the
// library picks (`Device::new`). Each backend renders in a session of its
// own, on a device of its own, which the render thread drives
// (`Driver::CallerThread`) so that the controls' overlay (overlay.rs) draws
// over each frame on that thread. On macOS both backends present to the window's one CAMetalLayer
// (metal_layer.rs). Elsewhere each makes its own surface of the window, which
// takes one session's frames at a time: the session that goes gives it up
// before the next one takes it.
//
// OpenGL ES is Linux's only: EGL is no system library on Windows, and macOS
// has neither it nor OpenGL ES.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use mln_engine::RenderMode;
use mln_runtime::{
    AttachOptions, BackendType, Completion, DemandMode, Device, DeviceOptions, Driver, DriverWork, FrameDemand,
    FrameDisposition, Map, RenderSession, RenderTarget, Surface, SurfaceOverlay,
};
use mln_style::bitmap::PremultipliedImage;
use mln_style::error::Error;
use mln_style::size::Size;

use crate::host::Inbox;
use crate::overlay::Paint;

/// A backend --backend names.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
    #[cfg(target_os = "macos")]
    Metal,
    #[cfg(not(target_vendor = "apple"))]
    Vulkan,
    #[cfg(windows)]
    D3d12,
    WebGpu,
    #[cfg(target_os = "linux")]
    OpenGl,
}

impl Kind {
    /// The backend of a --backend value.
    pub(crate) fn parse(name: &str) -> Result<Kind, String> {
        match name {
            #[cfg(target_os = "macos")]
            "metal" => Ok(Kind::Metal),
            #[cfg(not(target_vendor = "apple"))]
            "vulkan" => Ok(Kind::Vulkan),
            #[cfg(windows)]
            "d3d12" => Ok(Kind::D3d12),
            "webgpu" => Ok(Kind::WebGpu),
            #[cfg(target_os = "linux")]
            "opengl" => Ok(Kind::OpenGl),
            _ => Err(format!("Unknown backend '{name}' (available: {})", Kind::NAMES)),
        }
    }

    #[cfg(target_os = "macos")]
    const NAMES: &str = "metal, webgpu";
    #[cfg(target_os = "linux")]
    const NAMES: &str = "vulkan, opengl, webgpu";
    #[cfg(windows)]
    const NAMES: &str = "vulkan, d3d12, webgpu";
    #[cfg(all(not(target_vendor = "apple"), not(target_os = "linux"), not(windows)))]
    const NAMES: &str = "vulkan, webgpu";
    #[cfg(all(target_vendor = "apple", not(target_os = "macos")))]
    const NAMES: &str = "webgpu";

    /// The backends this platform has.
    pub(crate) const ALL: &[Kind] = &[
        #[cfg(target_os = "macos")]
        Kind::Metal,
        #[cfg(not(target_vendor = "apple"))]
        Kind::Vulkan,
        #[cfg(windows)]
        Kind::D3d12,
        #[cfg(target_os = "linux")]
        Kind::OpenGl,
        Kind::WebGpu,
    ];

    /// The backend after this one in `ALL`, around to the first.
    pub(crate) fn next(self) -> Kind {
        let index = Kind::ALL.iter().position(|&kind| kind == self).unwrap_or(0);
        Kind::ALL[(index + 1) % Kind::ALL.len()]
    }

    /// The backend's name in the controls.
    pub(crate) fn name(self) -> &'static str {
        match self {
            #[cfg(target_os = "macos")]
            Kind::Metal => "Metal",
            #[cfg(not(target_vendor = "apple"))]
            Kind::Vulkan => "Vulkan",
            #[cfg(windows)]
            Kind::D3d12 => "D3D12",
            Kind::WebGpu => "WebGPU",
            #[cfg(target_os = "linux")]
            Kind::OpenGl => "OpenGL ES",
        }
    }

    /// The kind of a device of `backend`, if the app renders with it.
    fn of(backend: BackendType) -> Option<Kind> {
        Kind::ALL.iter().copied().find(|kind| kind.backend_type() == backend)
    }

    fn backend_type(self) -> BackendType {
        match self {
            #[cfg(target_os = "macos")]
            Kind::Metal => BackendType::Metal,
            #[cfg(not(target_vendor = "apple"))]
            Kind::Vulkan => BackendType::Vulkan,
            #[cfg(windows)]
            Kind::D3d12 => BackendType::D3D12,
            Kind::WebGpu => BackendType::WebGPU,
            #[cfg(target_os = "linux")]
            Kind::OpenGl => BackendType::OpenGL,
        }
    }

    /// The overlay that paints the controls' `paint` over each frame.
    fn overlay(self, paint: Rc<RefCell<Paint>>) -> SurfaceOverlay {
        match self {
            #[cfg(target_os = "macos")]
            Kind::Metal => SurfaceOverlay::Metal(crate::metal_overlay::metal_overlay(paint)),
            #[cfg(not(target_vendor = "apple"))]
            Kind::Vulkan => SurfaceOverlay::Vulkan(crate::vulkan_overlay::vulkan_overlay(paint)),
            #[cfg(windows)]
            Kind::D3d12 => SurfaceOverlay::D3d12(crate::d3d12_overlay::d3d12_overlay(paint)),
            Kind::WebGpu => SurfaceOverlay::WebGpu(crate::overlay::webgpu_overlay(paint)),
            #[cfg(target_os = "linux")]
            Kind::OpenGl => SurfaceOverlay::Gl(crate::gl_overlay::gl_overlay(paint)),
        }
    }
}

/// The device that the library picks when the host names no backend, and
/// its kind. Fails when the app has no overlay for its backend.
pub(crate) fn default_device() -> Result<(Kind, Device), Error> {
    let device = Device::new(DeviceOptions::default())?;
    let backend = device.backend();
    let kind = Kind::of(backend).ok_or_else(|| {
        Error::unsupported(format!(
            "The library picked {backend:?}, which this app doesn't render with (available: {})",
            Kind::NAMES
        ))
    })?;
    Ok((kind, device))
}

/// The window that sessions make their surfaces of, off the window's thread:
/// on Windows with the handles that the window's thread took
/// (`win32_window.rs`).
#[cfg(all(not(target_os = "macos"), not(windows)))]
pub(crate) type SessionWindow = Arc<winit::window::Window>;
#[cfg(windows)]
pub(crate) type SessionWindow = Arc<crate::win32_window::SurfaceWindow>;

/// Makes the window's surfaces: on macOS through the window's thread, which
/// makes the window's Metal layer and the wgpu surfaces of it, and elsewhere
/// from the window itself.
pub(crate) struct WindowSurfaces {
    #[cfg(target_os = "macos")]
    pub(crate) proxy: winit::event_loop::EventLoopProxy<crate::host::UserEvent>,
    #[cfg(not(target_os = "macos"))]
    pub(crate) window: SessionWindow,
    /// Whether the sessions present a frame per refresh of the display:
    /// off in benchmark mode, which measures frame times.
    pub(crate) cap_frame_rate: bool,
}

impl WindowSurfaces {
    /// A surface of the window that a session of `kind` on `device`
    /// presents to.
    #[cfg(target_os = "macos")]
    fn surface(&self, kind: Kind, device: &Device) -> Result<Surface, Error> {
        use std::sync::mpsc;

        use crate::host::UserEvent;

        let closed = || Error::cancelled("The window closed before it had a surface");
        match kind {
            Kind::Metal => {
                let (reply, layer) = mpsc::channel();
                self.proxy.send_event(UserEvent::MakeLayer(reply)).map_err(|_| closed())?;
                let layer = layer.recv().map_err(|_| closed())?.map_err(Error::native)?;
                Ok(Surface::metal_layer(mln_mtl::Layer::new(layer.into_layer())))
            }
            Kind::WebGpu => {
                let Device::WebGpu(device) = device else {
                    return Err(Error::invalid_argument("A WebGPU surface needs a WebGPU device"));
                };
                let instance = device.instance().cloned().ok_or_else(|| Error::native("The device has no instance"))?;
                let (reply, surface) = mpsc::channel();
                self.proxy.send_event(UserEvent::MakeSurface(instance, reply)).map_err(|_| closed())?;
                Ok(Surface::wgpu(surface.recv().map_err(|_| closed())?.map_err(Error::native)?))
            }
        }
    }

    /// A surface of the window that a session of `kind` on `device`
    /// presents to.
    #[cfg(not(target_os = "macos"))]
    #[expect(clippy::unnecessary_wraps, reason = "macOS's surfaces come from the window's thread, which can fail")]
    fn surface(&self, kind: Kind, device: &Device) -> Result<Surface, Error> {
        let _ = (kind, device);
        Ok(Surface::window(Arc::clone(&self.window)))
    }
}

/// Services `driver` until `completion` completes, and returns its result.
fn serviced<T: Send + 'static>(driver: &DriverWork, completion: Completion<T>) -> Result<T, Error> {
    while !completion.is_complete() {
        driver.service()?;
        if !completion.is_complete() {
            std::thread::sleep(Duration::from_millis(1));
        }
    }
    completion.wait()
}

/// The attach options of a session that the render thread drives, which
/// wakes it through `inbox`.
fn attach_options(inbox: &Arc<Inbox>) -> AttachOptions {
    let work = Arc::clone(inbox);
    let results = Arc::clone(inbox);
    let redraw = Arc::clone(inbox);
    AttachOptions {
        driver: Driver::CallerThread,
        driver_work_wake: Some(Arc::new(move || work.wake())),
        frame_wake: Some(Arc::new(move || results.wake())),
        redraw_wake: Some(Arc::new(move || redraw.wake_for_redraw())),
        ..AttachOptions::default()
    }
}

/// The map's render session on the window, with one backend.
pub(crate) struct WindowSession {
    kind: Kind,
    device: Device,
    session: RenderSession,
    driver: DriverWork,
}

impl WindowSession {
    /// Attaches a session of `kind` to `map`, on `device` or a new device of
    /// the backend, that presents to the window at `size` logical pixels with
    /// the controls' `paint` over each frame, and wakes the render thread
    /// through `inbox`.
    pub(crate) fn attach(
        kind: Kind,
        device: Option<Device>,
        map: &Map,
        surfaces: &WindowSurfaces,
        size: Size,
        paint: Rc<RefCell<Paint>>,
        inbox: &Arc<Inbox>,
    ) -> Result<WindowSession, Error> {
        let device = match device {
            Some(device) => device,
            None => Device::with_backend(kind.backend_type(), DeviceOptions::default())?,
        };
        let mut surface = surfaces.surface(kind, &device)?;
        if !surfaces.cap_frame_rate {
            surface = surface.without_frame_rate_cap();
        }
        let target = RenderTarget::Surface { size, surface, device: Some(device.clone()) };
        let (session, attached) = map.attach(target, attach_options(inbox))?;
        let driver = session.take_driver_work()?;
        driver.set_overlay(kind.overlay(paint));
        // A session that failed to attach lets go of what it made as its
        // driver work drops.
        serviced(&driver, attached)?;
        Ok(WindowSession { kind, device, session, driver })
    }

    pub(crate) fn kind(&self) -> Kind {
        self.kind
    }

    pub(crate) fn device(&self) -> &Device {
        &self.device
    }

    pub(crate) fn session(&self) -> &RenderSession {
        &self.session
    }

    /// Runs the session's work that is ready: its frames among it.
    pub(crate) fn service(&self) -> Result<(), Error> {
        self.driver.service()
    }

    /// The GPU and how the map renders on it, for the status line. OpenGL
    /// names its GPU only in a context, which the controls' overlay has.
    pub(crate) fn adapter(&self, paint: &Paint) -> String {
        match &self.device {
            Device::WebGpu(device) => {
                let info = device.adapter_info();
                format!("{} (wgpu {:?})", info.name, info.backend)
            }
            #[cfg(target_os = "macos")]
            Device::Metal(device) => format!("{} (Metal)", device.name()),
            #[cfg(not(target_vendor = "apple"))]
            Device::Vulkan(device) => format!("{} (Vulkan)", crate::vulkan_overlay::device_name(device)),
            #[cfg(windows)]
            Device::D3d12(device) => format!("{} (D3D12)", crate::d3d12_overlay::device_name(device)),
            _ => match &paint.renderer {
                Some(renderer) => format!("{renderer} ({})", self.kind.name()),
                None => self.kind.name().to_owned(),
            },
        }
    }

    /// Releases the session, servicing it until it has let go of the
    /// window's surface and its graphics objects.
    pub(crate) fn close(self) {
        let released = self.session.release().and_then(|released| serviced(&self.driver, released));
        if let Err(e) = released {
            log::warn!("The render session didn't end cleanly: {e}");
        }
    }
}

/// Renders `map` at `size` logical pixels in an offscreen session on
/// `device`, once everything it shows has loaded or `timeout` passed, and
/// reads the frame's pixels. The map has no other session meanwhile.
pub(crate) fn read_offscreen(
    map: &Map,
    device: Device,
    size: Size,
    inbox: &Arc<Inbox>,
    timeout: Duration,
) -> Result<PremultipliedImage, Error> {
    let target = RenderTarget::Offscreen { size, device: Some(device) };
    let (session, attached) = map.attach(target, attach_options(inbox))?;
    let driver = session.take_driver_work()?;
    serviced(&driver, attached)?;
    let deadline = Instant::now() + timeout;
    // The first frame draws what the session has; the next ones wait for the
    // map's updates as the session's tiles load.
    let mut mode = DemandMode::Force;
    loop {
        session.demand_frame(FrameDemand::new(mode, 0))?;
        mode = DemandMode::WaitForUpdate;
        let mut results = Vec::new();
        while results.is_empty() && Instant::now() < deadline {
            driver.service()?;
            results = session.drain_frame_results()?;
            if results.is_empty() {
                std::thread::sleep(Duration::from_millis(1));
            }
        }
        // Labels fade in after the frame that has everything loaded.
        let settled = results.iter().any(|result| {
            result.disposition == FrameDisposition::Rendered && result.mode == RenderMode::Full && !result.needs_repaint
        });
        if settled || Instant::now() >= deadline {
            break;
        }
    }
    let image = serviced(&driver, session.read_texture()?);
    serviced(&driver, session.release()?)?;
    image
}
