//! The programs of the `fill-extrusion` layer type in MSL, for `mln_mtl`.

use mln_gfx::gfx_types::AttributeDataType::{Float2, Float4, Short2, UShort2, UShort4};
use mln_gfx::shaders::program::Program;
use mln_gfx::shaders::source::msl::{Attribute, Msl, Source, Texture};

use super::programs;
use super::programs::{
    FILL_EXTRUSION_LAYER_UBO_COUNT, ID_FILL_EXTRUSION_BASE_VERTEX_ATTRIBUTE, ID_FILL_EXTRUSION_COLOR_VERTEX_ATTRIBUTE,
    ID_FILL_EXTRUSION_DECIMALS_ED_ATTRIBUTE, ID_FILL_EXTRUSION_HEIGHT_VERTEX_ATTRIBUTE,
    ID_FILL_EXTRUSION_IMAGE_TEXTURE, ID_FILL_EXTRUSION_NEXT_DECIMALS_ED_ATTRIBUTE,
    ID_FILL_EXTRUSION_NEXT_OUTLINE_POS_ATTRIBUTE, ID_FILL_EXTRUSION_OUTLINE_POS_ATTRIBUTE,
    ID_FILL_EXTRUSION_PATTERN_FROM_VERTEX_ATTRIBUTE, ID_FILL_EXTRUSION_PATTERN_TO_VERTEX_ATTRIBUTE,
    ID_FILL_EXTRUSION_POS_VERTEX_ATTRIBUTE,
};

/// `wall_normal`, the normal at a wall's end from its outline vertex's
/// `decimals_ed.x`: the one the walls there share, or else `own`, the
/// wall's own.
macro_rules! wall_normal {
    () => {
        r"
float2 wall_normal(const uint bits, const float2 own) {
    if ((bits & 2u) == 0u) {
        return own;
    }
    const float angle = float(bits >> 2) * (2.0 * M_PI_F / 16384.0) - M_PI_F;
    return float2(metal::cos(angle), metal::sin(angle));
}
"
    };
}

const FILL_EXTRUSION_UBO_COUNT: usize = FILL_EXTRUSION_LAYER_UBO_COUNT;

// An instance of the instanced shaders is an outline vertex, and the vertex
// after it.
const FILL_EXTRUSION_ATTRIBUTES: &[Attribute] = &[
    Attribute::new(0, Short2, FILL_EXTRUSION_UBO_COUNT, ID_FILL_EXTRUSION_POS_VERTEX_ATTRIBUTE),
    Attribute::new(1, UShort2, FILL_EXTRUSION_UBO_COUNT, ID_FILL_EXTRUSION_DECIMALS_ED_ATTRIBUTE),
    // Data driven
    Attribute::new(2, Float4, FILL_EXTRUSION_UBO_COUNT + 1, ID_FILL_EXTRUSION_COLOR_VERTEX_ATTRIBUTE),
    Attribute::new(3, Float2, FILL_EXTRUSION_UBO_COUNT + 1, ID_FILL_EXTRUSION_BASE_VERTEX_ATTRIBUTE),
    Attribute::new(4, Float2, FILL_EXTRUSION_UBO_COUNT + 1, ID_FILL_EXTRUSION_HEIGHT_VERTEX_ATTRIBUTE),
];
const FILL_EXTRUSION_INSTANCED_ATTRIBUTES: &[Attribute] =
    &[Attribute::new(0, Short2, FILL_EXTRUSION_UBO_COUNT, ID_FILL_EXTRUSION_POS_VERTEX_ATTRIBUTE)];
