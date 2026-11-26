//! The GLSL, for Vulkan and D3D12, of the lines' programs.

use mln_gfx::shaders::source::GlslSource;

/// The line programs' `LINE_NORMAL_SCALE` and `MAX_LINE_DISTANCE`.
macro_rules! line_constants {
    () => {
        r#"
// The maximum allowed miter limit is 2.0 at the moment. the extrude normal is stored
// in a byte (-128..127). We scale regular normals up to length 63, but there are also
// "special" normals that have a bigger length (of up to 126 in this case).
#define LINE_NORMAL_SCALE (1.0 / (127 / 2))

// The attribute conveying progress along a line is scaled to [0, 2^15).
#define MAX_LINE_DISTANCE 32767.0
"#
    };
}

pub(crate) const LINE: GlslSource = GlslSource {
    vertex: concat!(
        line_constants!(),
        r"

layout(location = 0) out vec2 frag_normal;
layout(location = 1) out vec2 frag_width2;
layout(location = 2) out float frag_gamma_scale;

#if !defined(HAS_UNIFORM_u_color)
layout(location = 3) out vec4 frag_color;
#endif

#if !defined(HAS_UNIFORM_u_blur)
layout(location = 4) out float frag_blur;
#endif

#if !defined(HAS_UNIFORM_u_opacity)
layout(location = 5) out float frag_opacity;
#endif

#ifdef PROJECTION_GLOBE
layout(location = 6) out float frag_tile_x;
#endif

void main() {
    const LineDrawableUBO drawable = drawableVector.elements[constant.ubo_index];

#ifndef HAS_UNIFORM_u_color
    frag_color = unpack_mix_color(in_color, drawable.color_t);
#endif

#ifndef HAS_UNIFORM_u_blur
    frag_blur = unpack_mix_float(in_blur, drawable.blur_t);
#endif

#ifndef HAS_UNIFORM_u_opacity
    frag_opacity = unpack_mix_float(in_opacity, drawable.opacity_t);
#endif

#ifndef HAS_UNIFORM_u_gapwidth
    const float gapwidth = unpack_mix_float(in_gapwidth, drawable.gapwidth_t) / 2.0;
#else
    const float gapwidth = props.gapwidth / 2.0;
#endif

#ifndef HAS_UNIFORM_u_offset
    const float offset = unpack_mix_float(in_offset, drawable.offset_t) * -1.0;
#else
    const float offset = props.offset * -1.0;
#endif

#ifndef HAS_UNIFORM_u_width
    float width = unpack_mix_float(in_width, drawable.width_t);
#else
    float width = props.width;
#endif

    // the distance over which the line edge fades out.
    // Retina devices need a smaller distance to avoid aliasing.
    const float ANTIALIASING = 1.0 / DEVICE_PIXEL_RATIO / 2.0;

    vec2 a_extrude = in_data.xy - 128.0;
    float a_direction = mod(in_data.z, 4.0) - 1.0;
    vec2 pos = floor(in_pos_normal * 0.5);

    // x is 1 if it's a round cap, 0 otherwise
    // y is 1 if the normal points up, and -1 if it points down
    // We store these in the least significant bit of in_pos_normal
    vec2 normal = in_pos_normal - 2.0 * pos;
    frag_normal = vec2(normal.x, normal.y * 2.0 - 1.0);

    // these transformations used to be applied in the JS and native code bases.
    // moved them into the shader for clarity and simplicity.
    float halfwidth = width / 2.0;

    float inset = gapwidth + (gapwidth > 0.0 ? ANTIALIASING : 0.0);
    float outset = gapwidth + halfwidth * (gapwidth > 0.0 ? 2.0 : 1.0) + (halfwidth == 0.0 ? 0.0 : ANTIALIASING);

    // Scale the extrusion vector down to a normal and then up by the line width
    // of this vertex.
    vec2 dist = outset * a_extrude * LINE_NORMAL_SCALE;

    // Calculate the offset when drawing a line that is to the side of the actual line.
    // We do this by creating a vector that points towards the extrude, but rotate
    // it when we're drawing round end points (a_direction = -1 or 1) since their
    // extrude vector points in another direction.
    float u = 0.5 * a_direction;
    float t = 1.0 - abs(u);
    vec2 offset2 = offset * a_extrude * LINE_NORMAL_SCALE * frag_normal.y * mat2(t, -u, u, t);

#ifdef PROJECTION_GLOBE
    const float adjustedThickness = projectLineThickness(pos.y, projectionVector.elements[constant.ubo_index]);
    const vec4 projected_no_extrude = projectTile(pos + offset2 / drawable.ratio * adjustedThickness, projectionVector.elements[constant.ubo_index]);
    const vec2 extrudedPos = pos + (offset2 + dist) / drawable.ratio * adjustedThickness;
    gl_Position = projectTile(extrudedPos, projectionVector.elements[constant.ubo_index]);
    frag_tile_x = antimeridianClipX(extrudedPos, projectionVector.elements[constant.ubo_index]);
    vec4 projected_extrude = gl_Position - projected_no_extrude;
#else
    vec4 projected_extrude = drawable.matrix * vec4(dist / drawable.ratio, 0.0, 0.0);
    gl_Position = drawable.matrix * vec4(pos + offset2 / drawable.ratio, 0.0, 1.0) + projected_extrude;
#endif

    // calculate how much the perspective view squishes or stretches the extrude
    float extrude_length_without_perspective = length(dist);
    float extrude_length_with_perspective = length(projected_extrude.xy / gl_Position.w * paintParams.units_to_pixels);
    frag_gamma_scale = extrude_length_without_perspective / extrude_length_with_perspective;

    frag_width2 = vec2(outset, inset);
}
",
    ),
    fragment: r"

layout(location = 0) in vec2 frag_normal;
layout(location = 1) in vec2 frag_width2;
layout(location = 2) in float frag_gamma_scale;

#if !defined(HAS_UNIFORM_u_color)
layout(location = 3) in vec4 frag_color;
#endif

#if !defined(HAS_UNIFORM_u_blur)
layout(location = 4) in float frag_blur;
#endif

#if !defined(HAS_UNIFORM_u_opacity)
layout(location = 5) in float frag_opacity;
#endif

layout(location = 0) out vec4 out_color;

#ifdef PROJECTION_GLOBE
layout(location = 6) in float frag_tile_x;
#endif

void main() {
#ifdef PROJECTION_GLOBE
    if (clippedAtAntimeridian(frag_tile_x)) {
        discard;
    }
#endif

#ifdef HAS_UNIFORM_u_color
    vec4 color = props.color;
#else
    vec4 color = frag_color;
#endif

#ifdef HAS_UNIFORM_u_blur
    float blur = props.blur;
#else
    float blur = frag_blur;
#endif

#ifdef HAS_UNIFORM_u_opacity
    float opacity = props.opacity;
#else
    float opacity = frag_opacity;
#endif

    // Calculate the distance of the pixel from the line in pixels.
    float dist = length(frag_normal) * frag_width2.s;

    // Calculate the antialiasing fade factor. This is either when fading in
    // the line in case of an offset line (frag_width2.t) or when fading out
    // (frag_width2.s)
    float blur2 = (blur + 1.0 / DEVICE_PIXEL_RATIO) * frag_gamma_scale;
    float alpha = clamp(min(dist - (frag_width2.t - blur2), frag_width2.s - dist) / blur2, 0.0, 1.0);

    out_color = color * (alpha * opacity);
}
",
};

