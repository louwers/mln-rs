// C++ undefined behaviour kept out of the port: `static_cast<int32_t>` of a
// double outside the int32 range (or NaN) saturates here (`as i32`); unsigned
// `uint32_t` arithmetic on `tileY` wraps (`wrapping_*`); shifts by the zoom
// mask the count like x86 does (`wrapping_shl`), as in mln_tile_id.

use std::collections::VecDeque;
use std::iter::Peekable;

use geometry::Point;
use mln_math::clamp_f64;
use mln_tile_id::UnwrappedTileID;
use mln_util::error::Error;
use mln_util::geo::LatLng;
use mln_util::geometry::{FeatureType, Geometry, MultiPoint, Polygon, to_feature_type};
use mln_util::projection::Projection;
use mln_vec_map::VecMap;

pub(crate) type Bounds = Vec<Bound>;
pub(crate) type BoundsMap = VecMap<u32, Bounds>;

/// A chain of points from a local minimum to a local maximum. `winding`
/// indicates the direction of the original geometry.
///
/// The C++ struct declares a copy constructor and a move assignment
/// operator that both copy/move all three fields; `Clone` and Rust moves are
/// the same thing.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Bound {
    pub(crate) points: Vec<Point<f64>>,
    pub(crate) current_point: usize,
    pub(crate) winding: bool,
}

impl Bound {
    /// Compute the interpolated x coordinate at y for the current edge
    #[must_use]
    pub(crate) fn interpolate(&self, y: u32) -> f64 {
        let p0 = &self.points[self.current_point];
        let p1 = &self.points[self.current_point + 1];

        let dx = p1.x - p0.x;
        let dy = p1.y - p0.y;
        let mut x = p0.x;
        if dx == 0.0 {
            return x;
        } else if dy == 0.0 {
            return if f64::from(y) <= p0.y { p0.x } else { p1.x };
        }
        if f64::from(y) < p0.y {
            return x;
        }
        if f64::from(y) > p1.y {
            return p1.x;
        }
        x = (dx / dy) * (f64::from(y) - p0.y) + p0.x;
        x
    }
}

type PointList = Vec<Point<f64>>;

#[derive(Clone, Copy, Debug, PartialEq)]
struct TileSpan {
    xmin: i32,
    xmax: i32,
    winding: bool,
}

/// Reorder a ring of points such that it starts at a point with a local
/// minimum y-coordinate
fn start_list_on_local_minimum(points: &mut PointList) {
    // Iterators are indices; the caller guarantees `points.len() >= 2`.
    let end = points.len();
    let mut prev_pt = end - 2;
    let mut pt = 0;
    let mut next_pt = pt + 1;
    while pt != end {
        if (points[pt].y <= points[prev_pt].y) && (points[pt].y < points[next_pt].y) {
            break;
        }
        prev_pt = pt;
        pt += 1;
        next_pt += 1;
        if next_pt == end {
            next_pt = 1;
        }
    }
    if pt == end {
        return;
    }
    // Re-close linear rings with first_pt = last_pt
    if points.last() == points.first() {
        points.pop();
    }
    // `std::rotate(points.begin(), pt, points.end())`
    points.rotate_left(pt);
    points.push(points[0]);
}

/// Create a bound towards a local maximum point, starting from pt.
///  Traverse from current pt until the next pt changes y-direction, and copy
///  all points from start to end (inclusive) into a Bound.
fn create_bound_towards_maximum(points: &PointList, pt: &mut usize) -> Bound {
    if points.len() - *pt < 2 {
        return Bound::default();
    }

    let begin = *pt;
    let mut next_pt = begin + 1;
    while points[*pt].y <= points[next_pt].y {
        *pt += 1;
        next_pt += 1;
        if next_pt == points.len() {
            *pt += 1;
            break;
        }
    }

    let pt_distance = next_pt - begin;
    if pt_distance < 2 {
        return Bound::default();
    }

    let mut bnd = Bound::default();
    bnd.points.reserve(next_pt - begin);
    bnd.points.extend_from_slice(&points[begin..next_pt]);
    bnd.winding = true;
    bnd
}

