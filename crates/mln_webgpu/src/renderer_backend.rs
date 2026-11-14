// The backend and the context share the device and queue (`Rc<BackendImpl>`),
// and resources hold an `Rc<Context>`.
//
// What the backend draws into is a target built on it (`OffscreenTarget`,
// `SurfaceTarget`), whose draw targets give the render passes their views.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use mln_gfx::context::Context as GfxContext;
use mln_gfx::gpu::BufferId;
use mln_gfx::renderer_backend::{self, RendererBackendBase};
use mln_gfx::shader_registry::ShaderRegistry;
use mln_gfx::shaders::program_parameters::ProgramParameters;
use mln_gfx::shaders::shader_source::BuiltIn;
use mln_gfx::uniform_buffer::Released;

use crate::context::Context;
use crate::device::Device;
use crate::gpu::WebGpu;
use crate::shaders::shader_group::ShaderGroup;
use crate::shaders::shader_source::shader_source;

pub(crate) struct BackendImpl {
    device: wgpu::Device,
    queue: wgpu::Queue,
    depth_stencil_format: Cell<Option<wgpu::TextureFormat>>,
    color_format: Cell<Option<wgpu::TextureFormat>>,
    /// The `Gpu` the renderer's buffers belong to. The drawable path writes
    /// and submits through it too, so that its commands see those writes.
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
            depth_stencil_format: Cell::new(None),
            color_format: Cell::new(None),
            gpu: RefCell::new(WebGpu::new(device)),
            released: Rc::default(),
        }
    }

    pub(crate) fn gpu(&self) -> &RefCell<WebGpu> {
        &self.gpu
    }

    /// The wgpu buffer that holds `buffer`, and where in it and how many
    /// bytes `buffer` takes.
    pub(crate) fn native_buffer(&self, buffer: BufferId) -> Option<(wgpu::Buffer, u64, u64)> {
        let gpu = self.gpu.borrow();
        gpu.native_buffer(buffer).map(|(native, offset, size)| (native.clone(), offset, size))
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
    pub(crate) fn set_depth_stencil_format(&self, format: Option<wgpu::TextureFormat>) {
        self.depth_stencil_format.set(format);
    }
    pub(crate) fn get_depth_stencil_format(&self) -> Option<wgpu::TextureFormat> {
        self.depth_stencil_format.get()
    }
    pub(crate) fn set_color_format(&self, format: Option<wgpu::TextureFormat>) {
        self.color_format.set(format);
    }
    pub(crate) fn get_color_format(&self) -> Option<wgpu::TextureFormat> {
        self.color_format.get()
    }

    /// The formats of the color and depth-stencil attachments of the pass
    /// being recorded: what its pipelines must draw into.
    pub(crate) fn target_formats(&self) -> (wgpu::TextureFormat, Option<wgpu::TextureFormat>) {
        (self.color_format.get().unwrap_or(wgpu::TextureFormat::Bgra8Unorm), self.depth_stencil_format.get())
    }

    /// Writes `data` at `offset` in `buffer` ahead of the next
    /// [`submit`](Self::submit)'s commands, as `Queue::write_buffer` does,
    /// but small writes are gathered and copied into place together.
    pub(crate) fn write_buffer(&self, buffer: &wgpu::Buffer, offset: u64, data: &[u8]) {
        self.gpu.borrow_mut().write_native(buffer, offset, data);
    }

    /// Writes the bytes of `data` at `offset` in `buffer`
    /// straight from the JS array they're in (`Writes::write_from_host`).
    /// `offset` is a multiple of 4; a length that isn't is padded with
    /// zeros.
    #[cfg(target_family = "wasm")]
    pub(crate) fn write_buffer_from_host(
        &self,
        buffer: &wgpu::Buffer,
        offset: u64,
        data: &mln_util::host_bytes::HostBytes,
    ) {
        self.gpu.borrow_mut().write_native_from_host(buffer, offset, data);
    }

    /// Submits `command_buffer` after the copies of the writes
    /// gathered by [`write_buffer`](Self::write_buffer).
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

/// The shader types `init_shaders` registers, with the instanced fill
/// extrusion shaders after their plain ones.
const REGISTERED_SHADERS: &[BuiltIn] = &[
    BuiltIn::BackgroundShader,
    BuiltIn::BackgroundPatternShader,
    BuiltIn::SkyShader,
    BuiltIn::AtmosphereShader,
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
    BuiltIn::LinePatternShader,
    BuiltIn::LineSDFShader,
    BuiltIn::LineGradientSDFShader,
    BuiltIn::RasterShader,
    BuiltIn::SymbolIconShader,
    BuiltIn::SymbolSDFShader,
    BuiltIn::SymbolTextAndIconShader,
    BuiltIn::WideVectorShader,
    BuiltIn::GlobeDepthShader,
];

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
    pub(crate) fn webgpu_context(&self) -> Option<Rc<Context>> {
        let context = renderer_backend::RendererBackend::get_context(self);
        context.downcast_ref::<Context>().map(Context::rc)
    }

    /// Drops the context, releasing the buffers it caches first
    /// (`Context::release_cached_buffers`).
    pub(crate) fn reset_context(&mut self) {
        if let Some(context) = self.base.reset_context()
            && let Some(context) = context.downcast_ref::<Context>()
        {
            context.release_cached_buffers();
        }
    }

    pub(crate) fn create_context(&self) -> Rc<Context> {
        Context::new(Rc::clone(&self.impl_))
    }

    /// Registers one shader group per built-in shader.
    ///
    /// A type whose WGSL is not part of this build is skipped. A duplicate
    /// registration is an error.
    #[expect(clippy::unused_self, reason = "each backend registers its shaders through its own `init_shaders`")]
    pub(crate) fn init_shaders(&self, registry: &ShaderRegistry, program_parameters: &ProgramParameters) {
        for &shader_id in REGISTERED_SHADERS {
            let Some(source) = shader_source(shader_id) else {
                continue;
            };
            let group = Rc::new(ShaderGroup::new(shader_id, program_parameters.clone()));
            if !registry.register_shader_group(group, source.name) {
                panic!("Failed to register {} with shader registry!", source.name);
            }
        }
    }
}

impl renderer_backend::RendererBackend for RendererBackend {
    fn renderer_backend_base(&self) -> &RendererBackendBase {
        &self.base
    }

    fn init_shaders(&self, registry: &ShaderRegistry, parameters: &ProgramParameters) {
        RendererBackend::init_shaders(self, registry, parameters);
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
