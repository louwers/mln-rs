//! The WGSL of the engine's shaders and the symbol layer type's, and the
//! prelude every shader compiles after, which started as MapLibre Native's.

use mln_gfx::shaders::source::wgsl::Wgsl;

pub(crate) const ATMOSPHERE_SHADER: Wgsl = Wgsl {
    vertex: Some(
        r"
struct VertexInput {
    @location(0) position: vec2<i32>,
};

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) view_direction: vec3<f32>,
};

struct AtmospherePropsUBO {
    inv_view_projection: mat4x4<f32>,
    camera_position: vec4<f32>,
    sun_position: vec4<f32>,
};

@group(0) @binding(5) var<uniform> atmosphere: AtmospherePropsUBO;

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
    ),
    fragment: Some(
        r"
struct FragmentInput {
    @location(0) view_direction: vec3<f32>,
};

struct AtmospherePropsUBO {
    inv_view_projection: mat4x4<f32>,
    camera_position: vec4<f32>,
    sun_position: vec4<f32>,
};

@group(0) @binding(5) var<uniform> atmosphere: AtmospherePropsUBO;

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
    ),
    prelude: None,
};

pub(crate) const CLIPPING_MASK_PROGRAM: Wgsl = Wgsl {
    vertex: Some(
        r"
struct VertexInput {
    @location(2) position: vec2<i32>,
};

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
};

@group(0) @binding(4) var<uniform> projection: ProjectionUBO;

@vertex
fn main(in: VertexInput) -> VertexOutput {
    var out: VertexOutput;
    let pos = vec2<f32>(f32(in.position.x), f32(in.position.y));
    out.position = projectTileWithPoles(pos, pos, projection);
    return out;
}
",
    ),
    fragment: Some(
        r"
@fragment
fn main() -> @location(0) vec4<f32> {
    return vec4<f32>(0.0, 0.0, 0.0, 0.0);
}
",
    ),
    prelude: None,
};

pub(crate) const COLLISION_BOX_SHADER: Wgsl = Wgsl {
    vertex: Some(
        r"
struct VertexInput {
    @location(3) position: vec2<i32>,
    @location(4) anchor_position: vec2<i32>,
    @location(5) extrude: vec2<i32>,
    @location(6) placed: vec2<u32>,
    @location(7) shift: vec2<f32>,
    @location(8) height_offset: f32,
};

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) placed: f32,
    @location(1) not_used: f32,
};

struct CollisionDrawableUBO {
    matrix: mat4x4<f32>,
};

struct CollisionTilePropsUBO {
    extrude_scale: vec2<f32>,
    overscale_factor: f32,
    pad1: f32,
};

struct GlobalPaintParamsUBO {
    pattern_atlas_texsize: vec2<f32>,
    units_to_pixels: vec2<f32>,
    world_size: vec2<f32>,
    camera_to_center_distance: f32,
    symbol_fade_change: f32,
    aspect_ratio: f32,
    pixel_ratio: f32,
    map_zoom: f32,
    pad1: f32,
};

@group(0) @binding(0) var<uniform> paintParams: GlobalPaintParamsUBO;
@group(0) @binding(2) var<uniform> drawable: CollisionDrawableUBO;
@group(0) @binding(4) var<uniform> projection: ProjectionUBO;
@group(0) @binding(5) var<uniform> tile_props: CollisionTilePropsUBO;

@vertex
fn main(in: VertexInput) -> VertexOutput {
    var out: VertexOutput;

#ifdef PROJECTION_GLOBE
    let projected_point = projectTileWithElevation(vec2<f32>(f32(in.anchor_position.x), f32(in.anchor_position.y)), in.height_offset, projection);
#else
    let projected_point = drawable.matrix * vec4<f32>(f32(in.anchor_position.x), f32(in.anchor_position.y), in.height_offset, 1.0);
#endif
    let camera_to_anchor_distance = projected_point.w;

    let collision_perspective_ratio = clamp(
        0.5 + 0.5 * (paintParams.camera_to_center_distance / camera_to_anchor_distance),
        0.0,
        4.0);

#ifdef PROJECTION_GLOBE
    out.position = projectTileWithElevation(vec2<f32>(f32(in.position.x), f32(in.position.y)), in.height_offset, projection);
    // Boxes just past the horizon stay visible, as in GL JS; ones far behind it are clipped.
    if (out.position.z / out.position.w < 1.1) {
        out.position.z = 0.0;
    }
#else
    out.position = drawable.matrix * vec4<f32>(f32(in.position.x), f32(in.position.y), in.height_offset, 1.0);
#endif
    let extrude_shift = vec2<f32>(f32(in.extrude.x), f32(in.extrude.y)) + in.shift;
    out.position.x += extrude_shift.x * tile_props.extrude_scale.x * out.position.w * collision_perspective_ratio;
    out.position.y += extrude_shift.y * tile_props.extrude_scale.y * out.position.w * collision_perspective_ratio;

    out.placed = f32(in.placed.x);
    out.not_used = f32(in.placed.y);

    return out;
}
",
    ),
    fragment: Some(
        r"
struct FragmentInput {
    @location(0) placed: f32,
    @location(1) not_used: f32,
};

@fragment
fn main(in: FragmentInput) -> @location(0) vec4<f32> {
    let alpha = 0.5;

    // Red = collision, hide label
    var color = vec4<f32>(1.0, 0.0, 0.0, 1.0) * alpha;

    // Blue = no collision, label is showing
    if (in.placed > 0.5) {
        color = vec4<f32>(0.0, 0.0, 1.0, 0.5) * alpha;
    }

    if (in.not_used > 0.5) {
        // This box not used, fade it out
        color = color * 0.1;
    }

    return color;
}
",
    ),
    prelude: None,
};

pub(crate) const COLLISION_CIRCLE_SHADER: Wgsl = Wgsl {
    vertex: Some(
        r"
struct VertexInput {
    @location(3) position: vec2<i32>,
    @location(4) anchor_position: vec2<i32>,
    @location(5) extrude: vec2<i32>,
    @location(6) placed: vec2<u32>,
};

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) placed: f32,
    @location(1) not_used: f32,
    @location(2) radius: f32,
    @location(3) extrude: vec2<f32>,
    @location(4) extrude_scale: vec2<f32>,
};

struct GlobalPaintParamsUBO {
    pattern_atlas_texsize: vec2<f32>,
    units_to_pixels: vec2<f32>,
    world_size: vec2<f32>,
    camera_to_center_distance: f32,
    symbol_fade_change: f32,
    aspect_ratio: f32,
    pixel_ratio: f32,
    map_zoom: f32,
    pad1: f32,
};

struct CollisionDrawableUBO {
    matrix: mat4x4<f32>,
};

struct CollisionTilePropsUBO {
    extrude_scale: vec2<f32>,
    overscale_factor: f32,
    pad1: f32,
};

@group(0) @binding(0) var<uniform> paintParams: GlobalPaintParamsUBO;
@group(0) @binding(2) var<uniform> drawable: CollisionDrawableUBO;
@group(0) @binding(4) var<uniform> projection: ProjectionUBO;
@group(0) @binding(5) var<uniform> tile_props: CollisionTilePropsUBO;

@vertex
fn main(in: VertexInput) -> VertexOutput {
    var out: VertexOutput;



#ifdef PROJECTION_GLOBE
    let projected_point = projectTile(vec2<f32>(f32(in.anchor_position.x), f32(in.anchor_position.y)), projection);
#else
    let projected_point = drawable.matrix * vec4<f32>(f32(in.anchor_position.x), f32(in.anchor_position.y), 0.0, 1.0);
#endif
    let camera_to_anchor_distance = projected_point.w;

    let perspective = max(camera_to_anchor_distance, 1e-6);
    let collision_perspective_ratio = clamp(
        0.5 + 0.5 * (paintParams.camera_to_center_distance / perspective),
        0.0,
        4.0);

#ifdef PROJECTION_GLOBE
    var position = projectTile(vec2<f32>(f32(in.position.x), f32(in.position.y)), projection);
    // Circles just past the horizon stay visible, as in GL JS; ones far behind it are clipped.
    if (position.z / position.w < 1.1) {
        position.z = 0.0;
    }
#else
    var position = drawable.matrix * vec4<f32>(f32(in.position.x), f32(in.position.y), 0.0, 1.0);
#endif
    let padding_factor = 1.2;
    let extrude_vec = vec2<f32>(f32(in.extrude.x), f32(in.extrude.y));
    position.x += extrude_vec.x * tile_props.extrude_scale.x * padding_factor * position.w * collision_perspective_ratio;
    position.y += extrude_vec.y * tile_props.extrude_scale.y * padding_factor * position.w * collision_perspective_ratio;

    out.position = position;

    out.placed = f32(in.placed.x);
    out.not_used = f32(in.placed.y);
    out.radius = abs(f32(in.extrude.y));
    out.extrude = extrude_vec * padding_factor;
    out.extrude_scale = tile_props.extrude_scale * paintParams.camera_to_center_distance * collision_perspective_ratio;
    return out;
}
",
    ),
    fragment: Some(
        r"
struct FragmentInput {
    @location(0) placed: f32,
    @location(1) not_used: f32,
    @location(2) radius: f32,
    @location(3) extrude: vec2<f32>,
    @location(4) extrude_scale: vec2<f32>,
};

struct CollisionTilePropsUBO {
    extrude_scale: vec2<f32>,
    overscale_factor: f32,
    pad1: f32,
};

@group(0) @binding(5) var<uniform> tile_props: CollisionTilePropsUBO;

@fragment
fn main(in: FragmentInput) -> @location(0) vec4<f32> {
    let alpha = 0.5;

    // Red = collision, hide label
    var color = vec4<f32>(1.0, 0.0, 0.0, 1.0) * alpha;

    // Blue = no collision, label is showing
    if (in.placed > 0.5) {
        color = vec4<f32>(0.0, 0.0, 1.0, 0.5) * alpha;
    }

    if (in.not_used > 0.5) {
        color = color * 0.2;
    }

    // Add circle outline effect
    let extrude_scale_length = max(length(in.extrude_scale), 1e-6);
    let extrude_length = length(in.extrude) * extrude_scale_length;
    let overscale = max(tile_props.overscale_factor, 1.0);
    let stroke_width = 15.0 * extrude_scale_length / overscale;
    let radius = in.radius * extrude_scale_length;
    let distance_to_edge = abs(extrude_length - radius);
    let opacity_t = smoothstep(-stroke_width, 0.0, -distance_to_edge);

    return opacity_t * color;
}
",
    ),
    prelude: None,
};