pub(crate) const LINE_GRADIENT: GlslSource = GlslSource {
    vertex: concat!(
        line_constants!(),
        r"

layout(location = 0) out vec2 frag_normal;
layout(location = 1) out vec2 frag_width2;
layout(location = 2) out float frag_gamma_scale;
layout(location = 3) out float frag_lineprogress;

#if !defined(HAS_UNIFORM_u_blur)
layout(location = 4) out float frag_blur;
#endif

#if !defined(HAS_UNIFORM_u_opacity)
layout(location = 5) out float frag_opacity;
#endif

#ifdef PROJECTION_GLOBE
layout(location = 6) out float frag_tile_x;
#endif

void main() {
    const LineGradientDrawableUBO drawable = drawableVector.elements[constant.ubo_index];

#ifndef HAS_UNIFORM_u_blur
    frag_blur = unpack_mix_float(in_blur, drawable.blur_t);
#endif

#ifndef HAS_UNIFORM_u_opacity
    frag_opacity = unpack_mix_float(in_opacity, drawable.opacity_t);
#endif

#ifndef HAS_UNIFORM_u_gapwidth
    float gapwidth = unpack_mix_float(in_gapwidth, drawable.gapwidth_t) / 2.0;
#else
    float gapwidth = props.gapwidth / 2.0;
#endif

#ifndef HAS_UNIFORM_u_offset
    const float offset = unpack_mix_float(in_offset, drawable.offset_t) * -1.0;
#else
    const float offset = props.offset * -1.0;
#endif

#ifndef HAS_UNIFORM_u_width
    float width = unpack_mix_float(in_width, drawable.width_t);
#else
    float width = props.width;
#endif

    // the distance over which the line edge fades out.
    // Retina devices need a smaller distance to avoid aliasing.
    const float ANTIALIASING = 1.0 / DEVICE_PIXEL_RATIO / 2.0;

    vec2 a_extrude = in_data.xy - 128.0;
    float a_direction = mod(in_data.z, 4.0) - 1.0;
    vec2 pos = floor(in_pos_normal * 0.5);
    frag_lineprogress = (floor(in_data.z / 4.0) + in_data.w * 64.0) * 2.0 / MAX_LINE_DISTANCE;

    // x is 1 if it's a round cap, 0 otherwise
    // y is 1 if the normal points up, and -1 if it points down
    // We store these in the least significant bit of in_pos_normal
    vec2 normal = in_pos_normal - 2.0 * pos;
    frag_normal = vec2(normal.x, normal.y * 2.0 - 1.0);

    // these transformations used to be applied in the JS and native code bases.
    // moved them into the shader for clarity and simplicity.
    float halfwidth = width / 2.0;

    float inset = gapwidth + (gapwidth > 0.0 ? ANTIALIASING : 0.0);
    float outset = gapwidth + halfwidth * (gapwidth > 0.0 ? 2.0 : 1.0) + (halfwidth == 0.0 ? 0.0 : ANTIALIASING);

    // Scale the extrusion vector down to a normal and then up by the line width
    // of this vertex.
    vec2 dist = outset * a_extrude * LINE_NORMAL_SCALE;

    // Calculate the offset when drawing a line that is to the side of the actual line.
    // We do this by creating a vector that points towards the extrude, but rotate
    // it when we're drawing round end points (a_direction = -1 or 1) since their
    // extrude vector points in another direction.
    float u = 0.5 * a_direction;
    float t = 1.0 - abs(u);
    vec2 offset2 = offset * a_extrude * LINE_NORMAL_SCALE * frag_normal.y * mat2(t, -u, u, t);

#ifdef PROJECTION_GLOBE
    const float adjustedThickness = projectLineThickness(pos.y, projectionVector.elements[constant.ubo_index]);
    const vec4 projected_no_extrude = projectTile(pos + offset2 / drawable.ratio * adjustedThickness, projectionVector.elements[constant.ubo_index]);
    const vec2 extrudedPos = pos + (offset2 + dist) / drawable.ratio * adjustedThickness;
    gl_Position = projectTile(extrudedPos, projectionVector.elements[constant.ubo_index]);
    frag_tile_x = antimeridianClipX(extrudedPos, projectionVector.elements[constant.ubo_index]);
    vec4 projected_extrude = gl_Position - projected_no_extrude;
#else
    vec4 projected_extrude = drawable.matrix * vec4(dist / drawable.ratio, 0.0, 0.0);
    gl_Position = drawable.matrix * vec4(pos + offset2 / drawable.ratio, 0.0, 1.0) + projected_extrude;
#endif

    // calculate how much the perspective view squishes or stretches the extrude
    float extrude_length_without_perspective = length(dist);
    float extrude_length_with_perspective = length(projected_extrude.xy / gl_Position.w * paintParams.units_to_pixels);
    frag_gamma_scale = extrude_length_without_perspective / extrude_length_with_perspective;

    frag_width2 = vec2(outset, inset);
}
",
    ),
    fragment: r"

layout(location = 0) in vec2 frag_normal;
layout(location = 1) in vec2 frag_width2;
layout(location = 2) in float frag_gamma_scale;
layout(location = 3) in float frag_lineprogress;

#if !defined(HAS_UNIFORM_u_blur)
layout(location = 4) in float frag_blur;
#endif

#if !defined(HAS_UNIFORM_u_opacity)
layout(location = 5) in float frag_opacity;
#endif

layout(location = 0) out vec4 out_color;

#ifdef PROJECTION_GLOBE
layout(location = 6) in float frag_tile_x;
#endif

void main() {
#ifdef PROJECTION_GLOBE
    if (clippedAtAntimeridian(frag_tile_x)) {
        discard;
    }
#endif

#ifdef HAS_UNIFORM_u_blur
    float blur = props.blur;
#else
    float blur = frag_blur;
#endif

#ifdef HAS_UNIFORM_u_opacity
    float opacity = props.opacity;
#else
    float opacity = frag_opacity;
#endif

    // Calculate the distance of the pixel from the line in pixels.
    float dist = length(frag_normal) * frag_width2.s;

    // Calculate the antialiasing fade factor. This is either when fading in
    // the line in case of an offset line (frag_width2.t) or when fading out
    // (frag_width2.s)
    float blur2 = (blur + 1.0 / DEVICE_PIXEL_RATIO) * frag_gamma_scale;
    float alpha = clamp(min(dist - (frag_width2.t - blur2), frag_width2.s - dist) / blur2, 0.0, 1.0);

    // For gradient lines, lineprogress is the ratio along the entire line,
    // scaled to [0, 2^15), and the gradient ramp is stored in a texture.
    const vec4 color = texture(sampler2D(image_texture, image_sampler), vec2(frag_lineprogress, 0.5));

    out_color = color * (alpha * opacity);
}
",
};

