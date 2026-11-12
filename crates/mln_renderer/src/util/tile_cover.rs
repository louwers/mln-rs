// Ports src/mln/util/tile_cover.hpp and src/mln/util/tile_cover.cpp.
//
// The C++ overloads get distinct names: `tileCover(const TileCoverParameters&,
// ...)` is `tile_cover`, `tileCover(const LatLngBounds&, z)` is
// `tile_cover_bounds`, `tileCover(const Geometry<double>&, z)` is
// `tile_cover_geometry`, and `tileCount` likewise. The two `TileCover`
// constructors are `TileCover::new_from_bounds` / `new_from_geometry`.
//
// The geometry entry points return `Result`: C++ throws `std::domain_error`
// from the `LatLng` constructor while projecting a point whose latitude is
// out of range.
//
// `std::sort` is libstdc++'s introsort (ported below as `std_sort`), because
// the tile order for equal sort keys is observable in the results.

use std::cmp::Ordering;

use geometry::Point;
use mln_map_types::{TileCoordinate, TileLodMode, TransformState, ViewportMode};
use mln_math::{fmax, fmin};
use mln_style_types::SourceType;
use mln_tile_id::{OverscaledTileID, UnwrappedTileID};
use mln_util::bounding_volumes::{AABB, Frustum, IntersectionResult};
use mln_util::constants;
use mln_util::error::Error;
use mln_util::geo::{LatLng, LatLngBounds, ScreenCoordinate};
use mln_util::geometry::{Geometry, Polygon};
use mln_util::mat3::{vec3_length, vec3_scale, vec3_sub};
use mln_util::projection::Projection;
use mln_util::range::Range;
use mln_util::vectors::Vec3;

use crate::util::tile_cover_impl::Impl;

/// `std::min(a, b)` on doubles: `(b < a) ? b : a`.
fn std_min(a: f64, b: f64) -> f64 {
    if b < a { b } else { a }
}

/// `std::max(a, b)` on doubles: `(a < b) ? b : a`.
fn std_max(a: f64, b: f64) -> f64 {
    if a < b { b } else { a }
}

/// `LatLng{lat, lon}` built from the coordinates of an existing
/// `LatLngBounds` (with the latitude clamped to ±`LATITUDE_MAX`), so the
/// constructor's checks cannot fail.
fn bounds_lat_lng(lat: f64, lon: f64) -> LatLng {
    LatLng::new(lat, lon).expect("LatLng from LatLngBounds coordinates")
}

/// `ScanLine`: `const std::function<void(int32_t x0, int32_t x1, int32_t y)>`.
type ScanLine<'a> = dyn FnMut(i32, i32, i32) + 'a;

// Taken from polymaps src/Layer.js
// https://github.com/simplegeo/polymaps/blob/master/src/Layer.js#L333-L383
#[derive(Clone, Copy, Debug)]
struct Edge {
    x0: f64,
    y0: f64,
    x1: f64,
    y1: f64,
    dx: f64,
    dy: f64,
}

impl Edge {
    fn new(mut a: Point<f64>, mut b: Point<f64>) -> Self {
        if a.y > b.y {
            std::mem::swap(&mut a, &mut b);
        }
        Edge { x0: a.x, y0: a.y, x1: b.x, y1: b.y, dx: b.x - a.x, dy: b.y - a.y }
    }
}

/// scan-line conversion
fn scan_spans(mut e0: Edge, mut e1: Edge, ymin: i32, ymax: i32, scan_line: &mut ScanLine<'_>) {
    let y0 = fmax(f64::from(ymin), e1.y0.floor());
    let y1 = fmin(f64::from(ymax), e1.y1.ceil());

    // sort edges by x-coordinate
    let swap = if e0.x0 == e1.x0 && e0.y0 == e1.y0 {
        e0.x0 + e1.dy / e0.dy * e0.dx < e1.x1
    } else {
        e0.x1 - e1.dy / e0.dy * e0.dx < e1.x0
    };
    if swap {
        std::mem::swap(&mut e0, &mut e1);
    }

    // scan lines!
    let m0 = e0.dx / e0.dy;
    let m1 = e1.dx / e1.dy;
    let d0 = if e0.dx > 0.0 { 1.0 } else { 0.0 }; // use y + 1 to compute x0
    let d1 = if e1.dx < 0.0 { 1.0 } else { 0.0 }; // use y + 1 to compute x1
    // A double loop counter, as in C++.
    let mut y = y0;
    while y < y1 {
        let x0 = m0 * fmax(0.0, fmin(e0.dy, y + d0 - e0.y0)) + e0.x0;
        let x1 = m1 * fmax(0.0, fmin(e1.dy, y + d1 - e1.y0)) + e1.x0;
        // `static_cast<int32_t>`: UB out of range in C++, saturates here.
        scan_line(x1.floor() as i32, x0.ceil() as i32, y as i32);
        y += 1.0;
    }
}

/// scan-line conversion
fn scan_triangle(a: &Point<f64>, b: &Point<f64>, c: &Point<f64>, ymin: i32, ymax: i32, scan_line: &mut ScanLine<'_>) {
    let mut ab = Edge::new(*a, *b);
    let mut bc = Edge::new(*b, *c);
    let mut ca = Edge::new(*c, *a);

    // sort edges by y-length
    if ab.dy > bc.dy {
        std::mem::swap(&mut ab, &mut bc);
    }
    if ab.dy > ca.dy {
        std::mem::swap(&mut ab, &mut ca);
    }
    if bc.dy > ca.dy {
        std::mem::swap(&mut bc, &mut ca);
    }

    // scan span! scan span!
    // `if (ab.dy)`: true for any non-zero value, NaN included.
    if ab.dy != 0.0 {
        scan_spans(ca, ab, ymin, ymax, scan_line);
    }
    if bc.dy != 0.0 {
        scan_spans(ca, bc, ymin, ymax, scan_line);
    }
}

/// The anonymous-namespace `tileCover(tl, tr, br, bl, c, z)`.
fn tile_cover_points(
    tl: &Point<f64>,
    tr: &Point<f64>,
    br: &Point<f64>,
    bl: &Point<f64>,
    c: &Point<f64>,
    z: u8,
) -> Vec<UnwrappedTileID> {
    #[derive(Clone, Copy, Debug)]
    struct ID {
        x: i32,
        y: i32,
        sq_dist: f64,
    }

    // `1 << z` (an int shift).
    let tiles: i32 = 1i32.wrapping_shl(u32::from(z));

    // skip the first few allocations, assuming we usually end up with at least a few tiles
    let mut t: Vec<ID> = Vec::with_capacity(8);

    {
        let mut scan_line = |x0: i32, x1: i32, y: i32| {
            // Quirk kept: `y <= tiles` lets the row below the world through;
            // the UnwrappedTileID constructor clamps it.
            if y >= 0 && y <= tiles {
                let mut x = x0;
                while x < x1 {
                    let dx = f64::from(x) + 0.5 - c.x;
                    let dy = f64::from(y) + 0.5 - c.y;
                    t.push(ID { x, y, sq_dist: dx * dx + dy * dy });
                    x += 1;
                }
            }
        };

        // Divide the screen up in two triangles and scan each of them:
        // \---+
        // | \ |
        // +---\.
        scan_triangle(tl, tr, br, 0, tiles, &mut scan_line);
        scan_triangle(br, bl, tl, 0, tiles, &mut scan_line);
    }

    // Sort first by distance, then by x/y.
    // `std::tie(a.sqDist, a.x, a.y) < std::tie(b.sqDist, b.x, b.y)`: C++20
    // tuple `<=>`, so the first pair that is not equivalent decides and an
    // unordered (NaN) pair compares as not-less.
    std_sort(&mut t, |a, b| {
        match a.sq_dist.partial_cmp(&b.sq_dist) {
            Some(Ordering::Equal) => {}
            ordering => return ordering == Some(Ordering::Less),
        }
        (a.x, a.y) < (b.x, b.y)
    });

    // Erase duplicate tile IDs (they typically occur at the common side of both triangles).
    // (`std::unique` keeps the first element of each run, as `dedup_by` does.)
    t.dedup_by(|b, a| a.x == b.x && a.y == b.y);

    let mut result = Vec::with_capacity(t.len());
    for id in &t {
        result.push(UnwrappedTileID::new(z, i64::from(id.x), i64::from(id.y)));
    }
    result
}

