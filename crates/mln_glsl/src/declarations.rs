// A program's GLSL declarations, written from the program: what each stage
// reads of the engine's and the program's buffers, textures and attributes,
// at the sets and bindings of `layout` and at the attributes' locations, in
// the variants that read each.
//
// - A uniform block declares its members and has the buffer's name, but for
//   a block whose struct the stage declares too (the engine's projection, in
//   a vertex stage), which is the one member, of the buffer's name, of a
//   block without one.
// - A storage buffer is a block of the buffer's name, of an array of its
//   block's struct padded to the buffer's stride, `elements`; a vertex
//   buffer one of its `words`.
// - A texture is `<name>_texture`, and the sampler of one that a stage
//   samples `<name>_sampler`, combined where it is sampled.
// - A stage that reads a storage buffer reads the draw's index into it from
//   the push constants, `constant.ubo_index`.
// - An attribute is `in_<name>`. A property's attribute is in the variants
//   that read the property per vertex, as the pair of values that its
//   zoom-dependent values are: a `vec2` of a property of one value.

use std::fmt::Write;

use mln_gfx::gfx_types::AttributeDataType;
use mln_gfx::shaders::block::{Block, MemberType, UniformBlock};
use mln_gfx::shaders::layer_ubo::ProjectionUBO;
use mln_gfx::shaders::program::{
    Attribute, BufferKind, PAINT_PARAMS, Program, ProgramBuffer, Stages, TERRAIN_DEFINE, Variants, VertexWord,
};

use crate::front_end::Stage;
use crate::layout::{
    DescriptorSetType, drawable_uniform_binding, drawable_vertex_binding, global_uniform_binding,
    image_sampler_binding, image_texture_binding, layer_storage_binding, layer_uniform_binding,
};

/// What `stage` reads of the engine's declarations, which the stage's
/// prelude reads: the paint parameters, where the program reads them in the
/// stage, and the struct of a projection block, in a vertex stage.
pub(crate) fn engine(program: &Program, stage: Stage) -> Result<String, String> {
    let mut out = String::new();
    if program.paint_params.is_some_and(|stages| reads(stages, stage)) {
        declare_buffer(&mut out, &PAINT_PARAMS, &[])?;
    }
    if stage == Stage::Vertex {
        declare_struct(&mut out, ProjectionUBO::BLOCK, 0);
    }
    Ok(out)
}

/// The declarations of what `stage` reads of `program`: its attributes, its
/// buffers with its projection and the push constants, and its textures.
/// Fails for an attribute of a format that no GLSL type reads, and for a
/// buffer whose id has no binding of its kind in the layout.
pub(crate) fn program(program: &Program, stage: Stage) -> Result<String, String> {
    let mut out = String::new();
    // The structs the stage declares.
    let mut structs: Vec<&str> = Vec::new();
    if stage == Stage::Vertex {
        structs.push(ProjectionUBO::BLOCK.name);
        for (location, _, attribute) in program.locations() {
            let ty = input_type(attribute, is_property(program, attribute))?;
            let condition = attribute_condition(program, attribute);
            conditional(&mut out, condition.as_deref(), |out| {
                let _ = writeln!(out, "layout(location = {location}) in {ty} in_{};", attribute.name);
                Ok(())
            })?;
        }
    }
    let mut buffers: Vec<&ProgramBuffer> = program
        .buffers
        .iter()
        .chain(program.projection_buffer())
        .filter(|buffer| reads(buffer.stages, stage))
        .collect();
    buffers.sort_by_key(|buffer| buffer.id);
    if buffers.iter().any(|buffer| matches!(buffer.kind, BufferKind::Storage { .. })) {
        out.push_str("layout(push_constant) uniform Constants {\n    int ubo_index;\n} constant;\n");
    }
    for buffer in buffers {
        let condition = condition(program, buffer.variants);
        conditional(&mut out, condition.as_deref(), |out| declare_buffer(out, buffer, &structs))?;
        if let BufferKind::Storage { block, .. } = buffer.kind {
            structs.push(block.name);
        }
    }
    let image_set = DescriptorSetType::DrawableImage.set_index();
    for (slot, texture) in program.textures.iter().enumerate() {
        if texture.is_unused() || !reads(texture.stages, stage) {
            continue;
        }
        let condition = condition(program, texture.variants);
        conditional(&mut out, condition.as_deref(), |out| {
            let (name, binding) = (texture.name, image_texture_binding(slot));
            let _ = writeln!(out, "layout(set = {image_set}, binding = {binding}) uniform texture2D {name}_texture;");
            if texture.sampled() {
                let binding = image_sampler_binding(slot);
                let _ = writeln!(out, "layout(set = {image_set}, binding = {binding}) uniform sampler {name}_sampler;");
            }
            Ok(())
        })?;
    }
    Ok(out)
}

