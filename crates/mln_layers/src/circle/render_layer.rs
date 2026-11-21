use std::mem::offset_of;
use std::sync::Arc;

use geometry::Point;
use mln_gfx::gpu::Gpu;
use mln_gfx::shaders::layer_ubo::ProjectionUBO;
use mln_gfx::shaders::program::PROJECTION_BUFFER;
use mln_gfx::{AttributeDataType, ColorMode, CullFaceMode, DrawMode};
use mln_map_types::TileProjector;
use mln_render::draw::{
    DrawFrame, DrawGeometry, DrawList, DrawObjects, KeptDraw, SharedAttribute, Textures, TileDraws, TileMap,
    UniformWriter, Uniforms, VertexAttributeArray, bucket_indexes, bucket_vertices, cast_matrix, globe_extrude_scale,
    paint_vertices, projection_data, tile_unit_translation, to_projection_ubo,
};
use mln_render::layer::{
    Bucket, DrawContext, EvaluateCx, EvaluatedLayer, Evaluation, FeatureQuery, HitTest, LayerBucket, LayerType,
    LayerValues, LayoutParameters, SourceLayer,
};
use mln_style::expression::EvaluationContext;
use mln_style_types::{AlignmentType, CirclePitchScaleType};
use mln_tile_id::OverscaledTileID;
use mln_util::Color;
use mln_util::error::Error;

use super::bucket::{CircleBinders, CircleBucket, CircleLayoutVertex};
use super::layout::CircleLayout;
use super::programs::{DRAWABLE, ID_CIRCLE_POS_VERTEX_ATTRIBUTE, PROPS};
use super::ubo::{CircleDrawableUBO, CircleEvaluatedPropsUBO};
use super::{Circle, programs};
use crate::refresh_paint_attributes;

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

/// What a circle layer keeps besides its drawing.
#[derive(Default)]
pub struct CircleState {
    /// The frame's draws as the update makes them.
    draw_list: DrawList<CircleUniforms>,
}

