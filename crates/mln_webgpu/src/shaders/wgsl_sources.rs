//! The WGSL of the built-in shaders, which started as MapLibre Native's.

use super::shader_source::WgslShaderSource;

/// What the terrain variant of a program that hides behind the ground in
/// front of it reads of the ground's depth, within `#ifdef TERRAIN` of its
/// vertex stage: whether that ground hides a point (`ground_visibility`).
macro_rules! ground_visibility {
    () => {
        r"
#ifdef TERRAIN
// GL JS's `depthOpacity`: 1 where a point `w` from the eye, at `ndc`, is in
// front of the ground drawn there, fading to 0 behind it over GL JS's ramp
// of its NDC depth, which `depth_factor` scales; 1 where no ground is drawn
// there or `ndc` is outside the view.
fn ground_depth_opacity(ndc: vec2<f32>, w: f32, depth_factor: f32) -> f32 {
    if (any(abs(ndc) > vec2<f32>(1.0))) {
        return 1.0;
    }
    let size = vec2<i32>(textureDimensions(ground_depth));
    // The texels' rows run from the top.
    let pos = (vec2<f32>(ndc.x, -ndc.y) * 0.5 + 0.5) * vec2<f32>(size);
    let texel = clamp(vec2<i32>(floor(pos)), vec2<i32>(0), size - 1);
    let bytes = vec4<u32>(round(textureLoad(ground_depth, texel, 0) * 255.0));
    let ground = bitcast<f32>(bytes.x | (bytes.y << 8u) | (bytes.z << 16u) | (bytes.w << 24u));
    if (!(ground > 0.0)) {
        return 1.0;
    }
    let d = depth_factor * (1.0 / w - 1.0 / ground) + 0.0001;
    return 1.0 - clamp(-500.0 * d, 0.0, 1.0);
}

// GL JS's `calculate_visibility` of the point at `clip`: 1 where the ground
// drawn in front doesn't hide it, 0 where it does, averaged with the point
// 0.01 NDC above it where it is partly hidden. The ramp's scale is GL JS's:
// `2n` of a near plane `n` a fiftieth of the view's height, in pixels, a
// `GlobalPaintParamsUBO`'s `world_size.y / pixel_ratio`.
fn ground_visibility(clip: vec4<f32>, view_height: f32) -> f32 {
    if (clip.w <= 0.0) {
        return 1.0;
    }
    let ndc = clip.xy / clip.w;
    let depth_factor = view_height / 25.0;
    let visibility = ground_depth_opacity(ndc, clip.w, depth_factor);
    if (visibility > 0.95) {
        return 1.0;
    }
    return (visibility + ground_depth_opacity(ndc + vec2<f32>(0.0, 0.01), clip.w, depth_factor)) / 2.0;
}
#endif
"
    };
}

pub(crate) const ATMOSPHERE_SHADER: WgslShaderSource = WgslShaderSource {
    vertex: Some(
        r"
struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) view_direction: vec3<f32>,
};

@vertex
fn main(in: VertexInput) -> VertexOutput {
    let pos = vec2<f32>(f32(in.pos.x), f32(in.pos.y));
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

/// `get_pattern_pos`, the position in a pattern of a point in a tile, for
/// the programs that draw a pattern.
macro_rules! pattern_pos {
    () => {
        r"
// Get pattern position for texture atlas lookups
fn get_pattern_pos(pixel_coord_upper: vec2<f32>, pixel_coord_lower: vec2<f32>,
                   pattern_size: vec2<f32>, tile_units_to_pixels: f32, pos: vec2<f32>) -> vec2<f32> {
    let offset_a = gl_mod(pixel_coord_upper, pattern_size) * 256.0;
    let offset_b = gl_mod(offset_a, pattern_size) * 256.0 + pixel_coord_lower;
    let offset = gl_mod(offset_b, pattern_size);
    return (tile_units_to_pixels * pos + offset) / pattern_size;
}
"
    };
}
pub(crate) use pattern_pos;

pub(crate) const BACKGROUND_PATTERN_SHADER: WgslShaderSource = WgslShaderSource {
    vertex: Some(concat!(
        pattern_pos!(),
        r"
struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) pos_a: vec2<f32>,
    @location(1) pos_b: vec2<f32>,
};

@vertex
fn main(in: VertexInput) -> VertexOutput {
    var out: VertexOutput;

    let drawable = drawableVector[globalIndex.value];

    let pos = vec2<f32>(f32(in.pos.x), f32(in.pos.y));

    // Use get_pattern_pos helper function for pattern positioning
    out.pos_a = get_pattern_pos(
        drawable.pixel_coord_upper,
        drawable.pixel_coord_lower,
        props.scale_a * props.pattern_size_a,
        drawable.tile_units_to_pixels,
        pos
    );

    out.pos_b = get_pattern_pos(
        drawable.pixel_coord_upper,
        drawable.pixel_coord_lower,
        props.scale_b * props.pattern_size_b,
        drawable.tile_units_to_pixels,
        pos
    );

    let clip = projectTileWithPoles(pos, pos, projectionVector[globalIndex.value]);
    out.position = clip;
    return out;
}
",
    )),
    fragment: Some(
        r"
struct FragmentInput {
    @location(0) pos_a: vec2<f32>,
    @location(1) pos_b: vec2<f32>,
};

@fragment
fn main(in: FragmentInput) -> @location(0) vec4<f32> {
#ifdef OVERDRAW_INSPECTOR
    return vec4<f32>(1.0, 1.0, 1.0, 1.0);
#endif

    let texsize = paintParams.pattern_atlas_texsize;

    // A crossfade at 0 or 1 shows one pattern, so skip the other.
    var color_a = vec4<f32>(0.0);
    if (props.mix < 1.0) {
        let imagecoord_a = gl_mod(in.pos_a, vec2<f32>(1.0));
        let pos_a = atlas_pos(props.pattern_tl_a / texsize, (props.pattern_br_a - props.pattern_tl_a) / texsize * imagecoord_a);
        color_a = textureSample(image, image_sampler, pos_a);
    }
    var color_b = vec4<f32>(0.0);
    if (props.mix > 0.0) {
        let imagecoord_b = gl_mod(in.pos_b, vec2<f32>(1.0));
        let pos_b = atlas_pos(props.pattern_tl_b / texsize, (props.pattern_br_b - props.pattern_tl_b) / texsize * imagecoord_b);
        color_b = textureSample(image, image_sampler, pos_b);
    }

    // Mix patterns and apply opacity
    return mix(color_a, color_b, props.mix) * props.opacity;
}
",
    ),
    prelude: None,
};

pub(crate) const BACKGROUND_SHADER: WgslShaderSource = WgslShaderSource {
    vertex: Some(
        r"
struct VertexOutput {
    @builtin(position) position: vec4<f32>,
};

@vertex
fn main(in: VertexInput) -> VertexOutput {
    var out: VertexOutput;
    let drawable = drawableVector[globalIndex.value];
    let pos = vec2<f32>(f32(in.pos.x), f32(in.pos.y));
    let clip = projectTileWithPoles(pos, pos, projectionVector[globalIndex.value]);
    out.position = clip;
    return out;
}
",
    ),
    fragment: Some(
        r"
@fragment
fn main() -> @location(0) vec4<f32> {
#ifdef OVERDRAW_INSPECTOR
    return vec4<f32>(1.0, 1.0, 1.0, 1.0);
#endif

    return props.color * props.opacity;
}
",
    ),
    prelude: None,
};

pub(crate) const CIRCLE_SHADER: WgslShaderSource = WgslShaderSource {
    vertex: Some(concat!(
        ground_visibility!(),
        r"
// The key's DEVICE_PIXEL_RATIO, which the pipeline sets.
override DEVICE_PIXEL_RATIO: f32 = 1.0;

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) extrude: vec2<f32>,
    @location(1) antialiasblur: f32,
#ifndef HAS_UNIFORM_u_color
    @location(2) color: vec4<f32>,
#endif
#ifndef HAS_UNIFORM_u_radius
    @location(3) radius: f32,
#endif
#ifndef HAS_UNIFORM_u_blur
    @location(4) blur: f32,
#endif
#ifndef HAS_UNIFORM_u_opacity
    @location(5) opacity: f32,
#endif
#ifndef HAS_UNIFORM_u_stroke_color
    @location(6) stroke_color: vec4<f32>,
#endif
#ifndef HAS_UNIFORM_u_stroke_width
    @location(7) stroke_width: f32,
#endif
#ifndef HAS_UNIFORM_u_stroke_opacity
    @location(8) stroke_opacity: f32,
#endif
#ifdef TERRAIN
    // How much of the circle the ground in front of its center leaves.
    @location(9) visibility: f32,
#endif
};

@vertex
fn main(in: VertexInput) -> VertexOutput {
    var out: VertexOutput;

    let drawable = drawableVector[globalIndex.value];
    let scale_with_map = props.scale_with_map != 0;
    let pitch_with_map = props.pitch_with_map != 0;

    let pos_f = vec2<f32>(f32(in.pos.x), f32(in.pos.y));
    let extrude = glMod2v(pos_f, vec2<f32>(2.0, 2.0)) * 2.0 - vec2<f32>(1.0, 1.0);
    let scaled_extrude = extrude * drawable.extrude_scale;
    let circle_center = floor(pos_f * 0.5);

#ifdef HAS_UNIFORM_u_radius
    let radius = props.radius;
#else
    let radius = unpack_mix_float(in.radius, drawable.radius_t);
    out.radius = radius;
#endif

#ifdef HAS_UNIFORM_u_stroke_width
    let stroke_width = props.stroke_width;
#else
    let stroke_width = unpack_mix_float(in.stroke_width, drawable.stroke_width_t);
    out.stroke_width = stroke_width;
#endif

#ifndef HAS_UNIFORM_u_color
    out.color = unpack_mix_color(in.color, drawable.color_t);
#endif
#ifndef HAS_UNIFORM_u_blur
    out.blur = unpack_mix_float(in.blur, drawable.blur_t);
#endif
#ifndef HAS_UNIFORM_u_opacity
    out.opacity = unpack_mix_float(in.opacity, drawable.opacity_t);
#endif
#ifndef HAS_UNIFORM_u_stroke_color
    out.stroke_color = unpack_mix_color(in.stroke_color, drawable.stroke_color_t);
#endif
#ifndef HAS_UNIFORM_u_stroke_opacity
    out.stroke_opacity = unpack_mix_float(in.stroke_opacity, drawable.stroke_opacity_t);
#endif

    let radius_with_stroke = radius + stroke_width;

    let projection = projectionVector[globalIndex.value];
    // Over terrain, the circle stands on the ground under its center.
#ifdef TERRAIN
    let ele = in.ground_height;
    let view_height = paintParams.world_size.y / paintParams.pixel_ratio;
#ifdef PROJECTION_GLOBE
    out.visibility = ground_visibility(projectTileWithElevation(circle_center, ele, projection), view_height);
#else
    out.visibility = ground_visibility(drawable.matrix * vec4<f32>(circle_center, ele, 1.0), view_height);
#endif
#else
    let ele = 0.0;
#endif
    var position: vec4<f32>;
    if (pitch_with_map) {
#ifdef PROJECTION_GLOBE
        let center_vector = projectToSphere(circle_center + projection.translate, vec2<f32>(0.0, 0.0), projection);
        var angle_scale = drawable.globe_extrude_scale;
#endif
        var corner_position = circle_center;
        if (scale_with_map) {
            corner_position += scaled_extrude * radius_with_stroke;
#ifdef PROJECTION_GLOBE
            angle_scale *= radius_with_stroke;
#endif
        } else {
#ifdef PROJECTION_GLOBE
            let projected_center = interpolateProjection(circle_center, center_vector, ele, projection);
            angle_scale *= radius_with_stroke * (projected_center.w / paintParams.camera_to_center_distance);
#else
            let projected_center = drawable.matrix * vec4<f32>(circle_center, ele, 1.0);
#endif
            corner_position += scaled_extrude * radius_with_stroke *
                               (projected_center.w / paintParams.camera_to_center_distance);
        }
#ifdef PROJECTION_GLOBE
        let corner_vector = globeRotateVector(center_vector, extrude * angle_scale, projection);
        position = interpolateProjection(corner_position, corner_vector, ele, projection);
#else
        position = drawable.matrix * vec4<f32>(corner_position, ele, 1.0);
#endif
    } else {
#ifdef PROJECTION_GLOBE
        position = projectTileWithElevation(circle_center, ele, projection);
        if (position.z / position.w > 1.0) {
            position.x = 10000.0;
            position.y = 10000.0;
        }
#else
        position = drawable.matrix * vec4<f32>(circle_center, ele, 1.0);
#endif
        var factor = position.w;
        if (scale_with_map) {
            factor = paintParams.camera_to_center_distance;
        }
        let delta = scaled_extrude * radius_with_stroke * factor;
        position = vec4<f32>(position.x + delta.x, position.y + delta.y, position.z, position.w);
    }

    let antialiasblur = 1.0 / max(DEVICE_PIXEL_RATIO * radius_with_stroke, 1e-6);

    out.position = position;
    out.extrude = extrude;
    out.antialiasblur = antialiasblur;

    return out;
}
"
    )),
    fragment: Some(
        r"
struct FragmentInput {
    @location(0) extrude: vec2<f32>,
    @location(1) antialiasblur: f32,
#ifndef HAS_UNIFORM_u_color
    @location(2) color: vec4<f32>,
#endif
#ifndef HAS_UNIFORM_u_radius
    @location(3) radius: f32,
#endif
#ifndef HAS_UNIFORM_u_blur
    @location(4) blur: f32,
#endif
#ifndef HAS_UNIFORM_u_opacity
    @location(5) opacity: f32,
#endif
#ifndef HAS_UNIFORM_u_stroke_color
    @location(6) stroke_color: vec4<f32>,
#endif
#ifndef HAS_UNIFORM_u_stroke_width
    @location(7) stroke_width: f32,
#endif
#ifndef HAS_UNIFORM_u_stroke_opacity
    @location(8) stroke_opacity: f32,
#endif
#ifdef TERRAIN
    @location(9) visibility: f32,
#endif
};

@fragment
fn main(in: FragmentInput) -> @location(0) vec4<f32> {
#ifdef OVERDRAW_INSPECTOR
    return vec4<f32>(1.0, 1.0, 1.0, 1.0);
#endif

    let extrude_length = length(in.extrude);
    let antialiasblur = in.antialiasblur;
#ifdef HAS_UNIFORM_u_color
    let color = props.color;
#else
    let color = in.color;
#endif
#ifdef HAS_UNIFORM_u_radius
    let radius = props.radius;
#else
    let radius = in.radius;
#endif
#ifdef HAS_UNIFORM_u_blur
    let blur = props.blur;
#else
    let blur = in.blur;
#endif
#ifdef HAS_UNIFORM_u_opacity
    let opacity = props.opacity;
#else
    let opacity = in.opacity;
#endif
#ifdef HAS_UNIFORM_u_stroke_color
    let stroke_color = props.stroke_color;
#else
    let stroke_color = in.stroke_color;
#endif
#ifdef HAS_UNIFORM_u_stroke_width
    let stroke_width = props.stroke_width;
#else
    let stroke_width = in.stroke_width;
#endif
#ifdef HAS_UNIFORM_u_stroke_opacity
    let stroke_opacity = props.stroke_opacity;
#else
    let stroke_opacity = in.stroke_opacity;
#endif
    let antialiased_blur = -max(blur, antialiasblur);

    let opacity_t = smoothstep(0.0, antialiased_blur, extrude_length - 1.0);

    var color_t: f32;
    if (stroke_width < 0.01) {
        color_t = 0.0;
    } else {
        color_t = smoothstep(antialiased_blur, 0.0, extrude_length - radius / (radius + stroke_width));
    }

    let final_color = mix(color * opacity, stroke_color * stroke_opacity, color_t);
#ifdef TERRAIN
    return opacity_t * final_color * in.visibility;
#else
    return opacity_t * final_color;
#endif
}
",
    ),
    prelude: None,
};

