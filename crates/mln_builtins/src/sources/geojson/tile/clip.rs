//! Cuts features to the band between two lines parallel to an axis, and
//! wraps them around the antimeridian or cuts them at the world's edges.

use std::borrow::Cow;
use std::sync::Arc;

use smallvec::smallvec;

use super::geometry::{Bbox, Feature, Geometry, Line, Parts, Ring, Vertex, X};

/// The parts of `features` between `k1` and `k2` on `AXIS`. `min` and `max`
/// bound all of them on that axis: if they lie inside, they come back
/// borrowed. With `line_metrics`, each piece of a cut line becomes a feature
/// of its own, so that each has its own metrics.
pub(super) fn clip<const AXIS: usize>(
    features: &[Feature],
    k1: f64,
    k2: f64,
    min: f64,
    max: f64,
    line_metrics: bool,
) -> Cow<'_, [Feature]> {
    if min >= k1 && max < k2 {
        return Cow::Borrowed(features);
    }
    if max < k1 || min >= k2 {
        return Cow::Owned(Vec::new());
    }

    let mut clipped = Vec::with_capacity(features.len());
    for feature in features {
        let (min, max) = feature.bbox.range::<AXIS>();
        if min >= k1 && max < k2 {
            clipped.push(feature.clone());
            continue;
        }
        if max < k1 || min >= k2 {
            continue;
        }

        let geometry = match &feature.geometry {
            Geometry::Points(points) => {
                Geometry::Points(points.iter().copied().filter(|p| (k1..=k2).contains(&p.get::<AXIS>())).collect())
            }
            Geometry::Lines(lines) => {
                let mut pieces = Parts::new();
                for line in lines {
                    clip_line::<AXIS>(line, k1, k2, line_metrics, &mut pieces);
                }
                if line_metrics {
                    for piece in pieces {
                        clipped.extend(Feature::new(
                            Geometry::Lines(smallvec![piece]),
                            Arc::clone(&feature.properties),
                            feature.id.clone(),
                        ));
                    }
                    continue;
                }
                Geometry::Lines(pieces)
            }
            Geometry::Polygons(polygons) => Geometry::Polygons(
                polygons
                    .iter()
                    .map(|rings| {
                        rings
                            .iter()
                            .map(|ring| clip_ring::<AXIS>(ring, k1, k2))
                            .filter(|r| !r.points.is_empty())
                            .collect()
                    })
                    .filter(|rings: &Vec<Ring>| !rings.is_empty())
                    .collect(),
            ),
        };
        clipped.extend(Feature::new(geometry, Arc::clone(&feature.properties), feature.id.clone()));
    }
    Cow::Owned(clipped)
}

/// Appends the pieces of `line` between `k1` and `k2` to `pieces`.
pub(super) fn clip_line<const AXIS: usize>(
    line: &Line,
    k1: f64,
    k2: f64,
    line_metrics: bool,
    pieces: &mut Parts<Line>,
) {
    let [_, .., last] = line.points[..] else { return };
    let new_piece = || Line { points: Vec::new(), length: line.length, start: line.start, end: line.end };
    let mut piece = new_piece();
    // How far along the whole line `a` is.
    let mut distance = line.start;

    for pair in line.points.windows(2) {
        let (a, b) = (pair[0], pair[1]);
        let (ak, bk) = (a.get::<AXIS>(), b.get::<AXIS>());
        let segment = if line_metrics { (b.x - a.x).hypot(b.y - a.y) } else { 0.0 };

        if ak < k1 || ak > k2 {
            // Outside: the segment may enter at the edge it comes from. When
            // it ends on that edge, `b` starts the piece.
            let k = if ak < k1 { k1 } else { k2 };
            if (ak < k1 && bk >= k1) || (ak > k2 && bk <= k2) {
                let t = progress::<AXIS>(a, b, k);
                if bk != k {
                    piece.points.push(intersect::<AXIS>(a, b, k, t));
                }
                piece.start = distance + segment * t;
            }
        } else {
            piece.points.push(a);
        }

        // The segment may leave at either edge, entered or not.
        let exit = if bk < k1 && ak >= k1 {
            Some(k1)
        } else if bk > k2 && ak <= k2 {
            Some(k2)
        } else {
            None
        };
        if let Some(k) = exit {
            let t = progress::<AXIS>(a, b, k);
            piece.points.push(intersect::<AXIS>(a, b, k, t));
            piece.end = distance + segment * t;
            pieces.push(std::mem::replace(&mut piece, new_piece()));
        }
        distance += segment;
    }

    if (k1..=k2).contains(&last.get::<AXIS>()) {
        piece.points.push(last);
    }
    // A lone point, where the line only touches the band, isn't a line.
    if piece.points.len() > 1 {
        if line_metrics {
            piece.end = distance;
        }
        pieces.push(piece);
    }
}

