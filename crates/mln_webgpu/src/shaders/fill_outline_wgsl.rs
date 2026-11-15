//! The instanced fill outline WGSL: each segment of an outline is an
//! instance of a quad, two pixels wide as OpenGL draws the outline's wide
//! lines, since WebGPU draws lines one pixel wide. The fragment shaders are
//! the line shaders' in `wgsl_sources`.
//!
//! An instance reads a vertex of the fill and the end of the outline
//! segment that starts there as attributes; where no segment starts, the
//! end is the vertex itself and the instance draws nothing. The static
//! quad's x picks the segment's start (0) or end (1), its y the side.

use super::shader_source::WgslShaderSource;
use super::wgsl_sources;

/// `outline_corner`, the quad's corner of the attributes' segment.
macro_rules! outline_corner {
    () => {
        r#"
// The width of the outline in pixels.
const LINE_WIDTH: f32 = 2.0;

struct OutlineCorner {
    position: vec4<f32>,
    // The corner's end of the segment, from the top left in pixels, and how
    // far along the segment that is, from 0 at its start to 1 at its end.
    pos: vec2<f32>,
    t: f32,
};

// A corner of the quad that draws the segment from `start` to `end`, of
// clip space, as a wide line covers it: LINE_WIDTH pixels across its minor
// axis (vertically when it runs more across the screen than down it), from
// one end to the other, with the attributes of its end as a wide line's
// fragments get them.
fn outline_corner(start: vec4<f32>, end: vec4<f32>, corner: vec2<i32>) -> OutlineCorner {
    var out: OutlineCorner;
    // The part of the segment in front of the camera, which the GPU clips
    // further: its ends have to be on screen to widen it.
    let near_w = max(start.w, end.w) * 1e-4;
    if (start.w <= near_w && end.w <= near_w) {
        return out;
    }
    var a = start;
    var b = end;
    var ta = 0.0;
    var tb = 1.0;
    if (a.w < near_w) {
        ta = (a.w - near_w) / (a.w - b.w);
        a = mix(start, end, ta);
    }
    if (b.w < near_w) {
        tb = (start.w - near_w) / (start.w - b.w);
        b = mix(start, end, tb);
    }

    let world = paintParams.world_size;
    let sa = (a.xy / a.w * vec2<f32>(1.0, -1.0) + vec2<f32>(1.0)) * 0.5 * world;
    let sb = (b.xy / b.w * vec2<f32>(1.0, -1.0) + vec2<f32>(1.0)) * 0.5 * world;
    let d = sb - sa;
    if (all(d == vec2<f32>(0.0))) {
        return out;
    }
    let across = select(vec2<f32>(1.0, 0.0), vec2<f32>(0.0, 1.0), abs(d.x) >= abs(d.y));
    let offset = across * (f32(corner.y) - 0.5) * LINE_WIDTH;

    let at_end = corner.x != 0;
    let clip = select(a, b, at_end);
    out.position = clip + vec4<f32>(offset * vec2<f32>(2.0, -2.0) / world * clip.w, 0.0, 0.0);
    out.pos = select(sa, sb, at_end);
    out.t = select(ta, tb, at_end);
    return out;
}
"#
    };
}

/// The instanced fill outlines.
pub(crate) const FILL_OUTLINE_INSTANCED_SHADER: WgslShaderSource = WgslShaderSource {
    built_in: "FillOutlineInstancedShader",
    name: "FillOutlineInstancedShader",
    vertex: Some(concat!(
        r#"
struct VertexInput {
    @location(0) corner: vec2<i32>,
    @location(3) outline_pos: vec2<i32>,
    @location(4) outline_end: vec2<i32>,
#ifndef HAS_UNIFORM_u_outline_color
    @location(1) outline_color: vec4<f32>,
#endif
#ifndef HAS_UNIFORM_u_opacity
    @location(2) opacity: vec2<f32>,
#endif
};

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) color: vec4<f32>,
    @location(1) opacity: f32,
    @location(2) pos: vec2<f32>,
};

struct FillOutlineDrawableUBO {
    matrix: mat4x4<f32>,
    outline_color_t: f32,
    opacity_t: f32,
    pad1: f32,
    pad2: f32,
};

struct FillOutlineDrawableUnionUBO {
    fill: FillOutlineDrawableUBO,
    padding1: vec4<f32>,
    padding2: vec4<f32>,
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

struct FillOutlineEvaluatedPropsUBO {
    color: vec4<f32>,
    outline_color: vec4<f32>,
    opacity: f32,
    fade: f32,
    from_scale: f32,
    to_scale: f32,
};

struct GlobalIndexUBO {
    value: u32,
    pad0: vec3<u32>,
};

@group(0) @binding(0) var<uniform> paintParams: GlobalPaintParamsUBO;
@group(0) @binding(1) var<uniform> globalIndex: GlobalIndexUBO;
@group(0) @binding(2) var<storage, read> drawableVector: array<FillOutlineDrawableUnionUBO>;
@group(0) @binding(4) var<storage, read> projectionVector: array<ProjectionUBO>;
@group(0) @binding(6) var<uniform> props: FillOutlineEvaluatedPropsUBO;
"#,
        outline_corner!(),
        r#"
@vertex
fn main(in: VertexInput) -> VertexOutput {
    var out: VertexOutput;

    let drawable = drawableVector[globalIndex.value].fill;
    let projection = projectionVector[globalIndex.value];
    let start = vec2<f32>(in.outline_pos);
    let end = vec2<f32>(in.outline_end);
    let corner = outline_corner(projectTileWithPoles(start, start, projection),
                                projectTileWithPoles(end, end, projection), in.corner);
    out.position = corner.position;
    out.pos = corner.pos;

#ifdef HAS_UNIFORM_u_outline_color
    out.color = props.outline_color;
#else
    out.color = unpack_mix_color(in.outline_color, drawable.outline_color_t);
#endif

#ifdef HAS_UNIFORM_u_opacity
    out.opacity = props.opacity;
#else
    out.opacity = unpack_mix_float(in.opacity, drawable.opacity_t);
#endif

    return out;
}
"#
    )),
    fragment: wgsl_sources::FILL_OUTLINE_SHADER.fragment,
    prelude: None,
};

