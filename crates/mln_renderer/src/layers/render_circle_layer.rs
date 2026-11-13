use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use geometry::Point;
use mln_gfx::shader_group::{ShaderGroupPtr, StringIDSetsPair};
use mln_gfx::shaders::shader_defines::{ID_CIRCLE_COLOR_VERTEX_ATTRIBUTE, ID_CIRCLE_POS_VERTEX_ATTRIBUTE};
use mln_gfx::vertex_vector::VertexVectorBasePtr;
use mln_gfx::{AttributeDataType, ColorMode, Context, CullFaceMode, DepthMaskType, DrawMode, ShaderRegistry};
use mln_map_types::{TileProjector, TransformState};
use mln_style::layer::downcast_arc;
use mln_style::layer_properties::{LayerProperties, get_evaluated};
use mln_style::layers::circle_layer_impl::Impl as CircleLayerImpl;
use mln_style::layers::circle_layer_properties::{
    CircleBlur, CircleColor, CircleLayerProperties, CircleOpacity, CirclePaintProperties, CirclePitchAlignment,
    CirclePitchScale, CircleRadius, CircleStrokeColor, CircleStrokeOpacity, CircleStrokeWidth, CircleTranslate,
    CircleTranslateAnchor,
};
use mln_style::properties::Property;
use mln_style::{PropertyEvaluationParameters, TransitionParameters, layer};
use mln_style_types::{AlignmentType, CirclePitchScaleType};
use mln_util::Color;
use mln_util::identity::SimpleIdentity;
use mln_util::mat4::{Mat4, matrix};
use mln_util::vectors::{Vec2, Vec3, vec2_gl_fmod};

use super::circle_layer_tweaker::CircleLayerTweaker;
use super::{
    TileDrawables, drawable_binders_slot, drawable_has_layer_tweaker, drawable_render_tile_slot,
    refresh_if_bucket_updated,
};
use crate::bucket_geometry::{bucket_indexes, bucket_vertices, paint_vertices};
use crate::buckets::circle_bucket::{CircleBinders, CircleBucket, CircleLayoutVertex};
use crate::change_request::{ChangeRequest, UniqueChangeRequestVec};
use crate::layer_tweaker::{LayerTweakerPtr, layer_tweaker_slot};
use crate::paint_parameters::{PaintParameters, TransformParameters};
use crate::paint_property_binder::{PaintPropertyBinder, read_data_driven_paint_property};
use crate::render_layer::{
    CaptureView, RenderLayer, RenderLayerBase, Stats, capture_retained_features, compute_feature_ndc_bound,
    vertex_color_alpha, vertex_opacity,
};
use crate::render_pass::RenderPass;
use crate::render_tile::RenderTile;
use crate::render_tree::RenderTree;
use crate::update_parameters::UpdateParameters;

fn impl_cast(impl_: &Arc<dyn layer::Impl>) -> &CircleLayerImpl {
    debug_assert!(std::ptr::eq(impl_.get_type_info(), CircleLayerImpl::static_type_info()));
    impl_.downcast_ref::<CircleLayerImpl>().expect("RenderCircleLayer: not a circle layer impl")
}

const CIRCLE_SHADER_GROUP_NAME: &str = "CircleShader";

