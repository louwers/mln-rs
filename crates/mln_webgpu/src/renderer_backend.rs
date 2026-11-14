use std::cell::{RefCell, RefMut};
use std::rc::Rc;

use mln_gfx::backend::Type as BackendType;
use mln_gfx::context::{Context as GfxContext, ContextBase};
use mln_gfx::gpu::{Gpu, TextureFormat};
use mln_gfx::renderer_backend::{self, RendererBackendBase};

use crate::device::Device;
use crate::gpu::WebGpu;

/// A renderer's backend on a [`Device`]: the [`WebGpu`] that its resources
/// belong to, and that the targets built on it import their textures into.
pub struct RendererBackend {
    base: RendererBackendBase,
    gpu: Rc<RefCell<WebGpu>>,
    device: Device,
}

impl RendererBackend {
    /// A backend on `device`. Backends on one device render independently,
    /// each submitting to the device's queue.
    #[must_use]
    pub fn new(device: &Device) -> Self {
        let gpu = Rc::new(RefCell::new(WebGpu::new(device)));
        RendererBackend { base: RendererBackendBase::new(), gpu, device: device.clone() }
    }

    /// The device the backend renders on.
    pub fn device(&self) -> &Device {
        &self.device
    }

    pub(crate) fn gpu(&self) -> RefMut<'_, WebGpu> {
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

    /// WebGPU has no context to make current.
    fn activate(&self) {}
    fn deactivate(&self) {}
}

/// The renderer's caches over the backend's [`WebGpu`].
struct Context {
    base: ContextBase,
    gpu: Rc<RefCell<WebGpu>>,
}

impl GfxContext for Context {
    fn context_base(&self) -> &ContextBase {
        &self.base
    }

    fn backend_type(&self) -> BackendType {
        BackendType::WebGPU
    }

    fn begin_frame(&self) {}

    fn end_frame(&self) {}

    fn reduce_memory_usage(&self) {}

    /// Depth only, which every render target's pass clears.
    fn render_target_depth(&self) -> Option<TextureFormat> {
        Some(TextureFormat::Depth32Float)
    }

    fn gpu(&self) -> RefMut<'_, dyn Gpu> {
        RefMut::map(self.gpu.borrow_mut(), |gpu| -> &mut dyn Gpu { gpu })
    }
}
