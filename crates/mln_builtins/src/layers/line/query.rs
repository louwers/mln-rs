//! Where a line draws on the map, which a query meets: the stroke the
//! line's mesh draws around it (`mesh.rs`), its segments' quads with the
//! joins and caps of its layout, in a tile's units.

use glam::Vec2;
use mln_engine::query::shapes::{meets_circle, meets_polygon};
use mln_style::types::{LineCapType, LineJoinType};

/// How a line's mesh joins its segments and ends it.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Stroke {
    pub(crate) join: LineJoinType,
    pub(crate) cap: LineCapType,
    pub(crate) miter_limit: f32,
    pub(crate) round_limit: f32,
}

/// A piece of a line's stroke.
enum Piece {
    Polygon(Vec<Vec2>),
    Disc(Vec2, f32),
}

impl Piece {
    fn meets(&self, query: &[Vec2]) -> bool {
        match self {
            Piece::Polygon(polygon) => meets_polygon(query, std::slice::from_ref(polygon)),
            Piece::Disc(center, radius) => meets_circle(query, *center, *radius),
        }
    }

    fn contains(&self, p: Vec2) -> bool {
        self.meets(std::slice::from_ref(&p))
    }
}

/// The unit normal of the direction `d`, to its left in a tile's units
/// (whose y goes down).
fn normal(d: Vec2) -> Vec2 {
    Vec2::new(-d.y, d.x).normalize_or_zero()
}

/// Whether `query` meets the stroke of `line` that reaches `width` from it
/// at each of its points, as `stroke` joins and ends it: a closed line, one
/// whose ends meet, joins its ends instead.
fn meets(query: &[Vec2], line: &[Vec2], width: &[f32], stroke: Stroke) -> bool {
    pieces(line, width, stroke).iter().any(|piece| piece.meets(query))
}

/// Whether `p` is inside the stroke of `line` ([`meets`]).
fn contains(p: Vec2, line: &[Vec2], width: &[f32], stroke: Stroke) -> bool {
    pieces(line, width, stroke).iter().any(|piece| piece.contains(p))
}

/// The pieces of the stroke of `line`, whose union it is ([`meets`]).
fn pieces(line: &[Vec2], width: &[f32], stroke: Stroke) -> Vec<Piece> {
    let mut points: Vec<(Vec2, f32)> = line.iter().copied().zip(width.iter().copied()).collect();
    points.dedup_by(|a, b| a.0 == b.0);
    let closed = points.len() > 2 && points.first().map(|p| p.0) == points.last().map(|p| p.0);
    if closed {
        points.pop();
    }
    let mut pieces = Vec::new();
    match points.as_slice() {
        [] => return pieces,
        &[(p, w)] => {
            if stroke.cap == LineCapType::Round {
                pieces.push(Piece::Disc(p, w));
            }
            return pieces;
        }
        _ => {}
    }
    let n = points.len();
    let segments = if closed { n } else { n - 1 };
    for i in 0..segments {
        let ((a, wa), (b, wb)) = (points[i], points[(i + 1) % n]);
        let side = normal(b - a);
        pieces.push(Piece::Polygon(vec![a + side * wa, b + side * wb, b - side * wb, a - side * wa]));
    }
    // The joins, at each point between two segments.
    let joins = if closed { 0..n } else { 1..n - 1 };
    for i in joins {
        let (prev, (p, w), next) = (points[(i + n - 1) % n].0, points[i], points[(i + 1) % n].0);
        let (before, after) = (normal(p - prev), normal(next - p));
        let bisector = (before + after).normalize_or_zero();
        let cos_half_angle = bisector.dot(after);
        let miter_length = if cos_half_angle == 0.0 { f32::INFINITY } else { 1.0 / cos_half_angle };
        let miter_limit = match stroke.join {
            LineJoinType::Round if miter_length >= stroke.round_limit => {
                pieces.push(Piece::Disc(p, w));
                continue;
            }
            LineJoinType::Bevel => 1.05,
            _ => stroke.miter_limit,
        };
        for side in [1.0, -1.0] {
            let (before, after) = (before * side * w, after * side * w);
            if miter_length <= miter_limit {
                pieces.push(Piece::Polygon(vec![p, p + before, p + bisector * side * w * miter_length, p + after]));
            } else {
                pieces.push(Piece::Polygon(vec![p, p + before, p + after]));
            }
        }
    }
    // The caps, at the ends of a line that isn't closed.
    if !closed {
        for (end, inward) in [(0, 1), (n - 1, n - 2)] {
            let ((p, w), toward) = (points[end], points[inward].0);
            match stroke.cap {
                LineCapType::Butt => {}
                LineCapType::Round => pieces.push(Piece::Disc(p, w)),
                LineCapType::Square => {
                    let out = (p - toward).normalize_or_zero() * w;
                    let side = normal(out) * w;
                    pieces.push(Piece::Polygon(vec![p + side, p + side + out, p - side + out, p - side]));
                }
            }
        }
    }
    pieces
}

