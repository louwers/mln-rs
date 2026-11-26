//! The GLSL, for Vulkan and D3D12, of the rasters' program.

use mln_gfx::shaders::source::GlslSource;

pub(crate) const RASTER: GlslSource = GlslSource {
    vertex: r"

layout(location = 0) out vec3 frag_position0;
layout(location = 1) out vec3 frag_position1;

void main() {
    const RasterDrawableUBO drawable = drawableVector.elements[constant.ubo_index];

    // The vertices span the unit square, which the warp puts on the quad
    // of the corners: blended from the projective warp towards the
    // bilinear one, whose homogeneous denominator is one at the top left.
    vec2 fractional = vec2(in_pos) / 8192.0;
    vec2 top_left = props.coords_top.xy;
    vec2 top_right = props.coords_top.zw;
    vec2 bottom_left = props.coords_bottom.xy;
    vec2 bottom_right = props.coords_bottom.zw;
    vec2 bilinear = mix(mix(top_left, top_right, fractional.x), mix(bottom_left, bottom_right, fractional.x), fractional.y);
    float denominator = dot(props.image_warp.xy, fractional) + 1.0;
    vec2 across_top = top_right - top_left + props.image_warp.x * top_right;
    vec2 down_left = bottom_left - top_left + props.image_warp.y * bottom_left;
    vec2 projective = (across_top * fractional.x + down_left * fractional.y + top_left) / denominator;
    vec2 position = mix(projective, bilinear, props.image_warp.z);
    gl_Position = projectTile(position, position, projectionVector.elements[constant.ubo_index]);

    // We are using Int16 for texture position coordinates to give us enough precision for
    // fractional coordinates. We use 8192 to scale the texture coordinates in the buffer
    // as an arbitrarily high number to preserve adequate precision when rendering.
    // This is also the same value as the EXTENT we are using for our tile buffer pos coordinates,
    // so math for modifying either is consistent.
    vec2 position0 = (((in_texture_pos / 8192.0) - 0.5) / props.buffer_scale ) + 0.5;
    vec2 position1 = (position0 * props.scale_parent) + props.tl_parent;
    // The texture coordinates weighted by the reciprocal of the
    // denominator, blended as the position is, interpolate projectively.
    float weight = mix(1.0 / denominator, 1.0, props.image_warp.z);
    frag_position0 = vec3(position0 * weight, weight);
    frag_position1 = vec3(position1 * weight, weight);
}
",
    fragment: r"

layout(location = 0) in vec3 frag_position0;
layout(location = 1) in vec3 frag_position1;

layout(location = 0) out vec4 out_color;

void main() {

    // read and cross-fade colors from the main and parent tiles
    vec4 color0 = texture(sampler2D(image0_texture, image0_sampler), frag_position0.xy / frag_position0.z);
    vec4 color1 = texture(sampler2D(image1_texture, image1_sampler), frag_position1.xy / frag_position1.z);
    if (color0.a > 0.0) {
        color0.rgb = color0.rgb / color0.a;
    }
    if (color1.a > 0.0) {
        color1.rgb = color1.rgb / color1.a;
    }
    vec4 color = mix(color0, color1, props.fade_t);
    color.a *= props.opacity;
    vec3 rgb = color.rgb;

    // spin
    rgb = vec3(
        dot(rgb, props.spin_weights.xyz),
        dot(rgb, props.spin_weights.zxy),
        dot(rgb, props.spin_weights.yzx));

    // saturation
    float average = (color.r + color.g + color.b) / 3.0;
    rgb += (average - rgb) * props.saturation_factor;

    // contrast
    rgb = (rgb - 0.5) * props.contrast_factor + 0.5;

    // brightness
    vec3 high_vec = vec3(props.brightness_low, props.brightness_low, props.brightness_low);
    vec3 low_vec = vec3(props.brightness_high, props.brightness_high, props.brightness_high);

    out_color = vec4(vec3(mix(high_vec, low_vec, rgb) * color.a), color.a);
}
",
};
