//! The programs of the `fill-extrusion` layer type in GLSL ES, for `mln_gl`.

use mln_gfx::shaders::layer_ubo::{GlobalPaintParamsUBO, ProjectionUBO};
use mln_gfx::shaders::program::Program;
use mln_gfx::shaders::shader_defines::{
    ID_FILL_EXTRUSION_BASE_VERTEX_ATTRIBUTE, ID_FILL_EXTRUSION_COLOR_VERTEX_ATTRIBUTE,
    ID_FILL_EXTRUSION_DECIMALS_ED_ATTRIBUTE, ID_FILL_EXTRUSION_DRAWABLE_UBO, ID_FILL_EXTRUSION_HEIGHT_VERTEX_ATTRIBUTE,
    ID_FILL_EXTRUSION_IMAGE_TEXTURE, ID_FILL_EXTRUSION_NEXT_DECIMALS_ED_ATTRIBUTE,
    ID_FILL_EXTRUSION_NEXT_OUTLINE_POS_ATTRIBUTE, ID_FILL_EXTRUSION_OUTLINE_POS_ATTRIBUTE,
    ID_FILL_EXTRUSION_PATTERN_FROM_VERTEX_ATTRIBUTE, ID_FILL_EXTRUSION_PATTERN_TO_VERTEX_ATTRIBUTE,
    ID_FILL_EXTRUSION_POS_VERTEX_ATTRIBUTE, ID_FILL_EXTRUSION_PROPS_UBO, ID_FILL_EXTRUSION_TILE_PROPS_UBO,
    ID_GLOBAL_PAINT_PARAMS_UBO, ID_PROJECTION_UBO,
};
use mln_gfx::shaders::source::glsl_es::{Attribute, Glsl, Source, Texture, UniformBlock};

use super::programs;
use super::ubo::{FillExtrusionDrawableUBO, FillExtrusionPropsUBO, FillExtrusionTilePropsUBO};

// Fill Extrusion: the roofs
const FILL_EXTRUSION_BLOCKS: &[UniformBlock] = &[
    UniformBlock::element::<ProjectionUBO, ProjectionUBO>("ProjectionUBO", ID_PROJECTION_UBO),
    UniformBlock::element::<FillExtrusionDrawableUBO, FillExtrusionDrawableUBO>(
        "FillExtrusionDrawableUBO",
        ID_FILL_EXTRUSION_DRAWABLE_UBO,
    ),
    UniformBlock::element::<FillExtrusionTilePropsUBO, FillExtrusionTilePropsUBO>(
        "FillExtrusionTilePropsUBO",
        ID_FILL_EXTRUSION_TILE_PROPS_UBO,
    ),
    UniformBlock::new::<FillExtrusionPropsUBO>("FillExtrusionPropsUBO", ID_FILL_EXTRUSION_PROPS_UBO),
];
static FILL_EXTRUSION: Source = Source {
    glsl: &FILL_EXTRUSION_SHADER,
    uniform_blocks: FILL_EXTRUSION_BLOCKS,
    attributes: &[
        Attribute::new("a_pos", ID_FILL_EXTRUSION_POS_VERTEX_ATTRIBUTE),
        Attribute::new("a_decimals_ed", ID_FILL_EXTRUSION_DECIMALS_ED_ATTRIBUTE),
        Attribute::new("a_base", ID_FILL_EXTRUSION_BASE_VERTEX_ATTRIBUTE),
        Attribute::new("a_height", ID_FILL_EXTRUSION_HEIGHT_VERTEX_ATTRIBUTE),
        Attribute::new("a_color", ID_FILL_EXTRUSION_COLOR_VERTEX_ATTRIBUTE),
    ],
    instance_attributes: &[],
    textures: &[],
};

// Fill Extrusion Instanced: the walls, an instance per outline vertex
const FILL_EXTRUSION_INSTANCED_ATTRIBUTES: &[Attribute] =
    &[Attribute::new("a_pos", ID_FILL_EXTRUSION_POS_VERTEX_ATTRIBUTE)];
static FILL_EXTRUSION_INSTANCED: Source = Source {
    glsl: &FILL_EXTRUSION_INSTANCED_SHADER,
    uniform_blocks: FILL_EXTRUSION_BLOCKS,
    attributes: FILL_EXTRUSION_INSTANCED_ATTRIBUTES,
    instance_attributes: &[
        Attribute::new("a_outline_pos", ID_FILL_EXTRUSION_OUTLINE_POS_ATTRIBUTE),
        Attribute::new("a_decimals_ed", ID_FILL_EXTRUSION_DECIMALS_ED_ATTRIBUTE),
        Attribute::new("a_next_outline_pos", ID_FILL_EXTRUSION_NEXT_OUTLINE_POS_ATTRIBUTE),
        Attribute::new("a_next_decimals_ed", ID_FILL_EXTRUSION_NEXT_DECIMALS_ED_ATTRIBUTE),
        Attribute::new("a_base", ID_FILL_EXTRUSION_BASE_VERTEX_ATTRIBUTE),
        Attribute::new("a_height", ID_FILL_EXTRUSION_HEIGHT_VERTEX_ATTRIBUTE),
        Attribute::new("a_color", ID_FILL_EXTRUSION_COLOR_VERTEX_ATTRIBUTE),
    ],
    textures: &[],
};

