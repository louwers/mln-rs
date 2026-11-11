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
/// `LatLngBounds` (with the latitude clamped to ±LATITUDE_MAX), so the
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
    let y0 = fmax(ymin as f64, e1.y0.floor());
    let y1 = fmin(ymax as f64, e1.y1.ceil());

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
    // `1 << z` (an int shift).
    let tiles: i32 = 1i32.wrapping_shl(z as u32);

    #[derive(Clone, Copy, Debug)]
    struct ID {
        x: i32,
        y: i32,
        sq_dist: f64,
    }

    // skip the first few allocations, assuming we usually end up with at least a few tiles
    let mut t: Vec<ID> = Vec::with_capacity(8);

    {
        let mut scan_line = |x0: i32, x1: i32, y: i32| {
            // Quirk kept: `y <= tiles` lets the row below the world through;
            // the UnwrappedTileID constructor clamps it.
            if y >= 0 && y <= tiles {
                let mut x = x0;
                while x < x1 {
                    let dx = x as f64 + 0.5 - c.x;
                    let dy = y as f64 + 0.5 - c.y;
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
        result.push(UnwrappedTileID::new(z, id.x as i64, id.y as i64));
    }
    result
}

/// Helper class to stream tile-cover results per row
pub struct TileCover {
    impl_: Box<Impl>,
}

impl TileCover {
    /// `TileCover(const LatLngBounds&, uint8_t z)`.
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
        let sw = Projection::project_zoom(&bounds.southwest(), z as i32);
        let ne = Projection::project_zoom(&bounds.northeast(), z as i32);
        let se = Projection::project_zoom(&bounds.southeast(), z as i32);
        let nw = Projection::project_zoom(&bounds.northwest(), z as i32);

        let p: Polygon<f64> = vec![vec![sw, nw, ne, se, sw]];
        // Nothing is projected (`project = false`), so nothing can throw.
        TileCover {
            impl_: Box::new(Impl::new(z as i32, &Geometry::Polygon(p), false).expect("unprojected tile cover")),
        }
    }

    /// `TileCover(const Geometry<double>&, uint8_t z, bool project = true)`.
    /// When project == true, projects the geometry points to tile
    /// coordinates, and fails with
    /// [`ErrorKind::InvalidArgument`](mln_util::ErrorKind) for a point that
    /// isn't a valid `LatLng`.
    pub fn new_from_geometry(geom: &Geometry<f64>, z: u8, project: bool) -> Result<TileCover, Error> {
        Ok(TileCover { impl_: Box::new(Impl::new(z as i32, geom, project)?) })
    }

    pub fn next(&mut self) -> Option<UnwrappedTileID> {
        self.impl_.next()
    }

    pub fn has_next(&self) -> bool {
        self.impl_.has_next()
    }
}

#[derive(Clone, Debug)]
pub struct TileCoverParameters {
    pub transform_state: TransformState,
    pub tile_lod_min_radius: f64,
    pub tile_lod_scale: f64,
    pub tile_lod_pitch_threshold: f64,
    pub tile_lod_mode: TileLodMode,
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
        }
    }
}

