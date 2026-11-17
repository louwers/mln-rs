// A layer drawn into a texture the size of the frame, composited onto the
// frame texel for texel with its `*-layer-opacity`. For the `*-layer-blend`
// modes that no blend function expresses, it composites against a copy of
// the frame, the backdrop.

struct LayerCompositeUBO {
    opacity: f32,
    // A LAYER_COMPOSITE_BLEND_* mode: 0 draws the texture for the
    // pipeline's blend to composite.
    blend: i32,
    pad2: f32,
    pad3: f32,
};

// ID_LAYER_COMPOSITE_UBO
@group(1) @binding(5) var<uniform> props: LayerCompositeUBO;

// ID_LAYER_COMPOSITE_IMAGE_TEXTURE: the layer.
@group(3) @binding(0) var image_texture: texture_2d<f32>;
// ID_LAYER_COMPOSITE_BACKDROP_TEXTURE: the copy of the frame.
@group(3) @binding(1) var backdrop_texture: texture_2d<f32>;

// The location is ID_LAYER_COMPOSITE_POS_VERTEX_ATTRIBUTE: a corner of the
// unit square.
struct VertexInput {
    @location(0) position: vec2<i32>,
};

@vertex
fn vs_main(in: VertexInput) -> @builtin(position) vec4<f32> {
    let pos = vec2<f32>(in.position);
    return vec4<f32>(pos * 2.0 - 1.0, 0.0, 1.0);
}

// The blend modes of the W3C compositing spec that no blend function
// expresses, on premultiplied colors.
fn lum(c: vec3<f32>) -> f32 {
    return dot(c, vec3<f32>(0.3, 0.59, 0.11));
}

// Pulls a color back into gamut around its luminosity, which it keeps.
fn clip_color(color: vec3<f32>) -> vec3<f32> {
    var c = color;
    let l = lum(c);
    let n = min(min(c.r, c.g), c.b);
    let x = max(max(c.r, c.g), c.b);
    if (n < 0.0) {
        c = l + (c - l) * l / max(l - n, 1e-6);
    }
    if (x > 1.0) {
        c = l + (c - l) * (1.0 - l) / max(x - l, 1e-6);
    }
    return c;
}

fn set_lum(c: vec3<f32>, l: f32) -> vec3<f32> {
    return clip_color(c + (l - lum(c)));
}

fn composite(src: vec4<f32>, dst: vec4<f32>, mode: i32) -> vec4<f32> {
    let inv_dst_a = 1.0 - dst.a;
    let inv_src_a = 1.0 - src.a;
    var co: vec3<f32>;
    if (mode == 1) {
        // Multiply.
        co = inv_dst_a * src.rgb + inv_src_a * dst.rgb + src.rgb * dst.rgb;
    } else if (mode == 2) {
        // Overlay.
        let screen = step(vec3<f32>(dst.a), 2.0 * dst.rgb);
        let multiplied = inv_dst_a * src.rgb + inv_src_a * dst.rgb + 2.0 * dst.rgb * src.rgb;
        let screened = src.rgb + dst.rgb + dst.a * src.rgb + src.a * dst.rgb - vec3<f32>(src.a * dst.a) -
            2.0 * dst.rgb * src.rgb;
        co = mix(multiplied, screened, screen);
    } else {
        // Color and luminosity. Rounding to 8 bits can leave a
        // premultiplied channel above its alpha.
        var cs = vec3<f32>(0.0);
        if (src.a > 0.0) {
            cs = saturate(src.rgb / src.a);
        }
        var cb = vec3<f32>(0.0);
        if (dst.a > 0.0) {
            cb = saturate(dst.rgb / dst.a);
        }
        var blended: vec3<f32>;
        if (mode == 3) {
            blended = set_lum(cs, lum(cb));
        } else {
            blended = set_lum(cb, lum(cs));
        }
        co = inv_dst_a * src.rgb + inv_src_a * dst.rgb + src.a * dst.a * blended;
    }
    return vec4<f32>(co, src.a + dst.a * inv_src_a);
}

@fragment
fn fs_main(@builtin(position) position: vec4<f32>) -> @location(0) vec4<f32> {
    let texel = vec2<i32>(position.xy);
    let color = textureLoad(image_texture, texel, 0);
    @if(OVERDRAW_INSPECTOR) {
        return color;
    }
    @else {
        let src = color * props.opacity;
        if (props.blend == 0) {
            return src;
        }
        return composite(src, textureLoad(backdrop_texture, texel, 0), props.blend);
    }
}