const FILL_EXTRUSION_INSTANCED_INSTANCE_ATTRIBUTES: &[Attribute] = &[
    Attribute::new(1, Short2, FILL_EXTRUSION_UBO_COUNT + 1, ID_FILL_EXTRUSION_OUTLINE_POS_ATTRIBUTE),
    Attribute::new(2, UShort2, FILL_EXTRUSION_UBO_COUNT + 1, ID_FILL_EXTRUSION_DECIMALS_ED_ATTRIBUTE),
    // Data driven
    Attribute::new(3, Float4, FILL_EXTRUSION_UBO_COUNT + 2, ID_FILL_EXTRUSION_COLOR_VERTEX_ATTRIBUTE),
    Attribute::new(4, Float2, FILL_EXTRUSION_UBO_COUNT + 2, ID_FILL_EXTRUSION_BASE_VERTEX_ATTRIBUTE),
    Attribute::new(5, Float2, FILL_EXTRUSION_UBO_COUNT + 2, ID_FILL_EXTRUSION_HEIGHT_VERTEX_ATTRIBUTE),
    // The next outline vertex
    Attribute::new(6, Short2, FILL_EXTRUSION_UBO_COUNT + 3, ID_FILL_EXTRUSION_NEXT_OUTLINE_POS_ATTRIBUTE),
    Attribute::new(7, UShort2, FILL_EXTRUSION_UBO_COUNT + 3, ID_FILL_EXTRUSION_NEXT_DECIMALS_ED_ATTRIBUTE),
];
const FILL_EXTRUSION_PATTERN_ATTRIBUTES: &[Attribute] = &[
    Attribute::new(0, Short2, FILL_EXTRUSION_UBO_COUNT, ID_FILL_EXTRUSION_POS_VERTEX_ATTRIBUTE),
    Attribute::new(1, UShort2, FILL_EXTRUSION_UBO_COUNT, ID_FILL_EXTRUSION_DECIMALS_ED_ATTRIBUTE),
    // Data driven
    Attribute::new(2, Float2, FILL_EXTRUSION_UBO_COUNT + 1, ID_FILL_EXTRUSION_BASE_VERTEX_ATTRIBUTE),
    Attribute::new(3, Float2, FILL_EXTRUSION_UBO_COUNT + 1, ID_FILL_EXTRUSION_HEIGHT_VERTEX_ATTRIBUTE),
    Attribute::new(4, UShort4, FILL_EXTRUSION_UBO_COUNT + 1, ID_FILL_EXTRUSION_PATTERN_FROM_VERTEX_ATTRIBUTE),
    Attribute::new(5, UShort4, FILL_EXTRUSION_UBO_COUNT + 1, ID_FILL_EXTRUSION_PATTERN_TO_VERTEX_ATTRIBUTE),
];
const FILL_EXTRUSION_PATTERN_INSTANCED_INSTANCE_ATTRIBUTES: &[Attribute] = &[
    Attribute::new(1, Short2, FILL_EXTRUSION_UBO_COUNT + 1, ID_FILL_EXTRUSION_OUTLINE_POS_ATTRIBUTE),
    Attribute::new(2, UShort2, FILL_EXTRUSION_UBO_COUNT + 1, ID_FILL_EXTRUSION_DECIMALS_ED_ATTRIBUTE),
    // Data driven
    Attribute::new(3, Float2, FILL_EXTRUSION_UBO_COUNT + 2, ID_FILL_EXTRUSION_BASE_VERTEX_ATTRIBUTE),
    Attribute::new(4, Float2, FILL_EXTRUSION_UBO_COUNT + 2, ID_FILL_EXTRUSION_HEIGHT_VERTEX_ATTRIBUTE),
    Attribute::new(5, UShort4, FILL_EXTRUSION_UBO_COUNT + 2, ID_FILL_EXTRUSION_PATTERN_FROM_VERTEX_ATTRIBUTE),
    Attribute::new(6, UShort4, FILL_EXTRUSION_UBO_COUNT + 2, ID_FILL_EXTRUSION_PATTERN_TO_VERTEX_ATTRIBUTE),
    // The next outline vertex
    Attribute::new(7, Short2, FILL_EXTRUSION_UBO_COUNT + 3, ID_FILL_EXTRUSION_NEXT_OUTLINE_POS_ATTRIBUTE),
    Attribute::new(8, UShort2, FILL_EXTRUSION_UBO_COUNT + 3, ID_FILL_EXTRUSION_NEXT_DECIMALS_ED_ATTRIBUTE),
];
const FILL_EXTRUSION_PATTERN_TEXTURES: &[Texture] = &[Texture::new(0, ID_FILL_EXTRUSION_IMAGE_TEXTURE)];
static FILL_EXTRUSION: Source = Source {
    msl: &FILL_EXTRUSION_SHADER,
    attributes: FILL_EXTRUSION_ATTRIBUTES,
    instance_attributes: &[],
    textures: &[],
};
static FILL_EXTRUSION_INSTANCED: Source = Source {
    msl: &FILL_EXTRUSION_INSTANCED_SHADER,
    attributes: FILL_EXTRUSION_INSTANCED_ATTRIBUTES,
    instance_attributes: FILL_EXTRUSION_INSTANCED_INSTANCE_ATTRIBUTES,
    textures: &[],
};
// The pulled walls read the bucket's vertices and paint values by vertex
// index, four vertices a wall, as storage buffers
// (`ID_FILL_EXTRUSION_WALL_VERTICES` and on).
static FILL_EXTRUSION_PULLED: Source =
    Source { msl: &FILL_EXTRUSION_PULLED_SHADER, attributes: &[], instance_attributes: &[], textures: &[] };
static FILL_EXTRUSION_PATTERN: Source = Source {
    msl: &FILL_EXTRUSION_PATTERN_SHADER,
    attributes: FILL_EXTRUSION_PATTERN_ATTRIBUTES,
    instance_attributes: &[],
    textures: FILL_EXTRUSION_PATTERN_TEXTURES,
};
static FILL_EXTRUSION_PATTERN_INSTANCED: Source = Source {
    msl: &FILL_EXTRUSION_PATTERN_INSTANCED_SHADER,
    attributes: FILL_EXTRUSION_INSTANCED_ATTRIBUTES,
    instance_attributes: FILL_EXTRUSION_PATTERN_INSTANCED_INSTANCE_ATTRIBUTES,
    textures: FILL_EXTRUSION_PATTERN_TEXTURES,
};

/// The source of each of the type's programs.
pub(super) static SOURCES: &[(&Program, &Source)] = &[
    (&programs::FILL_EXTRUSION, &FILL_EXTRUSION),
    (&programs::FILL_EXTRUSION_INSTANCED, &FILL_EXTRUSION_INSTANCED),
    (&programs::FILL_EXTRUSION_PULLED, &FILL_EXTRUSION_PULLED),
    (&programs::FILL_EXTRUSION_PATTERN, &FILL_EXTRUSION_PATTERN),
    (&programs::FILL_EXTRUSION_PATTERN_INSTANCED, &FILL_EXTRUSION_PATTERN_INSTANCED),
];

// The walls' fragment stage, and `wall`, the vertex of a wall, which reads
// the instanced walls' attributes as the fields of `vertx`.
macro_rules! extrusion_wall {
    () => {
        r"
struct FragmentStage {
    float4 position [[position, invariant]];
    half4 color;
};

struct FragmentOutput {
    half4 color [[color(0)]];
    //float depth [[depth(less)]]; // Write depth value if it's less than what's already there
};

template <typename Vertex>
FragmentStage wall(thread const Vertex& vertx,
                   device const FillExtrusionDrawableUBO& drawable,
                   device const ProjectionUBO& projection,
                   device const FillExtrusionPropsUBO& props) {

    bool isDiscarded = glMod(float(vertx.decimals_ed.x), 2.0) > 0.0;
    if (isDiscarded) {
        return {
            .position = float4(0.0),
            .color    = half4(0.0),
        };
    }

#if defined(HAS_UNIFORM_u_base)
    const auto base   = max(props.light_position_base.w, 0.0);
#else
    const auto base   = max(unpack_mix_float(vertx.base, drawable.base_t), 0.0);
#endif
#if defined(HAS_UNIFORM_u_height)
    const auto height = max(props.height, 0.0);
#else
    const auto height = max(unpack_mix_float(vertx.height, drawable.height_t), 0.0);
#endif

    const float2 p1 = float2(vertx.outline_pos);
    const float2 p2 = float2(vertx.next_outline_pos);
    const float2 edgevector = normalize(p2 - p1);
    const float2 own = float2(-edgevector.y, edgevector.x);

    const float3 normal = float3(vertx.pos.x == 0 ? wall_normal(vertx.decimals_ed.x, own) : wall_normal(vertx.next_decimals_ed.x, own), 0.0);
    const float t = float(vertx.pos.y);
    const float z = (t != 0.0) ? height : base;     // TODO: This would come out wrong on GL for negative values, check it...

    const float4 position = projectTileFor3D(vertx.pos.x == 0.0 ? p1 : p2, z, projection);

#if defined(OVERDRAW_INSPECTOR)
    return {
        .position = position,
        .color    = half4(1.0),
    };
#endif

#if defined(HAS_UNIFORM_u_color)
    auto color = props.color;
#else
    auto color = unpack_mix_color(vertx.color, drawable.color_t);
#endif

    // Relative luminance (how dark/bright is the surface color?)
    const float luminance = color.r * 0.2126 + color.g * 0.7152 + color.b * 0.0722;

    float4 vcolor = float4(0.0, 0.0, 0.0, 1.0);

    // Add slight ambient lighting so no extrusions are totally black
    color += min(float4(0.03, 0.03, 0.03, 1.0), float4(1.0));

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
    const float3 light_color = props.light_color_pad.rgb;
    const float3 minLight = mix(0.0, 0.3, 1.0 - light_color.rgb);
    vcolor += float4(clamp(color.rgb * directional * light_color.rgb, minLight, 1.0), 0.0);

    return {
        .position = position,
        .color    = half4(vcolor * props.opacity),
    };
}

fragment FragmentOutput fragmentMain(FragmentStage in [[stage_in]]) {
    return { in.color/*, in.position.z*/ };
}
"
    };
}