fn capture_rendered_features(
    stats: &mut Stats,
    bucket: &CircleBucket,
    tile: &RenderTile,
    binders: &CircleBinders,
    evaluated: &CirclePaintProperties::PossiblyEvaluated,
    state: &TransformState,
    transform_params: &TransformParameters,
) {
    let tile_id = tile.get_overscaled_tile_id();

    let alpha = evaluated.get::<CircleOpacity>();
    let stroke_alpha = evaluated.get::<CircleStrokeOpacity>();
    let alpha_is_constant = alpha.is_constant();
    let stroke_alpha_is_constant = stroke_alpha.is_constant();
    if alpha_is_constant
        && stroke_alpha_is_constant
        && binders.circle_opacity.uniform_value(alpha) == 0.0
        && binders.circle_stroke_opacity.uniform_value(stroke_alpha) == 0.0
    {
        return;
    }

    let color = evaluated.get::<CircleColor>();
    let stroke_color = evaluated.get::<CircleStrokeColor>();
    let color_is_constant = color.is_constant();
    let stroke_color_is_constant = stroke_color.is_constant();
    if color_is_constant
        && stroke_color_is_constant
        && binders.circle_color.uniform_value(color).a == 0.0
        && binders.circle_stroke_color.uniform_value(stroke_color).a == 0.0
    {
        return;
    }

    let zoom = state.get_zoom();
    let zoom_fraction = state.get_zoom_fraction();
    let camera_to_center_distance = f64::from(state.get_camera_to_center_distance());
    let pixels_to_tile_units = tile_id.to_unwrapped().pixels_to_tile_units(1.0, zoom as f32);

    let radius = evaluated.get::<CircleRadius>().constant_or(&CircleRadius::default_value());
    let stroke_width = evaluated.get::<CircleStrokeWidth>().constant_or(&CircleStrokeWidth::default_value());
    let pitch_with_map = *evaluated.get::<CirclePitchAlignment>() == AlignmentType::Map;
    let scale_with_map = *evaluated.get::<CirclePitchScale>() == CirclePitchScaleType::Map;
    let size = state.get_size();
    let extrude_scale: Vec2 = if pitch_with_map {
        [f64::from(pixels_to_tile_units), f64::from(pixels_to_tile_units)]
    } else {
        [f64::from(2.0f32 / size.width as f32), f64::from(-2.0f32 / size.height as f32)]
    };
    let corner_radius = f64::from(radius + stroke_width);
    let use_projected_center = !pitch_with_map || !scale_with_map;

    // The position of a circle's quad corner: in view space when the circle
    // is pitched with the map, else in NDC.
    let corner_position = |vertex: &CircleLayoutVertex, tile_matrix: &Mat4| -> Vec3 {
        let pos = [f64::from(vertex.a1[0]), f64::from(vertex.a1[1])];
        // circle center (view space)
        let center = [(pos[0] / 2.0).trunc(), (pos[1] / 2.0).trunc()];
        // corner offset (view space)
        let extrude = vec2_gl_fmod(&pos, 2.0).map(|v| v * 2.0 - 1.0);
        // scaled corner offset (view space)
        let offset = [extrude[0] * extrude_scale[0] * corner_radius, extrude[1] * extrude_scale[1] * corner_radius];
        // projected center (clip space)
        let projected_center = if use_projected_center {
            matrix::transform_mat4_value(&[center[0], center[1], 0.0, 1.0], tile_matrix)
        } else {
            [0.0; 4]
        };

        if pitch_with_map {
            let clip_scale = if scale_with_map { 1.0 } else { projected_center[3] / camera_to_center_distance };
            [center[0] + offset[0] * clip_scale, center[1] + offset[1] * clip_scale, 0.0]
        } else {
            let clip_scale = if scale_with_map { camera_to_center_distance } else { projected_center[3] };
            let clip_pos = [
                projected_center[0] + offset[0] * clip_scale,
                projected_center[1] + offset[1] * clip_scale,
                projected_center[2],
                projected_center[3],
            ];
            [clip_pos[0] / clip_pos[3], clip_pos[1] / clip_pos[3], clip_pos[2] / clip_pos[3]]
        }
    };

    // The same corners in NDC on the globe: a circle on the viewport keeps its
    // pixel radius around the projected center, a circle on the map is the
    // ellipse its east and north radii project to.
    let globe_corner_position =
        |vertex: &CircleLayoutVertex, projector: &TileProjector<'_>, translation: &Vec2| -> Vec3 {
            let pos = [f64::from(vertex.a1[0]), f64::from(vertex.a1[1])];
            let center = Point::new((pos[0] / 2.0).trunc() + translation[0], (pos[1] / 2.0).trunc() + translation[1]);
            let projected = projector.project_as_drawn(&center, 0.0);
            if projected.occluded || projected.signed_distance_from_camera <= 0.0 {
                // Not drawn; NaN stays out of the bound.
                return [f64::NAN, f64::NAN, 0.0];
            }
            let w = projected.signed_distance_from_camera;
            let half_extent: Vec2 = if pitch_with_map {
                let tile_radius = corner_radius
                    * f64::from(pixels_to_tile_units)
                    * projector.pitched_text_correction(&center)
                    * if scale_with_map { 1.0 } else { w / camera_to_center_distance };
                let east = projector.project_as_drawn(&Point::new(center.x + tile_radius, center.y), 0.0).point;
                let north = projector.project_as_drawn(&Point::new(center.x, center.y + tile_radius), 0.0).point;
                let (east, north) = (
                    [east.x - projected.point.x, east.y - projected.point.y],
                    [north.x - projected.point.x, north.y - projected.point.y],
                );
                [east[0].hypot(north[0]), east[1].hypot(north[1])]
            } else {
                let clip_scale = if scale_with_map { camera_to_center_distance } else { w } / w;
                [
                    corner_radius * 2.0 / f64::from(size.width) * clip_scale,
                    corner_radius * 2.0 / f64::from(size.height) * clip_scale,
                ]
            };
            let extrude = vec2_gl_fmod(&pos, 2.0).map(|v| v * 2.0 - 1.0);
            [projected.point.x + extrude[0] * half_extent[0], projected.point.y + extrude[1] * half_extent[1], 0.0]
        };

    // The globe's corners are already in NDC.
    let mut identity: Mat4 = [0.0; 16];
    matrix::identity(&mut identity);

    let buffer = &binders.interleaved_vertex_buffer;
    let vertices = bucket.vertices.data();
    capture_retained_features(
        stats,
        bucket,
        tile_id,
        state,
        transform_params,
        *evaluated.get::<CircleTranslate>(),
        *evaluated.get::<CircleTranslateAnchor>(),
        |vertex_offset| {
            if !alpha_is_constant || !stroke_alpha_is_constant {
                let interpolated = |is_constant: bool, binder: &PaintPropertyBinder<f32>| {
                    if is_constant {
                        Some(1.0)
                    } else {
                        binder.get_vertex_value(buffer, vertex_offset).and_then(|v| vertex_opacity(&v, zoom_fraction))
                    }
                };
                if interpolated(alpha_is_constant, &binders.circle_opacity) == Some(0.0)
                    && interpolated(stroke_alpha_is_constant, &binders.circle_stroke_opacity) == Some(0.0)
                {
                    return false;
                }
            }
            if !color_is_constant {
                let alpha = |binder: &PaintPropertyBinder<Color>| {
                    binder.get_vertex_value(buffer, vertex_offset).and_then(|v| vertex_color_alpha(&v, zoom_fraction))
                };
                if alpha(&binders.circle_color) == Some(0.0) && alpha(&binders.circle_stroke_color) == Some(0.0) {
                    return false;
                }
            }
            true
        },
        |view, vertex_offset, vertex_count| {
            let vertex = |i: usize| vertices.get(vertex_offset.checked_add(i)?);
            match view {
                CaptureView::Matrix(tile_matrix) => {
                    compute_feature_ndc_bound(vertex_count, tile_matrix, !pitch_with_map, |i| {
                        vertex(i).map(|vertex| corner_position(vertex, tile_matrix))
                    })
                }
                CaptureView::Globe { projector, translation } => {
                    compute_feature_ndc_bound(vertex_count, &identity, true, |i| {
                        vertex(i).map(|vertex| globe_corner_position(vertex, projector, translation))
                    })
                }
            }
        },
    );
}

