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
use crate::shaders::program::{Program, ProgramBuffer, PropertyType};
use crate::shaders::source::{Named, Stage, expand as expand_properties, per_vertex};

/// The GLSL of a program.
///
/// A stage compiles from the structs of the engine's blocks, `mln_glsl`'s
/// prelude of the stage, the structs of the program's blocks, the
/// declarations of what `prelude` or the stage names of what the stage may
/// read ([`Program::bindings`](crate::shaders::program::Program::bindings)),
/// `prelude` and the stage's source. `drawIndex`, the draw's index into its
/// layer's storage buffers, is a push constant. A buffer's declaration is a block
/// without an instance name, at the set and binding of its id and kind in
/// `mln_glsl`'s layout, whose one member has the buffer's
/// [`name`](crate::shaders::program::ProgramBuffer::name): the buffer's block, or
/// an array of the elements, or of the vertices' 32-bit words, of a storage
/// buffer. A fragment stage's declarations also have the program's textures
/// and their samplers, by their names, a
/// `texture2D` and a `sampler` at the bindings of the texture's slot in the
/// layout's image set, which a stage combines where it samples:
/// `texture(sampler2D(image, image_sampler), uv)`.
#[derive(Debug)]
pub struct Glsl {
    /// What both stages read before their sources, "" if nothing.
    pub prelude: &'static str,
    /// The vertex shader.
    pub vertex: &'static str,
    /// The fragment shader.
    pub fragment: &'static str,
}

/// The structs of the blocks of `buffers`, the engine's or a program's. A
/// buffer of one is a
/// `std140` block of one member of the struct, or of an array of them.
#[must_use]
pub fn blocks<'a>(buffers: impl IntoIterator<Item = &'a ProgramBuffer>) -> String {
    super::blocks(buffers, declaration)
}

/// The declaration of `block` in the GLSL: a struct of its members.
fn declaration(block: &Block) -> String {
    let members = c_like_members(
        block,
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

/// The height rule of [`mln_util::dem`] in the GLSL of `mln_glsl`, a string
/// literal that a vertex stage's source names with `concat!`, before the code
/// that reads the ground's height, as `concat!(ground_height!(), "...")`. It
/// defines `ground_height(pos, dem_coords, props)`, the height in meters,
/// exaggerated, at `pos` in tile units of a tile that reads its DEM at
/// `dem_coords`: the DEM uv's scale and offset, then the DEM's width in pixels,
/// 0 for flat ground. The DEM's texels decode by `props`, a
/// [`TerrainPropsUBO`](crate::shaders::terrain_ubo::TerrainPropsUBO). It
/// interpolates between the texels of the program's texture
/// [`GROUND_DEM`](crate::shaders::terrain_ubo::GROUND_DEM), which the vertex
/// stage reads, in the same `f32` operations, in the same order, as
/// [`mln_util::dem::sample`].
#[doc(hidden)]
#[macro_export]
macro_rules! __ground_height_glsl {
    () => {
        r"
// The height in meters of the DEM's bordered texel at `texel`, clamped to the
// texture, decoded by `props`.
float ground_texel_height(const ivec2 texel, const TerrainPropsUBO props) {
    const ivec2 last = textureSize(ground_dem, 0) - 1;
    const vec3 rgb = round(texelFetch(ground_dem, clamp(texel, ivec2(0), last), 0).rgb * 255.0);
    const vec4 unpack = props.unpack;
    return (rgb.r * unpack.x + rgb.g * unpack.y + rgb.b * unpack.z + unpack.w) * props.unpack_scale;
}

// The height in meters, exaggerated, at `pos` in tile units of a tile that
// reads its DEM at `dem_coords`: the DEM uv's scale and offset, then the
// DEM's width in pixels, 0 for flat ground.
float ground_height(const vec2 pos, const vec4 dem_coords, const TerrainPropsUBO props) {
    const float dim = dem_coords.w;
    if (dim == 0.0) {
        return 0.0;
    }
    const vec2 coord = (pos / 8192.0 * dem_coords.x + dem_coords.yz) * dim + 0.5;
    const vec2 first = floor(coord);
    const vec2 f = coord - first;
    const ivec2 texel = ivec2(first);
    const float tl = ground_texel_height(texel, props);
    const float tr = ground_texel_height(texel + ivec2(1, 0), props);
    const float bl = ground_texel_height(texel + ivec2(0, 1), props);
    const float br = ground_texel_height(texel + ivec2(1, 1), props);
    const float top = tl * (1.0 - f.x) + tr * f.x;
    const float bottom = bl * (1.0 - f.x) + br * f.x;
    return (top * (1.0 - f.y) + bottom * f.y) * props.exaggeration;
}
"
    };
}

#[doc(inline)]
pub use crate::__ground_height_glsl as ground_height;
