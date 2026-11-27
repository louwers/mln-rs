//! The GLSL, for Vulkan and D3D12, of the terrain's programs, and the
//! snippets of the ground that other programs include.

use mln_gfx::shaders::source::GlslSource;

/// What the terrain variant of a program that hides behind the ground in
/// front of it reads of the ground's depth, within `#ifdef TERRAIN` of its
/// vertex stage: whether that ground hides a point (`ground_visibility`).
#[doc(hidden)]
#[macro_export]
macro_rules! terrain_glsl_ground_visibility {
    () => {
        r"
#ifdef TERRAIN
// GL JS's `depthOpacity`: 1 where a point `w` from the eye, at `ndc`, is in
// front of the ground drawn there, fading to 0 behind it over GL JS's ramp
// of its NDC depth, which `depth_factor` scales; 1 where no ground is drawn
// there or `ndc` is outside the view. The texels' rows run from the top.
float ground_depth_opacity(const vec2 ndc, const float w, const float depth_factor) {
    if (any(greaterThan(abs(ndc), vec2(1.0)))) {
        return 1.0;
    }
    const ivec2 size = textureSize(ground_depth_texture, 0);
    const vec2 pos = (vec2(ndc.x, -ndc.y) * 0.5 + 0.5) * vec2(size);
    const ivec2 texel = clamp(ivec2(floor(pos)), ivec2(0), size - 1);
    const uvec4 bytes = uvec4(round(texelFetch(ground_depth_texture, texel, 0) * 255.0));
    const float ground = uintBitsToFloat(bytes.x | (bytes.y << 8u) | (bytes.z << 16u) | (bytes.w << 24u));
    if (!(ground > 0.0)) {
        return 1.0;
    }
    const float d = depth_factor * (1.0 / w - 1.0 / ground) + 0.0001;
    return 1.0 - clamp(-500.0 * d, 0.0, 1.0);
}

// GL JS's `calculate_visibility` of the point at `clip`: 1 where the ground
// drawn in front doesn't hide it, 0 where it does, averaged with the point
// 0.01 NDC above it where it is partly hidden. The ramp's scale is GL JS's:
// `2n` of a near plane `n` a fiftieth of the view's height, in pixels, a
// `GlobalPaintParamsUBO`'s `world_size.y / pixel_ratio`.
float ground_visibility(const vec4 clip, const float view_height) {
    if (clip.w <= 0.0) {
        return 1.0;
    }
    const vec2 ndc = clip.xy / clip.w;
    const float depth_factor = view_height / 25.0;
    const float visibility = ground_depth_opacity(ndc, clip.w, depth_factor);
    if (visibility > 0.95) {
        return 1.0;
    }
    return (visibility + ground_depth_opacity(ndc + vec2(0.0, 0.01), clip.w, depth_factor)) / 2.0;
}
#endif
"
    };
}
pub(crate) use crate::terrain_glsl_ground_visibility as ground_visibility;

/// The height rule of `crate::tile::dem`, over the DEM tile a draw reads: its
/// RGBA8 texels behind a one-texel border, decoded by the terrain's uniform
/// block, `terrain`. A macro, so that `concat!` puts it in the vertex stages
/// that read the DEM.
#[doc(hidden)]
#[macro_export]
macro_rules! terrain_glsl_ground_height {
    () => {
        r"

// The height in meters of the DEM's bordered texel at `texel`, clamped to the
// texture, decoded by `terrain`.
float ground_texel_height(const ivec2 texel) {
    const ivec2 last = textureSize(ground_dem_texture, 0) - 1;
    const vec3 rgb = round(texelFetch(ground_dem_texture, clamp(texel, ivec2(0), last), 0).rgb * 255.0);
    const vec4 unpack = terrain.unpack;
    return (rgb.r * unpack.x + rgb.g * unpack.y + rgb.b * unpack.z + unpack.w) * terrain.unpack_scale;
}

// The height in meters, exaggerated, at `pos` in tile units of a tile that
// reads its DEM at `dem_coords`: the DEM uv's scale and offset, then the
// DEM's width in pixels, 0 for flat ground.
float ground_height(const vec2 pos, const vec4 dem_coords) {
    const float dim = dem_coords.w;
    if (dim == 0.0) {
        return 0.0;
    }
    const vec2 coord = (pos / 8192.0 * dem_coords.x + dem_coords.yz) * dim + 0.5;
    const vec2 first = floor(coord);
    const vec2 f = coord - first;
    const ivec2 texel = ivec2(first);
    const float tl = ground_texel_height(texel);
    const float tr = ground_texel_height(texel + ivec2(1, 0));
    const float bl = ground_texel_height(texel + ivec2(0, 1));
    const float br = ground_texel_height(texel + ivec2(1, 1));
    const float top = tl * (1.0 - f.x) + tr * f.x;
    const float bottom = bl * (1.0 - f.x) + br * f.x;
    return (top * (1.0 - f.y) + bottom * f.y) * terrain.exaggeration;
}
"
    };
}
pub(crate) use crate::terrain_glsl_ground_height as ground_height;

