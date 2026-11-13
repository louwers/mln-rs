//! Polygon triangulation for fills and fill extrusions.

use std::cell::RefCell;

use geometry::Point;

thread_local! {
    // Reused so that each polygon doesn't allocate earcut's buffers again.
    static EARCUT: RefCell<earcut::Earcut<f64>> = RefCell::new(earcut::Earcut::new());
}

/// Triangulates `polygon`, an outer ring followed by its holes, with earcut.
///
/// The indices point into the polygon's rings concatenated in order. A ring
/// may repeat its first point at the end. A polygon whose outer ring is
/// empty has no triangles.
pub fn triangulate<T: Copy + Into<f64>, R: AsRef<[Point<T>]>>(polygon: &[R]) -> Vec<u32> {
    // earcut indexes out of bounds on an empty outer ring with holes.
    if polygon.first().is_none_or(|ring| ring.as_ref().is_empty()) {
        return Vec::new();
    }
    let mut hole_indices = Vec::with_capacity(polygon.len().saturating_sub(1));
    let mut n_vertices = 0;
    for (i, ring) in polygon.iter().enumerate() {
        if i > 0 {
            hole_indices.push(n_vertices);
        }
        n_vertices += ring.as_ref().len() as u32;
    }
    let points = polygon.iter().flat_map(|ring| ring.as_ref().iter().map(|p| [p.x.into(), p.y.into()]));
    let mut indices = Vec::new();
    EARCUT.with_borrow_mut(|earcut| earcut.earcut(points, &hole_indices, &mut indices));
    indices
}

#[cfg(test)]
mod tests {
    use super::*;

    fn area(points: &[Point<i16>], indices: &[u32]) -> f64 {
        indices
            .as_chunks::<3>()
            .0
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
        let indices = triangulate(&polygon);
        let points: Vec<Point<i16>> = polygon.concat();
        assert!(indices.iter().all(|&i| (i as usize) < points.len()));
        assert_eq!(area(&points, &indices), 100.0 - 36.0);
    }

    #[test]
    fn an_empty_outer_ring_has_no_triangles() {
        let hole = vec![Point::new(2i16, 2), Point::new(8, 2), Point::new(8, 8)];
        assert_eq!(triangulate(&[Vec::new(), hole]), Vec::<u32>::new());
    }
}
