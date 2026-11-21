//! The programs of the `hillshade` layer type in MSL, for `mln_mtl`.

use mln_gfx::gfx_types::AttributeDataType::Short2;
use mln_gfx::shaders::program::Program;
use mln_gfx::shaders::source::msl::{Attribute, Msl, Source, Texture};

use super::programs;
use super::programs::{
    HILLSHADE_LAYER_UBO_COUNT, ID_HILLSHADE_IMAGE_TEXTURE, ID_HILLSHADE_POS_VERTEX_ATTRIBUTE,
    ID_HILLSHADE_PREPARE_TILE_PROPS_UBO, ID_HILLSHADE_TEXTURE_POS_VERTEX_ATTRIBUTE,
};

const HILLSHADE_PREPARE_UBO_COUNT: usize = ID_HILLSHADE_PREPARE_TILE_PROPS_UBO + 1;
const HILLSHADE_UBO_COUNT: usize = HILLSHADE_LAYER_UBO_COUNT;

const HILLSHADE_ATTRIBUTES: &[Attribute] = &[
    Attribute::new(0, Short2, HILLSHADE_UBO_COUNT, ID_HILLSHADE_POS_VERTEX_ATTRIBUTE),
    Attribute::new(1, Short2, HILLSHADE_UBO_COUNT, ID_HILLSHADE_TEXTURE_POS_VERTEX_ATTRIBUTE),
];
const HILLSHADE_PREPARE_ATTRIBUTES: &[Attribute] = &[
    Attribute::new(0, Short2, HILLSHADE_PREPARE_UBO_COUNT, ID_HILLSHADE_POS_VERTEX_ATTRIBUTE),
    Attribute::new(1, Short2, HILLSHADE_PREPARE_UBO_COUNT, ID_HILLSHADE_TEXTURE_POS_VERTEX_ATTRIBUTE),
];
const HILLSHADE_TEXTURES: &[Texture] = &[Texture::new(0, ID_HILLSHADE_IMAGE_TEXTURE)];
static HILLSHADE: Source = Source {
    msl: &HILLSHADE_SHADER,
    attributes: HILLSHADE_ATTRIBUTES,
    instance_attributes: &[],
    textures: HILLSHADE_TEXTURES,
};
static HILLSHADE_PREPARE: Source = Source {
    msl: &HILLSHADE_PREPARE_SHADER,
    attributes: HILLSHADE_PREPARE_ATTRIBUTES,
    instance_attributes: &[],
    textures: HILLSHADE_TEXTURES,
};

/// The source of each of the type's programs.
pub(super) static SOURCES: &[(&Program, &Source)] =
    &[(&programs::HILLSHADE, &HILLSHADE), (&programs::HILLSHADE_PREPARE, &HILLSHADE_PREPARE)];

