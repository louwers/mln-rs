// A program's MSL declarations, written from the program: the structs of the
// blocks it reads, its vertex stage's input, and the parameters of each
// stage's main function for the buffers and textures the stage reads, within
// `#if`s for what only some variants read.
//
// - A block is a struct of its name, `alignas(16)`, which reads a `Vec3` as
//   a `packed_float3`. A storage buffer's block is padded to the buffer's
//   stride.
// - A buffer is `device const`, a reference to its block, or a pointer to
//   the elements of a storage buffer or the words of a vertex buffer, by the
//   buffer's name at the buffer index of its id. A stage that reads a
//   storage buffer reads the draw's index into it, `uboIndex`.
// - A texture is a `texture2d<float>` of its name at the texture index of
//   its slot, with `access::read` where its texels are read, and otherwise
//   `access::sample` and the sampler `<name>_sampler` at the sampler index
//   of its slot.
// - The vertex stage's input is `VertexStage`, of each attribute by its name
//   at `[[attribute(<location>)]]`. A property's attribute is in the variants
//   that read the property per vertex, as the pair of values that its
//   zoom-dependent values are: a `float2` of a property of one value.

use std::fmt::Write as _;

use mln_gfx::gfx_types::AttributeDataType;
use mln_gfx::gpu::GpuError;
use mln_gfx::shaders::bindings::GLOBAL_INDEX_ID;
use mln_gfx::shaders::block::{Block, MemberType, UniformBlock};
use mln_gfx::shaders::layer_ubo::{GlobalPaintParamsUBO, ProjectionUBO};
use mln_gfx::shaders::program::{
    Attribute, BufferKind, PAINT_PARAMS, Program, ProgramBuffer, ProgramTexture, Stages, TERRAIN_DEFINE, Variants,
    VertexWord,
};

/// The placeholder that a vertex function's source names after its own
/// parameters, where the parameters of what the vertex stage reads go.
pub(crate) const VERTEX_BINDINGS: &str = "MLN_VERTEX_BINDINGS";
/// The fragment function's counterpart of [`VERTEX_BINDINGS`].
pub(crate) const FRAGMENT_BINDINGS: &str = "MLN_FRAGMENT_BINDINGS";

/// A stage of a program.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Stage {
    Vertex,
    Fragment,
}

impl Stage {
    fn reads(self, stages: Stages) -> bool {
        match self {
            Stage::Vertex => stages.vertex(),
            Stage::Fragment => stages.fragment(),
        }
    }
}

/// The structs of the blocks that `program` reads, the engine's paint
/// parameters and projection first, which the prelude reads too, each once.
/// Fails where a block's struct would differ between two of its buffers.
pub(crate) fn blocks(program: &Program) -> Result<String, GpuError> {
    let mut written: Vec<(&str, usize)> = Vec::new();
    let mut out = String::new();
    let engine = [(GlobalPaintParamsUBO::BLOCK, None), (ProjectionUBO::BLOCK, None)];
    let buffers = program.buffers.iter().chain(program.projection_buffer());
    let blocks = buffers.filter_map(|buffer| Some((buffer.block()?, buffer.stride())));
    for (block, stride) in engine.into_iter().chain(blocks) {
        let size = stride.unwrap_or(block.size);
        if let Some(&(_, earlier)) = written.iter().find(|(name, _)| *name == block.name) {
            if earlier != size {
                let name = program.name;
                return Err(GpuError::Invalid(format!(
                    "{name} reads {} with strides {earlier} and {size}",
                    block.name
                )));
            }
            continue;
        }
        written.push((block.name, size));
        declare_struct(&mut out, block, size);
    }
    Ok(out)
}

/// Writes the struct of `block`, padded to `size` bytes with `float4`s: a
/// block's size is a multiple of 16.
fn declare_struct(out: &mut String, block: &Block, size: usize) {
    let name = block.name;
    let _ = writeln!(out, "struct alignas(16) {name} {{");
    for member in block.members {
        let name = member.name;
        let _ = match member.ty {
            MemberType::Float => writeln!(out, "    float {name};"),
            MemberType::Int => writeln!(out, "    int {name};"),
            MemberType::Uint => writeln!(out, "    uint {name};"),
            MemberType::Vec2 => writeln!(out, "    float2 {name};"),
            MemberType::Vec3 => writeln!(out, "    packed_float3 {name};"),
            MemberType::Vec4 => writeln!(out, "    float4 {name};"),
            MemberType::Mat4 => writeln!(out, "    float4x4 {name};"),
            MemberType::Vec4Array(count) => writeln!(out, "    float4 {name}[{count}];"),
        };
    }
    for pad in 0..size.saturating_sub(block.size) / 16 {
        let _ = writeln!(out, "    float4 union_pad{pad};");
    }
    let _ = writeln!(out, "}};\nstatic_assert(sizeof({name}) == {size}, \"wrong size\");");
}