pub(crate) const CLIPPING_MASK_PROGRAM: WgslShaderSource = WgslShaderSource {
    vertex: Some(
        r"
struct VertexOutput {
    @builtin(position) position: vec4<f32>,
};

@vertex
fn main(in: VertexInput) -> VertexOutput {
    var out: VertexOutput;
    let pos = vec2<f32>(f32(in.pos.x), f32(in.pos.y));
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

pub(crate) const COLLISION_BOX_SHADER: WgslShaderSource = WgslShaderSource {
    vertex: Some(
        r"
struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) placed: f32,
    @location(1) not_used: f32,
};

@vertex
fn main(in: VertexInput) -> VertexOutput {
    var out: VertexOutput;

#ifdef PROJECTION_GLOBE
    let projected_point = projectTileWithElevation(vec2<f32>(f32(in.anchor_pos.x), f32(in.anchor_pos.y)), in.height_offset, projection);
#else
    let projected_point = drawable.matrix * vec4<f32>(f32(in.anchor_pos.x), f32(in.anchor_pos.y), in.height_offset, 1.0);
#endif
    let camera_to_anchor_distance = projected_point.w;

    let collision_perspective_ratio = clamp(
        0.5 + 0.5 * (paintParams.camera_to_center_distance / camera_to_anchor_distance),
        0.0,
        4.0);

#ifdef PROJECTION_GLOBE
    out.position = projectTileWithElevation(vec2<f32>(f32(in.pos.x), f32(in.pos.y)), in.height_offset, projection);
    // Boxes just past the horizon stay visible, as in GL JS; ones far behind it are clipped.
    if (out.position.z / out.position.w < 1.1) {
        out.position.z = 0.0;
    }
#else
    out.position = drawable.matrix * vec4<f32>(f32(in.pos.x), f32(in.pos.y), in.height_offset, 1.0);
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

pub(crate) const COLLISION_CIRCLE_SHADER: WgslShaderSource = WgslShaderSource {
    vertex: Some(
        r"
struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) placed: f32,
    @location(1) not_used: f32,
    @location(2) radius: f32,
    @location(3) extrude: vec2<f32>,
    @location(4) extrude_scale: vec2<f32>,
};

@vertex
fn main(in: VertexInput) -> VertexOutput {
    var out: VertexOutput;



#ifdef PROJECTION_GLOBE
    let projected_point = projectTile(vec2<f32>(f32(in.anchor_pos.x), f32(in.anchor_pos.y)), projection);
#else
    let projected_point = drawable.matrix * vec4<f32>(f32(in.anchor_pos.x), f32(in.anchor_pos.y), 0.0, 1.0);
#endif
    let camera_to_anchor_distance = projected_point.w;

    let perspective = max(camera_to_anchor_distance, 1e-6);
    let collision_perspective_ratio = clamp(
        0.5 + 0.5 * (paintParams.camera_to_center_distance / perspective),
        0.0,
        4.0);

#ifdef PROJECTION_GLOBE
    var position = projectTile(vec2<f32>(f32(in.pos.x), f32(in.pos.y)), projection);
    // Circles just past the horizon stay visible, as in GL JS; ones far behind it are clipped.
    if (position.z / position.w < 1.1) {
        position.z = 0.0;
    }
#else
    var position = drawable.matrix * vec4<f32>(f32(in.pos.x), f32(in.pos.y), 0.0, 1.0);
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

pub(crate) const COLOR_RELIEF_SHADER: WgslShaderSource = WgslShaderSource {
    vertex: Some(
        r"
struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) frag_position: vec2<f32>,
};

@vertex
fn main(in: VertexInput) -> VertexOutput {
    var out: VertexOutput;
    let drawable = drawableVector[globalIndex.value];
    let tileProps = tilePropsVector[globalIndex.value];

    let pos = vec2<f32>(f32(in.pos.x), f32(in.pos.y));
    out.position = projectTileWithPoles(pos, pos, projectionVector[globalIndex.value]);

    let a_pos = pos;
    let epsilon = vec2<f32>(1.0, 1.0) / tileProps.dimension;
    let scale = (tileProps.dimension.x - 2.0) / tileProps.dimension.x;
    out.frag_position = (a_pos / 8192.0) * scale + epsilon;

    // Handle poles
    out.frag_position.y = select(out.frag_position.y, 0.0, a_pos.y < -32767.5);
    out.frag_position.y = select(out.frag_position.y, 1.0, a_pos.y > 32766.5);

    return out;
}
",
    ),
    fragment: Some(
        r"
struct FragmentInput {
    @location(0) frag_position: vec2<f32>,
};

// The height in meters of the DEM at `coord`, by the rule of
// `mln_util::dem`. The channels aren't rounded: the decode is linear in
// them, so where a linear sampler interpolates them, it interpolates the
// heights.
fn getElevation(coord: vec2<f32>, unpack: vec4<f32>, unpack_scale: f32) -> f32 {
    let rgb = textureSample(image, image_sampler, coord).rgb * 255.0;
    return (rgb.r * unpack.x + rgb.g * unpack.y + rgb.b * unpack.z + unpack.w) * unpack_scale;
}

// The stops are read at texel centres, from textures without mipmaps, so
// level 0 is exact. The elevation stops are read in a loop of varying
// length, where WGSL allows no implicit level.
fn getElevationStop(stop: i32, color_ramp_size: i32) -> f32 {
    let x = (f32(stop) + 0.5) / f32(color_ramp_size);
    return textureSampleLevel(elevation_stops, elevation_stops_sampler, vec2<f32>(x, 0.5), 0.0).r;
}

fn getColorStop(stop: i32, color_ramp_size: i32) -> vec4<f32> {
    let x = (f32(stop) + 0.5) / f32(color_ramp_size);
    return textureSample(color_stops, color_stops_sampler, vec2<f32>(x, 0.5));
}

@fragment
fn main(in: FragmentInput) -> @location(0) vec4<f32> {
#ifdef OVERDRAW_INSPECTOR
    return vec4<f32>(1.0, 1.0, 1.0, 1.0);
#endif

    let tileProps = tilePropsVector[globalIndex.value];
    let el = getElevation(in.frag_position, tileProps.unpack, tileProps.unpack_scale);

    // Binary search to find surrounding elevation stops
    var r: i32 = tileProps.color_ramp_size - 1;
    var l: i32 = 0;
    // The elevations at l and r, as the search reads them.
    var el_l: f32 = 0.0;
    var el_r: f32 = 0.0;

    loop {
        if (r - l <= 1) { break; }
        let m = (r + l) / 2;
        let el_m = getElevationStop(m, tileProps.color_ramp_size);
        if (el < el_m) {
            r = m;
            el_r = el_m;
        } else {
            l = m;
            el_l = el_m;
        }
    }

    // The search never reads the ramp's ends, so read an end that l or r
    // still is.
    if (l == 0) {
        el_l = getElevationStop(0, tileProps.color_ramp_size);
    }
    if (r == tileProps.color_ramp_size - 1) {
        el_r = getElevationStop(r, tileProps.color_ramp_size);
    }

    let color_l = getColorStop(l, tileProps.color_ramp_size);
    let color_r = getColorStop(r, tileProps.color_ramp_size);

    // Interpolate color based on elevation
    // Guard against division by zero when el_r == el_l
    let denom = el_r - el_l;
    var t: f32 = 0.0;
    if (abs(denom) >= 0.0001) {
        t = clamp((el - el_l) / denom, 0.0, 1.0);
    }
    let final_color = mix(color_l, color_r, t) * props.opacity;

    return vec4<f32>(final_color);
}
",
    ),
    prelude: None,
};

pub(crate) const FILL_EXTRUSION_PATTERN_SHADER: WgslShaderSource = WgslShaderSource {
    vertex: Some(concat!(
        pattern_pos!(),
        r"
struct VertexOutput {
    @builtin(position) @invariant position: vec4<f32>,
    @location(0) lighting: vec4<f32>,
    @location(1) pos_a: vec2<f32>,
    @location(2) pos_b: vec2<f32>,
#ifndef HAS_UNIFORM_u_pattern_from
    @location(3) @interpolate(flat) pattern_from: vec4<f32>,
#endif
#ifndef HAS_UNIFORM_u_pattern_to
    @location(4) @interpolate(flat) pattern_to: vec4<f32>,
#endif
};

@vertex
fn main(in: VertexInput) -> VertexOutput {
    var out: VertexOutput;
    let index = globalIndex.value;
    let drawable = drawableVector[index];

#ifndef HAS_UNIFORM_u_base
    let baseValue = max(unpack_mix_float(in.base, drawable.base_t), 0.0);
#else
    let baseValue = max(props.base, 0.0);
#endif

#ifndef HAS_UNIFORM_u_height
    let heightValue = max(unpack_mix_float(in.height, drawable.height_t), 0.0);
#else
    let heightValue = max(props.height, 0.0);
#endif

    let normal = vec3<f32>(vec2<f32>(in.normal2d) / 16384.0, select(0.0, 1.0, in.normal2d.x == 0 && in.normal2d.y == 0));
    let edgedistance = f32(in.decimals_ed.y);

    let t = glMod(f32(in.decimals_ed.x), 2.0);
    let z = select(baseValue, heightValue, t > 0.0);
    let decimals = unpack_float(f32(in.decimals_ed.x / 2)) / 128.0;

    out.position = projectTileFor3D(vec2<f32>(in.pos) + decimals, z, projectionVector[globalIndex.value]);

    var patternPos: vec2<f32>;
    if (normal.z == 1.0) {
        patternPos = vec2<f32>(in.pos);
    } else {
        patternPos = vec2<f32>(edgedistance, z * drawable.height_factor);
    }

    let pixelRatio = paintParams.pixel_ratio;
    let tileZoomRatio = drawable.tile_ratio;
    let fromScale = props.from_scale;
    let toScale = props.to_scale;

#ifdef HAS_UNIFORM_u_pattern_from
    let patternFrom = tilePropsVector[index].pattern_from;
#else
    let patternFrom = vec4<f32>(in.pattern_from);
    out.pattern_from = patternFrom;
#endif

#ifdef HAS_UNIFORM_u_pattern_to
    let patternTo = tilePropsVector[index].pattern_to;
#else
    let patternTo = vec4<f32>(in.pattern_to);
    out.pattern_to = patternTo;
#endif

    let pattern_tl_a = patternFrom.xy;
    let pattern_br_a = patternFrom.zw;
    let pattern_tl_b = patternTo.xy;
    let pattern_br_b = patternTo.zw;

    let display_size_a = vec2<f32>((pattern_br_a.x - pattern_tl_a.x) / pixelRatio,
                                   (pattern_br_a.y - pattern_tl_a.y) / pixelRatio);
    let display_size_b = vec2<f32>((pattern_br_b.x - pattern_tl_b.x) / pixelRatio,
                                   (pattern_br_b.y - pattern_tl_b.y) / pixelRatio);

    var lighting = vec4<f32>(0.0, 0.0, 0.0, 1.0);
    var directional = clamp(dot(normal, props.light_position), 0.0, 1.0);
    directional = mix(1.0 - props.light_intensity, max(0.5 + props.light_intensity, 1.0), directional);

    if (normal.z == 0.0) {
        let gradientMin = mix(0.7, 0.98, 1.0 - props.light_intensity);
        let factor = clamp((t + baseValue) * pow(heightValue / 150.0, 0.5), gradientMin, 1.0);
        directional *= (1.0 - props.vertical_gradient) + props.vertical_gradient * factor;
    }

    let lightColor = props.light_color;
    let lit = clamp(directional * lightColor,
                    mix(vec3<f32>(0.0), vec3<f32>(0.3), 1.0 - lightColor),
                    vec3<f32>(1.0));
    lighting = vec4<f32>(lighting.rgb + lit, lighting.a);
    lighting *= props.opacity;

    out.lighting = lighting;
    out.pos_a = get_pattern_pos(drawable.pixel_coord_upper,
                                drawable.pixel_coord_lower,
                                fromScale * display_size_a,
                                tileZoomRatio,
                                patternPos);
    out.pos_b = get_pattern_pos(drawable.pixel_coord_upper,
                                drawable.pixel_coord_lower,
                                toScale * display_size_b,
                                tileZoomRatio,
                                patternPos);

    return out;
}
",
    )),
    fragment: Some(
        r"
struct FragmentInput {
    @location(0) lighting: vec4<f32>,
    @location(1) pos_a: vec2<f32>,
    @location(2) pos_b: vec2<f32>,
#ifndef HAS_UNIFORM_u_pattern_from
    @location(3) @interpolate(flat) pattern_from: vec4<f32>,
#endif
#ifndef HAS_UNIFORM_u_pattern_to
    @location(4) @interpolate(flat) pattern_to: vec4<f32>,
#endif
};

@fragment
fn main(in: FragmentInput) -> @location(0) vec4<f32> {
#ifdef OVERDRAW_INSPECTOR
    return vec4<f32>(1.0, 1.0, 1.0, 1.0);
#endif

    let tileProps = tilePropsVector[globalIndex.value];

#ifdef HAS_UNIFORM_u_pattern_from
    let pattern_from = tileProps.pattern_from;
#else
    let pattern_from = in.pattern_from;
#endif

#ifdef HAS_UNIFORM_u_pattern_to
    let pattern_to = tileProps.pattern_to;
#else
    let pattern_to = in.pattern_to;
#endif

    let pattern_tl_a = pattern_from.xy;
    let pattern_br_a = pattern_from.zw;
    let pattern_tl_b = pattern_to.xy;
    let pattern_br_b = pattern_to.zw;

    // A crossfade at 0 or 1 shows one pattern, so skip the other.
    var color_a = vec4<f32>(0.0);
    if (props.fade < 1.0) {
        let imagecoord_a = gl_mod(in.pos_a, vec2<f32>(1.0));
        let pos_a = atlas_pos(pattern_tl_a / tileProps.texsize, (pattern_br_a - pattern_tl_a) / tileProps.texsize * imagecoord_a);
        color_a = textureSample(image, image_sampler, pos_a);
    }
    var color_b = vec4<f32>(0.0);
    if (props.fade > 0.0) {
        let imagecoord_b = gl_mod(in.pos_b, vec2<f32>(1.0));
        let pos_b = atlas_pos(pattern_tl_b / tileProps.texsize, (pattern_br_b - pattern_tl_b) / tileProps.texsize * imagecoord_b);
        color_b = textureSample(image, image_sampler, pos_b);
    }

    let pattern_color = mix(color_a, color_b, props.fade);
    return pattern_color * in.lighting;
}
",
    ),
    prelude: None,
};

pub(crate) const FILL_EXTRUSION_SHADER: WgslShaderSource = WgslShaderSource {
    vertex: Some(
        r"
struct VertexOutput {
    @builtin(position) @invariant position: vec4<f32>,
    @location(0) color: vec4<f32>,
};

@vertex
fn main(in: VertexInput) -> VertexOutput {
    var out: VertexOutput;
    let drawable = drawableVector[globalIndex.value];

#ifndef HAS_UNIFORM_u_base
    let baseValue = max(unpack_mix_float(in.base, drawable.base_t), 0.0);
#else
    let baseValue = max(props.base, 0.0);
#endif

#ifndef HAS_UNIFORM_u_height
    let heightValue = max(unpack_mix_float(in.height, drawable.height_t), 0.0);
#else
    let heightValue = max(props.height, 0.0);
#endif

    let normal = vec3<f32>(vec2<f32>(in.normal2d) / 16384.0, select(0.0, 1.0, in.normal2d.x == 0 && in.normal2d.y == 0));

    let t = glMod(f32(in.decimals_ed.x), 2.0);
    let z = select(baseValue, heightValue, t > 0.0);
    let decimals = unpack_float(f32(in.decimals_ed.x / 2)) / 128.0;

    out.position = projectTileFor3D(vec2<f32>(in.pos) + decimals, z, projectionVector[globalIndex.value]);

#ifdef OVERDRAW_INSPECTOR
    out.color = vec4<f32>(1.0, 1.0, 1.0, 1.0);
    return out;
#endif

#ifndef HAS_UNIFORM_u_color
    var color = unpack_mix_color(in.color, drawable.color_t);
#else
    var color = props.color;
#endif
    let luminance = dot(color.rgb, vec3<f32>(0.2126, 0.7152, 0.0722));
    color = color + vec4<f32>(0.03, 0.03, 0.03, 1.0);

    let directionalFraction = clamp(dot(normal, props.light_position), 0.0, 1.0);
    let minDirectional = 1.0 - props.light_intensity;
    let maxDirectional = max(1.0 - luminance + props.light_intensity, 1.0);
    var directional = mix(minDirectional, maxDirectional, directionalFraction);

    if (normal.z == 0.0) {
        let gradientMin = mix(0.7, 0.98, 1.0 - props.light_intensity);
        let factor = clamp((t + baseValue) * pow(heightValue / 150.0, 0.5), gradientMin, 1.0);
        directional *= (1.0 - props.vertical_gradient) + props.vertical_gradient * factor;
    }

    let lightColor = props.light_color;
    let minLight = mix(vec3<f32>(0.0), vec3<f32>(0.3), 1.0 - lightColor);
    let lit = clamp(color.rgb * directional * lightColor, minLight, vec3<f32>(1.0));

    var vcolor = vec4<f32>(0.0, 0.0, 0.0, 1.0);
    vcolor += vec4<f32>(lit, 0.0);

    out.color = vcolor * props.opacity;
    return out;
}
",
    ),
    fragment: Some(
        r"
struct FragmentInput {
    @location(0) color: vec4<f32>,
};

@fragment
fn main(in: FragmentInput) -> @location(0) vec4<f32> {
    return in.color;
}
",
    ),
    prelude: None,
};

pub(crate) const FILL_OUTLINE_PATTERN_SHADER: WgslShaderSource = WgslShaderSource {
    vertex: Some(concat!(
        pattern_pos!(),
        r"
struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) v_pos_a: vec2<f32>,
    @location(1) v_pos_b: vec2<f32>,
    @location(2) v_pos: vec2<f32>,
#ifndef HAS_UNIFORM_u_color
    @location(3) color: vec4<f32>,
#endif
#ifndef HAS_UNIFORM_u_pattern_from
    @location(4) @interpolate(flat) pattern_from: vec4<f32>,
#endif
#ifndef HAS_UNIFORM_u_pattern_to
    @location(5) @interpolate(flat) pattern_to: vec4<f32>,
#endif
#ifndef HAS_UNIFORM_u_opacity
    @location(6) opacity: f32,
#endif
};

@vertex
fn main(in: VertexInput) -> VertexOutput {
    var out: VertexOutput;
    let index = globalIndex.value;
    let drawable = drawableVector[index];
    let tileProps = tilePropsVector[index];

    var pattern_from: vec4<f32>;
#ifdef HAS_UNIFORM_u_pattern_from
    pattern_from = tileProps.pattern_from;
#else
    pattern_from = vec4<f32>(in.pattern_from);
#endif

    var pattern_to: vec4<f32>;
#ifdef HAS_UNIFORM_u_pattern_to
    pattern_to = tileProps.pattern_to;
#else
    pattern_to = vec4<f32>(in.pattern_to);
#endif

    let pattern_tl_a = pattern_from.xy;
    let pattern_br_a = pattern_from.zw;
    let pattern_tl_b = pattern_to.xy;
    let pattern_br_b = pattern_to.zw;

    let pixelRatio = paintParams.pixel_ratio;
    let tileZoomRatio = drawable.tile_ratio;
    let fromScale = props.from_scale;
    let toScale = props.to_scale;

    let display_size_a = vec2<f32>(
        (pattern_br_a.x - pattern_tl_a.x) / pixelRatio,
        (pattern_br_a.y - pattern_tl_a.y) / pixelRatio
    );
    let display_size_b = vec2<f32>(
        (pattern_br_b.x - pattern_tl_b.x) / pixelRatio,
        (pattern_br_b.y - pattern_tl_b.y) / pixelRatio
    );

    let pos = vec2<f32>(f32(in.pos.x), f32(in.pos.y));
    let clip = projectTileWithPoles(pos, pos, projectionVector[globalIndex.value]);
    let invW = 1.0 / clip.w;
    let ndcXY = clip.xy * invW;

    out.position = clip;
    out.v_pos_a = get_pattern_pos(
        drawable.pixel_coord_upper,
        drawable.pixel_coord_lower,
        fromScale * display_size_a,
        tileZoomRatio,
        pos
    );
    out.v_pos_b = get_pattern_pos(
        drawable.pixel_coord_upper,
        drawable.pixel_coord_lower,
        toScale * display_size_b,
        tileZoomRatio,
        pos
    );
    // Fragment positions use a top-left origin, while NDC Y points up.
    out.v_pos = (ndcXY * vec2<f32>(1.0, -1.0) + vec2<f32>(1.0, 1.0)) * 0.5 * paintParams.world_size;
#ifndef HAS_UNIFORM_u_color
    out.color = unpack_mix_color(in.color, drawable.color_t);
#endif
#ifndef HAS_UNIFORM_u_pattern_from
    out.pattern_from = pattern_from;
#endif
#ifndef HAS_UNIFORM_u_pattern_to
    out.pattern_to = pattern_to;
#endif
#ifndef HAS_UNIFORM_u_opacity
    out.opacity = unpack_mix_float(in.opacity, drawable.opacity_t);
#endif

    return out;
}
",
    )),
    fragment: Some(
        r"
struct FragmentInput {
    @builtin(position) position: vec4<f32>,
    @location(0) v_pos_a: vec2<f32>,
    @location(1) v_pos_b: vec2<f32>,
    @location(2) v_pos: vec2<f32>,
#ifndef HAS_UNIFORM_u_color
    @location(3) color: vec4<f32>,
#endif
#ifndef HAS_UNIFORM_u_pattern_from
    @location(4) @interpolate(flat) pattern_from: vec4<f32>,
#endif
#ifndef HAS_UNIFORM_u_pattern_to
    @location(5) @interpolate(flat) pattern_to: vec4<f32>,
#endif
#ifndef HAS_UNIFORM_u_opacity
    @location(6) opacity: f32,
#endif
};

@fragment
fn main(in: FragmentInput) -> @location(0) vec4<f32> {
#ifdef OVERDRAW_INSPECTOR
    return vec4<f32>(1.0, 1.0, 1.0, 1.0);
#endif

    let tileProps = tilePropsVector[globalIndex.value];

    let pattern_from =
#ifdef HAS_UNIFORM_u_pattern_from
        tileProps.pattern_from;
#else
        in.pattern_from;
#endif

    let pattern_to =
#ifdef HAS_UNIFORM_u_pattern_to
        tileProps.pattern_to;
#else
        in.pattern_to;
#endif

    let pattern_tl_a = pattern_from.xy;
    let pattern_br_a = pattern_from.zw;
    let pattern_tl_b = pattern_to.xy;
    let pattern_br_b = pattern_to.zw;

    // A crossfade at 0 or 1 shows one pattern, so skip the other.
    var color_a = vec4<f32>(0.0);
    if (props.fade < 1.0) {
        let imagecoord_a = gl_mod(in.v_pos_a, vec2<f32>(1.0));
        let pos_a = atlas_pos(pattern_tl_a / tileProps.texsize, (pattern_br_a - pattern_tl_a) / tileProps.texsize * imagecoord_a);
        color_a = textureSample(image, image_sampler, pos_a);
    }
    var color_b = vec4<f32>(0.0);
    if (props.fade > 0.0) {
        let imagecoord_b = gl_mod(in.v_pos_b, vec2<f32>(1.0));
        let pos_b = atlas_pos(pattern_tl_b / tileProps.texsize, (pattern_br_b - pattern_tl_b) / tileProps.texsize * imagecoord_b);
        color_b = textureSample(image, image_sampler, pos_b);
    }

    // Mix patterns and apply opacity
    let opacity =
#ifdef HAS_UNIFORM_u_opacity
        props.opacity;
#else
        in.opacity;
#endif

    let color =
#ifdef HAS_UNIFORM_u_color
        props.color;
#else
        in.color;
#endif

    let dist = length(in.v_pos - in.position.xy);
    let alpha = 1.0 - smoothstep(0.0, 1.0, dist);

    if (tileProps.sdf > 0.5) {
        let sdf_edge = (256.0 - 64.0) / 256.0;
        let sdf_gamma_a = max(fwidth(color_a.a) * 0.5, 1.0 / 255.0 / 16.0);
        let sdf_gamma_b = max(fwidth(color_b.a) * 0.5, 1.0 / 255.0 / 16.0);
        let sdf_alpha_a = smoothstep(sdf_edge - sdf_gamma_a, sdf_edge + sdf_gamma_a, color_a.a);
        let sdf_alpha_b = smoothstep(sdf_edge - sdf_gamma_b, sdf_edge + sdf_gamma_b, color_b.a);
        return mix(color * sdf_alpha_a, color * sdf_alpha_b, props.fade) * alpha * opacity;
    }
    // The outline fades over a pixel from the line, as in the GL and Vulkan
    // shaders; the C++ WebGPU source leaves `alpha` out here.
    return mix(color_a, color_b, props.fade) * alpha * opacity;
}
",
    ),
    prelude: None,
};

pub(crate) const FILL_OUTLINE_SHADER: WgslShaderSource = WgslShaderSource {
    vertex: Some(
        r"
struct VertexOutput {
    @builtin(position) position: vec4<f32>,
#ifndef HAS_UNIFORM_u_outline_color
    @location(0) color: vec4<f32>,
#endif
#ifndef HAS_UNIFORM_u_opacity
    @location(1) opacity: f32,
#endif
    @location(2) pos: vec2<f32>,
};

@vertex
fn main(in: VertexInput) -> VertexOutput {
    var out: VertexOutput;

    // Transform position using the matrix
    let drawable = drawableVector[globalIndex.value];
    let pos = vec2<f32>(f32(in.pos.x), f32(in.pos.y));
    let clip = projectTileWithPoles(pos, pos, projectionVector[globalIndex.value]);
    let invW = 1.0 / clip.w;
    let ndcXY = clip.xy * invW;
    out.position = clip;
    out.pos = (ndcXY * vec2<f32>(1.0, -1.0) + vec2<f32>(1.0)) * 0.5 * paintParams.world_size;

#ifndef HAS_UNIFORM_u_outline_color
    out.color = unpack_mix_color(in.outline_color, drawable.outline_color_t);
#endif
#ifndef HAS_UNIFORM_u_opacity
    out.opacity = unpack_mix_float(in.opacity, drawable.opacity_t);
#endif

    return out;
}
",
    ),
    fragment: Some(
        r"
struct FragmentInput {
    @builtin(position) position: vec4<f32>,
#ifndef HAS_UNIFORM_u_outline_color
    @location(0) color: vec4<f32>,
#endif
#ifndef HAS_UNIFORM_u_opacity
    @location(1) opacity: f32,
#endif
    @location(2) pos: vec2<f32>,
};

@fragment
fn main(in: FragmentInput) -> @location(0) vec4<f32> {
#ifdef OVERDRAW_INSPECTOR
    return vec4<f32>(1.0, 1.0, 1.0, 1.0);
#endif

    var color: vec4<f32>;
#ifdef HAS_UNIFORM_u_outline_color
    color = props.outline_color;
#else
    color = in.color;
#endif

    var opacity: f32;
#ifdef HAS_UNIFORM_u_opacity
    opacity = props.opacity;
#else
    opacity = in.opacity;
#endif

    // The outline fades over a pixel from the line, as in the GL and Vulkan
    // shaders; the C++ WebGPU source leaves `alpha` out here.
    let dist = length(in.pos - in.position.xy);
    let alpha = 1.0 - smoothstep(0.0, 1.0, dist);
    return color * alpha * opacity;
}
",
    ),
    prelude: None,
};

pub(crate) const FILL_PATTERN_SHADER: WgslShaderSource = WgslShaderSource {
    vertex: Some(concat!(
        pattern_pos!(),
        r"
struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) v_pos_a: vec2<f32>,
    @location(1) v_pos_b: vec2<f32>,
#ifndef HAS_UNIFORM_u_color
    @location(2) color: vec4<f32>,
#endif
#ifndef HAS_UNIFORM_u_pattern_from
    @location(3) @interpolate(flat) pattern_from: vec4<f32>,
#endif
#ifndef HAS_UNIFORM_u_pattern_to
    @location(4) @interpolate(flat) pattern_to: vec4<f32>,
#endif
#ifndef HAS_UNIFORM_u_opacity
    @location(5) opacity: f32,
#endif
};

@vertex
fn main(in: VertexInput) -> VertexOutput {
    var out: VertexOutput;
    let index = globalIndex.value;
    let drawable = drawableVector[index];
    let tileProps = tilePropsVector[index];

    var pattern_from: vec4<f32>;
#ifdef HAS_UNIFORM_u_pattern_from
    pattern_from = tileProps.pattern_from;
#else
    pattern_from = vec4<f32>(in.pattern_from);
#endif

    var pattern_to: vec4<f32>;
#ifdef HAS_UNIFORM_u_pattern_to
    pattern_to = tileProps.pattern_to;
#else
    pattern_to = vec4<f32>(in.pattern_to);
#endif

    let pattern_tl_a = pattern_from.xy;
    let pattern_br_a = pattern_from.zw;
    let pattern_tl_b = pattern_to.xy;
    let pattern_br_b = pattern_to.zw;

    let pixelRatio = paintParams.pixel_ratio;
    let tileZoomRatio = drawable.tile_ratio;
    let fromScale = props.from_scale;
    let toScale = props.to_scale;

    let display_size_a = vec2<f32>(
        (pattern_br_a.x - pattern_tl_a.x) / pixelRatio,
        (pattern_br_a.y - pattern_tl_a.y) / pixelRatio
    );
    let display_size_b = vec2<f32>(
        (pattern_br_b.x - pattern_tl_b.x) / pixelRatio,
        (pattern_br_b.y - pattern_tl_b.y) / pixelRatio
    );

    let pos = vec2<f32>(f32(in.pos.x), f32(in.pos.y));
    let clip = projectTileWithPoles(pos, pos, projectionVector[globalIndex.value]);
    out.position = clip;
    out.v_pos_a = get_pattern_pos(
        drawable.pixel_coord_upper,
        drawable.pixel_coord_lower,
        fromScale * display_size_a,
        tileZoomRatio,
        pos
    );
    out.v_pos_b = get_pattern_pos(
        drawable.pixel_coord_upper,
        drawable.pixel_coord_lower,
        toScale * display_size_b,
        tileZoomRatio,
        pos
    );
#ifndef HAS_UNIFORM_u_color
    out.color = unpack_mix_color(in.color, drawable.color_t);
#endif
#ifndef HAS_UNIFORM_u_pattern_from
    out.pattern_from = pattern_from;
#endif
#ifndef HAS_UNIFORM_u_pattern_to
    out.pattern_to = pattern_to;
#endif
#ifndef HAS_UNIFORM_u_opacity
    out.opacity = unpack_mix_float(in.opacity, drawable.opacity_t);
#endif

    return out;
}
",
    )),
    fragment: Some(
        r"
struct FragmentInput {
    @location(0) v_pos_a: vec2<f32>,
    @location(1) v_pos_b: vec2<f32>,
#ifndef HAS_UNIFORM_u_color
    @location(2) color: vec4<f32>,
#endif
#ifndef HAS_UNIFORM_u_pattern_from
    @location(3) @interpolate(flat) pattern_from: vec4<f32>,
#endif
#ifndef HAS_UNIFORM_u_pattern_to
    @location(4) @interpolate(flat) pattern_to: vec4<f32>,
#endif
#ifndef HAS_UNIFORM_u_opacity
    @location(5) opacity: f32,
#endif
};

@fragment
fn main(in: FragmentInput) -> @location(0) vec4<f32> {
#ifdef OVERDRAW_INSPECTOR
    return vec4<f32>(1.0, 1.0, 1.0, 1.0);
#endif

    let tileProps = tilePropsVector[globalIndex.value];

    let pattern_from =
#ifdef HAS_UNIFORM_u_pattern_from
        tileProps.pattern_from;
#else
        in.pattern_from;
#endif

    let pattern_to =
#ifdef HAS_UNIFORM_u_pattern_to
        tileProps.pattern_to;
#else
        in.pattern_to;
#endif

    let pattern_tl_a = pattern_from.xy;
    let pattern_br_a = pattern_from.zw;
    let pattern_tl_b = pattern_to.xy;
    let pattern_br_b = pattern_to.zw;

    // A crossfade at 0 or 1 shows one pattern, so skip the other.
    var color_a = vec4<f32>(0.0);
    if (props.fade < 1.0) {
        let imagecoord_a = gl_mod(in.v_pos_a, vec2<f32>(1.0));
        let pos_a = atlas_pos(pattern_tl_a / tileProps.texsize, (pattern_br_a - pattern_tl_a) / tileProps.texsize * imagecoord_a);
        color_a = textureSample(image, image_sampler, pos_a);
    }
    var color_b = vec4<f32>(0.0);
    if (props.fade > 0.0) {
        let imagecoord_b = gl_mod(in.v_pos_b, vec2<f32>(1.0));
        let pos_b = atlas_pos(pattern_tl_b / tileProps.texsize, (pattern_br_b - pattern_tl_b) / tileProps.texsize * imagecoord_b);
        color_b = textureSample(image, image_sampler, pos_b);
    }

    // Mix patterns and apply opacity
    let opacity =
#ifdef HAS_UNIFORM_u_opacity
        props.opacity;
#else
        in.opacity;
#endif

    let color =
#ifdef HAS_UNIFORM_u_color
        props.color;
#else
        in.color;
#endif

    if (tileProps.sdf > 0.5) {
        let sdf_edge = (256.0 - 64.0) / 256.0;
        let sdf_gamma_a = max(fwidth(color_a.a) * 0.5, 1.0 / 255.0 / 16.0);
        let sdf_gamma_b = max(fwidth(color_b.a) * 0.5, 1.0 / 255.0 / 16.0);
        let sdf_alpha_a = smoothstep(sdf_edge - sdf_gamma_a, sdf_edge + sdf_gamma_a, color_a.a);
        let sdf_alpha_b = smoothstep(sdf_edge - sdf_gamma_b, sdf_edge + sdf_gamma_b, color_b.a);
        return mix(color * sdf_alpha_a, color * sdf_alpha_b, props.fade) * opacity;
    }
    return mix(color_a, color_b, props.fade) * opacity;
}
",
    ),
    prelude: None,
};

