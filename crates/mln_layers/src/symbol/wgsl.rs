//! The programs of the `symbol` layer type, its symbols and their collision
//! boxes and circles, in WGSL, for `mln_webgpu`.

use mln_gfx::shaders::program::Program;
use mln_gfx::shaders::shader_defines::{
    ID_COLLISION_ANCHOR_POS_VERTEX_ATTRIBUTE, ID_COLLISION_EXTRUDE_VERTEX_ATTRIBUTE,
    ID_COLLISION_HEIGHT_OFFSET_VERTEX_ATTRIBUTE, ID_COLLISION_PLACED_VERTEX_ATTRIBUTE,
    ID_COLLISION_POS_VERTEX_ATTRIBUTE, ID_COLLISION_SHIFT_VERTEX_ATTRIBUTE, ID_SYMBOL_COLOR_ATTRIBUTE,
    ID_SYMBOL_DATA_ATTRIBUTE, ID_SYMBOL_FADE_OPACITY_ATTRIBUTE, ID_SYMBOL_HALO_BLUR_ATTRIBUTE,
    ID_SYMBOL_HALO_COLOR_ATTRIBUTE, ID_SYMBOL_HALO_WIDTH_ATTRIBUTE, ID_SYMBOL_HEIGHT_OFFSET_ATTRIBUTE,
    ID_SYMBOL_IMAGE_ICON_TEXTURE, ID_SYMBOL_IMAGE_TEXTURE, ID_SYMBOL_OPACITY_ATTRIBUTE,
    ID_SYMBOL_PIXEL_OFFSET_ATTRIBUTE, ID_SYMBOL_POS_OFFSET_ATTRIBUTE, ID_SYMBOL_PROJECTED_POS_ATTRIBUTE,
    ID_SYMBOL_TEX_ORIGIN_ATTRIBUTE,
};
use mln_gfx::shaders::source::wgsl::{Attribute, Source, Texture, Wgsl};

use super::programs;

const SYMBOL_ICON_ATTRIBUTES: &[Attribute] = &[
    Attribute::new(3, ID_SYMBOL_POS_OFFSET_ATTRIBUTE),
    Attribute::new(4, ID_SYMBOL_DATA_ATTRIBUTE),
    Attribute::new(5, ID_SYMBOL_PIXEL_OFFSET_ATTRIBUTE),
    Attribute::new(6, ID_SYMBOL_PROJECTED_POS_ATTRIBUTE),
    Attribute::new(7, ID_SYMBOL_FADE_OPACITY_ATTRIBUTE),
    Attribute::new(8, ID_SYMBOL_OPACITY_ATTRIBUTE),
    Attribute::new(9, ID_SYMBOL_TEX_ORIGIN_ATTRIBUTE),
    Attribute::new(10, ID_SYMBOL_HEIGHT_OFFSET_ATTRIBUTE),
];

const SYMBOL_ICON_TEXTURES: &[Texture] = &[Texture::new(0, ID_SYMBOL_IMAGE_TEXTURE)];

const SYMBOL_SDF_ATTRIBUTES: &[Attribute] = &[
    Attribute::new(3, ID_SYMBOL_POS_OFFSET_ATTRIBUTE),
    Attribute::new(4, ID_SYMBOL_DATA_ATTRIBUTE),
    Attribute::new(5, ID_SYMBOL_PIXEL_OFFSET_ATTRIBUTE),
    Attribute::new(6, ID_SYMBOL_PROJECTED_POS_ATTRIBUTE),
    Attribute::new(7, ID_SYMBOL_FADE_OPACITY_ATTRIBUTE),
    Attribute::new(8, ID_SYMBOL_COLOR_ATTRIBUTE),
    Attribute::new(9, ID_SYMBOL_HALO_COLOR_ATTRIBUTE),
    Attribute::new(10, ID_SYMBOL_OPACITY_ATTRIBUTE),
    Attribute::new(11, ID_SYMBOL_HALO_WIDTH_ATTRIBUTE),
    Attribute::new(12, ID_SYMBOL_HALO_BLUR_ATTRIBUTE),
    Attribute::new(13, ID_SYMBOL_TEX_ORIGIN_ATTRIBUTE),
    Attribute::new(14, ID_SYMBOL_HEIGHT_OFFSET_ATTRIBUTE),
];

const SYMBOL_SDF_TEXTURES: &[Texture] = &[Texture::new(0, ID_SYMBOL_IMAGE_TEXTURE)];