pub(crate) const GLOBE_DEPTH_SHADER: Wgsl = Wgsl {
    vertex: Some(
        r"
struct VertexInput {
    @location(0) position: vec2<i32>,
};

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
};

struct GlobalIndexUBO {
    value: u32,
    pad0: vec3<u32>,
};

@group(0) @binding(1) var<uniform> globalIndex: GlobalIndexUBO;
@group(0) @binding(4) var<storage, read> projectionVector: array<ProjectionUBO>;

@vertex
fn main(in: VertexInput) -> VertexOutput {
    var out: VertexOutput;
    out.position = projectTileFor3D(vec2<f32>(f32(in.position.x), f32(in.position.y)), 0.0, projectionVector[globalIndex.value]);
    return out;
}
",
    ),
    fragment: Some(
        r"
@fragment
fn main() -> @location(0) vec4<f32> {
    return vec4<f32>(0.0);
}
",
    ),
    prelude: None,
};

/// Port-only: a layer's texture, the size of the frame, composited onto it
/// texel for texel, with its opacity and, for the blend modes that no blend
/// function expresses, against a copy of the frame (`backdrop_texture`).
pub(crate) const LAYER_COMPOSITE_SHADER: Wgsl = Wgsl {
    vertex: Some(
        r"
struct VertexInput {
    @location(0) position: vec2<i32>,
};

@vertex
fn main(in: VertexInput) -> @builtin(position) vec4<f32> {
    let pos = vec2<f32>(in.position);
    return vec4<f32>(pos * 2.0 - 1.0, 0.0, 1.0);
}
",
    ),
    fragment: Some(
        r"
struct LayerCompositeUBO {
    opacity: f32,
    blend: i32,
    pad2: f32,
    pad3: f32,
};

@group(0) @binding(5) var<uniform> props: LayerCompositeUBO;
@group(1) @binding(1) var image_texture: texture_2d<f32>;
@group(1) @binding(2) var backdrop_texture: texture_2d<f32>;

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
    let color = textureLoad(image_texture, texel, 0);
#ifdef OVERDRAW_INSPECTOR
    return color;
#else
    let src = color * props.opacity;
    if (props.blend == 0) {
        return src;
    }
    return composite(src, textureLoad(backdrop_texture, texel, 0), props.blend);
#endif
}
",
    ),
    prelude: None,
};

pub(crate) const TILE_DEBUG_SHADER: Wgsl = Wgsl {
    vertex: Some(
        r"
// The key's DEVICE_PIXEL_RATIO, which the pipeline sets.
override DEVICE_PIXEL_RATIO: f32 = 1.0;

struct VertexInput {
    @location(4) pos_normal: vec2<i32>,  // packed position and normal
    @location(5) data: vec4<u32>,        // extrude, direction, linesofar
};

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) v_width2: vec2<f32>,
    @location(1) v_normal: vec2<f32>,
    @location(2) v_gamma_scale: f32,
#ifdef PROJECTION_GLOBE
    @location(6) v_tile_x: f32,
#endif
};

struct TileDebugDrawableUBO {
    matrix: mat4x4<f32>,
    ratio: f32,
    pad0: f32,
    pad1: f32,
    pad2: f32,
    pad3: f32,
    pad4: f32,
    pad5: f32,
    pad6: f32,
};

struct TileDebugPropsUBO {
    color: vec4<f32>,
    blur: f32,
    opacity: f32,
    gapwidth: f32,
    offset: f32,
    width: f32,
    pad0: f32,
    pad1: f32,
    pad2: f32,
};

struct GlobalPaintParamsUBO {
    pattern_atlas_texsize: vec2<f32>,
    units_to_pixels: vec2<f32>,
    world_size: vec2<f32>,
    camera_to_center_distance: f32,
    symbol_fade_change: f32,
    aspect_ratio: f32,
    pixel_ratio: f32,
    map_zoom: f32,
    pad1: f32,
};

struct GlobalIndexUBO {
    value: u32,
    pad0: vec3<u32>,
};

struct TileDebugDrawableElement {
    data: TileDebugDrawableUBO,
    pad0: vec4<f32>,
    pad1: vec4<f32>,
};

@group(0) @binding(0) var<uniform> paintParams: GlobalPaintParamsUBO;
@group(0) @binding(2) var<storage, read> drawableVector: array<TileDebugDrawableElement>;
@group(0) @binding(4) var<storage, read> projectionVector: array<ProjectionUBO>;
@group(0) @binding(5) var<uniform> props: TileDebugPropsUBO;
@group(0) @binding(1) var<uniform> globalIndex: GlobalIndexUBO;

@vertex
fn main(in: VertexInput) -> VertexOutput {
    var out: VertexOutput;
    let drawable = drawableVector[globalIndex.value].data;
    let ratio = max(drawable.ratio, 1e-6);
    let pixel_ratio = DEVICE_PIXEL_RATIO;
    let antialiasing = (1.0 / pixel_ratio) * 0.5;

    // Unpack vertex data
    let a_extrude = vec2<f32>(f32(in.data.x), f32(in.data.y)) - vec2<f32>(128.0, 128.0);
    let a_direction = f32(in.data.z & 3u) - 1.0;
    let raw_pos = vec2<f32>(f32(in.pos_normal.x), f32(in.pos_normal.y));
    let pos = floor(raw_pos * 0.5);
    var normal = raw_pos - pos * 2.0;
    normal.y = normal.y * 2.0 - 1.0;

    let gapwidth = props.gapwidth / 2.0;
    let offset = -props.offset;
    let width = props.width;

    let halfwidth = width * 0.5;
    let inset = gapwidth + select(0.0, antialiasing, gapwidth > 0.0);
    let outset = gapwidth + halfwidth * select(1.0, 2.0, gapwidth > 0.0) +
                 select(0.0, antialiasing, halfwidth != 0.0);

    // Scale the extrusion vector down to a normal and then up by the line width of this vertex
    let dist = outset * a_extrude * LINE_NORMAL_SCALE;

    // Calculate the offset when drawing a line that is to the side of the actual line
    let u = 0.5 * a_direction;
    let t = 1.0 - abs(u);
    let offset2 = offset * a_extrude * LINE_NORMAL_SCALE * normal.y * mat2x2<f32>(t, -u, u, t);

#ifdef PROJECTION_GLOBE
    let adjustedThickness = projectLineThickness(pos.y, projectionVector[globalIndex.value]);
    let projected_no_extrude = projectTile(pos + offset2 / ratio * adjustedThickness, projectionVector[globalIndex.value]);
    let extrudedPos = pos + (offset2 + dist) / ratio * adjustedThickness;
    let base = projectTile(extrudedPos, projectionVector[globalIndex.value]);
    out.v_tile_x = antimeridianClipX(extrudedPos, projectionVector[globalIndex.value]);
    let projected_extrude = base - projected_no_extrude;
#else
    let projected_extrude = drawable.matrix * vec4<f32>(dist / ratio, 0.0, 0.0);
    let base = drawable.matrix * vec4<f32>(pos + offset2 / ratio, 0.0, 1.0) + projected_extrude;
#endif
    let inv_w = 1.0 / base.w;

    out.position = base;

    let extrude_length_without_perspective = length(dist);
    let extrude_length_with_perspective =
        length((projected_extrude.xy / base.w) * paintParams.units_to_pixels);
    let gamma_denom = max(extrude_length_with_perspective, 1e-6);

    out.v_width2 = vec2<f32>(outset, inset);
    out.v_normal = normal;
    out.v_gamma_scale = extrude_length_without_perspective / gamma_denom;

    return out;
}
",
    ),
    fragment: Some(
        r"
// The key's DEVICE_PIXEL_RATIO, which the pipeline sets.
override DEVICE_PIXEL_RATIO: f32 = 1.0;

struct FragmentInput {
    @location(0) v_width2: vec2<f32>,
    @location(1) v_normal: vec2<f32>,
    @location(2) v_gamma_scale: f32,
#ifdef PROJECTION_GLOBE
    @location(6) v_tile_x: f32,
#endif
};

struct TileDebugPropsUBO {
    color: vec4<f32>,
    blur: f32,
    opacity: f32,
    gapwidth: f32,
    offset: f32,
    width: f32,
    pad0: f32,
    pad1: f32,
    pad2: f32,
};

struct GlobalPaintParamsUBO {
    pattern_atlas_texsize: vec2<f32>,
    units_to_pixels: vec2<f32>,
    world_size: vec2<f32>,
    camera_to_center_distance: f32,
    symbol_fade_change: f32,
    aspect_ratio: f32,
    pixel_ratio: f32,
    map_zoom: f32,
    pad1: f32,
};

@group(0) @binding(0) var<uniform> paintParams: GlobalPaintParamsUBO;
@group(0) @binding(5) var<uniform> props: TileDebugPropsUBO;

@fragment
fn main(in: FragmentInput) -> @location(0) vec4<f32> {
#ifdef PROJECTION_GLOBE
    if (clippedAtAntimeridian(in.v_tile_x)) {
        discard;
    }
#endif
#ifdef OVERDRAW_INSPECTOR
    return vec4<f32>(1.0, 1.0, 1.0, 1.0);
#endif
    let color = props.color;
    let blur = props.blur;
    let opacity = props.opacity;

    // Calculate the distance of the pixel from the line in pixels
    let dist = length(in.v_normal) * in.v_width2.x;

    // Calculate the antialiasing fade factor
    let antialiasing = 0.5 / DEVICE_PIXEL_RATIO;
    let blur2 = (blur + (1.0 / DEVICE_PIXEL_RATIO)) * in.v_gamma_scale;
    let denom = max(blur2, 1e-6);
    let alpha = clamp(min(dist - (in.v_width2.y - blur2), in.v_width2.x - dist) / denom, 0.0, 1.0);

    return color * (alpha * opacity);
}
",
    ),
    prelude: None,
};

