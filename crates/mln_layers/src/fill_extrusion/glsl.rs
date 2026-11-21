//! The programs of the `fill-extrusion` layer type in GLSL, which `mln_glsl` parses for `mln_vulkan` and `mln_d3d12`.

use mln_gfx::shaders::source::glsl::Glsl;

use crate::shaders::{atlas_pos, pattern_pos};

const FILL_EXTRUSION_SHADER_PRELUDE: &str = concat!(
    pattern_pos!(glsl),
    atlas_pos!(glsl),
    r"
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

"
);

pub(super) const FILL_EXTRUSION_SHADER: Glsl = Glsl {
    prelude: FILL_EXTRUSION_SHADER_PRELUDE,
    vertex: r"
invariant gl_Position;

layout(location = 0) in ivec2 in_position;
layout(location = 1) in uvec2 in_decimals_ed;

#property attribute color

#property attribute base

#property attribute height

layout(location = 0) out vec4 frag_color;

void main() {
    const FillExtrusionDrawableUBO drawable = drawableVector[constant.ubo_index];

    const float base = max($props.base, 0.0);

    const float height = max($props.height, 0.0);

    vec4 color = $props.color;

    const vec3 normal = vec3(0.0, 0.0, 1.0);
    const float t = 1.0;
    const float z = t > 0.0 ? height : base;
    gl_Position = projectTileFor3D(vec2(in_position), z, projectionVector[constant.ubo_index]);

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
    const float directionalFraction = clamp(dot(normal, props.light_position), 0.0, 1.0);

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
    const vec3 light_color = props.light_color;
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

    const FillExtrusionDrawableUBO drawable = drawableVector[constant.ubo_index];

    const float base = max($props.base, 0.0);

    const float height = max($props.height, 0.0);

    vec4 color = $props.color;
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

    gl_Position = projectTileFor3D(in_position.x == 0.0 ? p1 : p2, z, projectionVector[constant.ubo_index]);

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
    const float directionalFraction = clamp(dot(normal, props.light_position), 0.0, 1.0);

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
    const vec3 light_color = props.light_color;
    const vec3 minLight = mix(vec3(0.0), vec3(0.3), 1.0 - light_color.rgb);
    vcolor += vec4(clamp(color.rgb * directional * light_color.rgb, minLight, vec3(1.0)), 0.0);

    frag_color = vcolor * props.opacity;
"
    };
}

