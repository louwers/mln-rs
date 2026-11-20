//! The root signature of every pipeline, for the registers that
//! `shaders::hlsl` puts the shaders' resources in: `ubo_index` as a root
//! constant, each buffer as a root CBV or SRV, the textures as one table,
//! and the sampler heap as another, with the buffer of the indices in it of
//! a draw's samplers as a root SRV. Binding a draw sets GPU addresses only.

use mln_gfx::gpu::GpuError;
use mln_glsl::layout::DescriptorSetType;
use windows::Win32::Graphics::Direct3D::ID3DBlob;
use windows::Win32::Graphics::Direct3D12::{
    D3D_ROOT_SIGNATURE_VERSION_1, D3D12_DESCRIPTOR_RANGE, D3D12_DESCRIPTOR_RANGE_TYPE_SAMPLER,
    D3D12_DESCRIPTOR_RANGE_TYPE_SRV, D3D12_ROOT_CONSTANTS, D3D12_ROOT_DESCRIPTOR, D3D12_ROOT_DESCRIPTOR_TABLE,
    D3D12_ROOT_PARAMETER, D3D12_ROOT_PARAMETER_0, D3D12_ROOT_PARAMETER_TYPE_32BIT_CONSTANTS,
    D3D12_ROOT_PARAMETER_TYPE_CBV, D3D12_ROOT_PARAMETER_TYPE_DESCRIPTOR_TABLE, D3D12_ROOT_PARAMETER_TYPE_SRV,
    D3D12_ROOT_SIGNATURE_DESC, D3D12_ROOT_SIGNATURE_FLAG_ALLOW_INPUT_ASSEMBLER_INPUT_LAYOUT,
    D3D12_SHADER_VISIBILITY_ALL, D3D12_SHADER_VISIBILITY_PIXEL, D3D12SerializeRootSignature, ID3D12Device,
    ID3D12RootSignature,
};

use crate::heaps::{SAMPLER_HEAP_SIZE, TABLE_SIZE};
use crate::shaders::hlsl::{ROOT_CONSTANT_SPACE, SAMPLER_HEAP_SPACE, SAMPLER_INDEX_REGISTER};

/// The parameter of the root constants.
pub(crate) const ROOT_CONSTANTS: u32 = 0;

/// The sets of buffers, in the order of their parameters after the root
/// constants.
const BUFFER_SETS: [DescriptorSetType; 3] =
    [DescriptorSetType::Global, DescriptorSetType::Layer, DescriptorSetType::DrawableUniform];

const fn buffer_count() -> u32 {
    let mut count = 0;
    let mut index = 0;
    while index < BUFFER_SETS.len() {
        let shape = BUFFER_SETS[index].buffer_shape();
        count += shape.len() as u32;
        index += 1;
    }
    count
}

/// The parameter of the image set's texture table.
pub(crate) const TEXTURE_TABLE: u32 = ROOT_CONSTANTS + 1 + buffer_count();
/// The parameter of the buffer of a draw's sampler indices.
pub(crate) const SAMPLER_INDICES: u32 = TEXTURE_TABLE + 1;
/// The parameter of the sampler heap's table.
pub(crate) const SAMPLER_TABLE: u32 = SAMPLER_INDICES + 1;

/// How a buffer binds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum View {
    /// A uniform buffer: a root CBV.
    Constant,
    /// A storage buffer: a root SRV.
    Shader,
}

/// The parameter of the buffer at `binding` of `set`, and how it binds;
/// `None` for a binding that isn't a buffer's.
#[must_use]
pub(crate) fn buffer_parameter(set: u32, binding: u32) -> Option<(u32, View)> {
    let mut parameter = ROOT_CONSTANTS + 1;
    for type_ in BUFFER_SETS {
        let shape = type_.buffer_shape();
        if type_.set_index() == set {
            let binding = binding as usize;
            return match binding {
                _ if binding >= shape.len() => None,
                _ if shape.is_storage(binding) => Some((parameter + binding as u32, View::Shader)),
                _ => Some((parameter + binding as u32, View::Constant)),
            };
        }
        parameter += shape.len() as u32;
    }
    None
}

