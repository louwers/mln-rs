//! The GLSL ES, for OpenGL, of the terrain's programs, and the snippets of
//! the ground that other programs include.

use mln_gfx::shaders::block::{Block, UniformBlock};
use mln_gfx::shaders::source::{GlslEsDeclarations, GlslEsSource};

use super::ubo::TerrainSurfaceUBO;

/// The terrain reads the hillshade's tile properties, which have
/// `exaggeration` too.
pub(crate) const TERRAIN_NAMES: &[(&Block, &str, &str)] =
    &[(TerrainSurfaceUBO::BLOCK, "exaggeration", "u_terrain_exaggeration")];

/// The DEM, which GL ES samples at `highp`, and the names of the terrain's
/// members.
const TERRAIN_DECLARATIONS: GlslEsDeclarations =
    GlslEsDeclarations { highp_samplers: &["ground_dem"], names: TERRAIN_NAMES, ..GlslEsDeclarations::DEFAULT };

/// What the terrain variant of a program that hides behind the ground in
/// front of it reads of the ground's depth, within `#ifdef TERRAIN` of its
/// vertex shader: whether that ground hides a point (`ground_visibility`).
macro_rules! ground_visibility {
    () => {
        r"#ifdef TERRAIN

// GL JS's `depthOpacity`: 1 where a point `w` from the eye, at `ndc`, is in
// front of the ground drawn there, fading to 0 behind it over GL JS's ramp
// of its NDC depth, which `depth_factor` scales; 1 where no ground is drawn
// there or `ndc` is outside the view. GL stores the rows from the bottom.
highp float ground_depth_opacity(highp vec2 ndc, highp float w, highp float depth_factor) {
    if (any(greaterThan(abs(ndc), vec2(1.0)))) {
        return 1.0;
    }
    ivec2 size = textureSize(u_ground_depth, 0);
    highp vec2 pos = (ndc * 0.5 + 0.5) * vec2(size);
    ivec2 texel = clamp(ivec2(floor(pos)), ivec2(0), size - 1);
    highp uvec4 bytes = uvec4(round(texelFetch(u_ground_depth, texel, 0) * 255.0));
    highp float ground = uintBitsToFloat(bytes.x | (bytes.y << 8u) | (bytes.z << 16u) | (bytes.w << 24u));
    if (!(ground > 0.0)) {
        return 1.0;
    }
    highp float d = depth_factor * (1.0 / w - 1.0 / ground) + 0.0001;
    return 1.0 - clamp(-500.0 * d, 0.0, 1.0);
}

// GL JS's `calculate_visibility` of the point at `clip`: 1 where the ground
// drawn in front doesn't hide it, 0 where it does, averaged with the point
// 0.01 NDC above it where it is partly hidden. The ramp's scale is GL JS's:
// `2n` of a near plane `n` a fiftieth of the view's height, in pixels,
// `u_world_size.y / u_pixel_ratio`.
highp float ground_visibility(highp vec4 clip, highp float view_height) {
    if (clip.w <= 0.0) {
        return 1.0;
    }
    highp vec2 ndc = clip.xy / clip.w;
    highp float depth_factor = view_height / 25.0;
    highp float visibility = ground_depth_opacity(ndc, clip.w, depth_factor);
    if (visibility > 0.95) {
        return 1.0;
    }
    return (visibility + ground_depth_opacity(ndc + vec2(0.0, 0.01), clip.w, depth_factor)) / 2.0;
}
#endif

"
    };
}
pub(crate) use ground_visibility;

