//! The programs of the `line` layer type in GLSL, which `mln_glsl` parses for `mln_vulkan` and `mln_d3d12`.

use mln_gfx::shaders::source::glsl::Glsl;

/// What every vertex stage of the type reads: the scale of the extrusion
/// vectors, the width of a line on the globe, and the tile x that clips the
/// zoom 0 tile's lines at the antimeridian.
macro_rules! line_vertex_prelude {
    () => {
        r"
// The maximum allowed miter limit is 2.0 at the moment. the extrude normal is stored
// in a byte (-128..127). We scale regular normals up to length 63, but there are also
// special normals that have a bigger length (of up to 126 in this case).
#define LINE_NORMAL_SCALE (1.0 / (127 / 2))

#ifdef PROJECTION_GLOBE

float projectLineThickness(float tileY, ProjectionUBO projection) {
    const float thickness = 1.0 / circumferenceRatioAtTileY(tileY, projection);
    if (projection.transition < 0.999) {
        return mix(1.0, thickness, projection.transition);
    }
    return thickness;
}

// The zoom 0 tile's buffer wraps around the planet onto the tile itself, so the line shaders discard the fragments
// beyond its X extent; every other tile (a mercator width below the whole world's) hands on a value inside it.
float antimeridianClipX(vec2 posInTile, ProjectionUBO projection) {
    return projection.tile_mercator_coords.z * 8192.0 >= 1.0 ? posInTile.x : 0.0;
}

#else

float projectLineThickness(float tileY, ProjectionUBO projection) {
    return 1.0;
}

#endif
"
    };
}

/// What every fragment stage of the type reads: whether a fragment is past
/// the zoom 0 tile's x extent, see `antimeridianClipX`.
macro_rules! line_fragment_prelude {
    () => {
        r"
#ifdef PROJECTION_GLOBE
bool clippedAtAntimeridian(float tileX) {
    return tileX < 0.0 || tileX >= 8192.0;
}
#endif
"
    };
}

const LINE_SHADE_PRELUDE: &str = r"

#define idLineDrawableUBO           idDrawableReservedVertexOnlyUBO
#define idLineTilePropsUBO          idDrawableReservedFragmentOnlyUBO
#define idLineEvaluatedPropsUBO     layerUBOStartId

// The attribute conveying progress along a line is scaled to [0, 2^15).
#define MAX_LINE_DISTANCE 32767.0

";

