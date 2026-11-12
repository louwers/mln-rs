// The GPU and command queue that Metal renderers share.

use std::ffi::c_void;

use mln_util::error::Error;
use objc2::rc::Retained;
use objc2_metal::{MTLCreateSystemDefaultDevice, MTLDevice};

use crate::mtl_fwd::{MTLCommandQueuePtr, MTLDevicePtr};

/// A Metal device and the command queue that renderers on any thread
/// submit to: the library's, or the host's. Clones share them.
#[derive(Clone, Debug)]
pub struct Device {
    device: MTLDevicePtr,
    queue: MTLCommandQueuePtr,
    /// Whether the queue is the host's, which the host submits to too.
    host_queue: bool,
}

impl Device {
    /// The library's device: the GPU at `adapter_index` in
    /// `MTLCopyAllDevices` order on macOS, or the system's default GPU when
    /// `None`, with a command queue of its own.
    ///
    /// Fails with [`ErrorKind::NotFound`](mln_util::ErrorKind) when there is
    /// no GPU at `adapter_index`, with
    /// [`ErrorKind::Unsupported`](mln_util::ErrorKind) when there is no
    /// Metal device, and with [`ErrorKind::Native`](mln_util::ErrorKind)
    /// when the device makes no command queue.
    pub fn new(adapter_index: Option<usize>) -> Result<Device, Error> {
        let device = match adapter_index {
            None => MTLCreateSystemDefaultDevice().ok_or_else(|| Error::unsupported("No Metal device"))?,
            Some(index) => {
                #[cfg(target_os = "macos")]
                let devices = objc2_metal::MTLCopyAllDevices().to_vec();
                #[cfg(not(target_os = "macos"))]
                let devices: Vec<_> = MTLCreateSystemDefaultDevice().into_iter().collect();
                let count = devices.len();
                devices
                    .into_iter()
                    .nth(index)
                    .ok_or_else(|| Error::not_found(format!("adapter_index {index} is past the {count} GPUs")))?
            }
        };
        let queue = device.newCommandQueue().ok_or_else(|| Error::native("Failed to create a Metal command queue"))?;
        Ok(Device { device, queue, host_queue: false })
    }

    /// The host's device and command queue, or a queue of the device's own
    /// when `queue` is null.
    ///
    /// Fails with [`ErrorKind::InvalidArgument`](mln_util::ErrorKind) for a
    /// null device, and with [`ErrorKind::Native`](mln_util::ErrorKind) when
    /// the device makes no command queue.
    ///
    /// # Safety
    ///
    /// `device` is null or an `id<MTLDevice>`, and `queue` null or an
    /// `id<MTLCommandQueue>` of it. The device retains both.
    pub unsafe fn from_raw(device: *mut c_void, queue: *mut c_void) -> Result<Device, Error> {
        // SAFETY: the caller's contract: live objects of these protocols.
        let device: MTLDevicePtr = unsafe { Retained::retain(device.cast()) }
            .ok_or_else(|| Error::invalid_argument("the Metal device must not be null"))?;
        // SAFETY: as above.
        match unsafe { Retained::retain(queue.cast()) } {
            Some(queue) => Ok(Device { device, queue, host_queue: true }),
            None => {
                let queue =
                    device.newCommandQueue().ok_or_else(|| Error::native("Failed to create a Metal command queue"))?;
                Ok(Device { device, queue, host_queue: false })
            }
        }
    }

    /// Whether the queue is the host's, so that frames handed to the host can
    /// be ordered by the queue alone.
    pub fn has_host_queue(&self) -> bool {
        self.host_queue
    }

    /// Whether `self` and `other` are the same device and queue.
    pub fn same_as(&self, other: &Device) -> bool {
        Retained::as_ptr(&self.device) == Retained::as_ptr(&other.device)
            && Retained::as_ptr(&self.queue) == Retained::as_ptr(&other.queue)
    }

    #[must_use]
    pub fn device(&self) -> &MTLDevicePtr {
        &self.device
    }

    #[must_use]
    pub fn queue(&self) -> &MTLCommandQueuePtr {
        &self.queue
    }

    /// The name of the GPU.
    #[must_use]
    pub fn name(&self) -> String {
        self.device.name().to_string()
    }
}