pub(crate) const FILL_SHADER: WgslShaderSource = WgslShaderSource {
    vertex: Some(
        r"
struct VertexOutput {
    @builtin(position) position: vec4<f32>,
#ifndef HAS_UNIFORM_u_color
    @location(0) color: vec4<f32>,
#endif
#ifndef HAS_UNIFORM_u_opacity
    @location(1) opacity: f32,
#endif
};

@vertex
fn main(in: VertexInput) -> VertexOutput {
    var out: VertexOutput;

    // Transform position using the matrix
    let drawable = drawableVector[globalIndex.value];
    let pos = vec2<f32>(f32(in.pos.x), f32(in.pos.y));
    let clip = projectTileWithPoles(pos, pos, projectionVector[globalIndex.value]);
    out.position = clip;

#ifndef HAS_UNIFORM_u_color
    out.color = unpack_mix_color(in.color, drawable.color_t);
#endif
#ifndef HAS_UNIFORM_u_opacity
    out.opacity = unpack_mix_float(in.opacity, drawable.opacity_t);
#endif

    return out;
}
",
    ),
    fragment: Some(
        r"
struct FragmentInput {
    @builtin(position) position: vec4<f32>,
#ifndef HAS_UNIFORM_u_color
    @location(0) color: vec4<f32>,
#endif
#ifndef HAS_UNIFORM_u_opacity
    @location(1) opacity: f32,
#endif
};

@fragment
fn main(in: FragmentInput) -> @location(0) vec4<f32> {
#ifdef OVERDRAW_INSPECTOR
    return vec4<f32>(1.0, 1.0, 1.0, 1.0);
#endif

    var color: vec4<f32>;
#ifdef HAS_UNIFORM_u_color
    color = props.color;
#else
    color = in.color;
#endif

    var opacity: f32;
#ifdef HAS_UNIFORM_u_opacity
    opacity = props.opacity;
#else
    opacity = in.opacity;
#endif

    return color * opacity;
}
",
    ),
    prelude: None,
};

pub(crate) const GLOBE_DEPTH_SHADER: WgslShaderSource = WgslShaderSource {
    vertex: Some(
        r"
struct VertexOutput {
    @builtin(position) position: vec4<f32>,
};

@vertex
fn main(in: VertexInput) -> VertexOutput {
    var out: VertexOutput;
    out.position = projectTileFor3D(vec2<f32>(f32(in.pos.x), f32(in.pos.y)), 0.0, projectionVector[globalIndex.value]);
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

pub(crate) const HEATMAP_SHADER: WgslShaderSource = WgslShaderSource {
    vertex: Some(
        r"
struct VertexOutput {
    @builtin(position) position: vec4<f32>,
#ifndef HAS_UNIFORM_u_weight
    @location(0) weight: f32,
#endif
    @location(1) extrude: vec2<f32>,
};

const ZERO: f32 = 1.0 / 255.0 / 16.0;
const GAUSS_COEF: f32 = 0.3989422804014327;

@vertex
fn main(in: VertexInput) -> VertexOutput {
    var out: VertexOutput;

    let drawable = drawableVector[globalIndex.value];

#ifdef HAS_UNIFORM_u_weight
    let weight = props.weight;
#else
    let weight = unpack_mix_float(in.weight, drawable.weight_t);
#endif

#ifdef HAS_UNIFORM_u_radius
    let radius = props.radius;
#else
    let radius = unpack_mix_float(in.radius, drawable.radius_t);
#endif

    let pos = vec2<f32>(f32(in.pos.x), f32(in.pos.y));
    let unscaled_extrude = fma(vec2<f32>(f32(in.pos.x & 1), f32(in.pos.y & 1)), vec2<f32>(2.0, 2.0),
                               vec2<f32>(-1.0, -1.0));

    let S = sqrt(-2.0 * log(ZERO / (max(weight, ZERO) * max(props.intensity, ZERO) * GAUSS_COEF))) / 3.0;
    let extrude = S * unscaled_extrude;
    let scaled_extrude = extrude * radius * drawable.extrude_scale;

    let base = floor(pos * 0.5);
    let projection = projectionVector[globalIndex.value];
    // Over terrain, the kernel lies flat at the height of the ground under
    // its center. The ground in front of it doesn't hide it: its density
    // adds up in screen space, as GL JS's does.
#ifdef TERRAIN
    let ele = in.ground_height;
#else
    let ele = 0.0;
#endif
#ifdef PROJECTION_GLOBE
    let center_vector = projectToSphere(base, vec2<f32>(0.0, 0.0), projection);
    let corner_vector = globeRotateVector(center_vector, extrude * radius * drawable.globe_extrude_scale, projection);
    out.position = interpolateProjection(base + scaled_extrude, corner_vector, ele, projection);
#else
    out.position = drawable.matrix * vec4<f32>(base + scaled_extrude, ele, 1.0);
#endif
#ifndef HAS_UNIFORM_u_weight
    out.weight = weight;
#endif
    out.extrude = extrude;

    return out;
}
",
    ),
    fragment: Some(
        r"
struct FragmentInput {
#ifndef HAS_UNIFORM_u_weight
    @location(0) weight: f32,
#endif
    @location(1) extrude: vec2<f32>,
};

const GAUSS_COEF: f32 = 0.3989422804014327;

@fragment
fn main(in: FragmentInput) -> @location(0) vec4<f32> {
#ifdef OVERDRAW_INSPECTOR
    return vec4<f32>(1.0, 1.0, 1.0, 1.0);
#else
    let d = -0.5 * 3.0 * 3.0 * dot(in.extrude, in.extrude);
#ifdef HAS_UNIFORM_u_weight
    let weight = props.weight;
#else
    let weight = in.weight;
#endif
    let val = weight * props.intensity * GAUSS_COEF * exp(d);
    return vec4<f32>(val, 1.0, 1.0, 1.0);
#endif
}
",
    ),
    prelude: None,
};

pub(crate) const HEATMAP_TEXTURE_SHADER: WgslShaderSource = WgslShaderSource {
    vertex: Some(
        r"
struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) tex_coord: vec2<f32>,
};

@vertex
fn main(in: VertexInput) -> VertexOutput {
    var out: VertexOutput;

    let quad_pos = vec2<f32>(f32(in.pos.x), f32(in.pos.y));
    let scaled = quad_pos * paintParams.world_size;

    out.position = ubo.matrix * vec4<f32>(scaled, 0.0, 1.0);
    out.tex_coord = quad_pos;

    return out;
}
",
    ),
    fragment: Some(
        r"
struct FragmentInput {
    @location(0) tex_coord: vec2<f32>,
};

@fragment
fn main(in: FragmentInput) -> @location(0) vec4<f32> {
#ifdef OVERDRAW_INSPECTOR
    return vec4<f32>(1.0, 1.0, 1.0, 1.0);
#endif

    let t = textureSample(image, image_sampler, in.tex_coord).r;
    let color = textureSample(color_ramp, color_ramp_sampler, vec2<f32>(t, 0.5));
    return color * ubo.opacity;
}
",
    ),
    prelude: None,
};

/// Port-only: a layer's texture, the size of the frame, composited onto it
/// texel for texel, with its opacity and, for the blend modes that no blend
/// function expresses, against a copy of the frame (`backdrop_texture`).
pub(crate) const LAYER_COMPOSITE_SHADER: WgslShaderSource = WgslShaderSource {
    vertex: Some(
        r"
@vertex
fn main(in: VertexInput) -> @builtin(position) vec4<f32> {
    let pos = vec2<f32>(in.pos);
    return vec4<f32>(pos * 2.0 - 1.0, 0.0, 1.0);
}
",
    ),
    fragment: Some(
        r"
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
    ),
    prelude: None,
};

/// Port-only, for the backend conformance cases: quads at clip positions,
/// each vertex colored by the texel of `image` it names, which the
/// vertex stage loads.
#[cfg(test)]
pub(crate) const VERTEX_TEXEL_SHADER: WgslShaderSource = WgslShaderSource {
    vertex: Some(
        r"
struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) color: vec4<f32>,
};

@vertex
fn main(in: VertexInput) -> VertexOutput {
    var out: VertexOutput;
    out.position = vec4<f32>(vec2<f32>(in.pos_texel.xy), 0.0, 1.0);
    out.color = textureLoad(image, in.pos_texel.zw, 0);
    return out;
}
",
    ),
    fragment: Some(
        r"
@fragment
fn main(@location(0) color: vec4<f32>) -> @location(0) vec4<f32> {
    return color;
}
",
    ),
    prelude: None,
};

