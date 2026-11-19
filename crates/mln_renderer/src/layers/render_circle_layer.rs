use std::mem::offset_of;
use std::sync::Arc;

use geometry::Point;
use mln_gfx::gpu::Gpu;
use mln_gfx::shaders::circle_layer_ubo::{CircleDrawableUBO, CircleEvaluatedPropsUBO};
use mln_gfx::shaders::layer_ubo::ProjectionUBO;
use mln_gfx::shaders::segment::SegmentBase;
use mln_gfx::shaders::shader_defines::{
    ID_CIRCLE_COLOR_VERTEX_ATTRIBUTE, ID_CIRCLE_DRAWABLE_UBO, ID_CIRCLE_EVALUATED_PROPS_UBO,
    ID_CIRCLE_POS_VERTEX_ATTRIBUTE, ID_PROJECTION_UBO,
};
use mln_gfx::{AttributeDataType, ColorMode, CullFaceMode, DrawMode};
use mln_map_types::{TileProjector, TransformState};
use mln_style::expression::EvaluationContext;
use mln_style::layer_type::LayerStyle;
use mln_style::layers::Circle;
use mln_style::layers::circle_layer_properties::CirclePaintProperties;
use mln_style_types::{AlignmentType, CirclePitchScaleType};
use mln_tile_id::OverscaledTileID;
use mln_util::Color;
use mln_util::error::Error;

use super::{cast_matrix, refresh_paint_attributes};
use crate::bucket::Bucket;
use crate::bucket_geometry::{bucket_indexes, bucket_vertices, paint_vertices};
use crate::buckets::circle_bucket::{CircleBinders, CircleBucket, CircleLayoutVertex};
use crate::draw::{DrawGeometry, DrawObjects, Textures};
use crate::gfx::shader_group::{ShaderGroupPtr, StringIDSetsPair};
use crate::gfx::vertex_attribute::{SharedAttribute, VertexAttributeArray};
use crate::gfx::vertices::SharedVertices;
use crate::layer_drawing::{DrawList, KeptDraw, TileDraws, UniformWriter};
use crate::layer_type::{EvaluatedLayer, LayerType};
use crate::layout::circle_layout::CircleLayout;
use crate::layout::layout::{LayoutParameters, SourceLayer};
use crate::paint_parameters::PaintParameters;
use crate::paint_property_binder::read_data_driven_paint_property;
use crate::render_layer::{
    Atlases, LayerBucket, LayerParts, LayerValues, Parts, PartsMut, RenderLayer, RenderLayerBase, TypedLayer,
};
use crate::render_pass::RenderPass;
use crate::render_tile::{FrameTile, RenderTile};
use crate::tile_matrix::{globe_extrude_scale, projection_data, to_projection_ubo};

const CIRCLE_SHADER_GROUP_NAME: &str = "CircleShader";

/// The pass the layer's draws draw in.
const RENDER_PASS: RenderPass = RenderPass::Translucent;

fn read_circle_attributes(
    attrs: &mut VertexAttributeArray,
    b: &CircleBinders,
    shared: Option<&SharedVertices>,
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
    type TileBucket<'a> = LayerBucket<'a, Circle>;

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

    fn render_layer(style: Arc<LayerStyle<Circle>>) -> Box<dyn RenderLayer> {
        Box::new(RenderCircleLayer::new(style))
    }
}

pub(crate) struct RenderCircleLayer {
    base: RenderLayerBase,
    values: LayerValues<Circle>,
    drawing: TileDraws<KeptDraw>,
    /// The frame's draws as the update makes them.
    draw_list: DrawList<CircleUniforms>,
    circle_shader_group: Option<ShaderGroupPtr>,
}

impl RenderCircleLayer {
    pub(crate) fn new(impl_: Arc<LayerStyle<Circle>>) -> Self {
        let (base, values) = LayerValues::new(impl_);
        RenderCircleLayer {
            base,
            values,
            drawing: TileDraws::default(),
            draw_list: DrawList::default(),
            circle_shader_group: None,
        }
    }

