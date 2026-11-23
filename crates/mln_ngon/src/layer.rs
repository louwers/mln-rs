use std::f32::consts::PI;

use mln_rs::layer::intersection_tests::polygon_intersects_polygon;
use mln_rs::layer::{
    Color, Draw, DrawnPoint, Draws, EvaluateCx, Evaluation, Features, FrameTile, GeometryCoordinate,
    GeometryCoordinates, HitTest, Layer, LayerBucket, LayerValues, Mesh, MeshDesc, NewCx, PrepareCx, TileCx,
    TileTransform, View,
};
use mln_rs::mln_style_types::{AlignmentType, CirclePitchScaleType};

use crate::bucket::{NgonBucket, NgonLayout};
use crate::program::{NGON, NgonDraw, NgonProps};
use crate::{Ngon, paint};

/// An `org.maplibre.ngon` layer: a mesh per tile of its bucket's quads.
pub struct NgonLayer;

impl Layer for NgonLayer {
    type Style = Ngon;
    type Input = Features<NgonLayout>;
    type Tile = Mesh<NgonDraw>;

    fn new(_cx: &mut NewCx<'_, Self>) -> Self {
        NgonLayer
    }

    fn evaluate(&mut self, cx: EvaluateCx<'_, Self>) -> Evaluation {
        let paint = &cx.evaluated.paint;
        let sized = paint.ngon_radius.constant_or(&1.0) > 0.0 || paint.ngon_stroke_width.constant_or(&1.0) > 0.0;
        let colored = paint.ngon_color.constant_or(&Color::black()).a > 0.0
            || paint.ngon_stroke_color.constant_or(&Color::black()).a > 0.0;
        let opaque = paint.ngon_opacity.constant_or(&1.0) > 0.0 || paint.ngon_stroke_opacity.constant_or(&1.0) > 0.0;
        Evaluation::translucent_if(sized && colored && opaque)
    }

    fn tile(
        &mut self,
        cx: &mut TileCx<'_, Self>,
        _tile: FrameTile,
        laid: LayerBucket<'_, Ngon, NgonBucket>,
    ) -> Option<Mesh<NgonDraw>> {
        let bucket = laid.bucket;
        let mut mesh = MeshDesc::new(&NGON, laid.indexes(&bucket.triangles));
        mesh.vertices(laid.buffer(&bucket.corners))
            .vertices(laid.buffer(&bucket.fade))
            .paint(laid.binders(&bucket.binders));
        Some(cx.res.mesh(&mesh))
    }

    fn prepare(&self, cx: &PrepareCx<'_, Self>, out: &mut Draws) {
        let paint = &cx.layer.evaluated.paint;
        let how = transform(paint);
        let pitch_with_map = *paint.ngon_pitch_alignment.uniform() == AlignmentType::Map;
        let main = out.main();
        main.block(&NgonProps {
            scale_with_map: (*paint.ngon_pitch_scale.uniform() == CirclePitchScaleType::Map).into(),
            pitch_with_map: pitch_with_map.into(),
            fade: cx.frame.placement_fade,
            ..NgonProps::default()
        });
        for (tile, _, mesh) in cx.tiles() {
            let at = tile.project(cx.frame, &how);
            let draw = NgonDraw {
                extrude_scale: if pitch_with_map { [at.pixels_to_tile_units; 2] } else { cx.frame.pixels_to_clip },
                globe_extrude_scale: at.globe_extrude_scale,
                ..NgonDraw::default()
            };
            main.push_at(mesh, &at, Draw::DEFAULT, draw);
        }
    }

    fn query_radius(&self, layer: &LayerValues<Ngon>, laid: LayerBucket<'_, Ngon, NgonBucket>, _camera: &View) -> f32 {
        let paint = &layer.evaluated.paint;
        let binders = laid.binders(&laid.bucket.binders);
        let radius = binders.ngon_radius.max_or(&paint.ngon_radius).unwrap_or(paint::Defaults::ngon_radius());
        let stroke =
            binders.ngon_stroke_width.max_or(&paint.ngon_stroke_width).unwrap_or(paint::Defaults::ngon_stroke_width());
        // A stroke's corner is at most twice its width from the fill's, at a
        // triangle's.
        radius.max(0.0) + 2.0 * stroke.max(0.0) + transform(paint).translation_length()
    }