/// Create a bound towards a local minimum point, starting from pt.
///  Traverse from current pt until the next pt changes y-direction, and copy
///  all points from start to end (inclusive) into a Bound.
fn create_bound_towards_minimum(points: &PointList, pt: &mut usize) -> Bound {
    if points.len() - *pt < 2 {
        return Bound::default();
    }

    let begin = *pt;
    let mut next_pt = begin + 1;
    while points[*pt].y > points[next_pt].y {
        *pt += 1;
        next_pt += 1;
        if next_pt == points.len() {
            *pt += 1;
            break;
        }
    }

    let pt_distance = next_pt - begin;
    if pt_distance < 2 {
        return Bound::default();
    }
    let mut bnd = Bound::default();
    bnd.points.reserve(next_pt - begin);
    // For bounds that start at a max, reverse copy so that all bounds start at a min
    bnd.points.extend(points[begin..next_pt].iter().rev().copied());
    bnd.winding = false;
    bnd
}

/// The tile row of a bound's first point: `static_cast<uint32_t>(std::floor(
/// clamp(y, 0.0, static_cast<double>(maxTile))))`. The clamp is
/// `util::clamp` (fmin/fmax), so it never yields NaN.
fn start_row(y: f64, max_tile: f64) -> u32 {
    clamp_f64(y, 0.0, max_tile).floor() as u32
}

/// Given a set of points (ring or list) representing a shape, compute a set
/// of Bounds, where each Bound represents edges going from a local minima to
/// a local maxima point. The `BoundsMap` is an edge table indexed on the
/// starting Y-tile of each Bound.
fn build_bounds_map(points: &mut PointList, max_tile: u32, et: &mut BoundsMap, closed: bool) {
    if points.len() < 2 {
        return;
    }
    // While traversing closed rings, start the bounds at a local minimum.
    //  (For linestrings the starting point is always a local maxima/minima)
    if closed {
        start_list_on_local_minimum(points);
    }

    let mut points_iter = 0;
    while points_iter != points.len() {
        let to_max = create_bound_towards_maximum(points, &mut points_iter);
        let to_min = create_bound_towards_minimum(points, &mut points_iter);

        if to_max.points.len() >= 2 {
            // Projections may result in values beyond the bounds, clamp to max tile coordinates
            let y = start_row(to_max.points[0].y, f64::from(max_tile));
            et.entry(y).or_default().push(to_max);
        }
        if to_min.points.len() >= 2 {
            let y = start_row(to_min.points[0].y, f64::from(max_tile));
            et.entry(y).or_default().push(to_min);
        }
    }
    debug_assert_eq!(points_iter, points.len());
}

fn update_span(xp: &mut TileSpan, x: f64) {
    xp.xmin = std::cmp::min(xp.xmin, x.floor() as i32);
    xp.xmax = std::cmp::max(xp.xmax, x.ceil() as i32);
}

/// Use the active bounds, an accumulation of all bounds that enter the y tile
/// row, or start in that row.
/// Iterate all points of a bound until it exits the row (or ends) and compute
/// the set of X tiles it spans across. The winding direction of the bound is
/// also captured for each span to later fill tiles between bounds for
/// polygons
fn scan_row(y: u32, active_bounds: &mut Bounds) -> Vec<TileSpan> {
    let mut tile_range: Vec<TileSpan> = Vec::with_capacity(active_bounds.len());

    // `y + 1` is uint32_t arithmetic, converted to double where it meets one.
    let y1 = y.wrapping_add(1);
    for b in active_bounds.iter_mut() {
        // Quirk kept: xmax starts at 0, not INT_MIN.
        let mut xp = TileSpan { xmin: i32::MAX, xmax: 0, winding: b.winding };
        let mut x;
        let num_edges = b.points.len() - 1;
        while b.current_point < num_edges {
            x = b.interpolate(y);
            update_span(&mut xp, x);

            // If this edge ends beyond the current row, find the x-intercept
            // where it exits the row
            let p1 = b.points[b.current_point + 1];
            if p1.y > f64::from(y1) {
                x = b.interpolate(y1);
                update_span(&mut xp, x);
                break;
            } else if b.current_point == num_edges - 1 {
                // For last edge, consider x-intercept at the end of the edge.
                x = p1.x;
                update_span(&mut xp, x);
            }
            b.current_point += 1;
        }
        tile_range.push(xp);
    }
    // Erase bounds in the active table whose current edge ends inside this row,
    // or there are no more edges
    active_bounds.retain(|bound| {
        !(bound.current_point == bound.points.len() - 1 && bound.points[bound.current_point].y <= f64::from(y1))
    });
    // Sort the X-extents of each crossing bound by x_min, x_max
    tile_range.sort_by_key(|span| (span.xmin, span.xmax));

    tile_range
}

