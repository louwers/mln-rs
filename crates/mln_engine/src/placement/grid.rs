//! A uniform grid of boxes and circles, which placement fills with the
//! shapes it places and tests each new shape against.

use glam::Vec2;

/// An axis-aligned box, from its `min` corner to its `max` corner.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Aabb {
    pub min: Vec2,
    pub max: Vec2,
}

impl Aabb {
    #[must_use]
    pub const fn new(min: Vec2, max: Vec2) -> Self {
        Aabb { min, max }
    }

    /// Whether the boxes overlap or touch.
    fn intersects(&self, other: &Aabb) -> bool {
        self.min.cmple(other.max).all() && other.min.cmple(self.max).all()
    }
}

/// A collision shape in viewport pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Shape {
    Box(Aabb),
    Circle { center: Vec2, radius: f32 },
}

impl Shape {
    /// The smallest box that holds the shape.
    #[must_use]
    pub(crate) fn bounds(&self) -> Aabb {
        match *self {
            Shape::Box(aabb) => aabb,
            Shape::Circle { center, radius } => Aabb::new(center - radius, center + radius),
        }
    }

    /// Whether the shapes overlap. Boxes that touch overlap; circles that
    /// touch don't.
    #[must_use]
    pub(crate) fn intersects(&self, other: &Shape) -> bool {
        match (self, other) {
            (Shape::Box(a), Shape::Box(b)) => a.intersects(b),
            (Shape::Circle { center: a, radius: ra }, Shape::Circle { center: b, radius: rb }) => {
                let radii = ra + rb;
                radii * radii > a.distance_squared(*b)
            }
            (Shape::Box(aabb), &Shape::Circle { center, radius })
            | (&Shape::Circle { center, radius }, Shape::Box(aabb)) => {
                let half_size = (aabb.max - aabb.min) / 2.0;
                let distance = (center - (aabb.min + half_size)).abs();
                if distance.cmpgt(half_size + radius).any() {
                    return false;
                }
                if distance.cmple(half_size).any() {
                    return true;
                }
                (distance - half_size).length_squared() <= radius * radius
            }
        }
    }
}

/// Shapes in a plane from the origin to `size`, each with a value. The
/// plane is cut into square cells, and each cell lists the shapes whose
/// bounds touch it, so a query tests only the shapes that share a cell with
/// it. A shape outside the plane is listed in the cells at its edge.
#[derive(Clone, Debug)]
pub(crate) struct Grid<T> {
    size: Vec2,
    cell_size: f32,
    columns: usize,
    rows: usize,
    /// Cells per unit, in x and y.
    scale: Vec2,
    elements: Vec<(T, Shape)>,
    /// The indices into `elements` of each cell's shapes, row by row.
    cells: Vec<Vec<u32>>,
}

impl<T> Grid<T> {
    /// An empty grid over a plane of `size`, with cells `cell_size` wide.
    #[must_use]
    pub(crate) fn new(size: Vec2, cell_size: f32) -> Self {
        debug_assert!(size.cmpgt(Vec2::ZERO).all() && cell_size > 0.0);
        let cell_counts = (size / cell_size).ceil().max(Vec2::ONE);
        let (columns, rows) = (cell_counts.x as usize, cell_counts.y as usize);
        Grid {
            size,
            cell_size,
            columns,
            rows,
            scale: cell_counts / size,
            elements: Vec::new(),
            cells: (0..columns * rows).map(|_| Vec::new()).collect(),
        }
    }

    /// Empties the grid and sizes it as [`new`](Self::new) does, keeping its
    /// allocations when the size and cell size are the ones it had.
    pub(crate) fn reset(&mut self, size: Vec2, cell_size: f32) {
        if size != self.size || cell_size != self.cell_size {
            *self = Grid::new(size, cell_size);
            return;
        }
        self.elements.clear();
        for cell in &mut self.cells {
            cell.clear();
        }
    }

    #[must_use]
    pub(crate) fn is_empty(&self) -> bool {
        self.elements.is_empty()
    }

    pub(crate) fn insert(&mut self, value: T, shape: Shape) {
        debug_assert!(self.elements.len() < u32::MAX as usize);
        let id = self.elements.len() as u32;
        let ((x1, y1), (x2, y2)) = self.cell_range(&shape.bounds());
        for y in y1..=y2 {
            for cell in self.cells[y * self.columns..][..self.columns].iter_mut().take(x2 + 1).skip(x1) {
                cell.push(id);
            }
        }
        self.elements.push((value, shape));
    }

