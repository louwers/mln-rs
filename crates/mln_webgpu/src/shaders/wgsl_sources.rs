//! The WGSL prelude every shader compiles after, and the declarations of
//! the buffers and textures that a stage reads.

use mln_gfx::gfx_types::AttributeDataType;
use mln_gfx::shaders::program::{Buffer, BufferKind, DRAW_PROJECTION_BUFFER, PROJECTION_BUFFER, Texture, VIEW_BUFFER};

/// The WGSL every program compiles after.
pub(crate) const PRELUDE: &str = r"
// Constants
const PI: f32 = 3.14159265358979323846;

// WebGPU equivalent of GLSL mod function
fn glMod(x: f32, y: f32) -> f32 {
    return x - y * floor(x / y);
}

fn glMod2v(x: vec2<f32>, y: vec2<f32>) -> vec2<f32> {
    return x - y * floor(x / y);
}

// GLSL compatibility helpers
fn gl_mod(x: vec2<f32>, y: vec2<f32>) -> vec2<f32> {
    return glMod2v(x, y);
}

// Helper functions for unpacking data
fn unpack_float(packedValue: f32) -> vec2<f32> {
    let packedIntValue = i32(packedValue);
    let v0 = packedIntValue / 256;
    return vec2<f32>(f32(v0), f32(packedIntValue - v0 * 256));
}

// Decode a color that has been packed into two floats
fn decode_color(encoded: vec2<f32>) -> vec4<f32> {
    let e0 = unpack_float(encoded[0]) / 255.0;
    let e1 = unpack_float(encoded[1]) / 255.0;
    return vec4<f32>(e0.x, e0.y, e1.x, e1.y);
}

// Unpack and interpolate between two float values
fn unpack_mix_float(packedValue: vec2<f32>, t: f32) -> f32 {
    return mix(packedValue[0], packedValue[1], t);
}

// Unpack and interpolate between two colors
fn unpack_mix_color(packedColors: vec4<f32>, t: f32) -> vec4<f32> {
    let minColor = decode_color(vec2<f32>(packedColors[0], packedColors[1]));
    let maxColor = decode_color(vec2<f32>(packedColors[2], packedColors[3]));
    return mix(minColor, maxColor, t);
}

// The draw's index into the storage buffers it reads, at `DRAW_INDEX_ID`.
struct GlobalIndexUBO {
    value: u32,
    pad0: vec3<u32>,
};

// Pole vertices carry these sentinel Y values in their raw position.
const GLOBE_POLE_NORTH_Y: f32 = -32767.5;
const GLOBE_POLE_SOUTH_Y: f32 = 32766.5;

#ifdef PROJECTION_GLOBE

const GLOBE_RADIUS: f32 = 6371008.8;

// Tile position (0..EXTENT) to a point on the unit sphere; the pole sentinels in rawPos map to the poles.
fn projectToSphere(translatedPos: vec2<f32>, rawPos: vec2<f32>, projection: ProjectionUBO) -> vec3<f32> {
    let mercator_pos = projection.tile_mercator_coords.xy + projection.tile_mercator_coords.zw * translatedPos;
    let spherical_x = mercator_pos.x * PI * 2.0 + PI;
    // sin/cos of the latitude from the Mercator Y via the tangent half-angle identities: no atan, and float32 precision survives near the equator.
    let t = exp(PI - (mercator_pos.y * PI * 2.0));
    let t2 = t * t;
    let denom = t2 + 1.0;
    let sin_sy = (t2 - 1.0) / denom;
    let cos_sy = (2.0 * t) / denom;
    var pos = vec3<f32>(sin(spherical_x) * cos_sy, sin_sy, cos(spherical_x) * cos_sy);
    if (rawPos.y < GLOBE_POLE_NORTH_Y) {
        pos = vec3<f32>(0.0, 1.0, 0.0);
    }
    if (rawPos.y > GLOBE_POLE_SOUTH_Y) {
        pos = vec3<f32>(0.0, -1.0, 0.0);
    }
    return pos;
}