const HILLSHADE_PREPARE_SHADER: Msl = Msl {
    vertex_main_function: r"vertexMain",
    fragment_main_function: r"fragmentMain",
    prelude: r#"

enum {
    idHillshadePrepareDrawableUBO = idDrawableReservedVertexOnlyUBO,
    idHillshadePrepareTilePropsUBO = drawableReservedUBOCount,
    hillshadePrepareUBOCount
};

struct alignas(16) HillshadePrepareDrawableUBO {
    /*  0 */ float4x4 matrix;
    /* 64 */
};
static_assert(sizeof(HillshadePrepareDrawableUBO) == 4 * 16, "wrong size");

struct alignas(16) HillshadePrepareTilePropsUBO {
    /*  0 */ float4 unpack;
    /* 16 */ float2 dimension;
    /* 24 */ float zoom;
    /* 28 */ float maxzoom;
    /* 32 */
};
static_assert(sizeof(HillshadePrepareTilePropsUBO) == 2 * 16, "wrong size");

"#,
    source: r"

struct VertexStage {
    short2 pos [[attribute(0)]];
    short2 texture_pos [[attribute(1)]];
};

struct FragmentStage {
    float4 position [[position, invariant]];
    float2 pos;
};

FragmentStage vertex vertexMain(thread const VertexStage vertx [[stage_in]],
                                device const HillshadePrepareDrawableUBO& drawable [[buffer(idHillshadePrepareDrawableUBO)]],
                                device const HillshadePrepareTilePropsUBO& tileProps [[buffer(idHillshadePrepareTilePropsUBO)]]) {

    const float4 position = drawable.matrix * float4(float2(vertx.pos), 0, 1);

    float2 epsilon = 1.0 / tileProps.dimension;
    float scale = (tileProps.dimension.x - 2.0) / tileProps.dimension.x;
    float2 pos = (float2(vertx.texture_pos) / 8192.0) * scale + epsilon;

    return {
        .position    = position,
        .pos         = pos,
    };
}

float getElevation(float2 coord, float bias, texture2d<float, access::sample> image, sampler image_sampler, float4 unpack) {
    // Convert encoded elevation value to meters
    float4 data = image.sample(image_sampler, coord) * 255.0;
    data.a = -1.0;
    return dot(data, unpack);
}

half4 fragment fragmentMain(FragmentStage in [[stage_in]],
                            device const HillshadePrepareTilePropsUBO& tileProps [[buffer(idHillshadePrepareTilePropsUBO)]],
                            texture2d<float, access::sample> image [[texture(0)]],
                            sampler image_sampler [[sampler(0)]]) {
#if defined(OVERDRAW_INSPECTOR)
    return half4(1.0);
#endif

    float2 epsilon = 1.0 / tileProps.dimension;
    float tileSize = tileProps.dimension.x - 2.0;

    // queried pixels (using Sobel operator kernel):
    // +-----------+
    // |   |   |   |
    // | a | b | c |
    // |   |   |   |
    // +-----------+
    // |   |   |   |
    // | d | e | f |
    // |   |   |   |
    // +-----------+
    // |   |   |   |
    // | g | h | i |
    // |   |   |   |
    // +-----------+
    float a = getElevation(in.pos + float2(-epsilon.x, -epsilon.y), 0.0, image, image_sampler, tileProps.unpack);
    float b = getElevation(in.pos + float2(0, -epsilon.y), 0.0, image, image_sampler, tileProps.unpack);
    float c = getElevation(in.pos + float2(epsilon.x, -epsilon.y), 0.0, image, image_sampler, tileProps.unpack);
    float d = getElevation(in.pos + float2(-epsilon.x, 0), 0.0, image, image_sampler, tileProps.unpack);
  //float e = getElevation(in.pos, 0.0, image, image_sampler, tileProps.unpack);
    float f = getElevation(in.pos + float2(epsilon.x, 0), 0.0, image, image_sampler, tileProps.unpack);
    float g = getElevation(in.pos + float2(-epsilon.x, epsilon.y), 0.0, image, image_sampler, tileProps.unpack);
    float h = getElevation(in.pos + float2(0, epsilon.y), 0.0, image, image_sampler, tileProps.unpack);
    float i = getElevation(in.pos + float2(epsilon.x, epsilon.y), 0.0, image, image_sampler, tileProps.unpack);

    // Convert the raw pixel-space derivative (slope) into world-space slope.
    // The conversion factor is: tileSize / (8 * meters_per_pixel).
    // meters_per_pixel is calculated as pow(2.0, 28.2562 - u_zoom).
    // The exaggeration factor is applied to scale the effect at lower zooms.
    float exaggerationFactor = tileProps.zoom < 2.0 ? 0.4 : tileProps.zoom < 4.5 ? 0.35 : 0.3;
    float exaggeration = tileProps.zoom < 15.0 ? (tileProps.zoom - 15.0) * exaggerationFactor : 0.0;

    float2 deriv = float2(
        (c + f + f + i) - (a + d + d + g),
        (g + h + h + i) - (a + b + b + c)
    ) * tileSize / pow(2.0, exaggeration + (28.2562 - tileProps.zoom));

    // Encode the derivative into the color channels (r and g)
    // The derivative is scaled from world-space slope to the range [0, 1] for texture storage.
    // The maximum possible world-space derivative is assumed to be 4 (hence division by 8.0).
    float4 color = clamp(float4(
        deriv.x / 8.0 + 0.5,
        deriv.y / 8.0 + 0.5,
        1.0,
        1.0), 0.0, 1.0);

    return half4(color);
}
",
};

