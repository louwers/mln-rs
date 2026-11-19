// A renderer's backend on a `Device`, which backends on any thread share:
// its `VulkanGpu`, which owns what it makes on the device. A window target
// gets its surface from a `SurfaceProvider`.

use std::ffi::CStr;

use ash::vk;
use mln_gfx::renderer_backend;
use mln_util::error::Error;

use crate::device::Device;
use crate::gpu::VulkanGpu;

/// The platform's part of a target that presents to a window: its surface,
/// and the instance extensions it needs.
pub(crate) trait SurfaceProvider {
    /// The instance extensions the surface needs.
    fn instance_extensions(&self) -> Vec<&'static CStr>;

    /// Creates the window's surface.
    fn create_surface(&self, entry: &ash::Entry, instance: &ash::Instance) -> Result<vk::SurfaceKHR, Error>;
}

/// A renderer's device and context.
pub struct RendererBackend {
    /// The backend's `Gpu`, which owns the renderer's resources and submits
    /// its frames, made with the backend so that failing to make it is the
    /// backend's error.
    gpu: VulkanGpu,
}

impl RendererBackend {
    /// A backend on `device` for offscreen and borrowed targets, with one
    /// frame in flight.
    ///
    /// Fails with [`ErrorKind::Native`](mln_util::ErrorKind) when a Vulkan
    /// call fails.
    pub fn new(device: &Device) -> Result<Self, Error> {
        Self::with_frames(device, 1)
    }

    /// A backend on `device` that presents to windows, with two frames in
    /// flight.
    ///
    /// Fails as [`new`](Self::new) does, and with
    /// [`ErrorKind::Unsupported`](mln_util::ErrorKind) when the device can't
    /// present.
    pub fn presenting(device: &Device) -> Result<Self, Error> {
        if !device.presents() {
            return Err(Error::unsupported("The Vulkan device presents to no windows"));
        }
        Self::with_frames(device, 2)
    }

    fn with_frames(device: &Device, max_frames: u32) -> Result<Self, Error> {
        let gpu = VulkanGpu::with_frames(device, max_frames).map_err(|error| Error::native(error.to_string()))?;
        Ok(RendererBackend { gpu })
    }

    /// The device the backend renders on.
    pub(crate) fn device(&self) -> &Device {
        self.gpu.device()
    }

    /// The name of the GPU.
    #[must_use]
    pub fn get_device_name(&self) -> String {
        self.device()
            .shared
            .properties()
            .device_name_as_c_str()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default()
    }

    /// Whether the device renders on the CPU, as lavapipe does.
    #[must_use]
    pub fn is_software_device(&self) -> bool {
        self.device().shared.properties().device_type == vk::PhysicalDeviceType::CPU
    }
}

impl renderer_backend::RendererBackend for RendererBackend {
    type Gpu = VulkanGpu;
    type Scope<'a> = &'a mut VulkanGpu;

    fn gpu(&mut self) -> &mut VulkanGpu {
        &mut self.gpu
    }
}