/// The height rule of `mln_util::dem`, over the DEM tile a draw reads: its
/// RGBA8 texels behind a one-texel border, decoded by a `TerrainSurfaceUBO`.
macro_rules! ground_height {
    () => {
        r"
// The height in meters of the DEM's bordered texel at `texel`, clamped to
// the texture, decoded by `props`.
fn ground_texel_height(texel: vec2<i32>, props: TerrainSurfaceUBO) -> f32 {
    let last = vec2<i32>(textureDimensions(ground_dem)) - 1;
    let rgb = round(textureLoad(ground_dem, clamp(texel, vec2<i32>(0), last), 0).rgb * 255.0);
    let unpack = props.unpack;
    return (rgb.r * unpack.x + rgb.g * unpack.y + rgb.b * unpack.z + unpack.w) * props.unpack_scale;
}

// The height in meters, exaggerated, at `pos` in tile units of a tile that
// reads its DEM at `dem_coords`: the DEM uv's scale and offset, then the
// DEM's width in pixels, 0 for flat ground.
fn ground_height(pos: vec2<f32>, dem_coords: vec4<f32>, props: TerrainSurfaceUBO) -> f32 {
    let dim = dem_coords.w;
    if (dim == 0.0) {
        return 0.0;
    }
    let coord = (pos / 8192.0 * dem_coords.x + dem_coords.yz) * dim + 0.5;
    let first = floor(coord);
    let f = coord - first;
    let texel = vec2<i32>(first);
    let tl = ground_texel_height(texel, props);
    let tr = ground_texel_height(texel + vec2<i32>(1, 0), props);
    let bl = ground_texel_height(texel + vec2<i32>(0, 1), props);
    let br = ground_texel_height(texel + vec2<i32>(1, 1), props);
    let top = tl * (1.0 - f.x) + tr * f.x;
    let bottom = bl * (1.0 - f.x) + br * f.x;
    return (top * (1.0 - f.y) + bottom * f.y) * props.exaggeration;
}
"
    };
}

/// How a hillshade shades its slopes: its tile and evaluated blocks, and
/// `hillshade_color`, which shades a slope by the lights of `props`, the
/// `HillshadeEvaluatedPropsUBO` that the program binds. A macro, so that
/// `concat!` puts it in the fragment stages of the hillshade and of the
/// terrain's surface, which shades a drape of a hillshade's slopes.
macro_rules! hillshade_shading {
    () => {
        r"
const STANDARD: i32 = 0;
const COMBINED: i32 = 1;
const IGOR: i32 = 2;
const MULTIDIRECTIONAL: i32 = 3;
const BASIC: i32 = 4;

fn get_aspect(deriv: vec2<f32>) -> f32 {
    let aspectDefault = 0.5 * PI * select(-1.0, 1.0, deriv.y > 0.0);
    return select(aspectDefault, atan2(deriv.y, -deriv.x), deriv.x != 0.0);
}

fn standard_hillshade(deriv: vec2<f32>, tileProps: HillshadeTilePropsUBO) -> vec4<f32> {
    let azimuth = props.azimuths.x + PI;
    let slope = atan(0.625 * length(deriv));
    let aspect = get_aspect(deriv);

    let intensity = tileProps.exaggeration;

    let base = 1.875 - intensity * 1.75;
    let maxValue = 0.5 * PI;
    let denom = pow(base, maxValue) - 1.0;
    let useNonLinear = abs(intensity - 0.5) > 1e-6;
    let scaledSlope = select(slope, ((pow(base, slope) - 1.0) / denom) * maxValue, useNonLinear);

    let accent = cos(scaledSlope);
    let accentColor = (1.0 - accent) * props.accent * clamp(intensity * 2.0, 0.0, 1.0);

    let shade = abs(glMod((aspect + azimuth) / PI + 0.5, 2.0) - 1.0);
    let shadeColor = mix(props.shadows[0], props.highlights[0], shade) * sin(scaledSlope) * clamp(intensity * 2.0, 0.0, 1.0);

    return accentColor * (1.0 - shadeColor.a) + shadeColor;
}

fn basic_hillshade(deriv: vec2<f32>, tileProps: HillshadeTilePropsUBO) -> vec4<f32> {
    let scaled_deriv = deriv * tileProps.exaggeration * 2.0;
    let azimuth = props.azimuths.x + PI;
    let cos_az = cos(azimuth);
    let sin_az = sin(azimuth);
    let cos_alt = cos(props.altitudes.x);
    let sin_alt = sin(props.altitudes.x);

    let cang = (sin_alt - (scaled_deriv.y * cos_az * cos_alt - scaled_deriv.x * sin_az * cos_alt)) / sqrt(1.0 + dot(scaled_deriv, scaled_deriv));
    let shade = clamp(cang, 0.0, 1.0);

    return select(props.shadows[0] * (1.0 - 2.0 * shade),
                  props.highlights[0] * (2.0 * shade - 1.0),
                  shade > 0.5);
}

fn multidirectional_hillshade(deriv: vec2<f32>, tileProps: HillshadeTilePropsUBO) -> vec4<f32> {
    let scaled_deriv = deriv * tileProps.exaggeration * 2.0;
    var total_color = vec4<f32>(0.0, 0.0, 0.0, 0.0);

    let num_lights = min(tileProps.num_lights, 4);

    for (var i: i32 = 0; i < num_lights; i = i + 1) {
        let altitude = select(select(select(props.altitudes.x, props.altitudes.y, i == 1), props.altitudes.z, i == 2), props.altitudes.w, i == 3);
        let azimuth = select(select(select(props.azimuths.x, props.azimuths.y, i == 1), props.azimuths.z, i == 2), props.azimuths.w, i == 3);

        let cos_alt = cos(altitude);
        let sin_alt = sin(altitude);
        let cos_az = -cos(azimuth);
        let sin_az = -sin(azimuth);

        let cang = (sin_alt - (scaled_deriv.y * cos_az * cos_alt - scaled_deriv.x * sin_az * cos_alt)) / sqrt(1.0 + dot(scaled_deriv, scaled_deriv));
        let shade = clamp(cang, 0.0, 1.0);

        if (shade > 0.5) {
            total_color = total_color + props.highlights[i] * (2.0 * shade - 1.0) / f32(num_lights);
        } else {
            total_color = total_color + props.shadows[i] * (1.0 - 2.0 * shade) / f32(num_lights);
        }
    }

    return total_color;
}

fn combined_hillshade(deriv: vec2<f32>, tileProps: HillshadeTilePropsUBO) -> vec4<f32> {
    let scaled_deriv = deriv * tileProps.exaggeration * 2.0;
    let azimuth = props.azimuths.x + PI;
    let cos_az = cos(azimuth);
    let sin_az = sin(azimuth);
    let cos_alt = cos(props.altitudes.x);
    let sin_alt = sin(props.altitudes.x);

    let cang = acos((sin_alt - (scaled_deriv.y * cos_az * cos_alt - scaled_deriv.x * sin_az * cos_alt)) / sqrt(1.0 + dot(scaled_deriv, scaled_deriv)));
    let clamped_cang = clamp(cang, 0.0, PI / 2.0);

    let shade = clamped_cang * atan(length(scaled_deriv)) * 4.0 / PI / PI;
    let highlight = (PI / 2.0 - clamped_cang) * atan(length(scaled_deriv)) * 4.0 / PI / PI;

    return props.shadows[0] * shade + props.highlights[0] * highlight;
}

fn igor_hillshade(deriv: vec2<f32>, tileProps: HillshadeTilePropsUBO) -> vec4<f32> {
    let scaled_deriv = deriv * tileProps.exaggeration * 2.0;
    let aspect = get_aspect(scaled_deriv);
    let azimuth = props.azimuths.x + PI;

    let slope_strength = atan(length(scaled_deriv)) * 2.0 / PI;
    let aspect_strength = 1.0 - abs(glMod((aspect + azimuth) / PI + 0.5, 2.0) - 1.0);

    let shadow_strength = slope_strength * aspect_strength;
    let highlight_strength = slope_strength * (1.0 - aspect_strength);

    return props.shadows[0] * shadow_strength + props.highlights[0] * highlight_strength;
}

// The color of the slope `deriv`, shaded as `tileProps` says by the lights
// of `props`, the program's `HillshadeEvaluatedPropsUBO`.
fn hillshade_color(deriv: vec2<f32>, tileProps: HillshadeTilePropsUBO) -> vec4<f32> {
    if (tileProps.method == BASIC) {
        return basic_hillshade(deriv, tileProps);
    } else if (tileProps.method == COMBINED) {
        return combined_hillshade(deriv, tileProps);
    } else if (tileProps.method == IGOR) {
        return igor_hillshade(deriv, tileProps);
    } else if (tileProps.method == MULTIDIRECTIONAL) {
        return multidirectional_hillshade(deriv, tileProps);
    } else {
        return standard_hillshade(deriv, tileProps);
    }
}
"
    };
}

/// Port-only, for the backend conformance cases: probes of the terrain's
/// height rule. Each probe is a quad at clip positions whose vertices read
/// the height at a place of a tile, and whose fragments write that height's
/// bits, least significant byte first, as the texel's channels.
#[cfg(test)]
pub(crate) const TERRAIN_PROBE_SHADER: WgslShaderSource = WgslShaderSource {
    vertex: Some(
        r"
struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) @interpolate(flat) height: f32,
};

@vertex
fn main(in: VertexInput) -> VertexOutput {
    var out: VertexOutput;
    out.position = vec4<f32>(in.pos.xy, 0.0, 1.0);
    out.height = ground_height(in.pos.zw, in.dem_coords, terrain);
    return out;
}
",
    ),
    fragment: Some(
        r"
@fragment
fn main(@location(0) @interpolate(flat) height: f32) -> @location(0) vec4<f32> {
    let bits = bitcast<u32>(height);
    let bytes = vec4<u32>(bits, bits >> 8u, bits >> 16u, bits >> 24u) & vec4<u32>(255u);
    return vec4<f32>(bytes) / 255.0;
}
",
    ),
    prelude: Some(TERRAIN_PRELUDE),
};

/// The terrain's surface: a tile's grid, raised to the heights of the tile's
/// DEM with its skirts lowered and projected through the tile's entry of
/// `projectionVector`, in the colors of the tile's drape. A pole's vertices
/// stand at sea level. Where the tile's entry of `lightVector` has lights,
/// the drape holds a hillshade's slopes, premultiplied by its coverage,
/// which the surface shades with them. The fog of `terrain` covers it.
pub(crate) const TERRAIN_SHADER: WgslShaderSource = WgslShaderSource {
    vertex: Some(
        r"
struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    // Where in the drape, its north-west corner at 0 and its rows from the
    // north, as they are drawn.
    @location(0) drape_pos: vec2<f32>,
    // How far along the view, in pixels: the clip w.
    @location(1) view_depth: f32,
};

@vertex
fn main(in: VertexInput) -> VertexOutput {
    let drawable = drawableVector[globalIndex.value];
    let projection = projectionVector[globalIndex.value];
    let pos = vec2<f32>(in.pos.xy);
    let skirt = f32(in.pos.z);
    let height = surface_height(pos, drawable.dem_coords, terrain);
    var out: VertexOutput;
    out.position = projectTileFor3D(pos, height - skirt * terrain.skirt_length, projection);
    out.drape_pos = clamp(pos / 8192.0, vec2<f32>(0.0), vec2<f32>(1.0));
    out.view_depth = out.position.w;
    return out;
}
",
    ),
    fragment: Some(concat!(
        hillshade_shading!(),
        r"
struct FragmentInput {
    @location(0) drape_pos: vec2<f32>,
    @location(1) view_depth: f32,
};

@fragment
fn main(in: FragmentInput) -> @location(0) vec4<f32> {
    let color = textureSample(drape, drape_sampler, in.drape_pos);
    let light = lightVector[globalIndex.value];
    if (light.num_lights == 0 || color.a == 0.0) {
        return fogged(color, in.view_depth, terrain);
    }
    // The tile's latitudes run from its north edge, where the drape's rows
    // start.
    let latitude = mix(light.latrange.x, light.latrange.y, in.drape_pos.y);
    let deriv = ((color.rg / color.a) * 8.0 - vec2<f32>(4.0, 4.0)) / cos(radians(latitude));
    return fogged(hillshade_color(deriv, light) * color.a, in.view_depth, terrain);
}
"
    )),
    prelude: Some(TERRAIN_PRELUDE),
};

/// The ground's depth: the surface's grid, raised and projected as
/// `TERRAIN_SHADER` does, writing each pixel's eye distance, an `f32`'s bits,
/// least significant byte first, as the texel's channels.
pub(crate) const TERRAIN_DEPTH_SHADER: WgslShaderSource = WgslShaderSource {
    vertex: Some(
        r"
struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    // The eye distance, which interpolates exactly across a triangle.
    @location(0) distance: f32,
};

@vertex
fn main(in: VertexInput) -> VertexOutput {
    let drawable = drawableVector[globalIndex.value];
    let projection = projectionVector[globalIndex.value];
    let pos = vec2<f32>(in.pos.xy);
    let skirt = f32(in.pos.z);
    let height = surface_height(pos, drawable.dem_coords, terrain);
    var out: VertexOutput;
    out.position = projectTileFor3D(pos, height - skirt * terrain.skirt_length, projection);
    out.distance = out.position.w;
    return out;
}
",
    ),
    fragment: Some(
        r"
@fragment
fn main(@location(0) distance: f32) -> @location(0) vec4<f32> {
    let bits = bitcast<u32>(distance);
    let bytes = vec4<u32>(bits, bits >> 8u, bits >> 16u, bits >> 24u) & vec4<u32>(255u);
    return vec4<f32>(bytes) / 255.0;
}
",
    ),
    prelude: Some(TERRAIN_PRELUDE),
};

/// The surface's and the probe's prelude: the terrain's uniform block, the
/// height rule, the height of the surface's vertices, and the fog.
const TERRAIN_PRELUDE: &str = concat!(
    ground_height!(),
    r"
// How the terrain's DEM decodes, its exaggeration and its skirts' length,
// then the fog over the surface.

// `color`, premultiplied, of the surface `view_depth` pixels along the
// view, covered by `fog`, as GL JS's terrain is: mixed in linear components
// toward the fog's color by the square of how far past the ground blend the
// fog's depth is. The fog's alpha weighs its color; the coverage stays.
fn fogged(color: vec4<f32>, view_depth: f32, fog: TerrainSurfaceUBO) -> vec4<f32> {
    let near = fog.depth_range.x;
    let far = fog.depth_range.y;
    let depth = min(far / (far - near) * (1.0 - near / view_depth), 1.0);
    if (fog.ground_blend >= 1.0 || depth <= fog.ground_blend || color.a == 0.0) {
        return color;
    }
    let horizon = smoothstep(0.0, 1.0, (depth - fog.horizon_blend) / max(1.0 - fog.horizon_blend, 1e-6));
    let tint = mix(fog.fog_color, fog.horizon_color, horizon);
    let along = (depth - fog.ground_blend) / (1.0 - fog.ground_blend);
    let strength = along * along;
    let gamma = vec3<f32>(2.2);
    let surface = pow(color.rgb / color.a, gamma);
    let mixed = surface * (1.0 - strength * tint.a) + tint.rgb * strength;
    return vec4<f32>(pow(mixed, 1.0 / gamma) * color.a, color.a);
}

// The height of the surface's vertex at `pos`, in tile units of a tile that
// reads its DEM at `dem_coords`: the ground's, or sea level at a pole.
fn surface_height(pos: vec2<f32>, dem_coords: vec4<f32>, props: TerrainSurfaceUBO) -> f32 {
    if (pos.y < GLOBE_POLE_NORTH_Y || pos.y > GLOBE_POLE_SOUTH_Y) {
        return 0.0;
    }
    return ground_height(pos, dem_coords, props);
}
"
);

pub(crate) const HILLSHADE_PREPARE_SHADER: WgslShaderSource = WgslShaderSource {
    vertex: Some(
        r"
struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) tex_coord: vec2<f32>,
};

