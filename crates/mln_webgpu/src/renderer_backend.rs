// The backend and the context share the device and queue (`Rc<BackendImpl>`),
// and resources hold an `Rc<Context>`.
//
// What the backend draws into is a target built on it (`OffscreenTarget`,
// `SurfaceTarget`), whose draw targets give the render passes their views.

use std::cell::RefCell;
use std::rc::Rc;

use mln_gfx::context::Context as GfxContext;
use mln_gfx::gpu::TextureId;
use mln_gfx::released::Released;
use mln_gfx::renderer_backend::{self, RendererBackendBase};

use crate::context::Context;
use crate::device::Device;
use crate::gpu::WebGpu;

pub(crate) struct BackendImpl {
    device: wgpu::Device,
    queue: wgpu::Queue,
    /// The `Gpu` the renderer's resources belong to and its draws encode
    /// through.
    gpu: RefCell<WebGpu>,
    /// The buffers the renderer dropped, which a context destroys at the
    /// end of a frame.
    released: Rc<Released>,
}

impl BackendImpl {
    fn new(device: &Device) -> Self {
        BackendImpl {
            device: device.device().clone(),
            queue: device.queue().clone(),
            gpu: RefCell::new(WebGpu::new(device)),
            released: Rc::default(),
        }
    }

    pub(crate) fn gpu(&self) -> &RefCell<WebGpu> {
        &self.gpu
    }

    /// The wgpu texture of `texture`, and its view.
    pub(crate) fn native_texture(&self, texture: TextureId) -> Option<(wgpu::Texture, wgpu::TextureView)> {
        let gpu = self.gpu.borrow();
        gpu.native_texture(texture).map(|(texture, view)| (texture.clone(), view.clone()))
    }

    pub(crate) fn released(&self) -> &Rc<Released> {
        &self.released
    }
    pub(crate) fn device(&self) -> &wgpu::Device {
        &self.device
    }
    pub(crate) fn queue(&self) -> &wgpu::Queue {
        &self.queue
    }

    /// Submits `command_buffer` after the copies of the writes made to the
    /// `Gpu`'s buffers so far.
    pub(crate) fn submit(&self, command_buffer: wgpu::CommandBuffer) {
        self.gpu.borrow_mut().submit_native(command_buffer);
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

    /// The backend's context, made on first use. Call inside a
    /// `BackendScope`.
    /// The device state that the backend's contexts share, with its
    /// `WebGpu`.
    pub(crate) fn backend_impl(&self) -> &Rc<BackendImpl> {
        &self.impl_
    }

    pub(crate) fn webgpu_context(&self) -> Option<Rc<Context>> {
        let context = renderer_backend::RendererBackend::get_context(self);
        context.downcast_ref::<Context>().map(Context::rc)
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
