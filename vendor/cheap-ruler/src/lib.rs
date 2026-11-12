// Ports vendor/maplibre-native-base/deps/cheap-ruler-cpp/include/mapbox/cheap_ruler.hpp
// (mapbox::cheap_ruler::CheapRuler).
//
// libm calls are the platform's (Rust's f64 methods call glibc's libm on
// Linux, as the C++ does). `std::remainder` has no Rust std equivalent: the
// IEEE remainder is exact, so [`remainder`] computes it from the (exact)
// `fmod`. `::fmin`/`::fmax` follow glibc.

use geometry::{LineString, Point, Polygon};

/// `mapbox::geometry::box<double>`
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Box {
    pub min: Point<f64>,
    pub max: Point<f64>,
}

impl Box {
    #[must_use]
    pub const fn new(min: Point<f64>, max: Point<f64>) -> Self {
        Box { min, max }
    }
}

/// `std::remainder(x, y)`: `x - n * y` with `n` the integer nearest `x / y`
/// (ties to even), computed exactly; a zero result has the sign of `x`.
#[must_use]
pub fn remainder(x: f64, y: f64) -> f64 {
    let r = x % y; // fmod: exact, sign of x, |r| < |y|
    if r.is_nan() {
        return r;
    }
    let ay = y.abs();
    let ar = r.abs();
    // Round the quotient to nearest; on a tie, to even (the truncated
    // quotient is odd iff |fmod(x, 2y)| >= |y|; 2y = inf also works).
    let past_half = 2.0 * ar > ay || (2.0 * ar == ay && (x % (2.0 * y)).abs() >= ay);
    if past_half { if r > 0.0 { r - ay } else { r + ay } } else { r }
}

/// glibc `fmin`: the non-NaN argument; the first one when equal.
fn fmin(a: f64, b: f64) -> f64 {
    if a.is_nan() {
        b
    } else if b.is_nan() || a <= b {
        a
    } else {
        b
    }
}

/// glibc `fmax`: the non-NaN argument; the first one when equal.
fn fmax(a: f64, b: f64) -> f64 {
    if a.is_nan() {
        b
    } else if b.is_nan() || a >= b {
        a
    } else {
        b
    }
}

/// `CheapRuler::Unit`
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Unit {
    Kilometers,
    Miles,
    NauticalMiles,
    Meters,
    Yards,
    Feet,
    Inches,
}

impl Unit {
    /// `Metres = Meters`
    #[allow(non_upper_case_globals)]
    pub const Metres: Unit = Unit::Meters;
}

/// `CheapRuler`
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CheapRuler {
    ky: f64,
    kx: f64,
}

// Values that define WGS84 ellipsoid model of the Earth
const RE: f64 = 6378.137; // equatorial radius
const FE: f64 = 1.0 / 298.257_223_563; // flattening

const E2: f64 = FE * (2.0 - FE);
const RAD: f64 = std::f64::consts::PI / 180.0;

impl CheapRuler {
    /// `CheapRuler(latitude, unit)` (C++ default unit: Kilometers).
    #[must_use]
    pub fn new(latitude: f64, unit: Unit) -> Self {
        let m = match unit {
            Unit::Kilometers => 1.0,
            Unit::Miles => 1000.0 / 1609.344,
            Unit::NauticalMiles => 1000.0 / 1852.0,
            Unit::Meters => 1000.0,
            Unit::Yards => 1000.0 / 0.9144,
            Unit::Feet => 1000.0 / 0.3048,
            Unit::Inches => 1000.0 / 0.0254,
        };

        // Curvature formulas from https://en.wikipedia.org/wiki/Earth_radius#Meridional
        let mul = RAD * RE * m;
        let coslat = (latitude * RAD).cos();
        let w2 = 1.0 / (1.0 - E2 * (1.0 - coslat * coslat));
        let w = w2.sqrt();

        // multipliers for converting longitude and latitude degrees into
        // distance
        CheapRuler {
            kx: mul * w * coslat,          // based on normal radius of curvature
            ky: mul * w * w2 * (1.0 - E2), // based on meridonal radius of curvature
        }
    }

    /// `fromTile(y, z)` (asserts `z < 32`).
    #[must_use]
    pub fn from_tile(y: u32, z: u32) -> Self {
        debug_assert!(z < 32);
        let n = std::f64::consts::PI * (1.0 - 2.0 * (f64::from(y) + 0.5) / f64::from(1u32.wrapping_shl(z)));
        let latitude = n.sinh().atan() / RAD;

        CheapRuler::new(latitude, Unit::Kilometers)
    }