/// Whether `query` meets the line `line` drawn `outer` from it at each of
/// its points, and, for a line drawn with a gap, `inner` from it: the gap
/// shows nothing, so a query entirely in it doesn't meet the line.
pub(crate) fn meets_line(query: &[Vec2], line: &[Vec2], outer: &[f32], inner: Option<&[f32]>, stroke: Stroke) -> bool {
    if !meets(query, line, outer, stroke) {
        return false;
    }
    let Some(inner) = inner else { return true };
    !query.iter().all(|&p| contains(p, line, inner, stroke))
}

#[cfg(test)]
mod tests {
    use super::*;

    const BUTT_MITER: Stroke =
        Stroke { join: LineJoinType::Miter, cap: LineCapType::Butt, miter_limit: 2.0, round_limit: 1.05 };

    fn hits(p: Vec2, line: &[Vec2], width: f32, stroke: Stroke) -> bool {
        meets_line(&[p], line, &vec![width; line.len()], None, stroke)
    }

    #[test]
    fn a_butt_cap_ends_the_line_where_a_round_one_reaches_past_it() {
        let line = [Vec2::new(0.0, 0.0), Vec2::new(10.0, 0.0)];
        assert!(hits(Vec2::new(5.0, 1.5), &line, 2.0, BUTT_MITER));
        assert!(!hits(Vec2::new(5.0, 2.5), &line, 2.0, BUTT_MITER));
        assert!(!hits(Vec2::new(11.0, 0.0), &line, 2.0, BUTT_MITER));
        let round = Stroke { cap: LineCapType::Round, ..BUTT_MITER };
        assert!(hits(Vec2::new(11.0, 0.0), &line, 2.0, round));
        let square = Stroke { cap: LineCapType::Square, ..BUTT_MITER };
        assert!(hits(Vec2::new(11.5, 1.5), &line, 2.0, square));
    }

    #[test]
    fn a_miter_join_fills_the_corner_within_its_limit() {
        let line = [Vec2::new(0.0, 0.0), Vec2::new(10.0, 0.0), Vec2::new(10.0, 10.0)];
        // The outer corner of the right angle, √2 widths from the point.
        let corner = Vec2::new(11.8, -1.8);
        assert!(hits(corner, &line, 2.0, BUTT_MITER));
        let bevel = Stroke { join: LineJoinType::Bevel, ..BUTT_MITER };
        assert!(!hits(corner, &line, 2.0, bevel));
    }

    #[test]
    fn a_line_with_a_gap_shows_nothing_in_it() {
        let line = [Vec2::new(0.0, 0.0), Vec2::new(10.0, 0.0)];
        let (outer, inner) = ([4.0; 2], [2.0; 2]);
        assert!(meets_line(&[Vec2::new(5.0, 3.0)], &line, &outer, Some(&inner), BUTT_MITER));
        assert!(!meets_line(&[Vec2::new(5.0, 1.0)], &line, &outer, Some(&inner), BUTT_MITER));
    }
}