// Fill Extrusion Wall: the instanced walls' GLSL with every attribute per
// vertex, for walls whose quads are in the vertex buffer
static FILL_EXTRUSION_WALL: Source = Source {
    glsl: &FILL_EXTRUSION_INSTANCED_SHADER,
    uniform_blocks: FILL_EXTRUSION_BLOCKS,
    attributes: &[
        Attribute::new("a_pos", ID_FILL_EXTRUSION_POS_VERTEX_ATTRIBUTE),
        Attribute::new("a_outline_pos", ID_FILL_EXTRUSION_OUTLINE_POS_ATTRIBUTE),
        Attribute::new("a_decimals_ed", ID_FILL_EXTRUSION_DECIMALS_ED_ATTRIBUTE),
        Attribute::new("a_next_outline_pos", ID_FILL_EXTRUSION_NEXT_OUTLINE_POS_ATTRIBUTE),
        Attribute::new("a_next_decimals_ed", ID_FILL_EXTRUSION_NEXT_DECIMALS_ED_ATTRIBUTE),
        Attribute::new("a_base", ID_FILL_EXTRUSION_BASE_VERTEX_ATTRIBUTE),
        Attribute::new("a_height", ID_FILL_EXTRUSION_HEIGHT_VERTEX_ATTRIBUTE),
        Attribute::new("a_color", ID_FILL_EXTRUSION_COLOR_VERTEX_ATTRIBUTE),
    ],
    instance_attributes: &[],
    textures: &[],
};

// Fill Extrusion Pattern: the roofs
const FILL_EXTRUSION_PATTERN_BLOCKS: &[UniformBlock] = &[
    UniformBlock::element::<ProjectionUBO, ProjectionUBO>("ProjectionUBO", ID_PROJECTION_UBO),
    UniformBlock::new::<GlobalPaintParamsUBO>("GlobalPaintParamsUBO", ID_GLOBAL_PAINT_PARAMS_UBO),
    UniformBlock::element::<FillExtrusionDrawableUBO, FillExtrusionDrawableUBO>(
        "FillExtrusionDrawableUBO",
        ID_FILL_EXTRUSION_DRAWABLE_UBO,
    ),
    UniformBlock::element::<FillExtrusionTilePropsUBO, FillExtrusionTilePropsUBO>(
        "FillExtrusionTilePropsUBO",
        ID_FILL_EXTRUSION_TILE_PROPS_UBO,
    ),
    UniformBlock::new::<FillExtrusionPropsUBO>("FillExtrusionPropsUBO", ID_FILL_EXTRUSION_PROPS_UBO),
];
const FILL_EXTRUSION_PATTERN_TEXTURES: &[Texture] = &[Texture::new("u_image", ID_FILL_EXTRUSION_IMAGE_TEXTURE)];
static FILL_EXTRUSION_PATTERN: Source = Source {
    glsl: &FILL_EXTRUSION_PATTERN_SHADER,
    uniform_blocks: FILL_EXTRUSION_PATTERN_BLOCKS,
    attributes: &[
        Attribute::new("a_pos", ID_FILL_EXTRUSION_POS_VERTEX_ATTRIBUTE),
        Attribute::new("a_decimals_ed", ID_FILL_EXTRUSION_DECIMALS_ED_ATTRIBUTE),
        Attribute::new("a_base", ID_FILL_EXTRUSION_BASE_VERTEX_ATTRIBUTE),
        Attribute::new("a_height", ID_FILL_EXTRUSION_HEIGHT_VERTEX_ATTRIBUTE),
        Attribute::new("a_pattern_from", ID_FILL_EXTRUSION_PATTERN_FROM_VERTEX_ATTRIBUTE),
        Attribute::new("a_pattern_to", ID_FILL_EXTRUSION_PATTERN_TO_VERTEX_ATTRIBUTE),
    ],
    instance_attributes: &[],
    textures: FILL_EXTRUSION_PATTERN_TEXTURES,
};

// Fill Extrusion Pattern Instanced: the walls
static FILL_EXTRUSION_PATTERN_INSTANCED: Source = Source {
    glsl: &FILL_EXTRUSION_PATTERN_INSTANCED_SHADER,
    uniform_blocks: FILL_EXTRUSION_PATTERN_BLOCKS,
    attributes: FILL_EXTRUSION_INSTANCED_ATTRIBUTES,
    instance_attributes: &[
        Attribute::new("a_outline_pos", ID_FILL_EXTRUSION_OUTLINE_POS_ATTRIBUTE),
        Attribute::new("a_decimals_ed", ID_FILL_EXTRUSION_DECIMALS_ED_ATTRIBUTE),
        Attribute::new("a_next_outline_pos", ID_FILL_EXTRUSION_NEXT_OUTLINE_POS_ATTRIBUTE),
        Attribute::new("a_next_decimals_ed", ID_FILL_EXTRUSION_NEXT_DECIMALS_ED_ATTRIBUTE),
        Attribute::new("a_base", ID_FILL_EXTRUSION_BASE_VERTEX_ATTRIBUTE),
        Attribute::new("a_height", ID_FILL_EXTRUSION_HEIGHT_VERTEX_ATTRIBUTE),
        Attribute::new("a_pattern_from", ID_FILL_EXTRUSION_PATTERN_FROM_VERTEX_ATTRIBUTE),
        Attribute::new("a_pattern_to", ID_FILL_EXTRUSION_PATTERN_TO_VERTEX_ATTRIBUTE),
    ],
    textures: FILL_EXTRUSION_PATTERN_TEXTURES,
};

