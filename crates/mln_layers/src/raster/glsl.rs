//! The programs of the `raster` layer type in GLSL, which `mln_glsl` parses for `mln_vulkan` and `mln_d3d12`.

use mln_gfx::shaders::source::glsl::Glsl;

const RASTER_SHADER_PRELUDE: &str = r"

#define idRasterDrawableUBO         idDrawableReservedVertexOnlyUBO
#define idRasterEvaluatedPropsUBO   layerUBOStartId

";

pub(super) const RASTER_SHADER: Glsl = Glsl {
    prelude: RASTER_SHADER_PRELUDE,
    vertex: r"

layout(location = 0) in ivec2 in_position;
layout(location = 1) in ivec2 in_texture_position;

layout(std140, set = LAYER_SET_INDEX, binding = idRasterDrawableUBO) readonly buffer RasterDrawableUBOVector {
    RasterDrawableUBO drawable_ubo[];
} drawableVector;

layout(set = LAYER_SET_INDEX, binding = idRasterEvaluatedPropsUBO) uniform RasterEvaluatedPropsUBOBlock {
    RasterEvaluatedPropsUBO props;
};

layout(location = 0) out vec2 frag_position0;
layout(location = 1) out vec2 frag_position1;

void main() {
    const RasterDrawableUBO drawable = drawableVector.drawable_ubo[constant.ubo_index];

    gl_Position = projectTile(vec2(in_position), vec2(in_position), projectionVector.projection_ubo[constant.ubo_index]);

    // We are using Int16 for texture position coordinates to give us enough precision for
    // fractional coordinates. We use 8192 to scale the texture coordinates in the buffer
    // as an arbitrarily high number to preserve adequate precision when rendering.
    // This is also the same value as the EXTENT we are using for our tile buffer pos coordinates,
    // so math for modifying either is consistent.
    frag_position0 = (((in_texture_position / 8192.0) - 0.5) / props.buffer_scale ) + 0.5;
    frag_position1 = (frag_position0 * props.scale_parent) + props.tl_parent;
}
",
    fragment: r"

layout(location = 0) in vec2 frag_position0;
layout(location = 1) in vec2 frag_position1;

layout(location = 0) out vec4 out_color;

layout(set = LAYER_SET_INDEX, binding = idRasterEvaluatedPropsUBO) uniform RasterEvaluatedPropsUBOBlock {
    RasterEvaluatedPropsUBO props;
};

layout(set = DRAWABLE_IMAGE_SET_INDEX, binding = 0) uniform texture2D image0_texture;
layout(set = DRAWABLE_IMAGE_SET_INDEX, binding = samplerBindingStartId) uniform sampler image0_sampler;
layout(set = DRAWABLE_IMAGE_SET_INDEX, binding = 1) uniform texture2D image1_texture;
layout(set = DRAWABLE_IMAGE_SET_INDEX, binding = samplerBindingStartId + 1) uniform sampler image1_sampler;

void main() {

#if defined(OVERDRAW_INSPECTOR)
    out_color = vec4(1.0);
    return;
#endif

    // read and cross-fade colors from the main and parent tiles
    vec4 color0 = texture(sampler2D(image0_texture, image0_sampler), frag_position0);
    vec4 color1 = texture(sampler2D(image1_texture, image1_sampler), frag_position1);
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