const SYMBOL_TEXT_AND_ICON_ATTRIBUTES: &[Attribute] = &[
    Attribute::new(3, ID_SYMBOL_POS_OFFSET_ATTRIBUTE),
    Attribute::new(4, ID_SYMBOL_DATA_ATTRIBUTE),
    Attribute::new(5, ID_SYMBOL_PROJECTED_POS_ATTRIBUTE),
    Attribute::new(6, ID_SYMBOL_FADE_OPACITY_ATTRIBUTE),
    Attribute::new(7, ID_SYMBOL_COLOR_ATTRIBUTE),
    Attribute::new(8, ID_SYMBOL_HALO_COLOR_ATTRIBUTE),
    Attribute::new(9, ID_SYMBOL_OPACITY_ATTRIBUTE),
    Attribute::new(10, ID_SYMBOL_HALO_WIDTH_ATTRIBUTE),
    Attribute::new(11, ID_SYMBOL_HALO_BLUR_ATTRIBUTE),
    Attribute::new(12, ID_SYMBOL_TEX_ORIGIN_ATTRIBUTE),
    Attribute::new(13, ID_SYMBOL_HEIGHT_OFFSET_ATTRIBUTE),
];

const SYMBOL_TEXT_AND_ICON_TEXTURES: &[Texture] =
    &[Texture::new(0, ID_SYMBOL_IMAGE_TEXTURE), Texture::new(2, ID_SYMBOL_IMAGE_ICON_TEXTURE)];

static SYMBOL_ICON: Source = Source {
    wgsl: &SYMBOL_ICON_SHADER,
    attributes: SYMBOL_ICON_ATTRIBUTES,
    instance_attributes: &[],
    textures: SYMBOL_ICON_TEXTURES,
};

static SYMBOL_SDF: Source = Source {
    wgsl: &SYMBOL_SDF_SHADER,
    attributes: SYMBOL_SDF_ATTRIBUTES,
    instance_attributes: &[],
    textures: SYMBOL_SDF_TEXTURES,
};

static SYMBOL_TEXT_AND_ICON: Source = Source {
    wgsl: &SYMBOL_TEXT_AND_ICON_SHADER,
    attributes: SYMBOL_TEXT_AND_ICON_ATTRIBUTES,
    instance_attributes: &[],
    textures: SYMBOL_TEXT_AND_ICON_TEXTURES,
};

const COLLISION_BOX_ATTRIBUTES: &[Attribute] = &[
    Attribute::new(3, ID_COLLISION_POS_VERTEX_ATTRIBUTE),
    Attribute::new(4, ID_COLLISION_ANCHOR_POS_VERTEX_ATTRIBUTE),
    Attribute::new(5, ID_COLLISION_EXTRUDE_VERTEX_ATTRIBUTE),
    Attribute::new(6, ID_COLLISION_PLACED_VERTEX_ATTRIBUTE),
    Attribute::new(7, ID_COLLISION_SHIFT_VERTEX_ATTRIBUTE),
    Attribute::new(8, ID_COLLISION_HEIGHT_OFFSET_VERTEX_ATTRIBUTE),
];

const COLLISION_CIRCLE_ATTRIBUTES: &[Attribute] = &[
    Attribute::new(3, ID_COLLISION_POS_VERTEX_ATTRIBUTE),
    Attribute::new(4, ID_COLLISION_ANCHOR_POS_VERTEX_ATTRIBUTE),
    Attribute::new(5, ID_COLLISION_EXTRUDE_VERTEX_ATTRIBUTE),
    Attribute::new(6, ID_COLLISION_PLACED_VERTEX_ATTRIBUTE),
];

static COLLISION_BOX: Source = Source {
    wgsl: &COLLISION_BOX_SHADER,
    attributes: COLLISION_BOX_ATTRIBUTES,
    instance_attributes: &[],
    textures: &[],
};

static COLLISION_CIRCLE: Source = Source {
    wgsl: &COLLISION_CIRCLE_SHADER,
    attributes: COLLISION_CIRCLE_ATTRIBUTES,
    instance_attributes: &[],
    textures: &[],
};

/// The source of each of the type's programs.
pub(super) static SOURCES: &[(&Program, &Source)] = &[
    (&programs::SYMBOL_ICON, &SYMBOL_ICON),
    (&programs::SYMBOL_SDF, &SYMBOL_SDF),
    (&programs::SYMBOL_TEXT_AND_ICON, &SYMBOL_TEXT_AND_ICON),
    (&programs::COLLISION_BOX, &COLLISION_BOX),
    (&programs::COLLISION_CIRCLE, &COLLISION_CIRCLE),
];

const COLLISION_BOX_SHADER: Wgsl = Wgsl {
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

const COLLISION_CIRCLE_SHADER: Wgsl = Wgsl {
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

const SYMBOL_ICON_SHADER: Wgsl = Wgsl {
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

const SYMBOL_SDF_SHADER: Wgsl = Wgsl {
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

const SYMBOL_TEXT_AND_ICON_SHADER: Wgsl = Wgsl {
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
