// What the fill programs share: the layer's buffers, the pattern of a pattern
// fill, and where a fragment is in the window, which an outline fades from.
//
// The layer's buffer of drawables has elements the size of the largest
// drawable UBO, FillPatternDrawableUBO; a smaller one is padded to it, but on
// OpenGL, which binds one element as a block of its UBO's size.

import package::common::{atlas_pos, get_pattern_pos, glMod2};
import package::globals::{drawable_index, paintParams};

struct FillEvaluatedPropsUBO {
    color: vec4<f32>,
    outline_color: vec4<f32>,
    opacity: f32,
    fade: f32,
    from_scale: f32,
    to_scale: f32,
};

struct FillPatternDrawableUBO {
    matrix: mat4x4<f32>,
    pixel_coord_upper: vec2<f32>,
    pixel_coord_lower: vec2<f32>,
    tile_ratio: f32,
    // Each data-driven property's interpolation between its zoom stops.
    pattern_from_t: f32,
    pattern_to_t: f32,
    opacity_t: f32,
    color_t: f32,
    pad1: f32,
    pad2: f32,
    pad3: f32,
};

struct FillPatternTilePropsUBO {
    // The pattern images' corners in the atlas: top left in xy, bottom right
    // in zw.
    pattern_from: vec4<f32>,
    pattern_to: vec4<f32>,
    // The atlas's size.
    texsize: vec2<f32>,
    // Whether the images are SDFs, in the layer's color.
    sdf: f32,
    pad2: f32,
};

// ID_FILL_DRAWABLE_UBO of the pattern programs: one element for each
// drawable of the layer.
@group(1) @binding(2) var<storage, read> patternDrawables: array<FillPatternDrawableUBO>;
// ID_FILL_TILE_PROPS_UBO: one element for each drawable of the layer.
@group(1) @binding(5) var<storage, read> tilePropsVector: array<FillPatternTilePropsUBO>;
// ID_FILL_EVALUATED_PROPS_UBO
@group(1) @binding(6) var<uniform> props: FillEvaluatedPropsUBO;

// ID_FILL_IMAGE_TEXTURE: the pattern atlas.
@group(3) @binding(0) var image: texture_2d<f32>;
@group(3) @binding(3) var image_sampler: sampler;

fn load_pattern_drawable() -> FillPatternDrawableUBO {
    return patternDrawables[drawable_index()];
}

fn load_tile_props() -> FillPatternTilePropsUBO {
    return tilePropsVector[drawable_index()];
}

// The positions in the two patterns' images of the tile position `pos`, in
// image sizes: the from pattern's in xy, the to pattern's in zw.
fn pattern_positions(drawable: FillPatternDrawableUBO, pattern_from: vec4<f32>, pattern_to: vec4<f32>,
                     pos: vec2<f32>) -> vec4<f32> {
    let display_size_a = (pattern_from.zw - pattern_from.xy) / paintParams.pixel_ratio;
    let display_size_b = (pattern_to.zw - pattern_to.xy) / paintParams.pixel_ratio;
    let pos_a = get_pattern_pos(drawable.pixel_coord_upper, drawable.pixel_coord_lower,
                                props.from_scale * display_size_a, drawable.tile_ratio, pos);
    let pos_b = get_pattern_pos(drawable.pixel_coord_upper, drawable.pixel_coord_lower,
                                props.to_scale * display_size_b, drawable.tile_ratio, pos);
    return vec4<f32>(pos_a, pos_b);
}

// The color of the pattern at the positions `pattern_positions` gives,
// cross-faded between its two images; an SDF's in `color`.
fn pattern_color(pos_a: vec2<f32>, pos_b: vec2<f32>, pattern_from: vec4<f32>, pattern_to: vec4<f32>,
                 color: vec4<f32>) -> vec4<f32> {
    let tile = load_tile_props();
    let atlas_a = atlas_pos(pattern_from.xy / tile.texsize,
                            (pattern_from.zw - pattern_from.xy) / tile.texsize * glMod2(pos_a, 1.0));
    let atlas_b = atlas_pos(pattern_to.xy / tile.texsize,
                            (pattern_to.zw - pattern_to.xy) / tile.texsize * glMod2(pos_b, 1.0));
    let color_a = textureSample(image, image_sampler, atlas_a);
    let color_b = textureSample(image, image_sampler, atlas_b);
    if (tile.sdf > 0.5) {
        let edge = (256.0 - 64.0) / 256.0;
        let gamma_a = max(fwidth(color_a.a) * 0.5, 1.0 / 255.0 / 16.0);
        let gamma_b = max(fwidth(color_b.a) * 0.5, 1.0 / 255.0 / 16.0);
        let alpha_a = smoothstep(edge - gamma_a, edge + gamma_a, color_a.a);
        let alpha_b = smoothstep(edge - gamma_b, edge + gamma_b, color_b.a);
        return mix(color * alpha_a, color * alpha_b, props.fade);
    }
    return mix(color_a, color_b, props.fade);
}

// The pixel of the window that the clip position `clip` is at, from its
// bottom left.
fn window_position(clip: vec4<f32>) -> vec2<f32> {
    return (clip.xy / clip.w + 1.0) / 2.0 * paintParams.world_size;
}

// The window position `window`, from the bottom left, as a fragment's
// `@builtin(position)` has it: from the bottom left on OpenGL, from the top
// left elsewhere.
@if(gl)
fn fragment_position(window: vec2<f32>) -> vec2<f32> {
    return window;
}
@else
fn fragment_position(window: vec2<f32>) -> vec2<f32> {
    return vec2<f32>(window.x, paintParams.world_size.y - window.y);
}

// The coverage of a fragment at `frag_coord` by an outline through
// `line_pos`, both as `@builtin(position)` has them: it fades out over a
// pixel from the line.
fn outline_alpha(frag_coord: vec4<f32>, line_pos: vec2<f32>) -> f32 {
    return 1.0 - smoothstep(0.0, 1.0, length(line_pos - frag_coord.xy));
}

// The width of an instanced outline in pixels, as wide as OpenGL's outlines.
const LINE_WIDTH: f32 = 2.0;

// A corner of an instanced outline's quad.
struct OutlineCorner {
    position: vec4<f32>,
    // The corner's end of the segment, in pixels from the window's bottom
    // left, and how far along the segment that is, from 0 at its start to 1
    // at its end.
    window: vec2<f32>,
    t: f32,
};

// The corner `corner` of the quad that draws the segment from `start` to
// `end`, of clip space, as a wide line covers it: LINE_WIDTH pixels across
// its minor axis (vertically when it runs more across the window than up
// it), from one end to the other, with the attributes of its end as a wide
// line's fragments get them. The corner's x picks the segment's start (0) or
// end (1), its y the side.
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

    let sa = window_position(a);
    let sb = window_position(b);
    let d = sb - sa;
    if (all(d == vec2<f32>(0.0))) {
        return out;
    }
    let across = select(vec2<f32>(1.0, 0.0), vec2<f32>(0.0, 1.0), abs(d.x) >= abs(d.y));
    let offset = across * (f32(corner.y) - 0.5) * LINE_WIDTH;

    let at_end = corner.x != 0;
    let clip = select(a, b, at_end);
    out.position = clip + vec4<f32>(offset * 2.0 / paintParams.world_size * clip.w, 0.0, 0.0);
    out.window = select(sa, sb, at_end);
    out.t = select(ta, tb, at_end);
    return out;
}
