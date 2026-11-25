//! The engine's WGSL helpers, which every program's stages start with.

/// The start of every program's stages: the engine's helpers, which any
/// program can use.
pub(crate) const PRELUDE: &str = r"
// Constants
const PI: f32 = 3.14159265358979323846;

// WebGPU equivalent of GLSL mod function
fn glMod(x: f32, y: f32) -> f32 {
    return x - y * floor(x / y);
}

fn glMod2(x: vec2<f32>, y: f32) -> vec2<f32> {
    return x - y * floor(x / y);
}

fn glMod2v(x: vec2<f32>, y: vec2<f32>) -> vec2<f32> {
    return x - y * floor(x / y);
}

// GLSL compatibility helpers
fn gl_mod(x: vec2<f32>, y: vec2<f32>) -> vec2<f32> {
    return glMod2v(x, y);
}

// Radians conversion
fn radians(degrees: f32) -> f32 {
    return PI * degrees / 180.0;
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

// Pole vertices carry these sentinel Y values in their raw position.
const GLOBE_POLE_NORTH_Y: f32 = -32767.5;
const GLOBE_POLE_SOUTH_Y: f32 = 32766.5;

#ifdef PROJECTION_GLOBE

const GLOBE_RADIUS: f32 = 6371008.8;
// The tile's anchor, its middle, in tile units.
const GLOBE_ANCHOR: f32 = 4096.0;

// On the globe, points on the sphere are offsets from the tile's anchor, which the projection matrix carries, so float32
// keeps them exact close in. The small-angle functions below are exact to float32 where the GPU's builtins may only be
// close in absolute terms.

// e^x - 1.
fn globeExpm1(x: f32) -> f32 {
    if (abs(x) < 0.01) {
        return x * (1.0 + x * (0.5 + x * (1.0 / 6.0 + x * (1.0 / 24.0))));
    }
    return exp(x) - 1.0;
}

fn globeAtan(x: f32) -> f32 {
    let x2 = x * x;
    if (abs(x) < 0.05) {
        return x * (1.0 - x2 * (1.0 / 3.0 - x2 * (0.2 - x2 / 7.0)));
    }
    return atan(x);
}

fn globeTan(a: f32) -> f32 {
    let a2 = a * a;
    if (abs(a) < 0.1) {
        return a * (1.0 + a2 * (1.0 / 3.0 + a2 * (2.0 / 15.0)));
    }
    return tan(a);
}

// The change in the sine and cosine of an angle whose sine and cosine are `sc` when it turns by `a`.
fn globeTurn(sc: vec2<f32>, a: f32) -> vec2<f32> {
    let a2 = a * a;
    var sin_a = a * (1.0 - a2 / 6.0 * (1.0 - a2 / 20.0));
    // 1 - cos(a)
    var versin_a = a2 * 0.5 * (1.0 - a2 / 12.0 * (1.0 - a2 / 30.0));
    if (abs(a) >= 0.1) {
        let half_sin = sin(a * 0.5);
        sin_a = sin(a);
        versin_a = 2.0 * half_sin * half_sin;
    }
    return vec2<f32>(sc.y * sin_a - sc.x * versin_a, -sc.x * sin_a - sc.y * versin_a);
}

// Tile position (0..EXTENT) to a point on the unit sphere, as its offset from the tile's anchor. The offset is built from
// the position's distance from the anchor alone, so float32 rounds it to its own size, a tile's, not the sphere's. The
// pole sentinels in rawPos map to the poles.
fn projectToSphere(translatedPos: vec2<f32>, rawPos: vec2<f32>, projection: ProjectionUBO) -> vec3<f32> {
    let d = translatedPos - GLOBE_ANCHOR;
    let anchor = projection.anchor_angles;
    let d_lng = d.x * projection.tile_mercator_coords.z * (2.0 * PI);
    // The latitude is 2 atan(e^m) - pi/2 of the Mercator exponent m = pi - 2 pi y; its change from the anchor's, as one
    // atan of e^(m - m_anchor) - 1.
    let e = globeExpm1(d.y * projection.tile_mercator_coords.w * (-2.0 * PI));
    let d_lat = 2.0 * globeAtan(anchor.w * e / (2.0 + (1.0 + anchor.z) * e));
    let lng = globeTurn(anchor.xy, d_lng);
    let lat = globeTurn(anchor.zw, d_lat);
    let cos_lat = anchor.w + lat.y;
    var offset = vec3<f32>(lng.x * cos_lat + anchor.x * lat.y, lat.x, lng.y * cos_lat + anchor.y * lat.y);
    if (rawPos.y < GLOBE_POLE_NORTH_Y) {
        offset = vec3<f32>(0.0, 1.0, 0.0) - projection.anchor.xyz;
    }
    if (rawPos.y > GLOBE_POLE_SOUTH_Y) {
        offset = vec3<f32>(0.0, -1.0, 0.0) - projection.anchor.xyz;
    }
    return offset;
}

// The point `angles` (radians right and up) from a point on the unit sphere, both as offsets from the tile's anchor.
fn globeRotateVector(offset: vec3<f32>, angles: vec2<f32>, projection: ProjectionUBO) -> vec3<f32> {
    let vec = projection.anchor.xyz + offset;
    var axisRight = vec3<f32>(vec.z, 0.0, -vec.x);
    var axisUp = cross(axisRight, vec);
    axisRight = normalize(axisRight);
    axisUp = normalize(axisUp);
    let v = axisRight * globeTan(angles.x) + axisUp * globeTan(angles.y);
    // normalize(vec + v) - vec for a unit vec, without the cancellation.
    let v2 = dot(v, v);
    let len = sqrt(1.0 + v2);
    return offset + (v - vec * (v2 / (len + 1.0))) / len;
}

// cos(latitude) at a tile Y, from the same exp() form as projectToSphere.
fn circumferenceRatioAtTileY(tileY: f32, projection: ProjectionUBO) -> f32 {
    let mercator_pos_y = projection.tile_mercator_coords.y + projection.tile_mercator_coords.w * tileY;
    let t = exp(PI - (mercator_pos_y * PI * 2.0));
    return (2.0 * t) / (t * t + 1.0);
}

fn projectLineThickness(tileY: f32, projection: ProjectionUBO) -> f32 {
    let thickness = 1.0 / circumferenceRatioAtTileY(tileY, projection);
    if (projection.projection_transition < 0.999) {
        return mix(1.0, thickness, projection.projection_transition);
    }
    return thickness;
}

// The clipping plane's value at a point offset from the tile's anchor, off 1.
fn globeComputeClippingZ(offset: vec3<f32>, projection: ProjectionUBO) -> f32 {
    return 1.0 - (dot(offset, projection.clipping_plane.xyz) + projection.anchor.w);
}

// A point on the unit sphere, offset from the tile's anchor, raised by `elevation` meters.
fn globeElevate(offset: vec3<f32>, elevation: f32, projection: ProjectionUBO) -> vec3<f32> {
    return offset + (projection.anchor.xyz + offset) * (elevation / GLOBE_RADIUS);
}

// sphereOffset is projectToSphere's.
fn interpolateProjection(posInTile: vec2<f32>, sphereOffset: vec3<f32>, elevation: f32, projection: ProjectionUBO) -> vec4<f32> {
    let elevatedOffset = globeElevate(sphereOffset, elevation, projection);
    var globePosition = projection.matrix * vec4<f32>(elevatedOffset, 1.0);
    // Clip the far side of the globe through Z; the layer's depth shift keeps layer order.
    globePosition.z = globeComputeClippingZ(elevatedOffset, projection) * globePosition.w - projection.depth_offset;

    if (projection.projection_transition > 0.999) {
        return globePosition;
    }

    let flatPosition = projection.fallback_matrix * vec4<f32>(posInTile, elevation, 1.0);
    let z_globeness_threshold = 0.2;
    var result = globePosition;
    result.z = mix(0.0, globePosition.z, clamp((projection.projection_transition - z_globeness_threshold) / (1.0 - z_globeness_threshold), 0.0, 1.0));
    let xyw = mix(flatPosition.xyw, globePosition.xyw, projection.projection_transition);
    result.x = xyw.x;
    result.y = xyw.y;
    result.w = xyw.z;
    if ((posInTile.y < GLOBE_POLE_NORTH_Y) || (posInTile.y > GLOBE_POLE_SOUTH_Y)) {
        result = globePosition;
        let poles_hidden_anim_percentage = 0.02;
        result.z = mix(globePosition.z, 100.0, pow(max((1.0 - projection.projection_transition) / poles_hidden_anim_percentage, 0.0), 8.0));
    }
    return result;
}

// Keeps the matrix Z, for geometry that needs the depth buffer.
fn interpolateProjectionFor3D(posInTile: vec2<f32>, sphereOffset: vec3<f32>, elevation: f32, projection: ProjectionUBO) -> vec4<f32> {
    let globePosition = projection.matrix * vec4<f32>(globeElevate(sphereOffset, elevation, projection), 1.0);
    if (projection.projection_transition > 0.999) {
        return globePosition;
    }
    let fallbackPosition = projection.fallback_matrix * vec4<f32>(posInTile, elevation, 1.0);
    return mix(fallbackPosition, globePosition, projection.projection_transition);
}

fn projectTile(pos: vec2<f32>, projection: ProjectionUBO) -> vec4<f32> {
    return interpolateProjection(pos, projectToSphere(pos + projection.translate, vec2<f32>(0.0, 0.0), projection), 0.0, projection);
}

// The zoom 0 tile's buffer wraps around the planet onto the tile itself, so the line shaders discard the fragments
// beyond its X extent; every other tile (a mercator width below the whole world's) hands on a value inside it.
fn antimeridianClipX(posInTile: vec2<f32>, projection: ProjectionUBO) -> f32 {
    return select(0.0, posInTile.x, projection.tile_mercator_coords.z * 8192.0 >= 1.0);
}

fn clippedAtAntimeridian(tileX: f32) -> bool {
    return tileX < 0.0 || tileX >= 8192.0;
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

fn projectLineThickness(tileY: f32, projection: ProjectionUBO) -> f32 {
    return 1.0;
}

#endif

// The atlas texture coordinate of an image's origin plus an offset from it,
// both normalized by a power-of-two atlas size. Snapping the offset to 2^-24
// makes the sum exact, so the sampled position, and the pixels, don't depend
// on where the image sits in the atlas. Adding an unsnapped offset rounds it
// to a precision that depends on the origin.
fn atlas_pos(origin: vec2<f32>, offset: vec2<f32>) -> vec2<f32> {
    return origin + round(offset * 16777216.0) / 16777216.0;
}
";
