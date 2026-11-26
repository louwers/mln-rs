//! The GLSL ES, for OpenGL, of the fills' programs.

use mln_gfx::shaders::block::UniformBlock;
use mln_gfx::shaders::source::Precision::Low;
use mln_gfx::shaders::source::{GlslEsDeclarations, GlslEsSource};

use super::ubo::FillPatternTilePropsUBO;
use crate::layers::pattern::glsl_es::pattern_pos;

/// The precisions of the fills' values.
const FILL_DECLARATIONS: GlslEsDeclarations =
    GlslEsDeclarations { attributes: &[(Low, &["opacity"])], ..GlslEsDeclarations::DEFAULT };

/// The precisions of the pattern fills' values.
const FILL_PATTERN_DECLARATIONS: GlslEsDeclarations = GlslEsDeclarations {
    members: &[(FillPatternTilePropsUBO::BLOCK, Low, &["sdf"])],
    attributes: &[(Low, &["opacity"])],
    ..GlslEsDeclarations::DEFAULT
};

pub(crate) const FILL_OUTLINE_PATTERN: GlslEsSource = GlslEsSource {
    vertex: concat!(
        pattern_pos!(),
        r"out vec2 v_pos_a;
out vec2 v_pos_b;
out vec2 v_pos;

#ifndef HAS_UNIFORM_u_color
out highp vec4 color;
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

void main() {
    #ifndef HAS_UNIFORM_u_color
color = unpack_mix_color(a_color, u_color_t);
#else
highp vec4 color = u_color;
#endif
    #ifndef HAS_UNIFORM_u_opacity
opacity = unpack_mix_vec2(a_opacity, u_opacity_t);
#else
lowp float opacity = u_opacity;
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

    gl_Position = projectTile(a_pos, a_pos);

    vec2 display_size_a = vec2((pattern_br_a.x - pattern_tl_a.x) / pixelRatio, (pattern_br_a.y - pattern_tl_a.y) / pixelRatio);
    vec2 display_size_b = vec2((pattern_br_b.x - pattern_tl_b.x) / pixelRatio, (pattern_br_b.y - pattern_tl_b.y) / pixelRatio);

    v_pos_a = get_pattern_pos(u_pixel_coord_upper, u_pixel_coord_lower, fromScale * display_size_a, tileRatio, a_pos);
    v_pos_b = get_pattern_pos(u_pixel_coord_upper, u_pixel_coord_lower, toScale * display_size_b, tileRatio, a_pos);

    v_pos = (gl_Position.xy / gl_Position.w + 1.0) / 2.0 * u_world_size;
}
",
    ),
    fragment: r"#ifdef GL_ES
precision highp float;
#endif

in vec2 v_pos_a;
in vec2 v_pos_b;
in vec2 v_pos;

#ifndef HAS_UNIFORM_u_color
in highp vec4 color;
#endif
#ifndef HAS_UNIFORM_u_opacity
in lowp float opacity;
#endif
#ifndef HAS_UNIFORM_u_pattern_from
flat in highp vec4 pattern_from;
#endif
#ifndef HAS_UNIFORM_u_pattern_to
flat in highp vec4 pattern_to;
#endif

void main() {
    #ifdef HAS_UNIFORM_u_color
highp vec4 color = u_color;
#endif
    #ifdef HAS_UNIFORM_u_opacity
lowp float opacity = u_opacity;
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

    // find distance to outline for alpha interpolation

    float dist = length(v_pos - gl_FragCoord.xy);
    float alpha = 1.0 - smoothstep(0.0, 1.0, dist);


    if (u_sdf > 0.5) {
        highp float sdf_edge = (256.0 - 64.0) / 256.0;
        highp float sdf_gamma_a = max(fwidth(color1.a) * 0.5, 1.0 / 255.0 / 16.0);
        highp float sdf_gamma_b = max(fwidth(color2.a) * 0.5, 1.0 / 255.0 / 16.0);
        float sdf_alpha_a = smoothstep(sdf_edge - sdf_gamma_a, sdf_edge + sdf_gamma_a, color1.a);
        float sdf_alpha_b = smoothstep(sdf_edge - sdf_gamma_b, sdf_edge + sdf_gamma_b, color2.a);
        fragColor = mix(color * sdf_alpha_a, color * sdf_alpha_b, u_fade) * alpha * opacity;
    } else {
        fragColor = mix(color1, color2, u_fade) * alpha * opacity;
    }
}
",
    declarations: &FILL_PATTERN_DECLARATIONS,
};