    #[must_use]
    pub fn square_distance(&self, a: Point<f64>, b: Point<f64>) -> f64 {
        let dx = long_diff(a.x, b.x) * self.kx;
        let dy = (a.y - b.y) * self.ky;
        dx * dx + dy * dy
    }

    /// Given two points of the form [x = longitude, y = latitude], returns
    /// the distance.
    #[must_use]
    pub fn distance(&self, a: Point<f64>, b: Point<f64>) -> f64 {
        self.square_distance(a, b).sqrt()
    }

    /// Returns the bearing between two points in angles.
    #[must_use]
    pub fn bearing(&self, a: Point<f64>, b: Point<f64>) -> f64 {
        let dx = long_diff(b.x, a.x) * self.kx;
        let dy = (b.y - a.y) * self.ky;

        dx.atan2(dy) / RAD
    }

    /// Returns a new point given distance and bearing from the starting
    /// point.
    #[must_use]
    pub fn destination(&self, origin: Point<f64>, dist: f64, bearing: f64) -> Point<f64> {
        let a = bearing * RAD;

        self.offset(origin, a.sin() * dist, a.cos() * dist)
    }

    /// Returns a new point given easting and northing offsets from the
    /// starting point.
    #[must_use]
    pub fn offset(&self, origin: Point<f64>, dx: f64, dy: f64) -> Point<f64> {
        Point::new(origin.x + dx / self.kx, origin.y + dy / self.ky)
    }

    /// Given a line (an array of points), returns the total line distance.
    #[must_use]
    pub fn line_distance(&self, points: &LineString<f64>) -> f64 {
        let mut total = 0.0;

        for i in 1..points.len() {
            total += self.distance(points[i - 1], points[i]);
        }

        total
    }

    /// Given a polygon (an array of rings, where each ring is an array of
    /// points), returns the area.
    #[must_use]
    pub fn area(&self, poly: &Polygon<f64>) -> f64 {
        let mut sum = 0.0;

        for (i, ring) in poly.iter().enumerate() {
            // `unsigned` indices
            let len = ring.len() as u32;
            let mut k = len.wrapping_sub(1);
            for j in 0..len {
                let (rj, rk) = (ring[j as usize], ring[k as usize]);
                sum += long_diff(rj.x, rk.x) * (rj.y + rk.y) * (if i != 0 { -1.0 } else { 1.0 });
                k = j;
            }
        }

        (sum.abs() / 2.0) * self.kx * self.ky
    }

    /// Returns the point at a specified distance along the line.
    #[must_use]
    pub fn along(&self, line: &LineString<f64>, dist: f64) -> Point<f64> {
        let mut sum = 0.0;

        if line.is_empty() {
            return Point::new(0.0, 0.0);
        }

        if dist <= 0.0 {
            return line[0];
        }

        for i in 0..line.len() - 1 {
            let p0 = line[i];
            let p1 = line[i + 1];
            let d = self.distance(p0, p1);

            sum += d;

            if sum > dist {
                return Self::interpolate(p0, p1, (dist - (sum - d)) / d);
            }
        }

        line[line.len() - 1]
    }

    /// Returns the distance from a point `p` to a line segment `a` to `b`.
    #[must_use]
    pub fn point_to_segment_distance(&self, p: &Point<f64>, a: &Point<f64>, b: &Point<f64>) -> f64 {
        let mut x = a.x;
        let mut y = a.y;
        let dx = long_diff(b.x, x) * self.kx;
        let dy = (b.y - y) * self.ky;

        if dx != 0.0 || dy != 0.0 {
            let t = (long_diff(p.x, x) * self.kx * dx + (p.y - y) * self.ky * dy) / (dx * dx + dy * dy);
            if t > 1.0 {
                x = b.x;
                y = b.y;
            } else if t > 0.0 {
                x += (dx / self.kx) * t;
                y += (dy / self.ky) * t;
            }
        }
        self.distance(*p, Point::new(x, y))
    }