pub(super) const LINE_SHADER: Glsl = Glsl {
    prelude: LINE_SHADE_PRELUDE,
    vertex: concat!(
        line_vertex_prelude!(),
        r"

layout(location = 0) in ivec2 in_pos_normal;
layout(location = 1) in uvec4 in_data;

#property attribute color

#property attribute blur

#property attribute opacity

#property attribute gapwidth

#property attribute offset

#property attribute width

layout(push_constant) uniform Constants {
    int ubo_index;
} constant;

layout(std140, set = LAYER_SET_INDEX, binding = idLineDrawableUBO) readonly buffer LineDrawableUBOVector {
    LineDrawableUBO drawable_ubo[];
} drawableVector;

layout(std140, set = LAYER_SET_INDEX, binding = idProjectionUBO) readonly buffer ProjectionUBOVector {
    ProjectionUBO projection_ubo[];
} projectionVector;

layout(set = LAYER_SET_INDEX, binding = idLineEvaluatedPropsUBO) uniform LineEvaluatedPropsUBOBlock {
    LineEvaluatedPropsUBO props;
};

layout(location = 0) out vec2 frag_normal;
layout(location = 1) out vec2 frag_width2;
layout(location = 2) out float frag_gamma_scale;

#property varying color 3

#property varying blur 4

#property varying opacity 5

#ifdef PROJECTION_GLOBE
layout(location = 6) out float frag_tile_x;
#endif

void main() {
    const LineDrawableUBO drawable = drawableVector.drawable_ubo[constant.ubo_index];

    #property pass color

    #property pass blur

    #property pass opacity

    const float gapwidth = $props.gapwidth / 2.0;

    const float offset = $props.offset * -1.0;

    float width = $props.width;

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
    const float adjustedThickness = projectLineThickness(pos.y, projectionVector.projection_ubo[constant.ubo_index]);
    const vec4 projected_no_extrude = projectTile(pos + offset2 / drawable.ratio * adjustedThickness, projectionVector.projection_ubo[constant.ubo_index]);
    const vec2 extrudedPos = pos + (offset2 + dist) / drawable.ratio * adjustedThickness;
    gl_Position = projectTile(extrudedPos, projectionVector.projection_ubo[constant.ubo_index]);
    frag_tile_x = antimeridianClipX(extrudedPos, projectionVector.projection_ubo[constant.ubo_index]);
    vec4 projected_extrude = gl_Position - projected_no_extrude;
#else
    vec4 projected_extrude = drawable.matrix * vec4(dist / drawable.ratio, 0.0, 0.0);
    gl_Position = drawable.matrix * vec4(pos + offset2 / drawable.ratio, 0.0, 1.0) + projected_extrude;
#endif

    // calculate how much the perspective view squishes or stretches the extrude
    float extrude_length_without_perspective = length(dist);
    float extrude_length_with_perspective = length(projected_extrude.xy / gl_Position.w * view.units_to_pixels);
    frag_gamma_scale = extrude_length_without_perspective / extrude_length_with_perspective;

    frag_width2 = vec2(outset, inset);
}
"
    ),
    fragment: concat!(
        line_fragment_prelude!(),
        r"

layout(location = 0) in vec2 frag_normal;
layout(location = 1) in vec2 frag_width2;
layout(location = 2) in float frag_gamma_scale;

#property varying color 3

#property varying blur 4

#property varying opacity 5

layout(location = 0) out vec4 out_color;

layout(set = LAYER_SET_INDEX, binding = idLineEvaluatedPropsUBO) uniform LineEvaluatedPropsUBOBlock {
    LineEvaluatedPropsUBO props;
};

#ifdef PROJECTION_GLOBE
layout(location = 6) in float frag_tile_x;
#endif

void main() {
#ifdef PROJECTION_GLOBE
    if (clippedAtAntimeridian(frag_tile_x)) {
        discard;
    }
#endif

#ifdef OVERDRAW_INSPECTOR
    out_color = vec4(1.0);
    return;
#endif

    vec4 color = $props.color;

    float blur = $props.blur;

    float opacity = $props.opacity;

    // Calculate the distance of the pixel from the line in pixels.
    float dist = length(frag_normal) * frag_width2.s;

    // Calculate the antialiasing fade factor. This is either when fading in
    // the line in case of an offset line (frag_width2.t) or when fading out
    // (frag_width2.s)
    float blur2 = (blur + 1.0 / DEVICE_PIXEL_RATIO) * frag_gamma_scale;
    float alpha = clamp(min(dist - (frag_width2.t - blur2), frag_width2.s - dist) / blur2, 0.0, 1.0);

    out_color = color * (alpha * opacity);
}
"
    ),
};