pub(crate) const LINE_PATTERN: GlslSource = GlslSource {
    vertex: concat!(
        line_constants!(),
        r"

layout(location = 0) out vec2 frag_normal;
layout(location = 1) out vec2 frag_width2;
layout(location = 2) out float frag_gamma_scale;
layout(location = 3) out float frag_linesofar;

#if !defined(HAS_UNIFORM_u_blur)
layout(location = 4) out float frag_blur;
#endif

#if !defined(HAS_UNIFORM_u_opacity)
layout(location = 5) out float frag_opacity;
#endif

#if !defined(HAS_UNIFORM_u_pattern_from)
layout(location = 6) flat out vec4 frag_pattern_from;
#endif

#if !defined(HAS_UNIFORM_u_pattern_to)
layout(location = 7) flat out vec4 frag_pattern_to;
#endif
#if !defined(HAS_UNIFORM_u_width)
layout(location = 8) out float frag_floorwidth;
#endif

#ifdef PROJECTION_GLOBE
layout(location = 9) out float frag_tile_x;
#endif

void main() {
    const LinePatternDrawableUBO drawable = drawableVector.elements[constant.ubo_index];

#ifndef HAS_UNIFORM_u_blur
    frag_blur = unpack_mix_float(in_blur, drawable.blur_t);
#endif

#ifndef HAS_UNIFORM_u_opacity
    frag_opacity = unpack_mix_float(in_opacity, drawable.opacity_t);
#endif

#ifndef HAS_UNIFORM_u_gapwidth
    const float gapwidth = unpack_mix_float(in_gapwidth, drawable.gapwidth_t) / 2.0;
#else
    const float gapwidth = props.gapwidth / 2.0;
#endif

#ifndef HAS_UNIFORM_u_offset
    const float offset = unpack_mix_float(in_offset, drawable.offset_t) * -1.0;
#else
    const float offset = props.offset * -1.0;
#endif

#ifndef HAS_UNIFORM_u_width
    float width = unpack_mix_float(in_width, drawable.width_t);
#else
    float width = props.width;
#endif

#ifndef HAS_UNIFORM_u_pattern_from
    frag_pattern_from = in_pattern_from;
#endif

#ifndef HAS_UNIFORM_u_pattern_to
    frag_pattern_to = in_pattern_to;
#endif
#ifndef HAS_UNIFORM_u_width
    frag_floorwidth = unpack_mix_float(in_width, drawable.floorwidth_t);
#endif

    // the distance over which the line edge fades out.
    // Retina devices need a smaller distance to avoid aliasing.
    const float ANTIALIASING = 1.0 / DEVICE_PIXEL_RATIO / 2.0;
    const float LINE_DISTANCE_SCALE = 2.0;

    vec2 a_extrude = in_data.xy - 128.0;
    float a_direction = mod(in_data.z, 4.0) - 1.0;
    vec2 pos = floor(in_pos_normal * 0.5);

    // x is 1 if it's a round cap, 0 otherwise
    // y is 1 if the normal points up, and -1 if it points down
    // We store these in the least significant bit of in_pos_normal
    vec2 normal = in_pos_normal - 2.0 * pos;
    frag_normal = vec2(normal.x, normal.y * 2.0 - 1.0);

    // these transformations used to be applied in the JS and native code bases.
    // moved them into the shader for clarity and simplicity.
    float halfwidth = width / 2.0;

    float inset = gapwidth + (gapwidth > 0.0 ? ANTIALIASING : 0.0);
    float outset = gapwidth + halfwidth * (gapwidth > 0.0 ? 2.0 : 1.0) + (halfwidth == 0.0 ? 0.0 : ANTIALIASING);

    // Scale the extrusion vector down to a normal and then up by the line width
    // of this vertex.
    vec2 dist = outset * a_extrude * LINE_NORMAL_SCALE;

    // Calculate the offset when drawing a line that is to the side of the actual line.
    // We do this by creating a vector that points towards the extrude, but rotate
    // it when we're drawing round end points (a_direction = -1 or 1) since their
    // extrude vector points in another direction.
    float u = 0.5 * a_direction;
    float t = 1.0 - abs(u);
    vec2 offset2 = offset * a_extrude * LINE_NORMAL_SCALE * frag_normal.y * mat2(t, -u, u, t);

#ifdef PROJECTION_GLOBE
    const float adjustedThickness = projectLineThickness(pos.y, projectionVector.elements[constant.ubo_index]);
    const vec4 projected_no_extrude = projectTile(pos + offset2 / drawable.ratio * adjustedThickness, projectionVector.elements[constant.ubo_index]);
    const vec2 extrudedPos = pos + (offset2 + dist) / drawable.ratio * adjustedThickness;
    gl_Position = projectTile(extrudedPos, projectionVector.elements[constant.ubo_index]);
    frag_tile_x = antimeridianClipX(extrudedPos, projectionVector.elements[constant.ubo_index]);
    vec4 projected_extrude = gl_Position - projected_no_extrude;
#else
    vec4 projected_extrude = drawable.matrix * vec4(dist / drawable.ratio, 0.0, 0.0);
    gl_Position = drawable.matrix * vec4(pos + offset2 / drawable.ratio, 0.0, 1.0) + projected_extrude;
#endif

    // calculate how much the perspective view squishes or stretches the extrude
    float extrude_length_without_perspective = length(dist);
    float extrude_length_with_perspective = length(projected_extrude.xy / gl_Position.w * paintParams.units_to_pixels);
    frag_gamma_scale = extrude_length_without_perspective / extrude_length_with_perspective;

    frag_width2 = vec2(outset, inset);
    frag_linesofar = (floor(in_data.z / 4.0) + in_data.w * 64.0) * LINE_DISTANCE_SCALE;
}
",
    ),
    fragment: r"

layout(location = 0) in vec2 frag_normal;
layout(location = 1) in vec2 frag_width2;
layout(location = 2) in float frag_gamma_scale;
layout(location = 3) in float frag_linesofar;

#if !defined(HAS_UNIFORM_u_blur)
layout(location = 4) in float frag_blur;
#endif

#if !defined(HAS_UNIFORM_u_opacity)
layout(location = 5) in float frag_opacity;
#endif

#if !defined(HAS_UNIFORM_u_pattern_from)
layout(location = 6) flat in vec4 frag_pattern_from;
#endif

#if !defined(HAS_UNIFORM_u_pattern_to)
layout(location = 7) flat in vec4 frag_pattern_to;
#endif
#if !defined(HAS_UNIFORM_u_width)
layout(location = 8) in float frag_floorwidth;
#endif

layout(location = 0) out vec4 out_color;

#ifdef PROJECTION_GLOBE
layout(location = 9) in float frag_tile_x;
#endif

void main() {
#ifdef PROJECTION_GLOBE
    if (clippedAtAntimeridian(frag_tile_x)) {
        discard;
    }
#endif

    const LinePatternTilePropsUBO tileProps = tilePropsVector.elements[constant.ubo_index];

#ifdef HAS_UNIFORM_u_blur
    const float blur = props.blur;
#else
    const float blur = frag_blur;
#endif

#ifdef HAS_UNIFORM_u_opacity
    const float opacity = props.opacity;
#else
    const float opacity = frag_opacity;
#endif

#ifdef HAS_UNIFORM_u_pattern_from
    const vec4 pattern_from = tileProps.pattern_from;
#else
    const vec4 pattern_from = frag_pattern_from;
#endif

#ifdef HAS_UNIFORM_u_pattern_to
    const vec4 pattern_to = tileProps.pattern_to;
#else
    const vec4 pattern_to = frag_pattern_to;
#endif

    const vec2 pattern_tl_a = pattern_from.xy;
    const vec2 pattern_br_a = pattern_from.zw;
    const vec2 pattern_tl_b = pattern_to.xy;
    const vec2 pattern_br_b = pattern_to.zw;

    const float pixelRatio = tileProps.scale.x;
    const float tileZoomRatio = tileProps.scale.y;
    const float fromScale = tileProps.scale.z;
    const float toScale = tileProps.scale.w;

    const vec2 display_size_a = vec2((pattern_br_a.x - pattern_tl_a.x) / pixelRatio, (pattern_br_a.y - pattern_tl_a.y) / pixelRatio);
    const vec2 display_size_b = vec2((pattern_br_b.x - pattern_tl_b.x) / pixelRatio, (pattern_br_b.y - pattern_tl_b.y) / pixelRatio);

    const vec2 pattern_size_a = vec2(display_size_a.x * fromScale / tileZoomRatio, display_size_a.y);
    const vec2 pattern_size_b = vec2(display_size_b.x * toScale / tileZoomRatio, display_size_b.y);

#if defined(HAS_UNIFORM_u_width)
    const float floorwidth = props.floorwidth;
#else
    const float floorwidth = frag_floorwidth;
#endif
    const float aspect_a = display_size_a.y / floorwidth;
    const float aspect_b = display_size_b.y / floorwidth;

    // Calculate the distance of the pixel from the line in pixels.
    const float dist = length(frag_normal) * frag_width2.x;

    // Calculate the antialiasing fade factor. This is either when fading in
    // the line in case of an offset line (frag_width2.y) or when fading out
    // (frag_width2.x)
    const float blur2 = (blur + 1.0 / DEVICE_PIXEL_RATIO) * frag_gamma_scale;
    const float alpha = clamp(min(dist - (frag_width2.y - blur2), frag_width2.x - dist) / blur2, 0.0, 1.0);

    const float y = 0.5 * frag_normal.y + 0.5;

    const vec2 texel_size = 1.0 / tileProps.texsize;
    const vec2 half_texel = 0.5 * texel_size;
    // A crossfade at 0 or 1 shows one pattern, so skip the other.
    vec4 color_a = vec4(0.0);
    if (tileProps.fade < 1.0) {
        const float x_a = mod(frag_linesofar / pattern_size_a.x * aspect_a, 1.0);
        vec2 pos_a = atlas_pos((pattern_tl_a - 1.0) * texel_size, (pattern_br_a - pattern_tl_a + 2.0) * texel_size * vec2(x_a, y));
        pos_a = clamp(pos_a, (pattern_tl_a - 1.0) * texel_size + half_texel, (pattern_br_a + 1.0) * texel_size - half_texel);
        color_a = texture(sampler2D(image_texture, image_sampler), pos_a);
    }
    vec4 color_b = vec4(0.0);
    if (tileProps.fade > 0.0) {
        const float x_b = mod(frag_linesofar / pattern_size_b.x * aspect_b, 1.0);
        vec2 pos_b = atlas_pos((pattern_tl_b - 1.0) * texel_size, (pattern_br_b - pattern_tl_b + 2.0) * texel_size * vec2(x_b, y));
        pos_b = clamp(pos_b, (pattern_tl_b - 1.0) * texel_size + half_texel, (pattern_br_b + 1.0) * texel_size - half_texel);
        color_b = texture(sampler2D(image_texture, image_sampler), pos_b);
    }

    const vec4 color = mix(color_a, color_b, tileProps.fade);

    out_color = color * (alpha * opacity);
}
",
};

