//! The programs of the `symbol` layer type, its symbols and their collision
//! boxes and circles, in GLSL, which `mln_glsl` parses for `mln_vulkan` and
//! `mln_d3d12`.
//!
//! - Symbols read their values from vertex attributes, `tex_origin` and
//!   `height_offset` among them, not from per-instance storage buffers. The
//!   collision boxes read `height_offset` too.
//! - On the globe, a symbol vertex stage puts a screen-aligned label whose
//!   anchor is behind the horizon outside the clip volume, unless the label
//!   is raised.

use mln_gfx::gfx_types::AttributeDataType::{Float, Float2, Float3, Float4, Short2, Short4, UShort2, UShort4};
use mln_gfx::shaders::program::Program;
use mln_gfx::shaders::source::glsl::{Attribute, Glsl, Source, Texture};

use super::programs;
use super::programs::{
    ID_COLLISION_ANCHOR_POS_VERTEX_ATTRIBUTE, ID_COLLISION_EXTRUDE_VERTEX_ATTRIBUTE,
    ID_COLLISION_HEIGHT_OFFSET_VERTEX_ATTRIBUTE, ID_COLLISION_PLACED_VERTEX_ATTRIBUTE,
    ID_COLLISION_POS_VERTEX_ATTRIBUTE, ID_COLLISION_SHIFT_VERTEX_ATTRIBUTE, ID_SYMBOL_COLOR_ATTRIBUTE,
    ID_SYMBOL_DATA_ATTRIBUTE, ID_SYMBOL_FADE_OPACITY_ATTRIBUTE, ID_SYMBOL_HALO_BLUR_ATTRIBUTE,
    ID_SYMBOL_HALO_COLOR_ATTRIBUTE, ID_SYMBOL_HALO_WIDTH_ATTRIBUTE, ID_SYMBOL_HEIGHT_OFFSET_ATTRIBUTE,
    ID_SYMBOL_IMAGE_ICON_TEXTURE, ID_SYMBOL_IMAGE_TEXTURE, ID_SYMBOL_OPACITY_ATTRIBUTE,
    ID_SYMBOL_PIXEL_OFFSET_ATTRIBUTE, ID_SYMBOL_POS_OFFSET_ATTRIBUTE, ID_SYMBOL_PROJECTED_POS_ATTRIBUTE,
    ID_SYMBOL_TEX_ORIGIN_ATTRIBUTE,
};

/// `unpack_opacity`, a symbol's fade opacity and whether it fades in, from
/// the packed value of its vertex.
macro_rules! unpack_opacity {
    () => {
        r"
vec2 unpack_opacity(const float packedOpacity) {
    int intOpacity = int(packedOpacity) / 2;
    return vec2(float(intOpacity) / 127.0, mod(packedOpacity, 2.0));
}
"
    };
}

const COLLISION_BOX_ATTRIBUTES: &[Attribute] = &[
    Attribute::new(0, Short2, ID_COLLISION_POS_VERTEX_ATTRIBUTE),
    Attribute::new(1, Short2, ID_COLLISION_ANCHOR_POS_VERTEX_ATTRIBUTE),
    Attribute::new(2, Short2, ID_COLLISION_EXTRUDE_VERTEX_ATTRIBUTE),
    // Dynamic
    Attribute::new(3, UShort2, ID_COLLISION_PLACED_VERTEX_ATTRIBUTE),
    Attribute::new(4, Float2, ID_COLLISION_SHIFT_VERTEX_ATTRIBUTE),
    // Layout
    Attribute::new(5, Float, ID_COLLISION_HEIGHT_OFFSET_VERTEX_ATTRIBUTE),
];

const COLLISION_CIRCLE_ATTRIBUTES: &[Attribute] = &[
    Attribute::new(0, Short2, ID_COLLISION_POS_VERTEX_ATTRIBUTE),
    Attribute::new(1, Short2, ID_COLLISION_ANCHOR_POS_VERTEX_ATTRIBUTE),
    Attribute::new(2, Short2, ID_COLLISION_EXTRUDE_VERTEX_ATTRIBUTE),
    // Dynamic
    Attribute::new(3, UShort2, ID_COLLISION_PLACED_VERTEX_ATTRIBUTE),
];

static COLLISION_BOX: Source = Source {
    glsl: &COLLISION_BOX_SHADER,
    attributes: COLLISION_BOX_ATTRIBUTES,
    instance_attributes: &[],
    textures: &[],
};

static COLLISION_CIRCLE: Source = Source {
    glsl: &COLLISION_CIRCLE_SHADER,
    attributes: COLLISION_CIRCLE_ATTRIBUTES,
    instance_attributes: &[],
    textures: &[],
};

// Symbols: the layout vertex (`SymbolLayoutVertex`, with `tex_origin`), the
// dynamic vertex (`projected_pos`), the opacity vertex (`fade_opacity`) and
// the interleaved data-driven vertex, in four buffers.
const SYMBOL_ICON_ATTRIBUTES: &[Attribute] = &[
    Attribute::new(0, Short4, ID_SYMBOL_POS_OFFSET_ATTRIBUTE),
    Attribute::new(1, UShort4, ID_SYMBOL_DATA_ATTRIBUTE),
    Attribute::new(2, Short4, ID_SYMBOL_PIXEL_OFFSET_ATTRIBUTE),
    // Dynamic
    Attribute::new(3, Float3, ID_SYMBOL_PROJECTED_POS_ATTRIBUTE),
    // Opacity
    Attribute::new(4, Float, ID_SYMBOL_FADE_OPACITY_ATTRIBUTE),
    // Data driven
    Attribute::new(5, Float2, ID_SYMBOL_OPACITY_ATTRIBUTE),
    // Layout
    Attribute::new(6, UShort2, ID_SYMBOL_TEX_ORIGIN_ATTRIBUTE),
    Attribute::new(7, Float, ID_SYMBOL_HEIGHT_OFFSET_ATTRIBUTE),
];

const SYMBOL_SDF_ATTRIBUTES: &[Attribute] = &[
    Attribute::new(0, Short4, ID_SYMBOL_POS_OFFSET_ATTRIBUTE),
    Attribute::new(1, UShort4, ID_SYMBOL_DATA_ATTRIBUTE),
    Attribute::new(2, Short4, ID_SYMBOL_PIXEL_OFFSET_ATTRIBUTE),
    // Dynamic
    Attribute::new(3, Float3, ID_SYMBOL_PROJECTED_POS_ATTRIBUTE),
    // Opacity
    Attribute::new(4, Float, ID_SYMBOL_FADE_OPACITY_ATTRIBUTE),
    // Data driven
    Attribute::new(5, Float4, ID_SYMBOL_COLOR_ATTRIBUTE),
    Attribute::new(6, Float4, ID_SYMBOL_HALO_COLOR_ATTRIBUTE),
    Attribute::new(7, Float2, ID_SYMBOL_OPACITY_ATTRIBUTE),
    Attribute::new(8, Float2, ID_SYMBOL_HALO_WIDTH_ATTRIBUTE),
    Attribute::new(9, Float2, ID_SYMBOL_HALO_BLUR_ATTRIBUTE),
    // Layout
    Attribute::new(10, UShort2, ID_SYMBOL_TEX_ORIGIN_ATTRIBUTE),
    Attribute::new(11, Float, ID_SYMBOL_HEIGHT_OFFSET_ATTRIBUTE),
];

