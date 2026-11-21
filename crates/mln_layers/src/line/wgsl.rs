//! The programs of the `line` layer type in WGSL, for `mln_webgpu`.

use mln_gfx::shaders::program::Program;
use mln_gfx::shaders::source::wgsl::{Attribute, Source, Texture, Wgsl};

use super::programs;
use super::programs::{
    ID_LINE_BLUR_VERTEX_ATTRIBUTE, ID_LINE_COLOR_VERTEX_ATTRIBUTE, ID_LINE_DASH_TEXTURE,
    ID_LINE_DASHARRAY_VERTEX_ATTRIBUTE, ID_LINE_DATA_VERTEX_ATTRIBUTE, ID_LINE_FLOOR_WIDTH_VERTEX_ATTRIBUTE,
    ID_LINE_GAP_WIDTH_VERTEX_ATTRIBUTE, ID_LINE_IMAGE_TEXTURE, ID_LINE_OFFSET_VERTEX_ATTRIBUTE,
    ID_LINE_OPACITY_VERTEX_ATTRIBUTE, ID_LINE_PATTERN_FROM_VERTEX_ATTRIBUTE, ID_LINE_PATTERN_TO_VERTEX_ATTRIBUTE,
    ID_LINE_POS_NORMAL_VERTEX_ATTRIBUTE, ID_LINE_WIDTH_VERTEX_ATTRIBUTE,
};

