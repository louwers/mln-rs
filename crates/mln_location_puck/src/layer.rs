use mln_rs::layer::intersection_tests::polygon_intersects_polygon;
use mln_rs::layer::{
    Draw, Draws, EvaluateCx, Evaluation, FrameQuery, GeometryCoordinate, GeometryCoordinates, Hit, Layer, Mesh,
    MeshDesc, NewCx, NoTiles, PrepareCx, TileTransform,
};
use mln_rs::mln_util::feature::Feature;

use crate::LocationPuck;
use crate::parts::{Ground, parts};
use crate::program::{PUCK, PartDraw};

/// An `org.maplibre.location-puck` layer: a mesh of a quad, which draws
/// each of its parts.
pub struct LocationPuckLayer {
    quad: Mesh<PartDraw>,
}

impl Layer for LocationPuckLayer {
    type Style = LocationPuck;
    type Input = NoTiles;
    type Tile = ();

    fn new(cx: &mut NewCx<'_, Self>) -> Self {
        let corners = cx.res.buffer(&[[-1i16, -1], [1, -1], [1, 1], [-1, 1]]);
        let indexes = cx.res.index_buffer(&[0, 1, 2, 0, 2, 3]);
        let mut quad = MeshDesc::new(&PUCK, &indexes);
        quad.vertices(&corners);
        LocationPuckLayer { quad: cx.res.mesh(&quad) }
    }

    fn evaluate(&mut self, cx: EvaluateCx<'_, Self>) -> Evaluation {
        Evaluation::translucent_if(parts(&cx.evaluated.paint).next().is_some())
    }

    fn prepare(&self, cx: &PrepareCx<'_, Self>, out: &mut Draws) {
        let paint = &cx.layer.evaluated.paint;
        let main = out.main();
        for anchor in cx.frame.anchors(*paint.position.uniform()) {
            let at = cx.frame.tile(anchor.tile, &TileTransform::default());
            let ground = Ground::new(paint, cx.frame.camera, &anchor);
            for (order, part) in parts(paint).enumerate() {
                let draw = Draw { order: order as i64, ..Draw::DEFAULT };
                main.push_at(&self.quad, &at, draw, ground.draw(&part, cx.frame.pixel_ratio));
            }
        }
    }

    /// A point at the position where the query hits the puck or the arrow
    /// of any of its world copies.
    fn hit_frame(&self, query: &FrameQuery<'_, Self>, hits: &mut Vec<Hit>) {
        let paint = &query.layer.evaluated.paint;
        let position = *paint.position.uniform();
        let pixel = |x: f64, y: f64| GeometryCoordinate::new(x.round() as i16, y.round() as i16);
        let geometry: GeometryCoordinates = query.geometry.iter().map(|point| pixel(point.x, point.y)).collect();
        let hit = query.anchors(position).any(|anchor| {
            let ground = Ground::new(paint, query.camera, &anchor);
            parts(paint).filter(|part| part.hit).any(|part| {
                let [center, x, y] = ground.square(&part);
                let corner = |cx: f64, cy: f64| {
                    let point = [center[0] + cx * x[0] + cy * y[0], center[1] + cx * x[1] + cy * y[1]];
                    query.to_screen(anchor.tile, point).map(|screen| pixel(screen.x, screen.y))
                };
                let square: Option<GeometryCoordinates> =
                    [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0), (-1.0, -1.0)]
                        .into_iter()
                        .map(|(cx, cy)| corner(cx, cy))
                        .collect();
                square.is_some_and(|square| polygon_intersects_polygon(&geometry, &square))
            })
        });
        if hit {
            let geometry = mln_rs::geo_types::Point::new(position.longitude(), position.latitude());
            let feature = Feature { geometry: Some(geometry.into()), ..Feature::default() };
            hits.push(Hit { feature, distance: 0.0 });
        }
    }
}
