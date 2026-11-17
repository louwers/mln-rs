// What the collision debug programs share: the boxes' and circles' buffers,
// their perspective, and their colors.

import package::globals::paintParams;
import package::projection::ProjectionUBO;

struct CollisionDrawableUBO {
    matrix: mat4x4<f32>,
};

struct CollisionTilePropsUBO {
    extrude_scale: vec2<f32>,
    overscale_factor: f32,
    pad1: f32,
};

// ID_COLLISION_DRAWABLE_UBO
@group(2) @binding(2) var<uniform> drawable: CollisionDrawableUBO;
// ID_COLLISION_PROJECTION_UBO: the draw's projection.
@group(2) @binding(4) var<uniform> projection: ProjectionUBO;
// ID_COLLISION_TILE_PROPS_UBO
@group(2) @binding(5) var<uniform> tileProps: CollisionTilePropsUBO;

// The factor that scales a box or circle with its anchor's distance from
// the camera, `camera_to_anchor_distance` its projected anchor's w: clamped
// so that boxes near the camera in pitched or overzoomed tiles aren't
// oversized.
fn perspective_ratio(camera_to_anchor_distance: f32) -> f32 {
    return clamp(0.5 + 0.5 * (paintParams.camera_to_center_distance / camera_to_anchor_distance), 0.0, 4.0);
}

// The color of a box or circle: blue if its symbol is placed, red if it
// collided, faded by `unused_fade` if the symbol doesn't use it.
fn collision_color(placed: f32, not_used: f32, unused_fade: f32) -> vec4<f32> {
    let alpha = 0.5;
    var color = vec4<f32>(1.0, 0.0, 0.0, 1.0) * alpha;
    if (placed > 0.5) {
        color = vec4<f32>(0.0, 0.0, 1.0, 0.5) * alpha;
    }
    if (not_used > 0.5) {
        color *= unused_fade;
    }
    return color;
}