pub(super) const FILL_EXTRUSION_INSTANCED_SHADER: Glsl = Glsl {
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

#property attribute color

#property attribute base

#property attribute height

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
pub(super) const FILL_EXTRUSION_PULLED_SHADER: Glsl = Glsl {
    prelude: FILL_EXTRUSION_SHADER_PRELUDE,
    vertex: concat!(
        r"
invariant gl_Position;

",
        fill_extrusion_wall_declarations!(),
        r"
// The bucket's vertices, two words each: the position's x and y in the
// halves of the first, decimals_ed's in those of the second.

#if !defined(HAS_UNIFORM_u_color) || !defined(HAS_UNIFORM_u_base) || !defined(HAS_UNIFORM_u_height)
// The bucket's paint values, interleaved as the roofs read them.
#endif

void main() {
    // Four vertices a wall, in the order of the static quad's corners.
    const uint wall = uint(gl_VertexIndex) / 4u;
    const uint corner = uint(gl_VertexIndex) % 4u;
    const uint pos = wallVertices[2u * wall];
    const uint decimals_ed = wallVertices[2u * wall + 1u];
    const uint next_pos = wallVertices[2u * wall + 2u];
    const uint next_decimals_ed = wallVertices[2u * wall + 3u];

    // What the instanced walls read as vertex inputs.
    const ivec2 in_position = ivec2(corner < 2u ? 1 : 0, int(corner & 1u));
    const ivec2 in_outline_pos = ivec2(int(pos << 16u) >> 16, int(pos) >> 16);
    const uvec2 in_decimals_ed = uvec2(decimals_ed & 0xffffu, decimals_ed >> 16u);
    const ivec2 in_next_outline_pos = ivec2(int(next_pos << 16u) >> 16, int(next_pos) >> 16);
    const uvec2 in_next_decimals_ed = uvec2(next_decimals_ed & 0xffffu, next_decimals_ed >> 16u);

    // The wall's paint values are its outline vertex's.
    const uint paint_layout = drawableVector[constant.ubo_index].paint_layout;
    const uint paint = wall * (paint_layout & 0xffu);
#if !defined(HAS_UNIFORM_u_base)
    const uint base_at = paint + ((paint_layout >> 8u) & 0xffu);
    const vec2 in_base = vec2(wallPaint[base_at], wallPaint[base_at + 1u]);
#endif
#if !defined(HAS_UNIFORM_u_height)
    const uint height_at = paint + ((paint_layout >> 16u) & 0xffu);
    const vec2 in_height = vec2(wallPaint[height_at], wallPaint[height_at + 1u]);
#endif
#if !defined(HAS_UNIFORM_u_color)
    const uint color_at = paint + (paint_layout >> 24u);
    const vec4 in_color = vec4(wallPaint[color_at], wallPaint[color_at + 1u],
                               wallPaint[color_at + 2u], wallPaint[color_at + 3u]);
#endif
",
        fill_extrusion_wall_main!(),
        r"}
"
    ),
    fragment: FILL_EXTRUSION_INSTANCED_SHADER.fragment,
};

pub(super) const FILL_EXTRUSION_PATTERN_SHADER: Glsl = Glsl {
    prelude: FILL_EXTRUSION_SHADER_PRELUDE,
    vertex: r"
invariant gl_Position;

layout(location = 0) in ivec2 in_position;
layout(location = 1) in uvec2 in_decimals_ed;

#property attribute base

#property attribute height

#property attribute pattern_from

#property attribute pattern_to

layout(location = 0) out vec4 frag_lighting;
layout(location = 1) out vec2 frag_pos_a;
layout(location = 2) out vec2 frag_pos_b;

#property varying pattern_from 3 flat

#property varying pattern_to 4 flat

void main() {
    const FillExtrusionDrawableUBO drawable = drawableVector[constant.ubo_index];
    const FillExtrusionTilePropsUBO tileProps = tilePropsVector[constant.ubo_index];

    const float base = max($props.base, 0.0);

    const float height = max($props.height, 0.0);

    const vec3 normal = vec3(0.0, 0.0, 1.0);
    const float t = 1.0;
    const float z = t > 0.0 ? height : base;
    gl_Position = projectTileFor3D(vec2(in_position), z, projectionVector[constant.ubo_index]);

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

    const float pixelRatio = view.pixel_ratio;
    const float tileZoomRatio = drawable.tile_ratio;
    const float fromScale = props.from_scale;
    const float toScale = props.to_scale;

    const vec2 display_size_a = vec2((pattern_br_a.x - pattern_tl_a.x) / pixelRatio, (pattern_br_a.y - pattern_tl_a.y) / pixelRatio);
    const vec2 display_size_b = vec2((pattern_br_b.x - pattern_tl_b.x) / pixelRatio, (pattern_br_b.y - pattern_tl_b.y) / pixelRatio);

    const vec2 pos = vec2(in_position); // extrusion top

    vec4 lighting = vec4(0.0, 0.0, 0.0, 1.0);
    float directional = clamp(dot(normal, props.light_position), 0.0, 1.0);
    directional = mix((1.0 - props.light_intensity), max((0.5 + props.light_intensity), 1.0), directional);

    if (normal.z == 0.0) {
        // This avoids another branching statement, but multiplies by a constant of 0.84 if no
        // vertical gradient, and otherwise calculates the gradient based on base + height
        // TODO: If we're optimizing to the level of avoiding branches, we should pre-compute
        //       the square root when height is a uniform.
        const float fMin = mix(0.7, 0.98, 1.0 - props.light_intensity);
        const float factor = clamp((t + base) * pow(height / 150.0, 0.5), fMin, 1.0);
        directional *= (1.0 - props.vertical_gradient) + (props.vertical_gradient * factor);    }

    lighting.rgb += clamp(directional * props.light_color, mix(vec3(0.0), vec3(0.3), 1.0 - props.light_color), vec3(1.0));
    lighting *= props.opacity;

    frag_lighting = lighting;
    frag_pos_a = get_pattern_pos(drawable.pixel_coord_upper, drawable.pixel_coord_lower, fromScale * display_size_a, tileZoomRatio, pos);
    frag_pos_b = get_pattern_pos(drawable.pixel_coord_upper, drawable.pixel_coord_lower, toScale * display_size_b, tileZoomRatio, pos);

    #property pass pattern_from

    #property pass pattern_to
#endif
}
",
    fragment: r"

layout(location = 0) in vec4 frag_lighting;
layout(location = 1) in vec2 frag_pos_a;
layout(location = 2) in vec2 frag_pos_b;

#property varying pattern_from 3 flat

#property varying pattern_to 4 flat

layout(location = 0) out vec4 out_color;

void main() {

#if defined(OVERDRAW_INSPECTOR)
    out_color = vec4(1.0);
    return;
#else

    const FillExtrusionTilePropsUBO tileProps = tilePropsVector[constant.ubo_index];

    const vec4 pattern_from = $tileProps.pattern_from;
    const vec4 pattern_to = $tileProps.pattern_to;

    const vec2 pattern_tl_a = pattern_from.xy;
    const vec2 pattern_br_a = pattern_from.zw;
    const vec2 pattern_tl_b = pattern_to.xy;
    const vec2 pattern_br_b = pattern_to.zw;

    // A crossfade at 0 or 1 shows one pattern, so skip the other.
    vec4 color1 = vec4(0.0);
    if (props.fade < 1.0) {
        const vec2 imagecoord = mod(frag_pos_a, 1.0);
        const vec2 pos = atlas_pos(pattern_tl_a / tileProps.texsize, (pattern_br_a - pattern_tl_a) / tileProps.texsize * imagecoord);
        color1 = texture(sampler2D(image, image_sampler), pos);
    }
    vec4 color2 = vec4(0.0);
    if (props.fade > 0.0) {
        const vec2 imagecoord_b = mod(frag_pos_b, 1.0);
        const vec2 pos2 = atlas_pos(pattern_tl_b / tileProps.texsize, (pattern_br_b - pattern_tl_b) / tileProps.texsize * imagecoord_b);
        color2 = texture(sampler2D(image, image_sampler), pos2);
    }

    out_color = mix(color1, color2, props.fade) * frag_lighting;
#endif
}
",
};

pub(super) const FILL_EXTRUSION_PATTERN_INSTANCED_SHADER: Glsl = Glsl {
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

#property attribute base

#property attribute height

#property attribute pattern_from

#property attribute pattern_to

layout(location = 0) out vec4 frag_lighting;
layout(location = 1) out vec2 frag_pos_a;
layout(location = 2) out vec2 frag_pos_b;

#property varying pattern_from 3 flat

#property varying pattern_to 4 flat

void main() {
    const vec2 instancePos = vec2(in_outline_pos);
    const vec2 instanceDecimalsEd = vec2(in_decimals_ed);

    const FillExtrusionDrawableUBO drawable = drawableVector[constant.ubo_index];
    const FillExtrusionTilePropsUBO tileProps = tilePropsVector[constant.ubo_index];

    const float base = max($props.base, 0.0);

    const float height = max($props.height, 0.0);
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

    gl_Position = projectTileFor3D(in_position.x == 0.0 ? p1 : p2, z, projectionVector[constant.ubo_index]);

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

    const float pixelRatio = view.pixel_ratio;
    const float tileZoomRatio = drawable.tile_ratio;
    const float fromScale = props.from_scale;
    const float toScale = props.to_scale;

    const vec2 display_size_a = vec2((pattern_br_a.x - pattern_tl_a.x) / pixelRatio, (pattern_br_a.y - pattern_tl_a.y) / pixelRatio);
    const vec2 display_size_b = vec2((pattern_br_b.x - pattern_tl_b.x) / pixelRatio, (pattern_br_b.y - pattern_tl_b.y) / pixelRatio);

    const vec2 pos = vec2(edgedistance, z * drawable.height_factor); // extrusion side

    vec4 lighting = vec4(0.0, 0.0, 0.0, 1.0);
    float directional = clamp(dot(normal, props.light_position), 0.0, 1.0);
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

    lighting.rgb += clamp(directional * props.light_color, mix(vec3(0.0), vec3(0.3), 1.0 - props.light_color), vec3(1.0));
    lighting *= props.opacity;

    frag_lighting = lighting;
    frag_pos_a = get_pattern_pos(drawable.pixel_coord_upper, drawable.pixel_coord_lower, fromScale * display_size_a, tileZoomRatio, pos);
    frag_pos_b = get_pattern_pos(drawable.pixel_coord_upper, drawable.pixel_coord_lower, toScale * display_size_b, tileZoomRatio, pos);

    #property pass pattern_from

    #property pass pattern_to
#endif
}
",
    fragment: r"

layout(location = 0) in vec4 frag_lighting;
layout(location = 1) in vec2 frag_pos_a;
layout(location = 2) in vec2 frag_pos_b;

#property varying pattern_from 3 flat

#property varying pattern_to 4 flat

layout(location = 0) out vec4 out_color;

void main() {

#if defined(OVERDRAW_INSPECTOR)
    out_color = vec4(1.0);
    return;
#else

    const FillExtrusionTilePropsUBO tileProps = tilePropsVector[constant.ubo_index];

    const vec4 pattern_from = $tileProps.pattern_from;
    const vec4 pattern_to = $tileProps.pattern_to;

    const vec2 pattern_tl_a = pattern_from.xy;
    const vec2 pattern_br_a = pattern_from.zw;
    const vec2 pattern_tl_b = pattern_to.xy;
    const vec2 pattern_br_b = pattern_to.zw;

    // A crossfade at 0 or 1 shows one pattern, so skip the other.
    vec4 color1 = vec4(0.0);
    if (props.fade < 1.0) {
        const vec2 imagecoord = mod(frag_pos_a, 1.0);
        const vec2 pos = atlas_pos(pattern_tl_a / tileProps.texsize, (pattern_br_a - pattern_tl_a) / tileProps.texsize * imagecoord);
        color1 = texture(sampler2D(image, image_sampler), pos);
    }
    vec4 color2 = vec4(0.0);
    if (props.fade > 0.0) {
        const vec2 imagecoord_b = mod(frag_pos_b, 1.0);
        const vec2 pos2 = atlas_pos(pattern_tl_b / tileProps.texsize, (pattern_br_b - pattern_tl_b) / tileProps.texsize * imagecoord_b);
        color2 = texture(sampler2D(image, image_sampler), pos2);
    }

    out_color = mix(color1, color2, props.fade) * frag_lighting;
#endif
}
",
};
