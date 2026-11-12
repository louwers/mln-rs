// Ports include/mln/mtl/renderer_backend.hpp and src/mln/mtl/renderer_backend.cpp
//
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

use std::any::Any;
use std::cell::RefCell;
use std::ffi::c_void;
use std::rc::Rc;

use mln_gfx::context::Context as GfxContext;
use mln_gfx::renderer_backend::{self, RendererBackendBase};
use mln_gfx::shader_registry::ShaderRegistry;
use mln_gfx::shaders::program_parameters::ProgramParameters;
use mln_gfx::shaders::shader_source::BuiltIn;
use mln_util::logging::{Event, Log};
use objc2_metal::{MTLDevice, MTLGPUFamily};

use crate::context::Context;
use crate::device::Device;
use crate::mtl_fwd::{MTLCommandQueuePtr, MTLDevicePtr};
use crate::shaders::shader_group::ShaderGroup;
use crate::shaders::shader_source::shader_source;

/// The device state the backend and its context share.
pub struct BackendImpl {
    device: MTLDevicePtr,
    command_queue: MTLCommandQueuePtr,
    base_vertex_instance_drawing_supported: bool,
}

impl BackendImpl {
    #[must_use]
    pub fn get_device(&self) -> &MTLDevicePtr {
        &self.device
    }

    #[must_use]
    pub fn get_command_queue(&self) -> &MTLCommandQueuePtr {
        &self.command_queue
    }

    #[must_use]
    pub fn is_base_vertex_instance_drawing_supported(&self) -> bool {
        self.base_vertex_instance_drawing_supported
    }
}

/// `mtl::RendererBackend`: a renderer's context on a [`Device`].
pub struct RendererBackend {
    base: RendererBackendBase,
    impl_: Rc<BackendImpl>,
    device: Device,
    /// The autorelease pools opened by `activate`, innermost last. Scopes
    /// of other backends can nest between two of this backend's.
    autorelease_pools: RefCell<Vec<*mut c_void>>,
}

/// The shader types registered by `initShaders`, in the C++ order.
/// `init_shaders` skips the ones without a Metal source in the port
/// (`shader_source`): the location indicator and wide vector shaders.
const REGISTERED_SHADERS: &[BuiltIn] = &[
    BuiltIn::BackgroundShader,
    BuiltIn::BackgroundPatternShader,
    BuiltIn::CircleShader,
    BuiltIn::ClippingMaskProgram,
    BuiltIn::CollisionBoxShader,
    BuiltIn::CollisionCircleShader,
    BuiltIn::DebugShader,
    BuiltIn::FillShader,
    BuiltIn::FillOutlineShader,
    BuiltIn::FillPatternShader,
    BuiltIn::FillOutlinePatternShader,
    BuiltIn::FillOutlineTriangulatedShader,
    BuiltIn::FillExtrusionShader,
    BuiltIn::FillExtrusionInstancedShader,
    BuiltIn::FillExtrusionPatternShader,
    BuiltIn::FillExtrusionPatternInstancedShader,
    BuiltIn::HeatmapShader,
    BuiltIn::HeatmapTextureShader,
    BuiltIn::HillshadeShader,
    BuiltIn::HillshadePrepareShader,
    BuiltIn::ColorReliefShader,
    BuiltIn::LineShader,
    BuiltIn::LineGradientShader,
    BuiltIn::LineSDFShader,
    BuiltIn::LineGradientSDFShader,
    BuiltIn::LinePatternShader,
    BuiltIn::LocationIndicatorShader,
    BuiltIn::LocationIndicatorTexturedShader,
    BuiltIn::RasterShader,
    BuiltIn::SymbolIconShader,
    BuiltIn::SymbolSDFShader,
    BuiltIn::SymbolTextAndIconShader,
    BuiltIn::WideVectorShader,
    BuiltIn::SkyShader,
    BuiltIn::AtmosphereShader,
    BuiltIn::GlobeDepthShader,
];

impl RendererBackend {
    /// A backend on `device`, which submits to the device's command queue.
    #[must_use]
    pub fn new(device: &Device) -> Self {
        // C++ checks Apple3 only, which no Mac GPU but Apple silicon is in;
        // every Mac2 GPU supports base vertex and instance too.
        let mtl_device = device.device();
        let base_vertex_instance_drawing_supported =
            mtl_device.supportsFamily(MTLGPUFamily::Apple3) || mtl_device.supportsFamily(MTLGPUFamily::Mac2);
        RendererBackend {
            base: RendererBackendBase::new(),
            impl_: Rc::new(BackendImpl {
                device: mtl_device.clone(),
                command_queue: device.queue().clone(),
                base_vertex_instance_drawing_supported,
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
    pub fn mtl_context(&self) -> Option<Rc<Context>> {
        let context = renderer_backend::RendererBackend::get_context(self);
        context.as_any().downcast_ref::<Context>().map(Context::rc)
    }

    pub fn backend_impl(&self) -> &Rc<BackendImpl> {
        &self.impl_
    }

    pub fn get_device(&self) -> &MTLDevicePtr {
        self.impl_.get_device()
    }

    pub fn get_command_queue(&self) -> &MTLCommandQueuePtr {
        self.impl_.get_command_queue()
    }

    /// `context.reset()`, which also releases what the context caches
    /// (`Context::release_cached_resources`).
    pub(crate) fn reset_context(&mut self) {
        if let Some(context) = self.base.reset_context()
            && let Some(context) = context.as_any().downcast_ref::<Context>()
        {
            context.release_cached_resources();
        }
    }

    /// `activate()`: opens the autorelease pool of a frame.
    pub fn activate(&self) {
        // SAFETY: `deactivate` pops it on this thread, after the pools
        // opened inside the scope (BackendScopes nest on one thread).
        let pool = unsafe { objc2::ffi::objc_autoreleasePoolPush() };
        self.autorelease_pools.borrow_mut().push(pool);
    }

    /// `deactivate()`: drains the autorelease pool of the innermost
    /// `activate`.
    pub fn deactivate(&self) {
        if let Some(pool) = self.autorelease_pools.borrow_mut().pop() {
            // SAFETY: `pool` is the innermost pool: the scopes opened inside
            // the one that pushed it have ended.
            unsafe { objc2::ffi::objc_autoreleasePoolPop(pool) };
        }
    }

    /// `createContext()`
    pub fn create_context(&self) -> Rc<Context> {
        Context::new(self.impl_.clone())
    }

    /// `initShaders`: register one `mtl::ShaderGroup` per built-in shader.
    /// A duplicate registration is an error, which C++ throws and the port
    /// logs.
    pub fn init_shaders(&self, registry: &ShaderRegistry, program_parameters: &ProgramParameters) {
        for &shader_id in REGISTERED_SHADERS {
            let Some(source) = shader_source(shader_id) else {
                continue;
            };
            let group = Rc::new(ShaderGroup::new(shader_id, program_parameters.clone()));
            if !registry.register_shader_group(group, source.name) {
                Log::error(Event::Shader, format!("Failed to register {} with shader registry!", source.name));
                debug_assert!(false, "duplicate shader group");
            }
        }
    }
}

impl renderer_backend::RendererBackend for RendererBackend {
    fn renderer_backend_base(&self) -> &RendererBackendBase {
        &self.base
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn init_shaders(&self, registry: &ShaderRegistry, parameters: &ProgramParameters) {
        RendererBackend::init_shaders(self, registry, parameters);
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