const FILL_EXTRUSION_INSTANCED_SHADER: Msl = Msl {
    vertex_main_function: r"vertexMain",
    fragment_main_function: r"fragmentMain",
    prelude: concat!(
        wall_normal!(),
        r#"

enum {
    idFillExtrusionDrawableUBO = idDrawableReservedVertexOnlyUBO,
    idFillExtrusionTilePropsUBO = drawableReservedUBOCount,
    idFillExtrusionPropsUBO,
    fillExtrusionUBOCount
};

struct alignas(16) FillExtrusionDrawableUBO {
    /*   0 */ float4x4 matrix;
    /*  64 */ float2 pixel_coord_upper;
    /*  72 */ float2 pixel_coord_lower;
    /*  80 */ float height_factor;
    /*  84 */ float tile_ratio;

    // Interpolations
    /*  88 */ float base_t;
    /*  92 */ float height_t;
    /*  96 */ float color_t;
    /* 100 */ float pattern_from_t;
    /* 104 */ float pattern_to_t;
    /* 108 */ uint32_t paint_layout;
    /* 112 */
};
static_assert(sizeof(FillExtrusionDrawableUBO) == 7 * 16, "wrong size");

struct alignas(16) FillExtrusionTilePropsUBO {
    /*  0 */ float4 pattern_from;
    /* 16 */ float4 pattern_to;
    /* 32 */ float2 texsize;
    /* 40 */ float pad1;
    /* 44 */ float pad2;
    /* 48 */
};
static_assert(sizeof(FillExtrusionTilePropsUBO) == 3 * 16, "wrong size");

/// Evaluated properties that do not depend on the tile
struct alignas(16) FillExtrusionPropsUBO {
    /*  0 */ float4 color;
    /* 16 */ float4 light_color_pad;
    /* 32 */ float4 light_position_base;
    /* 48 */ float height;
    /* 52 */ float light_intensity;
    /* 56 */ float vertical_gradient;
    /* 60 */ float opacity;
    /* 64 */ float fade;
    /* 68 */ float from_scale;
    /* 72 */ float to_scale;
    /* 76 */ float pad2;
    /* 80 */
};
static_assert(sizeof(FillExtrusionPropsUBO) == 5 * 16, "wrong size");

"#
    ),
    source: concat!(
        r"

struct VertexStage {
    short2 pos [[attribute(0)]];
    short2 outline_pos [[attribute(1)]];
    ushort2 decimals_ed [[attribute(2)]];
    short2 next_outline_pos [[attribute(6)]];
    ushort2 next_decimals_ed [[attribute(7)]];

#if !defined(HAS_UNIFORM_u_color)
    float4 color [[attribute(3)]];
#endif
#if !defined(HAS_UNIFORM_u_base)
    float2 base [[attribute(4)]];
#endif
#if !defined(HAS_UNIFORM_u_height)
    float2 height [[attribute(5)]];
#endif
};

",
        extrusion_wall!(),
        r"
FragmentStage vertex vertexMain(thread const VertexStage vertx [[stage_in]],
                                device const uint32_t& uboIndex [[buffer(idGlobalUBOIndex)]],
                                device const FillExtrusionDrawableUBO* drawableVector [[buffer(idFillExtrusionDrawableUBO)]],
                                device const ProjectionUBO* projectionVector [[buffer(idProjectionUBO)]],
                                device const FillExtrusionPropsUBO& props [[buffer(idFillExtrusionPropsUBO)]]) {
    return wall(vertx, drawableVector[uboIndex], projectionVector[uboIndex], props);
}

"
    ),
};