fn read_circle_attributes(
    attrs: &mut mln_gfx::vertex_attribute::VertexAttributeArray,
    b: &CircleBinders,
    shared: &VertexVectorBasePtr,
    evaluated: &CirclePaintProperties::PossiblyEvaluated,
    pau: &mut StringIDSetsPair,
) {
    let buffer = &b.interleaved_vertex_buffer;
    let mut id = ID_CIRCLE_COLOR_VERTEX_ATTRIBUTE;
    read_data_driven_paint_property(
        attrs,
        &b.circle_color,
        buffer,
        shared,
        &["a_color"],
        evaluated.get::<CircleColor>().is_constant(),
        pau,
        &mut id,
    );
    read_data_driven_paint_property(
        attrs,
        &b.circle_radius,
        buffer,
        shared,
        &["a_radius"],
        evaluated.get::<CircleRadius>().is_constant(),
        pau,
        &mut id,
    );
    read_data_driven_paint_property(
        attrs,
        &b.circle_blur,
        buffer,
        shared,
        &["a_blur"],
        evaluated.get::<CircleBlur>().is_constant(),
        pau,
        &mut id,
    );
    read_data_driven_paint_property(
        attrs,
        &b.circle_opacity,
        buffer,
        shared,
        &["a_opacity"],
        evaluated.get::<CircleOpacity>().is_constant(),
        pau,
        &mut id,
    );
    read_data_driven_paint_property(
        attrs,
        &b.circle_stroke_color,
        buffer,
        shared,
        &["a_stroke_color"],
        evaluated.get::<CircleStrokeColor>().is_constant(),
        pau,
        &mut id,
    );
    read_data_driven_paint_property(
        attrs,
        &b.circle_stroke_width,
        buffer,
        shared,
        &["a_stroke_width"],
        evaluated.get::<CircleStrokeWidth>().is_constant(),
        pau,
        &mut id,
    );
    read_data_driven_paint_property(
        attrs,
        &b.circle_stroke_opacity,
        buffer,
        shared,
        &["a_stroke_opacity"],
        evaluated.get::<CircleStrokeOpacity>().is_constant(),
        pau,
        &mut id,
    );
}