/// The vertex stage's input, `VertexStage`, of `program`'s attributes; empty
/// for a program without attributes. Fails for an attribute of a format that
/// no MSL type reads.
pub(crate) fn vertex_input(program: &Program) -> Result<String, GpuError> {
    let mut out = String::new();
    if program.locations().next().is_none() {
        return Ok(out);
    }
    out.push_str("struct VertexStage {\n");
    for (location, _, attribute) in program.locations() {
        let ty = input_type(attribute, is_property(program, attribute))?;
        let line = format!("    {ty} {} [[attribute({location})]];\n", attribute.name);
        conditional(&mut out, attribute_condition(program, attribute).as_deref(), &line);
    }
    out.push_str("};\n");
    Ok(out)
}

/// The parameters of `stage`'s main function for what it reads of the
/// engine's and `program`'s buffers and textures, each on a line of its own
/// after a comma, to follow the function's own parameters.
pub(crate) fn parameters(program: &Program, stage: Stage) -> String {
    let paint_params = program.paint_params.map(|stages| ProgramBuffer { stages, ..PAINT_PARAMS });
    let mut buffers: Vec<ProgramBuffer> = paint_params
        .into_iter()
        .chain(program.projection_buffer().copied())
        .chain(program.buffers.iter().copied())
        .filter(|buffer| stage.reads(buffer.stages))
        .collect();
    buffers.sort_by_key(|buffer| buffer.id);
    let mut out = String::new();
    let mut index = buffers.iter().any(|buffer| matches!(buffer.kind, BufferKind::Storage { .. }));
    for buffer in &buffers {
        if index && buffer.id > GLOBAL_INDEX_ID {
            let _ = writeln!(out, ", device const uint32_t& uboIndex [[buffer({GLOBAL_INDEX_ID})]]");
            index = false;
        }
        let (name, id) = (buffer.name, buffer.id);
        let line = match buffer.kind {
            BufferKind::Uniform(block) | BufferKind::DrawUniform(block) => {
                format!(", device const {}& {name} [[buffer({id})]]\n", block.name)
            }
            BufferKind::Storage { block, .. } => format!(", device const {}* {name} [[buffer({id})]]\n", block.name),
            BufferKind::Vertices(word) => {
                let word = match word {
                    VertexWord::Uint => "uint",
                    VertexWord::Float => "float",
                };
                format!(", device const {word}* {name} [[buffer({id})]]\n")
            }
        };
        conditional(&mut out, condition(program, buffer.variants).as_deref(), &line);
    }
    for (slot, texture) in program.textures.iter().enumerate() {
        if texture.is_unused() || !stage.reads(texture.stages) {
            continue;
        }
        conditional(&mut out, condition(program, texture.variants).as_deref(), &texture_parameters(texture, slot));
    }
    out
}

/// The parameters of `texture`, at `slot`: the texture, and its sampler
/// where a stage samples it.
fn texture_parameters(texture: &ProgramTexture, slot: usize) -> String {
    let name = texture.name;
    if texture.sampled() {
        format!(
            ", texture2d<float, access::sample> {name} [[texture({slot})]]\n, sampler {name}_sampler [[sampler({slot})]]\n"
        )
    } else {
        format!(", texture2d<float, access::read> {name} [[texture({slot})]]\n")
    }
}

fn is_property(program: &Program, attribute: &Attribute) -> bool {
    program.properties.iter().any(|property| property.name == attribute.name)
}

/// The `#if` that holds what only `variants` of `program` read, if they
/// aren't all of its variants.
fn condition(program: &Program, variants: Variants) -> Option<String> {
    match variants {
        Variants::All => None,
        Variants::Standing => Some(format!("#if defined({TERRAIN_DEFINE})")),
        Variants::PerVertex if program.properties.is_empty() => None,
        Variants::PerVertex => {
            let uniforms: Vec<String> = program
                .properties
                .iter()
                .map(|property| format!("!defined(HAS_UNIFORM_u_{})", property.name))
                .collect();
            Some(format!("#if {}", uniforms.join(" || ")))
        }
    }
}

/// The `#if`s that hold `attribute`: its variants', and for a property's,
/// that the variant reads the property per vertex.
fn attribute_condition(program: &Program, attribute: &Attribute) -> Option<String> {
    let property = is_property(program, attribute).then(|| format!("#if !defined(HAS_UNIFORM_u_{})", attribute.name));
    match (condition(program, attribute.variants), property) {
        (None, None) => None,
        (Some(condition), None) | (None, Some(condition)) => Some(condition),
        (Some(variants), Some(property)) => Some(format!("{variants}\n{property}")),
    }
}