@vertex
fn main(in: VertexInput) -> VertexOutput {
    var out: VertexOutput;
    out.position = drawable.matrix * vec4<f32>(f32(in.pos.x), f32(in.pos.y), 0.0, 1.0);

    let epsilon = vec2<f32>(1.0, 1.0) / tileProps.dimension;
    let scale = (tileProps.dimension.x - 2.0) / tileProps.dimension.x;
    out.tex_coord = vec2<f32>(f32(in.texture_pos.x), f32(in.texture_pos.y)) / 8192.0 * scale + epsilon;
    return out;
}
",
    ),
    fragment: Some(
        r"
struct FragmentInput {
    @location(0) tex_coord: vec2<f32>,
};

// The height in meters of the DEM at `coord`, by the rule of
// `mln_util::dem`. The channels aren't rounded: the decode is linear in
// them, so where a linear sampler interpolates them, it interpolates the
// heights.
fn getElevation(coord: vec2<f32>) -> f32 {
    let rgb = textureSample(image, image_sampler, coord).rgb * 255.0;
    let unpack = tileProps.unpack;
    return (rgb.r * unpack.x + rgb.g * unpack.y + rgb.b * unpack.z + unpack.w) * tileProps.unpack_scale;
}

@fragment
fn main(in: FragmentInput) -> @location(0) vec4<f32> {
#ifdef OVERDRAW_INSPECTOR
    return vec4<f32>(1.0, 1.0, 1.0, 1.0);
#endif

    let epsilon = vec2<f32>(1.0, 1.0) / tileProps.dimension;
    let tileSize = tileProps.dimension.x - 2.0;

    let a = getElevation(in.tex_coord + vec2<f32>(-epsilon.x, -epsilon.y));
    let b = getElevation(in.tex_coord + vec2<f32>(0.0, -epsilon.y));
    let c = getElevation(in.tex_coord + vec2<f32>(epsilon.x, -epsilon.y));
    let d = getElevation(in.tex_coord + vec2<f32>(-epsilon.x, 0.0));
    let f = getElevation(in.tex_coord + vec2<f32>(epsilon.x, 0.0));
    let g = getElevation(in.tex_coord + vec2<f32>(-epsilon.x, epsilon.y));
    let h = getElevation(in.tex_coord + vec2<f32>(0.0, epsilon.y));
    let i = getElevation(in.tex_coord + vec2<f32>(epsilon.x, epsilon.y));

    var exaggerationFactor = 0.3;
    if (tileProps.zoom < 4.5) {
        exaggerationFactor = 0.35;
    }
    if (tileProps.zoom < 2.0) {
        exaggerationFactor = 0.4;
    }
    var exaggeration = 0.0;
    if (tileProps.zoom < 15.0) {
        exaggeration = (tileProps.zoom - 15.0) * exaggerationFactor;
    }

    let denom = pow(2.0, exaggeration + (28.2562 - tileProps.zoom));
    let deriv = vec2<f32>((c + f + f + i) - (a + d + d + g),
                          (g + h + h + i) - (a + b + b + c)) * tileSize / denom;

    let color = clamp(vec4<f32>(deriv.x / 8.0 + 0.5,
                                 deriv.y / 8.0 + 0.5,
                                 1.0,
                                 1.0),
                      vec4<f32>(0.0, 0.0, 0.0, 0.0),
                      vec4<f32>(1.0, 1.0, 1.0, 1.0));
    return color;
}
",
    ),
    prelude: None,
};

pub(crate) const HILLSHADE_SHADER: WgslShaderSource = WgslShaderSource {
    vertex: Some(
        r"
struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) tex_coord: vec2<f32>,
};

@vertex
fn main(in: VertexInput) -> VertexOutput {
    var out: VertexOutput;
    let drawable = drawableVector[globalIndex.value];
    let pos = vec2<f32>(f32(in.pos.x), f32(in.pos.y));
    out.position = projectTileWithPoles(pos, pos, projectionVector[globalIndex.value]);

    var tex = vec2<f32>(f32(in.texture_pos.x), f32(in.texture_pos.y)) / 8192.0;
    tex.y = 1.0 - tex.y;
    out.tex_coord = tex;
    return out;
}
",
    ),
    fragment: Some(concat!(
        hillshade_shading!(),
        r"
struct FragmentInput {
    @location(0) tex_coord: vec2<f32>,
};

@fragment
fn main(in: FragmentInput) -> @location(0) vec4<f32> {
#ifdef OVERDRAW_INSPECTOR
    return vec4<f32>(1.0, 1.0, 1.0, 1.0);
#endif

    let tileProps = tilePropsVector[globalIndex.value];
    let pixel = textureSample(image, image_sampler, in.tex_coord);
    // Without lights, the slopes as they were prepared, which the terrain's
    // surface shades.
    if (tileProps.num_lights == 0) {
        return pixel;
    }

    let latRange = tileProps.latrange;
    let latitude = (latRange.x - latRange.y) * in.tex_coord.y + latRange.y;
    let scaleFactor = cos(radians(latitude));

    let deriv = ((pixel.rg * 8.0) - vec2<f32>(4.0, 4.0)) / scaleFactor;
    return hillshade_color(deriv, tileProps);
}
"
    )),
    prelude: None,
};

/// The line programs' `LINE_NORMAL_SCALE` and `MAX_LINE_DISTANCE`.
macro_rules! line_constants {
    () => {
        r"
const LINE_NORMAL_SCALE: f32 = 1.0 / 63.0;  // 1.0 / (127 / 2)
const MAX_LINE_DISTANCE: f32 = 32767.0;
"
    };
}

pub(crate) const LINE_GRADIENT_SDF_SHADER: WgslShaderSource = WgslShaderSource {
    vertex: Some(concat!(
        line_constants!(),
        r"
// The key's DEVICE_PIXEL_RATIO, which the pipeline sets.
override DEVICE_PIXEL_RATIO: f32 = 1.0;

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) v_width2: vec2<f32>,
    @location(1) v_normal: vec2<f32>,
    @location(2) v_gamma_scale: f32,
    @location(3) v_tex_a: vec2<f32>,
    @location(4) v_tex_b: vec2<f32>,
#ifndef HAS_UNIFORM_u_blur
    @location(5) v_blur: f32,
#endif
#ifndef HAS_UNIFORM_u_opacity
    @location(6) v_opacity: f32,
#endif
#ifndef HAS_UNIFORM_u_floorwidth
    @location(7) v_floorwidth: f32,
#endif
    @location(8) v_lineprogress: f32,
#ifdef PROJECTION_GLOBE
    @location(9) v_tile_x: f32,
#endif
#ifndef HAS_UNIFORM_u_dasharray
    @location(10) @interpolate(flat) v_dash_width: f32,
#endif
};

@vertex
fn main(in: VertexInput) -> VertexOutput {
    var out: VertexOutput;
    let index = globalIndex.value;
    let drawable = drawableVector[index];

    // Constants
    let LINE_DISTANCE_SCALE = 2.0;
    let MAX_LINE_DISTANCE = 32767.0;

    // Unpack vertex data
    let a_extrude = vec2<f32>(f32(in.data.x), f32(in.data.y)) - 128.0;
    let a_direction = f32(in.data.z % 4u) - 1.0;
    let v_linesofar = (floor(f32(in.data.z) * 0.25) + f32(in.data.w) * 64.0) * LINE_DISTANCE_SCALE;
    let v_lineprogress = (f32(in.data.z / 4u) + f32(in.data.w) * 64.0) * 2.0 / MAX_LINE_DISTANCE;

    // Unpack position and normal
    let pos = floor(vec2<f32>(f32(in.pos_normal.x), f32(in.pos_normal.y)) * 0.5);
    let normal = vec2<f32>(f32(in.pos_normal.x), f32(in.pos_normal.y)) - 2.0 * pos;
    let v_normal = vec2<f32>(normal.x, normal.y * 2.0 - 1.0);

    // Unpack attributes using helper functions
#ifndef HAS_UNIFORM_u_blur
    out.v_blur = unpack_mix_float(in.blur, drawable.blur_t);
#endif
#ifndef HAS_UNIFORM_u_opacity
    out.v_opacity = unpack_mix_float(in.opacity, drawable.opacity_t);
#endif
#ifdef HAS_UNIFORM_u_gapwidth
    let gapwidth = props.gapwidth / 2.0;
#else
    let gapwidth = unpack_mix_float(in.gapwidth, drawable.gapwidth_t) / 2.0;
#endif
#ifdef HAS_UNIFORM_u_offset
    let offset = -props.offset;
#else
    let offset = -unpack_mix_float(in.offset, drawable.offset_t);
#endif
#ifdef HAS_UNIFORM_u_width
    let width = props.width;
#else
    let width = unpack_mix_float(in.width, drawable.width_t);
#endif
#ifdef HAS_UNIFORM_u_floorwidth
    let floorwidth = props.floorwidth;
#else
    let floorwidth = unpack_mix_float(in.floorwidth, drawable.floorwidth_t);
    out.v_floorwidth = floorwidth;
#endif

    let pixel_ratio = max(DEVICE_PIXEL_RATIO, 1e-6);
    let antialiasing = 0.5 / pixel_ratio;

    let halfwidth = width / 2.0;
    let inset = gapwidth + select(0.0, antialiasing, gapwidth > 0.0);
    let outset = gapwidth + halfwidth * select(1.0, 2.0, gapwidth > 0.0) +
                 select(0.0, antialiasing, halfwidth != 0.0);

    // Scale the extrusion vector
    let dist = outset * a_extrude * LINE_NORMAL_SCALE;

    // Calculate the offset
    let u = 0.5 * a_direction;
    let t = 1.0 - abs(u);
    let offset2 = offset * a_extrude * LINE_NORMAL_SCALE * v_normal.y * mat2x2<f32>(t, -u, u, t);

#ifdef PROJECTION_GLOBE
    let adjustedThickness = projectLineThickness(pos.y, projectionVector[globalIndex.value]);
    let projected_no_extrude = projectTile(pos + offset2 / drawable.ratio * adjustedThickness, projectionVector[globalIndex.value]);
    let extrudedPos = pos + (offset2 + dist) / drawable.ratio * adjustedThickness;
    let position_globe = projectTile(extrudedPos, projectionVector[globalIndex.value]);
    out.v_tile_x = antimeridianClipX(extrudedPos, projectionVector[globalIndex.value]);
    let projected_extrude = position_globe - projected_no_extrude;
    let position = position_globe;
#else
    let projected_extrude = drawable.matrix * vec4<f32>(dist / drawable.ratio, 0.0, 0.0);
    let position = drawable.matrix * vec4<f32>(pos + offset2 / drawable.ratio, 0.0, 1.0) + projected_extrude;
#endif

    // Calculate gamma scale
    let extrude_length_without_perspective = length(dist);
    let extrude_length_with_perspective = length(projected_extrude.xy / position.w * paintParams.units_to_pixels);

    // Calculate texture coordinates
    var patternscale_a = drawable.patternscale_a;
    var patternscale_b = drawable.patternscale_b;
    var tex_y_a = drawable.tex_y_a;
    var tex_y_b = drawable.tex_y_b;
#ifndef HAS_UNIFORM_u_dasharray
    // The dash attribute: the pattern's middle row, row span and length.
    patternscale_a = vec2<f32>(drawable.patternscale_a.x / in.dasharray.z, -in.dasharray.y / 2.0 / drawable.atlas_height);
    patternscale_b = vec2<f32>(drawable.patternscale_b.x / in.dasharray.z, patternscale_a.y);
    tex_y_a = (in.dasharray.x + 0.5) / drawable.atlas_height;
    tex_y_b = tex_y_a;
    out.v_dash_width = in.dasharray.z;
#endif
    let tex_a = vec2<f32>(
        v_linesofar * patternscale_a.x / floorwidth,
        v_normal.y * patternscale_a.y + tex_y_a
    );
    let tex_b = vec2<f32>(
        v_linesofar * patternscale_b.x / floorwidth,
        v_normal.y * patternscale_b.y + tex_y_b
    );

    out.position = position;
    out.v_width2 = vec2<f32>(outset, inset);
    out.v_normal = v_normal;
    out.v_gamma_scale = extrude_length_without_perspective / extrude_length_with_perspective;
    out.v_tex_a = tex_a;
    out.v_tex_b = tex_b;
    out.v_lineprogress = v_lineprogress;

    return out;
}
",
    )),
    fragment: Some(
        r"
// The key's DEVICE_PIXEL_RATIO, which the pipeline sets.
override DEVICE_PIXEL_RATIO: f32 = 1.0;

struct FragmentInput {
    @location(0) v_width2: vec2<f32>,
    @location(1) v_normal: vec2<f32>,
    @location(2) v_gamma_scale: f32,
    @location(3) v_tex_a: vec2<f32>,
    @location(4) v_tex_b: vec2<f32>,
#ifndef HAS_UNIFORM_u_blur
    @location(5) v_blur: f32,
#endif
#ifndef HAS_UNIFORM_u_opacity
    @location(6) v_opacity: f32,
#endif
#ifndef HAS_UNIFORM_u_floorwidth
    @location(7) v_floorwidth: f32,
#endif
    @location(8) v_lineprogress: f32,
#ifdef PROJECTION_GLOBE
    @location(9) v_tile_x: f32,
#endif
#ifndef HAS_UNIFORM_u_dasharray
    @location(10) @interpolate(flat) v_dash_width: f32,
#endif
};

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
    let tileProps = tilePropsVector[globalIndex.value];
#ifdef HAS_UNIFORM_u_blur
    let blur = props.blur;
#else
    let blur = in.v_blur;
#endif
#ifdef HAS_UNIFORM_u_opacity
    let opacity = props.opacity;
#else
    let opacity = in.v_opacity;
#endif
#ifdef HAS_UNIFORM_u_floorwidth
    let floorwidth = props.floorwidth;
#else
    let floorwidth = in.v_floorwidth;
#endif

    // Calculate the distance of the pixel from the line
    let dist = length(in.v_normal) * in.v_width2.x;

    // Calculate the antialiasing fade factor
    let pixel_ratio = max(DEVICE_PIXEL_RATIO, 1e-6);
    let blur2 = (blur + 1.0 / pixel_ratio) * in.v_gamma_scale;
    let alpha = clamp(min(dist - (in.v_width2.y - blur2), in.v_width2.x - dist) / blur2, 0.0, 1.0);

    // Sample SDF texture
    let dist_a = textureSample(image_dash, image_dash_sampler, in.v_tex_a).r;
    let dist_b = textureSample(image_dash, image_dash_sampler, in.v_tex_b).r;
    let sdfdist = mix(dist_a, dist_b, tileProps.mix);
    var sdfgamma = tileProps.sdfgamma;
#ifndef HAS_UNIFORM_u_dasharray
    sdfgamma = sdfgamma / in.v_dash_width;
#endif

    // Calculate SDF alpha
    let sdf_alpha = smoothstep(0.5 - sdfgamma / floorwidth, 0.5 + sdfgamma / floorwidth, sdfdist);

    // The gradient ramp is stored in a texture indexed by the ratio along the entire line
    let color = textureSample(image, image_sampler, vec2<f32>(in.v_lineprogress, 0.5));

    return color * (alpha * opacity * sdf_alpha);
}
",
    ),
    prelude: None,
};

pub(crate) const LINE_GRADIENT_SHADER: WgslShaderSource = WgslShaderSource {
    vertex: Some(concat!(
        line_constants!(),
        r"
// The key's DEVICE_PIXEL_RATIO, which the pipeline sets.
override DEVICE_PIXEL_RATIO: f32 = 1.0;

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) v_width2: vec2<f32>,
    @location(1) v_normal: vec2<f32>,
    @location(2) v_gamma_scale: f32,
    @location(3) v_lineprogress: f32,
#ifndef HAS_UNIFORM_u_blur
    @location(4) v_blur: f32,
#endif
#ifndef HAS_UNIFORM_u_opacity
    @location(5) v_opacity: f32,
#endif
#ifdef PROJECTION_GLOBE
    @location(6) v_tile_x: f32,
#endif
};

@vertex
fn main(in: VertexInput) -> VertexOutput {
    var out: VertexOutput;
    let drawable = drawableVector[globalIndex.value];

    // Constants
    let pixel_ratio = max(DEVICE_PIXEL_RATIO, 1e-6);
    let antialiasing = 0.5 / pixel_ratio;
    let MAX_LINE_DISTANCE = 32767.0;

    // Unpack vertex data
    let a_extrude = vec2<f32>(f32(in.data.x), f32(in.data.y)) - 128.0;
    let a_direction = f32(in.data.z % 4u) - 1.0;
    let v_lineprogress = (f32(in.data.z / 4u) + f32(in.data.w) * 64.0) * 2.0 / MAX_LINE_DISTANCE;

    // Unpack position and normal
    let pos = floor(vec2<f32>(f32(in.pos_normal.x), f32(in.pos_normal.y)) * 0.5);
    let normal = vec2<f32>(f32(in.pos_normal.x), f32(in.pos_normal.y)) - 2.0 * pos;
    let v_normal = vec2<f32>(normal.x, normal.y * 2.0 - 1.0);

    // Unpack attributes using helper functions
#ifndef HAS_UNIFORM_u_blur
    out.v_blur = unpack_mix_float(in.blur, drawable.blur_t);
#endif
#ifndef HAS_UNIFORM_u_opacity
    out.v_opacity = unpack_mix_float(in.opacity, drawable.opacity_t);
#endif
#ifdef HAS_UNIFORM_u_gapwidth
    let gapwidth = props.gapwidth / 2.0;
#else
    let gapwidth = unpack_mix_float(in.gapwidth, drawable.gapwidth_t) / 2.0;
#endif
#ifdef HAS_UNIFORM_u_offset
    let offset = -props.offset;
#else
    let offset = -unpack_mix_float(in.offset, drawable.offset_t);
#endif
#ifdef HAS_UNIFORM_u_width
    let width = props.width;
#else
    let width = unpack_mix_float(in.width, drawable.width_t);
#endif

    let halfwidth = width / 2.0;
    let inset = gapwidth + select(0.0, antialiasing, gapwidth > 0.0);
    let outset = gapwidth + halfwidth * select(1.0, 2.0, gapwidth > 0.0) +
                 select(0.0, antialiasing, halfwidth != 0.0);

    // Scale the extrusion vector down to a normal and then up by the line width
    let dist = outset * a_extrude * LINE_NORMAL_SCALE;

    // Calculate the offset
    let u = 0.5 * a_direction;
    let t = 1.0 - abs(u);
    let offset2 = offset * a_extrude * LINE_NORMAL_SCALE * v_normal.y * mat2x2<f32>(t, -u, u, t);

#ifdef PROJECTION_GLOBE
    let adjustedThickness = projectLineThickness(pos.y, projectionVector[globalIndex.value]);
    let projected_no_extrude = projectTile(pos + offset2 / drawable.ratio * adjustedThickness, projectionVector[globalIndex.value]);
    let extrudedPos = pos + (offset2 + dist) / drawable.ratio * adjustedThickness;
    let position_globe = projectTile(extrudedPos, projectionVector[globalIndex.value]);
    out.v_tile_x = antimeridianClipX(extrudedPos, projectionVector[globalIndex.value]);
    let projected_extrude = position_globe - projected_no_extrude;
    let position = position_globe;
#else
    let projected_extrude = drawable.matrix * vec4<f32>(dist / drawable.ratio, 0.0, 0.0);
    let position = drawable.matrix * vec4<f32>(pos + offset2 / drawable.ratio, 0.0, 1.0) + projected_extrude;
#endif

    // Calculate gamma scale
    let extrude_length_without_perspective = length(dist);
    let extrude_length_with_perspective = length(projected_extrude.xy / position.w * paintParams.units_to_pixels);

    out.position = position;
    out.v_width2 = vec2<f32>(outset, inset);
    out.v_normal = v_normal;
    out.v_gamma_scale = extrude_length_without_perspective / extrude_length_with_perspective;
    out.v_lineprogress = v_lineprogress;

    return out;
}
",
    )),
    fragment: Some(
        r"
// The key's DEVICE_PIXEL_RATIO, which the pipeline sets.
override DEVICE_PIXEL_RATIO: f32 = 1.0;

struct FragmentInput {
    @location(0) v_width2: vec2<f32>,
    @location(1) v_normal: vec2<f32>,
    @location(2) v_gamma_scale: f32,
    @location(3) v_lineprogress: f32,
#ifndef HAS_UNIFORM_u_blur
    @location(4) v_blur: f32,
#endif
#ifndef HAS_UNIFORM_u_opacity
    @location(5) v_opacity: f32,
#endif
#ifdef PROJECTION_GLOBE
    @location(6) v_tile_x: f32,
#endif
};

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
#ifdef HAS_UNIFORM_u_blur
    let blur = props.blur;
#else
    let blur = in.v_blur;
#endif
#ifdef HAS_UNIFORM_u_opacity
    let opacity = props.opacity;
#else
    let opacity = in.v_opacity;
#endif

    // Calculate the distance of the pixel from the line
    let dist = length(in.v_normal) * in.v_width2.x;

    // Calculate the antialiasing fade factor
    let pixel_ratio = max(DEVICE_PIXEL_RATIO, 1e-6);
    let blur2 = (blur + 1.0 / pixel_ratio) * in.v_gamma_scale;
    let alpha = clamp(min(dist - (in.v_width2.y - blur2), in.v_width2.x - dist) / blur2, 0.0, 1.0);

    // Sample gradient texture
    let color = textureSample(image, image_sampler, vec2<f32>(in.v_lineprogress, 0.5));

    return color * (alpha * opacity);
}
",
    ),
    prelude: None,
};

