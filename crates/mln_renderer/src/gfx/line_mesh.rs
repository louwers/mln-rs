//! Line meshes: a strip of triangles along a line, whose vertices hold the
//! direction to extrude them in, so that the line shaders draw one mesh at
//! any line width, gap width and offset.

use geometry::Point;
use glam::DVec2;
use mln_style::constants::{EXTENT, TILE_SIZE_D};
use mln_style::types::{LineCapType, LineJoinType};

use crate::gfx::mesh::Indexed;

/// Whether a vertex is rounded or sharp.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum VertexRound {
    Sharp,
    Round,
}

impl From<VertexRound> for bool {
    fn from(r: VertexRound) -> bool {
        matches!(r, VertexRound::Round)
    }
}

/// Which side of the line a vertex is on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum VertexSide {
    Left,
    Right,
}

impl From<VertexSide> for bool {
    fn from(s: VertexSide) -> bool {
        matches!(s, VertexSide::Right)
    }
}

/// A vertex of a line mesh: its position, doubled, with the round and up
/// flags in the low bits, then its extrusion and distance along the line.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, bytemuck::Pod, bytemuck::Zeroable)]
#[repr(C)]
pub(crate) struct LineLayoutVertex {
    pub(crate) a1: [i16; 2],
    pub(crate) a2: [u8; 4],
}

impl LineLayoutVertex {
    /// The vertex at `at` that the shader moves by `extrude` times half the
    /// line's width. `round` marks a round cap or join, `side` distinguishes
    /// the two vertices at this point, and the sign of `end` indicates which
    /// way the vertex is moved along the line. `distance` is in the buffer's
    /// units, [`LINE_DISTANCE_SCALE`] of the distance along the line.
    fn new(at: Point<i16>, extrude: DVec2, round: VertexRound, side: VertexSide, end: f64, distance: u32) -> Self {
        // The direction, -1, 0 or 1, plus 1.
        let dir = u8::from(end >= 0.0) + u8::from(end > 0.0);
        // A byte with 128 for 0, in units of 1/63.
        let byte = |v: f64| ((v * 63.0).round() + 128.0).clamp(0.0, 255.0) as u8;
        LineLayoutVertex {
            a1: [
                at.x.wrapping_mul(2) | i16::from(bool::from(round)),
                at.y.wrapping_mul(2) | i16::from(bool::from(side)),
            ],
            // The direction in the low two bits, then the distance's low 6
            // bits, and its high 8 bits.
            a2: [byte(extrude.x), byte(extrude.y), dir | ((distance & 0x3F) << 2) as u8, (distance >> 6) as u8],
        }
    }
}

/// The distance along a line is stored in this fraction of tile units,
/// trading precision for range.
const LINE_DISTANCE_SCALE: f64 = 0.5;

/// The longest distance along a line, in tile units, that fits the buffer's
/// 14 bits.
const MAX_LINE_DISTANCE: f64 = (1 << 14) as f64 / LINE_DISTANCE_SCALE;

/// A corner whose half angle has a cosine under this, cos(37.5°), tilts a
/// dash pattern visibly, since its inner and outer edges have the same
/// distance along the line. Points [`SHARP_CORNER_OFFSET`] on either side
/// of it confine the tilt to that stretch.
const COS_HALF_SHARP_CORNER: f64 = 0.793_353_340_291_235_2;

/// How far from a sharp corner, in pixels, the points that confine its tilt
/// are.
const SHARP_CORNER_OFFSET: f64 = 15.0;

/// The angle each pie slice of a fake round join spans.
const DEGREES_PER_SLICE: f64 = 20.0;

/// How a line is meshed: the shape of its joins and caps, from its layout
/// properties.
#[derive(Clone, Copy, Debug)]
pub(crate) struct LineOptions {
    /// Whether the line is a polygon's ring, which ends where it starts and
    /// has a join there instead of caps.
    pub(crate) closed: bool,
    pub(crate) join: LineJoinType,
    pub(crate) cap: LineCapType,
    pub(crate) miter_limit: f32,
    pub(crate) round_limit: f32,
    /// How many times the tile is magnified past its data's zoom.
    pub(crate) overscaling: u32,
    /// The part of the whole line that this one is, from and to a fraction
    /// of its length. Distances along it are then fractions of the whole
    /// line, which `line-gradient` reads, rather than tile units.
    pub(crate) clip: Option<(f64, f64)>,
}

impl Default for LineOptions {
    fn default() -> Self {
        LineOptions {
            closed: false,
            join: LineJoinType::Miter,
            cap: LineCapType::Butt,
            miter_limit: 2.0,
            round_limit: 1.05,
            overscaling: 1,
            clip: None,
        }
    }
}

