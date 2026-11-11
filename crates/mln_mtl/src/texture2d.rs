// Ports include/mln/mtl/texture2d.hpp and src/mln/mtl/texture2d.cpp
//
// Differences from C++:
// - Textures that aren't render targets, which the CPU writes, don't allow
//   GPU-optimized contents (see `create_metal_texture`).
// - Port-only: `read_pixels`, the copy of the texture that offscreen
//   textures read their image back with, through a blit into shared memory
//   (C++ reads the texture with `getBytes`, which on macOS needs a
//   synchronized managed texture).

use std::any::Any;
use std::ffi::c_void;
use std::ptr::NonNull;
use std::rc::Rc;

use mln_gfx::context::Context as _;
use mln_gfx::texture2d::{self, SamplerState};
use mln_gfx::types::{TextureChannelDataType, TextureFilterType, TexturePixelType, TextureWrapType};
use mln_util::image::PremultipliedImage;
use mln_util::logging::{Event, Log};
use mln_util::size::Size;
use objc2::rc::autoreleasepool;
use objc2::runtime::ProtocolObject;
use objc2_metal::{
    MTLBlitCommandEncoder, MTLBuffer, MTLCommandBuffer, MTLCommandEncoder, MTLCommandQueue, MTLDevice, MTLOrigin,
    MTLPixelFormat, MTLRegion, MTLResourceOptions, MTLSamplerAddressMode, MTLSamplerDescriptor, MTLSamplerMinMagFilter,
    MTLSize, MTLStorageMode, MTLTexture, MTLTextureDescriptor, MTLTextureUsage,
};

use crate::context::Context;
use crate::mtl_fwd::{MTLBufferPtr, MTLSamplerStatePtr, MTLTexturePtr};
use crate::render_pass::RenderPass;

pub struct Texture2D {
    context: Rc<Context>,
    metal_texture: Option<MTLTexturePtr>,
    metal_sampler_state: Option<MTLSamplerStatePtr>,
    size: Size,
    pixel_format: TexturePixelType,
    channel_type: TextureChannelDataType,
    usage: MTLTextureUsage,
    sampler_state: SamplerState,
    image: Option<Rc<PremultipliedImage>>,
    texture_dirty: bool,
    sampler_state_dirty: bool,
}

/// Depth and stencil textures can't have the default storage mode on macOS
/// (shared on Apple silicon, managed elsewhere) or on the iOS simulator.
fn is_depth_or_stencil(format: MTLPixelFormat) -> bool {
    matches!(
        format,
        MTLPixelFormat::Depth16Unorm
            | MTLPixelFormat::Depth32Float
            | MTLPixelFormat::Stencil8
            | MTLPixelFormat::Depth24Unorm_Stencil8
            | MTLPixelFormat::Depth32Float_Stencil8
            | MTLPixelFormat::X32_Stencil8
            | MTLPixelFormat::X24_Stencil8
    )
}

impl Texture2D {
    pub fn new(context: Rc<Context>) -> Self {
        Texture2D {
            context,
            metal_texture: None,
            metal_sampler_state: None,
            size: Size::new(0, 0),
            pixel_format: TexturePixelType::RGBA,
            channel_type: TextureChannelDataType::UnsignedByte,
            usage: MTLTextureUsage::ShaderRead,
            sampler_state: SamplerState::default(),
            image: None,
            texture_dirty: true,
            sampler_state_dirty: true,
        }
    }

    pub fn set_usage(&mut self, usage: MTLTextureUsage) {
        self.usage = usage;
        self.texture_dirty = true;
    }

    /// `getMetalTexture()`
    pub fn get_metal_texture(&self) -> Option<&MTLTexturePtr> {
        debug_assert!(self.metal_texture.is_some());
        self.metal_texture.as_ref()
    }

