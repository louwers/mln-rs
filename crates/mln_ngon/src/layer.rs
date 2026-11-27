use std::f32::consts::PI;

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
use mln_engine::query::shapes::meets_polygon;
use mln_engine::tile::OverscaledTileID;
use mln_gfx::shaders::layer_ubo::ProjectionUBO;
use mln_gfx::{AttributeDataType, ColorMode, CullFaceMode, DrawMode};
use mln_style::color::Color;
use mln_style::expression::EvaluationContext;
use mln_style::feature::{FeatureState, GeometryCollection, GeometryTileFeature};
use mln_style::types::{AlignmentType, CirclePitchScaleType};

use crate::Ngon;
use crate::bucket::{NgonBinders, NgonLayout};
use crate::program::{NGON, NgonDrawableUBO, NgonEvaluatedPropsUBO};

/// The slot of the program's texture of the ground's depth.
const GROUND_DEPTH_SLOT: usize = NGON.texture_slot("ground_depth");

/// The pass the layer's draws draw in.
const RENDER_PASS: RenderPass = RenderPass::Translucent;

/// An `org.maplibre.ngon` layer: a draw per tile of its bucket's quads,
/// which stand on the ground over terrain.
pub(crate) struct NgonLayer;

impl NgonLayer {
    /// The block of the draw of `tile_id`, whose bucket's binders are
    /// `binders`, with the layer's values `layer`, and its projection.
    fn uniforms(
        layer: &EvaluatedLayer<Ngon>,
        tile_id: &OverscaledTileID,
        binders: &NgonBinders,
        frame: &LayerFrame<'_>,
    ) -> (NgonDrawableUBO, ProjectionUBO) {
        let paint = &layer.paint;
        let zoom = frame.view.zoom() as f32;
        let tile_id = tile_id.to_unwrapped();
        let projection = projection_data(
            &tile_id,
            frame,
            *paint.ngon_translate.uniform(),
            *paint.ngon_translate_anchor.uniform(),
            false,
            false,
            None,
            false,
        );
        let pitch_with_map = *paint.ngon_pitch_alignment.uniform() == AlignmentType::Map;
        let pixels_to_tile_units = tile_id.pixels_to_tile_units(1.0, zoom);
        let latitude_scale = frame.view.projection().circle_radius_correction(frame.view);
        let draw = NgonDrawableUBO {
            matrix: cast_matrix(&projection.fallback_matrix),
            extrude_scale: if pitch_with_map { [pixels_to_tile_units; 2] } else { frame.pixels_to_gl_units },
            radius_t: binders.ngon_radius.interpolation_factor(zoom),
            corners_t: binders.ngon_corners.interpolation_factor(zoom),
            rotate_t: binders.ngon_rotate.interpolation_factor(zoom),
            color_t: binders.ngon_color.interpolation_factor(zoom),
            blur_t: binders.ngon_blur.interpolation_factor(zoom),
            opacity_t: binders.ngon_opacity.interpolation_factor(zoom),
            stroke_width_t: binders.ngon_stroke_width.interpolation_factor(zoom),
            stroke_color_t: binders.ngon_stroke_color.interpolation_factor(zoom),
            stroke_opacity_t: binders.ngon_stroke_opacity.interpolation_factor(zoom),
            globe_extrude_scale: globe_extrude_scale(&tile_id, zoom, latitude_scale),
        };
        (draw, to_projection_ubo(&projection))
    }

    /// The block of the layer's values that its draws share.
    fn props(layer: &EvaluatedLayer<Ngon>) -> NgonEvaluatedPropsUBO {
        let paint = &layer.paint;
        NgonEvaluatedPropsUBO {
            color: paint.ngon_color.uniform().to_float_array(),
            stroke_color: paint.ngon_stroke_color.uniform().to_float_array(),
            radius: *paint.ngon_radius.uniform(),
            corners: *paint.ngon_corners.uniform(),
            rotate: *paint.ngon_rotate.uniform(),
            blur: *paint.ngon_blur.uniform(),
            opacity: *paint.ngon_opacity.uniform(),
            stroke_width: *paint.ngon_stroke_width.uniform(),
            stroke_opacity: *paint.ngon_stroke_opacity.uniform(),
            scale_with_map: i32::from(*paint.ngon_pitch_scale.uniform() == CirclePitchScaleType::Map),
            pitch_with_map: i32::from(*paint.ngon_pitch_alignment.uniform() == AlignmentType::Map),
            pad1: 0.0,
            pad2: 0.0,
            pad3: 0.0,
        }
    }
}

