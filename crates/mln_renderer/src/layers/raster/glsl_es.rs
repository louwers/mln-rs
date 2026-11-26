//! The GLSL ES, for OpenGL, of the rasters' program.

use mln_gfx::shaders::source::{GlslEsDeclarations, GlslEsSource};

pub(crate) const RASTER: GlslEsSource = GlslEsSource {
    vertex: r"out vec3 v_pos0;
out vec3 v_pos1;

void main() {
    // The vertices span the unit square, which the warp puts on the quad
    // of the corners: blended from the projective warp towards the
    // bilinear one, whose homogeneous denominator is one at the top left.
    vec2 fractional = a_pos / 8192.0;
    vec2 top_left = u_coords_top.xy;
    vec2 top_right = u_coords_top.zw;
    vec2 bottom_left = u_coords_bottom.xy;
    vec2 bottom_right = u_coords_bottom.zw;
    vec2 bilinear = mix(mix(top_left, top_right, fractional.x), mix(bottom_left, bottom_right, fractional.x), fractional.y);
    float denominator = dot(u_image_warp.xy, fractional) + 1.0;
    vec2 across_top = top_right - top_left + u_image_warp.x * top_right;
    vec2 down_left = bottom_left - top_left + u_image_warp.y * bottom_left;
    vec2 projective = (across_top * fractional.x + down_left * fractional.y + top_left) / denominator;
    vec2 position = mix(projective, bilinear, u_image_warp.z);
    gl_Position = projectTile(position, position);
    // We are using Int16 for texture position coordinates to give us enough precision for
    // fractional coordinates. We use 8192 to scale the texture coordinates in the buffer
    // as an arbitrarily high number to preserve adequate precision when rendering.
    // This is also the same value as the EXTENT we are using for our tile buffer pos coordinates,
    // so math for modifying either is consistent.
    vec2 pos0 = (((a_texture_pos / 8192.0) - 0.5) / u_buffer_scale ) + 0.5;
    vec2 pos1 = (pos0 * u_scale_parent) + u_tl_parent;
    // The texture coordinates weighted by the reciprocal of the
    // denominator, blended as the position is, interpolate projectively.
    float weight = mix(1.0 / denominator, 1.0, u_image_warp.z);
    v_pos0 = vec3(pos0 * weight, weight);
    v_pos1 = vec3(pos1 * weight, weight);
}
",
    fragment: r"in highp vec3 v_pos0;
in highp vec3 v_pos1;

void main() {

    // read and cross-fade colors from the main and parent tiles
    vec4 color0 = texture(u_image0, v_pos0.xy / v_pos0.z);
    vec4 color1 = texture(u_image1, v_pos1.xy / v_pos1.z);
    if (color0.a > 0.0) {
        color0.rgb = color0.rgb / color0.a;
    }
    if (color1.a > 0.0) {
        color1.rgb = color1.rgb / color1.a;
    }
    vec4 color = mix(color0, color1, u_fade_t);
    float alpha = color.a * u_opacity;
    vec3 rgb = color.rgb;

    // spin
    rgb = vec3(
        dot(rgb, u_spin_weights.xyz),
        dot(rgb, u_spin_weights.zxy),
        dot(rgb, u_spin_weights.yzx));

    // saturation
    float average = (color.r + color.g + color.b) / 3.0;
    rgb += (average - rgb) * u_saturation_factor;

    // contrast
    rgb = (rgb - 0.5) * u_contrast_factor + 0.5;

    // brightness
    vec3 u_high_vec = vec3(u_brightness_low, u_brightness_low, u_brightness_low);
    vec3 u_low_vec = vec3(u_brightness_high, u_brightness_high, u_brightness_high);

    fragColor = vec4(mix(u_high_vec, u_low_vec, rgb) * alpha, alpha);
}
",
    declarations: &GlslEsDeclarations::DEFAULT,
};
