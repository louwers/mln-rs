use std::f32::consts::PI;

use geometry::Point;
use glam::Vec2;
use mln_engine::camera::{TileCoordinate, TileProjector, View};
use mln_engine::draw::ground::{FrameGround, StandingDraw, TileGround};
use mln_engine::draw::{
    Draw, DrawGeometry, Draws, LayerFrame, RenderPass, RenderTile, SegmentBase, SharedAttribute, Textures, Uniforms,
    VertexAttributeArray, cast_matrix, globe_extrude_scale, projection_data, refresh_paint_attributes,
    to_projection_ubo,
};
use mln_engine::geo::ScreenCoordinate;
use mln_engine::layer::binders::PaintBinders as _;
use mln_engine::layer::{
    EvaluateCx, EvaluatedLayer, Evaluation, Features, FrameTile, Layer, LayerValues, OnTerrain, PrepareCx, QueryRegion,
    TileCx, TileData, bucket_indexes, bucket_vertices, paint_vertices,
};
use mln_engine::query::translate_query_geometry;
use mln_engine::tile::OverscaledTileID;
use mln_gfx::shaders::layer_ubo::ProjectionUBO;
use mln_gfx::{AttributeDataType, ColorMode, CullFaceMode, DrawMode};
use mln_style::color::Color;
use mln_style::expression::EvaluationContext;
use mln_style::feature::{FeatureState, GeometryCollection, GeometryCoordinate, GeometryTileFeature};
use mln_style::geo::WrapMode;
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
    /// stroke's outer edge, where it draws.
    fn hit(
        &self,
        values: &LayerValues<Ngon>,
        region: &QueryRegion<'_>,
        feature: &dyn GeometryTileFeature,
        geometry: &GeometryCollection,
        feature_state: &FeatureState,
    ) -> bool {
        let QueryRegion { geometry: query, screen, ground, view, projector, pixels_to_tile_units, .. } = *region;
        let paint = &values.evaluated.paint;
        let translate = *paint.ngon_translate.uniform();
        let anchor = *paint.ngon_translate_anchor.uniform();
        let bearing = view.angle() as f32;
        let translated = translate_query_geometry(query, translate, anchor, bearing, pixels_to_tile_units);
        let translated = translated.as_ref().unwrap_or(query);

        let context =
            EvaluationContext::with_zoom_feature_state(view.zoom() as f32, Some(feature), Some(feature_state));
        let shape = Shape {
            corners: corners(paint.ngon_corners.evaluate(&context)),
            radius: paint.ngon_radius.evaluate(&context).max(0.0),
            stroke_width: paint.ngon_stroke_width.evaluate(&context).max(0.0),
            rotate: paint.ngon_rotate.evaluate(&context).to_radians(),
        };

        // A polygon aligned with the map is compared with the query in the
        // plane of the tile, whose y goes down, and one aligned with the
        // viewport in the plane of the viewport, in pixels from its bottom
        // left. Over terrain, a polygon stands at the height of the ground
        // under its center: aligned with the map, it lies in the plane at
        // that height, where the query's lines of sight meet it; aligned
        // with the viewport, it shows where it draws, which the query's
        // screen points test.
        let align_with_map = *paint.ngon_pitch_alignment.uniform() == AlignmentType::Map;
        let viewport_query: Vec<Vec2> = if align_with_map {
            translated.iter().map(|p| tile_point(*p)).collect()
        } else if ground.is_some() {
            let height = f64::from(view.size().height);
            screen.iter().map(|p| Vec2::new(p.x as f32, (height - p.y) as f32)).collect()
        } else {
            translated.iter().map(|p| project(projector, view, tile_point(*p), 0.0)).collect()
        };
        let size = shape.outer_radius() * if align_with_map { pixels_to_tile_units } else { 1.0 };

        // The shader hides a polygon whose center the horizon covers where
        // it draws it: translated, and through the projection as blended.
        let drawn_translation = RenderTile::tile_unit_translation(projector.tile_id(), translate, anchor, view);
        let scale = *paint.ngon_pitch_scale.uniform();
        for &point in geometry.iter().flatten() {
            let elevation = ground_height(ground, point);
            let drawn = tile_point(point) + Vec2::from(drawn_translation);
            if view.is_globe() && projector.project_as_drawn(&f64_point(drawn), elevation).occluded {
                continue;
            }
            let plane: Vec<Vec2>;
            let query = if align_with_map && ground.is_some() {
                let at_elevation = plane_geometry(screen, elevation, view, projector);
                let translated =
                    translate_query_geometry(&at_elevation, translate, anchor, bearing, pixels_to_tile_units);
                plane = translated.as_ref().unwrap_or(&at_elevation).iter().map(|p| tile_point(*p)).collect();
                &plane
            } else {
                &viewport_query
            };
            let center = if align_with_map {
                tile_point(point)
            } else if ground.is_some() {
                project(projector, view, drawn, elevation)
            } else {
                project(projector, view, tile_point(point), 0.0)
            };
            let distance = projector.project(&f64_point(tile_point(point)), elevation).signed_distance_from_camera;
            let camera_distance = view.camera_to_center_distance();
            let size = match (scale, align_with_map) {
                (CirclePitchScaleType::Viewport, true) => size * (distance / camera_distance) as f32,
                (CirclePitchScaleType::Map, false) => size * (camera_distance / distance) as f32,
                _ => size,
            };
            if polygons_intersect(query, &shape.outline(center, size, !align_with_map)) {
                return true;
            }
        }
        false
    }
}

