use std::cell::{RefCell, RefMut};
use std::rc::Rc;

use mln_gfx::gpu::Gpu;
use mln_gfx::renderer_backend;

use crate::device::Device;
use crate::gpu::WebGpu;

/// A renderer's backend on a [`Device`]: the [`WebGpu`] that its resources
/// belong to, and that the targets built on it import their textures into.
pub struct RendererBackend {
    gpu: Rc<RefCell<WebGpu>>,
    device: Device,
}

impl RendererBackend {
    /// A backend on `device`. Backends on one device render independently,
    /// each submitting to the device's queue.
    #[must_use]
    pub fn new(device: &Device) -> Self {
        let gpu = Rc::new(RefCell::new(WebGpu::new(device)));
        RendererBackend { gpu, device: device.clone() }
    }

    /// The device the backend renders on.
    #[must_use]
    pub fn device(&self) -> &Device {
        &self.device
    }

    pub(crate) fn gpu(&self) -> RefMut<'_, WebGpu> {
        self.gpu.borrow_mut()
    }
}

impl renderer_backend::RendererBackend for RendererBackend {
    fn gpu(&self) -> RefMut<'_, dyn Gpu> {
        RefMut::map(self.gpu.borrow_mut(), |gpu| -> &mut dyn Gpu { gpu })
    }
}