pub(crate) const LINE_SDF: GlslSource = GlslSource {
    vertex: concat!(
        line_constants!(),
        r"

layout(location = 0) out vec2 frag_normal;
layout(location = 1) out vec2 frag_width2;
layout(location = 2) out float frag_gamma_scale;
layout(location = 3) out vec2 frag_tex_a;
layout(location = 4) out vec2 frag_tex_b;

#if !defined(HAS_UNIFORM_u_color)
layout(location = 5) out vec4 frag_color;
#endif

#if !defined(HAS_UNIFORM_u_blur)
layout(location = 6) out float frag_blur;
#endif

#if !defined(HAS_UNIFORM_u_opacity)
layout(location = 7) out float frag_opacity;
#endif

#if !defined(HAS_UNIFORM_u_floorwidth)
layout(location = 8) out float frag_floorwidth;
#endif

#ifdef PROJECTION_GLOBE
layout(location = 9) out float frag_tile_x;
#endif

#if !defined(HAS_UNIFORM_u_dasharray)
layout(location = 10) flat out float frag_dash_width;
#endif

void main() {
    const LineSDFDrawableUBO drawable = drawableVector.elements[constant.ubo_index];

#ifndef HAS_UNIFORM_u_color
    frag_color = unpack_mix_color(in_color, drawable.color_t);
#endif

#ifndef HAS_UNIFORM_u_blur
    frag_blur = unpack_mix_float(in_blur, drawable.blur_t);
#endif

#ifndef HAS_UNIFORM_u_opacity
    frag_opacity = unpack_mix_float(in_opacity, drawable.opacity_t);
#endif

#ifndef HAS_UNIFORM_u_floorwidth
    const float floorwidth = unpack_mix_float(in_floorwidth, drawable.floorwidth_t);
    frag_floorwidth = floorwidth;
#else
    const float floorwidth = props.floorwidth;
#endif

#ifndef HAS_UNIFORM_u_offset
    const float offset = unpack_mix_float(in_offset, drawable.offset_t) * -1.0;
#else
    const float offset = props.offset * -1.0;
#endif

#ifndef HAS_UNIFORM_u_width
    const float width = unpack_mix_float(in_width, drawable.width_t);
#else
    const float width = props.width;
#endif

#ifndef HAS_UNIFORM_u_gapwidth
    const float gapwidth = unpack_mix_float(in_gapwidth, drawable.gapwidth_t) / 2.0;
#else
    const float gapwidth = props.gapwidth / 2.0;
#endif

    // the distance over which the line edge fades out.
    // Retina devices need a smaller distance to avoid aliasing.
    const float ANTIALIASING = 1.0 / DEVICE_PIXEL_RATIO / 2.0;
    const float LINE_DISTANCE_SCALE = 2.0;

    vec2 a_extrude = in_data.xy - 128.0;
    float a_direction = mod(in_data.z, 4.0) - 1.0;
    float linesofar = (floor(in_data.z / 4.0) + in_data.w * 64.0) * LINE_DISTANCE_SCALE;
    vec2 pos = floor(in_pos_normal * 0.5);

    // x is 1 if it's a round cap, 0 otherwise
    // y is 1 if the normal points up, and -1 if it points down
    // We store these in the least significant bit of in_pos_normal
    vec2 normal = in_pos_normal - 2.0 * pos;
    frag_normal = vec2(normal.x, normal.y * 2.0 - 1.0);

    // these transformations used to be applied in the JS and native code bases.
    // moved them into the shader for clarity and simplicity.
    float halfwidth = width / 2.0;

    float inset = gapwidth + (gapwidth > 0.0 ? ANTIALIASING : 0.0);
    float outset = gapwidth + halfwidth * (gapwidth > 0.0 ? 2.0 : 1.0) + (halfwidth == 0.0 ? 0.0 : ANTIALIASING);

    // Scale the extrusion vector down to a normal and then up by the line width
    // of this vertex.
    vec2 dist = outset * a_extrude * LINE_NORMAL_SCALE;

    // Calculate the offset when drawing a line that is to the side of the actual line.
    // We do this by creating a vector that points towards the extrude, but rotate
    // it when we're drawing round end points (a_direction = -1 or 1) since their
    // extrude vector points in another direction.
    float u = 0.5 * a_direction;
    float t = 1.0 - abs(u);
    vec2 offset2 = offset * a_extrude * LINE_NORMAL_SCALE * frag_normal.y * mat2(t, -u, u, t);

#ifdef PROJECTION_GLOBE
    const float adjustedThickness = projectLineThickness(pos.y, projectionVector.elements[constant.ubo_index]);
    const vec4 projected_no_extrude = projectTile(pos + offset2 / drawable.ratio * adjustedThickness, projectionVector.elements[constant.ubo_index]);
    const vec2 extrudedPos = pos + (offset2 + dist) / drawable.ratio * adjustedThickness;
    gl_Position = projectTile(extrudedPos, projectionVector.elements[constant.ubo_index]);
    frag_tile_x = antimeridianClipX(extrudedPos, projectionVector.elements[constant.ubo_index]);
    vec4 projected_extrude = gl_Position - projected_no_extrude;
#else
    vec4 projected_extrude = drawable.matrix * vec4(dist / drawable.ratio, 0.0, 0.0);
    gl_Position = drawable.matrix * vec4(pos + offset2 / drawable.ratio, 0.0, 1.0) + projected_extrude;
#endif

    // calculate how much the perspective view squishes or stretches the extrude
    float extrude_length_without_perspective = length(dist);
    float extrude_length_with_perspective = length(projected_extrude.xy / gl_Position.w * paintParams.units_to_pixels);
    frag_gamma_scale = extrude_length_without_perspective / extrude_length_with_perspective;

    frag_width2 = vec2(outset, inset);

#ifdef HAS_UNIFORM_u_dasharray
    const vec2 patternscale_a = drawable.patternscale_a;
    const vec2 patternscale_b = drawable.patternscale_b;
    const float tex_y_a = drawable.tex_y_a;
    const float tex_y_b = drawable.tex_y_b;
#else
    // The dash attribute: the pattern's middle row, row span and length.
    const vec2 patternscale_a = vec2(drawable.patternscale_a.x / in_dasharray.z, -in_dasharray.y / 2.0 / drawable.atlas_height);
    const vec2 patternscale_b = vec2(drawable.patternscale_b.x / in_dasharray.z, patternscale_a.y);
    const float tex_y_a = (in_dasharray.x + 0.5) / drawable.atlas_height;
    const float tex_y_b = tex_y_a;
    frag_dash_width = in_dasharray.z;
#endif
    frag_tex_a = vec2(linesofar * patternscale_a.x / floorwidth, frag_normal.y * patternscale_a.y + tex_y_a);
    frag_tex_b = vec2(linesofar * patternscale_b.x / floorwidth, frag_normal.y * patternscale_b.y + tex_y_b);
}
",
    ),
    fragment: r"

layout(location = 0) in vec2 frag_normal;
layout(location = 1) in vec2 frag_width2;
layout(location = 2) in float frag_gamma_scale;
layout(location = 3) in vec2 frag_tex_a;
layout(location = 4) in vec2 frag_tex_b;

#if !defined(HAS_UNIFORM_u_color)
layout(location = 5) in vec4 frag_color;
#endif

#if !defined(HAS_UNIFORM_u_blur)
layout(location = 6) in float frag_blur;
#endif

#if !defined(HAS_UNIFORM_u_opacity)
layout(location = 7) in float frag_opacity;
#endif

#if !defined(HAS_UNIFORM_u_floorwidth)
layout(location = 8) in float frag_floorwidth;
#endif

layout(location = 0) out vec4 out_color;

#ifdef PROJECTION_GLOBE
layout(location = 9) in float frag_tile_x;
#endif

#if !defined(HAS_UNIFORM_u_dasharray)
layout(location = 10) flat in float frag_dash_width;
#endif

void main() {
#ifdef PROJECTION_GLOBE
    if (clippedAtAntimeridian(frag_tile_x)) {
        discard;
    }
#endif

    const LineSDFTilePropsUBO tileProps = tilePropsVector.elements[constant.ubo_index];

#ifdef HAS_UNIFORM_u_color
    const vec4 color = props.color;
#else
    const vec4 color = frag_color;
#endif

#ifdef HAS_UNIFORM_u_blur
    const float blur = props.blur;
#else
    const float blur = frag_blur;
#endif

#ifdef HAS_UNIFORM_u_opacity
    const float opacity = props.opacity;
#else
    const float opacity = frag_opacity;
#endif

#ifdef HAS_UNIFORM_u_floorwidth
    const float floorwidth = props.floorwidth;
#else
    const float floorwidth = frag_floorwidth;
#endif

    // Calculate the distance of the pixel from the line in pixels.
    const float dist = length(frag_normal) * frag_width2.x;

    // Calculate the antialiasing fade factor. This is either when fading in the
    // line in case of an offset line (`v_width2.y`) or when fading out (`v_width2.x`)
    const float blur2 = (blur + 1.0 / DEVICE_PIXEL_RATIO) * frag_gamma_scale;

    const float sdfdist_a = texture(sampler2D(image_texture, image_sampler), frag_tex_a).a;
    const float sdfdist_b = texture(sampler2D(image_texture, image_sampler), frag_tex_b).a;
    const float sdfdist = mix(sdfdist_a, sdfdist_b, tileProps.mix);
#ifdef HAS_UNIFORM_u_dasharray
    const float sdfgamma = tileProps.sdfgamma;
#else
    const float sdfgamma = tileProps.sdfgamma / frag_dash_width;
#endif
    const float alpha = clamp(min(dist - (frag_width2.y - blur2), frag_width2.x - dist) / blur2, 0.0, 1.0) *
                        smoothstep(0.5 - sdfgamma / floorwidth, 0.5 + sdfgamma / floorwidth, sdfdist);

    out_color = color * (alpha * opacity);
}
",
};

