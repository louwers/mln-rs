// Ports vendor/polylabel/include/mapbox/polylabel.hpp (mapbox::polylabel and
// mapbox::detail::{getSegDistSq, pointToPolygonDist, Cell,
// getCentroidCell}) for `T = double`, the instantiation
// layout/symbol_layout.cpp uses.
//
// The debug printing is not ported.

use std::cmp::Ordering;
use std::collections::BinaryHeap;

use geometry::{LinearRing, Point, Polygon};

/// `polygon.at(0)` / `ring.at(0)` on an empty vector (`std::out_of_range`).
const OUT_OF_RANGE: &str = "vector::_M_range_check: __n (which is 0) >= this->size() (which is 0)";

// get squared distance from a point to a segment
#[must_use]
pub fn get_seg_dist_sq(p: &Point<f64>, a: &Point<f64>, b: &Point<f64>) -> f64 {
    let mut x = a.x;
    let mut y = a.y;
    let mut dx = b.x - x;
    let mut dy = b.y - y;

    if dx != 0.0 || dy != 0.0 {
        let t = ((p.x - x) * dx + (p.y - y) * dy) / (dx * dx + dy * dy);

        if t > 1.0 {
            x = b.x;
            y = b.y;
        } else if t > 0.0 {
            x += dx * t;
            y += dy * t;
        }
    }

    dx = p.x - x;
    dy = p.y - y;

    dx * dx + dy * dy
}

// signed distance from point to polygon outline (negative if point is
// outside)
#[must_use]
pub fn point_to_polygon_dist(point: &Point<f64>, polygon: &Polygon<f64>) -> f64 {
    let mut inside = false;
    let mut min_dist_sq = f64::INFINITY;

    for ring in polygon {
        let len = ring.len();
        let mut j = len.wrapping_sub(1);
        for i in 0..len {
            let a = &ring[i];
            let b = &ring[j];

            if (a.y > point.y) != (b.y > point.y) && (point.x < (b.x - a.x) * (point.y - a.y) / (b.y - a.y) + a.x) {
                inside = !inside;
            }

            min_dist_sq = f64::min(min_dist_sq, get_seg_dist_sq(point, a, b));
            j = i;
        }
    }

    (if inside { 1.0 } else { -1.0 }) * min_dist_sq.sqrt()
}

/// `Cell<double>`
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Cell {
    /// cell center
    pub c: Point<f64>,
    /// half the cell size
    pub h: f64,
    /// distance from cell center to polygon
    pub d: f64,
    /// max distance to polygon within a cell
    pub max: f64,
}

impl Cell {
    #[must_use]
    pub fn new(c: Point<f64>, h: f64, polygon: &Polygon<f64>) -> Self {
        let d = point_to_polygon_dist(&c, polygon);
        Cell { c, h, d, max: d + h * std::f64::consts::SQRT_2 }
    }
}

/// A [`Cell`] in the cell queue, ordered by `max`.
struct QueuedCell(Cell);

impl Ord for QueuedCell {
    fn cmp(&self, other: &Self) -> Ordering {
        self.0.max.total_cmp(&other.0.max)
    }
}

impl PartialOrd for QueuedCell {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl PartialEq for QueuedCell {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other).is_eq()
    }
}

impl Eq for QueuedCell {}

// get polygon centroid
pub fn get_centroid_cell(polygon: &Polygon<f64>) -> Result<Cell, String> {
    let mut area = 0.0;
    let mut c = Point::new(0.0, 0.0);
    let ring = polygon.first().ok_or(OUT_OF_RANGE)?;

    let len = ring.len();
    let mut j = len.wrapping_sub(1);
    for i in 0..len {
        let a = &ring[i];
        let b = &ring[j];
        let f = a.x * b.y - b.x * a.y;
        c.x += (a.x + b.x) * f;
        c.y += (a.y + b.y) * f;
        area += f * 3.0;
        j = i;
    }

    let centre = if area == 0.0 { *ring.first().ok_or(OUT_OF_RANGE)? } else { Point::new(c.x / area, c.y / area) };
    Ok(Cell::new(centre, 0.0, polygon))
}

/// `mapbox::geometry::envelope(ring)`: (min, max).
fn envelope(ring: &LinearRing<f64>) -> (Point<f64>, Point<f64>) {
    let mut min = Point::new(f64::INFINITY, f64::INFINITY);
    let mut max = Point::new(f64::NEG_INFINITY, f64::NEG_INFINITY);
    for point in ring {
        if min.x > point.x {
            min.x = point.x;
        }
        if min.y > point.y {
            min.y = point.y;
        }
        if max.x < point.x {
            max.x = point.x;
        }
        if max.y < point.y {
            max.y = point.y;
        }
    }
    (min, max)
}

/// `polylabel(polygon, precision)` (C++ default precision: 1). `Err` is the
/// `std::out_of_range` of an empty polygon or outer ring.
pub fn polylabel(polygon: &Polygon<f64>, precision: f64) -> Result<Point<f64>, String> {
    // find the bounding box of the outer ring
    let (min, max) = envelope(polygon.first().ok_or(OUT_OF_RANGE)?);

    let size = Point::new(max.x - min.x, max.y - min.y);

    let cell_size = f64::min(size.x, size.y);
    let mut h = cell_size / 2.0;

    // a priority queue of cells in order of their "potential" (max distance
    // to polygon)
    let mut cell_queue = BinaryHeap::new();

    if cell_size == 0.0 {
        return Ok(min);
    }

    // cover polygon with initial cells
    let mut x = min.x;
    while x < max.x {
        let mut y = min.y;
        while y < max.y {
            cell_queue.push(QueuedCell(Cell::new(Point::new(x + h, y + h), h, polygon)));
            y += cell_size;
        }
        x += cell_size;
    }

    // take centroid as the first best guess
    let mut best_cell = get_centroid_cell(polygon)?;

    // special case for rectangular polygons
    let bbox_cell = Cell::new(Point::new(min.x + size.x / 2.0, min.y + size.y / 2.0), 0.0, polygon);
    if bbox_cell.d > best_cell.d {
        best_cell = bbox_cell;
    }

    // pick the most promising cell from the queue
    while let Some(QueuedCell(cell)) = cell_queue.pop() {
        // update the best cell if we found a better one
        if cell.d > best_cell.d {
            best_cell = cell;
        }

        // do not drill down further if there's no chance of a better solution
        if cell.max - best_cell.d <= precision {
            continue;
        }

        // split the cell into four cells
        h = cell.h / 2.0;
        cell_queue.push(QueuedCell(Cell::new(Point::new(cell.c.x - h, cell.c.y - h), h, polygon)));
        cell_queue.push(QueuedCell(Cell::new(Point::new(cell.c.x + h, cell.c.y - h), h, polygon)));
        cell_queue.push(QueuedCell(Cell::new(Point::new(cell.c.x - h, cell.c.y + h), h, polygon)));
        cell_queue.push(QueuedCell(Cell::new(Point::new(cell.c.x + h, cell.c.y + h), h, polygon)));
    }

    Ok(best_cell.c)
}