/// What lies of `ring` between `k1` and `k2`: the ring cut along them, and
/// closed.
pub(super) fn clip_ring<const AXIS: usize>(ring: &Ring, k1: f64, k2: f64) -> Ring {
    let mut points = Vec::new();
    if let [_, .., last] = ring.points[..] {
        for pair in ring.points.windows(2) {
            let (a, b) = (pair[0], pair[1]);
            let (ak, bk) = (a.get::<AXIS>(), b.get::<AXIS>());
            if ak < k1 {
                if bk > k1 {
                    points.push(intersect::<AXIS>(a, b, k1, progress::<AXIS>(a, b, k1)));
                }
            } else if ak > k2 {
                if bk < k2 {
                    points.push(intersect::<AXIS>(a, b, k2, progress::<AXIS>(a, b, k2)));
                }
            } else {
                points.push(a);
            }
            if bk < k1 && ak >= k1 {
                points.push(intersect::<AXIS>(a, b, k1, progress::<AXIS>(a, b, k1)));
            } else if bk > k2 && ak <= k2 {
                points.push(intersect::<AXIS>(a, b, k2, progress::<AXIS>(a, b, k2)));
            }
        }
        if (k1..=k2).contains(&last.get::<AXIS>()) {
            points.push(last);
        }
    }
    if let (Some(&first), Some(&last)) = (points.first(), points.last())
        && !first.same_position(last)
    {
        points.push(first);
    }
    Ring { points, area: ring.area }
}

/// How far along the segment from `a` to `b` it crosses `k` on `AXIS`.
fn progress<const AXIS: usize>(a: Vertex, b: Vertex, k: f64) -> f64 {
    (k - a.get::<AXIS>()) / (b.get::<AXIS>() - a.get::<AXIS>())
}

/// The point `t` along the segment from `a` to `b`, which is `k` on `AXIS`.
fn intersect<const AXIS: usize>(a: Vertex, b: Vertex, k: f64, t: f64) -> Vertex {
    if AXIS == X { Vertex::new(k, (b.y - a.y) * t + a.y, 1.0) } else { Vertex::new((b.x - a.x) * t + a.x, k, 1.0) }
}

/// `features` with a copy of what lies within `buffer` (in the unit square)
/// past either side of the world on the other side, so that tiles at the
/// antimeridian get what crosses it.
pub(super) fn wrap(features: Vec<Feature>, buffer: f64, line_metrics: bool) -> Vec<Feature> {
    let left = clip::<X>(&features, -1.0 - buffer, buffer, -1.0, 2.0, line_metrics);
    let right = clip::<X>(&features, 1.0 - buffer, 2.0 + buffer, -1.0, 2.0, line_metrics);
    if left.is_empty() && right.is_empty() {
        return features;
    }
    let mut left = left.into_owned();
    let mut right = right.into_owned();
    let center = clip::<X>(&features, -buffer, 1.0 + buffer, -1.0, 2.0, line_metrics);

    for f in &mut left {
        f.shift_x(1.0);
    }
    for f in &mut right {
        f.shift_x(-1.0);
    }
    let mut merged = left;
    merged.extend(center.into_owned());
    merged.append(&mut right);
    merged
}

/// `features` without what lies past `buffer` (in the unit square) beyond
/// the world's edges on `AXIS`.
pub(super) fn clip_to_world<const AXIS: usize>(
    features: Vec<Feature>,
    buffer: f64,
    line_metrics: bool,
) -> Vec<Feature> {
    let mut bbox = Bbox::EMPTY;
    for feature in &features {
        bbox.extend(feature.bbox);
    }
    let (min, max) = bbox.range::<AXIS>();
    if let Cow::Owned(clipped) = clip::<AXIS>(&features, -buffer, 1.0 + buffer, min, max, line_metrics) {
        clipped
    } else {
        features
    }
}
