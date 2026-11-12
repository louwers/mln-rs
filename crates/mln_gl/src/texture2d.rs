// Ports include/mln/gl/texture2d.hpp and src/mln/gl/texture2d.cpp
//
// Differences from C++:
// - A shader's samplers read fixed texture units (`ShaderProgram`), so
//   `bind` takes the unit alone, and nothing is unbound after a draw. An
//   offscreen texture unbinds its texture before it is rendered into
//   (offscreen_texture.rs).
// - Each texture is a GL texture of its own (C++ takes them from a pool).

use std::any::Any;
use std::rc::{Rc, Weak};

use glow::HasContext;
use mln_gfx::context::Context as _;
use mln_gfx::texture2d::{self, SamplerState};
use mln_gfx::types::{TextureChannelDataType, TexturePixelType};
use mln_util::image::PremultipliedImage;
use mln_util::logging::{Event, Log};
use mln_util::size::Size;

use crate::context::Context;
use crate::enums;
use crate::types::TextureID;

pub struct Texture2D {
    context: Weak<Context>,
    texture: Option<TextureID>,
    size: Size,
    pixel_format: TexturePixelType,
    channel_type: TextureChannelDataType,
    sampler_state: SamplerState,
    image: Option<Rc<PremultipliedImage>>,
    sampler_state_dirty: bool,
    storage_dirty: bool,
    /// The bytes of the texture object's storage.
    allocated_bytes: i32,
}

impl Texture2D {
    #[must_use]
    pub fn new(context: Weak<Context>) -> Self {
        Texture2D {
            context,
            texture: None,
            size: Size::new(0, 0),
            pixel_format: TexturePixelType::RGBA,
            channel_type: TextureChannelDataType::UnsignedByte,
            sampler_state: SamplerState::default(),
            image: None,
            sampler_state_dirty: false,
            storage_dirty: false,
            allocated_bytes: 0,
        }
    }

    /// `getTextureID()`
    #[must_use]
    pub fn get_texture_id(&self) -> Option<TextureID> {
        self.texture
    }

    /// `allocateTexture()`: a new texture object with storage for the size
    /// and format, bound to texture unit 0.
    fn allocate_texture(&mut self, context: &Context) {
        self.release_texture(context);
        let gl = context.gl();
        // SAFETY: the texture is new, and its storage has no data yet.
        unsafe {
            let texture = match gl.create_texture() {
                Ok(texture) => texture,
                Err(error) => {
                    Log::error(Event::GraphicsBackend, format!("Failed to create a texture: {error}"));
                    return;
                }
            };
            context.state().activate_texture(gl, 0, Some(texture));
            let desktop = context.is_desktop();
            gl.tex_image_2d(
                glow::TEXTURE_2D,
                0,
                enums::texture_pixel_sized_for(self.pixel_format, self.channel_type, desktop) as i32,
                self.size.width as i32,
                self.size.height as i32,
                0,
                enums::texture_pixel(self.pixel_format, desktop),
                enums::texture_channel_data(self.channel_type),
                glow::PixelUnpackData::Slice(None),
            );
            if let Some(swizzle) = enums::texture_swizzle(self.pixel_format, desktop) {
                let names = [
                    glow::TEXTURE_SWIZZLE_R,
                    glow::TEXTURE_SWIZZLE_G,
                    glow::TEXTURE_SWIZZLE_B,
                    glow::TEXTURE_SWIZZLE_A,
                ];
                for (name, source) in names.into_iter().zip(swizzle) {
                    gl.tex_parameter_i32(glow::TEXTURE_2D, name, source as i32);
                }
            }
            self.texture = Some(texture);
        }
        self.allocated_bytes = texture2d::Texture2D::get_data_size(self) as i32;
        let mut stats = context.rendering_stats();
        stats.num_created_textures += 1;
        stats.num_active_textures += 1;
        stats.mem_textures += self.allocated_bytes;
    }

    fn release_texture(&mut self, context: &Context) {
        if let Some(texture) = self.texture.take() {
            context.abandon_texture(texture);
            let mut stats = context.rendering_stats();
            stats.num_active_textures -= 1;
            stats.mem_textures -= self.allocated_bytes;
        }
    }

    /// `updateTextureData(data)`
    fn update_texture_data(&mut self, context: &Context, data: Option<&[u8]>) {
        if data.is_some() {
            let size = self.size;
            self.upload_sub_region_in(context, data, size, 0, 0);
        }
        self.storage_dirty = false;
        self.update_sampler_configuration(context, 0);
    }

