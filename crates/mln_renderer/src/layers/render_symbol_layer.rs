// C++ drawables share the bucket's vertex vectors and see placement's
// changes through them (`updateModified`). Here a drawable gets copies of
// the vectors; when placement, sorting or feature state change the bucket
// (its per-kind change counters), `update` replaces just those attributes
// (opacity, dynamic positions, binders) and the triangle indices of the
// tile's existing drawables, as C++ `updateTileDrawable` does.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::Ordering;

use geometry::Point;
use mln_gfx::projection_variant::ProjectionVariant;
use mln_gfx::shader_group::{ShaderGroupPtr, StringIDSetsPair};
use mln_gfx::shaders::shader_defines::{
    ID_COLLISION_ANCHOR_POS_VERTEX_ATTRIBUTE, ID_COLLISION_EXTRUDE_VERTEX_ATTRIBUTE,
    ID_COLLISION_PLACED_VERTEX_ATTRIBUTE, ID_COLLISION_POS_VERTEX_ATTRIBUTE, ID_COLLISION_SHIFT_VERTEX_ATTRIBUTE,
    ID_SYMBOL_DATA_ATTRIBUTE, ID_SYMBOL_FADE_OPACITY_ATTRIBUTE, ID_SYMBOL_IMAGE_ICON_TEXTURE, ID_SYMBOL_IMAGE_TEXTURE,
    ID_SYMBOL_OPACITY_ATTRIBUTE, ID_SYMBOL_PIXEL_OFFSET_ATTRIBUTE, ID_SYMBOL_POS_OFFSET_ATTRIBUTE,
    ID_SYMBOL_PROJECTED_POS_ATTRIBUTE, ID_SYMBOL_TEX_ORIGIN_ATTRIBUTE,
};
use mln_gfx::vertex_attribute::VertexAttributeArray;
use mln_gfx::{
    AttributeDataType, ColorMode, Context, CullFaceMode, DepthMaskType, DrawMode, DrawableTweakerPtr,
    LayerGroupBasePtr, ShaderRegistry,
};
use mln_map_types::{TileProjector, TransformState};
use mln_style::layer::downcast_arc;
use mln_style::layer_properties::{LayerProperties, get_evaluated};
use mln_style::layers::symbol_layer_impl::{Impl as SymbolLayerImpl, SymbolLayerPaintPropertyOverrides};
use mln_style::layers::symbol_layer_properties::{
    IconColor, IconHaloBlur, IconHaloColor, IconHaloWidth, IconOffset, IconOpacity, IconPitchAlignment,
    IconRotationAlignment, IconSize, IconTextFit, IconTranslate, IconTranslateAnchor, SymbolLayerProperties,
    SymbolLayoutProperties, SymbolPaintProperties, SymbolPlacement, SymbolScreenSpace, SymbolSortKey, TextColor,
    TextField, TextHaloBlur, TextHaloColor, TextHaloWidth, TextOpacity, TextPitchAlignment, TextRotationAlignment,
    TextTranslate, TextTranslateAnchor,
};
use mln_style::properties::Property;
use mln_style::{PropertyEvaluationParameters, TransitionParameters, layer};
use mln_style_types::{AlignmentType, IconTextFitType, SymbolPlacementType, TranslateAnchorType};
use mln_tile_id::{OverscaledTileID, UnwrappedTileID};
use mln_util::Color;
use mln_util::constants::EXTENT;
use mln_util::identity::SimpleIdentity;
use mln_util::interpolate::Interpolate;
use mln_util::mat4::{Mat4, matrix};
use mln_util::vectors::{Vec2, Vec3, Vec4, vec2_max_scalar};

use super::collision_layer_tweaker::{CollisionDrawableData, CollisionLayerTweaker};
use super::symbol_layer_tweaker::SymbolLayerTweaker;
use super::{TileDrawables, drawable_binders_slot, drawable_render_tile_slot, shared_vertices};
use crate::bucket::Bucket;
use crate::bucket_geometry::{BucketGeometry, bucket_vertices, paint_vertices, released_indexes};
use crate::buckets::symbol_bucket::{
    Buffer, CollisionBoxDynamicVertex, CollisionBoxLayoutVertex, CollisionBuffer, PaintProperties, SymbolBucket,
};
use crate::change_request::{ChangeRequest, UniqueChangeRequestVec};
use crate::gfx::drawable_atlases_tweaker::DrawableAtlasesTweaker;
use crate::layer_tweaker::{LayerTweakerPtr, get_tile_matrix_for, layer_tweaker_slot};
use crate::layout::symbol_projection::{get_gl_coord_matrix, get_label_plane_matrix};
use crate::paint_parameters::{PaintParameters, TransformParameters};
use crate::paint_property_binder::read_data_driven_paint_property;
use crate::render_layer::{
    BucketPlacementData, LayerPrepareParameters, RenderLayer, RenderLayerBase, Stats, SymbolTranslate,
    activate_layer_group, change_layer_index, compute_feature_ndc_bound, vertex_color_alpha, vertex_opacity,
};
use crate::render_pass::RenderPass;
use crate::render_tile::RenderTile;
use crate::render_tree::RenderTree;
use crate::update_parameters::UpdateParameters;

fn impl_cast(impl_: &Arc<dyn layer::Impl>) -> &SymbolLayerImpl {
    debug_assert!(std::ptr::eq(impl_.get_type_info(), SymbolLayerImpl::static_type_info()));
    impl_.downcast_ref::<SymbolLayerImpl>().expect("RenderSymbolLayer: not a symbol layer impl")
}

