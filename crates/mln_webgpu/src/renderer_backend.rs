// The backend and the context share the device and queue (`Rc<BackendImpl>`),
// and resources hold an `Rc<Context>`.
//
// What the backend draws into is a target built on it (`OffscreenTarget`,
// `SurfaceTarget`, `BorrowedTarget`), whose textures it imports into the
// backend's `WebGpu`.

use std::cell::RefCell;
use std::rc::Rc;

use mln_gfx::context::Context as GfxContext;
use mln_gfx::released::Released;
use mln_gfx::renderer_backend::{self, RendererBackendBase};

use crate::context::Context;
use crate::device::Device;
use crate::gpu::WebGpu;

pub(crate) struct BackendImpl {
    /// The `Gpu` the renderer's resources belong to and its draws encode
    /// through.
    gpu: RefCell<WebGpu>,
    /// The buffers the renderer dropped, which a context destroys at the
    /// end of a frame.
    released: Rc<Released>,
}

impl BackendImpl {
    fn new(device: &Device) -> Self {
        BackendImpl { gpu: RefCell::new(WebGpu::new(device)), released: Rc::default() }
    }

    pub(crate) fn gpu(&self) -> &RefCell<WebGpu> {
        &self.gpu
    }

    pub(crate) fn released(&self) -> &Rc<Released> {
        &self.released
    }
}

/// A renderer's context on a [`Device`].
pub struct RendererBackend {
    base: RendererBackendBase,
    impl_: Rc<BackendImpl>,
    device: Device,
}

impl RendererBackend {
    /// A backend on `device`. Backends on one device render independently,
    /// each submitting to the device's queue.
    #[must_use]
    pub fn new(device: &Device) -> Self {
        let impl_ = BackendImpl::new(device);
        RendererBackend { base: RendererBackendBase::new(), impl_: Rc::new(impl_), device: device.clone() }
    }

    /// The device the backend renders on.
    pub fn device(&self) -> &Device {
        &self.device
    }

    /// The device state that the backend's contexts share, with its
    /// `WebGpu`.
    pub(crate) fn backend_impl(&self) -> &Rc<BackendImpl> {
        &self.impl_
    }

    /// Drops the context, releasing the resources it caches first.
    pub(crate) fn reset_context(&mut self) {
        if let Some(context) = self.base.reset_context() {
            context.release_cached_resources();
        }
    }

    pub(crate) fn create_context(&self) -> Rc<Context> {
        Context::new(Rc::clone(&self.impl_))
    }
}

impl renderer_backend::RendererBackend for RendererBackend {
    fn renderer_backend_base(&self) -> &RendererBackendBase {
        &self.base
    }

    fn create_context(&self) -> Rc<dyn GfxContext> {
        RendererBackend::create_context(self)
    }

    /// WebGPU has no context to make current.
    fn activate(&self) {}
    fn deactivate(&self) {}
}

impl Drop for RendererBackend {
    fn drop(&mut self) {
        self.reset_context();
    }
}