const LINE_ATTRIBUTES: &[Attribute] = &[
    Attribute::new(4, ID_LINE_POS_NORMAL_VERTEX_ATTRIBUTE),
    Attribute::new(5, ID_LINE_DATA_VERTEX_ATTRIBUTE),
    Attribute::new(6, ID_LINE_COLOR_VERTEX_ATTRIBUTE),
    Attribute::new(7, ID_LINE_BLUR_VERTEX_ATTRIBUTE),
    Attribute::new(8, ID_LINE_OPACITY_VERTEX_ATTRIBUTE),
    Attribute::new(9, ID_LINE_GAP_WIDTH_VERTEX_ATTRIBUTE),
    Attribute::new(10, ID_LINE_OFFSET_VERTEX_ATTRIBUTE),
    Attribute::new(11, ID_LINE_WIDTH_VERTEX_ATTRIBUTE),
];
const LINE_GRADIENT_ATTRIBUTES: &[Attribute] = &[
    Attribute::new(4, ID_LINE_POS_NORMAL_VERTEX_ATTRIBUTE),
    Attribute::new(5, ID_LINE_DATA_VERTEX_ATTRIBUTE),
    Attribute::new(6, ID_LINE_BLUR_VERTEX_ATTRIBUTE),
    Attribute::new(7, ID_LINE_OPACITY_VERTEX_ATTRIBUTE),
    Attribute::new(8, ID_LINE_GAP_WIDTH_VERTEX_ATTRIBUTE),
    Attribute::new(9, ID_LINE_OFFSET_VERTEX_ATTRIBUTE),
    Attribute::new(10, ID_LINE_WIDTH_VERTEX_ATTRIBUTE),
];
const LINE_IMAGE_TEXTURES: &[Texture] = &[Texture::new(0, ID_LINE_IMAGE_TEXTURE)];
const LINE_PATTERN_ATTRIBUTES: &[Attribute] = &[
    Attribute::new(4, ID_LINE_POS_NORMAL_VERTEX_ATTRIBUTE),
    Attribute::new(5, ID_LINE_DATA_VERTEX_ATTRIBUTE),
    Attribute::new(6, ID_LINE_BLUR_VERTEX_ATTRIBUTE),
    Attribute::new(7, ID_LINE_OPACITY_VERTEX_ATTRIBUTE),
    Attribute::new(8, ID_LINE_GAP_WIDTH_VERTEX_ATTRIBUTE),
    Attribute::new(9, ID_LINE_OFFSET_VERTEX_ATTRIBUTE),
    Attribute::new(10, ID_LINE_WIDTH_VERTEX_ATTRIBUTE),
    Attribute::new(11, ID_LINE_PATTERN_FROM_VERTEX_ATTRIBUTE),
    Attribute::new(12, ID_LINE_PATTERN_TO_VERTEX_ATTRIBUTE),
];
const LINE_SDF_ATTRIBUTES: &[Attribute] = &[
    Attribute::new(4, ID_LINE_POS_NORMAL_VERTEX_ATTRIBUTE),
    Attribute::new(5, ID_LINE_DATA_VERTEX_ATTRIBUTE),
    Attribute::new(6, ID_LINE_COLOR_VERTEX_ATTRIBUTE),
    Attribute::new(7, ID_LINE_BLUR_VERTEX_ATTRIBUTE),
    Attribute::new(8, ID_LINE_OPACITY_VERTEX_ATTRIBUTE),
    Attribute::new(9, ID_LINE_GAP_WIDTH_VERTEX_ATTRIBUTE),
    Attribute::new(10, ID_LINE_OFFSET_VERTEX_ATTRIBUTE),
    Attribute::new(11, ID_LINE_WIDTH_VERTEX_ATTRIBUTE),
    Attribute::new(12, ID_LINE_FLOOR_WIDTH_VERTEX_ATTRIBUTE),
    Attribute::new(13, ID_LINE_DASHARRAY_VERTEX_ATTRIBUTE),
];
const LINE_GRADIENT_SDF_ATTRIBUTES: &[Attribute] = &[
    Attribute::new(4, ID_LINE_POS_NORMAL_VERTEX_ATTRIBUTE),
    Attribute::new(5, ID_LINE_DATA_VERTEX_ATTRIBUTE),
    Attribute::new(6, ID_LINE_BLUR_VERTEX_ATTRIBUTE),
    Attribute::new(7, ID_LINE_OPACITY_VERTEX_ATTRIBUTE),
    Attribute::new(8, ID_LINE_GAP_WIDTH_VERTEX_ATTRIBUTE),
    Attribute::new(9, ID_LINE_OFFSET_VERTEX_ATTRIBUTE),
    Attribute::new(10, ID_LINE_WIDTH_VERTEX_ATTRIBUTE),
    Attribute::new(11, ID_LINE_FLOOR_WIDTH_VERTEX_ATTRIBUTE),
    Attribute::new(12, ID_LINE_DASHARRAY_VERTEX_ATTRIBUTE),
];
// A texture at location L binds its sampler at L and the texture at L + 1,
// so the dash atlas sits at 2.
const LINE_GRADIENT_SDF_TEXTURES: &[Texture] =
    &[Texture::new(0, ID_LINE_IMAGE_TEXTURE), Texture::new(2, ID_LINE_DASH_TEXTURE)];
static LINE: Source =
    Source { wgsl: &LINE_SHADER, attributes: LINE_ATTRIBUTES, instance_attributes: &[], textures: &[] };
static LINE_GRADIENT: Source = Source {
    wgsl: &LINE_GRADIENT_SHADER,
    attributes: LINE_GRADIENT_ATTRIBUTES,
    instance_attributes: &[],
    textures: LINE_IMAGE_TEXTURES,
};
static LINE_PATTERN: Source = Source {
    wgsl: &LINE_PATTERN_SHADER,
    attributes: LINE_PATTERN_ATTRIBUTES,
    instance_attributes: &[],
    textures: LINE_IMAGE_TEXTURES,
};
static LINE_SDF: Source = Source {
    wgsl: &LINE_SDF_SHADER,
    attributes: LINE_SDF_ATTRIBUTES,
    instance_attributes: &[],
    textures: LINE_IMAGE_TEXTURES,
};
static LINE_GRADIENT_SDF: Source = Source {
    wgsl: &LINE_GRADIENT_SDF_SHADER,
    attributes: LINE_GRADIENT_SDF_ATTRIBUTES,
    instance_attributes: &[],
    textures: LINE_GRADIENT_SDF_TEXTURES,
};

