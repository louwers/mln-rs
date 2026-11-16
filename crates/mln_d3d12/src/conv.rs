//! The D3D12 values of the interface's formats and pipeline state.

use mln_gfx::color_mode::BlendFunction;
use mln_gfx::gfx_types::{AttributeDataType, DrawModeType};
use mln_gfx::gpu::{FixedState, TextureFormat};
use mln_gfx::types::{
    ColorBlendEquationType, ColorBlendFactorType, CullFaceSideType, CullFaceWindingType, DepthFunctionType,
    StencilFunctionType, StencilOpType,
};
use windows::Win32::Foundation::FALSE;
use windows::Win32::Graphics::Direct3D::{
    D3D_PRIMITIVE_TOPOLOGY, D3D_PRIMITIVE_TOPOLOGY_LINELIST, D3D_PRIMITIVE_TOPOLOGY_LINESTRIP,
    D3D_PRIMITIVE_TOPOLOGY_POINTLIST, D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST, D3D_PRIMITIVE_TOPOLOGY_TRIANGLESTRIP,
};
use windows::Win32::Graphics::Direct3D12::{
    D3D12_BLEND, D3D12_BLEND_BLEND_FACTOR, D3D12_BLEND_DESC, D3D12_BLEND_DEST_ALPHA, D3D12_BLEND_DEST_COLOR,
    D3D12_BLEND_INV_BLEND_FACTOR, D3D12_BLEND_INV_DEST_ALPHA, D3D12_BLEND_INV_DEST_COLOR, D3D12_BLEND_INV_SRC_ALPHA,
    D3D12_BLEND_INV_SRC_COLOR, D3D12_BLEND_ONE, D3D12_BLEND_OP, D3D12_BLEND_OP_ADD, D3D12_BLEND_OP_REV_SUBTRACT,
    D3D12_BLEND_OP_SUBTRACT, D3D12_BLEND_SRC_ALPHA, D3D12_BLEND_SRC_ALPHA_SAT, D3D12_BLEND_SRC_COLOR, D3D12_BLEND_ZERO,
    D3D12_COLOR_WRITE_ENABLE_ALPHA, D3D12_COLOR_WRITE_ENABLE_BLUE, D3D12_COLOR_WRITE_ENABLE_GREEN,
    D3D12_COLOR_WRITE_ENABLE_RED, D3D12_COMPARISON_FUNC, D3D12_COMPARISON_FUNC_ALWAYS, D3D12_COMPARISON_FUNC_EQUAL,
    D3D12_COMPARISON_FUNC_GREATER, D3D12_COMPARISON_FUNC_GREATER_EQUAL, D3D12_COMPARISON_FUNC_LESS,
    D3D12_COMPARISON_FUNC_LESS_EQUAL, D3D12_COMPARISON_FUNC_NEVER, D3D12_COMPARISON_FUNC_NOT_EQUAL,
    D3D12_CONSERVATIVE_RASTERIZATION_MODE_OFF, D3D12_CULL_MODE_BACK, D3D12_CULL_MODE_FRONT, D3D12_CULL_MODE_NONE,
    D3D12_DEPTH_STENCIL_DESC, D3D12_DEPTH_STENCILOP_DESC, D3D12_DEPTH_WRITE_MASK_ALL, D3D12_DEPTH_WRITE_MASK_ZERO,
    D3D12_FILL_MODE_SOLID, D3D12_LOGIC_OP_NOOP, D3D12_PRIMITIVE_TOPOLOGY_TYPE, D3D12_PRIMITIVE_TOPOLOGY_TYPE_LINE,
    D3D12_PRIMITIVE_TOPOLOGY_TYPE_POINT, D3D12_PRIMITIVE_TOPOLOGY_TYPE_TRIANGLE, D3D12_RASTERIZER_DESC,
    D3D12_RENDER_TARGET_BLEND_DESC, D3D12_STENCIL_OP, D3D12_STENCIL_OP_DECR, D3D12_STENCIL_OP_DECR_SAT,
    D3D12_STENCIL_OP_INCR, D3D12_STENCIL_OP_INCR_SAT, D3D12_STENCIL_OP_INVERT, D3D12_STENCIL_OP_KEEP,
    D3D12_STENCIL_OP_REPLACE, D3D12_STENCIL_OP_ZERO,
};
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_FORMAT, DXGI_FORMAT_A8_UNORM, DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_FORMAT_B8G8R8A8_UNORM_SRGB,
    DXGI_FORMAT_D24_UNORM_S8_UINT, DXGI_FORMAT_D32_FLOAT, DXGI_FORMAT_D32_FLOAT_S8X24_UINT, DXGI_FORMAT_R8_SINT,
    DXGI_FORMAT_R8_UINT, DXGI_FORMAT_R8_UNORM, DXGI_FORMAT_R8G8_SINT, DXGI_FORMAT_R8G8_UINT, DXGI_FORMAT_R8G8_UNORM,
    DXGI_FORMAT_R8G8B8A8_SINT, DXGI_FORMAT_R8G8B8A8_UINT, DXGI_FORMAT_R8G8B8A8_UNORM, DXGI_FORMAT_R8G8B8A8_UNORM_SRGB,
    DXGI_FORMAT_R10G10B10A2_UNORM, DXGI_FORMAT_R16_FLOAT, DXGI_FORMAT_R16_SINT, DXGI_FORMAT_R16_UINT,
    DXGI_FORMAT_R16G16_SINT, DXGI_FORMAT_R16G16_UINT, DXGI_FORMAT_R16G16B16A16_FLOAT, DXGI_FORMAT_R16G16B16A16_SINT,
    DXGI_FORMAT_R16G16B16A16_UINT, DXGI_FORMAT_R32_FLOAT, DXGI_FORMAT_R32_SINT, DXGI_FORMAT_R32_UINT,
    DXGI_FORMAT_R32G32_FLOAT, DXGI_FORMAT_R32G32_SINT, DXGI_FORMAT_R32G32_UINT, DXGI_FORMAT_R32G32B32_FLOAT,
    DXGI_FORMAT_R32G32B32_SINT, DXGI_FORMAT_R32G32B32_UINT, DXGI_FORMAT_R32G32B32A32_FLOAT,
    DXGI_FORMAT_R32G32B32A32_SINT, DXGI_FORMAT_R32G32B32A32_UINT,
};