    /// The entries in the layer's storage buffers of the draw of `tile_id`,
    /// whose bucket's binders are `binders`, with the layer's values `layer`:
    /// its matrix, the scales of its circles' extrusion, and the
    /// interpolation of the layer's values.
    fn uniforms(
        layer: &EvaluatedLayer<Circle>,
        tile_id: &OverscaledTileID,
        binders: &CircleBinders,
        parameters: &PaintParameters<'_>,
    ) -> CircleUniforms {
        const IN_VIEWPORT_PIXEL_UNITS: bool = false;
        const NEAR_CLIPPED: bool = false;
        let evaluated = &layer.paint;
        let zoom = parameters.state.get_zoom() as f32;
        let tile_id = tile_id.to_unwrapped();
        let projection = projection_data(
            &tile_id,
            parameters,
            *evaluated.circle_translate.uniform(),
            *evaluated.circle_translate_anchor.uniform(),
            NEAR_CLIPPED,
            IN_VIEWPORT_PIXEL_UNITS,
            None,
            false,
        );

        let pitch_with_map = *evaluated.circle_pitch_alignment.uniform() == AlignmentType::Map;
        let pixels_to_tile_units = tile_id.pixels_to_tile_units(1.0, zoom);
        let latitude_scale = parameters.state.get_projection().circle_radius_correction(parameters.state);
        CircleUniforms {
            draw_ubo: CircleDrawableUBO {
                matrix: cast_matrix(&projection.fallback_matrix),
                extrude_scale: if pitch_with_map {
                    [pixels_to_tile_units, pixels_to_tile_units]
                } else {
                    parameters.pixels_to_gl_units
                },
                color_t: binders.circle_color.interpolation_factor(zoom),
                radius_t: binders.circle_radius.interpolation_factor(zoom),
                blur_t: binders.circle_blur.interpolation_factor(zoom),
                opacity_t: binders.circle_opacity.interpolation_factor(zoom),
                stroke_color_t: binders.circle_stroke_color.interpolation_factor(zoom),
                stroke_width_t: binders.circle_stroke_width.interpolation_factor(zoom),
                stroke_opacity_t: binders.circle_stroke_opacity.interpolation_factor(zoom),
                globe_extrude_scale: globe_extrude_scale(&tile_id, zoom, latitude_scale),
                pad2: 0.0,
                pad3: 0.0,
            },
            projection: to_projection_ubo(&projection),
        }
    }

    /// Writes the layer's buffers of the frame's draws.
    fn write_uniforms(layer: &EvaluatedLayer<Circle>, writer: &mut UniformWriter<'_, CircleUniforms>) {
        let evaluated = &layer.paint;
        let props = CircleEvaluatedPropsUBO {
            color: evaluated.circle_color.uniform().to_float_array(),
            stroke_color: evaluated.circle_stroke_color.uniform().to_float_array(),
            radius: *evaluated.circle_radius.uniform(),
            blur: *evaluated.circle_blur.uniform(),
            opacity: *evaluated.circle_opacity.uniform(),
            stroke_width: *evaluated.circle_stroke_width.uniform(),
            stroke_opacity: *evaluated.circle_stroke_opacity.uniform(),
            scale_with_map: i32::from(*evaluated.circle_pitch_scale.uniform() == CirclePitchScaleType::Map),
            pitch_with_map: i32::from(*evaluated.circle_pitch_alignment.uniform() == AlignmentType::Map),
            pad1: 0.0,
        };
        writer.uniform(ID_CIRCLE_EVALUATED_PROPS_UBO, &props);
        writer.storage(ID_CIRCLE_DRAWABLE_UBO, |uniforms| uniforms.draw_ubo);
        writer.storage(ID_PROJECTION_UBO, |uniforms| uniforms.projection);
    }
}

/// A draw's entries in the layer's storage buffers.
struct CircleUniforms {
    draw_ubo: CircleDrawableUBO,
    projection: ProjectionUBO,
}

impl TypedLayer for RenderCircleLayer {
    type Type = Circle;
    type Drawing = TileDraws<KeptDraw>;

