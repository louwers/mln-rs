//! The GLSL ES, for OpenGL, of the lines' programs.

use mln_gfx::shaders::block::{Block, UniformBlock};
use mln_gfx::shaders::source::Precision::{self, Low, Medium};
use mln_gfx::shaders::source::{GlslEsDeclarations, GlslEsSource};

use super::ubo::{
    LineDrawableUBO, LineEvaluatedPropsUBO, LineGradientDrawableUBO, LinePatternDrawableUBO, LinePatternTilePropsUBO,
    LineSDFDrawableUBO,
};

/// The precisions of the members of the lines' evaluated properties.
const LINE_PROPS: [(&Block, Precision, &[&str]); 2] = [
    (LineEvaluatedPropsUBO::BLOCK, Medium, &["gapwidth", "width"]),
    (LineEvaluatedPropsUBO::BLOCK, Low, &["blur", "opacity", "offset", "floorwidth"]),
];

/// The precisions of the plain lines' values.
const LINE_DECLARATIONS: GlslEsDeclarations = GlslEsDeclarations {
    members: &[
        (LineDrawableUBO::BLOCK, Medium, &["ratio"]),
        (LineDrawableUBO::BLOCK, Low, &["color_t", "blur_t", "opacity_t", "gapwidth_t", "offset_t", "width_t"]),
        LINE_PROPS[0],
        LINE_PROPS[1],
    ],
    attributes: &[(Medium, &["gapwidth", "width"]), (Low, &["blur", "offset", "opacity"])],
    ..GlslEsDeclarations::DEFAULT
};

/// The precisions of the gradient lines' values.
const LINE_GRADIENT_DECLARATIONS: GlslEsDeclarations = GlslEsDeclarations {
    members: &[
        (LineGradientDrawableUBO::BLOCK, Medium, &["ratio"]),
        (LineGradientDrawableUBO::BLOCK, Low, &["blur_t", "opacity_t", "gapwidth_t", "offset_t", "width_t"]),
        LINE_PROPS[0],
        LINE_PROPS[1],
    ],
    attributes: &[(Medium, &["gapwidth", "width"]), (Low, &["blur", "offset", "opacity"])],
    ..GlslEsDeclarations::DEFAULT
};

/// The precisions of the pattern lines' values.
const LINE_PATTERN_DECLARATIONS: GlslEsDeclarations = GlslEsDeclarations {
    members: &[
        (LinePatternDrawableUBO::BLOCK, Medium, &["ratio"]),
        (
            LinePatternDrawableUBO::BLOCK,
            Low,
            &[
                "blur_t",
                "opacity_t",
                "gapwidth_t",
                "offset_t",
                "width_t",
                "pattern_from_t",
                "pattern_to_t",
                "floorwidth_t",
            ],
        ),
        (LinePatternTilePropsUBO::BLOCK, Medium, &["scale"]),
        LINE_PROPS[0],
        LINE_PROPS[1],
    ],
    attributes: &[(Medium, &["gapwidth", "width"]), (Low, &["blur", "offset", "opacity"])],
    ..GlslEsDeclarations::DEFAULT
};

/// The precisions of the members of the dashed lines' draws.
const LINE_SDF_DRAWABLE: [(&Block, Precision, &[&str]); 2] = [
    (LineSDFDrawableUBO::BLOCK, Medium, &["ratio"]),
    (
        LineSDFDrawableUBO::BLOCK,
        Low,
        &["color_t", "blur_t", "opacity_t", "gapwidth_t", "offset_t", "width_t", "floorwidth_t"],
    ),
];

/// The precisions of the dashed lines' values.
const LINE_SDF_DECLARATIONS: GlslEsDeclarations = GlslEsDeclarations {
    members: &[LINE_SDF_DRAWABLE[0], LINE_SDF_DRAWABLE[1], LINE_PROPS[0], LINE_PROPS[1]],
    attributes: &[(Medium, &["gapwidth", "width"]), (Low, &["blur", "floorwidth", "offset", "opacity"])],
    ..GlslEsDeclarations::DEFAULT
};

/// The precisions of the dashed gradient lines' values.
const LINE_GRADIENT_SDF_DECLARATIONS: GlslEsDeclarations = GlslEsDeclarations {
    members: &[LINE_SDF_DRAWABLE[0], LINE_SDF_DRAWABLE[1], LINE_PROPS[0], LINE_PROPS[1]],
    attributes: &[(Medium, &["gapwidth", "offset", "width"]), (Low, &["blur", "floorwidth", "opacity"])],
    ..GlslEsDeclarations::DEFAULT
};