/// Serializes and makes the root signature.
pub(crate) fn create(device: &ID3D12Device) -> Result<ID3D12RootSignature, GpuError> {
    let all = D3D12_SHADER_VISIBILITY_ALL;
    let pixel = D3D12_SHADER_VISIBILITY_PIXEL;
    let mut parameters = vec![D3D12_ROOT_PARAMETER {
        ParameterType: D3D12_ROOT_PARAMETER_TYPE_32BIT_CONSTANTS,
        Anonymous: D3D12_ROOT_PARAMETER_0 {
            Constants: D3D12_ROOT_CONSTANTS {
                ShaderRegister: 0,
                RegisterSpace: u32::from(ROOT_CONSTANT_SPACE),
                Num32BitValues: 1,
            },
        },
        ShaderVisibility: all,
    }];
    let descriptor = |kind, register, space, visibility| D3D12_ROOT_PARAMETER {
        ParameterType: kind,
        Anonymous: D3D12_ROOT_PARAMETER_0 {
            Descriptor: D3D12_ROOT_DESCRIPTOR { ShaderRegister: register, RegisterSpace: space },
        },
        ShaderVisibility: visibility,
    };
    for type_ in BUFFER_SETS {
        let shape = type_.buffer_shape();
        for binding in 0..shape.len() as u32 {
            let kind = if shape.is_storage(binding as usize) {
                D3D12_ROOT_PARAMETER_TYPE_SRV
            } else {
                D3D12_ROOT_PARAMETER_TYPE_CBV
            };
            parameters.push(descriptor(kind, binding, type_.set_index(), all));
        }
    }
    let image_space = DescriptorSetType::DrawableImage.set_index();
    let textures = D3D12_DESCRIPTOR_RANGE {
        RangeType: D3D12_DESCRIPTOR_RANGE_TYPE_SRV,
        NumDescriptors: TABLE_SIZE,
        BaseShaderRegister: 0,
        RegisterSpace: image_space,
        OffsetInDescriptorsFromTableStart: 0,
    };
    let samplers = D3D12_DESCRIPTOR_RANGE {
        RangeType: D3D12_DESCRIPTOR_RANGE_TYPE_SAMPLER,
        NumDescriptors: SAMPLER_HEAP_SIZE,
        BaseShaderRegister: 0,
        RegisterSpace: u32::from(SAMPLER_HEAP_SPACE),
        OffsetInDescriptorsFromTableStart: 0,
    };
    let table = |range: &D3D12_DESCRIPTOR_RANGE| D3D12_ROOT_PARAMETER {
        ParameterType: D3D12_ROOT_PARAMETER_TYPE_DESCRIPTOR_TABLE,
        Anonymous: D3D12_ROOT_PARAMETER_0 {
            DescriptorTable: D3D12_ROOT_DESCRIPTOR_TABLE { NumDescriptorRanges: 1, pDescriptorRanges: range },
        },
        ShaderVisibility: pixel,
    };
    debug_assert_eq!(parameters.len() as u32, TEXTURE_TABLE);
    parameters.push(table(&textures));
    parameters.push(descriptor(D3D12_ROOT_PARAMETER_TYPE_SRV, SAMPLER_INDEX_REGISTER, image_space, pixel));
    parameters.push(table(&samplers));
    let desc = D3D12_ROOT_SIGNATURE_DESC {
        NumParameters: parameters.len() as u32,
        pParameters: parameters.as_ptr(),
        NumStaticSamplers: 0,
        pStaticSamplers: std::ptr::null(),
        Flags: D3D12_ROOT_SIGNATURE_FLAG_ALLOW_INPUT_ASSEMBLER_INPUT_LAYOUT,
    };
    let mut blob: Option<ID3DBlob> = None;
    let mut errors: Option<ID3DBlob> = None;
    // SAFETY: the description and its ranges outlive the call, and the
    // blob holds what it serialized.
    unsafe {
        if let Err(error) = D3D12SerializeRootSignature(
            &raw const desc,
            D3D_ROOT_SIGNATURE_VERSION_1,
            &raw mut blob,
            Some(&raw mut errors),
        ) {
            let message = errors.map_or_else(String::new, |errors| {
                let bytes = std::slice::from_raw_parts(errors.GetBufferPointer().cast::<u8>(), errors.GetBufferSize());
                String::from_utf8_lossy(bytes).into_owned()
            });
            return Err(GpuError::Invalid(format!("serializing the root signature: {error} {message}")));
        }
        let blob = blob.ok_or_else(|| GpuError::Invalid("no serialized root signature".to_owned()))?;
        let bytes = std::slice::from_raw_parts(blob.GetBufferPointer().cast::<u8>(), blob.GetBufferSize());
        device.CreateRootSignature(0, bytes).map_err(|error| GpuError::Invalid(format!("CreateRootSignature: {error}")))
    }
}
