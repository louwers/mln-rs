//! The MSL of the passes' program.

use mln_gfx::shaders::source::MslSource;

pub(crate) const IMAGE_PROCESS: MslSource = MslSource {
    vertex_main_function: r"vertexMain",
    fragment_main_function: r"fragmentMain",
    source: r"
struct FragmentStage {
    float4 position [[position, invariant]];
};

FragmentStage vertex vertexMain(thread const VertexStage vertx [[stage_in]] MLN_VERTEX_BINDINGS) {
    return {
        .position = float4(float2(vertx.pos) * 2.0 - 1.0, 0.0, 1.0),
    };
}

// The least half width of a threshold's antialiased edge, in coverage.
constant float PROCESS_THRESHOLD_EDGE = 0.001;

// A premultiplied color's own color, white where it covers nothing.
static float3 process_unpremultiplied(float4 c) {
    if (c.a <= 0.0) {
        return float3(1.0);
    }
    return clamp(c.rgb / c.a, float3(0.0), float3(1.0));
}

// `c`'s color at the coverage `a`.
static float4 process_with_coverage(float4 c, float a) {
    const float coverage = clamp(a, 0.0, 1.0);
    return float4(process_unpremultiplied(c) * coverage, coverage);
}

// `c` put together with `o` as the combine's `mode` says.
static float4 process_combine(float4 c, float4 o, int mode) {
    switch (mode) {
        case 0: return c * o.a;
        case 1: return c * (1.0 - o.a);
        case 2: return max(c, o);
        case 3: return min(c, o);
        case 4: return o + c * (1.0 - o.a);
        default: return c + o * (1.0 - c.a);
    }
}

float4 fragment fragmentMain(FragmentStage in [[stage_in]] MLN_FRAGMENT_BINDINGS) {
    const float2 uv = in.position.xy * process.texel.xy;
    float4 c = image.sample(image_sampler, uv, level(0.0));
    const int taps = int(process.blur.z);
    if (taps > 0) {
        c *= process.blur.w;
        for (int i = 0; i < taps; i++) {
            const float4 pair = process.taps[i / 2];
            const float2 tap = i % 2 == 1 ? pair.zw : pair.xy;
            const float2 offset = process.blur.xy * tap.x;
            const float4 sum = image.sample(image_sampler, uv + offset, level(0.0))
                + image.sample(image_sampler, uv - offset, level(0.0));
            c += sum * tap.y;
        }
    }
    const int count = int(process.texel.z);
    for (int i = 0; i < count; i++) {
        const float4 op = process.ops[2 * i];
        const int kind = int(op.x);
        if (kind == 1) {
            const float edge = max(fwidth(c.a) * 0.5, PROCESS_THRESHOLD_EDGE);
            c = process_with_coverage(c, smoothstep(op.y - edge, op.y + edge, c.a));
        } else if (kind == 2) {
            c = process.ops[2 * i + 1] * c.a;
        } else if (kind == 3) {
            c *= op.y;
        } else if (kind == 4) {
            const float4 o = other.sample(other_sampler, uv, level(0.0));
            c = process_combine(c, o, int(op.y));
        }
    }
    return c;
}
",
    fast_math: true,
};