/// `p`, in tile units, as a vector.
fn tile_point(p: GeometryCoordinate) -> Vec2 {
    Vec2::new(f32::from(p.x), f32::from(p.y))
}

fn f64_point(p: Vec2) -> Point<f64> {
    Point::new(f64::from(p.x), f64::from(p.y))
}

/// Where `p`, in tile units of the tile `projector` projects, `elevation`
/// meters up, shows in `view`, in pixels from its bottom left.
fn project(projector: &TileProjector<'_>, view: &View, p: Vec2, elevation: f64) -> Vec2 {
    let clip = projector.project(&f64_point(p), elevation).point;
    let size = view.size();
    Vec2::new((clip.x as f32 + 1.0) * size.width as f32 * 0.5, (clip.y as f32 + 1.0) * size.height as f32 * 0.5)
}

/// The height of `ground` under `p`, in tile units of its tile, in meters;
/// 0 without ground.
fn ground_height(ground: Option<TileGround<'_>>, p: GeometryCoordinate) -> f64 {
    ground.map_or(0.0, |ground| f64::from(ground.height(tile_point(p))))
}

/// Where the lines of sight through `screen`, points from the top left of
/// `view`, meet the plane `elevation` meters up, in tile units of the tile
/// `projector` projects.
fn plane_geometry(
    screen: &[ScreenCoordinate],
    elevation: f64,
    view: &View,
    projector: &TileProjector<'_>,
) -> Vec<GeometryCoordinate> {
    let height = f64::from(view.size().height);
    screen
        .iter()
        .filter_map(|p| {
            let up = ScreenCoordinate::new(p.x, height - p.y);
            let location = view.screen_coordinate_to_lat_lng_at(&up, elevation, WrapMode::Unwrapped).ok()?;
            let world = TileCoordinate::from_lat_lng(0.0, &location).p;
            Some(TileCoordinate::to_geometry_coordinate(projector.tile_id(), &world))
        })
        .collect()
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
    /// it, in a plane whose y goes up for `y_up` and else down. The first
    /// corner is on top, turned by the rotation clockwise.
    fn outline(&self, center: Vec2, size: f32, y_up: bool) -> Vec<Vec2> {
        let up = if y_up { 1.0 } else { -1.0 };
        (0..self.corners)
            .map(|i| {
                let angle = self.rotate + 2.0 * PI * i as f32 / self.corners as f32;
                center + size * Vec2::new(angle.sin(), up * angle.cos())
            })
            .collect()
    }
}

/// Whether the polygon or point `a` and the polygon `b` meet: one has a
/// corner inside the other, or their edges cross.
fn polygons_intersect(a: &[Vec2], b: &[Vec2]) -> bool {
    a.iter().any(|&p| contains(b, p))
        || b.iter().any(|&p| contains(a, p))
        || edges(a).any(|(a0, a1)| edges(b).any(|(b0, b1)| segments_cross(a0, a1, b0, b1)))
}

/// The edges of the ring `ring`, closed.
fn edges(ring: &[Vec2]) -> impl Iterator<Item = (Vec2, Vec2)> + '_ {
    ring.iter().zip(ring.iter().cycle().skip(1)).map(|(&a, &b)| (a, b))
}

/// Whether `p` is inside the ring `ring`, by the crossings of a ray from it.
fn contains(ring: &[Vec2], p: Vec2) -> bool {
    edges(ring)
        .filter(|(a, b)| (a.y > p.y) != (b.y > p.y) && p.x < (b.x - a.x) * (p.y - a.y) / (b.y - a.y) + a.x)
        .count()
        % 2
        == 1
}

/// Whether the segments `a0`-`a1` and `b0`-`b1` cross.
fn segments_cross(a0: Vec2, a1: Vec2, b0: Vec2, b1: Vec2) -> bool {
    let turn = |a: Vec2, b: Vec2, c: Vec2| (b - a).perp_dot(c - a) > 0.0;
    turn(a0, b0, b1) != turn(a1, b0, b1) && turn(a0, a1, b0) != turn(a0, a1, b1)
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
        let down = triangle.outline(center, 20.0, false);
        assert!(down[0].abs_diff_eq(Vec2::new(100.0, 80.0), 1e-4));
        assert!(down[1].x > 100.0 && down[1].y > 100.0);
        let up = triangle.outline(center, 20.0, true);
        assert!(up[0].abs_diff_eq(Vec2::new(100.0, 120.0), 1e-4));
        assert!(up[1].x > 100.0 && up[1].y < 100.0);
        let turned = Shape { rotate: PI / 2.0, ..triangle };
        assert!(turned.outline(center, 20.0, false)[0].abs_diff_eq(Vec2::new(120.0, 100.0), 1e-4));
    }

    #[test]
    fn an_outline_is_not_rounded_to_whole_units() {
        // A point a quarter unit outside a corner misses the polygon, and
        // one a quarter unit inside hits it.
        let square = Shape { corners: 4, radius: 2.0, stroke_width: 0.0, rotate: PI / 4.0 };
        let outline = square.outline(Vec2::ZERO, 2.0, false);
        let corner = 2.0 * (PI / 4.0).sin();
        assert!(!polygons_intersect(&[Vec2::splat(corner + 0.25)], &outline));
        assert!(polygons_intersect(&[Vec2::splat(corner - 0.25)], &outline));
    }
}