const HILLSHADE_SHADER: Msl = Msl {
    vertex_main_function: r"vertexMain",
    fragment_main_function: r"fragmentMain",
    prelude: r#"

inline float radians(float degrees) {
    return M_PI_F * degrees / 180.0;
}

enum {
    idHillshadeDrawableUBO = idDrawableReservedVertexOnlyUBO,
    idHillshadeTilePropsUBO = idDrawableReservedFragmentOnlyUBO,
    idHillshadeEvaluatedPropsUBO = drawableReservedUBOCount,
    hillshadeUBOCount
};

struct alignas(16) HillshadeDrawableUBO {
    /*  0 */ float4x4 matrix;
    /* 64 */
};
static_assert(sizeof(HillshadeDrawableUBO) == 4 * 16, "wrong size");

struct alignas(16) HillshadeTilePropsUBO {
    /*  0 */ float2 latrange;
    /*  8 */ float exaggeration;
    /* 12 */ int32_t method;
    /* 16 */ int32_t num_lights;
    /* 20 */ float pad0;
    /* 24 */ float pad1;
    /* 28 */ float pad2;
    /* 32 */
};
static_assert(sizeof(HillshadeTilePropsUBO) == 2 * 16, "wrong size");

/// Evaluated properties that do not depend on the tile
struct alignas(16) HillshadeEvaluatedPropsUBO {
    /*  0 */ float4 accent;
    /* 16 */ float4 altitudes;       // Up to 4 altitude values (in radians)
    /* 32 */ float4 azimuths;        // Up to 4 azimuth values (in radians)
    /* 48 */ float4 shadows[4];      // Shadow colors (up to 4 lights)
    /* 112 */ float4 highlights[4];  // Highlight colors (up to 4 lights)
    /* 176 */
};
static_assert(sizeof(HillshadeEvaluatedPropsUBO) == 11 * 16, "wrong size");

"#,
    source: r"

#define PI 3.141592653589793
#define STANDARD 0
#define COMBINED 1
#define IGOR 2
#define MULTIDIRECTIONAL 3
#define BASIC 4

struct VertexStage {
    short2 pos [[attribute(0)]];
    short2 texture_pos [[attribute(1)]];
};

struct FragmentStage {
    float4 position [[position, invariant]];
    float2 pos;
};

FragmentStage vertex vertexMain(thread const VertexStage vertx [[stage_in]],
                                device const uint32_t& uboIndex [[buffer(idGlobalUBOIndex)]],
                                device const HillshadeDrawableUBO* drawableVector [[buffer(idHillshadeDrawableUBO)]],
                                device const ProjectionUBO* projectionVector [[buffer(idProjectionUBO)]]) {

    device const HillshadeDrawableUBO& drawable = drawableVector[uboIndex];

    const float4 position = projectTile(float2(vertx.pos), float2(vertx.pos), projectionVector[uboIndex]);
    float2 pos = float2(vertx.texture_pos) / 8192.0;
    // Metal's texture coordinate origin differs from some renderers;
    // restore Y-flip to match prepare pass and historical behavior.
    pos.y = 1.0 - pos.y;

    return {
        .position    = position,
        .pos         = pos,
    };
}

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

half4 fragment fragmentMain(FragmentStage in [[stage_in]],
                            device const uint32_t& uboIndex [[buffer(idGlobalUBOIndex)]],
                            device const HillshadeTilePropsUBO* tilePropsVector [[buffer(idHillshadeTilePropsUBO)]],
                            device const HillshadeEvaluatedPropsUBO& props [[buffer(idHillshadeEvaluatedPropsUBO)]],
                            texture2d<float, access::sample> image [[texture(0)]],
                            sampler image_sampler [[sampler(0)]]) {
#if defined(OVERDRAW_INSPECTOR)
    return half4(1.0);
#endif

    device const HillshadeTilePropsUBO& tileProps = tilePropsVector[uboIndex];
    thread half4 fragColor;

    float4 pixel = image.sample(image_sampler, in.pos);

    // Scale the derivative based on the mercator distortion at this latitude
    float latitude = (tileProps.latrange.x - tileProps.latrange.y) * in.pos.y + tileProps.latrange.y;
    float scaleFactor = cos(radians(latitude));

    // The derivative is scaled back from [0, 1] texture range to world-space slope
    // Texture range [0, 1] corresponds to slope range [-4, 4]
    float2 deriv = ((pixel.rg * 8.0) - 4.0) / scaleFactor;

    // Dispatch to the selected hillshade method
    if (tileProps.method == BASIC) {
        basic_hillshade(deriv, tileProps, props, fragColor);
    } else if (tileProps.method == COMBINED) {
        combined_hillshade(deriv, tileProps, props, fragColor);
    } else if (tileProps.method == IGOR) {
        igor_hillshade(deriv, tileProps, props, fragColor);
    } else if (tileProps.method == MULTIDIRECTIONAL) {
        multidirectional_hillshade(deriv, tileProps, props, fragColor);
    } else {
        // Default to STANDARD
        standard_hillshade(deriv, tileProps, props, fragColor);
    }

    return fragColor;
}
",
};