const SYMBOL_ICON_SHADER_NAME: &str = "SymbolIconShader";
const SYMBOL_SDF_SHADER_NAME: &str = "SymbolSDFShader";
const SYMBOL_TEXT_AND_ICON_SHADER_NAME: &str = "SymbolTextAndIconShader";
const POS_OFFSET_ATTRIB_NAME: &str = "a_pos_offset";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SymbolType {
    Text,
    IconRGBA,
    IconSDF,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct SymbolPropertyValues {
    pub(crate) pitch_alignment: AlignmentType,
    pub(crate) rotation_alignment: AlignmentType,
    pub(crate) translate: [f32; 2],
    pub(crate) translate_anchor: TranslateAnchorType,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct SymbolDrawableData {
    pub(crate) is_halo: bool,
    pub(crate) bucket_variable_placement: bool,
    pub(crate) symbol_type: SymbolType,
    pub(crate) pitch_alignment: AlignmentType,
    pub(crate) rotation_alignment: AlignmentType,
    pub(crate) placement: SymbolPlacementType,
    pub(crate) text_fit: IconTextFitType,
    pub(crate) is_offset: bool,
    /// The bucket segment the drawable draws (`sortFeaturesByKey`), or all
    /// of them.
    pub(crate) segment: Option<usize>,
}

/// The bucket change counters a tile's drawables reflect.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct TileVersions {
    opacity: u32,
    dynamic: u32,
    sort: u32,
    binder: u32,
}

impl TileVersions {
    fn of(bucket: &SymbolBucket) -> Self {
        TileVersions {
            opacity: bucket.opacity_version.load(Ordering::SeqCst),
            dynamic: bucket.dynamic_version.load(Ordering::SeqCst),
            sort: bucket.sort_version.load(Ordering::SeqCst),
            binder: bucket.binder_version.load(Ordering::SeqCst),
        }
    }
}

#[must_use]
pub(crate) fn icon_property_values(
    evaluated: &SymbolPaintProperties::PossiblyEvaluated,
    layout: &SymbolLayoutProperties::PossiblyEvaluated,
) -> SymbolPropertyValues {
    SymbolPropertyValues {
        pitch_alignment: *layout.get::<IconPitchAlignment>(),
        rotation_alignment: *layout.get::<IconRotationAlignment>(),
        translate: *evaluated.get::<IconTranslate>(),
        translate_anchor: *evaluated.get::<IconTranslateAnchor>(),
    }
}

#[must_use]
pub(crate) fn text_property_values(
    evaluated: &SymbolPaintProperties::PossiblyEvaluated,
    layout: &SymbolLayoutProperties::PossiblyEvaluated,
) -> SymbolPropertyValues {
    SymbolPropertyValues {
        pitch_alignment: *layout.get::<TextPitchAlignment>(),
        rotation_alignment: *layout.get::<TextRotationAlignment>(),
        translate: *evaluated.get::<TextTranslate>(),
        translate_anchor: *evaluated.get::<TextTranslateAnchor>(),
    }
}

/// The tile matrix of screen-space symbols.
pub(crate) fn get_screen_matrix(translation: [f32; 2]) -> Mat4 {
    let mut m: Mat4 = [0.0; 16];
    matrix::ortho(&mut m, 0.0, f64::from(EXTENT), -f64::from(EXTENT), 0.0, 0.0, 1.0);
    let a = m;
    matrix::translate(&mut m, &a, 0.0, -f64::from(EXTENT), 0.0);
    let a = m;
    matrix::translate(&mut m, &a, f64::from(translation[0]), f64::from(translation[1]), 0.0);
    m
}

/// Bounds each visible placed symbol by projecting its glyph and icon quads
/// as the symbol shaders do.
///
/// `is_screen_space` and `is_offset` are the layer's `symbol-screen-space`
/// and whether it sets `icon-offset`.
#[expect(
    clippy::too_many_arguments,
    reason = "the tile, its bucket, the layer's paint and the camera that place its symbols' captured features"
)]
fn capture_rendered_features(
    stats: &mut Stats,
    layer_id: &str,
    tile: &RenderTile,
    bucket: &SymbolBucket,
    evaluated: &SymbolPaintProperties::PossiblyEvaluated,
    state: &TransformState,
    transform_params: &TransformParameters,
    is_screen_space: bool,
    is_offset: bool,
) {
    if !Bucket::has_data(bucket) {
        return;
    }
    let bucket_state = bucket.state();
    let has_text_data = !bucket_state.text.segments.is_empty();
    let has_icon_data = !bucket_state.icon.segments.is_empty();
    let has_sdf_icon_data = !bucket_state.sdf_icon.segments.is_empty();

    let icon_opacity = evaluated.get::<IconOpacity>();
    let icon_color = evaluated.get::<IconColor>();
    let icon_halo_color = evaluated.get::<IconHaloColor>();
    let text_opacity = evaluated.get::<TextOpacity>();
    let text_color = evaluated.get::<TextColor>();
    let text_halo_color = evaluated.get::<TextHaloColor>();

    let visible_text = has_text_data
        && text_opacity.constant_or(&TextOpacity::default_value()) > 0.0
        && (text_color.constant_or(&TextColor::default_value()).a > 0.0
            || text_halo_color.constant_or(&TextHaloColor::default_value()).a > 0.0);
    let visible_icons = (has_icon_data || has_sdf_icon_data)
        && icon_opacity.constant_or(&IconOpacity::default_value()) > 0.0
        && (icon_color.constant_or(&IconColor::default_value()).a > 0.0
            || icon_halo_color.constant_or(&IconHaloColor::default_value()).a > 0.0);
    if !visible_text && !visible_icons {
        return;
    }

    let Some(bucket_paint_properties) = bucket.paint_properties.get(layer_id) else {
        return;
    };
    let bucket_paint_properties = bucket_paint_properties.read();
    let icon_binders = &bucket_paint_properties.icon_binders;
    let text_binders = &bucket_paint_properties.text_binders;

    let overscaled_tile_id = *tile.get_overscaled_tile_id();
    let tile_id = overscaled_tile_id.to_unwrapped();

    let text_translation = evaluated.get::<TextTranslate>();
    let icon_translation = evaluated.get::<IconTranslate>();
    let text_translation_anchor = *evaluated.get::<TextTranslateAnchor>();
    let icon_translation_anchor = *evaluated.get::<IconTranslateAnchor>();
    let zoom = state.get_zoom();
    let zoom_fraction = state.get_zoom_fraction();
    let pixels_to_tile_units = tile_id.pixels_to_tile_units(1.0, zoom as f32);
    let camera_to_center_distance = f64::from(state.get_camera_to_center_distance());
    let aspect_ratio = f64::from(state.get_size().aspect_ratio());

    let bucket_layout = &*bucket.layout;
    let placement = *bucket_layout.get::<SymbolPlacement>();
    let text_fit = *bucket_layout.get::<IconTextFit>();

    let get_tile_matrix = |translation: &[f32; 2], anchor: TranslateAnchorType| {
        get_tile_matrix_for(
            &tile_id,
            state,
            transform_params,
            *translation,
            anchor,
            /*origin=*/ None,
            /*near_clipped=*/ false,
            /*in_viewport_pixel_units=*/ false,
            /*aligned=*/ false,
        )
    };
    // As the tweaker draws them: the tile matrices leave the translation out,
    // which the shader adds to point anchors and placement to the positions
    // it lays out.
    let untranslated = [0.0f32; 2];
    let text_drawable_matrix = if is_screen_space {
        get_screen_matrix(*text_translation)
    } else {
        get_tile_matrix(&untranslated, text_translation_anchor)
    };
    let icon_drawable_matrix = if is_screen_space {
        get_screen_matrix(*icon_translation)
    } else {
        get_tile_matrix(&untranslated, icon_translation_anchor)
    };

    // On the globe the tile's projection stands in for the tile matrix.
    let projector = (state.is_globe_rendering() && !is_screen_space).then(|| TileProjector::new(state, &tile_id));
    let tile_translation = |translation: &[f32; 2], anchor: TranslateAnchorType| -> Vec2 {
        if is_screen_space {
            return [0.0, 0.0];
        }
        let translate = RenderTile::tile_unit_translation(&tile_id, *translation, anchor, state);
        [f64::from(translate[0]), f64::from(translate[1])]
    };
    let text_tile_translation = tile_translation(text_translation, text_translation_anchor);
    let icon_tile_translation = tile_translation(icon_translation, icon_translation_anchor);

    let mut compute_buffer_bounds = |buffer: &Buffer, is_text: bool| {
        let values = if is_text {
            text_property_values(evaluated, bucket_layout)
        } else {
            icon_property_values(evaluated, bucket_layout)
        };
        let pitch_with_map = values.pitch_alignment == AlignmentType::Map;
        let rotate_with_map = values.rotation_alignment == AlignmentType::Map;
        let along_line = placement != SymbolPlacementType::Point && values.rotation_alignment == AlignmentType::Map;
        let has_variable_placement =
            bucket_state.has_variable_placement && (is_text || text_fit != IconTextFitType::None);

        let size_binder = if is_text { &bucket.text_size_binder } else { &bucket.icon_size_binder };
        let evaluated_size = size_binder.evaluate_for_zoom(zoom as f32);

        let label_plane_matrix = get_label_plane_matrix(pitch_with_map, rotate_with_map, state, pixels_to_tile_units);
        let gl_coord_matrix = get_gl_coord_matrix(pitch_with_map, rotate_with_map, state, pixels_to_tile_units);

        // `projectTile` of the shaders: the tile matrix on Mercator, the
        // tile's projection on the globe.
        let drawable_matrix = if is_text { &text_drawable_matrix } else { &icon_drawable_matrix };
        let translation = if is_text { text_tile_translation } else { icon_tile_translation };
        let project_tile = |tile_point: Vec2| -> Vec4 {
            if let Some(projector) = &projector {
                let projected = projector.project_as_drawn(&Point::new(tile_point[0], tile_point[1]), 0.0);
                let w = projected.signed_distance_from_camera;
                return [projected.point.x * w, projected.point.y * w, 0.0, w];
            }
            matrix::transform_mat4_value(&[tile_point[0], tile_point[1], 0.0, 1.0], drawable_matrix)
        };

        let rotate_in_shader = rotate_with_map && !pitch_with_map && !along_line;

        let static_vertices = buffer.attribute_data.data();
        let dynamic_vertices = buffer.dynamic_attribute_data_for(layer_id);

        for symbol in &buffer.placed_symbols {
            let Some(feature_id) = symbol.feature_id.as_ref().filter(|_| !symbol.hidden) else {
                continue;
            };
            // The shader hides what the horizon covers, whatever the
            // placement made of it.
            if let Some(projector) = &projector {
                let anchor = Point::new(
                    f64::from(symbol.anchor_point.x) + translation[0],
                    f64::from(symbol.anchor_point.y) + translation[1],
                );
                if projector.project_as_drawn(&anchor, 0.0).occluded {
                    continue;
                }
            }

            let start_index = symbol.start_index;
            let vertex_count = symbol.glyph_offsets.len() * 4;

            // Consider the dynamic opacity and color of the first vertex.
            // If either is zero, skip the symbol.
            let opacity_is_constant = if is_text { text_opacity.is_constant() } else { icon_opacity.is_constant() };
            if !opacity_is_constant {
                let vertex = if is_text {
                    text_binders.text_opacity.get_vertex_value(&text_binders.interleaved_vertex_buffer, start_index)
                } else {
                    icon_binders.icon_opacity.get_vertex_value(&icon_binders.interleaved_vertex_buffer, start_index)
                };
                if vertex.and_then(|v| vertex_opacity(&v, zoom_fraction)) == Some(0.0) {
                    continue;
                }
            }
            let color_is_constant = if is_text { text_color.is_constant() } else { icon_color.is_constant() };
            if !color_is_constant {
                let vertex = if is_text {
                    text_binders.text_color.get_vertex_value(&text_binders.interleaved_vertex_buffer, start_index)
                } else {
                    icon_binders.icon_color.get_vertex_value(&icon_binders.interleaved_vertex_buffer, start_index)
                };
                if vertex.and_then(|v| vertex_color_alpha(&v, zoom_fraction)) == Some(0.0) {
                    continue;
                }
            }

            let get_vertex = |i: usize| -> Option<Vec3> {
                let index = start_index.checked_add(i)?;
                let static_vertex = static_vertices.get(index)?;
                let dynamic_vertex = dynamic_vertices.get(index)?;

                let a_pos: Vec2 = [f64::from(static_vertex.a1[0]), f64::from(static_vertex.a1[1])];
                let a_offset: Vec2 = [f64::from(static_vertex.a1[2]), f64::from(static_vertex.a1[3])];
                let a_size: Vec2 = [f64::from(static_vertex.a2[2]), f64::from(static_vertex.a2[3])];
                let a_pxoffset: Vec2 = [f64::from(static_vertex.a3[0]), f64::from(static_vertex.a3[1])];
                let a_min_font_scale: Vec2 =
                    [f64::from(static_vertex.a3[2]) / 256.0, f64::from(static_vertex.a3[3]) / 256.0];
                let a_size_min = (a_size[0] / 2.0).floor();
                let in_projected_pos = [f64::from(dynamic_vertex.a1[0]), f64::from(dynamic_vertex.a1[1])];
                let segment_angle = -dynamic_vertex.a1[2];
                let projected_point = project_tile([a_pos[0] + translation[0], a_pos[1] + translation[1]]);
                let camera_to_anchor_distance = projected_point[3];

                let distance_ratio = if pitch_with_map {
                    camera_to_anchor_distance / camera_to_center_distance
                } else {
                    camera_to_center_distance / camera_to_anchor_distance
                } as f32;
                let perspective_ratio = mln_math::clamp_f64(0.5 + 0.5 * f64::from(distance_ratio), 0.0, 4.0) as f32;

                let mut size = if !evaluated_size.is_zoom_constant && !evaluated_size.is_feature_constant {
                    (a_size_min.interpolate(&a_size[1], f64::from(evaluated_size.size_t)) / 128.0) as f32
                } else if evaluated_size.is_zoom_constant && !evaluated_size.is_feature_constant {
                    (a_size_min / 128.0) as f32
                } else {
                    evaluated_size.size
                };
                if !is_offset {
                    size *= perspective_ratio;
                }

                let font_scale = if is_text { f64::from(size) / 24.0 } else { f64::from(size) };

                let mut symbol_rotation = 0.0f32;
                if rotate_in_shader {
                    let offset_projected_point =
                        project_tile([a_pos[0] + translation[0] + 1.0, a_pos[1] + translation[1]]);
                    let a = [projected_point[0] / projected_point[3], projected_point[1] / projected_point[3]];
                    let b = [
                        offset_projected_point[0] / offset_projected_point[3],
                        offset_projected_point[1] / offset_projected_point[3],
                    ];
                    symbol_rotation = ((b[1] - a[1]) / aspect_ratio).atan2(b[0] - a[0]) as f32;
                }

                let angle_sin = f64::from((segment_angle + symbol_rotation).sin());
                let angle_cos = f64::from((segment_angle + symbol_rotation).cos());

                let translated_pos = [in_projected_pos[0] + translation[0], in_projected_pos[1] + translation[1]];
                let projected_pos = if along_line || has_variable_placement {
                    [in_projected_pos[0], in_projected_pos[1], 0.0, 1.0]
                } else if pitch_with_map {
                    matrix::transform_mat4_value(&[translated_pos[0], translated_pos[1], 0.0, 1.0], &label_plane_matrix)
                } else {
                    matrix::transform_mat4_value(&project_tile(translated_pos), &label_plane_matrix)
                };
                let pos0 = [projected_pos[0] / projected_pos[3], projected_pos[1] / projected_pos[3]];
                let font_scales = vec2_max_scalar(&a_min_font_scale, font_scale);
                let pos_offset = [
                    a_offset[0] * font_scales[0] / 32.0 + a_pxoffset[0] / 16.0,
                    a_offset[1] * font_scales[1] / 32.0 + a_pxoffset[1] / 16.0,
                ];
                // mat2{cos, -sin, sin, cos} * posOffset
                let rotated = [
                    pos_offset[0] * angle_cos + pos_offset[1] * angle_sin,
                    -pos_offset[0] * angle_sin + pos_offset[1] * angle_cos,
                ];

                // Pitched labels come back from the label plane in tile
                // units, scaled for their latitude on the globe.
                let mut projection_scaling = 1.0;
                if let Some(projector) = projector.as_ref().filter(|_| pitch_with_map) {
                    let anchor_tile = matrix::transform_mat4_value(&[pos0[0], pos0[1], 0.0, 1.0], &gl_coord_matrix);
                    projection_scaling = projector.pitched_text_correction(&Point::new(anchor_tile[0], anchor_tile[1]));
                }
                let mut out_pos = matrix::transform_mat4_value(
                    &[pos0[0] + rotated[0] * projection_scaling, pos0[1] + rotated[1] * projection_scaling, 0.0, 1.0],
                    &gl_coord_matrix,
                );
                if pitch_with_map {
                    out_pos = project_tile([out_pos[0] / out_pos[3], out_pos[1] / out_pos[3]]);
                }
                Some([out_pos[0] / out_pos[3], out_pos[1] / out_pos[3], out_pos[2] / out_pos[3]])
            };
            let identity = {
                let mut m: Mat4 = [0.0; 16];
                matrix::identity(&mut m);
                m
            };
            if let Some(bound) = compute_feature_ndc_bound(vertex_count, &identity, true, get_vertex) {
                stats.add_rendered_feature(feature_id, &bound, HashSet::from([overscaled_tile_id]));
            }
        }
    };

    let icons_visible = icon_opacity.constant_or(&1.0) > 0.0
        && (icon_color.constant_or(&IconColor::default_value()).a > 0.0
            || icon_halo_color.constant_or(&IconHaloColor::default_value()).a > 0.0);
    let text_visible = text_opacity.constant_or(&1.0) > 0.0
        && (text_color.constant_or(&TextColor::default_value()).a > 0.0
            || text_halo_color.constant_or(&TextHaloColor::default_value()).a > 0.0);

    if has_icon_data && icons_visible {
        compute_buffer_bounds(&bucket_state.icon, /* is_text = */ false);
    }
    if has_text_data && text_visible {
        compute_buffer_bounds(&bucket_state.text, /* is_text = */ true);
    }
    if has_sdf_icon_data && (text_visible || icons_visible) {
        compute_buffer_bounds(&bucket_state.sdf_icon, /* is_text = */ false);
    }
}

