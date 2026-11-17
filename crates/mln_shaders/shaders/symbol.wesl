// What the symbol programs share: the layer's buffers, where a vertex of a
// symbol's quad goes, and the coverage of an SDF glyph or icon.

import package::common::SDF_PX;
import package::globals::{drawable_index, paintParams};
import package::projection::{ProjectionUBO, projectTileWithElevation};
@if(PROJECTION_GLOBE)
import package::projection::circumferenceRatioAtTileY;

struct SymbolDrawableUBO {
    matrix: mat4x4<f32>,
    label_plane_matrix: mat4x4<f32>,
    coord_matrix: mat4x4<f32>,

    texsize: vec2<f32>,
    texsize_icon: vec2<f32>,

    is_text_prop: i32,
    rotate_symbol: i32,
    pitch_with_map: i32,
    is_size_zoom_constant: i32,
    is_size_feature_constant: i32,
    is_offset: i32,

    // Interpolates between zoom stops when the size is a composite function.
    size_t: f32,
    // The size when it is both zoom and feature constant.
    size: f32,

    // Each data-driven property's interpolation between its zoom stops.
    fill_color_t: f32,
    halo_color_t: f32,
    opacity_t: f32,
    halo_width_t: f32,
    halo_blur_t: f32,

    is_along_line: i32,
    is_variable_anchor: i32,
    pitched_scale: f32,
    translation: vec2<f32>,
    pad1: f32,
    pad2: f32,
};

