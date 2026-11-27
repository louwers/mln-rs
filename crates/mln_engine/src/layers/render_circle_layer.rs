use std::mem::offset_of;
use std::sync::Arc;

use geometry::Point;
use glam::Vec2;
use mln_gfx::shaders::layer_ubo::ProjectionUBO;
use mln_gfx::{AttributeDataType, ColorMode, CullFaceMode, DrawMode};
use mln_style::color::Color;
use mln_style::expression::EvaluationContext;
use mln_style::geo::WrapMode;
use mln_style::layer::LayerStyle;
use mln_style::layers::Circle;
use mln_style::types::{AlignmentType, CirclePitchScaleType};

use super::{cast_matrix, refresh_paint_attributes};
use crate::buckets::circle_bucket::{CircleBinders, CircleLayoutVertex};
use crate::camera::{TileCoordinate, TileProjector};
use crate::draw::draws::{Draw, Draws, LayerFrame};
use crate::draw::objects::{DrawGeometry, Textures};
use crate::draw::segment::SegmentBase;
use crate::draw::vertex_attribute::{SharedAttribute, Uniforms, VertexAttributeArray};
use crate::layer::bucket_geometry::{bucket_indexes, bucket_vertices, paint_vertices};
use crate::layer::layer_type::{EvaluatedLayer, LayerType};
use crate::layer::paint_property_binder::PaintBinders as _;
use crate::layer::render_layer::{
    EvaluateCx, Evaluation, Hosted, LayerValues, OnTerrain, PrepareCx, RenderLayer, TileCx, TypedLayer,
};
use crate::layer::tile_input::{Features, TileData};
use crate::layers::circle::programs;
use crate::layers::circle::ubo::{CircleDrawableUBO, CircleEvaluatedPropsUBO};
use crate::layout::circle_layout::CircleLayout;
use crate::query::feature_index::QueryRegion;
use crate::render::pass::RenderPass;
use crate::render::terrain::{FrameGround, StandingDraw, TileGround};
use crate::render::tile::{FrameTile, RenderTile};
use crate::render::tile_matrix::{globe_extrude_scale, projection_data, to_projection_ubo};
use crate::tile::OverscaledTileID;

/// The slots of the programs' textures that the draws bind.
const GROUND_DEPTH_SLOT: usize = programs::CIRCLE.texture_slot("ground_depth");

/// The pass the layer's draws draw in.
const RENDER_PASS: RenderPass = RenderPass::Translucent;

// query
/// Where `p`, in tile units of the tile `projector` projects, `elevation`
/// meters up, shows in a view of `size`, in pixels from its bottom left.
fn project_point(
    p: Point<f64>,
    elevation: f64,
    projector: &TileProjector<'_>,
    size: mln_style::size::Size,
) -> mln_style::feature::GeometryCoordinate {
    let pos = projector.project(&p, elevation).point;
    // `(float(pos.x) + 1) * size.width * 0.5`: float, then double.
    mln_style::feature::GeometryCoordinate::new(
        (f64::from((pos.x as f32 + 1.0) * size.width as f32) * 0.5) as i16,
        (f64::from((pos.y as f32 + 1.0) * size.height as f32) * 0.5) as i16,
    )
}

fn project_query_geometry(
    query_geometry: &mln_style::feature::GeometryCoordinates,
    projector: &TileProjector<'_>,
    size: mln_style::size::Size,
) -> mln_style::feature::GeometryCoordinates {
    query_geometry
        .iter()
        .map(|p| project_point(Point::new(f64::from(p.x), f64::from(p.y)), 0.0, projector, size))
        .collect()
}

/// `screen`, points from the top left of a view of `size`, in pixels from
/// its bottom left.
fn viewport_geometry(
    screen: &[crate::geo::ScreenCoordinate],
    size: mln_style::size::Size,
) -> mln_style::feature::GeometryCoordinates {
    let height = f64::from(size.height);
    screen.iter().map(|p| mln_style::feature::GeometryCoordinate::new(p.x as i16, (height - p.y) as i16)).collect()
}

/// The height of `ground` under `p`, in tile units of its tile, in meters;
/// 0 without ground.
fn ground_height(ground: Option<TileGround<'_>>, p: mln_style::feature::GeometryCoordinate) -> f64 {
    ground.map_or(0.0, |ground| f64::from(ground.height(Vec2::new(f32::from(p.x), f32::from(p.y)))))
}

