// A program's GL ES declarations, written from the program: the std140
// uniform blocks of the engine's and the program's buffers that each stage
// reads, its textures' samplers and its attributes, in the variants that
// read each, at the precisions of `precision`, which the program's GL ES
// declarations (`GlslEsDeclarations`) give.
//
// - A block is anonymous, as GL JS's are, so that its members are the
//   program's uniforms, `u_<member>`, but where the program's GL ES
//   declarations name them otherwise, and
//   a pad `<buffer>_<pad>`. The engine's projection, which every vertex
//   stage declares for its prelude, is the instance `projection` instead:
//   the programs' own blocks have its members' names (`matrix`).
// - A storage buffer's block is the element of it that a draw binds.
// - The blocks that both stages read come first, in both stages: SwiftShader
//   misreads a fragment stage whose blocks shared with the vertex stage
//   don't come first, in the vertex stage's order
//   (maplibre/maplibre-native#4625).
// - A texture is the sampler `u_<name>`, which samples the texture unit of
//   its slot.
// - An attribute is `a_<name>`, at its location, of floats, which GL
//   converts its format to. A property's attribute is in the variants that
//   read the property per vertex, as the pair of values that its
//   zoom-dependent values are: a `vec2` of a property of one value.

use std::fmt::Write as _;

use mln_gfx::shaders::block::{Block, Member, MemberType, UniformBlock};
use mln_gfx::shaders::layer_ubo::ProjectionUBO;
use mln_gfx::shaders::program::{Attribute, PAINT_PARAMS, Program, ProgramBuffer, Stages, TERRAIN_DEFINE, Variants};
use mln_gfx::shaders::source::GlslEsDeclarations;

use super::precision;
use crate::enums::attribute_format;

/// A stage of a program.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Stage {
    Vertex,
    Fragment,
}

/// The engine's declarations that `stage`'s prelude reads: the projection
/// block, in a vertex stage.
pub(crate) fn engine(stage: Stage) -> String {
    let mut out = String::new();
    if stage == Stage::Vertex {
        let block = ProjectionUBO::BLOCK;
        let _ = writeln!(out, "layout(std140) uniform {} {{", block.name);
        for member in block.members {
            declare_member(&mut out, &GlslEsDeclarations::DEFAULT, block, member, member.name);
        }
        out.push_str("} projection;\n");
    }
    out
}

/// The declarations of what `stage` of `program` reads: its attributes, its
/// blocks, with the paint parameters', and its textures' samplers, as the
/// program's GL ES `declarations` qualify and name them. Fails for an
/// attribute of a format that GL can't bind and for a vertex buffer, which
/// GL ES binds as attributes.
pub(crate) fn program(program: &Program, declarations: &GlslEsDeclarations, stage: Stage) -> Result<String, String> {
    let mut out = String::new();
    if stage == Stage::Vertex {
        for (location, _, attribute) in program.locations() {
            let ty = input_type(program, attribute)?;
            let precision = precision::attribute(declarations, attribute).qualifier();
            conditional(&mut out, attribute_condition(program, attribute).as_deref(), |out| {
                let _ = writeln!(out, "layout(location = {location}) in {precision} {ty} a_{};", attribute.name);
            });
        }
    }
    let shared = blocks(program).filter(|&(_, stages)| stages == Stages::Both);
    let own = blocks(program).filter(|&(_, stages)| stages != Stages::Both && reads(stages, stage));
    for (buffer, _) in shared.chain(own) {
        let block = block(buffer)?;
        conditional(&mut out, condition(program, buffer.variants).as_deref(), |out| {
            let _ = writeln!(out, "layout(std140) uniform {} {{", block.name);
            for member in block.members {
                declare_member(out, declarations, block, member, &member_name(declarations, buffer, block, member));
            }
            out.push_str("};\n");
        });
    }
    for texture in program.textures {
        if texture.is_unused() || !reads(texture.stages, stage) {
            continue;
        }
        let precision = precision::sampler(declarations, texture)
            .map_or(String::new(), |precision| format!("{} ", precision.qualifier()));
        conditional(&mut out, condition(program, texture.variants).as_deref(), |out| {
            let _ = writeln!(out, "uniform {precision}sampler2D u_{};", texture.name);
        });
    }
    Ok(out)
}

/// The buffers whose blocks `program`'s stages declare, but the
/// projection's, with the stages that read each: the paint parameters, if
/// the program reads them, and the program's own.
pub(crate) fn blocks(program: &Program) -> impl Iterator<Item = (&ProgramBuffer, Stages)> {
    let paint_params = program.paint_params.map(|stages| (&PAINT_PARAMS, stages));
    paint_params.into_iter().chain(program.buffers.iter().map(|buffer| (buffer, buffer.stages)))
}