/// `fogged`, which covers a color of the surface with the fog of the
/// terrain's uniform block, `terrain`.
#[doc(hidden)]
#[macro_export]
macro_rules! terrain_glsl_terrain_fog {
    () => {
        r"

// `color`, premultiplied, of the surface `view_depth` pixels along the
// view, covered by the fog of `terrain`, as GL JS's terrain is: mixed
// in linear components toward the fog's color by the square of how far past
// the ground blend the fog's depth is. The fog's alpha weighs its color; the
// coverage stays.
vec4 fogged(const vec4 color, const float view_depth) {
    const float near = terrain.depth_range.x;
    const float far = terrain.depth_range.y;
    const float depth = min(far / (far - near) * (1.0 - near / view_depth), 1.0);
    if (terrain.ground_blend >= 1.0 || depth <= terrain.ground_blend || color.a == 0.0) {
        return color;
    }
    const float horizon = smoothstep(0.0, 1.0, (depth - terrain.horizon_blend) / max(1.0 - terrain.horizon_blend, 1e-6));
    const vec4 tint = mix(terrain.fog_color, terrain.horizon_color, horizon);
    const float along = (depth - terrain.ground_blend) / (1.0 - terrain.ground_blend);
    const float strength = along * along;
    const vec3 surface = pow(color.rgb / color.a, vec3(2.2));
    const vec3 mixed = surface * (1.0 - strength * tint.a) + tint.rgb * strength;
    return vec4(pow(mixed, vec3(1.0 / 2.2)) * color.a, color.a);
}
"
    };
}
pub(crate) use crate::terrain_glsl_terrain_fog as terrain_fog;

/// The surface's and the probe's vertex prelude: the terrain's fog, the
/// height rule, and the height of the surface's vertices.
#[doc(hidden)]
#[macro_export]
macro_rules! terrain_glsl_terrain_props {
    () => {
        concat!(
            terrain_fog!(),
            ground_height!(),
            r"

// The height of the surface's vertex at `pos`, in tile units of a tile that
// reads its DEM at `dem_coords`: the ground's, or sea level at a pole.
float surface_height(const vec2 pos, const vec4 dem_coords) {
    if (pos.y < GLOBE_POLE_NORTH_Y || pos.y > GLOBE_POLE_SOUTH_Y) {
        return 0.0;
    }
    return ground_height(pos, dem_coords);
}
"
        )
    };
}
pub(crate) use crate::terrain_glsl_terrain_props as terrain_props;

