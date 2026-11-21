//! The WGSL of the engine's programs.

use mln_gfx::shaders::source::wgsl::Wgsl;

pub(crate) const ATMOSPHERE_SHADER: Wgsl = Wgsl {
    vertex: r"
struct VertexInput {
    @location(0) position: vec2<i32>,
};

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) view_direction: vec3<f32>,
};

@vertex
fn main(in: VertexInput) -> VertexOutput {
    let pos = vec2<f32>(f32(in.position.x), f32(in.position.y));
    let view_position = atmosphere.inv_view_projection * vec4<f32>(pos, 0.0, 1.0);
    var out: VertexOutput;
    // WebGPU uses a zero-to-one NDC depth range, so 0.5 matches WebGL's
    // framebuffer depth for clip-space z=0.
    out.position = vec4<f32>(pos, 0.5, 1.0);
    out.view_direction = view_position.xyz / view_position.w - atmosphere.camera_position.xyz;
    return out;
}
",
    fragment: r"
struct FragmentInput {
    @location(0) view_direction: vec3<f32>,
};

const ATMOSPHERE_PI: f32 = 3.141592653589793;
const PRIMARY_STEPS: i32 = 5;
const LIGHT_STEPS: i32 = 3;
const EARTH_RADIUS: f32 = 6371000.0;
const ATMOSPHERE_RADIUS: f32 = 6471000.0;

fn ray_sphere_intersection(origin: vec3<f32>, direction: vec3<f32>, radius: f32) -> vec2<f32> {
    let a = dot(direction, direction);
    let b = 2.0 * dot(direction, origin);
    let c = dot(origin, origin) - radius * radius;
    let discriminant = b * b - 4.0 * a * c;
    if (discriminant < 0.0) {
        return vec2<f32>(1e5, -1e5);
    }
    let root = sqrt(discriminant);
    return vec2<f32>((-b - root) / (2.0 * a), (-b + root) / (2.0 * a));
}

fn scatter(direction: vec3<f32>, origin: vec3<f32>, sun: vec3<f32>) -> vec4<f32> {
    let rayleigh_coefficient = vec3<f32>(5.5e-6, 13.0e-6, 22.4e-6);
    let mie_coefficient = 21e-6;
    let rayleigh_height = 8e3;
    let mie_height = 1.2e3;
    let mie_direction = 0.758;
    let ray = normalize(direction);
    let sun_direction = normalize(sun);

    var bounds = ray_sphere_intersection(origin, ray, ATMOSPHERE_RADIUS);
    if (bounds.x > bounds.y) {
        return vec4<f32>(0.0, 0.0, 0.0, 1.0);
    }
    bounds.x = max(bounds.x, 0.0);
    let ground = ray_sphere_intersection(origin, ray, EARTH_RADIUS);
    if (ground.x <= ground.y && ground.x > 0.0) {
        bounds.y = min(bounds.y, ground.x);
    }

    let primary_step = (bounds.y - bounds.x) / f32(PRIMARY_STEPS);
    var primary_time = bounds.x + primary_step * 0.5;
    var primary_rayleigh_depth = 0.0;
    var primary_mie_depth = 0.0;
    var total_rayleigh = vec3<f32>(0.0);
    var total_mie = vec3<f32>(0.0);
    let mu = dot(ray, sun_direction);
    let mu_squared = mu * mu;
    let g_squared = mie_direction * mie_direction;
    let rayleigh_phase = 3.0 / (16.0 * ATMOSPHERE_PI) * (1.0 + mu_squared);
    let mie_phase = 3.0 / (8.0 * ATMOSPHERE_PI) * ((1.0 - g_squared) * (mu_squared + 1.0)) /
        (pow(1.0 + g_squared - 2.0 * mu * mie_direction, 1.5) * (2.0 + g_squared));

    for (var i: i32 = 0; i < PRIMARY_STEPS; i++) {
        let sample_position = origin + ray * primary_time;
        let sample_height = length(sample_position) - EARTH_RADIUS;
        let rayleigh_step = exp(-sample_height / rayleigh_height) * primary_step;
        let mie_step = exp(-sample_height / mie_height) * primary_step;
        primary_rayleigh_depth += rayleigh_step;
        primary_mie_depth += mie_step;
        let light_step = ray_sphere_intersection(sample_position, sun_direction, ATMOSPHERE_RADIUS).y /
            f32(LIGHT_STEPS);
        var light_time = light_step * 0.5;
        var light_rayleigh_depth = 0.0;
        var light_mie_depth = 0.0;
        for (var j: i32 = 0; j < LIGHT_STEPS; j++) {
            let light_position = sample_position + sun_direction * light_time;
            let light_height = length(light_position) - EARTH_RADIUS;
            light_rayleigh_depth += exp(-light_height / rayleigh_height) * light_step;
            light_mie_depth += exp(-light_height / mie_height) * light_step;
            light_time += light_step;
        }
        let attenuation = exp(-(mie_coefficient * (primary_mie_depth + light_mie_depth) +
            rayleigh_coefficient * (primary_rayleigh_depth + light_rayleigh_depth)));
        total_rayleigh += rayleigh_step * attenuation;
        total_mie += mie_step * attenuation;
        primary_time += primary_step;
    }

    let opacity = exp(-(length(rayleigh_coefficient) * length(total_rayleigh) +
        mie_coefficient * length(total_mie)));
    let color = 22.0 * (rayleigh_phase * rayleigh_coefficient * total_rayleigh +
        mie_phase * mie_coefficient * total_mie);
    return vec4<f32>(color, opacity);
}

