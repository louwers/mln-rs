// Ports include/mln/mtl/offscreen_texture.hpp and src/mln/mtl/offscreen_texture.cpp
//
// Port-only: the image is read back through a buffer kept for the reads.
// After a read, the next frame's command buffer ends with a copy of the
// color texture into it, so that a read right after that frame needs no work
// of the GPU's own; other reads copy the texture then (C++ reads the texture
// itself; see texture2d.rs).

use std::any::Any;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

use mln_gfx::command_encoder::CommandEncoder;
use mln_gfx::texture2d::{SamplerState, Texture2D as _, Texture2DPtr};
use mln_gfx::types::{TextureChannelDataType, TextureFilterType, TexturePixelType, TextureWrapType};
use mln_gfx::{frame, offscreen_texture};
use mln_util::image::PremultipliedImage;
use mln_util::size::Size;
use objc2_metal::{MTLCommandBuffer, MTLCommandQueue, MTLRenderPassDescriptor, MTLTextureUsage};

use crate::context::Context;
use crate::draw_target::{DrawTarget, TargetResource};
use crate::mtl_fwd::{MTLBufferPtr, MTLCommandBufferPtr, MTLRenderPassDescriptorPtr};
use crate::texture2d::{Texture2D, buffer_bytes};

fn linear_clamp() -> SamplerState {
    SamplerState {
        filter: TextureFilterType::Linear,
        wrap_u: TextureWrapType::Clamp,
        wrap_v: TextureWrapType::Clamp,
        ..SamplerState::default()
    }
}

/// A texture the passes render into.
fn render_target_texture(
    context: &Context,
    size: Size,
    pixel_type: TexturePixelType,
    channel_type: TextureChannelDataType,
) -> Rc<RefCell<Texture2D>> {
    let texture = context.create_mtl_texture_2d();
    {
        let mut t = texture.borrow_mut();
        t.set_size(size);
        t.set_format(pixel_type, channel_type);
        t.set_sampler_configuration(linear_clamp());
        t.set_usage(MTLTextureUsage::ShaderRead | MTLTextureUsage::ShaderWrite | MTLTextureUsage::RenderTarget);
    }
    texture
}

pub struct OffscreenTextureResource {
    context: Rc<Context>,
    size: Size,
    color_texture: Rc<RefCell<Texture2D>>,
    depth_texture: Option<Rc<RefCell<Texture2D>>>,
    stencil_texture: Option<Rc<RefCell<Texture2D>>>,
    command_buffer: RefCell<Option<MTLCommandBufferPtr>>,
    render_pass_descriptor: RefCell<Option<MTLRenderPassDescriptorPtr>>,
    /// The buffer the color texture is copied into to read it back.
    readback_buffer: RefCell<Option<MTLBufferPtr>>,
    /// Whether the readback buffer holds the last frame swapped.
    readback_current: Cell<bool>,
    /// Whether the next frame copies its image into the readback buffer: set
    /// by a read, cleared by the frame after it.
    copy_next_frame: Cell<bool>,
}

impl OffscreenTextureResource {
    pub fn new(context: Rc<Context>, size: Size, type_: TextureChannelDataType, depth: bool, stencil: bool) -> Self {
        debug_assert!(size.width != 0 && size.height != 0);
        let color_texture = render_target_texture(&context, size, TexturePixelType::RGBA, type_);
        let depth_texture = depth
            .then(|| render_target_texture(&context, size, TexturePixelType::Depth, TextureChannelDataType::Float));
        // On the iOS simulator, the depth target is Depth32Float_Stencil8.
        let stencil_texture = (stencil && !cfg!(target_abi = "sim")).then(|| {
            render_target_texture(&context, size, TexturePixelType::Stencil, TextureChannelDataType::UnsignedByte)
        });
        mln_gfx::context::Context::rendering_stats(&*context).num_frame_buffers += 1;
        OffscreenTextureResource {
            context,
            size,
            color_texture,
            depth_texture,
            stencil_texture,
            command_buffer: RefCell::new(None),
            render_pass_descriptor: RefCell::new(None),
            readback_buffer: RefCell::new(None),
            readback_current: Cell::new(false),
            copy_next_frame: Cell::new(false),
        }
    }