const SYMBOL_TEXT_AND_ICON_ATTRIBUTES: &[Attribute] = &[
    Attribute::new(0, Short4, ID_SYMBOL_POS_OFFSET_ATTRIBUTE),
    Attribute::new(1, UShort4, ID_SYMBOL_DATA_ATTRIBUTE),
    // Dynamic
    Attribute::new(2, Float3, ID_SYMBOL_PROJECTED_POS_ATTRIBUTE),
    // Opacity
    Attribute::new(3, Float, ID_SYMBOL_FADE_OPACITY_ATTRIBUTE),
    // Data driven
    Attribute::new(4, Float4, ID_SYMBOL_COLOR_ATTRIBUTE),
    Attribute::new(5, Float4, ID_SYMBOL_HALO_COLOR_ATTRIBUTE),
    Attribute::new(6, Float2, ID_SYMBOL_OPACITY_ATTRIBUTE),
    Attribute::new(7, Float2, ID_SYMBOL_HALO_WIDTH_ATTRIBUTE),
    Attribute::new(8, Float2, ID_SYMBOL_HALO_BLUR_ATTRIBUTE),
    // Layout
    Attribute::new(9, UShort2, ID_SYMBOL_TEX_ORIGIN_ATTRIBUTE),
    Attribute::new(10, Float, ID_SYMBOL_HEIGHT_OFFSET_ATTRIBUTE),
];

const SYMBOL_TEXTURES: &[Texture] = &[Texture::new(0, ID_SYMBOL_IMAGE_TEXTURE)];

static SYMBOL_ICON: Source = Source {
    glsl: &SYMBOL_ICON_SHADER,
    attributes: SYMBOL_ICON_ATTRIBUTES,
    instance_attributes: &[],
    textures: SYMBOL_TEXTURES,
};

static SYMBOL_SDF: Source = Source {
    glsl: &SYMBOL_SDF_SHADER,
    attributes: SYMBOL_SDF_ATTRIBUTES,
    instance_attributes: &[],
    textures: SYMBOL_TEXTURES,
};

static SYMBOL_TEXT_AND_ICON: Source = Source {
    glsl: &SYMBOL_TEXT_AND_ICON_SHADER,
    attributes: SYMBOL_TEXT_AND_ICON_ATTRIBUTES,
    instance_attributes: &[],
    textures: &[Texture::new(0, ID_SYMBOL_IMAGE_TEXTURE), Texture::new(1, ID_SYMBOL_IMAGE_ICON_TEXTURE)],
};

/// The source of each of the type's programs.
pub(super) static SOURCES: &[(&Program, &Source)] = &[
    (&programs::SYMBOL_ICON, &SYMBOL_ICON),
    (&programs::SYMBOL_SDF, &SYMBOL_SDF),
    (&programs::SYMBOL_TEXT_AND_ICON, &SYMBOL_TEXT_AND_ICON),
    (&programs::COLLISION_BOX, &COLLISION_BOX),
    (&programs::COLLISION_CIRCLE, &COLLISION_CIRCLE),
];

const COLLISION_SHADER_PRELUDE: &str = r"

#define idCollisionDrawableUBO      drawableUBOStartId
#define idCollisionTilePropsUBO     drawableUBOStartId + 3
#define idCollisionProjectionUBO    drawableUBOStartId + 2

";

const COLLISION_BOX_SHADER: Glsl = Glsl {
    prelude: COLLISION_SHADER_PRELUDE,
    vertex: r"

layout(location = 0) in ivec2 in_position;
layout(location = 1) in ivec2 in_anchor_position;
layout(location = 2) in ivec2 in_extrude;
layout(location = 3) in uvec2 in_placed;
layout(location = 4) in vec2 in_shift;
layout(location = 5) in float in_height_offset;

layout(set = DRAWABLE_UBO_SET_INDEX, binding = idCollisionDrawableUBO) uniform CollisionDrawableUBO {
    mat4 matrix;
} drawable;

layout(set = DRAWABLE_UBO_SET_INDEX, binding = idCollisionTilePropsUBO) uniform CollisionTilePropsUBO {
    vec2 extrude_scale;
    float overscale_factor;
    float pad1;
} tileProps;

layout(std140, set = DRAWABLE_UBO_SET_INDEX, binding = idCollisionProjectionUBO) uniform CollisionProjectionUBO {
    ProjectionUBO projection;
} collisionProjection;

layout(location = 0) out float frag_placed;
layout(location = 1) out float frag_notUsed;

void main() {

#ifdef PROJECTION_GLOBE
    vec4 projectedPoint = projectTileWithElevation(vec2(in_anchor_position), in_height_offset, collisionProjection.projection);
#else
    vec4 projectedPoint = drawable.matrix * vec4(in_anchor_position, in_height_offset, 1.0);
#endif
    float camera_to_anchor_distance = projectedPoint.w;
    float collision_perspective_ratio = clamp(
        0.5 + 0.5 * (paintParams.camera_to_center_distance / camera_to_anchor_distance),
        0.0, // Prevents oversized near-field boxes in pitched/overzoomed tiles
        4.0);

#ifdef PROJECTION_GLOBE
    gl_Position = projectTileWithElevation(vec2(in_position), in_height_offset, collisionProjection.projection);
    // Boxes just past the horizon stay visible, as in GL JS; ones far behind it are clipped.
    if (gl_Position.z / gl_Position.w < 1.1) {
        gl_Position.z = 0.0;
    }
#else
    gl_Position = drawable.matrix * vec4(in_position, in_height_offset, 1.0);
#endif
    gl_Position.xy += (in_extrude + in_shift) * tileProps.extrude_scale * gl_Position.w * collision_perspective_ratio;

    frag_placed = in_placed.x;
    frag_notUsed = in_placed.y;
}
",
    fragment: r"

layout(location = 0) in float frag_placed;
layout(location = 1) in float frag_notUsed;

layout(location = 0) out vec4 out_color;

void main() {

    float alpha = 0.5;

    // Red = collision, hide label
    vec4 color = vec4(1.0, 0.0, 0.0, 1.0) * alpha;

    // Blue = no collision, label is showing
    if (frag_placed > 0.5) {
        color = vec4(0.0, 0.0, 1.0, 0.5) * alpha;
    }

    if (frag_notUsed > 0.5) {
        // This box not used, fade it out
        color *= 0.1;
    }

    out_color = color;
}
",
};

