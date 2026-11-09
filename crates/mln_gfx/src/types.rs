// Ports include/mln/gfx/types.hpp

use serde::{Deserialize, Serialize};

/// Color blending equation type
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum ColorBlendEquationType {
    /// O = sS + dD
    Add,
    /// O = sS - dD
    Subtract,
    /// O = dD - sS
    ReverseSubtract,
}

/// Color blending factor type
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum ColorBlendFactorType {
    Zero,
    One,
    SrcColor,
    OneMinusSrcColor,
    SrcAlpha,
    OneMinusSrcAlpha,
    DstAlpha,
    OneMinusDstAlpha,
    DstColor,
    OneMinusDstColor,
    SrcAlphaSaturate,
    ConstantColor,
    OneMinusConstantColor,
    ConstantAlpha,
    OneMinusConstantAlpha,
}

/// Depth function type
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum DepthFunctionType {
    Never,
    Less,
    Equal,
    LessEqual,
    Greater,
    NotEqual,
    GreaterEqual,
    Always,
}

/// Depth buffer masking type (`enum class DepthMaskType : bool`)
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum DepthMaskType {
    /// The depth buffer is not written to
    ReadOnly = 0,
    /// The depth buffer is updated by the new fragments' depth
    ReadWrite = 1,
}

/// Stencil function type
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum StencilFunctionType {
    Never,
    Less,
    Equal,
    LessEqual,
    Greater,
    NotEqual,
    GreaterEqual,
    Always,
}

/// Stencil operation type
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum StencilOpType {
    Zero,
    Keep,
    Replace,
    Increment,
    Decrement,
    Invert,
    IncrementWrap,
    DecrementWrap,
}

/// Cull face side
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum CullFaceSideType {
    Front,
    Back,
    FrontAndBack,
}

/// Define the orientation of front facing polygons
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum CullFaceWindingType {
    Clockwise,
    CounterClockwise,
}

/// Buffer usage type
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum BufferUsageType {
    StreamDraw,
    StaticDraw,
    DynamicDraw,
}

/// Texture pixel type
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[repr(u8)]
pub enum TexturePixelType {
    RGBA,
    Alpha,
    Stencil,
    Depth,
    Luminance,
}

/// Texture channel data type
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum TextureChannelDataType {
    UnsignedByte,
    HalfFloat,
    Float,
}

/// Texture mip map type
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TextureMipMapType {
    No,
    Yes,
}

/// Texture filter type
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TextureFilterType {
    Nearest,
    Linear,
}

/// Texture wrap mode
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TextureWrapType {
    Clamp,
    Repeat,
}

/// Render buffer pixel type
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum RenderbufferPixelType {
    RGBA,
    Depth,
    DepthStencil,
}