/// The height rule of `crate::tile::dem`, over the DEM tile a draw reads: its
/// RGBA8 texels behind a one-texel border, decoded as the unpack weights
/// and scale say. A macro, so that `concat!` puts it in the vertex stages,
/// which read the DEM with `texelFetch`.
macro_rules! ground_height {
    () => {
        r"// The height in meters of the DEM's bordered texel at `texel`, clamped to the
// texture, decoded by `unpack` and `unpack_scale`.
highp float ground_texel_height(ivec2 texel, highp vec4 unpack, highp float unpack_scale) {
    ivec2 last = textureSize(u_ground_dem, 0) - 1;
    highp vec3 rgb = round(texelFetch(u_ground_dem, clamp(texel, ivec2(0), last), 0).rgb * 255.0);
    return (rgb.r * unpack.x + rgb.g * unpack.y + rgb.b * unpack.z + unpack.w) * unpack_scale;
}

// The height in meters, exaggerated by `exaggeration`, at `pos` in tile
// units of a tile that reads its DEM at `dem_coords`: the DEM uv's scale and
// offset, then the DEM's width in pixels, 0 for flat ground.
highp float ground_height(highp vec2 pos, highp vec4 dem_coords, highp vec4 unpack, highp float unpack_scale,
                          highp float exaggeration) {
    highp float dim = dem_coords.w;
    if (dim == 0.0) {
        return 0.0;
    }
    highp vec2 coord = (pos / 8192.0 * dem_coords.x + dem_coords.yz) * dim + 0.5;
    highp vec2 first = floor(coord);
    highp vec2 f = coord - first;
    ivec2 texel = ivec2(first);
    highp float tl = ground_texel_height(texel, unpack, unpack_scale);
    highp float tr = ground_texel_height(texel + ivec2(1, 0), unpack, unpack_scale);
    highp float bl = ground_texel_height(texel + ivec2(0, 1), unpack, unpack_scale);
    highp float br = ground_texel_height(texel + ivec2(1, 1), unpack, unpack_scale);
    highp float top = tl * (1.0 - f.x) + tr * f.x;
    highp float bottom = bl * (1.0 - f.x) + br * f.x;
    return (top * (1.0 - f.y) + bottom * f.y) * exaggeration;
}

"
    };
}
#[cfg(feature = "gpu-conformance")]
pub(crate) use ground_height;

