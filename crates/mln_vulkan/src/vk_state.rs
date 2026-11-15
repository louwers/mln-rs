//! The Vulkan values of the interface's vertex formats and pipeline state.

use ash::vk;
use mln_gfx::gfx_types::{AttributeDataType, DrawModeType};
use mln_gfx::types::{
    ColorBlendEquationType, ColorBlendFactorType, CullFaceSideType, CullFaceWindingType, DepthFunctionType,
    StencilFunctionType, StencilOpType,
};

#[must_use]
pub(crate) fn format(value: AttributeDataType) -> vk::Format {
    use AttributeDataType as A;
    match value {
        A::Byte => vk::Format::R8_SINT,
        A::Byte2 => vk::Format::R8G8_SINT,
        A::Byte3 => vk::Format::R8G8B8_SINT,
        A::Byte4 => vk::Format::R8G8B8A8_SINT,
        A::UByte => vk::Format::R8_UINT,
        A::UByte2 => vk::Format::R8G8_UINT,
        A::UByte3 => vk::Format::R8G8B8_UINT,
        A::UByte4 => vk::Format::R8G8B8A8_UINT,
        A::UByte4Normalized => vk::Format::R8G8B8A8_UNORM,
        A::Short => vk::Format::R16_SINT,
        A::Short2 => vk::Format::R16G16_SINT,
        A::Short3 => vk::Format::R16G16B16_SINT,
        A::Short4 => vk::Format::R16G16B16A16_SINT,
        A::UShort => vk::Format::R16_UINT,
        A::UShort2 => vk::Format::R16G16_UINT,
        A::UShort3 => vk::Format::R16G16B16_UINT,
        A::UShort4 => vk::Format::R16G16B16A16_UINT,
        A::Int => vk::Format::R32_SINT,
        A::Int2 => vk::Format::R32G32_SINT,
        A::Int3 => vk::Format::R32G32B32_SINT,
        A::Int4 => vk::Format::R32G32B32A32_SINT,
        A::UInt => vk::Format::R32_UINT,
        A::UInt2 => vk::Format::R32G32_UINT,
        A::UInt3 => vk::Format::R32G32B32_UINT,
        A::UInt4 => vk::Format::R32G32B32A32_UINT,
        A::Float => vk::Format::R32_SFLOAT,
        A::Float2 => vk::Format::R32G32_SFLOAT,
        A::Float3 => vk::Format::R32G32B32_SFLOAT,
        A::Float4 => vk::Format::R32G32B32A32_SFLOAT,
        _ => vk::Format::UNDEFINED,
    }
}

#[must_use]
pub(crate) fn primitive_topology(value: DrawModeType) -> vk::PrimitiveTopology {
    match value {
        DrawModeType::Points => vk::PrimitiveTopology::POINT_LIST,
        DrawModeType::Lines => vk::PrimitiveTopology::LINE_LIST,
        DrawModeType::LineLoop | DrawModeType::LineStrip => vk::PrimitiveTopology::LINE_STRIP,
        DrawModeType::Triangles => vk::PrimitiveTopology::TRIANGLE_LIST,
        DrawModeType::TriangleStrip => vk::PrimitiveTopology::TRIANGLE_STRIP,
        DrawModeType::TriangleFan => vk::PrimitiveTopology::TRIANGLE_FAN,
    }
}

#[must_use]
pub(crate) fn cull_mode(value: CullFaceSideType) -> vk::CullModeFlags {
    match value {
        CullFaceSideType::Back => vk::CullModeFlags::BACK,
        CullFaceSideType::Front => vk::CullModeFlags::FRONT,
        CullFaceSideType::FrontAndBack => vk::CullModeFlags::FRONT_AND_BACK,
    }
}

#[must_use]
pub(crate) fn front_face(value: CullFaceWindingType) -> vk::FrontFace {
    match value {
        CullFaceWindingType::Clockwise => vk::FrontFace::CLOCKWISE,
        CullFaceWindingType::CounterClockwise => vk::FrontFace::COUNTER_CLOCKWISE,
    }
}

