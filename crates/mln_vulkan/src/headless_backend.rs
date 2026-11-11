// Ports platform/default/include/mln/vulkan/headless_backend.hpp and
// platform/default/src/mln/vulkan/headless_backend.cpp, plus
// `Backend::Create<Backend::Type::Vulkan>`.

use std::any::Any;
use std::rc::Rc;

use ash::vk;
use mln_gfx::context::Context as GfxContext;
use mln_gfx::headless_backend;
use mln_gfx::renderable::{Renderable, RenderableBase, SwapBehaviour};
use mln_gfx::renderer_backend::{self, ContextMode, RendererBackendBase};
use mln_gfx::shader_registry::ShaderRegistry;
use mln_gfx::shaders::program_parameters::ProgramParameters;
use mln_util::error::Error;
use mln_util::image::PremultipliedImage;
use mln_util::size::Size;

use crate::context::Context;
use crate::renderable_resource::{RenderableResourceBox, SurfaceRenderableResource, downcast_resource};
use crate::renderer_backend::RendererBackend;

/// `vulkan::HeadlessBackend`: a `vulkan::RendererBackend` that renders into
/// images of its own and keeps a copy of every frame to read
/// (`HeadlessRenderableResource`).
pub struct HeadlessBackend {
    renderer_backend: RendererBackend,
    renderable: RenderableBase,
}

impl HeadlessBackend {
    /// `HeadlessBackend(size = {256, 256}, swapBehaviour = NoFlush,
    /// contextMode = Unique)`, or an error if there is no Vulkan device.
    pub fn new(size: Size, _swap_behaviour: SwapBehaviour, mode: ContextMode) -> Result<Self, Error> {
        let (renderer_backend, _) = RendererBackend::new(mode, None)?;
        Ok(HeadlessBackend { renderer_backend, renderable: RenderableBase::new(size, None) })
    }

    pub fn vulkan_backend(&self) -> &RendererBackend {
        &self.renderer_backend
    }

    /// The name of the GPU.
    pub fn get_device_name(&self) -> String {
        self.renderer_backend
            .backend_impl()
            .get_device_properties()
            .device_name_as_c_str()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default()
    }

    /// Whether the device renders on the CPU, as lavapipe does.
    pub fn is_software_device(&self) -> bool {
        self.renderer_backend.backend_impl().get_device_properties().device_type == vk::PhysicalDeviceType::CPU
    }
}

impl Drop for HeadlessBackend {
    fn drop(&mut self) {
        // Explicitly reset the renderable resource, then the context so
        // that it is destructed and cleaned up.
        self.set_resource(None);
        self.renderer_backend.reset_context();
    }
}

impl Renderable for HeadlessBackend {
    fn renderable_base(&self) -> &RenderableBase {
        &self.renderable
    }
}

impl renderer_backend::RendererBackend for HeadlessBackend {
    fn renderer_backend_base(&self) -> &RendererBackendBase {
        &self.renderer_backend.base
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    /// `getDefaultRenderable()`: creates the resource first.
    fn get_default_renderable(&self) -> &dyn Renderable {
        if !self.has_resource()
            && let Some(context) =
                <Self as renderer_backend::RendererBackend>::get_context(self).as_any().downcast_ref::<Context>()
        {
            let resource = SurfaceRenderableResource::new(
                context,
                self.get_size(),
                None,
                None,
                vk::PresentModeKHR::FIFO,
                /*read_every_frame=*/ true,
            );
            context.set_surface(&resource);
            self.set_resource(Some(RenderableResourceBox::new_rc(resource)));
        }
        self
    }

    fn init_shaders(&self, registry: &ShaderRegistry, parameters: &ProgramParameters) {
        self.renderer_backend.init_shaders(registry, parameters);
    }

    fn create_context(&self) -> Rc<dyn GfxContext> {
        self.renderer_backend.create_context()
    }

    fn activate(&self) {}

    fn deactivate(&self) {}
}

impl headless_backend::HeadlessBackend for HeadlessBackend {
    fn read_still_image(&self) -> PremultipliedImage {
        self.get_resource()
            .and_then(|resource| downcast_resource::<SurfaceRenderableResource>(&*resource)?.read_image())
            .unwrap_or_else(|| PremultipliedImage::new(self.get_size()))
    }

    fn get_renderer_backend(&self) -> &dyn renderer_backend::RendererBackend {
        self
    }
}