@fragment
fn main(in: FragmentInput) -> @location(0) vec4<f32> {
    let camera = atmosphere.camera_position.xyz * EARTH_RADIUS;
    var color = scatter(in.view_direction, camera, atmosphere.sun_position.xyz);
    color = vec4<f32>(1.0 - exp(-color.rgb), color.a);
    color = pow(color, vec4<f32>(1.0 / 2.2));
    return vec4<f32>(color.rgb, 1.0 - color.a) * atmosphere.sun_position.w;
}
",
    prelude: "",
};

pub(crate) const CLIPPING_MASK_PROGRAM: Wgsl = Wgsl {
    vertex: r"
struct VertexInput {
    @location(0) position: vec2<i32>,
};

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
};

@vertex
fn main(in: VertexInput) -> VertexOutput {
    var out: VertexOutput;
    let pos = vec2<f32>(f32(in.position.x), f32(in.position.y));
    out.position = projectTileWithPoles(pos, pos, drawProjection);
    return out;
}
",
    fragment: r"
@fragment
fn main() -> @location(0) vec4<f32> {
    return vec4<f32>(0.0, 0.0, 0.0, 0.0);
}
",
    prelude: "",
};

pub(crate) const GLOBE_DEPTH_SHADER: Wgsl = Wgsl {
    vertex: r"
struct VertexInput {
    @location(0) position: vec2<i32>,
};

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
};

@vertex
fn main(in: VertexInput) -> VertexOutput {
    var out: VertexOutput;
    out.position = projectTileFor3D(vec2<f32>(f32(in.position.x), f32(in.position.y)), 0.0, projectionVector[globalIndex.value]);
    return out;
}
",
    fragment: r"
@fragment
fn main() -> @location(0) vec4<f32> {
    return vec4<f32>(0.0);
}
",
    prelude: "",
};

/// A layer's texture, the size of the frame, composited onto it
/// texel for texel, with its opacity and, for the blend modes that no blend
/// function expresses, against a copy of the frame (`backdrop`).
pub(crate) const LAYER_COMPOSITE_SHADER: Wgsl = Wgsl {
    vertex: r"
struct VertexInput {
    @location(0) position: vec2<i32>,
};

@vertex
fn main(in: VertexInput) -> @builtin(position) vec4<f32> {
    let pos = vec2<f32>(in.position);
    return vec4<f32>(pos * 2.0 - 1.0, 0.0, 1.0);
}
",
    fragment: r"

// The blend modes that no blend function expresses, of the W3C compositing
// spec, on premultiplied colors.
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
        co = inv_dst_a * src.rgb + inv_src_a * dst.rgb + src.rgb * dst.rgb;
    } else if (mode == 2) {
        let screen = step(vec3<f32>(dst.a), 2.0 * dst.rgb);
        let multiplied = inv_dst_a * src.rgb + inv_src_a * dst.rgb + 2.0 * dst.rgb * src.rgb;
        let screened = src.rgb + dst.rgb + dst.a * src.rgb + src.a * dst.rgb - vec3<f32>(src.a * dst.a) - 2.0 * dst.rgb * src.rgb;
        co = mix(multiplied, screened, screen);
    } else {
        // Rounding to 8 bits can leave a premultiplied channel above its alpha.
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
fn main(@builtin(position) position: vec4<f32>) -> @location(0) vec4<f32> {
    let texel = vec2<i32>(position.xy);
    let color = textureLoad(image, texel, 0);
#ifdef OVERDRAW_INSPECTOR
    return color;
#else
    let src = color * props.opacity;
    if (props.blend == 0) {
        return src;
    }
    return composite(src, textureLoad(backdrop, texel, 0), props.blend);
#endif
}
",
    prelude: "",
};

pub(crate) const TILE_DEBUG_SHADER: Wgsl = Wgsl {
    vertex: r"
// The key's DEVICE_PIXEL_RATIO, which the pipeline sets.
override DEVICE_PIXEL_RATIO: f32 = 1.0;

struct VertexInput {
    // The quad's corner: x picks the segment's start (0) or end (1), y its side.
    @location(0) corner: vec2<i32>,
    // Per instance: where the segment starts and ends, in tile units.
    @location(1) start_pos: vec2<i32>,
    @location(2) end_pos: vec2<i32>,
};

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    // The offset in pixels from the segment's start, along the segment and
    // across it, on the map.
    @location(0) offset: vec2<f32>,
    // The segment's length in pixels on the map.
    @location(1) @interpolate(flat) segment_length: f32,
    // The pixels on the map that a pixel on screen spans across the segment.
    @location(2) gamma_scale: f32,
#ifdef PROJECTION_GLOBE
    // The x in tile units on the zoom 0 tile, 0 on other tiles.
    @location(3) tile_x: f32,
#endif
};