pub(crate) const PRELUDE: Wgsl = Wgsl {
    vertex: None,
    fragment: None,
    prelude: Some(
        r"
// Constants
const PI: f32 = 3.14159265358979323846;
const LINE_NORMAL_SCALE: f32 = 1.0 / 63.0;  // 1.0 / (127 / 2)
const MAX_LINE_DISTANCE: f32 = 32767.0;
const SDF_PX: f32 = 8.0;

// WebGPU equivalent of GLSL mod function
fn glMod(x: f32, y: f32) -> f32 {
    return x - y * floor(x / y);
}

fn glMod2(x: vec2<f32>, y: f32) -> vec2<f32> {
    return x - y * floor(x / y);
}

fn glMod2v(x: vec2<f32>, y: vec2<f32>) -> vec2<f32> {
    return x - y * floor(x / y);
}

// GLSL compatibility helpers
fn gl_mod(x: vec2<f32>, y: vec2<f32>) -> vec2<f32> {
    return glMod2v(x, y);
}

// Radians conversion
fn radians(degrees: f32) -> f32 {
    return PI * degrees / 180.0;
}

// Helper functions for unpacking data
fn unpack_float(packedValue: f32) -> vec2<f32> {
    let packedIntValue = i32(packedValue);
    let v0 = packedIntValue / 256;
    return vec2<f32>(f32(v0), f32(packedIntValue - v0 * 256));
}

fn unpack_opacity(packedOpacity: f32) -> vec2<f32> {
    let intOpacity = i32(packedOpacity) / 2;
    return vec2<f32>(f32(intOpacity) / 127.0, glMod(packedOpacity, 2.0));
}

// Decode a color that has been packed into two floats
fn decode_color(encoded: vec2<f32>) -> vec4<f32> {
    let e0 = unpack_float(encoded[0]) / 255.0;
    let e1 = unpack_float(encoded[1]) / 255.0;
    return vec4<f32>(e0.x, e0.y, e1.x, e1.y);
}

// Unpack and interpolate between two float values
fn unpack_mix_float(packedValue: vec2<f32>, t: f32) -> f32 {
    return mix(packedValue[0], packedValue[1], t);
}

// Unpack and interpolate between two colors
fn unpack_mix_color(packedColors: vec4<f32>, t: f32) -> vec4<f32> {
    let minColor = decode_color(vec2<f32>(packedColors[0], packedColors[1]));
    let maxColor = decode_color(vec2<f32>(packedColors[2], packedColors[3]));
    return mix(minColor, maxColor, t);
}

// Get pattern position for texture atlas lookups
fn get_pattern_pos(pixel_coord_upper: vec2<f32>, pixel_coord_lower: vec2<f32>,
                   pattern_size: vec2<f32>, tile_units_to_pixels: f32, pos: vec2<f32>) -> vec2<f32> {
    let offset_a = gl_mod(pixel_coord_upper, pattern_size) * 256.0;
    let offset_b = gl_mod(offset_a, pattern_size) * 256.0 + pixel_coord_lower;
    let offset = gl_mod(offset_b, pattern_size);
    return (tile_units_to_pixels * pos + offset) / pattern_size;
}

struct ProjectionUBO {
    matrix: mat4x4<f32>,
    fallback_matrix: mat4x4<f32>,
    tile_mercator_coords: vec4<f32>,
    clipping_plane: vec4<f32>,
    projection_transition: f32,
    depth_offset: f32,
    translate: vec2<f32>,
};

// Pole vertices carry these sentinel Y values in their raw position.
const GLOBE_POLE_NORTH_Y: f32 = -32767.5;
const GLOBE_POLE_SOUTH_Y: f32 = 32766.5;

#ifdef PROJECTION_GLOBE

const GLOBE_RADIUS: f32 = 6371008.8;

// Tile position (0..EXTENT) to a point on the unit sphere; the pole sentinels in rawPos map to the poles.
fn projectToSphere(translatedPos: vec2<f32>, rawPos: vec2<f32>, projection: ProjectionUBO) -> vec3<f32> {
    let mercator_pos = projection.tile_mercator_coords.xy + projection.tile_mercator_coords.zw * translatedPos;
    let spherical_x = mercator_pos.x * PI * 2.0 + PI;
    // sin/cos of the latitude from the Mercator Y via the tangent half-angle identities: no atan, and float32 precision survives near the equator.
    let t = exp(PI - (mercator_pos.y * PI * 2.0));
    let t2 = t * t;
    let denom = t2 + 1.0;
    let sin_sy = (t2 - 1.0) / denom;
    let cos_sy = (2.0 * t) / denom;
    var pos = vec3<f32>(sin(spherical_x) * cos_sy, sin_sy, cos(spherical_x) * cos_sy);
    if (rawPos.y < GLOBE_POLE_NORTH_Y) {
        pos = vec3<f32>(0.0, 1.0, 0.0);
    }
    if (rawPos.y > GLOBE_POLE_SOUTH_Y) {
        pos = vec3<f32>(0.0, -1.0, 0.0);
    }
    return pos;
}

fn globeRotateVector(vec: vec3<f32>, angles: vec2<f32>) -> vec3<f32> {
    var axisRight = vec3<f32>(vec.z, 0.0, -vec.x);
    var axisUp = cross(axisRight, vec);
    axisRight = normalize(axisRight);
    axisUp = normalize(axisUp);
    let t = tan(angles);
    return normalize(vec + axisRight * t.x + axisUp * t.y);
}

// cos(latitude) at a tile Y, from the same exp() form as projectToSphere.
fn circumferenceRatioAtTileY(tileY: f32, projection: ProjectionUBO) -> f32 {
    let mercator_pos_y = projection.tile_mercator_coords.y + projection.tile_mercator_coords.w * tileY;
    let t = exp(PI - (mercator_pos_y * PI * 2.0));
    return (2.0 * t) / (t * t + 1.0);
}

fn projectLineThickness(tileY: f32, projection: ProjectionUBO) -> f32 {
    let thickness = 1.0 / circumferenceRatioAtTileY(tileY, projection);
    if (projection.projection_transition < 0.999) {
        return mix(1.0, thickness, projection.projection_transition);
    }
    return thickness;
}

fn globeComputeClippingZ(spherePos: vec3<f32>, projection: ProjectionUBO) -> f32 {
    return 1.0 - (dot(spherePos, projection.clipping_plane.xyz) + projection.clipping_plane.w);
}

fn interpolateProjection(posInTile: vec2<f32>, spherePos: vec3<f32>, elevation: f32, projection: ProjectionUBO) -> vec4<f32> {
    let elevatedPos = spherePos * (1.0 + elevation / GLOBE_RADIUS);
    var globePosition = projection.matrix * vec4<f32>(elevatedPos, 1.0);
    // Clip the far side of the globe through Z; the layer's depth shift keeps layer order.
    globePosition.z = globeComputeClippingZ(elevatedPos, projection) * globePosition.w - projection.depth_offset;

    if (projection.projection_transition > 0.999) {
        return globePosition;
    }

    let flatPosition = projection.fallback_matrix * vec4<f32>(posInTile, elevation, 1.0);
    let z_globeness_threshold = 0.2;
    var result = globePosition;
    result.z = mix(0.0, globePosition.z, clamp((projection.projection_transition - z_globeness_threshold) / (1.0 - z_globeness_threshold), 0.0, 1.0));
    let xyw = mix(flatPosition.xyw, globePosition.xyw, projection.projection_transition);
    result.x = xyw.x;
    result.y = xyw.y;
    result.w = xyw.z;
    if ((posInTile.y < GLOBE_POLE_NORTH_Y) || (posInTile.y > GLOBE_POLE_SOUTH_Y)) {
        result = globePosition;
        let poles_hidden_anim_percentage = 0.02;
        result.z = mix(globePosition.z, 100.0, pow(max((1.0 - projection.projection_transition) / poles_hidden_anim_percentage, 0.0), 8.0));
    }
    return result;
}

// Keeps the matrix Z, for geometry that needs the depth buffer.
fn interpolateProjectionFor3D(posInTile: vec2<f32>, spherePos: vec3<f32>, elevation: f32, projection: ProjectionUBO) -> vec4<f32> {
    let elevatedPos = spherePos * (1.0 + elevation / GLOBE_RADIUS);
    let globePosition = projection.matrix * vec4<f32>(elevatedPos, 1.0);
    if (projection.projection_transition > 0.999) {
        return globePosition;
    }
    let fallbackPosition = projection.fallback_matrix * vec4<f32>(posInTile, elevation, 1.0);
    return mix(fallbackPosition, globePosition, projection.projection_transition);
}

fn projectTile(pos: vec2<f32>, projection: ProjectionUBO) -> vec4<f32> {
    return interpolateProjection(pos, projectToSphere(pos + projection.translate, vec2<f32>(0.0, 0.0), projection), 0.0, projection);
}

// The zoom 0 tile's buffer wraps around the planet onto the tile itself, so the line shaders discard the fragments
// beyond its X extent; every other tile (a mercator width below the whole world's) hands on a value inside it.
fn antimeridianClipX(posInTile: vec2<f32>, projection: ProjectionUBO) -> f32 {
    return select(0.0, posInTile.x, projection.tile_mercator_coords.z * 8192.0 >= 1.0);
}

fn clippedAtAntimeridian(tileX: f32) -> bool {
    return tileX < 0.0 || tileX >= 8192.0;
}

// The variant for geometry that can carry pole vertices; rawPos is the untranslated position.
fn projectTileWithPoles(pos: vec2<f32>, rawPos: vec2<f32>, projection: ProjectionUBO) -> vec4<f32> {
    return interpolateProjection(pos, projectToSphere(pos + projection.translate, rawPos, projection), 0.0, projection);
}

fn projectTileWithElevation(pos: vec2<f32>, elevation: f32, projection: ProjectionUBO) -> vec4<f32> {
    return interpolateProjection(pos, projectToSphere(pos + projection.translate, vec2<f32>(0.0, 0.0), projection), elevation, projection);
}

fn projectTileFor3D(pos: vec2<f32>, elevation: f32, projection: ProjectionUBO) -> vec4<f32> {
    return interpolateProjectionFor3D(pos, projectToSphere(pos + projection.translate, pos, projection), elevation, projection);
}

#else

fn projectTile(pos: vec2<f32>, projection: ProjectionUBO) -> vec4<f32> {
    return projection.matrix * vec4<f32>(pos, 0.0, 1.0);
}

// Pole vertices only exist on the globe; put them behind the near plane so their triangles are clipped.
fn projectTileWithPoles(pos: vec2<f32>, rawPos: vec2<f32>, projection: ProjectionUBO) -> vec4<f32> {
    var result = projection.matrix * vec4<f32>(pos, 0.0, 1.0);
    if (rawPos.y < GLOBE_POLE_NORTH_Y || rawPos.y > GLOBE_POLE_SOUTH_Y) {
        result.z = -10000000.0;
    }
    return result;
}

fn projectTileWithElevation(pos: vec2<f32>, elevation: f32, projection: ProjectionUBO) -> vec4<f32> {
    return projection.matrix * vec4<f32>(pos, elevation, 1.0);
}

fn projectTileFor3D(pos: vec2<f32>, elevation: f32, projection: ProjectionUBO) -> vec4<f32> {
    return projection.matrix * vec4<f32>(pos, elevation, 1.0);
}

fn projectLineThickness(tileY: f32, projection: ProjectionUBO) -> f32 {
    return 1.0;
}

#endif

// The atlas texture coordinate of an image's origin plus an offset from it,
// both normalized by a power-of-two atlas size. Snapping the offset to 2^-24
// makes the sum exact, so the sampled position, and the pixels, don't depend
// on where the image sits in the atlas. Adding an unsnapped offset rounds it
// to a precision that depends on the origin.
fn atlas_pos(origin: vec2<f32>, offset: vec2<f32>) -> vec2<f32> {
    return origin + round(offset * 16777216.0) / 16777216.0;
}
",
    ),
};

