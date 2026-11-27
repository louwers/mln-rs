//! Where a line draws on the map, which a query meets: the stroke the
//! line's mesh draws around it (`mesh.rs`), its segments' quads with the
//! joins and caps of its layout, in a tile's units.

use glam::Vec2;
use mln_engine::query::shapes::{meets_circle, meets_polygon, ring_contains, segment_crossing};
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

    /// How far along the segment `a`-`b`, from 0 to 1, it crosses the
    /// piece's outline.
    fn crossings(&self, a: Vec2, b: Vec2) -> Vec<f32> {
        match self {
            Piece::Polygon(polygon) => polygon
                .iter()
                .zip(polygon.iter().cycle().skip(1))
                .filter_map(|(&p, &q)| segment_crossing(a, b, p, q))
                .collect(),
            Piece::Disc(center, radius) => {
                // |a + t (b - a) - center| = radius.
                let (d, f) = (b - a, a - *center);
                let (qa, qb, qc) = (d.dot(d), 2.0 * f.dot(d), f.dot(f) - radius * radius);
                let discriminant = qb * qb - 4.0 * qa * qc;
                if qa == 0.0 || discriminant < 0.0 {
                    return Vec::new();
                }
                let root = discriminant.sqrt();
                [(-qb - root) / (2.0 * qa), (-qb + root) / (2.0 * qa)]
                    .into_iter()
                    .filter(|t| (0.0..=1.0).contains(t))
                    .collect()
            }
        }
    }
}

/// The unit normal of the direction `d`, to its left in a tile's units
/// (whose y goes down).
fn normal(d: Vec2) -> Vec2 {
    Vec2::new(-d.y, d.x).normalize_or_zero()
}

/// The pieces of the stroke of `line` that reaches `width` from it at each
/// of its points, as `stroke` joins and ends it, whose union it is: a
/// closed line, one whose ends meet, joins its ends instead.
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
/// shows nothing, so the query meets such a line where it meets the stroke
/// outside the gap. A ring meets it where its outline does, or where it
/// holds the middle of the stroke on either side of a segment.
pub(crate) fn meets_line(query: &[Vec2], line: &[Vec2], outer: &[f32], inner: Option<&[f32]>, stroke: Stroke) -> bool {
    let outer_pieces = pieces(line, outer, stroke);
    if !outer_pieces.iter().any(|piece| piece.meets(query)) {
        return false;
    }
    let Some(inner) = inner else { return true };
    let inner_pieces = pieces(line, inner, stroke);
    let shows = |p: Vec2| {
        outer_pieces.iter().any(|piece| piece.contains(p)) && !inner_pieces.iter().any(|piece| piece.contains(p))
    };
    if query.len() < 3 {
        return query.iter().any(|&p| shows(p));
    }
    // Between two of the points where an edge of the query crosses the
    // pieces' outlines, it is all in the stroke or all out of it.
    let outline_shows = query.iter().zip(query.iter().cycle().skip(1)).any(|(&a, &b)| {
        let mut crossings: Vec<f32> =
            outer_pieces.iter().chain(&inner_pieces).flat_map(|piece| piece.crossings(a, b)).collect();
        crossings.extend([0.0, 1.0]);
        crossings.sort_by(f32::total_cmp);
        shows(a) || crossings.windows(2).any(|pair| shows(a.lerp(b, f32::midpoint(pair[0], pair[1]))))
    });
    if outline_shows {
        return true;
    }
    // The stroke where it lies wholly inside the query, as inside a closed
    // line's gap.
    line.windows(2).zip(outer.windows(2).zip(inner.windows(2))).any(|(segment, (outer, inner))| {
        let side = normal(segment[1] - segment[0]);
        [0.0, 0.5, 1.0].into_iter().any(|t: f32| {
            let at = segment[0].lerp(segment[1], t);
            let reach = f32::midpoint(outer[0] + (outer[1] - outer[0]) * t, inner[0] + (inner[1] - inner[0]) * t);
            [side, -side].into_iter().map(|side| at + side * reach).any(|p| ring_contains(query, p) && shows(p))
        })
    })
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

    fn rectangle(x0: f32, y0: f32, x1: f32, y1: f32) -> [Vec2; 4] {
        [Vec2::new(x0, y0), Vec2::new(x1, y0), Vec2::new(x1, y1), Vec2::new(x0, y1)]
    }

    #[test]
    fn a_ring_with_its_corners_in_a_gap_meets_the_stroke_its_edges_cross() {
        // A U whose upright segments' gaps hold the ring's corners, while
        // the ring's edges cross their strokes.
        let line = [Vec2::new(0.0, 10.0), Vec2::new(0.0, 0.0), Vec2::new(10.0, 0.0), Vec2::new(10.0, 10.0)];
        let (outer, inner) = ([2.0; 4], [1.0; 4]);
        let across = rectangle(0.0, 4.0, 10.0, 6.0);
        assert!(meets_line(&across, &line, &outer, Some(&inner), BUTT_MITER));
        // One wholly in an upright's gap.
        let within = rectangle(-0.5, 4.0, 0.5, 6.0);
        assert!(!meets_line(&within, &line, &outer, Some(&inner), BUTT_MITER));
        // One in the gap past a butt end, where nothing draws either.
        let past_end = rectangle(-0.5, 9.0, 0.5, 12.0);
        assert!(!meets_line(&past_end, &line, &outer, Some(&inner), BUTT_MITER));
    }

    #[test]
    fn a_ring_in_a_closed_lines_gap_meets_the_stroke_it_holds() {
        let line = [
            Vec2::new(0.0, 0.0),
            Vec2::new(10.0, 0.0),
            Vec2::new(10.0, 10.0),
            Vec2::new(0.0, 10.0),
            Vec2::new(0.0, 0.0),
        ];
        let (outer, inner) = ([2.0; 5], [1.0; 5]);
        // Its outline runs all the way round in the gap, around the stroke
        // inside the gap.
        let around = rectangle(0.5, 0.5, 9.5, 9.5);
        assert!(meets_line(&around, &line, &outer, Some(&inner), BUTT_MITER));
    }
}
