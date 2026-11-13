// `Enum<T>::from` reads GL state back, which only the C++ tests of the state
// values do.

use mln_gfx::gfx_types::{AttributeDataType, DrawModeType};
use mln_gfx::types::{
    BufferUsageType, ColorBlendEquationType, ColorBlendFactorType, CullFaceSideType, CullFaceWindingType,
    DepthFunctionType, StencilFunctionType, StencilOpType, TextureChannelDataType, TextureFilterType, TexturePixelType,
    TextureWrapType,
};

#[must_use]
pub(crate) fn draw_mode(value: DrawModeType) -> u32 {
    match value {
        DrawModeType::Points => glow::POINTS,
        DrawModeType::Lines => glow::LINES,
        DrawModeType::LineLoop => glow::LINE_LOOP,
        DrawModeType::LineStrip => glow::LINE_STRIP,
        DrawModeType::Triangles => glow::TRIANGLES,
        DrawModeType::TriangleStrip => glow::TRIANGLE_STRIP,
        DrawModeType::TriangleFan => glow::TRIANGLE_FAN,
    }
}

#[must_use]
pub(crate) fn color_blend_equation(value: ColorBlendEquationType) -> u32 {
    match value {
        ColorBlendEquationType::Add => glow::FUNC_ADD,
        ColorBlendEquationType::Subtract => glow::FUNC_SUBTRACT,
        ColorBlendEquationType::ReverseSubtract => glow::FUNC_REVERSE_SUBTRACT,
    }
}

#[must_use]
pub(crate) fn color_blend_factor(value: ColorBlendFactorType) -> u32 {
    match value {
        ColorBlendFactorType::Zero => glow::ZERO,
        ColorBlendFactorType::One => glow::ONE,
        ColorBlendFactorType::SrcColor => glow::SRC_COLOR,
        ColorBlendFactorType::OneMinusSrcColor => glow::ONE_MINUS_SRC_COLOR,
        ColorBlendFactorType::DstColor => glow::DST_COLOR,
        ColorBlendFactorType::OneMinusDstColor => glow::ONE_MINUS_DST_COLOR,
        ColorBlendFactorType::SrcAlpha => glow::SRC_ALPHA,
        ColorBlendFactorType::OneMinusSrcAlpha => glow::ONE_MINUS_SRC_ALPHA,
        ColorBlendFactorType::DstAlpha => glow::DST_ALPHA,
        ColorBlendFactorType::OneMinusDstAlpha => glow::ONE_MINUS_DST_ALPHA,
        ColorBlendFactorType::ConstantColor => glow::CONSTANT_COLOR,
        ColorBlendFactorType::OneMinusConstantColor => glow::ONE_MINUS_CONSTANT_COLOR,
        ColorBlendFactorType::ConstantAlpha => glow::CONSTANT_ALPHA,
        ColorBlendFactorType::OneMinusConstantAlpha => glow::ONE_MINUS_CONSTANT_ALPHA,
        ColorBlendFactorType::SrcAlphaSaturate => glow::SRC_ALPHA_SATURATE,
    }
}

#[must_use]
pub(crate) fn depth_function(value: DepthFunctionType) -> u32 {
    match value {
        DepthFunctionType::Never => glow::NEVER,
        DepthFunctionType::Less => glow::LESS,
        DepthFunctionType::Equal => glow::EQUAL,
        DepthFunctionType::LessEqual => glow::LEQUAL,
        DepthFunctionType::Greater => glow::GREATER,
        DepthFunctionType::NotEqual => glow::NOTEQUAL,
        DepthFunctionType::GreaterEqual => glow::GEQUAL,
        DepthFunctionType::Always => glow::ALWAYS,
    }
}

#[must_use]
pub(crate) fn stencil_function(value: StencilFunctionType) -> u32 {
    match value {
        StencilFunctionType::Never => glow::NEVER,
        StencilFunctionType::Less => glow::LESS,
        StencilFunctionType::Equal => glow::EQUAL,
        StencilFunctionType::LessEqual => glow::LEQUAL,
        StencilFunctionType::Greater => glow::GREATER,
        StencilFunctionType::NotEqual => glow::NOTEQUAL,
        StencilFunctionType::GreaterEqual => glow::GEQUAL,
        StencilFunctionType::Always => glow::ALWAYS,
    }
}

#[must_use]
pub(crate) fn stencil_op(value: StencilOpType) -> u32 {
    match value {
        StencilOpType::Zero => glow::ZERO,
        StencilOpType::Keep => glow::KEEP,
        StencilOpType::Replace => glow::REPLACE,
        StencilOpType::Increment => glow::INCR,
        StencilOpType::Decrement => glow::DECR,
        StencilOpType::Invert => glow::INVERT,
        StencilOpType::IncrementWrap => glow::INCR_WRAP,
        StencilOpType::DecrementWrap => glow::DECR_WRAP,
    }
}

#[must_use]
pub(crate) fn cull_face_side(value: CullFaceSideType) -> u32 {
    match value {
        CullFaceSideType::Front => glow::FRONT,
        CullFaceSideType::Back => glow::BACK,
        CullFaceSideType::FrontAndBack => glow::FRONT_AND_BACK,
    }
}

#[must_use]
pub(crate) fn cull_face_winding(value: CullFaceWindingType) -> u32 {
    match value {
        CullFaceWindingType::Clockwise => glow::CW,
        CullFaceWindingType::CounterClockwise => glow::CCW,
    }
}

#[must_use]
pub(crate) fn buffer_usage(value: BufferUsageType) -> u32 {
    match value {
        BufferUsageType::StreamDraw => glow::STREAM_DRAW,
        BufferUsageType::StaticDraw => glow::STATIC_DRAW,
        BufferUsageType::DynamicDraw => glow::DYNAMIC_DRAW,
    }
}