pub(crate) const LINE_PATTERN_SHADER: WgslShaderSource = WgslShaderSource {
    vertex: Some(concat!(
        line_constants!(),
        r"
// The key's DEVICE_PIXEL_RATIO, which the pipeline sets.
override DEVICE_PIXEL_RATIO: f32 = 1.0;

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) v_width2: vec2<f32>,
    @location(1) v_normal: vec2<f32>,
    @location(2) v_gamma_scale: f32,
    @location(3) v_linesofar: f32,
#ifndef HAS_UNIFORM_u_blur
    @location(4) v_blur: f32,
#endif
#ifndef HAS_UNIFORM_u_opacity
    @location(5) v_opacity: f32,
#endif
#ifndef HAS_UNIFORM_u_pattern_from
    @location(6) @interpolate(flat) v_pattern_from: vec4<f32>,
#endif
#ifndef HAS_UNIFORM_u_pattern_to
    @location(7) @interpolate(flat) v_pattern_to: vec4<f32>,
#endif
#ifndef HAS_UNIFORM_u_width
    @location(8) v_floorwidth: f32,
#endif
#ifdef PROJECTION_GLOBE
    @location(9) v_tile_x: f32,
#endif
};

@vertex
fn main(in: VertexInput) -> VertexOutput {
    var out: VertexOutput;
    let index = globalIndex.value;
    let drawable = drawableVector[index];

    let ratio = max(drawable.ratio, 1e-6);
    let antialiasing = 0.5 / DEVICE_PIXEL_RATIO;
    let lineDistanceScale = 2.0;

    let a_extrude = vec2<f32>(f32(in.data.x), f32(in.data.y)) - 128.0;
    let a_direction = f32(in.data.z & 3u) - 1.0;
    let v_linesofar = (floor(f32(in.data.z) * 0.25) + f32(in.data.w) * 64.0) * lineDistanceScale;

    let raw_pos = vec2<f32>(f32(in.pos_normal.x), f32(in.pos_normal.y));
    let pos = floor(raw_pos * 0.5);
    let normal = raw_pos - 2.0 * pos;
    let v_normal = vec2<f32>(normal.x, normal.y * 2.0 - 1.0);

#ifndef HAS_UNIFORM_u_blur
    out.v_blur = unpack_mix_float(in.blur, drawable.blur_t);
#endif
#ifndef HAS_UNIFORM_u_opacity
    out.v_opacity = unpack_mix_float(in.opacity, drawable.opacity_t);
#endif
#ifdef HAS_UNIFORM_u_gapwidth
    let gapwidth = props.gapwidth * 0.5;
#else
    let gapwidth = unpack_mix_float(in.gapwidth, drawable.gapwidth_t) * 0.5;
#endif
#ifdef HAS_UNIFORM_u_offset
    let offset = -props.offset;
#else
    let offset = -unpack_mix_float(in.offset, drawable.offset_t);
#endif
    // The floor width comes from the width's attribute.
#ifdef HAS_UNIFORM_u_width
    let width = props.width;
#else
    let width = unpack_mix_float(in.width, drawable.width_t);
    out.v_floorwidth = unpack_mix_float(in.width, drawable.floorwidth_t);
#endif
#ifndef HAS_UNIFORM_u_pattern_from
    out.v_pattern_from = vec4<f32>(in.pattern_from);
#endif
#ifndef HAS_UNIFORM_u_pattern_to
    out.v_pattern_to = vec4<f32>(in.pattern_to);
#endif

    let halfwidth = width * 0.5;
    let inset = gapwidth + select(0.0, antialiasing, gapwidth > 0.0);
    let outset = gapwidth + halfwidth * select(1.0, 2.0, gapwidth > 0.0) + select(0.0, antialiasing, halfwidth != 0.0);

    let dist = outset * a_extrude * LINE_NORMAL_SCALE;

    let u = 0.5 * a_direction;
    let t = 1.0 - abs(u);
    let offset2 = offset * a_extrude * LINE_NORMAL_SCALE * v_normal.y * mat2x2<f32>(t, -u, u, t);

#ifdef PROJECTION_GLOBE
    let adjustedThickness = projectLineThickness(pos.y, projectionVector[globalIndex.value]);
    let projected_no_extrude = projectTile(pos + offset2 / ratio * adjustedThickness, projectionVector[globalIndex.value]);
    let extrudedPos = pos + (offset2 + dist) / ratio * adjustedThickness;
    let position_globe = projectTile(extrudedPos, projectionVector[globalIndex.value]);
    out.v_tile_x = antimeridianClipX(extrudedPos, projectionVector[globalIndex.value]);
    let projected_extrude = position_globe - projected_no_extrude;
    let position = position_globe;
#else
    let projected_extrude = drawable.matrix * vec4<f32>(dist / ratio, 0.0, 0.0);
    let position = drawable.matrix * vec4<f32>(pos + offset2 / ratio, 0.0, 1.0) + projected_extrude;
#endif

    let extrude_length_without_perspective = length(dist);
    let extrude_length_with_perspective = length(projected_extrude.xy / position.w * paintParams.units_to_pixels);
    let gamma_scale = extrude_length_without_perspective / max(extrude_length_with_perspective, 1e-6);

    out.position = position;
    out.v_width2 = vec2<f32>(outset, inset);
    out.v_normal = v_normal;
    out.v_gamma_scale = gamma_scale;
    out.v_linesofar = v_linesofar;

    return out;
}
",
    )),
    fragment: Some(
        r"
// The key's DEVICE_PIXEL_RATIO, which the pipeline sets.
override DEVICE_PIXEL_RATIO: f32 = 1.0;

struct FragmentInput {
    @location(0) v_width2: vec2<f32>,
    @location(1) v_normal: vec2<f32>,
    @location(2) v_gamma_scale: f32,
    @location(3) v_linesofar: f32,
#ifndef HAS_UNIFORM_u_blur
    @location(4) v_blur: f32,
#endif
#ifndef HAS_UNIFORM_u_opacity
    @location(5) v_opacity: f32,
#endif
#ifndef HAS_UNIFORM_u_pattern_from
    @location(6) @interpolate(flat) v_pattern_from: vec4<f32>,
#endif
#ifndef HAS_UNIFORM_u_pattern_to
    @location(7) @interpolate(flat) v_pattern_to: vec4<f32>,
#endif
#ifndef HAS_UNIFORM_u_width
    @location(8) v_floorwidth: f32,
#endif
#ifdef PROJECTION_GLOBE
    @location(9) v_tile_x: f32,
#endif
};

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
    let tileProps = tilePropsVector[globalIndex.value];

#ifdef HAS_UNIFORM_u_pattern_from
    let pattern_from = tileProps.pattern_from;
#else
    let pattern_from = in.v_pattern_from;
#endif
#ifdef HAS_UNIFORM_u_pattern_to
    let pattern_to = tileProps.pattern_to;
#else
    let pattern_to = in.v_pattern_to;
#endif
#ifdef HAS_UNIFORM_u_blur
    let blur = props.blur;
#else
    let blur = in.v_blur;
#endif
#ifdef HAS_UNIFORM_u_opacity
    let opacity = props.opacity;
#else
    let opacity = in.v_opacity;
#endif
#ifdef HAS_UNIFORM_u_width
    let floorwidth = max(props.floorwidth, 1e-6);
#else
    let floorwidth = max(in.v_floorwidth, 1e-6);
#endif

    let pattern_tl_a = pattern_from.xy;
    let pattern_br_a = pattern_from.zw;
    let pattern_tl_b = pattern_to.xy;
    let pattern_br_b = pattern_to.zw;

    let pixelRatio = tileProps.scale.x;
    let tileZoomRatio = max(tileProps.scale.y, 1e-6);
    let fromScale = tileProps.scale.z;
    let toScale = tileProps.scale.w;

    let display_size_a = vec2<f32>((pattern_br_a.x - pattern_tl_a.x) / pixelRatio,
                                   (pattern_br_a.y - pattern_tl_a.y) / pixelRatio);
    let display_size_b = vec2<f32>((pattern_br_b.x - pattern_tl_b.x) / pixelRatio,
                                   (pattern_br_b.y - pattern_tl_b.y) / pixelRatio);

    let pattern_size_a = vec2<f32>(display_size_a.x * fromScale / tileZoomRatio, display_size_a.y);
    let pattern_size_b = vec2<f32>(display_size_b.x * toScale / tileZoomRatio, display_size_b.y);

    let aspect_a = display_size_a.y / floorwidth;
    let aspect_b = display_size_b.y / floorwidth;

    let dist = length(in.v_normal) * in.v_width2.x;
    let invPixelRatio = 1.0 / max(DEVICE_PIXEL_RATIO, 1e-6);
    let blur2 = (blur + invPixelRatio) * in.v_gamma_scale;
    let alpha = clamp(min(dist - (in.v_width2.y - blur2), in.v_width2.x - dist) / blur2, 0.0, 1.0);

    let patternSizeAX = max(pattern_size_a.x, 1e-6);
    let patternSizeBX = max(pattern_size_b.x, 1e-6);

    let y = 0.5 * in.v_normal.y + 0.5;

    let texel_size = 1.0 / tileProps.texsize;
    let half_texel = 0.5 * texel_size;
    // A crossfade at 0 or 1 shows one pattern, so skip the other.
    var color_a = vec4<f32>(0.0);
    if (tileProps.fade < 1.0) {
        let x_a = glMod(in.v_linesofar / patternSizeAX * aspect_a, 1.0);
        var pos_a = atlas_pos((pattern_tl_a - 1.0) * texel_size, (pattern_br_a - pattern_tl_a + 2.0) * texel_size * vec2<f32>(x_a, y));
        pos_a = clamp(pos_a, (pattern_tl_a - 1.0) * texel_size + half_texel, (pattern_br_a + 1.0) * texel_size - half_texel);
        color_a = textureSample(image, image_sampler, pos_a);
    }
    var color_b = vec4<f32>(0.0);
    if (tileProps.fade > 0.0) {
        let x_b = glMod(in.v_linesofar / patternSizeBX * aspect_b, 1.0);
        var pos_b = atlas_pos((pattern_tl_b - 1.0) * texel_size, (pattern_br_b - pattern_tl_b + 2.0) * texel_size * vec2<f32>(x_b, y));
        pos_b = clamp(pos_b, (pattern_tl_b - 1.0) * texel_size + half_texel, (pattern_br_b + 1.0) * texel_size - half_texel);
        color_b = textureSample(image, image_sampler, pos_b);
    }

    let pattern_color = mix(color_a, color_b, tileProps.fade);

    return pattern_color * (alpha * opacity);
}
",
    ),
    prelude: None,
};

pub(crate) const LINE_SDF_SHADER: WgslShaderSource = WgslShaderSource {
    vertex: Some(concat!(
        line_constants!(),
        r"
// The key's DEVICE_PIXEL_RATIO, which the pipeline sets.
override DEVICE_PIXEL_RATIO: f32 = 1.0;

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) v_width2: vec2<f32>,
    @location(1) v_normal: vec2<f32>,
    @location(2) v_gamma_scale: f32,
    @location(3) v_tex_a: vec2<f32>,
    @location(4) v_tex_b: vec2<f32>,
#ifndef HAS_UNIFORM_u_color
    @location(5) v_color: vec4<f32>,
#endif
#ifndef HAS_UNIFORM_u_blur
    @location(6) v_blur: f32,
#endif
#ifndef HAS_UNIFORM_u_opacity
    @location(7) v_opacity: f32,
#endif
#ifndef HAS_UNIFORM_u_floorwidth
    @location(8) v_floorwidth: f32,
#endif
#ifdef PROJECTION_GLOBE
    @location(9) v_tile_x: f32,
#endif
#ifndef HAS_UNIFORM_u_dasharray
    @location(10) @interpolate(flat) v_dash_width: f32,
#endif
};

@vertex
fn main(in: VertexInput) -> VertexOutput {
    var out: VertexOutput;
    let index = globalIndex.value;
    let drawable = drawableVector[index];

    // Constants
    let LINE_DISTANCE_SCALE = 2.0;

    // Unpack vertex data
    let a_extrude = vec2<f32>(f32(in.data.x), f32(in.data.y)) - 128.0;
    let a_direction = f32(in.data.z % 4u) - 1.0;
    let v_linesofar = (floor(f32(in.data.z) * 0.25) + f32(in.data.w) * 64.0) * LINE_DISTANCE_SCALE;

    // Unpack position and normal
    let pos = floor(vec2<f32>(f32(in.pos_normal.x), f32(in.pos_normal.y)) * 0.5);
    let normal = vec2<f32>(f32(in.pos_normal.x), f32(in.pos_normal.y)) - 2.0 * pos;
    let v_normal = vec2<f32>(normal.x, normal.y * 2.0 - 1.0);

    // Unpack attributes using helper functions
#ifndef HAS_UNIFORM_u_color
    out.v_color = unpack_mix_color(in.color, drawable.color_t);
#endif
#ifndef HAS_UNIFORM_u_blur
    out.v_blur = unpack_mix_float(in.blur, drawable.blur_t);
#endif
#ifndef HAS_UNIFORM_u_opacity
    out.v_opacity = unpack_mix_float(in.opacity, drawable.opacity_t);
#endif
#ifdef HAS_UNIFORM_u_gapwidth
    let gapwidth = props.gapwidth / 2.0;
#else
    let gapwidth = unpack_mix_float(in.gapwidth, drawable.gapwidth_t) / 2.0;
#endif
#ifdef HAS_UNIFORM_u_offset
    let offset = -props.offset;
#else
    let offset = -unpack_mix_float(in.offset, drawable.offset_t);
#endif
#ifdef HAS_UNIFORM_u_width
    let width = props.width;
#else
    let width = unpack_mix_float(in.width, drawable.width_t);
#endif
#ifdef HAS_UNIFORM_u_floorwidth
    let floorwidth = props.floorwidth;
#else
    let floorwidth = unpack_mix_float(in.floorwidth, drawable.floorwidth_t);
    out.v_floorwidth = floorwidth;
#endif

    let pixel_ratio = max(DEVICE_PIXEL_RATIO, 1e-6);
    let antialiasing = 0.5 / pixel_ratio;

    let halfwidth = width / 2.0;
    let inset = gapwidth + select(0.0, antialiasing, gapwidth > 0.0);
    let outset = gapwidth + halfwidth * select(1.0, 2.0, gapwidth > 0.0) +
                 select(0.0, antialiasing, halfwidth != 0.0);

    // Scale the extrusion vector
    let dist = outset * a_extrude * LINE_NORMAL_SCALE;

    // Calculate the offset
    let u = 0.5 * a_direction;
    let t = 1.0 - abs(u);
    let offset2 = offset * a_extrude * LINE_NORMAL_SCALE * v_normal.y * mat2x2<f32>(t, -u, u, t);

#ifdef PROJECTION_GLOBE
    let adjustedThickness = projectLineThickness(pos.y, projectionVector[globalIndex.value]);
    let projected_no_extrude = projectTile(pos + offset2 / drawable.ratio * adjustedThickness, projectionVector[globalIndex.value]);
    let extrudedPos = pos + (offset2 + dist) / drawable.ratio * adjustedThickness;
    let position_globe = projectTile(extrudedPos, projectionVector[globalIndex.value]);
    out.v_tile_x = antimeridianClipX(extrudedPos, projectionVector[globalIndex.value]);
    let projected_extrude = position_globe - projected_no_extrude;
    let position = position_globe;
#else
    let projected_extrude = drawable.matrix * vec4<f32>(dist / drawable.ratio, 0.0, 0.0);
    let position = drawable.matrix * vec4<f32>(pos + offset2 / drawable.ratio, 0.0, 1.0) + projected_extrude;
#endif

    // Calculate gamma scale
    let extrude_length_without_perspective = length(dist);
    let extrude_length_with_perspective = length(projected_extrude.xy / position.w * paintParams.units_to_pixels);

    // Calculate texture coordinates
    var patternscale_a = drawable.patternscale_a;
    var patternscale_b = drawable.patternscale_b;
    var tex_y_a = drawable.tex_y_a;
    var tex_y_b = drawable.tex_y_b;
#ifndef HAS_UNIFORM_u_dasharray
    // The dash attribute: the pattern's middle row, row span and length.
    patternscale_a = vec2<f32>(drawable.patternscale_a.x / in.dasharray.z, -in.dasharray.y / 2.0 / drawable.atlas_height);
    patternscale_b = vec2<f32>(drawable.patternscale_b.x / in.dasharray.z, patternscale_a.y);
    tex_y_a = (in.dasharray.x + 0.5) / drawable.atlas_height;
    tex_y_b = tex_y_a;
    out.v_dash_width = in.dasharray.z;
#endif
    let tex_a = vec2<f32>(
        v_linesofar * patternscale_a.x / floorwidth,
        v_normal.y * patternscale_a.y + tex_y_a
    );
    let tex_b = vec2<f32>(
        v_linesofar * patternscale_b.x / floorwidth,
        v_normal.y * patternscale_b.y + tex_y_b
    );

    out.position = position;
    out.v_width2 = vec2<f32>(outset, inset);
    out.v_normal = v_normal;
    out.v_gamma_scale = extrude_length_without_perspective / extrude_length_with_perspective;
    out.v_tex_a = tex_a;
    out.v_tex_b = tex_b;

    return out;
}
",
    )),
    fragment: Some(
        r"
// The key's DEVICE_PIXEL_RATIO, which the pipeline sets.
override DEVICE_PIXEL_RATIO: f32 = 1.0;

struct FragmentInput {
    @location(0) v_width2: vec2<f32>,
    @location(1) v_normal: vec2<f32>,
    @location(2) v_gamma_scale: f32,
    @location(3) v_tex_a: vec2<f32>,
    @location(4) v_tex_b: vec2<f32>,
#ifndef HAS_UNIFORM_u_color
    @location(5) v_color: vec4<f32>,
#endif
#ifndef HAS_UNIFORM_u_blur
    @location(6) v_blur: f32,
#endif
#ifndef HAS_UNIFORM_u_opacity
    @location(7) v_opacity: f32,
#endif
#ifndef HAS_UNIFORM_u_floorwidth
    @location(8) v_floorwidth: f32,
#endif
#ifdef PROJECTION_GLOBE
    @location(9) v_tile_x: f32,
#endif
#ifndef HAS_UNIFORM_u_dasharray
    @location(10) @interpolate(flat) v_dash_width: f32,
#endif
};

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
    let tileProps = tilePropsVector[globalIndex.value];
#ifdef HAS_UNIFORM_u_color
    let color = props.color;
#else
    let color = in.v_color;
#endif
#ifdef HAS_UNIFORM_u_blur
    let blur = props.blur;
#else
    let blur = in.v_blur;
#endif
#ifdef HAS_UNIFORM_u_opacity
    let opacity = props.opacity;
#else
    let opacity = in.v_opacity;
#endif
#ifdef HAS_UNIFORM_u_floorwidth
    let floorwidth = props.floorwidth;
#else
    let floorwidth = in.v_floorwidth;
#endif

    // Calculate the distance of the pixel from the line
    let dist = length(in.v_normal) * in.v_width2.x;

    // Calculate the antialiasing fade factor
    let pixel_ratio = max(DEVICE_PIXEL_RATIO, 1e-6);
    let blur2 = (blur + 1.0 / pixel_ratio) * in.v_gamma_scale;
    let alpha = clamp(min(dist - (in.v_width2.y - blur2), in.v_width2.x - dist) / blur2, 0.0, 1.0);

    // Sample SDF texture
    let dist_a = textureSample(image, image_sampler, in.v_tex_a).r;
    let dist_b = textureSample(image, image_sampler, in.v_tex_b).r;
    let sdfdist = mix(dist_a, dist_b, tileProps.mix);
    var sdfgamma = tileProps.sdfgamma;
#ifndef HAS_UNIFORM_u_dasharray
    sdfgamma = sdfgamma / in.v_dash_width;
#endif

    // Calculate SDF alpha
    let sdf_alpha = smoothstep(0.5 - sdfgamma / floorwidth, 0.5 + sdfgamma / floorwidth, sdfdist);

    return color * (alpha * opacity * sdf_alpha);
}
",
    ),
    prelude: None,
};