/// The entries in the layer's storage buffers of the draw of `tile_id`,
/// whose bucket's binders are `binders`, with the layer's values `layer`:
/// its matrix, the scales of its circles' extrusion, and the
/// interpolation of the layer's values.
fn uniforms(
    layer: &EvaluatedLayer<Circle>,
    tile_id: &OverscaledTileID,
    binders: &CircleBinders,
    frame: &DrawFrame<'_>,
) -> CircleUniforms {
    const IN_VIEWPORT_PIXEL_UNITS: bool = false;
    const NEAR_CLIPPED: bool = false;
    let evaluated = &layer.paint;
    let zoom = frame.view.zoom() as f32;
    let tile_id = tile_id.to_unwrapped();
    let projection = projection_data(
        &tile_id,
        frame,
        *evaluated.circle_translate.uniform(),
        *evaluated.circle_translate_anchor.uniform(),
        NEAR_CLIPPED,
        IN_VIEWPORT_PIXEL_UNITS,
        None,
        false,
    );

    let pitch_with_map = *evaluated.circle_pitch_alignment.uniform() == AlignmentType::Map;
    let pixels_to_tile_units = tile_id.pixels_to_tile_units(1.0, zoom);
    let latitude_scale = frame.view.projection().circle_radius_correction(frame.view);
    CircleUniforms {
        draw_ubo: CircleDrawableUBO {
            matrix: cast_matrix(&projection.fallback_matrix),
            extrude_scale: if pitch_with_map {
                [pixels_to_tile_units, pixels_to_tile_units]
            } else {
                frame.pixels_to_gl_units
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
        scale_with_map: (*evaluated.circle_pitch_scale.uniform() == CirclePitchScaleType::Map).into(),
        pitch_with_map: (*evaluated.circle_pitch_alignment.uniform() == AlignmentType::Map).into(),
        pad1: 0.0,
    };
    writer.uniform(PROPS.id, &props);
    writer.storage(DRAWABLE.id, |uniforms| uniforms.draw_ubo);
    writer.storage(PROJECTION_BUFFER.id, |uniforms| uniforms.projection);
}

/// A draw's entries in the layer's storage buffers.
struct CircleUniforms {
    draw_ubo: CircleDrawableUBO,
    projection: ProjectionUBO,
}

impl LayerType for Circle {
    type Layout = CircleLayout;
    type Bucket = CircleBucket;
    type View<'a> = LayerBucket<'a, Circle>;
    type Tile = KeptDraw;
    type Drawing = TileDraws<KeptDraw>;
    type State = CircleState;

    fn create_layout(
        group: &[Arc<EvaluatedLayer<Circle>>],
        parameters: &mut LayoutParameters<'_>,
        source_layer: SourceLayer,
    ) -> Result<Option<CircleLayout>, Error> {
        CircleLayout::new(parameters.bucket_parameters, group, source_layer, parameters.filter_groups).map(Some)
    }

    fn feature_query() -> Option<&'static dyn FeatureQuery<Circle>> {
        Some(&Circle)
    }

    fn new_layer(_layer: &LayerValues<Circle>) -> (CircleState, TileDraws<KeptDraw>) {
        Default::default()
    }

    fn evaluate(_state: &mut CircleState, cx: EvaluateCx<'_, Circle>) -> Evaluation {
        let evaluated = &cx.evaluated.paint;
        if (evaluated.circle_radius.constant_or(&1.0) > 0.0 || evaluated.circle_stroke_width.constant_or(&1.0) > 0.0)
            && (evaluated.circle_color.constant_or(&Color::black()).a > 0.0
                || evaluated.circle_stroke_color.constant_or(&Color::black()).a > 0.0)
            && (evaluated.circle_opacity.constant_or(&1.0) > 0.0
                || evaluated.circle_stroke_opacity.constant_or(&1.0) > 0.0)
        {
            Evaluation::TRANSLUCENT
        } else {
            Evaluation::NONE
        }
    }

    /// Makes the frame's draws of the layer, one of each of its tiles, which
    /// it builds for a tile it keeps none of, and writes the layer's buffers
    /// they read.
    fn make_draws(
        state: &mut CircleState,
        drawing: &mut TileDraws<KeptDraw>,
        tile_values: &mut TileMap<KeptDraw>,
        gpu: &mut dyn Gpu,
        cx: &mut DrawContext<'_, '_, Circle>,
    ) {
        let layer_id = cx.layer.id();
        let layer = &*cx.layer.evaluated;
        let evaluated = &layer.paint;
        let frame = &cx.frame;
        for (index, tile) in cx.tiles.iter().enumerate() {
            let tile_id = tile.id();
            let render_data = tile.view_to_draw();
            let Some(render_data) = render_data.filter(|r| r.bucket.has_data()) else {
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

            // The draw's attributes read the bucket's vertices in the
            // context's store, which upload again when they change.
            let changes = tile_values.take_changes(index);
            let value = tile_values.get_mut(index);
            let context = &mut *cx.context;
            if let Some(cached) = value.as_mut() {
                refresh_paint_attributes(&mut cached.objects, changes, |attrs| {
                    binders.bind(evaluated, paint(context).as_ref(), attrs, &mut Uniforms::default());
                });
            } else if !bucket.vertices.is_empty()
                && let (Some(vertices), Some(indexes)) =
                    (bucket_vertices(geometry, &bucket.vertices), bucket_indexes(geometry, &bucket.triangles))
            {
                let mut uniforms = Uniforms::default();
                let mut attrs = VertexAttributeArray::new();
                binders.bind(evaluated, paint(context).as_ref(), &mut attrs, &mut uniforms);
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
                    context.shader_key(&programs::CIRCLE, uniforms, cx.projection_variant),
                    context.color_mode(ColorMode::alpha_blended()),
                    CullFaceMode::disabled(),
                    DrawGeometry {
                        vertices: attrs,
                        vertex_count,
                        mode: DrawMode::Triangles,
                        indexes: indexes.into(),
                        segments: bucket.segments.clone(),
                        instances: None,
                    },
                    Textures::default(),
                );
                *value = Some(KeptDraw::new(objects));
            }

            let Some(draw) = value.as_ref().map(|cached| cached.frame_draw(Some(tile_id))) else {
                continue;
            };
            state.draw_list.push(draw, uniforms(layer, &tile_id, binders, frame));
        }

        drawing.finish(&mut state.draw_list, tile_values, gpu, cx.context, |writer| {
            write_uniforms(layer, writer);
        });
    }
}

impl FeatureQuery<Circle> for Circle {
    fn query_radius(&self, bucket: &CircleBucket, layer: &EvaluatedLayer<Circle>) -> f32 {
        bucket.query_radius(layer)
    }

    fn intersects(&self, layer: &EvaluatedLayer<Circle>, test: &HitTest<'_>) -> bool {
        let HitTest { query_geometry, feature, geometry, zoom, view, pixels_to_tile_units, projector, feature_state } =
            *test;
        let evaluated = &layer.paint;
        // Translate query geometry
        let translated = test.translated_query_geometry(
            *evaluated.circle_translate.uniform(),
            *evaluated.circle_translate_anchor.uniform(),
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
            projected = project_query_geometry(translated_query_geometry, projector, view.size());
            &projected
        };
        let transformed_size = if align_with_map { size * pixels_to_tile_units } else { size };

        // The shader hides a circle whose center the horizon covers where it
        // draws it: translated, and through the projection as blended. The
        // query finds none there either.
        let drawn_translation = tile_unit_translation(
            projector.tile_id(),
            *evaluated.circle_translate.uniform(),
            *evaluated.circle_translate_anchor.uniform(),
            view,
        );
        for ring in geometry.iter() {
            for point in ring {
                if view.is_globe()
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
                    if align_with_map { *point } else { project_point(*point, projector, view.size()) };

                let projected = projector.project(&Point::new(f64::from(point.x), f64::from(point.y)), 0.0);
                let mut adjusted_size = transformed_size;
                let w = projected.signed_distance_from_camera;
                let pitch_scale = &evaluated.circle_pitch_scale;
                let pitch_alignment = &evaluated.circle_pitch_alignment;
                if *pitch_scale.uniform() == CirclePitchScaleType::Viewport
                    && *pitch_alignment.uniform() == AlignmentType::Map
                {
                    adjusted_size *= (w / view.camera_to_center_distance()) as f32;
                } else if *pitch_scale.uniform() == CirclePitchScaleType::Map
                    && *pitch_alignment.uniform() == AlignmentType::Viewport
                {
                    adjusted_size *= (view.camera_to_center_distance() / w) as f32;
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