    /// Whether `hit` accepts a value whose shape intersects `shape`. It stops
    /// at the first it accepts, and may be called more than once with a
    /// value.
    pub(crate) fn any(&self, shape: &Shape, mut hit: impl FnMut(&T) -> bool) -> bool {
        self.find_candidate(&shape.bounds(), |id| {
            let (value, other) = &self.elements[id as usize];
            shape.intersects(other) && hit(value)
        })
    }

    /// The values whose shapes intersect `shape`, each once, in the order
    /// they were inserted, with their shapes.
    pub(crate) fn query(&self, shape: &Shape) -> impl Iterator<Item = (&T, &Shape)> {
        let mut ids = Vec::new();
        self.find_candidate(&shape.bounds(), |id| {
            ids.push(id);
            false
        });
        ids.sort_unstable();
        ids.dedup();
        ids.into_iter()
            .map(|id| {
                let (value, other) = &self.elements[id as usize];
                (value, other)
            })
            .filter(move |(_, other)| shape.intersects(other))
    }

    /// Calls `f` with the index of every shape in a cell that `bounds`
    /// touches, until it returns true, and returns whether it did. A shape
    /// in several of those cells comes up for each.
    fn find_candidate(&self, bounds: &Aabb, mut f: impl FnMut(u32) -> bool) -> bool {
        if self.elements.is_empty() || bounds.max.cmplt(Vec2::ZERO).any() || bounds.min.cmpge(self.size).any() {
            return false;
        }
        if bounds.min.cmple(Vec2::ZERO).all() && bounds.max.cmpge(self.size).all() {
            // Every cell: each shape once.
            return (0..self.elements.len() as u32).any(f);
        }
        let ((x1, y1), (x2, y2)) = self.cell_range(bounds);
        (y1..=y2).any(|y| {
            self.cells[y * self.columns..][..self.columns].iter().take(x2 + 1).skip(x1).flatten().any(|&id| f(id))
        })
    }

