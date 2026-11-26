//! The WGSL of the rasters' program.

use mln_gfx::shaders::source::WgslSource;

pub(crate) const RASTER: WgslSource = WgslSource {
    vertex: Some(
        r"
struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) pos0: vec3<f32>,
    @location(1) pos1: vec3<f32>,
};

@vertex
fn main(in: VertexInput) -> VertexOutput {
    var out: VertexOutput;
    let drawable = drawableVector[globalIndex.value];
    // The vertices span the unit square, which the warp puts on the quad
    // of the corners: blended from the projective warp towards the
    // bilinear one, whose homogeneous denominator is one at the top left.
    let fractional = vec2<f32>(f32(in.pos.x), f32(in.pos.y)) / 8192.0;
    let top_left = props.coords_top.xy;
    let top_right = props.coords_top.zw;
    let bottom_left = props.coords_bottom.xy;
    let bottom_right = props.coords_bottom.zw;
    let bilinear = mix(mix(top_left, top_right, fractional.x), mix(bottom_left, bottom_right, fractional.x), fractional.y);
    let denominator = dot(props.image_warp.xy, fractional) + 1.0;
    let across_top = top_right - top_left + props.image_warp.x * top_right;
    let down_left = bottom_left - top_left + props.image_warp.y * bottom_left;
    let projective = (across_top * fractional.x + down_left * fractional.y + top_left) / denominator;
    let pos = mix(projective, bilinear, props.image_warp.z);
    let clip = projectTileWithPoles(pos, pos, projectionVector[globalIndex.value]);
    out.position = clip;

    // The texture coordinates weighted by the reciprocal of the
    // denominator, blended as the position is, interpolate projectively.
    let weight = mix(1.0 / denominator, 1.0, props.image_warp.z);
    let tex = vec2<f32>(f32(in.texture_pos.x), f32(in.texture_pos.y));
    let pos0 = (((tex / 8192.0) - vec2<f32>(0.5, 0.5)) / props.buffer_scale) + vec2<f32>(0.5, 0.5);
    out.pos0 = vec3<f32>(pos0 * weight, weight);
    out.pos1 = vec3<f32>((pos0 * props.scale_parent + props.tl_parent) * weight, weight);

    return out;
}
",
    ),
    fragment: Some(
        r"
struct FragmentInput {
    @location(0) pos0: vec3<f32>,
    @location(1) pos1: vec3<f32>,
};

@fragment
fn main(in: FragmentInput) -> @location(0) vec4<f32> {
    var color0 = textureSample(image0, image0_sampler, in.pos0.xy / in.pos0.z);
    var color1 = textureSample(image1, image1_sampler, in.pos1.xy / in.pos1.z);

    if (color0.a > 0.0) {
        color0 = vec4<f32>(color0.rgb / color0.a, color0.a);
    }
    if (color1.a > 0.0) {
        color1 = vec4<f32>(color1.rgb / color1.a, color1.a);
    }

    var color = mix(color0, color1, props.fade_t);
    color.a = color.a * props.opacity;
    var rgb = color.rgb;

    let spin = props.spin_weights;
    rgb = vec3<f32>(dot(rgb, spin.xyz),
                    dot(rgb, spin.zxy),
                    dot(rgb, spin.yzx));

    let average = (color.r + color.g + color.b) / 3.0;
    rgb = rgb + (average - rgb) * props.saturation_factor;
    rgb = (rgb - vec3<f32>(0.5, 0.5, 0.5)) * props.contrast_factor + vec3<f32>(0.5, 0.5, 0.5);

    let high_vec = vec3<f32>(props.brightness_low, props.brightness_low, props.brightness_low);
    let low_vec = vec3<f32>(props.brightness_high, props.brightness_high, props.brightness_high);

    let final_rgb = mix(high_vec, low_vec, rgb) * color.a;
    return vec4<f32>(final_rgb, color.a);
}
",
    ),
    prelude: None,
};