    fn get_metal_pixel_format(&self) -> MTLPixelFormat {
        // Intel and AMD GPUs and the iOS simulator need the combined format:
        // the depth and stencil formats of a pipeline must be identical.
        if (cfg!(target_arch = "x86_64") || cfg!(target_abi = "sim"))
            && matches!(self.channel_type, TextureChannelDataType::Float | TextureChannelDataType::UnsignedByte)
            && matches!(self.pixel_format, TexturePixelType::Depth | TexturePixelType::Stencil)
        {
            return MTLPixelFormat::Depth32Float_Stencil8;
        }
        match (self.channel_type, self.pixel_format) {
            (TextureChannelDataType::UnsignedByte, TexturePixelType::RGBA) => MTLPixelFormat::RGBA8Unorm,
            (TextureChannelDataType::UnsignedByte, TexturePixelType::Alpha) => MTLPixelFormat::A8Unorm,
            (TextureChannelDataType::UnsignedByte, TexturePixelType::Stencil) => MTLPixelFormat::Stencil8,
            (TextureChannelDataType::HalfFloat, TexturePixelType::RGBA) => MTLPixelFormat::RGBA16Float,
            (TextureChannelDataType::HalfFloat, TexturePixelType::Alpha) => MTLPixelFormat::R16Float,
            (TextureChannelDataType::Float, TexturePixelType::RGBA) => MTLPixelFormat::RGBA32Float,
            (TextureChannelDataType::Float, TexturePixelType::Alpha) => MTLPixelFormat::R32Float,
            (TextureChannelDataType::Float, TexturePixelType::Depth) => MTLPixelFormat::Depth32Float,
            _ => {
                debug_assert!(false, "unsupported texture format");
                MTLPixelFormat::Invalid
            }
        }
    }

    fn create_metal_texture(&mut self) {
        if self.size == Size::new(0, 0) {
            return;
        }
        self.destroy_metal_texture();
        let format = self.get_metal_pixel_format();
        if format == MTLPixelFormat::Invalid {
            return;
        }

        // SAFETY: the size is non-zero and the format a valid 2D one.
        let descriptor = unsafe {
            MTLTextureDescriptor::texture2DDescriptorWithPixelFormat_width_height_mipmapped(
                format,
                self.size.width as usize,
                self.size.height as usize,
                false,
            )
        };
        descriptor.setUsage(self.usage);
        if !self.usage.contains(MTLTextureUsage::RenderTarget) {
            // The CPU writes this texture (`replaceRegion`). On Apple GPUs,
            // writing GPU-optimized contents compresses them on the CPU,
            // through one queue that the renderers of all threads wait for.
            descriptor.setAllowGPUOptimizedContents(false);
        }
        if (cfg!(target_os = "macos") || cfg!(target_abi = "sim")) && is_depth_or_stencil(format) {
            descriptor.setStorageMode(MTLStorageMode::Private);
        }
        self.metal_texture = self.context.get_backend().get_device().newTextureWithDescriptor(&descriptor);
        if self.metal_texture.is_none() {
            Log::error(Event::Render, "Failed to create a Metal texture");
            return;
        }
        self.texture_dirty = false;
        let data_size = texture2d::Texture2D::get_data_size(self) as i32;
        let mut stats = self.context.rendering_stats();
        stats.num_created_textures += 1;
        stats.num_active_textures += 1;
        stats.mem_textures += data_size;
    }

    fn destroy_metal_texture(&mut self) {
        if self.metal_texture.take().is_none() {
            return;
        }
        let data_size = texture2d::Texture2D::get_data_size(self) as i32;
        let mut stats = self.context.rendering_stats();
        stats.num_active_textures -= 1;
        stats.mem_textures -= data_size;
    }

    pub fn update_sampler_configuration(&mut self) {
        let descriptor = MTLSamplerDescriptor::new();
        let filter = if self.sampler_state.filter == TextureFilterType::Nearest {
            MTLSamplerMinMagFilter::Nearest
        } else {
            MTLSamplerMinMagFilter::Linear
        };
        let wrap = |wrap: TextureWrapType| {
            if wrap == TextureWrapType::Clamp {
                MTLSamplerAddressMode::ClampToEdge
            } else {
                MTLSamplerAddressMode::Repeat
            }
        };
        descriptor.setMinFilter(filter);
        descriptor.setMagFilter(filter);
        descriptor.setSAddressMode(wrap(self.sampler_state.wrap_u));
        descriptor.setTAddressMode(wrap(self.sampler_state.wrap_v));
        self.metal_sampler_state = self.context.get_backend().get_device().newSamplerStateWithDescriptor(&descriptor);
        if self.metal_sampler_state.is_none() {
            Log::error(Event::Render, "Failed to create a Metal sampler state");
            return;
        }
        self.sampler_state_dirty = false;
    }