#[expect(
    clippy::too_many_arguments,
    reason = "the bucket's buffers and the layer's paint properties, and the attributes and uniforms they fill"
)]
fn update_tile_attributes(
    geometry: Option<&BucketGeometry>,
    bucket: &SymbolBucket,
    buffer: &Buffer,
    layer_id: &str,
    is_text: bool,
    paint_props: &PaintProperties,
    evaluated: &SymbolPaintProperties::PossiblyEvaluated,
    attribs: &mut VertexAttributeArray,
    properties_as_uniforms: &mut StringIDSetsPair,
) {
    set_binder_attributes(geometry, bucket, is_text, paint_props, evaluated, attribs, properties_as_uniforms);
    set_static_and_dynamic_attributes(geometry, bucket, buffer, layer_id, attribs);
}

/// The data-driven paint property half of `updateTileAttributes`.
fn set_binder_attributes(
    geometry: Option<&BucketGeometry>,
    bucket: &SymbolBucket,
    is_text: bool,
    paint_props: &PaintProperties,
    evaluated: &SymbolPaintProperties::PossiblyEvaluated,
    attribs: &mut VertexAttributeArray,
    properties_as_uniforms: &mut StringIDSetsPair,
) {
    let mut id = ID_SYMBOL_OPACITY_ATTRIBUTE;
    let pau = properties_as_uniforms;
    if is_text {
        let b = &paint_props.text_binders;
        let ib = &b.interleaved_vertex_buffer;
        let shared = paint_vertices(geometry, bucket, ib, b.depends_on_feature_state());
        read_data_driven_paint_property(
            attribs,
            &b.text_opacity,
            ib,
            &shared,
            &["a_opacity"],
            evaluated.get::<TextOpacity>().is_constant(),
            pau,
            &mut id,
        );
        read_data_driven_paint_property(
            attribs,
            &b.text_color,
            ib,
            &shared,
            &["a_fill_color"],
            evaluated.get::<TextColor>().is_constant(),
            pau,
            &mut id,
        );
        read_data_driven_paint_property(
            attribs,
            &b.text_halo_color,
            ib,
            &shared,
            &["a_halo_color"],
            evaluated.get::<TextHaloColor>().is_constant(),
            pau,
            &mut id,
        );
        read_data_driven_paint_property(
            attribs,
            &b.text_halo_width,
            ib,
            &shared,
            &["a_halo_width"],
            evaluated.get::<TextHaloWidth>().is_constant(),
            pau,
            &mut id,
        );
        read_data_driven_paint_property(
            attribs,
            &b.text_halo_blur,
            ib,
            &shared,
            &["a_halo_blur"],
            evaluated.get::<TextHaloBlur>().is_constant(),
            pau,
            &mut id,
        );
    } else {
        let b = &paint_props.icon_binders;
        let ib = &b.interleaved_vertex_buffer;
        let shared = paint_vertices(geometry, bucket, ib, b.depends_on_feature_state());
        read_data_driven_paint_property(
            attribs,
            &b.icon_opacity,
            ib,
            &shared,
            &["a_opacity"],
            evaluated.get::<IconOpacity>().is_constant(),
            pau,
            &mut id,
        );
        read_data_driven_paint_property(
            attribs,
            &b.icon_color,
            ib,
            &shared,
            &["a_fill_color"],
            evaluated.get::<IconColor>().is_constant(),
            pau,
            &mut id,
        );
        read_data_driven_paint_property(
            attribs,
            &b.icon_halo_color,
            ib,
            &shared,
            &["a_halo_color"],
            evaluated.get::<IconHaloColor>().is_constant(),
            pau,
            &mut id,
        );
        read_data_driven_paint_property(
            attribs,
            &b.icon_halo_width,
            ib,
            &shared,
            &["a_halo_width"],
            evaluated.get::<IconHaloWidth>().is_constant(),
            pau,
            &mut id,
        );
        read_data_driven_paint_property(
            attribs,
            &b.icon_halo_blur,
            ib,
            &shared,
            &["a_halo_blur"],
            evaluated.get::<IconHaloBlur>().is_constant(),
            pau,
            &mut id,
        );
    }
}