/// How a hillshade shades its slopes: `hillshade`, which writes to
/// `fragColor` a slope shaded by the lights of `HillshadeEvaluatedPropsUBO`.
/// A macro, so that `concat!` puts it in the fragment stages of the
/// hillshade and of the terrain's surface, which shades a drape of a
/// hillshade's slopes.
macro_rules! hillshade_shading {
    () => {
        r#"#define PI 3.141592653589793

#define STANDARD 0
#define COMBINED 1
#define IGOR 2
#define MULTIDIRECTIONAL 3
#define BASIC 4

float get_aspect(vec2 deriv) {
    return deriv.x != 0.0 ?
    atan(deriv.y, -deriv.x) : PI / 2.0 * (deriv.y > 0.0 ? 1.0 : -1.0);
}

// MapLibre's legacy hillshade algorithm (Method 0)
void standard_hillshade(vec2 deriv) {
    float azimuth = u_azimuths.x + PI;
    float slope = atan(0.625 * length(deriv));
    float aspect = get_aspect(deriv);

    // Note: This implementation uses u_exaggeration as intensity, though it typically should be derived from u_altitudes.x.
    float intensity = u_exaggeration;

    // Scale the slope exponentially based on intensity
    float base = 1.875 - intensity * 1.75;
    float maxValue = 0.5 * PI;
    float scaledSlope = abs(intensity - 0.5) > 1e-6 ? ((pow(base, slope) - 1.0) / (pow(base, maxValue) - 1.0)) * maxValue : slope;

    float accent = cos(scaledSlope);
    vec4 accent_color = (1.0 - accent) * u_accent * clamp(intensity * 2.0, 0.0, 1.0);

    // Calculate shadow/highlight based on aspect
    float shade = abs(mod((aspect + azimuth) / PI + 0.5, 2.0) - 1.0);
    vec4 shade_color = mix(u_shadows[0], u_highlights[0], shade) * sin(scaledSlope) * clamp(intensity * 2.0, 0.0, 1.0);

    fragColor = accent_color * (1.0 - shade_color.a) + shade_color;
}

// Basic directional hillshade (Method 4)
void basic_hillshade(vec2 deriv) {
    deriv = deriv * u_exaggeration * 2.0; // Exaggerate the slope derivative
    float azimuth = u_azimuths.x + PI;
    float cos_az = cos(azimuth);
    float sin_az = sin(azimuth);
    float cos_alt = cos(u_altitudes.x);
    float sin_alt = sin(u_altitudes.x);

    // Calculate the cosine of the angle between the light vector and the surface normal
    float cang = (sin_alt - (deriv.y * cos_az * cos_alt - deriv.x * sin_az * cos_alt)) / sqrt(1.0 + dot(deriv, deriv));
    float shade = clamp(cang, 0.0, 1.0); // cang is the hillshade intensity [0, 1]

    // Blend shadow and highlight based on intensity
    if (shade > 0.5) {
        fragColor = u_highlights[0] * (2.0 * shade - 1.0); // Highlight strength [0, 1]
    } else {
        fragColor = u_shadows[0] * (1.0 - 2.0 * shade);    // Shadow strength [0, 1]
    }
}

// Multidirectional hillshade (Method 3)
void multidirectional_hillshade(vec2 deriv) {
    deriv = deriv * u_exaggeration * 2.0; // Exaggerate the slope derivative
    fragColor = vec4(0, 0, 0, 0);

    // Iterate through all light sources (up to u_num_lights, max 4)
    for (int i = 0; i < 4; i++) {
        if (i >= u_num_lights) break;

        // Access altitude and azimuth from vec4 UBOs
        float altitude = (i == 0) ? u_altitudes.x : (i == 1) ? u_altitudes.y : (i == 2) ? u_altitudes.z : u_altitudes.w;
        float azimuth = (i == 0) ? u_azimuths.x : (i == 1) ? u_azimuths.y : (i == 2) ? u_azimuths.z : u_azimuths.w;

        float cos_alt = cos(altitude);
        float sin_alt = sin(altitude);
        // Negate cos/sin azimuth for correct light direction in the normal calculation
        float cos_az = -cos(azimuth);
        float sin_az = -sin(azimuth);

        // Calculate the cosine of the angle between the light vector and the surface normal
        float cang = (sin_alt - (deriv.y * cos_az * cos_alt - deriv.x * sin_az * cos_alt)) /
                     sqrt(1.0 + dot(deriv, deriv));
        float shade = clamp(cang, 0.0, 1.0); // cang is the hillshade intensity [0, 1]

        // Accumulate shadow/highlight contribution from each light
        if (shade > 0.5) {
            fragColor += u_highlights[i] * (2.0 * shade - 1.0) / float(u_num_lights);
        } else {
            fragColor += u_shadows[i] * (1.0 - 2.0 * shade) / float(u_num_lights);
        }
    }
}

// Combined shadow and highlight method (Method 1)
void combined_hillshade(vec2 deriv) {
    // Only supports one light source (index 0)
    deriv = deriv * u_exaggeration * 2.0;
    float azimuth = u_azimuths.x + PI;
    float cos_az = cos(azimuth);
    float sin_az = sin(azimuth);
    float cos_alt = cos(u_altitudes.x);
    float sin_alt = sin(u_altitudes.x);

    // Calculate the angle between the light vector and the surface normal
    float cang = acos((sin_alt - (deriv.y * cos_az * cos_alt - deriv.x * sin_az * cos_alt)) /
                      sqrt(1.0 + dot(deriv, deriv)));

    cang = clamp(cang, 0.0, PI / 2.0); // Clamp angle to 90 degrees (half hemisphere)

    // The "shade" and "highlight" components are calculated from cang (angle) and the magnitude of the slope
    float shade = cang * atan(length(deriv)) * 4.0 / PI / PI;
    float highlight = (PI / 2.0 - cang) * atan(length(deriv)) * 4.0 / PI / PI;

    fragColor = u_shadows[0] * shade + u_highlights[0] * highlight;
}

// Igor's shadow/highlight method (Method 2)
void igor_hillshade(vec2 deriv) {
    // Only supports one light source (index 0)
    deriv = deriv * u_exaggeration * 2.0;
    float aspect = get_aspect(deriv);
    float azimuth = u_azimuths.x + PI;

    // Slope strength is magnitude of slope vector, normalized to [0, 1]
    float slope_strength = atan(length(deriv)) * 2.0 / PI;

    // Aspect strength is difference between aspect and light azimuth, normalized to [0, 1]
    float aspect_strength = 1.0 - abs(mod((aspect + azimuth) / PI + 0.5, 2.0) - 1.0);

    float shadow_strength = slope_strength * aspect_strength;
    float highlight_strength = slope_strength * (1.0 - aspect_strength);

    fragColor = u_shadows[0] * shadow_strength + u_highlights[0] * highlight_strength;
}

// Writes to `fragColor` the color of the slope `deriv`, shaded as
// `u_method` says by the lights of `HillshadeEvaluatedPropsUBO`.
void hillshade(vec2 deriv) {
    switch (u_method) {
        case BASIC:
            basic_hillshade(deriv);
            break;
        case COMBINED:
            combined_hillshade(deriv);
            break;
        case IGOR:
            igor_hillshade(deriv);
            break;
        case MULTIDIRECTIONAL:
            multidirectional_hillshade(deriv);
            break;
        case STANDARD:
        default:
            standard_hillshade(deriv);
            break;
    }
}

"#
    };
}
pub(crate) use hillshade_shading;

/// The surface's and the probe's vertex prelude: the height rule, and the
/// height of the surface's vertices.
macro_rules! terrain_props {
    () => {
        concat!(
            ground_height!(),
            r"// The height of the surface's vertex at `pos`, in tile units of a tile
// that reads its DEM at `dem_coords`: the ground's, or sea level at a pole.
highp float surface_height(highp vec2 pos, highp vec4 dem_coords) {
    if (pos.y < GLOBE_POLE_NORTH_Y || pos.y > GLOBE_POLE_SOUTH_Y) {
        return 0.0;
    }
    return ground_height(pos, dem_coords, u_unpack, u_unpack_scale, u_terrain_exaggeration);
}

"
        )
    };
}
#[cfg(feature = "gpu-conformance")]
pub(crate) use terrain_props;