pub(crate) const LINE_GRADIENT_SDF: GlslEsSource = GlslEsSource {
    vertex: r#"// floor(127 / 2) == 63.0
// the maximum allowed miter limit is 2.0 at the moment. the extrude normal is
// stored in a byte (-128..127). we scale regular normals up to length 63, but
// there are also "special" normals that have a bigger length (of up to 126 in
// this case).
// #define scale 63.0
#define scale 0.015873016

// We scale the distance before adding it to the buffers so that we can store
// long distances for long segments. Use this value to unscale the distance.
#define LINE_DISTANCE_SCALE 2.0

// The gradient ramp is indexed by the ratio along the entire line, stored scaled to [0, 2^15).
#define MAX_LINE_DISTANCE 32767.0

out vec2 v_normal;
out vec2 v_width2;
out highp vec2 v_tex_a;
out highp vec2 v_tex_b;
out float v_gamma_scale;
out highp float v_lineprogress;

#ifndef HAS_UNIFORM_u_blur
out lowp float blur;
#endif
#ifndef HAS_UNIFORM_u_opacity
out lowp float opacity;
#endif
#ifndef HAS_UNIFORM_u_width
out mediump float width;
#endif
#ifndef HAS_UNIFORM_u_floorwidth
out lowp float floorwidth;
#endif
#ifndef HAS_UNIFORM_u_dasharray
flat out highp float v_dash_width;
#endif

#ifdef PROJECTION_GLOBE
out float v_tile_x;
#endif

void main() {
    #ifndef HAS_UNIFORM_u_blur
blur = unpack_mix_vec2(a_blur, u_blur_t);
#else
lowp float blur = u_blur;
#endif
    #ifndef HAS_UNIFORM_u_opacity
opacity = unpack_mix_vec2(a_opacity, u_opacity_t);
#else
lowp float opacity = u_opacity;
#endif
    #ifndef HAS_UNIFORM_u_gapwidth
mediump float gapwidth = unpack_mix_vec2(a_gapwidth, u_gapwidth_t);
#else
mediump float gapwidth = u_gapwidth;
#endif
    #ifndef HAS_UNIFORM_u_offset
mediump float offset = unpack_mix_vec2(a_offset, u_offset_t);
#else
lowp float offset = u_offset;
#endif
    #ifndef HAS_UNIFORM_u_width
width = unpack_mix_vec2(a_width, u_width_t);
#else
mediump float width = u_width;
#endif
    #ifndef HAS_UNIFORM_u_floorwidth
floorwidth = unpack_mix_vec2(a_floorwidth, u_floorwidth_t);
#else
lowp float floorwidth = u_floorwidth;
#endif

    // the distance over which the line edge fades out.
    // Retina devices need a smaller distance to avoid aliasing.
    float ANTIALIASING = 1.0 / DEVICE_PIXEL_RATIO / 2.0;

    vec2 a_extrude = a_data.xy - 128.0;
    float a_direction = mod(a_data.z, 4.0) - 1.0;
    float a_linesofar = (floor(a_data.z / 4.0) + a_data.w * 64.0) * LINE_DISTANCE_SCALE;
    v_lineprogress = (floor(a_data.z / 4.0) + a_data.w * 64.0) * 2.0 / MAX_LINE_DISTANCE;

    vec2 pos = floor(a_pos_normal * 0.5);

    // x is 1 if it's a round cap, 0 otherwise
    // y is 1 if the normal points up, and -1 if it points down
    // We store these in the least significant bit of a_pos_normal
    mediump vec2 normal = a_pos_normal - 2.0 * pos;
    normal.y = normal.y * 2.0 - 1.0;
    v_normal = normal;

    // these transformations used to be applied in the JS and native code bases.
    // moved them into the shader for clarity and simplicity.
    gapwidth = gapwidth / 2.0;
    float halfwidth = width / 2.0;
    offset = -1.0 * offset;

    float inset = gapwidth + (gapwidth > 0.0 ? ANTIALIASING : 0.0);
    float outset = gapwidth + halfwidth * (gapwidth > 0.0 ? 2.0 : 1.0) + (halfwidth == 0.0 ? 0.0 : ANTIALIASING);

    // Scale the extrusion vector down to a normal and then up by the line width
    // of this vertex.
    mediump vec2 dist =outset * a_extrude * scale;

    // Calculate the offset when drawing a line that is to the side of the actual line.
    // We do this by creating a vector that points towards the extrude, but rotate
    // it when we're drawing round end points (a_direction = -1 or 1) since their
    // extrude vector points in another direction.
    mediump float u = 0.5 * a_direction;
    mediump float t = 1.0 - abs(u);
    mediump vec2 offset2 = offset * a_extrude * scale * normal.y * mat2(t, -u, u, t);

#ifdef PROJECTION_GLOBE
    float adjustedThickness = projectLineThickness(pos.y);
    vec4 projected_no_extrude = projectTile(pos + offset2 / u_ratio * adjustedThickness);
    vec2 extrudedPos = pos + (offset2 + dist) / u_ratio * adjustedThickness;
    gl_Position = projectTile(extrudedPos);
    v_tile_x = antimeridianClipX(extrudedPos);
    vec4 projected_extrude = gl_Position - projected_no_extrude;
#else
    vec4 projected_extrude = u_matrix * vec4(dist / u_ratio, 0.0, 0.0);
    gl_Position = u_matrix * vec4(pos + offset2 / u_ratio, 0.0, 1.0) + projected_extrude;
#endif

    // calculate how much the perspective view squishes or stretches the extrude
    float extrude_length_without_perspective = length(dist);
    float extrude_length_with_perspective = length(projected_extrude.xy / gl_Position.w * u_units_to_pixels);
    v_gamma_scale = extrude_length_without_perspective / extrude_length_with_perspective;

#ifdef HAS_UNIFORM_u_dasharray
    highp vec2 patternscale_a = u_patternscale_a;
    highp vec2 patternscale_b = u_patternscale_b;
    highp float tex_y_a = u_tex_y_a;
    highp float tex_y_b = u_tex_y_b;
#else
    // The dash attribute: the pattern's middle row, row span and length.
    highp vec2 patternscale_a = vec2(u_patternscale_a.x / a_dasharray.z, -a_dasharray.y / 2.0 / u_atlas_height);
    highp vec2 patternscale_b = vec2(u_patternscale_b.x / a_dasharray.z, patternscale_a.y);
    highp float tex_y_a = (a_dasharray.x + 0.5) / u_atlas_height;
    highp float tex_y_b = tex_y_a;
    v_dash_width = a_dasharray.z;
#endif
    v_tex_a = vec2(a_linesofar * patternscale_a.x / floorwidth, normal.y * patternscale_a.y + tex_y_a);
    v_tex_b = vec2(a_linesofar * patternscale_b.x / floorwidth, normal.y * patternscale_b.y + tex_y_b);

    v_width2 = vec2(outset, inset);
}
"#,
    fragment: r"in vec2 v_normal;
in vec2 v_width2;
in highp vec2 v_tex_a;
in highp vec2 v_tex_b;
in float v_gamma_scale;
in highp float v_lineprogress;

#ifndef HAS_UNIFORM_u_blur
in lowp float blur;
#endif
#ifndef HAS_UNIFORM_u_opacity
in lowp float opacity;
#endif
#ifndef HAS_UNIFORM_u_width
in mediump float width;
#endif
#ifndef HAS_UNIFORM_u_floorwidth
in lowp float floorwidth;
#endif
#ifndef HAS_UNIFORM_u_dasharray
flat in highp float v_dash_width;
#endif

#ifdef PROJECTION_GLOBE
in highp float v_tile_x;
#endif

void main() {
#ifdef PROJECTION_GLOBE
    if (clippedAtAntimeridian(v_tile_x)) {
        discard;
    }
#endif
    #ifdef HAS_UNIFORM_u_blur
lowp float blur = u_blur;
#endif
    #ifdef HAS_UNIFORM_u_opacity
lowp float opacity = u_opacity;
#endif
    #ifdef HAS_UNIFORM_u_width
mediump float width = u_width;
#endif
    #ifdef HAS_UNIFORM_u_floorwidth
lowp float floorwidth = u_floorwidth;
#endif

    // Calculate the distance of the pixel from the line in pixels.
    float dist = length(v_normal) * v_width2.s;

    // Calculate the antialiasing fade factor. This is either when fading in
    // the line in case of an offset line (v_width2.t) or when fading out
    // (v_width2.s)
    float blur2 = (blur + 1.0 / DEVICE_PIXEL_RATIO) * v_gamma_scale;
    float alpha = clamp(min(dist - (v_width2.t - blur2), v_width2.s - dist) / blur2, 0.0, 1.0);

    float sdfdist_a = texture(u_image_dash, v_tex_a).a;
    float sdfdist_b = texture(u_image_dash, v_tex_b).a;
    float sdfdist = mix(sdfdist_a, sdfdist_b, u_mix);
#ifdef HAS_UNIFORM_u_dasharray
    highp float sdfgamma = u_sdfgamma;
#else
    highp float sdfgamma = u_sdfgamma / v_dash_width;
#endif
    alpha *= smoothstep(0.5 - sdfgamma / floorwidth, 0.5 + sdfgamma / floorwidth, sdfdist);

    // For gradient lines, v_lineprogress is the ratio along the entire line,
    // scaled to [0, 2^15), and the gradient ramp is stored in a texture.
    vec4 color = texture(u_image, vec2(v_lineprogress, 0.5));

    fragColor = color * (alpha * opacity);
}
",
    declarations: &LINE_GRADIENT_SDF_DECLARATIONS,
};

