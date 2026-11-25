// A program's MSL reads the program's attribute at location `l` as
// `[[attribute(l)]]`, and its texture at slot `s` and the texture's sampler
// as `[[texture(s)]]` and `[[sampler(s)]]`. Its vertex buffers bind after the
// shader's own buffers, from `<layer>UBOCount` on, one at each index.

use mln_gfx::shaders::program::Program;
use mln_gfx::shaders::programs;
use mln_gfx::shaders::shader_defines::{
    ATMOSPHERE_LAYER_UBO_COUNT, BACKGROUND_UBO_COUNT, CIRCLE_LAYER_UBO_COUNT, COLLISION_UBO_COUNT,
    COLOR_RELIEF_LAYER_UBO_COUNT, DRAWABLE_RESERVED_UBO_COUNT, FILL_EXTRUSION_LAYER_UBO_COUNT, FILL_LAYER_UBO_COUNT,
    HEATMAP_LAYER_UBO_COUNT, HILLSHADE_LAYER_UBO_COUNT, ID_HEATMAP_TEXTURE_PROPS_UBO,
    ID_HILLSHADE_PREPARE_TILE_PROPS_UBO, ID_TERRAIN_LIGHT_UBO, LAYER_COMPOSITE_UBO_COUNT, LINE_LAYER_UBO_COUNT,
    RASTER_LAYER_UBO_COUNT, SKY_LAYER_UBO_COUNT, SYMBOL_LAYER_UBO_COUNT,
};

use super::msl_sources;

/// The MSL of a shader. Its library compiles from
/// `msl_sources::PRELUDE + prelude + source`.
#[derive(Debug)]
pub(crate) struct MslShaderSource {
    pub(crate) vertex_main_function: &'static str,
    pub(crate) fragment_main_function: &'static str,
    /// The shader's own prelude constant (its UBO ids and structs), "" if none.
    pub(crate) prelude: &'static str,
    pub(crate) source: &'static str,
}

/// A shader: its MSL, and where its vertex buffers bind.
#[derive(Debug)]
pub(crate) struct ShaderSource {
    pub(crate) msl: &'static MslShaderSource,
    /// The buffer index of the first vertex buffer, past the indexes of the
    /// shader's own buffers; `None` for a shader without attributes.
    pub(crate) first_vertex_buffer: Option<usize>,
    /// Whether its library compiles with fast math, Metal's default, which
    /// lets the compiler fuse and reorder floating-point operations.
    pub(crate) fast_math: bool,
}

// The number of UBOs each shader binds before its vertex buffers, for the
// shaders whose count shader_defines.rs doesn't name.
const ATMOSPHERE_UBO_COUNT: usize = ATMOSPHERE_LAYER_UBO_COUNT;
const CIRCLE_UBO_COUNT: usize = CIRCLE_LAYER_UBO_COUNT;
const CLIPPING_MASK_UBO_COUNT: usize = DRAWABLE_RESERVED_UBO_COUNT;
const COLOR_RELIEF_UBO_COUNT: usize = COLOR_RELIEF_LAYER_UBO_COUNT;
const FILL_UBO_COUNT: usize = FILL_LAYER_UBO_COUNT;
const FILL_EXTRUSION_UBO_COUNT: usize = FILL_EXTRUSION_LAYER_UBO_COUNT;
const GLOBE_DEPTH_UBO_COUNT: usize = DRAWABLE_RESERVED_UBO_COUNT;
const HEATMAP_UBO_COUNT: usize = HEATMAP_LAYER_UBO_COUNT;
const HEATMAP_TEXTURE_UBO_COUNT: usize = ID_HEATMAP_TEXTURE_PROPS_UBO + 1;
const HILLSHADE_PREPARE_UBO_COUNT: usize = ID_HILLSHADE_PREPARE_TILE_PROPS_UBO + 1;
const HILLSHADE_UBO_COUNT: usize = HILLSHADE_LAYER_UBO_COUNT;
const LINE_UBO_COUNT: usize = LINE_LAYER_UBO_COUNT;
const RASTER_UBO_COUNT: usize = RASTER_LAYER_UBO_COUNT;
const SKY_UBO_COUNT: usize = SKY_LAYER_UBO_COUNT;
const SYMBOL_UBO_COUNT: usize = SYMBOL_LAYER_UBO_COUNT;
const TERRAIN_UBO_COUNT: usize = ID_TERRAIN_LIGHT_UBO + 1;
#[cfg(test)]
const VERTEX_TEXEL_UBO_COUNT: usize = DRAWABLE_RESERVED_UBO_COUNT;