/// How a hillshade shades its slopes: `hillshade`, which writes to
/// `out_color` a slope shaded by the lights of `props`, a
/// `HillshadeEvaluatedPropsUBO`. A macro, so that `concat!` puts it in the fragment stages of the
/// hillshade and of the terrain's surface, which shades a drape of a
/// hillshade's slopes.
#[doc(hidden)]
#[macro_export]
macro_rules! terrain_glsl_hillshade_shading {
    () => {
        r"
#define PI 3.141592653589793
#define STANDARD 0
#define COMBINED 1
#define IGOR 2
#define MULTIDIRECTIONAL 3
#define BASIC 4

float get_aspect(vec2 deriv) {
    return deriv.x != 0.0 ? atan(deriv.y, -deriv.x) : PI / 2.0 * (deriv.y > 0.0 ? 1.0 : -1.0);
}

// MapLibre's legacy hillshade algorithm (Method 0: STANDARD)
void standard_hillshade(vec2 deriv, const HillshadeTilePropsUBO tileProps) {
    float azimuth = props.azimuths.x + PI;
    float slope = atan(0.625 * length(deriv));
    float aspect = get_aspect(deriv);

    float intensity = tileProps.exaggeration;

    // Scale the slope exponentially based on intensity
    float base = 1.875 - intensity * 1.75;
    float maxValue = 0.5 * PI;
    float scaledSlope = abs(intensity - 0.5) > 1e-6 ? ((pow(base, slope) - 1.0) / (pow(base, maxValue) - 1.0)) * maxValue : slope;

    float accent = cos(scaledSlope);
    vec4 accent_color = (1.0 - accent) * props.accent * clamp(intensity * 2.0, 0.0, 1.0);

    float shade = abs(mod((aspect + azimuth) / PI + 0.5, 2.0) - 1.0);
    vec4 shade_color = mix(props.shadows[0], props.highlights[0], shade) * sin(scaledSlope) * clamp(intensity * 2.0, 0.0, 1.0);

    out_color = accent_color * (1.0 - shade_color.a) + shade_color;
}

// Basic directional hillshade (Method 4: BASIC)
void basic_hillshade(vec2 deriv, const HillshadeTilePropsUBO tileProps) {
    deriv = deriv * tileProps.exaggeration * 2.0;
    float azimuth = props.azimuths.x + PI;
    float cos_az = cos(azimuth);
    float sin_az = sin(azimuth);
    float cos_alt = cos(props.altitudes.x);
    float sin_alt = sin(props.altitudes.x);

    // Calculate the cosine of the angle between the light vector and the surface normal
    float cang = (sin_alt - (deriv.y * cos_az * cos_alt - deriv.x * sin_az * cos_alt)) / sqrt(1.0 + dot(deriv, deriv));
    float shade = clamp(cang, 0.0, 1.0);

    // Blend shadow and highlight based on intensity
    if (shade > 0.5) {
        out_color = props.highlights[0] * (2.0 * shade - 1.0);
    } else {
        out_color = props.shadows[0] * (1.0 - 2.0 * shade);
    }
}

// Multidirectional hillshade (Method 3: MULTIDIRECTIONAL)
void multidirectional_hillshade(vec2 deriv, const HillshadeTilePropsUBO tileProps) {
    deriv = deriv * tileProps.exaggeration * 2.0;
    vec4 total_color = vec4(0, 0, 0, 0);

    int num_lights = min(tileProps.num_lights, 4);

    // Iterate through all light sources
    for (int i = 0; i < num_lights; i++) {
        // Select the light's altitude and azimuth from the vec4 UBOs: a local
        // array indexed by the loop can live in scratch memory.
        float altitude = i == 0 ? props.altitudes.x : i == 1 ? props.altitudes.y : i == 2 ? props.altitudes.z : props.altitudes.w;
        float azimuth = i == 0 ? props.azimuths.x : i == 1 ? props.azimuths.y : i == 2 ? props.azimuths.z : props.azimuths.w;

        float cos_alt = cos(altitude);
        float sin_alt = sin(altitude);
        // Negate cos/sin azimuth for correct light direction
        float cos_az = -cos(azimuth);
        float sin_az = -sin(azimuth);

        // Calculate the cosine of the angle between the light vector and the surface normal
        float cang = (sin_alt - (deriv.y * cos_az * cos_alt - deriv.x * sin_az * cos_alt)) / sqrt(1.0 + dot(deriv, deriv));
        float shade = clamp(cang, 0.0, 1.0);

        // Accumulate shadow/highlight contribution from each light
        if (shade > 0.5) {
            total_color += props.highlights[i] * (2.0 * shade - 1.0) / float(num_lights);
        } else {
            total_color += props.shadows[i] * (1.0 - 2.0 * shade) / float(num_lights);
        }
    }

    out_color = total_color;
}

// Combined shadow and highlight method (Method 1: COMBINED)
void combined_hillshade(vec2 deriv, const HillshadeTilePropsUBO tileProps) {
    // Only supports one light source (index 0)
    deriv = deriv * tileProps.exaggeration * 2.0;
    float azimuth = props.azimuths.x + PI;
    float cos_az = cos(azimuth);
    float sin_az = sin(azimuth);
    float cos_alt = cos(props.altitudes.x);
    float sin_alt = sin(props.altitudes.x);

    // Calculate the angle between the light vector and the surface normal
    float cang = acos((sin_alt - (deriv.y * cos_az * cos_alt - deriv.x * sin_az * cos_alt)) / sqrt(1.0 + dot(deriv, deriv)));

    cang = clamp(cang, 0.0, PI / 2.0);

    // Calculate shade and highlight components from angle and slope magnitude
    float shade = cang * atan(length(deriv)) * 4.0 / PI / PI;
    float highlight = (PI / 2.0 - cang) * atan(length(deriv)) * 4.0 / PI / PI;

    out_color = props.shadows[0] * shade + props.highlights[0] * highlight;
}

// Igor's shadow/highlight method (Method 2: IGOR)
void igor_hillshade(vec2 deriv, const HillshadeTilePropsUBO tileProps) {
    // Only supports one light source (index 0)
    deriv = deriv * tileProps.exaggeration * 2.0;
    float aspect = get_aspect(deriv);
    float azimuth = props.azimuths.x + PI;

    // Slope strength is magnitude of slope vector, normalized to [0, 1]
    float slope_strength = atan(length(deriv)) * 2.0 / PI;

    // Aspect strength is difference between aspect and light azimuth, normalized to [0, 1]
    float aspect_strength = 1.0 - abs(mod((aspect + azimuth) / PI + 0.5, 2.0) - 1.0);

    float shadow_strength = slope_strength * aspect_strength;
    float highlight_strength = slope_strength * (1.0 - aspect_strength);

    out_color = props.shadows[0] * shadow_strength + props.highlights[0] * highlight_strength;
}

// Writes to `out_color` the color of the slope `deriv`, shaded as
// `tileProps` says by the lights of `props`.
void hillshade(vec2 deriv, const HillshadeTilePropsUBO tileProps) {
    if (tileProps.method == BASIC) {
        basic_hillshade(deriv, tileProps);
    } else if (tileProps.method == COMBINED) {
        combined_hillshade(deriv, tileProps);
    } else if (tileProps.method == IGOR) {
        igor_hillshade(deriv, tileProps);
    } else if (tileProps.method == MULTIDIRECTIONAL) {
        multidirectional_hillshade(deriv, tileProps);
    } else {
        standard_hillshade(deriv, tileProps);
    }
}

"
    };
}
pub(crate) use crate::terrain_glsl_hillshade_shading as hillshade_shading;

