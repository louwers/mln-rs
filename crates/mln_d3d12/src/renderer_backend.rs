use std::cell::{RefCell, RefMut};
use std::rc::Rc;

use mln_gfx::gpu::{Gpu, GpuError};
use mln_gfx::renderer_backend;

use crate::device::Device;
use crate::gpu::D3d12Gpu;

/// A renderer's backend on a [`Device`]: the [`D3d12Gpu`] that its resources
/// belong to.
pub struct RendererBackend {
    gpu: Rc<RefCell<D3d12Gpu>>,
}

impl RendererBackend {
    /// A backend on `device`. Backends on one device render independently,
    /// each submitting to the device's queue. Fails as [`D3d12Gpu::new`]
    /// does.
    pub fn new(device: &Device) -> Result<Self, GpuError> {
        Ok(RendererBackend { gpu: Rc::new(RefCell::new(D3d12Gpu::new(device)?)) })
    }

    /// The backend's [`D3d12Gpu`].
    #[must_use]
    pub fn d3d12_gpu(&self) -> RefMut<'_, D3d12Gpu> {
        self.gpu.borrow_mut()
    }
}

impl renderer_backend::RendererBackend for RendererBackend {
    fn gpu(&self) -> RefMut<'_, dyn Gpu> {
        RefMut::map(self.gpu.borrow_mut(), |gpu| -> &mut dyn Gpu { gpu })
    }
}
