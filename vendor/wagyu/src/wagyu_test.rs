// Smoke tests of the port; //difftest/tile:geometry_difftest compares it
// with the C++ wagyu.

use geometry::{MultiPolygon, Point};

use crate::{ClipType, FillType, PolygonType, Wagyu};

fn ring(points: &[(i32, i32)]) -> Vec<Point<i32>> {
    points.iter().map(|&(x, y)| Point::new(x, y)).collect()
}

#[test]
fn empty_clipper_returns_false() {
    let mut w = Wagyu::new();
    let mut out: MultiPolygon<i32> = Vec::new();
    assert_eq!(w.execute(ClipType::Union, &mut out, FillType::EvenOdd, FillType::EvenOdd), Ok(false));
    assert_eq!(out, [] as [std::vec::Vec<std::vec::Vec<geometry::Point<i32>>>; 0]);
}

#[test]
fn degenerate_rings_are_rejected() {
    let mut w = Wagyu::new();
    assert_eq!(w.add_ring(&ring(&[(0, 0), (1, 1)]), PolygonType::Subject), Ok(false));
    assert_eq!(w.add_ring(&ring(&[(0, 0), (1, 1), (2, 2)]), PolygonType::Subject), Ok(false));
    assert_eq!(w.add_ring(&ring(&[(0, 0), (0, 0), (0, 0)]), PolygonType::Subject), Ok(false));
}

#[test]
fn union_of_a_square() {
    let mut w = Wagyu::new();
    assert_eq!(w.add_ring(&ring(&[(0, 0), (0, 10), (10, 10), (10, 0), (0, 0)]), PolygonType::Subject), Ok(true));
    let mut out: MultiPolygon<i32> = Vec::new();
    assert_eq!(w.execute(ClipType::Union, &mut out, FillType::EvenOdd, FillType::EvenOdd), Ok(true));
    assert_eq!(out.len(), 1);
    assert_eq!(out[0].len(), 1);
    assert_eq!(out[0][0].len(), 5);
    assert_eq!(out[0][0].first(), out[0][0].last());
}

#[test]
fn self_intersecting_bowtie_splits() {
    let mut w = Wagyu::new();
    assert_eq!(w.add_ring(&ring(&[(0, 0), (10, 10), (10, 0), (0, 10), (0, 0)]), PolygonType::Subject), Ok(true));
    let mut out: MultiPolygon<i32> = Vec::new();
    assert_eq!(w.execute(ClipType::Union, &mut out, FillType::EvenOdd, FillType::EvenOdd), Ok(true));
    assert_eq!(out.len(), 2);
}