#[must_use]
pub(crate) fn blend_op(value: ColorBlendEquationType) -> vk::BlendOp {
    match value {
        ColorBlendEquationType::Add => vk::BlendOp::ADD,
        ColorBlendEquationType::Subtract => vk::BlendOp::SUBTRACT,
        ColorBlendEquationType::ReverseSubtract => vk::BlendOp::REVERSE_SUBTRACT,
    }
}

#[must_use]
pub(crate) fn blend_factor(value: ColorBlendFactorType) -> vk::BlendFactor {
    use ColorBlendFactorType as F;
    match value {
        F::Zero => vk::BlendFactor::ZERO,
        F::One => vk::BlendFactor::ONE,
        F::SrcColor => vk::BlendFactor::SRC_COLOR,
        F::OneMinusSrcColor => vk::BlendFactor::ONE_MINUS_SRC_COLOR,
        F::SrcAlpha => vk::BlendFactor::SRC_ALPHA,
        F::OneMinusSrcAlpha => vk::BlendFactor::ONE_MINUS_SRC_ALPHA,
        F::DstAlpha => vk::BlendFactor::DST_ALPHA,
        F::OneMinusDstAlpha => vk::BlendFactor::ONE_MINUS_DST_ALPHA,
        F::DstColor => vk::BlendFactor::DST_COLOR,
        F::OneMinusDstColor => vk::BlendFactor::ONE_MINUS_DST_COLOR,
        F::SrcAlphaSaturate => vk::BlendFactor::SRC_ALPHA_SATURATE,
        F::ConstantColor => vk::BlendFactor::CONSTANT_COLOR,
        F::OneMinusConstantColor => vk::BlendFactor::ONE_MINUS_CONSTANT_COLOR,
        F::ConstantAlpha => vk::BlendFactor::CONSTANT_ALPHA,
        F::OneMinusConstantAlpha => vk::BlendFactor::ONE_MINUS_CONSTANT_ALPHA,
    }
}

#[must_use]
pub(crate) fn depth_compare_op(value: DepthFunctionType) -> vk::CompareOp {
    match value {
        DepthFunctionType::Never => vk::CompareOp::NEVER,
        DepthFunctionType::Less => vk::CompareOp::LESS,
        DepthFunctionType::Equal => vk::CompareOp::EQUAL,
        DepthFunctionType::LessEqual => vk::CompareOp::LESS_OR_EQUAL,
        DepthFunctionType::Greater => vk::CompareOp::GREATER,
        DepthFunctionType::NotEqual => vk::CompareOp::NOT_EQUAL,
        DepthFunctionType::GreaterEqual => vk::CompareOp::GREATER_OR_EQUAL,
        DepthFunctionType::Always => vk::CompareOp::ALWAYS,
    }
}

#[must_use]
pub(crate) fn stencil_compare_op(value: StencilFunctionType) -> vk::CompareOp {
    match value {
        StencilFunctionType::Never => vk::CompareOp::NEVER,
        StencilFunctionType::Less => vk::CompareOp::LESS,
        StencilFunctionType::Equal => vk::CompareOp::EQUAL,
        StencilFunctionType::LessEqual => vk::CompareOp::LESS_OR_EQUAL,
        StencilFunctionType::Greater => vk::CompareOp::GREATER,
        StencilFunctionType::NotEqual => vk::CompareOp::NOT_EQUAL,
        StencilFunctionType::GreaterEqual => vk::CompareOp::GREATER_OR_EQUAL,
        StencilFunctionType::Always => vk::CompareOp::ALWAYS,
    }
}

#[must_use]
pub(crate) fn stencil_op(value: StencilOpType) -> vk::StencilOp {
    match value {
        StencilOpType::Zero => vk::StencilOp::ZERO,
        StencilOpType::Keep => vk::StencilOp::KEEP,
        StencilOpType::Replace => vk::StencilOp::REPLACE,
        StencilOpType::Increment => vk::StencilOp::INCREMENT_AND_CLAMP,
        StencilOpType::Decrement => vk::StencilOp::DECREMENT_AND_CLAMP,
        StencilOpType::Invert => vk::StencilOp::INVERT,
        StencilOpType::IncrementWrap => vk::StencilOp::INCREMENT_AND_WRAP,
        StencilOpType::DecrementWrap => vk::StencilOp::DECREMENT_AND_WRAP,
    }
}
