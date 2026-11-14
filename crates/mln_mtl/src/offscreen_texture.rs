// Port-only: the image is read back through a buffer kept for the reads.
// After a read, the next frame's command buffer ends with a copy of the
// color texture into it, so that a read right after that frame needs no work
// of the GPU's own; other reads copy the texture then (C++ reads the texture
// itself; see readback.rs).

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use mln_gfx::command_encoder::CommandEncoder;
use mln_gfx::context::Context as _;
use mln_gfx::texture2d::{SamplerState, Texture2DPtr, texture_format};
use mln_gfx::types::{TextureChannelDataType, TextureFilterType, TexturePixelType, TextureWrapType};
use mln_gfx::{frame, offscreen_texture};
use mln_util::image::PremultipliedImage;
use mln_util::logging::{Event, Log};
use mln_util::size::Size;
use objc2::Message;
use objc2::rc::autoreleasepool;
use objc2_metal::{
    MTLCommandBuffer, MTLCommandQueue, MTLDevice, MTLPixelFormat, MTLRenderPassDescriptor, MTLResourceOptions,
    MTLStorageMode, MTLTextureDescriptor, MTLTextureUsage,
};

use crate::context::Context;
use crate::draw_target::{DrawTarget, TargetResource};
use crate::mtl_fwd::{MTLBufferPtr, MTLCommandBufferPtr, MTLRenderPassDescriptorPtr, MTLTexturePtr};
use crate::readback::{buffer_bytes, encode_texture_copy};

fn linear_clamp() -> SamplerState {
    SamplerState {
        filter: TextureFilterType::Linear,
        wrap_u: TextureWrapType::Clamp,
        wrap_v: TextureWrapType::Clamp,
        ..SamplerState::default()
    }
}

/// A depth or depth-stencil texture of `format` that the passes render
/// into.
fn depth_stencil_texture(context: &Context, size: Size, format: MTLPixelFormat) -> Option<MTLTexturePtr> {
    // SAFETY: the size is non-zero and the format a valid 2D one.
    let descriptor = unsafe {
        MTLTextureDescriptor::texture2DDescriptorWithPixelFormat_width_height_mipmapped(
            format,
            size.width as usize,
            size.height as usize,
            false,
        )
    };
    descriptor.setUsage(MTLTextureUsage::ShaderRead | MTLTextureUsage::ShaderWrite | MTLTextureUsage::RenderTarget);
    // Depth and stencil textures can't have the default storage mode on
    // macOS (shared on Apple silicon, managed elsewhere) or on the iOS
    // simulator.
    if cfg!(target_os = "macos") || cfg!(target_abi = "sim") {
        descriptor.setStorageMode(MTLStorageMode::Private);
    }
    let texture = context.get_backend().get_device().newTextureWithDescriptor(&descriptor);
    if texture.is_none() {
        Log::error(Event::Render, "Failed to create a Metal depth or stencil texture");
    }
    texture
}

pub struct OffscreenTextureResource {
    context: Rc<Context>,
    size: Size,
    /// A texture of the backend's `Gpu`, which the renderer samples.
    color_texture: Texture2DPtr,
    depth_texture: Option<MTLTexturePtr>,
    stencil_texture: Option<MTLTexturePtr>,
    command_buffer: RefCell<Option<MTLCommandBufferPtr>>,
    render_pass_descriptor: RefCell<Option<MTLRenderPassDescriptorPtr>>,
    /// The buffer the color texture is copied into to read it back.
    readback_buffer: RefCell<Option<MTLBufferPtr>>,
    /// Whether the readback buffer holds the last frame swapped.
    readback_current: Cell<bool>,
    /// The committed command buffer that copies the color texture into the
    /// readback buffer, until a read has waited for it.
    readback_copy: RefCell<Option<MTLCommandBufferPtr>>,
    /// Whether the next frame copies its image into the readback buffer: set
    /// by a read, cleared by the frame after it.
    copy_next_frame: Cell<bool>,
}