/// The vertex-buffer half of `updateTileAttributes`.
fn set_static_and_dynamic_attributes(
    geometry: Option<&BucketGeometry>,
    bucket: &SymbolBucket,
    buffer: &Buffer,
    layer_id: &str,
    attribs: &mut VertexAttributeArray,
) {
    set_layout_attributes(geometry, bucket, buffer, attribs);
    set_projected_pos_attribute(buffer, layer_id, attribs);
    set_fade_opacity_attribute(buffer, attribs);
}

/// The static half of `updateTileAttributes`: the layout vertices.
fn set_layout_attributes(
    geometry: Option<&BucketGeometry>,
    bucket: &SymbolBucket,
    buffer: &Buffer,
    attribs: &mut VertexAttributeArray,
) {
    let layout_size = size_of::<crate::buckets::symbol_bucket::SymbolLayoutVertex>() as u32;
    let layout_data = bucket_vertices(geometry, bucket, &buffer.attribute_data);
    if let Some(attr) = attribs.set(ID_SYMBOL_POS_OFFSET_ATTRIBUTE, -1, AttributeDataType::Invalid, 0) {
        attr.set_shared_raw_data(Some(Rc::clone(&layout_data)), 0, 0, layout_size, AttributeDataType::Short4);
    }
    if let Some(attr) = attribs.set(ID_SYMBOL_DATA_ATTRIBUTE, -1, AttributeDataType::Invalid, 0) {
        attr.set_shared_raw_data(Some(Rc::clone(&layout_data)), 8, 0, layout_size, AttributeDataType::UShort4);
    }
    if let Some(attr) = attribs.set(ID_SYMBOL_PIXEL_OFFSET_ATTRIBUTE, -1, AttributeDataType::Invalid, 0) {
        attr.set_shared_raw_data(Some(Rc::clone(&layout_data)), 16, 0, layout_size, AttributeDataType::Short4);
    }
    if let Some(attr) = attribs.set(ID_SYMBOL_TEX_ORIGIN_ATTRIBUTE, -1, AttributeDataType::Invalid, 0) {
        attr.set_shared_raw_data(Some(layout_data), 24, 0, layout_size, AttributeDataType::UShort2);
    }
}

/// Sets the dynamic layout vertices, which placement updates: the layer's
/// own when it has them.
fn set_projected_pos_attribute(buffer: &Buffer, layer_id: &str, attribs: &mut VertexAttributeArray) {
    if let Some(attr) = attribs.set(ID_SYMBOL_PROJECTED_POS_ATTRIBUTE, -1, AttributeDataType::Invalid, 0) {
        attr.set_shared_raw_data(
            Some(shared_vertices(buffer.dynamic_attribute_data_for(layer_id))),
            0,
            0,
            size_of::<crate::buckets::symbol_bucket::SymbolDynamicLayoutVertex>() as u32,
            AttributeDataType::Float3,
        );
    }
}

/// Sets the opacity vertices, which placement updates.
fn set_fade_opacity_attribute(buffer: &Buffer, attribs: &mut VertexAttributeArray) {
    if let Some(attr) = attribs.set(ID_SYMBOL_FADE_OPACITY_ATTRIBUTE, -1, AttributeDataType::Invalid, 0) {
        attr.set_shared_raw_data(
            Some(shared_vertices(&buffer.opacity_attribute_data)),
            0,
            0,
            size_of::<crate::buckets::symbol_bucket::SymbolOpacityVertex>() as u32,
            AttributeDataType::Float,
        );
    }
}

/// Which of a tile's drawable inputs changed since they were built.
#[derive(Clone, Copy, Debug)]
struct DrawableRefresh {
    opacity: bool,
    dynamic: bool,
    sort: bool,
    binders: bool,
}

impl DrawableRefresh {
    const ALL: DrawableRefresh = DrawableRefresh { opacity: true, dynamic: true, sort: true, binders: true };
}

/// Replaces the attributes (and indices) of an existing drawable that
/// placement, sorting or feature state changed. Halo and fill drawables share
/// their attribute array, which is refreshed once (`refreshed_attributes`).
fn update_tile_drawable(
    drawable: &mut dyn mln_gfx::Drawable,
    geometry: Option<&BucketGeometry>,
    bucket: &SymbolBucket,
    layer_id: &str,
    evaluated: &SymbolPaintProperties::PossiblyEvaluated,
    refresh: DrawableRefresh,
    refreshed_attributes: &mut Vec<*const ()>,
) {
    let Some(mut draw_data) = drawable.get_data().and_then(|d| d.downcast_ref::<SymbolDrawableData>()).copied() else {
        return;
    };
    let is_text = draw_data.symbol_type == SymbolType::Text;
    let sdf_icons = draw_data.symbol_type == SymbolType::IconSDF;

    let state = bucket.state();

    // This property can be set after the initial appearance of the tile, as
    // part of the layout process.
    if draw_data.bucket_variable_placement != state.has_variable_placement {
        draw_data.bucket_variable_placement = state.has_variable_placement;
        drawable.set_data(Some(Box::new(draw_data)));
    }

    let buffer = if is_text {
        &state.text
    } else if sdf_icons {
        &state.sdf_icon
    } else {
        &state.icon
    };

    // A drawable whose symbols are all hidden by placement draws nothing.
    // Its attributes aren't refreshed while it is disabled, so it refreshes
    // all of them when placement shows it again.
    let was_enabled = drawable.get_enabled();
    if refresh.opacity || refresh.sort {
        drawable.set_enabled(buffer.segment_has_visible_vertices(draw_data.segment));
    }
    if !drawable.get_enabled() {
        return;
    }
    let refresh = if was_enabled { refresh } else { DrawableRefresh::ALL };

    if !(refresh.opacity || refresh.dynamic || refresh.sort || refresh.binders) {
        return;
    }

    if let Some(attrs) = drawable.get_vertex_attributes().cloned() {
        let key = Rc::as_ptr(&attrs).cast::<()>();
        if !refreshed_attributes.contains(&key) {
            refreshed_attributes.push(key);
            let mut attrs = attrs.borrow_mut();
            if refresh.dynamic {
                set_projected_pos_attribute(buffer, layer_id, &mut attrs);
            }
            if refresh.opacity {
                set_fade_opacity_attribute(buffer, &mut attrs);
            }
            if refresh.binders
                && let Some(paint_props) = bucket.paint_properties.get(layer_id)
            {
                let paint_props = paint_props.read();
                let mut properties_as_uniforms = StringIDSetsPair::default();
                set_binder_attributes(
                    geometry,
                    bucket,
                    is_text,
                    &paint_props,
                    evaluated,
                    &mut attrs,
                    &mut properties_as_uniforms,
                );
            }
        }
    }

    if refresh.sort && !buffer.triangles.is_empty() {
        let segments: Vec<mln_gfx::drawable::UniqueDrawSegment> = match draw_data.segment {
            Some(i) => buffer.segments.get(i).map(|s| vec![s]).unwrap_or_default(),
            None => buffer.segments.iter().collect(),
        }
        .into_iter()
        .map(|segment| Box::new(mln_gfx::drawable::DrawSegment::new(DrawMode::Triangles(), segment.copy())))
        .collect();
        drawable.set_index_data(Some(released_indexes(&buffer.triangles)), segments);
    }
}

/// A `SegmentGroup` (a `RenderableSegment` plus its segments).
struct SegmentGroup {
    sort_key: f32,
    type_: SymbolType,
    tile_index: usize,
    tile_id: UnwrappedTileID,
    overscaled_z: u8,
    /// One segment (`sortFeaturesByKey`) or all segments of the buffer.
    segment: Option<usize>,
}

impl SegmentGroup {
    fn less(lhs: &SegmentGroup, rhs: &SegmentGroup) -> bool {
        // Sort renderable segments by a sort key.
        if lhs.sort_key < rhs.sort_key {
            return true;
        }

        // In cases when sort key is the same, sort by the type of a segment
        // (text over icons), and for segments of the same type with the same
        // sort key, sort by a tile id.
        if lhs.sort_key == rhs.sort_key {
            if lhs.type_ != SymbolType::Text && rhs.type_ == SymbolType::Text {
                return true;
            }

            if lhs.type_ == rhs.type_ {
                return lhs.tile_id < rhs.tile_id;
            }
        }

        false
    }
}