#[must_use]
pub(crate) fn texture_format(format: TextureFormat) -> DXGI_FORMAT {
    match format {
        TextureFormat::R8Unorm => DXGI_FORMAT_R8_UNORM,
        TextureFormat::Rg8Unorm => DXGI_FORMAT_R8G8_UNORM,
        TextureFormat::A8Unorm => DXGI_FORMAT_A8_UNORM,
        TextureFormat::R16Float => DXGI_FORMAT_R16_FLOAT,
        TextureFormat::R32Float => DXGI_FORMAT_R32_FLOAT,
        TextureFormat::Rgba8Unorm => DXGI_FORMAT_R8G8B8A8_UNORM,
        TextureFormat::Rgba8UnormSrgb => DXGI_FORMAT_R8G8B8A8_UNORM_SRGB,
        TextureFormat::Bgra8Unorm => DXGI_FORMAT_B8G8R8A8_UNORM,
        TextureFormat::Bgra8UnormSrgb => DXGI_FORMAT_B8G8R8A8_UNORM_SRGB,
        TextureFormat::Rgb10a2Unorm => DXGI_FORMAT_R10G10B10A2_UNORM,
        TextureFormat::Rgba16Float => DXGI_FORMAT_R16G16B16A16_FLOAT,
        TextureFormat::Rgba32Float => DXGI_FORMAT_R32G32B32A32_FLOAT,
        TextureFormat::Depth32Float => DXGI_FORMAT_D32_FLOAT,
        TextureFormat::Depth24PlusStencil8 => DXGI_FORMAT_D24_UNORM_S8_UINT,
        TextureFormat::Depth32FloatStencil8 => DXGI_FORMAT_D32_FLOAT_S8X24_UINT,
    }
}

/// The format of `format`, `None` for a format the interface has no name
/// for.
#[must_use]
pub(crate) fn from_texture_format(format: DXGI_FORMAT) -> Option<TextureFormat> {
    TextureFormat::ALL.into_iter().chain([TextureFormat::A8Unorm]).find(|&known| texture_format(known) == format)
}

#[must_use]
pub(crate) fn is_depth(format: TextureFormat) -> bool {
    matches!(
        format,
        TextureFormat::Depth32Float | TextureFormat::Depth24PlusStencil8 | TextureFormat::Depth32FloatStencil8
    )
}