/// Whether `stages` has `stage`.
fn reads(stages: Stages, stage: Stage) -> bool {
    match stage {
        Stage::Vertex => stages.vertex(),
        Stage::Fragment => stages.fragment(),
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
        Variants::Standing => Some(format!("#ifdef {TERRAIN_DEFINE}")),
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

/// The `#if` that holds `attribute`: its variants', and for a property's,
/// that the variant reads the property per vertex.
fn attribute_condition(program: &Program, attribute: &Attribute) -> Option<String> {
    let property = is_property(program, attribute).then(|| format!("!defined(HAS_UNIFORM_u_{})", attribute.name));
    match (condition(program, attribute.variants), property) {
        (None, None) => None,
        (None, Some(property)) => Some(format!("#if {property}")),
        (Some(variants), None) => Some(variants),
        (Some(variants), Some(property)) => Some(format!("{variants}\n#if {property}")),
    }
}

/// Writes what `write` writes, within `condition` if there is one.
fn conditional(
    out: &mut String,
    condition: Option<&str>,
    write: impl FnOnce(&mut String) -> Result<(), String>,
) -> Result<(), String> {
    let Some(condition) = condition else {
        return write(out);
    };
    out.push_str(condition);
    out.push('\n');
    write(out)?;
    for _ in condition.lines() {
        out.push_str("#endif\n");
    }
    Ok(())
}

/// The GLSL type that reads `attribute`, a property's if `property`.
fn input_type(attribute: &Attribute, property: bool) -> Result<&'static str, String> {
    use AttributeDataType::{
        Byte, Byte2, Byte3, Byte4, Float, Float2, Float3, Float4, Int, Int2, Int3, Int4, Invalid, Short, Short2,
        Short3, Short4, UByte, UByte2, UByte3, UByte4, UByte4Normalized, UInt, UInt2, UInt3, UInt4, UShort, UShort2,
        UShort3, UShort4, UShort8,
    };
    Ok(match attribute.format {
        Float if property => "vec2",
        Float => "float",
        Float2 => "vec2",
        Float3 => "vec3",
        Float4 | UByte4Normalized => "vec4",
        Byte | Short | Int => "int",
        Byte2 | Short2 | Int2 => "ivec2",
        Byte3 | Short3 | Int3 => "ivec3",
        Byte4 | Short4 | Int4 => "ivec4",
        UByte | UShort | UInt => "uint",
        UByte2 | UShort2 | UInt2 => "uvec2",
        UByte3 | UShort3 | UInt3 => "uvec3",
        UByte4 | UShort4 | UInt4 => "uvec4",
        UShort8 | Invalid => {
            return Err(format!("no GLSL type reads attribute {} of {:?}", attribute.name, attribute.format));
        }
    })
}

/// Writes the declaration of `buffer`, and the struct of its block where
/// `structs` doesn't have it. Fails for an id that the layout has no binding
/// of its kind for, which the [`ProgramBuffer`] constructors give none.
fn declare_buffer(out: &mut String, buffer: &ProgramBuffer, structs: &[&str]) -> Result<(), String> {
    let (set, binding) = match buffer.kind {
        BufferKind::Uniform(_) => match global_uniform_binding(buffer.id) {
            Some(binding) => (DescriptorSetType::Global, Some(binding)),
            None => (DescriptorSetType::Layer, layer_uniform_binding(buffer.id)),
        },
        BufferKind::DrawUniform(_) => (DescriptorSetType::DrawableUniform, drawable_uniform_binding(buffer.id)),
        BufferKind::Storage { .. } => (DescriptorSetType::Layer, layer_storage_binding(buffer.id)),
        BufferKind::Vertices(_) => (DescriptorSetType::DrawableUniform, drawable_vertex_binding(buffer.id)),
    };
    let binding = binding.ok_or_else(|| format!("the layout has no binding for buffer {}", buffer.name))?;
    let (set, name) = (set.set_index(), buffer.name);
    match buffer.kind {
        BufferKind::Uniform(block) | BufferKind::DrawUniform(block) if structs.contains(&block.name) => {
            let block = block.name;
            let _ = writeln!(
                out,
                "layout(set = {set}, binding = {binding}) uniform {block}Block {{\n    {block} {name};\n}};"
            );
        }
        BufferKind::Uniform(block) | BufferKind::DrawUniform(block) => {
            let _ = writeln!(out, "layout(set = {set}, binding = {binding}) uniform {} {{", block.name);
            members(out, block, 0);
            let _ = writeln!(out, "}} {name};");
        }
        BufferKind::Storage { block, stride } => {
            if !structs.contains(&block.name) {
                declare_struct(out, block, stride);
            }
            let block = block.name;
            let _ = writeln!(
                out,
                "layout(std140, set = {set}, binding = {binding}) readonly buffer {block}Vector {{\n    {block} elements[];\n}} {name};"
            );
        }
        BufferKind::Vertices(word) => {
            let word = match word {
                VertexWord::Uint => "uint",
                VertexWord::Float => "float",
            };
            let _ = writeln!(
                out,
                "layout(std430, set = {set}, binding = {binding}) readonly buffer {name}Words {{\n    {word} words[];\n}} {name};"
            );
        }
    }
    Ok(())
}

/// Writes the struct of `block`, padded to `stride` bytes.
fn declare_struct(out: &mut String, block: &Block, stride: usize) {
    let _ = writeln!(out, "struct {} {{", block.name);
    members(out, block, stride);
    out.push_str("};\n");
}

/// Writes the members of `block`, padded to `stride` bytes with `vec4`s: a
/// block's size is a multiple of 16.
fn members(out: &mut String, block: &Block, stride: usize) {
    for member in block.members {
        let name = member.name;
        let _ = match member.ty {
            MemberType::Float => writeln!(out, "    float {name};"),
            MemberType::Int => writeln!(out, "    int {name};"),
            MemberType::Uint => writeln!(out, "    uint {name};"),
            MemberType::Vec2 => writeln!(out, "    vec2 {name};"),
            MemberType::Vec3 => writeln!(out, "    vec3 {name};"),
            MemberType::Vec4 => writeln!(out, "    vec4 {name};"),
            MemberType::Mat4 => writeln!(out, "    mat4 {name};"),
            MemberType::Vec4Array(count) => writeln!(out, "    vec4 {name}[{count}];"),
        };
    }
    for pad in 0..stride.saturating_sub(block.size) / 16 {
        let _ = writeln!(out, "    vec4 union_pad{pad};");
    }
}
