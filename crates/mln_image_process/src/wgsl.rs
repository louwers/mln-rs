//! The WGSL of the passes' program.

use mln_gfx::shaders::source::WgslSource;

pub(crate) const IMAGE_PROCESS: WgslSource = WgslSource {
    vertex: Some(
        r"
@vertex
fn main(in: VertexInput) -> @builtin(position) vec4<f32> {
    return vec4<f32>(vec2<f32>(in.pos) * 2.0 - 1.0, 0.0, 1.0);
}
",
    ),
    fragment: Some(
        r"
// The least half width of a threshold's antialiased edge, in coverage.
const PROCESS_THRESHOLD_EDGE: f32 = 0.001;

// A premultiplied color's own color, white where it covers nothing.
fn process_unpremultiplied(c: vec4<f32>) -> vec3<f32> {
    if (c.a <= 0.0) {
        return vec3<f32>(1.0);
    }
    return clamp(c.rgb / c.a, vec3<f32>(0.0), vec3<f32>(1.0));
}

// `c`'s color at the coverage `a`.
fn process_with_coverage(c: vec4<f32>, a: f32) -> vec4<f32> {
    let coverage = clamp(a, 0.0, 1.0);
    return vec4<f32>(process_unpremultiplied(c) * coverage, coverage);
}

// `c` put together with `o` as the combine's `mode` says.
fn process_combine(c: vec4<f32>, o: vec4<f32>, mode: u32) -> vec4<f32> {
    switch mode {
        case 0u: { return c * o.a; }
        case 1u: { return c * (1.0 - o.a); }
        case 2u: { return max(c, o); }
        case 3u: { return min(c, o); }
        case 4u: { return o + c * (1.0 - o.a); }
        default: { return c + o * (1.0 - c.a); }
    }
}

@fragment
fn main(@builtin(position) position: vec4<f32>) -> @location(0) vec4<f32> {
    let uv = position.xy * process.texel.xy;
    var c = textureSampleLevel(image, image_sampler, uv, 0.0);
    let taps = u32(process.blur.z);
    if (taps > 0u) {
        c *= process.blur.w;
        for (var i = 0u; i < taps; i++) {
            let pair = process.taps[i / 2u];
            let tap = select(pair.xy, pair.zw, i % 2u == 1u);
            let offset = process.blur.xy * tap.x;
            let sum = textureSampleLevel(image, image_sampler, uv + offset, 0.0)
                + textureSampleLevel(image, image_sampler, uv - offset, 0.0);
            c += sum * tap.y;
        }
    }
    let count = u32(process.texel.z);
    for (var i = 0u; i < count; i++) {
        let op = process.ops[2u * i];
        let kind = u32(op.x);
        if (kind == 1u) {
            let edge = max(fwidth(c.a) * 0.5, PROCESS_THRESHOLD_EDGE);
            c = process_with_coverage(c, smoothstep(op.y - edge, op.y + edge, c.a));
        } else if (kind == 2u) {
            c = process.ops[2u * i + 1u] * c.a;
        } else if (kind == 3u) {
            c *= op.y;
        } else if (kind == 4u) {
            let o = textureSampleLevel(other, other_sampler, uv, 0.0);
            c = process_combine(c, o, u32(op.y));
        }
    }
    return c;
}
",
    ),
    prelude: None,
};