pub(crate) struct RenderSymbolLayer {
    base: RenderLayerBase,
    unevaluated: SymbolPaintProperties::Unevaluated,
    has_format_section_overrides: bool,

    icon_size: f32,
    text_size: f32,

    symbol_icon_group: Option<ShaderGroupPtr>,
    symbol_sdf_group: Option<ShaderGroupPtr>,
    symbol_text_and_icon_group: Option<ShaderGroupPtr>,

    /// The bucket version each tile's drawables were built from.
    tile_bucket_versions: HashMap<OverscaledTileID, TileVersions>,

    collision_box_group: Option<ShaderGroupPtr>,
    collision_circle_group: Option<ShaderGroupPtr>,
    collision_tile_layer_group: Option<LayerGroupBasePtr>,
    collision_layer_tweaker: Option<LayerTweakerPtr>,
}

impl RenderSymbolLayer {
    pub(crate) fn new(impl_: Arc<SymbolLayerImpl>) -> Self {
        let unevaluated = impl_.paint.untransitioned();
        let properties: Arc<dyn LayerProperties> = Arc::new(SymbolLayerProperties::new(impl_));
        let mut base = RenderLayerBase::new(properties);
        base.style_dependencies = unevaluated.get_dependencies();
        RenderSymbolLayer {
            base,
            unevaluated,
            has_format_section_overrides: false,
            icon_size: 1.0,
            text_size: 16.0,
            symbol_icon_group: None,
            symbol_sdf_group: None,
            symbol_text_and_icon_group: None,
            tile_bucket_versions: HashMap::new(),
            collision_box_group: None,
            collision_circle_group: None,
            collision_tile_layer_group: None,
            collision_layer_tweaker: None,
        }
    }

    /// Removes the tile's symbol and collision drawables.
    fn remove_tile_all(&mut self, render_pass: RenderPass, tile_id: &OverscaledTileID) {
        self.remove_tile(render_pass, tile_id);
        if let Some(group) = &self.collision_tile_layer_group {
            let mut group = group.borrow_mut();
            if let Some(tile_group) = group.as_tile_layer_group_mut() {
                tile_group.remove_drawables(render_pass, tile_id);
            }
        }
    }

    fn get_collision_tile_layer_group(
        &mut self,
        context: &dyn Context,
        changes: &mut UniqueChangeRequestVec,
    ) -> Option<LayerGroupBasePtr> {
        if self.collision_tile_layer_group.is_none() {
            self.collision_tile_layer_group = context.create_tile_layer_group(
                self.base.layer_index,
                /*initial_capacity=*/ 64,
                format!("{}-collision", self.get_id()),
            );
            if self.collision_tile_layer_group.is_some() {
                activate_layer_group(self.collision_tile_layer_group.as_ref(), true, changes);
            }
        }
        let group = self.collision_tile_layer_group.clone()?;
        if self.collision_layer_tweaker.is_none() {
            let tweaker: LayerTweakerPtr = Rc::new(RefCell::new(CollisionLayerTweaker::new(
                self.get_id().to_owned(),
                Arc::clone(&self.base.evaluated_properties),
            )));
            self.collision_layer_tweaker = Some(Rc::clone(&tweaker));
            changes.push(ChangeRequest::AddLayerTweaker(Rc::clone(&group), Rc::downgrade(&tweaker)));
        }
        Some(group)
    }

    /// The `addCollisionDrawables(isText, hasCollisionBox,
    /// hasCollisionCircle)` lambda of `update`.
    #[expect(
        clippy::too_many_arguments,
        reason = "the tile's bucket and paint, and the context, passes and change list that the drawables go to"
    )]
    fn add_collision_drawables(
        &mut self,
        context: &dyn Context,
        changes: &mut UniqueChangeRequestVec,
        passes: RenderPass,
        tile_id: OverscaledTileID,
        bucket: &SymbolBucket,
        evaluated: &SymbolPaintProperties::PossiblyEvaluated,
        is_text: bool,
    ) {
        let (has_collision_box, has_collision_circle) = if is_text {
            (bucket.has_text_collision_box_data(), bucket.has_text_collision_circle_data())
        } else {
            (bucket.has_icon_collision_box_data(), bucket.has_icon_collision_circle_data())
        };
        if !has_collision_box && !has_collision_circle {
            return;
        }

        let Some(collision_group) = self.get_collision_tile_layer_group(context, changes) else {
            return;
        };

        let values = if is_text {
            text_property_values(evaluated, &bucket.layout)
        } else {
            icon_property_values(evaluated, &bucket.layout)
        };
        let suffix = if is_text { "text/" } else { "icon/" };
        let layer_collision_prefix = format!("{}-collision/", self.get_id());

        let mut collision_builder = context.create_drawable_builder();
        collision_builder.set_sub_layer_index(0);
        collision_builder.set_enable_depth(false);
        collision_builder.set_render_pass(passes);
        collision_builder.set_cull_face_mode(&CullFaceMode::disabled());
        collision_builder.set_color_mode(&ColorMode::alpha_blended());

        let state = bucket.state();
        let mut add = |buffer: &CollisionBuffer, group: &Option<ShaderGroupPtr>, mode: DrawMode, name: &str| {
            let Some(group) = group else {
                return;
            };
            let Some(shader) =
                group.get_or_create_shader(context, &Default::default(), self.base.projection_variant, "")
            else {
                return;
            };
            collision_builder.set_drawable_name(format!("{layer_collision_prefix}{suffix}{name}"));
            collision_builder.set_shader(Some(shader));
            collision_builder.set_raw_vertices(Vec::new(), buffer.vertices.len(), AttributeDataType::Short2);
            collision_builder.set_vertex_attributes(Some(get_collision_vertex_attributes(context, buffer)));
            collision_builder.set_segments_shared(mode, Some(released_indexes(&buffer.indices)), &buffer.segments);
            collision_builder.flush(context);
        };

        if has_collision_box {
            let collision_box = if is_text { &state.text_collision_box } else { &state.icon_collision_box };
            if let Some(collision_box) = collision_box {
                add(collision_box, &self.collision_box_group, DrawMode::Lines(1.0), "box");
            }
        }
        if has_collision_circle {
            let collision_circle = if is_text { &state.text_collision_circle } else { &state.icon_collision_circle };
            if let Some(collision_circle) = collision_circle {
                add(collision_circle, &self.collision_circle_group, DrawMode::Triangles(), "circle");
            }
        }
        drop(state);

        // add drawables to layer group
        let tweaker = self.collision_layer_tweaker.clone();
        let mut group = collision_group.borrow_mut();
        let Some(tile_group) = group.as_tile_layer_group_mut() else {
            return;
        };
        for mut drawable in collision_builder.clear_drawables() {
            drawable.set_tile_id(tile_id);
            drawable.set_layer_tweaker(tweaker.as_ref().and_then(layer_tweaker_slot));
            drawable.set_data(Some(Box::new(CollisionDrawableData {
                translate: values.translate,
                translate_anchor: values.translate_anchor,
            })));
            tile_group.add_drawable(passes, &tile_id, drawable);
        }
    }
}

fn get_collision_vertex_attributes(
    context: &dyn Context,
    buffer: &CollisionBuffer,
) -> mln_gfx::vertex_attribute::VertexAttributeArrayPtr {
    let vertex_attrs = context.create_vertex_attribute_array();
    {
        let mut attrs = vertex_attrs.borrow_mut();
        let layout_stride = size_of::<CollisionBoxLayoutVertex>() as u32;
        let layout = shared_vertices(&buffer.vertices);
        let dynamic_stride = size_of::<CollisionBoxDynamicVertex>() as u32;
        let dynamic = shared_vertices(&buffer.dynamic_vertices);
        if let Some(attr) = attrs.set(ID_COLLISION_POS_VERTEX_ATTRIBUTE, -1, AttributeDataType::Invalid, 0) {
            attr.set_shared_raw_data(Some(Rc::clone(&layout)), 0, 0, layout_stride, AttributeDataType::Short2);
        }
        if let Some(attr) = attrs.set(ID_COLLISION_ANCHOR_POS_VERTEX_ATTRIBUTE, -1, AttributeDataType::Invalid, 0) {
            attr.set_shared_raw_data(Some(Rc::clone(&layout)), 4, 0, layout_stride, AttributeDataType::Short2);
        }
        if let Some(attr) = attrs.set(ID_COLLISION_EXTRUDE_VERTEX_ATTRIBUTE, -1, AttributeDataType::Invalid, 0) {
            attr.set_shared_raw_data(Some(layout), 8, 0, layout_stride, AttributeDataType::Short2);
        }
        if let Some(attr) = attrs.set(ID_COLLISION_PLACED_VERTEX_ATTRIBUTE, -1, AttributeDataType::Invalid, 0) {
            attr.set_shared_raw_data(Some(Rc::clone(&dynamic)), 0, 0, dynamic_stride, AttributeDataType::UShort2);
        }
        if let Some(attr) = attrs.set(ID_COLLISION_SHIFT_VERTEX_ATTRIBUTE, -1, AttributeDataType::Invalid, 0) {
            attr.set_shared_raw_data(Some(dynamic), 4, 0, dynamic_stride, AttributeDataType::Float2);
        }
    }
    vertex_attrs
}