fn globeRotateVector(vec: vec3<f32>, angles: vec2<f32>) -> vec3<f32> {
    var axisRight = vec3<f32>(vec.z, 0.0, -vec.x);
    var axisUp = cross(axisRight, vec);
    axisRight = normalize(axisRight);
    axisUp = normalize(axisUp);
    let t = tan(angles);
    return normalize(vec + axisRight * t.x + axisUp * t.y);
}

// cos(latitude) at a tile Y, from the same exp() form as projectToSphere.
fn circumferenceRatioAtTileY(tileY: f32, projection: ProjectionUBO) -> f32 {
    let mercator_pos_y = projection.tile_mercator_coords.y + projection.tile_mercator_coords.w * tileY;
    let t = exp(PI - (mercator_pos_y * PI * 2.0));
    return (2.0 * t) / (t * t + 1.0);
}

fn globeComputeClippingZ(spherePos: vec3<f32>, projection: ProjectionUBO) -> f32 {
    return 1.0 - (dot(spherePos, projection.clipping_plane.xyz) + projection.clipping_plane.w);
}

fn interpolateProjection(posInTile: vec2<f32>, spherePos: vec3<f32>, elevation: f32, projection: ProjectionUBO) -> vec4<f32> {
    let elevatedPos = spherePos * (1.0 + elevation / GLOBE_RADIUS);
    var globePosition = projection.matrix * vec4<f32>(elevatedPos, 1.0);
    // Clip the far side of the globe through Z; the layer's depth shift keeps layer order.
    globePosition.z = globeComputeClippingZ(elevatedPos, projection) * globePosition.w - projection.depth_offset;

    if (projection.transition > 0.999) {
        return globePosition;
    }

    let flatPosition = projection.fallback_matrix * vec4<f32>(posInTile, elevation, 1.0);
    let z_globeness_threshold = 0.2;
    var result = globePosition;
    result.z = mix(0.0, globePosition.z, clamp((projection.transition - z_globeness_threshold) / (1.0 - z_globeness_threshold), 0.0, 1.0));
    let xyw = mix(flatPosition.xyw, globePosition.xyw, projection.transition);
    result.x = xyw.x;
    result.y = xyw.y;
    result.w = xyw.z;
    if ((posInTile.y < GLOBE_POLE_NORTH_Y) || (posInTile.y > GLOBE_POLE_SOUTH_Y)) {
        result = globePosition;
        let poles_hidden_anim_percentage = 0.02;
        result.z = mix(globePosition.z, 100.0, pow(max((1.0 - projection.transition) / poles_hidden_anim_percentage, 0.0), 8.0));
    }
    return result;
}

// Keeps the matrix Z, for geometry that needs the depth buffer.
fn interpolateProjectionFor3D(posInTile: vec2<f32>, spherePos: vec3<f32>, elevation: f32, projection: ProjectionUBO) -> vec4<f32> {
    let elevatedPos = spherePos * (1.0 + elevation / GLOBE_RADIUS);
    let globePosition = projection.matrix * vec4<f32>(elevatedPos, 1.0);
    if (projection.transition > 0.999) {
        return globePosition;
    }
    let fallbackPosition = projection.fallback_matrix * vec4<f32>(posInTile, elevation, 1.0);
    return mix(fallbackPosition, globePosition, projection.transition);
}

fn projectTile(pos: vec2<f32>, projection: ProjectionUBO) -> vec4<f32> {
    return interpolateProjection(pos, projectToSphere(pos + projection.translate, vec2<f32>(0.0, 0.0), projection), 0.0, projection);
}

// The variant for geometry that can carry pole vertices; rawPos is the untranslated position.
fn projectTileWithPoles(pos: vec2<f32>, rawPos: vec2<f32>, projection: ProjectionUBO) -> vec4<f32> {
    return interpolateProjection(pos, projectToSphere(pos + projection.translate, rawPos, projection), 0.0, projection);
}

fn projectTileWithElevation(pos: vec2<f32>, elevation: f32, projection: ProjectionUBO) -> vec4<f32> {
    return interpolateProjection(pos, projectToSphere(pos + projection.translate, vec2<f32>(0.0, 0.0), projection), elevation, projection);
}

fn projectTileFor3D(pos: vec2<f32>, elevation: f32, projection: ProjectionUBO) -> vec4<f32> {
    return interpolateProjectionFor3D(pos, projectToSphere(pos + projection.translate, pos, projection), elevation, projection);
}

