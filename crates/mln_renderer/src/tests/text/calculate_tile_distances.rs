use geometry::Point;
use mln_tile_id::GeometryCoordinates;

use crate::geometry::anchor::Anchor;
use crate::layout::symbol_layout::SymbolLayout;

// gtest: calculateTileDistances.Point
#[test]
fn point() {
    let line: GeometryCoordinates = vec![Point::new(1i16, 1)];
    let mut distances = vec![f32::NAN; line.len()];
    SymbolLayout::calculate_tile_distances(&line, &Anchor::new(1.0, 1.0, 0.0, Some(0)), &mut distances);
    assert_eq!(distances, vec![0.0f32]);
}

// gtest: calculateTileDistances.Line
#[test]
fn line() {
    let line: GeometryCoordinates = vec![Point::new(1i16, 1), Point::new(1, 2), Point::new(1, 3), Point::new(1, 4)];
    let mut distances = vec![f32::NAN; line.len()];
    SymbolLayout::calculate_tile_distances(&line, &Anchor::new(1.0, 3.0, 0.0, Some(2)), &mut distances);
    assert_eq!(distances, vec![2.0f32, 1.0, 0.0, 1.0]);
}

// gtest: calculateTileDistances.EmptySegment
#[test]
fn empty_segment() {
    let line: GeometryCoordinates = Vec::new();
    let mut distances = Vec::new();
    SymbolLayout::calculate_tile_distances(&line, &Anchor::new(1.0, 1.0, 0.0, None), &mut distances);
    assert_eq!(distances, Vec::<f32>::new());
}