// Fill Extrusion Pattern Wall: as Fill Extrusion Wall
static FILL_EXTRUSION_PATTERN_WALL: Source = Source {
    glsl: &FILL_EXTRUSION_PATTERN_INSTANCED_SHADER,
    uniform_blocks: FILL_EXTRUSION_PATTERN_BLOCKS,
    attributes: &[
        Attribute::new("a_pos", ID_FILL_EXTRUSION_POS_VERTEX_ATTRIBUTE),
        Attribute::new("a_outline_pos", ID_FILL_EXTRUSION_OUTLINE_POS_ATTRIBUTE),
        Attribute::new("a_decimals_ed", ID_FILL_EXTRUSION_DECIMALS_ED_ATTRIBUTE),
        Attribute::new("a_next_outline_pos", ID_FILL_EXTRUSION_NEXT_OUTLINE_POS_ATTRIBUTE),
        Attribute::new("a_next_decimals_ed", ID_FILL_EXTRUSION_NEXT_DECIMALS_ED_ATTRIBUTE),
        Attribute::new("a_base", ID_FILL_EXTRUSION_BASE_VERTEX_ATTRIBUTE),
        Attribute::new("a_height", ID_FILL_EXTRUSION_HEIGHT_VERTEX_ATTRIBUTE),
        Attribute::new("a_pattern_from", ID_FILL_EXTRUSION_PATTERN_FROM_VERTEX_ATTRIBUTE),
        Attribute::new("a_pattern_to", ID_FILL_EXTRUSION_PATTERN_TO_VERTEX_ATTRIBUTE),
    ],
    instance_attributes: &[],
    textures: FILL_EXTRUSION_PATTERN_TEXTURES,
};

/// The source of each of the type's programs.
pub(super) static SOURCES: &[(&Program, &Source)] = &[
    (&programs::FILL_EXTRUSION, &FILL_EXTRUSION),
    (&programs::FILL_EXTRUSION_INSTANCED, &FILL_EXTRUSION_INSTANCED),
    (&programs::FILL_EXTRUSION_WALL, &FILL_EXTRUSION_WALL),
    (&programs::FILL_EXTRUSION_PATTERN, &FILL_EXTRUSION_PATTERN),
    (&programs::FILL_EXTRUSION_PATTERN_INSTANCED, &FILL_EXTRUSION_PATTERN_INSTANCED),
    (&programs::FILL_EXTRUSION_PATTERN_WALL, &FILL_EXTRUSION_PATTERN_WALL),
];