/// Helper class to stream tile-cover results per row
pub struct TileCover {
    impl_: Box<Impl>,
}

impl TileCover {
    /// `TileCover(const LatLngBounds&, uint8_t z)`.
    #[must_use]
    pub fn new_from_bounds(bounds_: &LatLngBounds, z: u8) -> TileCover {
        let mut bounds = LatLngBounds::hull(
            bounds_lat_lng(std_max(bounds_.south(), -constants::LATITUDE_MAX), bounds_.west()),
            bounds_lat_lng(std_min(bounds_.north(), constants::LATITUDE_MAX), bounds_.east()),
        );

        if bounds.is_empty() || bounds.south() > constants::LATITUDE_MAX || bounds.north() < -constants::LATITUDE_MAX {
            bounds = LatLngBounds::world();
        }

        // `Projection::project(LatLng, uint8_t)` picks the `int32_t zoom`
        // overload (integral promotion beats the conversion to double).
        let sw = Projection::project_zoom(&bounds.southwest(), i32::from(z));
        let ne = Projection::project_zoom(&bounds.northeast(), i32::from(z));
        let se = Projection::project_zoom(&bounds.southeast(), i32::from(z));
        let nw = Projection::project_zoom(&bounds.northwest(), i32::from(z));

        let p: Polygon<f64> = vec![vec![sw, nw, ne, se, sw]];
        // Nothing is projected (`project = false`), so nothing can throw.
        TileCover {
            impl_: Box::new(Impl::new(i32::from(z), &Geometry::Polygon(p), false).expect("unprojected tile cover")),
        }
    }

    /// `TileCover(const Geometry<double>&, uint8_t z, bool project = true)`.
    /// When project == true, projects the geometry points to tile
    /// coordinates, and fails with
    /// [`ErrorKind::InvalidArgument`](mln_util::ErrorKind) for a point that
    /// isn't a valid `LatLng`.
    pub fn new_from_geometry(geom: &Geometry<f64>, z: u8, project: bool) -> Result<TileCover, Error> {
        Ok(TileCover { impl_: Box::new(Impl::new(i32::from(z), geom, project)?) })
    }

    #[must_use]
    pub fn has_next(&self) -> bool {
        self.impl_.has_next()
    }
}

impl Iterator for TileCover {
    type Item = UnwrappedTileID;

    fn next(&mut self) -> Option<UnwrappedTileID> {
        self.impl_.next()
    }
}

/// C++ holds the transform state by reference (it must outlive the
/// parameters); the port keeps the copy it always had.
#[derive(Clone, Debug)]
pub struct TileCoverParameters {
    pub transform_state: TransformState,
    pub tile_lod_min_radius: f64,
    pub tile_lod_scale: f64,
    pub tile_lod_pitch_threshold: f64,
    pub tile_lod_mode: TileLodMode,
    /// The zoom the source loads at the center of the view, shifted for its
    /// tile size and not yet clamped to its zoom range; the globe's level of
    /// detail falls off from it, as GL JS's does.
    pub requested_zoom: Option<f64>,
    /// Whether the source rounds zooms to whole tiles, as raster sources do,
    /// instead of flooring them.
    pub round_zoom: bool,
}

impl TileCoverParameters {
    /// `TileCoverParameters{transformState}` with the default member
    /// initializers for the other fields.
    pub fn new(transform_state: TransformState) -> Self {
        TileCoverParameters {
            transform_state,
            tile_lod_min_radius: 3.0,
            tile_lod_scale: 1.0,
            tile_lod_pitch_threshold: (60.0 / 180.0) * std::f64::consts::PI,
            tile_lod_mode: TileLodMode::Default,
            requested_zoom: None,
            round_zoom: false,
        }
    }
}

#[must_use]
pub fn covering_zoom_level(zoom: f64, type_: SourceType, size: u16) -> i32 {
    let mut zoom = zoom;
    zoom += mln_math::log2(constants::TILE_SIZE_D / f64::from(size));
    if type_ == SourceType::Raster || type_ == SourceType::RasterDEM {
        zoom.round() as i32
    } else {
        zoom.floor() as i32
    }
}

/// The globe tile cover: a port of GL JS `coveringTiles` with
/// `GlobeCoveringTilesDetailsProvider`. Tiles are tested against the camera
/// frustum in unit-sphere space with a convex bounding volume each, and
/// against the horizon plane, so the far side of the planet loads nothing.
mod globe {
    use std::cmp::Ordering;
    use std::f64::consts::PI;

    use geometry::Point;
    use mln_map_types::VerticalPerspectiveProjection;
    use mln_tile_id::{CanonicalTileID, OverscaledTileID, UnwrappedTileID};
    use mln_util::bounding_volumes::{Frustum, IntersectionResult};
    use mln_util::constants;
    use mln_util::mat3::{vec3_cross, vec3_dot, vec3_normalize, vec3_scale, vec3_sub};
    use mln_util::mat4::{Mat4, matrix};
    use mln_util::projection::Projection;
    use mln_util::range::Range;
    use mln_util::vectors::{Vec3, Vec4};

    use super::{TileCoverParameters, std_max, std_min};
    use crate::util::tile_lod::{TileZoomFunction, elevation_for_tile_culling};

    /// `std::clamp(value, lo, hi)`: `value < lo ? lo : hi < value ? hi :
    /// value`.
    fn clamp(value: f64, lo: f64, hi: f64) -> f64 {
        if value < lo {
            lo
        } else if hi < value {
            hi
        } else {
            value
        }
    }

    /// A tile's bounding volume in unit-sphere space: a box at zoom 0 and 1,
    /// a wedge with up to eight corners after.
    struct ConvexVolume {
        points: [Vec3; 8],
        point_count: usize,
        planes: [Vec4; 6],
    }

    impl ConvexVolume {
        fn new() -> Self {
            ConvexVolume { points: [[0.0; 3]; 8], point_count: 0, planes: [[0.0; 4]; 6] }
        }

        fn add_point(&mut self, point: Vec3) {
            debug_assert!(self.point_count < self.points.len());
            self.points[self.point_count] = point;
            self.point_count += 1;
        }

