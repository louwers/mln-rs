// The sky: a full-screen quad at the far plane, colored above the horizon
// and blended into the horizon's color near it.

struct SkyPropsUBO {
    sky_color: vec4<f32>,
    horizon_color: vec4<f32>,
    // A point of the horizon and its normal, in pixels.
    horizon: vec2<f32>,
    horizon_normal: vec2<f32>,
    viewport_size: vec2<f32>,
    sky_horizon_blend: f32,
    sky_blend: f32,
};

// ID_SKY_PROPS_UBO
@group(1) @binding(5) var<uniform> sky: SkyPropsUBO;

// The location is ID_SKY_POS_VERTEX_ATTRIBUTE: a corner in clip space.
struct VertexInput {
    @location(0) position: vec2<i32>,
};

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) pos: vec2<f32>,
};

@vertex
fn vs_main(in: VertexInput) -> VertexOutput {
    var out: VertexOutput;
    let pos = vec2<f32>(in.position);
    out.position = vec4<f32>(pos, 1.0, 1.0);
    out.pos = pos;
    return out;
}

@fragment
fn fs_main(@location(0) pos: vec2<f32>) -> @location(0) vec4<f32> {
    let pixel = (pos * 0.5 + 0.5) * sky.viewport_size;
    let distance_to_horizon = dot(pixel - sky.horizon, sky.horizon_normal);
    var color = vec4<f32>(0.0);
    if (distance_to_horizon > 0.0) {
        if (sky.sky_horizon_blend > 0.0 && distance_to_horizon < sky.sky_horizon_blend) {
            let blend = 1.0 - distance_to_horizon / sky.sky_horizon_blend;
            color = mix(sky.sky_color, sky.horizon_color, blend * blend);
        } else {
            color = sky.sky_color;
        }
    }
    return color * (1.0 - sky.sky_blend);
}