// query
fn project_point(
    p: mln_tile_id::geometry_tile_data::GeometryCoordinate,
    projector: &TileProjector<'_>,
    size: mln_util::size::Size,
) -> mln_tile_id::geometry_tile_data::GeometryCoordinate {
    let pos = projector.project(&Point::new(f64::from(p.x), f64::from(p.y)), 0.0).point;
    // `(float(pos.x) + 1) * size.width * 0.5`: float, then double.
    mln_tile_id::geometry_tile_data::GeometryCoordinate::new(
        (f64::from((pos.x as f32 + 1.0) * size.width as f32) * 0.5) as i16,
        (f64::from((pos.y as f32 + 1.0) * size.height as f32) * 0.5) as i16,
    )
}

fn project_query_geometry(
    query_geometry: &mln_tile_id::geometry_tile_data::GeometryCoordinates,
    projector: &TileProjector<'_>,
    size: mln_util::size::Size,
) -> mln_tile_id::geometry_tile_data::GeometryCoordinates {
    query_geometry.iter().map(|p| project_point(*p, projector, size)).collect()
}

pub(crate) struct RenderCircleLayer {
    base: RenderLayerBase,
    // Paint properties
    unevaluated: CirclePaintProperties::Unevaluated,
    circle_shader_group: Option<ShaderGroupPtr>,
}

impl RenderCircleLayer {
    pub(crate) fn new(impl_: Arc<CircleLayerImpl>) -> Self {
        let unevaluated = impl_.paint.untransitioned();
        let properties: Arc<dyn LayerProperties> = Arc::new(CircleLayerProperties::new(impl_));
        let mut base = RenderLayerBase::new(properties);
        base.style_dependencies = unevaluated.get_dependencies();
        RenderCircleLayer { base, unevaluated, circle_shader_group: None }
    }
}

impl RenderLayer for RenderCircleLayer {
    fn base(&self) -> &RenderLayerBase {
        &self.base
    }

    fn base_mut(&mut self) -> &mut RenderLayerBase {
        &mut self.base
    }

    fn transition(&mut self, parameters: &TransitionParameters) {
        let prior = std::mem::take(&mut self.unevaluated);
        self.unevaluated = impl_cast(&self.base.base_impl).paint.transitioned(parameters, prior);
        self.base.style_dependencies = self.unevaluated.get_dependencies();
    }