const FILL_EXTRUSION_PULLED_SHADER: Msl = Msl {
    vertex_main_function: r"vertexMain",
    fragment_main_function: r"fragmentMain",
    prelude: FILL_EXTRUSION_INSTANCED_SHADER.prelude,
    source: concat!(
        r"

enum {
    idFillExtrusionWallVertices = 7,
    idFillExtrusionWallPaint
};

// A vertex of the bucket, which the walls read by index.
struct BucketVertex {
    short2 pos;
    ushort2 decimals_ed;
};

// What the instanced walls read as attributes.
struct WallVertex {
    short2 pos;
    short2 outline_pos;
    ushort2 decimals_ed;
    short2 next_outline_pos;
    ushort2 next_decimals_ed;
#if !defined(HAS_UNIFORM_u_color)
    float4 color;
#endif
#if !defined(HAS_UNIFORM_u_base)
    float2 base;
#endif
#if !defined(HAS_UNIFORM_u_height)
    float2 height;
#endif
};
",
        extrusion_wall!(),
        r"
FragmentStage vertex vertexMain(uint vertex_id [[vertex_id]],
                                device const uint32_t& uboIndex [[buffer(idGlobalUBOIndex)]],
                                device const FillExtrusionDrawableUBO* drawableVector [[buffer(idFillExtrusionDrawableUBO)]],
                                device const ProjectionUBO* projectionVector [[buffer(idProjectionUBO)]],
                                device const FillExtrusionPropsUBO& props [[buffer(idFillExtrusionPropsUBO)]],
                                device const BucketVertex* wallVertices [[buffer(idFillExtrusionWallVertices)]],
                                device const float* wallPaint [[buffer(idFillExtrusionWallPaint)]]) {
    device const FillExtrusionDrawableUBO& drawable = drawableVector[uboIndex];

    // Four vertices a wall, in the order of the static quad's corners.
    const uint wall_index = vertex_id / 4;
    const uint corner = vertex_id % 4;
    const BucketVertex outline = wallVertices[wall_index];
    const BucketVertex next = wallVertices[wall_index + 1];

    WallVertex vertx;
    vertx.pos = short2(corner < 2 ? 1 : 0, corner & 1);
    vertx.outline_pos = outline.pos;
    vertx.decimals_ed = outline.decimals_ed;
    vertx.next_outline_pos = next.pos;
    vertx.next_decimals_ed = next.decimals_ed;

    // The wall's paint values are its outline vertex's, where the drawable's
    // `paint_layout` says: the floats a vertex takes, then the offsets of
    // base, height and color, a byte each from the lowest.
    const uint paint_layout = drawable.paint_layout;
    device const float* paint = wallPaint + wall_index * (paint_layout & 0xff);
#if !defined(HAS_UNIFORM_u_base)
    vertx.base = float2(*(device const packed_float2*)(paint + ((paint_layout >> 8) & 0xff)));
#endif
#if !defined(HAS_UNIFORM_u_height)
    vertx.height = float2(*(device const packed_float2*)(paint + ((paint_layout >> 16) & 0xff)));
#endif
#if !defined(HAS_UNIFORM_u_color)
    vertx.color = float4(*(device const packed_float4*)(paint + (paint_layout >> 24)));
#endif

    return wall(vertx, drawable, projectionVector[uboIndex], props);
}
"
    ),
};

