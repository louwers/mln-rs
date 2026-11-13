// Ports src/mln/gl/offscreen_texture.hpp/.cpp and src/mln/gl/framebuffer.hpp
//
// The texture has no depth or stencil buffer, as in C++: without them every
// depth and stencil test passes.

use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};

use glow::HasContext;
use mln_gfx::command_encoder::CommandEncoder;
use mln_gfx::context::Context as _;
use mln_gfx::texture2d::{SamplerState, Texture2D as _, Texture2DPtr};
use mln_gfx::types::{TextureChannelDataType, TextureFilterType, TexturePixelType, TextureWrapType};
use mln_gfx::{frame, offscreen_texture};
use mln_util::image::PremultipliedImage;
use mln_util::logging::{Event, Log};
use mln_util::size::Size;

use crate::context::Context;
use crate::draw_target::{DrawTarget, TargetResource};
use crate::texture2d::Texture2D;
use crate::types::FramebufferID;

pub struct OffscreenTextureResource {
    context: Weak<Context>,
    size: Size,
    texture: Rc<RefCell<Texture2D>>,
    /// Made by the first `bind`.
    framebuffer: Cell<Option<FramebufferID>>,
}

impl OffscreenTextureResource {
    pub fn new(context: &Context, size: Size, type_: TextureChannelDataType) -> Self {
        debug_assert!(size.width != 0 && size.height != 0);
        let texture = context.create_gl_texture_2d();
        {
            let mut texture = texture.borrow_mut();
            texture.set_size(size);
            texture.set_format(TexturePixelType::RGBA, type_);
            texture.set_sampler_configuration(SamplerState {
                filter: TextureFilterType::Linear,
                wrap_u: TextureWrapType::Clamp,
                wrap_v: TextureWrapType::Clamp,
                ..SamplerState::default()
            });
        }
        OffscreenTextureResource { context: context.weak(), size, texture, framebuffer: Cell::new(None) }
    }

    /// `Context::createFramebuffer(const gfx::Texture2D& color)`
    fn create_framebuffer(&self, context: &Context) -> Option<FramebufferID> {
        let gl = context.gl();
        self.texture.borrow_mut().create();
        let texture = self.texture.borrow().get_texture_id()?;
        // SAFETY: the framebuffer is new and the texture alive.
        unsafe {
            let framebuffer = gl.create_framebuffer().ok()?;
            context.state().bind_framebuffer(gl, Some(framebuffer));
            gl.framebuffer_texture_2d(glow::FRAMEBUFFER, glow::COLOR_ATTACHMENT0, glow::TEXTURE_2D, Some(texture), 0);
            let status = gl.check_framebuffer_status(glow::FRAMEBUFFER);
            if status != glow::FRAMEBUFFER_COMPLETE {
                Log::error(Event::GraphicsBackend, format!("Couldn't create framebuffer: status {status:#x}"));
            }
            context.rendering_stats().num_frame_buffers += 1;
            Some(framebuffer)
        }
    }

    /// `readStillImage()`
    pub fn read_still_image(&self) -> PremultipliedImage {
        let mut image = PremultipliedImage::new(self.size);
        if let Some(context) = self.context.upgrade()
            && let Some(framebuffer) = self.framebuffer.get()
        {
            context.bind_framebuffer(Some(framebuffer), self.size, false);
            image.data = context.read_framebuffer(self.size);
        }
        image
    }

    pub fn get_texture(&self) -> Texture2DPtr {
        self.texture.clone()
    }
}

impl Drop for OffscreenTextureResource {
    fn drop(&mut self) {
        if let Some(context) = self.context.upgrade()
            && let Some(framebuffer) = self.framebuffer.get()
        {
            context.abandon_framebuffer(framebuffer);
            context.rendering_stats().num_frame_buffers -= 1;
        }
    }
}

impl TargetResource for OffscreenTextureResource {
    fn bind(&self) {
        let Some(context) = self.context.upgrade() else {
            return;
        };
        if self.framebuffer.get().is_none() {
            self.framebuffer.set(self.create_framebuffer(&context));
        }
        // A texture can't be read while it is rendered into.
        if let Some(texture) = self.texture.borrow().get_texture_id() {
            context.state().unbind_texture(context.gl(), texture);
        }
        context.bind_framebuffer(self.framebuffer.get(), self.size, false);
    }
}

/// `gl::OffscreenTexture`
pub struct OffscreenTexture {
    target: DrawTarget,
    resource: Rc<OffscreenTextureResource>,
    /// The resource's color texture.
    texture: Texture2DPtr,
}

impl OffscreenTexture {
    pub fn new(context: &Context, size: Size, type_: TextureChannelDataType) -> Self {
        let resource = Rc::new(OffscreenTextureResource::new(context, size, type_));
        let texture = resource.get_texture();
        OffscreenTexture { target: DrawTarget::new(size, resource.clone()), resource, texture }
    }

    pub fn resource(&self) -> &OffscreenTextureResource {
        &self.resource
    }
}

impl offscreen_texture::OffscreenTexture for OffscreenTexture {
    fn draw_target(&self) -> &dyn frame::DrawTarget {
        &self.target
    }

    fn is_renderable(&self) -> bool {
        let resource = self.resource();
        resource.bind();
        resource.framebuffer.get().is_some()
    }

    fn read_still_image(&self) -> PremultipliedImage {
        self.resource().read_still_image()
    }

    fn get_texture(&self) -> Texture2DPtr {
        self.texture.clone()
    }

    /// GL runs the commands in the order they come.
    fn submit(&self, _encoder: &dyn CommandEncoder) {}
}