/// The buffers that meshing a line reuses.
#[derive(Default)]
pub(crate) struct LineScratch {
    points: Vec<Point<i16>>,
    triangles: Vec<[u32; 3]>,
}

/// Adds the mesh of `line` to `vertices`, and its triangles to `triangles`.
/// A line of fewer than two distinct points, or a ring of fewer than three,
/// adds nothing.
pub(crate) fn add_line(
    vertices: &mut Vec<LineLayoutVertex>,
    triangles: &mut Indexed<'_>,
    scratch: &mut LineScratch,
    line: &[Point<i16>],
    options: &LineOptions,
) {
    let points = &mut scratch.points;
    points.clear();
    points.extend_from_slice(line);
    points.dedup();
    if options.closed && points.first() != points.last() {
        points.push(points[0]);
    }
    if points.len() < if options.closed { 4 } else { 2 } {
        return;
    }

    let clip = options.clip.map(|(start, end)| Clip {
        start,
        end,
        length: points.windows(2).map(|segment| distance(segment[0], segment[1])).sum(),
    });
    let first = vertices.len();
    scratch.triangles.clear();
    Strip { vertices, first, triangles: &mut scratch.triangles, last: [None; 2], distance: 0.0, clip }
        .add_line(points, options);
    triangles.add(vertices, first, scratch.triangles.iter().copied());
}

/// The part of the whole line a line is; see [`LineOptions::clip`].
#[derive(Clone, Copy)]
struct Clip {
    start: f64,
    end: f64,
    /// The line's length in tile units.
    length: f64,
}

/// How a line turns at a vertex.
struct Turn {
    /// The normals of the segments before and after the vertex.
    prev_normal: DVec2,
    next_normal: DVec2,
    /// The normal that bisects them: zero where the line turns back on
    /// itself.
    join_normal: DVec2,
    cos_half_angle: f64,
    /// How much longer than the line is wide a miter join is.
    miter_length: f64,
}

impl Turn {
    fn new(prev_normal: DVec2, next_normal: DVec2) -> Turn {
        let join_normal = (prev_normal + next_normal).normalize_or_zero();
        let cos_half_angle = join_normal.dot(next_normal);
        let miter_length = if cos_half_angle == 0.0 { f64::INFINITY } else { 1.0 / cos_half_angle };
        Turn { prev_normal, next_normal, join_normal, cos_half_angle, miter_length }
    }

    fn turns_left(&self) -> bool {
        self.prev_normal.perp_dot(self.next_normal) > 0.0
    }
}

/// What a vertex of a line draws: a join of the segments on either side of
/// it, or a cap where the line ends.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Corner {
    Miter,
    /// A triangle that fills the gap on the outside of the turn.
    Bevel,
    /// A bevel too long for the extrusion's range, drawn across the corner
    /// instead.
    FlipBevel,
    /// A bevel of several triangles, which looks round at the widths lines
    /// are drawn at.
    FakeRound,
    /// A join or cap whose ends the fragment shader rounds.
    Round,
    Butt,
    Square,
}

impl Corner {
    /// The corner of a vertex where the line turns by `turn`: a join if
    /// `join`, and a cap otherwise.
    fn new(options: &LineOptions, join: bool, turn: &Turn) -> Corner {
        if !join {
            return match options.cap {
                LineCapType::Butt => Corner::Butt,
                LineCapType::Square => Corner::Square,
                LineCapType::Round => Corner::Round,
            };
        }
        let miter_length = turn.miter_length;
        let miter_limit = match options.join {
            LineJoinType::Round if miter_length >= f64::from(options.round_limit) => {
                return if miter_length <= 2.0 { Corner::FakeRound } else { Corner::Round };
            }
            LineJoinType::Bevel => 1.05,
            _ => f64::from(options.miter_limit),
        };
        if miter_length <= miter_limit {
            Corner::Miter
        } else if miter_length <= 2.0 {
            Corner::Bevel
        } else {
            // The extrusion reaches 128 / 63, about 2, times the width.
            Corner::FlipBevel
        }
    }
}

/// A triangle strip being added to a line's vertices.
struct Strip<'a> {
    vertices: &'a mut Vec<LineLayoutVertex>,
    /// The line's first vertex, which the triangles count from.
    first: usize,
    triangles: &'a mut Vec<[u32; 3]>,
    /// The two vertices the next one makes a triangle with.
    last: [Option<u32>; 2],
    /// How far along the line the vertices being added are, in tile units.
    distance: f64,
    clip: Option<Clip>,
}

