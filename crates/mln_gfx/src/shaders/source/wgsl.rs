//! WGSL, which `mln_webgpu` compiles.
//!
//! A property's directives (see [`source`](super#properties)) write:
//!
//! - `#property attribute <name>`: a field `@location(<location>) <name>`
//!   of the vertex input struct: `vec2<f32>` for a float, and `vec4<f32>`,
//!   or `vec4<u32>` for an attribute of `UShort4`, otherwise.
//! - `#property varying <name> <location> [flat]`: a field
//!   `@location(<location>) <name>` of the vertex output and fragment input
//!   structs, `f32` for a float and `vec4<f32>` otherwise, interpolated
//!   `flat` if the directive says so.
//! - `#property pass <name>`: `out.<name> = ` its value per vertex.
//!
//! A vertex stage reads a property's values per vertex from `in.<name>`,
//! mixed by `drawable.<name>_t`, and a fragment stage from `in.<name>`.

use std::fmt::Write as _;

use crate::gpu::GpuError;
use crate::shaders::block::{Block, MemberType};
use crate::shaders::program::{ENGINE_BUFFERS, Program, PropertyType};
use crate::shaders::source::{Named, Stage, expand as expand_properties, per_vertex};

/// The WGSL of a program: its stages, and the constants and structs of its
/// own that both stages read.
///
/// A stage compiles from the backend's prelude, the structs of the engine's
/// blocks and the program's, the declarations of the buffers that `prelude`
/// or the stage names, `prelude` and the stage's source. The backend
/// declares `globalIndex`, the draw's index into the storage buffers it
/// reads, the engine's buffers and the program's, each by its
/// [`name`](crate::shaders::program::Buffer::name) at its id in group 0: a
/// block as a `var<uniform>`, and an array of the elements, or of the
/// vertices' 32-bit words, of a storage buffer as a `var<storage, read>`.
/// It declares the program's textures and their samplers in group 1, by
/// their names, as a `texture_2d<f32>` and a `sampler` at bindings of the
/// texture's slot. A stage that names a block, a texture or a sampler in any
/// variant declares it in every variant, and a vertex buffer only in the
/// variants that name it.
#[derive(Debug)]
pub struct Wgsl {
    /// The vertex stage.
    pub vertex: &'static str,
    /// The fragment stage.
    pub fragment: &'static str,
    /// What both stages read, "" if nothing.
    pub prelude: &'static str,
}

/// The structs of the blocks of the engine's buffers and `program`'s,
/// each of a storage buffer's element padded to its stride.
#[must_use]
pub fn blocks(program: &Program) -> String {
    super::blocks(ENGINE_BUFFERS.iter().chain(program.buffers), declaration)
}

/// The declaration of `block` in WGSL: a struct of its members, padded to
/// `size` bytes, the stride of the storage buffer whose elements it is, or
/// its own size.
fn declaration(block: &Block, size: usize) -> String {
    let mut text = format!("struct {} {{\n", block.name);
    for member in block.members {
        let ty = match member.ty {
            MemberType::Float => "f32".to_owned(),
            MemberType::Int => "i32".to_owned(),
            MemberType::Uint | MemberType::Bool => "u32".to_owned(),
            MemberType::Vec2 => "vec2<f32>".to_owned(),
            MemberType::Vec3 => "vec3<f32>".to_owned(),
            MemberType::Vec4 => "vec4<f32>".to_owned(),
            MemberType::Mat4 => "mat4x4<f32>".to_owned(),
            MemberType::Vec4Array(n) => format!("array<vec4<f32>, {n}>"),
        };
        _ = writeln!(text, "    {}: {ty},", member.name);
    }
    if size > block.size {
        _ = writeln!(text, "    stride_pad: array<vec4<f32>, {}>,", (size - block.size) / 16);
    }
    text + "};\n"
}

/// The vertex and fragment stages of `program`'s WGSL, `source`, with its
/// properties' directives written out, as the [module docs](self) say.
/// Fails for a directive that names no property of the program, or that is
/// malformed.
pub fn expand(program: &Program, source: &Wgsl) -> Result<[String; 2], GpuError> {
    let stage = |text: &str, stage| {
        expand_properties(
            program,
            text,
            "",
            |role, named| directive(named, role, stage),
            |named| Ok(value(named, stage)),
        )
    };
    Ok([stage(source.vertex, Stage::Vertex)?, stage(source.fragment, Stage::Fragment)?])
}

fn directive(named: &Named<'_>, role: &str, stage: Stage) -> Result<String, GpuError> {
    let name = named.name();
    let text = match role {
        "attribute" => {
            named.words(false, false, false)?;
            let ty = match named.ty() {
                PropertyType::Float => "vec2<f32>",
                PropertyType::Vec4 if named.integers() => "vec4<u32>",
                PropertyType::Color | PropertyType::Vec4 => "vec4<f32>",
            };
            format!("@location({}) {name}: {ty},", named.location()?)
        }
        "varying" => {
            let value = named.value(true)?;
            let flat = if named.flat { "@interpolate(flat) " } else { "" };
            let ty = if named.ty() == PropertyType::Float { "f32" } else { "vec4<f32>" };
            format!("@location({value}) {flat}{name}: {ty},")
        }
        "pass" if stage == Stage::Vertex => {
            named.words(false, false, false)?;
            format!("out.{name} = {};", value(named, stage))
        }
        _ => return Err(GpuError::Invalid(format!("no WGSL property directive {role} for {name}"))),
    };
    Ok(per_vertex(named, &text))
}

/// What a stage reads of `named` per vertex.
fn value(named: &Named<'_>, stage: Stage) -> String {
    let name = named.name();
    match (stage, named.ty()) {
        (Stage::Vertex, PropertyType::Float) => format!("unpack_mix_float(in.{name}, drawable.{name}_t)"),
        (Stage::Vertex, PropertyType::Color) => format!("unpack_mix_color(in.{name}, drawable.{name}_t)"),
        (Stage::Vertex, PropertyType::Vec4) if named.integers() => format!("vec4<f32>(in.{name})"),
        // A fragment stage's varying, or the values themselves.
        (Stage::Fragment, _) | (Stage::Vertex, PropertyType::Vec4) => format!("in.{name}"),
    }
}
