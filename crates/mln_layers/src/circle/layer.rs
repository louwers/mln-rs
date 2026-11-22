use geometry::Point;
use mln_map_types::TileProjector;
use mln_render::draw::{Draw, Draws, Mesh, MeshDesc};
use mln_render::layer::{
    EvaluateCx, Evaluation, Features, FrameTile, HitTest, Layer, LayerBucket, LayerValues, NewCx, PrepareCx, TileCx,
    TileTransform,
};
use mln_style::expression::EvaluationContext;
use mln_style_types::{AlignmentType, CirclePitchScaleType};
use mln_tile_id::geometry_tile_data::{GeometryCoordinate, GeometryCoordinates};
use mln_util::Color;
use mln_util::size::Size;

use super::bucket::CircleBucket;
use super::layout::CircleLayout;
use super::programs::CIRCLE;
use super::ubo::{CircleDrawableUBO, CircleEvaluatedPropsUBO};
use super::{Circle, CirclePaintProperties};

type Paint = CirclePaintProperties::PossiblyEvaluated;

/// A circle layer: a mesh per tile of its bucket's quads.
pub struct CircleLayer;

impl Layer for CircleLayer {
    type Style = Circle;
    type Input = Features<CircleLayout>;
    type Tile = Mesh<CircleDrawableUBO>;

    fn new(_cx: &mut NewCx<'_, Self>) -> Self {
        CircleLayer
    }

    fn evaluate(&mut self, cx: EvaluateCx<'_, Self>) -> Evaluation {
        let paint = &cx.evaluated.paint;
        let sized = paint.circle_radius.constant_or(&1.0) > 0.0 || paint.circle_stroke_width.constant_or(&1.0) > 0.0;
        let colored = paint.circle_color.constant_or(&Color::black()).a > 0.0
            || paint.circle_stroke_color.constant_or(&Color::black()).a > 0.0;
        let opaque =
            paint.circle_opacity.constant_or(&1.0) > 0.0 || paint.circle_stroke_opacity.constant_or(&1.0) > 0.0;
        if sized && colored && opaque { Evaluation::TRANSLUCENT } else { Evaluation::NONE }
    }

    fn tile(
        &mut self,
        cx: &mut TileCx<'_, Self>,
        _tile: FrameTile,
        laid: LayerBucket<'_, Circle, CircleBucket>,
    ) -> Option<Mesh<CircleDrawableUBO>> {
        let bucket = laid.bucket;
        let mut mesh = MeshDesc::new(&CIRCLE);
        mesh.vertices(laid.buffer(&bucket.corners))
            .indexes(laid.buffer(&bucket.triangles), &bucket.segments)
            .paint(laid.binders(&bucket.binders), &cx.layer.evaluated.paint);
        Some(cx.res.mesh(&mesh))
    }

    fn prepare(&self, cx: &PrepareCx<'_, Self>, out: &mut Draws) {
        let paint = &cx.layer.evaluated.paint;
        let how = TileTransform {
            translate: *paint.circle_translate.uniform(),
            anchor: *paint.circle_translate_anchor.uniform(),
            ..TileTransform::default()
        };
        let zoom = cx.frame.camera.zoom() as f32;
        let pitch_with_map = *paint.circle_pitch_alignment.uniform() == AlignmentType::Map;
        let main = out.main();
        main.block(&props(paint));
        for (tile, laid, mesh) in cx.tiles() {
            let at = tile.project(cx.frame, &how);
            let b = laid.binders(&laid.bucket.binders);
            let draw = CircleDrawableUBO {
                matrix: at.fallback_matrix,
                extrude_scale: if pitch_with_map { [at.pixels_to_tile_units; 2] } else { cx.frame.pixels_to_clip },
                color_t: b.circle_color.interpolation_factor(zoom),
                radius_t: b.circle_radius.interpolation_factor(zoom),
                blur_t: b.circle_blur.interpolation_factor(zoom),
                opacity_t: b.circle_opacity.interpolation_factor(zoom),
                stroke_color_t: b.circle_stroke_color.interpolation_factor(zoom),
                stroke_width_t: b.circle_stroke_width.interpolation_factor(zoom),
                stroke_opacity_t: b.circle_stroke_opacity.interpolation_factor(zoom),
                globe_extrude_scale: at.globe_extrude_scale,
                ..CircleDrawableUBO::default()
            };
            main.push_at(mesh, &at, Draw::DEFAULT, draw);
        }
    }