pub(super) const LINE_GRADIENT_SHADER: Glsl = Glsl {
    prelude: LINE_SHADE_PRELUDE,
    vertex: concat!(
        line_vertex_prelude!(),
        r"

layout(location = 0) in ivec2 in_pos_normal;
layout(location = 1) in uvec4 in_data;

#property attribute blur

#property attribute opacity

#property attribute gapwidth

#property attribute offset

#property attribute width

layout(push_constant) uniform Constants {
    int ubo_index;
} constant;

layout(std140, set = LAYER_SET_INDEX, binding = idLineDrawableUBO) readonly buffer LineGradientDrawableUBOVector {
    LineGradientDrawableUBO drawable_ubo[];
} drawableVector;

layout(std140, set = LAYER_SET_INDEX, binding = idProjectionUBO) readonly buffer ProjectionUBOVector {
    ProjectionUBO projection_ubo[];
} projectionVector;

layout(set = LAYER_SET_INDEX, binding = idLineEvaluatedPropsUBO) uniform LineEvaluatedPropsUBOBlock {
    LineEvaluatedPropsUBO props;
};

layout(location = 0) out vec2 frag_normal;
layout(location = 1) out vec2 frag_width2;
layout(location = 2) out float frag_gamma_scale;
layout(location = 3) out float frag_lineprogress;

#property varying blur 4

#property varying opacity 5

#ifdef PROJECTION_GLOBE
layout(location = 6) out float frag_tile_x;
#endif

void main() {
    const LineGradientDrawableUBO drawable = drawableVector.drawable_ubo[constant.ubo_index];

    #property pass blur

    #property pass opacity

    float gapwidth = $props.gapwidth / 2.0;

    const float offset = $props.offset * -1.0;

    float width = $props.width;

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
    const float adjustedThickness = projectLineThickness(pos.y, projectionVector.projection_ubo[constant.ubo_index]);
    const vec4 projected_no_extrude = projectTile(pos + offset2 / drawable.ratio * adjustedThickness, projectionVector.projection_ubo[constant.ubo_index]);
    const vec2 extrudedPos = pos + (offset2 + dist) / drawable.ratio * adjustedThickness;
    gl_Position = projectTile(extrudedPos, projectionVector.projection_ubo[constant.ubo_index]);
    frag_tile_x = antimeridianClipX(extrudedPos, projectionVector.projection_ubo[constant.ubo_index]);
    vec4 projected_extrude = gl_Position - projected_no_extrude;
#else
    vec4 projected_extrude = drawable.matrix * vec4(dist / drawable.ratio, 0.0, 0.0);
    gl_Position = drawable.matrix * vec4(pos + offset2 / drawable.ratio, 0.0, 1.0) + projected_extrude;
#endif

    // calculate how much the perspective view squishes or stretches the extrude
    float extrude_length_without_perspective = length(dist);
    float extrude_length_with_perspective = length(projected_extrude.xy / gl_Position.w * view.units_to_pixels);
    frag_gamma_scale = extrude_length_without_perspective / extrude_length_with_perspective;

    frag_width2 = vec2(outset, inset);
}
"
    ),
    fragment: concat!(
        line_fragment_prelude!(),
        r"

layout(location = 0) in vec2 frag_normal;
layout(location = 1) in vec2 frag_width2;
layout(location = 2) in float frag_gamma_scale;
layout(location = 3) in float frag_lineprogress;

#property varying blur 4

#property varying opacity 5

layout(set = DRAWABLE_IMAGE_SET_INDEX, binding = 0) uniform texture2D image0_texture;
layout(set = DRAWABLE_IMAGE_SET_INDEX, binding = samplerBindingStartId) uniform sampler image0_sampler;

layout(location = 0) out vec4 out_color;

layout(set = LAYER_SET_INDEX, binding = idLineEvaluatedPropsUBO) uniform LineEvaluatedPropsUBOBlock {
    LineEvaluatedPropsUBO props;
};

#ifdef PROJECTION_GLOBE
layout(location = 6) in float frag_tile_x;
#endif

void main() {
#ifdef PROJECTION_GLOBE
    if (clippedAtAntimeridian(frag_tile_x)) {
        discard;
    }
#endif

#ifdef OVERDRAW_INSPECTOR
    out_color = vec4(1.0);
    return;
#endif

    float blur = $props.blur;

    float opacity = $props.opacity;

    // Calculate the distance of the pixel from the line in pixels.
    float dist = length(frag_normal) * frag_width2.s;

    // Calculate the antialiasing fade factor. This is either when fading in
    // the line in case of an offset line (frag_width2.t) or when fading out
    // (frag_width2.s)
    float blur2 = (blur + 1.0 / DEVICE_PIXEL_RATIO) * frag_gamma_scale;
    float alpha = clamp(min(dist - (frag_width2.t - blur2), frag_width2.s - dist) / blur2, 0.0, 1.0);

    // For gradient lines, lineprogress is the ratio along the entire line,
    // scaled to [0, 2^15), and the gradient ramp is stored in a texture.
    const vec4 color = texture(sampler2D(image0_texture, image0_sampler), vec2(frag_lineprogress, 0.5));

    out_color = color * (alpha * opacity);
}
"
    ),
};