macro_rules! shader_source {
    // A shader that compiles without fast math.
    (precise $name:ident, $ubo_count:expr) => {
        static $name: ShaderSource =
            ShaderSource { msl: &msl_sources::$name, first_vertex_buffer: Some($ubo_count), fast_math: false };
    };
    // A shader without attributes.
    ($name:ident) => {
        static $name: ShaderSource =
            ShaderSource { msl: &msl_sources::$name, first_vertex_buffer: None, fast_math: true };
    };
    ($name:ident, $ubo_count:expr) => {
        static $name: ShaderSource =
            ShaderSource { msl: &msl_sources::$name, first_vertex_buffer: Some($ubo_count), fast_math: true };
    };
}

shader_source!(BACKGROUND_SHADER, BACKGROUND_UBO_COUNT);
shader_source!(BACKGROUND_PATTERN_SHADER, BACKGROUND_UBO_COUNT);
shader_source!(CIRCLE_SHADER, CIRCLE_UBO_COUNT);
shader_source!(CLIPPING_MASK_PROGRAM, CLIPPING_MASK_UBO_COUNT);
shader_source!(COLLISION_BOX_SHADER, COLLISION_UBO_COUNT);
shader_source!(COLLISION_CIRCLE_SHADER, COLLISION_UBO_COUNT);
shader_source!(COLOR_RELIEF_SHADER, COLOR_RELIEF_UBO_COUNT);
shader_source!(FILL_SHADER, FILL_UBO_COUNT);
shader_source!(FILL_PATTERN_SHADER, FILL_UBO_COUNT);
// An instance of the outline shaders is a segment of an outline: a vertex
// and the end of its segment, which the same vertices and a buffer of the
// ends provide. The quad's corner is the vertex. Metal draws no lines wider
// than 1 px, so it draws every outline with these.
shader_source!(FILL_OUTLINE_INSTANCED_SHADER, FILL_UBO_COUNT);
shader_source!(FILL_OUTLINE_PATTERN_INSTANCED_SHADER, FILL_UBO_COUNT);
// The pulled outlines read the same vertices, ends and paint values by
// vertex index, four vertices a segment, as storage buffers
// (`ID_FILL_OUTLINE_VERTICES` and on).
shader_source!(FILL_OUTLINE_PULLED_SHADER);
// An instance of the instanced shaders is an outline vertex, and the vertex
// after it.
shader_source!(FILL_EXTRUSION_SHADER, FILL_EXTRUSION_UBO_COUNT);
shader_source!(FILL_EXTRUSION_INSTANCED_SHADER, FILL_EXTRUSION_UBO_COUNT);
// The pulled walls read the bucket's vertices and paint values by vertex
// index, four vertices a wall, as storage buffers
// (`ID_FILL_EXTRUSION_WALL_VERTICES` and on).
shader_source!(FILL_EXTRUSION_PULLED_SHADER);
shader_source!(FILL_EXTRUSION_PATTERN_SHADER, FILL_EXTRUSION_UBO_COUNT);
shader_source!(FILL_EXTRUSION_PATTERN_INSTANCED_SHADER, FILL_EXTRUSION_UBO_COUNT);
shader_source!(GLOBE_DEPTH_SHADER, GLOBE_DEPTH_UBO_COUNT);
shader_source!(HEATMAP_SHADER, HEATMAP_UBO_COUNT);
shader_source!(HEATMAP_TEXTURE_SHADER, HEATMAP_TEXTURE_UBO_COUNT);
shader_source!(LAYER_COMPOSITE_SHADER, LAYER_COMPOSITE_UBO_COUNT);
#[cfg(test)]
shader_source!(VERTEX_TEXEL_SHADER, VERTEX_TEXEL_UBO_COUNT);
// Fast math would let the compiler fuse or reorder the f32 operations of the
// terrain's height rule, which the CPU does one at a time, in order
// (`mln_util::dem`).
shader_source!(precise TERRAIN_SHADER, TERRAIN_UBO_COUNT);
shader_source!(precise TERRAIN_DEPTH_SHADER, TERRAIN_UBO_COUNT);
#[cfg(test)]
shader_source!(precise TERRAIN_PROBE_SHADER, TERRAIN_UBO_COUNT);
shader_source!(HILLSHADE_SHADER, HILLSHADE_UBO_COUNT);
shader_source!(HILLSHADE_PREPARE_SHADER, HILLSHADE_PREPARE_UBO_COUNT);
shader_source!(LINE_SHADER, LINE_UBO_COUNT);
shader_source!(LINE_GRADIENT_SHADER, LINE_UBO_COUNT);
shader_source!(LINE_PATTERN_SHADER, LINE_UBO_COUNT);
shader_source!(LINE_SDF_SHADER, LINE_UBO_COUNT);
shader_source!(LINE_GRADIENT_SDF_SHADER, LINE_UBO_COUNT);
shader_source!(RASTER_SHADER, RASTER_UBO_COUNT);
shader_source!(SKY_SHADER, SKY_UBO_COUNT);
shader_source!(ATMOSPHERE_SHADER, ATMOSPHERE_UBO_COUNT);
shader_source!(SYMBOL_ICON_SHADER, SYMBOL_UBO_COUNT);
shader_source!(SYMBOL_SDF_SHADER, SYMBOL_UBO_COUNT);
shader_source!(SYMBOL_TEXT_AND_ICON_SHADER, SYMBOL_UBO_COUNT);

