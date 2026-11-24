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
/// blocks and the program's, the declarations of what `prelude` or the
/// stage names of what the stage may read
/// ([`Program::bindings`](crate::shaders::program::Program::bindings)),
/// `prelude` and the stage's source. The backend declares `drawIndex`, the
/// draw's index into the storage buffers it reads, and the buffers, each by its
/// [`name`](crate::shaders::program::ProgramBuffer::name) at its id in group 0: a
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

/// The structs of the blocks of the engine's buffers and `program`'s.
#[must_use]
pub fn blocks(program: &Program) -> String {
    super::blocks(ENGINE_BUFFERS.iter().chain(program.buffers), declaration)
}

/// The declaration of `block` in WGSL: a struct of its members.
fn declaration(block: &Block) -> String {
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

/// The height rule of [`mln_util::dem`] in WGSL, a string literal that a vertex
/// stage's source names with `concat!`, before the code that reads the ground's
/// height, as `concat!(ground_height!(), "...")`. It defines
/// `ground_height(pos, dem_coords, props)`, the height in meters, exaggerated,
/// at `pos` in tile units of a tile that reads its DEM at `dem_coords`: the DEM
/// uv's scale and offset, then the DEM's width in pixels, 0 for flat ground.
/// The DEM's texels decode by `props`, a
/// [`TerrainPropsUBO`](crate::shaders::terrain_ubo::TerrainPropsUBO). It
/// interpolates between the texels of the program's texture
/// [`GROUND_DEM`](crate::shaders::terrain_ubo::GROUND_DEM), which the vertex
/// stage reads, in the same `f32` operations, in the same order, as
/// [`mln_util::dem::sample`].
#[doc(hidden)]
#[macro_export]
macro_rules! __ground_height_wgsl {
    () => {
        r"
// The height in meters of the DEM's bordered texel at `texel`, clamped to
// the texture, decoded by `props`.
fn ground_texel_height(texel: vec2<i32>, props: TerrainPropsUBO) -> f32 {
    let last = vec2<i32>(textureDimensions(ground_dem)) - 1;
    let rgb = round(textureLoad(ground_dem, clamp(texel, vec2<i32>(0), last), 0).rgb * 255.0);
    let unpack = props.unpack;
    return (rgb.r * unpack.x + rgb.g * unpack.y + rgb.b * unpack.z + unpack.w) * props.unpack_scale;
}

// The height in meters, exaggerated, at `pos` in tile units of a tile that
// reads its DEM at `dem_coords`: the DEM uv's scale and offset, then the
// DEM's width in pixels, 0 for flat ground.
fn ground_height(pos: vec2<f32>, dem_coords: vec4<f32>, props: TerrainPropsUBO) -> f32 {
    let dim = dem_coords.w;
    if (dim == 0.0) {
        return 0.0;
    }
    let coord = (pos / 8192.0 * dem_coords.x + dem_coords.yz) * dim + 0.5;
    let first = floor(coord);
    let f = coord - first;
    let texel = vec2<i32>(first);
    let tl = ground_texel_height(texel, props);
    let tr = ground_texel_height(texel + vec2<i32>(1, 0), props);
    let bl = ground_texel_height(texel + vec2<i32>(0, 1), props);
    let br = ground_texel_height(texel + vec2<i32>(1, 1), props);
    let top = tl * (1.0 - f.x) + tr * f.x;
    let bottom = bl * (1.0 - f.x) + br * f.x;
    return (top * (1.0 - f.y) + bottom * f.y) * props.exaggeration;
}
"
    };
}

#[doc(inline)]
pub use crate::__ground_height_wgsl as ground_height;