        fn points(&self) -> &[Vec3] {
            &self.points[..self.point_count]
        }
    }

    fn passes(plane: &Vec4, point: &Vec3) -> bool {
        plane[0] * point[0] + plane[1] * point[1] + plane[2] * point[2] + plane[3] >= 0.0
    }

    fn intersects_frustum(volume: &ConvexVolume, frustum: &Frustum) -> IntersectionResult {
        let mut fully_inside = true;
        for plane in frustum.get_planes() {
            let passed = volume.points().iter().filter(|point| passes(plane, point)).count();
            if passed == 0 {
                return IntersectionResult::Separate;
            }
            if passed < volume.point_count {
                fully_inside = false;
            }
        }
        if fully_inside {
            return IntersectionResult::Contains;
        }
        for plane in &volume.planes {
            let passed = frustum.get_points().iter().filter(|point| passes(plane, point)).count();
            if passed == 0 {
                return IntersectionResult::Separate;
            }
        }
        IntersectionResult::Intersects
    }

    fn intersects_plane(volume: &ConvexVolume, plane: &Vec4) -> IntersectionResult {
        let positive = volume.points().iter().filter(|point| passes(plane, point)).count();
        if positive == volume.point_count {
            return IntersectionResult::Contains;
        }
        if positive == 0 {
            return IntersectionResult::Separate;
        }
        IntersectionResult::Intersects
    }

    fn is_tile_visible(frustum: &Frustum, volume: &ConvexVolume, plane: &Vec4) -> IntersectionResult {
        let frustum_test = intersects_frustum(volume, frustum);
        if frustum_test == IntersectionResult::Separate {
            return frustum_test;
        }
        let plane_test = intersects_plane(volume, plane);
        if plane_test == IntersectionResult::Separate {
            return IntersectionResult::Separate;
        }
        if frustum_test == IntersectionResult::Contains && plane_test == IntersectionResult::Contains {
            return IntersectionResult::Contains;
        }
        IntersectionResult::Intersects
    }

    fn three_plane_intersection(p0: &Vec4, p1: &Vec4, p2: &Vec4) -> Vec3 {
        let n0: Vec3 = [p0[0], p0[1], p0[2]];
        let n1: Vec3 = [p1[0], p1[1], p1[2]];
        let n2: Vec3 = [p2[0], p2[1], p2[2]];
        let det = vec3_dot(&n0, &vec3_cross(&n1, &n2));
        if det == 0.0 {
            return [0.0, 0.0, 0.0];
        }
        let mut sum = vec3_scale(&vec3_cross(&n1, &n2), -p0[3]);
        sum = vec3_sub(&sum, &vec3_scale(&vec3_cross(&n2, &n0), p1[3]));
        sum = vec3_sub(&sum, &vec3_scale(&vec3_cross(&n0, &n1), p2[3]));
        vec3_scale(&sum, 1.0 / det)
    }

    fn aabb_volume(min: &Vec3, max: &Vec3) -> ConvexVolume {
        let mut volume = ConvexVolume::new();
        for i in 0..8 {
            volume.add_point([
                if i & 1 != 0 { max[0] } else { min[0] },
                if i & 2 != 0 { max[1] } else { min[1] },
                if i & 4 != 0 { max[2] } else { min[2] },
            ]);
        }
        volume.planes = [
            [-1.0, 0.0, 0.0, max[0]],
            [1.0, 0.0, 0.0, -min[0]],
            [0.0, -1.0, 0.0, max[1]],
            [0.0, 1.0, 0.0, -min[1]],
            [0.0, 0.0, -1.0, max[2]],
            [0.0, 0.0, 1.0, -min[2]],
        ];
        volume
    }

    fn axis_min_max(axis: &Vec3, points: &[Vec3]) -> (f64, f64) {
        let mut min = f64::INFINITY;
        let mut max = f64::NEG_INFINITY;
        for point in points {
            let dot = vec3_dot(axis, point);
            min = std_min(min, dot);
            max = std_max(max, dot);
        }
        (min, max)
    }

    fn to_sphere(x: f64, y: f64, tile: &CanonicalTileID) -> Vec3 {
        VerticalPerspectiveProjection::tile_coordinates_to_sphere(
            &Point::new(x, y),
            &UnwrappedTileID::with_wrap(0, *tile),
        )
    }

    /// `elevation` in meters grows the volume above the sphere's surface.
    fn tile_bounding_volume(tile: &CanonicalTileID, elevation: f64) -> ConvexVolume {
        // Distances from the planet's center in a world where the surface is
        // at 1.
        let min_radius = std_min(0.0, elevation) / constants::EARTH_RADIUS_M + 1.0;
        let max_radius = std_max(0.0, elevation) / constants::EARTH_RADIUS_M + 1.0;
        if tile.z == 0 {
            return aabb_volume(&[-max_radius, -max_radius, -max_radius], &[max_radius, max_radius, max_radius]);
        }
        if tile.z == 1 {
            // X is 1 at lng 90E, Y is 1 at the north pole, Z is 1 at null
            // island.
            return aabb_volume(
                &[
                    if tile.x == 0 { -max_radius } else { 0.0 },
                    if tile.y == 0 { 0.0 } else { -max_radius },
                    -max_radius,
                ],
                &[if tile.x == 0 { 0.0 } else { max_radius }, if tile.y == 0 { max_radius } else { 0.0 }, max_radius],
            );
        }
        let extent = f64::from(constants::EXTENT);
        let corners: [Vec3; 4] = [
            to_sphere(0.0, 0.0, tile),
            to_sphere(extent, 0.0, tile),
            to_sphere(extent, extent, tile),
            to_sphere(0.0, extent, tile),
        ];
        // Corners at both radii, a pole if the tile touches one, the center,
        // and the bulging edge midpoint.
        let mut extremes: [Vec3; 11] = [[0.0; 3]; 11];
        let mut extreme_count = 0;
        for corner in &corners {
            extremes[extreme_count] = vec3_scale(corner, max_radius);
            extreme_count += 1;
        }
        if max_radius != min_radius {
            for corner in &corners {
                extremes[extreme_count] = vec3_scale(corner, min_radius);
                extreme_count += 1;
            }
        }
        let last_row: u32 = (1u32 << tile.z) - 1;
        if tile.y == 0 {
            extremes[extreme_count] = [0.0, 1.0, 0.0];
            extreme_count += 1;
        }
        if tile.y == last_row {
            extremes[extreme_count] = [0.0, -1.0, 0.0];
            extreme_count += 1;
        }

        // The up/down axis is the tile center; north/south is orthogonal to
        // it; the east and west planes follow the tile's meridian edges and
        // are not parallel, so the volume is a wedge rather than a box.
        let center = to_sphere(extent / 2.0, extent / 2.0, tile);
        let center_east = vec3_normalize(&vec3_cross(&[0.0, 1.0, 0.0], &center));
        let north = vec3_normalize(&vec3_cross(&center, &center_east));
        let axis_east = vec3_normalize(&vec3_cross(&corners[2], &corners[1]));
        let axis_west = vec3_normalize(&vec3_cross(&corners[0], &corners[3]));

        extremes[extreme_count] = vec3_scale(&center, max_radius);
        extreme_count += 1;
        // The edge midpoint that bulges away from the center: the north edge
        // in the south hemisphere, the south edge in the north.
        extremes[extreme_count] = if tile.y >= (1u32 << tile.z) / 2 {
            vec3_scale(&to_sphere(extent / 2.0, 0.0, tile), max_radius)
        } else {
            vec3_scale(&to_sphere(extent / 2.0, extent, tile), max_radius)
        };
        extreme_count += 1;
        let extreme_span = &extremes[..extreme_count];

        let (up_min, up_max) = axis_min_max(&center, extreme_span);
        let (north_min, north_max) = axis_min_max(&north, extreme_span);
        let plane_up: Vec4 = [-center[0], -center[1], -center[2], up_max];
        let plane_down: Vec4 = [center[0], center[1], center[2], -up_min];
        let plane_north: Vec4 = [-north[0], -north[1], -north[2], north_max];
        let plane_south: Vec4 = [north[0], north[1], north[2], -north_min];
        let plane_east: Vec4 = [axis_east[0], axis_east[1], axis_east[2], 0.0];
        let plane_west: Vec4 = [axis_west[0], axis_west[1], axis_west[2], 0.0];

        let mut volume = ConvexVolume::new();
        if tile.y == 0 {
            volume.add_point(three_plane_intersection(&plane_west, &plane_east, &plane_up));
            volume.add_point(three_plane_intersection(&plane_west, &plane_east, &plane_down));
        } else {
            volume.add_point(three_plane_intersection(&plane_north, &plane_east, &plane_up));
            volume.add_point(three_plane_intersection(&plane_north, &plane_east, &plane_down));
            volume.add_point(three_plane_intersection(&plane_north, &plane_west, &plane_up));
            volume.add_point(three_plane_intersection(&plane_north, &plane_west, &plane_down));
        }
        if tile.y == last_row {
            volume.add_point(three_plane_intersection(&plane_west, &plane_east, &plane_up));
            volume.add_point(three_plane_intersection(&plane_west, &plane_east, &plane_down));
        } else {
            volume.add_point(three_plane_intersection(&plane_south, &plane_east, &plane_up));
            volume.add_point(three_plane_intersection(&plane_south, &plane_east, &plane_down));
            volume.add_point(three_plane_intersection(&plane_south, &plane_west, &plane_up));
            volume.add_point(three_plane_intersection(&plane_south, &plane_west, &plane_down));
        }
        volume.planes = [plane_up, plane_down, plane_north, plane_south, plane_east, plane_west];
        volume
    }

