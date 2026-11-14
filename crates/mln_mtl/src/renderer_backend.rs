use std::cell::{RefCell, RefMut};
use std::rc::Rc;

use mln_gfx::gpu::Gpu;
use mln_gfx::renderer_backend;

use crate::device::Device;
use crate::gpu::MetalGpu;

/// A renderer's backend on a [`Device`]: the [`MetalGpu`] that its
/// resources belong to, and that the targets built on it import their
/// textures into.
pub struct RendererBackend {
    gpu: Rc<RefCell<MetalGpu>>,
    device: Device,
}

impl RendererBackend {
    /// A backend on `device`, which submits to the device's command queue.
    #[must_use]
    pub fn new(device: &Device) -> Self {
        let gpu = Rc::new(RefCell::new(MetalGpu::new(device)));
        RendererBackend { gpu, device: device.clone() }
    }

    /// The device the backend renders on.
    #[must_use]
    pub fn device(&self) -> &Device {
        &self.device
    }

    /// The backend's `Gpu`, which targets that outlive a borrow of the
    /// backend share.
    pub(crate) fn shared_gpu(&self) -> &Rc<RefCell<MetalGpu>> {
        &self.gpu
    }

    pub(crate) fn gpu(&self) -> RefMut<'_, MetalGpu> {
        self.gpu.borrow_mut()
    }
}

impl renderer_backend::RendererBackend for RendererBackend {
    fn gpu(&self) -> RefMut<'_, dyn Gpu> {
        RefMut::map(self.gpu.borrow_mut(), |gpu| -> &mut dyn Gpu { gpu })
    }
}
