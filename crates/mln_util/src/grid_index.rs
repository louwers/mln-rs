// The C++ `insert`, `hitTest` and private `query` overloads on `BBox` and
// `BCircle` become `insert`/`insert_circle`, `hit_test`/`hit_test_circle`
// and `query_box_fn`/`query_circle_fn`. The explicit instantiation in
// grid_index.cpp (`GridIndex<RefIndexedSubfeature>`) needs no Rust
// equivalent.

use std::collections::HashSet;

use geometry::Point;
use mln_math::{fmax, fmin};

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct GeometryBox<T> {
    pub min: Point<T>,
    pub max: Point<T>,
}

impl<T> GeometryBox<T> {
    pub const fn new(min: Point<T>, max: Point<T>) -> Self {
        GeometryBox { min, max }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Circle<T> {
    pub center: Point<T>,
    pub(crate) radius: T,
}

impl<T> Circle<T> {
    pub const fn new(center: Point<T>, radius: T) -> Self {
        Circle { center, radius }
    }
}

pub type BBox = GeometryBox<f32>;
pub type BCircle = Circle<f32>;

/// `GridIndex` is a data structure for testing the intersection of
/// circles and rectangles in a 2d plane.
/// It is optimized for rapid insertion and querying.
/// `GridIndex` splits the plane into a set of "cells" and keeps track
/// of which geometries intersect with each cell. At query time,
/// full geometry comparisons are only done for items that share
/// at least one cell. As long as the geometries are relatively
/// uniformly distributed across the plane, this greatly reduces
/// the number of comparisons necessary.
#[derive(Clone, Debug)]
pub struct GridIndex<T> {
    width: f32,
    height: f32,

    estimated_elements_per_cell: usize,

    x_cell_count: usize,
    y_cell_count: usize,
    x_scale: f64,
    y_scale: f64,

    box_elements: Vec<(T, BBox)>,
    circle_elements: Vec<(T, BCircle)>,

    box_cells: Vec<Vec<u32>>,
    circle_cells: Vec<Vec<u32>>,
}

impl<T: Clone> GridIndex<T> {
    #[must_use]
    pub fn new(width: f32, height: f32, cell_size: u32) -> Self {
        // `std::ceil(width / cellSize_)`: float division (the cell size is
        // converted to float), ceilf.
        let x_cell_count = (width / cell_size as f32).ceil() as usize;
        let y_cell_count = (height / cell_size as f32).ceil() as usize;
        // `xCellCount / width` is a float division, stored as double.
        let x_scale = f64::from(x_cell_count as f32 / width);
        let y_scale = f64::from(y_cell_count as f32 / height);
        debug_assert!(width > 0.0);
        debug_assert!(height > 0.0);
        GridIndex {
            width,
            height,
            estimated_elements_per_cell: 0,
            x_cell_count,
            y_cell_count,
            x_scale,
            y_scale,
            box_elements: Vec::new(),
            circle_elements: Vec::new(),
            box_cells: vec![Vec::new(); x_cell_count * y_cell_count],
            circle_cells: vec![Vec::new(); x_cell_count * y_cell_count],
        }
    }

    /// Set the expected number of elements per cell to avoid small
    /// re-allocations for populated cells.
    pub fn reserve(&mut self, value: usize) {
        self.estimated_elements_per_cell = value;
    }

    /// Port-only: the inserted boxes, in insertion order. Inserting them
    /// again into an empty index of the same size rebuilds this one.
    #[must_use]
    pub fn box_elements(&self) -> &[(T, BBox)] {
        &self.box_elements
    }

    /// Port-only: whether circles were inserted (`box_elements` doesn't
    /// list them).
    #[must_use]
    pub fn has_circles(&self) -> bool {
        !self.circle_elements.is_empty()
    }

    pub fn insert(&mut self, t: T, bbox: &BBox) {
        debug_assert!(self.box_elements.len() < u32::MAX as usize);
        let uid = self.box_elements.len() as u32;

        let cx1 = self.convert_to_x_cell_coord(bbox.min.x);
        let cy1 = self.convert_to_y_cell_coord(bbox.min.y);
        let cx2 = self.convert_to_x_cell_coord(bbox.max.x);
        let cy2 = self.convert_to_y_cell_coord(bbox.max.y);

        for x in cx1..=cx2 {
            for y in cy1..=cy2 {
                let cell = &mut self.box_cells[self.x_cell_count * y + x];
                if self.estimated_elements_per_cell != 0 && cell.is_empty() {
                    cell.reserve(self.estimated_elements_per_cell);
                }
                cell.push(uid);
            }
        }

        self.box_elements.push((t, *bbox));
    }

    pub fn insert_circle(&mut self, t: T, bcircle: &BCircle) {
        debug_assert!(self.circle_elements.len() < u32::MAX as usize);
        let uid = self.circle_elements.len() as u32;

        let cx1 = self.convert_to_x_cell_coord(bcircle.center.x - bcircle.radius);
        let cy1 = self.convert_to_y_cell_coord(bcircle.center.y - bcircle.radius);
        let cx2 = self.convert_to_x_cell_coord(bcircle.center.x + bcircle.radius);
        let cy2 = self.convert_to_y_cell_coord(bcircle.center.y + bcircle.radius);

        for x in cx1..=cx2 {
            for y in cy1..=cy2 {
                let cell = &mut self.circle_cells[self.x_cell_count * y + x];
                if self.estimated_elements_per_cell != 0 && cell.is_empty() {
                    cell.reserve(self.estimated_elements_per_cell);
                }
                cell.push(uid);
            }
        }

        self.circle_elements.push((t, *bcircle));
    }

    #[must_use]
    pub fn query(&self, query_bbox: &BBox) -> Vec<T> {
        let mut result = Vec::new();
        self.query_box_fn(query_bbox, &mut |t: &T, _: &BBox| -> bool {
            result.push(t.clone());
            false
        });
        result
    }

    #[must_use]
    pub fn query_with_boxes(&self, query_bbox: &BBox) -> Vec<(T, BBox)> {
        let mut result = Vec::new();
        self.query_box_fn(query_bbox, &mut |t: &T, bbox: &BBox| -> bool {
            result.push((t.clone(), *bbox));
            false
        });
        result
    }

    #[must_use]
    pub fn hit_test(&self, query_bbox: &BBox, predicate: Option<&dyn Fn(&T) -> bool>) -> bool {
        let mut hit = false;
        self.query_box_fn(query_bbox, &mut |t: &T, _: &BBox| -> bool {
            if predicate.is_none_or(|predicate| predicate(t)) {
                hit = true;
                true
            } else {
                false
            }
        });
        hit
    }

    #[must_use]
    pub fn hit_test_circle(&self, query_bcircle: &BCircle, predicate: Option<&dyn Fn(&T) -> bool>) -> bool {
        let mut hit = false;
        self.query_circle_fn(query_bcircle, &mut |t: &T, _: &BBox| -> bool {
            if predicate.is_none_or(|predicate| predicate(t)) {
                hit = true;
                true
            } else {
                false
            }
        });
        hit
    }

    #[must_use]
    pub fn empty(&self) -> bool {
        self.box_elements.is_empty() && self.circle_elements.is_empty()
    }

    fn no_intersection(&self, query_bbox: &BBox) -> bool {
        query_bbox.max.x < 0.0
            || query_bbox.min.x >= self.width
            || query_bbox.max.y < 0.0
            || query_bbox.min.y >= self.height
    }

    fn complete_intersection(&self, query_bbox: &BBox) -> bool {
        query_bbox.min.x <= 0.0
            && query_bbox.min.y <= 0.0
            && self.width <= query_bbox.max.x
            && self.height <= query_bbox.max.y
    }

    fn convert_to_box(circle: &BCircle) -> BBox {
        BBox::new(
            Point::new(circle.center.x - circle.radius, circle.center.y - circle.radius),
            Point::new(circle.center.x + circle.radius, circle.center.y + circle.radius),
        )
    }

    /// The private `query(const BBox&, std::function<bool(const T&, const BBox&)>)`.
    fn query_box_fn(&self, query_bbox: &BBox, result_fn: &mut dyn FnMut(&T, &BBox) -> bool) {
        let mut seen_boxes: HashSet<u32> = HashSet::new();
        let mut seen_circles: HashSet<u32> = HashSet::new();

        if self.no_intersection(query_bbox) {
            return;
        } else if self.complete_intersection(query_bbox) {
            for element in &self.box_elements {
                if result_fn(&element.0, &element.1) {
                    return;
                }
            }
            for element in &self.circle_elements {
                if result_fn(&element.0, &Self::convert_to_box(&element.1)) {
                    return;
                }
            }
            return;
        }

        let cx1 = self.convert_to_x_cell_coord(query_bbox.min.x);
        let cy1 = self.convert_to_y_cell_coord(query_bbox.min.y);
        let cx2 = self.convert_to_x_cell_coord(query_bbox.max.x);
        let cy2 = self.convert_to_y_cell_coord(query_bbox.max.y);

        for x in cx1..=cx2 {
            for y in cy1..=cy2 {
                let cell_index = self.x_cell_count * y + x;
                // Look up other boxes
                for &uid in &self.box_cells[cell_index] {
                    if !seen_boxes.contains(&uid) {
                        seen_boxes.insert(uid);

                        let pair = &self.box_elements[uid as usize];
                        let bbox = &pair.1;
                        if Self::boxes_collide(query_bbox, bbox) && result_fn(&pair.0, bbox) {
                            return;
                        }
                    }
                }

                // Look up circles
                for &uid in &self.circle_cells[cell_index] {
                    if !seen_circles.contains(&uid) {
                        seen_circles.insert(uid);

                        let pair = &self.circle_elements[uid as usize];
                        let bcircle = &pair.1;
                        if Self::circle_and_box_collide(bcircle, query_bbox)
                            && result_fn(&pair.0, &Self::convert_to_box(bcircle))
                        {
                            return;
                        }
                    }
                }
            }
        }
    }

    /// The private `query(const BCircle&, std::function<bool(const T&, const BBox&)>)`.
    fn query_circle_fn(&self, query_bcircle: &BCircle, result_fn: &mut dyn FnMut(&T, &BBox) -> bool) {
        let mut seen_boxes: HashSet<u32> = HashSet::new();
        let mut seen_circles: HashSet<u32> = HashSet::new();

        let query_bbox = Self::convert_to_box(query_bcircle);
        if self.no_intersection(&query_bbox) {
            return;
        } else if self.complete_intersection(&query_bbox) {
            for element in &self.box_elements {
                if result_fn(&element.0, &element.1) {
                    return;
                }
            }
            for element in &self.circle_elements {
                if result_fn(&element.0, &Self::convert_to_box(&element.1)) {
                    return;
                }
            }
            // C++ quirk: unlike the BBox overload, there is no `return` here,
            // so the cell lookup below runs too (and may report elements a
            // second time).
        }

        let cx1 = self.convert_to_x_cell_coord(query_bcircle.center.x - query_bcircle.radius);
        let cy1 = self.convert_to_y_cell_coord(query_bcircle.center.y - query_bcircle.radius);
        let cx2 = self.convert_to_x_cell_coord(query_bcircle.center.x + query_bcircle.radius);
        let cy2 = self.convert_to_y_cell_coord(query_bcircle.center.y + query_bcircle.radius);

        for x in cx1..=cx2 {
            for y in cy1..=cy2 {
                let cell_index = self.x_cell_count * y + x;
                // Look up boxes
                for &uid in &self.box_cells[cell_index] {
                    if !seen_boxes.contains(&uid) {
                        seen_boxes.insert(uid);

                        let pair = &self.box_elements[uid as usize];
                        let bbox = &pair.1;
                        if Self::circle_and_box_collide(query_bcircle, bbox) && result_fn(&pair.0, bbox) {
                            return;
                        }
                    }
                }

                // Look up other circles
                for &uid in &self.circle_cells[cell_index] {
                    if !seen_circles.contains(&uid) {
                        seen_circles.insert(uid);

                        let pair = &self.circle_elements[uid as usize];
                        let bcircle = &pair.1;
                        if Self::circles_collide(query_bcircle, bcircle)
                            && result_fn(&pair.0, &Self::convert_to_box(bcircle))
                        {
                            return;
                        }
                    }
                }
            }
        }
    }

    /// The cell column of `x`, clamped to the grid:
    /// `fmax(0.0, fmin(x_cell_count - 1.0, floor(x * x_scale)))` in double.
    fn convert_to_x_cell_coord(&self, x: f32) -> usize {
        fmax(0.0, fmin(self.x_cell_count as f64 - 1.0, (f64::from(x) * self.x_scale).floor())) as usize
    }

    fn convert_to_y_cell_coord(&self, y: f32) -> usize {
        fmax(0.0, fmin(self.y_cell_count as f64 - 1.0, (f64::from(y) * self.y_scale).floor())) as usize
    }

    fn boxes_collide(first: &BBox, second: &BBox) -> bool {
        first.min.x <= second.max.x
            && first.min.y <= second.max.y
            && first.max.x >= second.min.x
            && first.max.y >= second.min.y
    }

    fn circles_collide(first: &BCircle, second: &BCircle) -> bool {
        let dx = second.center.x - first.center.x;
        let dy = second.center.y - first.center.y;
        let both_radii = first.radius + second.radius;
        (both_radii * both_radii) > (dx * dx + dy * dy)
    }

    fn circle_and_box_collide(circle: &BCircle, box_: &BBox) -> bool {
        let half_rect_width = (box_.max.x - box_.min.x) / 2.0;
        let dist_x = (circle.center.x - (box_.min.x + half_rect_width)).abs();
        if dist_x > (half_rect_width + circle.radius) {
            return false;
        }

        let half_rect_height = (box_.max.y - box_.min.y) / 2.0;
        let dist_y = (circle.center.y - (box_.min.y + half_rect_height)).abs();
        if dist_y > (half_rect_height + circle.radius) {
            return false;
        }

        if dist_x <= half_rect_width || dist_y <= half_rect_height {
            return true;
        }

        let dx = dist_x - half_rect_width;
        let dy = dist_y - half_rect_height;
        (dx * dx + dy * dy) <= (circle.radius * circle.radius)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bbox(x1: f32, y1: f32, x2: f32, y2: f32) -> BBox {
        BBox::new(Point::new(x1, y1), Point::new(x2, y2))
    }

    fn bcircle(x: f32, y: f32, r: f32) -> BCircle {
        BCircle::new(Point::new(x, y), r)
    }

    // gtest: GridIndex.IndexesFeatures
    #[test]
    fn indexes_features() {
        let mut grid: GridIndex<i16> = GridIndex::new(100.0, 100.0, 10);
        grid.insert(0, &bbox(4.0, 10.0, 6.0, 30.0));
        grid.insert(1, &bbox(4.0, 10.0, 30.0, 12.0));
        grid.insert(2, &bbox(-10.0, 30.0, 5.0, 35.0));

        assert_eq!(grid.query(&bbox(4.0, 10.0, 5.0, 11.0)), vec![0, 1]);
        assert_eq!(grid.query(&bbox(24.0, 10.0, 25.0, 11.0)), vec![1]);
        assert_eq!(grid.query(&bbox(40.0, 40.0, 100.0, 100.0)), Vec::<i16>::new());
        assert_eq!(grid.query(&bbox(-6.0, 0.0, 3.0, 100.0)), vec![2]);
        assert_eq!(grid.query(&bbox(-1000.0, -1000.0, 1000.0, 1000.0)), vec![0, 1, 2]);
    }

    // gtest: GridIndex.DuplicateKeys
    #[test]
    fn duplicate_keys() {
        const KEY: i16 = 123;
        let mut grid: GridIndex<i16> = GridIndex::new(100.0, 100.0, 10);
        grid.insert(KEY, &bbox(3.0, 4.0, 4.0, 4.0));
        grid.insert(KEY, &bbox(13.0, 13.0, 14.0, 14.0));
        grid.insert(KEY, &bbox(23.0, 23.0, 24.0, 24.0));

        assert_eq!(grid.query(&bbox(0.0, 0.0, 30.0, 30.0)), vec![KEY, KEY, KEY]);
    }

    // gtest: GridIndex.CircleCircle
    #[test]
    fn circle_circle() {
        let mut grid: GridIndex<i16> = GridIndex::new(100.0, 100.0, 10);
        grid.insert_circle(0, &bcircle(50.0, 50.0, 10.0));
        grid.insert_circle(1, &bcircle(60.0, 60.0, 15.0));
        grid.insert_circle(2, &bcircle(-10.0, 110.0, 20.0));

        assert!(grid.hit_test_circle(&bcircle(55.0, 55.0, 2.0), None));
        assert!(!grid.hit_test_circle(&bcircle(10.0, 10.0, 10.0), None));
        assert!(grid.hit_test_circle(&bcircle(0.0, 100.0, 10.0), None));
        assert!(grid.hit_test_circle(&bcircle(80.0, 60.0, 10.0), None));
    }

    // gtest: GridIndex.CircleBox
    #[test]
    fn circle_box() {
        let mut grid: GridIndex<i16> = GridIndex::new(100.0, 100.0, 10);
        grid.insert_circle(0, &bcircle(50.0, 50.0, 10.0));
        grid.insert_circle(1, &bcircle(60.0, 60.0, 15.0));
        grid.insert_circle(2, &bcircle(-10.0, 110.0, 20.0));

        assert_eq!(grid.query(&bbox(45.0, 45.0, 55.0, 55.0)), vec![0, 1]);
        assert_eq!(grid.query(&bbox(0.0, 0.0, 30.0, 30.0)), Vec::<i16>::new());
        assert_eq!(grid.query(&bbox(0.0, 80.0, 20.0, 100.0)), vec![2]);
    }

    // gtest: GridIndex.IndexesFeaturesOverflow
    #[test]
    fn indexes_features_overflow() {
        let mut grid: GridIndex<i16> = GridIndex::new(5000.0, 5000.0, 25);
        grid.insert(0, &bbox(4500.0, 4500.0, 4900.0, 4900.0));
        assert_eq!(grid.query(&bbox(4000.0, 4000.0, 5000.0, 5000.0)), vec![0]);
    }
}
