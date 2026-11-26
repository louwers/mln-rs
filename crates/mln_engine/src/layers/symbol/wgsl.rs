//! The WGSL of the symbols' programs.

use mln_gfx::shaders::source::WgslSource;

use crate::render::terrain::wgsl::ground_visibility;

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

pub(crate) const SYMBOL_ICON: WgslSource = WgslSource {
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

pub(crate) const SYMBOL_SDF: WgslSource = WgslSource {
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

pub(crate) const SYMBOL_TEXT_AND_ICON: WgslSource = WgslSource {
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
