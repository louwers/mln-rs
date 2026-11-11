//! What a render session renders into, and how it attaches.

use std::sync::Arc;

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

/// What a render session renders into.
#[derive(Debug)]
pub enum RenderTarget {
    /// An image of `size` logical pixels, rendered at the map's pixel ratio,
    /// whose pixels [`read_texture`](crate::RenderSession::read_texture)
    /// reads. It renders on `device`, or on a device of its own when that is
    /// `None`.
    Offscreen { size: Size, device: Option<OffscreenDevice> },
    /// A surface of `size` logical pixels, rendered at the map's pixel ratio,
    /// that shows each frame that the session renders. It renders on
    /// `device`, or, for a [`Surface::window`], on a device of its own when
    /// that is `None`.
    ///
    /// [`release_target`](crate::RenderSession::release_target) lets go of
    /// the surface, and [`set_surface`](crate::RenderSession::set_surface)
    /// gives the session another one.
    Surface { size: Size, surface: Surface, device: Option<HostDevice> },
}

/// What a surface session presents to.
pub struct Surface {
    pub(crate) kind: SurfaceKind,
}

pub(crate) enum SurfaceKind {
    #[cfg(not(target_family = "wasm"))]
    Window(Arc<dyn wgpu::DisplayAndWindowHandle>),
    #[cfg(not(target_family = "wasm"))]
    Wgpu(wgpu::Surface<'static>),
}

impl Surface {
    /// Whether the session makes the surface on a window.
    pub(crate) fn is_window(&self) -> bool {
        match self.kind {
            #[cfg(not(target_family = "wasm"))]
            SurfaceKind::Window(_) => true,
            #[cfg(not(target_family = "wasm"))]
            SurfaceKind::Wgpu(_) => false,
        }
    }
}

impl std::fmt::Debug for Surface {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Surface").finish_non_exhaustive()
    }
}

#[cfg(not(target_family = "wasm"))]
impl Surface {
    /// A window, which the session makes its surface on with the instance of
    /// its device. The window must outlive the session's use of it: until a
    /// [`set_surface`](crate::RenderSession::set_surface) or
    /// [`release_target`](crate::RenderSession::release_target) replaces it,
    /// or the session detaches or is abandoned.
    pub fn window(window: impl wgpu::DisplayAndWindowHandle + 'static) -> Surface {
        Surface { kind: SurfaceKind::Window(Arc::new(window)) }
    }

    /// A surface that the host made with the instance of the [`HostDevice`]
    /// that the session renders on.
    pub fn wgpu(surface: wgpu::Surface<'static>) -> Surface {
        Surface { kind: SurfaceKind::Wgpu(surface) }
    }
}

/// A GPU device of the host's, which a surface session renders on.
#[derive(Clone)]
pub struct HostDevice {
    #[cfg(not(target_family = "wasm"))]
    pub(crate) instance: wgpu::Instance,
    #[cfg(not(target_family = "wasm"))]
    pub(crate) device: wgpu::Device,
    #[cfg(not(target_family = "wasm"))]
    pub(crate) queue: wgpu::Queue,
    #[cfg(not(target_family = "wasm"))]
    pub(crate) format: wgpu::TextureFormat,
}

impl std::fmt::Debug for HostDevice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HostDevice").finish_non_exhaustive()
    }
}

#[cfg(not(target_family = "wasm"))]
impl HostDevice {
    /// The device and queue that `instance` made. The session configures its
    /// surfaces with `format`, which each surface must support.
    pub fn new(
        instance: wgpu::Instance,
        device: wgpu::Device,
        queue: wgpu::Queue,
        format: wgpu::TextureFormat,
    ) -> HostDevice {
        HostDevice { instance, device, queue, format }
    }
}

/// How to create an [`OffscreenDevice`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct OffscreenDeviceOptions {
    /// The GPU to use, as an index into the platform's list of GPUs. `None`
    /// picks the high-performance one.
    pub adapter_index: Option<usize>,
}

/// A GPU device that offscreen sessions on many maps can share. Sessions on
/// it keep working after the last handle drops, until they detach.
#[derive(Clone)]
pub struct OffscreenDevice {
    #[cfg(not(target_family = "wasm"))]
    pub(crate) device: wgpu::Device,
    #[cfg(not(target_family = "wasm"))]
    pub(crate) queue: wgpu::Queue,
}

impl std::fmt::Debug for OffscreenDevice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OffscreenDevice").finish_non_exhaustive()
    }
}

impl OffscreenDevice {
    /// Creates a device.
    ///
    /// Fails with [`ErrorKind::NotFound`](mln_util::ErrorKind) when
    /// `adapter_index` names no GPU, with
    /// [`ErrorKind::Unsupported`](mln_util::ErrorKind) when there is no GPU or
    /// in the browser, where a session renders on the page's device, and with
    /// [`ErrorKind::Native`](mln_util::ErrorKind) when the GPU can't create
    /// the device.
    pub fn new(options: OffscreenDeviceOptions) -> Result<OffscreenDevice, Error> {
        #[cfg(not(target_family = "wasm"))]
        {
            let (device, queue) = mln_webgpu::HeadlessBackend::request_device(options.adapter_index)?;
            Ok(OffscreenDevice { device, queue })
        }
        #[cfg(target_family = "wasm")]
        {
            let _ = options;
            Err(Error::unsupported("the browser has no offscreen devices"))
        }
    }
}
