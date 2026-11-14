// The device and command queue are shared (`Rc<BackendImpl>`) by the backend
// and its context: resources reach them through the context, as C++'s do
// through `Context& -> getBackend()`.
//
// What the backend draws into is a target built on it (`OffscreenTarget`,
// `SurfaceTarget`).
//
// C++ drains an autorelease pool around each frame
// (`MLN_CREATE_AUTORELEASEPOOL` in `Renderer::Impl::render`), which the
// renderer of the port can't name; the backend opens it when activated
// and drains it when deactivated, around each frame's explicit
// `BackendScope`.

use std::cell::RefCell;
use std::ffi::c_void;
use std::rc::Rc;

use mln_gfx::context::Context as GfxContext;
use mln_gfx::gpu::Gpu as _;
use mln_gfx::released::Released;
use mln_gfx::renderer_backend::{self, RendererBackendBase};
use mln_util::logging::{Event, Log};

use crate::context::Context;
use crate::device::Device;
use crate::gpu::MetalGpu;
use crate::mtl_fwd::{MTLCommandQueuePtr, MTLDevicePtr};

/// The device state the backend and its context share.
pub(crate) struct BackendImpl {
    device: MTLDevicePtr,
    command_queue: MTLCommandQueuePtr,
    /// The `Gpu` the renderer's resources belong to and its draws encode
    /// through, whose writes a frame's commands read (`flush_writes`).
    gpu: RefCell<MetalGpu>,
    /// The buffers the renderer dropped, which a context destroys at the
    /// end of a frame.
    released: Rc<Released>,
}

impl BackendImpl {
    #[must_use]
    pub(crate) fn get_device(&self) -> &MTLDevicePtr {
        &self.device
    }

    #[must_use]
    pub(crate) fn get_command_queue(&self) -> &MTLCommandQueuePtr {
        &self.command_queue
    }

    pub(crate) fn gpu(&self) -> &RefCell<MetalGpu> {
        &self.gpu
    }

    pub(crate) fn released(&self) -> &Rc<Released> {
        &self.released
    }

    /// Commits the writes made to the `Gpu`'s buffers so far, ahead of a
    /// command buffer that reads them and is committed next.
    pub(crate) fn flush_writes(&self) {
        if let Err(error) = self.gpu.borrow_mut().submit(&[]) {
            Log::error(Event::Render, format!("Committing the buffer writes failed: {error}"));
        }
    }
}

/// A renderer's context on a [`Device`].
pub struct RendererBackend {
    base: RendererBackendBase,
    impl_: Rc<BackendImpl>,
    device: Device,
    /// The autorelease pools opened by `activate`, innermost last. Scopes
    /// of other backends can nest between two of this backend's.
    autorelease_pools: RefCell<Vec<*mut c_void>>,
}

impl RendererBackend {
    /// A backend on `device`, which submits to the device's command queue.
    #[must_use]
    pub fn new(device: &Device) -> Self {
        let mtl_device = device.device();
        RendererBackend {
            base: RendererBackendBase::new(),
            impl_: Rc::new(BackendImpl {
                device: mtl_device.clone(),
                command_queue: device.queue().clone(),
                gpu: RefCell::new(MetalGpu::new(device)),
                released: Rc::default(),
            }),
            device: device.clone(),
            autorelease_pools: RefCell::new(Vec::new()),
        }
    }

    /// The device the backend renders on.
    pub fn device(&self) -> &Device {
        &self.device
    }

    /// The backend's context, made on first use. Call inside a
    /// `BackendScope`.
    pub(crate) fn mtl_context(&self) -> Option<Rc<Context>> {
        let context = renderer_backend::RendererBackend::get_context(self);
        context.downcast_ref::<Context>().map(Context::rc)
    }

    pub(crate) fn backend_impl(&self) -> &Rc<BackendImpl> {
        &self.impl_
    }

    pub(crate) fn get_device(&self) -> &MTLDevicePtr {
        self.impl_.get_device()
    }

    /// Drops the context, releasing what it caches first.
    pub(crate) fn reset_context(&mut self) {
        if let Some(context) = self.base.reset_context()
            && let Some(context) = context.downcast_ref::<Context>()
        {
            context.release_resources();
        }
    }

    /// Opens the autorelease pool of a frame.
    pub(crate) fn activate(&self) {
        // SAFETY: `deactivate` pops it on this thread, after the pools
        // opened inside the scope (BackendScopes nest on one thread).
        let pool = unsafe { objc2::ffi::objc_autoreleasePoolPush() };
        self.autorelease_pools.borrow_mut().push(pool);
    }

    /// Drains the autorelease pool of the innermost `activate`.
    pub(crate) fn deactivate(&self) {
        if let Some(pool) = self.autorelease_pools.borrow_mut().pop() {
            // SAFETY: `pool` is the innermost pool: the scopes opened inside
            // the one that pushed it have ended.
            unsafe { objc2::ffi::objc_autoreleasePoolPop(pool) };
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

    fn activate(&self) {
        RendererBackend::activate(self);
    }

    fn deactivate(&self) {
        RendererBackend::deactivate(self);
    }
}

impl Drop for RendererBackend {
    fn drop(&mut self) {
        self.reset_context();
        self.deactivate();
    }
}