    // Distances below are in a world of size 1, to the nearer tile edge.
    fn distance_to_tile_simple(point: f64, tile: f64, tile_size: f64) -> f64 {
        let delta = point - tile;
        if delta < 0.0 { -delta } else { std_max(0.0, delta - tile_size) }
    }

    fn distance_to_tile_wrap_x(point_x: f64, point_y: f64, corner_x: f64, corner_y: f64, tile_size: f64) -> f64 {
        let to_point_x = point_x - corner_x;
        let distance_x = if to_point_x < 0.0 {
            std_min(-to_point_x, 1.0 + to_point_x - tile_size)
        } else if to_point_x > tile_size {
            std_min(std_max(to_point_x - tile_size, 0.0), 1.0 - to_point_x)
        } else {
            0.0
        };
        std_max(distance_x, distance_to_tile_simple(point_y, corner_y, tile_size))
    }

    /// X wraps at the antimeridian; across a pole Y mirrors and X shifts by
    /// half the world.
    fn distance_to_tile_2d(point_x: f64, point_y: f64, tile: &CanonicalTileID) -> f64 {
        let scale = f64::from(1u32 << tile.z);
        let tile_size = 1.0 / scale;
        let corner_x = f64::from(tile.x) / scale;
        let corner_y = f64::from(tile.y) / scale;
        let mut smallest = 2.0;
        smallest = std_min(smallest, distance_to_tile_wrap_x(point_x, point_y, corner_x, corner_y, tile_size));
        smallest = std_min(
            smallest,
            distance_to_tile_wrap_x(point_x, point_y, corner_x + 0.5, -corner_y - tile_size, tile_size),
        );
        smallest = std_min(
            smallest,
            distance_to_tile_wrap_x(point_x, point_y, corner_x + 0.5, 2.0 - corner_y - tile_size, tile_size),
        );
        smallest
    }

    /// The wrap that keeps a tile loaded while the view crosses the
    /// antimeridian.
    fn wrap_for(center_x: f64, tile: &CanonicalTileID) -> i16 {
        let scale = f64::from(1u32 << tile.z);
        let tile_size = 1.0 / scale;
        let tile_x = f64::from(tile.x) / scale;
        let current = distance_to_tile_simple(center_x, tile_x, tile_size);
        let left = distance_to_tile_simple(center_x, tile_x - 1.0, tile_size);
        let right = distance_to_tile_simple(center_x, tile_x + 1.0, tile_size);
        // std::min({current, left, right}): the first of the smallest.
        let smallest = std_min(std_min(current, left), right);
        if smallest == right {
            return 1;
        }
        if smallest == left {
            return -1;
        }
        0
    }

