//! A program's stages as HLSL for shader model 6.0, with its resources in the
//! registers of D3D12's root signature:
//!
//! - Descriptor set N of the Vulkan layout ([`crate::layout`]) is register
//!   space N, and a buffer's binding is its register: `b` for a uniform
//!   buffer, a root CBV, and `t` for a storage buffer, a root SRV.
//! - The image set's textures are `t0` to `t2` of its space, one descriptor
//!   table.
//! - naga reads samplers out of a heap of them (`nagaSamplerHeap`, a table
//!   from `s0` of [`SAMPLER_HEAP_SPACE`]) at the indices of a buffer of
//!   `uint`s (`nagaGroup3SamplerIndexArray`, at [`SAMPLER_INDEX_REGISTER`] of
//!   the image set's space): a sampler's register is its index in that
//!   buffer. The comparison sampler heap naga also declares is unused.
//! - The immediates (the draw's index) are root constants at `b0` of
//!   [`ROOT_CONSTANT_SPACE`].

use std::sync::LazyLock;

use naga::ResourceBinding;
use naga::back::hlsl;

use crate::layout::{DescriptorSetType, IMAGE_SAMPLER_START, IMAGE_TEXTURE_COUNT};
use crate::{Program, Stage, Target};

/// The space of the root constants, after the descriptor sets'.
pub const ROOT_CONSTANT_SPACE: u8 = DescriptorSetType::COUNT as u8;
/// The space of the sampler heap's table.
pub const SAMPLER_HEAP_SPACE: u8 = ROOT_CONSTANT_SPACE + 1;
/// The space naga declares its comparison sampler heap in, which no shader
/// reads.
const COMPARISON_SAMPLER_HEAP_SPACE: u8 = SAMPLER_HEAP_SPACE + 1;
/// The register in the image set's space of the sampler index buffer: the one
/// after its textures.
pub const SAMPLER_INDEX_REGISTER: u32 = IMAGE_TEXTURE_COUNT as u32;

fn target(space: u8, register: u32) -> hlsl::BindTarget {
    hlsl::BindTarget { space, register, ..hlsl::BindTarget::default() }
}

/// The options of every module with the Vulkan layout's bindings.
fn options() -> hlsl::Options {
    let mut binding_map = hlsl::BindingMap::new();
    for type_ in DescriptorSetType::ALL {
        let set = type_.set_index();
        let shape = type_.buffer_shape();
        for binding in 0..(shape.storage + shape.uniform) as u32 {
            binding_map.insert(ResourceBinding { group: set, binding }, target(set as u8, binding));
        }
    }
    let image_set = DescriptorSetType::DrawableImage.set_index();
    for index in 0..IMAGE_TEXTURE_COUNT as u32 {
        binding_map.insert(ResourceBinding { group: image_set, binding: index }, target(image_set as u8, index));
        let sampler = ResourceBinding { group: image_set, binding: IMAGE_SAMPLER_START as u32 + index };
        binding_map.insert(sampler, target(image_set as u8, index));
    }
    hlsl::Options {
        shader_model: hlsl::ShaderModel::V6_0,
        binding_map,
        fake_missing_bindings: false,
        immediates_target: Some(target(ROOT_CONSTANT_SPACE, 0)),
        sampler_heap_target: hlsl::SamplerHeapBindTargets {
            standard_samplers: target(SAMPLER_HEAP_SPACE, 0),
            comparison_samplers: target(COMPARISON_SAMPLER_HEAP_SPACE, 0),
        },
        sampler_buffer_binding_map: [(
            hlsl::SamplerIndexBufferKey { group: image_set },
            target(image_set as u8, SAMPLER_INDEX_REGISTER),
        )]
        .into(),
        // No checks added around loops and indexes, and no workgroups.
        zero_initialize_workgroup_memory: false,
        restrict_indexing: false,
        force_loop_bounding: false,
        ..hlsl::Options::default()
    }
}

/// The options of every module with the Vulkan layout's bindings: D3D12's
/// programs of `mln_shaders` and of its own GLSL alike.
pub static OPTIONS: LazyLock<hlsl::Options> = LazyLock::new(options);

/// A stage written as HLSL.
#[derive(Clone, Debug)]
pub struct Hlsl {
    pub source: String,
    /// The name naga gave the entry point.
    pub entry_point: String,
}

/// Writes the entry point of `stage` and `name` of `module` as HLSL.
pub fn write(
    module: &naga::Module,
    info: &naga::valid::ModuleInfo,
    stage: naga::ShaderStage,
    name: &str,
) -> Result<Hlsl, String> {
    let pipeline_options = hlsl::PipelineOptions { entry_point: Some((stage, name.to_owned())) };
    let mut source = String::new();
    let reflection = hlsl::Writer::new(&mut source, &OPTIONS, &pipeline_options)
        .write(module, info, None)
        .map_err(|error| error.to_string())?;
    let entry_point = match reflection.entry_point_names.into_iter().next() {
        Some(Ok(name)) => name,
        Some(Err(error)) => return Err(error.to_string()),
        None => return Err("no entry point".to_owned()),
    };
    Ok(Hlsl { source, entry_point })
}

impl Program {
    /// `stage` of a D3D12 program as HLSL, written once. Its clip space is
    /// the program's.
    pub fn hlsl(&self, stage: Stage) -> Result<&Hlsl, String> {
        debug_assert_eq!(self.target, Target::D3d12, "the bindings are the Vulkan layout's");
        self.written.hlsl[stage as usize]
            .get_or_init(|| {
                write(&self.module, &self.info, stage.naga(), stage.entry_point())
                    .map_err(|error| format!("{} {stage:?}: {error}", self.name))
            })
            .as_ref()
            .map_err(Clone::clone)
    }
}
