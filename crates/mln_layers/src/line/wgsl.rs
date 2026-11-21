//! The programs of the `line` layer type in WGSL, for `mln_webgpu`.

use mln_gfx::shaders::source::wgsl::Wgsl;

use crate::shaders::atlas_pos;

/// What every stage of the type's programs reads: the scale of the
/// extrusion vectors, the width of a line on the globe, and the clipping of
/// the zoom 0 tile's lines at the antimeridian.
const LINE_PRELUDE: &str = concat!(
    atlas_pos!(wgsl),
    r"
const LINE_NORMAL_SCALE: f32 = 1.0 / 63.0;  // 1.0 / (127 / 2)

#ifdef PROJECTION_GLOBE

fn projectLineThickness(tileY: f32, projection: ProjectionUBO) -> f32 {
    let thickness = 1.0 / circumferenceRatioAtTileY(tileY, projection);
    if (projection.transition < 0.999) {
        return mix(1.0, thickness, projection.transition);
    }
    return thickness;
}

// The zoom 0 tile's buffer wraps around the planet onto the tile itself, so the line shaders discard the fragments
// beyond its X extent; every other tile (a mercator width below the whole world's) hands on a value inside it.
fn antimeridianClipX(posInTile: vec2<f32>, projection: ProjectionUBO) -> f32 {
    return select(0.0, posInTile.x, projection.tile_mercator_coords.z * 8192.0 >= 1.0);
}

fn clippedAtAntimeridian(tileX: f32) -> bool {
    return tileX < 0.0 || tileX >= 8192.0;
}

#else

fn projectLineThickness(tileY: f32, projection: ProjectionUBO) -> f32 {
    return 1.0;
}

#endif
"
);

pub(super) const LINE_GRADIENT_SDF_SHADER: Wgsl = Wgsl {
    vertex: r"
// The key's DEVICE_PIXEL_RATIO, which the pipeline sets.
override DEVICE_PIXEL_RATIO: f32 = 1.0;

struct VertexInput {
    @location(0) pos_normal: vec2<i32>,
    @location(1) data: vec4<u32>,
    #property attribute blur
    #property attribute opacity
    #property attribute gapwidth
    #property attribute offset
    #property attribute width
    #property attribute floorwidth
    #property attribute dasharray
};

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) v_width2: vec2<f32>,
    @location(1) v_normal: vec2<f32>,
    @location(2) v_gamma_scale: f32,
    @location(3) v_tex_a: vec2<f32>,
    @location(4) v_tex_b: vec2<f32>,
    #property varying blur 5
    #property varying opacity 6
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
    let index = drawIndex;
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
    #property pass blur
    #property pass opacity
    let gapwidth = $props.gapwidth / 2.0;
    let offset = -$props.offset;
    let width = $props.width;
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
    let adjustedThickness = projectLineThickness(pos.y, projectionVector[drawIndex]);
    let projected_no_extrude = projectTile(pos + offset2 / drawable.ratio * adjustedThickness, projectionVector[drawIndex]);
    let extrudedPos = pos + (offset2 + dist) / drawable.ratio * adjustedThickness;
    let position_globe = projectTile(extrudedPos, projectionVector[drawIndex]);
    out.v_tile_x = antimeridianClipX(extrudedPos, projectionVector[drawIndex]);
    let projected_extrude = position_globe - projected_no_extrude;
    let position = position_globe;
#else
    let projected_extrude = drawable.matrix * vec4<f32>(dist / drawable.ratio, 0.0, 0.0);
    let position = drawable.matrix * vec4<f32>(pos + offset2 / drawable.ratio, 0.0, 1.0) + projected_extrude;
#endif

    // Calculate gamma scale
    let extrude_length_without_perspective = length(dist);
    let extrude_length_with_perspective = length(projected_extrude.xy / position.w * view.units_to_pixels);

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
    fragment: r"
// The key's DEVICE_PIXEL_RATIO, which the pipeline sets.
override DEVICE_PIXEL_RATIO: f32 = 1.0;

struct FragmentInput {
    @location(0) v_width2: vec2<f32>,
    @location(1) v_normal: vec2<f32>,
    @location(2) v_gamma_scale: f32,
    @location(3) v_tex_a: vec2<f32>,
    @location(4) v_tex_b: vec2<f32>,
    #property varying blur 5
    #property varying opacity 6
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
    let tileProps = tilePropsVector[drawIndex];
    let blur = $props.blur;
    let opacity = $props.opacity;
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
    prelude: LINE_PRELUDE,
};

