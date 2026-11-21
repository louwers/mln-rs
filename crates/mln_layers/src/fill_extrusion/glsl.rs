//! The programs of the `fill-extrusion` layer type in GLSL, which `mln_glsl` parses for `mln_vulkan` and `mln_d3d12`.

use mln_gfx::gfx_types::AttributeDataType::{Float2, Float4, Short2, UShort2, UShort4};
use mln_gfx::shaders::program::Program;
use mln_gfx::shaders::shader_defines::{
    ID_FILL_EXTRUSION_BASE_VERTEX_ATTRIBUTE, ID_FILL_EXTRUSION_COLOR_VERTEX_ATTRIBUTE,
    ID_FILL_EXTRUSION_DECIMALS_ED_ATTRIBUTE, ID_FILL_EXTRUSION_HEIGHT_VERTEX_ATTRIBUTE,
    ID_FILL_EXTRUSION_IMAGE_TEXTURE, ID_FILL_EXTRUSION_NEXT_DECIMALS_ED_ATTRIBUTE,
    ID_FILL_EXTRUSION_NEXT_OUTLINE_POS_ATTRIBUTE, ID_FILL_EXTRUSION_OUTLINE_POS_ATTRIBUTE,
    ID_FILL_EXTRUSION_PATTERN_FROM_VERTEX_ATTRIBUTE, ID_FILL_EXTRUSION_PATTERN_TO_VERTEX_ATTRIBUTE,
    ID_FILL_EXTRUSION_POS_VERTEX_ATTRIBUTE,
};
use mln_gfx::shaders::source::glsl::{Attribute, Glsl, Source, Texture};

use super::programs;

// An instance of the instanced fill extrusion shaders is an outline vertex
// and the vertex after it.
const FILL_EXTRUSION_ATTRIBUTES: &[Attribute] = &[
    Attribute::new(0, Short2, ID_FILL_EXTRUSION_POS_VERTEX_ATTRIBUTE),
    Attribute::new(1, UShort2, ID_FILL_EXTRUSION_DECIMALS_ED_ATTRIBUTE),
    // Data driven
    Attribute::new(2, Float4, ID_FILL_EXTRUSION_COLOR_VERTEX_ATTRIBUTE),
    Attribute::new(3, Float2, ID_FILL_EXTRUSION_BASE_VERTEX_ATTRIBUTE),
    Attribute::new(4, Float2, ID_FILL_EXTRUSION_HEIGHT_VERTEX_ATTRIBUTE),
];
const FILL_EXTRUSION_INSTANCED_ATTRIBUTES: &[Attribute] =
    &[Attribute::new(0, Short2, ID_FILL_EXTRUSION_POS_VERTEX_ATTRIBUTE)];
