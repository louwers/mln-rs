//! The fills' WGSL, and the instanced fill outlines': each segment of an
//! outline is an instance of a quad, two pixels wide as OpenGL draws the
//! outline's wide lines, since WebGPU draws lines one pixel wide. Their
//! fragment stages are the outlines'.
//!
//! An instance reads a vertex of the fill and the end of the outline
//! segment that starts there as attributes; where no segment starts, the
//! end is the vertex itself and the instance draws nothing. The static
//! quad's x picks the segment's start (0) or end (1), its y the side.
//!
//! The pulled outlines draw four vertices a segment without vertex
//! attributes. A vertex's index picks its segment and its corner of the
//! quad, and the segment's ends and paint values come from the buffers that
//! hold them for the fill, bound as storage buffers.

use mln_gfx::shaders::source::WgslSource;

use crate::layers::pattern::wgsl::pattern_pos;

pub(crate) const FILL_OUTLINE_PATTERN: WgslSource = WgslSource {
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

pub(crate) const FILL_OUTLINE: WgslSource = WgslSource {
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

pub(crate) const FILL_PATTERN: WgslSource = WgslSource {
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

pub(crate) const FILL: WgslSource = WgslSource {
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

/// The vertex output of the outlines without a pattern.
macro_rules! plain_declarations {
    () => {
        r#"
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
"#
    };
}

/// The instanced fill outlines.
pub(crate) const FILL_OUTLINE_INSTANCED: WgslSource = WgslSource {
    vertex: Some(concat!(
        r#"
"#,
        plain_declarations!(),
        outline_corner!(),
        r#"
@vertex
fn main(in: VertexInput) -> VertexOutput {
    var out: VertexOutput;

    let drawable = drawableVector[globalIndex.value];
    let projection = projectionVector[globalIndex.value];
    let start = vec2<f32>(in.outline_pos);
    let end = vec2<f32>(in.outline_end);
    let corner = outline_corner(projectTileWithPoles(start, start, projection),
                                projectTileWithPoles(end, end, projection), in.pos);
    out.position = corner.position;
    out.pos = corner.pos;

#ifndef HAS_UNIFORM_u_outline_color
    out.color = unpack_mix_color(in.outline_color, drawable.outline_color_t);
#endif
#ifndef HAS_UNIFORM_u_opacity
    out.opacity = unpack_mix_float(in.opacity, drawable.opacity_t);
#endif

    return out;
}
"#
    )),
    fragment: FILL_OUTLINE.fragment,
    prelude: None,
};

/// The pulled fill outlines.
pub(crate) const FILL_OUTLINE_PULLED: WgslSource = WgslSource {
    vertex: Some(concat!(
        plain_declarations!(),
        r#"
// `outlineVertices` and `outlineEnds` hold the fill's vertices and the ends
// of the outline segments that start at them, a word each: x and y in its
// halves.
fn tile_position(word: u32) -> vec2<f32> {
    return vec2<f32>(vec2<i32>(i32(word << 16u) >> 16u, i32(word) >> 16u));
}
"#,
        outline_corner!(),
        r#"
@vertex
fn main(@builtin(vertex_index) vertex_index: u32) -> VertexOutput {
    var out: VertexOutput;

    let drawable = drawableVector[globalIndex.value];
    let projection = projectionVector[globalIndex.value];
    // Four vertices a segment, in the order of the instanced quad's corners.
    let segment = vertex_index / 4u;
    let quad_corner = vertex_index % 4u;
    let start = tile_position(outlineVertices[segment]);
    let end = tile_position(outlineEnds[segment]);
    let corner = outline_corner(projectTileWithPoles(start, start, projection),
                                projectTileWithPoles(end, end, projection),
                                vec2<i32>(i32(quad_corner >> 1u), i32(quad_corner & 1u)));
    out.position = corner.position;
    out.pos = corner.pos;

    // The segment's paint values are its start's.
    let paint_layout = drawable.paint_layout;
    let paint = segment * (paint_layout & 0xffu);
#ifndef HAS_UNIFORM_u_outline_color
    let color_at = paint + ((paint_layout >> 8u) & 0xffu);
    let outline_color = vec4<f32>(outlinePaint[color_at], outlinePaint[color_at + 1u],
                                  outlinePaint[color_at + 2u], outlinePaint[color_at + 3u]);
    out.color = unpack_mix_color(outline_color, drawable.outline_color_t);
#endif
#ifndef HAS_UNIFORM_u_opacity
    let opacity_at = paint + ((paint_layout >> 16u) & 0xffu);
    out.opacity = unpack_mix_float(vec2<f32>(outlinePaint[opacity_at], outlinePaint[opacity_at + 1u]),
                                   drawable.opacity_t);
#endif

    return out;
}
"#
    )),
    fragment: FILL_OUTLINE.fragment,
    prelude: None,
};

/// The instanced fill outlines of a pattern.
pub(crate) const FILL_OUTLINE_PATTERN_INSTANCED: WgslSource = WgslSource {
    vertex: Some(concat!(
        pattern_pos!(),
        r#"
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
                                projectTileWithPoles(end, end, projection), in.pos);
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
    fragment: FILL_OUTLINE_PATTERN.fragment,
    prelude: None,
};