    pub(super) fn tile_cover(
        state: &TileCoverParameters,
        z: u8,
        zoom_range: Range<u8>,
        overscaled_z: Option<u8>,
    ) -> Vec<OverscaledTileID> {
        #[derive(Clone, Copy, Debug)]
        struct Node {
            zoom: u8,
            x: u32,
            y: u32,
            wrap: i16,
            fully_visible: bool,
        }

        #[derive(Clone, Copy, Debug)]
        struct ResultTile {
            id: OverscaledTileID,
            sqr_dist: f64,
        }

        let transform = &state.transform_state;
        let projection = transform.get_projection_data(&UnwrappedTileID::new(0, 0, 0));
        let mut inverse: Mat4 = [0.0; 16];
        // `invert` reports a singular matrix with `true`.
        if matrix::invert(&mut inverse, &projection.main_matrix) {
            return Vec::new();
        }
        // The globe matrix has no Mercator y flip, which reverses the winding
        // the plane normals come from. The far plane stops at the horizon, so
        // the frustum's corners bound the visible cap and not the space behind
        // the planet.
        let horizon = projection.clipping_plane;
        let frustum = Frustum::from_inv_proj_matrix(&inverse, 1.0, 0.0, /*flipped_y*/ true, Some(&horizon));

        // Tiles nearer the camera than the center may load finer than the
        // nominal zoom, up to the source's maximum.
        let max_zoom = zoom_range.max;
        let min_zoom = zoom_range.min;
        let overscaled_zoom = std::cmp::max(overscaled_z.unwrap_or(z), max_zoom);
        let num_tiles = 2.0f64.powf(f64::from(z));
        let transform_zoom = transform.get_zoom();
        let requested_center_zoom =
            state.requested_zoom.unwrap_or(transform_zoom + (f64::from(z) - transform_zoom.floor()));
        let whole_zoom = |zoom: f64| if state.round_zoom { zoom.round() } else { zoom.floor() };
        let allow_variable_zoom = whole_zoom(requested_center_zoom) > 4.0;

        let center = Projection::project(
            &transform.get_lat_lng(mln_util::geo::WrapMode::Unwrapped),
            1.0 / constants::TILE_SIZE_D,
        );
        let center_coord: Vec3 = [center.x, center.y, 0.0];
        // The globe keeps its center on whichever world copy it has reached,
        // where GL JS keeps it on the first. Wraps, sort keys and tile
        // distances are taken against the center brought back to the first
        // copy, as GL JS takes them, and the wraps moved into the center's
        // copy.
        let center_world = center_coord[0].floor();
        let first_copy_center_x = center_coord[0] - center_world;
        let camera_position = transform.get_free_camera_options().position;
        debug_assert!(camera_position.is_some());
        let Some(camera_position) = camera_position else {
            return Vec::new();
        };
        // Past the Mercator range there are no tiles, so as GL JS does, they
        // are chosen around the center brought back to the edge of the range,
        // and the camera moves with it.
        let mut camera_coord = camera_position;
        let latitude = transform.get_lat_lng(mln_util::geo::WrapMode::Unwrapped).latitude();
        if latitude.abs() > constants::LATITUDE_MAX {
            camera_coord[1] += center.y - (0.5 - mln_math::asinh(mln_math::deg2rad(latitude).tan()) / constants::M2PI);
        }
        let distance_to_center_2d = (center_coord[0] - camera_coord[0]).hypot(center_coord[1] - camera_coord[1]);
        let distance_z = (center_coord[2] - camera_coord[2]).abs();
        let distance_to_center_3d = distance_to_center_2d.hypot(distance_z);
        let fov_degrees = f64::from(transform.get_field_of_view()) * 180.0 / PI;
        let tile_zoom = TileZoomFunction::new(
            fov_degrees,
            TileZoomFunction::DEFAULT_MAX_ZOOM_LEVELS_ON_SCREEN,
            TileZoomFunction::DEFAULT_TILE_COUNT_MAX_MIN_RATIO,
        );
        let culling_elevation = elevation_for_tile_culling(transform.get_pitch() * 180.0 / PI, fov_degrees, 0.0, 0.0);

        let mut stack: Vec<Node> = Vec::with_capacity(64);
        let mut result: Vec<ResultTile> = Vec::with_capacity(64);
        stack.push(Node { zoom: 0, x: 0, y: 0, wrap: 0, fully_visible: false });

        while let Some(mut node) = stack.pop() {
            let tile = CanonicalTileID::new(node.zoom, node.x, node.y);
            let volume = tile_bounding_volume(&tile, culling_elevation);

            if !node.fully_visible {
                let intersection = is_tile_visible(&frustum, &volume, &horizon);
                if intersection == IntersectionResult::Separate {
                    continue;
                }
                node.fully_visible = intersection == IntersectionResult::Contains;
            }

            let mut desired_z = f64::from(z);
            if allow_variable_zoom {
                let dist_to_tile_2d = distance_to_tile_2d(camera_coord[0] - center_world, camera_coord[1], &tile);
                desired_z = whole_zoom(tile_zoom.call(
                    requested_center_zoom,
                    dist_to_tile_2d,
                    distance_z,
                    distance_to_center_3d,
                ));
            }
            let target_zoom = clamp(desired_z, 0.0, f64::from(max_zoom)) as u8;

            if node.zoom >= target_zoom {
                if node.zoom < min_zoom {
                    continue;
                }
                node.wrap = (f64::from(wrap_for(first_copy_center_x, &tile)) + center_world) as i16;
                // GL JS's sort key: the tile's own x/y against the center at
                // the nominal zoom.
                let dx = num_tiles * first_copy_center_x - 0.5 - f64::from(node.x);
                let dy = num_tiles * center_coord[1] - 0.5 - f64::from(node.y);
                result.push(ResultTile {
                    id: OverscaledTileID::with_zxy(
                        if node.zoom == max_zoom { overscaled_zoom } else { node.zoom },
                        node.wrap,
                        node.zoom,
                        node.x,
                        node.y,
                    ),
                    sqr_dist: dx * dx + dy * dy,
                });
                continue;
            }
            for i in 0..4u32 {
                stack.push(Node {
                    zoom: node.zoom + 1,
                    x: (node.x << 1) + (i % 2),
                    y: (node.y << 1) + (i >> 1),
                    wrap: node.wrap,
                    fully_visible: node.fully_visible,
                });
            }
        }

        // GL JS's sort is stable: equally distant tiles, as around a pole,
        // keep the order they were found in.
        result.sort_by(|a, b| {
            if a.sqr_dist < b.sqr_dist {
                Ordering::Less
            } else if b.sqr_dist < a.sqr_dist {
                Ordering::Greater
            } else {
                Ordering::Equal
            }
        });
        result.iter().map(|tile| tile.id).collect()
    }
}