pub(crate) const SKY_SHADER: Wgsl = Wgsl {
    vertex: Some(
        r"
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
    ),
    fragment: Some(
        r"
struct FragmentInput {
    @location(0) pos: vec2<f32>,
};

struct SkyPropsUBO {
    sky_color: vec4<f32>,
    horizon_color: vec4<f32>,
    horizon: vec2<f32>,
    horizon_normal: vec2<f32>,
    viewport_size: vec2<f32>,
    sky_horizon_blend: f32,
    sky_blend: f32,
};

@group(0) @binding(5) var<uniform> sky: SkyPropsUBO;

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
    ),
    prelude: None,
};

pub(crate) const SYMBOL_ICON_SHADER: Wgsl = Wgsl {
    vertex: Some(
        r"
struct VertexInput {
    @location(3) pos_offset: vec4<i32>,
    @location(4) data: vec4<u32>,
    @location(5) pixeloffset: vec4<i32>,
    @location(6) projected_pos: vec3<f32>,
    @location(7) fade_opacity: f32,
#ifndef HAS_UNIFORM_u_opacity
    @location(8) opacity: vec2<f32>,
#endif
    @location(9) tex_origin: vec2<u32>,
    @location(10) height_offset: f32,
};

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) tex: vec2<f32>,
    @location(1) fade_opacity: f32,
#ifndef HAS_UNIFORM_u_opacity
    @location(2) opacity: f32,
#endif
    @location(3) @interpolate(flat) tex_origin: vec2<f32>,
};

@group(0) @binding(0) var<uniform> paintParams: GlobalPaintParamsUBO;
@group(0) @binding(1) var<uniform> globalIndex: GlobalIndexUBO;
@group(0) @binding(2) var<storage, read> drawableVector: array<SymbolDrawableUBO>;
@group(0) @binding(4) var<storage, read> projectionVector: array<ProjectionUBO>;