pub(super) const LINE_PATTERN_SHADER: Glsl = Glsl {
    prelude: LINE_SHADE_PRELUDE,
    vertex: concat!(
        line_vertex_prelude!(),
        r"

layout(location = 0) in ivec2 in_pos_normal;
layout(location = 1) in uvec4 in_data;

#property attribute blur

#property attribute opacity

#property attribute gapwidth

#property attribute offset

#property attribute width

#property attribute pattern_from

#property attribute pattern_to

layout(push_constant) uniform Constants {
    int ubo_index;
} constant;

layout(std140, set = LAYER_SET_INDEX, binding = idLineDrawableUBO) readonly buffer LinePatternDrawableUBOVector {
    LinePatternDrawableUBO drawable_ubo[];
} drawableVector;

layout(std140, set = LAYER_SET_INDEX, binding = idProjectionUBO) readonly buffer ProjectionUBOVector {
    ProjectionUBO projection_ubo[];
} projectionVector;

layout(set = LAYER_SET_INDEX, binding = idLineEvaluatedPropsUBO) uniform LineEvaluatedPropsUBOBlock {
    LineEvaluatedPropsUBO props;
};

layout(location = 0) out vec2 frag_normal;
layout(location = 1) out vec2 frag_width2;
layout(location = 2) out float frag_gamma_scale;
layout(location = 3) out float frag_linesofar;

#property varying blur 4

#property varying opacity 5

#property varying pattern_from 6 flat

#property varying pattern_to 7 flat
#if !defined(HAS_UNIFORM_u_width)
layout(location = 8) out float frag_floorwidth;
#endif

#ifdef PROJECTION_GLOBE
layout(location = 9) out float frag_tile_x;
#endif

void main() {
    const LinePatternDrawableUBO drawable = drawableVector.drawable_ubo[constant.ubo_index];

    #property pass blur

    #property pass opacity

    const float gapwidth = $props.gapwidth / 2.0;

    const float offset = $props.offset * -1.0;

    float width = $props.width;

    #property pass pattern_from

    #property pass pattern_to
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
    const float adjustedThickness = projectLineThickness(pos.y, projectionVector.projection_ubo[constant.ubo_index]);
    const vec4 projected_no_extrude = projectTile(pos + offset2 / drawable.ratio * adjustedThickness, projectionVector.projection_ubo[constant.ubo_index]);
    const vec2 extrudedPos = pos + (offset2 + dist) / drawable.ratio * adjustedThickness;
    gl_Position = projectTile(extrudedPos, projectionVector.projection_ubo[constant.ubo_index]);
    frag_tile_x = antimeridianClipX(extrudedPos, projectionVector.projection_ubo[constant.ubo_index]);
    vec4 projected_extrude = gl_Position - projected_no_extrude;
#else
    vec4 projected_extrude = drawable.matrix * vec4(dist / drawable.ratio, 0.0, 0.0);
    gl_Position = drawable.matrix * vec4(pos + offset2 / drawable.ratio, 0.0, 1.0) + projected_extrude;
#endif

    // calculate how much the perspective view squishes or stretches the extrude
    float extrude_length_without_perspective = length(dist);
    float extrude_length_with_perspective = length(projected_extrude.xy / gl_Position.w * view.units_to_pixels);
    frag_gamma_scale = extrude_length_without_perspective / extrude_length_with_perspective;

    frag_width2 = vec2(outset, inset);
    frag_linesofar = (floor(in_data.z / 4.0) + in_data.w * 64.0) * LINE_DISTANCE_SCALE;
}
"
    ),
    fragment: concat!(
        line_fragment_prelude!(),
        r"

layout(location = 0) in vec2 frag_normal;
layout(location = 1) in vec2 frag_width2;
layout(location = 2) in float frag_gamma_scale;
layout(location = 3) in float frag_linesofar;

#property varying blur 4

#property varying opacity 5

#property varying pattern_from 6 flat

#property varying pattern_to 7 flat
#if !defined(HAS_UNIFORM_u_width)
layout(location = 8) in float frag_floorwidth;
#endif

layout(location = 0) out vec4 out_color;

layout(push_constant) uniform Constants {
    int ubo_index;
} constant;

layout(std140, set = LAYER_SET_INDEX, binding = idLineTilePropsUBO) readonly buffer LinePatternTilePropertiesUBOVector {
    LinePatternTilePropsUBO tile_props_ubo[];
} tilePropsVector;

layout(set = LAYER_SET_INDEX, binding = idLineEvaluatedPropsUBO) uniform LineEvaluatedPropsUBOBlock {
    LineEvaluatedPropsUBO props;
};

layout(set = DRAWABLE_IMAGE_SET_INDEX, binding = 0) uniform texture2D image0_texture;
layout(set = DRAWABLE_IMAGE_SET_INDEX, binding = samplerBindingStartId) uniform sampler image0_sampler;

#ifdef PROJECTION_GLOBE
layout(location = 9) in float frag_tile_x;
#endif

void main() {
#ifdef PROJECTION_GLOBE
    if (clippedAtAntimeridian(frag_tile_x)) {
        discard;
    }
#endif

#ifdef OVERDRAW_INSPECTOR
    out_color = vec4(1.0);
    return;
#else

    const LinePatternTilePropsUBO tileProps = tilePropsVector.tile_props_ubo[constant.ubo_index];

    const float blur = $props.blur;

    const float opacity = $props.opacity;

    const vec4 pattern_from = $tileProps.pattern_from;

    const vec4 pattern_to = $tileProps.pattern_to;

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
        color_a = texture(sampler2D(image0_texture, image0_sampler), pos_a);
    }
    vec4 color_b = vec4(0.0);
    if (tileProps.fade > 0.0) {
        const float x_b = mod(frag_linesofar / pattern_size_b.x * aspect_b, 1.0);
        vec2 pos_b = atlas_pos((pattern_tl_b - 1.0) * texel_size, (pattern_br_b - pattern_tl_b + 2.0) * texel_size * vec2(x_b, y));
        pos_b = clamp(pos_b, (pattern_tl_b - 1.0) * texel_size + half_texel, (pattern_br_b + 1.0) * texel_size - half_texel);
        color_b = texture(sampler2D(image0_texture, image0_sampler), pos_b);
    }

    const vec4 color = mix(color_a, color_b, tileProps.fade);

    out_color = color * (alpha * opacity);
#endif
}
"
    ),
};