const COLLISION_CIRCLE_SHADER: Glsl = Glsl {
    prelude: COLLISION_SHADER_PRELUDE,
    vertex: r"

layout(location = 0) in ivec2 in_position;
layout(location = 1) in ivec2 in_anchor_position;
layout(location = 2) in ivec2 in_extrude;
layout(location = 3) in uvec2 in_placed;

layout(set = DRAWABLE_UBO_SET_INDEX, binding = idCollisionDrawableUBO) uniform CollisionDrawableUBO {
    mat4 matrix;
} drawable;

layout(set = DRAWABLE_UBO_SET_INDEX, binding = idCollisionTilePropsUBO) uniform CollisionTilePropsUBO {
    vec2 extrude_scale;
    float overscale_factor;
    float pad1;
} tileProps;

layout(std140, set = DRAWABLE_UBO_SET_INDEX, binding = idCollisionProjectionUBO) uniform CollisionProjectionUBO {
    ProjectionUBO projection;
} collisionProjection;

layout(location = 0) out float frag_placed;
layout(location = 1) out float frag_notUsed;
layout(location = 2) out float frag_radius;
layout(location = 3) out vec2 frag_extrude;
layout(location = 4) out vec2 frag_extrude_scale;

void main() {

#ifdef PROJECTION_GLOBE
    vec4 projectedPoint = projectTile(vec2(in_anchor_position), collisionProjection.projection);
#else
    vec4 projectedPoint = drawable.matrix * vec4(in_anchor_position, 0, 1);
#endif
    float camera_to_anchor_distance = projectedPoint.w;
    float collision_perspective_ratio = clamp(
        0.5 + 0.5 * (paintParams.camera_to_center_distance / camera_to_anchor_distance),
        0.0, // Prevents oversized near-field circles in pitched/overzoomed tiles
        4.0);

    float padding_factor = 1.2; // Pad the vertices slightly to make room for anti-alias blur
#ifdef PROJECTION_GLOBE
    gl_Position = projectTile(vec2(in_position), collisionProjection.projection);
    // Circles just past the horizon stay visible, as in GL JS; ones far behind it are clipped.
    if (gl_Position.z / gl_Position.w < 1.1) {
        gl_Position.z = 0.0;
    }
#else
    gl_Position = drawable.matrix * vec4(in_position, 0.0, 1.0);
#endif
    gl_Position.xy += in_extrude * tileProps.extrude_scale * padding_factor * gl_Position.w * collision_perspective_ratio;

    frag_placed = in_placed.x;
    frag_notUsed = in_placed.y;
    frag_radius = abs(float(in_extrude.y)); // We don't pitch the circles, so both units of the extrusion vector are equal in magnitude to the radius
    frag_extrude = in_extrude * padding_factor;
    frag_extrude_scale = tileProps.extrude_scale * paintParams.camera_to_center_distance * collision_perspective_ratio;
}
",
    fragment: r"

layout(location = 0) in float frag_placed;
layout(location = 1) in float frag_notUsed;
layout(location = 2) in float frag_radius;
layout(location = 3) in vec2 frag_extrude;
layout(location = 4) in vec2 frag_extrude_scale;

layout(location = 0) out vec4 out_color;

layout(set = DRAWABLE_UBO_SET_INDEX, binding = idCollisionTilePropsUBO) uniform CollisionTilePropsUBO {
    vec2 extrude_scale;
    float overscale_factor;
    float pad1;
} tileProps;

void main() {

    float alpha = 0.5;

    // Red = collision, hide label
    vec4 color = vec4(1.0, 0.0, 0.0, 1.0) * alpha;

    // Blue = no collision, label is showing
    if (frag_placed > 0.5) {
        color = vec4(0.0, 0.0, 1.0, 0.5) * alpha;
    }

    if (frag_notUsed > 0.5) {
        // This box not used, fade it out
        color *= 0.2;
    }

    float extrude_scale_length = length(frag_extrude_scale);
    float extrude_length = length(frag_extrude) * extrude_scale_length;
    float stroke_width = 15.0 * extrude_scale_length / tileProps.overscale_factor;
    float radius = frag_radius * extrude_scale_length;

    float distance_to_edge = abs(extrude_length - radius);
    float opacity_t = smoothstep(-stroke_width, 0.0, -distance_to_edge);

    out_color = color * opacity_t;
}
",
};

const SYMBOL_SHADER_PRELUDE: &str = concat!(
    r"

#define idSymbolDrawableUBO         idDrawableReservedVertexOnlyUBO
#define idSymbolTilePropsUBO        idDrawableReservedFragmentOnlyUBO
#define idSymbolEvaluatedPropsUBO   layerUBOStartId

#define SDF_PX 8.0

",
    unpack_opacity!()
);

