//! MSL, which `mln_mtl` compiles.
//!
//! A property's directives (see [`source`](super#properties)) write:
//!
//! - `#property attribute <name>`: a member `<name> [[attribute(<location>)]]`
//!   of the vertex stage's input struct: `float2` for a float, and `float4`,
//!   or `ushort4` for an attribute of `UShort4`, otherwise.
//! - `#property varying <name> <type> [flat]`: a member `<type> <name>` of
//!   the struct the vertex stage outputs, `[[flat]]` if the directive says
//!   so.
//! - `#property pass <name>`: `.<name> = ` its value per vertex, as `half`
//!   or `half4` if its varying is, and a comma, which initializes the
//!   member of the vertex stage's output.
//!
//! A main function's stage is the source from its line to the other's, and
//! the text before both is the vertex stage's. A vertex stage reads a property's values per vertex from
//! `vertex_in.<name>`, mixed by `drawable.<name>_t`, and a fragment stage from
//! `in.<name>`.

use crate::gpu::GpuError;
use crate::shaders::block::{Block, MemberType, c_like_members};
use crate::shaders::program::{Buffer, Program, PropertyType};
use crate::shaders::source::{Named, Stage, expand as expand_properties, per_vertex};

/// The MSL of a program. Its library compiles from the backend's prelude,
/// the structs of the engine's blocks and the program's, `prelude` and
/// `source`. A main function has the parameters of the buffers that its
/// body names, which the backend adds after its own: `uboIndex`, the draw's
/// index into its layer's storage buffers, the engine's buffers and the
/// program's, each by its [`name`](crate::shaders::program::Buffer::name)
/// at the buffer index of its id: a reference to a block, and a pointer to
/// the elements or the vertices of a storage buffer.
#[derive(Debug)]
pub struct Msl {
    /// The name of the vertex function.
    pub vertex_main_function: &'static str,
    /// The name of the fragment function.
    pub fragment_main_function: &'static str,
    /// The shader's own constants and structs, "" if none.
    pub prelude: &'static str,
    /// The functions.
    pub source: &'static str,
}

/// The structs of the blocks of `buffers`, the engine's or a program's, each
/// of a storage buffer's element padded to its stride.
#[must_use]
pub fn blocks<'a>(buffers: impl IntoIterator<Item = &'a Buffer>) -> String {
    super::blocks(buffers, declaration)
}

/// The declaration of `block` in MSL: a struct of its members, aligned to
/// 16 bytes and padded to `size` bytes, the stride of the storage buffer
/// whose elements it is, or its own size, which a `static_assert` checks.
fn declaration(block: &Block, size: usize) -> String {
    let members = c_like_members(
        block,
        size,
        |ty| match ty {
            MemberType::Float => "float",
            MemberType::Int | MemberType::Bool => "int",
            MemberType::Uint => "uint",
            MemberType::Vec2 => "float2",
            MemberType::Vec3 => "packed_float3",
            MemberType::Vec4 | MemberType::Vec4Array(_) => "float4",
            MemberType::Mat4 => "float4x4",
        },
        "float4",
    );
    let name = block.name;
    format!("struct alignas(16) {name} {{\n{members}}};\nstatic_assert(sizeof({name}) == {size}, \"wrong size\");\n")
}

/// The source of `program`'s MSL, `msl`, with its properties' directives
/// written out, as the [module docs](self) say. Fails for a directive that
/// names no property of the program, or that is malformed.
pub fn expand(program: &Program, msl: &Msl) -> Result<String, GpuError> {
    let source = msl.source;
    // Where each main function's line starts, and its stage.
    let line_of = |name: &str| {
        let at = source.find(&format!(" {name}("))?;
        Some(source[..at].rfind('\n').map_or(0, |line| line + 1))
    };
    let mut mains: Vec<(usize, Stage)> =
        [(msl.vertex_main_function, Stage::Vertex), (msl.fragment_main_function, Stage::Fragment)]
            .into_iter()
            .filter_map(|(name, stage)| Some((line_of(name)?, stage)))
            .collect();
    mains.sort_by_key(|&(at, _)| at);
    // The type of each varying a directive declares, which the vertex
    // stage converts its value to.
    let varyings: Vec<(&str, &str)> = source
        .lines()
        .filter_map(|line| {
            let mut words = line.trim().strip_prefix("#property varying ")?.split_whitespace();
            Some((words.next()?, words.next()?))
        })
        .collect();
    let stage = |text, stage| {
        let directive = |role: &str, named: &Named<'_>| {
            let name = named.name();
            let text = match role {
                "attribute" => {
                    named.words(false, false, false)?;
                    let ty = match named.ty() {
                        PropertyType::Float => "float2",
                        PropertyType::Vec4 if named.integers() => "ushort4",
                        PropertyType::Color | PropertyType::Vec4 => "float4",
                    };
                    format!("{ty} {name} [[attribute({})]];", named.location()?)
                }
                "varying" => {
                    let value = named.value(true)?;
                    let flat = if named.flat { " [[flat]]" } else { "" };
                    format!("{value} {name}{flat};")
                }
                "pass" if stage == Stage::Vertex => {
                    named.words(false, false, false)?;
                    let ty = varyings.iter().find(|(varying, _)| *varying == name).map_or("", |(_, ty)| *ty);
                    let value = value(named, stage);
                    if ty.starts_with("half") {
                        format!(".{name} = {ty}({value}),")
                    } else {
                        format!(".{name} = {value},")
                    }
                }
                _ => return Err(GpuError::Invalid(format!("no MSL property directive {role} for {name}"))),
            };
            Ok(per_vertex(named, &text))
        };
        expand_properties(program, text, "", directive, |named| Ok(value(named, stage)))
    };
    // The text before the first main function, the structs, is the vertex
    // stage's, and each main function's, to the next one, its stage's.
    let mut parts = vec![(0, Stage::Vertex)];
    parts.extend(mains);
    let mut expanded = String::with_capacity(source.len());
    for (index, &(start, part)) in parts.iter().enumerate() {
        let end = parts.get(index + 1).map_or(source.len(), |&(end, _)| end);
        expanded.push_str(&stage(&source[start..end], part)?);
    }
    Ok(expanded)
}

/// What a stage reads of `named` per vertex.
fn value(named: &Named<'_>, stage: Stage) -> String {
    let name = named.name();
    match (stage, named.ty()) {
        (Stage::Fragment, _) => format!("in.{name}"),
        (Stage::Vertex, PropertyType::Float) => format!("unpack_mix_float(vertex_in.{name}, drawable.{name}_t)"),
        (Stage::Vertex, PropertyType::Color) => format!("unpack_mix_color(vertex_in.{name}, drawable.{name}_t)"),
        (Stage::Vertex, PropertyType::Vec4) if named.integers() => format!("float4(vertex_in.{name})"),
        (Stage::Vertex, PropertyType::Vec4) => format!("vertex_in.{name}"),
    }
}
