//! The mips of a mipmapped texture, drawn a level at a time from the level
//! before: D3D12 has no command that makes them.

use std::collections::hash_map::Entry;

use mln_gfx::color_mode::ColorMode;
use mln_gfx::cull_face_mode::CullFaceMode;
use mln_gfx::gfx_types::DrawModeType;
use mln_gfx::gpu::{DepthState, FixedState, GpuError, StencilState};
use mln_glsl::Stage;
use mln_util::hash::HashMap;
use windows::Win32::Graphics::Direct3D12::{
    D3D12_COMPARISON_FUNC_NEVER, D3D12_DESCRIPTOR_RANGE, D3D12_DESCRIPTOR_RANGE_TYPE_SRV,
    D3D12_FILTER_MIN_MAG_MIP_LINEAR, D3D12_GRAPHICS_PIPELINE_STATE_DESC, D3D12_PRIMITIVE_TOPOLOGY_TYPE_TRIANGLE,
    D3D12_ROOT_DESCRIPTOR_TABLE, D3D12_ROOT_PARAMETER, D3D12_ROOT_PARAMETER_0,
    D3D12_ROOT_PARAMETER_TYPE_DESCRIPTOR_TABLE, D3D12_ROOT_SIGNATURE_DESC, D3D12_ROOT_SIGNATURE_FLAG_NONE,
    D3D12_SHADER_BYTECODE, D3D12_SHADER_VISIBILITY_PIXEL, D3D12_STATIC_BORDER_COLOR_TRANSPARENT_BLACK,
    D3D12_STATIC_SAMPLER_DESC, D3D12_TEXTURE_ADDRESS_MODE_CLAMP, ID3D12Device, ID3D12PipelineState,
    ID3D12RootSignature,
};
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT, DXGI_FORMAT_UNKNOWN, DXGI_SAMPLE_DESC};

use crate::shaders::ShaderCompiler;
use crate::{conv, root_signature};

/// A triangle over the target, and each texel of it sampled at its center
/// in the level before, between the four texels it covers there, which the
/// linear sampler averages.
const SHADER: &str = r"
Texture2D<float4> level : register(t0);
SamplerState linear_sampler : register(s0);

struct Varyings {
    float4 position : SV_Position;
    float2 uv : TEXCOORD0;
};

Varyings vs_main(uint index : SV_VertexID) {
    Varyings varyings;
    varyings.uv = float2((index << 1) & 2, index & 2);
    varyings.position = float4(varyings.uv.x * 2.0 - 1.0, 1.0 - varyings.uv.y * 2.0, 0.0, 1.0);
    return varyings;
}

float4 ps_main(Varyings varyings) : SV_Target {
    return level.SampleLevel(linear_sampler, varyings.uv, 0.0);
}
";

/// What draws mips, made for the first texture whose mips are made: a root
/// signature of the view of the level before, in a table, and a linear
/// sampler, and a pipeline state for each format.
pub(crate) struct Mipmapper {
    pub(crate) root_signature: ID3D12RootSignature,
    vertex: Vec<u8>,
    pixel: Vec<u8>,
    pipelines: HashMap<i32, ID3D12PipelineState>,
}