    fn evaluate(&mut self, parameters: &PropertyEvaluationParameters) {
        let previous_evaluated = get_evaluated::<CircleLayerProperties>(&self.base.evaluated_properties).clone();
        let mut properties = CircleLayerProperties::with_evaluated(
            downcast_arc::<CircleLayerImpl>(&self.base.base_impl),
            self.unevaluated.evaluate_with_previous(parameters, &previous_evaluated),
        );
        let evaluated = &properties.evaluated;

        self.base.passes = if (evaluated.get::<CircleRadius>().constant_or(&1.0) > 0.0
            || evaluated.get::<CircleStrokeWidth>().constant_or(&1.0) > 0.0)
            && (evaluated.get::<CircleColor>().constant_or(&Color::black()).a > 0.0
                || evaluated.get::<CircleStrokeColor>().constant_or(&Color::black()).a > 0.0)
            && (evaluated.get::<CircleOpacity>().constant_or(&1.0) > 0.0
                || evaluated.get::<CircleStrokeOpacity>().constant_or(&1.0) > 0.0)
        {
            RenderPass::Translucent
        } else {
            RenderPass::None
        };
        properties.base.render_passes = self.base.passes.underlying_type();
        self.base.evaluated_properties = Arc::new(properties);

        if let Some(layer_tweaker) = &self.base.layer_tweaker {
            layer_tweaker.borrow_mut().update_properties(self.base.evaluated_properties.clone());
        }
    }

    fn has_transition(&self) -> bool {
        self.unevaluated.has_transition()
    }

    fn has_crossfade(&self) -> bool {
        false
    }