impl Layer for NgonLayer {
    type Style = Ngon;
    type Input = Features<NgonLayout>;
    type Tile = StandingDraw;

    const TERRAIN: OnTerrain = OnTerrain::Stands { hides_behind_ground: true };

    fn new(_values: &LayerValues<Ngon>) -> Self {
        NgonLayer
    }

    fn query_radius(data: TileData<'_, NgonLayer>, layer: &EvaluatedLayer<Ngon>) -> f32 {
        data.bucket.query_radius(layer)
    }

    fn evaluate(&mut self, cx: EvaluateCx<'_, Self>) -> Evaluation {
        let paint = &cx.evaluated.paint;
        let sized = paint.ngon_radius.constant_or(&1.0) > 0.0 || paint.ngon_stroke_width.constant_or(&1.0) > 0.0;
        let colored = paint.ngon_color.constant_or(&Color::black()).a > 0.0
            || paint.ngon_stroke_color.constant_or(&Color::black()).a > 0.0;
        let opaque = paint.ngon_opacity.constant_or(&1.0) > 0.0 || paint.ngon_stroke_opacity.constant_or(&1.0) > 0.0;
        if sized && colored && opaque { Evaluation::TRANSLUCENT } else { Evaluation::NONE }
    }

    /// The draw of a tile's polygons.
    fn tile(
        &mut self,
        cx: &mut TileCx<'_, '_, Self>,
        tile: FrameTile<'_, NgonLayer>,
        _data: TileData<'_, NgonLayer>,
        _previous: Option<StandingDraw>,
    ) -> Option<StandingDraw> {
        let render_data = tile.data_for_pass(RENDER_PASS)?;
        let bucket = render_data.bucket;
        let binders = bucket.binders.get(&cx.base.style.base().id)?;
        let geometry = render_data.geometry();
        if bucket.vertices.is_empty() || bucket.segments.is_empty() {
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
        let stride = size_of::<[i16; 2]>() as u32;
        attrs.set_shared("pos", SharedAttribute::new(vertices, AttributeDataType::Short2, 0, stride));
        let shader = context.shader_key(&NGON, uniforms, cx.base.projection_variant);
        let mesh = cx.res.mesh(
            shader,
            ColorMode::alpha_blended(),
            CullFaceMode::disabled(),
            DrawGeometry {
                vertices: attrs,
                vertex_count: bucket.vertices.len(),
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
        render_tiles: &[FrameTile<'_, NgonLayer>],
        out: &mut Draws,
    ) {
        let layer_id = cx.base.style.base().id.as_str();
        let layer = &*cx.values.evaluated;
        let paint = &layer.paint;
        // Over terrain, the polygons stand on the frame's ground and hide
        // behind it.
        let ground = cx.frame.ground.as_deref();
        for tile in render_tiles {
            let tile_id = tile.id();
            let Some(render_data) = tile.data_for_pass(RENDER_PASS) else {
                continue;
            };
            let bucket = render_data.bucket;
            let Some(binders) = bucket.binders.get(layer_id) else {
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
                let vertices = paint_vertices(context, geometry, buffer, binders.depends_on_feature_state());
                binders.bind(paint, vertices.as_ref(), attrs, &mut Uniforms::default());
            });
            draw.stand(context, ground, tile_id.to_unwrapped(), "ground_height", &bucket.centers);
            draw.mesh.set_texture(context, GROUND_DEPTH_SLOT, ground.and_then(FrameGround::depth));
            let (draw_ubo, projection) = Self::uniforms(layer, &tile_id, binders, cx.frame);
            // The draws keep the order they're pushed in, the tiles'.
            out.main().push_at(&draw.mesh, tile_id, &projection, Draw::DEFAULT, draw_ubo);
        }
        out.main().block(&Self::props(layer));
    }

    /// Whether the query meets the outline of a polygon of `feature`, its
    /// stroke's outer edge, where it draws: one aligned with the viewport
    /// around where its center draws on the screen, and one aligned with
    /// the map lying flat on it at its center's height, which the query
    /// meets where its lines of sight do. A polygon whose center the frame
    /// hides, behind the camera, the globe's horizon or the ground in
    /// front, doesn't show.
    fn hit(
        &self,
        values: &LayerValues<Ngon>,
        query: &TileQuery<'_>,
        feature: &dyn GeometryTileFeature,
        geometry: &GeometryCollection,
        feature_state: &FeatureState,
    ) -> bool {
        let paint = &values.evaluated.paint;
        let query = query.translated(*paint.ngon_translate.uniform(), *paint.ngon_translate_anchor.uniform());
        let view = query.view();
        let context =
            EvaluationContext::with_zoom_feature_state(view.zoom() as f32, Some(feature), Some(feature_state));
        let shape = Shape {
            corners: corners(paint.ngon_corners.evaluate(&context)),
            radius: paint.ngon_radius.evaluate(&context).max(0.0),
            stroke_width: paint.ngon_stroke_width.evaluate(&context).max(0.0),
            rotate: paint.ngon_rotate.evaluate(&context).to_radians(),
        };
        let align_with_map = *paint.ngon_pitch_alignment.uniform() == AlignmentType::Map;
        let scale_with_map = *paint.ngon_pitch_scale.uniform() == CirclePitchScaleType::Map;
        let camera = view.camera_to_center_distance();
        let pixels = query.pixels();
        // The query on the level the polygons lie on, which over terrain
        // changes with the ground under them.
        let mut level: Option<(f64, Vec<Vec2>)> = None;
        for &point in geometry.iter().flatten() {
            let p = Point::new(f64::from(point.x), f64::from(point.y));
            let height = query.ground_height(p);
            let Some(center) = query.project(p, height) else { continue };
            let meets = if align_with_map {
                // A size on the map, which perspective shrinks with
                // distance unless it is as large there as at the center.
                let perspective = if scale_with_map { 1.0 } else { center.distance / camera };
                let size = (f64::from(shape.outer_radius()) * query.pixel(p) * perspective) as f32;
                let on_level = match &level {
                    Some((at, on_level)) if *at == height => on_level,
                    _ => &level.insert((height, query.on_level(height))).1,
                };
                meets_polygon(on_level, &[shape.outline(Vec2::new(p.x as f32, p.y as f32), size)])
            } else {
                // A size on the screen, which shrinks with distance where it
                // scales with the map.
                let perspective = if scale_with_map { camera / center.distance } else { 1.0 };
                let size = (f64::from(shape.outer_radius()) * perspective) as f32;
                meets_polygon(&pixels, &[shape.outline(center.point, size)])
            };
            if meets && !query.behind_ground(&center) {
                return true;
            }
        }
        false
    }
}

/// The corners of a polygon whose `ngon-corners` is `value`.
fn corners(value: f32) -> u32 {
    value.round().clamp(3.0, 360.0) as u32
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
        self.radius + self.stroke_width / (PI / self.corners as f32).cos()
    }

    /// The corners of the stroke's outer edge around `center`, `size` from
    /// it, in a plane whose y goes down, the screen's or a tile's. The
    /// first corner is on top, turned by the rotation clockwise.
    fn outline(&self, center: Vec2, size: f32) -> Vec<Vec2> {
        (0..self.corners)
            .map(|i| {
                let angle = self.rotate + 2.0 * PI * i as f32 / self.corners as f32;
                center + size * Vec2::new(angle.sin(), -angle.cos())
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
        let center = Vec2::new(100.0, 100.0);
        let triangle = Shape { corners: 3, radius: 0.0, stroke_width: 0.0, rotate: 0.0 };
        let outline = triangle.outline(center, 20.0);
        assert!(outline[0].abs_diff_eq(Vec2::new(100.0, 80.0), 1e-4));
        assert!(outline[1].x > 100.0 && outline[1].y > 100.0);
        let turned = Shape { rotate: PI / 2.0, ..triangle };
        assert!(turned.outline(center, 20.0)[0].abs_diff_eq(Vec2::new(120.0, 100.0), 1e-4));
    }

    #[test]
    fn an_outline_is_not_rounded_to_whole_units() {
        // A point a quarter unit outside a corner misses the polygon, and
        // one a quarter unit inside hits it.
        let square = Shape { corners: 4, radius: 2.0, stroke_width: 0.0, rotate: PI / 4.0 };
        let outline = [square.outline(Vec2::ZERO, 2.0)];
        let corner = 2.0 * (PI / 4.0).sin();
        assert!(!meets_polygon(&[Vec2::splat(corner + 0.25)], &outline));
        assert!(meets_polygon(&[Vec2::splat(corner - 0.25)], &outline));
    }
}