impl RenderLayer for RenderSymbolLayer {
    fn base(&self) -> &RenderLayerBase {
        &self.base
    }

    fn base_mut(&mut self) -> &mut RenderLayerBase {
        &mut self.base
    }

    fn transition(&mut self, parameters: &TransitionParameters) {
        let prior = std::mem::take(&mut self.unevaluated);
        self.unevaluated = impl_cast(&self.base.base_impl).paint.transitioned(parameters, prior);
        self.has_format_section_overrides =
            SymbolLayerPaintPropertyOverrides::has_overrides(impl_cast(&self.base.base_impl).layout.get::<TextField>());
        self.base.style_dependencies = self.unevaluated.get_dependencies();
    }

    fn evaluate(&mut self, parameters: &PropertyEvaluationParameters) {
        let previous_evaluated = get_evaluated::<SymbolLayerProperties>(&self.base.evaluated_properties).clone();
        let mut properties = SymbolLayerProperties::with_evaluated(
            downcast_arc::<SymbolLayerImpl>(&self.base.base_impl),
            self.unevaluated.evaluate_with_previous(parameters, &previous_evaluated),
        );

        if self.has_format_section_overrides {
            mln_style::format_section_override::set_overrides(
                &impl_cast(&self.base.base_impl).layout,
                &mut properties.evaluated,
                &self.unevaluated,
                &parameters.global_state,
            );
        }

        let evaluated = &properties.evaluated;
        let has_icon_opacity = evaluated.get::<IconColor>().constant_or(&IconColor::default_value()).a > 0.0
            || evaluated.get::<IconHaloColor>().constant_or(&IconHaloColor::default_value()).a > 0.0;
        let has_text_opacity = evaluated.get::<TextColor>().constant_or(&TextColor::default_value()).a > 0.0
            || evaluated.get::<TextHaloColor>().constant_or(&TextHaloColor::default_value()).a > 0.0;

        self.base.passes =
            if (evaluated.get::<IconOpacity>().constant_or(&1.0) > 0.0 && has_icon_opacity && self.icon_size > 0.0)
                || (evaluated.get::<TextOpacity>().constant_or(&1.0) > 0.0 && has_text_opacity && self.text_size > 0.0)
            {
                RenderPass::Translucent
            } else {
                RenderPass::None
            };
        properties.base.render_passes = self.base.passes.underlying_type();
        self.base.evaluated_properties = Arc::new(properties);

        // The symbol tweaker supports updating properties. When the style
        // changes, it will be replaced in `RenderLayer::layerChanged`
        if let Some(layer_tweaker) = &self.base.layer_tweaker {
            layer_tweaker.borrow_mut().update_properties(Arc::clone(&self.base.evaluated_properties));
        }
    }

    fn has_transition(&self) -> bool {
        self.unevaluated.has_transition()
    }

    fn has_crossfade(&self) -> bool {
        false
    }

    fn mark_layer_renderable(&mut self, will_render: bool, changes: &mut UniqueChangeRequestVec) {
        crate::render_layer::mark_layer_renderable_base(self, will_render, changes);
        if self.collision_tile_layer_group.is_some() {
            activate_layer_group(self.collision_tile_layer_group.as_ref(), will_render, changes);
        }
    }

    fn layer_removed(&mut self, changes: &mut UniqueChangeRequestVec) {
        // RenderLayer::layerRemoved
        self.remove_all_drawables();
        let group = self.base.layer_group.clone();
        activate_layer_group(group.as_ref(), false, changes);
        if self.collision_tile_layer_group.is_some() {
            activate_layer_group(self.collision_tile_layer_group.as_ref(), false, changes);
        }
    }

    fn layer_index_changed(&mut self, new_layer_index: i32, changes: &mut UniqueChangeRequestVec) {
        // RenderLayer::layerIndexChanged
        self.base.layer_index = new_layer_index;
        let group = self.base.layer_group.clone();
        change_layer_index(group.as_ref(), new_layer_index, changes);

        change_layer_index(self.collision_tile_layer_group.as_ref(), new_layer_index, changes);
    }

    fn remove_all_drawables(&mut self) {
        if let Some(group) = &self.base.layer_group {
            group.borrow_mut().clear_drawables();
        }
        if let Some(group) = &self.collision_tile_layer_group {
            group.borrow_mut().clear_drawables();
        }
        self.tile_bucket_versions.clear();
    }

    fn prepare(&mut self, params: &mut LayerPrepareParameters<'_>) {
        let Some(source) = params.source else {
            self.base.render_tiles = None;
            self.base.render_tile_ids.clear();
            return;
        };
        self.base.render_tiles = source.get_render_tiles_sorted_by_y_position();
        self.base.render_tiles_owner = source.get_raw_render_tiles();

        self.update_render_tile_ids();
        if self.base.render_tiles.is_some() {
            self.add_render_passes_from_tiles();
        }

        self.base.placement_data.clear();
        self.base.follower_data.clear();

        let evaluated = &get_evaluated::<SymbolLayerProperties>(&self.base.evaluated_properties);
        let text_translate = SymbolTranslate {
            offset: *evaluated.get::<TextTranslate>(),
            anchor: *evaluated.get::<TextTranslateAnchor>(),
        };
        let icon_translate = SymbolTranslate {
            offset: *evaluated.get::<IconTranslate>(),
            anchor: *evaluated.get::<IconTranslateAnchor>(),
        };

        let Some(render_tiles) = self.base.render_tiles.clone() else {
            return;
        };
        let owner = Rc::clone(render_tiles.owner());
        for render_tile in render_tiles.iter() {
            let Some(bucket_ptr) = render_tile.get_bucket(&*self.base.base_impl) else {
                continue;
            };
            let Some(bucket) = (**bucket_ptr).downcast_ref::<SymbolBucket>() else {
                continue;
            };
            let Some(tile_index) = owner.iter().position(|t| std::ptr::eq(t, render_tile)) else {
                continue;
            };
            if bucket.bucket_leader_id != self.get_id() {
                self.base.follower_data.push(BucketPlacementData {
                    bucket: Arc::clone(bucket_ptr),
                    tile_owner: Rc::clone(&owner),
                    tile_index,
                    feature_index: None,
                    source_id: self.base.base_impl.source.clone(),
                    sort_key_range: None,
                    text_translate,
                    icon_translate,
                });
                continue;
            }
            // Only place this layer if it's the "group leader" for the bucket
            let feature_index = render_tile.get_feature_index();

            if bucket.sort_key_ranges.is_empty() {
                self.base.placement_data.push(BucketPlacementData {
                    bucket: Arc::clone(bucket_ptr),
                    tile_owner: Rc::clone(&owner),
                    tile_index,
                    feature_index,
                    source_id: self.base.base_impl.source.clone(),
                    sort_key_range: None,
                    text_translate,
                    icon_translate,
                });
            } else {
                for sort_key_range in &bucket.sort_key_ranges {
                    let layer_data = BucketPlacementData {
                        bucket: Arc::clone(bucket_ptr),
                        tile_owner: Rc::clone(&owner),
                        tile_index,
                        feature_index: feature_index.clone(),
                        source_id: self.base.base_impl.source.clone(),
                        sort_key_range: Some(*sort_key_range),
                        text_translate,
                        icon_translate,
                    };
                    if self.base.placement_data.last().is_none_or(|last| {
                        last.sort_key_range.is_some_and(|last| last.sort_key <= sort_key_range.sort_key)
                    }) {
                        // Already in order, as ranges with a constant sort key always are.
                        self.base.placement_data.push(layer_data);
                        continue;
                    }
                    // std::upper_bound by sort key
                    let sort_position = self.base.placement_data.partition_point(|lhs| {
                        let key = lhs.sort_key_range.expect("sort key range").sort_key;
                        sort_key_range.sort_key.partial_cmp(&key) != Some(std::cmp::Ordering::Less)
                    });
                    self.base.placement_data.insert(sort_position, layer_data);
                }
            }
        }
    }