/// `tileCover(const TileCoverParameters&, uint8_t z, Range<uint8_t>
/// zoomRange, optional<uint8_t> overscaledZ = nullopt)`.
pub fn tile_cover(
    state: &TileCoverParameters,
    z: u8,
    zoom_range: Range<u8>,
    overscaled_z: Option<u8>,
) -> Vec<OverscaledTileID> {
    #[derive(Clone, Copy, Debug)]
    struct Node {
        aabb: AABB,
        zoom: u8,
        x: u32,
        y: u32,
        wrap: i16,
        fully_visible: bool,
    }

    #[derive(Clone, Copy, Debug)]
    struct ResultTile {
        id: OverscaledTileID,
        sqr_dist: f64,
    }

    if state.transform_state.is_globe_rendering() {
        return globe::tile_cover(state, z, zoom_range, overscaled_z);
    }

    let children_of = |node: &Node| -> Vec<Node> {
        let mut children = vec![*node; 4];
        for i in 0..4u32 {
            // uint32_t arithmetic.
            let child_x = (node.x << 1).wrapping_add(i % 2);
            let child_y = (node.y << 1).wrapping_add(i >> 1);

            let child = &mut children[i as usize];
            *child = *node;
            child.aabb = node.aabb.quadrant(i as i32);
            child.zoom = node.zoom.wrapping_add(1);
            child.x = child_x;
            child.y = child_y;
        }
        children
    };

    let transform = &state.transform_state;
    let num_tiles = 2.0f64.powf(f64::from(z));
    let world_size = Projection::world_size(transform.get_scale());
    let allow_variable_zoom = transform.get_pitch() > state.tile_lod_pitch_threshold;
    let min_zoom: u8 = if allow_variable_zoom { zoom_range.min } else { z };
    let max_zoom: u8 =
        if (state.tile_lod_mode == TileLodMode::Distance) && allow_variable_zoom { zoom_range.max } else { z };
    let overscaled_zoom: u8 = std::cmp::max(overscaled_z.unwrap_or(z), max_zoom);
    let flipped_y = transform.get_viewport_mode() == ViewportMode::FlippedY;

    let size = transform.get_size();
    let center_point = TileCoordinate::from_screen_coordinate(
        transform,
        z,
        &ScreenCoordinate::new(f64::from(size.width) / 2.0, f64::from(size.height) / 2.0),
    )
    .p;

    let center_coord: Vec3 = [center_point.x, center_point.y, 0.0];

    let camera_position = transform.get_free_camera_options().position;
    debug_assert!(camera_position.is_some());
    let Some(camera_position_mercator) = camera_position else {
        return Vec::new();
    };
    let nominal_scale = 2.0f64.powf(f64::from(z));
    let camera_coord = vec3_scale(&camera_position_mercator, nominal_scale);
    let camera_to_center_distance_mercator = vec3_length(&vec3_sub(&camera_coord, &center_coord)) / world_size;

    let frustum = Frustum::from_inv_proj_matrix(
        &transform.get_inv_projection_matrix(),
        world_size,
        f64::from(z),
        flipped_y,
        None,
    );

    // There should always be a certain number of maximum zoom level tiles
    // surrounding the center location
    debug_assert!(state.tile_lod_min_radius >= 1.0);
    let radius_of_max_lvl_lod_in_tiles = std_max(1.0, state.tile_lod_min_radius);

    let new_root_tile = |wrap: i16| -> Node {
        Node {
            aabb: AABB::new(
                [f64::from(wrap) * num_tiles, 0.0, 0.0],
                [f64::from(i32::from(wrap) + 1) * num_tiles, num_tiles, 0.0],
            ),
            zoom: 0,
            x: 0,
            y: 0,
            wrap,
            fully_visible: false,
        }
    };

    // Perform depth-first traversal on tile tree to find visible tiles
    let mut stack: Vec<Node> = Vec::new();
    let mut result: Vec<ResultTile> = Vec::new();
    stack.reserve(128);

    // World copies shall be rendered three times on both sides from closest to farthest
    for i in 1..=3i16 {
        stack.push(new_root_tile(-i));
        stack.push(new_root_tile(i));
    }

    stack.push(new_root_tile(0));

    while let Some(mut node) = stack.pop() {
        // Use cached visibility information of ancestor nodes
        if !node.fully_visible {
            let intersection = frustum.intersects(&node.aabb);

            if intersection == IntersectionResult::Separate {
                continue;
            }

            node.fully_visible = intersection == IntersectionResult::Contains;
        }

        let should_split_tile = if state.tile_lod_mode == TileLodMode::Distance {
            let cam_to_tile_mercator = vec3_scale(&node.aabb.distance_xyz(&camera_coord), 1.0 / world_size);
            let distance_to_tile_mercator = vec3_length(&cam_to_tile_mercator);
            let cos_pitch_to_tile = std_max(0.0, cam_to_tile_mercator[2] / distance_to_tile_mercator);
            let pitch_exponent = 0.5; // 0: constant screen width, 1/2: constant screen area, 1: constant screen height
            let tile_scale = 2.0f64.powf(f64::from(node.zoom));
            distance_to_tile_mercator * tile_scale
                < cos_pitch_to_tile.powf(pitch_exponent) * camera_to_center_distance_mercator / state.tile_lod_scale
                    * nominal_scale
        } else {
            let distance_xyz = node.aabb.distance_xyz(&center_coord);
            // `std::max_element`: the first largest element.
            let mut longest_dim = 0;
            for i in 1..distance_xyz.len() {
                if distance_xyz[longest_dim] < distance_xyz[i] {
                    longest_dim = i;
                }
            }

            // We're using distance based heuristics to determine if a tile should
            // be split into quadrants or not. radiusOfMaxLvlLodInTiles defines that
            // there's always a certain number of maxLevel tiles next to the map
            // center. Using the fact that a parent node in quadtree is twice the
            // size of its children (per dimension) we can define distance
            // thresholds for each relative level:
            // f(k) = offset + 2 + 4 + 8 + 16 + ... + 2^k
            // This is the same as:
            // f(k) = offset + 2^(k+1)-2
            let dist_to_split = radius_of_max_lvl_lod_in_tiles
                + f64::from(1i32.wrapping_shl((i32::from(max_zoom) - i32::from(node.zoom)) as u32))
                - 2.0;
            distance_xyz[longest_dim] * state.tile_lod_scale < dist_to_split
        };

        // Have we reached the target depth or is the tile too far away to be any split further?
        if node.zoom == max_zoom || (!should_split_tile && node.zoom >= min_zoom) {
            // Perform precise intersection test between the frustum and aabb.
            // This will cull < 1% false positives missed by the original test
            if node.fully_visible || frustum.intersects_precise(&node.aabb, true) != IntersectionResult::Separate {
                let id = OverscaledTileID::with_zxy(
                    if node.zoom == max_zoom { overscaled_zoom } else { node.zoom },
                    node.wrap,
                    node.zoom,
                    node.x,
                    node.y,
                );
                let coord_to_load_first: Vec3 =
                    if state.tile_lod_mode == TileLodMode::Distance { camera_coord } else { center_coord };
                let dx = f64::from(node.wrap) * num_tiles + f64::from(node.x) + 0.5 - coord_to_load_first[0];
                let dy = f64::from(node.y) + 0.5 - coord_to_load_first[1];

                result.push(ResultTile { id, sqr_dist: dx * dx + dy * dy });
            }
        } else {
            let children = children_of(&node);
            stack.extend(children);
        }
    }

    // Sort results by distance
    std_sort(&mut result, |a, b| a.sqr_dist < b.sqr_dist);

    let mut ids = Vec::with_capacity(result.len());

    for tile in &result {
        ids.push(tile.id);
    }

    ids
}

/// `tileCover(const LatLngBounds&, uint8_t z)`.
#[must_use]
pub fn tile_cover_bounds(bounds_: &LatLngBounds, z: u8) -> Vec<UnwrappedTileID> {
    if bounds_.is_empty() || bounds_.south() > constants::LATITUDE_MAX || bounds_.north() < -constants::LATITUDE_MAX {
        return Vec::new();
    }

    let bounds = LatLngBounds::hull(
        bounds_lat_lng(std_max(bounds_.south(), -constants::LATITUDE_MAX), bounds_.west()),
        bounds_lat_lng(std_min(bounds_.north(), constants::LATITUDE_MAX), bounds_.east()),
    );

    let zoom = i32::from(z);
    tile_cover_points(
        &Projection::project_zoom(&bounds.northwest(), zoom),
        &Projection::project_zoom(&bounds.northeast(), zoom),
        &Projection::project_zoom(&bounds.southeast(), zoom),
        &Projection::project_zoom(&bounds.southwest(), zoom),
        &Projection::project_zoom(&bounds.center(), zoom),
        z,
    )
}

/// `tileCover(const Geometry<double>&, uint8_t z)`.
pub fn tile_cover_geometry(geometry: &Geometry<f64>, z: u8) -> Result<Vec<UnwrappedTileID>, Error> {
    let mut result = Vec::new();
    let mut tc = TileCover::new_from_geometry(geometry, z, true)?;
    while tc.has_next() {
        result.push(tc.next().expect("has_next"));
    }

    Ok(result)
}

/// Compute only the count of tiles needed for tileCover.
///
/// Taken from <https://github.com/mapbox/sphericalmercator#xyzbbox-zoom-tms_style-srs>
/// Computes the projected tiles for the lower left and upper right points of the bounds
/// and uses that to compute the tile cover count
#[must_use]
pub fn tile_count_bounds(bounds: &LatLngBounds, zoom: u8) -> u64 {
    if zoom == 0 {
        return 1;
    }
    let sw = Projection::project_zoom(&bounds.southwest(), i32::from(zoom));
    let ne = Projection::project_zoom(&bounds.northeast(), i32::from(zoom));
    let max_tile = 2.0f64.powf(f64::from(zoom));
    let x1 = sw.x.floor();
    let x2 = ne.x.ceil() - 1.0;
    let y1 = mln_math::clamp_f64(sw.y.floor(), 0.0, max_tile - 1.0);
    let y2 = mln_math::clamp_f64(ne.y.floor(), 0.0, max_tile - 1.0);

    let dx = if x1 > x2 { (max_tile - x1) + x2 } else { x2 - x1 };
    let dy = y1 - y2;
    // `static_cast<uint64_t>`: UB for negative or too large values in C++;
    // saturates here.
    ((dx + 1.0) * (dy + 1.0)) as u64
}