#else

fn projectTile(pos: vec2<f32>, projection: ProjectionUBO) -> vec4<f32> {
    return projection.matrix * vec4<f32>(pos, 0.0, 1.0);
}

// Pole vertices only exist on the globe; put them behind the near plane so their triangles are clipped.
fn projectTileWithPoles(pos: vec2<f32>, rawPos: vec2<f32>, projection: ProjectionUBO) -> vec4<f32> {
    var result = projection.matrix * vec4<f32>(pos, 0.0, 1.0);
    if (rawPos.y < GLOBE_POLE_NORTH_Y || rawPos.y > GLOBE_POLE_SOUTH_Y) {
        result.z = -10000000.0;
    }
    return result;
}

fn projectTileWithElevation(pos: vec2<f32>, elevation: f32, projection: ProjectionUBO) -> vec4<f32> {
    return projection.matrix * vec4<f32>(pos, elevation, 1.0);
}

fn projectTileFor3D(pos: vec2<f32>, elevation: f32, projection: ProjectionUBO) -> vec4<f32> {
    return projection.matrix * vec4<f32>(pos, elevation, 1.0);
}

#endif

";

/// The declaration of the draw's index into the storage buffers it reads,
/// which a stage reads as `globalIndex`.
pub(crate) const GLOBAL_INDEX_DECLARATION: &str = "@group(0) @binding(1) var<uniform> globalIndex: GlobalIndexUBO;";

/// The engine's buffers that a stage may read without declaring them,
/// besides `globalIndex`: the view's block and the projection blocks.
pub(crate) const ENGINE_BINDINGS: [Buffer; 3] = [VIEW_BUFFER, PROJECTION_BUFFER, DRAW_PROJECTION_BUFFER];

/// The declaration of `buffer` at the binding of its id in group 0, by its
/// name: a block as a uniform, and an array of the elements or the vertex
/// words of a storage buffer.
pub(crate) fn declaration(buffer: &Buffer) -> String {
    let (space, ty) = match buffer.kind {
        BufferKind::Uniform(block) | BufferKind::DrawUniform(block) => ("uniform", block.name.to_owned()),
        BufferKind::Storage { block, .. } => ("storage, read", format!("array<{}>", block.name)),
        BufferKind::Vertices { format } => ("storage, read", format!("array<{}>", word(format))),
    };
    format!("@group(0) @binding({}) var<{space}> {}: {ty};", buffer.id, buffer.name)
}

/// The group that textures and their samplers bind in.
pub(crate) const TEXTURE_GROUP: u32 = 1;

/// The binding in [`TEXTURE_GROUP`] of the texture at slot `slot`, and of
/// its sampler, at the binding before it.
pub(crate) const fn texture_bindings(slot: usize) -> [u32; 2] {
    let sampler = 2 * slot as u32;
    [sampler + 1, sampler]
}

/// The texture slot of `binding` in [`TEXTURE_GROUP`], and whether it is
/// the sampler's binding: the inverse of [`texture_bindings`].
pub(crate) const fn texture_slot(binding: u32) -> (usize, bool) {
    ((binding / 2) as usize, binding.is_multiple_of(2))
}

/// The declarations of `texture`, at slot `slot`, and of its sampler, by
/// their names.
pub(crate) fn texture_declarations(slot: usize, texture: &Texture) -> [String; 2] {
    let [texture_binding, sampler_binding] = texture_bindings(slot);
    [
        format!("@group({TEXTURE_GROUP}) @binding({texture_binding}) var {}: texture_2d<f32>;", texture.name),
        format!("@group({TEXTURE_GROUP}) @binding({sampler_binding}) var {}: sampler;", texture.sampler()),
    ]
}

/// The 32-bit words that a stage reads vertices of `format` as.
fn word(format: AttributeDataType) -> &'static str {
    use AttributeDataType::{Float, Float2, Float3, Float4, Int, Int2, Int3, Int4};
    match format {
        Float | Float2 | Float3 | Float4 => "f32",
        Int | Int2 | Int3 | Int4 => "i32",
        _ => "u32",
    }
}