impl Strip<'_> {
    /// Adds the vertices of `points`, which have no repeated point and
    /// repeat the first at the end of a ring.
    fn add_line(&mut self, points: &[Point<i16>], options: &LineOptions) {
        let n = points.len();
        let closed = options.closed;
        let sharp_corner_offset = match options.overscaling.max(1) {
            overscaling @ ..=16 => SHARP_CORNER_OFFSET * f64::from(EXTENT) / (TILE_SIZE_D * f64::from(overscaling)),
            _ => 0.0,
        };

        // A ring's first point follows the one before its last, which is
        // the first again. An open line goes straight on past its ends.
        let mut prev = closed.then(|| points[n - 2]);
        let mut prev_normal = if closed { normal(points[n - 2], points[0]) } else { normal(points[0], points[1]) };
        for (i, &at) in points.iter().enumerate() {
            let next = points.get(i + 1).copied().or_else(|| closed.then(|| points[1]));
            let next_normal = next.map_or(prev_normal, |next| normal(at, next));
            let turn = Turn::new(prev_normal, next_normal);
            let join = prev.is_some() && next.is_some();
            let corner = Corner::new(options, join, &turn);
            let sharp = join && turn.cos_half_angle < COS_HALF_SHARP_CORNER;

            if let Some(mut from) = prev {
                // A ring's first corner leaves this to its last.
                if sharp && i > 0 && distance(from, at) > 2.0 * sharp_corner_offset {
                    let before = toward(at, from, sharp_corner_offset);
                    self.distance += distance(from, before);
                    self.add_pair(before, prev_normal, 0.0, 0.0, VertexRound::Sharp);
                    from = before;
                }
                self.distance += distance(from, at);
            }

            self.add_corner(corner, at, i > 0, next.is_some(), &turn);
            prev = Some(at);

            if let Some(next) = next
                && sharp
                // A ring's last corner leaves this to its first.
                && i + 1 < n
                && distance(at, next) > 2.0 * sharp_corner_offset
            {
                let after = toward(at, next, sharp_corner_offset);
                self.distance += distance(at, after);
                self.add_pair(after, next_normal, 0.0, 0.0, VertexRound::Sharp);
                prev = Some(after);
            }
            prev_normal = next_normal;
        }
    }

    /// Adds the vertices of `corner` at `at`: those that end the segment
    /// before it, if `ends`, and those that start the one after it, if
    /// `starts`.
    fn add_corner(&mut self, corner: Corner, at: Point<i16>, ends: bool, starts: bool, turn: &Turn) {
        let Turn { prev_normal, next_normal, join_normal, miter_length, .. } = *turn;
        match corner {
            Corner::Miter => self.add_pair(at, join_normal * miter_length, 0.0, 0.0, VertexRound::Sharp),
            Corner::FlipBevel => {
                let normal = if miter_length > 100.0 {
                    // The segments are almost parallel.
                    -next_normal
                } else {
                    let direction = if turn.turns_left() { -1.0 } else { 1.0 };
                    let bevel_length =
                        miter_length * (prev_normal + next_normal).length() / (prev_normal - next_normal).length();
                    join_normal.perp() * bevel_length * direction
                };
                self.add_pair(at, normal, 0.0, 0.0, VertexRound::Sharp);
                self.add_pair(at, -normal, 0.0, 0.0, VertexRound::Sharp);
            }
            Corner::Bevel | Corner::FakeRound => {
                let turns_left = turn.turns_left();
                let offset = -(miter_length * miter_length - 1.0).sqrt();
                let (left, right) = if turns_left { (offset, 0.0) } else { (0.0, offset) };
                if ends {
                    self.add_pair(at, prev_normal, left, right, VertexRound::Sharp);
                }
                if corner == Corner::FakeRound {
                    self.add_pie_slices(at, turn);
                }
                self.add_pair(at, next_normal, -left, -right, VertexRound::Sharp);
            }
            Corner::Butt | Corner::Square | Corner::Round => {
                let round = if corner == Corner::Round { VertexRound::Round } else { VertexRound::Sharp };
                let end = if corner == Corner::Butt { 0.0 } else { 1.0 };
                if ends {
                    if round == VertexRound::Round {
                        self.add_pair(at, prev_normal, 0.0, 0.0, VertexRound::Sharp);
                    }
                    self.add_pair(at, prev_normal, end, end, round);
                    if end != 0.0 {
                        // The cap ends this strip.
                        self.last = [None; 2];
                    }
                }
                if starts {
                    self.add_pair(at, next_normal, -end, -end, round);
                    if round == VertexRound::Round {
                        self.add_pair(at, next_normal, 0.0, 0.0, VertexRound::Sharp);
                    }
                }
            }
        }
    }

    /// Fills the outside of a turn with pie slices of about
    /// [`DEGREES_PER_SLICE`] each.
    fn add_pie_slices(&mut self, at: Point<i16>, turn: &Turn) {
        let approx_angle = 2.0 * (2.0 - 2.0 * turn.cos_half_angle).sqrt();
        let slices = (approx_angle.to_degrees() / DEGREES_PER_SLICE).round() as u32;
        let cos_angle = turn.prev_normal.dot(turn.next_normal);
        let turns_left = turn.turns_left();
        let (side, sign) = if turns_left { (VertexSide::Right, -1.0) } else { (VertexSide::Left, 1.0) };
        for m in 1..slices {
            let mut t = f64::from(m) / f64::from(slices);
            if t != 0.5 {
                // An approximation of spherical interpolation, from
                // https://observablehq.com/@mourner/approximating-geometric-slerp
                let t2 = t - 0.5;
                let a = 1.0904 + cos_angle * (-3.2452 + cos_angle * (3.55645 - cos_angle * 1.43519));
                let b = 0.848_013 + cos_angle * (-1.06021 + cos_angle * 0.215_638);
                t += t * t2 * (t - 1.0) * (a * t2 * t2 + b);
            }
            let normal = turn.prev_normal.lerp(turn.next_normal, t).normalize();
            let slice = self.push(at, normal * sign, VertexRound::Sharp, side, 0.0);
            // Each slice fans out from the vertex on the inside of the turn.
            self.last[usize::from(turns_left)] = Some(slice);
        }
    }

    /// Adds a vertex on each side of the line at `at`, extruded along
    /// `normal`, and moved back along the line by `end_left` and `end_right`
    /// times the extrusion: forward for a negative one.
    fn add_pair(&mut self, at: Point<i16>, normal: DVec2, end_left: f64, end_right: f64, round: VertexRound) {
        let along = normal.perp();
        for (extrude, side, end) in [
            (normal - along * end_left, VertexSide::Left, end_left),
            (-normal - along * end_right, VertexSide::Right, -end_right),
        ] {
            let vertex = self.push(at, extrude, round, side, end);
            self.last = [self.last[1], Some(vertex)];
        }

        // Past the most distance the buffer holds, the distance starts over
        // at zero, at a pair of vertices of its own.
        if self.clip.is_none() && self.distance > MAX_LINE_DISTANCE / 2.0 {
            self.distance = 0.0;
            self.add_pair(at, normal, end_left, end_right, round);
        }
    }

    /// Pushes a vertex, makes a triangle of it and the strip's last two,
    /// and returns its index in the line.
    fn push(&mut self, at: Point<i16>, extrude: DVec2, round: VertexRound, side: VertexSide, end: f64) -> u32 {
        let distance = match self.clip {
            // The fraction of the whole line, in the buffer's range.
            Some(clip) => {
                let fraction = if clip.length > 0.0 { self.distance / clip.length } else { 0.0 };
                (fraction * (clip.end - clip.start) + clip.start) * (MAX_LINE_DISTANCE - 1.0)
            }
            None => self.distance,
        };
        self.vertices.push(LineLayoutVertex::new(
            at,
            extrude,
            round,
            side,
            end,
            (distance * LINE_DISTANCE_SCALE) as u32,
        ));
        let vertex = (self.vertices.len() - 1 - self.first) as u32;
        if let [Some(a), Some(b)] = self.last {
            self.triangles.push([a, b, vertex]);
        }
        vertex
    }
}

fn vec(p: Point<i16>) -> DVec2 {
    DVec2::new(f64::from(p.x), f64::from(p.y))
}

fn distance(a: Point<i16>, b: Point<i16>) -> f64 {
    vec(a).distance(vec(b))
}

/// The unit normal of the segment from `a` to `b`.
fn normal(a: Point<i16>, b: Point<i16>) -> DVec2 {
    (vec(b) - vec(a)).normalize().perp()
}

/// The point `distance` from `from` toward `to`, rounded to tile
/// coordinates.
fn toward(from: Point<i16>, to: Point<i16>, distance: f64) -> Point<i16> {
    let step = ((vec(to) - vec(from)) * (distance / vec(from).distance(vec(to)))).round();
    Point::new(from.x + step.x as i16, from.y + step.y as i16)
}