const FILL_EXTRUSION_INSTANCED_SHADER: Glsl = Glsl {
    vertex: r"invariant gl_Position;

// The corner of the wall quad: x picks the instance's outline vertex (0) or
// the next one (1), y the base (0) or the top (1).
layout (location = 0) in vec2 a_pos;
// Per instance: an outline vertex and the one after it.
layout (location = 1) in vec2 a_outline_pos;
layout (location = 2) in vec2 a_decimals_ed;
layout (location = 8) in vec2 a_next_outline_pos;
layout (location = 9) in vec2 a_next_decimals_ed;
out vec4 v_color;

layout (std140) uniform FillExtrusionDrawableUBO {
    highp mat4 u_matrix;
    highp vec2 u_pixel_coord_upper;
    highp vec2 u_pixel_coord_lower;
    highp float u_height_factor;
    highp float u_tile_ratio;
    // Interpolations
    highp float u_base_t;
    highp float u_height_t;
    highp float u_color_t;
    highp float u_pattern_from_t;
    highp float u_pattern_to_t;
    lowp float drawable_pad1;
};

layout (std140) uniform FillExtrusionTilePropsUBO {
    highp vec4 u_pattern_from;
    highp vec4 u_pattern_to;
    highp vec2 u_texsize;
    lowp float tileprops_pad1;
    lowp float tileprops_pad2;
};

layout (std140) uniform FillExtrusionPropsUBO {
    highp vec4 u_color;
    highp vec3 u_lightcolor;
    lowp float props_pad1;
    highp vec3 u_lightpos;
    highp float u_base;
    highp float u_height;
    highp float u_lightintensity;
    highp float u_vertical_gradient;
    highp float u_opacity;
    highp float u_fade;
    highp float u_from_scale;
    highp float u_to_scale;
    lowp float props_pad2;
};

#ifndef HAS_UNIFORM_u_base
layout (location = 3) in highp vec2 a_base;
#endif
#ifndef HAS_UNIFORM_u_height
layout (location = 4) in highp vec2 a_height;
#endif
#ifndef HAS_UNIFORM_u_color
layout (location = 5) in highp vec4 a_color;
#endif

void main() {
    // The vertex that ends a ring has no wall.
    if (mod(a_decimals_ed.x, 2.0) > 0.0) {
        gl_Position = vec4(0.0);
        return;
    }

    #ifndef HAS_UNIFORM_u_base
highp float base = unpack_mix_vec2(a_base, u_base_t);
#else
highp float base = u_base;
#endif
    #ifndef HAS_UNIFORM_u_height
highp float height = unpack_mix_vec2(a_height, u_height_t);
#else
highp float height = u_height;
#endif
    #ifndef HAS_UNIFORM_u_color
highp vec4 color = unpack_mix_color(a_color, u_color_t);
#else
highp vec4 color = u_color;
#endif

    vec2 p1 = a_outline_pos;
    vec2 p2 = a_next_outline_pos;
    vec2 edge = normalize(p2 - p1);
    vec2 own = vec2(-edge.y, edge.x);
    vec3 normal = vec3(a_pos.x == 0.0 ? wall_normal(a_decimals_ed.x, own) : wall_normal(a_next_decimals_ed.x, own), 0.0);

    base = max(0.0, base);
    height = max(0.0, height);

    float t = a_pos.y;
    float z = t > 0.0 ? height : base;

    gl_Position = projectTileFor3D(a_pos.x == 0.0 ? p1 : p2, z);

    // Relative luminance (how dark/bright is the surface color?)
    float colorvalue = color.r * 0.2126 + color.g * 0.7152 + color.b * 0.0722;

    v_color = vec4(0.0, 0.0, 0.0, 1.0);

    // Add slight ambient lighting so no extrusions are totally black
    vec4 ambientlight = vec4(0.03, 0.03, 0.03, 1.0);
    color += ambientlight;

    // Calculate cos(theta), where theta is the angle between surface normal and diffuse light ray
    float directional = clamp(dot(normal, u_lightpos), 0.0, 1.0);

    // Adjust directional so that
    // the range of values for highlight/shading is narrower
    // with lower light intensity
    // and with lighter/brighter surface colors
    directional = mix((1.0 - u_lightintensity), max((1.0 - colorvalue + u_lightintensity), 1.0), directional);

    // Add gradient along z axis of side surfaces
    if (normal.z == 0.0) {
        // This avoids another branching statement, but multiplies by a constant of 0.84 if no vertical gradient,
        // and otherwise calculates the gradient based on base + height
        float fMin = mix(0.7, 0.98, 1.0 - u_lightintensity);
        float factor = clamp((t + base) * pow(height / 150.0, 0.5), fMin, 1.0);
        directional *= (1.0 - u_vertical_gradient) + (u_vertical_gradient * factor);
    }

    // Assign final color based on surface + ambient light color, diffuse light directional, and light color
    // with lower bounds adjusted to hue of light
    // so that shading is tinted with the complementary (opposite) color to the light color
    v_color.r += clamp(color.r * directional * u_lightcolor.r, mix(0.0, 0.3, 1.0 - u_lightcolor.r), 1.0);
    v_color.g += clamp(color.g * directional * u_lightcolor.g, mix(0.0, 0.3, 1.0 - u_lightcolor.g), 1.0);
    v_color.b += clamp(color.b * directional * u_lightcolor.b, mix(0.0, 0.3, 1.0 - u_lightcolor.b), 1.0);
    v_color *= u_opacity;
}
",
    fragment: r"in vec4 v_color;

void main() {
    fragColor = v_color;

#ifdef OVERDRAW_INSPECTOR
    fragColor = vec4(1.0);
#endif
}
",
};

