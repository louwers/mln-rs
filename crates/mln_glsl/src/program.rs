use std::collections::BTreeSet;
use std::fmt::Write as _;

use mln_gfx::gfx_types::AttributeDataType;
use mln_gfx::gpu::GpuError;
use mln_gfx::shaders::program::{
    Buffer, BufferKind, DRAW_PROJECTION_BUFFER, ENGINE_BUFFERS, PROJECTION_BUFFER, Program,
};
use mln_gfx::shaders::source::glsl::{self, Glsl};
use mln_gfx::shaders::source::identifiers;

use crate::front_end::Stage;
use crate::layout::{
    DescriptorSetType, drawable_uniform_binding, drawable_vertex_binding, global_uniform_binding,
    image_sampler_binding, image_texture_binding, layer_storage_binding, layer_uniform_binding,
};
use crate::sources::UBO_INDEX_DECLARATION;

/// The engine's buffers that a stage may read without declaring them,
/// besides `constant`: the projection blocks. Every stage's prelude
/// declares the view's block.
const ENGINE_BINDINGS: [Buffer; 2] = [PROJECTION_BUFFER, DRAW_PROJECTION_BUFFER];

/// The source of `stage` of `glsl`, `program`'s GLSL, for naga's GLSL front
/// end, with `defines` (`#define` lines): the version, the defines, the
/// structs of the engine's blocks, the stage's prelude, the declarations of
/// the engine's buffers that the stage or the program's prelude names
/// (`constant`, the draw's index, and the projection blocks), the structs
/// of the program's blocks, each a storage buffer's element padded to its
/// stride, the declarations of the program's buffers, textures and samplers
/// that the stage or the program's prelude names, and the program's own
/// prelude and stage with its properties written out ([`glsl::expand`]).
/// Fails as [`glsl::expand`] does.
pub fn compose(program: &Program, glsl: &Glsl, stage: Stage, defines: &str) -> Result<String, GpuError> {
    let source = glsl::expand(program, glsl, stage.into())?;
    let (engine, own) = (glsl::blocks(&ENGINE_BUFFERS), glsl::blocks(program.buffers));
    let named: BTreeSet<&str> = identifiers(glsl.prelude).chain(identifiers(&source)).collect();
    let mut reserved = String::new();
    if named.contains("constant") {
        reserved.push_str(UBO_INDEX_DECLARATION);
        reserved.push('\n');
    }
    reserved += &declarations(&ENGINE_BINDINGS, &named)?;
    let declared = declarations(program.buffers, &named)?;
    let textures = texture_declarations(program, &named);
    Ok(crate::front_end::compose(
        stage,
        defines,
        &engine,
        &[&reserved, &own, &declared, &textures, glsl.prelude].concat(),
        &source,
    ))
}

/// The declarations of those of `program`'s textures and their samplers
/// that `named` has the names of, by their names, at their slots' bindings
/// in the image set: a `texture2D` and a `sampler`, which a stage combines
/// where it samples the texture.
fn texture_declarations(program: &Program, named: &BTreeSet<&str>) -> String {
    let set = DescriptorSetType::DrawableImage.set_index();
    let mut declarations = String::new();
    for (slot, texture) in program.textures.iter().enumerate() {
        let sampler = texture.sampler();
        if named.contains(texture.name) {
            let binding = image_texture_binding(slot);
            _ = writeln!(declarations, "layout(set = {set}, binding = {binding}) uniform texture2D {};", texture.name);
        }
        if named.contains(sampler.as_str()) {
            let binding = image_sampler_binding(slot);
            _ = writeln!(declarations, "layout(set = {set}, binding = {binding}) uniform sampler {sampler};");
        }
    }
    declarations
}

/// The declarations of those of `buffers` that `named` has the names of.
fn declarations(buffers: &[Buffer], named: &BTreeSet<&str>) -> Result<String, GpuError> {
    let mut declarations = String::new();
    for buffer in buffers.iter().filter(|buffer| named.contains(buffer.name)) {
        declarations.push_str(&declaration(buffer)?);
        declarations.push('\n');
    }
    Ok(declarations)
}

/// The declaration of `buffer`, at the set and binding that its id and kind
/// have in [`layout`](crate::layout), by its name: a block as the member of
/// a `std140` uniform block, the elements of a storage buffer as the array
/// of a `std140` one, and its vertices' 32-bit words as the array of a
/// `std430` one. The blocks have no instance names, so a stage reads the
/// buffer by its name alone. Fails for an id that the layout has no binding
/// of its kind for.
pub(crate) fn declaration(buffer: &Buffer) -> Result<String, GpuError> {
    let (set, binding) = match buffer.kind {
        BufferKind::Uniform(_) => match global_uniform_binding(buffer.id) {
            Some(binding) => (DescriptorSetType::Global, Some(binding)),
            None => (DescriptorSetType::Layer, layer_uniform_binding(buffer.id)),
        },
        BufferKind::DrawUniform(_) => (DescriptorSetType::DrawableUniform, drawable_uniform_binding(buffer.id)),
        BufferKind::Storage { .. } => (DescriptorSetType::Layer, layer_storage_binding(buffer.id)),
        BufferKind::Vertices { .. } => (DescriptorSetType::DrawableUniform, drawable_vertex_binding(buffer.id)),
    };
    let binding = binding.ok_or_else(|| GpuError::Invalid(format!("no binding for buffer {}", buffer.name)))?;
    let (set, name) = (set.set_index(), buffer.name);
    Ok(match buffer.kind {
        BufferKind::Uniform(block) | BufferKind::DrawUniform(block) => format!(
            "layout(std140, set = {set}, binding = {binding}) uniform {0}Block {{\n    {0} {name};\n}};",
            block.name
        ),
        BufferKind::Storage { block, .. } => format!(
            "layout(std140, set = {set}, binding = {binding}) readonly buffer {0}Vector {{\n    {0} {name}[];\n}};",
            block.name
        ),
        BufferKind::Vertices { format } => format!(
            "layout(std430, set = {set}, binding = {binding}) readonly buffer {name}Words {{\n    {} {name}[];\n}};",
            word(format)
        ),
    })
}

/// The 32-bit words that a stage reads vertices of `format` as.
fn word(format: AttributeDataType) -> &'static str {
    use AttributeDataType::{Float, Float2, Float3, Float4, Int, Int2, Int3, Int4};
    match format {
        Float | Float2 | Float3 | Float4 => "float",
        Int | Int2 | Int3 | Int4 => "int",
        _ => "uint",
    }
}
