//! The programs of the `fill-extrusion` layer type in MSL, for `mln_mtl`.

use mln_gfx::shaders::source::msl::Msl;

use crate::shaders::{atlas_pos, pattern_pos};

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

// The walls' fragment stage, and `wall`, the vertex of a wall, which reads
// the instanced walls' attributes as the fields of `vertex_in`.
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
FragmentStage wall(thread const Vertex& vertex_in,
                   device const FillExtrusionDrawableUBO& drawable,
                   device const ProjectionUBO& projection,
                   device const FillExtrusionPropsUBO& props) {

    bool isDiscarded = glMod(float(vertex_in.decimals_ed.x), 2.0) > 0.0;
    if (isDiscarded) {
        return {
            .position = float4(0.0),
            .color    = half4(0.0),
        };
    }

    const auto base   = max($props.base, 0.0);
    const auto height = max($props.height, 0.0);

    const float2 p1 = float2(vertex_in.outline_pos);
    const float2 p2 = float2(vertex_in.next_outline_pos);
    const float2 edgevector = normalize(p2 - p1);
    const float2 own = float2(-edgevector.y, edgevector.x);

    const float3 normal = float3(vertex_in.pos.x == 0 ? wall_normal(vertex_in.decimals_ed.x, own) : wall_normal(vertex_in.next_decimals_ed.x, own), 0.0);
    const float t = float(vertex_in.pos.y);
    const float z = (t != 0.0) ? height : base;     // TODO: This would come out wrong on GL for negative values, check it...

    const float4 position = projectTileFor3D(vertex_in.pos.x == 0.0 ? p1 : p2, z, projection);

#if defined(OVERDRAW_INSPECTOR)
    return {
        .position = position,
        .color    = half4(1.0),
    };
#endif

    auto color = $props.color;

    // Relative luminance (how dark/bright is the surface color?)
    const float luminance = color.r * 0.2126 + color.g * 0.7152 + color.b * 0.0722;

    float4 vcolor = float4(0.0, 0.0, 0.0, 1.0);

    // Add slight ambient lighting so no extrusions are totally black
    color += min(float4(0.03, 0.03, 0.03, 1.0), float4(1.0));

    // Calculate cos(theta), where theta is the angle between surface normal and diffuse light ray
    const float directionalFraction = clamp(dot(normal, float3(props.light_position)), 0.0, 1.0);

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
    const float3 light_color = float3(props.light_color);
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

pub(super) const FILL_EXTRUSION_INSTANCED_SHADER: Msl = Msl {
    vertex_main_function: r"vertexMain",
    fragment_main_function: r"fragmentMain",
    prelude: concat!(
        wall_normal!(),
        r"

"
    ),
    source: concat!(
        r"

struct VertexStage {
    short2 pos [[attribute(0)]];
    short2 outline_pos [[attribute(1)]];
    ushort2 decimals_ed [[attribute(2)]];
    short2 next_outline_pos [[attribute(6)]];
    ushort2 next_decimals_ed [[attribute(7)]];

    #property attribute color
    #property attribute base
    #property attribute height
};

",
        extrusion_wall!(),
        r"
FragmentStage vertex vertexMain(thread const VertexStage vertex_in [[stage_in]]) {
    return wall(vertex_in, drawableVector[uboIndex], projectionVector[uboIndex], props);
}

"
    ),
};

pub(super) const FILL_EXTRUSION_PULLED_SHADER: Msl = Msl {
    vertex_main_function: r"vertexMain",
    fragment_main_function: r"fragmentMain",
    prelude: FILL_EXTRUSION_INSTANCED_SHADER.prelude,
    source: concat!(
        r"

// What the instanced walls read as attributes.
struct WallVertex {
    short2 pos;
    short2 outline_pos;
    ushort2 decimals_ed;
    short2 next_outline_pos;
    ushort2 next_decimals_ed;
    #property varying color float4
    #property varying base float2
    #property varying height float2
};
",
        extrusion_wall!(),
        r"
FragmentStage vertex vertexMain(uint vertex_id [[vertex_id]]) {
    device const FillExtrusionDrawableUBO& drawable = drawableVector[uboIndex];

    // Four vertices a wall, in the order of the static quad's corners.
    const uint wall_index = vertex_id / 4;
    const uint corner = vertex_id % 4;
    // A vertex of the bucket: its position, then `decimals_ed`.
    const short4 outline = wallVertices[wall_index];
    const short4 next = wallVertices[wall_index + 1];

    WallVertex vertex_in;
    vertex_in.pos = short2(corner < 2 ? 1 : 0, corner & 1);
    vertex_in.outline_pos = outline.xy;
    vertex_in.decimals_ed = ushort2(outline.zw);
    vertex_in.next_outline_pos = next.xy;
    vertex_in.next_decimals_ed = ushort2(next.zw);

    // The wall's paint values are its outline vertex's, where the drawable's
    // `paint_layout` says: the floats a vertex takes, then the offsets of
    // base, height and color, a byte each from the lowest.
    const uint paint_layout = drawable.paint_layout;
    device const float* paint = wallPaint + wall_index * (paint_layout & 0xff);
#if !defined(HAS_UNIFORM_u_base)
    vertex_in.base = float2(*(device const packed_float2*)(paint + ((paint_layout >> 8) & 0xff)));
#endif
#if !defined(HAS_UNIFORM_u_height)
    vertex_in.height = float2(*(device const packed_float2*)(paint + ((paint_layout >> 16) & 0xff)));
#endif
#if !defined(HAS_UNIFORM_u_color)
    vertex_in.color = float4(*(device const packed_float4*)(paint + (paint_layout >> 24)));
#endif

    return wall(vertex_in, drawable, projectionVector[uboIndex], props);
}
"
    ),
};

pub(super) const FILL_EXTRUSION_PATTERN_INSTANCED_SHADER: Msl = Msl {
    vertex_main_function: r"vertexMain",
    fragment_main_function: r"fragmentMain",
    prelude: concat!(
        pattern_pos!(msl),
        atlas_pos!(msl),
        wall_normal!(),
        r"

"
    ),
    source: r"

struct VertexStage {
    short2 pos [[attribute(0)]];
    short2 outline_pos [[attribute(1)]];
    ushort2 decimals_ed [[attribute(2)]];
    short2 next_outline_pos [[attribute(7)]];
    ushort2 next_decimals_ed [[attribute(8)]];

    #property attribute base
    #property attribute height
    #property attribute pattern_from
    #property attribute pattern_to
};

struct FragmentStage {
    float4 position [[position, invariant]];
    float4 lighting;
    float2 pos_a;
    float2 pos_b;

    #property varying pattern_from float4 flat
    #property varying pattern_to float4 flat
};

struct FragmentOutput {
    half4 color [[color(0)]];
    //float depth [[depth(less)]]; // Write depth value if it's less than what's already there
};

FragmentStage vertex vertexMain(thread const VertexStage vertex_in [[stage_in]]) {

    bool isDiscarded = glMod(float(vertex_in.decimals_ed.x), 2.0) > 0.0;
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

    const auto base   = max($props.base, 0.0);
    const auto height = max($props.height, 0.0);

    const float2 p1 = float2(vertex_in.outline_pos);
    const float2 p2 = float2(vertex_in.next_outline_pos);
    const float2 edgevector = normalize(p2 - p1);
    const float2 own = float2(-edgevector.y, edgevector.x);

    const float3 normal = float3(vertex_in.pos.x == 0 ? wall_normal(vertex_in.decimals_ed.x, own) : wall_normal(vertex_in.next_decimals_ed.x, own), 0.0);
    const float edgedistance = vertex_in.pos.x == 0 ? vertex_in.next_decimals_ed.y : vertex_in.decimals_ed.y;
    const float t = float(vertex_in.pos.y);
    const float z = (t != 0.0) ? height : base;     // TODO: This would come out wrong on GL for negative values, check it...

    const float4 position = projectTileFor3D(vertex_in.pos.x == 0.0 ? p1 : p2, z, projectionVector[uboIndex]);

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

    const auto pattern_from = $tileProps.pattern_from;
    const auto pattern_to   = $tileProps.pattern_to;

    const float2 pattern_tl_a = pattern_from.xy;
    const float2 pattern_br_a = pattern_from.zw;
    const float2 pattern_tl_b = pattern_to.xy;
    const float2 pattern_br_b = pattern_to.zw;

    const float pixelRatio = view.pixel_ratio;
    const float tileZoomRatio = drawable.tile_ratio;
    const float fromScale = props.from_scale;
    const float toScale = props.to_scale;

    const float2 display_size_a = float2((pattern_br_a.x - pattern_tl_a.x) / pixelRatio, (pattern_br_a.y - pattern_tl_a.y) / pixelRatio);
    const float2 display_size_b = float2((pattern_br_b.x - pattern_tl_b.x) / pixelRatio, (pattern_br_b.y - pattern_tl_b.y) / pixelRatio);

    const float2 pos = float2(edgedistance, z * drawable.height_factor); // extrusion side

    float4 lighting = float4(0.0, 0.0, 0.0, 1.0);
    float directional = clamp(dot(normal, float3(props.light_position)), 0.0, 1.0);
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

    lighting.rgb += clamp(directional * float3(props.light_color), mix(float3(0.0), float3(0.3), 1.0 - float3(props.light_color)), float3(1.0));
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
                                     texture2d<float, access::sample> image0 [[texture(0)]],
                                     sampler image0_sampler [[sampler(0)]]) {
#if defined(OVERDRAW_INSPECTOR)
    return {half4(1.0)/*, in.position.z*/};
#endif

    device const FillExtrusionTilePropsUBO& tileProps = tilePropsVector[uboIndex];

    const auto pattern_from = float4($tileProps.pattern_from);
    const auto pattern_to = float4($tileProps.pattern_to);

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

pub(super) const FILL_EXTRUSION_PATTERN_SHADER: Msl = Msl {
    vertex_main_function: r"vertexMain",
    fragment_main_function: r"fragmentMain",
    prelude: concat!(
        pattern_pos!(msl),
        atlas_pos!(msl),
        r"

"
    ),
    source: r"

struct VertexStage {
    short2 pos [[attribute(0)]];
    ushort2 decimals_ed [[attribute(1)]];

    #property attribute base
    #property attribute height
    #property attribute pattern_from
    #property attribute pattern_to
};

struct FragmentStage {
    float4 position [[position, invariant]];
    float4 lighting;
    float2 pos_a;
    float2 pos_b;

    #property varying pattern_from float4 flat
    #property varying pattern_to float4 flat
};

struct FragmentOutput {
    half4 color [[color(0)]];
    //float depth [[depth(less)]]; // Write depth value if it's less than what's already there
};

FragmentStage vertex vertexMain(thread const VertexStage vertex_in [[stage_in]]) {

    device const FillExtrusionDrawableUBO& drawable = drawableVector[uboIndex];
    device const FillExtrusionTilePropsUBO& tileProps = tilePropsVector[uboIndex];

    const auto base   = max($props.base, 0.0);
    const auto height = max($props.height, 0.0);

    const float3 normal = float3(0.0, 0.0, 1.0);
    const float t = 1.0;
    const float z = (t != 0.0) ? height : base;     // TODO: This would come out wrong on GL for negative values, check it...
    const float4 position = projectTileFor3D(float2(vertex_in.pos), z, projectionVector[uboIndex]);

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

    const auto pattern_from = $tileProps.pattern_from;
    const auto pattern_to   = $tileProps.pattern_to;

    const float2 pattern_tl_a = pattern_from.xy;
    const float2 pattern_br_a = pattern_from.zw;
    const float2 pattern_tl_b = pattern_to.xy;
    const float2 pattern_br_b = pattern_to.zw;

    const float pixelRatio = view.pixel_ratio;
    const float tileZoomRatio = drawable.tile_ratio;
    const float fromScale = props.from_scale;
    const float toScale = props.to_scale;

    const float2 display_size_a = float2((pattern_br_a.x - pattern_tl_a.x) / pixelRatio, (pattern_br_a.y - pattern_tl_a.y) / pixelRatio);
    const float2 display_size_b = float2((pattern_br_b.x - pattern_tl_b.x) / pixelRatio, (pattern_br_b.y - pattern_tl_b.y) / pixelRatio);

    const float2 pos = float2(vertex_in.pos); // extrusion top

    float4 lighting = float4(0.0, 0.0, 0.0, 1.0);
    float directional = clamp(dot(normal, float3(props.light_position)), 0.0, 1.0);
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

    lighting.rgb += clamp(directional * float3(props.light_color), mix(float3(0.0), float3(0.3), 1.0 - float3(props.light_color)), float3(1.0));
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
                                     texture2d<float, access::sample> image0 [[texture(0)]],
                                     sampler image0_sampler [[sampler(0)]]) {
#if defined(OVERDRAW_INSPECTOR)
    return {half4(1.0)/*, in.position.z*/};
#endif

    device const FillExtrusionTilePropsUBO& tileProps = tilePropsVector[uboIndex];

    const auto pattern_from = float4($tileProps.pattern_from);
    const auto pattern_to = float4($tileProps.pattern_to);

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

pub(super) const FILL_EXTRUSION_SHADER: Msl = Msl {
    vertex_main_function: r"vertexMain",
    fragment_main_function: r"fragmentMain",
    prelude: "",
    source: r"

struct VertexStage {
    short2 pos [[attribute(0)]];
    ushort2 decimals_ed [[attribute(1)]];

    #property attribute color
    #property attribute base
    #property attribute height
};

struct FragmentStage {
    float4 position [[position, invariant]];
    half4 color;
};

struct FragmentOutput {
    half4 color [[color(0)]];
    //float depth [[depth(less)]]; // Write depth value if it's less than what's already there
};

FragmentStage vertex vertexMain(thread const VertexStage vertex_in [[stage_in]]) {

    device const FillExtrusionDrawableUBO& drawable = drawableVector[uboIndex];

    const auto base   = max($props.base, 0.0);
    const auto height = max($props.height, 0.0);

    const float3 normal = float3(0.0, 0.0, 1.0);
    const float t = 1.0;
    const float z = (t != 0.0) ? height : base;     // TODO: This would come out wrong on GL for negative values, check it...
    const float4 position = projectTileFor3D(float2(vertex_in.pos), z, projectionVector[uboIndex]);

#if defined(OVERDRAW_INSPECTOR)
    return {
        .position = position,
        .color    = half4(1.0),
    };
#endif

    auto color = $props.color;

    // Relative luminance (how dark/bright is the surface color?)
    const float luminance = color.r * 0.2126 + color.g * 0.7152 + color.b * 0.0722;

    float4 vcolor = float4(0.0, 0.0, 0.0, 1.0);

    // Add slight ambient lighting so no extrusions are totally black
    color += min(float4(0.03, 0.03, 0.03, 1.0), float4(1.0));

    // Calculate cos(theta), where theta is the angle between surface normal and diffuse light ray
    const float directionalFraction = clamp(dot(normal, float3(props.light_position)), 0.0, 1.0);

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
    const float3 light_color = float3(props.light_color);
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