const SYMBOL_ICON_SHADER: Glsl = Glsl {
    prelude: SYMBOL_SHADER_PRELUDE,
    vertex: r"

layout(location = 0) in ivec4 in_pos_offset;
layout(location = 1) in uvec4 in_data;
layout(location = 2) in ivec4 in_pixeloffset;
layout(location = 3) in vec3 in_projected_pos;
layout(location = 4) in float in_fade_opacity;

#if !defined(HAS_UNIFORM_u_opacity)
layout(location = 5) in vec2 in_opacity;
#endif

// The top-left of the quad's atlas rectangle.
layout(location = 6) in uvec2 in_tex_origin;
// The symbol's symbol-height-offset, in meters.
layout(location = 7) in float in_height_offset;

layout(push_constant) uniform Constants {
    int ubo_index;
} constant;

struct SymbolDrawableUBO {
    mat4 matrix;
    mat4 label_plane_matrix;
    mat4 coord_matrix;

    vec2 texsize;
    vec2 texsize_icon;

    int is_text_prop;
	int rotate_symbol;
    int pitch_with_map;
    int is_size_zoom_constant;
    int is_size_feature_constant;
    int is_offset;

    float size_t;
    float size;

    // Interpolations
    float fill_color_t;
    float halo_color_t;
    float opacity_t;
    float halo_width_t;
    float halo_blur_t;

    int is_along_line;
    int is_variable_anchor;
    float pitched_scale;
    vec2 translation;
    float gamma_scale;
    int is_halo;
};

layout(std140, set = LAYER_SET_INDEX, binding = idSymbolDrawableUBO) readonly buffer SymbolDrawableUBOVector {
    SymbolDrawableUBO drawable_ubo[];
} drawableVector;

layout(std140, set = LAYER_SET_INDEX, binding = idProjectionUBO) readonly buffer ProjectionUBOVector {
    ProjectionUBO projection_ubo[];
} projectionVector;

layout(set = LAYER_SET_INDEX, binding = idSymbolEvaluatedPropsUBO) uniform SymbolEvaluatedPropsUBO {
    vec4 text_fill_color;
    vec4 text_halo_color;
    float text_opacity;
    float text_halo_width;
    float text_halo_blur;
    float pad1;
    vec4 icon_fill_color;
    vec4 icon_halo_color;
    float icon_opacity;
    float icon_halo_width;
    float icon_halo_blur;
    float fade_change;
} props;

layout(location = 0) out vec2 frag_tex;
layout(location = 1) out float frag_opacity;

// The texture coordinate is relative to the quad's atlas origin, which
// atlas_pos adds back.
layout(location = 2) flat out vec2 frag_tex_origin;

void main() {
    const SymbolDrawableUBO drawable = drawableVector.drawable_ubo[constant.ubo_index];
    const ProjectionUBO projection = projectionVector.projection_ubo[constant.ubo_index];

    const vec2 raw_fade_opacity = unpack_opacity(in_fade_opacity);
    const float fade_change = raw_fade_opacity[1] > 0.5 ? props.fade_change : -props.fade_change;
    const float fade_opacity = max(0.0, min(1.0, raw_fade_opacity[0] + fade_change));
#if defined(HAS_UNIFORM_u_opacity)
    const float visibility = fade_opacity;
#else
    const float visibility = unpack_mix_float(in_opacity, drawable.opacity_t) * fade_opacity;
#endif
    // A quad that draws nothing goes to one point off screen, so it's never
    // rasterized. All four vertices of a quad share its fade and opacity.
    if (visibility == 0.0) {
        gl_Position = vec4(-2.0, -2.0, -2.0, 1.0);
        return;
    }

    const vec2 a_pos = in_pos_offset.xy;
    const vec2 a_offset = in_pos_offset.zw;

    const vec2 a_tex = in_data.xy;
    const vec2 a_size = in_data.zw;

    const float a_size_min = floor(a_size[0] * 0.5);
    const vec2 a_pxoffset = in_pixeloffset.xy;
    const vec2 a_minFontScale = in_pixeloffset.zw / 256.0;

    const float segment_angle = -in_projected_pos[2];

    float size;
    if (drawable.is_size_zoom_constant == 0 && drawable.is_size_feature_constant == 0) {
        size = mix(a_size_min, a_size[1], drawable.size_t) / 128.0;
    } else if (drawable.is_size_zoom_constant != 0 && drawable.is_size_feature_constant == 0) {
        size = a_size_min / 128.0;
    } else {
        size = drawable.size;
    }

    const vec2 translated_a_pos = a_pos + drawable.translation;
    const vec4 projectedPoint = projectTileWithElevation(translated_a_pos, in_height_offset, projection);
#ifdef PROJECTION_GLOBE
    // A screen-aligned label whose anchor is behind the horizon goes at
    // once, rather than fading out once placement next finds it occluded:
    // its clip Z is 0, so the horizon never clips it. A pitched label's
    // vertices go through the globe projection, which clips them.
    // A raised label is left to placement, which hides it once the planet
    // blocks the line of sight to it.
    if (drawable.pitch_with_map == 0 && in_height_offset <= 0.0 && projectedPoint.z > projectedPoint.w) {
        gl_Position = vec4(-2.0, -2.0, -2.0, 1.0);
        return;
    }
#endif
    const float camera_to_anchor_distance = projectedPoint.w;
    // See comments in symbol_sdf.vertex
    const float distance_ratio = drawable.pitch_with_map != 0 ?
        camera_to_anchor_distance / paintParams.camera_to_center_distance :
        paintParams.camera_to_center_distance / camera_to_anchor_distance;
    const float perspective_ratio = clamp(
            0.5 + 0.5 * distance_ratio,
            0.0, // Prevents oversized near-field symbols in pitched/overzoomed tiles
            4.0);

    if (drawable.is_offset == 0) {
        size *= perspective_ratio;
    }

    const float fontScale = drawable.is_text_prop != 0 ? size / 24.0 : size;

    float symbol_rotation = 0.0;
    if (drawable.rotate_symbol != 0) {
        // See comments in symbol_sdf.vertex
        const vec4 offsetProjectedPoint = projectTileWithElevation(translated_a_pos + vec2(1, 0), in_height_offset, projection);

        const vec2 a = projectedPoint.xy / projectedPoint.w;
        const vec2 b = offsetProjectedPoint.xy / offsetProjectedPoint.w;
        symbol_rotation = atan((b.y - a.y) / paintParams.aspect_ratio, b.x - a.x);
    }

    const float angle_sin = sin(segment_angle + symbol_rotation);
    const float angle_cos = cos(segment_angle + symbol_rotation);
    const mat2 rotation_matrix = mat2(angle_cos, -1.0 * angle_sin, angle_sin, angle_cos);

    const vec2 projected_pos_xy = in_projected_pos.xy;
    vec4 projected_pos;
    if (drawable.is_along_line != 0 || drawable.is_variable_anchor != 0) {
        projected_pos = vec4(projected_pos_xy, in_height_offset, 1.0);
    } else if (drawable.pitch_with_map != 0) {
        projected_pos = drawable.label_plane_matrix * vec4(projected_pos_xy + drawable.translation, in_height_offset, 1.0);
    } else {
        projected_pos = drawable.label_plane_matrix * projectTileWithElevation(projected_pos_xy + drawable.translation, in_height_offset, projection);
    }
    const float z = float(drawable.pitch_with_map) * projected_pos.z / projected_pos.w;

    float projectionScaling = 1.0;
#ifdef PROJECTION_GLOBE
    if (drawable.pitch_with_map != 0) {
        const float anchor_pos_tile_y = (drawable.coord_matrix * vec4(projected_pos.xy / projected_pos.w, z, 1.0)).y;
        projectionScaling = mix(projectionScaling, 1.0 / circumferenceRatioAtTileY(anchor_pos_tile_y, projection) * drawable.pitched_scale, projection.projection_transition);
    }
#endif
    const vec2 posOffset = a_offset * max(a_minFontScale, fontScale) / 32.0 + a_pxoffset / 16.0;
    gl_Position = drawable.coord_matrix * vec4(projected_pos.xy / projected_pos.w + rotation_matrix * posOffset * projectionScaling, z, 1.0);
    if (drawable.pitch_with_map != 0) {
        gl_Position = projectTileWithElevation(gl_Position.xy, gl_Position.z, projection);
    }

    const vec2 tex_origin = vec2(in_tex_origin);
    frag_tex = (a_tex - tex_origin) / drawable.texsize;
    frag_tex_origin = tex_origin / drawable.texsize;

    frag_opacity = visibility;
}
",
    fragment: r"

layout(location = 0) in vec2 frag_tex;
layout(location = 1) in float frag_opacity;

// The texture coordinate is relative to the quad's atlas origin, which
// atlas_pos adds back.
layout(location = 2) flat in vec2 frag_tex_origin;

layout(location = 0) out vec4 out_color;

layout(push_constant) uniform Constants {
    int ubo_index;
} constant;

struct SymbolTilePropsUBO {
    int is_text;
    int is_halo;
    float pad1;
    float pad2;
};

layout(std140, set = LAYER_SET_INDEX, binding = idSymbolTilePropsUBO) readonly buffer SymbolTilePropsUBOVector {
    SymbolTilePropsUBO tile_props_ubo[];
} tilePropsVector;

layout(set = LAYER_SET_INDEX, binding = idSymbolEvaluatedPropsUBO) uniform SymbolEvaluatedPropsUBO {
    vec4 text_fill_color;
    vec4 text_halo_color;
    float text_opacity;
    float text_halo_width;
    float text_halo_blur;
    float pad1;
    vec4 icon_fill_color;
    vec4 icon_halo_color;
    float icon_opacity;
    float icon_halo_width;
    float icon_halo_blur;
    float fade_change;
} props;

layout(set = DRAWABLE_IMAGE_SET_INDEX, binding = 0) uniform texture2D image0_texture;
layout(set = DRAWABLE_IMAGE_SET_INDEX, binding = samplerBindingStartId) uniform sampler image0_sampler;

void main() {
#if defined(OVERDRAW_INSPECTOR)
    out_color = vec4(1.0);
    return;
#else

    const SymbolTilePropsUBO tileProps = tilePropsVector.tile_props_ubo[constant.ubo_index];

#if defined(HAS_UNIFORM_u_opacity)
    const float opacity = (tileProps.is_text != 0 ? props.text_opacity : props.icon_opacity) * frag_opacity;
#else
    const float opacity = frag_opacity; // fade_opacity is baked in for this case
#endif

    out_color = texture(sampler2D(image0_texture, image0_sampler), atlas_pos(frag_tex_origin, frag_tex)) * opacity;
#endif
}
",
};