@vertex
fn main(in: VertexInput) -> VertexOutput {
    var out: VertexOutput;

    let drawable = drawableVector[globalIndex.value];

    let raw_fade_opacity = unpack_opacity(in.fade_opacity);
    let fade_change = select(-paintParams.symbol_fade_change, paintParams.symbol_fade_change, raw_fade_opacity.y > 0.5);
    let fade_opacity = max(0.0, min(1.0, raw_fade_opacity.x + fade_change));

#ifdef HAS_UNIFORM_u_opacity
    let final_opacity = fade_opacity;
#else
    let paint_opacity = unpack_mix_float(in.opacity, drawable.opacity_t);
    let final_opacity = paint_opacity * fade_opacity;
#endif

    // Cull vertices with zero opacity
    if (final_opacity == 0.0) {
        out.position = vec4<f32>(c_offscreen_degenerate_triangle_location,
                                 c_offscreen_degenerate_triangle_location,
                                 c_offscreen_degenerate_triangle_location, 1.0);
        return out;
    }

    let a_pos = vec2<f32>(f32(in.pos_offset.x), f32(in.pos_offset.y));
    let a_offset = vec2<f32>(f32(in.pos_offset.z), f32(in.pos_offset.w));

    let a_tex = vec2<f32>(f32(in.data.x), f32(in.data.y));
    let a_size = vec2<f32>(f32(in.data.z), f32(in.data.w));

    let a_size_min = floor(a_size.x * 0.5);
    let a_pxoffset = vec2<f32>(f32(in.pixeloffset.x), f32(in.pixeloffset.y));
    let a_minFontScale = vec2<f32>(f32(in.pixeloffset.z), f32(in.pixeloffset.w)) / 256.0;

    let segment_angle = -in.projected_pos.z;

    let size_zoom_constant = drawable.is_size_zoom_constant != 0u;
    let size_feature_constant = drawable.is_size_feature_constant != 0u;
    var size: f32;
    if (!size_zoom_constant && !size_feature_constant) {
        size = mix(a_size_min, a_size.y, drawable.size_t) / 128.0;
    } else if (size_zoom_constant && !size_feature_constant) {
        size = a_size_min / 128.0;
    } else {
        size = drawable.size;
    }

    let projection = projectionVector[globalIndex.value];
    let translated_a_pos = a_pos + drawable.translation;
    let projectedPoint = projectTileWithElevation(translated_a_pos, in.height_offset, projection);
#ifdef PROJECTION_GLOBE
    // A screen-aligned label whose anchor is behind the horizon goes at
    // once, rather than fading out once placement next finds it occluded:
    // its clip Z is 0, so the horizon never clips it. A pitched label's
    // vertices go through the globe projection, which clips them.
    // A raised label is left to placement, which hides it once the planet
    // blocks the line of sight to it.
    if (drawable.pitch_with_map == 0u && in.height_offset <= 0.0 && projectedPoint.z > projectedPoint.w) {
        out.position = vec4<f32>(c_offscreen_degenerate_triangle_location,
                                 c_offscreen_degenerate_triangle_location,
                                 c_offscreen_degenerate_triangle_location, 1.0);
        return out;
    }
#endif
    let camera_to_anchor_distance = projectedPoint.w;
    let pitch_with_map = drawable.pitch_with_map != 0u;
    let distance_ratio = select(
        paintParams.camera_to_center_distance / camera_to_anchor_distance,
        camera_to_anchor_distance / paintParams.camera_to_center_distance,
        pitch_with_map
    );
    let perspective_ratio = clamp(0.5 + 0.5 * distance_ratio, 0.0, 4.0);

    if (drawable.is_offset == 0u) {
        size *= perspective_ratio;
    }

    let is_text = drawable.is_text_prop != 0u;
    let fontScale = select(size, size / 24.0, is_text);

    var symbol_rotation = 0.0;
    if (drawable.rotate_symbol != 0u) {
        let offsetProjectedPoint = projectTileWithElevation(translated_a_pos + vec2<f32>(1.0, 0.0), in.height_offset, projection);
        let a = projectedPoint.xy / projectedPoint.w;
        let b = offsetProjectedPoint.xy / offsetProjectedPoint.w;
        symbol_rotation = atan2((b.y - a.y) / paintParams.aspect_ratio, b.x - a.x);
    }

    let angle_sin = sin(segment_angle + symbol_rotation);
    let angle_cos = cos(segment_angle + symbol_rotation);
    let rotation_matrix = mat2x2<f32>(angle_cos, -angle_sin, angle_sin, angle_cos);

    var projected_pos: vec4<f32>;
    if (drawable.is_along_line != 0u || drawable.is_variable_anchor != 0u) {
        projected_pos = vec4<f32>(in.projected_pos.xy, in.height_offset, 1.0);
    } else if (drawable.pitch_with_map != 0u) {
        projected_pos = drawable.label_plane_matrix * vec4<f32>(in.projected_pos.xy + drawable.translation, in.height_offset, 1.0);
    } else {
        projected_pos = drawable.label_plane_matrix * projectTileWithElevation(in.projected_pos.xy + drawable.translation, in.height_offset, projection);
    }
    let z = select(0.0, projected_pos.z / projected_pos.w, drawable.pitch_with_map != 0u);

    var projectionScaling = 1.0;
#ifdef PROJECTION_GLOBE
    if (drawable.pitch_with_map != 0u) {
        let anchor_pos_tile_y = (drawable.coord_matrix * vec4<f32>(projected_pos.xy / projected_pos.w, z, 1.0)).y;
        projectionScaling = mix(projectionScaling, 1.0 / circumferenceRatioAtTileY(anchor_pos_tile_y, projection) * drawable.pitched_scale, projection.projection_transition);
    }
#endif
    let posOffset = a_offset * max(a_minFontScale, vec2<f32>(fontScale)) / 32.0 + a_pxoffset / 16.0;
    var position = drawable.coord_matrix * vec4<f32>(projected_pos.xy / projected_pos.w + rotation_matrix * posOffset * projectionScaling, z, 1.0);
    if (drawable.pitch_with_map != 0u) {
        position = projectTileWithElevation(position.xy, position.z, projection);
    }

    out.position = position;
    let tex_origin = vec2<f32>(f32(in.tex_origin.x), f32(in.tex_origin.y));
    out.tex = (a_tex - tex_origin) / drawable.texsize;
    out.tex_origin = tex_origin / drawable.texsize;
    out.fade_opacity = fade_opacity;
#ifndef HAS_UNIFORM_u_opacity
    out.opacity = final_opacity;
#endif

    return out;
}
",
    ),
    fragment: Some(
        r"
struct FragmentInput {
    @location(0) tex: vec2<f32>,
    @location(1) fade_opacity: f32,
#ifndef HAS_UNIFORM_u_opacity
    @location(2) opacity: f32,
#endif
    @location(3) @interpolate(flat) tex_origin: vec2<f32>,
};

@group(0) @binding(3) var<storage, read> tilePropsVector: array<SymbolTilePropsUBO>;
@group(0) @binding(1) var<uniform> globalIndex: GlobalIndexUBO;
@group(0) @binding(5) var<uniform> props: SymbolEvaluatedPropsUBO;
@group(1) @binding(0) var texture_sampler: sampler;
@group(1) @binding(1) var image: texture_2d<f32>;

@fragment
fn main(in: FragmentInput) -> @location(0) vec4<f32> {
#ifdef OVERDRAW_INSPECTOR
    return vec4<f32>(1.0, 1.0, 1.0, 1.0);
#endif

    let tileProps = tilePropsVector[globalIndex.value];
#ifdef HAS_UNIFORM_u_opacity
    let base_opacity = select(props.icon_opacity, props.text_opacity, tileProps.is_text != 0u);
    let opacity = base_opacity * in.fade_opacity;
#else
    // The vertex stage multiplies the fade opacity in.
    let opacity = in.opacity;
#endif
    return textureSample(image, texture_sampler, atlas_pos(in.tex_origin, in.tex)) * opacity;
}
",
    ),
    prelude: Some(
        r"
// Symbol shader UBO indices
const idGlobalPaintParamsUBO = 0u;
const idGlobalUBOIndex = 1u;
const idSymbolDrawableUBO = 2u;
const idSymbolTilePropsUBO = 3u;
const idSymbolEvaluatedPropsUBO = 4u;
const symbolUBOCount = 5u;

struct GlobalPaintParamsUBO {
    pattern_atlas_texsize: vec2<f32>,
    units_to_pixels: vec2<f32>,
    world_size: vec2<f32>,
    camera_to_center_distance: f32,
    symbol_fade_change: f32,
    aspect_ratio: f32,
    pixel_ratio: f32,
    map_zoom: f32,
    pad1: f32,
};

struct SymbolDrawableUBO {
    matrix: mat4x4<f32>,
    label_plane_matrix: mat4x4<f32>,
    coord_matrix: mat4x4<f32>,

    texsize: vec2<f32>,
    texsize_icon: vec2<f32>,

    is_text_prop: u32,
    rotate_symbol: u32,
    pitch_with_map: u32,
    is_size_zoom_constant: u32,
    is_size_feature_constant: u32,
    is_offset: u32,

    size_t: f32,
    size: f32,

    // Interpolations
    fill_color_t: f32,
    halo_color_t: f32,
    opacity_t: f32,
    halo_width_t: f32,
    halo_blur_t: f32,

    is_along_line: u32,
    is_variable_anchor: u32,
    pitched_scale: f32,
    translation: vec2<f32>,
    gamma_scale: f32,
    is_halo: u32,
};

struct SymbolTilePropsUBO {
    is_text: u32,
    is_halo: u32,
    pad1: f32,
    pad2: f32,
};

struct SymbolEvaluatedPropsUBO {
    text_fill_color: vec4<f32>,
    text_halo_color: vec4<f32>,
    text_opacity: f32,
    text_halo_width: f32,
    text_halo_blur: f32,
    pad1: f32,
    icon_fill_color: vec4<f32>,
    icon_halo_color: vec4<f32>,
    icon_opacity: f32,
    icon_halo_width: f32,
    icon_halo_blur: f32,
    pad2: f32,
};

struct GlobalIndexUBO {
    value: u32,
    pad0: vec3<u32>,
};

const c_offscreen_degenerate_triangle_location: f32 = -2.0;
",
    ),
};

pub(crate) const SYMBOL_SDF_SHADER: Wgsl = Wgsl {
    vertex: Some(
        r"
struct VertexInput {
    @location(3) pos_offset: vec4<i32>,
    @location(4) data: vec4<u32>,
    @location(5) pixeloffset: vec4<i32>,
    @location(6) projected_pos: vec3<f32>,
    @location(7) fade_opacity: f32,
#ifndef HAS_UNIFORM_u_fill_color
    @location(8) fill_color: vec4<f32>,
#endif
#ifndef HAS_UNIFORM_u_halo_color
    @location(9) halo_color: vec4<f32>,
#endif
#ifndef HAS_UNIFORM_u_opacity
    @location(10) opacity: vec2<f32>,
#endif
#ifndef HAS_UNIFORM_u_halo_width
    @location(11) halo_width: vec2<f32>,
#endif
#ifndef HAS_UNIFORM_u_halo_blur
    @location(12) halo_blur: vec2<f32>,
#endif
    @location(13) tex_origin: vec2<u32>,
    @location(14) height_offset: f32,
};

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) tex: vec2<f32>,
    @location(1) fade_opacity: f32,
    @location(2) gamma_scaled: f32,
    @location(3) halo_edge: f32,
#ifndef HAS_UNIFORM_u_fill_color
    @location(4) fill_color: vec4<f32>,
#endif
#ifndef HAS_UNIFORM_u_halo_color
    @location(5) halo_color: vec4<f32>,
#endif
    @location(9) @interpolate(flat) tex_origin: vec2<f32>,
};

@group(0) @binding(0) var<uniform> paintParams: GlobalPaintParamsUBO;
@group(0) @binding(1) var<uniform> globalIndex: GlobalIndexUBO;
@group(0) @binding(2) var<storage, read> drawableVector: array<SymbolDrawableUBO>;
@group(0) @binding(4) var<storage, read> projectionVector: array<ProjectionUBO>;
@group(0) @binding(5) var<uniform> props: SymbolEvaluatedPropsUBO;