/// The source of each of the type's programs.
pub(super) static SOURCES: &[(&Program, &Source)] = &[
    (&programs::LINE, &LINE),
    (&programs::LINE_GRADIENT, &LINE_GRADIENT),
    (&programs::LINE_PATTERN, &LINE_PATTERN),
    (&programs::LINE_SDF, &LINE_SDF),
    (&programs::LINE_GRADIENT_SDF, &LINE_GRADIENT_SDF),
];

const LINE_GRADIENT_SDF_SHADER: Wgsl = Wgsl {
    vertex: Some(
        r"
// The key's DEVICE_PIXEL_RATIO, which the pipeline sets.
override DEVICE_PIXEL_RATIO: f32 = 1.0;

struct VertexInput {
    @location(4) pos_normal: vec2<i32>,
    @location(5) data: vec4<u32>,
#ifndef HAS_UNIFORM_u_blur
    @location(6) blur: vec2<f32>,
#endif
#ifndef HAS_UNIFORM_u_opacity
    @location(7) opacity: vec2<f32>,
#endif
#ifndef HAS_UNIFORM_u_gapwidth
    @location(8) gapwidth: vec2<f32>,
#endif
#ifndef HAS_UNIFORM_u_offset
    @location(9) offset: vec2<f32>,
#endif
#ifndef HAS_UNIFORM_u_width
    @location(10) width: vec2<f32>,
#endif
#ifndef HAS_UNIFORM_u_floorwidth
    @location(11) floorwidth: vec2<f32>,
#endif
#ifndef HAS_UNIFORM_u_dasharray
    @location(12) dasharray: vec4<f32>,
#endif
};

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

struct LineSDFDrawableUBO {
    matrix: mat4x4<f32>,
    patternscale_a: vec2<f32>,
    patternscale_b: vec2<f32>,
    tex_y_a: f32,
    tex_y_b: f32,
    ratio: f32,
    color_t: f32,
    blur_t: f32,
    opacity_t: f32,
    gapwidth_t: f32,
    offset_t: f32,
    width_t: f32,
    floorwidth_t: f32,
    atlas_height: f32,
    pad2: f32,
};

struct LineEvaluatedPropsUBO {
    color: vec4<f32>,
    blur: f32,
    opacity: f32,
    gapwidth: f32,
    offset: f32,
    width: f32,
    floorwidth: f32,
    pad0: f32,
    pad1: f32,
};

struct GlobalPaintParamsUBO {
    units_to_pixels: vec2<f32>,
    world_size: vec2<f32>,
    camera_to_center_distance: f32,
    aspect_ratio: f32,
    pixel_ratio: f32,
    pad1: f32,
};

struct GlobalIndexUBO {
    value: u32,
    pad0: vec3<u32>,
};

struct LineSDFDrawableEntry {
    data: LineSDFDrawableUBO,
};

@group(0) @binding(0) var<uniform> paintParams: GlobalPaintParamsUBO;
@group(0) @binding(2) var<storage, read> drawableVector: array<LineSDFDrawableEntry>;
@group(0) @binding(4) var<storage, read> projectionVector: array<ProjectionUBO>;
@group(0) @binding(5) var<uniform> props: LineEvaluatedPropsUBO;
@group(0) @binding(1) var<uniform> globalIndex: GlobalIndexUBO;

@vertex
fn main(in: VertexInput) -> VertexOutput {
    var out: VertexOutput;
    let index = globalIndex.value;
    let drawable = drawableVector[index].data;

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
    ),
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

struct LineEvaluatedPropsUBO {
    color: vec4<f32>,
    blur: f32,
    opacity: f32,
    gapwidth: f32,
    offset: f32,
    width: f32,
    floorwidth: f32,
    pad0: f32,
    pad1: f32,
};

struct LineSDFTilePropsUBO {
    sdfgamma: f32,
    mix: f32,
    pad1: f32,
    pad2: f32,
};

struct GlobalPaintParamsUBO {
    units_to_pixels: vec2<f32>,
    world_size: vec2<f32>,
    camera_to_center_distance: f32,
    aspect_ratio: f32,
    pixel_ratio: f32,
    pad1: f32,
};

struct GlobalIndexUBO {
    value: u32,
    pad0: vec3<u32>,
};

struct LineSDFTilePropsEntry {
    data: LineSDFTilePropsUBO,
    pad0: vec4<f32>,
    pad1: vec4<f32>,
    pad2: vec4<f32>,
};

@group(0) @binding(0) var<uniform> paintParams: GlobalPaintParamsUBO;
@group(0) @binding(3) var<storage, read> tilePropsVector: array<LineSDFTilePropsEntry>;
@group(0) @binding(1) var<uniform> globalIndex: GlobalIndexUBO;
@group(0) @binding(5) var<uniform> props: LineEvaluatedPropsUBO;
@group(1) @binding(0) var gradient_sampler: sampler;
@group(1) @binding(1) var gradient_texture: texture_2d<f32>;
@group(1) @binding(2) var dash_sampler: sampler;
@group(1) @binding(3) var dash_texture: texture_2d<f32>;

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
    let tileProps = tilePropsVector[globalIndex.value].data;
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
    let dist_a = textureSample(dash_texture, dash_sampler, in.v_tex_a).r;
    let dist_b = textureSample(dash_texture, dash_sampler, in.v_tex_b).r;
    let sdfdist = mix(dist_a, dist_b, tileProps.mix);
    var sdfgamma = tileProps.sdfgamma;
#ifndef HAS_UNIFORM_u_dasharray
    sdfgamma = sdfgamma / in.v_dash_width;
#endif

    // Calculate SDF alpha
    let sdf_alpha = smoothstep(0.5 - sdfgamma / floorwidth, 0.5 + sdfgamma / floorwidth, sdfdist);

    // The gradient ramp is stored in a texture indexed by the ratio along the entire line
    let color = textureSample(gradient_texture, gradient_sampler, vec2<f32>(in.v_lineprogress, 0.5));

    return color * (alpha * opacity * sdf_alpha);
}
",
    ),
    prelude: None,
};

