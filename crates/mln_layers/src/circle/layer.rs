use mln_map_types::View;
use mln_render::draw::{Draw, Draws, Mesh, MeshDesc};
use mln_render::layer::{
    DrawnPoint, EvaluateCx, Evaluation, Features, FrameTile, HitTest, Layer, LayerBucket, LayerValues, NewCx,
    PrepareCx, TileCx, TileTransform,
};
use mln_style_types::{AlignmentType, CirclePitchScaleType};
use mln_tile_id::geometry_tile_data::GeometryCoordinates;
use mln_util::Color;
use mln_util::intersection_tests::polygon_intersects_buffered_point;

use super::bucket::CircleBucket;
use super::layout::CircleLayout;
use super::programs::CIRCLE;
use super::ubo::{CircleDrawableUBO, CircleEvaluatedPropsUBO};
use super::{Circle, CirclePaintProperties};

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
        let mut mesh = MeshDesc::new(&CIRCLE, laid.indexes(&bucket.triangles));
        mesh.vertices(laid.buffer(&bucket.corners)).paint(laid.binders(&bucket.binders), &cx.layer.evaluated.paint);
        Some(cx.res.mesh(&mesh))
    }

    fn prepare(&self, cx: &PrepareCx<'_, Self>, out: &mut Draws) {
        let paint = &cx.layer.evaluated.paint;
        let how = TileTransform {
            translate: *paint.circle_translate.uniform(),
            anchor: *paint.circle_translate_anchor.uniform(),
            ..TileTransform::default()
        };
        let pitch_with_map = *paint.circle_pitch_alignment.uniform() == AlignmentType::Map;
        let main = out.main();
        main.block(&CircleEvaluatedPropsUBO {
            scale_with_map: (*paint.circle_pitch_scale.uniform() == CirclePitchScaleType::Map).into(),
            pitch_with_map: pitch_with_map.into(),
            ..CircleEvaluatedPropsUBO::default()
        });
        for (tile, _, mesh) in cx.tiles() {
            let at = tile.project(cx.frame, &how);
            let draw = CircleDrawableUBO {
                extrude_scale: if pitch_with_map { [at.pixels_to_tile_units; 2] } else { cx.frame.pixels_to_clip },
                globe_extrude_scale: at.globe_extrude_scale,
                ..CircleDrawableUBO::default()
            };
            main.push_at(mesh, &at, Draw::DEFAULT, draw);
        }
    }

    fn query_radius(
        &self,
        layer: &LayerValues<Circle>,
        laid: LayerBucket<'_, Circle, CircleBucket>,
        _camera: &View,
    ) -> f32 {
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
        let how = TileTransform {
            translate: *paint.circle_translate.uniform(),
            anchor: *paint.circle_translate_anchor.uniform(),
            ..TileTransform::default()
        };
        let translated = test.translated_query_geometry(how.translate, how.anchor);
        let query = translated.as_ref().unwrap_or(test.query_geometry);

        let size = test.feature_value(&paint.circle_radius) + test.feature_value(&paint.circle_stroke_width);

        // A circle aligned with the map is compared with the query in the
        // plane of the tile, and one aligned with the viewport in the plane
        // of the viewport: the first grows on the screen as it nears the
        // camera, the second in the tile as it goes away.
        let align_with_map = *paint.circle_pitch_alignment.uniform() == AlignmentType::Map;
        let on_screen: GeometryCoordinates;
        let query = if align_with_map {
            query
        } else {
            on_screen = query.iter().map(|p| test.to_screen(*p)).collect();
            &on_screen
        };
        let size = if align_with_map { size * test.pixels_to_tile_units } else { size };
        let scale = *paint.circle_pitch_scale.uniform();
        let camera_distance = test.camera.camera_to_center_distance();
        test.drawn_points(&how)
            .any(|DrawnPoint { point, distance }| {
                let size = match (scale, align_with_map) {
                    (CirclePitchScaleType::Viewport, true) => size * (distance / camera_distance) as f32,
                    (CirclePitchScaleType::Map, false) => size * (camera_distance / distance) as f32,
                    _ => size,
                };
                let point = if align_with_map { point } else { test.to_screen(point) };
                polygon_intersects_buffered_point(query, &point, size)
            })
            .then_some(0.0)
    }
}