    // query
    fn query_intersects_feature(
        &self,
        query_geometry: &mln_tile_id::geometry_tile_data::GeometryCoordinates,
        feature: &dyn mln_tile_id::geometry_tile_data::GeometryTileFeature,
        zoom: f32,
        transform_state: &TransformState,
        pixels_to_tile_units: f32,
        projector: &TileProjector<'_>,
        feature_state: &mln_style::expression::expression::FeatureState,
    ) -> bool {
        use mln_style_types::{AlignmentType, CirclePitchScaleType};

        use crate::geometry::feature_index::FeatureIndex;

        let evaluated = get_evaluated::<CircleLayerProperties>(&self.base.evaluated_properties);
        // Translate query geometry
        let translated = FeatureIndex::translate_query_geometry(
            query_geometry,
            *evaluated.get::<CircleTranslate>(),
            *evaluated.get::<CircleTranslateAnchor>(),
            transform_state.get_bearing() as f32,
            pixels_to_tile_units,
        );
        let translated_query_geometry = translated.as_ref().unwrap_or(query_geometry);

        // Evaluate functions
        let radius = evaluated.evaluate_state::<CircleRadius>(zoom, feature, feature_state);
        let stroke = evaluated.evaluate_state::<CircleStrokeWidth>(zoom, feature, feature_state);
        let size = radius + stroke;

        // For pitch-alignment: map, compare feature geometry to query geometry in
        // the plane of the tile Otherwise, compare geometry in the plane of the
        // viewport A circle with fixed scaling relative to the viewport gets larger
        // in tile space as it moves into the distance A circle with fixed scaling
        // relative to the map gets smaller in viewport space as it moves into the
        // distance
        let align_with_map = *evaluated.get::<CirclePitchAlignment>() == AlignmentType::Map;
        let projected;
        let transformed_query_geometry = if align_with_map {
            translated_query_geometry
        } else {
            projected = project_query_geometry(translated_query_geometry, projector, transform_state.get_size());
            &projected
        };
        let transformed_size = if align_with_map { size * pixels_to_tile_units } else { size };

        // The shader hides a circle whose center the horizon covers where it
        // draws it: translated, and through the projection as blended. The
        // query finds none there either.
        let drawn_translation = RenderTile::tile_unit_translation(
            projector.get_tile_id(),
            *evaluated.get::<CircleTranslate>(),
            *evaluated.get::<CircleTranslateAnchor>(),
            transform_state,
        );
        let geometry = feature.get_geometries();
        for ring in geometry.iter() {
            for point in ring {
                if transform_state.is_globe_rendering()
                    && projector
                        .project_as_drawn(
                            &Point::new(
                                f64::from(f32::from(point.x) + drawn_translation[0]),
                                f64::from(f32::from(point.y) + drawn_translation[1]),
                            ),
                            0.0,
                        )
                        .occluded
                {
                    continue;
                }
                let transformed_point =
                    if align_with_map { *point } else { project_point(*point, projector, transform_state.get_size()) };

                let projected = projector.project(&Point::new(f64::from(point.x), f64::from(point.y)), 0.0);
                let mut adjusted_size = transformed_size;
                let w = projected.signed_distance_from_camera;
                let pitch_scale = *evaluated.get::<CirclePitchScale>();
                let pitch_alignment = *evaluated.get::<CirclePitchAlignment>();
                if pitch_scale == CirclePitchScaleType::Viewport && pitch_alignment == AlignmentType::Map {
                    adjusted_size *= (w / f64::from(transform_state.get_camera_to_center_distance())) as f32;
                } else if pitch_scale == CirclePitchScaleType::Map && pitch_alignment == AlignmentType::Viewport {
                    adjusted_size *= (f64::from(transform_state.get_camera_to_center_distance()) / w) as f32;
                }

                if mln_util::intersection_tests::polygon_intersects_buffered_point(
                    transformed_query_geometry,
                    &transformed_point,
                    adjusted_size,
                ) {
                    return true;
                }
            }
        }

        false
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
        self.update_projection_variant(state);
        self.base.stats.rendered_features.clear();

        let Some(render_tiles) = self.base.render_tiles.clone().filter(|t| !t.is_empty()) else {
            self.remove_all_drawables();
            return;
        };

        // Set up a layer group
        if self.base.layer_group.is_none() {
            match context.create_tile_layer_group(
                self.base.layer_index,
                /*initial_capacity=*/ 64,
                self.get_id().to_owned(),
            ) {
                Some(layer_group) => self.set_layer_group(Some(layer_group), changes),
                None => return,
            }
        }
        let layer_group = self.base.layer_group.clone().expect("layer group");

        if self.base.layer_tweaker.is_none() {
            let tweaker: LayerTweakerPtr = Rc::new(RefCell::new(CircleLayerTweaker::new(
                self.get_id().to_owned(),
                self.base.evaluated_properties.clone(),
            )));
            self.base.layer_tweaker = Some(tweaker.clone());
            // layerGroup->addLayerTweaker(layerTweaker)
            changes.push(ChangeRequest::AddLayerTweaker(layer_group.clone(), Rc::downgrade(&tweaker)));
        }

        if self.circle_shader_group.is_none() {
            self.circle_shader_group = shaders.get_shader_group(CIRCLE_SHADER_GROUP_NAME);
        }
        let Some(circle_shader_group) = self.circle_shader_group.clone() else {
            self.remove_all_drawables();
            return;
        };

        let render_pass = RenderPass::Translucent;

        if (render_pass.underlying_type() & self.base.evaluated_properties.render_passes) == 0 {
            return;
        }

        {
            let mut group = layer_group.borrow_mut();
            let tile_layer_group = group.as_tile_layer_group_mut().expect("circle layer group is a TileLayerGroup");
            let base = &self.base;
            tile_layer_group.remove_drawables_if(|drawable| {
                drawable.get_tile_id().is_some_and(|id| !base.render_tile_ids.contains_key(&id))
            });
        }

        let evaluated_properties = self.base.evaluated_properties.clone();
        let evaluated = get_evaluated::<CircleLayerProperties>(&evaluated_properties);
        let layer_tweaker = self.base.layer_tweaker.clone();
        let mut properties_as_uniforms: StringIDSetsPair = Default::default();

        for tile in render_tiles.iter() {
            let tile_id = *tile.get_overscaled_tile_id();

            let base_impl = self.base.base_impl.clone();
            let render_data = super::render_data_for_pass(tile, &*base_impl, render_pass);
            let Some(render_data) = render_data.filter(|r| r.bucket.has_data()) else {
                self.remove_tile(render_pass, &tile_id);
                continue;
            };

            let Some(bucket) = (*render_data.bucket).downcast_ref::<CircleBucket>() else {
                continue;
            };
            let vertex_count = bucket.vertices.len();
            let Some(binders_lock) = bucket.paint_property_binders.get(self.get_id()) else {
                continue;
            };
            let binders_guard = binders_lock.read();
            let paint_property_binders = &*binders_guard;
            let geometry = tile.get_bucket_geometry();
            let paint = || {
                paint_vertices(
                    geometry,
                    bucket,
                    &paint_property_binders.interleaved_vertex_buffer,
                    paint_property_binders.depends_on_feature_state(),
                )
            };

            let prev_bucket_id = self.get_render_tile_bucket_id(&tile_id);
            if prev_bucket_id != SimpleIdentity::EMPTY && prev_bucket_id != bucket.base.bucket_id {
                // This tile was previously set up from a different bucket,
                // drop and re-create any drawables for it.
                self.remove_tile(render_pass, &tile_id);
            }
            self.set_render_tile_bucket_id(&tile_id, bucket.base.bucket_id);

            if update_parameters.capture_rendered_features {
                let evaluated = get_evaluated::<CircleLayerProperties>(&render_data.layer_properties);
                let params = &render_tree.get_parameters().transform_params;
                capture_rendered_features(
                    &mut self.base.stats,
                    bucket,
                    tile,
                    paint_property_binders,
                    evaluated,
                    state,
                    params,
                );
            }

            // If there are already drawables for this tile, update their UBOs
            // and move on to the next tile.
            let update_existing = |drawable: &mut dyn mln_gfx::Drawable| {
                // A drawable produced on a previous style/bucket should not be updated.
                let current = drawable_has_layer_tweaker(drawable, layer_tweaker.as_ref());
                if current {
                    refresh_if_bucket_updated(drawable, |attrs| {
                        read_circle_attributes(
                            attrs,
                            paint_property_binders,
                            &paint(),
                            evaluated,
                            &mut Default::default(),
                        );
                    });
                }
                current
            };
            if self.update_tile(render_pass, &tile_id, update_existing) {
                continue;
            }

            properties_as_uniforms.0.clear();
            properties_as_uniforms.1.clear();

            let circle_vertex_attrs = context.create_vertex_attribute_array();
            read_circle_attributes(
                &mut circle_vertex_attrs.borrow_mut(),
                paint_property_binders,
                &paint(),
                evaluated,
                &mut properties_as_uniforms,
            );

            let Some(circle_shader) = circle_shader_group.get_or_create_shader(
                context,
                &properties_as_uniforms,
                self.base.projection_variant,
                "a_pos",
            ) else {
                continue;
            };

            if let Some(attr) =
                circle_vertex_attrs.borrow_mut().set(ID_CIRCLE_POS_VERTEX_ATTRIBUTE, -1, AttributeDataType::Invalid, 0)
            {
                attr.set_shared_raw_data(
                    Some(bucket_vertices(geometry, bucket, &bucket.vertices)),
                    0, // offsetof(CircleLayoutVertex, a1)
                    0,
                    size_of::<CircleLayoutVertex>() as u32,
                    AttributeDataType::Short2,
                );
            }

            let mut circle_builder = context.create_drawable_builder();
            circle_builder.set_shader(Some(circle_shader));
            circle_builder.set_depth_type(DepthMaskType::ReadOnly);
            circle_builder.set_color_mode(&ColorMode::alpha_blended());
            circle_builder.set_cull_face_mode(&CullFaceMode::disabled());

            circle_builder.set_render_pass(render_pass);
            circle_builder.set_vertex_attributes(Some(circle_vertex_attrs));

            circle_builder.set_raw_vertices(Vec::new(), vertex_count, AttributeDataType::Short2);
            circle_builder.set_segments_shared(
                DrawMode::Triangles(),
                Some(bucket_indexes(geometry, bucket, &bucket.triangles)),
                &bucket.segments,
            );

            circle_builder.flush(context);

            let mut group = layer_group.borrow_mut();
            let tile_layer_group = group.as_tile_layer_group_mut().expect("TileLayerGroup");
            for mut drawable in circle_builder.clear_drawables() {
                drawable.set_tile_id(tile_id);
                drawable.set_layer_tweaker(layer_tweaker.as_ref().and_then(layer_tweaker_slot));
                drawable.set_binders(drawable_binders_slot(&render_data.bucket, self.get_id()));
                drawable.set_render_tile(drawable_render_tile_slot(render_tiles.owner(), tile));

                tile_layer_group.add_drawable(render_pass, &tile_id, drawable);
            }
        }
    }
}
