use std::cell::{RefCell, RefMut};
use std::rc::Rc;

use mln_gfx::backend::Type as BackendType;
use mln_gfx::context::{Context as GfxContext, ContextBase};
use mln_gfx::gpu::Gpu;
use mln_gfx::renderer_backend::{self, RendererBackendBase};

use crate::device::Device;
use crate::gpu::MetalGpu;

/// A renderer's backend on a [`Device`]: the [`MetalGpu`] that its
/// resources belong to, and that the targets built on it import their
/// textures into.
pub struct RendererBackend {
    base: RendererBackendBase,
    gpu: Rc<RefCell<MetalGpu>>,
    device: Device,
}

impl RendererBackend {
    /// A backend on `device`, which submits to the device's command queue.
    #[must_use]
    pub fn new(device: &Device) -> Self {
        let gpu = Rc::new(RefCell::new(MetalGpu::new(device)));
        RendererBackend { base: RendererBackendBase::new(), gpu, device: device.clone() }
    }

    /// The device the backend renders on.
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
    fn renderer_backend_base(&self) -> &RendererBackendBase {
        &self.base
    }

    fn create_context(&self) -> Rc<dyn GfxContext> {
        Rc::new(Context { base: ContextBase::default(), gpu: Rc::clone(&self.gpu) })
    }

    /// Metal has no context to make current. `MetalGpu` and the targets
    /// drain autorelease pools of their own.
    fn activate(&self) {}
    fn deactivate(&self) {}
}

/// The renderer's caches over the backend's [`MetalGpu`].
struct Context {
    base: ContextBase,
    gpu: Rc<RefCell<MetalGpu>>,
}

impl GfxContext for Context {
    fn context_base(&self) -> &ContextBase {
        &self.base
    }

    fn backend_type(&self) -> BackendType {
        BackendType::Metal
    }

    fn begin_frame(&self) {}

    fn end_frame(&self) {}

    fn reduce_memory_usage(&self) {}

    fn gpu(&self) -> RefMut<'_, dyn Gpu> {
        RefMut::map(self.gpu.borrow_mut(), |gpu| -> &mut dyn Gpu { gpu })
    }
}