const FILL_EXTRUSION_INSTANCED_INSTANCE_ATTRIBUTES: &[Attribute] = &[
    Attribute::new(1, Short2, ID_FILL_EXTRUSION_OUTLINE_POS_ATTRIBUTE),
    Attribute::new(2, UShort2, ID_FILL_EXTRUSION_DECIMALS_ED_ATTRIBUTE),
    // Data driven
    Attribute::new(3, Float4, ID_FILL_EXTRUSION_COLOR_VERTEX_ATTRIBUTE),
    Attribute::new(4, Float2, ID_FILL_EXTRUSION_BASE_VERTEX_ATTRIBUTE),
    Attribute::new(5, Float2, ID_FILL_EXTRUSION_HEIGHT_VERTEX_ATTRIBUTE),
    // The next outline vertex
    Attribute::new(6, Short2, ID_FILL_EXTRUSION_NEXT_OUTLINE_POS_ATTRIBUTE),
    Attribute::new(7, UShort2, ID_FILL_EXTRUSION_NEXT_DECIMALS_ED_ATTRIBUTE),
];
const FILL_EXTRUSION_PATTERN_ATTRIBUTES: &[Attribute] = &[
    Attribute::new(0, Short2, ID_FILL_EXTRUSION_POS_VERTEX_ATTRIBUTE),
    Attribute::new(1, UShort2, ID_FILL_EXTRUSION_DECIMALS_ED_ATTRIBUTE),
    // Data driven
    Attribute::new(2, Float2, ID_FILL_EXTRUSION_BASE_VERTEX_ATTRIBUTE),
    Attribute::new(3, Float2, ID_FILL_EXTRUSION_HEIGHT_VERTEX_ATTRIBUTE),
    Attribute::new(4, UShort4, ID_FILL_EXTRUSION_PATTERN_FROM_VERTEX_ATTRIBUTE),
    Attribute::new(5, UShort4, ID_FILL_EXTRUSION_PATTERN_TO_VERTEX_ATTRIBUTE),
];
const FILL_EXTRUSION_PATTERN_INSTANCED_INSTANCE_ATTRIBUTES: &[Attribute] = &[
    Attribute::new(1, Short2, ID_FILL_EXTRUSION_OUTLINE_POS_ATTRIBUTE),
    Attribute::new(2, UShort2, ID_FILL_EXTRUSION_DECIMALS_ED_ATTRIBUTE),
    // Data driven
    Attribute::new(3, Float2, ID_FILL_EXTRUSION_BASE_VERTEX_ATTRIBUTE),
    Attribute::new(4, Float2, ID_FILL_EXTRUSION_HEIGHT_VERTEX_ATTRIBUTE),
    Attribute::new(5, UShort4, ID_FILL_EXTRUSION_PATTERN_FROM_VERTEX_ATTRIBUTE),
    Attribute::new(6, UShort4, ID_FILL_EXTRUSION_PATTERN_TO_VERTEX_ATTRIBUTE),
    // The next outline vertex
    Attribute::new(7, Short2, ID_FILL_EXTRUSION_NEXT_OUTLINE_POS_ATTRIBUTE),
    Attribute::new(8, UShort2, ID_FILL_EXTRUSION_NEXT_DECIMALS_ED_ATTRIBUTE),
];
// The wall shaders run the instanced ones' GLSL with every attribute per
// vertex, for walls whose quads are in the vertex buffer.
const FILL_EXTRUSION_WALL_ATTRIBUTES: &[Attribute] = &[
    Attribute::new(0, Short2, ID_FILL_EXTRUSION_POS_VERTEX_ATTRIBUTE),
    Attribute::new(1, Short2, ID_FILL_EXTRUSION_OUTLINE_POS_ATTRIBUTE),
    Attribute::new(2, UShort2, ID_FILL_EXTRUSION_DECIMALS_ED_ATTRIBUTE),
    // Data driven
    Attribute::new(3, Float4, ID_FILL_EXTRUSION_COLOR_VERTEX_ATTRIBUTE),
    Attribute::new(4, Float2, ID_FILL_EXTRUSION_BASE_VERTEX_ATTRIBUTE),
    Attribute::new(5, Float2, ID_FILL_EXTRUSION_HEIGHT_VERTEX_ATTRIBUTE),
    // The next outline vertex
    Attribute::new(6, Short2, ID_FILL_EXTRUSION_NEXT_OUTLINE_POS_ATTRIBUTE),
    Attribute::new(7, UShort2, ID_FILL_EXTRUSION_NEXT_DECIMALS_ED_ATTRIBUTE),
];
const FILL_EXTRUSION_PATTERN_WALL_ATTRIBUTES: &[Attribute] = &[
    Attribute::new(0, Short2, ID_FILL_EXTRUSION_POS_VERTEX_ATTRIBUTE),
    Attribute::new(1, Short2, ID_FILL_EXTRUSION_OUTLINE_POS_ATTRIBUTE),
    Attribute::new(2, UShort2, ID_FILL_EXTRUSION_DECIMALS_ED_ATTRIBUTE),
    // Data driven
    Attribute::new(3, Float2, ID_FILL_EXTRUSION_BASE_VERTEX_ATTRIBUTE),
    Attribute::new(4, Float2, ID_FILL_EXTRUSION_HEIGHT_VERTEX_ATTRIBUTE),
    Attribute::new(5, UShort4, ID_FILL_EXTRUSION_PATTERN_FROM_VERTEX_ATTRIBUTE),
    Attribute::new(6, UShort4, ID_FILL_EXTRUSION_PATTERN_TO_VERTEX_ATTRIBUTE),
    // The next outline vertex
    Attribute::new(7, Short2, ID_FILL_EXTRUSION_NEXT_OUTLINE_POS_ATTRIBUTE),
    Attribute::new(8, UShort2, ID_FILL_EXTRUSION_NEXT_DECIMALS_ED_ATTRIBUTE),
];
const FILL_EXTRUSION_PATTERN_TEXTURES: &[Texture] = &[Texture::new(0, ID_FILL_EXTRUSION_IMAGE_TEXTURE)];
static FILL_EXTRUSION: Source = Source {
    glsl: &FILL_EXTRUSION_SHADER,
    attributes: FILL_EXTRUSION_ATTRIBUTES,
    instance_attributes: &[],
    textures: &[],
};
static FILL_EXTRUSION_INSTANCED: Source = Source {
    glsl: &FILL_EXTRUSION_INSTANCED_SHADER,
    attributes: FILL_EXTRUSION_INSTANCED_ATTRIBUTES,
    instance_attributes: FILL_EXTRUSION_INSTANCED_INSTANCE_ATTRIBUTES,
    textures: &[],
};
static FILL_EXTRUSION_PATTERN: Source = Source {
    glsl: &FILL_EXTRUSION_PATTERN_SHADER,
    attributes: FILL_EXTRUSION_PATTERN_ATTRIBUTES,
    instance_attributes: &[],
    textures: FILL_EXTRUSION_PATTERN_TEXTURES,
};
static FILL_EXTRUSION_PATTERN_INSTANCED: Source = Source {
    glsl: &FILL_EXTRUSION_PATTERN_INSTANCED_SHADER,
    attributes: FILL_EXTRUSION_INSTANCED_ATTRIBUTES,
    instance_attributes: FILL_EXTRUSION_PATTERN_INSTANCED_INSTANCE_ATTRIBUTES,
    textures: FILL_EXTRUSION_PATTERN_TEXTURES,
};
// The walls whose quads are in the vertex buffer run the instanced walls'
// GLSL with every attribute per vertex.
static FILL_EXTRUSION_WALL: Source = Source {
    glsl: &FILL_EXTRUSION_INSTANCED_SHADER,
    attributes: FILL_EXTRUSION_WALL_ATTRIBUTES,
    instance_attributes: &[],
    textures: &[],
};
static FILL_EXTRUSION_PATTERN_WALL: Source = Source {
    glsl: &FILL_EXTRUSION_PATTERN_INSTANCED_SHADER,
    attributes: FILL_EXTRUSION_PATTERN_WALL_ATTRIBUTES,
    instance_attributes: &[],
    textures: FILL_EXTRUSION_PATTERN_TEXTURES,
};
// The pulled walls and outlines have no vertex inputs: they read the
// vertices from storage buffers.
static FILL_EXTRUSION_PULLED: Source =
    Source { glsl: &FILL_EXTRUSION_PULLED_SHADER, attributes: &[], instance_attributes: &[], textures: &[] };

/// The source of each of the type's programs.
pub(super) static SOURCES: &[(&Program, &Source)] = &[
    (&programs::FILL_EXTRUSION, &FILL_EXTRUSION),
    (&programs::FILL_EXTRUSION_INSTANCED, &FILL_EXTRUSION_INSTANCED),
    (&programs::FILL_EXTRUSION_PATTERN, &FILL_EXTRUSION_PATTERN),
    (&programs::FILL_EXTRUSION_PATTERN_INSTANCED, &FILL_EXTRUSION_PATTERN_INSTANCED),
    (&programs::FILL_EXTRUSION_WALL, &FILL_EXTRUSION_WALL),
    (&programs::FILL_EXTRUSION_PATTERN_WALL, &FILL_EXTRUSION_PATTERN_WALL),
    (&programs::FILL_EXTRUSION_PULLED, &FILL_EXTRUSION_PULLED),
];

const FILL_EXTRUSION_SHADER_PRELUDE: &str = r"

#define idFillExtrusionDrawableUBO              idDrawableReservedVertexOnlyUBO
#define idFillExtrusionTilePropsUBO             drawableReservedUBOCount
#define idFillExtrusionPropsUBO                 layerUBOStartId + 1
#define idFillExtrusionWallVertices             drawableUBOStartId + 5
#define idFillExtrusionWallPaint                drawableUBOStartId + 6

// The normal at the end of a fill extrusion wall from its outline vertex's
// packed decimals_ed.x: the one the walls there share, whose angle from -pi
// its bits 2 to 15 hold where bit 1 is set, or else `own`, the wall's.
vec2 wall_normal(const uint bits, const vec2 own) {
    if ((bits & 2u) == 0u) {
        return own;
    }
    const float angle = float(bits >> 2u) * (6.283185307179586 / 16384.0) - 3.141592653589793;
    return vec2(cos(angle), sin(angle));
}