@vertex
fn main(in: VertexInput) -> VertexOutput {
    var out: VertexOutput;
    let projection = projectionVector[globalIndex.value];
    let start = vec2<f32>(in.start_pos);
    let end = vec2<f32>(in.end_pos);

    // The tile units a pixel spans; on the globe, more toward the poles,
    // where a tile unit covers less of the sphere.
    var units = 1.0 / (projection.tile_mercator_coords.z * props.world_width);
#ifdef PROJECTION_GLOBE
    let ratio = circumferenceRatioAtTileY((start.y + end.y) * 0.5, projection);
    units = units * mix(1.0, 1.0 / ratio, projection.transition);
#endif

    // The quad around the segment on the map, reaching half the width and
    // the antialiased edge past its sides and ends.
    let tile_length = distance(start, end);
    var along = vec2<f32>(1.0, 0.0);
    if (tile_length > 0.0) {
        along = (end - start) / tile_length;
    }
    let across = vec2<f32>(-along.y, along.x);
    let segment_length = tile_length / units;
    let reach = props.width * 0.5 + 0.5 / DEVICE_PIXEL_RATIO;
    let offset = vec2<f32>(select(-reach, segment_length + reach, in.corner.x != 0),
                           select(-reach, reach, in.corner.y != 0));
    let base = start + along * (offset.x * units);
    let tile_pos = base + across * (offset.y * units);
    let corner = projectTile(tile_pos, projection);
    let middle = projectTile(base, projection);
    let extent = length((corner.xy - middle.xy) / corner.w * view.units_to_pixels);

    out.position = corner;
    out.offset = offset;
    out.segment_length = segment_length;
    out.gamma_scale = reach / max(extent, 1e-6);
#ifdef PROJECTION_GLOBE
    // The zoom 0 tile's quads reach past its x extent around the planet onto
    // the tile itself; the fragment stage discards what lies past it.
    out.tile_x = select(0.0, tile_pos.x, projection.tile_mercator_coords.z * 8192.0 >= 1.0);
#endif
    return out;
}
",
    fragment: r"
// The key's DEVICE_PIXEL_RATIO, which the pipeline sets.
override DEVICE_PIXEL_RATIO: f32 = 1.0;

struct FragmentInput {
    @location(0) offset: vec2<f32>,
    @location(1) @interpolate(flat) segment_length: f32,
    @location(2) gamma_scale: f32,
#ifdef PROJECTION_GLOBE
    @location(3) tile_x: f32,
#endif
};

@fragment
fn main(in: FragmentInput) -> @location(0) vec4<f32> {
#ifdef PROJECTION_GLOBE
    if (in.tile_x < 0.0 || in.tile_x >= 8192.0) {
        discard;
    }
#endif
#ifdef OVERDRAW_INSPECTOR
    return vec4<f32>(1.0);
#else
    // The distance from the segment, whose ends are round, and the coverage
    // of an antialiased edge one device pixel wide on screen.
    let beyond = max(max(-in.offset.x, in.offset.x - in.segment_length), 0.0);
    let dist = length(vec2<f32>(beyond, in.offset.y));
    let edge = max(in.gamma_scale / DEVICE_PIXEL_RATIO, 1e-6);
    let alpha = clamp((props.width * 0.5 + 0.5 / DEVICE_PIXEL_RATIO - dist) / edge, 0.0, 1.0);
    return props.color * alpha;
#endif
}
",
    prelude: "",
};

pub(crate) const SKY_SHADER: Wgsl = Wgsl {
    vertex: r"
struct VertexInput {
    @location(0) position: vec2<i32>,
};

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) pos: vec2<f32>,
};

@vertex
fn main(in: VertexInput) -> VertexOutput {
    let pos = vec2<f32>(f32(in.position.x), f32(in.position.y));
    var out: VertexOutput;
    out.position = vec4<f32>(pos, 1.0, 1.0);
    out.pos = pos;
    return out;
}
",
    fragment: r"
struct FragmentInput {
    @location(0) pos: vec2<f32>,
};

@fragment
fn main(in: FragmentInput) -> @location(0) vec4<f32> {
    let pixel = (in.pos * 0.5 + 0.5) * sky.viewport_size;
    let distance_to_horizon = dot(pixel - sky.horizon, sky.horizon_normal);
    var color = vec4<f32>(0.0);
    if (distance_to_horizon > 0.0) {
        if (sky.sky_horizon_blend > 0.0 && distance_to_horizon < sky.sky_horizon_blend) {
            let blend = 1.0 - distance_to_horizon / sky.sky_horizon_blend;
            color = mix(sky.sky_color, sky.horizon_color, blend * blend);
        } else {
            color = sky.sky_color;
        }
    }
    return color * (1.0 - sky.sky_blend);
}
",
    prelude: "",
};
