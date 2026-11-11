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
    /// The host's graphics thread.
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
    /// host demands a frame.
    pub redraw_wake: Option<Wake>,
}

impl std::fmt::Debug for AttachOptions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AttachOptions")
            .field("driver", &self.driver)
            .field("frame_wake", &self.frame_wake.is_some())
            .field("redraw_wake", &self.redraw_wake.is_some())
            .finish()
    }
}

/// What a render session renders into.
#[derive(Clone, Debug)]
pub enum RenderTarget {
    /// An image of `size` logical pixels, rendered at the map's pixel ratio,
    /// whose pixels [`read_texture`](crate::RenderSession::read_texture)
    /// reads. It renders on `device`, or on a device of its own when that is
    /// `None`.
    Offscreen { size: Size, device: Option<OffscreenDevice> },
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