const SYMBOL_SDF_SHADER: Glsl = Glsl {
    prelude: SYMBOL_SHADER_PRELUDE,
    vertex: r"

layout(location = 0) in ivec4 in_pos_offset;
layout(location = 1) in uvec4 in_data;
layout(location = 2) in ivec4 in_pixeloffset;
layout(location = 3) in vec3 in_projected_pos;
layout(location = 4) in float in_fade_opacity;

#if !defined(HAS_UNIFORM_u_fill_color)
layout(location = 5) in vec4 in_fill_color;
#endif

#if !defined(HAS_UNIFORM_u_halo_color)
layout(location = 6) in vec4 in_halo_color;
#endif

#if !defined(HAS_UNIFORM_u_opacity)
layout(location = 7) in vec2 in_opacity;
#endif

#if !defined(HAS_UNIFORM_u_halo_width)
layout(location = 8) in vec2 in_halo_width;
#endif

#if !defined(HAS_UNIFORM_u_halo_blur)
layout(location = 9) in vec2 in_halo_blur;
#endif

// The top-left of the quad's atlas rectangle.
layout(location = 10) in uvec2 in_tex_origin;
// The symbol's symbol-height-offset, in meters.
layout(location = 11) in float in_height_offset;

layout(push_constant) uniform Constants {
    int ubo_index;
} constant;

struct SymbolDrawableUBO {
    mat4 matrix;
    mat4 label_plane_matrix;
    mat4 coord_matrix;

    vec2 texsize;
    vec2 texsize_icon;

    int is_text_prop;
	int rotate_symbol;
    int pitch_with_map;
    int is_size_zoom_constant;
    int is_size_feature_constant;
    int is_offset;

    float size_t;
    float size;

    // Interpolations
    float fill_color_t;
    float halo_color_t;
    float opacity_t;
    float halo_width_t;
    float halo_blur_t;

    int is_along_line;
    int is_variable_anchor;
    float pitched_scale;
    vec2 translation;
    float gamma_scale;
    int is_halo;
};

layout(std140, set = LAYER_SET_INDEX, binding = idSymbolDrawableUBO) readonly buffer SymbolDrawableUBOVector {
    SymbolDrawableUBO drawable_ubo[];
} drawableVector;

layout(std140, set = LAYER_SET_INDEX, binding = idProjectionUBO) readonly buffer ProjectionUBOVector {
    ProjectionUBO projection_ubo[];
} projectionVector;

layout(set = LAYER_SET_INDEX, binding = idSymbolEvaluatedPropsUBO) uniform SymbolEvaluatedPropsUBO {
    vec4 text_fill_color;
    vec4 text_halo_color;
    float text_opacity;
    float text_halo_width;
    float text_halo_blur;
    float pad1;
    vec4 icon_fill_color;
    vec4 icon_halo_color;
    float icon_opacity;
    float icon_halo_width;
    float icon_halo_blur;
    float fade_change;
} props;

layout(location = 0) out vec2 frag_tex;
// The fade opacity, times the opacity when it's per vertex.
layout(location = 1) out float frag_fade_opacity;
// A glyph's edge's gamma, scaled by clip w, and the inner edge of its halo:
// the same over a quad but for the clip w, which interpolates linearly.
layout(location = 2) out float frag_gamma_scaled;
layout(location = 3) out float frag_halo_edge;

#if !defined(HAS_UNIFORM_u_fill_color)
layout(location = 4) out vec4 frag_fill_color;
#endif

#if !defined(HAS_UNIFORM_u_halo_color)
layout(location = 5) out vec4 frag_halo_color;
#endif

// The texture coordinate is relative to the quad's atlas origin, which
// atlas_pos adds back.
layout(location = 9) flat out vec2 frag_tex_origin;

void main() {
    const SymbolDrawableUBO drawable = drawableVector.drawable_ubo[constant.ubo_index];
    const ProjectionUBO projection = projectionVector.projection_ubo[constant.ubo_index];

    const vec2 raw_fade_opacity = unpack_opacity(in_fade_opacity);
    const float fade_change = raw_fade_opacity[1] > 0.5 ? props.fade_change : -props.fade_change;
    const float fade_opacity = max(0.0, min(1.0, raw_fade_opacity[0] + fade_change));
#if defined(HAS_UNIFORM_u_opacity)
    const float visibility = fade_opacity;
#else
    const float opacity = unpack_mix_float(in_opacity, drawable.opacity_t);
    const float visibility = min(fade_opacity, opacity);
#endif
    // A quad that draws nothing goes to one point off screen, so it's never
    // rasterized. All four vertices of a quad share its fade and opacity.
    if (visibility == 0.0) {
        gl_Position = vec4(-2.0, -2.0, -2.0, 1.0);
        return;
    }

    const vec2 a_pos = in_pos_offset.xy;
    const vec2 a_offset = in_pos_offset.zw;

    const vec2 a_tex = in_data.xy;
    const vec2 a_size = in_data.zw;

    const float a_size_min = floor(a_size[0] * 0.5);
    const vec2 a_pxoffset = in_pixeloffset.xy;
    const float segment_angle = -in_projected_pos[2];

    float size;
    if (drawable.is_size_zoom_constant == 0 && drawable.is_size_feature_constant == 0) {
        size = mix(a_size_min, a_size[1], drawable.size_t) / 128.0;
    } else if (drawable.is_size_zoom_constant != 0 && drawable.is_size_feature_constant == 0) {
        size = a_size_min / 128.0;
    } else {
        size = drawable.size;
    }

    const vec2 translated_a_pos = a_pos + drawable.translation;
    const vec4 projectedPoint = projectTileWithElevation(translated_a_pos, in_height_offset, projection);
#ifdef PROJECTION_GLOBE
    // A screen-aligned label whose anchor is behind the horizon goes at
    // once, rather than fading out once placement next finds it occluded:
    // its clip Z is 0, so the horizon never clips it. A pitched label's
    // vertices go through the globe projection, which clips them.
    // A raised label is left to placement, which hides it once the planet
    // blocks the line of sight to it.
    if (drawable.pitch_with_map == 0 && in_height_offset <= 0.0 && projectedPoint.z > projectedPoint.w) {
        gl_Position = vec4(-2.0, -2.0, -2.0, 1.0);
        return;
    }
#endif
    const float camera_to_anchor_distance = projectedPoint.w;
    // If the label is pitched with the map, layout is done in pitched space,
    // which makes labels in the distance smaller relative to viewport space.
    // We counteract part of that effect by multiplying by the perspective ratio.
    // If the label isn't pitched with the map, we do layout in viewport space,
    // which makes labels in the distance larger relative to the features around
    // them. We counteract part of that effect by dividing by the perspective ratio.
    const float distance_ratio = drawable.pitch_with_map != 0 ?
        camera_to_anchor_distance / paintParams.camera_to_center_distance :
        paintParams.camera_to_center_distance / camera_to_anchor_distance;
    const float perspective_ratio = clamp(
            0.5 + 0.5 * distance_ratio,
            0.0, // Prevents oversized near-field symbols in pitched/overzoomed tiles
            4.0);

    if (drawable.is_offset == 0) {
        size *= perspective_ratio;
    }

    const float fontScale = drawable.is_text_prop != 0 ? size / 24.0 : size;

    float symbol_rotation = 0.0;
    if (drawable.rotate_symbol != 0) {
        // Point labels with 'rotation-alignment: map' are horizontal with respect to tile units
        // To figure out that angle in projected space, we draw a short horizontal line in tile
        // space, project it, and measure its angle in projected space.
        const vec4 offsetProjectedPoint = projectTileWithElevation(translated_a_pos + vec2(1, 0), in_height_offset, projection);

        const vec2 a = projectedPoint.xy / projectedPoint.w;
        const vec2 b = offsetProjectedPoint.xy / offsetProjectedPoint.w;
        symbol_rotation = atan((b.y - a.y) / paintParams.aspect_ratio, b.x - a.x);
    }

    const float angle_sin = sin(segment_angle + symbol_rotation);
    const float angle_cos = cos(segment_angle + symbol_rotation);
    const mat2 rotation_matrix = mat2(angle_cos, -1.0 * angle_sin, angle_sin, angle_cos);

    const vec2 projected_pos_xy = in_projected_pos.xy;
    vec4 projected_pos;
    if (drawable.is_along_line != 0 || drawable.is_variable_anchor != 0) {
        projected_pos = vec4(projected_pos_xy, in_height_offset, 1.0);
    } else if (drawable.pitch_with_map != 0) {
        projected_pos = drawable.label_plane_matrix * vec4(projected_pos_xy + drawable.translation, in_height_offset, 1.0);
    } else {
        projected_pos = drawable.label_plane_matrix * projectTileWithElevation(projected_pos_xy + drawable.translation, in_height_offset, projection);
    }
    const float z = float(drawable.pitch_with_map) * projected_pos.z / projected_pos.w;

    float projectionScaling = 1.0;
#ifdef PROJECTION_GLOBE
    if (drawable.pitch_with_map != 0) {
        const float anchor_pos_tile_y = (drawable.coord_matrix * vec4(projected_pos.xy / projected_pos.w, z, 1.0)).y;
        projectionScaling = mix(projectionScaling, 1.0 / circumferenceRatioAtTileY(anchor_pos_tile_y, projection) * drawable.pitched_scale, projection.projection_transition);
    }
#endif
    const vec2 pos_rot = a_offset / 32.0 * fontScale + a_pxoffset;
    gl_Position = drawable.coord_matrix * vec4(projected_pos.xy / projected_pos.w + rotation_matrix * pos_rot * projectionScaling, z, 1.0);
    if (drawable.pitch_with_map != 0) {
        gl_Position = projectTileWithElevation(gl_Position.xy, gl_Position.z, projection);
    }

    const vec2 tex_origin = vec2(in_tex_origin);
    frag_tex = (a_tex - tex_origin) / drawable.texsize;
    frag_tex_origin = tex_origin / drawable.texsize;
#if defined(HAS_UNIFORM_u_opacity)
    frag_fade_opacity = fade_opacity;
#else
    frag_fade_opacity = fade_opacity * opacity;
#endif

#if defined(HAS_UNIFORM_u_halo_width)
    const float halo_width = drawable.is_text_prop != 0 ? props.text_halo_width : props.icon_halo_width;
#else
    const float halo_width = unpack_mix_float(in_halo_width, drawable.halo_width_t);
#endif
#if defined(HAS_UNIFORM_u_halo_blur)
    const float halo_blur = drawable.is_text_prop != 0 ? props.text_halo_blur : props.icon_halo_blur;
#else
    const float halo_blur = unpack_mix_float(in_halo_blur, drawable.halo_blur_t);
#endif
    const float EDGE_GAMMA = 0.105 / DEVICE_PIXEL_RATIO;
    const float haloBlurGamma = drawable.is_halo != 0 ? halo_blur * 1.19 / SDF_PX : 0.0;
    frag_gamma_scaled = (haloBlurGamma + EDGE_GAMMA) / (fontScale * drawable.gamma_scale) * gl_Position.w;
    frag_halo_edge = (6.0 - halo_width / fontScale) / SDF_PX;

#if !defined(HAS_UNIFORM_u_fill_color)
    frag_fill_color = unpack_mix_color(in_fill_color, drawable.fill_color_t);
#endif
#if !defined(HAS_UNIFORM_u_halo_color)
    frag_halo_color = unpack_mix_color(in_halo_color, drawable.halo_color_t);
#endif
}
",
    fragment: r"

layout(location = 0) in vec2 frag_tex;
layout(location = 1) in float frag_fade_opacity;
layout(location = 2) in float frag_gamma_scaled;
layout(location = 3) in float frag_halo_edge;

#if !defined(HAS_UNIFORM_u_fill_color)
layout(location = 4) in vec4 frag_fill_color;
#endif

#if !defined(HAS_UNIFORM_u_halo_color)
layout(location = 5) in vec4 frag_halo_color;
#endif

// The texture coordinate is relative to the quad's atlas origin, which
// atlas_pos adds back.
layout(location = 9) flat in vec2 frag_tex_origin;

layout(location = 0) out vec4 out_color;

layout(push_constant) uniform Constants {
    int ubo_index;
} constant;

struct SymbolTilePropsUBO {
    int is_text;
    int is_halo;
    float pad1;
    float pad2;
};

layout(std140, set = LAYER_SET_INDEX, binding = idSymbolTilePropsUBO) readonly buffer SymbolTilePropsUBOVector {
    SymbolTilePropsUBO tile_props_ubo[];
} tilePropsVector;

layout(set = LAYER_SET_INDEX, binding = idSymbolEvaluatedPropsUBO) uniform SymbolEvaluatedPropsUBO {
    vec4 text_fill_color;
    vec4 text_halo_color;
    float text_opacity;
    float text_halo_width;
    float text_halo_blur;
    float pad1;
    vec4 icon_fill_color;
    vec4 icon_halo_color;
    float icon_opacity;
    float icon_halo_width;
    float icon_halo_blur;
    float fade_change;
} props;

layout(set = DRAWABLE_IMAGE_SET_INDEX, binding = 0) uniform texture2D image0_texture;
layout(set = DRAWABLE_IMAGE_SET_INDEX, binding = samplerBindingStartId) uniform sampler image0_sampler;

void main() {

#if defined(OVERDRAW_INSPECTOR)
    out_color = vec4(1.0);
    return;
#else

    const SymbolTilePropsUBO tileProps = tilePropsVector.tile_props_ubo[constant.ubo_index];

#if defined(HAS_UNIFORM_u_fill_color)
    const vec4 fill_color = tileProps.is_text != 0 ? props.text_fill_color : props.icon_fill_color;
#else
    const vec4 fill_color = frag_fill_color;
#endif
#if defined(HAS_UNIFORM_u_halo_color)
    const vec4 halo_color = tileProps.is_text != 0 ? props.text_halo_color : props.icon_halo_color;
#else
    const vec4 halo_color = frag_halo_color;
#endif
#if defined(HAS_UNIFORM_u_opacity)
    const float opacity = (tileProps.is_text != 0 ? props.text_opacity : props.icon_opacity) * frag_fade_opacity;
#else
    const float opacity = frag_fade_opacity;
#endif

    const float gammaScaled = frag_gamma_scaled;
    const vec4 color = tileProps.is_halo != 0 ? halo_color : fill_color;
    // A halo's inner edge sits a gamma outside the fill's.
    const float fillInnerEdge = (256.0 - 64.0) / 256.0;
    const float innerEdge = tileProps.is_halo != 0 ? fillInnerEdge + gammaScaled : fillInnerEdge;
    const float dist = texture(sampler2D(image0_texture, image0_sampler), atlas_pos(frag_tex_origin, frag_tex)).a;
    float alpha = smoothstep(innerEdge - gammaScaled, innerEdge + gammaScaled, dist);
    if (tileProps.is_halo != 0) {
        // When drawing halos, if the fill is translucent we want
        // the inside of the halo to be translucent as well
        alpha = min(smoothstep(frag_halo_edge - gammaScaled, frag_halo_edge + gammaScaled, dist), 1.0 - alpha);
    }

    out_color = color * (alpha * opacity);
#endif
}
",
};

