//! Ports `test/text/calculate_tile_distances.test.cpp`.

use geometry::Point;
use mln_style::feature::GeometryCoordinates;

use crate::layers::symbol::anchor::Anchor;
use crate::layers::symbol::layout::SymbolLayout;

// gtest: calculateTileDistances.Point
#[test]
fn point() {
    let line: GeometryCoordinates = vec![Point::new(1i16, 1)];
    let distances = SymbolLayout::tile_distances(&line, &Anchor::new(1.0, 1.0, 0.0, Some(0)));
    assert_eq!(distances, vec![0.0f32]);
}

// gtest: calculateTileDistances.Line
#[test]
fn line() {
    let line: GeometryCoordinates = vec![Point::new(1i16, 1), Point::new(1, 2), Point::new(1, 3), Point::new(1, 4)];
    let distances = SymbolLayout::tile_distances(&line, &Anchor::new(1.0, 3.0, 0.0, Some(2)));
    assert_eq!(distances, vec![2.0f32, 1.0, 0.0, 1.0]);
}

// gtest: calculateTileDistances.EmptySegment
#[test]
fn empty_segment() {
    let line: GeometryCoordinates = Vec::new();
    let distances = SymbolLayout::tile_distances(&line, &Anchor::new(1.0, 1.0, 0.0, None));
    assert_eq!(distances, Vec::<f32>::new());
}