/// `tileCount(const Geometry<double>&, uint8_t z)`.
pub fn tile_count_geometry(geometry: &Geometry<f64>, z: u8) -> Result<u64, Error> {
    let mut tile_count: u64 = 0;

    let mut tc = TileCover::new_from_geometry(geometry, z, true)?;
    while tc.next().is_some() {
        tile_count += 1;
    }
    Ok(tile_count)
}

// --- libstdc++ `std::sort` (bits/stl_algo.h, bits/stl_heap.h) ---
//
// `std::sort` is not stable: for elements with equal keys (equal `sqrDist`
// in `tile_cover`, for example) the result order is whatever introsort
// produces, so the port runs the same algorithm (`__introsort_loop` with
// median-of-three pivots, heap sort past the depth limit, then
// `__final_insertion_sort` with `_S_threshold = 16`). Iterators are indices
// into the whole slice. Where libstdc++'s unguarded loops would leave the
// range (only possible when `comp` is not a strict weak ordering, e.g. with
// NaN keys; UB in C++), indexing panics here.
//
// TODO(port): move to a shared module (mln_util) once other ports need it.

const S_THRESHOLD: usize = 16;

/// `std::sort(v.begin(), v.end(), comp)` as libstdc++ implements it.
pub(crate) fn std_sort<T: Copy, F: FnMut(&T, &T) -> bool>(v: &mut [T], mut comp: F) {
    let n = v.len();
    if n != 0 {
        // `std::__lg(n) * 2`
        let lg = (usize::BITS - 1 - n.leading_zeros()) as usize;
        introsort_loop(v, 0, n, lg * 2, &mut comp);
        final_insertion_sort(v, 0, n, &mut comp);
    }
}

fn introsort_loop<T: Copy, F: FnMut(&T, &T) -> bool>(
    v: &mut [T],
    first: usize,
    mut last: usize,
    mut depth_limit: usize,
    comp: &mut F,
) {
    while last - first > S_THRESHOLD {
        if depth_limit == 0 {
            // `std::__partial_sort(first, last, last, comp)`: `__heap_select`
            // with `middle == last` is just `__make_heap`.
            make_heap(v, first, last, comp);
            sort_heap(v, first, last, comp);
            return;
        }
        depth_limit -= 1;
        let cut = unguarded_partition_pivot(v, first, last, comp);
        introsort_loop(v, cut, last, depth_limit, comp);
        last = cut;
    }
}

fn move_median_to_first<T: Copy, F: FnMut(&T, &T) -> bool>(
    v: &mut [T],
    result: usize,
    a: usize,
    b: usize,
    c: usize,
    comp: &mut F,
) {
    if comp(&v[a], &v[b]) {
        if comp(&v[b], &v[c]) {
            v.swap(result, b);
        } else if comp(&v[a], &v[c]) {
            v.swap(result, c);
        } else {
            v.swap(result, a);
        }
    } else if comp(&v[a], &v[c]) {
        v.swap(result, a);
    } else if comp(&v[b], &v[c]) {
        v.swap(result, c);
    } else {
        v.swap(result, b);
    }
}

fn unguarded_partition<T: Copy, F: FnMut(&T, &T) -> bool>(
    v: &mut [T],
    mut first: usize,
    mut last: usize,
    pivot: usize,
    comp: &mut F,
) -> usize {
    loop {
        while comp(&v[first], &v[pivot]) {
            first += 1;
        }
        last -= 1;
        while comp(&v[pivot], &v[last]) {
            last -= 1;
        }
        if first >= last {
            return first;
        }
        v.swap(first, last);
        first += 1;
    }
}

fn unguarded_partition_pivot<T: Copy, F: FnMut(&T, &T) -> bool>(
    v: &mut [T],
    first: usize,
    last: usize,
    comp: &mut F,
) -> usize {
    let mid = first + (last - first) / 2;
    move_median_to_first(v, first, first + 1, mid, last - 1, comp);
    unguarded_partition(v, first + 1, last, first, comp)
}

fn unguarded_linear_insert<T: Copy, F: FnMut(&T, &T) -> bool>(v: &mut [T], mut last: usize, comp: &mut F) {
    let val = v[last];
    let mut next = last - 1;
    while comp(&val, &v[next]) {
        v[last] = v[next];
        last = next;
        next -= 1;
    }
    v[last] = val;
}

fn insertion_sort<T: Copy, F: FnMut(&T, &T) -> bool>(v: &mut [T], first: usize, last: usize, comp: &mut F) {
    if first == last {
        return;
    }

    for i in first + 1..last {
        if comp(&v[i], &v[first]) {
            let val = v[i];
            // `std::move_backward(first, i, i + 1)`
            v.copy_within(first..i, first + 1);
            v[first] = val;
        } else {
            unguarded_linear_insert(v, i, comp);
        }
    }
}

fn unguarded_insertion_sort<T: Copy, F: FnMut(&T, &T) -> bool>(v: &mut [T], first: usize, last: usize, comp: &mut F) {
    for i in first..last {
        unguarded_linear_insert(v, i, comp);
    }
}

fn final_insertion_sort<T: Copy, F: FnMut(&T, &T) -> bool>(v: &mut [T], first: usize, last: usize, comp: &mut F) {
    if last - first > S_THRESHOLD {
        insertion_sort(v, first, first + S_THRESHOLD, comp);
        unguarded_insertion_sort(v, first + S_THRESHOLD, last, comp);
    } else {
        insertion_sort(v, first, last, comp);
    }
}

// Heap distances are `ptrdiff_t` in C++; `(hole_index - 1) / 2` relies on
// signed division truncating -1 / 2 to 0.

fn push_heap<T: Copy, F: FnMut(&T, &T) -> bool>(
    v: &mut [T],
    first: usize,
    mut hole_index: isize,
    top_index: isize,
    value: T,
    comp: &mut F,
) {
    let at = |i: isize| first + i as usize;
    let mut parent = (hole_index - 1) / 2;
    while hole_index > top_index && comp(&v[at(parent)], &value) {
        v[at(hole_index)] = v[at(parent)];
        hole_index = parent;
        parent = (hole_index - 1) / 2;
    }
    v[at(hole_index)] = value;
}

fn adjust_heap<T: Copy, F: FnMut(&T, &T) -> bool>(
    v: &mut [T],
    first: usize,
    mut hole_index: isize,
    len: isize,
    value: T,
    comp: &mut F,
) {
    let at = |i: isize| first + i as usize;
    let top_index = hole_index;
    let mut second_child = hole_index;
    while second_child < (len - 1) / 2 {
        second_child = 2 * (second_child + 1);
        if comp(&v[at(second_child)], &v[at(second_child - 1)]) {
            second_child -= 1;
        }
        v[at(hole_index)] = v[at(second_child)];
        hole_index = second_child;
    }
    if (len & 1) == 0 && second_child == (len - 2) / 2 {
        second_child = 2 * (second_child + 1);
        v[at(hole_index)] = v[at(second_child - 1)];
        hole_index = second_child - 1;
    }
    push_heap(v, first, hole_index, top_index, value, comp);
}

