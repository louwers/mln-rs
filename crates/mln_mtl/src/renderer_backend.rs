// Ports include/mln/mtl/renderer_backend.hpp and src/mln/mtl/renderer_backend.cpp
//
// The device and command queue are shared (`Rc<BackendImpl>`) by the backend
// and its context: resources reach them through the context, as C++'s do
// through `Context& -> getBackend()`.
//
// `getDefaultRenderable()` is implemented by the platform backends
// (HeadlessBackend, SurfaceBackend), which embed this struct.
//
// C++ drains an autorelease pool around each frame
// (`MLN_CREATE_AUTORELEASEPOOL` in `Renderer::Impl::render`), which the
// renderer of the port can't name; the backend opens it when activated
// and drains it when deactivated, around each frame's explicit
// `BackendScope`.

use std::cell::Cell;
use std::ffi::c_void;
use std::rc::Rc;

use mln_gfx::renderer_backend::{ContextMode, RendererBackendBase};
use mln_gfx::shader_registry::ShaderRegistry;
use mln_gfx::shaders::program_parameters::ProgramParameters;
use mln_gfx::shaders::shader_source::BuiltIn;
use mln_util::logging::{Event, Log};
use objc2_metal::{MTLCreateSystemDefaultDevice, MTLDevice, MTLGPUFamily};

use crate::context::Context;
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
    pub fn get_device(&self) -> &MTLDevicePtr {
        &self.device
    }

    pub fn get_command_queue(&self) -> &MTLCommandQueuePtr {
        &self.command_queue
    }

    pub fn is_base_vertex_instance_drawing_supported(&self) -> bool {
        self.base_vertex_instance_drawing_supported
    }
}

/// `mtl::RendererBackend`: embedded by the platform backends, which
/// implement `gfx::RendererBackend` by delegating to it.
pub struct RendererBackend {
    pub base: RendererBackendBase,
    impl_: Rc<BackendImpl>,
    /// The autorelease pool opened by `activate`.
    autorelease_pool: Cell<Option<*mut c_void>>,
}

/// The shader types registered by `initShaders`, in the C++ order, without
/// the instanced fill extrusion shaders: the port's renderer builds fill
/// extrusions without instancing.
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
    BuiltIn::FillExtrusionPatternShader,
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
    /// `RendererBackend(contextMode)`: the system's default device and a
    /// command queue on it, or an error if there is no Metal device.
    pub fn new(context_mode: ContextMode) -> Result<Self, String> {
        let device = MTLCreateSystemDefaultDevice().ok_or("No Metal device")?;
        let command_queue = device.newCommandQueue().ok_or("Failed to create a Metal command queue")?;
        // C++ checks Apple3 only, which no Mac GPU but Apple silicon is in;
        // every Mac2 GPU supports base vertex and instance too.
        let base_vertex_instance_drawing_supported =
            device.supportsFamily(MTLGPUFamily::Apple3) || device.supportsFamily(MTLGPUFamily::Mac2);
        Ok(RendererBackend {
            base: RendererBackendBase::new(context_mode),
            impl_: Rc::new(BackendImpl { device, command_queue, base_vertex_instance_drawing_supported }),
            autorelease_pool: Cell::new(None),
        })
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
        if self.autorelease_pool.get().is_none() {
            // SAFETY: `deactivate` pops it on this thread, after the pools
            // opened inside the scope (BackendScope nests on one thread).
            self.autorelease_pool.set(Some(unsafe { objc2::ffi::objc_autoreleasePoolPush() }));
        }
    }

    /// `deactivate()`: drains the autorelease pool `activate` opened.
    pub fn deactivate(&self) {
        if let Some(pool) = self.autorelease_pool.take() {
            // SAFETY: `pool` is the innermost pool, pushed by `activate`.
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

impl Drop for RendererBackend {
    fn drop(&mut self) {
        self.reset_context();
        self.deactivate();
    }
}
