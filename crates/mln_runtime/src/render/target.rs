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
    /// The GPU to use, as an index into the backend's list of GPUs. `None`
    /// picks the default one.
    pub adapter_index: Option<usize>,
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
    /// isn't there. OpenGL ES picks no GPU, so with an `adapter_index` it is
    /// skipped.
    ///
    /// Fails with [`ErrorKind::NotFound`](mln_util::ErrorKind) when
    /// `adapter_index` names no GPU, with
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
        let DeviceOptions { adapter_index } = options;
        match backend {
            #[cfg(all(feature = "webgpu", not(target_family = "wasm")))]
            BackendType::WebGPU => mln_webgpu::Device::new(adapter_index).map(Device::WebGpu),
            #[cfg(all(feature = "metal", target_vendor = "apple"))]
            BackendType::Metal => mln_mtl::Device::new(adapter_index).map(Device::Metal),
            #[cfg(all(feature = "vulkan", not(any(target_vendor = "apple", target_family = "wasm"))))]
            BackendType::Vulkan => mln_vulkan::Device::new(adapter_index).map(Device::Vulkan),
            #[cfg(all(feature = "gl", not(target_family = "wasm")))]
            BackendType::OpenGL => match adapter_index {
                Some(_) => Err(Error::unsupported("OpenGL ES picks no GPU")),
                None => mln_gl::Device::new().map(Device::Gl),
            },
            #[allow(unreachable_patterns)]
            _ => {
                let _ = adapter_index;
                Err(Error::unsupported(format!("this build has no {backend:?} backend")))
            }
        }
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