/// The format of the pixels of a texture of `value` pixels. Desktop OpenGL's
/// core profile has no alpha or luminance textures, so a `desktop` context
/// keeps them in the red channel ([`texture_swizzle`]).
#[must_use]
pub(crate) fn texture_pixel(value: TexturePixelType, desktop: bool) -> u32 {
    match value {
        TexturePixelType::RGBA => glow::RGBA,
        TexturePixelType::Alpha | TexturePixelType::Luminance if desktop => glow::RED,
        TexturePixelType::Alpha => glow::ALPHA,
        TexturePixelType::Stencil => glow::STENCIL_INDEX,
        TexturePixelType::Depth => glow::DEPTH_COMPONENT,
        TexturePixelType::Luminance => glow::LUMINANCE,
    }
}

/// The internal format of a texture of `value` pixels with `type_` channels,
/// on a `desktop` context or an OpenGL ES one.
#[must_use]
pub(crate) fn texture_pixel_sized_for(value: TexturePixelType, type_: TextureChannelDataType, desktop: bool) -> u32 {
    match (type_, value) {
        (TextureChannelDataType::UnsignedByte, TexturePixelType::Alpha | TexturePixelType::Luminance) if desktop => {
            glow::R8
        }
        (TextureChannelDataType::UnsignedByte, _) => texture_pixel(value, desktop),
        (TextureChannelDataType::HalfFloat, TexturePixelType::RGBA) => glow::RGBA16F,
        (TextureChannelDataType::Float, TexturePixelType::RGBA) => glow::RGBA32F,
        _ => {
            debug_assert!(false, "unsupported texture format");
            texture_pixel(value, desktop)
        }
    }
}

/// The `GL_TEXTURE_SWIZZLE_R`, `_G`, `_B` and `_A` of a texture of `value`
/// pixels on a `desktop` context, which reads its alpha or luminance
/// textures from the red channel, or `None` where the texture reads its
/// channels as they are.
#[must_use]
pub(crate) fn texture_swizzle(value: TexturePixelType, desktop: bool) -> Option<[u32; 4]> {
    match value {
        TexturePixelType::Alpha if desktop => Some([glow::ZERO, glow::ZERO, glow::ZERO, glow::RED]),
        TexturePixelType::Luminance if desktop => Some([glow::RED, glow::RED, glow::RED, glow::ONE]),
        _ => None,
    }
}

#[must_use]
pub(crate) fn texture_channel_data(value: TextureChannelDataType) -> u32 {
    match value {
        TextureChannelDataType::UnsignedByte => glow::UNSIGNED_BYTE,
        TextureChannelDataType::HalfFloat => glow::HALF_FLOAT,
        TextureChannelDataType::Float => glow::FLOAT,
    }
}

#[must_use]
pub(crate) fn texture_filter(value: TextureFilterType) -> i32 {
    match value {
        TextureFilterType::Nearest => glow::NEAREST as i32,
        TextureFilterType::Linear => glow::LINEAR as i32,
    }
}

#[must_use]
pub(crate) fn texture_wrap(value: TextureWrapType) -> i32 {
    match value {
        TextureWrapType::Clamp => glow::CLAMP_TO_EDGE as i32,
        TextureWrapType::Repeat => glow::REPEAT as i32,
    }
}

/// How `glVertexAttribPointer` reads an attribute of `value`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct AttributeFormat {
    pub(crate) components: i32,
    pub(crate) type_: u32,
    pub(crate) normalized: bool,
}

#[must_use]
pub(crate) fn attribute_format(value: AttributeDataType) -> Option<AttributeFormat> {
    use AttributeDataType as A;
    let (components, type_, normalized) = match value {
        A::Byte => (1, glow::BYTE, false),
        A::Byte2 => (2, glow::BYTE, false),
        A::Byte3 => (3, glow::BYTE, false),
        A::Byte4 => (4, glow::BYTE, false),
        A::UByte => (1, glow::UNSIGNED_BYTE, false),
        A::UByte2 => (2, glow::UNSIGNED_BYTE, false),
        A::UByte3 => (3, glow::UNSIGNED_BYTE, false),
        A::UByte4 => (4, glow::UNSIGNED_BYTE, false),
        A::UByte4Normalized => (4, glow::UNSIGNED_BYTE, true),
        A::Short => (1, glow::SHORT, false),
        A::Short2 => (2, glow::SHORT, false),
        A::Short3 => (3, glow::SHORT, false),
        A::Short4 => (4, glow::SHORT, false),
        A::UShort => (1, glow::UNSIGNED_SHORT, false),
        A::UShort2 => (2, glow::UNSIGNED_SHORT, false),
        A::UShort3 => (3, glow::UNSIGNED_SHORT, false),
        A::UShort4 => (4, glow::UNSIGNED_SHORT, false),
        A::Int => (1, glow::INT, false),
        A::Int2 => (2, glow::INT, false),
        A::Int3 => (3, glow::INT, false),
        A::Int4 => (4, glow::INT, false),
        A::UInt => (1, glow::UNSIGNED_INT, false),
        A::UInt2 => (2, glow::UNSIGNED_INT, false),
        A::UInt3 => (3, glow::UNSIGNED_INT, false),
        A::UInt4 => (4, glow::UNSIGNED_INT, false),
        A::Float => (1, glow::FLOAT, false),
        A::Float2 => (2, glow::FLOAT, false),
        A::Float3 => (3, glow::FLOAT, false),
        A::Float4 => (4, glow::FLOAT, false),
        A::UShort8 | A::Invalid => return None,
    };
    Some(AttributeFormat { components, type_, normalized })
}