/// The format of a vertex input, `None` for one DXGI has no format of: the
/// three-component 8- and 16-bit ones.
#[must_use]
pub(crate) fn vertex_format(value: AttributeDataType) -> Option<DXGI_FORMAT> {
    use AttributeDataType as A;
    Some(match value {
        A::Byte => DXGI_FORMAT_R8_SINT,
        A::Byte2 => DXGI_FORMAT_R8G8_SINT,
        A::Byte4 => DXGI_FORMAT_R8G8B8A8_SINT,
        A::UByte => DXGI_FORMAT_R8_UINT,
        A::UByte2 => DXGI_FORMAT_R8G8_UINT,
        A::UByte4 => DXGI_FORMAT_R8G8B8A8_UINT,
        A::UByte4Normalized => DXGI_FORMAT_R8G8B8A8_UNORM,
        A::Short => DXGI_FORMAT_R16_SINT,
        A::Short2 => DXGI_FORMAT_R16G16_SINT,
        A::Short4 => DXGI_FORMAT_R16G16B16A16_SINT,
        A::UShort => DXGI_FORMAT_R16_UINT,
        A::UShort2 => DXGI_FORMAT_R16G16_UINT,
        A::UShort4 => DXGI_FORMAT_R16G16B16A16_UINT,
        A::Int => DXGI_FORMAT_R32_SINT,
        A::Int2 => DXGI_FORMAT_R32G32_SINT,
        A::Int3 => DXGI_FORMAT_R32G32B32_SINT,
        A::Int4 => DXGI_FORMAT_R32G32B32A32_SINT,
        A::UInt => DXGI_FORMAT_R32_UINT,
        A::UInt2 => DXGI_FORMAT_R32G32_UINT,
        A::UInt3 => DXGI_FORMAT_R32G32B32_UINT,
        A::UInt4 => DXGI_FORMAT_R32G32B32A32_UINT,
        A::Float => DXGI_FORMAT_R32_FLOAT,
        A::Float2 => DXGI_FORMAT_R32G32_FLOAT,
        A::Float3 => DXGI_FORMAT_R32G32B32_FLOAT,
        A::Float4 => DXGI_FORMAT_R32G32B32A32_FLOAT,
        _ => return None,
    })
}

/// The topology type of a pipeline of `value` and the topology its draws
/// set, `None` for the primitives D3D12 doesn't draw: fans and loops.
#[must_use]
pub(crate) fn topology(value: DrawModeType) -> Option<(D3D12_PRIMITIVE_TOPOLOGY_TYPE, D3D_PRIMITIVE_TOPOLOGY)> {
    Some(match value {
        DrawModeType::Points => (D3D12_PRIMITIVE_TOPOLOGY_TYPE_POINT, D3D_PRIMITIVE_TOPOLOGY_POINTLIST),
        DrawModeType::Lines => (D3D12_PRIMITIVE_TOPOLOGY_TYPE_LINE, D3D_PRIMITIVE_TOPOLOGY_LINELIST),
        DrawModeType::LineStrip => (D3D12_PRIMITIVE_TOPOLOGY_TYPE_LINE, D3D_PRIMITIVE_TOPOLOGY_LINESTRIP),
        DrawModeType::Triangles => (D3D12_PRIMITIVE_TOPOLOGY_TYPE_TRIANGLE, D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST),
        DrawModeType::TriangleStrip => (D3D12_PRIMITIVE_TOPOLOGY_TYPE_TRIANGLE, D3D_PRIMITIVE_TOPOLOGY_TRIANGLESTRIP),
        DrawModeType::LineLoop | DrawModeType::TriangleFan => return None,
    })
}

fn blend_op(value: ColorBlendEquationType) -> D3D12_BLEND_OP {
    match value {
        ColorBlendEquationType::Add => D3D12_BLEND_OP_ADD,
        ColorBlendEquationType::Subtract => D3D12_BLEND_OP_SUBTRACT,
        ColorBlendEquationType::ReverseSubtract => D3D12_BLEND_OP_REV_SUBTRACT,
    }
}

