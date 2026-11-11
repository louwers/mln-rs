// Ports platform/default/include/mln/mtl/headless_backend.hpp and
// platform/default/src/mln/mtl/headless_backend.cpp, plus
// `Backend::Create<Backend::Type::Metal>`.

use std::any::Any;
use std::rc::Rc;

use mln_gfx::context::Context as GfxContext;
use mln_gfx::headless_backend;
use mln_gfx::renderable::{Renderable, RenderableBase, SwapBehaviour};
use mln_gfx::renderer_backend::{self, ContextMode, RendererBackendBase};
use mln_gfx::shader_registry::ShaderRegistry;
use mln_gfx::shaders::program_parameters::ProgramParameters;
use mln_gfx::types::TextureChannelDataType;
use mln_util::error::Error;
use mln_util::image::PremultipliedImage;
use mln_util::size::Size;
use objc2_metal::MTLDevice;

use crate::context::Context;
use crate::mtl_fwd::{MTLCommandBufferPtr, MTLRenderPassDescriptorPtr};
use crate::offscreen_texture::{OffscreenTexture, OffscreenTextureResource};
use crate::renderable_resource::{RenderableResource, RenderableResourceBox, downcast_resource};
use crate::renderer_backend::RendererBackend;

/// The renderable resource of the headless backend: an offscreen texture
/// with depth and stencil.
struct HeadlessRenderableResource {
    offscreen_texture: OffscreenTexture,
}

impl HeadlessRenderableResource {
    fn with_offscreen<R>(&self, f: impl FnOnce(&OffscreenTextureResource) -> R) -> Option<R> {
        self.offscreen_texture.with_resource(f)
    }
}

impl RenderableResource for HeadlessRenderableResource {
    fn bind(&self) {
        self.with_offscreen(|r| r.bind());
    }

    fn swap(&self) {
        self.with_offscreen(|r| r.swap());
    }

    fn get_command_buffer(&self) -> Option<MTLCommandBufferPtr> {
        self.with_offscreen(|r| r.get_command_buffer()).flatten()
    }

    fn get_render_pass_descriptor(&self) -> Option<MTLRenderPassDescriptorPtr> {
        self.with_offscreen(|r| r.get_render_pass_descriptor()).flatten()
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// `mtl::HeadlessBackend`: a `mtl::RendererBackend` that renders into an
/// offscreen texture.
pub struct HeadlessBackend {
    renderer_backend: RendererBackend,
    renderable: RenderableBase,
}

impl HeadlessBackend {
    /// `HeadlessBackend(size = {256, 256}, swapBehaviour = NoFlush,
    /// contextMode = Unique)`, or an error if there is no Metal device.
    pub fn new(size: Size, _swap_behaviour: SwapBehaviour, mode: ContextMode) -> Result<Self, Error> {
        Ok(HeadlessBackend {
            renderer_backend: RendererBackend::new(mode)?,
            renderable: RenderableBase::new(size, None),
        })
    }

    pub fn mtl_backend(&self) -> &RendererBackend {
        &self.renderer_backend
    }

    /// The name of the GPU.
    pub fn get_device_name(&self) -> String {
        self.renderer_backend.get_device().name().to_string()
    }

    fn with_resource<R>(&self, f: impl FnOnce(&HeadlessRenderableResource) -> R) -> Option<R> {
        let resource = self.get_resource()?;
        downcast_resource::<HeadlessRenderableResource>(&*resource).map(f)
    }
}

impl Drop for HeadlessBackend {
    fn drop(&mut self) {
        // The renderable resource goes before the context.
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

    /// `getDefaultRenderable()`: creates the resource first (`ensureResource`).
    fn get_default_renderable(&self) -> &dyn Renderable {
        if !self.has_resource()
            && let Some(context) =
                <Self as renderer_backend::RendererBackend>::get_context(self).as_any().downcast_ref::<Context>()
        {
            let offscreen_texture = context.create_offscreen_texture_with(
                self.get_size(),
                TextureChannelDataType::UnsignedByte,
                true,
                true,
            );
            self.set_resource(Some(RenderableResourceBox::new_rc(HeadlessRenderableResource { offscreen_texture })));
        }
        self
    }

    fn init_shaders(&self, registry: &ShaderRegistry, parameters: &ProgramParameters) {
        self.renderer_backend.init_shaders(registry, parameters);
    }

    fn create_context(&self) -> Rc<dyn GfxContext> {
        self.renderer_backend.create_context()
    }

    fn activate(&self) {
        self.renderer_backend.activate();
    }

    fn deactivate(&self) {
        self.renderer_backend.deactivate();
    }
}

impl headless_backend::HeadlessBackend for HeadlessBackend {
    fn read_still_image(&self) -> PremultipliedImage {
        self.with_resource(|r| r.with_offscreen(OffscreenTextureResource::read_still_image))
            .flatten()
            .unwrap_or_else(|| PremultipliedImage::new(self.get_size()))
    }

    fn get_renderer_backend(&self) -> &dyn renderer_backend::RendererBackend {
        self
    }
}