const FILL_EXTRUSION_PATTERN_INSTANCED_SHADER: Msl = Msl {
    vertex_main_function: r"vertexMain",
    fragment_main_function: r"fragmentMain",
    prelude: concat!(
        wall_normal!(),
        r#"

enum {
    idFillExtrusionDrawableUBO = idDrawableReservedVertexOnlyUBO,
    idFillExtrusionTilePropsUBO = drawableReservedUBOCount,
    idFillExtrusionPropsUBO,
    fillExtrusionUBOCount
};

struct alignas(16) FillExtrusionDrawableUBO {
    /*   0 */ float4x4 matrix;
    /*  64 */ float2 pixel_coord_upper;
    /*  72 */ float2 pixel_coord_lower;
    /*  80 */ float height_factor;
    /*  84 */ float tile_ratio;

    // Interpolations
    /*  88 */ float base_t;
    /*  92 */ float height_t;
    /*  96 */ float color_t;
    /* 100 */ float pattern_from_t;
    /* 104 */ float pattern_to_t;
    /* 108 */ uint32_t paint_layout;
    /* 112 */
};
static_assert(sizeof(FillExtrusionDrawableUBO) == 7 * 16, "wrong size");

struct alignas(16) FillExtrusionTilePropsUBO {
    /*  0 */ float4 pattern_from;
    /* 16 */ float4 pattern_to;
    /* 32 */ float2 texsize;
    /* 40 */ float pad1;
    /* 44 */ float pad2;
    /* 48 */
};
static_assert(sizeof(FillExtrusionTilePropsUBO) == 3 * 16, "wrong size");

/// Evaluated properties that do not depend on the tile
struct alignas(16) FillExtrusionPropsUBO {
    /*  0 */ float4 color;
    /* 16 */ float4 light_color_pad;
    /* 32 */ float4 light_position_base;
    /* 48 */ float height;
    /* 52 */ float light_intensity;
    /* 56 */ float vertical_gradient;
    /* 60 */ float opacity;
    /* 64 */ float fade;
    /* 68 */ float from_scale;
    /* 72 */ float to_scale;
    /* 76 */ float pad2;
    /* 80 */
};
static_assert(sizeof(FillExtrusionPropsUBO) == 5 * 16, "wrong size");

"#
    ),
    source: r"

struct VertexStage {
    short2 pos [[attribute(0)]];
    short2 outline_pos [[attribute(1)]];
    ushort2 decimals_ed [[attribute(2)]];
    short2 next_outline_pos [[attribute(7)]];
    ushort2 next_decimals_ed [[attribute(8)]];

#if !defined(HAS_UNIFORM_u_base)
    float2 base [[attribute(3)]];
#endif
#if !defined(HAS_UNIFORM_u_height)
    float2 height [[attribute(4)]];
#endif
#if !defined(HAS_UNIFORM_u_pattern_from)
    ushort4 pattern_from [[attribute(5)]];
#endif
#if !defined(HAS_UNIFORM_u_pattern_to)
    ushort4 pattern_to [[attribute(6)]];
#endif
};

struct FragmentStage {
    float4 position [[position, invariant]];
    float4 lighting;
    float2 pos_a;
    float2 pos_b;

#if !defined(HAS_UNIFORM_u_pattern_from)
    float4 pattern_from [[flat]];
#endif
#if !defined(HAS_UNIFORM_u_pattern_to)
    float4 pattern_to [[flat]];
#endif
};

struct FragmentOutput {
    half4 color [[color(0)]];
    //float depth [[depth(less)]]; // Write depth value if it's less than what's already there
};

FragmentStage vertex vertexMain(thread const VertexStage vertx [[stage_in]],
                                device const GlobalPaintParamsUBO& paintParams [[buffer(idGlobalPaintParamsUBO)]],
                                device const uint32_t& uboIndex [[buffer(idGlobalUBOIndex)]],
                                device const FillExtrusionDrawableUBO* drawableVector [[buffer(idFillExtrusionDrawableUBO)]],
                                device const ProjectionUBO* projectionVector [[buffer(idProjectionUBO)]],
                                device const FillExtrusionTilePropsUBO* tilePropsVector [[buffer(idFillExtrusionTilePropsUBO)]],
                                device const FillExtrusionPropsUBO& props [[buffer(idFillExtrusionPropsUBO)]]) {

    bool isDiscarded = glMod(float(vertx.decimals_ed.x), 2.0) > 0.0;
    if (isDiscarded) {
        return {
            .position       = float4(0.0),
            .lighting       = float4(0.0),
            .pos_a          = float2(0.0),
            .pos_b          = float2(0.0),

#if !defined(HAS_UNIFORM_u_pattern_from)
            .pattern_from   = float4(0.0),
#endif
#if !defined(HAS_UNIFORM_u_pattern_to)
            .pattern_to     = float4(0.0),
#endif
        };
    }

    device const FillExtrusionDrawableUBO& drawable = drawableVector[uboIndex];
    device const FillExtrusionTilePropsUBO& tileProps = tilePropsVector[uboIndex];

#if defined(HAS_UNIFORM_u_base)
    const auto base   = max(props.light_position_base.w, 0.0);
#else
    const auto base   = max(unpack_mix_float(vertx.base, drawable.base_t), 0.0);
#endif
#if defined(HAS_UNIFORM_u_height)
    const auto height = max(props.height, 0.0);
#else
    const auto height = max(unpack_mix_float(vertx.height, drawable.height_t), 0.0);
#endif

    const float2 p1 = float2(vertx.outline_pos);
    const float2 p2 = float2(vertx.next_outline_pos);
    const float2 edgevector = normalize(p2 - p1);
    const float2 own = float2(-edgevector.y, edgevector.x);

    const float3 normal = float3(vertx.pos.x == 0 ? wall_normal(vertx.decimals_ed.x, own) : wall_normal(vertx.next_decimals_ed.x, own), 0.0);
    const float edgedistance = vertx.pos.x == 0 ? vertx.next_decimals_ed.y : vertx.decimals_ed.y;
    const float t = float(vertx.pos.y);
    const float z = (t != 0.0) ? height : base;     // TODO: This would come out wrong on GL for negative values, check it...

    const float4 position = projectTileFor3D(vertx.pos.x == 0.0 ? p1 : p2, z, projectionVector[uboIndex]);

#if defined(OVERDRAW_INSPECTOR)
    return {
        .position       = position,
        .lighting       = float4(1.0),
        .pos_a          = float2(1.0),
        .pos_b          = float2(1.0),

#if !defined(HAS_UNIFORM_u_pattern_from)
        .pattern_from   = float4(1.0),
#endif
#if !defined(HAS_UNIFORM_u_pattern_to)
        .pattern_to     = float4(1.0),
#endif
    };
#endif

#if defined(HAS_UNIFORM_u_pattern_from)
    const auto pattern_from = tileProps.pattern_from;
#else
    const auto pattern_from = float4(vertx.pattern_from);
#endif
#if defined(HAS_UNIFORM_u_pattern_to)
    const auto pattern_to   = tileProps.pattern_to;
#else
    const auto pattern_to   = float4(vertx.pattern_to);
#endif

    const float2 pattern_tl_a = pattern_from.xy;
    const float2 pattern_br_a = pattern_from.zw;
    const float2 pattern_tl_b = pattern_to.xy;
    const float2 pattern_br_b = pattern_to.zw;

    const float pixelRatio = paintParams.pixel_ratio;
    const float tileZoomRatio = drawable.tile_ratio;
    const float fromScale = props.from_scale;
    const float toScale = props.to_scale;

    const float2 display_size_a = float2((pattern_br_a.x - pattern_tl_a.x) / pixelRatio, (pattern_br_a.y - pattern_tl_a.y) / pixelRatio);
    const float2 display_size_b = float2((pattern_br_b.x - pattern_tl_b.x) / pixelRatio, (pattern_br_b.y - pattern_tl_b.y) / pixelRatio);

    const float2 pos = float2(edgedistance, z * drawable.height_factor); // extrusion side

    float4 lighting = float4(0.0, 0.0, 0.0, 1.0);
    float directional = clamp(dot(normal, props.light_position_base.xyz), 0.0, 1.0);
    directional = mix((1.0 - props.light_intensity), max((0.5 + props.light_intensity), 1.0), directional);

    if (normal.z == 0.0) {
        // This avoids another branching statement, but multiplies by a constant of 0.84 if no vertical gradient,
        // and otherwise calculates the gradient based on base + height
        // TODO: If we're optimizing to the level of avoiding branches, we should pre-compute
        //       the square root when height is a uniform.
        const float fMin = mix(0.7, 0.98, 1.0 - props.light_intensity);
        const float factor = clamp((t + base) * pow(height / 150.0, 0.5), fMin, 1.0);
        directional *= (1.0 - props.vertical_gradient) + (props.vertical_gradient * factor);
    }

    lighting.rgb += clamp(directional * props.light_color_pad.rgb, mix(float3(0.0), float3(0.3), 1.0 - props.light_color_pad.rgb), float3(1.0));
    lighting *= props.opacity;

    return {
        .position       = position,
        .lighting       = lighting,
        .pos_a          = get_pattern_pos(drawable.pixel_coord_upper, drawable.pixel_coord_lower, fromScale * display_size_a, tileZoomRatio, pos),
        .pos_b          = get_pattern_pos(drawable.pixel_coord_upper, drawable.pixel_coord_lower, toScale * display_size_b, tileZoomRatio, pos),

#if !defined(HAS_UNIFORM_u_pattern_from)
        .pattern_from   = pattern_from,
#endif
#if !defined(HAS_UNIFORM_u_pattern_to)
        .pattern_to     = pattern_to,
#endif
    };
}

fragment FragmentOutput fragmentMain(FragmentStage in [[stage_in]],
                                     device const uint32_t& uboIndex [[buffer(idGlobalUBOIndex)]],
                                     device const FillExtrusionTilePropsUBO* tilePropsVector [[buffer(idFillExtrusionTilePropsUBO)]],
                                     device const FillExtrusionPropsUBO& props [[buffer(idFillExtrusionPropsUBO)]],
                                     texture2d<float, access::sample> image0 [[texture(0)]],
                                     sampler image0_sampler [[sampler(0)]]) {
#if defined(OVERDRAW_INSPECTOR)
    return {half4(1.0)/*, in.position.z*/};
#endif

    device const FillExtrusionTilePropsUBO& tileProps = tilePropsVector[uboIndex];

#if defined(HAS_UNIFORM_u_pattern_from)
    const auto pattern_from = float4(tileProps.pattern_from);
#else
    const auto pattern_from = float4(in.pattern_from);
#endif
#if defined(HAS_UNIFORM_u_pattern_to)
    const auto pattern_to = float4(tileProps.pattern_to);
#else
    const auto pattern_to = float4(in.pattern_to);
#endif

    const float2 pattern_tl_a = pattern_from.xy;
    const float2 pattern_br_a = pattern_from.zw;
    const float2 pattern_tl_b = pattern_to.xy;
    const float2 pattern_br_b = pattern_to.zw;

    // A crossfade at 0 or 1 shows one pattern, so skip the other.
    float4 color1 = float4(0.0);
    if (props.fade < 1.0) {
        const float2 imagecoord = glMod(in.pos_a, 1.0);
        const float2 pos = atlas_pos(pattern_tl_a / tileProps.texsize, (pattern_br_a - pattern_tl_a) / tileProps.texsize * imagecoord);
        color1 = image0.sample(image0_sampler, pos);
    }
    float4 color2 = float4(0.0);
    if (props.fade > 0.0) {
        const float2 imagecoord_b = glMod(in.pos_b, 1.0);
        const float2 pos2 = atlas_pos(pattern_tl_b / tileProps.texsize, (pattern_br_b - pattern_tl_b) / tileProps.texsize * imagecoord_b);
        color2 = image0.sample(image0_sampler, pos2);
    }

    return {half4(mix(color1, color2, props.fade) * in.lighting)/*, in.position.z*/};
}
",
};

