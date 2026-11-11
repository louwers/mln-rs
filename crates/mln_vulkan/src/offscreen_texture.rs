// Ports include/mln/vulkan/offscreen_texture.hpp and src/mln/vulkan/offscreen_texture.cpp
//
// Differences from C++:
// - The render pass is the context's for the texture's format
//   (`Context::get_render_pass`).
// - The framebuffer is destroyed when the frame that may use it is done.

use std::any::Any;
use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};

use ash::vk;
use mln_gfx::context::Context as _;
use mln_gfx::offscreen_texture;
use mln_gfx::renderable::{Renderable, RenderableBase};
use mln_gfx::texture2d::{SamplerState, Texture2D as _, Texture2DPtr};
use mln_gfx::types::{TextureChannelDataType, TextureFilterType, TexturePixelType, TextureWrapType};
use mln_util::image::PremultipliedImage;
use mln_util::logging::{Event, Log};
use mln_util::size::Size;

use crate::context::{Context, RenderPassKey};
use crate::renderable_resource::{RenderableResource, RenderableResourceBox};
use crate::renderer_backend::BackendImpl;
use crate::texture2d::{Texture2D, Texture2DUsage};

pub struct OffscreenTextureResource {
    backend: Rc<BackendImpl>,
    context: Weak<Context>,
    extent: vk::Extent2D,
    color_texture: Rc<RefCell<Texture2D>>,
    render_pass: Cell<vk::RenderPass>,
    framebuffer: Cell<vk::Framebuffer>,
}

impl OffscreenTextureResource {
    fn new(context: &Context, size: Size, type_: TextureChannelDataType) -> Self {
        debug_assert!(size.width != 0 && size.height != 0);
        let color_texture = context.create_vulkan_texture_2d();
        {
            let mut texture = color_texture.borrow_mut();
            texture.set_size(size);
            texture.set_format(TexturePixelType::RGBA, type_);
            texture.set_sampler_configuration(SamplerState {
                filter: TextureFilterType::Linear,
                wrap_u: TextureWrapType::Clamp,
                wrap_v: TextureWrapType::Clamp,
                ..SamplerState::default()
            });
            texture.set_usage(Texture2DUsage::Attachment);
        }
        context.rendering_stats().num_frame_buffers += 1;
        OffscreenTextureResource {
            backend: context.get_backend().clone(),
            context: context.weak(),
            extent: vk::Extent2D { width: size.width, height: size.height },
            color_texture,
            render_pass: Cell::new(vk::RenderPass::null()),
            framebuffer: Cell::new(vk::Framebuffer::null()),
        }
    }

    pub fn read_still_image(&self) -> PremultipliedImage {
        self.color_texture
            .borrow_mut()
            .read_image()
            .unwrap_or_else(|| PremultipliedImage::new(Size::new(self.extent.width, self.extent.height)))
    }

    pub fn get_texture(&self) -> Texture2DPtr {
        self.color_texture.clone()
    }

    /// `createRenderPass()`: the render pass and the framebuffer over the
    /// color texture.
    fn create_render_pass(&self) {
        if self.framebuffer.get() != vk::Framebuffer::null() {
            return;
        }
        let Some(context) = self.context.upgrade() else {
            return;
        };
        let texture = self.color_texture.borrow();
        let render_pass = context.get_render_pass(RenderPassKey {
            color_format: texture.get_vulkan_format(),
            depth_format: None,
            color_final_layout: vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
            color_load: false,
        });
        let attachments = [texture.get_vulkan_image_view()];
        if attachments[0] == vk::ImageView::null() {
            return;
        }
        let create_info = vk::FramebufferCreateInfo::default()
            .render_pass(render_pass)
            .attachments(&attachments)
            .width(self.extent.width)
            .height(self.extent.height)
            .layers(1);
        // SAFETY: the view is live and of the render pass's format.
        match unsafe { self.backend.get_device().create_framebuffer(&create_info, None) } {
            Ok(framebuffer) => {
                self.render_pass.set(render_pass);
                self.framebuffer.set(framebuffer);
            }
            Err(_) => Log::error(Event::Render, "Vulkan framebuffer creation failed"),
        }
    }
}

impl RenderableResource for OffscreenTextureResource {
    fn bind(&self) {
        self.color_texture.borrow_mut().create();
        self.create_render_pass();
    }

    fn get_extent(&self) -> vk::Extent2D {
        self.extent
    }

    fn get_render_pass(&self) -> vk::RenderPass {
        self.render_pass.get()
    }

    fn get_framebuffer(&self) -> vk::Framebuffer {
        self.framebuffer.get()
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

impl Drop for OffscreenTextureResource {
    fn drop(&mut self) {
        let framebuffer = self.framebuffer.replace(vk::Framebuffer::null());
        let backend = self.backend.clone();
        // SAFETY: the commands that use the framebuffer have been waited
        // for when the deletion queue runs, or the context is gone.
        let destroy = move || unsafe {
            if framebuffer != vk::Framebuffer::null() {
                backend.get_device().destroy_framebuffer(framebuffer, None);
            }
        };
        match self.context.upgrade() {
            Some(context) => {
                context.rendering_stats().num_frame_buffers -= 1;
                context.enqueue_deletion(Box::new(move |_| destroy()));
            }
            None => destroy(),
        }
    }
}

/// `vulkan::OffscreenTexture`: a renderable over a color texture, without
/// depth or stencil.
pub struct OffscreenTexture {
    renderable: RenderableBase,
    resource: Rc<OffscreenTextureResource>,
}

impl OffscreenTexture {
    pub fn new(context: &Context, size: Size, type_: TextureChannelDataType) -> Self {
        let resource = Rc::new(OffscreenTextureResource::new(context, size, type_));
        OffscreenTexture {
            renderable: RenderableBase::new(size, Some(RenderableResourceBox::new_rc(resource.clone()))),
            resource,
        }
    }
}

impl Renderable for OffscreenTexture {
    fn renderable_base(&self) -> &RenderableBase {
        &self.renderable
    }
}

impl offscreen_texture::OffscreenTexture for OffscreenTexture {
    fn is_renderable(&self) -> bool {
        true
    }

    fn read_still_image(&self) -> PremultipliedImage {
        self.resource.read_still_image()
    }

    fn get_texture(&self) -> Texture2DPtr {
        self.resource.get_texture()
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}
