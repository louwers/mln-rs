use std::collections::BTreeSet;

use mln_gfx::gfx_types::AttributeDataType;
use mln_gfx::gpu::GpuError;
use mln_gfx::shaders::program::{Binding, BufferKind, ENGINE_BUFFERS, Program, ProgramBuffer};
use mln_gfx::shaders::source::glsl::{self, Glsl};
use mln_gfx::shaders::source::identifiers;

use crate::front_end::Stage;
use crate::layout::{
    DescriptorSetType, drawable_uniform_binding, drawable_vertex_binding, global_uniform_binding,
    image_sampler_binding, image_texture_binding, layer_storage_binding, layer_uniform_binding,
};
use crate::sources::DRAW_INDEX_DECLARATION;

/// The source of `stage` of `glsl`, `program`'s GLSL, for naga's GLSL front
/// end, with `defines` (`#define` lines): the version, the defines, the
/// structs of the engine's blocks, the stage's prelude, the structs of the
/// program's blocks, the declarations of what the stage or the program's
/// prelude names of what the stage may read ([`Program::bindings`]), each
/// at the set and binding that [`layout`](crate::layout) gives its id and
/// kind, and the program's own prelude and stage with its properties
/// written out ([`glsl::expand`]). Fails as [`glsl::expand`] does, and for
/// a buffer whose id the layout has no binding of its kind for.
pub fn compose(program: &Program, glsl: &Glsl, stage: Stage, defines: &str) -> Result<String, GpuError> {
    let source = glsl::expand(program, glsl, stage.into())?;
    let (engine, own) = (glsl::blocks(&ENGINE_BUFFERS), glsl::blocks(program.buffers));
    let named: BTreeSet<&str> = identifiers(glsl.prelude).chain(identifiers(&source)).collect();
    let mut declarations = String::new();
    let image_set = DescriptorSetType::DrawableImage.set_index();
    for binding in program.bindings(stage.into()) {
        let name = binding.name();
        if !named.contains(&*name) {
            continue;
        }
        let declaration = match binding {
            Binding::DrawIndex => DRAW_INDEX_DECLARATION.to_owned(),
            Binding::Buffer(buffer) => declaration(buffer)?,
            Binding::Texture(slot, _) => {
                format!(
                    "layout(set = {image_set}, binding = {}) uniform texture2D {name};",
                    image_texture_binding(slot)
                )
            }
            Binding::Sampler(slot, _) => {
                format!("layout(set = {image_set}, binding = {}) uniform sampler {name};", image_sampler_binding(slot))
            }
        };
        declarations.push_str(&declaration);
        declarations.push('\n');
    }
    Ok(crate::front_end::compose(stage, defines, &engine, &[&own, &declarations, glsl.prelude].concat(), &source))
}

/// The declaration of `buffer`, at the set and binding that its id and kind
/// have in [`layout`](crate::layout), by its name: a block as the member of
/// a `std140` uniform block, the elements of a storage buffer as the array
/// of a `std140` one, and its vertices' 32-bit words as the array of a
/// `std430` one. The blocks have no instance names, so a stage reads the
/// buffer by its name alone. Fails for an id that the layout has no binding
/// of its kind for.
pub(crate) fn declaration(buffer: &ProgramBuffer) -> Result<String, GpuError> {
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
    use AttributeDataType::{Float, Float2, Float4, Int, Int2, Int4};
    match format {
        Float | Float2 | Float4 => "float",
        Int | Int2 | Int4 => "int",
        _ => "uint",
    }
}