/// The terrain's surface: a tile's grid, raised to the heights of the tile's
/// DEM with its skirts lowered and projected through the tile's entry of
/// `projectionVector`, in the colors of the tile's drape. A pole's vertices
/// stand at sea level. The drape is sampled where the vertex is in the tile:
/// the back end's flip of Y puts its rows north first.
/// Where the tile's entry of `tilePropsVector` has lights, the drape holds a
/// hillshade's slopes, premultiplied by its coverage, which the surface
/// shades with them. The fog of `terrain` covers it.
pub(crate) const TERRAIN: GlslSource = GlslSource {
    vertex: concat!(
        terrain_props!(),
        r"

layout(location = 0) out vec2 frag_drape_pos;
// How far along the view, in pixels: the clip w.
layout(location = 1) out float frag_view_depth;

void main() {
    const TerrainDrawableUBO drawable = drawableVector.elements[constant.ubo_index];
    const ProjectionUBO projection = projectionVector.elements[constant.ubo_index];
    const vec2 pos = vec2(in_pos.xy);
    const float skirt = float(in_pos.z);
    const float height = surface_height(pos, drawable.dem_coords);
    gl_Position = projectTileFor3D(pos, height - skirt * terrain.skirt_length, projection);
    frag_drape_pos = clamp(pos / 8192.0, vec2(0.0), vec2(1.0));
    frag_view_depth = gl_Position.w;
}
"
    ),
    fragment: concat!(
        terrain_fog!(),
        r"

layout(location = 0) in vec2 frag_drape_pos;
layout(location = 1) in float frag_view_depth;

layout(location = 0) out vec4 out_color;

",
        hillshade_shading!(),
        r"
void main() {
    const vec4 color = texture(sampler2D(drape_texture, drape_sampler), frag_drape_pos);
    const HillshadeTilePropsUBO light = lightVector.elements[constant.ubo_index];
    if (light.num_lights == 0 || color.a == 0.0) {
        out_color = fogged(color, frag_view_depth);
        return;
    }
    // The tile's latitudes run from its north edge, where the drape's rows
    // start.
    const float latitude = mix(light.latrange.x, light.latrange.y, frag_drape_pos.y);
    const vec2 deriv = ((color.rg / color.a) * 8.0 - 4.0) / cos(radians(latitude));
    hillshade(deriv, light);
    out_color = fogged(out_color * color.a, frag_view_depth);
}
"
    ),
};

/// The ground's depth: the surface's grid, raised and projected as
/// `TERRAIN` does, writing each pixel's eye distance, an `f32`'s bits,
/// least significant byte first, as the texel's channels.
pub(crate) const TERRAIN_DEPTH: GlslSource = GlslSource {
    vertex: concat!(
        terrain_props!(),
        r"

// The eye distance, which interpolates exactly across a triangle.
layout(location = 0) out float frag_distance;

void main() {
    const TerrainDrawableUBO drawable = drawableVector.elements[constant.ubo_index];
    const ProjectionUBO projection = projectionVector.elements[constant.ubo_index];
    const vec2 pos = vec2(in_pos.xy);
    const float skirt = float(in_pos.z);
    const float height = surface_height(pos, drawable.dem_coords);
    gl_Position = projectTileFor3D(pos, height - skirt * terrain.skirt_length, projection);
    frag_distance = gl_Position.w;
}
"
    ),
    fragment: r"

layout(location = 0) in float frag_distance;

layout(location = 0) out vec4 out_color;

void main() {
    const uint bits = floatBitsToUint(frag_distance);
    const uvec4 bytes = uvec4(bits, bits >> 8u, bits >> 16u, bits >> 24u) & uvec4(255u);
    out_color = vec4(bytes) / 255.0;
}
",
};