";

const FILL_EXTRUSION_SHADER: Glsl = Glsl {
    prelude: FILL_EXTRUSION_SHADER_PRELUDE,
    vertex: r"
invariant gl_Position;

layout(location = 0) in ivec2 in_position;
layout(location = 1) in uvec2 in_decimals_ed;

#if !defined(HAS_UNIFORM_u_color)
layout(location = 2) in vec4 in_color;
#endif

#if !defined(HAS_UNIFORM_u_base)
layout(location = 3) in vec2 in_base;
#endif

#if !defined(HAS_UNIFORM_u_height)
layout(location = 4) in vec2 in_height;
#endif

layout(push_constant) uniform Constants {
    int ubo_index;
} constant;

struct FillExtrusionDrawableUBO {
    mat4 matrix;
    vec2 pixel_coord_upper;
    vec2 pixel_coord_lower;
    float height_factor;
    float tile_ratio;
    // Interpolations
    float base_t;
    float height_t;
    float color_t;
    float pattern_from_t;
    float pattern_to_t;
    float pad1;
};

layout(std140, set = LAYER_SET_INDEX, binding = idFillExtrusionDrawableUBO) readonly buffer FillExtrusionDrawableUBOVector {
    FillExtrusionDrawableUBO drawable_ubo[];
} drawableVector;

layout(std140, set = LAYER_SET_INDEX, binding = idProjectionUBO) readonly buffer ProjectionUBOVector {
    ProjectionUBO projection_ubo[];
} projectionVector;

layout(set = LAYER_SET_INDEX, binding = idFillExtrusionPropsUBO) uniform FillExtrusionPropsUBO {
    vec4 color;
    vec4 light_color_pad;
    vec4 light_position_base;
    float height;
    float light_intensity;
    float vertical_gradient;
    float opacity;
    float fade;
    float from_scale;
    float to_scale;
    float pad2;
} props;

layout(location = 0) out vec4 frag_color;

void main() {
    const FillExtrusionDrawableUBO drawable = drawableVector.drawable_ubo[constant.ubo_index];

#if defined(HAS_UNIFORM_u_base)
    const float base = max(props.light_position_base.w, 0.0);
#else
    const float base = max(unpack_mix_float(in_base, drawable.base_t), 0.0);
#endif

#if defined(HAS_UNIFORM_u_height)
    const float height = max(props.height, 0.0);
#else
    const float height = max(unpack_mix_float(in_height, drawable.height_t), 0.0);
#endif

#if defined(HAS_UNIFORM_u_color)
    vec4 color = props.color;
#else
    vec4 color = unpack_mix_color(in_color, drawable.color_t);
#endif

    const vec3 normal = vec3(0.0, 0.0, 1.0);
    const float t = 1.0;
    const float z = t > 0.0 ? height : base;
    gl_Position = projectTileFor3D(vec2(in_position), z, projectionVector.projection_ubo[constant.ubo_index]);

#if defined(OVERDRAW_INSPECTOR)
    frag_color = vec4(1.0);
    return;
#endif

    // Relative luminance (how dark/bright is the surface color?)
    const float luminance = color.r * 0.2126 + color.g * 0.7152 + color.b * 0.0722;

    vec4 vcolor = vec4(0.0, 0.0, 0.0, 1.0);

    // Add slight ambient lighting so no extrusions are totally black
    color += vec4(0.03, 0.03, 0.03, 1.0);

    // Calculate cos(theta), where theta is the angle between surface normal and diffuse light ray
    const float directionalFraction = clamp(dot(normal, props.light_position_base.xyz), 0.0, 1.0);

    // Adjust directional so that the range of values for highlight/shading is
    // narrower with lower light intensity and with lighter/brighter surface colors
    const float minDirectional = 1.0 - props.light_intensity;
    const float maxDirectional = max(1.0 - luminance + props.light_intensity, 1.0);
    float directional = mix(minDirectional, maxDirectional, directionalFraction);

    // Add gradient along z axis of side surfaces
    if (normal.z == 0.0) {
        // This avoids another branching statement, but multiplies by a constant of 0.84 if no
        // vertical gradient, and otherwise calculates the gradient based on base + height
        // TODO: If we're optimizing to the level of avoiding branches, we should pre-compute
        //       the square root when height is a uniform.
        const float fMin = mix(0.7, 0.98, 1.0 - props.light_intensity);
        const float factor = clamp((t + base) * pow(height / 150.0, 0.5), fMin, 1.0);
        directional *= (1.0 - props.vertical_gradient) + (props.vertical_gradient * factor);
    }

    // Assign final color based on surface + ambient light color, diffuse light directional,
    // and light color with lower bounds adjusted to hue of light so that shading is tinted
    // with the complementary (opposite) color to the light color
    const vec3 light_color = props.light_color_pad.rgb;
    const vec3 minLight = mix(vec3(0.0), vec3(0.3), 1.0 - light_color.rgb);
    vcolor += vec4(clamp(color.rgb * directional * light_color.rgb, minLight, vec3(1.0)), 0.0);

    frag_color = vcolor * props.opacity;
}
",
    fragment: r"

layout(location = 0) in vec4 frag_color;
layout(location = 0) out vec4 out_color;

void main() {
    out_color = frag_color;
}
",
};

/// The declarations of the fill extrusion walls' vertex stages.
macro_rules! fill_extrusion_wall_declarations {
    () => {
        r"
layout(push_constant) uniform Constants {
    int ubo_index;
} constant;

struct FillExtrusionDrawableUBO {
    mat4 matrix;
    vec2 pixel_coord_upper;
    vec2 pixel_coord_lower;
    float height_factor;
    float tile_ratio;
    // Interpolations
    float base_t;
    float height_t;
    float color_t;
    float pattern_from_t;
    float pattern_to_t;
    // Where the pulled walls find a vertex's paint values.
    uint paint_layout;
};

layout(std140, set = LAYER_SET_INDEX, binding = idFillExtrusionDrawableUBO) readonly buffer FillExtrusionDrawableUBOVector {
    FillExtrusionDrawableUBO drawable_ubo[];
} drawableVector;

layout(std140, set = LAYER_SET_INDEX, binding = idProjectionUBO) readonly buffer ProjectionUBOVector {
    ProjectionUBO projection_ubo[];
} projectionVector;

layout(set = LAYER_SET_INDEX, binding = idFillExtrusionPropsUBO) uniform FillExtrusionPropsUBO {
    vec4 color;
    vec4 light_color_pad;
    vec4 light_position_base;
    float height;
    float light_intensity;
    float vertical_gradient;
    float opacity;
    float fade;
    float from_scale;
    float to_scale;
    float pad2;
} props;

layout(location = 0) out vec4 frag_color;

"
    };
}