    /// The columns and rows of the cells at `bounds`' corners, clamped to
    /// the grid. Bounds reversed across cells, as negative padding makes,
    /// give a range whose start cell is past its end, which is empty.
    fn cell_range(&self, bounds: &Aabb) -> ((usize, usize), (usize, usize)) {
        // Casting saturates: a negative or NaN coordinate is in the first
        // cell.
        let cell = |p: Vec2| {
            let p = p * self.scale;
            ((p.x as usize).min(self.columns - 1), (p.y as usize).min(self.rows - 1))
        };
        (cell(bounds.min), cell(bounds.max))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn aabb(x1: f32, y1: f32, x2: f32, y2: f32) -> Shape {
        Shape::Box(Aabb::new(Vec2::new(x1, y1), Vec2::new(x2, y2)))
    }

    fn circle(x: f32, y: f32, radius: f32) -> Shape {
        Shape::Circle { center: Vec2::new(x, y), radius }
    }

    fn query(grid: &Grid<i16>, shape: Shape) -> Vec<i16> {
        grid.query(&shape).map(|(value, _)| *value).collect()
    }

    #[test]
    fn a_reversed_box_is_in_no_cell() {
        let mut grid = Grid::new(Vec2::splat(100.0), 10.0);
        grid.insert(0, aabb(60.0, 10.0, 20.0, 30.0));
        assert!(!grid.any(&aabb(30.0, 15.0, 40.0, 25.0), |_| true));
        assert_eq!(query(&grid, aabb(60.0, 10.0, 20.0, 30.0)), Vec::<i16>::new());
    }

    #[test]
    fn indexes_boxes() {
        let mut grid = Grid::new(Vec2::splat(100.0), 10.0);
        grid.insert(0, aabb(4.0, 10.0, 6.0, 30.0));
        grid.insert(1, aabb(4.0, 10.0, 30.0, 12.0));
        grid.insert(2, aabb(-10.0, 30.0, 5.0, 35.0));

        assert_eq!(query(&grid, aabb(4.0, 10.0, 5.0, 11.0)), [0, 1]);
        assert_eq!(query(&grid, aabb(24.0, 10.0, 25.0, 11.0)), [1]);
        assert_eq!(query(&grid, aabb(40.0, 40.0, 100.0, 100.0)), [0i16; 0]);
        assert_eq!(query(&grid, aabb(-6.0, 0.0, 3.0, 100.0)), [2]);
        assert_eq!(query(&grid, aabb(-1000.0, -1000.0, 1000.0, 1000.0)), [0, 1, 2]);
    }

    #[test]
    fn reset_empties_the_grid() {
        let mut grid = Grid::new(Vec2::splat(100.0), 10.0);
        grid.insert(0, aabb(4.0, 10.0, 6.0, 30.0));
        grid.insert(1, circle(50.0, 50.0, 5.0));
        let everything = aabb(-1000.0, -1000.0, 1000.0, 1000.0);

        grid.reset(Vec2::splat(100.0), 10.0);
        assert!(grid.is_empty());
        assert_eq!(query(&grid, everything), [0i16; 0]);
        grid.insert(2, aabb(4.0, 10.0, 6.0, 30.0));
        assert_eq!(query(&grid, everything), [2]);

        grid.reset(Vec2::new(200.0, 50.0), 10.0);
        assert!(grid.is_empty());
        grid.insert(3, aabb(150.0, 10.0, 160.0, 20.0));
        assert_eq!(query(&grid, aabb(155.0, 15.0, 156.0, 16.0)), [3]);
    }

    #[test]
    fn duplicate_values() {
        let mut grid = Grid::new(Vec2::splat(100.0), 10.0);
        grid.insert(123, aabb(3.0, 4.0, 4.0, 4.0));
        grid.insert(123, aabb(13.0, 13.0, 14.0, 14.0));
        grid.insert(123, aabb(23.0, 23.0, 24.0, 24.0));

        assert_eq!(query(&grid, aabb(0.0, 0.0, 30.0, 30.0)), [123, 123, 123]);
    }

    #[test]
    fn circles_hit_circles() {
        let mut grid = Grid::new(Vec2::splat(100.0), 10.0);
        grid.insert(0, circle(50.0, 50.0, 10.0));
        grid.insert(1, circle(60.0, 60.0, 15.0));
        grid.insert(2, circle(-10.0, 110.0, 20.0));

        let any = |shape| grid.any(&shape, |_| true);
        assert!(any(circle(55.0, 55.0, 2.0)));
        assert!(!any(circle(10.0, 10.0, 10.0)));
        assert!(any(circle(0.0, 100.0, 10.0)));
        assert!(any(circle(80.0, 60.0, 10.0)));
    }

    #[test]
    fn boxes_hit_circles() {
        let mut grid = Grid::new(Vec2::splat(100.0), 10.0);
        grid.insert(0, circle(50.0, 50.0, 10.0));
        grid.insert(1, circle(60.0, 60.0, 15.0));
        grid.insert(2, circle(-10.0, 110.0, 20.0));

        assert_eq!(query(&grid, aabb(45.0, 45.0, 55.0, 55.0)), [0, 1]);
        assert_eq!(query(&grid, aabb(0.0, 0.0, 30.0, 30.0)), [0i16; 0]);
        assert_eq!(query(&grid, aabb(0.0, 80.0, 20.0, 100.0)), [2]);
    }

    #[test]
    fn any_skips_rejected_values() {
        let mut grid = Grid::new(Vec2::splat(100.0), 10.0);
        grid.insert(0, aabb(0.0, 0.0, 50.0, 50.0));
        grid.insert(1, circle(30.0, 30.0, 10.0));

        assert!(grid.any(&aabb(25.0, 25.0, 35.0, 35.0), |&value| value == 1));
        assert!(!grid.any(&aabb(25.0, 25.0, 35.0, 35.0), |&value| value == 2));
        // The whole grid takes the path that skips the cells.
        assert!(grid.any(&aabb(-1.0, -1.0, 101.0, 101.0), |&value| value == 1));
        assert!(!grid.any(&aabb(-1.0, -1.0, 101.0, 101.0), |&value| value == 2));
    }

    #[test]
    fn large_grids_reach_their_last_cell() {
        let mut grid = Grid::new(Vec2::splat(5000.0), 25.0);
        grid.insert(0, aabb(4500.0, 4500.0, 4900.0, 4900.0));
        assert_eq!(query(&grid, aabb(4000.0, 4000.0, 5000.0, 5000.0)), [0]);
    }
}