/// The instanced fill outlines of a pattern.
pub(crate) const FILL_OUTLINE_PATTERN_INSTANCED_SHADER: WgslShaderSource = WgslShaderSource {
    built_in: "FillOutlinePatternInstancedShader",
    name: "FillOutlinePatternInstancedShader",
    vertex: Some(concat!(
        r#"
struct VertexInput {
    @location(0) corner: vec2<i32>,
    @location(9) outline_pos: vec2<i32>,
    @location(10) outline_end: vec2<i32>,
#ifndef HAS_UNIFORM_u_color
    @location(5) color: vec4<f32>,
#endif
#ifndef HAS_UNIFORM_u_pattern_from
    @location(6) pattern_from: vec4<u32>,
#endif
#ifndef HAS_UNIFORM_u_pattern_to
    @location(7) pattern_to: vec4<u32>,
#endif
#ifndef HAS_UNIFORM_u_opacity
    @location(8) opacity: vec2<f32>,
#endif
};

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

struct FillOutlinePatternDrawableUBO {
    matrix: mat4x4<f32>,
    pixel_coord_upper: vec2<f32>,
    pixel_coord_lower: vec2<f32>,
    tile_ratio: f32,
    pattern_from_t: f32,
    pattern_to_t: f32,
    opacity_t: f32,
    color_t: f32,
    pad1: f32,
    pad2: f32,
    pad3: f32,
};

struct FillOutlinePatternTilePropsUBO {
    pattern_from: vec4<f32>,
    pattern_to: vec4<f32>,
    texsize: vec2<f32>,
    sdf: f32,
    pad2: f32,
};

struct FillEvaluatedPropsUBO {
    color: vec4<f32>,
    outline_color: vec4<f32>,
    opacity: f32,
    fade: f32,
    from_scale: f32,
    to_scale: f32,
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

@group(0) @binding(0) var<uniform> paintParams: GlobalPaintParamsUBO;
@group(0) @binding(2) var<storage, read> drawableVector: array<FillOutlinePatternDrawableUBO>;
@group(0) @binding(4) var<storage, read> projectionVector: array<ProjectionUBO>;
@group(0) @binding(5) var<storage, read> tilePropsVector: array<FillOutlinePatternTilePropsUBO>;
@group(0) @binding(6) var<uniform> props: FillEvaluatedPropsUBO;
@group(0) @binding(1) var<uniform> globalIndex: GlobalIndexUBO;
"#,
        outline_corner!(),
        r#"
@vertex
fn main(in: VertexInput) -> VertexOutput {
    var out: VertexOutput;
    let index = globalIndex.value;
    let drawable = drawableVector[index];
    let tileProps = tilePropsVector[index];
    let projection = projectionVector[index];

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

    let pixelRatio = paintParams.pixel_ratio;
    let display_size_a = (pattern_from.zw - pattern_from.xy) / pixelRatio;
    let display_size_b = (pattern_to.zw - pattern_to.xy) / pixelRatio;

    let start = vec2<f32>(in.outline_pos);
    let end = vec2<f32>(in.outline_end);
    let corner = outline_corner(projectTileWithPoles(start, start, projection),
                                projectTileWithPoles(end, end, projection), in.corner);
    let pos = mix(start, end, corner.t);

    out.position = corner.position;
    out.v_pos_a = get_pattern_pos(drawable.pixel_coord_upper, drawable.pixel_coord_lower,
                                  props.from_scale * display_size_a, drawable.tile_ratio, pos);
    out.v_pos_b = get_pattern_pos(drawable.pixel_coord_upper, drawable.pixel_coord_lower,
                                  props.to_scale * display_size_b, drawable.tile_ratio, pos);
    out.v_pos = corner.pos;
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
"#
    )),
    fragment: wgsl_sources::FILL_OUTLINE_PATTERN_SHADER.fragment,
    prelude: None,
};