/// Writes `text`, within the `#if`s of `condition`, a line each, if there
/// are any.
fn conditional(out: &mut String, condition: Option<&str>, text: &str) {
    let Some(condition) = condition else {
        out.push_str(text);
        return;
    };
    let _ = writeln!(out, "{condition}");
    out.push_str(text);
    for _ in condition.lines() {
        out.push_str("#endif\n");
    }
}

/// The MSL type that reads `attribute`, a property's if `property`.
fn input_type(attribute: &Attribute, property: bool) -> Result<&'static str, GpuError> {
    use AttributeDataType::{
        Byte, Byte2, Byte3, Byte4, Float, Float2, Float3, Float4, Int, Int2, Int3, Int4, Invalid, Short, Short2,
        Short3, Short4, UByte, UByte2, UByte3, UByte4, UByte4Normalized, UInt, UInt2, UInt3, UInt4, UShort, UShort2,
        UShort3, UShort4, UShort8,
    };
    Ok(match attribute.format {
        Float if property => "float2",
        Float => "float",
        Float2 => "float2",
        Float3 => "float3",
        Float4 | UByte4Normalized => "float4",
        Byte => "char",
        Byte2 => "char2",
        Byte3 => "char3",
        Byte4 => "char4",
        UByte => "uchar",
        UByte2 => "uchar2",
        UByte3 => "uchar3",
        UByte4 => "uchar4",
        Short => "short",
        Short2 => "short2",
        Short3 => "short3",
        Short4 => "short4",
        UShort => "ushort",
        UShort2 => "ushort2",
        UShort3 => "ushort3",
        UShort4 => "ushort4",
        Int => "int",
        Int2 => "int2",
        Int3 => "int3",
        Int4 => "int4",
        UInt => "uint",
        UInt2 => "uint2",
        UInt3 => "uint3",
        UInt4 => "uint4",
        UShort8 | Invalid => {
            return Err(GpuError::Unsupported(format!(
                "no MSL type reads attribute {} of {:?}",
                attribute.name, attribute.format
            )));
        }
    })
}

#[cfg(test)]
mod tests {
    use mln_engine::layers::circle::programs::CIRCLE;
    use mln_engine::layers::fill::programs::FILL_OUTLINE_INSTANCED;
    use mln_engine::layers::fill_extrusion::programs::FILL_EXTRUSION_PULLED;

    use super::*;

    #[test]
    fn a_union_stride_pads_the_block_to_it() {
        let blocks = blocks(&FILL_OUTLINE_INSTANCED).unwrap();
        assert!(blocks.contains("    float pad2;\n    float4 union_pad0;\n    float4 union_pad1;\n"), "{blocks}");
        assert!(blocks.contains("static_assert(sizeof(FillOutlineDrawableUBO) == 112, \"wrong size\");\n"));
    }

    #[test]
    fn a_stage_takes_what_it_reads() {
        assert_eq!(
            parameters(&CIRCLE, Stage::Vertex),
            ", device const GlobalPaintParamsUBO& paintParams [[buffer(0)]]\n\
             , device const uint32_t& uboIndex [[buffer(1)]]\n\
             , device const CircleDrawableUBO* drawableVector [[buffer(2)]]\n\
             , device const ProjectionUBO* projectionVector [[buffer(4)]]\n\
             , device const CircleEvaluatedPropsUBO& props [[buffer(5)]]\n\
             #if defined(TERRAIN)\n\
             , texture2d<float, access::read> ground_depth [[texture(1)]]\n\
             #endif\n"
        );
        assert_eq!(
            parameters(&CIRCLE, Stage::Fragment),
            ", device const CircleEvaluatedPropsUBO& props [[buffer(5)]]\n"
        );
    }

    #[test]
    fn a_variant_reads_a_property_per_vertex_where_it_has_no_uniform() {
        let input = vertex_input(&CIRCLE).unwrap();
        assert!(input.contains("    short2 pos [[attribute(0)]];\n"), "{input}");
        assert!(input.contains("#if !defined(HAS_UNIFORM_u_color)\n    float4 color [[attribute(1)]];\n#endif\n"));
        assert!(input.contains("#if defined(TERRAIN)\n    float ground_height [[attribute(8)]];\n#endif\n"));
    }

    #[test]
    fn a_pulled_program_reads_its_vertices_as_words() {
        let vertex = parameters(&FILL_EXTRUSION_PULLED, Stage::Vertex);
        assert!(vertex.contains(", device const uint* wallVertices [[buffer(7)]]\n"), "{vertex}");
        let paint = "#if !defined(HAS_UNIFORM_u_base) || !defined(HAS_UNIFORM_u_color) || !defined(HAS_UNIFORM_u_height)\n\
                     , device const float* wallPaint [[buffer(8)]]\n#endif\n";
        assert!(vertex.contains(paint), "{vertex}");
        assert_eq!(vertex_input(&FILL_EXTRUSION_PULLED).unwrap(), "");
    }
}
