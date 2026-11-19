use mln_gfx::renderer_backend;

use crate::device::Device;
use crate::gpu::MetalGpu;

/// A renderer's backend on a [`Device`]: the [`MetalGpu`] that its
/// resources belong to, and that the targets built on it import their
/// textures into.
pub struct RendererBackend {
    gpu: MetalGpu,
    device: Device,
}

impl RendererBackend {
    /// A backend on `device`, which submits to the device's command queue.
    #[must_use]
    pub fn new(device: &Device) -> Self {
        RendererBackend { gpu: MetalGpu::new(device), device: device.clone() }
    }

    /// The device the backend renders on.
    #[must_use]
    pub fn device(&self) -> &Device {
        &self.device
    }
}

impl renderer_backend::RendererBackend for RendererBackend {
    type Gpu = MetalGpu;
    type Scope<'a> = &'a mut MetalGpu;

    fn gpu(&mut self) -> &mut MetalGpu {
        &mut self.gpu
    }
}