    /// Returns `(point, index, t)`: the closest point on the line to `p`,
    /// the start index of its segment, and its position `t` in 0..1 on that
    /// segment.
    #[must_use]
    pub fn point_on_line(&self, line: &LineString<f64>, p: Point<f64>) -> (Point<f64>, u32, f64) {
        let mut min_dist = f64::INFINITY;
        let (mut min_x, mut min_y, mut min_i, mut min_t) = (0.0, 0.0, 0.0, 0.0);

        if line.is_empty() {
            return (Point::new(0.0, 0.0), 0, 0.0);
        }

        for i in 0..(line.len() - 1) as u32 {
            let iu = i as usize;
            let mut t = 0.0;
            let mut x = line[iu].x;
            let mut y = line[iu].y;
            let dx = long_diff(line[iu + 1].x, x) * self.kx;
            let dy = (line[iu + 1].y - y) * self.ky;

            if dx != 0.0 || dy != 0.0 {
                t = (long_diff(p.x, x) * self.kx * dx + (p.y - y) * self.ky * dy) / (dx * dx + dy * dy);
                if t > 1.0 {
                    x = line[iu + 1].x;
                    y = line[iu + 1].y;
                } else if t > 0.0 {
                    x += (dx / self.kx) * t;
                    y += (dy / self.ky) * t;
                }
            }

            let sq_dist = self.square_distance(p, Point::new(x, y));

            if sq_dist < min_dist {
                min_dist = sq_dist;
                min_x = x;
                min_y = y;
                min_i = f64::from(i);
                min_t = t;
            }
        }

        // `unsigned(minI)`: an index, exact.
        (Point::new(min_x, min_y), min_i as u32, fmax(0.0, fmin(1.0, min_t)))
    }

    /// Returns a part of the given line between the start and the stop
    /// points (or their closest points on the line).
    #[must_use]
    pub fn line_slice(&self, start: Point<f64>, stop: Point<f64>, line: &LineString<f64>) -> LineString<f64> {
        let mut p1 = self.point_on_line(line, start);
        let mut p2 = self.point_on_line(line, stop);

        if p1.1 > p2.1 || (p1.1 == p2.1 && p1.2 > p2.2) {
            std::mem::swap(&mut p1, &mut p2);
        }

        let mut slice = vec![p1.0];

        let l = p1.1.wrapping_add(1);
        let r = p2.1;

        if line[l as usize] != slice[0] && l <= r {
            slice.push(line[l as usize]);
        }

        let mut i = l.wrapping_add(1);
        while i <= r {
            slice.push(line[i as usize]);
            i += 1;
        }

        if line[r as usize] != p2.0 {
            slice.push(p2.0);
        }

        slice
    }

    /// Returns a part of the given line between the start and the stop
    /// points indicated by distance along the line.
    #[must_use]
    pub fn line_slice_along(&self, start: f64, stop: f64, line: &LineString<f64>) -> LineString<f64> {
        let mut sum = 0.0;
        let mut slice = LineString::new();

        for i in 1..line.len() {
            let p0 = line[i - 1];
            let p1 = line[i];
            let d = self.distance(p0, p1);

            sum += d;

            if sum > start && slice.is_empty() {
                slice.push(Self::interpolate(p0, p1, (start - (sum - d)) / d));
            }

            if sum >= stop {
                slice.push(Self::interpolate(p0, p1, (stop - (sum - d)) / d));
                return slice;
            }

            if sum > start {
                slice.push(p1);
            }
        }

        slice
    }

    /// Given a point, returns a bounding box object ([w, s, e, n]) created
    /// from the given point buffered by a given distance.
    #[must_use]
    pub fn buffer_point(&self, p: Point<f64>, buffer: f64) -> Box {
        let v = buffer / self.ky;
        let h = buffer / self.kx;

        Box::new(Point::new(p.x - h, p.y - v), Point::new(p.x + h, p.y + v))
    }

    /// Given a bounding box, returns the box buffered by a given distance.
    #[must_use]
    pub fn buffer_bbox(&self, bbox: Box, buffer: f64) -> Box {
        let v = buffer / self.ky;
        let h = buffer / self.kx;

        Box::new(Point::new(bbox.min.x - h, bbox.min.y - v), Point::new(bbox.max.x + h, bbox.max.y + v))
    }

    /// Returns true if the given point is inside in the given bounding box,
    /// otherwise false.
    #[must_use]
    pub fn inside_bbox(p: Point<f64>, bbox: Box) -> bool {
        p.y >= bbox.min.y && p.y <= bbox.max.y && long_diff(p.x, bbox.min.x) >= 0.0 && long_diff(p.x, bbox.max.x) <= 0.0
    }

    #[must_use]
    pub fn interpolate(a: Point<f64>, b: Point<f64>, t: f64) -> Point<f64> {
        let dx = long_diff(b.x, a.x);
        let dy = b.y - a.y;

        Point::new(a.x + dx * t, a.y + dy * t)
    }
}

fn long_diff(a: f64, b: f64) -> f64 {
    remainder(a - b, 360.0)
}