    pub fn bind(&mut self, render_pass: &RenderPass, location: usize) {
        debug_assert!(!self.texture_dirty);
        // Update the sampler state if it was changed after resource creation.
        if self.sampler_state_dirty {
            self.update_sampler_configuration();
        }
        render_pass.set_fragment_texture(self.metal_texture.as_ref(), location);
        render_pass.set_fragment_sampler_state(self.metal_sampler_state.as_ref(), location);
        self.context.rendering_stats().num_texture_bindings += 1;
    }

    pub fn unbind(&self, _render_pass: &RenderPass, _location: usize) {
        self.context.rendering_stats().num_texture_bindings -= 1;
    }

    /// Port-only: the size in bytes of the texture's pixels, rows tightly
    /// packed.
    pub fn pixels_length(&self) -> usize {
        self.size.width as usize * self.size.height as usize * texture2d::Texture2D::get_pixel_stride(self)
    }

    /// Port-only: encodes a copy of the texture's pixels into `buffer`,
    /// which holds `pixels_length()` bytes, rows tightly packed. False if
    /// there is nothing to copy.
    pub fn encode_copy_to(&self, command_buffer: &ProtocolObject<dyn MTLCommandBuffer>, buffer: &MTLBufferPtr) -> bool {
        let Some(texture) = self.metal_texture.as_ref() else {
            return false;
        };
        let length = self.pixels_length();
        debug_assert!(buffer.length() >= length);
        let Some(blit) = command_buffer.blitCommandEncoder() else {
            return false;
        };
        // SAFETY: the region is the whole texture and the buffer holds it.
        unsafe {
            blit.copyFromTexture_sourceSlice_sourceLevel_sourceOrigin_sourceSize_toBuffer_destinationOffset_destinationBytesPerRow_destinationBytesPerImage(
                texture,
                0,
                0,
                MTLOrigin { x: 0, y: 0, z: 0 },
                MTLSize { width: self.size.width as usize, height: self.size.height as usize, depth: 1 },
                buffer,
                0,
                self.size.width as usize * texture2d::Texture2D::get_pixel_stride(self),
                length,
            );
        }
        blit.endEncoding();
        true
    }

    /// Port-only: a shared buffer that holds the texture's pixels.
    pub fn new_pixels_buffer(&self) -> Option<MTLBufferPtr> {
        self.context
            .get_backend()
            .get_device()
            .newBufferWithLength_options(self.pixels_length().max(1), MTLResourceOptions::StorageModeShared)
    }

    /// Port-only: the texture's pixels, copied through shared memory once
    /// the GPU work queued before is done. Rows are tightly packed.
    pub fn read_pixels(&self) -> Option<Vec<u8>> {
        autoreleasepool(|_| {
            let buffer = self.new_pixels_buffer()?;
            let command_buffer = self.context.get_backend().get_command_queue().commandBuffer()?;
            if !self.encode_copy_to(&command_buffer, &buffer) {
                return None;
            }
            command_buffer.commit();
            command_buffer.waitUntilCompleted();
            Some(buffer_bytes(&buffer, self.pixels_length()).to_vec())
        })
    }
}

/// The first `length` bytes of a shared buffer that no GPU work writes.
pub(crate) fn buffer_bytes(buffer: &MTLBufferPtr, length: usize) -> &[u8] {
    debug_assert!(buffer.length() >= length);
    // SAFETY: the buffer is shared memory of at least `length` bytes, and no
    // GPU work writes it while the slice lives.
    unsafe { std::slice::from_raw_parts(buffer.contents().as_ptr().cast::<u8>(), length) }
}

impl Drop for Texture2D {
    fn drop(&mut self) {
        self.destroy_metal_texture();
    }
}