@vertex
fn main(in: VertexInput) -> VertexOutput {
    var out: VertexOutput;

    let drawable = drawableVector[globalIndex.value];

    let fade_opacity = unpack_opacity(in.fade_opacity);
    let fade_change = select(-paintParams.symbol_fade_change, paintParams.symbol_fade_change, fade_opacity.y > 0.5);
    let fo = max(0.0, min(1.0, fade_opacity.x + fade_change));

#ifdef HAS_UNIFORM_u_opacity
    let visibility = fo;
#else
    let opacity = unpack_mix_float(in.opacity, drawable.opacity_t);
    let visibility = min(fo, opacity);
#endif

    // Cull vertices with zero opacity
    if (visibility == 0.0) {
        out.position = vec4<f32>(c_offscreen_degenerate_triangle_location,
                                 c_offscreen_degenerate_triangle_location,
                                 c_offscreen_degenerate_triangle_location, 1.0);
        return out;
    }

    let a_pos = vec2<f32>(f32(in.pos_offset.x), f32(in.pos_offset.y));
    let a_offset = vec2<f32>(f32(in.pos_offset.z), f32(in.pos_offset.w));

    let a_tex = vec2<f32>(f32(in.data.x), f32(in.data.y));
    let a_size = vec2<f32>(f32(in.data.z), f32(in.data.w));

    let a_size_min = floor(a_size.x * 0.5);
    let a_pxoffset = vec2<f32>(f32(in.pixeloffset.x), f32(in.pixeloffset.y));

    let segment_angle = -in.projected_pos.z;

    var size: f32;
    let size_zoom_constant = drawable.is_size_zoom_constant != 0u;
    let size_feature_constant = drawable.is_size_feature_constant != 0u;
    if (!size_zoom_constant && !size_feature_constant) {
        size = mix(a_size_min, a_size.y, drawable.size_t) / 128.0;
    } else if (size_zoom_constant && !size_feature_constant) {
        size = a_size_min / 128.0;
    } else {
        size = drawable.size;
    }

    let projection = projectionVector[globalIndex.value];
    let translated_a_pos = a_pos + drawable.translation;
    let projectedPoint = projectTileWithElevation(translated_a_pos, in.height_offset, projection);
#ifdef PROJECTION_GLOBE
    // A screen-aligned label whose anchor is behind the horizon goes at
    // once, rather than fading out once placement next finds it occluded:
    // its clip Z is 0, so the horizon never clips it. A pitched label's
    // vertices go through the globe projection, which clips them.
    // A raised label is left to placement, which hides it once the planet
    // blocks the line of sight to it.
    if (drawable.pitch_with_map == 0u && in.height_offset <= 0.0 && projectedPoint.z > projectedPoint.w) {
        out.position = vec4<f32>(c_offscreen_degenerate_triangle_location,
                                 c_offscreen_degenerate_triangle_location,
                                 c_offscreen_degenerate_triangle_location, 1.0);
        return out;
    }
#endif
    let camera_to_anchor_distance = projectedPoint.w;
    let pitch_with_map = drawable.pitch_with_map != 0u;
    let distance_ratio = select(
        paintParams.camera_to_center_distance / camera_to_anchor_distance,
        camera_to_anchor_distance / paintParams.camera_to_center_distance,
        pitch_with_map
    );
    let perspective_ratio = clamp(0.5 + 0.5 * distance_ratio, 0.0, 4.0);

    if (drawable.is_offset == 0u) {
        size *= perspective_ratio;
    }

    let is_text = drawable.is_text_prop != 0u;
    let fontScale = select(size, size / 24.0, is_text);

    var symbol_rotation = 0.0;
    if (drawable.rotate_symbol != 0u) {
        let offsetProjectedPoint = projectTileWithElevation(translated_a_pos + vec2<f32>(1.0, 0.0), in.height_offset, projection);
        let a = projectedPoint.xy / projectedPoint.w;
        let b = offsetProjectedPoint.xy / offsetProjectedPoint.w;
        symbol_rotation = atan2((b.y - a.y) / paintParams.aspect_ratio, b.x - a.x);
    }

    let angle_sin = sin(segment_angle + symbol_rotation);
    let angle_cos = cos(segment_angle + symbol_rotation);
    let rotation_matrix = mat2x2<f32>(angle_cos, -angle_sin, angle_sin, angle_cos);

    var projected_pos: vec4<f32>;
    if (drawable.is_along_line != 0u || drawable.is_variable_anchor != 0u) {
        projected_pos = vec4<f32>(in.projected_pos.xy, in.height_offset, 1.0);
    } else if (drawable.pitch_with_map != 0u) {
        projected_pos = drawable.label_plane_matrix * vec4<f32>(in.projected_pos.xy + drawable.translation, in.height_offset, 1.0);
    } else {
        projected_pos = drawable.label_plane_matrix * projectTileWithElevation(in.projected_pos.xy + drawable.translation, in.height_offset, projection);
    }
    let z = select(0.0, projected_pos.z / projected_pos.w, drawable.pitch_with_map != 0u);

    var projectionScaling = 1.0;
#ifdef PROJECTION_GLOBE
    if (drawable.pitch_with_map != 0u) {
        let anchor_pos_tile_y = (drawable.coord_matrix * vec4<f32>(projected_pos.xy / projected_pos.w, z, 1.0)).y;
        projectionScaling = mix(projectionScaling, 1.0 / circumferenceRatioAtTileY(anchor_pos_tile_y, projection) * drawable.pitched_scale, projection.projection_transition);
    }
#endif
    let pos_rot = a_offset / 32.0 * fontScale + a_pxoffset;
    var position = drawable.coord_matrix * vec4<f32>(projected_pos.xy / projected_pos.w + rotation_matrix * pos_rot * projectionScaling, z, 1.0);
    if (drawable.pitch_with_map != 0u) {
        position = projectTileWithElevation(position.xy, position.z, projection);
    }

    out.position = position;
    let tex_origin = vec2<f32>(f32(in.tex_origin.x), f32(in.tex_origin.y));
    out.tex = (a_tex - tex_origin) / drawable.texsize;
    out.tex_origin = tex_origin / drawable.texsize;
    let gamma_scale = position.w;
#ifdef HAS_UNIFORM_u_opacity
    out.fade_opacity = fo;
#else
    out.fade_opacity = fo * opacity;
#endif

    // A glyph's edge's gamma, scaled by clip w, and the inner edge of its
    // halo: the same over a quad but for the clip w, which interpolates
    // linearly.
#ifndef HAS_UNIFORM_u_halo_width
    let halo_width = unpack_mix_float(in.halo_width, drawable.halo_width_t);
#else
    let halo_width = select(props.icon_halo_width, props.text_halo_width, drawable.is_text_prop != 0u);
#endif
#ifndef HAS_UNIFORM_u_halo_blur
    let halo_blur = unpack_mix_float(in.halo_blur, drawable.halo_blur_t);
#else
    let halo_blur = select(props.icon_halo_blur, props.text_halo_blur, drawable.is_text_prop != 0u);
#endif
    let EDGE_GAMMA = 0.105 / paintParams.pixel_ratio;
    let halo_blur_gamma = select(0.0, halo_blur * 1.19 / SDF_PX, drawable.is_halo != 0u);
    out.gamma_scaled = (halo_blur_gamma + EDGE_GAMMA) / (fontScale * drawable.gamma_scale) * gamma_scale;
    out.halo_edge = (6.0 - halo_width / fontScale) / SDF_PX;
#ifndef HAS_UNIFORM_u_fill_color
    out.fill_color = unpack_mix_color(in.fill_color, drawable.fill_color_t);
#endif
#ifndef HAS_UNIFORM_u_halo_color
    out.halo_color = unpack_mix_color(in.halo_color, drawable.halo_color_t);
#endif

    return out;
}
",
    ),
    fragment: Some(
        r"
struct FragmentInput {
    @location(0) tex: vec2<f32>,
    @location(1) fade_opacity: f32,
    @location(2) gamma_scaled: f32,
    @location(3) halo_edge: f32,
#ifndef HAS_UNIFORM_u_fill_color
    @location(4) fill_color: vec4<f32>,
#endif
#ifndef HAS_UNIFORM_u_halo_color
    @location(5) halo_color: vec4<f32>,
#endif
    @location(9) @interpolate(flat) tex_origin: vec2<f32>,
};

@group(0) @binding(0) var<uniform> paintParams: GlobalPaintParamsUBO;
@group(0) @binding(3) var<storage, read> tilePropsVector: array<SymbolTilePropsUBO>;
@group(0) @binding(1) var<uniform> globalIndex: GlobalIndexUBO;
@group(0) @binding(5) var<uniform> props: SymbolEvaluatedPropsUBO;
@group(1) @binding(0) var texture_sampler: sampler;
@group(1) @binding(1) var glyph_atlas: texture_2d<f32>;

@fragment
fn main(in: FragmentInput) -> @location(0) vec4<f32> {
#ifdef OVERDRAW_INSPECTOR
    return vec4<f32>(1.0, 1.0, 1.0, 1.0);
#endif

    let tileProps = tilePropsVector[globalIndex.value];
#ifndef HAS_UNIFORM_u_fill_color
    let fill_color = in.fill_color;
#else
    let fill_color = select(props.icon_fill_color, props.text_fill_color, tileProps.is_text != 0u);
#endif
#ifndef HAS_UNIFORM_u_halo_color
    let halo_color = in.halo_color;
#else
    let halo_color = select(props.icon_halo_color, props.text_halo_color, tileProps.is_text != 0u);
#endif
#ifndef HAS_UNIFORM_u_opacity
    let opacity = in.fade_opacity;
#else
    let opacity = select(props.icon_opacity, props.text_opacity, tileProps.is_text != 0u) * in.fade_opacity;
#endif

    let color = select(fill_color, halo_color, tileProps.is_halo != 0u);
    let gamma_scaled = in.gamma_scaled;

    // For halos, offset the inner edge outward to create the ring
    var inner_edge = (256.0 - 64.0) / 256.0;
    if (tileProps.is_halo != 0u) {
        inner_edge = inner_edge + gamma_scaled;
    }

    let sample = textureSample(glyph_atlas, texture_sampler, atlas_pos(in.tex_origin, in.tex));
    let dist = select(sample.a, sample.r, tileProps.is_text != 0u);
    var alpha = smoothstep(inner_edge - gamma_scaled, inner_edge + gamma_scaled, dist);

    // When drawing halos, make the inside transparent so the fill can show through
    if (tileProps.is_halo != 0u) {
        alpha = min(smoothstep(in.halo_edge - gamma_scaled, in.halo_edge + gamma_scaled, dist), 1.0 - alpha);
    }

    let coverage = alpha * opacity;
    let outAlpha = color.a * coverage;

    return vec4<f32>(color.rgb * coverage, outAlpha);
}
",
    ),
    prelude: Some(
        r"
// Symbol shader UBO indices
const idGlobalPaintParamsUBO = 0u;
const idGlobalUBOIndex = 1u;
const idSymbolDrawableUBO = 2u;
const idSymbolTilePropsUBO = 3u;
const idSymbolEvaluatedPropsUBO = 4u;
const symbolUBOCount = 5u;

struct GlobalPaintParamsUBO {
    pattern_atlas_texsize: vec2<f32>,
    units_to_pixels: vec2<f32>,
    world_size: vec2<f32>,
    camera_to_center_distance: f32,
    symbol_fade_change: f32,
    aspect_ratio: f32,
    pixel_ratio: f32,
    map_zoom: f32,
    pad1: f32,
};

struct SymbolDrawableUBO {
    matrix: mat4x4<f32>,
    label_plane_matrix: mat4x4<f32>,
    coord_matrix: mat4x4<f32>,

    texsize: vec2<f32>,
    texsize_icon: vec2<f32>,

    is_text_prop: u32,
    rotate_symbol: u32,
    pitch_with_map: u32,
    is_size_zoom_constant: u32,
    is_size_feature_constant: u32,
    is_offset: u32,

    size_t: f32,
    size: f32,

    // Interpolations
    fill_color_t: f32,
    halo_color_t: f32,
    opacity_t: f32,
    halo_width_t: f32,
    halo_blur_t: f32,

    is_along_line: u32,
    is_variable_anchor: u32,
    pitched_scale: f32,
    translation: vec2<f32>,
    gamma_scale: f32,
    is_halo: u32,
};

struct SymbolTilePropsUBO {
    is_text: u32,
    is_halo: u32,
    pad1: f32,
    pad2: f32,
};

struct SymbolEvaluatedPropsUBO {
    text_fill_color: vec4<f32>,
    text_halo_color: vec4<f32>,
    text_opacity: f32,
    text_halo_width: f32,
    text_halo_blur: f32,
    pad1: f32,
    icon_fill_color: vec4<f32>,
    icon_halo_color: vec4<f32>,
    icon_opacity: f32,
    icon_halo_width: f32,
    icon_halo_blur: f32,
    pad2: f32,
};

struct GlobalIndexUBO {
    value: u32,
    pad0: vec3<u32>,
};

const c_offscreen_degenerate_triangle_location: f32 = -2.0;
",
    ),
};