/// The block of `buffer`. Fails for a vertex buffer, which GL ES binds as
/// attributes.
fn block(buffer: &ProgramBuffer) -> Result<&'static Block, String> {
    buffer.block().ok_or_else(|| format!("GL ES binds vertex buffer {} as attributes", buffer.name))
}

/// The GL ES name of `member` of `buffer`'s `block`, an anonymous block,
/// of a program of `declarations`.
fn member_name(declarations: &GlslEsDeclarations, buffer: &ProgramBuffer, block: &Block, member: &Member) -> String {
    if precision::is_pad(member) {
        return format!("{}_{}", buffer.name, member.name);
    }
    match declarations.names.iter().find(|&&(of, name, _)| of.name == block.name && name == member.name) {
        Some(&(_, _, name)) => name.to_owned(),
        None => format!("u_{}", member.name),
    }
}

/// Writes the declaration of `member` of `block`, of a program of
/// `declarations`, as `name`.
fn declare_member(out: &mut String, declarations: &GlslEsDeclarations, block: &Block, member: &Member, name: &str) {
    let precision = precision::member(declarations, block, member).qualifier();
    let _ = match member.ty {
        MemberType::Float => writeln!(out, "    {precision} float {name};"),
        MemberType::Int => writeln!(out, "    {precision} int {name};"),
        MemberType::Uint => writeln!(out, "    {precision} uint {name};"),
        MemberType::Vec2 => writeln!(out, "    {precision} vec2 {name};"),
        MemberType::Vec3 => writeln!(out, "    {precision} vec3 {name};"),
        MemberType::Vec4 => writeln!(out, "    {precision} vec4 {name};"),
        MemberType::Mat4 => writeln!(out, "    {precision} mat4 {name};"),
        MemberType::Vec4Array(count) => writeln!(out, "    {precision} vec4 {name}[{count}];"),
    };
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
fn conditional(out: &mut String, condition: Option<&str>, write: impl FnOnce(&mut String)) {
    let Some(condition) = condition else {
        return write(out);
    };
    out.push_str(condition);
    out.push('\n');
    write(out);
    for _ in condition.lines() {
        out.push_str("#endif\n");
    }
}

/// The GLSL type that reads `program`'s `attribute`: floats, as many as its
/// format has, or two of a property of one.
fn input_type(program: &Program, attribute: &Attribute) -> Result<&'static str, String> {
    let format = attribute_format(attribute.format)
        .ok_or_else(|| format!("GL binds no attribute of {:?}, as {} is", attribute.format, attribute.name))?;
    Ok(match format.components {
        1 if is_property(program, attribute) => "vec2",
        1 => "float",
        2 => "vec2",
        3 => "vec3",
        _ => "vec4",
    })
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use mln_engine::shaders::PROGRAMS;
    use mln_gfx::shaders::source::GlslEsSource;

    use super::*;

    /// Every program the renderer draws with that has GLSL ES, with its
    /// GLSL ES.
    fn programs() -> impl Iterator<Item = (&'static Program, &'static GlslEsSource)> {
        PROGRAMS.iter().filter_map(|&program| Some((program, program.sources.glsl_es?)))
    }

    /// The blocks of `program`'s buffers.
    fn program_blocks(program: &Program) -> Vec<(&ProgramBuffer, &'static Block)> {
        blocks(program).filter_map(|(buffer, _)| Some((buffer, buffer.block()?))).collect()
    }

    /// An anonymous block's members are its program's uniforms, so no two
    /// members of a program's blocks have one name.
    #[test]
    fn the_members_of_a_programs_blocks_have_names_of_their_own() {
        for (program, source) in programs() {
            let mut names = BTreeSet::new();
            for (buffer, block) in program_blocks(program) {
                for member in block.members {
                    let name = member_name(source.declarations, buffer, block, member);
                    assert!(names.insert(name.clone()), "{}: two members are {name}", program.name);
                }
            }
        }
    }

    /// Each member that a program's declarations name otherwise is of a
    /// block whose member's `u_<member>` another block of a program that
    /// has its block has too.
    #[test]
    fn every_member_named_otherwise_needs_it() {
        for (_, source) in programs() {
            for &(of, member, _) in source.declarations.names {
                let needed = programs().any(|(program, _)| {
                    let blocks = program_blocks(program);
                    blocks.iter().any(|&(_, block)| block == of)
                        && blocks
                            .iter()
                            .any(|&(_, block)| block != of && block.members.iter().any(|m| m.name == member))
                });
                assert!(needed, "{}::{member}", of.name);
            }
        }
    }
}