pub(crate) const LINE_SHADER: WgslShaderSource = WgslShaderSource {
    vertex: Some(concat!(
        line_constants!(),
        r"
// The key's DEVICE_PIXEL_RATIO, which the pipeline sets.
override DEVICE_PIXEL_RATIO: f32 = 1.0;

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) v_width2: vec2<f32>,
    @location(1) v_normal: vec2<f32>,
    @location(2) v_gamma_scale: f32,
#ifndef HAS_UNIFORM_u_color
    @location(3) v_color: vec4<f32>,
#endif
#ifndef HAS_UNIFORM_u_blur
    @location(4) v_blur: f32,
#endif
#ifndef HAS_UNIFORM_u_opacity
    @location(5) v_opacity: f32,
#endif
#ifdef PROJECTION_GLOBE
    @location(6) v_tile_x: f32,
#endif
};

@vertex
fn main(in: VertexInput) -> VertexOutput {
    var out: VertexOutput;
    let drawable = drawableVector[globalIndex.value];
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

#ifndef HAS_UNIFORM_u_color
    out.v_color = unpack_mix_color(in.color, drawable.color_t);
#endif
#ifndef HAS_UNIFORM_u_blur
    out.v_blur = unpack_mix_float(in.blur, drawable.blur_t);
#endif
#ifndef HAS_UNIFORM_u_opacity
    out.v_opacity = unpack_mix_float(in.opacity, drawable.opacity_t);
#endif

#ifdef HAS_UNIFORM_u_gapwidth
    let gapwidth = props.gapwidth / 2.0;
#else
    let gapwidth = unpack_mix_float(in.gapwidth, drawable.gapwidth_t) / 2.0;
#endif
#ifdef HAS_UNIFORM_u_offset
    let offset = -props.offset;
#else
    let offset = -unpack_mix_float(in.offset, drawable.offset_t);
#endif
#ifdef HAS_UNIFORM_u_width
    let width = props.width;
#else
    let width = unpack_mix_float(in.width, drawable.width_t);
#endif

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
    )),
    fragment: Some(
        r"
// The key's DEVICE_PIXEL_RATIO, which the pipeline sets.
override DEVICE_PIXEL_RATIO: f32 = 1.0;

struct FragmentInput {
    @location(0) v_width2: vec2<f32>,
    @location(1) v_normal: vec2<f32>,
    @location(2) v_gamma_scale: f32,
#ifndef HAS_UNIFORM_u_color
    @location(3) v_color: vec4<f32>,
#endif
#ifndef HAS_UNIFORM_u_blur
    @location(4) v_blur: f32,
#endif
#ifndef HAS_UNIFORM_u_opacity
    @location(5) v_opacity: f32,
#endif
#ifdef PROJECTION_GLOBE
    @location(6) v_tile_x: f32,
#endif
};

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
#ifdef HAS_UNIFORM_u_color
    let color = props.color;
#else
    let color = in.v_color;
#endif
#ifdef HAS_UNIFORM_u_blur
    let blur = props.blur;
#else
    let blur = in.v_blur;
#endif
#ifdef HAS_UNIFORM_u_opacity
    let opacity = props.opacity;
#else
    let opacity = in.v_opacity;
#endif

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

/// The start of every program's stages: the engine's helpers, which any
/// program can use.
pub(crate) const PRELUDE: WgslShaderSource = WgslShaderSource {
    vertex: None,
    fragment: None,
    prelude: Some(
        r"
// Constants
const PI: f32 = 3.14159265358979323846;

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

// Pole vertices carry these sentinel Y values in their raw position.
const GLOBE_POLE_NORTH_Y: f32 = -32767.5;
const GLOBE_POLE_SOUTH_Y: f32 = 32766.5;

#ifdef PROJECTION_GLOBE

const GLOBE_RADIUS: f32 = 6371008.8;
// The tile's anchor, its middle, in tile units.
const GLOBE_ANCHOR: f32 = 4096.0;

// On the globe, points on the sphere are offsets from the tile's anchor, which the projection matrix carries, so float32
// keeps them exact close in. The small-angle functions below are exact to float32 where the GPU's builtins may only be
// close in absolute terms.

// e^x - 1.
fn globeExpm1(x: f32) -> f32 {
    if (abs(x) < 0.01) {
        return x * (1.0 + x * (0.5 + x * (1.0 / 6.0 + x * (1.0 / 24.0))));
    }
    return exp(x) - 1.0;
}

fn globeAtan(x: f32) -> f32 {
    let x2 = x * x;
    if (abs(x) < 0.05) {
        return x * (1.0 - x2 * (1.0 / 3.0 - x2 * (0.2 - x2 / 7.0)));
    }
    return atan(x);
}

fn globeTan(a: f32) -> f32 {
    let a2 = a * a;
    if (abs(a) < 0.1) {
        return a * (1.0 + a2 * (1.0 / 3.0 + a2 * (2.0 / 15.0)));
    }
    return tan(a);
}

// The change in the sine and cosine of an angle whose sine and cosine are `sc` when it turns by `a`.
fn globeTurn(sc: vec2<f32>, a: f32) -> vec2<f32> {
    let a2 = a * a;
    var sin_a = a * (1.0 - a2 / 6.0 * (1.0 - a2 / 20.0));
    // 1 - cos(a)
    var versin_a = a2 * 0.5 * (1.0 - a2 / 12.0 * (1.0 - a2 / 30.0));
    if (abs(a) >= 0.1) {
        let half_sin = sin(a * 0.5);
        sin_a = sin(a);
        versin_a = 2.0 * half_sin * half_sin;
    }
    return vec2<f32>(sc.y * sin_a - sc.x * versin_a, -sc.x * sin_a - sc.y * versin_a);
}

// Tile position (0..EXTENT) to a point on the unit sphere, as its offset from the tile's anchor. The offset is built from
// the position's distance from the anchor alone, so float32 rounds it to its own size, a tile's, not the sphere's. The
// pole sentinels in rawPos map to the poles.
fn projectToSphere(translatedPos: vec2<f32>, rawPos: vec2<f32>, projection: ProjectionUBO) -> vec3<f32> {
    let d = translatedPos - GLOBE_ANCHOR;
    let anchor = projection.anchor_angles;
    let d_lng = d.x * projection.tile_mercator_coords.z * (2.0 * PI);
    // The latitude is 2 atan(e^m) - pi/2 of the Mercator exponent m = pi - 2 pi y; its change from the anchor's, as one
    // atan of e^(m - m_anchor) - 1.
    let e = globeExpm1(d.y * projection.tile_mercator_coords.w * (-2.0 * PI));
    let d_lat = 2.0 * globeAtan(anchor.w * e / (2.0 + (1.0 + anchor.z) * e));
    let lng = globeTurn(anchor.xy, d_lng);
    let lat = globeTurn(anchor.zw, d_lat);
    let cos_lat = anchor.w + lat.y;
    var offset = vec3<f32>(lng.x * cos_lat + anchor.x * lat.y, lat.x, lng.y * cos_lat + anchor.y * lat.y);
    if (rawPos.y < GLOBE_POLE_NORTH_Y) {
        offset = vec3<f32>(0.0, 1.0, 0.0) - projection.anchor.xyz;
    }
    if (rawPos.y > GLOBE_POLE_SOUTH_Y) {
        offset = vec3<f32>(0.0, -1.0, 0.0) - projection.anchor.xyz;
    }
    return offset;
}

// The point `angles` (radians right and up) from a point on the unit sphere, both as offsets from the tile's anchor.
fn globeRotateVector(offset: vec3<f32>, angles: vec2<f32>, projection: ProjectionUBO) -> vec3<f32> {
    let vec = projection.anchor.xyz + offset;
    var axisRight = vec3<f32>(vec.z, 0.0, -vec.x);
    var axisUp = cross(axisRight, vec);
    axisRight = normalize(axisRight);
    axisUp = normalize(axisUp);
    let v = axisRight * globeTan(angles.x) + axisUp * globeTan(angles.y);
    // normalize(vec + v) - vec for a unit vec, without the cancellation.
    let v2 = dot(v, v);
    let len = sqrt(1.0 + v2);
    return offset + (v - vec * (v2 / (len + 1.0))) / len;
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

// The clipping plane's value at a point offset from the tile's anchor, off 1.
fn globeComputeClippingZ(offset: vec3<f32>, projection: ProjectionUBO) -> f32 {
    return 1.0 - (dot(offset, projection.clipping_plane.xyz) + projection.anchor.w);
}

// A point on the unit sphere, offset from the tile's anchor, raised by `elevation` meters.
fn globeElevate(offset: vec3<f32>, elevation: f32, projection: ProjectionUBO) -> vec3<f32> {
    return offset + (projection.anchor.xyz + offset) * (elevation / GLOBE_RADIUS);
}

// sphereOffset is projectToSphere's.
fn interpolateProjection(posInTile: vec2<f32>, sphereOffset: vec3<f32>, elevation: f32, projection: ProjectionUBO) -> vec4<f32> {
    let elevatedOffset = globeElevate(sphereOffset, elevation, projection);
    var globePosition = projection.matrix * vec4<f32>(elevatedOffset, 1.0);
    // Clip the far side of the globe through Z; the layer's depth shift keeps layer order.
    globePosition.z = globeComputeClippingZ(elevatedOffset, projection) * globePosition.w - projection.depth_offset;

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
fn interpolateProjectionFor3D(posInTile: vec2<f32>, sphereOffset: vec3<f32>, elevation: f32, projection: ProjectionUBO) -> vec4<f32> {
    let globePosition = projection.matrix * vec4<f32>(globeElevate(sphereOffset, elevation, projection), 1.0);
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

pub(crate) const RASTER_SHADER: WgslShaderSource = WgslShaderSource {
    vertex: Some(
        r"
struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) pos0: vec3<f32>,
    @location(1) pos1: vec3<f32>,
};

@vertex
fn main(in: VertexInput) -> VertexOutput {
    var out: VertexOutput;
    let drawable = drawableVector[globalIndex.value];
    // The vertices span the unit square, which the warp puts on the quad
    // of the corners: blended from the projective warp towards the
    // bilinear one, whose homogeneous denominator is one at the top left.
    let fractional = vec2<f32>(f32(in.pos.x), f32(in.pos.y)) / 8192.0;
    let top_left = props.coords_top.xy;
    let top_right = props.coords_top.zw;
    let bottom_left = props.coords_bottom.xy;
    let bottom_right = props.coords_bottom.zw;
    let bilinear = mix(mix(top_left, top_right, fractional.x), mix(bottom_left, bottom_right, fractional.x), fractional.y);
    let denominator = dot(props.image_warp.xy, fractional) + 1.0;
    let across_top = top_right - top_left + props.image_warp.x * top_right;
    let down_left = bottom_left - top_left + props.image_warp.y * bottom_left;
    let projective = (across_top * fractional.x + down_left * fractional.y + top_left) / denominator;
    let pos = mix(projective, bilinear, props.image_warp.z);
    let clip = projectTileWithPoles(pos, pos, projectionVector[globalIndex.value]);
    out.position = clip;

    // The texture coordinates weighted by the reciprocal of the
    // denominator, blended as the position is, interpolate projectively.
    let weight = mix(1.0 / denominator, 1.0, props.image_warp.z);
    let tex = vec2<f32>(f32(in.texture_pos.x), f32(in.texture_pos.y));
    let pos0 = (((tex / 8192.0) - vec2<f32>(0.5, 0.5)) / props.buffer_scale) + vec2<f32>(0.5, 0.5);
    out.pos0 = vec3<f32>(pos0 * weight, weight);
    out.pos1 = vec3<f32>((pos0 * props.scale_parent + props.tl_parent) * weight, weight);

    return out;
}
",
    ),
    fragment: Some(
        r"
struct FragmentInput {
    @location(0) pos0: vec3<f32>,
    @location(1) pos1: vec3<f32>,
};

@fragment
fn main(in: FragmentInput) -> @location(0) vec4<f32> {
#ifdef OVERDRAW_INSPECTOR
    return vec4<f32>(1.0, 1.0, 1.0, 1.0);
#endif

    var color0 = textureSample(image0, image0_sampler, in.pos0.xy / in.pos0.z);
    var color1 = textureSample(image1, image1_sampler, in.pos1.xy / in.pos1.z);

    if (color0.a > 0.0) {
        color0 = vec4<f32>(color0.rgb / color0.a, color0.a);
    }
    if (color1.a > 0.0) {
        color1 = vec4<f32>(color1.rgb / color1.a, color1.a);
    }

    var color = mix(color0, color1, props.fade_t);
    color.a = color.a * props.opacity;
    var rgb = color.rgb;

    let spin = props.spin_weights;
    rgb = vec3<f32>(dot(rgb, spin.xyz),
                    dot(rgb, spin.zxy),
                    dot(rgb, spin.yzx));

    let average = (color.r + color.g + color.b) / 3.0;
    rgb = rgb + (average - rgb) * props.saturation_factor;
    rgb = (rgb - vec3<f32>(0.5, 0.5, 0.5)) * props.contrast_factor + vec3<f32>(0.5, 0.5, 0.5);

    let high_vec = vec3<f32>(props.brightness_low, props.brightness_low, props.brightness_low);
    let low_vec = vec3<f32>(props.brightness_high, props.brightness_high, props.brightness_high);

    let final_rgb = mix(high_vec, low_vec, rgb) * color.a;
    return vec4<f32>(final_rgb, color.a);
}
",
    ),
    prelude: None,
};

pub(crate) const SKY_SHADER: WgslShaderSource = WgslShaderSource {
    vertex: Some(
        r"
struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) pos: vec2<f32>,
};

@vertex
fn main(in: VertexInput) -> VertexOutput {
    let pos = vec2<f32>(f32(in.pos.x), f32(in.pos.y));
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

/// `unpack_opacity`, a symbol's fade opacity from its packed attribute.
macro_rules! unpack_opacity {
    () => {
        r"
fn unpack_opacity(packedOpacity: f32) -> vec2<f32> {
    let intOpacity = i32(packedOpacity) / 2;
    return vec2<f32>(f32(intOpacity) / 127.0, glMod(packedOpacity, 2.0));
}
"
    };
}

pub(crate) const SYMBOL_ICON_SHADER: WgslShaderSource = WgslShaderSource {
    vertex: Some(concat!(
        unpack_opacity!(),
        ground_visibility!(),
        r"
struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) tex: vec2<f32>,
    @location(1) fade_opacity: f32,
#ifndef HAS_UNIFORM_u_opacity
    @location(2) opacity: f32,
#endif
    @location(3) @interpolate(flat) tex_origin: vec2<f32>,
};

@vertex
fn main(in: VertexInput) -> VertexOutput {
    var out: VertexOutput;

    let drawable = drawableVector[globalIndex.value];

    // How much of the symbol the ground in front of its anchor leaves.
#ifdef TERRAIN
    let ground_anchor = vec2<f32>(f32(in.pos_offset.x), f32(in.pos_offset.y)) + drawable.translation;
    let ground_clip = projectTileWithElevation(ground_anchor, in.height_offset, projectionVector[globalIndex.value]);
    let ground_visible = ground_visibility(ground_clip, paintParams.world_size.y / paintParams.pixel_ratio);
#else
    let ground_visible = 1.0;
#endif

    let raw_fade_opacity = unpack_opacity(in.fade_opacity);
    let fade_change = select(-paintParams.symbol_fade_change, paintParams.symbol_fade_change, raw_fade_opacity.y > 0.5);
    let fade_opacity = max(0.0, min(ground_visible, raw_fade_opacity.x + fade_change));

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

    let size_zoom_constant = drawable.is_size_zoom_constant != 0;
    let size_feature_constant = drawable.is_size_feature_constant != 0;
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
    if (drawable.pitch_with_map == 0 && in.height_offset <= 0.0 && projectedPoint.z > projectedPoint.w) {
        out.position = vec4<f32>(c_offscreen_degenerate_triangle_location,
                                 c_offscreen_degenerate_triangle_location,
                                 c_offscreen_degenerate_triangle_location, 1.0);
        return out;
    }
#endif
    let camera_to_anchor_distance = projectedPoint.w;
    let pitch_with_map = drawable.pitch_with_map != 0;
    let distance_ratio = select(
        paintParams.camera_to_center_distance / camera_to_anchor_distance,
        camera_to_anchor_distance / paintParams.camera_to_center_distance,
        pitch_with_map
    );
    let perspective_ratio = clamp(0.5 + 0.5 * distance_ratio, 0.0, 4.0);

    if (drawable.is_offset == 0) {
        size *= perspective_ratio;
    }

    let is_text = drawable.is_text_prop != 0;
    let fontScale = select(size, size / 24.0, is_text);

    var symbol_rotation = 0.0;
    if (drawable.rotate_symbol != 0) {
        let offsetProjectedPoint = projectTileWithElevation(translated_a_pos + vec2<f32>(1.0, 0.0), in.height_offset, projection);
        let a = projectedPoint.xy / projectedPoint.w;
        let b = offsetProjectedPoint.xy / offsetProjectedPoint.w;
        symbol_rotation = atan2((b.y - a.y) / paintParams.aspect_ratio, b.x - a.x);
    }

    let angle_sin = sin(segment_angle + symbol_rotation);
    let angle_cos = cos(segment_angle + symbol_rotation);
    let rotation_matrix = mat2x2<f32>(angle_cos, -angle_sin, angle_sin, angle_cos);

    var projected_pos: vec4<f32>;
    if (drawable.is_along_line != 0 || drawable.is_variable_anchor != 0) {
        // A glyph along a line stands at its own height, which the label's
        // layout gives it; a label at a variable anchor at its anchor's.
        let height = select(in.height_offset, in.projected_pos.w, drawable.is_along_line != 0);
        projected_pos = vec4<f32>(in.projected_pos.xy, height, 1.0);
    } else if (drawable.pitch_with_map != 0) {
        projected_pos = drawable.label_plane_matrix * vec4<f32>(in.projected_pos.xy + drawable.translation, in.height_offset, 1.0);
    } else {
        projected_pos = drawable.label_plane_matrix * projectTileWithElevation(in.projected_pos.xy + drawable.translation, in.height_offset, projection);
    }
    let z = select(0.0, projected_pos.z / projected_pos.w, drawable.pitch_with_map != 0);

    var projectionScaling = 1.0;
#ifdef PROJECTION_GLOBE
    if (drawable.pitch_with_map != 0) {
        let anchor_pos_tile_y = (drawable.coord_matrix * vec4<f32>(projected_pos.xy / projected_pos.w, z, 1.0)).y;
        projectionScaling = mix(projectionScaling, 1.0 / circumferenceRatioAtTileY(anchor_pos_tile_y, projection) * drawable.pitched_scale, projection.projection_transition);
    }
#endif
    let posOffset = a_offset * max(a_minFontScale, vec2<f32>(fontScale)) / 32.0 + a_pxoffset / 16.0;
    var position = drawable.coord_matrix * vec4<f32>(projected_pos.xy / projected_pos.w + rotation_matrix * posOffset * projectionScaling, z, 1.0);
    if (drawable.pitch_with_map != 0) {
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
    )),
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

@fragment
fn main(in: FragmentInput) -> @location(0) vec4<f32> {
#ifdef OVERDRAW_INSPECTOR
    return vec4<f32>(1.0, 1.0, 1.0, 1.0);
#endif

    let tileProps = tilePropsVector[globalIndex.value];
#ifdef HAS_UNIFORM_u_opacity
    let base_opacity = select(props.icon_opacity, props.text_opacity, tileProps.is_text != 0);
    let opacity = base_opacity * in.fade_opacity;
#else
    // The vertex stage multiplies the fade opacity in.
    let opacity = in.opacity;
#endif
    return textureSample(texture, texture_sampler, atlas_pos(in.tex_origin, in.tex)) * opacity;
}
",
    ),
    prelude: Some(
        r"
const c_offscreen_degenerate_triangle_location: f32 = -2.0;
",
    ),
};

pub(crate) const SYMBOL_SDF_SHADER: WgslShaderSource = WgslShaderSource {
    vertex: Some(concat!(
        unpack_opacity!(),
        ground_visibility!(),
        r"
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

@vertex
fn main(in: VertexInput) -> VertexOutput {
    var out: VertexOutput;

    let drawable = drawableVector[globalIndex.value];

    // How much of the symbol the ground in front of its anchor leaves.
#ifdef TERRAIN
    let ground_anchor = vec2<f32>(f32(in.pos_offset.x), f32(in.pos_offset.y)) + drawable.translation;
    let ground_clip = projectTileWithElevation(ground_anchor, in.height_offset, projectionVector[globalIndex.value]);
    let ground_visible = ground_visibility(ground_clip, paintParams.world_size.y / paintParams.pixel_ratio);
#else
    let ground_visible = 1.0;
#endif

    let fade_opacity = unpack_opacity(in.fade_opacity);
    let fade_change = select(-paintParams.symbol_fade_change, paintParams.symbol_fade_change, fade_opacity.y > 0.5);
    let fo = max(0.0, min(ground_visible, fade_opacity.x + fade_change));

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
    let size_zoom_constant = drawable.is_size_zoom_constant != 0;
    let size_feature_constant = drawable.is_size_feature_constant != 0;
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
    if (drawable.pitch_with_map == 0 && in.height_offset <= 0.0 && projectedPoint.z > projectedPoint.w) {
        out.position = vec4<f32>(c_offscreen_degenerate_triangle_location,
                                 c_offscreen_degenerate_triangle_location,
                                 c_offscreen_degenerate_triangle_location, 1.0);
        return out;
    }
#endif
    let camera_to_anchor_distance = projectedPoint.w;
    let pitch_with_map = drawable.pitch_with_map != 0;
    let distance_ratio = select(
        paintParams.camera_to_center_distance / camera_to_anchor_distance,
        camera_to_anchor_distance / paintParams.camera_to_center_distance,
        pitch_with_map
    );
    let perspective_ratio = clamp(0.5 + 0.5 * distance_ratio, 0.0, 4.0);

    if (drawable.is_offset == 0) {
        size *= perspective_ratio;
    }

    let is_text = drawable.is_text_prop != 0;
    let fontScale = select(size, size / 24.0, is_text);

    var symbol_rotation = 0.0;
    if (drawable.rotate_symbol != 0) {
        let offsetProjectedPoint = projectTileWithElevation(translated_a_pos + vec2<f32>(1.0, 0.0), in.height_offset, projection);
        let a = projectedPoint.xy / projectedPoint.w;
        let b = offsetProjectedPoint.xy / offsetProjectedPoint.w;
        symbol_rotation = atan2((b.y - a.y) / paintParams.aspect_ratio, b.x - a.x);
    }

    let angle_sin = sin(segment_angle + symbol_rotation);
    let angle_cos = cos(segment_angle + symbol_rotation);
    let rotation_matrix = mat2x2<f32>(angle_cos, -angle_sin, angle_sin, angle_cos);

    var projected_pos: vec4<f32>;
    if (drawable.is_along_line != 0 || drawable.is_variable_anchor != 0) {
        // A glyph along a line stands at its own height, which the label's
        // layout gives it; a label at a variable anchor at its anchor's.
        let height = select(in.height_offset, in.projected_pos.w, drawable.is_along_line != 0);
        projected_pos = vec4<f32>(in.projected_pos.xy, height, 1.0);
    } else if (drawable.pitch_with_map != 0) {
        projected_pos = drawable.label_plane_matrix * vec4<f32>(in.projected_pos.xy + drawable.translation, in.height_offset, 1.0);
    } else {
        projected_pos = drawable.label_plane_matrix * projectTileWithElevation(in.projected_pos.xy + drawable.translation, in.height_offset, projection);
    }
    let z = select(0.0, projected_pos.z / projected_pos.w, drawable.pitch_with_map != 0);

    var projectionScaling = 1.0;
#ifdef PROJECTION_GLOBE
    if (drawable.pitch_with_map != 0) {
        let anchor_pos_tile_y = (drawable.coord_matrix * vec4<f32>(projected_pos.xy / projected_pos.w, z, 1.0)).y;
        projectionScaling = mix(projectionScaling, 1.0 / circumferenceRatioAtTileY(anchor_pos_tile_y, projection) * drawable.pitched_scale, projection.projection_transition);
    }
#endif
    let pos_rot = a_offset / 32.0 * fontScale + a_pxoffset;
    var position = drawable.coord_matrix * vec4<f32>(projected_pos.xy / projected_pos.w + rotation_matrix * pos_rot * projectionScaling, z, 1.0);
    if (drawable.pitch_with_map != 0) {
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
    let halo_width = select(props.icon_halo_width, props.text_halo_width, drawable.is_text_prop != 0);
#endif
#ifndef HAS_UNIFORM_u_halo_blur
    let halo_blur = unpack_mix_float(in.halo_blur, drawable.halo_blur_t);
#else
    let halo_blur = select(props.icon_halo_blur, props.text_halo_blur, drawable.is_text_prop != 0);
#endif
    let EDGE_GAMMA = 0.105 / paintParams.pixel_ratio;
    let halo_blur_gamma = select(0.0, halo_blur * 1.19 / SDF_PX, drawable.is_halo != 0);
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
    )),
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

@fragment
fn main(in: FragmentInput) -> @location(0) vec4<f32> {
#ifdef OVERDRAW_INSPECTOR
    return vec4<f32>(1.0, 1.0, 1.0, 1.0);
#endif

    let tileProps = tilePropsVector[globalIndex.value];
#ifndef HAS_UNIFORM_u_fill_color
    let fill_color = in.fill_color;
#else
    let fill_color = select(props.icon_fill_color, props.text_fill_color, tileProps.is_text != 0);
#endif
#ifndef HAS_UNIFORM_u_halo_color
    let halo_color = in.halo_color;
#else
    let halo_color = select(props.icon_halo_color, props.text_halo_color, tileProps.is_text != 0);
#endif
#ifndef HAS_UNIFORM_u_opacity
    let opacity = in.fade_opacity;
#else
    let opacity = select(props.icon_opacity, props.text_opacity, tileProps.is_text != 0) * in.fade_opacity;
#endif

    let color = select(fill_color, halo_color, tileProps.is_halo != 0);
    let gamma_scaled = in.gamma_scaled;

    // For halos, offset the inner edge outward to create the ring
    var inner_edge = (256.0 - 64.0) / 256.0;
    if (tileProps.is_halo != 0) {
        inner_edge = inner_edge + gamma_scaled;
    }

    let sample = textureSample(texture, texture_sampler, atlas_pos(in.tex_origin, in.tex));
    let dist = select(sample.a, sample.r, tileProps.is_text != 0);
    var alpha = smoothstep(inner_edge - gamma_scaled, inner_edge + gamma_scaled, dist);

    // When drawing halos, make the inside transparent so the fill can show through
    if (tileProps.is_halo != 0) {
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
const c_offscreen_degenerate_triangle_location: f32 = -2.0;
const SDF_PX: f32 = 8.0;
",
    ),
};

pub(crate) const SYMBOL_TEXT_AND_ICON_SHADER: WgslShaderSource = WgslShaderSource {
    vertex: Some(concat!(
        unpack_opacity!(),
        ground_visibility!(),
        r"
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

const SDF = 1.0;
const ICON = 0.0;

@vertex
fn main(in: VertexInput) -> VertexOutput {
    var out: VertexOutput;

    let drawable = drawableVector[globalIndex.value];

    // How much of the symbol the ground in front of its anchor leaves.
#ifdef TERRAIN
    let ground_anchor = vec2<f32>(f32(in.pos_offset.x), f32(in.pos_offset.y)) + drawable.translation;
    let ground_clip = projectTileWithElevation(ground_anchor, in.height_offset, projectionVector[globalIndex.value]);
    let ground_visible = ground_visibility(ground_clip, paintParams.world_size.y / paintParams.pixel_ratio);
#else
    let ground_visible = 1.0;
#endif

    let fade_opacity = unpack_opacity(in.fade_opacity);
    let fade_change = select(-paintParams.symbol_fade_change, paintParams.symbol_fade_change, fade_opacity.y > 0.5);
    let fo = max(0.0, min(ground_visible, fade_opacity.x + fade_change));

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

    let size_zoom_constant = drawable.is_size_zoom_constant != 0;
    let size_feature_constant = drawable.is_size_feature_constant != 0;
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
    if (drawable.pitch_with_map == 0 && in.height_offset <= 0.0 && projectedPoint.z > projectedPoint.w) {
        out.position = vec4<f32>(c_offscreen_degenerate_triangle_location,
                                 c_offscreen_degenerate_triangle_location,
                                 c_offscreen_degenerate_triangle_location, 1.0);
        return out;
    }
#endif
    let camera_to_anchor_distance = projectedPoint.w;
    let pitch_with_map = drawable.pitch_with_map != 0;
    let distance_ratio = select(
        paintParams.camera_to_center_distance / camera_to_anchor_distance,
        camera_to_anchor_distance / paintParams.camera_to_center_distance,
        pitch_with_map
    );
    let perspective_ratio = clamp(0.5 + 0.5 * distance_ratio, 0.0, 4.0);

    if (drawable.is_offset == 0) {
        size *= perspective_ratio;
    }

    let fontScale = size / 24.0;

    var symbol_rotation = 0.0;
    if (drawable.rotate_symbol != 0) {
        let offsetProjectedPoint = projectTileWithElevation(translated_a_pos + vec2<f32>(1.0, 0.0), in.height_offset, projection);
        let a = projectedPoint.xy / projectedPoint.w;
        let b = offsetProjectedPoint.xy / offsetProjectedPoint.w;
        symbol_rotation = atan2((b.y - a.y) / paintParams.aspect_ratio, b.x - a.x);
    }

    let angle_sin = sin(segment_angle + symbol_rotation);
    let angle_cos = cos(segment_angle + symbol_rotation);
    let rotation_matrix = mat2x2<f32>(angle_cos, -angle_sin, angle_sin, angle_cos);

    var projected_pos: vec4<f32>;
    if (drawable.is_along_line != 0 || drawable.is_variable_anchor != 0) {
        // A glyph along a line stands at its own height, which the label's
        // layout gives it; a label at a variable anchor at its anchor's.
        let height = select(in.height_offset, in.projected_pos.w, drawable.is_along_line != 0);
        projected_pos = vec4<f32>(in.projected_pos.xy, height, 1.0);
    } else if (drawable.pitch_with_map != 0) {
        projected_pos = drawable.label_plane_matrix * vec4<f32>(in.projected_pos.xy + drawable.translation, in.height_offset, 1.0);
    } else {
        projected_pos = drawable.label_plane_matrix * projectTileWithElevation(in.projected_pos.xy + drawable.translation, in.height_offset, projection);
    }
    let z = select(0.0, projected_pos.z / projected_pos.w, drawable.pitch_with_map != 0);

    var projectionScaling = 1.0;
#ifdef PROJECTION_GLOBE
    if (drawable.pitch_with_map != 0 && drawable.is_along_line == 0) {
        let anchor_pos_tile_y = (drawable.coord_matrix * vec4<f32>(projected_pos.xy / projected_pos.w, z, 1.0)).y;
        projectionScaling = mix(projectionScaling, 1.0 / circumferenceRatioAtTileY(anchor_pos_tile_y, projection) * drawable.pitched_scale, projection.projection_transition);
    }
#endif
    let pos_rot = a_offset / 32.0 * fontScale;
    var position = drawable.coord_matrix * vec4<f32>(projected_pos.xy / projected_pos.w + rotation_matrix * pos_rot * projectionScaling, z, 1.0);
    if (drawable.pitch_with_map != 0) {
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
    let halo_width = select(props.icon_halo_width, props.text_halo_width, drawable.is_text_prop != 0);
#endif
#ifndef HAS_UNIFORM_u_halo_blur
    let halo_blur = unpack_mix_float(in.halo_blur, drawable.halo_blur_t);
#else
    let halo_blur = select(props.icon_halo_blur, props.text_halo_blur, drawable.is_text_prop != 0);
#endif
    let EDGE_GAMMA = 0.105 / paintParams.pixel_ratio;
    let halo_blur_gamma = select(0.0, halo_blur * 1.19 / SDF_PX, drawable.is_halo != 0);
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
    )),
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

@fragment
fn main(in: FragmentInput) -> @location(0) vec4<f32> {
#ifdef OVERDRAW_INSPECTOR
    return vec4<f32>(1.0, 1.0, 1.0, 1.0);
#endif

    let tileProps = tilePropsVector[globalIndex.value];
#ifndef HAS_UNIFORM_u_fill_color
    let fill_color = in.fill_color;
#else
    let fill_color = select(props.icon_fill_color, props.text_fill_color, tileProps.is_text != 0);
#endif
#ifndef HAS_UNIFORM_u_halo_color
    let halo_color = in.halo_color;
#else
    let halo_color = select(props.icon_halo_color, props.text_halo_color, tileProps.is_text != 0);
#endif
#ifndef HAS_UNIFORM_u_opacity
    let opacity = in.fade_opacity;
#else
    let opacity = select(props.icon_opacity, props.text_opacity, tileProps.is_text != 0) * in.fade_opacity;
#endif

    if (in.is_icon != 0.0) {
        let alpha = opacity;
        let iconSample = textureSampleLevel(texture_icon, texture_icon_sampler, atlas_pos(in.tex_origin, in.tex), 0.0);
        return iconSample * alpha;
    }

    let color = select(fill_color, halo_color, tileProps.is_halo != 0);
    let gamma_scaled = in.gamma_scaled;

    // For halos, offset the inner edge outward to create the ring
    var inner_edge = (256.0 - 64.0) / 256.0;
    if (tileProps.is_halo != 0) {
        inner_edge = inner_edge + gamma_scaled;
    }

    let sample = textureSampleLevel(texture, texture_sampler, atlas_pos(in.tex_origin, in.tex), 0.0);
    let dist = select(sample.a, sample.r, tileProps.is_text != 0);
    var alpha = smoothstep(inner_edge - gamma_scaled, inner_edge + gamma_scaled, dist);

    // When drawing halos, make the inside transparent so the fill can show through
    if (tileProps.is_halo != 0) {
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
const c_offscreen_degenerate_triangle_location: f32 = -2.0;
const SDF_PX: f32 = 8.0;
",
    ),
};