pub(crate) const LINE_GRADIENT: GlslEsSource = GlslEsSource {
    vertex: r#"
// the attribute conveying progress along a line is scaled to [0, 2^15)
#define MAX_LINE_DISTANCE 32767.0

// floor(127 / 2) == 63.0
// the maximum allowed miter limit is 2.0 at the moment. the extrude normal is
// stored in a byte (-128..127). we scale regular normals up to length 63, but
// there are also "special" normals that have a bigger length (of up to 126 in
// this case).
// #define scale 63.0
#define scale 0.015873016

out vec2 v_normal;
out vec2 v_width2;
out float v_gamma_scale;
out highp float v_lineprogress;

#ifndef HAS_UNIFORM_u_blur
out lowp float blur;
#endif
#ifndef HAS_UNIFORM_u_opacity
out lowp float opacity;
#endif

#ifdef PROJECTION_GLOBE
out float v_tile_x;
#endif

void main() {
    #ifndef HAS_UNIFORM_u_blur
blur = unpack_mix_vec2(a_blur, u_blur_t);
#else
lowp float blur = u_blur;
#endif
    #ifndef HAS_UNIFORM_u_opacity
opacity = unpack_mix_vec2(a_opacity, u_opacity_t);
#else
lowp float opacity = u_opacity;
#endif
    #ifndef HAS_UNIFORM_u_gapwidth
mediump float gapwidth = unpack_mix_vec2(a_gapwidth, u_gapwidth_t);
#else
mediump float gapwidth = u_gapwidth;
#endif
    #ifndef HAS_UNIFORM_u_offset
lowp float offset = unpack_mix_vec2(a_offset, u_offset_t);
#else
lowp float offset = u_offset;
#endif
    #ifndef HAS_UNIFORM_u_width
mediump float width = unpack_mix_vec2(a_width, u_width_t);
#else
mediump float width = u_width;
#endif

    // the distance over which the line edge fades out.
    // Retina devices need a smaller distance to avoid aliasing.
    float ANTIALIASING = 1.0 / DEVICE_PIXEL_RATIO / 2.0;

    vec2 a_extrude = a_data.xy - 128.0;
    float a_direction = mod(a_data.z, 4.0) - 1.0;

    v_lineprogress = (floor(a_data.z / 4.0) + a_data.w * 64.0) * 2.0 / MAX_LINE_DISTANCE;

    vec2 pos = floor(a_pos_normal * 0.5);

    // x is 1 if it's a round cap, 0 otherwise
    // y is 1 if the normal points up, and -1 if it points down
    // We store these in the least significant bit of a_pos_normal
    mediump vec2 normal = a_pos_normal - 2.0 * pos;
    normal.y = normal.y * 2.0 - 1.0;
    v_normal = normal;

    // these transformations used to be applied in the JS and native code bases.
    // moved them into the shader for clarity and simplicity.
    gapwidth = gapwidth / 2.0;
    float halfwidth = width / 2.0;
    offset = -1.0 * offset;

    float inset = gapwidth + (gapwidth > 0.0 ? ANTIALIASING : 0.0);
    float outset = gapwidth + halfwidth * (gapwidth > 0.0 ? 2.0 : 1.0) + (halfwidth == 0.0 ? 0.0 : ANTIALIASING);

    // Scale the extrusion vector down to a normal and then up by the line width
    // of this vertex.
    mediump vec2 dist = outset * a_extrude * scale;

    // Calculate the offset when drawing a line that is to the side of the actual line.
    // We do this by creating a vector that points towards the extrude, but rotate
    // it when we're drawing round end points (a_direction = -1 or 1) since their
    // extrude vector points in another direction.
    mediump float u = 0.5 * a_direction;
    mediump float t = 1.0 - abs(u);
    mediump vec2 offset2 = offset * a_extrude * scale * normal.y * mat2(t, -u, u, t);

#ifdef PROJECTION_GLOBE
    float adjustedThickness = projectLineThickness(pos.y);
    vec4 projected_no_extrude = projectTile(pos + offset2 / u_ratio * adjustedThickness);
    vec2 extrudedPos = pos + (offset2 + dist) / u_ratio * adjustedThickness;
    gl_Position = projectTile(extrudedPos);
    v_tile_x = antimeridianClipX(extrudedPos);
    vec4 projected_extrude = gl_Position - projected_no_extrude;
#else
    vec4 projected_extrude = u_matrix * vec4(dist / u_ratio, 0.0, 0.0);
    gl_Position = u_matrix * vec4(pos + offset2 / u_ratio, 0.0, 1.0) + projected_extrude;
#endif

    // calculate how much the perspective view squishes or stretches the extrude
    float extrude_length_without_perspective = length(dist);
    float extrude_length_with_perspective = length(projected_extrude.xy / gl_Position.w * u_units_to_pixels);
    v_gamma_scale = extrude_length_without_perspective / extrude_length_with_perspective;

    v_width2 = vec2(outset, inset);
}
"#,
    fragment: r"in vec2 v_width2;
in vec2 v_normal;
in float v_gamma_scale;
in highp float v_lineprogress;

#ifndef HAS_UNIFORM_u_blur
in lowp float blur;
#endif
#ifndef HAS_UNIFORM_u_opacity
in lowp float opacity;
#endif

#ifdef PROJECTION_GLOBE
in highp float v_tile_x;
#endif

void main() {
#ifdef PROJECTION_GLOBE
    if (clippedAtAntimeridian(v_tile_x)) {
        discard;
    }
#endif
    #ifdef HAS_UNIFORM_u_blur
lowp float blur = u_blur;
#endif
    #ifdef HAS_UNIFORM_u_opacity
lowp float opacity = u_opacity;
#endif

    // Calculate the distance of the pixel from the line in pixels.
    float dist = length(v_normal) * v_width2.s;

    // Calculate the antialiasing fade factor. This is either when fading in
    // the line in case of an offset line (v_width2.t) or when fading out
    // (v_width2.s)
    float blur2 = (blur + 1.0 / DEVICE_PIXEL_RATIO) * v_gamma_scale;
    float alpha = clamp(min(dist - (v_width2.t - blur2), v_width2.s - dist) / blur2, 0.0, 1.0);

    // For gradient lines, v_lineprogress is the ratio along the entire line,
    // scaled to [0, 2^15), and the gradient ramp is stored in a texture.
    vec4 color = texture(u_image, vec2(v_lineprogress, 0.5));

    fragColor = color * (alpha * opacity);
}
",
    declarations: &LINE_GRADIENT_DECLARATIONS,
};

