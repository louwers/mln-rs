//! The quads that point features draw as.

use geometry::Point;
use mln_render::layer::Indexes;
use mln_util::constants::EXTENT;

/// Adds a quad to `corners` for each of `points`, and its triangles to
/// `triangles`: each corner the point doubled, with the corner's right and
/// top sides in the low bits.
pub(crate) fn add_quads(
    corners: &mut Vec<[i16; 2]>,
    triangles: &mut Indexes,
    points: impl IntoIterator<Item = Point<i16>>,
) {
    for p in points {
        let (x, y) = (p.x.wrapping_mul(2), p.y.wrapping_mul(2));
        let first = corners.len();
        corners.extend([[x, y], [x | 1, y], [x | 1, y | 1], [x, y | 1]]);
        triangles.add(corners, first, [[0, 1, 2], [0, 3, 2]]);
    }
}

/// Whether `point` is inside its tile, rather than in a neighbor's buffer.
pub(crate) fn in_tile(point: &Point<i16>) -> bool {
    (0..EXTENT).contains(&i32::from(point.x)) && (0..EXTENT).contains(&i32::from(point.y))
}