fn make_heap<T: Copy, F: FnMut(&T, &T) -> bool>(v: &mut [T], first: usize, last: usize, comp: &mut F) {
    if last - first < 2 {
        return;
    }

    let len = (last - first) as isize;
    let mut parent = (len - 2) / 2;
    loop {
        let value = v[first + parent as usize];
        adjust_heap(v, first, parent, len, value, comp);
        if parent == 0 {
            return;
        }
        parent -= 1;
    }
}

fn pop_heap<T: Copy, F: FnMut(&T, &T) -> bool>(v: &mut [T], first: usize, last: usize, result: usize, comp: &mut F) {
    let value = v[result];
    v[result] = v[first];
    adjust_heap(v, first, 0, (last - first) as isize, value, comp);
}

fn sort_heap<T: Copy, F: FnMut(&T, &T) -> bool>(v: &mut [T], first: usize, mut last: usize, comp: &mut F) {
    while last - first > 1 {
        last -= 1;
        pop_heap(v, first, last, last, comp);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone, Copy)]
    struct E {
        key: u32,
        idx: u64,
    }

    // The reference values below come from libstdc++ (GCC 15.3, -std=c++20):
    // the same LCG fills `n` elements with keys in `0..keys`, `std::sort` /
    // `std::partial_sort(begin + off, end, end)` sorts them by key only, and
    // the FNV-1a hash of the resulting index order is printed.
    fn make(n: u32, keys: u32) -> Vec<E> {
        let mut state = n.wrapping_mul(31).wrapping_add(keys);
        (0..u64::from(n))
            .map(|idx| {
                state = state.wrapping_mul(1_103_515_245).wrapping_add(12345);
                E { key: ((state >> 16) & 0x7fff) % keys, idx }
            })
            .collect()
    }

    fn hash(v: &[E]) -> u64 {
        let mut h: u64 = 1_469_598_103_934_665_603;
        for e in v {
            h ^= e.idx;
            h = h.wrapping_mul(1_099_511_628_211);
        }
        h
    }

    const SORT: &[(u32, u32, u64)] = &[
        (0, 3, 1_469_598_103_934_665_603),
        (1, 3, 4_953_163_356_653_287_321),
        (2, 1000, 11_125_488_673_309_971_922),
        (5, 1, 16_553_567_279_584_964_729),
        (5, 3, 14_394_188_932_140_642_875),
        (5, 1000, 12_850_371_070_075_698_435),
        (16, 1, 615_731_795_747_517_251),
        (16, 3, 14_598_784_300_330_927_239),
        (16, 1000, 13_067_083_390_130_113_883),
        (17, 1, 12_142_191_458_066_772_073),
        (17, 3, 13_853_817_294_641_342_161),
        (17, 1000, 563_012_451_418_373_431),
        (18, 1, 5_036_946_713_153_761_986),
        (18, 3, 9_359_206_580_369_302_924),
        (18, 1000, 3_489_360_189_878_631_466),
        (33, 1, 4_342_150_641_277_792_007),
        (33, 3, 16_809_619_996_606_302_123),
        (33, 1000, 17_786_421_032_727_766_049),
        (100, 1, 1_494_447_923_934_049_643),
        (100, 3, 14_657_547_681_949_098_889),
        (100, 1000, 5_015_142_636_931_887_253),
        (257, 1, 9_451_109_056_036_632_721),
        (257, 3, 12_779_444_720_448_154_427),
        (257, 1000, 11_065_013_591_796_304_113),
        (1000, 1, 12_134_223_775_326_807_227),
        (1000, 3, 12_493_799_056_634_839_375),
        (1000, 1000, 3_068_094_519_739_835_583),
        (5000, 1, 7_884_124_110_981_860_555),
        (5000, 3, 10_004_906_514_490_205_587),
        (5000, 1000, 14_874_049_691_706_547_159),
    ];

    const HEAP: &[(u32, u32, u64)] = &[
        (5, 1, 15_486_957_069_233_966_805),
        (5, 3, 14_394_188_932_140_642_875),
        (16, 1, 7_474_976_027_390_535_375),
        (16, 3, 12_283_545_573_958_081_619),
        (16, 1000, 16_629_833_858_576_844_435),
        (17, 1, 6_653_128_762_809_732_651),
        (17, 3, 9_499_825_458_292_309_071),
        (17, 1000, 14_243_645_748_220_843_191),
        (18, 1, 6_369_017_254_872_719_408),
        (18, 3, 11_697_980_667_836_372_002),
        (18, 1000, 9_138_633_810_072_843_698),
        (33, 1, 10_763_085_765_877_827_387),
        (33, 3, 10_625_529_904_606_193_397),
        (33, 1000, 10_570_816_534_142_537_483),
        (100, 1, 17_524_909_542_049_215_755),
        (100, 3, 1_648_911_209_219_688_905),
        (100, 1000, 8_297_980_626_135_366_727),
        (257, 1, 17_857_370_511_256_261_451),
        (257, 3, 14_779_427_894_545_510_531),
        (257, 1000, 15_786_095_388_650_306_839),
    ];

    #[test]
    fn std_sort_matches_libstdcxx() {
        for &(n, keys, expected) in SORT {
            let mut v = make(n, keys);
            std_sort(&mut v, |a, b| a.key < b.key);
            assert!(v.windows(2).all(|w| w[0].key <= w[1].key), "n={n} keys={keys}");
            assert_eq!(hash(&v), expected, "n={n} keys={keys}");
        }
    }

    #[test]
    fn std_partial_sort_matches_libstdcxx() {
        for &(n, keys, expected) in HEAP {
            let mut v = make(n, keys);
            let off = if n > 5 { 5 } else { 0 };
            let len = v.len();
            let mut comp = |a: &E, b: &E| a.key < b.key;
            make_heap(&mut v, off, len, &mut comp);
            sort_heap(&mut v, off, len, &mut comp);
            assert!(v[off..].windows(2).all(|w| w[0].key <= w[1].key), "n={n} keys={keys}");
            assert_eq!(hash(&v), expected, "n={n} keys={keys}");
        }
    }

    /// `tileCount(Geometry)` streams the same `TileCover` as
    /// `tileCover(Geometry)` (the line of `TileCover.GeomLineZ10`).
    #[test]
    fn tile_count_geometry_counts_the_cover() {
        let line = vec![
            Point::new(-121.493_682_861_328_12, 38.579_037_146_674_59),
            Point::new(-122.442_283_630_371_1, 37.773_157_169_570_695),
        ];
        let geom = Geometry::LineString(line);
        assert_eq!(7, tile_count_geometry(&geom, 10).unwrap());
        assert_eq!(7, tile_cover_geometry(&geom, 10).unwrap().len());
        assert!(tile_count_geometry(&Geometry::Point(Point::new(0.0, 97.0)), 2).is_err());
    }
}