    fn parts(&self) -> Parts<'_, Self> {
        Parts { base: &self.base, values: &self.values, drawing: &self.drawing }
    }

    fn parts_mut(&mut self) -> PartsMut<'_, Self> {
        PartsMut { base: &mut self.base, values: &mut self.values, drawing: &mut self.drawing }
    }

    fn passes(&self, evaluated: &EvaluatedLayer<Circle>) -> RenderPass {
        let evaluated = &evaluated.paint;
        if (evaluated.circle_radius.constant_or(&1.0) > 0.0 || evaluated.circle_stroke_width.constant_or(&1.0) > 0.0)
            && (evaluated.circle_color.constant_or(&Color::black()).a > 0.0
                || evaluated.circle_stroke_color.constant_or(&Color::black()).a > 0.0)
            && (evaluated.circle_opacity.constant_or(&1.0) > 0.0
                || evaluated.circle_stroke_opacity.constant_or(&1.0) > 0.0)
        {
            RenderPass::Translucent
        } else {
            RenderPass::None
        }
    }

    /// Makes the frame's draws of the layer, one of each of its tiles, which
    /// it builds for a tile it keeps none of, and writes the layer's buffers
    /// they read.
    fn update_draws(
        &mut self,
        gpu: &mut dyn Gpu,
        paint_parameters: &mut PaintParameters<'_>,
        _atlases: &Atlases,
        render_tiles: &[FrameTile<'_, Circle>],
    ) {
        let shaders = &paint_parameters.static_data.shaders;

        if self.circle_shader_group.is_none() {
            self.circle_shader_group = shaders.shader_group(CIRCLE_SHADER_GROUP_NAME);
        }
        let Some(circle_shader_group) = &self.circle_shader_group else {
            self.remove_all_draws();
            return;
        };

        let layer_id = self.base.style.base().id.as_str();
        let layer = &*self.values.evaluated;
        let evaluated = &layer.paint;
        for tile in render_tiles {
            let tile_id = tile.id();
            let Some(mut entry) = self.drawing.tiles_mut().entry(&tile_id) else {
                continue;
            };

            let render_data = tile.bucket_for_pass(RENDER_PASS);
            let Some(render_data) = render_data.filter(|r| r.bucket.has_data()) else {
                *entry.value = None;
                continue;
            };
            let bucket = render_data.bucket;
            let Some(binders) = bucket.paint_property_binders.get(layer_id) else {
                continue;
            };
            let geometry = render_data.geometry();
            let paint = |context: &mut _| {
                paint_vertices(
                    context,
                    geometry,
                    &binders.interleaved_vertex_buffer,
                    binders.depends_on_feature_state(),
                )
            };

            // A draw built from another bucket goes.
            entry.set_bucket(bucket.base.bucket_id);

            // The draw's attributes read the bucket's vertices in the
            // context's store, which upload again when they change.
            let changes = entry.take_changes();
            let context = &mut *paint_parameters.context;
            if let Some(cached) = entry.value.as_mut() {
                refresh_paint_attributes(&mut cached.objects, changes, |attrs| {
                    read_circle_attributes(attrs, binders, paint(context).as_ref(), evaluated, &mut Default::default());
                });
            } else if !bucket.vertices.is_empty()
                && let (Some(vertices), Some(indexes)) =
                    (bucket_vertices(geometry, &bucket.vertices), bucket_indexes(geometry, &bucket.triangles))
            {
                let mut properties_as_uniforms = StringIDSetsPair::default();
                let mut attrs = VertexAttributeArray::new();
                let paint = paint(context);
                read_circle_attributes(&mut attrs, binders, paint.as_ref(), evaluated, &mut properties_as_uniforms);
                attrs.set_shared(
                    ID_CIRCLE_POS_VERTEX_ATTRIBUTE,
                    SharedAttribute::new(
                        vertices,
                        AttributeDataType::Short2,
                        offset_of!(CircleLayoutVertex, a1) as u32,
                        size_of::<CircleLayoutVertex>() as u32,
                    ),
                );
                let vertex_count = if bucket.segments.is_empty() { 0 } else { bucket.vertices.len() };
                let objects = DrawObjects::new(
                    circle_shader_group.shader(&properties_as_uniforms, self.base.projection_variant),
                    ColorMode::alpha_blended(),
                    CullFaceMode::disabled(),
                    DrawGeometry {
                        vertices: attrs,
                        vertex_count,
                        mode: DrawMode::Triangles(),
                        indexes: indexes.into(),
                        segments: bucket.segments.iter().map(SegmentBase::copy).collect(),
                        instances: None,
                    },
                    Textures::default(),
                );
                *entry.value = Some(KeptDraw::new(objects));
            }

            let Some(draw) = entry.value.as_ref().map(|cached| cached.frame_draw(Some(tile_id))) else {
                continue;
            };
            self.draw_list.push(draw, Self::uniforms(layer, &tile_id, binders, paint_parameters));
        }

        self.draw_list.finish(self.drawing.frame_mut(), gpu, paint_parameters.context, |writer| {
            Self::write_uniforms(layer, writer);
        });
    }
}

impl RenderLayer for RenderCircleLayer {
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
        use crate::geometry::feature_index::FeatureIndex;

        let evaluated = &self.values.evaluated.paint;
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
}