const LINE_GRADIENT_SHADER: Wgsl = Wgsl {
    vertex: Some(
        r"
// The key's DEVICE_PIXEL_RATIO, which the pipeline sets.
override DEVICE_PIXEL_RATIO: f32 = 1.0;

struct VertexInput {
    @location(4) pos_normal: vec2<i32>,
    @location(5) data: vec4<u32>,
#ifndef HAS_UNIFORM_u_blur
    @location(6) blur: vec2<f32>,
#endif
#ifndef HAS_UNIFORM_u_opacity
    @location(7) opacity: vec2<f32>,
#endif
#ifndef HAS_UNIFORM_u_gapwidth
    @location(8) gapwidth: vec2<f32>,
#endif
#ifndef HAS_UNIFORM_u_offset
    @location(9) offset: vec2<f32>,
#endif
#ifndef HAS_UNIFORM_u_width
    @location(10) width: vec2<f32>,
#endif
};

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

struct LineGradientDrawableUBO {
    matrix: mat4x4<f32>,
    ratio: f32,
    blur_t: f32,
    opacity_t: f32,
    gapwidth_t: f32,
    offset_t: f32,
    width_t: f32,
    pad1: f32,
    pad2: f32,
};

struct LineEvaluatedPropsUBO {
    color: vec4<f32>,
    blur: f32,
    opacity: f32,
    gapwidth: f32,
    offset: f32,
    width: f32,
    floorwidth: f32,
    pad0: f32,
    pad1: f32,
};

struct GlobalPaintParamsUBO {
    units_to_pixels: vec2<f32>,
    world_size: vec2<f32>,
    camera_to_center_distance: f32,
    aspect_ratio: f32,
    pixel_ratio: f32,
    pad1: f32,
};

struct GlobalIndexUBO {
    value: u32,
    pad0: vec3<u32>,
};

struct LineGradientDrawableEntry {
    data: LineGradientDrawableUBO,
    pad0: vec4<f32>,
    pad1: vec4<f32>,
};

@group(0) @binding(0) var<uniform> paintParams: GlobalPaintParamsUBO;
@group(0) @binding(2) var<storage, read> drawableVector: array<LineGradientDrawableEntry>;
@group(0) @binding(4) var<storage, read> projectionVector: array<ProjectionUBO>;
@group(0) @binding(5) var<uniform> props: LineEvaluatedPropsUBO;
@group(0) @binding(1) var<uniform> globalIndex: GlobalIndexUBO;

@vertex
fn main(in: VertexInput) -> VertexOutput {
    var out: VertexOutput;
    let drawable = drawableVector[globalIndex.value].data;

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
    ),
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

struct LineEvaluatedPropsUBO {
    color: vec4<f32>,
    blur: f32,
    opacity: f32,
    gapwidth: f32,
    offset: f32,
    width: f32,
    floorwidth: f32,
    pad0: f32,
    pad1: f32,
};

struct GlobalPaintParamsUBO {
    units_to_pixels: vec2<f32>,
    world_size: vec2<f32>,
    camera_to_center_distance: f32,
    aspect_ratio: f32,
    pixel_ratio: f32,
    pad1: f32,
};

@group(0) @binding(0) var<uniform> paintParams: GlobalPaintParamsUBO;
@group(0) @binding(5) var<uniform> props: LineEvaluatedPropsUBO;
@group(1) @binding(0) var gradient_sampler: sampler;
@group(1) @binding(1) var gradient_texture: texture_2d<f32>;

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
    let color = textureSample(gradient_texture, gradient_sampler, vec2<f32>(in.v_lineprogress, 0.5));

    return color * (alpha * opacity);
}
",
    ),
    prelude: None,
};

