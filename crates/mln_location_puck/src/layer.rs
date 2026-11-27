use glam::Vec2;
use mln_engine::draw::ground::FrameGround;
use mln_engine::draw::{
    Context, Draw, DrawGeometry, DrawIndexes, Draws, Mesh, RenderStaticData, Textures, Uniforms, projection_data,
    raw_vertices, to_projection_ubo,
};
use mln_engine::layer::{
    EvaluateCx, Evaluation, FrameQuery, FrameTile, Layer, LayerValues, NoTiles, OnTerrain, PrepareCx,
};
use mln_engine::tile::OverscaledTileID;
use mln_engine::tile::feature::Feature;
use mln_gfx::projection_variant::ProjectionVariant;
use mln_gfx::{AttributeDataType, ColorMode, CullFaceMode, DrawMode};
use mln_style::types::TranslateAnchorType;

use crate::LocationPuck;
use crate::anchor::{anchors, ground_visibility, to_screen};
use crate::parts::{Ground, parts};
use crate::program::PUCK;

/// The slot of the program's texture of the ground's depth.
const GROUND_DEPTH_SLOT: usize = PUCK.texture_slot("ground_depth");

/// An `org.maplibre.location-puck` layer: a quad, which draws each of its
/// parts at each anchor of its position, standing on the ground over
/// terrain.
pub(crate) struct LocationPuckLayer {
    /// The quad, made for a projection variant: the frame's last.
    quad: Option<(ProjectionVariant, Mesh)>,
}

impl LocationPuckLayer {
    /// The quad of the unit square, in the projection `variant`.
    fn quad(context: &mut Context, variant: ProjectionVariant) -> Mesh {
        let corners: [[i16; 2]; 4] = [[-1, -1], [1, -1], [-1, 1], [1, 1]];
        let shader = context.shader_key(&PUCK, Uniforms::default(), variant);
        let geometry = DrawGeometry {
            vertices: raw_vertices(
                "pos",
                bytemuck::cast_slice(&corners).to_vec(),
                corners.len(),
                AttributeDataType::Short2,
            ),
            vertex_count: corners.len(),
            mode: DrawMode::Triangles(),
            indexes: DrawIndexes::new(RenderStaticData::quad_triangle_indices()),
            segments: RenderStaticData::tile_triangle_segments(),
            instances: None,
        };
        Mesh::new(context, shader, ColorMode::alpha_blended(), CullFaceMode::disabled(), geometry, Textures::default())
    }
}

impl Layer for LocationPuckLayer {
    type Style = LocationPuck;
    type Input = NoTiles;
    type Tile = ();

    const TERRAIN: OnTerrain = OnTerrain::Stands { hides_behind_ground: true };

    fn new(_values: &LayerValues<LocationPuck>) -> Self {
        LocationPuckLayer { quad: None }
    }

    fn evaluate(&mut self, cx: EvaluateCx<'_, Self>) -> Evaluation {
        if parts(&cx.evaluated.paint).next().is_some() { Evaluation::TRANSLUCENT } else { Evaluation::NONE }
    }

