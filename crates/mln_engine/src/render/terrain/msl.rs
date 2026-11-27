//! The MSL of the terrain's programs, and the snippets of the ground that
//! other programs include.

use mln_gfx::shaders::source::MslSource;

/// What the terrain variant of a program that hides behind the ground in
/// front of it reads of the ground's depth, within `#if defined(TERRAIN)`:
/// whether that ground hides a point (`ground_visibility`). Its vertex
/// function reads the ground's depth from the program's `ground_depth`.
#[doc(hidden)]
#[macro_export]
macro_rules! terrain_msl_ground_visibility {
    () => {
        r#"
#if defined(TERRAIN)
// GL JS's `depthOpacity`: 1 where a point `w` from the eye, at `ndc`, is in
// front of the ground drawn there, fading to 0 behind it over GL JS's ramp
// of its NDC depth, which `depth_factor` scales; 1 where no ground is drawn
// there or `ndc` is outside the view. The texels' rows run from the top.
static float ground_depth_opacity(float2 ndc, float w, float depth_factor, texture2d<float, access::read> depth) {
    if (any(abs(ndc) > float2(1.0))) {
        return 1.0;
    }
    const int2 size = int2(int(depth.get_width()), int(depth.get_height()));
    const float2 pos = (float2(ndc.x, -ndc.y) * 0.5 + 0.5) * float2(size);
    const int2 texel = clamp(int2(floor(pos)), int2(0), size - 1);
    const uint4 bytes = uint4(round(depth.read(uint2(texel)) * 255.0));
    const float ground = as_type<float>(bytes.x | (bytes.y << 8) | (bytes.z << 16) | (bytes.w << 24));
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
static float ground_visibility(float4 clip, float view_height, texture2d<float, access::read> depth) {
    if (clip.w <= 0.0) {
        return 1.0;
    }
    const float2 ndc = clip.xy / clip.w;
    const float depth_factor = view_height / 25.0;
    const float visibility = ground_depth_opacity(ndc, clip.w, depth_factor, depth);
    if (visibility > 0.95) {
        return 1.0;
    }
    return (visibility + ground_depth_opacity(ndc + float2(0.0, 0.01), clip.w, depth_factor, depth)) / 2.0;
}
#endif
"#
    };
}
pub(crate) use crate::terrain_msl_ground_visibility as ground_visibility;

/// The height rule of `crate::tile::dem`, over the DEM tile a draw reads: its
/// RGBA8 texels behind a one-texel border, decoded by a `TerrainSurfaceUBO`.
/// A macro, so that `concat!` puts it in the sources of the programs that
/// read the DEM.
#[doc(hidden)]
#[macro_export]
macro_rules! terrain_msl_ground_height {
    () => {
        r#"

// The height in meters of the DEM's bordered texel at `texel`, clamped to the
// texture.
static float ground_texel_height(texture2d<float, access::read> dem, int2 texel,
                                 device const TerrainSurfaceUBO& props) {
    const int2 last = int2(int(dem.get_width()), int(dem.get_height())) - 1;
    const float3 rgb = round(dem.read(uint2(clamp(texel, int2(0), last))).rgb * 255.0);
    return (rgb.r * props.unpack.x + rgb.g * props.unpack.y + rgb.b * props.unpack.z + props.unpack.w) *
        props.unpack_scale;
}

// The height in meters, exaggerated, at `pos` in tile units of a tile that
// reads its DEM at `dem_coords`: the DEM uv's scale and offset, then the DEM's
// width in pixels, 0 for flat ground.
static float ground_height(float2 pos, float4 dem_coords, texture2d<float, access::read> dem,
                           device const TerrainSurfaceUBO& props) {
    const float dim = dem_coords.w;
    if (dim == 0.0) {
        return 0.0;
    }
    const float2 coord = (pos / 8192.0 * dem_coords.x + dem_coords.yz) * dim + 0.5;
    const float2 first = floor(coord);
    const float2 f = coord - first;
    const int2 texel = int2(first);
    const float tl = ground_texel_height(dem, texel, props);
    const float tr = ground_texel_height(dem, texel + int2(1, 0), props);
    const float bl = ground_texel_height(dem, texel + int2(0, 1), props);
    const float br = ground_texel_height(dem, texel + int2(1, 1), props);
    const float top = tl * (1.0 - f.x) + tr * f.x;
    const float bottom = bl * (1.0 - f.x) + br * f.x;
    return (top * (1.0 - f.y) + bottom * f.y) * props.exaggeration;
}

"#
    };
}
pub(crate) use crate::terrain_msl_ground_height as ground_height;

/// How a hillshade shades its slopes: `hillshade_color`, which shades a
/// slope by the lights of a `HillshadeEvaluatedPropsUBO`. A macro, so that
/// `concat!` puts it in the sources of the hillshade and of the terrain's
/// surface.
#[doc(hidden)]
#[macro_export]
macro_rules! terrain_msl_hillshade_shading {
    () => {
        r"
#define PI 3.141592653589793
#define STANDARD 0
#define COMBINED 1
#define IGOR 2
#define MULTIDIRECTIONAL 3
#define BASIC 4

// Helper function to calculate aspect (normalized direction of slope)
float get_aspect(float2 deriv) {
    return deriv.x != 0.0 ? atan2(deriv.y, -deriv.x) : PI / 2.0 * (deriv.y > 0.0 ? 1.0 : -1.0);
}

// MapLibre's legacy hillshade algorithm (Method 0: STANDARD)
void standard_hillshade(float2 deriv, device const HillshadeTilePropsUBO& tileProps, device const HillshadeEvaluatedPropsUBO& props, thread half4& fragColor) {
    float azimuth = props.azimuths.x + PI;
    float slope = atan(0.625 * length(deriv));
    float aspect = get_aspect(deriv);

    float intensity = tileProps.exaggeration;

    // Scale the slope exponentially based on intensity
    float base = 1.875 - intensity * 1.75;
    float maxValue = 0.5 * PI;
    float scaledSlope = abs(intensity - 0.5) > 1e-6 ? ((pow(base, slope) - 1.0) / (pow(base, maxValue) - 1.0)) * maxValue : slope;

    float accent = cos(scaledSlope);
    float4 accent_color = (1.0 - accent) * props.accent * clamp(intensity * 2.0, 0.0, 1.0);

    float shade = abs(glMod((aspect + azimuth) / PI + 0.5, 2.0) - 1.0);
    float4 shade_color = mix(props.shadows[0], props.highlights[0], shade) * sin(scaledSlope) * clamp(intensity * 2.0, 0.0, 1.0);

    fragColor = half4(accent_color * (1.0 - shade_color.a) + shade_color);
}

// Basic directional hillshade (Method 4: BASIC)
void basic_hillshade(float2 deriv, device const HillshadeTilePropsUBO& tileProps, device const HillshadeEvaluatedPropsUBO& props, thread half4& fragColor) {
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
        fragColor = half4(props.highlights[0] * (2.0 * shade - 1.0));
    } else {
        fragColor = half4(props.shadows[0] * (1.0 - 2.0 * shade));
    }
}

// Multidirectional hillshade (Method 3: MULTIDIRECTIONAL)
void multidirectional_hillshade(float2 deriv, device const HillshadeTilePropsUBO& tileProps, device const HillshadeEvaluatedPropsUBO& props, thread half4& fragColor) {
    deriv = deriv * tileProps.exaggeration * 2.0;
    float4 total_color = float4(0, 0, 0, 0);

    int num_lights = min(tileProps.num_lights, 4);

    // Iterate through all light sources
    for (int i = 0; i < num_lights; i++) {
        // Access altitude and azimuth from vec4 UBOs
        float altitude = (i == 0) ? props.altitudes.x : (i == 1) ? props.altitudes.y : (i == 2) ? props.altitudes.z : props.altitudes.w;
        float azimuth = (i == 0) ? props.azimuths.x : (i == 1) ? props.azimuths.y : (i == 2) ? props.azimuths.z : props.azimuths.w;

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

    fragColor = half4(total_color);
}

// Combined shadow and highlight method (Method 1: COMBINED)
void combined_hillshade(float2 deriv, device const HillshadeTilePropsUBO& tileProps, device const HillshadeEvaluatedPropsUBO& props, thread half4& fragColor) {
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

    fragColor = half4(props.shadows[0] * shade + props.highlights[0] * highlight);
}

// Igor's shadow/highlight method (Method 2: IGOR)
void igor_hillshade(float2 deriv, device const HillshadeTilePropsUBO& tileProps, device const HillshadeEvaluatedPropsUBO& props, thread half4& fragColor) {
    // Only supports one light source (index 0)
    deriv = deriv * tileProps.exaggeration * 2.0;
    float aspect = get_aspect(deriv);
    float azimuth = props.azimuths.x + PI;

    // Slope strength is magnitude of slope vector, normalized to [0, 1]
    float slope_strength = atan(length(deriv)) * 2.0 / PI;

    // Aspect strength is difference between aspect and light azimuth, normalized to [0, 1]
    float aspect_strength = 1.0 - abs(glMod((aspect + azimuth) / PI + 0.5, 2.0) - 1.0);

    float shadow_strength = slope_strength * aspect_strength;
    float highlight_strength = slope_strength * (1.0 - aspect_strength);

    fragColor = half4(props.shadows[0] * shadow_strength + props.highlights[0] * highlight_strength);
}

// The color of the slope `deriv`, shaded as `tileProps` says by the lights
// of `props`.
static half4 hillshade_color(float2 deriv, device const HillshadeTilePropsUBO& tileProps,
                             device const HillshadeEvaluatedPropsUBO& props) {
    thread half4 fragColor;
    if (tileProps.method == BASIC) {
        basic_hillshade(deriv, tileProps, props, fragColor);
    } else if (tileProps.method == COMBINED) {
        combined_hillshade(deriv, tileProps, props, fragColor);
    } else if (tileProps.method == IGOR) {
        igor_hillshade(deriv, tileProps, props, fragColor);
    } else if (tileProps.method == MULTIDIRECTIONAL) {
        multidirectional_hillshade(deriv, tileProps, props, fragColor);
    } else {
        standard_hillshade(deriv, tileProps, props, fragColor);
    }
    return fragColor;
}
"
    };
}
pub(crate) use crate::terrain_msl_hillshade_shading as hillshade_shading;

/// The terrain's surface: a tile's grid, raised to the heights of the tile's
/// DEM with its skirts lowered and projected through the tile's entry of
/// `projectionVector`, in the colors of the tile's drape. A pole's vertices
/// stand at sea level. Rows of a pass's texture are stored north first, as
/// the drape is read.
/// Where the tile's entry of `lightVector` has lights, the drape holds a
/// hillshade's slopes, premultiplied by its coverage, which the surface
/// shades with them. The fog of its `TerrainSurfaceUBO` covers it.
pub(crate) const TERRAIN: MslSource = MslSource {
    vertex_main_function: r"vertexMain",
    fragment_main_function: r"fragmentMain",
    source: concat!(
        ground_height!(),
        hillshade_shading!(),
        r"
// The height of the surface's vertex at `pos`, in tile units of a tile that
// reads its DEM at `dem_coords`: the ground's, or sea level at a pole.
static float surface_height(float2 pos, float4 dem_coords, texture2d<float, access::read> dem,
                            device const TerrainSurfaceUBO& props) {
    if (pos.y < GLOBE_POLE_NORTH_Y || pos.y > GLOBE_POLE_SOUTH_Y) {
        return 0.0;
    }
    return ground_height(pos, dem_coords, dem, props);
}

// `color`, premultiplied, of the surface `view_depth` pixels along the
// view, covered by `fog`, as GL JS's terrain is: mixed in linear components
// toward the fog's color by the square of how far past the ground blend the
// fog's depth is. The fog's alpha weighs its color; the coverage stays.
static float4 fogged(float4 color, float view_depth, device const TerrainSurfaceUBO& fog) {
    const float near = fog.depth_range.x;
    const float far = fog.depth_range.y;
    const float depth = min(far / (far - near) * (1.0 - near / view_depth), 1.0);
    if (fog.ground_blend >= 1.0 || depth <= fog.ground_blend || color.a == 0.0) {
        return color;
    }
    const float horizon = smoothstep(0.0, 1.0, (depth - fog.horizon_blend) / max(1.0 - fog.horizon_blend, 1e-6));
    const float4 tint = mix(fog.fog_color, fog.horizon_color, horizon);
    const float along = (depth - fog.ground_blend) / (1.0 - fog.ground_blend);
    const float strength = along * along;
    const float3 surface = pow(color.rgb / color.a, float3(2.2));
    const float3 mixed = surface * (1.0 - strength * tint.a) + tint.rgb * strength;
    return float4(pow(mixed, float3(1.0 / 2.2)) * color.a, color.a);
}

struct FragmentStage {
    float4 position [[position, invariant]];
    float2 drape_pos;
    // How far along the view, in pixels: the clip w.
    float view_depth;
};

FragmentStage vertex vertexMain(thread const VertexStage vertx [[stage_in]] MLN_VERTEX_BINDINGS) {
    device const TerrainDrawableUBO& drawable = drawableVector[uboIndex];
    device const ProjectionUBO& projection = projectionVector[uboIndex];
    const float2 pos = float2(vertx.pos.xy);
    const float skirt = float(vertx.pos.z);
    const float height = surface_height(pos, drawable.dem_coords, ground_dem, terrain);
    const float4 position = projectTileFor3D(pos, height - skirt * terrain.skirt_length, projection);
    return {
        .position = position,
        .drape_pos = clamp(pos / 8192.0, float2(0.0), float2(1.0)),
        .view_depth = position.w,
    };
}

half4 fragment fragmentMain(FragmentStage in [[stage_in]] MLN_FRAGMENT_BINDINGS) {
    const float4 color = drape.sample(drape_sampler, in.drape_pos);
    device const HillshadeTilePropsUBO& light = lightVector[uboIndex];
    if (light.num_lights == 0 || color.a == 0.0) {
        return half4(fogged(color, in.view_depth, terrain));
    }
    // The tile's latitudes run from its north edge, where the drape's rows
    // start.
    const float latitude = mix(light.latrange.x, light.latrange.y, in.drape_pos.y);
    const float2 deriv = ((color.rg / color.a) * 8.0 - 4.0) / cos(radians(latitude));
    const float4 shaded = float4(hillshade_color(deriv, light, props)) * color.a;
    return half4(fogged(shaded, in.view_depth, terrain));
}
"
    ),
    // Fast math would let the compiler fuse or reorder the f32 operations of
    // the terrain's height rule, which the CPU does one at a time, in order
    // (`crate::tile::dem`).
    fast_math: false,
};

/// The ground's depth: the surface's grid, raised and projected as
/// `TERRAIN` does, writing each pixel's eye distance, an `f32`'s bits,
/// least significant byte first, as the texel's channels.
pub(crate) const TERRAIN_DEPTH: MslSource = MslSource {
    vertex_main_function: r"vertexMain",
    fragment_main_function: r"fragmentMain",
    source: concat!(
        ground_height!(),
        r"
// The height of the surface's vertex at `pos`, in tile units of a tile that
// reads its DEM at `dem_coords`: the ground's, or sea level at a pole.
static float surface_height(float2 pos, float4 dem_coords, texture2d<float, access::read> dem,
                            device const TerrainSurfaceUBO& props) {
    if (pos.y < GLOBE_POLE_NORTH_Y || pos.y > GLOBE_POLE_SOUTH_Y) {
        return 0.0;
    }
    return ground_height(pos, dem_coords, dem, props);
}

struct FragmentStage {
    float4 position [[position, invariant]];
    // The eye distance, which interpolates exactly across a triangle.
    float distance;
};

FragmentStage vertex vertexMain(thread const VertexStage vertx [[stage_in]] MLN_VERTEX_BINDINGS) {
    device const TerrainDrawableUBO& drawable = drawableVector[uboIndex];
    device const ProjectionUBO& projection = projectionVector[uboIndex];
    const float2 pos = float2(vertx.pos.xy);
    const float skirt = float(vertx.pos.z);
    const float height = surface_height(pos, drawable.dem_coords, ground_dem, terrain);
    const float4 position = projectTileFor3D(pos, height - skirt * terrain.skirt_length, projection);
    return {
        .position = position,
        .distance = position.w,
    };
}

float4 fragment fragmentMain(FragmentStage in [[stage_in]] MLN_FRAGMENT_BINDINGS) {
    const uint bits = as_type<uint>(in.distance);
    const uint4 bytes = uint4(bits, bits >> 8, bits >> 16, bits >> 24) & uint4(255);
    return float4(bytes) / 255.0;
}
"
    ),
    // Fast math would let the compiler fuse or reorder the f32 operations of
    // the terrain's height rule, which the CPU does one at a time, in order
    // (`crate::tile::dem`).
    fast_math: false,
};