const LINE_PATTERN_SHADER: Wgsl = Wgsl {
    vertex: Some(
        r"
// The key's DEVICE_PIXEL_RATIO, which the pipeline sets.
override DEVICE_PIXEL_RATIO: f32 = 1.0;

struct VertexInput {
    @location(4) pos_normal: vec2<i32>,
    @location(5) data: vec4<u32>,
#ifndef HAS_UNIFORM_u_blur
    @location(6) blur: vec2<f32>,
#endif
#ifndef HAS_UNIFORM_u_opacity
    @location(7) opacity: vec2<f32>,
#endif
#ifndef HAS_UNIFORM_u_gapwidth
    @location(8) gapwidth: vec2<f32>,
#endif
#ifndef HAS_UNIFORM_u_offset
    @location(9) offset: vec2<f32>,
#endif
#ifndef HAS_UNIFORM_u_width
    @location(10) width: vec2<f32>,
#endif
#ifndef HAS_UNIFORM_u_pattern_from
    @location(11) pattern_from: vec4<u32>,
#endif
#ifndef HAS_UNIFORM_u_pattern_to
    @location(12) pattern_to: vec4<u32>,
#endif
};

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

struct LinePatternDrawableUBO {
    matrix: mat4x4<f32>,
    ratio: f32,
    blur_t: f32,
    opacity_t: f32,
    gapwidth_t: f32,
    offset_t: f32,
    width_t: f32,
    pattern_from_t: f32,
    pattern_to_t: f32,
    floorwidth_t: f32,
    pad1: f32,
    pad2: f32,
    pad3: f32,
};

struct LinePatternTilePropsUBO {
    pattern_from: vec4<f32>,
    pattern_to: vec4<f32>,
    scale: vec4<f32>,
    texsize: vec2<f32>,
    fade: f32,
    pad2: f32,
};

struct LineEvaluatedPropsUBO {
    color: vec4<f32>,
    blur: f32,
    opacity: f32,
    gapwidth: f32,
    offset: f32,
    width: f32,
    floorwidth: f32,
    pad0: f32,
    pad1: f32,
};

struct GlobalPaintParamsUBO {
    units_to_pixels: vec2<f32>,
    world_size: vec2<f32>,
    camera_to_center_distance: f32,
    aspect_ratio: f32,
    pixel_ratio: f32,
    pad1: f32,
};

struct GlobalIndexUBO {
    value: u32,
    pad0: vec3<u32>,
};

struct LinePatternDrawableEntry {
    data: LinePatternDrawableUBO,
    pad0: vec4<f32>,
};

struct LinePatternTilePropsEntry {
    data: LinePatternTilePropsUBO,
};

@group(0) @binding(0) var<uniform> paintParams: GlobalPaintParamsUBO;
@group(0) @binding(2) var<storage, read> drawableVector: array<LinePatternDrawableEntry>;
@group(0) @binding(4) var<storage, read> projectionVector: array<ProjectionUBO>;
@group(0) @binding(3) var<storage, read> tilePropsVector: array<LinePatternTilePropsEntry>;
@group(0) @binding(5) var<uniform> props: LineEvaluatedPropsUBO;
@group(0) @binding(1) var<uniform> globalIndex: GlobalIndexUBO;

@vertex
fn main(in: VertexInput) -> VertexOutput {
    var out: VertexOutput;
    let index = globalIndex.value;
    let drawable = drawableVector[index].data;
    let tileProps = tilePropsVector[index].data;

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
    ),
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

struct LineEvaluatedPropsUBO {
    color: vec4<f32>,
    blur: f32,
    opacity: f32,
    gapwidth: f32,
    offset: f32,
    width: f32,
    floorwidth: f32,
    pad0: f32,
    pad1: f32,
};

struct GlobalPaintParamsUBO {
    units_to_pixels: vec2<f32>,
    world_size: vec2<f32>,
    camera_to_center_distance: f32,
    aspect_ratio: f32,
    pixel_ratio: f32,
    pad1: f32,
};

struct LinePatternTilePropsUBO {
    pattern_from: vec4<f32>,
    pattern_to: vec4<f32>,
    scale: vec4<f32>,
    texsize: vec2<f32>,
    fade: f32,
    pad2: f32,
};

struct GlobalIndexUBO {
    value: u32,
    pad0: vec3<u32>,
};

struct LinePatternTilePropsEntry {
    data: LinePatternTilePropsUBO,
};

@group(0) @binding(0) var<uniform> paintParams: GlobalPaintParamsUBO;
@group(0) @binding(3) var<storage, read> tilePropsVector: array<LinePatternTilePropsEntry>;
@group(0) @binding(1) var<uniform> globalIndex: GlobalIndexUBO;
@group(0) @binding(5) var<uniform> props: LineEvaluatedPropsUBO;
@group(1) @binding(0) var pattern_sampler: sampler;
@group(1) @binding(1) var pattern_texture: texture_2d<f32>;

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
    let tileProps = tilePropsVector[globalIndex.value].data;

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
        color_a = textureSample(pattern_texture, pattern_sampler, pos_a);
    }
    var color_b = vec4<f32>(0.0);
    if (tileProps.fade > 0.0) {
        let x_b = glMod(in.v_linesofar / patternSizeBX * aspect_b, 1.0);
        var pos_b = atlas_pos((pattern_tl_b - 1.0) * texel_size, (pattern_br_b - pattern_tl_b + 2.0) * texel_size * vec2<f32>(x_b, y));
        pos_b = clamp(pos_b, (pattern_tl_b - 1.0) * texel_size + half_texel, (pattern_br_b + 1.0) * texel_size - half_texel);
        color_b = textureSample(pattern_texture, pattern_sampler, pos_b);
    }

    let pattern_color = mix(color_a, color_b, tileProps.fade);

    return pattern_color * (alpha * opacity);
}
",
    ),
    prelude: None,
};