    /// The image the passes rendered, once they have run.
    pub fn read_still_image(&self) -> PremultipliedImage {
        let mut image = PremultipliedImage::new(self.size);
        let color = self.color_texture.borrow();
        if color.get_data_size() != image.data.len() {
            return image;
        }
        let mut buffer = self.readback_buffer.borrow_mut();
        if buffer.is_none() {
            *buffer = color.new_pixels_buffer();
        }
        let Some(buffer) = buffer.as_ref() else {
            return image;
        };
        if !self.readback_current.get() {
            if !color.copy_to(buffer) {
                return image;
            }
            self.readback_current.set(true);
        }
        let length = image.data.len();
        image.data.copy_from_slice(buffer_bytes(buffer, length));
        self.copy_next_frame.set(true);
        image
    }

    pub fn get_texture(&self) -> Texture2DPtr {
        self.color_texture.clone()
    }
}

impl Drop for OffscreenTextureResource {
    fn drop(&mut self) {
        mln_gfx::context::Context::rendering_stats(&*self.context).num_frame_buffers -= 1;
    }
}

impl OffscreenTextureResource {
    /// Commits the passes encoded since `bind`, with the copy a read asked
    /// for, and waits for them.
    pub fn commit(&self) {
        let command_buffer = self.command_buffer.borrow_mut().take();
        debug_assert!(command_buffer.is_some());
        if let Some(command_buffer) = command_buffer {
            if self.copy_next_frame.replace(false)
                && let Some(buffer) = self.readback_buffer.borrow().as_ref()
            {
                let copied = self.color_texture.borrow().encode_copy_to(&command_buffer, buffer);
                self.readback_current.set(copied);
            }
            command_buffer.commit();
            command_buffer.waitUntilCompleted();
        }
        self.render_pass_descriptor.borrow_mut().take();
    }
}

impl TargetResource for OffscreenTextureResource {
    fn bind(&self) {
        // An upload pass and a render pass both bind this resource in one
        // frame, and only the swap after the render pass commits. Keep the
        // pending command buffer: a new one per bind leaves the previous one
        // uncommitted, one per frame, until the queue has none left to hand
        // out.
        let mut command_buffer = self.command_buffer.borrow_mut();
        if command_buffer.is_none() {
            *command_buffer = self.context.get_backend().get_command_queue().commandBuffer();
        }
        self.readback_current.set(false);
        let descriptor = MTLRenderPassDescriptor::new();
        let mut color = self.color_texture.borrow_mut();
        color.create();
        // SAFETY: index 0 is in the attachment array's bounds.
        unsafe { descriptor.colorAttachments().objectAtIndexedSubscript(0) }
            .setTexture(color.get_metal_texture().map(|t| &**t));
        if let Some(depth) = &self.depth_texture {
            let mut depth = depth.borrow_mut();
            depth.create();
            descriptor.depthAttachment().setTexture(depth.get_metal_texture().map(|t| &**t));
        }
        if let Some(stencil) = &self.stencil_texture {
            let mut stencil = stencil.borrow_mut();
            stencil.create();
            descriptor.stencilAttachment().setTexture(stencil.get_metal_texture().map(|t| &**t));
        }
        *self.render_pass_descriptor.borrow_mut() = Some(descriptor);
    }

    fn get_command_buffer(&self) -> Option<MTLCommandBufferPtr> {
        self.command_buffer.borrow().clone()
    }

    fn get_render_pass_descriptor(&self) -> Option<MTLRenderPassDescriptorPtr> {
        debug_assert!(self.render_pass_descriptor.borrow().is_some());
        self.render_pass_descriptor.borrow().clone()
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// `mtl::OffscreenTexture`
pub struct OffscreenTexture {
    target: DrawTarget,
    resource: Rc<OffscreenTextureResource>,
    /// The resource's color texture.
    texture: Texture2DPtr,
}

impl OffscreenTexture {
    pub fn new(context: Rc<Context>, size: Size, type_: TextureChannelDataType, depth: bool, stencil: bool) -> Self {
        let resource = Rc::new(OffscreenTextureResource::new(context, size, type_, depth, stencil));
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

    /// C++ asserts: nothing calls it.
    fn is_renderable(&self) -> bool {
        debug_assert!(false);
        true
    }

    fn read_still_image(&self) -> PremultipliedImage {
        self.resource().read_still_image()
    }

    fn get_texture(&self) -> Texture2DPtr {
        self.texture.clone()
    }

    fn submit(&self, _encoder: &dyn CommandEncoder) {
        self.resource().commit();
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}
