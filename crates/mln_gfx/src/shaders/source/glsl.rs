//! The GLSL that `mln_glsl` parses with naga's GLSL front end and the
//! Vulkan and D3D12 backends write as SPIR-V and HLSL, whose buffers and
//! textures bind at `mln_glsl`'s descriptor set layout.
//!
//! A property's directives (see [`source`](super#properties)) write:
//!
//! - `#property attribute <name>`: the vertex stage's input
//!   `layout(location = <location>) in <type> in_<name>`: `vec2` for a
//!   float, and `vec4`, or `uvec4` for an attribute of `UShort4`, otherwise.
//! - `#property varying <name> <location> [flat]`: the vertex stage's output
//!   and the fragment stage's input `frag_<name>` at `<location>`, `float`
//!   for a float and `vec4` otherwise, `flat` if the directive says so.
//! - `#property pass <name>`: `frag_<name> = ` its value per vertex.
//!
//! A vertex stage reads a property's values per vertex from `in_<name>`,
//! mixed by `drawable.<name>_t`, and a fragment stage from `frag_<name>`.

use crate::gpu::GpuError;
use crate::shaders::block::{Block, MemberType, c_like_members};
use crate::shaders::program::{Buffer, Program, PropertyType};
use crate::shaders::source::{Named, Stage, expand as expand_properties, per_vertex};

/// The GLSL of a program.
///
/// A stage compiles from the structs of the engine's blocks, `mln_glsl`'s
/// prelude of the stage, the structs of the program's blocks, `prelude` and
/// the stage's source.
#[derive(Debug)]
pub struct Glsl {
    /// The shader's own constants (its buffer bindings), "" if none.
    pub prelude: &'static str,
    /// The vertex shader.
    pub vertex: &'static str,
    /// The fragment shader.
    pub fragment: &'static str,
}

/// The structs of the blocks of `buffers`, the engine's or a program's, each
/// of a storage buffer's element padded to its stride. A buffer of one is a
/// `std140` block of one member of the struct, or of an array of them.
#[must_use]
pub fn blocks<'a>(buffers: impl IntoIterator<Item = &'a Buffer>) -> String {
    super::blocks(buffers, declaration)
}

/// The declaration of `block` in the GLSL: a struct of its members, padded
/// to `size` bytes, the stride of the storage buffer whose elements it is,
/// or its own size.
fn declaration(block: &Block, size: usize) -> String {
    let members = c_like_members(
        block,
        size,
        |ty| match ty {
            MemberType::Float => "float",
            MemberType::Int | MemberType::Bool => "int",
            MemberType::Uint => "uint",
            MemberType::Vec2 => "vec2",
            MemberType::Vec3 => "vec3",
            MemberType::Vec4 | MemberType::Vec4Array(_) => "vec4",
            MemberType::Mat4 => "mat4",
        },
        "vec4",
    );
    format!("struct {} {{\n{members}}};\n", block.name)
}

/// The `stage` of `program`'s GLSL, `glsl`, with its properties'
/// directives written out, as the [module docs](self) say. Fails for a
/// directive that names no property of the program, or that is malformed.
pub fn expand(program: &Program, glsl: &Glsl, stage: Stage) -> Result<String, GpuError> {
    let text = match stage {
        Stage::Vertex => glsl.vertex,
        Stage::Fragment => glsl.fragment,
    };
    expand_properties(program, text, "", |role, named| directive(named, role, stage), |named| Ok(value(named, stage)))
}

fn directive(named: &Named<'_>, role: &str, stage: Stage) -> Result<String, GpuError> {
    let name = named.name();
    let text = match role {
        "attribute" if stage == Stage::Vertex => {
            named.words(false, false, false)?;
            let ty = match named.ty() {
                PropertyType::Float => "vec2",
                PropertyType::Vec4 if named.integers() => "uvec4",
                PropertyType::Color | PropertyType::Vec4 => "vec4",
            };
            format!("layout(location = {}) in {ty} in_{name};", named.location()?)
        }
        "varying" => {
            let value = named.value(true)?;
            let flat = if named.flat { "flat " } else { "" };
            let direction = if stage == Stage::Vertex { "out" } else { "in" };
            let ty = if named.ty() == PropertyType::Float { "float" } else { "vec4" };
            format!("layout(location = {value}) {flat}{direction} {ty} frag_{name};")
        }
        "pass" if stage == Stage::Vertex => {
            named.words(false, false, false)?;
            format!("frag_{name} = {};", value(named, stage))
        }
        _ => return Err(GpuError::Invalid(format!("no GLSL property directive {role} for {name}"))),
    };
    Ok(per_vertex(named, &text))
}

/// What a stage reads of `named` per vertex.
fn value(named: &Named<'_>, stage: Stage) -> String {
    let name = named.name();
    match (stage, named.ty()) {
        (Stage::Fragment, _) => format!("frag_{name}"),
        (Stage::Vertex, PropertyType::Float) => format!("unpack_mix_float(in_{name}, drawable.{name}_t)"),
        (Stage::Vertex, PropertyType::Color) => format!("unpack_mix_color(in_{name}, drawable.{name}_t)"),
        (Stage::Vertex, PropertyType::Vec4) => format!("in_{name}"),
    }
}
