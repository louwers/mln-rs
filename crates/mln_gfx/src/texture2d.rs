//! How the renderer's textures are sampled, and the formats they have.

use crate::gpu::TextureFormat;
use crate::types::{TextureChannelDataType, TextureFilterType, TexturePixelType, TextureWrapType};

pub const MAX_ACTIVE_TEXTURE_UNITS: i32 = 8;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
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

/// The texture format of `pixel` pixels of `channel` channels that a shader
/// samples.
#[must_use]
pub fn texture_format(pixel: TexturePixelType, channel: TextureChannelDataType) -> TextureFormat {
    match (pixel, channel) {
        (TexturePixelType::Alpha | TexturePixelType::Luminance, TextureChannelDataType::UnsignedByte) => {
            TextureFormat::A8Unorm
        }
        (TexturePixelType::Alpha | TexturePixelType::Luminance, TextureChannelDataType::HalfFloat) => {
            TextureFormat::R16Float
        }
        (TexturePixelType::Alpha | TexturePixelType::Luminance, TextureChannelDataType::Float) => {
            TextureFormat::R32Float
        }
        (TexturePixelType::Depth, _) => TextureFormat::Depth32Float,
        (TexturePixelType::Stencil, _) => TextureFormat::Depth24PlusStencil8,
        (TexturePixelType::RGBA, TextureChannelDataType::UnsignedByte) => TextureFormat::Rgba8Unorm,
        (TexturePixelType::RGBA, TextureChannelDataType::HalfFloat) => TextureFormat::Rgba16Float,
        (TexturePixelType::RGBA, TextureChannelDataType::Float) => TextureFormat::Rgba32Float,
    }
}