const FILL_EXTRUSION_PATTERN_INSTANCED_SHADER: Glsl = Glsl {
    vertex: r"invariant gl_Position;

// The corner of the wall quad: x picks the instance's outline vertex (0) or
// the next one (1), y the base (0) or the top (1).
layout (location = 0) in vec2 a_pos;
// Per instance: an outline vertex and the one after it.
layout (location = 1) in vec2 a_outline_pos;
layout (location = 2) in vec2 a_decimals_ed;
layout (location = 8) in vec2 a_next_outline_pos;
layout (location = 9) in vec2 a_next_decimals_ed;

out vec2 v_pos_a;
out vec2 v_pos_b;
out vec4 v_lighting;

layout (std140) uniform GlobalPaintParamsUBO {
    highp vec2 u_pattern_atlas_texsize;
    highp vec2 u_units_to_pixels;
    highp vec2 u_world_size;
    highp float u_camera_to_center_distance;
    highp float u_symbol_fade_change;
    highp float u_aspect_ratio;
    highp float u_pixel_ratio;
    highp float u_map_zoom;
    lowp float global_pad1;
};

layout (std140) uniform FillExtrusionDrawableUBO {
    highp mat4 u_matrix;
    highp vec2 u_pixel_coord_upper;
    highp vec2 u_pixel_coord_lower;
    highp float u_height_factor;
    highp float u_tile_ratio;
    // Interpolations
    highp float u_base_t;
    highp float u_height_t;
    highp float u_color_t;
    highp float u_pattern_from_t;
    highp float u_pattern_to_t;
    lowp float drawable_pad1;
};

layout (std140) uniform FillExtrusionTilePropsUBO {
    highp vec4 u_pattern_from;
    highp vec4 u_pattern_to;
    highp vec2 u_texsize;
    lowp float tileprops_pad1;
    lowp float tileprops_pad2;
};

layout (std140) uniform FillExtrusionPropsUBO {
    highp vec4 u_color;
    highp vec3 u_lightcolor;
    lowp float props_pad1;
    highp vec3 u_lightpos;
    highp float u_base;
    highp float u_height;
    highp float u_lightintensity;
    highp float u_vertical_gradient;
    highp float u_opacity;
    highp float u_fade;
    highp float u_from_scale;
    highp float u_to_scale;
    lowp float props_pad2;
};

#ifndef HAS_UNIFORM_u_base
layout (location = 3) in highp vec2 a_base;
out highp float base;
#endif
#ifndef HAS_UNIFORM_u_height
layout (location = 4) in highp vec2 a_height;
out highp float height;
#endif
#ifndef HAS_UNIFORM_u_pattern_from
layout (location = 5) in highp vec4 a_pattern_from;
flat out highp vec4 pattern_from;
#endif
#ifndef HAS_UNIFORM_u_pattern_to
layout (location = 6) in highp vec4 a_pattern_to;
flat out highp vec4 pattern_to;
#endif

void main() {
    // The vertex that ends a ring has no wall.
    if (mod(a_decimals_ed.x, 2.0) > 0.0) {
        gl_Position = vec4(0.0);
        return;
    }

    #ifndef HAS_UNIFORM_u_base
base = unpack_mix_vec2(a_base, u_base_t);
#else
highp float base = u_base;
#endif
    #ifndef HAS_UNIFORM_u_height
height = unpack_mix_vec2(a_height, u_height_t);
#else
highp float height = u_height;
#endif
    #ifndef HAS_UNIFORM_u_pattern_from
pattern_from = a_pattern_from;
#else
highp vec4 pattern_from = u_pattern_from;
#endif
    #ifndef HAS_UNIFORM_u_pattern_to
pattern_to = a_pattern_to;
#else
highp vec4 pattern_to = u_pattern_to;
#endif

    vec2 pattern_tl_a = pattern_from.xy;
    vec2 pattern_br_a = pattern_from.zw;
    vec2 pattern_tl_b = pattern_to.xy;
    vec2 pattern_br_b = pattern_to.zw;

    float pixelRatio = u_pixel_ratio;
    float tileRatio = u_tile_ratio;
    float fromScale = u_from_scale;
    float toScale = u_to_scale;

    vec2 p1 = a_outline_pos;
    vec2 p2 = a_next_outline_pos;
    vec2 edge = normalize(p2 - p1);
    vec2 own = vec2(-edge.y, edge.x);
    vec3 normal = vec3(a_pos.x == 0.0 ? wall_normal(a_decimals_ed.x, own) : wall_normal(a_next_decimals_ed.x, own), 0.0);
    float edgedistance = a_pos.x == 0.0 ? a_next_decimals_ed.y : a_decimals_ed.y;

    vec2 display_size_a = vec2((pattern_br_a.x - pattern_tl_a.x) / pixelRatio, (pattern_br_a.y - pattern_tl_a.y) / pixelRatio);
    vec2 display_size_b = vec2((pattern_br_b.x - pattern_tl_b.x) / pixelRatio, (pattern_br_b.y - pattern_tl_b.y) / pixelRatio);

    base = max(0.0, base);
    height = max(0.0, height);

    float t = a_pos.y;
    float z = t > 0.0 ? height : base;

    gl_Position = projectTileFor3D(a_pos.x == 0.0 ? p1 : p2, z);

    // extrusion top or side
    vec2 pos = normal.z == 1.0 ? a_pos : vec2(edgedistance, z * u_height_factor);

    v_pos_a = get_pattern_pos(u_pixel_coord_upper, u_pixel_coord_lower, fromScale * display_size_a, tileRatio, pos);
    v_pos_b = get_pattern_pos(u_pixel_coord_upper, u_pixel_coord_lower, toScale * display_size_b, tileRatio, pos);

    v_lighting = vec4(0.0, 0.0, 0.0, 1.0);
    float directional = clamp(dot(normal, u_lightpos), 0.0, 1.0);
    directional = mix((1.0 - u_lightintensity), max((0.5 + u_lightintensity), 1.0), directional);

    if (normal.z == 0.0) {
        // This avoids another branching statement, but multiplies by a constant of 0.84 if no vertical gradient,
        // and otherwise calculates the gradient based on base + height
        float fMin = mix(0.7, 0.98, 1.0 - u_lightintensity);
        float factor = clamp((t + base) * pow(height / 150.0, 0.5), fMin, 1.0);
        directional *= (1.0 - u_vertical_gradient) + (u_vertical_gradient * factor);
    }

    v_lighting.rgb += clamp(directional * u_lightcolor, mix(vec3(0.0), vec3(0.3), 1.0 - u_lightcolor), vec3(1.0));
    v_lighting *= u_opacity;
}
",
    fragment: r"#ifdef GL_ES
precision highp float;
#endif

in vec2 v_pos_a;
in vec2 v_pos_b;
in vec4 v_lighting;

layout (std140) uniform FillExtrusionTilePropsUBO {
    highp vec4 u_pattern_from;
    highp vec4 u_pattern_to;
    highp vec2 u_texsize;
    lowp float tileprops_pad1;
    lowp float tileprops_pad2;
};

layout (std140) uniform FillExtrusionPropsUBO {
    highp vec4 u_color;
    highp vec3 u_lightcolor;
    lowp float props_pad1;
    highp vec3 u_lightpos;
    highp float u_base;
    highp float u_height;
    highp float u_lightintensity;
    highp float u_vertical_gradient;
    highp float u_opacity;
    highp float u_fade;
    highp float u_from_scale;
    highp float u_to_scale;
    lowp float props_pad2;
};

uniform sampler2D u_image;

#ifndef HAS_UNIFORM_u_base
in highp float base;
#endif
#ifndef HAS_UNIFORM_u_height
in highp float height;
#endif
#ifndef HAS_UNIFORM_u_pattern_from
flat in highp vec4 pattern_from;
#endif
#ifndef HAS_UNIFORM_u_pattern_to
flat in highp vec4 pattern_to;
#endif

void main() {
    #ifdef HAS_UNIFORM_u_base
highp float base = u_base;
#endif
    #ifdef HAS_UNIFORM_u_height
highp float height = u_height;
#endif
    #ifdef HAS_UNIFORM_u_pattern_from
highp vec4 pattern_from = u_pattern_from;
#endif
    #ifdef HAS_UNIFORM_u_pattern_to
highp vec4 pattern_to = u_pattern_to;
#endif

    vec2 pattern_tl_a = pattern_from.xy;
    vec2 pattern_br_a = pattern_from.zw;
    vec2 pattern_tl_b = pattern_to.xy;
    vec2 pattern_br_b = pattern_to.zw;

    // A crossfade at 0 or 1 shows one pattern, so skip the other.
    vec4 color1 = vec4(0.0);
    if (u_fade < 1.0) {
        vec2 imagecoord = mod(v_pos_a, 1.0);
        highp vec2 pos = atlas_pos(pattern_tl_a / u_texsize, (pattern_br_a - pattern_tl_a) / u_texsize * imagecoord);
        color1 = texture(u_image, pos);
    }
    vec4 color2 = vec4(0.0);
    if (u_fade > 0.0) {
        vec2 imagecoord_b = mod(v_pos_b, 1.0);
        highp vec2 pos2 = atlas_pos(pattern_tl_b / u_texsize, (pattern_br_b - pattern_tl_b) / u_texsize * imagecoord_b);
        color2 = texture(u_image, pos2);
    }

    vec4 mixedColor = mix(color1, color2, u_fade);

    fragColor = mixedColor * v_lighting;

#ifdef OVERDRAW_INSPECTOR
    fragColor = vec4(1.0);
#endif
}
",
};