pub(crate) const SYMBOL_TEXT_AND_ICON_SHADER: Wgsl = Wgsl {
    vertex: Some(
        r"
struct VertexInput {
    @location(3) pos_offset: vec4<i32>,
    @location(4) data: vec4<u32>,
    @location(5) projected_pos: vec3<f32>,
    @location(6) fade_opacity: f32,
#ifndef HAS_UNIFORM_u_fill_color
    @location(7) fill_color: vec4<f32>,
#endif
#ifndef HAS_UNIFORM_u_halo_color
    @location(8) halo_color: vec4<f32>,
#endif
#ifndef HAS_UNIFORM_u_opacity
    @location(9) opacity: vec2<f32>,
#endif
#ifndef HAS_UNIFORM_u_halo_width
    @location(10) halo_width: vec2<f32>,
#endif
#ifndef HAS_UNIFORM_u_halo_blur
    @location(11) halo_blur: vec2<f32>,
#endif
    @location(12) tex_origin: vec2<u32>,
    @location(13) height_offset: f32,
};

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) tex: vec2<f32>,
    @location(1) fade_opacity: f32,
    @location(2) gamma_scaled: f32,
    @location(3) halo_edge: f32,
    @location(4) is_icon: f32,
#ifndef HAS_UNIFORM_u_fill_color
    @location(5) fill_color: vec4<f32>,
#endif
#ifndef HAS_UNIFORM_u_halo_color
    @location(6) halo_color: vec4<f32>,
#endif
    @location(10) @interpolate(flat) tex_origin: vec2<f32>,
};

@group(0) @binding(0) var<uniform> paintParams: GlobalPaintParamsUBO;
@group(0) @binding(1) var<uniform> globalIndex: GlobalIndexUBO;
@group(0) @binding(2) var<storage, read> drawableVector: array<SymbolDrawableUBO>;
@group(0) @binding(4) var<storage, read> projectionVector: array<ProjectionUBO>;
@group(0) @binding(5) var<uniform> props: SymbolEvaluatedPropsUBO;

const SDF = 1.0;
const ICON = 0.0;