pub(crate) const FILL_OUTLINE: GlslEsSource = GlslEsSource {
    vertex: r"out vec2 v_pos;

#ifndef HAS_UNIFORM_u_outline_color
out highp vec4 outline_color;
#endif
#ifndef HAS_UNIFORM_u_opacity
out lowp float opacity;
#endif

void main() {
    #ifndef HAS_UNIFORM_u_outline_color
outline_color = unpack_mix_color(a_outline_color, u_outline_color_t);
#endif
    #ifndef HAS_UNIFORM_u_opacity
opacity = unpack_mix_vec2(a_opacity, u_opacity_t);
#endif

    gl_Position = projectTile(a_pos, a_pos);
    v_pos = (gl_Position.xy / gl_Position.w + 1.0) / 2.0 * u_world_size;
}
",
    fragment: r"in highp vec2 v_pos;

#ifndef HAS_UNIFORM_u_outline_color
in highp vec4 outline_color;
#endif
#ifndef HAS_UNIFORM_u_opacity
in lowp float opacity;
#endif

void main() {
    #ifdef HAS_UNIFORM_u_outline_color
highp vec4 outline_color = u_outline_color;
#endif
    #ifdef HAS_UNIFORM_u_opacity
lowp float opacity = u_opacity;
#endif

    float dist = length(v_pos - gl_FragCoord.xy);
    float alpha = 1.0 - smoothstep(0.0, 1.0, dist);
    fragColor = outline_color * (alpha * opacity);
}
",
    declarations: &FILL_DECLARATIONS,
};

/// `outline_corner`, the corner of an instanced outline's quad.
macro_rules! outline_corner {
    () => {
        r"
// The width of the outline in pixels.
const float LINE_WIDTH = 2.0;

// A corner of the quad that draws the segment from `start` to `end`, of clip
// space, as a wide line covers it: LINE_WIDTH pixels across its minor axis
// (vertically when it runs more across the screen than up it), from one end
// to the other, with the attributes of its end as a wide line's fragments get
// them: `pos`, the end in window coordinates, and `t`, how far along the
// segment that is, from 0 at its start to 1 at its end.
vec4 outline_corner(highp vec4 start, highp vec4 end, vec2 corner, out highp vec2 pos, out highp float t) {
    pos = vec2(0.0);
    t = 0.0;
    // The part of the segment in front of the camera, which the GPU clips
    // further: its ends have to be on screen to widen it.
    highp float near_w = max(start.w, end.w) * 1e-4;
    if (start.w <= near_w && end.w <= near_w) {
        return vec4(0.0);
    }
    highp vec4 a = start;
    highp vec4 b = end;
    highp float ta = 0.0;
    highp float tb = 1.0;
    if (a.w < near_w) {
        ta = (a.w - near_w) / (a.w - b.w);
        a = mix(start, end, ta);
    }
    if (b.w < near_w) {
        tb = (start.w - near_w) / (start.w - b.w);
        b = mix(start, end, tb);
    }

    highp vec2 sa = (a.xy / a.w + 1.0) / 2.0 * u_world_size;
    highp vec2 sb = (b.xy / b.w + 1.0) / 2.0 * u_world_size;
    highp vec2 d = sb - sa;
    if (d == vec2(0.0)) {
        return vec4(0.0);
    }
    highp vec2 across = abs(d.x) >= abs(d.y) ? vec2(0.0, 1.0) : vec2(1.0, 0.0);
    highp vec2 offset = across * (corner.y - 0.5) * LINE_WIDTH;

    bool at_end = corner.x != 0.0;
    highp vec4 clip = at_end ? b : a;
    pos = at_end ? sb : sa;
    t = at_end ? tb : ta;
    return clip + vec4(offset * 2.0 / u_world_size * clip.w, 0.0, 0.0);
}
"
    };
}

pub(crate) const FILL_OUTLINE_INSTANCED: GlslEsSource = GlslEsSource {
    vertex: concat!(
        r"out vec2 v_pos;

#ifndef HAS_UNIFORM_u_outline_color
out highp vec4 outline_color;
#endif
#ifndef HAS_UNIFORM_u_opacity
out lowp float opacity;
#endif
",
        outline_corner!(),
        r"
void main() {
    #ifndef HAS_UNIFORM_u_outline_color
outline_color = unpack_mix_color(a_outline_color, u_outline_color_t);
#endif
    #ifndef HAS_UNIFORM_u_opacity
opacity = unpack_mix_vec2(a_opacity, u_opacity_t);
#endif

    highp float t;
    gl_Position = outline_corner(projectTile(a_outline_pos, a_outline_pos), projectTile(a_outline_end, a_outline_end), a_pos, v_pos, t);
}
"
    ),
    fragment: FILL_OUTLINE.fragment,
    declarations: &FILL_DECLARATIONS,
};

pub(crate) const FILL_OUTLINE_PATTERN_INSTANCED: GlslEsSource = GlslEsSource {
    vertex: concat!(
        pattern_pos!(),
        r"out vec2 v_pos_a;
out vec2 v_pos_b;
out vec2 v_pos;

#ifndef HAS_UNIFORM_u_color
out highp vec4 color;
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
",
        outline_corner!(),
        r"
void main() {
    #ifndef HAS_UNIFORM_u_color
color = unpack_mix_color(a_color, u_color_t);
#else
highp vec4 color = u_color;
#endif
    #ifndef HAS_UNIFORM_u_opacity
opacity = unpack_mix_vec2(a_opacity, u_opacity_t);
#else
lowp float opacity = u_opacity;
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

    vec2 display_size_a = (pattern_from.zw - pattern_from.xy) / u_pixel_ratio;
    vec2 display_size_b = (pattern_to.zw - pattern_to.xy) / u_pixel_ratio;

    highp float t;
    gl_Position = outline_corner(projectTile(a_outline_pos, a_outline_pos), projectTile(a_outline_end, a_outline_end), a_pos, v_pos, t);
    vec2 pos = mix(a_outline_pos, a_outline_end, t);

    v_pos_a = get_pattern_pos(u_pixel_coord_upper, u_pixel_coord_lower, u_from_scale * display_size_a, u_tile_ratio, pos);
    v_pos_b = get_pattern_pos(u_pixel_coord_upper, u_pixel_coord_lower, u_to_scale * display_size_b, u_tile_ratio, pos);
}
"
    ),
    fragment: FILL_OUTLINE_PATTERN.fragment,
    declarations: &FILL_PATTERN_DECLARATIONS,
};

