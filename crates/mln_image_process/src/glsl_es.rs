//! The GLSL ES, for OpenGL, of the passes' program.

use mln_gfx::shaders::source::{GlslEsDeclarations, GlslEsSource};

/// Every value at `highp`, the textures too: a scratch pass's are half
/// floats.
const IMAGE_PROCESS_DECLARATIONS: GlslEsDeclarations =
    GlslEsDeclarations { highp_samplers: &["image", "other"], ..GlslEsDeclarations::DEFAULT };

pub(crate) const IMAGE_PROCESS: GlslEsSource = GlslEsSource {
    vertex: r"void main() {
    gl_Position = vec4(a_pos * 2.0 - 1.0, 0.0, 1.0);
}
",
    fragment: r"// The least half width of a threshold's antialiased edge, in coverage.
const highp float PROCESS_THRESHOLD_EDGE = 0.001;

// A premultiplied color's own color, white where it covers nothing.
highp vec3 process_unpremultiplied(highp vec4 c) {
    if (c.a <= 0.0) {
        return vec3(1.0);
    }
    return clamp(c.rgb / c.a, vec3(0.0), vec3(1.0));
}

// `c`'s color at the coverage `a`.
highp vec4 process_with_coverage(highp vec4 c, highp float a) {
    highp float coverage = clamp(a, 0.0, 1.0);
    return vec4(process_unpremultiplied(c) * coverage, coverage);
}

// `c` put together with `o` as the combine's `mode` says.
highp vec4 process_combine(highp vec4 c, highp vec4 o, int mode) {
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
    highp vec2 uv = gl_FragCoord.xy * u_texel.xy;
    highp vec4 c = textureLod(u_image, uv, 0.0);
    int taps = int(u_blur.z);
    if (taps > 0) {
        c *= u_blur.w;
        for (int i = 0; i < taps; i++) {
            highp vec4 pair = u_taps[i / 2];
            highp vec2 tap = i % 2 == 1 ? pair.zw : pair.xy;
            highp vec2 offset = u_blur.xy * tap.x;
            highp vec4 sum = textureLod(u_image, uv + offset, 0.0) + textureLod(u_image, uv - offset, 0.0);
            c += sum * tap.y;
        }
    }
    int count = int(u_texel.z);
    for (int i = 0; i < count; i++) {
        highp vec4 op = u_ops[2 * i];
        int kind = int(op.x);
        if (kind == 1) {
            highp float edge = max(fwidth(c.a) * 0.5, PROCESS_THRESHOLD_EDGE);
            c = process_with_coverage(c, smoothstep(op.y - edge, op.y + edge, c.a));
        } else if (kind == 2) {
            c = u_ops[2 * i + 1] * c.a;
        } else if (kind == 3) {
            c *= op.y;
        } else if (kind == 4) {
            highp vec4 o = textureLod(u_other, uv, 0.0);
            c = process_combine(c, o, int(op.y));
        }
    }
    fragColor = c;
}
",
    declarations: &IMAGE_PROCESS_DECLARATIONS,
};
