//! What a render session renders into, and how it attaches.

use std::sync::Arc;

use mln_gfx::Window;
pub use mln_gfx::backend::Type as BackendType;
use mln_util::error::Error;
use mln_util::size::Size;

/// A callback that tells the host about something it should act on, such as
/// frame results to drain. It runs on a library thread and must not block.
pub type Wake = Arc<dyn Fn() + Send + Sync>;

/// The thread that runs a render session's graphics work.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Driver {
    /// A thread of the session's own.
    #[default]
    CoreWorker,
    /// The host's graphics thread, which runs the work through the session's
    /// [`DriverWork`](crate::DriverWork).
    CallerThread,
}

/// How a session attaches to its map.
#[derive(Clone, Default)]
pub struct AttachOptions {
    pub driver: Driver,
    /// Runs when frame results wait for
    /// [`drain_frame_results`](crate::RenderSession::drain_frame_results),
    /// and not again until a drain takes them.
    pub frame_wake: Option<Wake>,
    /// Runs when the map has something new to draw, and not again until the
    /// host demands a frame, or while the session has no target.
    pub redraw_wake: Option<Wake>,
    /// Runs when a [`Driver::CallerThread`] session has work for
    /// [`DriverWork::service`](crate::DriverWork::service), and not again
    /// until a service starts.
    pub driver_work_wake: Option<Wake>,
}

impl std::fmt::Debug for AttachOptions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AttachOptions")
            .field("driver", &self.driver)
            .field("frame_wake", &self.frame_wake.is_some())
            .field("redraw_wake", &self.redraw_wake.is_some())
            .field("driver_work_wake", &self.driver_work_wake.is_some())
            .finish()
    }
}

/// What a render session renders into. The session builds it into the
/// backend's objects on the thread that renders, on `device`, or on a device
/// that [`Device::new`] makes when that is `None`. The device's backend is
/// the session's.
#[derive(Debug)]
pub enum RenderTarget {
    /// An image of `size` logical pixels, rendered at the map's pixel ratio,
    /// whose pixels [`read_texture`](crate::RenderSession::read_texture)
    /// reads.
    Offscreen { size: Size, device: Option<Device> },
    /// A surface of `size` logical pixels, rendered at the map's pixel ratio,
    /// that shows each frame that the session renders. Without a device, a
    /// window renders on the platform's native backend where that takes
    /// windows, and on WebGPU otherwise (on Apple's platforms, whose Metal
    /// takes a layer); a Metal layer renders on Metal.
    ///
    /// [`release_target`](crate::RenderSession::release_target) lets go of
    /// the surface, and [`set_surface`](crate::RenderSession::set_surface)
    /// gives the session another one.
    Surface { size: Size, surface: Surface, device: Option<Device> },
}

/// What a surface session presents to.
pub struct Surface {
    pub(crate) kind: SurfaceKind,
}

pub(crate) enum SurfaceKind {
    Window(Arc<dyn Window>),
    #[cfg(all(feature = "metal", target_vendor = "apple"))]
    MetalLayer(mln_mtl::Layer),
    #[cfg(all(feature = "webgpu", not(target_family = "wasm")))]
    Wgpu(wgpu::Surface<'static>),
}

impl Surface {
    /// A window, which the session makes its surface on. The window must
    /// outlive the session's use of it: until a
    /// [`set_surface`](crate::RenderSession::set_surface) or
    /// [`release_target`](crate::RenderSession::release_target) replaces it,
    /// or the session detaches or is abandoned.
    pub fn window(window: impl Window + 'static) -> Surface {
        Surface { kind: SurfaceKind::Window(Arc::new(window)) }
    }

    /// A `CAMetalLayer`, which a session on Metal presents to. It must
    /// outlive the session's use of it, as a window must.
    #[cfg(all(feature = "metal", target_vendor = "apple"))]
    pub fn metal_layer(layer: mln_mtl::Layer) -> Surface {
        Surface { kind: SurfaceKind::MetalLayer(layer) }
    }

    /// A surface that the host made with the instance of the WebGPU
    /// [`Device`] that the session renders on.
    #[cfg(all(feature = "webgpu", not(target_family = "wasm")))]
    pub fn wgpu(surface: wgpu::Surface<'static>) -> Surface {
        Surface { kind: SurfaceKind::Wgpu(surface) }
    }

    /// The surface again, if it is a window.
    pub(crate) fn clone_window(&self) -> Option<Surface> {
        match &self.kind {
            SurfaceKind::Window(window) => Some(Surface { kind: SurfaceKind::Window(window.clone()) }),
            #[allow(unreachable_patterns)]
            _ => None,
        }
    }

    /// Whether the session can make the surface without the host's device.
    pub(crate) fn needs_no_device(&self) -> bool {
        match self.kind {
            SurfaceKind::Window(_) => true,
            #[cfg(all(feature = "metal", target_vendor = "apple"))]
            SurfaceKind::MetalLayer(_) => true,
            #[cfg(all(feature = "webgpu", not(target_family = "wasm")))]
            SurfaceKind::Wgpu(_) => false,
        }
    }
}

impl std::fmt::Debug for Surface {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Surface").finish_non_exhaustive()
    }
}