/// The body of the fill extrusion walls' vertex stages, which reads its
/// wall's corner, outline vertices and paint values from `in_position`,
/// `in_outline_pos`, `in_decimals_ed`, `in_next_outline_pos`,
/// `in_next_decimals_ed`, `in_color`, `in_base` and `in_height`.
macro_rules! fill_extrusion_wall_main {
    () => {
        r"

    const vec2 instancePos = vec2(in_outline_pos);
    const vec2 instanceDecimalsEd = vec2(in_decimals_ed);

    const FillExtrusionDrawableUBO drawable = drawableVector.drawable_ubo[constant.ubo_index];

#if defined(HAS_UNIFORM_u_base)
    const float base = max(props.light_position_base.w, 0.0);
#else
    const float base = max(unpack_mix_float(in_base, drawable.base_t), 0.0);
#endif

#if defined(HAS_UNIFORM_u_height)
    const float height = max(props.height, 0.0);
#else
    const float height = max(unpack_mix_float(in_height, drawable.height_t), 0.0);
#endif

#if defined(HAS_UNIFORM_u_color)
    vec4 color = props.color;
#else
    vec4 color = unpack_mix_color(in_color, drawable.color_t);
#endif
    bool isDiscarded = mod(instanceDecimalsEd.x, 2.0) > 0.0;
    if (isDiscarded) {
        gl_Position = vec4(0.0);
        frag_color = vec4(0.0);
        return;
    }

    const vec2 nextInstancePos = vec2(in_next_outline_pos);
    const vec2 nextInstanceDecimalsEd = vec2(in_next_decimals_ed);

    const vec2 p1 = instancePos;
    const vec2 p2 = nextInstancePos;
    const vec2 edgevector = normalize(p2 - p1);
    const vec2 own = vec2(-edgevector.y, edgevector.x);

    const vec3 normal = vec3(in_position.x == 0.0 ? wall_normal(in_decimals_ed.x, own) : wall_normal(in_next_decimals_ed.x, own), 0.0);
    const float t = float(in_position.y);
    const float z = t > 0.0 ? height : base;

    gl_Position = projectTileFor3D(in_position.x == 0.0 ? p1 : p2, z, projectionVector.projection_ubo[constant.ubo_index]);

#if defined(OVERDRAW_INSPECTOR)
    frag_color = vec4(1.0);
    return;
#endif

    // Relative luminance (how dark/bright is the surface color?)
    const float luminance = color.r * 0.2126 + color.g * 0.7152 + color.b * 0.0722;

    vec4 vcolor = vec4(0.0, 0.0, 0.0, 1.0);

    // Add slight ambient lighting so no extrusions are totally black
    color += vec4(0.03, 0.03, 0.03, 1.0);

    // Calculate cos(theta), where theta is the angle between surface normal and diffuse light ray
    const float directionalFraction = clamp(dot(normal, props.light_position_base.xyz), 0.0, 1.0);

    // Adjust directional so that the range of values for highlight/shading is
    // narrower with lower light intensity and with lighter/brighter surface colors
    const float minDirectional = 1.0 - props.light_intensity;
    const float maxDirectional = max(1.0 - luminance + props.light_intensity, 1.0);
    float directional = mix(minDirectional, maxDirectional, directionalFraction);

    // Add gradient along z axis of side surfaces
    if (normal.z == 0.0) {
        // This avoids another branching statement, but multiplies by a constant of 0.84 if no
        // vertical gradient, and otherwise calculates the gradient based on base + height
        // TODO: If we're optimizing to the level of avoiding branches, we should pre-compute
        //       the square root when height is a uniform.
        const float fMin = mix(0.7, 0.98, 1.0 - props.light_intensity);
        const float factor = clamp((t + base) * pow(height / 150.0, 0.5), fMin, 1.0);
        directional *= (1.0 - props.vertical_gradient) + (props.vertical_gradient * factor);
    }

    // Assign final color based on surface + ambient light color, diffuse light directional,
    // and light color with lower bounds adjusted to hue of light so that shading is tinted
    // with the complementary (opposite) color to the light color
    const vec3 light_color = props.light_color_pad.rgb;
    const vec3 minLight = mix(vec3(0.0), vec3(0.3), 1.0 - light_color.rgb);
    vcolor += vec4(clamp(color.rgb * directional * light_color.rgb, minLight, vec3(1.0)), 0.0);

    frag_color = vcolor * props.opacity;
"
    };
}