pub(crate) const LINE_PATTERN: GlslEsSource = GlslEsSource {
    vertex: r#"// floor(127 / 2) == 63.0
// the maximum allowed miter limit is 2.0 at the moment. the extrude normal is
// stored in a byte (-128..127). we scale regular normals up to length 63, but
// there are also "special" normals that have a bigger length (of up to 126 in
// this case).
// #define scale 63.0
#define scale 0.015873016

// We scale the distance before adding it to the buffers so that we can store
// long distances for long segments. Use this value to unscale the distance.
#define LINE_DISTANCE_SCALE 2.0

out vec2 v_normal;
out vec2 v_width2;
out float v_linesofar;
out float v_gamma_scale;
out float v_floorwidth;

#ifndef HAS_UNIFORM_u_blur
out lowp float blur;
#endif
#ifndef HAS_UNIFORM_u_opacity
out lowp float opacity;
#endif
#ifndef HAS_UNIFORM_u_pattern_from
flat out highp vec4 pattern_from;
#endif
#ifndef HAS_UNIFORM_u_pattern_to
flat out highp vec4 pattern_to;
#endif

#ifdef PROJECTION_GLOBE
out float v_tile_x;
#endif

void main() {
    #ifndef HAS_UNIFORM_u_blur
blur = unpack_mix_vec2(a_blur, u_blur_t);
#else
lowp float blur = u_blur;
#endif
    #ifndef HAS_UNIFORM_u_opacity
opacity = unpack_mix_vec2(a_opacity, u_opacity_t);
#else
lowp float opacity = u_opacity;
#endif
    #ifndef HAS_UNIFORM_u_offset
lowp float offset = unpack_mix_vec2(a_offset, u_offset_t);
#else
lowp float offset = u_offset;
#endif
    #ifndef HAS_UNIFORM_u_gapwidth
mediump float gapwidth = unpack_mix_vec2(a_gapwidth, u_gapwidth_t);
#else
mediump float gapwidth = u_gapwidth;
#endif
    #ifndef HAS_UNIFORM_u_width
mediump float width = unpack_mix_vec2(a_width, u_width_t);
#else
mediump float width = u_width;
#endif
    #ifndef HAS_UNIFORM_u_pattern_from
pattern_from = a_pattern_from;
#endif
    #ifndef HAS_UNIFORM_u_pattern_to
pattern_to = a_pattern_to;
#endif

#ifndef HAS_UNIFORM_u_width
    v_floorwidth = unpack_mix_vec2(a_width, u_floorwidth_t);
#else
    v_floorwidth = u_floorwidth;
#endif

    // the distance over which the line edge fades out.
    // Retina devices need a smaller distance to avoid aliasing.
    float ANTIALIASING = 1.0 / DEVICE_PIXEL_RATIO / 2.0;

    vec2 a_extrude = a_data.xy - 128.0;
    float a_direction = mod(a_data.z, 4.0) - 1.0;
    float a_linesofar = (floor(a_data.z / 4.0) + a_data.w * 64.0) * LINE_DISTANCE_SCALE;
    // float tileRatio = u_scale.y;
    vec2 pos = floor(a_pos_normal * 0.5);

    // x is 1 if it's a round cap, 0 otherwise
    // y is 1 if the normal points up, and -1 if it points down
    // We store these in the least significant bit of a_pos_normal
    mediump vec2 normal = a_pos_normal - 2.0 * pos;
    normal.y = normal.y * 2.0 - 1.0;
    v_normal = normal;

    // these transformations used to be applied in the JS and native code bases.
    // moved them into the shader for clarity and simplicity.
    gapwidth = gapwidth / 2.0;
    float halfwidth = width / 2.0;
    offset = -1.0 * offset;

    float inset = gapwidth + (gapwidth > 0.0 ? ANTIALIASING : 0.0);
    float outset = gapwidth + halfwidth * (gapwidth > 0.0 ? 2.0 : 1.0) + (halfwidth == 0.0 ? 0.0 : ANTIALIASING);

    // Scale the extrusion vector down to a normal and then up by the line width
    // of this vertex.
    mediump vec2 dist = outset * a_extrude * scale;

    // Calculate the offset when drawing a line that is to the side of the actual line.
    // We do this by creating a vector that points towards the extrude, but rotate
    // it when we're drawing round end points (a_direction = -1 or 1) since their
    // extrude vector points in another direction.
    mediump float u = 0.5 * a_direction;
    mediump float t = 1.0 - abs(u);
    mediump vec2 offset2 = offset * a_extrude * scale * normal.y * mat2(t, -u, u, t);

#ifdef PROJECTION_GLOBE
    float adjustedThickness = projectLineThickness(pos.y);
    vec4 projected_no_extrude = projectTile(pos + offset2 / u_ratio * adjustedThickness);
    vec2 extrudedPos = pos + (offset2 + dist) / u_ratio * adjustedThickness;
    gl_Position = projectTile(extrudedPos);
    v_tile_x = antimeridianClipX(extrudedPos);
    vec4 projected_extrude = gl_Position - projected_no_extrude;
#else
    vec4 projected_extrude = u_matrix * vec4(dist / u_ratio, 0.0, 0.0);
    gl_Position = u_matrix * vec4(pos + offset2 / u_ratio, 0.0, 1.0) + projected_extrude;
#endif

    // calculate how much the perspective view squishes or stretches the extrude
    float extrude_length_without_perspective = length(dist);
    float extrude_length_with_perspective = length(projected_extrude.xy / gl_Position.w * u_units_to_pixels);
    v_gamma_scale = extrude_length_without_perspective / extrude_length_with_perspective;

    v_linesofar = a_linesofar;
    v_width2 = vec2(outset, inset);
}
"#,
    fragment: r"#ifdef GL_ES
precision highp float;
#endif

in vec2 v_normal;
in vec2 v_width2;
in float v_linesofar;
in float v_gamma_scale;
in float v_floorwidth;

#ifndef HAS_UNIFORM_u_pattern_from
flat in highp vec4 pattern_from;
#endif
#ifndef HAS_UNIFORM_u_pattern_to
flat in highp vec4 pattern_to;
#endif
#ifndef HAS_UNIFORM_u_blur
in lowp float blur;
#endif
#ifndef HAS_UNIFORM_u_opacity
in lowp float opacity;
#endif

#ifdef PROJECTION_GLOBE
in highp float v_tile_x;
#endif

void main() {
#ifdef PROJECTION_GLOBE
    if (clippedAtAntimeridian(v_tile_x)) {
        discard;
    }
#endif
    #ifdef HAS_UNIFORM_u_pattern_from
highp vec4 pattern_from = u_pattern_from;
#endif
    #ifdef HAS_UNIFORM_u_pattern_to
highp vec4 pattern_to = u_pattern_to;
#endif

    #ifdef HAS_UNIFORM_u_blur
lowp float blur = u_blur;
#endif
    #ifdef HAS_UNIFORM_u_opacity
lowp float opacity = u_opacity;
#endif

    vec2 pattern_tl_a = pattern_from.xy;
    vec2 pattern_br_a = pattern_from.zw;
    vec2 pattern_tl_b = pattern_to.xy;
    vec2 pattern_br_b = pattern_to.zw;

    float pixelRatio = u_scale.x;
    float tileZoomRatio = u_scale.y;
    float fromScale = u_scale.z;
    float toScale = u_scale.w;

    vec2 display_size_a = vec2((pattern_br_a.x - pattern_tl_a.x) / pixelRatio, (pattern_br_a.y - pattern_tl_a.y) / pixelRatio);
    vec2 display_size_b = vec2((pattern_br_b.x - pattern_tl_b.x) / pixelRatio, (pattern_br_b.y - pattern_tl_b.y) / pixelRatio);

    vec2 pattern_size_a = vec2(display_size_a.x * fromScale / tileZoomRatio, display_size_a.y);
    vec2 pattern_size_b = vec2(display_size_b.x * toScale / tileZoomRatio, display_size_b.y);

    float aspect_a = display_size_a.y / v_floorwidth;
    float aspect_b = display_size_b.y / v_floorwidth;

    // Calculate the distance of the pixel from the line in pixels.
    float dist = length(v_normal) * v_width2.s;

    // Calculate the antialiasing fade factor. This is either when fading in
    // the line in case of an offset line (v_width2.t) or when fading out
    // (v_width2.s)
    float blur2 = (blur + 1.0 / DEVICE_PIXEL_RATIO) * v_gamma_scale;
    float alpha = clamp(min(dist - (v_width2.t - blur2), v_width2.s - dist) / blur2, 0.0, 1.0);

    float y = 0.5 * v_normal.y + 0.5;

    vec2 texel_size = 1.0 / u_texsize;
    vec2 half_texel = 0.5 * texel_size;
    // A crossfade at 0 or 1 shows one pattern, so skip the other.
    vec4 color_a = vec4(0.0);
    if (u_fade < 1.0) {
        float x_a = mod(v_linesofar / pattern_size_a.x * aspect_a, 1.0);
        highp vec2 pos_a = atlas_pos((pattern_tl_a - 1.0) * texel_size, (pattern_br_a - pattern_tl_a + 2.0) * texel_size * vec2(x_a, y));
        pos_a = clamp(pos_a, (pattern_tl_a - 1.0) * texel_size + half_texel, (pattern_br_a + 1.0) * texel_size - half_texel);
        color_a = texture(u_image, pos_a);
    }
    vec4 color_b = vec4(0.0);
    if (u_fade > 0.0) {
        float x_b = mod(v_linesofar / pattern_size_b.x * aspect_b, 1.0);
        highp vec2 pos_b = atlas_pos((pattern_tl_b - 1.0) * texel_size, (pattern_br_b - pattern_tl_b + 2.0) * texel_size * vec2(x_b, y));
        pos_b = clamp(pos_b, (pattern_tl_b - 1.0) * texel_size + half_texel, (pattern_br_b + 1.0) * texel_size - half_texel);
        color_b = texture(u_image, pos_b);
    }

    vec4 color = mix(color_a, color_b, u_fade);

    fragColor = color * alpha * opacity;
}
",
    declarations: &LINE_PATTERN_DECLARATIONS,
};