/// The factor of `value` for color, or with `alpha` for alpha, which D3D12
/// blends with the alpha factors only.
fn blend_factor(value: ColorBlendFactorType, alpha: bool) -> D3D12_BLEND {
    use ColorBlendFactorType as F;
    match value {
        F::Zero => D3D12_BLEND_ZERO,
        F::One => D3D12_BLEND_ONE,
        F::SrcColor if alpha => D3D12_BLEND_SRC_ALPHA,
        F::SrcColor => D3D12_BLEND_SRC_COLOR,
        F::OneMinusSrcColor if alpha => D3D12_BLEND_INV_SRC_ALPHA,
        F::OneMinusSrcColor => D3D12_BLEND_INV_SRC_COLOR,
        F::SrcAlpha => D3D12_BLEND_SRC_ALPHA,
        F::OneMinusSrcAlpha => D3D12_BLEND_INV_SRC_ALPHA,
        F::DstAlpha => D3D12_BLEND_DEST_ALPHA,
        F::OneMinusDstAlpha => D3D12_BLEND_INV_DEST_ALPHA,
        F::DstColor if alpha => D3D12_BLEND_DEST_ALPHA,
        F::DstColor => D3D12_BLEND_DEST_COLOR,
        F::OneMinusDstColor if alpha => D3D12_BLEND_INV_DEST_ALPHA,
        F::OneMinusDstColor => D3D12_BLEND_INV_DEST_COLOR,
        F::SrcAlphaSaturate => D3D12_BLEND_SRC_ALPHA_SAT,
        // `OMSetBlendFactor`'s constant. A color factor of its alpha
        // (`D3D12_BLEND_ALPHA_FACTOR`) needs a newer runtime than D3D12's
        // first, and the renderer blends with no constant.
        F::ConstantColor | F::ConstantAlpha => D3D12_BLEND_BLEND_FACTOR,
        F::OneMinusConstantColor | F::OneMinusConstantAlpha => D3D12_BLEND_INV_BLEND_FACTOR,
    }
}

/// The blend of a pipeline's one render target.
#[must_use]
pub(crate) fn blend(state: &FixedState) -> D3D12_BLEND_DESC {
    let mask = state.color_mask;
    let write_mask = (u8::from(mask.r) * D3D12_COLOR_WRITE_ENABLE_RED.0 as u8)
        | (u8::from(mask.g) * D3D12_COLOR_WRITE_ENABLE_GREEN.0 as u8)
        | (u8::from(mask.b) * D3D12_COLOR_WRITE_ENABLE_BLUE.0 as u8)
        | (u8::from(mask.a) * D3D12_COLOR_WRITE_ENABLE_ALPHA.0 as u8);
    let function = state.blend;
    let op = blend_op(function.equation());
    let target = D3D12_RENDER_TARGET_BLEND_DESC {
        // `Replace` writes the fragment's color as it is.
        BlendEnable: (function != BlendFunction::Replace).into(),
        LogicOpEnable: FALSE,
        SrcBlend: blend_factor(function.src_factor(), false),
        DestBlend: blend_factor(function.dst_factor(), false),
        BlendOp: op,
        SrcBlendAlpha: blend_factor(function.src_factor(), true),
        DestBlendAlpha: blend_factor(function.dst_factor(), true),
        BlendOpAlpha: op,
        LogicOp: D3D12_LOGIC_OP_NOOP,
        RenderTargetWriteMask: write_mask,
    };
    D3D12_BLEND_DESC { AlphaToCoverageEnable: FALSE, IndependentBlendEnable: FALSE, RenderTarget: [target; 8] }
}

/// The rasterizer state: GL's winding, as clip space is y-up as GL's.
#[must_use]
pub(crate) fn rasterizer(state: &FixedState) -> D3D12_RASTERIZER_DESC {
    let cull = &state.cull;
    let cull_mode = match (cull.enabled, cull.side) {
        (false, _) => D3D12_CULL_MODE_NONE,
        (true, CullFaceSideType::Front) => D3D12_CULL_MODE_FRONT,
        // D3D12 can't cull both faces, which draws nothing but depth and
        // stencil of points and lines; the renderer doesn't ask for it.
        (true, CullFaceSideType::Back | CullFaceSideType::FrontAndBack) => D3D12_CULL_MODE_BACK,
    };
    D3D12_RASTERIZER_DESC {
        FillMode: D3D12_FILL_MODE_SOLID,
        CullMode: cull_mode,
        FrontCounterClockwise: (cull.winding == CullFaceWindingType::CounterClockwise).into(),
        DepthBias: 0,
        DepthBiasClamp: 0.0,
        SlopeScaledDepthBias: 0.0,
        DepthClipEnable: true.into(),
        MultisampleEnable: FALSE,
        AntialiasedLineEnable: FALSE,
        ForcedSampleCount: 0,
        ConservativeRaster: D3D12_CONSERVATIVE_RASTERIZATION_MODE_OFF,
    }
}

