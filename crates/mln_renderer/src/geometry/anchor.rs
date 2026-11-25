use glam::Vec2;
use mln_style::feature::GeometryCoordinate;

/// Where a symbol is placed: a point, and for a label along a line, the
/// line's angle there and the index of the segment the point is on.
#[derive(Clone, Copy, Debug, PartialEq, mln_wire::Wire)]
pub(crate) struct Anchor {
    pub(crate) point: Vec2,
    pub(crate) angle: f32,
    pub(crate) segment: Option<usize>,
}

impl Anchor {
    #[must_use]
    pub(crate) fn new(x: f32, y: f32, angle: f32, segment: Option<usize>) -> Self {
        Anchor { point: Vec2::new(x, y), angle, segment }
    }
}

/// A tile coordinate as a float vector.
#[must_use]
pub(crate) fn to_vec2(p: GeometryCoordinate) -> Vec2 {
    Vec2::new(f32::from(p.x), f32::from(p.y))
}

/// The segments of `line`, as their start and end points.
pub(crate) fn segments(line: &[GeometryCoordinate]) -> impl Iterator<Item = (Vec2, Vec2)> + '_ {
    line.windows(2).map(|w| (to_vec2(w[0]), to_vec2(w[1])))
}

/// The length of `line`.
#[must_use]
pub(crate) fn line_length(line: &[GeometryCoordinate]) -> f32 {
    segments(line).map(|(a, b)| a.distance(b)).sum()
}

/// Walks back along `line` from `point`, which is on the segment that starts
/// at vertex `segment`, to the first vertex at least `distance` back.
/// Returns that vertex's index and its distance from `point` along the line,
/// or the line's first vertex and its distance if none is that far back.
#[must_use]
pub(crate) fn vertex_behind(line: &[GeometryCoordinate], point: Vec2, segment: usize, distance: f32) -> (usize, f32) {
    let mut behind = 0.0;
    let mut next = point;
    for (i, &vertex) in line[..=segment].iter().enumerate().rev() {
        let vertex = to_vec2(vertex);
        behind += vertex.distance(next);
        next = vertex;
        if behind >= distance {
            return (i, behind);
        }
    }
    (0, behind)
}