struct SymbolTilePropsUBO {
    is_text: i32,
    is_halo: i32,
    gamma_scale: f32,
    pad1: f32,
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

// ID_SYMBOL_DRAWABLE_UBO: one element for each drawable of the layer.
@group(1) @binding(2) var<storage, read> drawableVector: array<SymbolDrawableUBO>;
// ID_SYMBOL_TILE_PROPS_UBO: one element for each drawable of the layer.
@group(1) @binding(3) var<storage, read> tilePropsVector: array<SymbolTilePropsUBO>;
// ID_SYMBOL_EVALUATED_PROPS_UBO
@group(1) @binding(5) var<uniform> props: SymbolEvaluatedPropsUBO;

fn load_drawable() -> SymbolDrawableUBO {
    return drawableVector[drawable_index()];
}

fn load_tile_props() -> SymbolTilePropsUBO {
    return tilePropsVector[drawable_index()];
}

// A clip position outside the clip volume, for a vertex whose symbol is not
// drawn: its triangles are all outside too.
const OFFSCREEN = vec4<f32>(-2.0, -2.0, -2.0, 1.0);

// The fade opacity a vertex carries packed with its direction
// (`unpack_opacity`), moved on by the fade since placement.
fn symbol_fade_opacity(packed: vec2<f32>) -> f32 {
    let fade_change = select(-paintParams.symbol_fade_change, paintParams.symbol_fade_change, packed.y > 0.5);
    return clamp(packed.x + fade_change, 0.0, 1.0);
}

// The symbol's size, before the perspective's scaling, from the vertex's
// sizes at the zoom stops (`a_size`, the minimum's double plus whether the
// symbol is an SDF).
fn symbol_size(a_size: vec2<f32>, drawable: SymbolDrawableUBO) -> f32 {
    let a_size_min = floor(a_size.x * 0.5);
    if (drawable.is_size_zoom_constant == 0 && drawable.is_size_feature_constant == 0) {
        return mix(a_size_min, a_size.y, drawable.size_t) / 128.0;
    } else if (drawable.is_size_zoom_constant != 0 && drawable.is_size_feature_constant == 0) {
        return a_size_min / 128.0;
    }
    return drawable.size;
}

// Whether a screen-aligned symbol at ground level whose anchor projects to
// `anchor` is behind the globe's horizon: it goes at once, rather than
// fading out once placement next finds it occluded, as its clip Z of 0
// escapes the horizon's clipping. A pitched symbol's vertices go through the
// globe projection, which clips them, and placement hides a raised one once
// the planet blocks the line of sight to it.
@if(PROJECTION_GLOBE)
fn behind_horizon(anchor: vec4<f32>, height_offset: f32, drawable: SymbolDrawableUBO) -> bool {
    return drawable.pitch_with_map == 0 && height_offset <= 0.0 && anchor.z > anchor.w;
}
@else
fn behind_horizon(anchor: vec4<f32>, height_offset: f32, drawable: SymbolDrawableUBO) -> bool {
    return false;
}

// What a symbol's size scales by for its anchor's distance from the camera,
// `anchor_w` its clip W. A symbol pitched with the map is laid out in pitched
// space, which shrinks distant symbols relative to the viewport: the ratio
// counteracts part of that. One not pitched is laid out in the viewport,
// which makes distant symbols larger relative to the features around them:
// the ratio counteracts part of that too.
fn perspective_ratio(anchor_w: f32, drawable: SymbolDrawableUBO) -> f32 {
    let distance_ratio = select(
        paintParams.camera_to_center_distance / anchor_w,
        anchor_w / paintParams.camera_to_center_distance,
        drawable.pitch_with_map != 0
    );
    // Clamped, against oversized symbols near the camera in pitched or
    // overzoomed tiles.
    return clamp(0.5 + 0.5 * distance_ratio, 0.0, 4.0);
}

// The clip position of a vertex `offset` from its symbol's anchor in the
// label plane, in pixels before rotation:
// - `anchor` is the anchor's clip position, `translated_pos` its tile
//   position after the layer's translation.
// - `projected_pos` is the vertex's dynamic position: the anchor in the
//   label plane (xy) and the segment's angle (z).
// - `globe_scaling` is whether a pitched symbol on the globe scales with its
//   latitude.
fn symbol_position(
    drawable: SymbolDrawableUBO,
    projection: ProjectionUBO,
    anchor: vec4<f32>,
    translated_pos: vec2<f32>,
    projected_pos: vec3<f32>,
    height_offset: f32,
    offset: vec2<f32>,
    globe_scaling: bool,
) -> vec4<f32> {
    var symbol_rotation = 0.0;
    if (drawable.rotate_symbol != 0) {
        // A point symbol aligned with the map is horizontal in tile units:
        // its angle on screen is that of a short horizontal line in the tile,
        // projected.
        let offset_anchor = projectTileWithElevation(translated_pos + vec2<f32>(1.0, 0.0), height_offset, projection);
        let a = anchor.xy / anchor.w;
        let b = offset_anchor.xy / offset_anchor.w;
        symbol_rotation = atan2((b.y - a.y) / paintParams.aspect_ratio, b.x - a.x);
    }
    let angle = -projected_pos.z + symbol_rotation;
    let angle_sin = sin(angle);
    let angle_cos = cos(angle);
    let rotation_matrix = mat2x2<f32>(angle_cos, -angle_sin, angle_sin, angle_cos);

    var label_pos: vec4<f32>;
    if (drawable.is_along_line != 0 || drawable.is_variable_anchor != 0) {
        label_pos = vec4<f32>(projected_pos.xy, height_offset, 1.0);
    } else if (drawable.pitch_with_map != 0) {
        label_pos = drawable.label_plane_matrix * vec4<f32>(projected_pos.xy + drawable.translation, height_offset, 1.0);
    } else {
        label_pos = drawable.label_plane_matrix *
                    projectTileWithElevation(projected_pos.xy + drawable.translation, height_offset, projection);
    }
    let z = select(0.0, label_pos.z / label_pos.w, drawable.pitch_with_map != 0);

    var projection_scaling = 1.0;
    @if(PROJECTION_GLOBE) {
        if (globe_scaling && drawable.pitch_with_map != 0) {
            let anchor_tile_y = (drawable.coord_matrix * vec4<f32>(label_pos.xy / label_pos.w, z, 1.0)).y;
            projection_scaling = mix(projection_scaling,
                                     1.0 / circumferenceRatioAtTileY(anchor_tile_y, projection) * drawable.pitched_scale,
                                     projection.projection_transition);
        }
    }
    var position = drawable.coord_matrix *
                   vec4<f32>(label_pos.xy / label_pos.w + rotation_matrix * offset * projection_scaling, z, 1.0);
    if (drawable.pitch_with_map != 0) {
        position = projectTileWithElevation(position.xy, position.z, projection);
    }
    return position;
}

// The paint of an SDF symbol at a fragment.
struct SdfPaint {
    fill_color: vec4<f32>,
    halo_color: vec4<f32>,
    opacity: f32,
    halo_width: f32,
    halo_blur: f32,
};

// The tile's text or icon value of each of the layer's paint properties.
fn uniform_sdf_paint(tile: SymbolTilePropsUBO) -> SdfPaint {
    if (tile.is_text != 0) {
        return SdfPaint(props.text_fill_color, props.text_halo_color, props.text_opacity, props.text_halo_width,
                        props.text_halo_blur);
    }
    return SdfPaint(props.icon_fill_color, props.icon_halo_color, props.icon_opacity, props.icon_halo_width,
                    props.icon_halo_blur);
}

// The color of an SDF symbol's fill, or of its halo if the tile draws halos,
// where its distance field reads `dist`. `font_scale` is the symbol's size
// relative to the glyphs' 24 px, and `gamma_scale` its clip W.
fn sdf_color(paint: SdfPaint, dist: f32, tile: SymbolTilePropsUBO, font_scale: f32, gamma_scale: f32,
             fade_opacity: f32) -> vec4<f32> {
    let edge_gamma = 0.105 / paintParams.pixel_ratio;
    let font_gamma = font_scale * tile.gamma_scale;
    let fill_gamma = edge_gamma / font_gamma;
    let halo_gamma = (paint.halo_blur * 1.19 / SDF_PX + edge_gamma) / font_gamma;
    let is_halo = tile.is_halo != 0;
    let gamma_scaled = select(fill_gamma, halo_gamma, is_halo) * gamma_scale;
    let fill_inner_edge = (256.0 - 64.0) / 256.0;
    let inner_edge = select(fill_inner_edge, fill_inner_edge + halo_gamma * gamma_scale, is_halo);
    var alpha = smoothstep(inner_edge - gamma_scaled, inner_edge + gamma_scaled, dist);
    if (is_halo) {
        // The inside of a halo is as translucent as the fill it surrounds.
        let halo_edge = (6.0 - paint.halo_width / font_scale) / SDF_PX;
        alpha = min(smoothstep(halo_edge - gamma_scaled, halo_edge + gamma_scaled, dist), 1.0 - alpha);
    }
    let color = select(paint.fill_color, paint.halo_color, is_halo);
    return color * (alpha * paint.opacity * fade_opacity);
}