/// The geometry visitor that builds the edge table. Projecting a point with
/// an out-of-range latitude is an `Err`.
struct BuildBoundsMap {
    zoom: i32,
    project: bool,
}

impl BuildBoundsMap {
    fn new(z: i32, p: bool) -> Self {
        BuildBoundsMap { zoom: z, project: p }
    }

    /// `1 << zoom` (an int shift).
    fn max_tile(&self) -> i32 {
        1i32.wrapping_shl(self.zoom as u32)
    }

    fn project_point(&self, p: &Point<f64>) -> Result<Point<f64>, Error> {
        Ok(Projection::project_zoom(&LatLng::new(p.y, p.x)?, self.zoom))
    }

    fn build_table(&self, points: &[Point<f64>], et: &mut BoundsMap, closed: bool) -> Result<(), Error> {
        let mut projected_points: PointList = Vec::new();
        if self.project {
            projected_points.reserve(points.len());
            for p in points {
                projected_points.push(self.project_point(p)?);
            }
        } else {
            projected_points.extend_from_slice(points);
        }
        build_bounds_map(&mut projected_points, self.max_tile() as u32, et, closed);
        Ok(())
    }

    fn build_polygon_table(&self, polygon: &Polygon<f64>, et: &mut BoundsMap) -> Result<(), Error> {
        for ring in polygon {
            self.build_table(ring, et, true)?;
        }
        Ok(())
    }

    /// The body of `operator()(const Point<double>&)`, which
    /// `operator()(const MultiPoint<double>&)` repeats per point.
    fn point(&self, p: &Point<f64>, et: &mut BoundsMap) -> Result<(), Error> {
        let mut bnd = Bound::default();
        let mut point = *p;
        if self.project {
            point = self.project_point(p)?;
        }
        bnd.points.extend([point, point]);
        bnd.winding = false;
        let y = start_row(point.y, f64::from(self.max_tile()));
        et.entry(y).or_default().push(bnd);
        Ok(())
    }

    fn multi_point(&self, points: &MultiPoint<f64>, et: &mut BoundsMap) -> Result<(), Error> {
        for p in points {
            self.point(p, et)?;
        }
        Ok(())
    }

    fn apply(&self, geom: &Geometry<f64>) -> Result<BoundsMap, Error> {
        let mut et = BoundsMap::new();
        match geom {
            Geometry::Empty | Geometry::GeometryCollection(_) => {}
            Geometry::Point(p) => self.point(p, &mut et)?,
            Geometry::MultiPoint(points) => self.multi_point(points, &mut et)?,
            Geometry::LineString(lines) => self.build_table(lines, &mut et, false)?,
            Geometry::MultiLineString(lines) => {
                for line in lines {
                    self.build_table(line, &mut et, false)?;
                }
            }
            Geometry::Polygon(polygon) => self.build_polygon_table(polygon, &mut et)?,
            Geometry::MultiPolygon(polygons) => {
                for polygon in polygons {
                    self.build_polygon_table(polygon, &mut et)?;
                }
            }
        }
        Ok(et)
    }
}

type TileSpans = VecDeque<(i32, i32)>;

/// Implements a modified scan-line algorithm to provide a streaming interface
/// for tile cover on arbitrary shapes. A `BoundsMap` is generated from the
/// input geometry where each tuple indicates the set of Bounds that start at a
/// y tile coordinate. Each bound represents a chain of edges from a local
/// y-minima to a local y-maxima. For each row, the activeBounds list
/// aggregates all bounds that enter into or begin in that row. This running
/// list of bounds is scanned, capturing the x-coordinates spanned by edges in
/// a bound until the bound exits the row (or ends). The result is a set of
/// (possibly overlapping) min,max pairs of x coordinates (spans). In the
/// simplest case a span represents the x-coordinates at which a single edge
/// intersects the top and bottom of a tile row. Interior tiles of a polygon
/// are captured by merging spans using the non-zero rule. The result of a scan
/// using `nextRow()` is a list of spans (tileXSpans) of x-coordinates that
/// includes edges and interiors of polygons. `next()` returns a tileID for each
/// x-coordinate from (first, second] in each span in tileXSpans.
pub(crate) struct Impl {
    zoom: i32,
    is_closed: bool,

