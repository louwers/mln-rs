//! The GLSL, for Vulkan and D3D12, of the passes' program.

use mln_gfx::shaders::source::GlslSource;

pub(crate) const IMAGE_PROCESS: GlslSource = GlslSource {
    vertex: r"
void main() {
    gl_Position = vec4(vec2(in_pos) * 2.0 - 1.0, 0.0, 1.0);
}
",
    fragment: r"
layout(location = 0) out vec4 out_color;

// The least half width of a threshold's antialiased edge, in coverage.
const float PROCESS_THRESHOLD_EDGE = 0.001;

// A premultiplied color's own color, white where it covers nothing.
vec3 process_unpremultiplied(vec4 c) {
    if (c.a <= 0.0) {
        return vec3(1.0);
    }
    return clamp(c.rgb / c.a, vec3(0.0), vec3(1.0));
}

// `c`'s color at the coverage `a`.
vec4 process_with_coverage(vec4 c, float a) {
    float coverage = clamp(a, 0.0, 1.0);
    return vec4(process_unpremultiplied(c) * coverage, coverage);
}

// `c` put together with `o` as the combine's `mode` says.
vec4 process_combine(vec4 c, vec4 o, int mode) {
    if (mode == 0) {
        return c * o.a;
    } else if (mode == 1) {
        return c * (1.0 - o.a);
    } else if (mode == 2) {
        return max(c, o);
    } else if (mode == 3) {
        return min(c, o);
    } else if (mode == 4) {
        return o + c * (1.0 - o.a);
    }
    return c + o * (1.0 - c.a);
}

void main() {
    vec2 uv = gl_FragCoord.xy * process.texel.xy;
    vec4 c = textureLod(sampler2D(image_texture, image_sampler), uv, 0.0);
    int taps = int(process.blur.z);
    if (taps > 0) {
        c *= process.blur.w;
        for (int i = 0; i < taps; i++) {
            vec4 pair = process.taps[i / 2];
            vec2 tap = i % 2 == 1 ? pair.zw : pair.xy;
            vec2 offset = process.blur.xy * tap.x;
            vec4 sum = textureLod(sampler2D(image_texture, image_sampler), uv + offset, 0.0)
                + textureLod(sampler2D(image_texture, image_sampler), uv - offset, 0.0);
            c += sum * tap.y;
        }
    }
    int count = int(process.texel.z);
    for (int i = 0; i < count; i++) {
        vec4 op = process.ops[2 * i];
        int kind = int(op.x);
        if (kind == 1) {
            float edge = max(fwidth(c.a) * 0.5, PROCESS_THRESHOLD_EDGE);
            c = process_with_coverage(c, smoothstep(op.y - edge, op.y + edge, c.a));
        } else if (kind == 2) {
            c = process.ops[2 * i + 1] * c.a;
        } else if (kind == 3) {
            c *= op.y;
        } else if (kind == 4) {
            vec4 o = textureLod(sampler2D(other_texture, other_sampler), uv, 0.0);
            c = process_combine(c, o, int(op.y));
        }
    }
    out_color = c;
}
",
};
