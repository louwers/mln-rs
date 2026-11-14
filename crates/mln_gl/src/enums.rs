// The GL enums of the renderer's types.

use mln_gfx::gfx_types::{AttributeDataType, DrawModeType};
use mln_gfx::types::{
    ColorBlendEquationType, ColorBlendFactorType, CullFaceSideType, CullFaceWindingType, DepthFunctionType,
    StencilFunctionType, StencilOpType, TextureFilterType, TextureWrapType,
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