pub(super) const LINE_GRADIENT_SHADER: Wgsl = Wgsl {
    vertex: r"
// The key's DEVICE_PIXEL_RATIO, which the pipeline sets.
override DEVICE_PIXEL_RATIO: f32 = 1.0;

struct VertexInput {
    @location(0) pos_normal: vec2<i32>,
    @location(1) data: vec4<u32>,
    #property attribute blur
    #property attribute opacity
    #property attribute gapwidth
    #property attribute offset
    #property attribute width
};

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) v_width2: vec2<f32>,
    @location(1) v_normal: vec2<f32>,
    @location(2) v_gamma_scale: f32,
    @location(3) v_lineprogress: f32,
    #property varying blur 4
    #property varying opacity 5
#ifdef PROJECTION_GLOBE
    @location(6) v_tile_x: f32,
#endif
};

@vertex
fn main(in: VertexInput) -> VertexOutput {
    var out: VertexOutput;
    let drawable = drawableVector[drawIndex];

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
    #property pass blur
    #property pass opacity
    let gapwidth = $props.gapwidth / 2.0;
    let offset = -$props.offset;
    let width = $props.width;

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
    let adjustedThickness = projectLineThickness(pos.y, projectionVector[drawIndex]);
    let projected_no_extrude = projectTile(pos + offset2 / drawable.ratio * adjustedThickness, projectionVector[drawIndex]);
    let extrudedPos = pos + (offset2 + dist) / drawable.ratio * adjustedThickness;
    let position_globe = projectTile(extrudedPos, projectionVector[drawIndex]);
    out.v_tile_x = antimeridianClipX(extrudedPos, projectionVector[drawIndex]);
    let projected_extrude = position_globe - projected_no_extrude;
    let position = position_globe;
#else
    let projected_extrude = drawable.matrix * vec4<f32>(dist / drawable.ratio, 0.0, 0.0);
    let position = drawable.matrix * vec4<f32>(pos + offset2 / drawable.ratio, 0.0, 1.0) + projected_extrude;
#endif

    // Calculate gamma scale
    let extrude_length_without_perspective = length(dist);
    let extrude_length_with_perspective = length(projected_extrude.xy / position.w * view.units_to_pixels);

    out.position = position;
    out.v_width2 = vec2<f32>(outset, inset);
    out.v_normal = v_normal;
    out.v_gamma_scale = extrude_length_without_perspective / extrude_length_with_perspective;
    out.v_lineprogress = v_lineprogress;

    return out;
}
",
    fragment: r"
// The key's DEVICE_PIXEL_RATIO, which the pipeline sets.
override DEVICE_PIXEL_RATIO: f32 = 1.0;

struct FragmentInput {
    @location(0) v_width2: vec2<f32>,
    @location(1) v_normal: vec2<f32>,
    @location(2) v_gamma_scale: f32,
    @location(3) v_lineprogress: f32,
    #property varying blur 4
    #property varying opacity 5
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
    let blur = $props.blur;
    let opacity = $props.opacity;

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
    prelude: LINE_PRELUDE,
};

pub(super) const LINE_PATTERN_SHADER: Wgsl = Wgsl {
    vertex: r"
// The key's DEVICE_PIXEL_RATIO, which the pipeline sets.
override DEVICE_PIXEL_RATIO: f32 = 1.0;

struct VertexInput {
    @location(0) pos_normal: vec2<i32>,
    @location(1) data: vec4<u32>,
    #property attribute blur
    #property attribute opacity
    #property attribute gapwidth
    #property attribute offset
    #property attribute width
    #property attribute pattern_from
    #property attribute pattern_to
};

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) v_width2: vec2<f32>,
    @location(1) v_normal: vec2<f32>,
    @location(2) v_gamma_scale: f32,
    @location(3) v_linesofar: f32,
    #property varying blur 4
    #property varying opacity 5
    #property varying pattern_from 6 flat
    #property varying pattern_to 7 flat
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
    let index = drawIndex;
    let drawable = drawableVector[index];
    let tileProps = tilePropsVector[index];

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

    #property pass blur
    #property pass opacity
    let gapwidth = $props.gapwidth * 0.5;
    let offset = -$props.offset;
    // The floor width comes from the width's attribute.
#ifdef HAS_UNIFORM_u_width
    let width = props.width;
#else
    let width = unpack_mix_float(in.width, drawable.width_t);
    out.v_floorwidth = unpack_mix_float(in.width, drawable.floorwidth_t);
