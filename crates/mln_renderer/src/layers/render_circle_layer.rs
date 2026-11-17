use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use geometry::Point;
use mln_gfx::shader_group::{ShaderGroupPtr, StringIDSetsPair};
use mln_gfx::shaders::shader_defines::{ID_CIRCLE_COLOR_VERTEX_ATTRIBUTE, ID_CIRCLE_POS_VERTEX_ATTRIBUTE};
use mln_gfx::vertex_vector::VertexVectorBasePtr;
use mln_gfx::{
    AttributeDataType, ColorMode, CullFaceMode, DepthMaskType, DrawMode, ShaderRegistry, VertexAttributeArray,
};
use mln_map_types::{TileProjector, TransformState};
use mln_style::expression::EvaluationContext;
use mln_style::layer_type::{ErasedLayerStyle, LayerStyle};
use mln_style::layers::Circle;
use mln_style::layers::circle_layer_properties::CirclePaintProperties;
use mln_style::{PropertyEvaluationParameters, TransitionParameters};
use mln_util::Color;
use mln_util::error::Error;

use super::circle_layer_tweaker::CircleLayerTweaker;
use super::{
    DrawableBinders, DrawableRenderTile, TileDrawables, drawable_has_layer_tweaker, refresh_if_bucket_updated,
};
use crate::bucket_geometry::{bucket_indexes, bucket_vertices, paint_vertices};
use crate::buckets::circle_bucket::{CircleBinders, CircleBucket, CircleLayoutVertex};
use crate::change_request::{ChangeRequest, UniqueChangeRequestVec};
use crate::drawable_builder::DrawableBuilder;
use crate::gfx::context::Context;
use crate::layer_group::LayerGroup;
use crate::layer_tweaker::LayerTweakerPtr;
use crate::layer_type::{ErasedEvaluatedLayer, EvaluatedLayer, LayerType, evaluated};
use crate::layout::circle_layout::CircleLayout;
use crate::layout::layout::{LayoutParameters, SourceLayer};
use crate::paint_parameters::PaintParameters;
use crate::paint_property_binder::read_data_driven_paint_property;
use crate::render_layer::{RenderLayer, RenderLayerBase, RenderLayerPtr, set_style};
use crate::render_pass::RenderPass;
use crate::render_tile::RenderTile;
use crate::render_tree::RenderTree;
use crate::update_parameters::UpdateParameters;

const CIRCLE_SHADER_GROUP_NAME: &str = "CircleShader";