pub fn covering_zoom_level(zoom: f64, type_: SourceType, size: u16) -> i32 {
    let mut zoom = zoom;
    zoom += mln_math::log2(constants::TILE_SIZE_D / size as f64);
    if type_ == SourceType::Raster || type_ == SourceType::RasterDEM || type_ == SourceType::Video {
        zoom.round() as i32
    } else {
        zoom.floor() as i32
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
    let num_tiles = 2.0f64.powf(z as f64);
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
        &ScreenCoordinate::new(size.width as f64 / 2.0, size.height as f64 / 2.0),
    )
    .p;

    let center_coord: Vec3 = [center_point.x, center_point.y, 0.0];

    // C++ asserts that the position is set, then dereferences it.
    let camera_position_mercator: Vec3 = transform.get_free_camera_options().position.expect("free camera position");
    let nominal_scale = 2.0f64.powf(z as f64);
    let camera_coord = vec3_scale(&camera_position_mercator, nominal_scale);
    let camera_to_center_distance_mercator = vec3_length(&vec3_sub(&camera_coord, &center_coord)) / world_size;

    let frustum =
        Frustum::from_inv_proj_matrix(&transform.get_inv_projection_matrix(), world_size, z as f64, flipped_y);

    // There should always be a certain number of maximum zoom level tiles
    // surrounding the center location
    debug_assert!(state.tile_lod_min_radius >= 1.0);
    let radius_of_max_lvl_lod_in_tiles = std_max(1.0, state.tile_lod_min_radius);

    let new_root_tile = |wrap: i16| -> Node {
        Node {
            aabb: AABB::new(
                [wrap as f64 * num_tiles, 0.0, 0.0],
                [(wrap as i32 + 1) as f64 * num_tiles, num_tiles, 0.0],
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

        let should_split_tile;
        if state.tile_lod_mode == TileLodMode::Distance {
            let cam_to_tile_mercator = vec3_scale(&node.aabb.distance_xyz(&camera_coord), 1.0 / world_size);
            let distance_to_tile_mercator = vec3_length(&cam_to_tile_mercator);
            let cos_pitch_to_tile = std_max(0.0, cam_to_tile_mercator[2] / distance_to_tile_mercator);
            let pitch_exponent = 0.5; // 0: constant screen width, 1/2: constant screen area, 1: constant screen height
            let tile_scale = 2.0f64.powf(node.zoom as f64);
            should_split_tile = distance_to_tile_mercator * tile_scale
                < cos_pitch_to_tile.powf(pitch_exponent) * camera_to_center_distance_mercator / state.tile_lod_scale
                    * nominal_scale;
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
                + 1i32.wrapping_shl((max_zoom as i32 - node.zoom as i32) as u32) as f64
                - 2.0;
            should_split_tile = distance_xyz[longest_dim] * state.tile_lod_scale < dist_to_split;
        }

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
                let dx = node.wrap as f64 * num_tiles + node.x as f64 + 0.5 - coord_to_load_first[0];
                let dy = node.y as f64 + 0.5 - coord_to_load_first[1];

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
pub fn tile_cover_bounds(bounds_: &LatLngBounds, z: u8) -> Vec<UnwrappedTileID> {
    if bounds_.is_empty() || bounds_.south() > constants::LATITUDE_MAX || bounds_.north() < -constants::LATITUDE_MAX {
        return Vec::new();
    }

    let bounds = LatLngBounds::hull(
        bounds_lat_lng(std_max(bounds_.south(), -constants::LATITUDE_MAX), bounds_.west()),
        bounds_lat_lng(std_min(bounds_.north(), constants::LATITUDE_MAX), bounds_.east()),
    );

    let zoom = z as i32;
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
pub fn tile_count_bounds(bounds: &LatLngBounds, zoom: u8) -> u64 {
    if zoom == 0 {
        return 1;
    }
    let sw = Projection::project_zoom(&bounds.southwest(), zoom as i32);
    let ne = Projection::project_zoom(&bounds.northeast(), zoom as i32);
    let max_tile = 2.0f64.powf(zoom as f64);
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
        if !(first < last) {
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
        (0..n as u64)
            .map(|idx| {
                state = state.wrapping_mul(1103515245).wrapping_add(12345);
                E { key: ((state >> 16) & 0x7fff) % keys, idx }
            })
            .collect()
    }

    fn hash(v: &[E]) -> u64 {
        let mut h: u64 = 1469598103934665603;
        for e in v {
            h ^= e.idx;
            h = h.wrapping_mul(1099511628211);
        }
        h
    }

    const SORT: &[(u32, u32, u64)] = &[
        (0, 3, 1469598103934665603),
        (1, 3, 4953163356653287321),
        (2, 1000, 11125488673309971922),
        (5, 1, 16553567279584964729),
        (5, 3, 14394188932140642875),
        (5, 1000, 12850371070075698435),
        (16, 1, 615731795747517251),
        (16, 3, 14598784300330927239),
        (16, 1000, 13067083390130113883),
        (17, 1, 12142191458066772073),
        (17, 3, 13853817294641342161),
        (17, 1000, 563012451418373431),
        (18, 1, 5036946713153761986),
        (18, 3, 9359206580369302924),
        (18, 1000, 3489360189878631466),
        (33, 1, 4342150641277792007),
        (33, 3, 16809619996606302123),
        (33, 1000, 17786421032727766049),
        (100, 1, 1494447923934049643),
        (100, 3, 14657547681949098889),
        (100, 1000, 5015142636931887253),
        (257, 1, 9451109056036632721),
        (257, 3, 12779444720448154427),
        (257, 1000, 11065013591796304113),
        (1000, 1, 12134223775326807227),
        (1000, 3, 12493799056634839375),
        (1000, 1000, 3068094519739835583),
        (5000, 1, 7884124110981860555),
        (5000, 3, 10004906514490205587),
        (5000, 1000, 14874049691706547159),
    ];

    const HEAP: &[(u32, u32, u64)] = &[
        (5, 1, 15486957069233966805),
        (5, 3, 14394188932140642875),
        (16, 1, 7474976027390535375),
        (16, 3, 12283545573958081619),
        (16, 1000, 16629833858576844435),
        (17, 1, 6653128762809732651),
        (17, 3, 9499825458292309071),
        (17, 1000, 14243645748220843191),
        (18, 1, 6369017254872719408),
        (18, 3, 11697980667836372002),
        (18, 1000, 9138633810072843698),
        (33, 1, 10763085765877827387),
        (33, 3, 10625529904606193397),
        (33, 1000, 10570816534142537483),
        (100, 1, 17524909542049215755),
        (100, 3, 1648911209219688905),
        (100, 1000, 8297980626135366727),
        (257, 1, 17857370511256261451),
        (257, 3, 14779427894545510531),
        (257, 1000, 15786095388650306839),
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
            Point::new(-121.49368286132812, 38.57903714667459),
            Point::new(-122.4422836303711, 37.773157169570695),
        ];
        let geom = Geometry::LineString(line);
        assert_eq!(7, tile_count_geometry(&geom, 10).unwrap());
        assert_eq!(7, tile_cover_geometry(&geom, 10).unwrap().len());
        assert!(tile_count_geometry(&Geometry::Point(Point::new(0.0, 97.0)), 2).is_err());
    }
}