#endif
    #property pass pattern_from
    #property pass pattern_to

    let halfwidth = width * 0.5;
    let inset = gapwidth + select(0.0, antialiasing, gapwidth > 0.0);
    let outset = gapwidth + halfwidth * select(1.0, 2.0, gapwidth > 0.0) + select(0.0, antialiasing, halfwidth != 0.0);

    let dist = outset * a_extrude * LINE_NORMAL_SCALE;

    let u = 0.5 * a_direction;
    let t = 1.0 - abs(u);
    let offset2 = offset * a_extrude * LINE_NORMAL_SCALE * v_normal.y * mat2x2<f32>(t, -u, u, t);

#ifdef PROJECTION_GLOBE
    let adjustedThickness = projectLineThickness(pos.y, projectionVector[drawIndex]);
    let projected_no_extrude = projectTile(pos + offset2 / ratio * adjustedThickness, projectionVector[drawIndex]);
    let extrudedPos = pos + (offset2 + dist) / ratio * adjustedThickness;
    let position_globe = projectTile(extrudedPos, projectionVector[drawIndex]);
    out.v_tile_x = antimeridianClipX(extrudedPos, projectionVector[drawIndex]);
    let projected_extrude = position_globe - projected_no_extrude;
    let position = position_globe;
#else
    let projected_extrude = drawable.matrix * vec4<f32>(dist / ratio, 0.0, 0.0);
    let position = drawable.matrix * vec4<f32>(pos + offset2 / ratio, 0.0, 1.0) + projected_extrude;
#endif

    let extrude_length_without_perspective = length(dist);
    let extrude_length_with_perspective = length(projected_extrude.xy / position.w * view.units_to_pixels);
    let gamma_scale = extrude_length_without_perspective / max(extrude_length_with_perspective, 1e-6);

    out.position = position;
    out.v_width2 = vec2<f32>(outset, inset);
    out.v_normal = v_normal;
    out.v_gamma_scale = gamma_scale;
    out.v_linesofar = v_linesofar;

    return out;
}
",
    fragment: r"
// The key's DEVICE_PIXEL_RATIO, which the pipeline sets.
override DEVICE_PIXEL_RATIO: f32 = 1.0;

struct FragmentInput {
    @location(0) v_width2: vec2<f32>,
    @location(1) v_normal: vec2<f32>,
    @location(2) v_gamma_scale: f32,
    @location(3) v_linesofar: f32,
    #property varying blur 4
    #property varying opacity 5
    #property varying pattern_from 6 flat
    #property varying pattern_to 7 flat
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
    let tileProps = tilePropsVector[drawIndex];

    let pattern_from = $tileProps.pattern_from;
    let pattern_to = $tileProps.pattern_to;
    let blur = $props.blur;
    let opacity = $props.opacity;
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
    prelude: LINE_PRELUDE,
};

pub(super) const LINE_SDF_SHADER: Wgsl = Wgsl {
    vertex: r"
// The key's DEVICE_PIXEL_RATIO, which the pipeline sets.
override DEVICE_PIXEL_RATIO: f32 = 1.0;

struct VertexInput {
    @location(0) pos_normal: vec2<i32>,
    @location(1) data: vec4<u32>,
    #property attribute color
    #property attribute blur
    #property attribute opacity
    #property attribute gapwidth
    #property attribute offset
    #property attribute width
    #property attribute floorwidth
    #property attribute dasharray
};

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) v_width2: vec2<f32>,
    @location(1) v_normal: vec2<f32>,
    @location(2) v_gamma_scale: f32,
    @location(3) v_tex_a: vec2<f32>,
    @location(4) v_tex_b: vec2<f32>,
    #property varying color 5
    #property varying blur 6
    #property varying opacity 7
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
    let index = drawIndex;
    let drawable = drawableVector[index];
    let tileProps = tilePropsVector[index];

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
    #property pass color
    #property pass blur
    #property pass opacity
    let gapwidth = $props.gapwidth / 2.0;
    let offset = -$props.offset;
    let width = $props.width;
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
    let adjustedThickness = projectLineThickness(pos.y, projectionVector[drawIndex]);
    let projected_no_extrude = projectTile(pos + offset2 / drawable.ratio * adjustedThickness, projectionVector[drawIndex]);
    let extrudedPos = pos + (offset2 + dist) / drawable.ratio * adjustedThickness;
    let position_globe = projectTile(extrudedPos, projectionVector[drawIndex]);
    out.v_tile_x = antimeridianClipX(extrudedPos, projectionVector[drawIndex]);
    let projected_extrude = position_globe - projected_no_extrude;
    let position = position_globe;
