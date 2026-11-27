//! The scratch of tessellating a bucket's features.

use geometry::Point;

use crate::layers::line::mesh::LineScratch;

/// Triangulates polygons with earcut and holds the line mesher's scratch,
/// keeping their buffers from one feature to the next.
#[derive(Default)]
pub(crate) struct Tessellator {
    earcut: earcut::Earcut<f64>,
    holes: Vec<u32>,
    indices: Vec<u32>,
    pub(crate) line: LineScratch,
}

impl Tessellator {
    /// Triangulates `polygon`, an outer ring followed by its holes.
    ///
    /// The indices point into the polygon's rings concatenated in order. A
    /// ring may repeat its first point at the end. A polygon whose outer
    /// ring is empty has no triangles.
    pub(crate) fn triangulate<T: Copy + Into<f64>, R: AsRef<[Point<T>]>>(&mut self, polygon: &[R]) -> &[[u32; 3]] {
        // earcut indexes out of bounds on an empty outer ring with holes.
        if polygon.first().is_none_or(|ring| ring.as_ref().is_empty()) {
            return &[];
        }
        self.holes.clear();
        let mut n_vertices = 0;
        for ring in polygon {
            self.holes.push(n_vertices);
            n_vertices += ring.as_ref().len() as u32;
        }
        let points = polygon.iter().flat_map(|ring| ring.as_ref().iter().map(|p| [p.x.into(), p.y.into()]));
        self.earcut.earcut(points, &self.holes[1..], &mut self.indices);
        self.indices.as_chunks().0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn area(points: &[Point<i16>], triangles: &[[u32; 3]]) -> f64 {
        triangles
            .iter()
            .map(|t| {
                let [a, b, c] = t.map(|i| (f64::from(points[i as usize].x), f64::from(points[i as usize].y)));
                ((b.0 - a.0) * (c.1 - a.1) - (c.0 - a.0) * (b.1 - a.1)).abs() / 2.0
            })
            .sum()
    }

    #[test]
    fn holes_index_into_the_concatenated_rings() {
        let square = |min: i16, max: i16| {
            vec![
                Point::new(min, min),
                Point::new(max, min),
                Point::new(max, max),
                Point::new(min, max),
                Point::new(min, min),
            ]
        };
        // A closed outer ring, an empty ring and a closed hole.
        let polygon = [square(0, 10), Vec::new(), square(2, 8)];
        let mut tessellator = Tessellator::default();
        let triangles = tessellator.triangulate(&polygon);
        let points: Vec<Point<i16>> = polygon.concat();
        assert!(triangles.iter().flatten().all(|&i| (i as usize) < points.len()));
        assert_eq!(area(&points, triangles), 100.0 - 36.0);
    }

    #[test]
    fn an_empty_outer_ring_has_no_triangles() {
        let hole = vec![Point::new(2i16, 2), Point::new(8, 2), Point::new(8, 8)];
        let none: &[[u32; 3]] = &[];
        assert_eq!(Tessellator::default().triangulate(&[Vec::new(), hole]), none);
    }
}
