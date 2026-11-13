use std::any::Any;
use std::cell::RefCell;
use std::rc::Rc;

use mln_util::image::PremultipliedImage;
use mln_util::size::Size;

use crate::types::{TextureChannelDataType, TextureFilterType, TexturePixelType, TextureWrapType};

pub const MAX_ACTIVE_TEXTURE_UNITS: i32 = 8;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SamplerState {
    pub filter: TextureFilterType,
    pub wrap_u: TextureWrapType,
    pub wrap_v: TextureWrapType,
    pub max_anisotropy: u8,
    pub mipmapped: bool,
}

impl Default for SamplerState {
    fn default() -> Self {
        SamplerState {
            filter: TextureFilterType::Nearest,
            wrap_u: TextureWrapType::Clamp,
            wrap_v: TextureWrapType::Clamp,
            max_anisotropy: 1,
            mipmapped: false,
        }
    }
}

/// A backend texture. Unlike C++'s, the setters don't return the texture for
/// chaining.
pub trait Texture2D: Any {
    fn set_sampler_configuration(&mut self, sampler_state: SamplerState);
    fn set_format(&mut self, pixel_format: TexturePixelType, channel_type: TextureChannelDataType);
    fn set_size(&mut self, size: Size);
    fn set_image(&mut self, image: Option<Rc<PremultipliedImage>>);
    fn get_format(&self) -> TexturePixelType;
    fn get_size(&self) -> Size;
    fn get_data_size(&self) -> usize;
    fn get_pixel_stride(&self) -> usize;
    fn num_channels(&self) -> usize;
    fn create(&mut self);
    fn upload_data(&mut self, pixel_data: Option<&[u8]>, size: Size);
    fn upload_sub_region(&mut self, pixel_data: Option<&[u8]>, size: Size, x_offset: u16, y_offset: u16);
    /// Uploads the image set with `set_image`.
    fn upload(&mut self);
    fn needs_upload(&self) -> bool;
}

impl dyn Texture2D {
    /// The concrete type behind this trait object, if it is a `T`.
    pub fn downcast_ref<T: Texture2D>(&self) -> Option<&T> {
        let any: &dyn Any = self;
        any.downcast_ref()
    }

    /// The concrete type behind this trait object, if it is a `T`.
    pub fn downcast_mut<T: Texture2D>(&mut self) -> Option<&mut T> {
        let any: &mut dyn Any = self;
        any.downcast_mut()
    }
}

/// A shared texture, mutated through the shared handle (e.g.
/// `set_sampler_configuration` in tweakers), hence the `RefCell`.
pub type Texture2DPtr = Rc<RefCell<dyn Texture2D>>;

/// Uploads a premultiplied RGBA image to `texture`, setting its format.
pub fn upload_image(texture: &mut dyn Texture2D, img: &PremultipliedImage) {
    texture.set_format(TexturePixelType::RGBA, TextureChannelDataType::UnsignedByte);
    let data = if img.data.is_empty() { None } else { Some(img.data.as_slice()) };
    texture.upload_data(data, img.size);
}
