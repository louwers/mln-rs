//! The programs of the `background` layer type in MSL, for `mln_mtl`.

use mln_gfx::shaders::source::msl::Msl;

use crate::shaders::{atlas_pos, pattern_pos};

pub(super) const BACKGROUND_PATTERN_SHADER: Msl = Msl {
    vertex_main_function: r"vertexMain",
    fragment_main_function: r"fragmentMain",
    prelude: concat!(
        pattern_pos!(msl),
        atlas_pos!(msl),
        r"

//
// Background

//
// Background pattern

"
    ),
    source: r"

struct VertexStage {
    short2 position [[attribute(0)]];
};

struct FragmentStage {
    float4 position [[position, invariant]];
    float2 pos_a;
    float2 pos_b;
};

FragmentStage vertex vertexMain(VertexStage in [[stage_in]]) {

    device const BackgroundPatternDrawableUBO& drawable = drawableVector[drawIndex];

    const float2 pos = float2(in.position);
    const float2 pos_a = get_pattern_pos(drawable.pixel_coord_upper,
                                         drawable.pixel_coord_lower,
                                         props.scale_a * props.pattern_size_a,
                                         drawable.tile_units_to_pixels,
                                         pos);
    const float2 pos_b = get_pattern_pos(drawable.pixel_coord_upper,
                                         drawable.pixel_coord_lower,
                                         props.scale_b * props.pattern_size_b,
                                         drawable.tile_units_to_pixels,
                                         pos);
    return {
        .position = projectTile(float2(in.position.xy), float2(in.position.xy), projectionVector[drawIndex]),
        .pos_a = pos_a,
        .pos_b = pos_b,
    };
}

half4 fragment fragmentMain(FragmentStage in [[stage_in]]) {
#if defined(OVERDRAW_INSPECTOR)
    return half4(1.0);
#endif

    const float2 texsize = props.texsize;
    // A crossfade at 0 or 1 shows one pattern, so skip the other.
    float4 color1 = float4(0.0);
    if (props.mix < 1.0) {
        const float2 imagecoord = glMod(float2(in.pos_a), 1.0);
        const float2 pos = atlas_pos(props.pattern_tl_a / texsize, (props.pattern_br_a - props.pattern_tl_a) / texsize * imagecoord);
        color1 = image.sample(image_sampler, pos);
    }
    float4 color2 = float4(0.0);
    if (props.mix > 0.0) {
        const float2 imagecoord_b = glMod(float2(in.pos_b), 1.0);
        const float2 pos2 = atlas_pos(props.pattern_tl_b / texsize, (props.pattern_br_b - props.pattern_tl_b) / texsize * imagecoord_b);
        color2 = image.sample(image_sampler, pos2);
    }

    return half4(mix(color1, color2, props.mix) * props.opacity);
}
",
};

pub(super) const BACKGROUND_SHADER: Msl = Msl {
    vertex_main_function: r"vertexMain",
    fragment_main_function: r"fragmentMain",
    prelude: r"

//
// Background

//
// Background pattern

",
    source: r"

struct VertexStage {
    short2 position [[attribute(0)]];
};

struct FragmentStage {
    float4 position [[position, invariant]];
};

FragmentStage vertex vertexMain(VertexStage in [[stage_in]]) {
    return {
        .position = projectTile(float2(in.position.xy), float2(in.position.xy), projectionVector[drawIndex])
    };
}

half4 fragment fragmentMain(FragmentStage in [[stage_in]]) {
#if defined(OVERDRAW_INSPECTOR)
    return half4(1.0);
#endif

    return half4(props.color * props.opacity);
}
",
};
