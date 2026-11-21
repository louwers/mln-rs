//! What the layer types' programs and shader sources share.
//!
//! A layer type's module has its programs (`programs`), each a `static`
//! `Program` that declares what its shaders bind and carries their sources,
//! the uniform blocks its shaders read (`ubo`), each declared once with
//! `mln_gfx::uniform_block!`, which the backends declare in each language
//! for the programs that read them, and a module of sources for
//! each shading language: `wgsl` for `mln_webgpu`, `msl` for `mln_mtl`,
//! `glsl_es` for `mln_gl`, and `glsl` for `mln_vulkan` and `mln_d3d12`,
//! which parse it with `mln_glsl`. A source reads its program's buffers by
//! their names, which the backends declare, and its data-driven properties
//! through the directives that `mln_gfx::shaders::source` describes.
//!
//! The vertex buffers that the MSL reads follow the shader's buffers, from
//! the index after its last one: the layout vertices, then the fill
//! extrusions' instances, then the interleaved data-driven vertices, then
//! the fill extrusions' instances again, one vertex on, for each instance's
//! next outline vertex.
//!
//! The GLSL ES reads the vertices its renderer makes and draws what the
//! WGSL does:
//! - `atlas_pos` ([`atlas_pos!`]) places the samples of a
//!   pattern, and the pattern rectangles reach the fragment shaders `flat`.
//! - The fill extrusion shaders declare `gl_Position` invariant. Their vertex
//!   shaders draw the roofs, and the instanced ones the walls: an instance of
//!   a quad per outline vertex, as the WGSL does.
//! - The instanced fill outline shaders draw an outline where the context
//!   draws no 2 px lines: a quad per segment, two pixels wide as a wide line
//!   is, with the line shaders' fragment shaders.
//! - The fill, fill outline, fill extrusion and line pattern fragment shaders
//!   set `precision highp float` on GL ES, as GL JS's do: overzoomed past a
//!   source's maxzoom, their pattern positions outgrow a 16-bit mediump
//!   float (maplibre/maplibre-native#4951). The background pattern draws at
//!   the current zoom, so its positions stay small.
//! - The fill extrusion programs have the roofs' attributes, and the
//!   instanced ones, which the renderer draws the walls with, the instances'.
//!
//! The GLSL that `mln_glsl` parses:
//! - A fill extrusion wall reads its outline vertex and the next one as
//!   instance attributes. The pulled walls and outlines have no vertex
//!   inputs: they read the bucket's vertices from storage buffers in the
//!   drawable set, by `gl_VertexIndex`.
//! - The pattern rectangles reach the fragment stage flat.

/// The pattern helper `get_pattern_pos` in a shading language (`wgsl`,
/// `msl`, `glsl_es` or `glsl`), which a source `concat!`s before the text
/// that calls it: the position in a pattern of a tile position, from the
/// pattern's size and the tile's pixel coordinates in two 16-bit halves.
macro_rules! pattern_pos {
    (wgsl) => {
        r"
// Get pattern position for texture atlas lookups
fn get_pattern_pos(pixel_coord_upper: vec2<f32>, pixel_coord_lower: vec2<f32>,
                   pattern_size: vec2<f32>, tile_units_to_pixels: f32, pos: vec2<f32>) -> vec2<f32> {
    let offset_a = gl_mod(pixel_coord_upper, pattern_size) * 256.0;
    let offset_b = gl_mod(offset_a, pattern_size) * 256.0 + pixel_coord_lower;
    let offset = gl_mod(offset_b, pattern_size);
    return (tile_units_to_pixels * pos + offset) / pattern_size;
}
"
    };
    (msl) => {
        r"
// Unpack pattern position
inline float2 get_pattern_pos(const float2 pixel_coord_upper, const float2 pixel_coord_lower,
                     const float2 pattern_size, const float tile_units_to_pixels, const float2 pos) {
    const float2 offset = glMod(glMod(glMod(pixel_coord_upper, pattern_size) * 256.0, pattern_size) * 256.0 + pixel_coord_lower, pattern_size);
    return (tile_units_to_pixels * pos + offset) / pattern_size;
}
"
    };
    (glsl_es) => {
        r"
// At high zoom levels there are a ton of pixels between the world origin and the edge of the tile.
// The glsl spec only guarantees 16 bits of precision for highp floats. We need more than that.
//
// The pixel_coord is passed in as two 16 bit values:
// pixel_coord_upper = floor(pixel_coord / 2^16)
// pixel_coord_lower = mod(pixel_coord, 2^16)
//
// The offset is calculated in a series of steps that should preserve this precision:
vec2 get_pattern_pos(const vec2 pixel_coord_upper, const vec2 pixel_coord_lower,
    const vec2 pattern_size, const float tile_units_to_pixels, const vec2 pos) {

    vec2 offset = mod(mod(mod(pixel_coord_upper, pattern_size) * 256.0, pattern_size) * 256.0 + pixel_coord_lower, pattern_size);
    return (tile_units_to_pixels * pos + offset) / pattern_size;
}
"
    };
    (glsl) => {
        r"
// unpack pattern position
vec2 get_pattern_pos(const vec2 pixel_coord_upper, const vec2 pixel_coord_lower,
                        const vec2 pattern_size, const float tile_units_to_pixels, const vec2 pos) {
    const vec2 offset = mod(mod(mod(pixel_coord_upper, pattern_size) * 256.0, pattern_size) * 256.0 + pixel_coord_lower, pattern_size);
    return (tile_units_to_pixels * pos + offset) / pattern_size;
}
"
    };
}
pub(crate) use pattern_pos;

