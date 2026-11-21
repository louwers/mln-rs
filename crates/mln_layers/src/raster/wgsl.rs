//! The programs of the `raster` layer type in WGSL, for `mln_webgpu`.

use mln_gfx::shaders::source::wgsl::Wgsl;

pub(super) const RASTER_SHADER: Wgsl = Wgsl {
    vertex: r"
struct VertexInput {
    @location(0) position: vec2<i32>,
    @location(1) texcoord: vec2<i32>,
};

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) pos0: vec2<f32>,
    @location(1) pos1: vec2<f32>,
};

@group(0) @binding(2) var<storage, read> drawableVector: array<RasterDrawableUBO>;
@group(0) @binding(5) var<uniform> props: RasterEvaluatedPropsUBO;

@vertex
fn main(in: VertexInput) -> VertexOutput {
    var out: VertexOutput;
    let drawable = drawableVector[globalIndex.value];
    let pos = vec2<f32>(f32(in.position.x), f32(in.position.y));
    let clip = projectTileWithPoles(pos, pos, projectionVector[globalIndex.value]);
    out.position = clip;

    let tex = vec2<f32>(f32(in.texcoord.x), f32(in.texcoord.y));
    let pos0 = (((tex / 8192.0) - vec2<f32>(0.5, 0.5)) / props.buffer_scale) + vec2<f32>(0.5, 0.5);
    out.pos0 = pos0;
    out.pos1 = pos0 * props.scale_parent + props.tl_parent;

    return out;
}
",
    fragment: r"
struct FragmentInput {
    @location(0) pos0: vec2<f32>,
    @location(1) pos1: vec2<f32>,
};

@group(0) @binding(5) var<uniform> props: RasterEvaluatedPropsUBO;
@group(1) @binding(0) var texture_sampler: sampler;
@group(1) @binding(1) var image0: texture_2d<f32>;
@group(1) @binding(2) var image1: texture_2d<f32>;

@fragment
fn main(in: FragmentInput) -> @location(0) vec4<f32> {
#ifdef OVERDRAW_INSPECTOR
    return vec4<f32>(1.0, 1.0, 1.0, 1.0);
#endif

    var color0 = textureSample(image0, texture_sampler, in.pos0);
    var color1 = textureSample(image1, texture_sampler, in.pos1);

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
    prelude: "",
};