/// Where the lines of sight through `screen`, points from the top left of
/// `view`, meet the plane `elevation` meters up, in tile units of the tile
/// `projector` projects.
fn plane_geometry(
    screen: &[crate::geo::ScreenCoordinate],
    elevation: f64,
    view: &crate::camera::View,
    projector: &TileProjector<'_>,
) -> mln_style::feature::GeometryCoordinates {
    let height = f64::from(view.size().height);
    screen
        .iter()
        .filter_map(|p| {
            let up = crate::geo::ScreenCoordinate::new(p.x, height - p.y);
            let location = view.screen_coordinate_to_lat_lng_at(&up, elevation, WrapMode::Unwrapped).ok()?;
            let world = TileCoordinate::from_lat_lng(0.0, &location).p;
            Some(TileCoordinate::to_geometry_coordinate(projector.tile_id(), &world))
        })
        .collect()
}

impl LayerType for Circle {
    type Input = Features<CircleLayout>;

    fn query_radius(data: TileData<'_, Circle>, layer: &EvaluatedLayer<Circle>) -> f32 {
        data.bucket.query_radius(layer)
    }

    fn render_layer(style: Arc<LayerStyle<Circle>>) -> Box<dyn RenderLayer> {
        Hosted::<RenderCircleLayer>::boxed(style)
    }
}

pub(crate) struct RenderCircleLayer;

impl RenderCircleLayer {
    /// The block of the draw of `tile_id`, whose bucket's binders are
    /// `binders`, with the layer's values `layer`: its matrix, the scales of
    /// its circles' extrusion, and the interpolation of the layer's values;
    /// and its projection.
    fn uniforms(
        layer: &EvaluatedLayer<Circle>,
        tile_id: &OverscaledTileID,
        binders: &CircleBinders,
        frame: &LayerFrame<'_>,
    ) -> (CircleDrawableUBO, ProjectionUBO) {
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
        let draw_ubo = CircleDrawableUBO {
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
        };
        (draw_ubo, to_projection_ubo(&projection))
    }

    /// The block of the layer's values that its draws share.
    fn props(layer: &EvaluatedLayer<Circle>) -> CircleEvaluatedPropsUBO {
        let evaluated = &layer.paint;
        CircleEvaluatedPropsUBO {
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
        }
    }
}

impl TypedLayer for RenderCircleLayer {
    type Type = Circle;
    type Tile = StandingDraw;

    const TERRAIN: OnTerrain = OnTerrain::Stands { hides_behind_ground: true };

    fn new(_values: &LayerValues<Circle>) -> Self {
        RenderCircleLayer
    }