fn depth_func(value: DepthFunctionType) -> D3D12_COMPARISON_FUNC {
    match value {
        DepthFunctionType::Never => D3D12_COMPARISON_FUNC_NEVER,
        DepthFunctionType::Less => D3D12_COMPARISON_FUNC_LESS,
        DepthFunctionType::Equal => D3D12_COMPARISON_FUNC_EQUAL,
        DepthFunctionType::LessEqual => D3D12_COMPARISON_FUNC_LESS_EQUAL,
        DepthFunctionType::Greater => D3D12_COMPARISON_FUNC_GREATER,
        DepthFunctionType::NotEqual => D3D12_COMPARISON_FUNC_NOT_EQUAL,
        DepthFunctionType::GreaterEqual => D3D12_COMPARISON_FUNC_GREATER_EQUAL,
        DepthFunctionType::Always => D3D12_COMPARISON_FUNC_ALWAYS,
    }
}

fn stencil_func(value: StencilFunctionType) -> D3D12_COMPARISON_FUNC {
    match value {
        StencilFunctionType::Never => D3D12_COMPARISON_FUNC_NEVER,
        StencilFunctionType::Less => D3D12_COMPARISON_FUNC_LESS,
        StencilFunctionType::Equal => D3D12_COMPARISON_FUNC_EQUAL,
        StencilFunctionType::LessEqual => D3D12_COMPARISON_FUNC_LESS_EQUAL,
        StencilFunctionType::Greater => D3D12_COMPARISON_FUNC_GREATER,
        StencilFunctionType::NotEqual => D3D12_COMPARISON_FUNC_NOT_EQUAL,
        StencilFunctionType::GreaterEqual => D3D12_COMPARISON_FUNC_GREATER_EQUAL,
        StencilFunctionType::Always => D3D12_COMPARISON_FUNC_ALWAYS,
    }
}

fn stencil_op(value: StencilOpType) -> D3D12_STENCIL_OP {
    match value {
        StencilOpType::Zero => D3D12_STENCIL_OP_ZERO,
        StencilOpType::Keep => D3D12_STENCIL_OP_KEEP,
        StencilOpType::Replace => D3D12_STENCIL_OP_REPLACE,
        StencilOpType::Increment => D3D12_STENCIL_OP_INCR_SAT,
        StencilOpType::Decrement => D3D12_STENCIL_OP_DECR_SAT,
        StencilOpType::Invert => D3D12_STENCIL_OP_INVERT,
        StencilOpType::IncrementWrap => D3D12_STENCIL_OP_INCR,
        StencilOpType::DecrementWrap => D3D12_STENCIL_OP_DECR,
    }
}

/// The depth and stencil state, of a pass with a depth-stencil attachment.
#[must_use]
pub(crate) fn depth_stencil(state: &FixedState, has_depth_stencil: bool) -> D3D12_DEPTH_STENCIL_DESC {
    let stencil = &state.stencil;
    let face = D3D12_DEPTH_STENCILOP_DESC {
        StencilFailOp: stencil_op(stencil.fail),
        StencilDepthFailOp: stencil_op(stencil.depth_fail),
        StencilPassOp: stencil_op(stencil.pass),
        StencilFunc: stencil_func(stencil.test.func()),
    };
    D3D12_DEPTH_STENCIL_DESC {
        DepthEnable: has_depth_stencil.into(),
        DepthWriteMask: if state.depth.write { D3D12_DEPTH_WRITE_MASK_ALL } else { D3D12_DEPTH_WRITE_MASK_ZERO },
        DepthFunc: depth_func(state.depth.compare),
        StencilEnable: (has_depth_stencil && !stencil.is_disabled()).into(),
        StencilReadMask: stencil.test.masked().unwrap_or(0xFF) as u8,
        StencilWriteMask: stencil.write_mask as u8,
        FrontFace: face,
        BackFace: face,
    }
}
