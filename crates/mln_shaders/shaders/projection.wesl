// Tile positions to clip space, in Mercator or on the globe
// (`PROJECTION_GLOBE`), and the per-drawable projection buffer.

import package::common::PI;
import package::globals::drawable_index;

struct ProjectionUBO {
    matrix: mat4x4<f32>,
    fallback_matrix: mat4x4<f32>,
    tile_mercator_coords: vec4<f32>,
    clipping_plane: vec4<f32>,
    projection_transition: f32,
    depth_offset: f32,
    translate: vec2<f32>,
};

// ID_PROJECTION_UBO: one element for each drawable of the layer.
@group(1) @binding(4) var<storage, read> projectionVector: array<ProjectionUBO>;

// The draw's projection.
fn load_projection() -> ProjectionUBO {
    return projectionVector[drawable_index()];
}

// Pole vertices carry these sentinel Y values in their raw position.
const GLOBE_POLE_NORTH_Y: f32 = -32767.5;
const GLOBE_POLE_SOUTH_Y: f32 = 32766.5;

const GLOBE_RADIUS: f32 = 6371008.8;

// Tile position (0..EXTENT) to a point on the unit sphere; the pole sentinels
// in rawPos map to the poles.
@if(PROJECTION_GLOBE)
fn projectToSphere(translatedPos: vec2<f32>, rawPos: vec2<f32>, projection: ProjectionUBO) -> vec3<f32> {
    let mercator_pos = projection.tile_mercator_coords.xy + projection.tile_mercator_coords.zw * translatedPos;
    let spherical_x = mercator_pos.x * PI * 2.0 + PI;
    // sin/cos of the latitude from the Mercator Y via the tangent half-angle
    // identities: no atan, and float32 precision survives near the equator.
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

@if(PROJECTION_GLOBE)
fn globeRotateVector(vec: vec3<f32>, angles: vec2<f32>) -> vec3<f32> {
    var axisRight = vec3<f32>(vec.z, 0.0, -vec.x);
    var axisUp = cross(axisRight, vec);
    axisRight = normalize(axisRight);
    axisUp = normalize(axisUp);
    let t = tan(angles);
    return normalize(vec + axisRight * t.x + axisUp * t.y);
}

// cos(latitude) at a tile Y, from the same exp() form as projectToSphere.
@if(PROJECTION_GLOBE)
fn circumferenceRatioAtTileY(tileY: f32, projection: ProjectionUBO) -> f32 {
    let mercator_pos_y = projection.tile_mercator_coords.y + projection.tile_mercator_coords.w * tileY;
    let t = exp(PI - (mercator_pos_y * PI * 2.0));
    return (2.0 * t) / (t * t + 1.0);
}

// How much wider than in Mercator a line is drawn at a tile Y.
@if(PROJECTION_GLOBE)
fn projectLineThickness(tileY: f32, projection: ProjectionUBO) -> f32 {
    let thickness = 1.0 / circumferenceRatioAtTileY(tileY, projection);
    if (projection.projection_transition < 0.999) {
        return mix(1.0, thickness, projection.projection_transition);
    }
    return thickness;
}
@else
fn projectLineThickness(tileY: f32, projection: ProjectionUBO) -> f32 {
    return 1.0;
}

@if(PROJECTION_GLOBE)
fn globeComputeClippingZ(spherePos: vec3<f32>, projection: ProjectionUBO) -> f32 {
    return 1.0 - (dot(spherePos, projection.clipping_plane.xyz) + projection.clipping_plane.w);
}

@if(PROJECTION_GLOBE)
fn interpolateProjection(posInTile: vec2<f32>, spherePos: vec3<f32>, elevation: f32, projection: ProjectionUBO) -> vec4<f32> {
    let elevatedPos = spherePos * (1.0 + elevation / GLOBE_RADIUS);
    var globePosition = projection.matrix * vec4<f32>(elevatedPos, 1.0);
    // Clip the far side of the globe through Z; the layer's depth shift keeps
    // layer order.
    globePosition.z = globeComputeClippingZ(elevatedPos, projection) * globePosition.w - projection.depth_offset;

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
@if(PROJECTION_GLOBE)
fn interpolateProjectionFor3D(posInTile: vec2<f32>, spherePos: vec3<f32>, elevation: f32, projection: ProjectionUBO) -> vec4<f32> {
    let elevatedPos = spherePos * (1.0 + elevation / GLOBE_RADIUS);
    let globePosition = projection.matrix * vec4<f32>(elevatedPos, 1.0);
    if (projection.projection_transition > 0.999) {
        return globePosition;
    }
    let fallbackPosition = projection.fallback_matrix * vec4<f32>(posInTile, elevation, 1.0);
    return mix(fallbackPosition, globePosition, projection.projection_transition);
}

@if(PROJECTION_GLOBE)
fn projectTile(pos: vec2<f32>, projection: ProjectionUBO) -> vec4<f32> {
    return interpolateProjection(pos, projectToSphere(pos + projection.translate, vec2<f32>(0.0, 0.0), projection), 0.0, projection);
}
@else
fn projectTile(pos: vec2<f32>, projection: ProjectionUBO) -> vec4<f32> {
    return projection.matrix * vec4<f32>(pos, 0.0, 1.0);
}

// The variant for geometry that can carry pole vertices; rawPos is the
// untranslated position. In Mercator, pole vertices go behind the near plane,
// which clips their triangles.
@if(PROJECTION_GLOBE)
fn projectTileWithPoles(pos: vec2<f32>, rawPos: vec2<f32>, projection: ProjectionUBO) -> vec4<f32> {
    return interpolateProjection(pos, projectToSphere(pos + projection.translate, rawPos, projection), 0.0, projection);
}
@else
fn projectTileWithPoles(pos: vec2<f32>, rawPos: vec2<f32>, projection: ProjectionUBO) -> vec4<f32> {
    var result = projection.matrix * vec4<f32>(pos, 0.0, 1.0);
    if (rawPos.y < GLOBE_POLE_NORTH_Y || rawPos.y > GLOBE_POLE_SOUTH_Y) {
        result.z = -10000000.0;
    }
    return result;
}

@if(PROJECTION_GLOBE)
fn projectTileWithElevation(pos: vec2<f32>, elevation: f32, projection: ProjectionUBO) -> vec4<f32> {
    return interpolateProjection(pos, projectToSphere(pos + projection.translate, vec2<f32>(0.0, 0.0), projection), elevation, projection);
}
@else
fn projectTileWithElevation(pos: vec2<f32>, elevation: f32, projection: ProjectionUBO) -> vec4<f32> {
    return projection.matrix * vec4<f32>(pos, elevation, 1.0);
}

@if(PROJECTION_GLOBE)
fn projectTileFor3D(pos: vec2<f32>, elevation: f32, projection: ProjectionUBO) -> vec4<f32> {
    return interpolateProjectionFor3D(pos, projectToSphere(pos + projection.translate, pos, projection), elevation, projection);
}
@else
fn projectTileFor3D(pos: vec2<f32>, elevation: f32, projection: ProjectionUBO) -> vec4<f32> {
    return projection.matrix * vec4<f32>(pos, elevation, 1.0);
}

// The zoom 0 tile's buffer wraps around the planet onto the tile itself, so
// the line shaders discard the fragments beyond its X extent; every other
// tile (a mercator width below the whole world's) hands on a value inside it.
@if(PROJECTION_GLOBE)
fn antimeridianClipX(posInTile: vec2<f32>, projection: ProjectionUBO) -> f32 {
    return select(0.0, posInTile.x, projection.tile_mercator_coords.z * 8192.0 >= 1.0);
}

// Whether a line fragment is beyond the zoom 0 tile's X extent; see
// antimeridianClipX.
@if(PROJECTION_GLOBE)
fn clippedAtAntimeridian(tileX: f32) -> bool {
    return tileX < 0.0 || tileX >= 8192.0;
}