    /// Pushes a draw of each part at each anchor of the position, from the
    /// bottom part up.
    fn prepare(&mut self, cx: &mut PrepareCx<'_, '_, Self>, _tiles: &[FrameTile<'_, Self>], out: &mut Draws) {
        let paint = &cx.values.evaluated.paint;
        let frame = cx.frame;
        let context = &mut *cx.res.context;
        let variant = cx.base.projection_variant;
        let quad = match &mut self.quad {
            Some((made_for, quad)) if *made_for == variant => quad,
            quad => &mut quad.insert((variant, Self::quad(context, variant))).1,
        };
        // Over terrain, the parts stand on the frame's ground and hide
        // behind it.
        let ground = frame.ground.as_deref();
        quad.set_texture(context, GROUND_DEPTH_SLOT, ground.and_then(FrameGround::depth));
        let height = |tile, [x, y]: [f64; 2]| {
            ground.map_or(0.0, |ground| f64::from(ground.tile(tile).height(Vec2::new(x as f32, y as f32))))
        };

        let main = out.main();
        for anchor in anchors(frame.view, *paint.puck_position.uniform(), height) {
            let tile = OverscaledTileID::new(anchor.tile.canonical.z, anchor.tile.wrap, anchor.tile.canonical);
            let projection = to_projection_ubo(&projection_data(
                &anchor.tile,
                frame,
                [0.0, 0.0],
                TranslateAnchorType::Map,
                false,
                false,
                None,
                false,
            ));
            let at = Ground::new(paint, frame.view, &anchor);
            for (order, part) in parts(paint).enumerate() {
                let draw = Draw { order: order as i64, ..Draw::DEFAULT };
                main.push_at(quad, tile, &projection, draw, at.draw(&part, frame.pixel_ratio));
            }
        }
    }

    /// A point at the position where the query meets the puck or the arrow
    /// of any of its anchors, where they show: in front of the globe's
    /// horizon, and where the ground in front of the anchor fades them less
    /// than halfway.
    fn hit_frame(&self, query: &FrameQuery<'_, Self>, hits: &mut Vec<Feature>) {
        let paint = &query.values().evaluated.paint;
        let view = query.view();
        let position = *paint.puck_position.uniform();
        let geometry: Vec<Vec2> = query.geometry().iter().map(|p| Vec2::new(p.x as f32, p.y as f32)).collect();
        let height = |tile, [x, y]: [f64; 2]| {
            query.ground(tile).map_or(0.0, |ground| f64::from(ground.height(Vec2::new(x as f32, y as f32))))
        };
        let hit = anchors(view, position, height).iter().any(|anchor| {
            let ground = Ground::new(paint, view, anchor);
            let shows = || query.ground(anchor.tile).is_none() || ground_visibility(view, anchor, height) >= 0.5;
            let meets = parts(paint).filter(|part| part.hit).any(|part| {
                let [center, x, y] = ground.square(&part);
                let outline: Vec<[f64; 2]> = part
                    .outline()
                    .into_iter()
                    .map(|[cx, cy]| [center[0] + cx * x[0] + cy * y[0], center[1] + cx * x[1] + cy * y[1]])
                    .collect();
                let screen = |point: [f64; 2]| {
                    let screen = to_screen(view, anchor.tile, point, anchor.height)?;
                    Some(Vec2::new(screen.x as f32, screen.y as f32))
                };
                polygons_intersect(&geometry, &visible_outline(&outline, screen))
            });
            meets && shows()
        });
        if hit {
            let point = geo_types::Point::new(position.longitude(), position.latitude());
            hits.push(Feature { geometry: Some(point.into()), ..Feature::default() });
        }
    }
}

/// Where the ring `ring` shows on the screen, which `screen` gives of each
/// of its points, `None` where a point doesn't show: the ring clipped where
/// it passes out of sight, at the last point of each edge that shows, which
/// bisection finds.
fn visible_outline(ring: &[[f64; 2]], screen: impl Fn([f64; 2]) -> Option<Vec2>) -> Vec<Vec2> {
    let lerp = |a: [f64; 2], b: [f64; 2], t: f64| [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t];
    let mut outline = Vec::with_capacity(ring.len());
    for (&a, &b) in ring.iter().zip(ring.iter().cycle().skip(1)) {
        let (shown_a, shown_b) = (screen(a), screen(b));
        outline.extend(shown_a);
        let ((Some(last), None) | (None, Some(last))) = (shown_a, shown_b) else {
            continue;
        };
        // From the end that shows toward the one that doesn't.
        let (from, to) = if shown_a.is_some() { (a, b) } else { (b, a) };
        let (mut last, mut shown, mut hidden) = (last, 0.0, 1.0);
        for _ in 0..16 {
            let mid = f64::midpoint(shown, hidden);
            match screen(lerp(from, to, mid)) {
                Some(point) => (last, shown) = (point, mid),
                None => hidden = mid,
            }
        }
        outline.push(last);
    }
    outline
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
