use mln_gfx::gpu::GpuError;
use mln_gfx::renderer_backend;

use crate::device::Device;
use crate::gpu::D3d12Gpu;

/// A renderer's backend on a [`Device`]: the [`D3d12Gpu`] that its resources
/// belong to.
pub struct RendererBackend {
    gpu: D3d12Gpu,
}

impl RendererBackend {
    /// A backend on `device`. Backends on one device render independently,
    /// each submitting to the device's queue. Fails as [`D3d12Gpu::new`]
    /// does.
    pub fn new(device: &Device) -> Result<Self, GpuError> {
        Ok(RendererBackend { gpu: D3d12Gpu::new(device)? })
    }

    /// The device the backend renders on.
    #[must_use]
    pub fn device(&self) -> &Device {
        self.gpu.device()
    }
}

impl renderer_backend::RendererBackend for RendererBackend {
    type Gpu = D3d12Gpu;
    type Scope<'a> = &'a mut D3d12Gpu;

    fn gpu(&mut self) -> &mut D3d12Gpu {
        &mut self.gpu
    }
}
