use std::mem::offset_of;

use geometry::Point;
use glam::Vec2;
use mln_engine::draw::ground::{FrameGround, StandingDraw};
use mln_engine::draw::{
    Draw, DrawGeometry, Draws, LayerFrame, RenderPass, SegmentBase, SharedAttribute, Textures, Uniforms,
    VertexAttributeArray, cast_matrix, globe_extrude_scale, projection_data, refresh_paint_attributes,
    to_projection_ubo,
};
use mln_engine::layer::binders::PaintBinders as _;
use mln_engine::layer::{
    EvaluateCx, EvaluatedLayer, Evaluation, Features, FrameTile, Layer, LayerValues, OnTerrain, PrepareCx, TileCx,
    TileData, TileQuery, bucket_indexes, bucket_vertices, paint_vertices,
};
use mln_engine::query::shapes::meets_circle;
use mln_engine::tile::OverscaledTileID;
use mln_gfx::shaders::layer_ubo::ProjectionUBO;
use mln_gfx::{AttributeDataType, ColorMode, CullFaceMode, DrawMode};
use mln_style::color::Color;
use mln_style::expression::EvaluationContext;
use mln_style::layers::Circle;
use mln_style::types::{AlignmentType, CirclePitchScaleType};

use crate::layers::circle::bucket::{CircleBinders, CircleLayoutVertex};
use crate::layers::circle::layout::CircleLayout;
use crate::layers::circle::programs;
use crate::layers::circle::ubo::{CircleDrawableUBO, CircleEvaluatedPropsUBO};

/// The slots of the programs' textures that the draws bind.
const GROUND_DEPTH_SLOT: usize = programs::CIRCLE.texture_slot("ground_depth");

/// The pass the layer's draws draw in.
const RENDER_PASS: RenderPass = RenderPass::Translucent;

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

impl Layer for RenderCircleLayer {
    type Style = Circle;
    type Input = Features<CircleLayout>;

    fn query_radius(data: TileData<'_, RenderCircleLayer>, layer: &EvaluatedLayer<Circle>) -> f32 {
        data.bucket.query_radius(layer)
    }
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
        tile: FrameTile<'_, RenderCircleLayer>,
        _data: TileData<'_, RenderCircleLayer>,
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
    fn prepare(
        &mut self,
        cx: &mut PrepareCx<'_, '_, Self>,
        render_tiles: &[FrameTile<'_, RenderCircleLayer>],
        out: &mut Draws,
    ) {
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

    /// Whether the query meets a circle of `feature` where it draws: one
    /// aligned with the viewport is a circle on the screen around where its
    /// center draws, and one aligned with the map lies flat on it at its
    /// center's height, which the query meets where its lines of sight do.
    /// A circle whose center the frame hides, behind the camera, the
    /// globe's horizon or the ground in front, doesn't show.
    fn hit(
        &self,
        values: &LayerValues<Circle>,
        query: &TileQuery<'_>,
        feature: &dyn mln_style::feature::GeometryTileFeature,
        geometry: &mln_style::feature::GeometryCollection,
        feature_state: &mln_style::feature::FeatureState,
    ) -> bool {
        let paint = &values.evaluated.paint;
        let query = query.translated(*paint.circle_translate.uniform(), *paint.circle_translate_anchor.uniform());
        let view = query.view();
        let context =
            EvaluationContext::with_zoom_feature_state(view.zoom() as f32, Some(feature), Some(feature_state));
        let size = paint.circle_radius.evaluate(&context) + paint.circle_stroke_width.evaluate(&context);
        let align_with_map = *paint.circle_pitch_alignment.uniform() == AlignmentType::Map;
        let scale_with_map = *paint.circle_pitch_scale.uniform() == CirclePitchScaleType::Map;
        let camera = view.camera_to_center_distance();
        let pixels = query.pixels();
        // The query on the level the circles lie on, which over terrain
        // changes with the ground under them.
        let mut level: Option<(f64, Vec<Vec2>)> = None;
        for point in geometry.iter().flatten() {
            let p = Point::new(f64::from(point.x), f64::from(point.y));
            let height = query.ground_height(p);
            let Some(center) = query.project(p, height) else { continue };
            let meets = if align_with_map {
                // A size on the map, which perspective shrinks with
                // distance unless it is as large there as at the center.
                let perspective = if scale_with_map { 1.0 } else { center.distance / camera };
                let radius = (f64::from(size) * query.pixel(p) * perspective) as f32;
                let on_level = match &level {
                    Some((at, on_level)) if *at == height => on_level,
                    _ => &level.insert((height, query.on_level(height))).1,
                };
                meets_circle(on_level, Vec2::new(p.x as f32, p.y as f32), radius)
            } else {
                // A size on the screen, which shrinks with distance where it
                // scales with the map.
                let perspective = if scale_with_map { camera / center.distance } else { 1.0 };
                meets_circle(&pixels, center.point, (f64::from(size) * perspective) as f32)
            };
            if meets && !query.behind_ground(&center) {
                return true;
            }
        }
        false
    }
}