fn read_circle_attributes(
    attrs: &mut VertexAttributeArray,
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
        evaluated.circle_color.is_constant(),
        pau,
        &mut id,
    );
    read_data_driven_paint_property(
        attrs,
        &b.circle_radius,
        buffer,
        shared,
        &["a_radius"],
        evaluated.circle_radius.is_constant(),
        pau,
        &mut id,
    );
    read_data_driven_paint_property(
        attrs,
        &b.circle_blur,
        buffer,
        shared,
        &["a_blur"],
        evaluated.circle_blur.is_constant(),
        pau,
        &mut id,
    );
    read_data_driven_paint_property(
        attrs,
        &b.circle_opacity,
        buffer,
        shared,
        &["a_opacity"],
        evaluated.circle_opacity.is_constant(),
        pau,
        &mut id,
    );
    read_data_driven_paint_property(
        attrs,
        &b.circle_stroke_color,
        buffer,
        shared,
        &["a_stroke_color"],
        evaluated.circle_stroke_color.is_constant(),
        pau,
        &mut id,
    );
    read_data_driven_paint_property(
        attrs,
        &b.circle_stroke_width,
        buffer,
        shared,
        &["a_stroke_width"],
        evaluated.circle_stroke_width.is_constant(),
        pau,
        &mut id,
    );
    read_data_driven_paint_property(
        attrs,
        &b.circle_stroke_opacity,
        buffer,
        shared,
        &["a_stroke_opacity"],
        evaluated.circle_stroke_opacity.is_constant(),
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

impl LayerType for Circle {
    type Layout = CircleLayout;
    type Bucket = CircleBucket;

    fn create_layout(
        group: &[Arc<EvaluatedLayer<Circle>>],
        parameters: &mut LayoutParameters<'_>,
        source_layer: SourceLayer,
    ) -> Result<Option<CircleLayout>, Error> {
        CircleLayout::new(parameters.bucket_parameters, group, source_layer, parameters.filter_groups).map(Some)
    }

    fn query_radius(bucket: &CircleBucket, layer: &EvaluatedLayer<Circle>) -> f32 {
        bucket.query_radius(layer)
    }

    fn render_layer(style: Arc<LayerStyle<Circle>>) -> RenderLayerPtr {
        Rc::new(RefCell::new(RenderCircleLayer::new(style)))
    }
}

pub(crate) struct RenderCircleLayer {
    base: RenderLayerBase,
    style: Arc<LayerStyle<Circle>>,
    // Paint properties
    unevaluated: CirclePaintProperties::Unevaluated,
    circle_shader_group: Option<ShaderGroupPtr>,
}

impl RenderCircleLayer {
    pub(crate) fn new(impl_: Arc<LayerStyle<Circle>>) -> Self {
        let unevaluated = impl_.paint.untransitioned();
        let properties: Arc<dyn ErasedEvaluatedLayer> = Arc::new(EvaluatedLayer::<Circle>::new(Arc::clone(&impl_)));
        let mut base = RenderLayerBase::new(properties);
        base.style_dependencies = unevaluated.get_dependencies();
        RenderCircleLayer { base, style: impl_, unevaluated, circle_shader_group: None }
    }
}

impl RenderLayer for RenderCircleLayer {
    fn base(&self) -> &RenderLayerBase {
        &self.base
    }

    fn base_mut(&mut self) -> &mut RenderLayerBase {
        &mut self.base
    }

    fn restyle(&mut self, parameters: &TransitionParameters, style: &Arc<dyn ErasedLayerStyle>) {
        set_style(&mut self.base, &mut self.style, style);
        self.transition(parameters);
    }

    fn transition(&mut self, parameters: &TransitionParameters) {
        let prior = std::mem::take(&mut self.unevaluated);
        self.unevaluated = self.style.paint.transitioned(parameters, prior);
        self.base.style_dependencies = self.unevaluated.get_dependencies();
    }

    fn evaluate(&mut self, parameters: &PropertyEvaluationParameters) {
        let previous_evaluated = evaluated::<Circle>(&self.base.evaluated_properties).paint.clone();
        let mut properties = EvaluatedLayer::<Circle>::with_paint(
            Arc::clone(&self.style),
            self.unevaluated.evaluate_with_previous(parameters, &previous_evaluated),
        );
        let evaluated = &properties.paint;

        self.base.passes = if (evaluated.circle_radius.constant_or(&1.0) > 0.0
            || evaluated.circle_stroke_width.constant_or(&1.0) > 0.0)
            && (evaluated.circle_color.constant_or(&Color::black()).a > 0.0
                || evaluated.circle_stroke_color.constant_or(&Color::black()).a > 0.0)
            && (evaluated.circle_opacity.constant_or(&1.0) > 0.0
                || evaluated.circle_stroke_opacity.constant_or(&1.0) > 0.0)
        {
            RenderPass::Translucent
        } else {
            RenderPass::None
        };
        properties.render_passes = self.base.passes.underlying_type();
        self.base.evaluated_properties = Arc::new(properties);

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

    // query
    fn query_intersects_feature(
        &self,
        query_geometry: &mln_tile_id::geometry_tile_data::GeometryCoordinates,
        feature: &dyn mln_tile_id::geometry_tile_data::GeometryTileFeature,
        geometry: &mln_tile_id::geometry_tile_data::GeometryCollection,
        zoom: f32,
        transform_state: &TransformState,
        pixels_to_tile_units: f32,
        projector: &TileProjector<'_>,
        feature_state: &mln_style::expression::expression::FeatureState,
    ) -> bool {
        use mln_style_types::{AlignmentType, CirclePitchScaleType};

        use crate::geometry::feature_index::FeatureIndex;

        let evaluated = &evaluated::<Circle>(&self.base.evaluated_properties).paint;
        // Translate query geometry
        let translated = FeatureIndex::translate_query_geometry(
            query_geometry,
            *evaluated.circle_translate.uniform(),
            *evaluated.circle_translate_anchor.uniform(),
            transform_state.get_bearing() as f32,
            pixels_to_tile_units,
        );
        let translated_query_geometry = translated.as_ref().unwrap_or(query_geometry);

        // Evaluate functions
        let radius = evaluated.circle_radius.evaluate(&EvaluationContext::with_zoom_feature_state(
            zoom,
            Some(feature),
            Some(feature_state),
        ));
        let stroke = evaluated.circle_stroke_width.evaluate(&EvaluationContext::with_zoom_feature_state(
            zoom,
            Some(feature),
            Some(feature_state),
        ));
        let size = radius + stroke;

        // For pitch-alignment: map, compare feature geometry to query geometry in
        // the plane of the tile Otherwise, compare geometry in the plane of the
        // viewport A circle with fixed scaling relative to the viewport gets larger
        // in tile space as it moves into the distance A circle with fixed scaling
        // relative to the map gets smaller in viewport space as it moves into the
        // distance
        let align_with_map = *evaluated.circle_pitch_alignment.uniform() == AlignmentType::Map;
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
            *evaluated.circle_translate.uniform(),
            *evaluated.circle_translate_anchor.uniform(),
            transform_state,
        );
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
                let pitch_scale = &evaluated.circle_pitch_scale;
                let pitch_alignment = &evaluated.circle_pitch_alignment;
                if *pitch_scale.uniform() == CirclePitchScaleType::Viewport
                    && *pitch_alignment.uniform() == AlignmentType::Map
                {
                    adjusted_size *= (w / f64::from(transform_state.get_camera_to_center_distance())) as f32;
                } else if *pitch_scale.uniform() == CirclePitchScaleType::Map
                    && *pitch_alignment.uniform() == AlignmentType::Viewport
                {
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
        _context: &Context,
        state: &TransformState,
        _update_parameters: &Arc<UpdateParameters>,
        _paint_parameters: &PaintParameters<'_>,
        _render_tree: &RenderTree,
        changes: &mut UniqueChangeRequestVec,
    ) {
        self.update_projection_variant(state);

        let Some(render_tiles) = self.base.render_tiles.clone().filter(|t| !t.is_empty()) else {
            self.remove_all_drawables();
            return;
        };

        // Set up a layer group
        if self.base.layer_group.is_none() {
            self.set_layer_group(Some(LayerGroup::tiles(self.base.layer_index)), changes);
        }
        let layer_group = self.base.layer_group.clone().expect("layer group");

        if self.base.layer_tweaker.is_none() {
            let tweaker: LayerTweakerPtr = Rc::new(RefCell::new(CircleLayerTweaker::new(
                self.get_id().to_owned(),
                Arc::clone(&self.base.evaluated_properties),
            )));
            self.base.layer_tweaker = Some(Rc::clone(&tweaker));
            // layerGroup->addLayerTweaker(layerTweaker)
            changes.push(ChangeRequest::AddLayerTweaker(Rc::clone(&layer_group), Rc::downgrade(&tweaker)));
        }

        if self.circle_shader_group.is_none() {
            self.circle_shader_group = shaders.shader_group(CIRCLE_SHADER_GROUP_NAME);
        }
        let Some(circle_shader_group) = self.circle_shader_group.clone() else {
            self.remove_all_drawables();
            return;
        };

        let render_pass = RenderPass::Translucent;

        if (render_pass.underlying_type() & self.base.evaluated_properties.render_passes()) == 0 {
            return;
        }

        {
            let mut tile_layer_group = layer_group.borrow_mut();
            let base = &self.base;
            tile_layer_group.remove_drawables_if(|drawable| {
                drawable.tile_id.is_some_and(|id| !base.render_tile_ids.contains_key(&id))
            });
        }

        let evaluated_properties = Arc::clone(&self.base.evaluated_properties);
        let evaluated = &evaluated::<Circle>(&evaluated_properties).paint;
        let layer_tweaker = self.base.layer_tweaker.clone();
        let mut properties_as_uniforms: StringIDSetsPair = Default::default();

        for tile in render_tiles.iter() {
            let tile_id = *tile.get_overscaled_tile_id();

            let style = Arc::clone(&self.base.style);
            let render_data = super::render_data_for_pass(tile, &*style, render_pass);
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
            if prev_bucket_id.is_some_and(|prev| prev != bucket.base.bucket_id) {
                // This tile was previously set up from a different bucket,
                // drop and re-create any drawables for it.
                self.remove_tile(render_pass, &tile_id);
            }
            self.set_render_tile_bucket_id(&tile_id, bucket.base.bucket_id);

            // If there are already drawables for this tile, update their UBOs
            // and move on to the next tile.
            let update_existing = |drawable: &mut crate::drawable::Drawable| {
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

            let circle_vertex_attrs = VertexAttributeArray::new_ptr();
            read_circle_attributes(
                &mut circle_vertex_attrs.borrow_mut(),
                paint_property_binders,
                &paint(),
                evaluated,
                &mut properties_as_uniforms,
            );

            let circle_shader = circle_shader_group.shader(&properties_as_uniforms, self.base.projection_variant);

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

            let mut circle_builder = DrawableBuilder::boxed();
            circle_builder.set_shader(Some(circle_shader));
            circle_builder.set_depth_type(DepthMaskType::ReadOnly);
            circle_builder.set_color_mode(&ColorMode::alpha_blended());
            circle_builder.set_cull_face_mode(CullFaceMode::disabled());

            circle_builder.set_render_pass(render_pass);
            circle_builder.set_vertex_attributes(Some(circle_vertex_attrs));

            circle_builder.set_raw_vertices(Vec::new(), vertex_count, AttributeDataType::Short2);
            circle_builder.set_segments_shared(
                DrawMode::Triangles(),
                Some(bucket_indexes(geometry, bucket, &bucket.triangles)),
                &bucket.segments,
            );

            circle_builder.flush();

            let mut tile_layer_group = layer_group.borrow_mut();
            for mut drawable in circle_builder.clear_drawables() {
                drawable.tile_id = Some(tile_id);
                drawable.layer_tweaker = layer_tweaker.clone();
                drawable.binders = Some(DrawableBinders::new(&render_data.bucket, self.get_id()));
                drawable.render_tile = DrawableRenderTile::new(render_tiles.owner(), tile);

                tile_layer_group.add_tile_drawable(render_pass, &tile_id, drawable);
            }
        }
    }
}