const FILL_EXTRUSION_PATTERN_SHADER: Msl = Msl {
    vertex_main_function: r"vertexMain",
    fragment_main_function: r"fragmentMain",
    prelude: r#"

enum {
    idFillExtrusionDrawableUBO = idDrawableReservedVertexOnlyUBO,
    idFillExtrusionTilePropsUBO = drawableReservedUBOCount,
    idFillExtrusionPropsUBO,
    fillExtrusionUBOCount
};

struct alignas(16) FillExtrusionDrawableUBO {
    /*   0 */ float4x4 matrix;
    /*  64 */ float2 pixel_coord_upper;
    /*  72 */ float2 pixel_coord_lower;
    /*  80 */ float height_factor;
    /*  84 */ float tile_ratio;

    // Interpolations
    /*  88 */ float base_t;
    /*  92 */ float height_t;
    /*  96 */ float color_t;
    /* 100 */ float pattern_from_t;
    /* 104 */ float pattern_to_t;
    /* 108 */ uint32_t paint_layout;
    /* 112 */
};
static_assert(sizeof(FillExtrusionDrawableUBO) == 7 * 16, "wrong size");

struct alignas(16) FillExtrusionTilePropsUBO {
    /*  0 */ float4 pattern_from;
    /* 16 */ float4 pattern_to;
    /* 32 */ float2 texsize;
    /* 40 */ float pad1;
    /* 44 */ float pad2;
    /* 48 */
};
static_assert(sizeof(FillExtrusionTilePropsUBO) == 3 * 16, "wrong size");

/// Evaluated properties that do not depend on the tile
struct alignas(16) FillExtrusionPropsUBO {
    /*  0 */ float4 color;
    /* 16 */ float4 light_color_pad;
    /* 32 */ float4 light_position_base;
    /* 48 */ float height;
    /* 52 */ float light_intensity;
    /* 56 */ float vertical_gradient;
    /* 60 */ float opacity;
    /* 64 */ float fade;
    /* 68 */ float from_scale;
    /* 72 */ float to_scale;
    /* 76 */ float pad2;
    /* 80 */
};
static_assert(sizeof(FillExtrusionPropsUBO) == 5 * 16, "wrong size");

"#,
    source: r"

struct VertexStage {
    short2 pos [[attribute(0)]];
    ushort2 decimals_ed [[attribute(1)]];

#if !defined(HAS_UNIFORM_u_base)
    float2 base [[attribute(2)]];
#endif
#if !defined(HAS_UNIFORM_u_height)
    float2 height [[attribute(3)]];
#endif
#if !defined(HAS_UNIFORM_u_pattern_from)
    ushort4 pattern_from [[attribute(4)]];
#endif
#if !defined(HAS_UNIFORM_u_pattern_to)
    ushort4 pattern_to [[attribute(5)]];
#endif
};

struct FragmentStage {
    float4 position [[position, invariant]];
    float4 lighting;
    float2 pos_a;
    float2 pos_b;

#if !defined(HAS_UNIFORM_u_pattern_from)
    float4 pattern_from [[flat]];
#endif
#if !defined(HAS_UNIFORM_u_pattern_to)
    float4 pattern_to [[flat]];
#endif
};

struct FragmentOutput {
    half4 color [[color(0)]];
    //float depth [[depth(less)]]; // Write depth value if it's less than what's already there
};

FragmentStage vertex vertexMain(thread const VertexStage vertx [[stage_in]],
                                device const GlobalPaintParamsUBO& paintParams [[buffer(idGlobalPaintParamsUBO)]],
                                device const uint32_t& uboIndex [[buffer(idGlobalUBOIndex)]],
                                device const FillExtrusionDrawableUBO* drawableVector [[buffer(idFillExtrusionDrawableUBO)]],
                                device const ProjectionUBO* projectionVector [[buffer(idProjectionUBO)]],
                                device const FillExtrusionTilePropsUBO* tilePropsVector [[buffer(idFillExtrusionTilePropsUBO)]],
                                device const FillExtrusionPropsUBO& props [[buffer(idFillExtrusionPropsUBO)]]) {

    device const FillExtrusionDrawableUBO& drawable = drawableVector[uboIndex];
    device const FillExtrusionTilePropsUBO& tileProps = tilePropsVector[uboIndex];

#if defined(HAS_UNIFORM_u_base)
    const auto base   = max(props.light_position_base.w, 0.0);
#else
    const auto base   = max(unpack_mix_float(vertx.base, drawable.base_t), 0.0);
#endif
#if defined(HAS_UNIFORM_u_height)
    const auto height = max(props.height, 0.0);
#else
    const auto height = max(unpack_mix_float(vertx.height, drawable.height_t), 0.0);
#endif

    const float3 normal = float3(0.0, 0.0, 1.0);
    const float t = 1.0;
    const float z = (t != 0.0) ? height : base;     // TODO: This would come out wrong on GL for negative values, check it...
    const float4 position = projectTileFor3D(float2(vertx.pos), z, projectionVector[uboIndex]);

#if defined(OVERDRAW_INSPECTOR)
    return {
        .position       = position,
        .lighting       = float4(1.0),
        .pos_a          = float2(1.0),
        .pos_b          = float2(1.0),
#if !defined(HAS_UNIFORM_u_pattern_from)
        .pattern_from   = float4(1.0),
#endif
#if !defined(HAS_UNIFORM_u_pattern_to)
        .pattern_to     = float4(1.0),
#endif
    };
#endif

#if defined(HAS_UNIFORM_u_pattern_from)
    const auto pattern_from = tileProps.pattern_from;
#else
    const auto pattern_from = float4(vertx.pattern_from);
#endif
#if defined(HAS_UNIFORM_u_pattern_to)
    const auto pattern_to   = tileProps.pattern_to;
#else
    const auto pattern_to   = float4(vertx.pattern_to);
#endif

    const float2 pattern_tl_a = pattern_from.xy;
    const float2 pattern_br_a = pattern_from.zw;
    const float2 pattern_tl_b = pattern_to.xy;
    const float2 pattern_br_b = pattern_to.zw;

    const float pixelRatio = paintParams.pixel_ratio;
    const float tileZoomRatio = drawable.tile_ratio;
    const float fromScale = props.from_scale;
    const float toScale = props.to_scale;

    const float2 display_size_a = float2((pattern_br_a.x - pattern_tl_a.x) / pixelRatio, (pattern_br_a.y - pattern_tl_a.y) / pixelRatio);
    const float2 display_size_b = float2((pattern_br_b.x - pattern_tl_b.x) / pixelRatio, (pattern_br_b.y - pattern_tl_b.y) / pixelRatio);

    const float2 pos = float2(vertx.pos); // extrusion top

    float4 lighting = float4(0.0, 0.0, 0.0, 1.0);
    float directional = clamp(dot(normal, props.light_position_base.xyz), 0.0, 1.0);
    directional = mix((1.0 - props.light_intensity), max((0.5 + props.light_intensity), 1.0), directional);

    if (normal.z == 0.0) {
        // This avoids another branching statement, but multiplies by a constant of 0.84 if no vertical gradient,
        // and otherwise calculates the gradient based on base + height
        // TODO: If we're optimizing to the level of avoiding branches, we should pre-compute
        //       the square root when height is a uniform.
        const float fMin = mix(0.7, 0.98, 1.0 - props.light_intensity);
        const float factor = clamp((t + base) * pow(height / 150.0, 0.5), fMin, 1.0);
        directional *= (1.0 - props.vertical_gradient) + (props.vertical_gradient * factor);
    }

    lighting.rgb += clamp(directional * props.light_color_pad.rgb, mix(float3(0.0), float3(0.3), 1.0 - props.light_color_pad.rgb), float3(1.0));
    lighting *= props.opacity;

    return {
        .position       = position,
        .lighting       = lighting,
#if !defined(HAS_UNIFORM_u_pattern_from)
        .pattern_from   = pattern_from,
#endif
#if !defined(HAS_UNIFORM_u_pattern_to)
        .pattern_to     = pattern_to,
#endif
        .pos_a          = get_pattern_pos(drawable.pixel_coord_upper, drawable.pixel_coord_lower, fromScale * display_size_a, tileZoomRatio, pos),
        .pos_b          = get_pattern_pos(drawable.pixel_coord_upper, drawable.pixel_coord_lower, toScale * display_size_b, tileZoomRatio, pos),
    };
}

fragment FragmentOutput fragmentMain(FragmentStage in [[stage_in]],
                                     device const uint32_t& uboIndex [[buffer(idGlobalUBOIndex)]],
                                     device const FillExtrusionTilePropsUBO* tilePropsVector [[buffer(idFillExtrusionTilePropsUBO)]],
                                     device const FillExtrusionPropsUBO& props [[buffer(idFillExtrusionPropsUBO)]],
                                     texture2d<float, access::sample> image0 [[texture(0)]],
                                     sampler image0_sampler [[sampler(0)]]) {
#if defined(OVERDRAW_INSPECTOR)
    return {half4(1.0)/*, in.position.z*/};
#endif

    device const FillExtrusionTilePropsUBO& tileProps = tilePropsVector[uboIndex];

#if defined(HAS_UNIFORM_u_pattern_from)
    const auto pattern_from = float4(tileProps.pattern_from);
#else
    const auto pattern_from = float4(in.pattern_from);
#endif
#if defined(HAS_UNIFORM_u_pattern_to)
    const auto pattern_to = float4(tileProps.pattern_to);
#else
    const auto pattern_to = float4(in.pattern_to);
#endif

    const float2 pattern_tl_a = pattern_from.xy;
    const float2 pattern_br_a = pattern_from.zw;
    const float2 pattern_tl_b = pattern_to.xy;
    const float2 pattern_br_b = pattern_to.zw;

    // A crossfade at 0 or 1 shows one pattern, so skip the other.
    float4 color1 = float4(0.0);
    if (props.fade < 1.0) {
        const float2 imagecoord = glMod(in.pos_a, 1.0);
        const float2 pos = atlas_pos(pattern_tl_a / tileProps.texsize, (pattern_br_a - pattern_tl_a) / tileProps.texsize * imagecoord);
        color1 = image0.sample(image0_sampler, pos);
    }
    float4 color2 = float4(0.0);
    if (props.fade > 0.0) {
        const float2 imagecoord_b = glMod(in.pos_b, 1.0);
        const float2 pos2 = atlas_pos(pattern_tl_b / tileProps.texsize, (pattern_br_b - pattern_tl_b) / tileProps.texsize * imagecoord_b);
        color2 = image0.sample(image0_sampler, pos2);
    }

    return {half4(mix(color1, color2, props.fade) * in.lighting)/*, in.position.z*/};
}
",
};