pub(crate) const FILL_PATTERN: GlslEsSource = GlslEsSource {
    vertex: concat!(
        pattern_pos!(),
        r"out vec2 v_pos_a;
out vec2 v_pos_b;

#ifndef HAS_UNIFORM_u_color
out highp vec4 color;
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

void main() {
    #ifndef HAS_UNIFORM_u_color
color = unpack_mix_color(a_color, u_color_t);
#else
highp vec4 color = u_color;
#endif
    #ifndef HAS_UNIFORM_u_opacity
opacity = unpack_mix_vec2(a_opacity, u_opacity_t);
#else
lowp float opacity = u_opacity;
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
    float tileZoomRatio = u_tile_ratio;
    float fromScale = u_from_scale;
    float toScale = u_to_scale;

    vec2 display_size_a = vec2((pattern_br_a.x - pattern_tl_a.x) / pixelRatio, (pattern_br_a.y - pattern_tl_a.y) / pixelRatio);
    vec2 display_size_b = vec2((pattern_br_b.x - pattern_tl_b.x) / pixelRatio, (pattern_br_b.y - pattern_tl_b.y) / pixelRatio);
    gl_Position = projectTile(a_pos, a_pos);

    v_pos_a = get_pattern_pos(u_pixel_coord_upper, u_pixel_coord_lower, fromScale * display_size_a, tileZoomRatio, a_pos);
    v_pos_b = get_pattern_pos(u_pixel_coord_upper, u_pixel_coord_lower, toScale * display_size_b, tileZoomRatio, a_pos);
}
",
    ),
    fragment: r"#ifdef GL_ES
precision highp float;
#endif

in vec2 v_pos_a;
in vec2 v_pos_b;

#ifndef HAS_UNIFORM_u_color
in highp vec4 color;
#endif
#ifndef HAS_UNIFORM_u_opacity
in lowp float opacity;
#endif
#ifndef HAS_UNIFORM_u_pattern_from
flat in highp vec4 pattern_from;
#endif
#ifndef HAS_UNIFORM_u_pattern_to
flat in highp vec4 pattern_to;
#endif

void main() {
    #ifdef HAS_UNIFORM_u_color
highp vec4 color = u_color;
#endif
    #ifdef HAS_UNIFORM_u_opacity
lowp float opacity = u_opacity;
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

    if (u_sdf > 0.5) {
        highp float sdf_edge = (256.0 - 64.0) / 256.0;
        highp float sdf_gamma_a = max(fwidth(color1.a) * 0.5, 1.0 / 255.0 / 16.0);
        highp float sdf_gamma_b = max(fwidth(color2.a) * 0.5, 1.0 / 255.0 / 16.0);
        float sdf_alpha_a = smoothstep(sdf_edge - sdf_gamma_a, sdf_edge + sdf_gamma_a, color1.a);
        float sdf_alpha_b = smoothstep(sdf_edge - sdf_gamma_b, sdf_edge + sdf_gamma_b, color2.a);
        fragColor = mix(color * sdf_alpha_a, color * sdf_alpha_b, u_fade) * opacity;
    } else {
        fragColor = mix(color1, color2, u_fade) * opacity;
    }
}
",
    declarations: &FILL_PATTERN_DECLARATIONS,
};

pub(crate) const FILL: GlslEsSource = GlslEsSource {
    vertex: r"#ifndef HAS_UNIFORM_u_color
out highp vec4 color;
#endif
#ifndef HAS_UNIFORM_u_opacity
out lowp float opacity;
#endif

void main() {
    #ifndef HAS_UNIFORM_u_color
color = unpack_mix_color(a_color, u_color_t);
#endif
    #ifndef HAS_UNIFORM_u_opacity
opacity = unpack_mix_vec2(a_opacity, u_opacity_t);
#endif

    gl_Position = projectTile(a_pos, a_pos);
}
",
    fragment: r"#ifndef HAS_UNIFORM_u_color
in highp vec4 color;
#endif
#ifndef HAS_UNIFORM_u_opacity
in lowp float opacity;
#endif

void main() {
    #ifdef HAS_UNIFORM_u_color
highp vec4 color = u_color;
#endif
    #ifdef HAS_UNIFORM_u_opacity
lowp float opacity = u_opacity;
#endif

    fragColor = color * opacity;
}
",
    declarations: &FILL_DECLARATIONS,
};