    /// The bounds map's entries not yet moved into `active_bounds`, in key
    /// order.
    current_bounds: Peekable<mln_vec_map::IntoIter<u32, Bounds>>,
    /// List of bounds that begin at or before `tileY`
    active_bounds: Bounds,

    tile_x_spans: TileSpans,
    // Uninitialized in C++ when the bounds map is empty (and then never read).
    tile_y: u32,
    tile_x: i32,
}

impl Impl {
    /// The cover of `geom` at zoom `z`, projecting the geometry if `project`
    /// (C++ defaults it to true). Errors for a point with an out-of-range
    /// latitude while projecting.
    pub(crate) fn new(z: i32, geom: &Geometry<f64>, project: bool) -> Result<Impl, Error> {
        let is_closed = to_feature_type(geom) == FeatureType::Polygon;

        let to_bounds_map = BuildBoundsMap::new(z, project);
        let bounds_map = to_bounds_map.apply(geom)?;
        let empty = bounds_map.is_empty();
        let mut this = Impl {
            zoom: z,
            is_closed,
            current_bounds: bounds_map.into_iter().peekable(),
            active_bounds: Bounds::new(),
            tile_x_spans: TileSpans::new(),
            tile_y: 0,
            tile_x: 0,
        };
        if empty {
            return Ok(this);
        }

        // Initialize the active edge table, and current row span
        this.tile_y = 0;
        this.next_row();
        if let Some(&(first, _)) = this.tile_x_spans.front() {
            this.tile_x = first;
        }
        Ok(this)
    }

    /// Aggregate all Bounds that start in or enter into the next tileY row.
    /// Multi-geoms may have discontinuity in the `BoundMap`, so skip forward to
    /// the next tileY row when the current/next row has no more bounds in it.
    /// Use `scan_row` to generate the tileX spans. Merge spans to avoid
    /// duplicate tiles in `TileCoverImpl::next()`. For closed geometry, use the
    /// non-zero rule to expand (fill) tiles between pairs of spans.
    fn next_row(&mut self) {
        // Update activeBounds for next row
        if let Some(&(first, _)) = self.current_bounds.peek() {
            if self.active_bounds.is_empty() && first > self.tile_y {
                // For multi-geoms: use the next row with an edge table starting point
                self.tile_y = first;
            }
            if self.tile_y == first {
                let (_, bounds) = self.current_bounds.next().expect("peeked");
                self.active_bounds.extend(bounds);
            }
        }
        // Scan the active bounds and update currentRange with x_min, x_max pairs
        let xps = scan_row(self.tile_y, &mut self.active_bounds);
        if xps.is_empty() {
            return;
        }

        let mut x_min = xps[0].xmin;
        let mut x_max = xps[0].xmax;
        let mut nz_rule: i32 = if xps[0].winding { 1 } else { -1 };
        for xp in &xps[1..] {
            if (!self.is_closed || nz_rule == 0) && xp.xmin > x_max && xp.xmax >= x_max {
                self.tile_x_spans.push_back((x_min, x_max));
                x_min = xp.xmin;
            }
            nz_rule += if xp.winding { 1 } else { -1 };
            // Quirk kept: `std::max(x_min, xp.xmax)`, not `std::max(x_max, xp.xmax)`.
            x_max = std::cmp::max(x_min, xp.xmax);
        }
        self.tile_x_spans.push_back((x_min, x_max));
    }

    #[must_use]
    pub(crate) fn has_next(&self) -> bool {
        match self.tile_x_spans.front() {
            Some(&(_, second)) => self.tile_x < second && self.tile_y < 1u32.wrapping_shl(self.zoom as u32),
            None => false,
        }
    }
}

impl Iterator for Impl {
    type Item = UnwrappedTileID;

    fn next(&mut self) -> Option<UnwrappedTileID> {
        if !self.has_next() {
            return None;
        }

        let x = self.tile_x;
        let y = self.tile_y;
        self.tile_x += 1;
        if self.tile_x >= self.tile_x_spans.front().expect("has_next").1 {
            self.tile_x_spans.pop_front();
            if self.tile_x_spans.is_empty() {
                self.tile_y = self.tile_y.wrapping_add(1);
                self.next_row();
            }
            if let Some(&(first, _)) = self.tile_x_spans.front() {
                self.tile_x = first;
            }
        }
        // `UnwrappedTileID(zoom, x, y)`: the int32_t zoom narrows to uint8_t.
        Some(UnwrappedTileID::new(self.zoom as u8, i64::from(x), i64::from(y)))
    }
}