@vertex
fn main(in: VertexInput) -> VertexOutput {
    var out: VertexOutput;

    let drawable = drawableVector[globalIndex.value];

    let fade_opacity = unpack_opacity(in.fade_opacity);
    let fade_change = select(-paintParams.symbol_fade_change, paintParams.symbol_fade_change, fade_opacity.y > 0.5);
    let fo = max(0.0, min(1.0, fade_opacity.x + fade_change));

#ifdef HAS_UNIFORM_u_opacity
    let visibility = fo;
#else
    let opacity = unpack_mix_float(in.opacity, drawable.opacity_t);
    let visibility = min(fo, opacity);
#endif

    // Cull vertices with zero opacity
    if (visibility == 0.0) {
        out.position = vec4<f32>(c_offscreen_degenerate_triangle_location,
                                 c_offscreen_degenerate_triangle_location,
                                 c_offscreen_degenerate_triangle_location, 1.0);
        return out;
    }

    let a_pos = vec2<f32>(f32(in.pos_offset.x), f32(in.pos_offset.y));
    let a_offset = vec2<f32>(f32(in.pos_offset.z), f32(in.pos_offset.w));

    let a_tex = vec2<f32>(f32(in.data.x), f32(in.data.y));
    let a_size = vec2<f32>(f32(in.data.z), f32(in.data.w));

    let a_size_min = floor(a_size.x * 0.5);
    let is_sdf = a_size.x - 2.0 * a_size_min;

    let segment_angle = -in.projected_pos.z;

    let size_zoom_constant = drawable.is_size_zoom_constant != 0u;
    let size_feature_constant = drawable.is_size_feature_constant != 0u;
    var size: f32;
    if (!size_zoom_constant && !size_feature_constant) {
        size = mix(a_size_min, a_size.y, drawable.size_t) / 128.0;
    } else if (size_zoom_constant && !size_feature_constant) {
        size = a_size_min / 128.0;
    } else {
        size = drawable.size;
    }

    let projection = projectionVector[globalIndex.value];
    let translated_a_pos = a_pos + drawable.translation;
    let projectedPoint = projectTileWithElevation(translated_a_pos, in.height_offset, projection);
#ifdef PROJECTION_GLOBE
    // A screen-aligned label whose anchor is behind the horizon goes at
    // once, rather than fading out once placement next finds it occluded:
    // its clip Z is 0, so the horizon never clips it. A pitched label's
    // vertices go through the globe projection, which clips them.
    // A raised label is left to placement, which hides it once the planet
    // blocks the line of sight to it.
    if (drawable.pitch_with_map == 0u && in.height_offset <= 0.0 && projectedPoint.z > projectedPoint.w) {
        out.position = vec4<f32>(c_offscreen_degenerate_triangle_location,
                                 c_offscreen_degenerate_triangle_location,
                                 c_offscreen_degenerate_triangle_location, 1.0);
        return out;
    }
#endif
    let camera_to_anchor_distance = projectedPoint.w;
    let pitch_with_map = drawable.pitch_with_map != 0u;
    let distance_ratio = select(
        paintParams.camera_to_center_distance / camera_to_anchor_distance,
        camera_to_anchor_distance / paintParams.camera_to_center_distance,
        pitch_with_map
    );
    let perspective_ratio = clamp(0.5 + 0.5 * distance_ratio, 0.0, 4.0);

    if (drawable.is_offset == 0u) {
        size *= perspective_ratio;
    }

    let fontScale = size / 24.0;

    var symbol_rotation = 0.0;
    if (drawable.rotate_symbol != 0u) {
        let offsetProjectedPoint = projectTileWithElevation(translated_a_pos + vec2<f32>(1.0, 0.0), in.height_offset, projection);
        let a = projectedPoint.xy / projectedPoint.w;
        let b = offsetProjectedPoint.xy / offsetProjectedPoint.w;
        symbol_rotation = atan2((b.y - a.y) / paintParams.aspect_ratio, b.x - a.x);
    }

    let angle_sin = sin(segment_angle + symbol_rotation);
    let angle_cos = cos(segment_angle + symbol_rotation);
    let rotation_matrix = mat2x2<f32>(angle_cos, -angle_sin, angle_sin, angle_cos);

    var projected_pos: vec4<f32>;
    if (drawable.is_along_line != 0u || drawable.is_variable_anchor != 0u) {
        projected_pos = vec4<f32>(in.projected_pos.xy, in.height_offset, 1.0);
    } else if (drawable.pitch_with_map != 0u) {
        projected_pos = drawable.label_plane_matrix * vec4<f32>(in.projected_pos.xy + drawable.translation, in.height_offset, 1.0);
    } else {
        projected_pos = drawable.label_plane_matrix * projectTileWithElevation(in.projected_pos.xy + drawable.translation, in.height_offset, projection);
    }
    let z = select(0.0, projected_pos.z / projected_pos.w, drawable.pitch_with_map != 0u);

    var projectionScaling = 1.0;
#ifdef PROJECTION_GLOBE
    if (drawable.pitch_with_map != 0u && drawable.is_along_line == 0u) {
        let anchor_pos_tile_y = (drawable.coord_matrix * vec4<f32>(projected_pos.xy / projected_pos.w, z, 1.0)).y;
        projectionScaling = mix(projectionScaling, 1.0 / circumferenceRatioAtTileY(anchor_pos_tile_y, projection) * drawable.pitched_scale, projection.projection_transition);
    }
#endif
    let pos_rot = a_offset / 32.0 * fontScale;
    var position = drawable.coord_matrix * vec4<f32>(projected_pos.xy / projected_pos.w + rotation_matrix * pos_rot * projectionScaling, z, 1.0);
    if (drawable.pitch_with_map != 0u) {
        position = projectTileWithElevation(position.xy, position.z, projection);
    }
    let gamma_scale = position.w;
    let is_icon = select(1.0, 0.0, is_sdf == SDF);

    out.position = position;

    let inv_tex = 1.0 / select(drawable.texsize, drawable.texsize_icon, is_icon != 0.0);
    let tex_origin = vec2<f32>(f32(in.tex_origin.x), f32(in.tex_origin.y));
    out.tex = (a_tex - tex_origin) * inv_tex;
    out.tex_origin = tex_origin * inv_tex;
#ifdef HAS_UNIFORM_u_opacity
    out.fade_opacity = fo;
#else
    out.fade_opacity = fo * opacity;
#endif

    // A glyph's edge's gamma, scaled by clip w, and the inner edge of its
    // halo: the same over a quad but for the clip w, which interpolates
    // linearly.
#ifndef HAS_UNIFORM_u_halo_width
    let halo_width = unpack_mix_float(in.halo_width, drawable.halo_width_t);
#else
    let halo_width = select(props.icon_halo_width, props.text_halo_width, drawable.is_text_prop != 0u);
#endif
#ifndef HAS_UNIFORM_u_halo_blur
    let halo_blur = unpack_mix_float(in.halo_blur, drawable.halo_blur_t);
#else
    let halo_blur = select(props.icon_halo_blur, props.text_halo_blur, drawable.is_text_prop != 0u);
#endif
    let EDGE_GAMMA = 0.105 / paintParams.pixel_ratio;
    let halo_blur_gamma = select(0.0, halo_blur * 1.19 / SDF_PX, drawable.is_halo != 0u);
    out.gamma_scaled = (halo_blur_gamma + EDGE_GAMMA) / (fontScale * drawable.gamma_scale) * gamma_scale;
    out.halo_edge = (6.0 - halo_width / fontScale) / SDF_PX;
    out.is_icon = is_icon;

#ifndef HAS_UNIFORM_u_fill_color
    out.fill_color = unpack_mix_color(in.fill_color, drawable.fill_color_t);
#endif
#ifndef HAS_UNIFORM_u_halo_color
    out.halo_color = unpack_mix_color(in.halo_color, drawable.halo_color_t);
#endif

    return out;
}
",
    ),
    fragment: Some(
        r"
struct FragmentInput {
    @location(0) tex: vec2<f32>,
    @location(1) fade_opacity: f32,
    @location(2) gamma_scaled: f32,
    @location(3) halo_edge: f32,
    @location(4) is_icon: f32,
#ifndef HAS_UNIFORM_u_fill_color
    @location(5) fill_color: vec4<f32>,
#endif
#ifndef HAS_UNIFORM_u_halo_color
    @location(6) halo_color: vec4<f32>,
#endif
    @location(10) @interpolate(flat) tex_origin: vec2<f32>,
};

@group(0) @binding(0) var<uniform> paintParams: GlobalPaintParamsUBO;
@group(0) @binding(3) var<storage, read> tilePropsVector: array<SymbolTilePropsUBO>;
@group(0) @binding(1) var<uniform> globalIndex: GlobalIndexUBO;
@group(0) @binding(5) var<uniform> props: SymbolEvaluatedPropsUBO;
@group(1) @binding(0) var glyph_sampler: sampler;
@group(1) @binding(1) var glyph_image: texture_2d<f32>;
@group(1) @binding(2) var icon_sampler: sampler;
@group(1) @binding(3) var icon_image: texture_2d<f32>;

@fragment
fn main(in: FragmentInput) -> @location(0) vec4<f32> {
#ifdef OVERDRAW_INSPECTOR
    return vec4<f32>(1.0, 1.0, 1.0, 1.0);
#endif

    let tileProps = tilePropsVector[globalIndex.value];
#ifndef HAS_UNIFORM_u_fill_color
    let fill_color = in.fill_color;
#else
    let fill_color = select(props.icon_fill_color, props.text_fill_color, tileProps.is_text != 0u);
#endif
#ifndef HAS_UNIFORM_u_halo_color
    let halo_color = in.halo_color;
#else
    let halo_color = select(props.icon_halo_color, props.text_halo_color, tileProps.is_text != 0u);
#endif
#ifndef HAS_UNIFORM_u_opacity
    let opacity = in.fade_opacity;
#else
    let opacity = select(props.icon_opacity, props.text_opacity, tileProps.is_text != 0u) * in.fade_opacity;
#endif

    if (in.is_icon != 0.0) {
        let alpha = opacity;
        let iconSample = textureSampleLevel(icon_image, icon_sampler, atlas_pos(in.tex_origin, in.tex), 0.0);
        return iconSample * alpha;
    }

    let color = select(fill_color, halo_color, tileProps.is_halo != 0u);
    let gamma_scaled = in.gamma_scaled;

    // For halos, offset the inner edge outward to create the ring
    var inner_edge = (256.0 - 64.0) / 256.0;
    if (tileProps.is_halo != 0u) {
        inner_edge = inner_edge + gamma_scaled;
    }

    let sample = textureSampleLevel(glyph_image, glyph_sampler, atlas_pos(in.tex_origin, in.tex), 0.0);
    let dist = select(sample.a, sample.r, tileProps.is_text != 0u);
    var alpha = smoothstep(inner_edge - gamma_scaled, inner_edge + gamma_scaled, dist);

    // When drawing halos, make the inside transparent so the fill can show through
    if (tileProps.is_halo != 0u) {
        alpha = min(smoothstep(in.halo_edge - gamma_scaled, in.halo_edge + gamma_scaled, dist), 1.0 - alpha);
    }

    let coverage = alpha * opacity;
    let outAlpha = color.a * coverage;

    return vec4<f32>(color.rgb * coverage, outAlpha);

}
",
    ),
    prelude: Some(
        r"
// Symbol shader UBO indices
const idGlobalPaintParamsUBO = 0u;
const idGlobalUBOIndex = 1u;
const idSymbolDrawableUBO = 2u;
const idSymbolTilePropsUBO = 3u;
const idSymbolEvaluatedPropsUBO = 4u;
const symbolUBOCount = 5u;

struct GlobalPaintParamsUBO {
    pattern_atlas_texsize: vec2<f32>,
    units_to_pixels: vec2<f32>,
    world_size: vec2<f32>,
    camera_to_center_distance: f32,
    symbol_fade_change: f32,
    aspect_ratio: f32,
    pixel_ratio: f32,
    map_zoom: f32,
    pad1: f32,
};

struct SymbolDrawableUBO {
    matrix: mat4x4<f32>,
    label_plane_matrix: mat4x4<f32>,
    coord_matrix: mat4x4<f32>,

    texsize: vec2<f32>,
    texsize_icon: vec2<f32>,

    is_text_prop: u32,
    rotate_symbol: u32,
    pitch_with_map: u32,
    is_size_zoom_constant: u32,
    is_size_feature_constant: u32,
    is_offset: u32,

    size_t: f32,
    size: f32,

    // Interpolations
    fill_color_t: f32,
    halo_color_t: f32,
    opacity_t: f32,
    halo_width_t: f32,
    halo_blur_t: f32,

    is_along_line: u32,
    is_variable_anchor: u32,
    pitched_scale: f32,
    translation: vec2<f32>,
    gamma_scale: f32,
    is_halo: u32,
};

struct SymbolTilePropsUBO {
    is_text: u32,
    is_halo: u32,
    pad1: f32,
    pad2: f32,
};

struct SymbolEvaluatedPropsUBO {
    text_fill_color: vec4<f32>,
    text_halo_color: vec4<f32>,
    text_opacity: f32,
    text_halo_width: f32,
    text_halo_blur: f32,
    pad1: f32,
    icon_fill_color: vec4<f32>,
    icon_halo_color: vec4<f32>,
    icon_opacity: f32,
    icon_halo_width: f32,
    icon_halo_blur: f32,
    pad2: f32,
};

struct GlobalIndexUBO {
    value: u32,
    pad0: vec3<u32>,
};

const c_offscreen_degenerate_triangle_location: f32 = -2.0;
",
    ),
};