/// A GPU that sessions on many maps and threads share: one backend's
/// device. Sessions on it keep working after the last handle drops, until
/// they detach.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub enum Device {
    /// A WebGPU device: the library's, through wgpu, or the host's
    /// ([`Device::webgpu`]).
    #[cfg(all(feature = "webgpu", not(target_family = "wasm")))]
    WebGpu(mln_webgpu::Device),
    /// A Metal device and command queue.
    #[cfg(all(feature = "metal", target_vendor = "apple"))]
    Metal(mln_mtl::Device),
    /// A Vulkan GPU.
    #[cfg(all(feature = "vulkan", not(any(target_vendor = "apple", target_family = "wasm"))))]
    Vulkan(mln_vulkan::Device),
    /// An OpenGL ES share group.
    #[cfg(all(feature = "gl", not(target_family = "wasm")))]
    Gl(mln_gl::Device),
}

/// How to create a [`Device`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct DeviceOptions {
    /// The GPU to use. `None` picks the default one.
    pub adapter: Option<Adapter>,
}

/// A GPU, by what names it to a backend.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Adapter {
    /// An index into the backend's list of GPUs.
    Index(usize),
    /// The GPU's 16-byte UUID, as Vulkan's
    /// `VkPhysicalDeviceIDProperties.deviceUUID` reports it.
    Uuid([u8; 16]),
    /// The GPU's 8-byte LUID, as Vulkan's
    /// `VkPhysicalDeviceIDProperties.deviceLUID` reports it: on Windows, its
    /// DXGI adapter's.
    Luid([u8; 8]),
}

/// A kind of window or layer that a session presents to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum SurfaceType {
    /// A `CAMetalLayer`.
    MetalLayer,
    /// A Win32 window.
    Win32,
    /// An X11 window, through Xlib.
    Xlib,
    /// An X11 window, through XCB.
    Xcb,
    /// A Wayland surface.
    Wayland,
    /// An Android `ANativeWindow`.
    AndroidNdk,
}

impl SurfaceType {
    /// Whether a device of `backend` presents to the surfaces of this type
    /// on this platform, in this build.
    pub fn presents_with(self, backend: BackendType) -> bool {
        let windowing = match self {
            SurfaceType::MetalLayer => false,
            SurfaceType::Win32 => cfg!(windows),
            SurfaceType::Xlib | SurfaceType::Xcb | SurfaceType::Wayland => {
                cfg!(all(unix, not(target_vendor = "apple"), not(target_os = "android")))
            }
            SurfaceType::AndroidNdk => cfg!(target_os = "android"),
        };
        match backend {
            BackendType::Metal => {
                cfg!(all(feature = "metal", target_vendor = "apple")) && self == SurfaceType::MetalLayer
            }
            BackendType::Vulkan => {
                cfg!(all(feature = "vulkan", not(any(target_vendor = "apple", target_family = "wasm")))) && windowing
            }
            BackendType::OpenGL => {
                cfg!(all(feature = "gl", unix, not(target_vendor = "apple"), not(target_os = "android")))
                    && matches!(self, SurfaceType::Xlib | SurfaceType::Wayland)
            }
            BackendType::WebGPU => cfg!(all(feature = "webgpu", not(target_family = "wasm"))) && windowing,
            #[allow(unreachable_patterns)]
            _ => false,
        }
    }

    /// Whether a device of any backend of this build presents to the
    /// surfaces of this type.
    pub fn presents_with_any(self) -> bool {
        [BackendType::Metal, BackendType::Vulkan, BackendType::OpenGL, BackendType::WebGPU]
            .into_iter()
            .any(|backend| self.presents_with(backend))
    }

    /// Whether a session of this build presents to the surfaces of this
    /// type without the host's device: on the device that a surface session
    /// makes when it gets none.
    pub fn presents_without_device(self) -> bool {
        match self {
            SurfaceType::MetalLayer => self.presents_with(BackendType::Metal),
            _ if cfg!(target_vendor = "apple") => self.presents_with(BackendType::WebGPU),
            _ => [BackendType::Vulkan, BackendType::OpenGL, BackendType::WebGPU]
                .into_iter()
                .any(|backend| self.presents_with(backend)),
        }
    }
}

/// Whether a backend that failed with `error` is missing, so the next one
/// may take its place.
#[cfg(not(target_family = "wasm"))]
fn missing(error: &Error) -> bool {
    error.kind == mln_util::ErrorKind::Unsupported
}