const FILL_EXTRUSION_INSTANCED_SHADER: Glsl = Glsl {
    prelude: FILL_EXTRUSION_SHADER_PRELUDE,
    vertex: concat!(
        r"
invariant gl_Position;

layout(location = 0) in ivec2 in_position;

// An instance: an outline vertex, and the one after it, which the same buffer
// bound a vertex further on provides.
layout(location = 1) in ivec2 in_outline_pos;
layout(location = 2) in uvec2 in_decimals_ed;
layout(location = 6) in ivec2 in_next_outline_pos;
layout(location = 7) in uvec2 in_next_decimals_ed;

#if !defined(HAS_UNIFORM_u_color)
layout(location = 3) in vec4 in_color;
#endif

#if !defined(HAS_UNIFORM_u_base)
layout(location = 4) in vec2 in_base;
#endif

#if !defined(HAS_UNIFORM_u_height)
layout(location = 5) in vec2 in_height;
#endif

",
        fill_extrusion_wall_declarations!(),
        r"
void main() {
",
        fill_extrusion_wall_main!(),
        r"}
"
    ),
    fragment: r"

layout(location = 0) in vec4 frag_color;
layout(location = 0) out vec4 out_color;

void main() {
    out_color = frag_color;
}
",
};

/// Port-only: the walls of the instanced walls as quads without vertex
/// inputs. A vertex reads its wall's outline vertices and paint values from
/// storage buffers, by its index.
const FILL_EXTRUSION_PULLED_SHADER: Glsl = Glsl {
    prelude: FILL_EXTRUSION_SHADER_PRELUDE,
    vertex: concat!(
        r"
invariant gl_Position;

",
        fill_extrusion_wall_declarations!(),
        r"
// The bucket's vertices, two words each: the position's x and y in the
// halves of the first, decimals_ed's in those of the second.
layout(std430, set = DRAWABLE_UBO_SET_INDEX, binding = idFillExtrusionWallVertices) readonly buffer FillExtrusionWallVertices {
    uint vertices[];
} wallVertices;

#if !defined(HAS_UNIFORM_u_color) || !defined(HAS_UNIFORM_u_base) || !defined(HAS_UNIFORM_u_height)
// The bucket's paint values, interleaved as the roofs read them.
layout(std430, set = DRAWABLE_UBO_SET_INDEX, binding = idFillExtrusionWallPaint) readonly buffer FillExtrusionWallPaint {
    float paint[];
} wallPaint;
#endif

void main() {
    // Four vertices a wall, in the order of the static quad's corners.
    const uint wall = uint(gl_VertexIndex) / 4u;
    const uint corner = uint(gl_VertexIndex) % 4u;
    const uint pos = wallVertices.vertices[2u * wall];
    const uint decimals_ed = wallVertices.vertices[2u * wall + 1u];
    const uint next_pos = wallVertices.vertices[2u * wall + 2u];
    const uint next_decimals_ed = wallVertices.vertices[2u * wall + 3u];

    // What the instanced walls read as vertex inputs.
    const ivec2 in_position = ivec2(corner < 2u ? 1 : 0, int(corner & 1u));
    const ivec2 in_outline_pos = ivec2(int(pos << 16u) >> 16, int(pos) >> 16);
    const uvec2 in_decimals_ed = uvec2(decimals_ed & 0xffffu, decimals_ed >> 16u);
    const ivec2 in_next_outline_pos = ivec2(int(next_pos << 16u) >> 16, int(next_pos) >> 16);
    const uvec2 in_next_decimals_ed = uvec2(next_decimals_ed & 0xffffu, next_decimals_ed >> 16u);

    // The wall's paint values are its outline vertex's.
    const uint paint_layout = drawableVector.drawable_ubo[constant.ubo_index].paint_layout;
    const uint paint = wall * (paint_layout & 0xffu);
#if !defined(HAS_UNIFORM_u_base)
    const uint base_at = paint + ((paint_layout >> 8u) & 0xffu);
    const vec2 in_base = vec2(wallPaint.paint[base_at], wallPaint.paint[base_at + 1u]);
#endif
#if !defined(HAS_UNIFORM_u_height)
    const uint height_at = paint + ((paint_layout >> 16u) & 0xffu);
    const vec2 in_height = vec2(wallPaint.paint[height_at], wallPaint.paint[height_at + 1u]);
#endif
#if !defined(HAS_UNIFORM_u_color)
    const uint color_at = paint + (paint_layout >> 24u);
    const vec4 in_color = vec4(wallPaint.paint[color_at], wallPaint.paint[color_at + 1u],
                               wallPaint.paint[color_at + 2u], wallPaint.paint[color_at + 3u]);
#endif
",
        fill_extrusion_wall_main!(),
        r"}
"
    ),
    fragment: FILL_EXTRUSION_INSTANCED_SHADER.fragment,
};

const FILL_EXTRUSION_PATTERN_SHADER: Glsl = Glsl {
    prelude: FILL_EXTRUSION_SHADER_PRELUDE,
    vertex: r"
invariant gl_Position;

layout(location = 0) in ivec2 in_position;
layout(location = 1) in uvec2 in_decimals_ed;

#if !defined(HAS_UNIFORM_u_base)
layout(location = 2) in vec2 in_base;
#endif

#if !defined(HAS_UNIFORM_u_height)
layout(location = 3) in vec2 in_height;
#endif

#if !defined(HAS_UNIFORM_u_pattern_from)
layout(location = 4) in uvec4 in_pattern_from;
#endif

#if !defined(HAS_UNIFORM_u_pattern_to)
layout(location = 5) in uvec4 in_pattern_to;
#endif

layout(push_constant) uniform Constants {
    int ubo_index;
} constant;

struct FillExtrusionDrawableUBO {
    mat4 matrix;
    vec2 pixel_coord_upper;
    vec2 pixel_coord_lower;
    float height_factor;
    float tile_ratio;
    // Interpolations
    float base_t;
    float height_t;
    float color_t;
    float pattern_from_t;
    float pattern_to_t;
    float pad1;
};

layout(std140, set = LAYER_SET_INDEX, binding = idFillExtrusionDrawableUBO) readonly buffer FillExtrusionDrawableUBOVector {
    FillExtrusionDrawableUBO drawable_ubo[];
} drawableVector;

layout(std140, set = LAYER_SET_INDEX, binding = idProjectionUBO) readonly buffer ProjectionUBOVector {
    ProjectionUBO projection_ubo[];
} projectionVector;

struct FillExtrusionTilePropsUBO {
    vec4 pattern_from;
    vec4 pattern_to;
    vec2 texsize;
    float pad1;
    float pad2;
};

layout(std140, set = LAYER_SET_INDEX, binding = idFillExtrusionTilePropsUBO) readonly buffer FillExtrusionTilePropsUBOVector {
    FillExtrusionTilePropsUBO tile_props_ubo[];
} tilePropsVector;

layout(set = LAYER_SET_INDEX, binding = idFillExtrusionPropsUBO) uniform FillExtrusionPropsUBO {
    vec4 color;
    vec4 light_color_pad;
    vec4 light_position_base;
    float height;
    float light_intensity;
    float vertical_gradient;
    float opacity;
    float fade;
    float from_scale;
    float to_scale;
    float pad2;
} props;

layout(location = 0) out vec4 frag_lighting;
layout(location = 1) out vec2 frag_pos_a;
layout(location = 2) out vec2 frag_pos_b;

#if !defined(HAS_UNIFORM_u_pattern_from)
layout(location = 3) flat out vec4 frag_pattern_from;
#endif

#if !defined(HAS_UNIFORM_u_pattern_to)
layout(location = 4) flat out vec4 frag_pattern_to;
#endif

void main() {
    const FillExtrusionDrawableUBO drawable = drawableVector.drawable_ubo[constant.ubo_index];
    const FillExtrusionTilePropsUBO tileProps = tilePropsVector.tile_props_ubo[constant.ubo_index];

#if defined(HAS_UNIFORM_u_base)
    const float base = max(props.light_position_base.w, 0.0);
#else
    const float base = max(unpack_mix_float(in_base, drawable.base_t), 0.0);
#endif

#if defined(HAS_UNIFORM_u_height)
    const float height = max(props.height, 0.0);
#else
    const float height = max(unpack_mix_float(in_height, drawable.height_t), 0.0);
#endif

    const vec3 normal = vec3(0.0, 0.0, 1.0);
    const float t = 1.0;
    const float z = t > 0.0 ? height : base;
    gl_Position = projectTileFor3D(vec2(in_position), z, projectionVector.projection_ubo[constant.ubo_index]);

#if defined(OVERDRAW_INSPECTOR)
    frag_lighting = vec4(1.0);
    frag_pos_a = vec2(0.0);
    frag_pos_b = vec2(0.0);
#if !defined(HAS_UNIFORM_u_pattern_from)
    frag_pattern_from = vec4(0.0);
#endif
#if !defined(HAS_UNIFORM_u_pattern_to)
    frag_pattern_to = vec4(0.0);
#endif
    return;
#else

#if defined(HAS_UNIFORM_u_pattern_from)
    const vec4 pattern_from = tileProps.pattern_from;
#else
    const vec4 pattern_from = in_pattern_from;
    frag_pattern_from = in_pattern_from;
#endif

#if defined(HAS_UNIFORM_u_pattern_to)
    const vec4 pattern_to = tileProps.pattern_to;
#else
    const vec4 pattern_to = in_pattern_to;
    frag_pattern_to = in_pattern_to;
#endif

    const vec2 pattern_tl_a = pattern_from.xy;
    const vec2 pattern_br_a = pattern_from.zw;
    const vec2 pattern_tl_b = pattern_to.xy;
    const vec2 pattern_br_b = pattern_to.zw;

    const float pixelRatio = paintParams.pixel_ratio;
    const float tileZoomRatio = drawable.tile_ratio;
    const float fromScale = props.from_scale;
    const float toScale = props.to_scale;

    const vec2 display_size_a = vec2((pattern_br_a.x - pattern_tl_a.x) / pixelRatio, (pattern_br_a.y - pattern_tl_a.y) / pixelRatio);
    const vec2 display_size_b = vec2((pattern_br_b.x - pattern_tl_b.x) / pixelRatio, (pattern_br_b.y - pattern_tl_b.y) / pixelRatio);

    const vec2 pos = vec2(in_position); // extrusion top

    vec4 lighting = vec4(0.0, 0.0, 0.0, 1.0);
    float directional = clamp(dot(normal, props.light_position_base.xyz), 0.0, 1.0);
    directional = mix((1.0 - props.light_intensity), max((0.5 + props.light_intensity), 1.0), directional);

    if (normal.z == 0.0) {
        // This avoids another branching statement, but multiplies by a constant of 0.84 if no
        // vertical gradient, and otherwise calculates the gradient based on base + height
        // TODO: If we're optimizing to the level of avoiding branches, we should pre-compute
        //       the square root when height is a uniform.
        const float fMin = mix(0.7, 0.98, 1.0 - props.light_intensity);
        const float factor = clamp((t + base) * pow(height / 150.0, 0.5), fMin, 1.0);
        directional *= (1.0 - props.vertical_gradient) + (props.vertical_gradient * factor);    }

    lighting.rgb += clamp(directional * props.light_color_pad.rgb, mix(vec3(0.0), vec3(0.3), 1.0 - props.light_color_pad.rgb), vec3(1.0));
    lighting *= props.opacity;

    frag_lighting = lighting;
    frag_pos_a = get_pattern_pos(drawable.pixel_coord_upper, drawable.pixel_coord_lower, fromScale * display_size_a, tileZoomRatio, pos);
    frag_pos_b = get_pattern_pos(drawable.pixel_coord_upper, drawable.pixel_coord_lower, toScale * display_size_b, tileZoomRatio, pos);

#if !defined(HAS_UNIFORM_u_pattern_from)
    frag_pattern_from = in_pattern_from;
#endif

#if !defined(HAS_UNIFORM_u_pattern_to)
    frag_pattern_to = in_pattern_to;
#endif
#endif
}
",
    fragment: r"

layout(location = 0) in vec4 frag_lighting;
layout(location = 1) in vec2 frag_pos_a;
layout(location = 2) in vec2 frag_pos_b;

#if !defined(HAS_UNIFORM_u_pattern_from)
layout(location = 3) flat in vec4 frag_pattern_from;
#endif

#if !defined(HAS_UNIFORM_u_pattern_to)
layout(location = 4) flat in vec4 frag_pattern_to;
#endif

layout(location = 0) out vec4 out_color;

layout(push_constant) uniform Constants {
    int ubo_index;
} constant;

struct FillExtrusionTilePropsUBO {
    vec4 pattern_from;
    vec4 pattern_to;
    vec2 texsize;
    float pad1;
    float pad2;
};

layout(std140, set = LAYER_SET_INDEX, binding = idFillExtrusionTilePropsUBO) readonly buffer FillExtrusionTilePropsUBOVector {
    FillExtrusionTilePropsUBO tile_props_ubo[];
} tilePropsVector;

layout(set = LAYER_SET_INDEX, binding = idFillExtrusionPropsUBO) uniform FillExtrusionPropsUBO {
    vec4 color;
    vec4 light_color_pad;
    vec4 light_position_base;
    float height;
    float light_intensity;
    float vertical_gradient;
    float opacity;
    float fade;
    float from_scale;
    float to_scale;
    float pad2;
} props;

layout(set = DRAWABLE_IMAGE_SET_INDEX, binding = 0) uniform texture2D image0_texture;
layout(set = DRAWABLE_IMAGE_SET_INDEX, binding = samplerBindingStartId) uniform sampler image0_sampler;

void main() {

#if defined(OVERDRAW_INSPECTOR)
    out_color = vec4(1.0);
    return;
#else

    const FillExtrusionTilePropsUBO tileProps = tilePropsVector.tile_props_ubo[constant.ubo_index];

#if defined(HAS_UNIFORM_u_pattern_from)
    const vec4 pattern_from = tileProps.pattern_from;
#else
    const vec4 pattern_from = frag_pattern_from;
#endif
#if defined(HAS_UNIFORM_u_pattern_to)
    const vec4 pattern_to = tileProps.pattern_to;
#else
    const vec4 pattern_to = frag_pattern_to;
#endif

    const vec2 pattern_tl_a = pattern_from.xy;
    const vec2 pattern_br_a = pattern_from.zw;
    const vec2 pattern_tl_b = pattern_to.xy;
    const vec2 pattern_br_b = pattern_to.zw;

    // A crossfade at 0 or 1 shows one pattern, so skip the other.
    vec4 color1 = vec4(0.0);
    if (props.fade < 1.0) {
        const vec2 imagecoord = mod(frag_pos_a, 1.0);
        const vec2 pos = atlas_pos(pattern_tl_a / tileProps.texsize, (pattern_br_a - pattern_tl_a) / tileProps.texsize * imagecoord);
        color1 = texture(sampler2D(image0_texture, image0_sampler), pos);
    }
    vec4 color2 = vec4(0.0);
    if (props.fade > 0.0) {
        const vec2 imagecoord_b = mod(frag_pos_b, 1.0);
        const vec2 pos2 = atlas_pos(pattern_tl_b / tileProps.texsize, (pattern_br_b - pattern_tl_b) / tileProps.texsize * imagecoord_b);
        color2 = texture(sampler2D(image0_texture, image0_sampler), pos2);
    }

    out_color = mix(color1, color2, props.fade) * frag_lighting;
#endif
}
",
};

const FILL_EXTRUSION_PATTERN_INSTANCED_SHADER: Glsl = Glsl {
    prelude: FILL_EXTRUSION_SHADER_PRELUDE,
    vertex: r"
invariant gl_Position;

layout(location = 0) in ivec2 in_position;

// An instance: an outline vertex, and the one after it, which the same buffer
// bound a vertex further on provides.
layout(location = 1) in ivec2 in_outline_pos;
layout(location = 2) in uvec2 in_decimals_ed;
layout(location = 7) in ivec2 in_next_outline_pos;
layout(location = 8) in uvec2 in_next_decimals_ed;

#if !defined(HAS_UNIFORM_u_base)
layout(location = 3) in vec2 in_base;
#endif

#if !defined(HAS_UNIFORM_u_height)
layout(location = 4) in vec2 in_height;
#endif

#if !defined(HAS_UNIFORM_u_pattern_from)
layout(location = 5) in uvec4 in_pattern_from;
#endif

#if !defined(HAS_UNIFORM_u_pattern_to)
layout(location = 6) in uvec4 in_pattern_to;
#endif

layout(push_constant) uniform Constants {
    int ubo_index;
} constant;

struct FillExtrusionDrawableUBO {
    mat4 matrix;
    vec2 pixel_coord_upper;
    vec2 pixel_coord_lower;
    float height_factor;
    float tile_ratio;
    // Interpolations
    float base_t;
    float height_t;
    float color_t;
    float pattern_from_t;
    float pattern_to_t;
    float pad1;
};

layout(std140, set = LAYER_SET_INDEX, binding = idFillExtrusionDrawableUBO) readonly buffer FillExtrusionDrawableUBOVector {
    FillExtrusionDrawableUBO drawable_ubo[];
} drawableVector;

layout(std140, set = LAYER_SET_INDEX, binding = idProjectionUBO) readonly buffer ProjectionUBOVector {
    ProjectionUBO projection_ubo[];
} projectionVector;

struct FillExtrusionTilePropsUBO {
    vec4 pattern_from;
    vec4 pattern_to;
    vec2 texsize;
    float pad1;
    float pad2;
};

layout(std140, set = LAYER_SET_INDEX, binding = idFillExtrusionTilePropsUBO) readonly buffer FillExtrusionTilePropsUBOVector {
    FillExtrusionTilePropsUBO tile_props_ubo[];
} tilePropsVector;

layout(set = LAYER_SET_INDEX, binding = idFillExtrusionPropsUBO) uniform FillExtrusionPropsUBO {
    vec4 color;
    vec4 light_color_pad;
    vec4 light_position_base;
    float height;
    float light_intensity;
    float vertical_gradient;
    float opacity;
    float fade;
    float from_scale;
    float to_scale;
    float pad2;
} props;

layout(location = 0) out vec4 frag_lighting;
layout(location = 1) out vec2 frag_pos_a;
layout(location = 2) out vec2 frag_pos_b;

#if !defined(HAS_UNIFORM_u_pattern_from)
layout(location = 3) flat out vec4 frag_pattern_from;
#endif

#if !defined(HAS_UNIFORM_u_pattern_to)
layout(location = 4) flat out vec4 frag_pattern_to;
#endif

void main() {
    const vec2 instancePos = vec2(in_outline_pos);
    const vec2 instanceDecimalsEd = vec2(in_decimals_ed);

    const FillExtrusionDrawableUBO drawable = drawableVector.drawable_ubo[constant.ubo_index];
    const FillExtrusionTilePropsUBO tileProps = tilePropsVector.tile_props_ubo[constant.ubo_index];

#if defined(HAS_UNIFORM_u_base)
    const float base = max(props.light_position_base.w, 0.0);
#else
    const float base = max(unpack_mix_float(in_base, drawable.base_t), 0.0);
#endif

#if defined(HAS_UNIFORM_u_height)
    const float height = max(props.height, 0.0);
#else
    const float height = max(unpack_mix_float(in_height, drawable.height_t), 0.0);
#endif
    bool isDiscarded = mod(instanceDecimalsEd.x, 2.0) > 0.0;
    if (isDiscarded) {
        gl_Position = vec4(0.0);
        return;
    }

    const vec2 nextInstancePos = vec2(in_next_outline_pos);
    const vec2 nextInstanceDecimalsEd = vec2(in_next_decimals_ed);

    const vec2 p1 = instancePos;
    const vec2 p2 = nextInstancePos;
    const vec2 edgevector = normalize(p2 - p1);
    const vec2 own = vec2(-edgevector.y, edgevector.x);

    const vec3 normal = vec3(in_position.x == 0.0 ? wall_normal(in_decimals_ed.x, own) : wall_normal(in_next_decimals_ed.x, own), 0.0);
    const float edgedistance = in_position.x == 0.0 ? nextInstanceDecimalsEd.y : instanceDecimalsEd.y;
    const float t = float(in_position.y);
    const float z = t > 0.0 ? height : base;

    gl_Position = projectTileFor3D(in_position.x == 0.0 ? p1 : p2, z, projectionVector.projection_ubo[constant.ubo_index]);

#if defined(OVERDRAW_INSPECTOR)
    frag_lighting = vec4(1.0);
    frag_pos_a = vec2(0.0);
    frag_pos_b = vec2(0.0);
#if !defined(HAS_UNIFORM_u_pattern_from)
    frag_pattern_from = vec4(0.0);
#endif
#if !defined(HAS_UNIFORM_u_pattern_to)
    frag_pattern_to = vec4(0.0);
#endif
    return;
#else

#if defined(HAS_UNIFORM_u_pattern_from)
    const vec4 pattern_from = tileProps.pattern_from;
#else
    const vec4 pattern_from = in_pattern_from;
    frag_pattern_from = in_pattern_from;
#endif

#if defined(HAS_UNIFORM_u_pattern_to)
    const vec4 pattern_to = tileProps.pattern_to;
#else
    const vec4 pattern_to = in_pattern_to;
    frag_pattern_to = in_pattern_to;
#endif

    const vec2 pattern_tl_a = pattern_from.xy;
    const vec2 pattern_br_a = pattern_from.zw;
    const vec2 pattern_tl_b = pattern_to.xy;
    const vec2 pattern_br_b = pattern_to.zw;

    const float pixelRatio = paintParams.pixel_ratio;
    const float tileZoomRatio = drawable.tile_ratio;
    const float fromScale = props.from_scale;
    const float toScale = props.to_scale;

    const vec2 display_size_a = vec2((pattern_br_a.x - pattern_tl_a.x) / pixelRatio, (pattern_br_a.y - pattern_tl_a.y) / pixelRatio);
    const vec2 display_size_b = vec2((pattern_br_b.x - pattern_tl_b.x) / pixelRatio, (pattern_br_b.y - pattern_tl_b.y) / pixelRatio);

    const vec2 pos = vec2(edgedistance, z * drawable.height_factor); // extrusion side

    vec4 lighting = vec4(0.0, 0.0, 0.0, 1.0);
    float directional = clamp(dot(normal, props.light_position_base.xyz), 0.0, 1.0);
    directional = mix((1.0 - props.light_intensity), max((0.5 + props.light_intensity), 1.0), directional);

    if (normal.z == 0.0) {
        // This avoids another branching statement, but multiplies by a constant of 0.84 if no
        // vertical gradient, and otherwise calculates the gradient based on base + height
        // TODO: If we're optimizing to the level of avoiding branches, we should pre-compute
        //       the square root when height is a uniform.
        const float fMin = mix(0.7, 0.98, 1.0 - props.light_intensity);
        const float factor = clamp((t + base) * pow(height / 150.0, 0.5), fMin, 1.0);
        directional *= (1.0 - props.vertical_gradient) + (props.vertical_gradient * factor);
    }

    lighting.rgb += clamp(directional * props.light_color_pad.rgb, mix(vec3(0.0), vec3(0.3), 1.0 - props.light_color_pad.rgb), vec3(1.0));
    lighting *= props.opacity;

    frag_lighting = lighting;
    frag_pos_a = get_pattern_pos(drawable.pixel_coord_upper, drawable.pixel_coord_lower, fromScale * display_size_a, tileZoomRatio, pos);
    frag_pos_b = get_pattern_pos(drawable.pixel_coord_upper, drawable.pixel_coord_lower, toScale * display_size_b, tileZoomRatio, pos);

#if !defined(HAS_UNIFORM_u_pattern_from)
    frag_pattern_from = in_pattern_from;
#endif

#if !defined(HAS_UNIFORM_u_pattern_to)
    frag_pattern_to = in_pattern_to;
#endif
#endif
}
",
    fragment: r"

layout(location = 0) in vec4 frag_lighting;
layout(location = 1) in vec2 frag_pos_a;
layout(location = 2) in vec2 frag_pos_b;

#if !defined(HAS_UNIFORM_u_pattern_from)
layout(location = 3) flat in vec4 frag_pattern_from;
#endif

#if !defined(HAS_UNIFORM_u_pattern_to)
layout(location = 4) flat in vec4 frag_pattern_to;
#endif

layout(location = 0) out vec4 out_color;

layout(push_constant) uniform Constants {
    int ubo_index;
} constant;

struct FillExtrusionTilePropsUBO {
    vec4 pattern_from;
    vec4 pattern_to;
    vec2 texsize;
    float pad1;
    float pad2;
};

layout(std140, set = LAYER_SET_INDEX, binding = idFillExtrusionTilePropsUBO) readonly buffer FillExtrusionTilePropsUBOVector {
    FillExtrusionTilePropsUBO tile_props_ubo[];
} tilePropsVector;

layout(set = LAYER_SET_INDEX, binding = idFillExtrusionPropsUBO) uniform FillExtrusionPropsUBO {
    vec4 color;
    vec4 light_color_pad;
    vec4 light_position_base;
    float height;
    float light_intensity;
    float vertical_gradient;
    float opacity;
    float fade;
    float from_scale;
    float to_scale;
    float pad2;
} props;

layout(set = DRAWABLE_IMAGE_SET_INDEX, binding = 0) uniform texture2D image0_texture;
layout(set = DRAWABLE_IMAGE_SET_INDEX, binding = samplerBindingStartId) uniform sampler image0_sampler;

void main() {

#if defined(OVERDRAW_INSPECTOR)
    out_color = vec4(1.0);
    return;
#else

    const FillExtrusionTilePropsUBO tileProps = tilePropsVector.tile_props_ubo[constant.ubo_index];

#if defined(HAS_UNIFORM_u_pattern_from)
    const vec4 pattern_from = tileProps.pattern_from;
#else
    const vec4 pattern_from = frag_pattern_from;
#endif
#if defined(HAS_UNIFORM_u_pattern_to)
    const vec4 pattern_to = tileProps.pattern_to;
#else
    const vec4 pattern_to = frag_pattern_to;
#endif

    const vec2 pattern_tl_a = pattern_from.xy;
    const vec2 pattern_br_a = pattern_from.zw;
    const vec2 pattern_tl_b = pattern_to.xy;
    const vec2 pattern_br_b = pattern_to.zw;

    // A crossfade at 0 or 1 shows one pattern, so skip the other.
    vec4 color1 = vec4(0.0);
    if (props.fade < 1.0) {
        const vec2 imagecoord = mod(frag_pos_a, 1.0);
        const vec2 pos = atlas_pos(pattern_tl_a / tileProps.texsize, (pattern_br_a - pattern_tl_a) / tileProps.texsize * imagecoord);
        color1 = texture(sampler2D(image0_texture, image0_sampler), pos);
    }
    vec4 color2 = vec4(0.0);
    if (props.fade > 0.0) {
        const vec2 imagecoord_b = mod(frag_pos_b, 1.0);
        const vec2 pos2 = atlas_pos(pattern_tl_b / tileProps.texsize, (pattern_br_b - pattern_tl_b) / tileProps.texsize * imagecoord_b);
        color2 = texture(sampler2D(image0_texture, image0_sampler), pos2);
    }

    out_color = mix(color1, color2, props.fade) * frag_lighting;
#endif
}
",
};