const SYMBOL_TEXT_AND_ICON_SHADER: Glsl = Glsl {
    prelude: SYMBOL_SHADER_PRELUDE,
    vertex: r"

#define SDF 1.0
#define ICON 0.0

layout(location = 0) in ivec4 in_pos_offset;
layout(location = 1) in uvec4 in_data;
layout(location = 2) in vec3 in_projected_pos;
layout(location = 3) in float in_fade_opacity;

#if !defined(HAS_UNIFORM_u_fill_color)
layout(location = 4) in vec4 in_fill_color;
#endif

#if !defined(HAS_UNIFORM_u_halo_color)
layout(location = 5) in vec4 in_halo_color;
#endif

#if !defined(HAS_UNIFORM_u_opacity)
layout(location = 6) in vec2 in_opacity;
#endif

#if !defined(HAS_UNIFORM_u_halo_width)
layout(location = 7) in vec2 in_halo_width;
#endif

#if !defined(HAS_UNIFORM_u_halo_blur)
layout(location = 8) in vec2 in_halo_blur;
#endif

// The top-left of the quad's atlas rectangle.
layout(location = 9) in uvec2 in_tex_origin;
// The symbol's symbol-height-offset, in meters.
layout(location = 10) in float in_height_offset;

layout(push_constant) uniform Constants {
    int ubo_index;
} constant;

struct SymbolDrawableUBO {
    mat4 matrix;
    mat4 label_plane_matrix;
    mat4 coord_matrix;

    vec2 texsize;
    vec2 texsize_icon;

    int is_text_prop;
	int rotate_symbol;
    int pitch_with_map;
    int is_size_zoom_constant;
    int is_size_feature_constant;
    int is_offset;

    float size_t;
    float size;

    // Interpolations
    float fill_color_t;
    float halo_color_t;
    float opacity_t;
    float halo_width_t;
    float halo_blur_t;

    int is_along_line;
    int is_variable_anchor;
    float pitched_scale;
    vec2 translation;
    float gamma_scale;
    int is_halo;
};

layout(std140, set = LAYER_SET_INDEX, binding = idSymbolDrawableUBO) readonly buffer SymbolDrawableUBOVector {
    SymbolDrawableUBO drawable_ubo[];
} drawableVector;

layout(std140, set = LAYER_SET_INDEX, binding = idProjectionUBO) readonly buffer ProjectionUBOVector {
    ProjectionUBO projection_ubo[];
} projectionVector;

layout(set = LAYER_SET_INDEX, binding = idSymbolEvaluatedPropsUBO) uniform SymbolEvaluatedPropsUBO {
    vec4 text_fill_color;
    vec4 text_halo_color;
    float text_opacity;
    float text_halo_width;
    float text_halo_blur;
    float pad1;
    vec4 icon_fill_color;
    vec4 icon_halo_color;
    float icon_opacity;
    float icon_halo_width;
    float icon_halo_blur;
    float fade_change;
} props;

layout(location = 0) out vec2 frag_tex;
// The fade opacity, times the opacity when it's per vertex.
layout(location = 1) out float frag_fade_opacity;
// A glyph's edge's gamma, scaled by clip w, and the inner edge of its halo:
// the same over a quad but for the clip w, which interpolates linearly.
layout(location = 2) out float frag_gamma_scaled;
layout(location = 3) out float frag_halo_edge;

#if !defined(HAS_UNIFORM_u_fill_color)
layout(location = 4) out vec4 frag_fill_color;
#endif

#if !defined(HAS_UNIFORM_u_halo_color)
layout(location = 5) out vec4 frag_halo_color;
#endif

layout(location = 9) flat out int frag_is_icon;

// The texture coordinate is relative to the quad's atlas origin, which
// atlas_pos adds back.
layout(location = 10) flat out vec2 frag_tex_origin;

void main() {
    const SymbolDrawableUBO drawable = drawableVector.drawable_ubo[constant.ubo_index];
    const ProjectionUBO projection = projectionVector.projection_ubo[constant.ubo_index];

    const vec2 raw_fade_opacity = unpack_opacity(in_fade_opacity);
    const float fade_change = raw_fade_opacity[1] > 0.5 ? props.fade_change : -props.fade_change;
    const float fade_opacity = max(0.0, min(1.0, raw_fade_opacity[0] + fade_change));
#if defined(HAS_UNIFORM_u_opacity)
    const float visibility = fade_opacity;
#else
    const float opacity = unpack_mix_float(in_opacity, drawable.opacity_t);
    const float visibility = min(fade_opacity, opacity);
#endif
    // A quad that draws nothing goes to one point off screen, so it's never
    // rasterized. All four vertices of a quad share its fade and opacity.
    if (visibility == 0.0) {
        gl_Position = vec4(-2.0, -2.0, -2.0, 1.0);
        return;
    }

    const vec2 a_pos = in_pos_offset.xy;
    const vec2 a_offset = in_pos_offset.zw;

    const vec2 a_tex = in_data.xy;
    const vec2 a_size = in_data.zw;

    const float a_size_min = floor(a_size[0] * 0.5);
    const float is_sdf = a_size[0] - 2.0 * a_size_min;
    const float segment_angle = -in_projected_pos[2];

    float size;
    if (drawable.is_size_zoom_constant == 0 && drawable.is_size_feature_constant == 0) {
        size = mix(a_size_min, a_size[1], drawable.size_t) / 128.0;
    } else if (drawable.is_size_zoom_constant != 0 && drawable.is_size_feature_constant == 0) {
        size = a_size_min / 128.0;
    } else {
        size = drawable.size;
    }

    const vec2 translated_a_pos = a_pos + drawable.translation;
    const vec4 projectedPoint = projectTileWithElevation(translated_a_pos, in_height_offset, projection);
#ifdef PROJECTION_GLOBE
    // A screen-aligned label whose anchor is behind the horizon goes at
    // once, rather than fading out once placement next finds it occluded:
    // its clip Z is 0, so the horizon never clips it. A pitched label's
    // vertices go through the globe projection, which clips them.
    // A raised label is left to placement, which hides it once the planet
    // blocks the line of sight to it.
    if (drawable.pitch_with_map == 0 && in_height_offset <= 0.0 && projectedPoint.z > projectedPoint.w) {
        gl_Position = vec4(-2.0, -2.0, -2.0, 1.0);
        return;
    }
#endif
    const float camera_to_anchor_distance = projectedPoint.w;
    // If the label is pitched with the map, layout is done in pitched space,
    // which makes labels in the distance smaller relative to viewport space.
    // We counteract part of that effect by multiplying by the perspective ratio.
    // If the label isn't pitched with the map, we do layout in viewport space,
    // which makes labels in the distance larger relative to the features around
    // them. We counteract part of that effect by dividing by the perspective ratio.
    const float distance_ratio = drawable.pitch_with_map != 0 ?
        camera_to_anchor_distance / paintParams.camera_to_center_distance :
        paintParams.camera_to_center_distance / camera_to_anchor_distance;
    const float perspective_ratio = clamp(
            0.5 + 0.5 * distance_ratio,
            0.0, // Prevents oversized near-field symbols in pitched/overzoomed tiles
            4.0);

    if (drawable.is_offset == 0) {
        size *= perspective_ratio;
    }

    const float fontScale = size / 24.0;

    float symbol_rotation = 0.0;
    if (drawable.rotate_symbol != 0) {
        // Point labels with 'rotation-alignment: map' are horizontal with respect to tile units
        // To figure out that angle in projected space, we draw a short horizontal line in tile
        // space, project it, and measure its angle in projected space.
        const vec4 offsetProjectedPoint = projectTileWithElevation(translated_a_pos + vec2(1, 0), in_height_offset, projection);

        const vec2 a = projectedPoint.xy / projectedPoint.w;
        const vec2 b = offsetProjectedPoint.xy / offsetProjectedPoint.w;
        symbol_rotation = atan((b.y - a.y) / paintParams.aspect_ratio, b.x - a.x);
    }

    const float angle_sin = sin(segment_angle + symbol_rotation);
    const float angle_cos = cos(segment_angle + symbol_rotation);
    const mat2 rotation_matrix = mat2(angle_cos, -1.0 * angle_sin, angle_sin, angle_cos);

    const vec2 projected_pos_xy = in_projected_pos.xy;
    vec4 projected_pos;
    if (drawable.is_along_line != 0 || drawable.is_variable_anchor != 0) {
        projected_pos = vec4(projected_pos_xy, in_height_offset, 1.0);
    } else if (drawable.pitch_with_map != 0) {
        projected_pos = drawable.label_plane_matrix * vec4(projected_pos_xy + drawable.translation, in_height_offset, 1.0);
    } else {
        projected_pos = drawable.label_plane_matrix * projectTileWithElevation(projected_pos_xy + drawable.translation, in_height_offset, projection);
    }
    const float z = float(drawable.pitch_with_map) * projected_pos.z / projected_pos.w;

    float projectionScaling = 1.0;
#ifdef PROJECTION_GLOBE
    if (drawable.pitch_with_map != 0 && drawable.is_along_line == 0) {
        const float anchor_pos_tile_y = (drawable.coord_matrix * vec4(projected_pos.xy / projected_pos.w, z, 1.0)).y;
        projectionScaling = mix(projectionScaling, 1.0 / circumferenceRatioAtTileY(anchor_pos_tile_y, projection) * drawable.pitched_scale, projection.projection_transition);
    }
#endif
    const vec2 pos_rot = a_offset / 32.0 * fontScale;
    gl_Position = drawable.coord_matrix * vec4(projected_pos.xy / projected_pos.w + rotation_matrix * pos_rot * projectionScaling, z, 1.0);
    if (drawable.pitch_with_map != 0) {
        gl_Position = projectTileWithElevation(gl_Position.xy, gl_Position.z, projection);
    }

    const bool is_icon = (is_sdf == ICON);
	frag_is_icon = int(is_icon);

    const vec2 texsize = is_icon ? drawable.texsize_icon : drawable.texsize;
    const vec2 tex_origin = vec2(in_tex_origin);
    frag_tex = (a_tex - tex_origin) / texsize;
    frag_tex_origin = tex_origin / texsize;
#if defined(HAS_UNIFORM_u_opacity)
    frag_fade_opacity = fade_opacity;
#else
    frag_fade_opacity = fade_opacity * opacity;
#endif

#if defined(HAS_UNIFORM_u_halo_width)
    const float halo_width = drawable.is_text_prop != 0 ? props.text_halo_width : props.icon_halo_width;
#else
    const float halo_width = unpack_mix_float(in_halo_width, drawable.halo_width_t);
#endif
#if defined(HAS_UNIFORM_u_halo_blur)
    const float halo_blur = drawable.is_text_prop != 0 ? props.text_halo_blur : props.icon_halo_blur;
#else
    const float halo_blur = unpack_mix_float(in_halo_blur, drawable.halo_blur_t);
#endif
    const float EDGE_GAMMA = 0.105 / DEVICE_PIXEL_RATIO;
    const float haloBlurGamma = drawable.is_halo != 0 ? halo_blur * 1.19 / SDF_PX : 0.0;
    frag_gamma_scaled = (haloBlurGamma + EDGE_GAMMA) / (fontScale * drawable.gamma_scale) * gl_Position.w;
    frag_halo_edge = (6.0 - halo_width / fontScale) / SDF_PX;

#if !defined(HAS_UNIFORM_u_fill_color)
    frag_fill_color = unpack_mix_color(in_fill_color, drawable.fill_color_t);
#endif
#if !defined(HAS_UNIFORM_u_halo_color)
    frag_halo_color = unpack_mix_color(in_halo_color, drawable.halo_color_t);
#endif
}
",
    fragment: r"

layout(location = 0) in vec2 frag_tex;
layout(location = 1) in float frag_fade_opacity;
layout(location = 2) in float frag_gamma_scaled;
layout(location = 3) in float frag_halo_edge;

#if !defined(HAS_UNIFORM_u_fill_color)
layout(location = 4) in vec4 frag_fill_color;
#endif

#if !defined(HAS_UNIFORM_u_halo_color)
layout(location = 5) in vec4 frag_halo_color;
#endif

layout(location = 9) flat in int frag_is_icon;

// The texture coordinate is relative to the quad's atlas origin, which
// atlas_pos adds back.
layout(location = 10) flat in vec2 frag_tex_origin;

layout(location = 0) out vec4 out_color;

layout(push_constant) uniform Constants {
    int ubo_index;
} constant;

struct SymbolTilePropsUBO {
    int is_text;
    int is_halo;
    float pad1;
    float pad2;
};

layout(std140, set = LAYER_SET_INDEX, binding = idSymbolTilePropsUBO) readonly buffer SymbolTilePropsUBOVector {
    SymbolTilePropsUBO tile_props_ubo[];
} tilePropsVector;

layout(set = LAYER_SET_INDEX, binding = idSymbolEvaluatedPropsUBO) uniform SymbolEvaluatedPropsUBO {
    vec4 text_fill_color;
    vec4 text_halo_color;
    float text_opacity;
    float text_halo_width;
    float text_halo_blur;
    float pad1;
    vec4 icon_fill_color;
    vec4 icon_halo_color;
    float icon_opacity;
    float icon_halo_width;
    float icon_halo_blur;
    float fade_change;
} props;

layout(set = DRAWABLE_IMAGE_SET_INDEX, binding = 0) uniform texture2D glyph_image_texture;
layout(set = DRAWABLE_IMAGE_SET_INDEX, binding = samplerBindingStartId) uniform sampler glyph_image_sampler;
layout(set = DRAWABLE_IMAGE_SET_INDEX, binding = 1) uniform texture2D icon_image_texture;
layout(set = DRAWABLE_IMAGE_SET_INDEX, binding = samplerBindingStartId + 1) uniform sampler icon_image_sampler;

void main() {
#if defined(OVERDRAW_INSPECTOR)
    out_color = vec4(1.0);
    return;
#else

    const SymbolTilePropsUBO tileProps = tilePropsVector.tile_props_ubo[constant.ubo_index];

#if defined(HAS_UNIFORM_u_fill_color)
    const vec4 fill_color = tileProps.is_text != 0 ? props.text_fill_color : props.icon_fill_color;
#else
    const vec4 fill_color = frag_fill_color;
#endif
#if defined(HAS_UNIFORM_u_halo_color)
    const vec4 halo_color = tileProps.is_text != 0 ? props.text_halo_color : props.icon_halo_color;
#else
    const vec4 halo_color = frag_halo_color;
#endif
#if defined(HAS_UNIFORM_u_opacity)
    const float opacity = (tileProps.is_text != 0 ? props.text_opacity : props.icon_opacity) * frag_fade_opacity;
#else
    const float opacity = frag_fade_opacity;
#endif

    const vec2 pos = atlas_pos(frag_tex_origin, frag_tex);

    if (bool(frag_is_icon)) {
        out_color = texture(sampler2D(icon_image_texture, icon_image_sampler), pos) * opacity;
        return;
    }

    const float gammaScaled = frag_gamma_scaled;
    const vec4 color = tileProps.is_halo != 0 ? halo_color : fill_color;
    // A halo's inner edge sits a gamma outside the fill's.
    const float fillInnerEdge = (256.0 - 64.0) / 256.0;
    const float innerEdge = tileProps.is_halo != 0 ? fillInnerEdge + gammaScaled : fillInnerEdge;
    const float dist = texture(sampler2D(glyph_image_texture, glyph_image_sampler), pos).a;
    float alpha = smoothstep(innerEdge - gammaScaled, innerEdge + gammaScaled, dist);
    if (tileProps.is_halo != 0) {
        // When drawing halos, if the fill is translucent we want
        // the inside of the halo to be translucent as well
        alpha = min(smoothstep(frag_halo_edge - gammaScaled, frag_halo_edge + gammaScaled, dist), 1.0 - alpha);
    }

    out_color = color * (alpha * opacity);
#endif
}
",
};