impl Mipmapper {
    pub(crate) fn new(device: &ID3D12Device, compiler: &ShaderCompiler) -> Result<Mipmapper, GpuError> {
        let vertex = compiler.compile_hlsl(Stage::Vertex, SHADER, "vs_main")?;
        let pixel = compiler.compile_hlsl(Stage::Fragment, SHADER, "ps_main")?;
        let range = D3D12_DESCRIPTOR_RANGE {
            RangeType: D3D12_DESCRIPTOR_RANGE_TYPE_SRV,
            NumDescriptors: 1,
            BaseShaderRegister: 0,
            RegisterSpace: 0,
            OffsetInDescriptorsFromTableStart: 0,
        };
        let parameter = D3D12_ROOT_PARAMETER {
            ParameterType: D3D12_ROOT_PARAMETER_TYPE_DESCRIPTOR_TABLE,
            Anonymous: D3D12_ROOT_PARAMETER_0 {
                DescriptorTable: D3D12_ROOT_DESCRIPTOR_TABLE {
                    NumDescriptorRanges: 1,
                    pDescriptorRanges: &raw const range,
                },
            },
            ShaderVisibility: D3D12_SHADER_VISIBILITY_PIXEL,
        };
        let sampler = D3D12_STATIC_SAMPLER_DESC {
            Filter: D3D12_FILTER_MIN_MAG_MIP_LINEAR,
            AddressU: D3D12_TEXTURE_ADDRESS_MODE_CLAMP,
            AddressV: D3D12_TEXTURE_ADDRESS_MODE_CLAMP,
            AddressW: D3D12_TEXTURE_ADDRESS_MODE_CLAMP,
            MipLODBias: 0.0,
            MaxAnisotropy: 1,
            ComparisonFunc: D3D12_COMPARISON_FUNC_NEVER,
            BorderColor: D3D12_STATIC_BORDER_COLOR_TRANSPARENT_BLACK,
            MinLOD: 0.0,
            MaxLOD: 0.0,
            ShaderRegister: 0,
            RegisterSpace: 0,
            ShaderVisibility: D3D12_SHADER_VISIBILITY_PIXEL,
        };
        let desc = D3D12_ROOT_SIGNATURE_DESC {
            NumParameters: 1,
            pParameters: &raw const parameter,
            NumStaticSamplers: 1,
            pStaticSamplers: &raw const sampler,
            Flags: D3D12_ROOT_SIGNATURE_FLAG_NONE,
        };
        let root_signature = root_signature::serialize(device, &desc)?;
        Ok(Mipmapper { root_signature, vertex, pixel, pipelines: HashMap::default() })
    }

    /// The pipeline state that draws a level of `format`, made on first use.
    pub(crate) fn pipeline(
        &mut self,
        device: &ID3D12Device,
        format: DXGI_FORMAT,
    ) -> Result<&ID3D12PipelineState, GpuError> {
        let pipeline = match self.pipelines.entry(format.0) {
            Entry::Occupied(entry) => entry.into_mut(),
            Entry::Vacant(entry) => {
                let unblended = ColorMode::unblended();
                let state = FixedState {
                    primitive: DrawModeType::Triangles,
                    cull: CullFaceMode::disabled(),
                    blend: unblended.blend_function,
                    color_mask: unblended.mask,
                    depth: DepthState::DISABLED,
                    stencil: StencilState::DISABLED,
                };
                let bytecode = |code: &[u8]| D3D12_SHADER_BYTECODE {
                    pShaderBytecode: code.as_ptr().cast(),
                    BytecodeLength: code.len(),
                };
                let mut rtv_formats = [DXGI_FORMAT_UNKNOWN; 8];
                rtv_formats[0] = format;
                let desc = D3D12_GRAPHICS_PIPELINE_STATE_DESC {
                    // SAFETY: a copy of the pointer without a reference, which
                    // the description's ManuallyDrop never releases.
                    pRootSignature: unsafe { std::mem::transmute_copy(&self.root_signature) },
                    VS: bytecode(&self.vertex),
                    PS: bytecode(&self.pixel),
                    BlendState: conv::blend(&state),
                    SampleMask: u32::MAX,
                    RasterizerState: conv::rasterizer(&state),
                    DepthStencilState: conv::depth_stencil(&state, false),
                    PrimitiveTopologyType: D3D12_PRIMITIVE_TOPOLOGY_TYPE_TRIANGLE,
                    NumRenderTargets: 1,
                    RTVFormats: rtv_formats,
                    SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
                    ..Default::default()
                };
                // SAFETY: the description and its bytecode outlive the call.
                let pipeline = unsafe { device.CreateGraphicsPipelineState(&raw const desc) }
                    .map_err(|error| GpuError::Invalid(format!("mipmaps: CreateGraphicsPipelineState: {error}")))?;
                entry.insert(pipeline)
            }
        };
        Ok(pipeline)
    }
}