const FILL_EXTRUSION_SHADER: Msl = Msl {
    vertex_main_function: r"vertexMain",
    fragment_main_function: r"fragmentMain",
    prelude: r#"

enum {
    idFillExtrusionDrawableUBO = idDrawableReservedVertexOnlyUBO,
    idFillExtrusionTilePropsUBO = drawableReservedUBOCount,
    idFillExtrusionPropsUBO,
    fillExtrusionUBOCount
};

struct alignas(16) FillExtrusionDrawableUBO {
    /*   0 */ float4x4 matrix;
    /*  64 */ float2 pixel_coord_upper;
    /*  72 */ float2 pixel_coord_lower;
    /*  80 */ float height_factor;
    /*  84 */ float tile_ratio;

    // Interpolations
    /*  88 */ float base_t;
    /*  92 */ float height_t;
    /*  96 */ float color_t;
    /* 100 */ float pattern_from_t;
    /* 104 */ float pattern_to_t;
    /* 108 */ uint32_t paint_layout;
    /* 112 */
};
static_assert(sizeof(FillExtrusionDrawableUBO) == 7 * 16, "wrong size");

struct alignas(16) FillExtrusionTilePropsUBO {
    /*  0 */ float4 pattern_from;
    /* 16 */ float4 pattern_to;
    /* 32 */ float2 texsize;
    /* 40 */ float pad1;
    /* 44 */ float pad2;
    /* 48 */
};
static_assert(sizeof(FillExtrusionTilePropsUBO) == 3 * 16, "wrong size");

/// Evaluated properties that do not depend on the tile
struct alignas(16) FillExtrusionPropsUBO {
    /*  0 */ float4 color;
    /* 16 */ float4 light_color_pad;
    /* 32 */ float4 light_position_base;
    /* 48 */ float height;
    /* 52 */ float light_intensity;
    /* 56 */ float vertical_gradient;
    /* 60 */ float opacity;
    /* 64 */ float fade;
    /* 68 */ float from_scale;
    /* 72 */ float to_scale;
    /* 76 */ float pad2;
    /* 80 */
};
static_assert(sizeof(FillExtrusionPropsUBO) == 5 * 16, "wrong size");

"#,
    source: r"

struct VertexStage {
    short2 pos [[attribute(0)]];
    ushort2 decimals_ed [[attribute(1)]];

#if !defined(HAS_UNIFORM_u_color)
    float4 color [[attribute(2)]];
#endif
#if !defined(HAS_UNIFORM_u_base)
    float2 base [[attribute(3)]];
#endif
#if !defined(HAS_UNIFORM_u_height)
    float2 height [[attribute(4)]];
#endif
};

struct FragmentStage {
    float4 position [[position, invariant]];
    half4 color;
};

struct FragmentOutput {
    half4 color [[color(0)]];
    //float depth [[depth(less)]]; // Write depth value if it's less than what's already there
};

FragmentStage vertex vertexMain(thread const VertexStage vertx [[stage_in]],
                                device const uint32_t& uboIndex [[buffer(idGlobalUBOIndex)]],
                                device const FillExtrusionDrawableUBO* drawableVector [[buffer(idFillExtrusionDrawableUBO)]],
                                device const ProjectionUBO* projectionVector [[buffer(idProjectionUBO)]],
                                device const FillExtrusionPropsUBO& props [[buffer(idFillExtrusionPropsUBO)]]) {

    device const FillExtrusionDrawableUBO& drawable = drawableVector[uboIndex];

#if defined(HAS_UNIFORM_u_base)
    const auto base   = max(props.light_position_base.w, 0.0);
#else
    const auto base   = max(unpack_mix_float(vertx.base, drawable.base_t), 0.0);
#endif
#if defined(HAS_UNIFORM_u_height)
    const auto height = max(props.height, 0.0);
#else
    const auto height = max(unpack_mix_float(vertx.height, drawable.height_t), 0.0);
#endif

    const float3 normal = float3(0.0, 0.0, 1.0);
    const float t = 1.0;
    const float z = (t != 0.0) ? height : base;     // TODO: This would come out wrong on GL for negative values, check it...
    const float4 position = projectTileFor3D(float2(vertx.pos), z, projectionVector[uboIndex]);

#if defined(OVERDRAW_INSPECTOR)
    return {
        .position = position,
        .color    = half4(1.0),
    };
#endif

#if defined(HAS_UNIFORM_u_color)
    auto color = props.color;
#else
    auto color = unpack_mix_color(vertx.color, drawable.color_t);
#endif

    // Relative luminance (how dark/bright is the surface color?)
    const float luminance = color.r * 0.2126 + color.g * 0.7152 + color.b * 0.0722;

    float4 vcolor = float4(0.0, 0.0, 0.0, 1.0);

    // Add slight ambient lighting so no extrusions are totally black
    color += min(float4(0.03, 0.03, 0.03, 1.0), float4(1.0));

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
    const float3 light_color = props.light_color_pad.rgb;
    const float3 minLight = mix(0.0, 0.3, 1.0 - light_color.rgb);
    vcolor += float4(clamp(color.rgb * directional * light_color.rgb, minLight, 1.0), 0.0);

    return {
        .position = position,
        .color    = half4(vcolor * props.opacity),
    };
}

fragment FragmentOutput fragmentMain(FragmentStage in [[stage_in]]) {
    return { in.color/*, in.position.z*/ };
}
",
};
