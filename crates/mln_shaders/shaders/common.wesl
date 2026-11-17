// Constants and helpers for every program: GLSL's `mod`, the packing of
// paint properties into vertex attributes, and pattern and atlas positions.

const PI: f32 = 3.14159265358979323846;

// The maximum allowed miter limit is 2.0 at the moment. The extrude normal is
// stored in a byte (-128..127). Regular normals are scaled up to length 63,
// "special" normals up to 126.
const LINE_NORMAL_SCALE: f32 = 1.0 / 63.0;

// The attribute conveying progress along a line is scaled to [0, 2^15).
const MAX_LINE_DISTANCE: f32 = 32767.0;

const SDF_PX: f32 = 8.0;

// GLSL's `mod`: `x - y * floor(x / y)`, where WGSL's `%` truncates.
fn glMod(x: f32, y: f32) -> f32 {
    return x - y * floor(x / y);
}

fn glMod2(x: vec2<f32>, y: f32) -> vec2<f32> {
    return x - y * floor(x / y);
}

fn glMod2v(x: vec2<f32>, y: vec2<f32>) -> vec2<f32> {
    return x - y * floor(x / y);
}

// A pair of 8-bit unsigned integers packed into one float as
// `floor(input[0]) * 256 + input[1]`.
fn unpack_float(packedValue: f32) -> vec2<f32> {
    let packedIntValue = i32(packedValue);
    let v0 = packedIntValue / 256;
    return vec2<f32>(f32(v0), f32(packedIntValue - v0 * 256));
}

fn unpack_opacity(packedOpacity: f32) -> vec2<f32> {
    let intOpacity = i32(packedOpacity) / 2;
    return vec2<f32>(f32(intOpacity) / 127.0, glMod(packedOpacity, 2.0));
}

// A color packed into two floats:
// [ floor(color.r * 255) * 256 + color.g * 255,
//   floor(color.b * 255) * 256 + color.a * 255 ]
fn decode_color(encoded: vec2<f32>) -> vec4<f32> {
    let e0 = unpack_float(encoded[0]) / 255.0;
    let e1 = unpack_float(encoded[1]) / 255.0;
    return vec4<f32>(e0.x, e0.y, e1.x, e1.y);
}

// A paint property's value at the zoom between its two stops, `t` of the way.
fn unpack_mix_float(packedValue: vec2<f32>, t: f32) -> f32 {
    return mix(packedValue[0], packedValue[1], t);
}

// A color paint property's value at the zoom between its two stops, packed
// as two `decode_color` pairs.
fn unpack_mix_color(packedColors: vec4<f32>, t: f32) -> vec4<f32> {
    let minColor = decode_color(vec2<f32>(packedColors[0], packedColors[1]));
    let maxColor = decode_color(vec2<f32>(packedColors[2], packedColors[3]));
    return mix(minColor, maxColor, t);
}

// The two 16-bit halves of an integer, the low one signed.
fn unpack_int(value: i32) -> vec2<f32> {
    let low = (value << 16u) >> 16u;
    let high = value >> 16u;
    return vec2<f32>(f32(low), f32(high));
}

// The two 16-bit halves of an unsigned integer.
fn unpack_uint(value: u32) -> vec2<f32> {
    let low = value & 0xFFFFu;
    let high = (value >> 16u) & 0xFFFFu;
    return vec2<f32>(f32(low), f32(high));
}

// A pattern's position at `pos`. The pixel coordinate of the tile's corner is
// two 16-bit values, `pixel_coord_upper = floor(pixel_coord / 2^16)` and
// `pixel_coord_lower = mod(pixel_coord, 2^16)`, which keep more precision
// than one float would at high zoom levels.
fn get_pattern_pos(pixel_coord_upper: vec2<f32>, pixel_coord_lower: vec2<f32>,
                   pattern_size: vec2<f32>, tile_units_to_pixels: f32, pos: vec2<f32>) -> vec2<f32> {
    let offset_a = glMod2v(pixel_coord_upper, pattern_size) * 256.0;
    let offset_b = glMod2v(offset_a, pattern_size) * 256.0 + pixel_coord_lower;
    let offset = glMod2v(offset_b, pattern_size);
    return (tile_units_to_pixels * pos + offset) / pattern_size;
}

// The atlas texture coordinate of an image's origin plus an offset from it,
// both normalized by a power-of-two atlas size. Snapping the offset to 2^-24
// makes the sum exact, so the sampled position, and the pixels, don't depend
// on where the image sits in the atlas. Adding an unsnapped offset rounds it
// to a precision that depends on the origin. `round` rounds halves to even,
// in every backend's language.
fn atlas_pos(origin: vec2<f32>, offset: vec2<f32>) -> vec2<f32> {
    return origin + round(offset * 16777216.0) / 16777216.0;
}