    fn evaluate(&mut self, cx: EvaluateCx<'_, Self>) -> Evaluation {
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

    /// The draw of a tile's circles.
    fn tile(
        &mut self,
        cx: &mut TileCx<'_, '_, Self>,
        tile: FrameTile<'_, Circle>,
        _data: TileData<'_, Circle>,
        _previous: Option<StandingDraw>,
    ) -> Option<StandingDraw> {
        let render_data = tile.data_for_pass(RENDER_PASS)?;
        let bucket = render_data.bucket;
        let binders = bucket.paint_property_binders.get(&cx.base.style.base().id)?;
        let geometry = render_data.geometry();
        if bucket.vertices.is_empty() {
            return None;
        }
        let (vertices, indexes) =
            (bucket_vertices(geometry, &bucket.vertices)?, bucket_indexes(geometry, &bucket.triangles)?);
        let context = &mut *cx.res.context;
        let paint =
            paint_vertices(context, geometry, &binders.interleaved_vertex_buffer, binders.depends_on_feature_state());
        let mut uniforms = Uniforms::default();
        let mut attrs = VertexAttributeArray::new();
        binders.bind(&cx.values.evaluated.paint, paint.as_ref(), &mut attrs, &mut uniforms);
        attrs.set_shared(
            "pos",
            SharedAttribute::new(
                vertices,
                AttributeDataType::Short2,
                offset_of!(CircleLayoutVertex, a1) as u32,
                size_of::<CircleLayoutVertex>() as u32,
            ),
        );
        let vertex_count = if bucket.segments.is_empty() { 0 } else { bucket.vertices.len() };
        let shader = context.shader_key(&programs::CIRCLE, uniforms, cx.base.projection_variant);
        let mesh = cx.res.mesh(
            shader,
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
        Some(StandingDraw::new(mesh))
    }

    /// Pushes the frame's draws of the layer, one of each of its tiles.
    fn prepare(&mut self, cx: &mut PrepareCx<'_, '_, Self>, render_tiles: &[FrameTile<'_, Circle>], out: &mut Draws) {
        let layer_id = cx.base.style.base().id.as_str();
        let layer = &*cx.values.evaluated;
        let evaluated = &layer.paint;
        // Over terrain, the circles stand on the frame's ground and hide
        // behind it.
        let ground = cx.frame.ground.as_deref();
        for tile in render_tiles {
            let tile_id = tile.id();
            let Some(render_data) = tile.data_for_pass(RENDER_PASS) else {
                continue;
            };
            let bucket = render_data.bucket;
            let Some(binders) = bucket.paint_property_binders.get(layer_id) else {
                continue;
            };
            let Some(mut entry) = cx.tiles.entry(&tile_id) else {
                continue;
            };
            // The draw's attributes read the bucket's vertices in the
            // context's store, which upload again when they change.
            let changes = entry.take_changes();
            let Some(draw) = entry.value.as_mut() else {
                continue;
            };
            let context = &mut *cx.res.context;
            refresh_paint_attributes(&draw.mesh, context, changes, |context, attrs| {
                let geometry = render_data.geometry();
                let buffer = &binders.interleaved_vertex_buffer;
                let paint = paint_vertices(context, geometry, buffer, binders.depends_on_feature_state());
                binders.bind(evaluated, paint.as_ref(), attrs, &mut Uniforms::default());
            });
            draw.stand(context, ground, tile_id.to_unwrapped(), "ground_height", &bucket.centers);
            draw.mesh.set_texture(context, GROUND_DEPTH_SLOT, ground.and_then(FrameGround::depth));
            let (draw_ubo, projection) = Self::uniforms(layer, &tile_id, binders, cx.frame);
            // The draws keep the order they're pushed in, the tiles'.
            out.main().push_at(&draw.mesh, tile_id, &projection, Draw::DEFAULT, draw_ubo);
        }
        out.main().block(&Self::props(layer));
    }

    // query
    fn query_intersects_feature(
        &self,
        values: &LayerValues<Circle>,
        region: &QueryRegion<'_>,
        feature: &dyn mln_style::feature::GeometryTileFeature,
        geometry: &mln_style::feature::GeometryCollection,
        feature_state: &mln_style::feature::FeatureState,
    ) -> bool {
        use crate::query::feature_index::FeatureIndex;

        let QueryRegion { geometry: query_geometry, screen, ground, view, projector, pixels_to_tile_units, .. } =
            *region;
        let zoom = view.zoom() as f32;
        let evaluated = &values.evaluated.paint;
        // Translate query geometry
        let translated = FeatureIndex::translate_query_geometry(
            query_geometry,
            *evaluated.circle_translate.uniform(),
            *evaluated.circle_translate_anchor.uniform(),
            view.angle() as f32,
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
        //
        // Over terrain, a circle stands at the height of the ground under its
        // center: aligned with the map, it lies in the plane at that height,
        // where the query's lines of sight meet it; aligned with the
        // viewport, it shows where it draws, which the query's screen points
        // test, wherever their lines of sight meet the ground.
        let align_with_map = *evaluated.circle_pitch_alignment.uniform() == AlignmentType::Map;
        let projected;
        let viewport_query_geometry = if align_with_map {
            translated_query_geometry
        } else {
            projected = if ground.is_some() {
                viewport_geometry(screen, view.size())
            } else {
                project_query_geometry(translated_query_geometry, projector, view.size())
            };
            &projected
        };
        let transformed_size = if align_with_map { size * pixels_to_tile_units } else { size };

        // The shader hides a circle whose center the horizon covers where it
        // draws it: translated, and through the projection as blended. The
        // query finds none there either.
        let drawn_translation = RenderTile::tile_unit_translation(
            projector.tile_id(),
            *evaluated.circle_translate.uniform(),
            *evaluated.circle_translate_anchor.uniform(),
            view,
        );
        for ring in geometry.iter() {
            for point in ring {
                let elevation = ground_height(ground, *point);
                let drawn = Point::new(
                    f64::from(f32::from(point.x) + drawn_translation[0]),
                    f64::from(f32::from(point.y) + drawn_translation[1]),
                );
                if view.is_globe() && projector.project_as_drawn(&drawn, elevation).occluded {
                    continue;
                }
                let plane;
                let transformed_query_geometry = if align_with_map && ground.is_some() {
                    let at_elevation = plane_geometry(screen, elevation, view, projector);
                    plane = FeatureIndex::translate_query_geometry(
                        &at_elevation,
                        *evaluated.circle_translate.uniform(),
                        *evaluated.circle_translate_anchor.uniform(),
                        view.angle() as f32,
                        pixels_to_tile_units,
                    )
                    .unwrap_or(at_elevation);
                    &plane
                } else {
                    viewport_query_geometry
                };
                let transformed_point = if align_with_map {
                    *point
                } else if ground.is_some() {
                    project_point(drawn, elevation, projector, view.size())
                } else {
                    project_point(Point::new(f64::from(point.x), f64::from(point.y)), 0.0, projector, view.size())
                };

                let projected = projector.project(&Point::new(f64::from(point.x), f64::from(point.y)), elevation);
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

                if crate::geo::intersection_tests::polygon_intersects_buffered_point(
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
