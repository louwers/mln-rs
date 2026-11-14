// Differences from C++:
// - The render pass is the context's for the texture's format
//   (`Context::get_render_pass`).
// - The framebuffer is destroyed when the frame that may use it is done.

use std::cell::Cell;
use std::rc::{Rc, Weak};

use ash::vk;
use mln_gfx::command_encoder::CommandEncoder;
use mln_gfx::context::Context as _;
use mln_gfx::gpu::TextureRegion;
use mln_gfx::texture2d::{SamplerState, Texture2DPtr, texture_format};
use mln_gfx::types::{TextureChannelDataType, TextureFilterType, TexturePixelType, TextureWrapType};
use mln_gfx::{frame, offscreen_texture};
use mln_util::image::PremultipliedImage;
use mln_util::logging::{Event, Log};
use mln_util::size::Size;

use crate::context::{Context, RenderPassKey};
use crate::draw_target::{DrawTarget, TargetResource};
use crate::renderer_backend::BackendImpl;

pub(crate) struct OffscreenTextureResource {
    backend: Rc<BackendImpl>,
    context: Weak<Context>,
    extent: vk::Extent2D,
    /// A texture of the context's `Gpu`, which the renderer samples.
    color_texture: Texture2DPtr,
    render_pass: Cell<vk::RenderPass>,
    framebuffer: Cell<vk::Framebuffer>,
}

impl OffscreenTextureResource {
    fn new(context: &Context, size: Size, type_: TextureChannelDataType) -> Self {
        debug_assert!(size.width != 0 && size.height != 0);
        let color_texture = context.create_texture_2d();
        {
            let mut texture = color_texture.borrow_mut();
            texture.set_size(size);
            texture.set_format(texture_format(TexturePixelType::RGBA, type_));
            texture.set_render_target();
            texture.set_sampler(SamplerState {
                filter: TextureFilterType::Linear,
                wrap_u: TextureWrapType::Clamp,
                wrap_v: TextureWrapType::Clamp,
                ..SamplerState::default()
            });
        }
        context.rendering_stats().num_frame_buffers += 1;
        OffscreenTextureResource {
            backend: Rc::clone(context.get_backend()),
            context: context.weak(),
            extent: vk::Extent2D { width: size.width, height: size.height },
            color_texture,
            render_pass: Cell::new(vk::RenderPass::null()),
            framebuffer: Cell::new(vk::Framebuffer::null()),
        }
    }

    pub(crate) fn read_still_image(&self) -> PremultipliedImage {
        let size = Size::new(self.extent.width, self.extent.height);
        let texture = self.color_texture.borrow().id();
        let data = texture
            .zip(self.context.upgrade())
            .map(|(texture, context)| context.gpu().read_texels(texture, TextureRegion::whole(size)));
        match data {
            Some(Ok(data)) => PremultipliedImage::from_vec(size, data),
            Some(Err(error)) => {
                Log::error(Event::Render, format!("Reading an offscreen texture failed: {error}"));
                PremultipliedImage::new(size)
            }
            None => PremultipliedImage::new(size),
        }
    }

    pub(crate) fn get_texture(&self) -> Texture2DPtr {
        Rc::clone(&self.color_texture)
    }

    /// Makes the render pass and the framebuffer over the color texture, if
    /// they aren't made yet.
    fn create_render_pass(&self) {
        if self.framebuffer.get() != vk::Framebuffer::null() {
            return;
        }
        let Some(context) = self.context.upgrade() else {
            return;
        };
        let created = self.color_texture.borrow_mut().create(&*context);
        let native = created.ok().and_then(|texture| context.vulkan_gpu().native_texture(texture));
        let Some((image_view, format, _)) = native else {
            Log::error(Event::Render, "Vulkan offscreen texture creation failed");
            return;
        };
        let render_pass = context.get_render_pass(RenderPassKey {
            color_format: format,
            depth_format: None,
            color_final_layout: vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
            color_load: false,
            depth_final_layout: None,
        });
        let attachments = [image_view];
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

impl TargetResource for OffscreenTextureResource {
    fn bind(&self) {
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
}

impl Drop for OffscreenTextureResource {
    fn drop(&mut self) {
        let framebuffer = self.framebuffer.replace(vk::Framebuffer::null());
        let backend = Rc::clone(&self.backend);
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

/// A draw target over a color texture, without depth or stencil.
pub(crate) struct OffscreenTexture {
    target: DrawTarget,
    resource: Rc<OffscreenTextureResource>,
}

impl OffscreenTexture {
    pub(crate) fn new(context: &Context, size: Size, type_: TextureChannelDataType) -> Self {
        let resource = Rc::new(OffscreenTextureResource::new(context, size, type_));
        OffscreenTexture { target: DrawTarget::new(size, Rc::<OffscreenTextureResource>::clone(&resource)), resource }
    }
}

impl offscreen_texture::OffscreenTexture for OffscreenTexture {
    fn draw_target(&self) -> &dyn frame::DrawTarget {
        &self.target
    }

    fn is_renderable(&self) -> bool {
        true
    }

    fn read_still_image(&self) -> PremultipliedImage {
        self.resource.read_still_image()
    }

    fn get_texture(&self) -> Texture2DPtr {
        self.resource.get_texture()
    }

    /// The frame's one command buffer orders the passes that sample the
    /// texture after the pass that drew it.
    fn submit(&self, _encoder: &dyn CommandEncoder) {}
}
