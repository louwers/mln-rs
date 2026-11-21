//! GLSL ES 3.00, which `mln_gl` compiles, as `#version 330 core` on desktop
//! OpenGL.
//!
//! A block's members are in the program's scope, so a stage reads them by
//! their names alone. `mln_gl` declares a block only for a stage that names
//! one of them: WebGL2 rejects a draw whose program has a block without a
//! buffer, whether or not the shader reads it.
//!
//! A property's directives (see [`source`](super#properties)) write, each
//! a `float` for a float and a `vec4` otherwise, at the precision of the
//! property's uniform `u_<name>` in the program's blocks, or at the one
//! that the directive's last word names (`lowp`, `mediump` or `highp`):
//!
//! - `#property attribute <name>`: the vertex stage's input `a_<name>` at
//!   its location, a `vec2` for a float.
//! - `#property varying <name> [flat]`: the vertex stage's output and the
//!   fragment stage's input `<name>`, `flat` if the directive says so.
//! - `#property pass <name>`: in a vertex stage, `<name> = ` its value per
//!   vertex, or in a variant that reads it from its uniform, a local
//!   `<name>` of the uniform.
//! - `#property uniform <name>`: in a variant that reads it from its
//!   uniform, a local `<name>` of the uniform, which a fragment stage reads
//!   in place of the varying.
//!
//! A vertex stage reads a property's values per vertex from `a_<name>`,
//! mixed by `u_<name>_t`. A `$` goes before `u_<name>`, and only in a vertex
//! stage: a fragment stage reads the varying `<name>` per vertex.

use std::fmt::Write as _;

use crate::gpu::GpuError;
use crate::shaders::block::{Block, Member, MemberType};
use crate::shaders::program::{Program, PropertyType};
use crate::shaders::source::{Named, Stage, expand as expand_properties, from_uniform, per_vertex};

/// The GLSL of a program.
///
/// A stage compiles from `#version 300 es`, or `#version 330 core` on
/// desktop OpenGL, the program's defines, the blocks of the engine's
/// buffers and the program's that the stage names a member of, the
/// backend's prelude of that stage, and the shader's. A vertex stage that
/// names a member of the projection block or one of the backend's
/// projection helpers also has the block and the helpers.
#[derive(Debug)]
pub struct Glsl {
    /// The vertex shader.
    pub vertex: &'static str,
    /// The fragment shader.
    pub fragment: &'static str,
}

/// The declaration of `block` in GLSL ES: a `std140` uniform block whose
/// members are in the program's scope, named as [`Block`] says.
#[must_use]
pub fn declaration(block: &Block) -> String {
    let mut text = format!("layout (std140) uniform {} {{\n", block.name);
    for member in block.members {
        let (ty, array) = match member.ty {
            MemberType::Float => ("float", None),
            MemberType::Int => ("int", None),
            MemberType::Uint => ("uint", None),
            MemberType::Bool => ("bool", None),
            MemberType::Vec2 => ("vec2", None),
            MemberType::Vec3 => ("vec3", None),
            MemberType::Vec4 => ("vec4", None),
            MemberType::Mat4 => ("mat4", None),
            MemberType::Vec4Array(n) => ("vec4", Some(n)),
        };
        let name = member_name(block, member);
        let array = array.map(|n| format!("[{n}]")).unwrap_or_default();
        // A `bool` has no precision.
        let precision =
            if member.ty == MemberType::Bool { String::new() } else { format!("{} ", member.precision.glsl_es()) };
        _ = writeln!(text, "    {precision}{ty} {name}{array};");
    }
    text + "};\n"
}

/// The name of `member` of `block` in GLSL ES, in the program's scope.
#[must_use]
pub fn member_name(block: &Block, member: &Member) -> String {
    if member.padding {
        format!("{}_{}", block.name, member.name)
    } else {
        format!("{}{}", block.glsl_es_prefix, member.name)
    }
}

/// The vertex and fragment shaders of `program`'s GLSL ES, `glsl`, with its
/// properties' directives written out, as the [module docs](self) say.
/// Fails for a directive that names no property of the program, or that
/// names no precision for one without a uniform in the program's blocks,
/// or that is malformed.
pub fn expand(program: &Program, glsl: &Glsl) -> Result<[String; 2], GpuError> {
    let stage = |text, stage| {
        expand_properties(
            program,
            text,
            "u_",
            |role, named| directive(program, named, role, stage),
            |named| value(named, stage),
        )
    };
    Ok([stage(glsl.vertex, Stage::Vertex)?, stage(glsl.fragment, Stage::Fragment)?])
}

fn directive(program: &Program, named: &Named<'_>, role: &str, stage: Stage) -> Result<String, GpuError> {
    let name = named.name();
    // A last word that is a precision overrides the uniform's.
    let (args, precision) = match named.args.split_last() {
        Some((&last, args)) if ["lowp", "mediump", "highp"].contains(&last) => (args, last),
        _ => (&named.args[..], uniform(program, named)?.precision.glsl_es()),
    };
    let flat = match args {
        [] => false,
        ["flat"] if role == "varying" => true,
        _ => return Err(GpuError::Invalid(format!("bad arguments for property {name}: {:?}", named.args))),
    };
    let ty = if named.ty() == PropertyType::Float { "float" } else { "vec4" };
    let local = format!("{precision} {ty} {name} = u_{name};");
    Ok(match role {
        "attribute" if stage == Stage::Vertex => {
            let attribute = if named.ty() == PropertyType::Float { "vec2" } else { "vec4" };
            per_vertex(
                named,
                &format!("layout (location = {}) in {precision} {attribute} a_{name};", named.location()?),
            )
        }
        "varying" => {
            let flat = if flat { "flat " } else { "" };
            let direction = if stage == Stage::Vertex { "out" } else { "in" };
            per_vertex(named, &format!("{flat}{direction} {precision} {ty} {name};"))
        }
        "pass" if stage == Stage::Vertex => {
            format!("#ifndef HAS_UNIFORM_u_{name}\n{name} = {};\n#else\n{local}\n#endif", value(named, stage)?)
        }
        "uniform" => from_uniform(named, &local),
        _ => return Err(GpuError::Invalid(format!("no GLSL ES property directive {role} for {name}"))),
    })
}

/// The member of a block of `program` that is `named`'s uniform.
fn uniform(program: &Program, named: &Named<'_>) -> Result<&'static Member, GpuError> {
    let name = format!("u_{}", named.name());
    program
        .buffers
        .iter()
        .filter_map(super::super::program::Buffer::block)
        .find_map(|block| block.members.iter().find(|member| member_name(block, member) == name))
        .ok_or_else(|| GpuError::Invalid(format!("{} has no uniform {name}", program.name)))
}

/// What a vertex stage reads of `named` per vertex.
fn value(named: &Named<'_>, stage: Stage) -> Result<String, GpuError> {
    let name = named.name();
    Ok(match (stage, named.ty()) {
        (Stage::Fragment, _) => {
            return Err(GpuError::Invalid(format!("a `$` for {name} in a fragment shader, which reads `{name}`")));
        }
        (Stage::Vertex, PropertyType::Float) => format!("unpack_mix_vec2(a_{name}, u_{name}_t)"),
        (Stage::Vertex, PropertyType::Color) => format!("unpack_mix_color(a_{name}, u_{name}_t)"),
        (Stage::Vertex, PropertyType::Vec4) => format!("a_{name}"),
    })
}