/// The terrain's surface: a tile's grid, raised to the heights of the tile's
/// DEM with its skirts lowered and projected through the tile's projection
/// block, in the colors of the tile's drape. A pole's vertices stand at sea
/// level. GL stores the drape's rows bottom first, so the drape is sampled
/// at `1 - v`.
/// Where the tile's `HillshadeTilePropsUBO` has lights, the drape holds a
/// hillshade's slopes, premultiplied by its coverage, which the surface
/// shades with them. The fog of `TerrainSurfaceUBO` covers it.
pub(crate) const TERRAIN: GlslEsSource = GlslEsSource {
    vertex: concat!(
        terrain_props!(),
        r"out vec2 v_drape_pos;
// How far along the view, in pixels: the clip w.
out highp float v_view_depth;

void main() {
    float height = surface_height(a_pos.xy, u_dem_coords);
    gl_Position = projectTileFor3D(a_pos.xy, height - a_pos.z * u_skirt_length);
    vec2 drape_pos = clamp(a_pos.xy / 8192.0, 0.0, 1.0);
    v_drape_pos = vec2(drape_pos.x, 1.0 - drape_pos.y);
    v_view_depth = gl_Position.w;
}
"
    ),
    fragment: concat!(
        hillshade_shading!(),
        r"in vec2 v_drape_pos;
in highp float v_view_depth;

// `color`, premultiplied, of the surface `view_depth` pixels along the
// view, covered by the fog, as GL JS's terrain is: mixed in linear
// components toward the fog's color by the square of how far past the
// ground blend the fog's depth is. The fog's alpha weighs its color; the
// coverage stays.
highp vec4 fogged(highp vec4 color, highp float view_depth) {
    highp float near = u_depth_range.x;
    highp float far = u_depth_range.y;
    highp float depth = min(far / (far - near) * (1.0 - near / view_depth), 1.0);
    if (u_ground_blend >= 1.0 || depth <= u_ground_blend || color.a == 0.0) {
        return color;
    }
    highp float horizon = smoothstep(0.0, 1.0, (depth - u_horizon_blend) / max(1.0 - u_horizon_blend, 1e-6));
    highp vec4 tint = mix(u_fog_color, u_horizon_color, horizon);
    highp float along = (depth - u_ground_blend) / (1.0 - u_ground_blend);
    highp float strength = along * along;
    highp vec3 surface = pow(color.rgb / color.a, vec3(2.2));
    highp vec3 mixed = surface * (1.0 - strength * tint.a) + tint.rgb * strength;
    return vec4(pow(mixed, vec3(1.0 / 2.2)) * color.a, color.a);
}

void main() {
    vec4 color = texture(u_drape, v_drape_pos);
    if (u_num_lights == 0 || color.a == 0.0) {
        fragColor = fogged(color, v_view_depth);
    } else {
        // The tile's latitudes run from its north edge, where the drape's
        // rows start, at a `v` of 1.
        float latitude = mix(u_latrange[0], u_latrange[1], 1.0 - v_drape_pos.y);
        vec2 deriv = ((color.rg / color.a) * 8.0 - 4.0) / cos(radians(latitude));
        hillshade(deriv);
        fragColor = fogged(fragColor * color.a, v_view_depth);
    }
}
"
    ),
    declarations: &TERRAIN_DECLARATIONS,
};

/// The ground's depth: the surface's grid, raised and projected as
/// `TERRAIN` does, writing each pixel's eye distance, an `f32`'s bits,
/// least significant byte first, as the texel's channels.
pub(crate) const TERRAIN_DEPTH: GlslEsSource = GlslEsSource {
    vertex: concat!(
        terrain_props!(),
        r"// The eye distance, which interpolates exactly across a triangle.
out highp float v_distance;

void main() {
    float height = surface_height(a_pos.xy, u_dem_coords);
    gl_Position = projectTileFor3D(a_pos.xy, height - a_pos.z * u_skirt_length);
    v_distance = gl_Position.w;
}
"
    ),
    fragment: r"in highp float v_distance;

void main() {
    highp uint bits = floatBitsToUint(v_distance);
    highp uvec4 bytes = uvec4(bits, bits >> 8u, bits >> 16u, bits >> 24u) & uvec4(255u);
    fragColor = vec4(bytes) / 255.0;
}
",
    declarations: &TERRAIN_DECLARATIONS,
};