    fn update(
        &mut self,
        shaders: &ShaderRegistry,
        context: &dyn Context,
        state: &TransformState,
        update_parameters: &Arc<UpdateParameters>,
        _paint_parameters: &PaintParameters<'_>,
        render_tree: &RenderTree,
        changes: &mut UniqueChangeRequestVec,
    ) {
        /// The tweakers built for one tile.
        #[derive(Default)]
        struct TileInfo {
            text_tweaker: Option<DrawableTweakerPtr>,
            icon_tweaker: Option<DrawableTweakerPtr>,
        }

        self.update_projection_variant(state);
        self.base.stats.rendered_features.clear();
        let passes = self.base.passes;
        let Some(render_tiles) = self.base.render_tiles.clone().filter(|t| !t.is_empty()) else {
            self.remove_all_drawables();
            return;
        };
        if passes == RenderPass::None {
            self.remove_all_drawables();
            return;
        }

        // Set up a layer group
        if self.base.layer_group.is_none()
            && let Some(layer_group) = context.create_tile_layer_group(
                self.base.layer_index,
                /*initial_capacity=*/ 64,
                self.get_id().to_owned(),
            )
        {
            self.set_layer_group(Some(layer_group), changes);
        }
        let Some(layer_group) = self.base.layer_group.clone() else {
            return;
        };

        if self.base.layer_tweaker.is_none() {
            let tweaker: LayerTweakerPtr = Rc::new(RefCell::new(SymbolLayerTweaker::new(
                self.get_id().to_owned(),
                Arc::clone(&self.base.evaluated_properties),
            )));
            self.base.layer_tweaker = Some(Rc::clone(&tweaker));
            changes.push(ChangeRequest::AddLayerTweaker(Rc::clone(&layer_group), Rc::downgrade(&tweaker)));
        }

        if self.symbol_icon_group.is_none() {
            self.symbol_icon_group = shaders.get_shader_group(SYMBOL_ICON_SHADER_NAME);
        }
        if self.symbol_sdf_group.is_none() {
            self.symbol_sdf_group = shaders.get_shader_group(SYMBOL_SDF_SHADER_NAME);
        }
        if self.symbol_text_and_icon_group.is_none() {
            self.symbol_text_and_icon_group = shaders.get_shader_group(SYMBOL_TEXT_AND_ICON_SHADER_NAME);
        }
        if self.collision_box_group.is_none() {
            self.collision_box_group = shaders.get_shader_group("CollisionBoxShader");
        }
        if self.collision_circle_group.is_none() {
            self.collision_circle_group = shaders.get_shader_group("CollisionCircleShader");
        }

        // remove drawables that are dropped out of scope
        {
            let mut group = layer_group.borrow_mut();
            let tile_layer_group = group.as_tile_layer_group_mut().expect("symbol layer group is a TileLayerGroup");
            let base = &self.base;
            tile_layer_group.remove_drawables_if(|drawable| {
                // If the render pass has changed or the tile has dropped out
                // of the cover set, remove it.
                drawable.get_render_pass() != passes
                    || drawable.get_tile_id().is_some_and(|id| !base.render_tile_ids.contains_key(&id))
            });
        }
        if let Some(collision_group) = self.collision_tile_layer_group.clone() {
            let mut group = collision_group.borrow_mut();
            if let Some(tile_layer_group) = group.as_tile_layer_group_mut() {
                let base = &self.base;
                tile_layer_group.remove_drawables_if(|drawable| {
                    drawable.get_render_pass() != passes
                        || drawable.get_tile_id().is_some_and(|id| !base.render_tile_ids.contains_key(&id))
                });
            }
        }
        let render_tile_ids = &self.base.render_tile_ids;
        self.tile_bucket_versions.retain(|id, _| render_tile_ids.contains_key(id));

        let layout = &impl_cast(&self.base.base_impl).layout;
        let sort_features_by_key = layout.get::<SymbolSortKey>().is_expression();
        let constant_sort_key = layout.get::<SymbolSortKey>().is_constant();
        // Screen-space symbols are drawn over the view, not projected onto
        // the map.
        let screen_space_prop = layout.get::<SymbolScreenSpace>();
        let is_screen_space = if screen_space_prop.is_constant() {
            *screen_space_prop.as_constant()
        } else {
            SymbolScreenSpace::default_value()
        };
        let symbol_projection_variant =
            if is_screen_space { ProjectionVariant::Mercator } else { self.base.projection_variant };
        let is_offset = !layout.get::<IconOffset>().is_undefined();

        let current_zoom = state.get_zoom() as f32;
        let layer_prefix = format!("{}/", self.get_id());
        let layer_tweaker = self.base.layer_tweaker.clone();

        let mut renderable_segments: Vec<SegmentGroup> = Vec::new();

        let tiles: Vec<&RenderTile> = render_tiles.iter().collect();
        for (tile_index, tile) in tiles.iter().enumerate() {
            let tile_id = *tile.get_overscaled_tile_id();

            let base_impl = Arc::clone(&self.base.base_impl);
            let render_data = super::render_data_for_pass(tile, &*base_impl, passes);
            let Some(render_data) = render_data.filter(|r| r.bucket.has_data()) else {
                self.remove_tile_all(passes, &tile_id);
                self.tile_bucket_versions.remove(&tile_id);
                continue;
            };
            let Some(bucket) = (*render_data.bucket).downcast_ref::<SymbolBucket>() else {
                continue;
            };

            let prev_bucket_id = self.get_render_tile_bucket_id(&tile_id);
            if prev_bucket_id != SimpleIdentity::EMPTY && prev_bucket_id != bucket.base.bucket_id {
                // This tile was previously set up from a different bucket,
                // drop and re-create any drawables for it.
                self.remove_tile_all(passes, &tile_id);
                self.tile_bucket_versions.remove(&tile_id);
            }
            self.set_render_tile_bucket_id(&tile_id, bucket.base.bucket_id);
            let current_versions = TileVersions::of(bucket);
            let seen_versions = self.tile_bucket_versions.insert(tile_id, current_versions);
            // What changed since the drawables were built or last refreshed
            // (unknown: refresh everything).
            let refresh = match seen_versions {
                Some(seen) => DrawableRefresh {
                    opacity: seen.opacity != current_versions.opacity,
                    dynamic: seen.dynamic != current_versions.dynamic,
                    sort: seen.sort != current_versions.sort,
                    binders: seen.binder != current_versions.binder,
                },
                None => DrawableRefresh::ALL,
            };

            debug_assert!(bucket.paint_properties.contains_key(self.get_id()));
            if !bucket.paint_properties.contains_key(self.get_id()) {
                continue;
            }

            let evaluated = get_evaluated::<SymbolLayerProperties>(&render_data.layer_properties);

            if update_parameters.capture_rendered_features {
                let params = &render_tree.get_parameters().transform_params;
                capture_rendered_features(
                    &mut self.base.stats,
                    &self.base.base_impl.id,
                    tile,
                    bucket,
                    evaluated,
                    state,
                    params,
                    is_screen_space,
                    is_offset,
                );
            }

            // If we already have drawables for this tile, update them.
            let layer_id = self.get_id().to_owned();
            let mut refreshed_attributes: Vec<*const ()> = Vec::new();
            let update_existing = |drawable: &mut dyn mln_gfx::Drawable| {
                if !super::drawable_has_layer_tweaker(drawable, layer_tweaker.as_ref()) {
                    // This drawable was produced on a previous style/bucket,
                    // and should not be updated.
                    return false;
                }
                update_tile_drawable(
                    drawable,
                    tile.get_bucket_geometry(),
                    bucket,
                    &layer_id,
                    evaluated,
                    refresh,
                    &mut refreshed_attributes,
                );
                true
            };
            if self.update_tile(passes, &tile_id, update_existing) {
                // re-create collision drawables
                if let Some(group) = &self.collision_tile_layer_group {
                    let mut group = group.borrow_mut();
                    if let Some(tile_group) = group.as_tile_layer_group_mut() {
                        tile_group.remove_drawables(passes, &tile_id);
                    }
                }
                self.add_collision_drawables(context, changes, passes, tile_id, bucket, evaluated, false);
                self.add_collision_drawables(context, changes, passes, tile_id, bucket, evaluated, true);
                continue;
            }

            let mut serial_key = 1.0_f32;
            let state_guard = bucket.state();
            // A bucket laid out before a layout change can still have the
            // previous sort-key expression until it is rebuilt.
            let sort_segments =
                sort_features_by_key || (constant_sort_key && !bucket.layout.get::<SymbolSortKey>().is_constant());
            let mut add_renderables = |buffer: &Buffer, type_: SymbolType| {
                if sort_segments {
                    // Features need to be rendered in a specific order, so we
                    // add each segment individually
                    for (i, segment) in buffer.segments.iter().enumerate() {
                        renderable_segments.push(SegmentGroup {
                            sort_key: segment.sort_key,
                            type_,
                            tile_index,
                            tile_id: tile.id,
                            overscaled_z: tile_id.overscaled_z,
                            segment: Some(i),
                        });
                    }
                } else if !buffer.segments.is_empty() {
                    // Features can be rendered in the order produced, and as
                    // grouped by the bucket. With an explicit constant key,
                    // the segment's key keeps text above icons across tiles.
                    let sort_key = if constant_sort_key { buffer.segments[0].sort_key } else { serial_key };
                    renderable_segments.push(SegmentGroup {
                        sort_key,
                        type_,
                        tile_index,
                        tile_id: tile.id,
                        overscaled_z: tile_id.overscaled_z,
                        segment: None,
                    });
                    serial_key += 1.0;
                }
            };

            let Some(atlases) = tile.get_atlas_textures() else {
                continue;
            };
            let has_icon_texture = atlases.icon.borrow().is_some();
            let has_glyph_texture = atlases.glyph.borrow().is_some();

            if !state_guard.icon.segments.is_empty() && has_icon_texture {
                add_renderables(&state_guard.icon, SymbolType::IconRGBA);
            }
            if !state_guard.sdf_icon.segments.is_empty() && has_icon_texture {
                add_renderables(&state_guard.sdf_icon, SymbolType::IconSDF);
            }
            if !state_guard.text.segments.is_empty() && has_glyph_texture {
                add_renderables(&state_guard.text, SymbolType::Text);
            }

            drop(state_guard);
            self.add_collision_drawables(context, changes, passes, tile_id, bucket, evaluated, false);
            self.add_collision_drawables(context, changes, passes, tile_id, bucket, evaluated, true);
        }

        // std::multiset<SegmentGroup>: a stable sort keeps equal elements in
        // insertion order, as multiset::emplace does.
        renderable_segments.sort_by(|a, b| {
            if SegmentGroup::less(a, b) {
                std::cmp::Ordering::Less
            } else if SegmentGroup::less(b, a) {
                std::cmp::Ordering::Greater
            } else {
                std::cmp::Ordering::Equal
            }
        });

        // We'll be processing renderables across tiles, potentially
        // out-of-order, so keep track of some things by tile ID so we don't
        // have to re-build them multiple times.
        let mut tile_cache: HashMap<UnwrappedTileID, TileInfo> = HashMap::new();
        let mut builder: Option<mln_gfx::UniqueDrawableBuilder> = None;
        let mut properties_as_uniforms: StringIDSetsPair = Default::default();

        for group in &renderable_segments {
            let is_text = group.type_ == SymbolType::Text;
            let sdf_icons = group.type_ == SymbolType::IconSDF;

            let tile = tiles[group.tile_index];
            let tile_id = tile.id.overscale_to(group.overscaled_z);
            let base_impl = Arc::clone(&self.base.base_impl);
            let Some(render_data) = super::render_data_for_pass(tile, &*base_impl, passes) else {
                continue;
            };
            let Some(bucket) = (*render_data.bucket).downcast_ref::<SymbolBucket>() else {
                continue;
            };
            let state_guard = bucket.state();
            let buffer = if is_text {
                &state_guard.text
            } else if sdf_icons {
                &state_guard.sdf_icon
            } else {
                &state_guard.icon
            };

            if buffer.triangles.is_empty() {
                continue;
            }
            let visible = buffer.segment_has_visible_vertices(group.segment);

            let evaluated = get_evaluated::<SymbolLayerProperties>(&render_data.layer_properties);
            let Some(bucket_paint_properties) = bucket.paint_properties.get(self.get_id()) else {
                continue;
            };
            let bucket_paint_properties = bucket_paint_properties.read();

            let bucket_layout = &*bucket.layout;
            let values = if is_text {
                text_property_values(evaluated, bucket_layout)
            } else {
                icon_property_values(evaluated, bucket_layout)
            };

            let Some(atlases) = tile.get_atlas_textures() else {
                debug_assert!(false);
                continue;
            };

            let tile_info = tile_cache.entry(tile.id).or_default();

            properties_as_uniforms.0.clear();
            properties_as_uniforms.1.clear();

            let vertex_attribs = context.create_vertex_attribute_array();
            update_tile_attributes(
                tile.get_bucket_geometry(),
                bucket,
                buffer,
                self.get_id(),
                is_text,
                &bucket_paint_properties,
                evaluated,
                &mut vertex_attribs.borrow_mut(),
                &mut properties_as_uniforms,
            );

            let text_halo = evaluated.get::<TextHaloColor>().constant_or(&Color::black()).a > 0.0
                && evaluated.get::<TextHaloWidth>().constant_or(&1.0) != 0.0;
            let text_fill = evaluated.get::<TextColor>().constant_or(&Color::black()).a > 0.0;

            let icon_halo = evaluated.get::<IconHaloColor>().constant_or(&Color::black()).a > 0.0
                && evaluated.get::<IconHaloWidth>().constant_or(&1.0) != 0.0;
            let icon_fill = evaluated.get::<IconColor>().constant_or(&Color::black()).a > 0.0;

            if let Some(builder) = builder.as_mut() {
                builder.clear_tweakers();
            }

            let mut draw = |shader_group: &Option<ShaderGroupPtr>, is_halo: bool, suffix: &str| {
                let Some(shader_group) = shader_group else {
                    return;
                };

                // We can use the same tweakers for all the segments in a tile
                if is_text && tile_info.text_tweaker.is_none() {
                    let text_size_is_zoom_constant =
                        bucket.text_size_binder.evaluate_for_zoom(current_zoom).is_zoom_constant;
                    tile_info.text_tweaker = Some(Rc::new(DrawableAtlasesTweaker::new(
                        Some(Rc::clone(atlases)),
                        Some(ID_SYMBOL_IMAGE_ICON_TEXTURE),
                        Some(ID_SYMBOL_IMAGE_TEXTURE),
                        is_text,
                        false,
                        values.rotation_alignment,
                        false,
                        text_size_is_zoom_constant,
                    )));
                }
                if !is_text && tile_info.icon_tweaker.is_none() {
                    let icon_scaled =
                        bucket_layout.get::<IconSize>().constant_or(&1.0) != 1.0 || bucket.icons_need_linear;
                    tile_info.icon_tweaker = Some(Rc::new(DrawableAtlasesTweaker::new(
                        Some(Rc::clone(atlases)),
                        Some(ID_SYMBOL_IMAGE_ICON_TEXTURE),
                        Some(ID_SYMBOL_IMAGE_TEXTURE),
                        is_text,
                        sdf_icons,
                        values.rotation_alignment,
                        icon_scaled,
                        false,
                    )));
                }

                let builder = builder.get_or_insert_with(|| {
                    let mut builder = context.create_drawable_builder();
                    builder.set_sub_layer_index(0);
                    builder.set_render_pass(passes);
                    builder.set_cull_face_mode(&CullFaceMode::disabled());
                    builder.set_depth_type(DepthMaskType::ReadOnly);
                    builder.set_color_mode(&if (passes.underlying_type() & RenderPass::Translucent.underlying_type())
                        != 0
                    {
                        ColorMode::alpha_blended()
                    } else {
                        ColorMode::unblended()
                    });
                    builder
                });

                let Some(shader) = shader_group.get_or_create_shader(
                    context,
                    &properties_as_uniforms,
                    symbol_projection_variant,
                    POS_OFFSET_ATTRIB_NAME,
                ) else {
                    return;
                };
                builder.set_shader(Some(shader));

                builder.clear_tweakers();
                let tweaker = if is_text { tile_info.text_tweaker.clone() } else { tile_info.icon_tweaker.clone() };
                if let Some(tweaker) = tweaker {
                    builder.add_tweaker(tweaker);
                }
                builder.set_drawable_name(format!("{layer_prefix}{suffix}"));
                builder.set_vertex_attributes(Some(Rc::clone(&vertex_attribs)));

                builder.set_raw_vertices(Vec::new(), buffer.attribute_data.len(), AttributeDataType::Short4);

                let triangle_indices = Some(released_indexes(&buffer.triangles));
                match group.segment {
                    Some(i) => builder.set_segments_shared(
                        DrawMode::Triangles(),
                        triangle_indices,
                        std::slice::from_ref(&buffer.segments[i]),
                    ),
                    None => builder.set_segments_shared(DrawMode::Triangles(), triangle_indices, &buffer.segments),
                }

                builder.flush(context);

                let mut layer_group_ref = layer_group.borrow_mut();
                let tile_layer_group = layer_group_ref.as_tile_layer_group_mut().expect("TileLayerGroup");
                for mut drawable in builder.clear_drawables() {
                    drawable.set_tile_id(tile_id);
                    drawable.set_layer_tweaker(layer_tweaker.as_ref().and_then(layer_tweaker_slot));
                    drawable.set_render_tile(drawable_render_tile_slot(render_tiles.owner(), tile));
                    drawable.set_binders(drawable_binders_slot(&render_data.bucket, &base_impl.id));

                    let has_variable_placement = state_guard.has_variable_placement;
                    drawable.set_data(Some(Box::new(SymbolDrawableData {
                        is_halo,
                        bucket_variable_placement: has_variable_placement,
                        symbol_type: group.type_,
                        pitch_alignment: values.pitch_alignment,
                        rotation_alignment: values.rotation_alignment,
                        placement: *bucket_layout.get::<SymbolPlacement>(),
                        text_fit: *bucket_layout.get::<IconTextFit>(),
                        is_offset,
                        segment: group.segment,
                    })));
                    drawable.set_enabled(visible);

                    tile_layer_group.add_drawable(passes, &tile_id, drawable);
                }
            };

            if is_text {
                if bucket.icons_in_text {
                    if text_halo {
                        draw(&self.symbol_text_and_icon_group, true, "halo");
                    }
                    if text_fill {
                        draw(&self.symbol_text_and_icon_group, false, "fill");
                    }
                } else {
                    if text_halo {
                        draw(&self.symbol_sdf_group, true, "halo");
                    }
                    if text_fill {
                        draw(&self.symbol_sdf_group, false, "fill");
                    }
                }
            } else if sdf_icons {
                // icons
                if icon_halo {
                    draw(&self.symbol_sdf_group, true, "halo");
                }
                if icon_fill {
                    draw(&self.symbol_sdf_group, false, "fill");
                }
            } else {
                draw(&self.symbol_icon_group, false, "icon");
            }
        }
    }
}