pub(super) const LINE_SDF_SHADER: Glsl = Glsl {
    prelude: LINE_SHADE_PRELUDE,
    vertex: concat!(
        line_vertex_prelude!(),
        r"

layout(location = 0) in ivec2 in_pos_normal;
layout(location = 1) in uvec4 in_data;

#property attribute color

#property attribute blur

#property attribute opacity

#property attribute gapwidth

#property attribute offset

#property attribute width

#property attribute floorwidth

#property attribute dasharray

layout(push_constant) uniform Constants {
    int ubo_index;
} constant;

layout(std140, set = LAYER_SET_INDEX, binding = idLineDrawableUBO) readonly buffer LineSDFDrawableUBOVector {
    LineSDFDrawableUBO drawable_ubo[];
} drawableVector;

layout(std140, set = LAYER_SET_INDEX, binding = idProjectionUBO) readonly buffer ProjectionUBOVector {
    ProjectionUBO projection_ubo[];
} projectionVector;

layout(set = LAYER_SET_INDEX, binding = idLineEvaluatedPropsUBO) uniform LineEvaluatedPropsUBOBlock {
    LineEvaluatedPropsUBO props;
};

layout(location = 0) out vec2 frag_normal;
layout(location = 1) out vec2 frag_width2;
layout(location = 2) out float frag_gamma_scale;
layout(location = 3) out vec2 frag_tex_a;
layout(location = 4) out vec2 frag_tex_b;

#property varying color 5

#property varying blur 6

#property varying opacity 7

#property varying floorwidth 8

#ifdef PROJECTION_GLOBE
layout(location = 9) out float frag_tile_x;
#endif

#if !defined(HAS_UNIFORM_u_dasharray)
layout(location = 10) flat out float frag_dash_width;
#endif

void main() {
    const LineSDFDrawableUBO drawable = drawableVector.drawable_ubo[constant.ubo_index];

    #property pass color

    #property pass blur

    #property pass opacity

#ifndef HAS_UNIFORM_u_floorwidth
    const float floorwidth = unpack_mix_float(in_floorwidth, drawable.floorwidth_t);
    frag_floorwidth = floorwidth;
#else
    const float floorwidth = props.floorwidth;
#endif

    const float offset = $props.offset * -1.0;

    const float width = $props.width;

    const float gapwidth = $props.gapwidth / 2.0;

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
    const float adjustedThickness = projectLineThickness(pos.y, projectionVector.projection_ubo[constant.ubo_index]);
    const vec4 projected_no_extrude = projectTile(pos + offset2 / drawable.ratio * adjustedThickness, projectionVector.projection_ubo[constant.ubo_index]);
    const vec2 extrudedPos = pos + (offset2 + dist) / drawable.ratio * adjustedThickness;
    gl_Position = projectTile(extrudedPos, projectionVector.projection_ubo[constant.ubo_index]);
    frag_tile_x = antimeridianClipX(extrudedPos, projectionVector.projection_ubo[constant.ubo_index]);
    vec4 projected_extrude = gl_Position - projected_no_extrude;
#else
    vec4 projected_extrude = drawable.matrix * vec4(dist / drawable.ratio, 0.0, 0.0);
    gl_Position = drawable.matrix * vec4(pos + offset2 / drawable.ratio, 0.0, 1.0) + projected_extrude;
#endif

    // calculate how much the perspective view squishes or stretches the extrude
    float extrude_length_without_perspective = length(dist);
    float extrude_length_with_perspective = length(projected_extrude.xy / gl_Position.w * view.units_to_pixels);
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
"
    ),
    fragment: concat!(
        line_fragment_prelude!(),
        r"

layout(location = 0) in vec2 frag_normal;
layout(location = 1) in vec2 frag_width2;
layout(location = 2) in float frag_gamma_scale;
layout(location = 3) in vec2 frag_tex_a;
layout(location = 4) in vec2 frag_tex_b;

#property varying color 5

#property varying blur 6

#property varying opacity 7

#property varying floorwidth 8

layout(location = 0) out vec4 out_color;

layout(push_constant) uniform Constants {
    int ubo_index;
} constant;

layout(std140, set = LAYER_SET_INDEX, binding = idLineTilePropsUBO) readonly buffer LineSDFTilePropsUBOVector {
    LineSDFTilePropsUBO tile_props_ubo[];
} tilePropsVector;

layout(set = LAYER_SET_INDEX, binding = idLineEvaluatedPropsUBO) uniform LineEvaluatedPropsUBOBlock {
    LineEvaluatedPropsUBO props;
};

layout(set = DRAWABLE_IMAGE_SET_INDEX, binding = 0) uniform texture2D image0_texture;
layout(set = DRAWABLE_IMAGE_SET_INDEX, binding = samplerBindingStartId) uniform sampler image0_sampler;

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

#ifdef OVERDRAW_INSPECTOR
    out_color = vec4(1.0);
    return;
#endif

    const LineSDFTilePropsUBO tileProps = tilePropsVector.tile_props_ubo[constant.ubo_index];

    const vec4 color = $props.color;

    const float blur = $props.blur;

    const float opacity = $props.opacity;

    const float floorwidth = $props.floorwidth;

    // Calculate the distance of the pixel from the line in pixels.
    const float dist = length(frag_normal) * frag_width2.x;

    // Calculate the antialiasing fade factor. This is either when fading in the
    // line in case of an offset line (`v_width2.y`) or when fading out (`v_width2.x`)
    const float blur2 = (blur + 1.0 / DEVICE_PIXEL_RATIO) * frag_gamma_scale;

    const float sdfdist_a = texture(sampler2D(image0_texture, image0_sampler), frag_tex_a).a;
    const float sdfdist_b = texture(sampler2D(image0_texture, image0_sampler), frag_tex_b).a;
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
"
    ),
};

pub(super) const LINE_GRADIENT_SDF_SHADER: Glsl = Glsl {
    prelude: LINE_SHADE_PRELUDE,
    vertex: concat!(
        line_vertex_prelude!(),
        r"

layout(location = 0) in ivec2 in_pos_normal;
layout(location = 1) in uvec4 in_data;

#property attribute blur

#property attribute opacity

#property attribute gapwidth

#property attribute offset

#property attribute width

#property attribute floorwidth

#property attribute dasharray

layout(push_constant) uniform Constants {
    int ubo_index;
} constant;

layout(std140, set = LAYER_SET_INDEX, binding = idLineDrawableUBO) readonly buffer LineSDFDrawableUBOVector {
    LineSDFDrawableUBO drawable_ubo[];
} drawableVector;

layout(std140, set = LAYER_SET_INDEX, binding = idProjectionUBO) readonly buffer ProjectionUBOVector {
    ProjectionUBO projection_ubo[];
} projectionVector;

layout(set = LAYER_SET_INDEX, binding = idLineEvaluatedPropsUBO) uniform LineEvaluatedPropsUBOBlock {
    LineEvaluatedPropsUBO props;
};

layout(location = 0) out vec2 frag_normal;
layout(location = 1) out vec2 frag_width2;
layout(location = 2) out float frag_gamma_scale;
layout(location = 3) out vec2 frag_tex_a;
layout(location = 4) out vec2 frag_tex_b;
layout(location = 5) out float frag_lineprogress;

#property varying blur 6

#property varying opacity 7

#property varying floorwidth 8

#ifdef PROJECTION_GLOBE
layout(location = 9) out float frag_tile_x;
#endif

#if !defined(HAS_UNIFORM_u_dasharray)
layout(location = 10) flat out float frag_dash_width;
#endif

void main() {
    const LineSDFDrawableUBO drawable = drawableVector.drawable_ubo[constant.ubo_index];

    #property pass blur

    #property pass opacity

#ifndef HAS_UNIFORM_u_floorwidth
    const float floorwidth = unpack_mix_float(in_floorwidth, drawable.floorwidth_t);
    frag_floorwidth = floorwidth;
#else
    const float floorwidth = props.floorwidth;
#endif

    const float offset = $props.offset * -1.0;

    const float width = $props.width;

    const float gapwidth = $props.gapwidth / 2.0;

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
    const float adjustedThickness = projectLineThickness(pos.y, projectionVector.projection_ubo[constant.ubo_index]);
    const vec4 projected_no_extrude = projectTile(pos + offset2 / drawable.ratio * adjustedThickness, projectionVector.projection_ubo[constant.ubo_index]);
    const vec2 extrudedPos = pos + (offset2 + dist) / drawable.ratio * adjustedThickness;
    gl_Position = projectTile(extrudedPos, projectionVector.projection_ubo[constant.ubo_index]);
    frag_tile_x = antimeridianClipX(extrudedPos, projectionVector.projection_ubo[constant.ubo_index]);
    vec4 projected_extrude = gl_Position - projected_no_extrude;
#else
    vec4 projected_extrude = drawable.matrix * vec4(dist / drawable.ratio, 0.0, 0.0);
    gl_Position = drawable.matrix * vec4(pos + offset2 / drawable.ratio, 0.0, 1.0) + projected_extrude;
#endif

    // calculate how much the perspective view squishes or stretches the extrude
    float extrude_length_without_perspective = length(dist);
    float extrude_length_with_perspective = length(projected_extrude.xy / gl_Position.w * view.units_to_pixels);
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
"
    ),
    fragment: concat!(
        line_fragment_prelude!(),
        r"

layout(location = 0) in vec2 frag_normal;
layout(location = 1) in vec2 frag_width2;
layout(location = 2) in float frag_gamma_scale;
layout(location = 3) in vec2 frag_tex_a;
layout(location = 4) in vec2 frag_tex_b;
layout(location = 5) in float frag_lineprogress;

#property varying blur 6

#property varying opacity 7

#property varying floorwidth 8

#ifdef PROJECTION_GLOBE
layout(location = 9) in float frag_tile_x;
#endif

#if !defined(HAS_UNIFORM_u_dasharray)
layout(location = 10) flat in float frag_dash_width;
#endif

layout(location = 0) out vec4 out_color;

layout(push_constant) uniform Constants {
    int ubo_index;
} constant;

layout(std140, set = LAYER_SET_INDEX, binding = idLineTilePropsUBO) readonly buffer LineSDFTilePropsUBOVector {
    LineSDFTilePropsUBO tile_props_ubo[];
} tilePropsVector;

layout(set = LAYER_SET_INDEX, binding = idLineEvaluatedPropsUBO) uniform LineEvaluatedPropsUBOBlock {
    LineEvaluatedPropsUBO props;
};

layout(set = DRAWABLE_IMAGE_SET_INDEX, binding = 0) uniform texture2D image0_texture;
layout(set = DRAWABLE_IMAGE_SET_INDEX, binding = samplerBindingStartId) uniform sampler image0_sampler;
layout(set = DRAWABLE_IMAGE_SET_INDEX, binding = 1) uniform texture2D image1_texture;
layout(set = DRAWABLE_IMAGE_SET_INDEX, binding = samplerBindingStartId + 1) uniform sampler image1_sampler;

void main() {
#ifdef PROJECTION_GLOBE
    if (clippedAtAntimeridian(frag_tile_x)) {
        discard;
    }
#endif

#ifdef OVERDRAW_INSPECTOR
    out_color = vec4(1.0);
    return;
#endif

    const LineSDFTilePropsUBO tileProps = tilePropsVector.tile_props_ubo[constant.ubo_index];

    const float blur = $props.blur;

    const float opacity = $props.opacity;

    const float floorwidth = $props.floorwidth;

    // Calculate the distance of the pixel from the line in pixels.
    const float dist = length(frag_normal) * frag_width2.x;

    // Calculate the antialiasing fade factor. This is either when fading in the
    // line in case of an offset line (`v_width2.y`) or when fading out (`v_width2.x`)
    const float blur2 = (blur + 1.0 / DEVICE_PIXEL_RATIO) * frag_gamma_scale;

    const float sdfdist_a = texture(sampler2D(image1_texture, image1_sampler), frag_tex_a).a;
    const float sdfdist_b = texture(sampler2D(image1_texture, image1_sampler), frag_tex_b).a;
    const float sdfdist = mix(sdfdist_a, sdfdist_b, tileProps.mix);
#ifdef HAS_UNIFORM_u_dasharray
    const float sdfgamma = tileProps.sdfgamma;
#else
    const float sdfgamma = tileProps.sdfgamma / frag_dash_width;
#endif
    const float alpha = clamp(min(dist - (frag_width2.y - blur2), frag_width2.x - dist) / blur2, 0.0, 1.0) *
                        smoothstep(0.5 - sdfgamma / floorwidth, 0.5 + sdfgamma / floorwidth, sdfdist);

    // The gradient ramp is stored in a texture indexed by the ratio along the entire line.
    const vec4 color = texture(sampler2D(image0_texture, image0_sampler), vec2(frag_lineprogress, 0.5));

    out_color = color * (alpha * opacity);
}
"
    ),
};
