//! The programs of the `color-relief` layer type in GLSL ES, for `mln_gl`.

use mln_gfx::shaders::layer_ubo::ProjectionUBO;
use mln_gfx::shaders::program::Program;
use mln_gfx::shaders::shader_defines::ID_PROJECTION_UBO;
use mln_gfx::shaders::source::glsl_es::{Attribute, Glsl, Source, Texture, UniformBlock};

use super::programs;
use super::programs::{
    ID_COLOR_RELIEF_COLOR_STOPS_TEXTURE, ID_COLOR_RELIEF_DRAWABLE_UBO, ID_COLOR_RELIEF_ELEVATION_STOPS_TEXTURE,
    ID_COLOR_RELIEF_EVALUATED_PROPS_UBO, ID_COLOR_RELIEF_IMAGE_TEXTURE, ID_COLOR_RELIEF_POS_VERTEX_ATTRIBUTE,
    ID_COLOR_RELIEF_TEXTURE_POS_VERTEX_ATTRIBUTE, ID_COLOR_RELIEF_TILE_PROPS_UBO,
};
use super::ubo::{ColorReliefDrawableUBO, ColorReliefEvaluatedPropsUBO, ColorReliefTilePropsUBO};

// Color Relief
static COLOR_RELIEF: Source = Source {
    glsl: &COLOR_RELIEF_SHADER,
    uniform_blocks: &[
        UniformBlock::element::<ProjectionUBO, ProjectionUBO>("ProjectionUBO", ID_PROJECTION_UBO),
        UniformBlock::element::<ColorReliefDrawableUBO, ColorReliefDrawableUBO>(
            "ColorReliefDrawableUBO",
            ID_COLOR_RELIEF_DRAWABLE_UBO,
        ),
        UniformBlock::element::<ColorReliefTilePropsUBO, ColorReliefTilePropsUBO>(
            "ColorReliefTilePropsUBO",
            ID_COLOR_RELIEF_TILE_PROPS_UBO,
        ),
        UniformBlock::new::<ColorReliefEvaluatedPropsUBO>(
            "ColorReliefEvaluatedPropsUBO",
            ID_COLOR_RELIEF_EVALUATED_PROPS_UBO,
        ),
    ],
    attributes: &[
        Attribute::new("a_pos", ID_COLOR_RELIEF_POS_VERTEX_ATTRIBUTE),
        Attribute::new("a_texture_pos", ID_COLOR_RELIEF_TEXTURE_POS_VERTEX_ATTRIBUTE),
    ],
    instance_attributes: &[],
    textures: &[
        Texture::new("u_image", ID_COLOR_RELIEF_IMAGE_TEXTURE),
        Texture::new("u_elevation_stops", ID_COLOR_RELIEF_ELEVATION_STOPS_TEXTURE),
        Texture::new("u_color_stops", ID_COLOR_RELIEF_COLOR_STOPS_TEXTURE),
    ],
};

/// The source of each of the type's programs.
pub(super) static SOURCES: &[(&Program, &Source)] = &[(&programs::COLOR_RELIEF, &COLOR_RELIEF)];

const COLOR_RELIEF_SHADER: Glsl = Glsl {
    vertex: r"layout(std140) uniform ColorReliefDrawableUBO {
    highp mat4 u_matrix;
};

layout(std140) uniform ColorReliefTilePropsUBO {
    highp vec4 u_unpack;
    highp vec2 u_dimension;
    highp int u_color_ramp_size;
    highp float pad_tile0;
};

layout (location = 0) in vec2 a_pos;
layout (location = 1) in vec2 a_texture_pos;
out vec2 v_pos;

void main() {
    gl_Position = projectTile(a_pos, a_pos);

    highp vec2 epsilon = 1.0 / u_dimension;
    float scale = (u_dimension.x - 2.0) / u_dimension.x;
    v_pos = (a_pos / 8192.0) * scale + epsilon;

    // Handle poles
    if (a_pos.y < -32767.5) v_pos.y = 0.0;
    if (a_pos.y > 32766.5) v_pos.y = 1.0;
}
",
    fragment: r"#ifdef GL_ES
precision highp float;
#endif

layout(std140) uniform ColorReliefDrawableUBO {
    highp mat4 u_matrix;
};

layout(std140) uniform ColorReliefTilePropsUBO {
    highp vec4 u_unpack;
    highp vec2 u_dimension;
    highp int u_color_ramp_size;
    highp float pad_tile0;
};

layout(std140) uniform ColorReliefEvaluatedPropsUBO {
    highp float u_opacity;
    highp float pad_eval0;
    highp float pad_eval1;
    highp float pad_eval2;
};

uniform sampler2D u_image;
uniform sampler2D u_elevation_stops;
uniform sampler2D u_color_stops;

in vec2 v_pos;

float getElevation(vec2 coord) {
    // Convert encoded elevation value to meters
    vec4 data = texture(u_image, coord) * 255.0;
    data.a = -1.0;
    return dot(data, u_unpack);
}

float getElevationStop(int stop) {
    // Elevation stops are plain float values, not terrain-RGB encoded
    float x = (float(stop) + 0.5) / float(u_color_ramp_size);
    return texture(u_elevation_stops, vec2(x, 0.5)).r;
}

vec4 getColorStop(int stop) {
    float x = (float(stop) + 0.5) / float(u_color_ramp_size);
    return texture(u_color_stops, vec2(x, 0.5));
}

void main() {
    float el = getElevation(v_pos);

    // Binary search for color stops
    int r = (u_color_ramp_size - 1);
    int l = 0;
    // The elevations at l and r, as the search reads them.
    float el_l = 0.0;
    float el_r = 0.0;

    while (r - l > 1) {
        int m = (r + l) / 2;
        float el_m = getElevationStop(m);
        if (el < el_m) {
            r = m;
            el_r = el_m;
        } else {
            l = m;
            el_l = el_m;
        }
    }

    // The search never reads the ramp's ends, so read an end that l or r
    // still is.
    if (l == 0) {
        el_l = getElevationStop(0);
    }
    if (r == u_color_ramp_size - 1) {
        el_r = getElevationStop(r);
    }

    // Get colors for interpolation
    vec4 color_l = getColorStop(l);
    vec4 color_r = getColorStop(r);

    // Interpolate between the two colors
    // Guard against division by zero when el_r == el_l
    float denom = el_r - el_l;
    float t = (abs(denom) < 0.0001) ? 0.0 : clamp((el - el_l) / denom, 0.0, 1.0);
    fragColor = u_opacity * mix(color_l, color_r, t);

#ifdef OVERDRAW_INSPECTOR
    fragColor = vec4(1.0);
#endif
}
",
};