pub(crate) const LINE_SDF: GlslEsSource = GlslEsSource {
    vertex: r#"// floor(127 / 2) == 63.0
// the maximum allowed miter limit is 2.0 at the moment. the extrude normal is
// stored in a byte (-128..127). we scale regular normals up to length 63, but
// there are also "special" normals that have a bigger length (of up to 126 in
// this case).
// #define scale 63.0
#define scale 0.015873016

// We scale the distance before adding it to the buffers so that we can store
// long distances for long segments. Use this value to unscale the distance.
#define LINE_DISTANCE_SCALE 2.0

out vec2 v_normal;
out vec2 v_width2;
out vec2 v_tex_a;
out vec2 v_tex_b;
out float v_gamma_scale;

#ifndef HAS_UNIFORM_u_color
out highp vec4 color;
#endif
#ifndef HAS_UNIFORM_u_blur
out lowp float blur;
#endif
#ifndef HAS_UNIFORM_u_opacity
out lowp float opacity;
#endif
#ifndef HAS_UNIFORM_u_width
out mediump float width;
#endif
#ifndef HAS_UNIFORM_u_floorwidth
out lowp float floorwidth;
#endif
#ifndef HAS_UNIFORM_u_dasharray
flat out highp float v_dash_width;
#endif

#ifdef PROJECTION_GLOBE
out float v_tile_x;
#endif

void main() {
    #ifndef HAS_UNIFORM_u_color
color = unpack_mix_color(a_color, u_color_t);
#else
highp vec4 color = u_color;
#endif
    #ifndef HAS_UNIFORM_u_blur
blur = unpack_mix_vec2(a_blur, u_blur_t);
#else
lowp float blur = u_blur;
#endif
    #ifndef HAS_UNIFORM_u_opacity
opacity = unpack_mix_vec2(a_opacity, u_opacity_t);
#else
lowp float opacity = u_opacity;
#endif
    #ifndef HAS_UNIFORM_u_gapwidth
mediump float gapwidth = unpack_mix_vec2(a_gapwidth, u_gapwidth_t);
#else
mediump float gapwidth = u_gapwidth;
#endif
    #ifndef HAS_UNIFORM_u_offset
lowp float offset = unpack_mix_vec2(a_offset, u_offset_t);
#else
lowp float offset = u_offset;
#endif
    #ifndef HAS_UNIFORM_u_width
width = unpack_mix_vec2(a_width, u_width_t);
#else
mediump float width = u_width;
#endif
    #ifndef HAS_UNIFORM_u_floorwidth
floorwidth = unpack_mix_vec2(a_floorwidth, u_floorwidth_t);
#else
lowp float floorwidth = u_floorwidth;
#endif

    // the distance over which the line edge fades out.
    // Retina devices need a smaller distance to avoid aliasing.
    float ANTIALIASING = 1.0 / DEVICE_PIXEL_RATIO / 2.0;

    vec2 a_extrude = a_data.xy - 128.0;
    float a_direction = mod(a_data.z, 4.0) - 1.0;
    float a_linesofar = (floor(a_data.z / 4.0) + a_data.w * 64.0) * LINE_DISTANCE_SCALE;

    vec2 pos = floor(a_pos_normal * 0.5);

    // x is 1 if it's a round cap, 0 otherwise
    // y is 1 if the normal points up, and -1 if it points down
    // We store these in the least significant bit of a_pos_normal
    mediump vec2 normal = a_pos_normal - 2.0 * pos;
    normal.y = normal.y * 2.0 - 1.0;
    v_normal = normal;

    // these transformations used to be applied in the JS and native code bases.
    // moved them into the shader for clarity and simplicity.
    gapwidth = gapwidth / 2.0;
    float halfwidth = width / 2.0;
    offset = -1.0 * offset;

    float inset = gapwidth + (gapwidth > 0.0 ? ANTIALIASING : 0.0);
    float outset = gapwidth + halfwidth * (gapwidth > 0.0 ? 2.0 : 1.0) + (halfwidth == 0.0 ? 0.0 : ANTIALIASING);

    // Scale the extrusion vector down to a normal and then up by the line width
    // of this vertex.
    mediump vec2 dist =outset * a_extrude * scale;

    // Calculate the offset when drawing a line that is to the side of the actual line.
    // We do this by creating a vector that points towards the extrude, but rotate
    // it when we're drawing round end points (a_direction = -1 or 1) since their
    // extrude vector points in another direction.
    mediump float u = 0.5 * a_direction;
    mediump float t = 1.0 - abs(u);
    mediump vec2 offset2 = offset * a_extrude * scale * normal.y * mat2(t, -u, u, t);

#ifdef PROJECTION_GLOBE
    float adjustedThickness = projectLineThickness(pos.y);
    vec4 projected_no_extrude = projectTile(pos + offset2 / u_ratio * adjustedThickness);
    vec2 extrudedPos = pos + (offset2 + dist) / u_ratio * adjustedThickness;
    gl_Position = projectTile(extrudedPos);
    v_tile_x = antimeridianClipX(extrudedPos);
    vec4 projected_extrude = gl_Position - projected_no_extrude;
#else
    vec4 projected_extrude = u_matrix * vec4(dist / u_ratio, 0.0, 0.0);
    gl_Position = u_matrix * vec4(pos + offset2 / u_ratio, 0.0, 1.0) + projected_extrude;
#endif

    // calculate how much the perspective view squishes or stretches the extrude
    float extrude_length_without_perspective = length(dist);
    float extrude_length_with_perspective = length(projected_extrude.xy / gl_Position.w * u_units_to_pixels);
    v_gamma_scale = extrude_length_without_perspective / extrude_length_with_perspective;

#ifdef HAS_UNIFORM_u_dasharray
    highp vec2 patternscale_a = u_patternscale_a;
    highp vec2 patternscale_b = u_patternscale_b;
    highp float tex_y_a = u_tex_y_a;
    highp float tex_y_b = u_tex_y_b;
#else
    // The dash attribute: the pattern's middle row, row span and length.
    highp vec2 patternscale_a = vec2(u_patternscale_a.x / a_dasharray.z, -a_dasharray.y / 2.0 / u_atlas_height);
    highp vec2 patternscale_b = vec2(u_patternscale_b.x / a_dasharray.z, patternscale_a.y);
    highp float tex_y_a = (a_dasharray.x + 0.5) / u_atlas_height;
    highp float tex_y_b = tex_y_a;
    v_dash_width = a_dasharray.z;
#endif
    v_tex_a = vec2(a_linesofar * patternscale_a.x / floorwidth, normal.y * patternscale_a.y + tex_y_a);
    v_tex_b = vec2(a_linesofar * patternscale_b.x / floorwidth, normal.y * patternscale_b.y + tex_y_b);

    v_width2 = vec2(outset, inset);
}
"#,
    fragment: r"in vec2 v_normal;
in vec2 v_width2;
in highp vec2 v_tex_a;
in highp vec2 v_tex_b;
in float v_gamma_scale;

#ifndef HAS_UNIFORM_u_color
in highp vec4 color;
#endif
#ifndef HAS_UNIFORM_u_blur
in lowp float blur;
#endif
#ifndef HAS_UNIFORM_u_opacity
in lowp float opacity;
#endif
#ifndef HAS_UNIFORM_u_width
in mediump float width;
#endif
#ifndef HAS_UNIFORM_u_floorwidth
in lowp float floorwidth;
#endif
#ifndef HAS_UNIFORM_u_dasharray
flat in highp float v_dash_width;
#endif

#ifdef PROJECTION_GLOBE
in highp float v_tile_x;
#endif

void main() {
#ifdef PROJECTION_GLOBE
    if (clippedAtAntimeridian(v_tile_x)) {
        discard;
    }
#endif
    #ifdef HAS_UNIFORM_u_color
highp vec4 color = u_color;
#endif
    #ifdef HAS_UNIFORM_u_blur
lowp float blur = u_blur;
#endif
    #ifdef HAS_UNIFORM_u_opacity
lowp float opacity = u_opacity;
#endif
    #ifdef HAS_UNIFORM_u_width
mediump float width = u_width;
#endif
    #ifdef HAS_UNIFORM_u_floorwidth
lowp float floorwidth = u_floorwidth;
#endif

    // Calculate the distance of the pixel from the line in pixels.
    float dist = length(v_normal) * v_width2.s;

    // Calculate the antialiasing fade factor. This is either when fading in
    // the line in case of an offset line (v_width2.t) or when fading out
    // (v_width2.s)
    float blur2 = (blur + 1.0 / DEVICE_PIXEL_RATIO) * v_gamma_scale;
    float alpha = clamp(min(dist - (v_width2.t - blur2), v_width2.s - dist) / blur2, 0.0, 1.0);

    float sdfdist_a = texture(u_image, v_tex_a).a;
    float sdfdist_b = texture(u_image, v_tex_b).a;
    float sdfdist = mix(sdfdist_a, sdfdist_b, u_mix);
#ifdef HAS_UNIFORM_u_dasharray
    highp float sdfgamma = u_sdfgamma;
#else
    highp float sdfgamma = u_sdfgamma / v_dash_width;
#endif
    alpha *= smoothstep(0.5 - sdfgamma / floorwidth, 0.5 + sdfgamma / floorwidth, sdfdist);

    fragColor = color * (alpha * opacity);
}
",
    declarations: &LINE_SDF_DECLARATIONS,
};