    fn hit(&self, test: &HitTest<'_, Self>) -> Option<f32> {
        let paint = &test.layer.evaluated.paint;
        let how = transform(paint);
        let query = test.query_at(&how);
        let shape = Shape {
            corners: corners(test.feature_value(&paint.ngon_corners)),
            radius: test.feature_value(&paint.ngon_radius).max(0.0),
            stroke_width: test.feature_value(&paint.ngon_stroke_width).max(0.0),
            rotate: test.feature_value(&paint.ngon_rotate).to_radians(),
        };

        // A polygon aligned with the map is compared with the query in the
        // plane of the tile, and one aligned with the viewport in the plane
        // of the viewport.
        let align_with_map = *paint.ngon_pitch_alignment.uniform() == AlignmentType::Map;
        let on_screen: GeometryCoordinates;
        let query = if align_with_map {
            &*query
        } else {
            on_screen = query.iter().map(|p| test.to_screen(*p)).collect();
            &on_screen
        };
        let size = shape.outer_radius() * if align_with_map { test.pixels_to_tile_units } else { 1.0 };
        let scale = *paint.ngon_pitch_scale.uniform();
        let camera_distance = test.camera.camera_to_center_distance();
        test.drawn_points(&how)
            .any(|DrawnPoint { point, distance }| {
                let size = match (scale, align_with_map) {
                    (CirclePitchScaleType::Viewport, true) => size * (distance / camera_distance) as f32,
                    (CirclePitchScaleType::Map, false) => size * (camera_distance / distance) as f32,
                    _ => size,
                };
                let center = if align_with_map { point } else { test.to_screen(point) };
                polygon_intersects_polygon(query, &shape.outline(center, size))
            })
            .then_some(0.0)
    }
}

/// How the layer's `paint` places a tile's polygons: moved by its
/// translate.
pub(crate) fn transform(paint: &paint::PossiblyEvaluated) -> TileTransform {
    TileTransform::translated(*paint.ngon_translate.uniform(), *paint.ngon_translate_anchor.uniform())
}

/// The corners of a polygon whose `ngon-corners` is `value`.
fn corners(value: f32) -> u32 {
    value.round().clamp(3.0, 360.0) as u32
}

/// The distance from the center of a polygon of `radius`, `stroke_width`
/// and `corners`, as its properties give them, to its stroke's corners.
pub(crate) fn outer_radius(radius: f32, stroke_width: f32, corners: f32) -> f32 {
    let corners = self::corners(corners) as f32;
    radius.max(0.0) + stroke_width.max(0.0) / (PI / corners).cos()
}

/// A feature's polygon, in pixels.
struct Shape {
    corners: u32,
    radius: f32,
    stroke_width: f32,
    /// Radians clockwise.
    rotate: f32,
}

impl Shape {
    /// The distance from the center to the corners of the stroke's outer
    /// edge, which is as far from each edge of the fill as the stroke is
    /// wide.
    fn outer_radius(&self) -> f32 {
        outer_radius(self.radius, self.stroke_width, self.corners as f32)
    }

    /// The corners of the stroke's outer edge around `center`, `size` from
    /// it, in a plane whose y goes down.
    fn outline(&self, center: GeometryCoordinate, size: f32) -> Vec<GeometryCoordinate> {
        (0..self.corners)
            .map(|i| {
                let angle = self.rotate + 2.0 * PI * i as f32 / self.corners as f32;
                let (x, y) = (f32::from(center.x) + size * angle.sin(), f32::from(center.y) - size * angle.cos());
                GeometryCoordinate::new(x.round() as i16, y.round() as i16)
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn corners_round_to_a_whole_number_from_3_to_360() {
        assert_eq!([3, 3, 4, 5, 360, 360], [0.0, 3.4, 3.5, 5.0, 360.0, 400.0].map(corners));
    }

    #[test]
    fn a_stroke_is_as_wide_along_each_edge() {
        let square = Shape { corners: 4, radius: 10.0, stroke_width: 2.0, rotate: 0.0 };
        // The outer edge's apothem is the fill's plus the stroke's width.
        let apothem = square.outer_radius() * (PI / 4.0).cos();
        assert!((apothem - (10.0 * (PI / 4.0).cos() + 2.0)).abs() < 1e-4);
    }

    #[test]
    fn the_first_corner_is_on_top_and_corners_turn_clockwise() {
        let center = GeometryCoordinate::new(100, 100);
        let triangle = Shape { corners: 3, radius: 0.0, stroke_width: 0.0, rotate: 0.0 };
        let outline = triangle.outline(center, 20.0);
        assert_eq!(GeometryCoordinate::new(100, 80), outline[0]);
        assert!(outline[1].x > 100 && outline[1].y > 100);
        let turned = Shape { rotate: PI / 2.0, ..triangle };
        assert_eq!(GeometryCoordinate::new(120, 100), turned.outline(center, 20.0)[0]);
    }
}