const FILL_EXTRUSION_PATTERN_SHADER: Glsl = Glsl {
    vertex: r"invariant gl_Position;

layout (location = 0) in vec2 a_pos;
layout (location = 1) in vec2 a_decimals_ed;

out vec2 v_pos_a;
out vec2 v_pos_b;
out vec4 v_lighting;

layout (std140) uniform GlobalPaintParamsUBO {
    highp vec2 u_pattern_atlas_texsize;
    highp vec2 u_units_to_pixels;
    highp vec2 u_world_size;
    highp float u_camera_to_center_distance;
    highp float u_symbol_fade_change;
    highp float u_aspect_ratio;
    highp float u_pixel_ratio;
    highp float u_map_zoom;
    lowp float global_pad1;
};

layout (std140) uniform FillExtrusionDrawableUBO {
    highp mat4 u_matrix;
    highp vec2 u_pixel_coord_upper;
    highp vec2 u_pixel_coord_lower;
    highp float u_height_factor;
    highp float u_tile_ratio;
    // Interpolations
    highp float u_base_t;
    highp float u_height_t;
    highp float u_color_t;
    highp float u_pattern_from_t;
    highp float u_pattern_to_t;
    lowp float drawable_pad1;
};

layout (std140) uniform FillExtrusionTilePropsUBO {
    highp vec4 u_pattern_from;
    highp vec4 u_pattern_to;
    highp vec2 u_texsize;
    lowp float tileprops_pad1;
    lowp float tileprops_pad2;
};

layout (std140) uniform FillExtrusionPropsUBO {
    highp vec4 u_color;
    highp vec3 u_lightcolor;
    lowp float props_pad1;
    highp vec3 u_lightpos;
    highp float u_base;
    highp float u_height;
    highp float u_lightintensity;
    highp float u_vertical_gradient;
    highp float u_opacity;
    highp float u_fade;
    highp float u_from_scale;
    highp float u_to_scale;
    lowp float props_pad2;
};

#ifndef HAS_UNIFORM_u_base
layout (location = 3) in highp vec2 a_base;
out highp float base;
#endif
#ifndef HAS_UNIFORM_u_height
layout (location = 4) in highp vec2 a_height;
out highp float height;
#endif
#ifndef HAS_UNIFORM_u_pattern_from
layout (location = 5) in highp vec4 a_pattern_from;
flat out highp vec4 pattern_from;
#endif
#ifndef HAS_UNIFORM_u_pattern_to
layout (location = 6) in highp vec4 a_pattern_to;
flat out highp vec4 pattern_to;
#endif

void main() {
    #ifndef HAS_UNIFORM_u_base
base = unpack_mix_vec2(a_base, u_base_t);
#else
highp float base = u_base;
#endif
    #ifndef HAS_UNIFORM_u_height
height = unpack_mix_vec2(a_height, u_height_t);
#else
highp float height = u_height;
#endif
    #ifndef HAS_UNIFORM_u_pattern_from
pattern_from = a_pattern_from;
#else
highp vec4 pattern_from = u_pattern_from;
#endif
    #ifndef HAS_UNIFORM_u_pattern_to
pattern_to = a_pattern_to;
#else
highp vec4 pattern_to = u_pattern_to;
#endif

    vec2 pattern_tl_a = pattern_from.xy;
    vec2 pattern_br_a = pattern_from.zw;
    vec2 pattern_tl_b = pattern_to.xy;
    vec2 pattern_br_b = pattern_to.zw;

    float pixelRatio = u_pixel_ratio;
    float tileRatio = u_tile_ratio;
    float fromScale = u_from_scale;
    float toScale = u_to_scale;

    // A roof vertex: the walls are the instanced shader's.
    vec3 normal = vec3(0.0, 0.0, 1.0);
    float edgedistance = a_decimals_ed.y;

    vec2 display_size_a = vec2((pattern_br_a.x - pattern_tl_a.x) / pixelRatio, (pattern_br_a.y - pattern_tl_a.y) / pixelRatio);
    vec2 display_size_b = vec2((pattern_br_b.x - pattern_tl_b.x) / pixelRatio, (pattern_br_b.y - pattern_tl_b.y) / pixelRatio);

    base = max(0.0, base);
    height = max(0.0, height);

    float t = 1.0;
    float z = height;
    gl_Position = projectTileFor3D(a_pos, z);

    // extrusion top or side
    vec2 pos = normal.z == 1.0 ? a_pos : vec2(edgedistance, z * u_height_factor);

    v_pos_a = get_pattern_pos(u_pixel_coord_upper, u_pixel_coord_lower, fromScale * display_size_a, tileRatio, pos);
    v_pos_b = get_pattern_pos(u_pixel_coord_upper, u_pixel_coord_lower, toScale * display_size_b, tileRatio, pos);

    v_lighting = vec4(0.0, 0.0, 0.0, 1.0);
    float directional = clamp(dot(normal, u_lightpos), 0.0, 1.0);
    directional = mix((1.0 - u_lightintensity), max((0.5 + u_lightintensity), 1.0), directional);

    if (normal.z == 0.0) {
        // This avoids another branching statement, but multiplies by a constant of 0.84 if no vertical gradient,
        // and otherwise calculates the gradient based on base + height
        float fMin = mix(0.7, 0.98, 1.0 - u_lightintensity);
        float factor = clamp((t + base) * pow(height / 150.0, 0.5), fMin, 1.0);
        directional *= (1.0 - u_vertical_gradient) + (u_vertical_gradient * factor);
    }

    v_lighting.rgb += clamp(directional * u_lightcolor, mix(vec3(0.0), vec3(0.3), 1.0 - u_lightcolor), vec3(1.0));
    v_lighting *= u_opacity;
}
",
    fragment: r"#ifdef GL_ES
precision highp float;
#endif

in vec2 v_pos_a;
in vec2 v_pos_b;
in vec4 v_lighting;

layout (std140) uniform FillExtrusionTilePropsUBO {
    highp vec4 u_pattern_from;
    highp vec4 u_pattern_to;
    highp vec2 u_texsize;
    lowp float tileprops_pad1;
    lowp float tileprops_pad2;
};

layout (std140) uniform FillExtrusionPropsUBO {
    highp vec4 u_color;
    highp vec3 u_lightcolor;
    lowp float props_pad1;
    highp vec3 u_lightpos;
    highp float u_base;
    highp float u_height;
    highp float u_lightintensity;
    highp float u_vertical_gradient;
    highp float u_opacity;
    highp float u_fade;
    highp float u_from_scale;
    highp float u_to_scale;
    lowp float props_pad2;
};

uniform sampler2D u_image;

#ifndef HAS_UNIFORM_u_base
in highp float base;
#endif
#ifndef HAS_UNIFORM_u_height
in highp float height;
#endif
#ifndef HAS_UNIFORM_u_pattern_from
flat in highp vec4 pattern_from;
#endif
#ifndef HAS_UNIFORM_u_pattern_to
flat in highp vec4 pattern_to;
#endif

void main() {
    #ifdef HAS_UNIFORM_u_base
highp float base = u_base;
#endif
    #ifdef HAS_UNIFORM_u_height
highp float height = u_height;
#endif
    #ifdef HAS_UNIFORM_u_pattern_from
highp vec4 pattern_from = u_pattern_from;
#endif
    #ifdef HAS_UNIFORM_u_pattern_to
highp vec4 pattern_to = u_pattern_to;
#endif

    vec2 pattern_tl_a = pattern_from.xy;
    vec2 pattern_br_a = pattern_from.zw;
    vec2 pattern_tl_b = pattern_to.xy;
    vec2 pattern_br_b = pattern_to.zw;

    // A crossfade at 0 or 1 shows one pattern, so skip the other.
    vec4 color1 = vec4(0.0);
    if (u_fade < 1.0) {
        vec2 imagecoord = mod(v_pos_a, 1.0);
        highp vec2 pos = atlas_pos(pattern_tl_a / u_texsize, (pattern_br_a - pattern_tl_a) / u_texsize * imagecoord);
        color1 = texture(u_image, pos);
    }
    vec4 color2 = vec4(0.0);
    if (u_fade > 0.0) {
        vec2 imagecoord_b = mod(v_pos_b, 1.0);
        highp vec2 pos2 = atlas_pos(pattern_tl_b / u_texsize, (pattern_br_b - pattern_tl_b) / u_texsize * imagecoord_b);
        color2 = texture(u_image, pos2);
    }

    vec4 mixedColor = mix(color1, color2, u_fade);

    fragColor = mixedColor * v_lighting;

#ifdef OVERDRAW_INSPECTOR
    fragColor = vec4(1.0);
#endif
}
",
};