    /// `updateSamplerConfiguration()`, with the texture bound to
    /// `texture_unit`.
    fn update_sampler_configuration(&mut self, context: &Context, texture_unit: u32) {
        self.sampler_state_dirty = false;
        let gl = context.gl();
        context.state().activate_texture(gl, texture_unit, self.texture);
        let filter = enums::texture_filter(self.sampler_state.filter);
        // SAFETY: the texture is bound to the active unit.
        unsafe {
            gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MIN_FILTER, filter);
            gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MAG_FILTER, filter);
            gl.tex_parameter_i32(
                glow::TEXTURE_2D,
                glow::TEXTURE_WRAP_S,
                enums::texture_wrap(self.sampler_state.wrap_u),
            );
            gl.tex_parameter_i32(
                glow::TEXTURE_2D,
                glow::TEXTURE_WRAP_T,
                enums::texture_wrap(self.sampler_state.wrap_v),
            );
        }
    }

    /// `bind(location, textureUnit)`: binds the texture to `texture_unit`,
    /// which the sampler reads.
    pub fn bind(&mut self, context: &Context, texture_unit: u32) {
        debug_assert!((texture_unit as i32) < texture2d::MAX_ACTIVE_TEXTURE_UNITS);
        // Update the sampler state if it was changed after resource creation
        if self.sampler_state_dirty && self.texture.is_some() {
            self.update_sampler_configuration(context, texture_unit);
        }
        context.state().bind_texture(context.gl(), texture_unit, self.texture);
        context.rendering_stats().num_texture_bindings += 1;
    }

    fn upload_sub_region_in(
        &mut self,
        context: &Context,
        pixel_data: Option<&[u8]>,
        size: Size,
        x_offset: u16,
        y_offset: u16,
    ) {
        debug_assert!(self.texture.is_some());
        let Some(pixel_data) = pixel_data else {
            return;
        };
        let stride = texture2d::Texture2D::get_pixel_stride(self);
        if self.texture.is_none() || pixel_data.len() < stride * size.width as usize * size.height as usize {
            debug_assert!(false, "texture upload without a texture or with too few bytes");
            return;
        }
        // update sampler configuration if needed
        if self.sampler_state_dirty {
            self.update_sampler_configuration(context, 0);
        }
        let gl = context.gl();
        // Bind to TU 0 and upload
        context.state().activate_texture(gl, 0, self.texture);
        context.state().set_pixel_store_unpack_alignment(gl, 1);
        // SAFETY: the region is inside the texture's storage, and
        // `pixel_data` holds its pixels, tightly packed.
        unsafe {
            gl.tex_sub_image_2d(
                glow::TEXTURE_2D,
                0,
                i32::from(x_offset),
                i32::from(y_offset),
                size.width as i32,
                size.height as i32,
                enums::texture_pixel(self.pixel_format, context.is_desktop()),
                enums::texture_channel_data(self.channel_type),
                glow::PixelUnpackData::Slice(Some(pixel_data)),
            );
        }
        let mut stats = context.rendering_stats();
        stats.num_texture_updates += 1;
        stats.texture_update_bytes += stride * size.width as usize * size.height as usize;
    }
}

impl Drop for Texture2D {
    fn drop(&mut self) {
        if let Some(context) = self.context.upgrade() {
            self.release_texture(&context);
        }
    }
}

impl texture2d::Texture2D for Texture2D {
    fn set_sampler_configuration(&mut self, sampler_state: SamplerState) {
        self.sampler_state = sampler_state;
        self.sampler_state_dirty = true;
    }

    fn set_format(&mut self, pixel_format: TexturePixelType, channel_type: TextureChannelDataType) {
        if pixel_format == self.pixel_format && channel_type == self.channel_type {
            return;
        }
        self.pixel_format = pixel_format;
        self.channel_type = channel_type;
        self.storage_dirty = true;
    }

    fn set_size(&mut self, size: Size) {
        self.size = size;
        self.storage_dirty = true;
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
            TexturePixelType::Alpha => 1,
            _ => 0,
        }
    }

    fn create(&mut self) {
        let Some(context) = self.context.upgrade() else {
            return;
        };
        self.allocate_texture(&context);
        if self.storage_dirty {
            self.update_texture_data(&context, None);
        }
    }

    fn upload_data(&mut self, pixel_data: Option<&[u8]>, size: Size) {
        let Some(context) = self.context.upgrade() else {
            return;
        };
        if self.texture.is_none() || self.storage_dirty || size == Size::new(0, 0) || size != self.size {
            self.size = size;
            // Create the texture object if we don't already have one or if
            // storage is dirty
            self.allocate_texture(&context);
        }
        self.update_texture_data(&context, pixel_data);
    }

    fn upload_sub_region(&mut self, pixel_data: Option<&[u8]>, size: Size, x_offset: u16, y_offset: u16) {
        let Some(context) = self.context.upgrade() else {
            return;
        };
        self.upload_sub_region_in(&context, pixel_data, size, x_offset, y_offset);
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