/// The atlas helper `atlas_pos` in a shading language (`wgsl`, `msl`,
/// `glsl_es` or `glsl`), which a source `concat!`s before the text that
/// calls it: an image's origin in an atlas plus an offset from it, both
/// normalized by a power-of-two atlas size, with the offset snapped to
/// 2^-24 so that the sum is exact.
macro_rules! atlas_pos {
    (wgsl) => {
        r"
// The atlas texture coordinate of an image's origin plus an offset from it,
// both normalized by a power-of-two atlas size. Snapping the offset to 2^-24
// makes the sum exact, so the sampled position, and the pixels, don't depend
// on where the image sits in the atlas. Adding an unsnapped offset rounds it
// to a precision that depends on the origin.
fn atlas_pos(origin: vec2<f32>, offset: vec2<f32>) -> vec2<f32> {
    return origin + round(offset * 16777216.0) / 16777216.0;
}
"
    };
    (msl) => {
        r"
// The atlas texture coordinate of an image's origin plus an offset from it,
// both normalized by a power-of-two atlas size. Snapping the offset to 2^-24
// makes the sum exact, so the sampled position, and the pixels, don't depend
// on where the image sits in the atlas. Adding an unsnapped offset rounds it
// to a precision that depends on the origin. `rint` rounds halves to even,
// as WGSL's `round` does.
inline float2 atlas_pos(const float2 origin, const float2 offset) {
    return origin + rint(offset * 16777216.0) / 16777216.0;
}
"
    };
    (glsl_es) => {
        r"
// The atlas texture coordinate of an image's origin plus an offset from it,
// both normalized by a power-of-two atlas size. Snapping the offset to 2^-24
// makes the sum exact, so the sampled position, and the pixels, don't depend
// on where the image sits in the atlas. Adding an unsnapped offset rounds it
// to a precision that depends on the origin.
highp vec2 atlas_pos(highp vec2 origin, highp vec2 offset) {
    return origin + roundEven(offset * 16777216.0) / 16777216.0;
}
"
    };
    (glsl) => {
        r"
// The atlas texture coordinate of an image's origin plus an offset from it,
// both normalized by a power-of-two atlas size. Snapping the offset to 2^-24
// makes the sum exact, so the sampled position, and the pixels, don't depend
// on where the image sits in the atlas. Adding an unsnapped offset rounds it
// to a precision that depends on the origin. `roundEven` rounds halves to
// even, as WGSL's `round` does.
vec2 atlas_pos(const vec2 origin, const vec2 offset) {
    return origin + roundEven(offset * 16777216.0) / 16777216.0;
}
"
    };
}
pub(crate) use atlas_pos;
