use mln_gfx::renderer_backend;

use crate::device::Device;
use crate::gpu::WebGpu;

/// A renderer's backend on a [`Device`]: the [`WebGpu`] that its resources
/// belong to, and that the targets built on it import their textures into.
pub struct RendererBackend {
    gpu: WebGpu,
    device: Device,
}

impl RendererBackend {
    /// A backend on `device`. Backends on one device render independently,
    /// each submitting to the device's queue.
    #[must_use]
    pub fn new(device: &Device) -> Self {
        RendererBackend { gpu: WebGpu::new(device), device: device.clone() }
    }

    /// The device the backend renders on.
    #[must_use]
    pub fn device(&self) -> &Device {
        &self.device
    }
}

impl renderer_backend::RendererBackend for RendererBackend {
    type Gpu = WebGpu;
    type Scope<'a> = &'a mut WebGpu;

    fn gpu(&mut self) -> &mut WebGpu {
        &mut self.gpu
    }
}