#else
    let projected_extrude = drawable.matrix * vec4<f32>(dist / drawable.ratio, 0.0, 0.0);
    let position = drawable.matrix * vec4<f32>(pos + offset2 / drawable.ratio, 0.0, 1.0) + projected_extrude;
#endif

    // Calculate gamma scale
    let extrude_length_without_perspective = length(dist);
    let extrude_length_with_perspective = length(projected_extrude.xy / position.w * view.units_to_pixels);

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
    fragment: r"
// The key's DEVICE_PIXEL_RATIO, which the pipeline sets.
override DEVICE_PIXEL_RATIO: f32 = 1.0;

struct FragmentInput {
    @location(0) v_width2: vec2<f32>,
    @location(1) v_normal: vec2<f32>,
    @location(2) v_gamma_scale: f32,
    @location(3) v_tex_a: vec2<f32>,
    @location(4) v_tex_b: vec2<f32>,
    #property varying color 5
    #property varying blur 6
    #property varying opacity 7
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
    let tileProps = tilePropsVector[drawIndex];
    let color = $props.color;
    let blur = $props.blur;
    let opacity = $props.opacity;
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
    prelude: LINE_PRELUDE,
};

pub(super) const LINE_SHADER: Wgsl = Wgsl {
    vertex: r"
// The key's DEVICE_PIXEL_RATIO, which the pipeline sets.
override DEVICE_PIXEL_RATIO: f32 = 1.0;

struct VertexInput {
    @location(0) pos_normal: vec2<i32>,  // packed position and normal
    @location(1) data: vec4<u32>,        // extrude, direction, linesofar
    #property attribute color
    #property attribute blur
    #property attribute opacity
    #property attribute gapwidth
    #property attribute offset
    #property attribute width
};

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) v_width2: vec2<f32>,
    @location(1) v_normal: vec2<f32>,
    @location(2) v_gamma_scale: f32,
    #property varying color 3
    #property varying blur 4
    #property varying opacity 5
#ifdef PROJECTION_GLOBE
    @location(6) v_tile_x: f32,
#endif
};

@vertex
fn main(in: VertexInput) -> VertexOutput {
    var out: VertexOutput;
    let drawable = drawableVector[drawIndex];
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

    #property pass color
    #property pass blur
    #property pass opacity

    let gapwidth = $props.gapwidth / 2.0;
    let offset = -$props.offset;
    let width = $props.width;

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
    let adjustedThickness = projectLineThickness(pos.y, projectionVector[drawIndex]);
    let projected_no_extrude = projectTile(pos + offset2 / ratio * adjustedThickness, projectionVector[drawIndex]);
    let extrudedPos = pos + (offset2 + dist) / ratio * adjustedThickness;
    let base = projectTile(extrudedPos, projectionVector[drawIndex]);
    out.v_tile_x = antimeridianClipX(extrudedPos, projectionVector[drawIndex]);
    let projected_extrude = base - projected_no_extrude;
#else
    let projected_extrude = drawable.matrix * vec4<f32>(dist / ratio, 0.0, 0.0);
    let base = drawable.matrix * vec4<f32>(pos + offset2 / ratio, 0.0, 1.0) + projected_extrude;
#endif
    let inv_w = 1.0 / base.w;

    out.position = base;

    let extrude_length_without_perspective = length(dist);
    let extrude_length_with_perspective =
        length((projected_extrude.xy / base.w) * view.units_to_pixels);
    let gamma_denom = max(extrude_length_with_perspective, 1e-6);

    out.v_width2 = vec2<f32>(outset, inset);
    out.v_normal = normal;
    out.v_gamma_scale = extrude_length_without_perspective / gamma_denom;

    return out;
}
",
    fragment: r"
// The key's DEVICE_PIXEL_RATIO, which the pipeline sets.
override DEVICE_PIXEL_RATIO: f32 = 1.0;

struct FragmentInput {
    @location(0) v_width2: vec2<f32>,
    @location(1) v_normal: vec2<f32>,
    @location(2) v_gamma_scale: f32,
    #property varying color 3
    #property varying blur 4
    #property varying opacity 5
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
    let color = $props.color;
    let blur = $props.blur;
    let opacity = $props.opacity;

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
    prelude: LINE_PRELUDE,
};
