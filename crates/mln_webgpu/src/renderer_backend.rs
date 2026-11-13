// The C++ backend stores raw `WGPUDevice`/`WGPUQueue`/instance handles in its
// `Impl` and every resource reaches them through `Context& -> getBackend()`.
// Here the `Impl` is shared (`Rc<BackendImpl>`) by the backend and the
// context, and resources hold an `Rc<Context>`.
//
// What the backend draws into is a target built on it (`OffscreenTarget`,
// `SurfaceTarget`), whose draw targets give the render passes their views.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use mln_gfx::context::Context as GfxContext;
use mln_gfx::renderer_backend::{self, RendererBackendBase};
use mln_gfx::shader_registry::ShaderRegistry;
use mln_gfx::shaders::program_parameters::ProgramParameters;
use mln_gfx::shaders::shader_source::BuiltIn;

use crate::context::Context;
use crate::device::Device;
use crate::shaders::shader_group::ShaderGroup;
use crate::shaders::shader_source::shader_source;
use crate::writes::Writes;

pub(crate) struct BackendImpl {
    device: wgpu::Device,
    queue: wgpu::Queue,
    /// `None` for no format, C++'s `Undefined`.
    depth_stencil_format: Cell<Option<wgpu::TextureFormat>>,
    color_format: Cell<Option<wgpu::TextureFormat>>,
    writes: RefCell<Writes>,
    /// The page that small uniform buffers are taken from, and how many of
    /// its bytes are taken.
    uniform_page: RefCell<Option<(wgpu::Buffer, u64)>>,
}

/// The size of the buffers that small uniform buffers share.
const UNIFORM_PAGE: u64 = 16 << 10;

impl BackendImpl {
    fn new(device: wgpu::Device, queue: wgpu::Queue) -> Self {
        BackendImpl {
            device,
            queue,
            depth_stencil_format: Cell::new(None),
            color_format: Cell::new(None),
            writes: RefCell::default(),
            uniform_page: RefCell::new(None),
        }
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

    /// Port-only: writes `data` at `offset` in `buffer` ahead of the next
    /// [`submit`](Self::submit)'s commands, as `Queue::write_buffer` does,
    /// but small writes are gathered and copied into place together.
    pub(crate) fn write_buffer(&self, buffer: &wgpu::Buffer, offset: u64, data: &[u8]) {
        self.writes.borrow_mut().write(&self.device, &self.queue, buffer, offset, data);
    }

    /// Port-only: [`write_buffer`](Self::write_buffer) where the bytes after
    /// `data` up to `padded` bytes from `offset` already hold zeros
    /// (`Writes::write_padded`).
    pub(crate) fn write_buffer_padded(&self, buffer: &wgpu::Buffer, offset: u64, data: &[u8], padded: u64) {
        self.writes.borrow_mut().write_padded(&self.device, &self.queue, buffer, offset, data, padded);
    }

    /// Port-only: `size` bytes (a multiple of 256) of a new uniform buffer,
    /// zero filled: the buffer and their offset in it. Small ones are taken
    /// in turn from a shared page, so that UBOs made together sit together
    /// and their writes join one copy. A page is freed with the last of its
    /// UBOs (and the bind groups and commands that use them).
    pub(crate) fn allocate_uniform_buffer(&self, size: u64) -> (wgpu::Buffer, u64) {
        let create = |size| {
            self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("Uniform Buffer"),
                size,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            })
        };
        if size > UNIFORM_PAGE / 4 {
            return (create(size), 0);
        }
        let mut page = self.uniform_page.borrow_mut();
        let (buffer, taken) = match &mut *page {
            Some((buffer, taken)) if *taken + size <= UNIFORM_PAGE => (buffer, taken),
            _ => {
                let (buffer, taken) = page.insert((create(UNIFORM_PAGE), 0));
                (buffer, taken)
            }
        };
        let offset = *taken;
        *taken += size;
        (buffer.clone(), offset)
    }

    /// Port-only: writes the bytes of `data` at `offset` in `buffer`
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
        let (device, queue) = (&self.device, &self.queue);
        let len = data.len() as u64;
        let aligned = len & !(wgpu::COPY_BUFFER_ALIGNMENT - 1);
        let mut writes = self.writes.borrow_mut();
        writes.write_from_host(device, queue, buffer, offset, data, aligned);
        if aligned < len
            && let Some(array) = data.array()
        {
            // writeBuffer takes whole 4-byte words.
            let mut tail = [0u8; wgpu::COPY_BUFFER_ALIGNMENT as usize];
            array.subarray(aligned as u32, len as u32).copy_to(&mut tail[..(len - aligned) as usize]);
            writes.write(device, queue, buffer, offset + aligned, &tail);
        }
    }

    /// Port-only: submits `command_buffer` after the copies of the writes
    /// gathered by [`write_buffer`](Self::write_buffer).
    pub(crate) fn submit(&self, command_buffer: wgpu::CommandBuffer) {
        let writes = self.writes.borrow_mut().encode(&self.device, &self.queue);
        self.queue.submit(writes.into_iter().chain([command_buffer]));
    }
}

/// A renderer's context on a [`Device`].
pub struct RendererBackend {
    base: RendererBackendBase,
    impl_: Rc<BackendImpl>,
    device: Device,
}

/// The shader types registered by `initShaders`, in the C++ order, with the
/// instanced fill extrusion shaders after their plain ones, as on Metal.
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
        let impl_ = BackendImpl::new(device.device().clone(), device.queue().clone());
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
    /// C++ registers all 31 types; a type whose WGSL is not part of this
    /// build is skipped.
    /// A duplicate registration is an error, as in C++ (which throws).
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