/// The source of each built-in program that has MSL: those the renderer
/// draws with on Metal, which leaves out the fill outlines as lines, which
/// Metal draws no wider than 1 px, and the fill extrusion walls of devices
/// that rasterize on the CPU.
pub(crate) static BUILT_INS: &[(&Program, &ShaderSource)] = &[
    (&programs::BACKGROUND, &BACKGROUND_SHADER),
    (&programs::BACKGROUND_PATTERN, &BACKGROUND_PATTERN_SHADER),
    (&programs::CIRCLE, &CIRCLE_SHADER),
    (&programs::CLIPPING_MASK, &CLIPPING_MASK_PROGRAM),
    (&programs::COLLISION_BOX, &COLLISION_BOX_SHADER),
    (&programs::COLLISION_CIRCLE, &COLLISION_CIRCLE_SHADER),
    (&programs::COLOR_RELIEF, &COLOR_RELIEF_SHADER),
    (&programs::FILL, &FILL_SHADER),
    (&programs::FILL_PATTERN, &FILL_PATTERN_SHADER),
    (&programs::FILL_OUTLINE_INSTANCED, &FILL_OUTLINE_INSTANCED_SHADER),
    (&programs::FILL_OUTLINE_PULLED, &FILL_OUTLINE_PULLED_SHADER),
    (&programs::FILL_OUTLINE_PATTERN_INSTANCED, &FILL_OUTLINE_PATTERN_INSTANCED_SHADER),
    (&programs::FILL_EXTRUSION, &FILL_EXTRUSION_SHADER),
    (&programs::FILL_EXTRUSION_INSTANCED, &FILL_EXTRUSION_INSTANCED_SHADER),
    (&programs::FILL_EXTRUSION_PULLED, &FILL_EXTRUSION_PULLED_SHADER),
    (&programs::FILL_EXTRUSION_PATTERN, &FILL_EXTRUSION_PATTERN_SHADER),
    (&programs::FILL_EXTRUSION_PATTERN_INSTANCED, &FILL_EXTRUSION_PATTERN_INSTANCED_SHADER),
    (&programs::HEATMAP, &HEATMAP_SHADER),
    (&programs::HEATMAP_TEXTURE, &HEATMAP_TEXTURE_SHADER),
    (&programs::HILLSHADE, &HILLSHADE_SHADER),
    (&programs::HILLSHADE_PREPARE, &HILLSHADE_PREPARE_SHADER),
    (&programs::LINE, &LINE_SHADER),
    (&programs::LINE_GRADIENT, &LINE_GRADIENT_SHADER),
    (&programs::LINE_PATTERN, &LINE_PATTERN_SHADER),
    (&programs::LINE_SDF, &LINE_SDF_SHADER),
    (&programs::LINE_GRADIENT_SDF, &LINE_GRADIENT_SDF_SHADER),
    (&programs::RASTER, &RASTER_SHADER),
    (&programs::SYMBOL_ICON, &SYMBOL_ICON_SHADER),
    (&programs::SYMBOL_SDF, &SYMBOL_SDF_SHADER),
    (&programs::SYMBOL_TEXT_AND_ICON, &SYMBOL_TEXT_AND_ICON_SHADER),
    (&programs::SKY, &SKY_SHADER),
    (&programs::ATMOSPHERE, &ATMOSPHERE_SHADER),
    (&programs::GLOBE_DEPTH, &GLOBE_DEPTH_SHADER),
    (&programs::LAYER_COMPOSITE, &LAYER_COMPOSITE_SHADER),
    (&programs::TERRAIN, &TERRAIN_SHADER),
    (&programs::TERRAIN_DEPTH, &TERRAIN_DEPTH_SHADER),
    #[cfg(test)]
    (&programs::VERTEX_TEXEL, &VERTEX_TEXEL_SHADER),
    #[cfg(test)]
    (&programs::TERRAIN_PROBE, &TERRAIN_PROBE_SHADER),
];
