//! The pattern snippets that the background, fill and fill extrusion
//! programs share, in each language.

/// The WGSL snippets.
pub(crate) mod wgsl {
    /// `get_pattern_pos`, the position in a pattern of a point in a tile, for
    /// the programs that draw a pattern.
    macro_rules! pattern_pos {
        () => {
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
    }
    pub(crate) use pattern_pos;
}

/// The MSL snippets.
#[rustfmt::skip]
pub(crate) mod msl {
/// `get_pattern_pos`, the position in a pattern of a point in a tile, for
/// the programs that draw a pattern.
macro_rules! pattern_pos {
    () => {
        r"
// Unpack pattern position
inline float2 get_pattern_pos(const float2 pixel_coord_upper, const float2 pixel_coord_lower,
                     const float2 pattern_size, const float tile_units_to_pixels, const float2 pos) {
    const float2 offset = glMod(glMod(glMod(pixel_coord_upper, pattern_size) * 256.0, pattern_size) * 256.0 + pixel_coord_lower, pattern_size);
    return (tile_units_to_pixels * pos + offset) / pattern_size;
}
"
    };
}
pub(crate) use pattern_pos;
}

/// The GLSL, for Vulkan and D3D12 snippets.
#[rustfmt::skip]
pub(crate) mod glsl {
/// `get_pattern_pos`, the position in a pattern of a point in a tile, for
/// the programs that draw a pattern.
macro_rules! pattern_pos {
    () => {
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
}

/// The GLSL ES, for OpenGL snippets.
#[rustfmt::skip]
pub(crate) mod glsl_es {
/// `get_pattern_pos`, the position in a pattern of a point in a tile, for
/// the programs that draw a pattern.
macro_rules! pattern_pos {
    () => {
        r"
// The offset depends on how many pixels are between the world origin and the edge of the tile:
// vec2 offset = mod(pixel_coord, size)
//
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
}
pub(crate) use pattern_pos;
}
