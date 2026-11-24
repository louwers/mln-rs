// naga writes a program's GLSL as HLSL for shader model 6.0, with its
// resources in the registers of the root signature:
//
// - Descriptor set N is register space N, and a buffer's binding is its
//   register: `b` for a uniform buffer, a root CBV, and `t` for a storage
//   buffer or a vertex buffer read as one, a root SRV.
// - The image set's textures are `t0` to `t2` of its space, one descriptor
//   table. The global set's ground depth is `t2` of space 0, a table of its
//   own, which only the terrain variants of the programs that terrain hides
//   read.
// - naga reads samplers out of a heap of them (`nagaSamplerHeap`, a table
//   from `s0` of `SAMPLER_HEAP_SPACE`) at the indices of a buffer of `uint`s
//   (`nagaGroup3SamplerIndexArray`, at `SAMPLER_INDEX_REGISTER` of the image
//   set's space): a sampler's register is its index in that buffer. The
//   comparison sampler heap naga also declares is unused.
// - The push constants (`ubo_index`) are root constants at `b0` of
//   `ROOT_CONSTANT_SPACE`.
// - naga's special constants are root constants at
//   `SPECIAL_CONSTANTS_REGISTER` of that space. naga adds the first, a draw's
//   base vertex, to `SV_VertexID`, which D3D doesn't include it in, so that
//   `gl_VertexIndex` includes it as Vulkan's does.

use std::sync::LazyLock;

use mln_glsl::layout::{DescriptorSetType, GLOBAL_GROUND_DEPTH_BINDING, IMAGE_SAMPLER_START, IMAGE_TEXTURE_COUNT};
use mln_glsl::{Module, Stage};
use naga::ResourceBinding;
use naga::back::hlsl;

/// The space of the root constants, after the descriptor sets'.
pub(crate) const ROOT_CONSTANT_SPACE: u8 = DescriptorSetType::COUNT as u8;
/// The space of the sampler heap's table.
pub(crate) const SAMPLER_HEAP_SPACE: u8 = ROOT_CONSTANT_SPACE + 1;
/// The space naga declares its comparison sampler heap in, which no shader
/// reads.
const COMPARISON_SAMPLER_HEAP_SPACE: u8 = SAMPLER_HEAP_SPACE + 1;
/// The register in `ROOT_CONSTANT_SPACE` of naga's special constants.
pub(crate) const SPECIAL_CONSTANTS_REGISTER: u32 = 1;
/// The values of naga's special constants: the first vertex, the first
/// instance and one more.
pub(crate) const SPECIAL_CONSTANTS_VALUES: u32 = 3;
/// The register in the image set's space of the sampler index buffer: the one
/// after its textures.
pub(crate) const SAMPLER_INDEX_REGISTER: u32 = IMAGE_TEXTURE_COUNT as u32;

fn target(space: u8, register: u32) -> hlsl::BindTarget {
    hlsl::BindTarget { space, register, ..hlsl::BindTarget::default() }
}

/// The options every program is written with.
fn options() -> hlsl::Options {
    let mut binding_map = hlsl::BindingMap::new();
    for type_ in DescriptorSetType::ALL {
        let set = type_.set_index();
        let shape = type_.buffer_shape();
        for binding in 0..shape.len() as u32 {
            binding_map.insert(ResourceBinding { group: set, binding }, target(set as u8, binding));
        }
    }
    // The frame's ground depth, after the global set's buffers.
    let global = DescriptorSetType::Global.set_index();
    let ground_depth = ResourceBinding { group: global, binding: GLOBAL_GROUND_DEPTH_BINDING };
    binding_map.insert(ground_depth, target(global as u8, GLOBAL_GROUND_DEPTH_BINDING));
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
        special_constants_binding: Some(target(ROOT_CONSTANT_SPACE, SPECIAL_CONSTANTS_REGISTER)),
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

static OPTIONS: LazyLock<hlsl::Options> = LazyLock::new(options);

/// A stage written as HLSL.
pub(crate) struct Hlsl {
    pub(crate) source: String,
    /// The name naga gave the entry point.
    pub(crate) entry_point: String,
}

/// Writes a parsed stage as HLSL.
pub(crate) fn write(stage: Stage, module: &Module) -> Result<Hlsl, String> {
    let pipeline_options = hlsl::PipelineOptions { entry_point: Some((stage.naga(), "main".to_owned())) };
    let mut source = String::new();
    let reflection = hlsl::Writer::new(&mut source, &OPTIONS, &pipeline_options)
        .write(&module.module, &module.info, None)
        .map_err(|error| format!("{error:?}"))?;
    let entry_point = match reflection.entry_point_names.into_iter().next() {
        Some(Ok(name)) => name,
        Some(Err(error)) => return Err(format!("{error:?}")),
        None => return Err("no entry point".to_owned()),
    };
    Ok(Hlsl { source, entry_point })
}