pub(crate) const LINE: GlslEsSource = GlslEsSource {
    vertex: r#"// floor(127 / 2) == 63.0
// the maximum allowed miter limit is 2.0 at the moment. the extrude normal is
// stored in a byte (-128..127). we scale regular normals up to length 63, but
// there are also "special" normals that have a bigger length (of up to 126 in
// this case).
// #define scale 63.0
#define scale 0.015873016

out vec2 v_normal;
out vec2 v_width2;
out float v_gamma_scale;
out highp float v_linesofar;

#ifndef HAS_UNIFORM_u_color
out highp vec4 color;
#endif
#ifndef HAS_UNIFORM_u_blur
out lowp float blur;
#endif
#ifndef HAS_UNIFORM_u_opacity
out lowp float opacity;
#endif

#ifdef PROJECTION_GLOBE
out float v_tile_x;
#endif

void main() {
    #ifndef HAS_UNIFORM_u_color
color = unpack_mix_color(a_color, u_color_t);
#else
highp vec4 color = u_color;
#endif
    #ifndef HAS_UNIFORM_u_blur
blur = unpack_mix_vec2(a_blur, u_blur_t);
#else
lowp float blur = u_blur;
#endif
    #ifndef HAS_UNIFORM_u_opacity
opacity = unpack_mix_vec2(a_opacity, u_opacity_t);
#else
lowp float opacity = u_opacity;
#endif
    #ifndef HAS_UNIFORM_u_gapwidth
mediump float gapwidth = unpack_mix_vec2(a_gapwidth, u_gapwidth_t);
#else
mediump float gapwidth = u_gapwidth;
#endif
    #ifndef HAS_UNIFORM_u_offset
lowp float offset = unpack_mix_vec2(a_offset, u_offset_t);
#else
lowp float offset = u_offset;
#endif
    #ifndef HAS_UNIFORM_u_width
mediump float width = unpack_mix_vec2(a_width, u_width_t);
#else
mediump float width = u_width;
#endif

    // the distance over which the line edge fades out.
    // Retina devices need a smaller distance to avoid aliasing.
    float ANTIALIASING = 1.0 / DEVICE_PIXEL_RATIO / 2.0;

    vec2 a_extrude = a_data.xy - 128.0;
    float a_direction = mod(a_data.z, 4.0) - 1.0;

    v_linesofar = (floor(a_data.z / 4.0) + a_data.w * 64.0) * 2.0;

    vec2 pos = floor(a_pos_normal * 0.5);

    // x is 1 if it's a round cap, 0 otherwise
    // y is 1 if the normal points up, and -1 if it points down
    // We store these in the least significant bit of a_pos_normal
    mediump vec2 normal = a_pos_normal - 2.0 * pos;
    normal.y = normal.y * 2.0 - 1.0;
    v_normal = normal;

    // these transformations used to be applied in the JS and native code bases.
    // moved them into the shader for clarity and simplicity.
    gapwidth = gapwidth / 2.0;
    float halfwidth = width / 2.0;
    offset = -1.0 * offset;

    float inset = gapwidth + (gapwidth > 0.0 ? ANTIALIASING : 0.0);
    float outset = gapwidth + halfwidth * (gapwidth > 0.0 ? 2.0 : 1.0) + (halfwidth == 0.0 ? 0.0 : ANTIALIASING);

    // Scale the extrusion vector down to a normal and then up by the line width
    // of this vertex.
    mediump vec2 dist = outset * a_extrude * scale;

    // Calculate the offset when drawing a line that is to the side of the actual line.
    // We do this by creating a vector that points towards the extrude, but rotate
    // it when we're drawing round end points (a_direction = -1 or 1) since their
    // extrude vector points in another direction.
    mediump float u = 0.5 * a_direction;
    mediump float t = 1.0 - abs(u);
    mediump vec2 offset2 = offset * a_extrude * scale * normal.y * mat2(t, -u, u, t);

#ifdef PROJECTION_GLOBE
    float adjustedThickness = projectLineThickness(pos.y);
    vec4 projected_no_extrude = projectTile(pos + offset2 / u_ratio * adjustedThickness);
    vec2 extrudedPos = pos + (offset2 + dist) / u_ratio * adjustedThickness;
    gl_Position = projectTile(extrudedPos);
    v_tile_x = antimeridianClipX(extrudedPos);
    vec4 projected_extrude = gl_Position - projected_no_extrude;
#else
    vec4 projected_extrude = u_matrix * vec4(dist / u_ratio, 0.0, 0.0);
    gl_Position = u_matrix * vec4(pos + offset2 / u_ratio, 0.0, 1.0) + projected_extrude;
#endif

    // calculate how much the perspective view squishes or stretches the extrude
    float extrude_length_without_perspective = length(dist);
    float extrude_length_with_perspective = length(projected_extrude.xy / gl_Position.w * u_units_to_pixels);
    v_gamma_scale = extrude_length_without_perspective / extrude_length_with_perspective;

    v_width2 = vec2(outset, inset);
}
"#,
    fragment: r"in vec2 v_width2;
in vec2 v_normal;
in float v_gamma_scale;

#ifndef HAS_UNIFORM_u_color
in highp vec4 color;
#endif
#ifndef HAS_UNIFORM_u_blur
in lowp float blur;
#endif
#ifndef HAS_UNIFORM_u_opacity
in lowp float opacity;
#endif

#ifdef PROJECTION_GLOBE
in highp float v_tile_x;
#endif

void main() {
#ifdef PROJECTION_GLOBE
    if (clippedAtAntimeridian(v_tile_x)) {
        discard;
    }
#endif
    #ifdef HAS_UNIFORM_u_color
highp vec4 color = u_color;
#endif
    #ifdef HAS_UNIFORM_u_blur
lowp float blur = u_blur;
#endif
    #ifdef HAS_UNIFORM_u_opacity
lowp float opacity = u_opacity;
#endif

    // Calculate the distance of the pixel from the line in pixels.
    float dist = length(v_normal) * v_width2.s;

    // Calculate the antialiasing fade factor. This is either when fading in
    // the line in case of an offset line (v_width2.t) or when fading out
    // (v_width2.s)
    float blur2 = (blur + 1.0 / DEVICE_PIXEL_RATIO) * v_gamma_scale;
    float alpha = clamp(min(dist - (v_width2.t - blur2), v_width2.s - dist) / blur2, 0.0, 1.0);

    fragColor = color * (alpha * opacity);
}
",
    declarations: &LINE_DECLARATIONS,
};