pub(crate) const LINE_GRADIENT_SDF: GlslSource = GlslSource {
    vertex: concat!(
        line_constants!(),
        r"

layout(location = 0) out vec2 frag_normal;
layout(location = 1) out vec2 frag_width2;
layout(location = 2) out float frag_gamma_scale;
layout(location = 3) out vec2 frag_tex_a;
layout(location = 4) out vec2 frag_tex_b;
layout(location = 5) out float frag_lineprogress;

#if !defined(HAS_UNIFORM_u_blur)
layout(location = 6) out float frag_blur;
#endif

#if !defined(HAS_UNIFORM_u_opacity)
layout(location = 7) out float frag_opacity;
#endif

#if !defined(HAS_UNIFORM_u_floorwidth)
layout(location = 8) out float frag_floorwidth;
#endif

#ifdef PROJECTION_GLOBE
layout(location = 9) out float frag_tile_x;
#endif

#if !defined(HAS_UNIFORM_u_dasharray)
layout(location = 10) flat out float frag_dash_width;
#endif

void main() {
    const LineSDFDrawableUBO drawable = drawableVector.elements[constant.ubo_index];

#ifndef HAS_UNIFORM_u_blur
    frag_blur = unpack_mix_float(in_blur, drawable.blur_t);
#endif

#ifndef HAS_UNIFORM_u_opacity
    frag_opacity = unpack_mix_float(in_opacity, drawable.opacity_t);
#endif

#ifndef HAS_UNIFORM_u_floorwidth
    const float floorwidth = unpack_mix_float(in_floorwidth, drawable.floorwidth_t);
    frag_floorwidth = floorwidth;
#else
    const float floorwidth = props.floorwidth;
#endif

#ifndef HAS_UNIFORM_u_offset
    const float offset = unpack_mix_float(in_offset, drawable.offset_t) * -1.0;
#else
    const float offset = props.offset * -1.0;
#endif

#ifndef HAS_UNIFORM_u_width
    const float width = unpack_mix_float(in_width, drawable.width_t);
#else
    const float width = props.width;
#endif

#ifndef HAS_UNIFORM_u_gapwidth
    const float gapwidth = unpack_mix_float(in_gapwidth, drawable.gapwidth_t) / 2.0;
#else
    const float gapwidth = props.gapwidth / 2.0;
#endif

    // the distance over which the line edge fades out.
    // Retina devices need a smaller distance to avoid aliasing.
    const float ANTIALIASING = 1.0 / DEVICE_PIXEL_RATIO / 2.0;
    const float LINE_DISTANCE_SCALE = 2.0;

    vec2 a_extrude = in_data.xy - 128.0;
    float a_direction = mod(in_data.z, 4.0) - 1.0;
    float linesofar = (floor(in_data.z / 4.0) + in_data.w * 64.0) * LINE_DISTANCE_SCALE;
    frag_lineprogress = (floor(in_data.z / 4.0) + in_data.w * 64.0) * 2.0 / MAX_LINE_DISTANCE;
    vec2 pos = floor(in_pos_normal * 0.5);

    // x is 1 if it's a round cap, 0 otherwise
    // y is 1 if the normal points up, and -1 if it points down
    // We store these in the least significant bit of in_pos_normal
    vec2 normal = in_pos_normal - 2.0 * pos;
    frag_normal = vec2(normal.x, normal.y * 2.0 - 1.0);

    // these transformations used to be applied in the JS and native code bases.
    // moved them into the shader for clarity and simplicity.
    float halfwidth = width / 2.0;

    float inset = gapwidth + (gapwidth > 0.0 ? ANTIALIASING : 0.0);
    float outset = gapwidth + halfwidth * (gapwidth > 0.0 ? 2.0 : 1.0) + (halfwidth == 0.0 ? 0.0 : ANTIALIASING);

    // Scale the extrusion vector down to a normal and then up by the line width
    // of this vertex.
    vec2 dist = outset * a_extrude * LINE_NORMAL_SCALE;

    // Calculate the offset when drawing a line that is to the side of the actual line.
    // We do this by creating a vector that points towards the extrude, but rotate
    // it when we're drawing round end points (a_direction = -1 or 1) since their
    // extrude vector points in another direction.
    float u = 0.5 * a_direction;
    float t = 1.0 - abs(u);
    vec2 offset2 = offset * a_extrude * LINE_NORMAL_SCALE * frag_normal.y * mat2(t, -u, u, t);

#ifdef PROJECTION_GLOBE
    const float adjustedThickness = projectLineThickness(pos.y, projectionVector.elements[constant.ubo_index]);
    const vec4 projected_no_extrude = projectTile(pos + offset2 / drawable.ratio * adjustedThickness, projectionVector.elements[constant.ubo_index]);
    const vec2 extrudedPos = pos + (offset2 + dist) / drawable.ratio * adjustedThickness;
    gl_Position = projectTile(extrudedPos, projectionVector.elements[constant.ubo_index]);
    frag_tile_x = antimeridianClipX(extrudedPos, projectionVector.elements[constant.ubo_index]);
    vec4 projected_extrude = gl_Position - projected_no_extrude;
#else
    vec4 projected_extrude = drawable.matrix * vec4(dist / drawable.ratio, 0.0, 0.0);
    gl_Position = drawable.matrix * vec4(pos + offset2 / drawable.ratio, 0.0, 1.0) + projected_extrude;
#endif

    // calculate how much the perspective view squishes or stretches the extrude
    float extrude_length_without_perspective = length(dist);
    float extrude_length_with_perspective = length(projected_extrude.xy / gl_Position.w * paintParams.units_to_pixels);
    frag_gamma_scale = extrude_length_without_perspective / extrude_length_with_perspective;

    frag_width2 = vec2(outset, inset);

#ifdef HAS_UNIFORM_u_dasharray
    const vec2 patternscale_a = drawable.patternscale_a;
    const vec2 patternscale_b = drawable.patternscale_b;
    const float tex_y_a = drawable.tex_y_a;
    const float tex_y_b = drawable.tex_y_b;
#else
    // The dash attribute: the pattern's middle row, row span and length.
    const vec2 patternscale_a = vec2(drawable.patternscale_a.x / in_dasharray.z, -in_dasharray.y / 2.0 / drawable.atlas_height);
    const vec2 patternscale_b = vec2(drawable.patternscale_b.x / in_dasharray.z, patternscale_a.y);
    const float tex_y_a = (in_dasharray.x + 0.5) / drawable.atlas_height;
    const float tex_y_b = tex_y_a;
    frag_dash_width = in_dasharray.z;
#endif
    frag_tex_a = vec2(linesofar * patternscale_a.x / floorwidth, frag_normal.y * patternscale_a.y + tex_y_a);
    frag_tex_b = vec2(linesofar * patternscale_b.x / floorwidth, frag_normal.y * patternscale_b.y + tex_y_b);
}
",
    ),
    fragment: r"

layout(location = 0) in vec2 frag_normal;
layout(location = 1) in vec2 frag_width2;
layout(location = 2) in float frag_gamma_scale;
layout(location = 3) in vec2 frag_tex_a;
layout(location = 4) in vec2 frag_tex_b;
layout(location = 5) in float frag_lineprogress;

#if !defined(HAS_UNIFORM_u_blur)
layout(location = 6) in float frag_blur;
#endif

#if !defined(HAS_UNIFORM_u_opacity)
layout(location = 7) in float frag_opacity;
#endif

#if !defined(HAS_UNIFORM_u_floorwidth)
layout(location = 8) in float frag_floorwidth;
#endif

#ifdef PROJECTION_GLOBE
layout(location = 9) in float frag_tile_x;
#endif

#if !defined(HAS_UNIFORM_u_dasharray)
layout(location = 10) flat in float frag_dash_width;
#endif

layout(location = 0) out vec4 out_color;

void main() {
#ifdef PROJECTION_GLOBE
    if (clippedAtAntimeridian(frag_tile_x)) {
        discard;
    }
#endif

    const LineSDFTilePropsUBO tileProps = tilePropsVector.elements[constant.ubo_index];

#ifdef HAS_UNIFORM_u_blur
    const float blur = props.blur;
#else
    const float blur = frag_blur;
#endif

#ifdef HAS_UNIFORM_u_opacity
    const float opacity = props.opacity;
#else
    const float opacity = frag_opacity;
#endif

#ifdef HAS_UNIFORM_u_floorwidth
    const float floorwidth = props.floorwidth;
#else
    const float floorwidth = frag_floorwidth;
#endif

    // Calculate the distance of the pixel from the line in pixels.
    const float dist = length(frag_normal) * frag_width2.x;

    // Calculate the antialiasing fade factor. This is either when fading in the
    // line in case of an offset line (`v_width2.y`) or when fading out (`v_width2.x`)
    const float blur2 = (blur + 1.0 / DEVICE_PIXEL_RATIO) * frag_gamma_scale;

    const float sdfdist_a = texture(sampler2D(image_dash_texture, image_dash_sampler), frag_tex_a).a;
    const float sdfdist_b = texture(sampler2D(image_dash_texture, image_dash_sampler), frag_tex_b).a;
    const float sdfdist = mix(sdfdist_a, sdfdist_b, tileProps.mix);
#ifdef HAS_UNIFORM_u_dasharray
    const float sdfgamma = tileProps.sdfgamma;
#else
    const float sdfgamma = tileProps.sdfgamma / frag_dash_width;
#endif
    const float alpha = clamp(min(dist - (frag_width2.y - blur2), frag_width2.x - dist) / blur2, 0.0, 1.0) *
                        smoothstep(0.5 - sdfgamma / floorwidth, 0.5 + sdfgamma / floorwidth, sdfdist);

    // The gradient ramp is stored in a texture indexed by the ratio along the entire line.
    const vec4 color = texture(sampler2D(image_texture, image_sampler), vec2(frag_lineprogress, 0.5));

    out_color = color * (alpha * opacity);
}
",
};