impl Device {
    /// The library's device, on the platform's native backend: Metal on
    /// Apple's platforms, and Vulkan elsewhere, or OpenGL ES where there is
    /// no Vulkan. WebGPU (wgpu) takes the place of a native backend that
    /// isn't there. A backend that can't name the GPU that `options` names
    /// is skipped: OpenGL ES names none, and only Vulkan names one by UUID or
    /// LUID.
    ///
    /// Fails with [`ErrorKind::NotFound`](mln_util::ErrorKind) when
    /// the options name no GPU, with
    /// [`ErrorKind::Unsupported`](mln_util::ErrorKind) when no backend has a
    /// GPU or in the browser, where a session renders on the page's device,
    /// and with [`ErrorKind::Native`](mln_util::ErrorKind) when the GPU
    /// can't create the device.
    pub fn new(options: DeviceOptions) -> Result<Device, Error> {
        #[cfg(target_family = "wasm")]
        {
            let _ = options;
            Err(Error::unsupported("the browser has no library devices"))
        }
        #[cfg(not(target_family = "wasm"))]
        {
            let mut error = Error::unsupported("this build has no backend");
            for backend in Self::native_order() {
                match Self::with_backend(*backend, options) {
                    Ok(device) => return Ok(device),
                    Err(e) if missing(&e) => error = e,
                    Err(e) => return Err(e),
                }
            }
            Err(error)
        }
    }

    /// The library's device on `backend`.
    ///
    /// Fails as [`new`](Self::new) does, and with
    /// [`ErrorKind::Unsupported`](mln_util::ErrorKind) for a backend that
    /// isn't in this build or on this platform.
    pub fn with_backend(backend: BackendType, options: DeviceOptions) -> Result<Device, Error> {
        let DeviceOptions { adapter } = options;
        // Only Vulkan names its GPUs by ID here.
        let index = |adapter: Option<Adapter>| match adapter {
            None => Ok(None),
            Some(Adapter::Index(index)) => Ok(Some(index)),
            Some(_) => Err(Error::unsupported(format!("{backend:?} names no GPU by UUID or LUID"))),
        };
        match backend {
            #[cfg(all(feature = "webgpu", not(target_family = "wasm")))]
            BackendType::WebGPU => mln_webgpu::Device::new(index(adapter)?).map(Device::WebGpu),
            #[cfg(all(feature = "metal", target_vendor = "apple"))]
            BackendType::Metal => mln_mtl::Device::new(index(adapter)?).map(Device::Metal),
            #[cfg(all(feature = "vulkan", not(any(target_vendor = "apple", target_family = "wasm"))))]
            BackendType::Vulkan => match adapter {
                Some(Adapter::Uuid(uuid)) => mln_vulkan::Device::with_adapter_id(mln_vulkan::AdapterId::Uuid(uuid)),
                Some(Adapter::Luid(luid)) => mln_vulkan::Device::with_adapter_id(mln_vulkan::AdapterId::Luid(luid)),
                _ => mln_vulkan::Device::new(index(adapter)?),
            }
            .map(Device::Vulkan),
            #[cfg(all(feature = "gl", not(target_family = "wasm")))]
            BackendType::OpenGL => match adapter {
                Some(_) => Err(Error::unsupported("OpenGL ES picks no GPU")),
                None => mln_gl::Device::new().map(Device::Gl),
            },
            #[allow(unreachable_patterns)]
            _ => {
                let _ = (index, adapter);
                Err(Error::unsupported(format!("this build has no {backend:?} backend")))
            }
        }
    }

    /// Whether sessions on the device present to the surfaces of
    /// `surface`'s type.
    pub fn presents_to(&self, surface: SurfaceType) -> bool {
        surface.presents_with(self.backend())
    }

    /// The backends [`new`](Self::new) tries, in order.
    #[cfg(not(target_family = "wasm"))]
    fn native_order() -> &'static [BackendType] {
        if cfg!(target_vendor = "apple") {
            &[BackendType::Metal, BackendType::WebGPU]
        } else {
            &[BackendType::Vulkan, BackendType::OpenGL, BackendType::WebGPU]
        }
    }

    /// The host's WebGPU device and queue, which `instance` made. The
    /// session configures its surfaces with `format`, which each surface
    /// must support.
    #[cfg(all(feature = "webgpu", not(target_family = "wasm")))]
    pub fn webgpu(
        instance: wgpu::Instance,
        device: wgpu::Device,
        queue: wgpu::Queue,
        format: wgpu::TextureFormat,
    ) -> Device {
        Device::WebGpu(mln_webgpu::Device::from_wgpu(Some(instance), device, queue, Some(format)))
    }

    /// The backend the device is of.
    pub fn backend(&self) -> BackendType {
        match *self {
            #[cfg(all(feature = "webgpu", not(target_family = "wasm")))]
            Device::WebGpu(_) => BackendType::WebGPU,
            #[cfg(all(feature = "metal", target_vendor = "apple"))]
            Device::Metal(_) => BackendType::Metal,
            #[cfg(all(feature = "vulkan", not(any(target_vendor = "apple", target_family = "wasm"))))]
            Device::Vulkan(_) => BackendType::Vulkan,
            #[cfg(all(feature = "gl", not(target_family = "wasm")))]
            Device::Gl(_) => BackendType::OpenGL,
        }
    }
}
