//! The programs of the `background` layer type in GLSL ES, for `mln_gl`.

use mln_gfx::shaders::layer_ubo::{GlobalPaintParamsUBO, ProjectionUBO};
use mln_gfx::shaders::program::Program;
use mln_gfx::shaders::shader_defines::{
    ID_BACKGROUND_DRAWABLE_UBO, ID_BACKGROUND_IMAGE_TEXTURE, ID_BACKGROUND_POS_VERTEX_ATTRIBUTE,
    ID_BACKGROUND_PROPS_UBO, ID_GLOBAL_PAINT_PARAMS_UBO, ID_PROJECTION_UBO,
};
use mln_gfx::shaders::source::glsl_es::{Attribute, Glsl, Source, Texture, UniformBlock};

use super::programs;
use super::ubo::{
    BackgroundDrawableUBO, BackgroundDrawableUnionUBO, BackgroundPatternDrawableUBO, BackgroundPatternPropsUBO,
    BackgroundPropsUBO,
};

// Background
static BACKGROUND: Source = Source {
    glsl: &BACKGROUND_SHADER,
    uniform_blocks: &[
        UniformBlock::element::<ProjectionUBO, ProjectionUBO>("ProjectionUBO", ID_PROJECTION_UBO),
        UniformBlock::element::<BackgroundDrawableUBO, BackgroundDrawableUnionUBO>(
            "BackgroundDrawableUBO",
            ID_BACKGROUND_DRAWABLE_UBO,
        ),
        UniformBlock::new::<BackgroundPropsUBO>("BackgroundPropsUBO", ID_BACKGROUND_PROPS_UBO),
    ],
    attributes: &[Attribute::new("a_pos", ID_BACKGROUND_POS_VERTEX_ATTRIBUTE)],
    instance_attributes: &[],
    textures: &[],
};

// Background Pattern
static BACKGROUND_PATTERN: Source = Source {
    glsl: &BACKGROUND_PATTERN_SHADER,
    uniform_blocks: &[
        UniformBlock::element::<ProjectionUBO, ProjectionUBO>("ProjectionUBO", ID_PROJECTION_UBO),
        UniformBlock::new::<GlobalPaintParamsUBO>("GlobalPaintParamsUBO", ID_GLOBAL_PAINT_PARAMS_UBO),
        UniformBlock::element::<BackgroundPatternDrawableUBO, BackgroundDrawableUnionUBO>(
            "BackgroundPatternDrawableUBO",
            ID_BACKGROUND_DRAWABLE_UBO,
        ),
        UniformBlock::new::<BackgroundPatternPropsUBO>("BackgroundPatternPropsUBO", ID_BACKGROUND_PROPS_UBO),
    ],
    attributes: &[Attribute::new("a_pos", ID_BACKGROUND_POS_VERTEX_ATTRIBUTE)],
    instance_attributes: &[],
    textures: &[Texture::new("u_image", ID_BACKGROUND_IMAGE_TEXTURE)],
};

/// The source of each of the type's programs.
pub(super) static SOURCES: &[(&Program, &Source)] =
    &[(&programs::BACKGROUND, &BACKGROUND), (&programs::BACKGROUND_PATTERN, &BACKGROUND_PATTERN)];

const BACKGROUND_PATTERN_SHADER: Glsl = Glsl {
    vertex: r"layout (std140) uniform BackgroundPatternDrawableUBO {
    highp mat4 u_matrix;
    highp vec2 u_pixel_coord_upper;
    highp vec2 u_pixel_coord_lower;
    highp float u_tile_units_to_pixels;
    lowp float drawable_pad1;
    lowp float drawable_pad2;
    lowp float drawable_pad3;
};

layout (std140) uniform BackgroundPatternPropsUBO {
    highp vec2 u_pattern_tl_a;
    highp vec2 u_pattern_br_a;
    highp vec2 u_pattern_tl_b;
    highp vec2 u_pattern_br_b;
    highp vec2 u_pattern_size_a;
    highp vec2 u_pattern_size_b;
    highp float u_scale_a;
    highp float u_scale_b;
    highp float u_mix;
    highp float u_opacity;
};

layout (location = 0) in vec2 a_pos;
out mediump vec2 v_pos_a;
out mediump vec2 v_pos_b;

void main() {
    gl_Position = projectTile(a_pos, a_pos);

    v_pos_a = get_pattern_pos(u_pixel_coord_upper, u_pixel_coord_lower, u_scale_a * u_pattern_size_a, u_tile_units_to_pixels, a_pos);
    v_pos_b = get_pattern_pos(u_pixel_coord_upper, u_pixel_coord_lower, u_scale_b * u_pattern_size_b, u_tile_units_to_pixels, a_pos);
}
",
    fragment: r"layout (std140) uniform BackgroundPatternPropsUBO {
    highp vec2 u_pattern_tl_a;
    highp vec2 u_pattern_br_a;
    highp vec2 u_pattern_tl_b;
    highp vec2 u_pattern_br_b;
    highp vec2 u_pattern_size_a;
    highp vec2 u_pattern_size_b;
    highp float u_scale_a;
    highp float u_scale_b;
    highp float u_mix;
    highp float u_opacity;
};

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

uniform sampler2D u_image;

in mediump vec2 v_pos_a;
in mediump vec2 v_pos_b;

void main() {
    // A crossfade at 0 or 1 shows one pattern, so skip the other.
    vec4 color1 = vec4(0.0);
    if (u_mix < 1.0) {
        vec2 imagecoord = mod(v_pos_a, 1.0);
        highp vec2 pos = atlas_pos(u_pattern_tl_a / u_pattern_atlas_texsize, (u_pattern_br_a - u_pattern_tl_a) / u_pattern_atlas_texsize * imagecoord);
        color1 = texture(u_image, pos);
    }
    vec4 color2 = vec4(0.0);
    if (u_mix > 0.0) {
        vec2 imagecoord_b = mod(v_pos_b, 1.0);
        highp vec2 pos2 = atlas_pos(u_pattern_tl_b / u_pattern_atlas_texsize, (u_pattern_br_b - u_pattern_tl_b) / u_pattern_atlas_texsize * imagecoord_b);
        color2 = texture(u_image, pos2);
    }

    fragColor = mix(color1, color2, u_mix) * u_opacity;

#ifdef OVERDRAW_INSPECTOR
    fragColor = vec4(1.0);
#endif
}
",
};

const BACKGROUND_SHADER: Glsl = Glsl {
    vertex: r"layout (location = 0) in vec2 a_pos;
layout (std140) uniform BackgroundDrawableUBO {
    highp mat4 u_matrix;
};

void main() {
    gl_Position = projectTile(a_pos, a_pos);
}
",
    fragment: r"layout (std140) uniform BackgroundPropsUBO {
    highp vec4 u_color;
    highp float u_opacity;
    lowp float props_pad1;
    lowp float props_pad2;
    lowp float props_pad3;
};

void main() {
    fragColor = u_color * u_opacity;
#ifdef OVERDRAW_INSPECTOR
    fragColor = vec4(1.0);
#endif
}
",
};