const FILL_EXTRUSION_SHADER: Glsl = Glsl {
    vertex: r"invariant gl_Position;

layout (location = 0) in vec2 a_pos;
layout (location = 1) in vec2 a_decimals_ed;
out vec4 v_color;

layout (std140) uniform FillExtrusionDrawableUBO {
    highp mat4 u_matrix;
    highp vec2 u_pixel_coord_upper;
    highp vec2 u_pixel_coord_lower;
    highp float u_height_factor;
    highp float u_tile_ratio;
    // Interpolations
    highp float u_base_t;
    highp float u_height_t;
    highp float u_color_t;
    highp float u_pattern_from_t;
    highp float u_pattern_to_t;
    lowp float drawable_pad1;
};

layout (std140) uniform FillExtrusionTilePropsUBO {
    highp vec4 u_pattern_from;
    highp vec4 u_pattern_to;
    highp vec2 u_texsize;
    lowp float tileprops_pad1;
    lowp float tileprops_pad2;
};

layout (std140) uniform FillExtrusionPropsUBO {
    highp vec4 u_color;
    highp vec3 u_lightcolor;
    lowp float props_pad1;
    highp vec3 u_lightpos;
    highp float u_base;
    highp float u_height;
    highp float u_lightintensity;
    highp float u_vertical_gradient;
    highp float u_opacity;
    highp float u_fade;
    highp float u_from_scale;
    highp float u_to_scale;
    lowp float props_pad2;
};

#ifndef HAS_UNIFORM_u_base
layout (location = 3) in highp vec2 a_base;
#endif
#ifndef HAS_UNIFORM_u_height
layout (location = 4) in highp vec2 a_height;
#endif
#ifndef HAS_UNIFORM_u_color
layout (location = 5) in highp vec4 a_color;
#endif

void main() {
    #ifndef HAS_UNIFORM_u_base
highp float base = unpack_mix_vec2(a_base, u_base_t);
#else
highp float base = u_base;
#endif
    #ifndef HAS_UNIFORM_u_height
highp float height = unpack_mix_vec2(a_height, u_height_t);
#else
highp float height = u_height;
#endif
    #ifndef HAS_UNIFORM_u_color
highp vec4 color = unpack_mix_color(a_color, u_color_t);
#else
highp vec4 color = u_color;
#endif

    // A roof vertex: the walls are the instanced shader's.
    vec3 normal = vec3(0.0, 0.0, 1.0);

    base = max(0.0, base);
    height = max(0.0, height);

    float t = 1.0;
    float z = height;
    gl_Position = projectTileFor3D(a_pos, z);

    // Relative luminance (how dark/bright is the surface color?)
    float colorvalue = color.r * 0.2126 + color.g * 0.7152 + color.b * 0.0722;

    v_color = vec4(0.0, 0.0, 0.0, 1.0);

    // Add slight ambient lighting so no extrusions are totally black
    vec4 ambientlight = vec4(0.03, 0.03, 0.03, 1.0);
    color += ambientlight;

    // Calculate cos(theta), where theta is the angle between surface normal and diffuse light ray
    float directional = clamp(dot(normal, u_lightpos), 0.0, 1.0);

    // Adjust directional so that
    // the range of values for highlight/shading is narrower
    // with lower light intensity
    // and with lighter/brighter surface colors
    directional = mix((1.0 - u_lightintensity), max((1.0 - colorvalue + u_lightintensity), 1.0), directional);

    // Add gradient along z axis of side surfaces
    if (normal.z == 0.0) {
        // This avoids another branching statement, but multiplies by a constant of 0.84 if no vertical gradient,
        // and otherwise calculates the gradient based on base + height
        float fMin = mix(0.7, 0.98, 1.0 - u_lightintensity);
        float factor = clamp((t + base) * pow(height / 150.0, 0.5), fMin, 1.0);
        directional *= (1.0 - u_vertical_gradient) + (u_vertical_gradient * factor);
    }

    // Assign final color based on surface + ambient light color, diffuse light directional, and light color
    // with lower bounds adjusted to hue of light
    // so that shading is tinted with the complementary (opposite) color to the light color
    v_color.r += clamp(color.r * directional * u_lightcolor.r, mix(0.0, 0.3, 1.0 - u_lightcolor.r), 1.0);
    v_color.g += clamp(color.g * directional * u_lightcolor.g, mix(0.0, 0.3, 1.0 - u_lightcolor.g), 1.0);
    v_color.b += clamp(color.b * directional * u_lightcolor.b, mix(0.0, 0.3, 1.0 - u_lightcolor.b), 1.0);
    v_color *= u_opacity;
}
",
    fragment: r"in vec4 v_color;

void main() {
    fragColor = v_color;

#ifdef OVERDRAW_INSPECTOR
    fragColor = vec4(1.0);
#endif
}
",
};