const LINE_SDF_SHADER: Wgsl = Wgsl {
    vertex: Some(
        r"
// The key's DEVICE_PIXEL_RATIO, which the pipeline sets.
override DEVICE_PIXEL_RATIO: f32 = 1.0;

struct VertexInput {
    @location(4) pos_normal: vec2<i32>,
    @location(5) data: vec4<u32>,
#ifndef HAS_UNIFORM_u_color
    @location(6) color: vec4<f32>,
#endif
#ifndef HAS_UNIFORM_u_blur
    @location(7) blur: vec2<f32>,
#endif
#ifndef HAS_UNIFORM_u_opacity
    @location(8) opacity: vec2<f32>,
#endif
#ifndef HAS_UNIFORM_u_gapwidth
    @location(9) gapwidth: vec2<f32>,
#endif
#ifndef HAS_UNIFORM_u_offset
    @location(10) offset: vec2<f32>,
#endif
#ifndef HAS_UNIFORM_u_width
    @location(11) width: vec2<f32>,
#endif
#ifndef HAS_UNIFORM_u_floorwidth
    @location(12) floorwidth: vec2<f32>,
#endif
#ifndef HAS_UNIFORM_u_dasharray
    @location(13) dasharray: vec4<f32>,
#endif
};

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

struct LineSDFDrawableUBO {
    matrix: mat4x4<f32>,
    patternscale_a: vec2<f32>,
    patternscale_b: vec2<f32>,
    tex_y_a: f32,
    tex_y_b: f32,
    ratio: f32,
    color_t: f32,
    blur_t: f32,
    opacity_t: f32,
    gapwidth_t: f32,
    offset_t: f32,
    width_t: f32,
    floorwidth_t: f32,
    atlas_height: f32,
    pad2: f32,
};

struct LineSDFTilePropsUBO {
    sdfgamma: f32,
    mix: f32,
    pad1: f32,
    pad2: f32,
};

struct LineEvaluatedPropsUBO {
    color: vec4<f32>,
    blur: f32,
    opacity: f32,
    gapwidth: f32,
    offset: f32,
    width: f32,
    floorwidth: f32,
    pad0: f32,
    pad1: f32,
};

struct GlobalPaintParamsUBO {
    units_to_pixels: vec2<f32>,
    world_size: vec2<f32>,
    camera_to_center_distance: f32,
    aspect_ratio: f32,
    pixel_ratio: f32,
    pad1: f32,
};

struct GlobalIndexUBO {
    value: u32,
    pad0: vec3<u32>,
};

struct LineSDFDrawableEntry {
    data: LineSDFDrawableUBO,
};

struct LineSDFTilePropsEntry {
    data: LineSDFTilePropsUBO,
    pad0: vec4<f32>,
    pad1: vec4<f32>,
    pad2: vec4<f32>,
};

@group(0) @binding(0) var<uniform> paintParams: GlobalPaintParamsUBO;
@group(0) @binding(2) var<storage, read> drawableVector: array<LineSDFDrawableEntry>;
@group(0) @binding(4) var<storage, read> projectionVector: array<ProjectionUBO>;
@group(0) @binding(3) var<storage, read> tilePropsVector: array<LineSDFTilePropsEntry>;
@group(0) @binding(5) var<uniform> props: LineEvaluatedPropsUBO;
@group(0) @binding(1) var<uniform> globalIndex: GlobalIndexUBO;

@vertex
fn main(in: VertexInput) -> VertexOutput {
    var out: VertexOutput;
    let index = globalIndex.value;
    let drawable = drawableVector[index].data;
    let tileProps = tilePropsVector[index].data;

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
    ),
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

struct LineEvaluatedPropsUBO {
    color: vec4<f32>,
    blur: f32,
    opacity: f32,
    gapwidth: f32,
    offset: f32,
    width: f32,
    floorwidth: f32,
    pad0: f32,
    pad1: f32,
};

struct LineSDFTilePropsUBO {
    sdfgamma: f32,
    mix: f32,
    pad1: f32,
    pad2: f32,
};

struct GlobalPaintParamsUBO {
    units_to_pixels: vec2<f32>,
    world_size: vec2<f32>,
    camera_to_center_distance: f32,
    aspect_ratio: f32,
    pixel_ratio: f32,
    pad1: f32,
};

struct GlobalIndexUBO {
    value: u32,
    pad0: vec3<u32>,
};

struct LineSDFTilePropsEntry {
    data: LineSDFTilePropsUBO,
    pad0: vec4<f32>,
    pad1: vec4<f32>,
    pad2: vec4<f32>,
};

@group(0) @binding(0) var<uniform> paintParams: GlobalPaintParamsUBO;
@group(0) @binding(3) var<storage, read> tilePropsVector: array<LineSDFTilePropsEntry>;
@group(0) @binding(1) var<uniform> globalIndex: GlobalIndexUBO;
@group(0) @binding(5) var<uniform> props: LineEvaluatedPropsUBO;
@group(1) @binding(0) var sdf_sampler: sampler;
@group(1) @binding(1) var sdf_texture: texture_2d<f32>;

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
    let tileProps = tilePropsVector[globalIndex.value].data;
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
    let dist_a = textureSample(sdf_texture, sdf_sampler, in.v_tex_a).r;
    let dist_b = textureSample(sdf_texture, sdf_sampler, in.v_tex_b).r;
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

const LINE_SHADER: Wgsl = Wgsl {
    vertex: Some(
        r"
// The key's DEVICE_PIXEL_RATIO, which the pipeline sets.
override DEVICE_PIXEL_RATIO: f32 = 1.0;

struct VertexInput {
    @location(4) pos_normal: vec2<i32>,  // packed position and normal
    @location(5) data: vec4<u32>,        // extrude, direction, linesofar
#ifndef HAS_UNIFORM_u_color
    @location(6) color: vec4<f32>,
#endif
#ifndef HAS_UNIFORM_u_blur
    @location(7) blur: vec2<f32>,
#endif
#ifndef HAS_UNIFORM_u_opacity
    @location(8) opacity: vec2<f32>,
#endif
#ifndef HAS_UNIFORM_u_gapwidth
    @location(9) gapwidth: vec2<f32>,
#endif
#ifndef HAS_UNIFORM_u_offset
    @location(10) offset: vec2<f32>,
#endif
#ifndef HAS_UNIFORM_u_width
    @location(11) width: vec2<f32>,
#endif
};

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

struct LineDrawableUBO {
    matrix: mat4x4<f32>,
    ratio: f32,
    color_t: f32,
    blur_t: f32,
    opacity_t: f32,
    gapwidth_t: f32,
    offset_t: f32,
    width_t: f32,
    pad1: f32,
};

struct LineEvaluatedPropsUBO {
    color: vec4<f32>,
    blur: f32,
    opacity: f32,
    gapwidth: f32,
    offset: f32,
    width: f32,
    floorwidth: f32,
    pad0: f32,
    pad1: f32,
};

struct GlobalPaintParamsUBO {
    units_to_pixels: vec2<f32>,
    world_size: vec2<f32>,
    camera_to_center_distance: f32,
    aspect_ratio: f32,
    pixel_ratio: f32,
    pad1: f32,
};

struct GlobalIndexUBO {
    value: u32,
    pad0: vec3<u32>,
};

struct LineDrawableEntry {
    data: LineDrawableUBO,
    pad0: vec4<f32>,
    pad1: vec4<f32>,
};

@group(0) @binding(0) var<uniform> paintParams: GlobalPaintParamsUBO;
@group(0) @binding(2) var<storage, read> drawableVector: array<LineDrawableEntry>;
@group(0) @binding(4) var<storage, read> projectionVector: array<ProjectionUBO>;
@group(0) @binding(5) var<uniform> props: LineEvaluatedPropsUBO;
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
    ),
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

struct LineEvaluatedPropsUBO {
    color: vec4<f32>,
    blur: f32,
    opacity: f32,
    gapwidth: f32,
    offset: f32,
    width: f32,
    floorwidth: f32,
    pad0: f32,
    pad1: f32,
};

struct GlobalPaintParamsUBO {
    units_to_pixels: vec2<f32>,
    world_size: vec2<f32>,
    camera_to_center_distance: f32,
    aspect_ratio: f32,
    pixel_ratio: f32,
    pad1: f32,
};

@group(0) @binding(0) var<uniform> paintParams: GlobalPaintParamsUBO;
@group(0) @binding(5) var<uniform> props: LineEvaluatedPropsUBO;

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
