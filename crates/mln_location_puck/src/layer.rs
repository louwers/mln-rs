use geometry::Point;
use glam::Vec2;
use mln_engine::draw::ground::FrameGround;
use mln_engine::draw::{
    Context, Draw, DrawGeometry, DrawIndexes, Draws, Mesh, RenderStaticData, Textures, Uniforms, projection_data,
    raw_vertices, to_projection_ubo,
};
use mln_engine::layer::{
    EvaluateCx, Evaluation, FrameQuery, FrameTile, Layer, LayerValues, NoTiles, OnTerrain, PrepareCx,
};
use mln_engine::query::shapes::meets_polygon;
use mln_engine::tile::OverscaledTileID;
use mln_engine::tile::feature::Feature;
use mln_gfx::projection_variant::ProjectionVariant;
use mln_gfx::{AttributeDataType, ColorMode, CullFaceMode, DrawMode};
use mln_style::types::TranslateAnchorType;

use crate::LocationPuck;
use crate::anchor::anchors;
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
    /// of any of its anchors, where they show: in front of the camera and
    /// the globe's horizon, and where the ground in front of the anchor
    /// doesn't hide them.
    fn hit_frame(&self, query: &FrameQuery<'_, Self>, hits: &mut Vec<Feature>) {
        let paint = &query.values().evaluated.paint;
        let view = query.view();
        let position = *paint.puck_position.uniform();
        let pixels: Vec<Vec2> = query.geometry().iter().map(|p| Vec2::new(p.x as f32, p.y as f32)).collect();
        let height = |tile, [x, y]: [f64; 2]| query.tile(tile).ground_height(Point::new(x, y));
        let hit = anchors(view, position, height).iter().any(|anchor| {
            let tile = query.tile(anchor.tile);
            let Some(at) = tile.project(Point::new(anchor.point[0], anchor.point[1]), anchor.height) else {
                return false;
            };
            let ground = Ground::new(paint, view, anchor);
            let meets = parts(paint).filter(|part| part.hit).any(|part| {
                let [center, x, y] = ground.square(&part);
                let outline: Vec<Point<f64>> = part
                    .outline()
                    .into_iter()
                    .map(|[cx, cy]| Point::new(center[0] + cx * x[0] + cy * y[0], center[1] + cx * x[1] + cy * y[1]))
                    .collect();
                meets_polygon(&pixels, &[tile.project_ring(&outline, anchor.height)])
            });
            meets && !tile.behind_ground(&at)
        });
        if hit {
            let point = geo_types::Point::new(position.longitude(), position.latitude());
            hits.push(Feature { geometry: Some(point.into()), ..Feature::default() });
        }
    }
}