    fn query_radius(&self, layer: &LayerValues<Circle>, laid: LayerBucket<'_, Circle, CircleBucket>) -> f32 {
        let paint = &layer.evaluated.paint;
        let binders = laid.binders(&laid.bucket.binders);
        // A data-driven value's largest over the bucket's features, or else
        // its constant value or default.
        let radius = binders.circle_radius.max().or(paint.circle_radius.constant());
        let stroke = binders.circle_stroke_width.max().or(paint.circle_stroke_width.constant());
        let [x, y] = *paint.circle_translate.uniform();
        radius.unwrap_or(CirclePaintProperties::Defaults::circle_radius())
            + stroke.unwrap_or(CirclePaintProperties::Defaults::circle_stroke_width())
            + (x * x + y * y).sqrt()
    }

    fn hit(&self, test: &HitTest<'_, Self>) -> Option<f32> {
        let paint = &test.layer.evaluated.paint;
        let (translate, anchor) = (*paint.circle_translate.uniform(), *paint.circle_translate_anchor.uniform());
        let translated = test.translated_query_geometry(translate, anchor);
        let query = translated.as_ref().unwrap_or(test.query_geometry);

        let context =
            EvaluationContext::with_zoom_feature_state(test.zoom, Some(test.feature), Some(test.feature_state));
        let size = paint.circle_radius.evaluate(&context) + paint.circle_stroke_width.evaluate(&context);

        // A circle aligned with the map is compared with the query in the
        // plane of the tile, and one aligned with the viewport in the plane
        // of the viewport: the first grows on the screen as it nears the
        // camera, the second in the tile as it goes away.
        let align_with_map = *paint.circle_pitch_alignment.uniform() == AlignmentType::Map;
        let projected;
        let query = if align_with_map {
            query
        } else {
            projected = project_all(query, test.projector, test.camera.size());
            &projected
        };
        let size = if align_with_map { size * test.pixels_to_tile_units } else { size };
        let scale = *paint.circle_pitch_scale.uniform();

        // The shader hides a circle whose center the horizon covers where it
        // draws it: translated, and through the projection as blended.
        let drawn = test.translation(translate, anchor);
        let camera_distance = test.camera.camera_to_center_distance();
        for point in test.geometry.iter().flatten() {
            let at =
                |dx: f32, dy: f32| Point::new(f64::from(f32::from(point.x) + dx), f64::from(f32::from(point.y) + dy));
            if test.camera.is_globe() && test.projector.project_as_drawn(&at(drawn[0], drawn[1]), 0.0).occluded {
                continue;
            }
            let w = test.projector.project(&at(0.0, 0.0), 0.0).signed_distance_from_camera;
            let size = match (scale, align_with_map) {
                (CirclePitchScaleType::Viewport, true) => size * (w / camera_distance) as f32,
                (CirclePitchScaleType::Map, false) => size * (camera_distance / w) as f32,
                _ => size,
            };
            let point = if align_with_map { *point } else { project(*point, test.projector, test.camera.size()) };
            if mln_util::intersection_tests::polygon_intersects_buffered_point(query, &point, size) {
                return Some(0.0);
            }
        }
        None
    }
}

/// The layer's block of its values that don't depend on the tile.
fn props(paint: &Paint) -> CircleEvaluatedPropsUBO {
    CircleEvaluatedPropsUBO {
        color: paint.circle_color.uniform().to_float_array(),
        stroke_color: paint.circle_stroke_color.uniform().to_float_array(),
        radius: *paint.circle_radius.uniform(),
        blur: *paint.circle_blur.uniform(),
        opacity: *paint.circle_opacity.uniform(),
        stroke_width: *paint.circle_stroke_width.uniform(),
        stroke_opacity: *paint.circle_stroke_opacity.uniform(),
        scale_with_map: (*paint.circle_pitch_scale.uniform() == CirclePitchScaleType::Map).into(),
        pitch_with_map: (*paint.circle_pitch_alignment.uniform() == AlignmentType::Map).into(),
        ..CircleEvaluatedPropsUBO::default()
    }
}

/// `p`, a point of the tile that `projector` projects, on a screen of
/// `size`.
fn project(p: GeometryCoordinate, projector: &TileProjector<'_>, size: Size) -> GeometryCoordinate {
    let pos = projector.project(&Point::new(f64::from(p.x), f64::from(p.y)), 0.0).point;
    // Scaled in `f32`, halved in `f64`.
    GeometryCoordinate::new(
        (f64::from((pos.x as f32 + 1.0) * size.width as f32) * 0.5) as i16,
        (f64::from((pos.y as f32 + 1.0) * size.height as f32) * 0.5) as i16,
    )
}

/// [`project`] of each of `points`.
fn project_all(points: &GeometryCoordinates, projector: &TileProjector<'_>, size: Size) -> GeometryCoordinates {
    points.iter().map(|p| project(*p, projector, size)).collect()
}