impl OffscreenTextureResource {
    pub(crate) fn new(
        context: Rc<Context>,
        size: Size,
        type_: TextureChannelDataType,
        depth: bool,
        stencil: bool,
    ) -> Self {
        debug_assert!(size.width != 0 && size.height != 0);
        let color_texture = context.create_texture_2d();
        {
            let mut texture = color_texture.borrow_mut();
            texture.set_size(size);
            texture.set_format(texture_format(TexturePixelType::RGBA, type_));
            texture.set_render_target();
            texture.set_sampler(linear_clamp());
        }
        // With a stencil, one texture holds depth and stencil, as a
        // pipeline's formats name them.
        let stencil_texture =
            stencil.then(|| depth_stencil_texture(&context, size, MTLPixelFormat::Depth32Float_Stencil8)).flatten();
        let depth_texture = if stencil {
            stencil_texture.clone().filter(|_| depth)
        } else {
            depth.then(|| depth_stencil_texture(&context, size, MTLPixelFormat::Depth32Float)).flatten()
        };
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
            readback_copy: RefCell::new(None),
            copy_next_frame: Cell::new(false),
        }
    }

    /// The color texture's Metal texture, made on first use.
    fn color(&self) -> Option<MTLTexturePtr> {
        let created = self.color_texture.borrow_mut().create(&*self.context);
        let gpu = self.context.get_backend().gpu().borrow();
        created.ok().and_then(|texture| gpu.native_texture(texture)).map(Message::retain)
    }

    /// The bytes in a row of the color texture.
    fn bytes_per_row(&self) -> usize {
        let format = self.color_texture.borrow().format();
        self.size.width as usize * format.bytes_per_texel().unwrap_or(4) as usize
    }

    /// The image the passes committed so far rendered. Waits for them.
    pub(crate) fn read_still_image(&self) -> PremultipliedImage {
        let mut image = PremultipliedImage::new(self.size);
        let length = self.bytes_per_row() * self.size.height as usize;
        let Some(color) = self.color().filter(|_| length == image.data.len()) else {
            return image;
        };
        let mut buffer = self.readback_buffer.borrow_mut();
        if buffer.is_none() {
            *buffer = self
                .context
                .get_backend()
                .get_device()
                .newBufferWithLength_options(length, MTLResourceOptions::StorageModeShared);
        }
        let Some(buffer) = buffer.as_ref() else {
            return image;
        };
        if self.readback_current.get() {
            if let Some(copy) = self.readback_copy.borrow_mut().take() {
                copy.waitUntilCompleted();
            }
        } else {
            // The queue runs command buffers in commit order, so this copy
            // also waits for every pass committed before it.
            let copied = autoreleasepool(|_| {
                let Some(command_buffer) = self.context.get_backend().get_command_queue().commandBuffer() else {
                    return false;
                };
                if !encode_texture_copy(&command_buffer, &color, self.bytes_per_row(), buffer) {
                    return false;
                }
                command_buffer.commit();
                command_buffer.waitUntilCompleted();
                true
            });
            if !copied {
                return image;
            }
            self.readback_current.set(true);
        }
        image.data.copy_from_slice(buffer_bytes(buffer, length));
        self.copy_next_frame.set(true);
        image
    }

    pub(crate) fn get_texture(&self) -> Texture2DPtr {
        Rc::clone(&self.color_texture)
    }
}

impl Drop for OffscreenTextureResource {
    fn drop(&mut self) {
        mln_gfx::context::Context::rendering_stats(&*self.context).num_frame_buffers -= 1;
    }
}

impl OffscreenTextureResource {
    /// Commits the passes encoded since `bind`, with the copy a read asked
    /// for, without waiting for them, and returns the committed command
    /// buffer.
    ///
    /// Every pass goes to the backend's queue, which runs command buffers in
    /// commit order, so a later pass that samples the texture sees what
    /// these passes rendered.
    pub fn commit(&self) -> Option<MTLCommandBufferPtr> {
        let command_buffer = self.command_buffer.borrow_mut().take();
        debug_assert!(command_buffer.is_some());
        if let Some(command_buffer) = &command_buffer {
            if self.copy_next_frame.replace(false)
                && let Some(buffer) = self.readback_buffer.borrow().as_ref()
            {
                let copied = self
                    .color()
                    .is_some_and(|color| encode_texture_copy(command_buffer, &color, self.bytes_per_row(), buffer));
                self.readback_current.set(copied);
                *self.readback_copy.borrow_mut() = copied.then(|| command_buffer.clone());
            }
            self.context.get_backend().flush_writes();
            command_buffer.commit();
        }
        self.render_pass_descriptor.borrow_mut().take();
        command_buffer
    }
}

impl TargetResource for OffscreenTextureResource {
    fn bind(&self) {
        // A custom layer and a render pass both bind this resource in one
        // frame, and only the swap after the render pass commits. Keep the
        // pending command buffer: a new one per bind leaves the previous one
        // uncommitted, one per frame, until the queue has none left to hand
        // out.
        let mut command_buffer = self.command_buffer.borrow_mut();
        if command_buffer.is_none() {
            *command_buffer = self.context.get_backend().get_command_queue().commandBuffer();
        }
        self.readback_current.set(false);
        self.readback_copy.borrow_mut().take();
        let descriptor = MTLRenderPassDescriptor::new();
        // SAFETY: index 0 is in the attachment array's bounds.
        unsafe { descriptor.colorAttachments().objectAtIndexedSubscript(0) }.setTexture(self.color().as_deref());
        if let Some(depth) = &self.depth_texture {
            descriptor.depthAttachment().setTexture(Some(depth));
        }
        if let Some(stencil) = &self.stencil_texture {
            descriptor.stencilAttachment().setTexture(Some(stencil));
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
}

pub(crate) struct OffscreenTexture {
    target: DrawTarget,
    resource: Rc<OffscreenTextureResource>,
    /// The resource's color texture.
    texture: Texture2DPtr,
}

impl OffscreenTexture {
    pub(crate) fn new(
        context: Rc<Context>,
        size: Size,
        type_: TextureChannelDataType,
        depth: bool,
        stencil: bool,
    ) -> Self {
        let resource = Rc::new(OffscreenTextureResource::new(context, size, type_, depth, stencil));
        let texture = resource.get_texture();
        OffscreenTexture {
            target: DrawTarget::new(size, Rc::<OffscreenTextureResource>::clone(&resource)),
            resource,
            texture,
        }
    }

    pub(crate) fn resource(&self) -> &OffscreenTextureResource {
        &self.resource
    }
}

impl offscreen_texture::OffscreenTexture for OffscreenTexture {
    fn draw_target(&self) -> &dyn frame::DrawTarget {
        &self.target
    }

    /// Nothing calls it; C++ asserts so too.
    fn is_renderable(&self) -> bool {
        debug_assert!(false);
        true
    }

    fn read_still_image(&self) -> PremultipliedImage {
        self.resource.read_still_image()
    }

    fn get_texture(&self) -> Texture2DPtr {
        Rc::clone(&self.texture)
    }

    fn submit(&self, _encoder: &dyn CommandEncoder) {
        self.resource().commit();
    }
}