impl texture2d::Texture2D for Texture2D {
    fn set_sampler_configuration(&mut self, sampler_state: SamplerState) {
        if self.sampler_state.filter == sampler_state.filter
            && self.sampler_state.wrap_u == sampler_state.wrap_u
            && self.sampler_state.wrap_v == sampler_state.wrap_v
        {
            return;
        }
        self.sampler_state = sampler_state;
        self.sampler_state_dirty = true;
    }

    fn set_format(&mut self, pixel_format: TexturePixelType, channel_type: TextureChannelDataType) {
        if pixel_format == self.pixel_format && channel_type == self.channel_type {
            return;
        }
        self.destroy_metal_texture();
        self.pixel_format = pixel_format;
        self.channel_type = channel_type;
        self.texture_dirty = true;
    }

    fn set_size(&mut self, size: Size) {
        if size == self.size {
            return;
        }
        self.destroy_metal_texture();
        self.size = size;
        self.texture_dirty = true;
    }

    fn set_image(&mut self, image: Option<Rc<PremultipliedImage>>) {
        self.image = image;
    }

    fn get_format(&self) -> TexturePixelType {
        self.pixel_format
    }

    fn get_size(&self) -> Size {
        self.size
    }

    fn get_data_size(&self) -> usize {
        self.size.width as usize * self.size.height as usize * self.get_pixel_stride()
    }

    fn get_pixel_stride(&self) -> usize {
        match self.channel_type {
            TextureChannelDataType::UnsignedByte => self.num_channels(),
            TextureChannelDataType::HalfFloat => 2 * self.num_channels(),
            TextureChannelDataType::Float => 4 * self.num_channels(),
        }
    }

    fn num_channels(&self) -> usize {
        match self.pixel_format {
            TexturePixelType::RGBA => 4,
            TexturePixelType::Alpha | TexturePixelType::Stencil | TexturePixelType::Depth => 1,
            TexturePixelType::Luminance => 0,
        }
    }

    fn create(&mut self) {
        if self.texture_dirty {
            self.create_metal_texture();
        }
        if self.sampler_state_dirty {
            self.update_sampler_configuration();
        }
    }

    fn upload_data(&mut self, pixel_data: Option<&[u8]>, size: Size) {
        self.set_size(size);
        if self.texture_dirty {
            self.create_metal_texture();
        }
        if self.sampler_state_dirty {
            self.update_sampler_configuration();
        }
        if let Some(pixel_data) = pixel_data {
            self.upload_sub_region(Some(pixel_data), self.size, 0, 0);
        }
    }

    fn upload_sub_region(&mut self, pixel_data: Option<&[u8]>, size: Size, x_offset: u16, y_offset: u16) {
        let (Some(texture), Some(pixel_data)) = (self.metal_texture.as_ref(), pixel_data) else {
            return;
        };
        debug_assert!(!self.texture_dirty);
        let region = MTLRegion {
            origin: MTLOrigin { x: usize::from(x_offset), y: usize::from(y_offset), z: 0 },
            size: MTLSize { width: size.width as usize, height: size.height as usize, depth: 1 },
        };
        let bytes_per_row = size.width as usize * self.get_pixel_stride();
        debug_assert!(pixel_data.len() >= bytes_per_row * size.height as usize);
        if pixel_data.len() < bytes_per_row * size.height as usize {
            return;
        }
        // SAFETY: `pixel_data` holds the region's rows, which Metal copies.
        unsafe {
            texture.replaceRegion_mipmapLevel_withBytes_bytesPerRow(
                region,
                0,
                NonNull::from(pixel_data).cast::<c_void>(),
                bytes_per_row,
            );
        }
        let mut stats = self.context.rendering_stats();
        stats.num_texture_updates += 1;
        stats.texture_update_bytes += bytes_per_row * size.height as usize;
    }

    fn upload(&mut self) {
        if let Some(image) = self.image.take()
            && image.valid()
        {
            self.set_format(TexturePixelType::RGBA, TextureChannelDataType::UnsignedByte);
            self.upload_data(Some(&image.data), image.size);
        }
    }

    fn needs_upload(&self) -> bool {
        self.image.is_some()
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}
