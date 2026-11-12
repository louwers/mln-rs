// Ports vendor/wagyu/include/mapbox/geometry/wagyu/ring_util.hpp.

use crate::bound::{Bound, BoundPtr, EdgeSide};
use crate::edge::{Edge, is_horizontal};
use crate::point::{GPoint, NIL, PointPtr, RingPtr};
use crate::ring::{
    GBox, R, RingManager, create_new_point, create_new_point_before, create_new_ring, reverse_ring, ring_is_hole,
    ring1_replaces_ring2,
};
use crate::util::{cpp_max, cpp_min, greater_than_or_equal, llround, value_is_zero, values_are_equal};

/// `set_hole_state`.
fn set_hole_state(bnd: BoundPtr, active_bounds: &[BoundPtr], b: &[Bound], rings: &mut RingManager) {
    // std::find on the reverse range, then one step further left.
    let mut bnd_tmp: BoundPtr = NIL;
    if let Some(pos) = active_bounds.iter().rposition(|&x| x == bnd) {
        // Find first non line ring to the left of current bound.
        for i in (0..pos).rev() {
            let x = active_bounds[i];
            if x == NIL {
                continue;
            }
            if b[x].ring != NIL {
                if bnd_tmp == NIL {
                    bnd_tmp = x;
                } else if b[bnd_tmp].ring == b[x].ring {
                    bnd_tmp = NIL;
                }
            }
        }
    }
    let ring = b[bnd].ring;
    if bnd_tmp == NIL {
        rings.rings[ring].parent = NIL;
        rings.children.push(ring);
    } else {
        let parent = b[bnd_tmp].ring;
        rings.rings[ring].parent = parent;
        rings.rings[parent].children.push(ring);
    }
}

/// `update_current_hp_itr`.
pub(crate) fn update_current_hp_itr(scanline_y: i32, rings: &mut RingManager) {
    while rings.current_hp_itr < rings.hot_pixels.len() && rings.hot_pixels[rings.current_hp_itr].y > scanline_y {
        rings.current_hp_itr += 1;
    }
}

/// `round_towards_min`: 0.5 rounds to 0, -0.5 rounds to -1.
fn round_towards_min(val: f64) -> i32 {
    let half = val.floor() + 0.5;
    if values_are_equal(val, half) { val.floor() as i32 } else { llround(val) as i32 }
}

/// `round_towards_max`: 0.5 rounds to 1, -0.5 rounds to 0.
pub(crate) fn round_towards_max(val: f64) -> i32 {
    let half = val.floor() + 0.5;
    if values_are_equal(val, half) { val.ceil() as i32 } else { llround(val) as i32 }
}

/// `get_edge_min_x`.
fn get_edge_min_x(edge: &Edge, current_y: i32) -> i32 {
    if is_horizontal(edge) {
        if edge.bot.x < edge.top.x { edge.bot.x } else { edge.top.x }
    } else if edge.dx > 0.0 {
        if current_y == edge.top.y {
            edge.top.x
        } else {
            let lower_range_y = f64::from(current_y.wrapping_sub(edge.bot.y)) - 0.5;
            let return_val = f64::from(edge.bot.x) + edge.dx * lower_range_y;
            round_towards_min(return_val)
        }
    } else if current_y == edge.bot.y {
        edge.bot.x
    } else {
        let return_val = f64::from(edge.bot.x) + edge.dx * (f64::from(current_y.wrapping_sub(edge.bot.y)) + 0.5);
        round_towards_min(return_val)
    }
}

/// `get_edge_max_x`.
fn get_edge_max_x(edge: &Edge, current_y: i32) -> i32 {
    if is_horizontal(edge) {
        if edge.bot.x > edge.top.x { edge.bot.x } else { edge.top.x }
    } else if edge.dx < 0.0 {
        if current_y == edge.top.y {
            edge.top.x
        } else {
            let lower_range_y = f64::from(current_y.wrapping_sub(edge.bot.y)) - 0.5;
            let return_val = f64::from(edge.bot.x) + edge.dx * lower_range_y;
            round_towards_max(return_val)
        }
    } else if current_y == edge.bot.y {
        edge.bot.x
    } else {
        let return_val = f64::from(edge.bot.x) + edge.dx * (f64::from(current_y.wrapping_sub(edge.bot.y)) + 0.5);
        round_towards_max(return_val)
    }
}

/// The insertion step shared by `hot_pixel_set_left_to_right` and
/// `hot_pixel_set_right_to_left` (also the tail of `add_point_to_ring`).
fn insert_point(bnd: &Bound, hp: GPoint, rings: &mut RingManager) -> bool {
    let ring = bnd.ring;
    let op = rings.rings[ring].points;
    let to_front = bnd.side == EdgeSide::Left;
    if (to_front && hp.x == rings.pt(op).x && hp.y == rings.pt(op).y)
        || (!to_front && hp.x == rings.pt(rings.prev(op)).x && hp.y == rings.pt(rings.prev(op)).y)
    {
        return false;
    }
    let new_point = create_new_point_before(ring, hp, op, rings);
    if to_front {
        rings.rings[ring].points = new_point;
    }
    true
}

/// `hot_pixel_set_left_to_right` over `hot_pixels`[itr..end).
#[allow(clippy::too_many_arguments)]
fn hot_pixel_set_left_to_right(
    y: i32,
    start_x: i32,
    end_x: i32,
    bnd: &Bound,
    rings: &mut RingManager,
    itr: usize,
    end: usize,
    add_end_point: bool,
) {
    let x_min = get_edge_min_x(bnd.cur(), y).max(start_x);
    let x_max = get_edge_max_x(bnd.cur(), y).min(end_x);
    for i in itr..end {
        let hp = rings.hot_pixels[i];
        if hp.x < x_min {
            continue;
        }
        if hp.x > x_max {
            break;
        }
        if !add_end_point && hp.x == end_x {
            continue;
        }
        insert_point(bnd, hp, rings);
    }
}

/// `hot_pixel_set_right_to_left` over `hot_pixels`[first..last) backwards
/// (the reverse iterators of the C++).
#[allow(clippy::too_many_arguments)]
fn hot_pixel_set_right_to_left(
    y: i32,
    start_x: i32,
    end_x: i32,
    bnd: &Bound,
    rings: &mut RingManager,
    first: usize,
    last: usize,
    add_end_point: bool,
) {
    let x_min = get_edge_min_x(bnd.cur(), y).max(end_x);
    let x_max = get_edge_max_x(bnd.cur(), y).min(start_x);
    for i in (first..last).rev() {
        let hp = rings.hot_pixels[i];
        if hp.x > x_max {
            continue;
        }
        if hp.x < x_min {
            break;
        }
        if !add_end_point && hp.x == end_x {
            continue;
        }
        insert_point(bnd, hp, rings);
    }
}

/// `sort_hot_pixels`: y descending, then x ascending; duplicates removed.
pub(crate) fn sort_hot_pixels(rings: &mut RingManager) {
    rings.hot_pixels.sort_unstable_by(|a, b| b.y.cmp(&a.y).then(a.x.cmp(&b.x)));
    rings.hot_pixels.dedup();
}

/// `insert_hot_pixels_in_path`.
pub(crate) fn insert_hot_pixels_in_path(
    bnd: BoundPtr,
    end_pt: GPoint,
    b: &mut [Bound],
    rings: &mut RingManager,
    add_end_point: bool,
) {
    if end_pt == b[bnd].last_point {
        return;
    }

    let start_y = b[bnd].last_point.y;
    let start_x = b[bnd].last_point.x;
    let end_y = end_pt.y;
    let end_x = end_pt.x;

    let len = rings.hot_pixels.len();
    let mut itr = rings.current_hp_itr;
    while itr < len && rings.hot_pixels[itr].y <= start_y && itr != 0 {
        itr -= 1;
    }
    if start_x > end_x {
        while itr != len {
            if rings.hot_pixels[itr].y > start_y {
                itr += 1;
                continue;
            }
            if rings.hot_pixels[itr].y < end_y {
                break;
            }
            let y = rings.hot_pixels[itr].y;
            let run_start = itr;
            while itr != len && rings.hot_pixels[itr].y == y {
                itr += 1;
            }
            let add_end_point_itr = y != end_pt.y || add_end_point;
            hot_pixel_set_right_to_left(y, start_x, end_x, &b[bnd], rings, run_start, itr, add_end_point_itr);
        }
    } else {
        while itr != len {
            if rings.hot_pixels[itr].y > start_y {
                itr += 1;
                continue;
            }
            if rings.hot_pixels[itr].y < end_y {
                break;
            }
            let y = rings.hot_pixels[itr].y;
            let first_itr = itr;
            while itr != len && rings.hot_pixels[itr].y == y {
                itr += 1;
            }
            let add_end_point_itr = y != end_pt.y || add_end_point;
            hot_pixel_set_left_to_right(y, start_x, end_x, &b[bnd], rings, first_itr, itr, add_end_point_itr);
        }
    }
    b[bnd].last_point = end_pt;
}

/// `add_to_hot_pixels`.
pub(crate) fn add_to_hot_pixels(pt: GPoint, rings: &mut RingManager) {
    rings.hot_pixels.push(pt);
}

/// `add_first_point`.
fn add_first_point(bnd: BoundPtr, active_bounds: &[BoundPtr], pt: GPoint, b: &mut [Bound], rings: &mut RingManager) {
    let r = create_new_ring(rings);
    b[bnd].ring = r;
    let p = create_new_point(r, pt, rings);
    rings.rings[r].points = p;
    set_hole_state(bnd, active_bounds, b, rings);
    b[bnd].last_point = pt;
}

/// `add_point_to_ring`.
pub(crate) fn add_point_to_ring(bnd: BoundPtr, pt: GPoint, b: &mut [Bound], rings: &mut RingManager) {
    debug_assert!(b[bnd].ring != NIL);
    // Handle hot pixels
    insert_hot_pixels_in_path(bnd, pt, b, rings, false);

    // bnd.ring->points is the 'Left-most' point & bnd.ring->points->prev is the
    // 'Right-most'
    insert_point(&b[bnd], pt, rings);
}

/// `add_point`.
pub(crate) fn add_point(
    bnd: BoundPtr,
    active_bounds: &[BoundPtr],
    pt: GPoint,
    b: &mut [Bound],
    rings: &mut RingManager,
) {
    if b[bnd].ring == NIL {
        add_first_point(bnd, active_bounds, pt, b, rings);
    } else {
        add_point_to_ring(bnd, pt, b, rings);
    }
}

/// `add_local_minimum_point`.
pub(crate) fn add_local_minimum_point(
    b1: BoundPtr,
    b2: BoundPtr,
    active_bounds: &[BoundPtr],
    pt: GPoint,
    b: &mut [Bound],
    rings: &mut RingManager,
) {
    if is_horizontal(b[b2].cur()) || (b[b1].cur().dx > b[b2].cur().dx) {
        add_point(b1, active_bounds, pt, b, rings);
        b[b2].last_point = pt;
        b[b2].ring = b[b1].ring;
        b[b1].side = EdgeSide::Left;
        b[b2].side = EdgeSide::Right;
    } else {
        add_point(b2, active_bounds, pt, b, rings);
        b[b1].last_point = pt;
        b[b1].ring = b[b2].ring;
        b[b1].side = EdgeSide::Right;
        b[b2].side = EdgeSide::Left;
    }
}

/// `get_dx(point, point)`.
fn get_dx(rings: &RingManager, pt1: PointPtr, pt2: PointPtr) -> f64 {
    let (x1, y1) = rings.xy(pt1);
    let (x2, y2) = rings.xy(pt2);
    if y1 == y2 { f64::INFINITY } else { f64::from(x2.wrapping_sub(x1)) / f64::from(y2.wrapping_sub(y1)) }
}

/// `first_is_bottom_point`.
fn first_is_bottom_point(rings: &RingManager, btm_pt1: PointPtr, btm_pt2: PointPtr) -> bool {
    let mut p = rings.prev(btm_pt1);
    while rings.pt_eq(p, btm_pt1) && p != btm_pt1 {
        p = rings.prev(p);
    }
    let dx1p = get_dx(rings, btm_pt1, p).abs();

    p = rings.next(btm_pt1);
    while rings.pt_eq(p, btm_pt1) && p != btm_pt1 {
        p = rings.next(p);
    }
    let dx1n = get_dx(rings, btm_pt1, p).abs();

    p = rings.prev(btm_pt2);
    while rings.pt_eq(p, btm_pt2) && p != btm_pt2 {
        p = rings.prev(p);
    }
    let dx2p = get_dx(rings, btm_pt2, p).abs();

    p = rings.next(btm_pt2);
    while rings.pt_eq(p, btm_pt2) && p != btm_pt2 {
        p = rings.next(p);
    }
    let dx2n = get_dx(rings, btm_pt2, p).abs();

    if values_are_equal(cpp_max(dx1p, dx1n), cpp_max(dx2p, dx2n))
        && values_are_equal(cpp_min(dx1p, dx1n), cpp_min(dx2p, dx2n))
    {
        // if otherwise identical use orientation
        rings.area_from_point(btm_pt1).0 > 0.0
    } else {
        (greater_than_or_equal(dx1p, dx2p) && greater_than_or_equal(dx1p, dx2n))
            || (greater_than_or_equal(dx1n, dx2p) && greater_than_or_equal(dx1n, dx2n))
    }
}

/// `get_bottom_point`.
fn get_bottom_point(rings: &RingManager, mut pp: PointPtr) -> PointPtr {
    let mut dups = NIL;
    let mut p = rings.next(pp);
    while p != pp {
        let (px, py) = rings.xy(p);
        let (ppx, ppy) = rings.xy(pp);
        if py > ppy {
            pp = p;
            dups = NIL;
        } else if py == ppy && px <= ppx {
            if px < ppx {
                dups = NIL;
                pp = p;
            } else if rings.next(p) != pp && rings.prev(p) != pp {
                dups = p;
            }
        }
        p = rings.next(p);
    }
    if dups != NIL {
        // there appears to be at least 2 vertices at bottom_point so ...
        while dups != p {
            if !first_is_bottom_point(rings, p, dups) {
                pp = dups;
            }
            dups = rings.next(dups);
            while !rings.pt_eq(dups, pp) {
                dups = rings.next(dups);
            }
        }
    }
    pp
}

/// `get_lower_most_ring`: work out which polygon fragment has the correct
/// hole state.
fn get_lower_most_ring(ring1: RingPtr, ring2: RingPtr, rings: &mut RingManager) -> RingPtr {
    if rings.rings[ring1].bottom_point == NIL {
        rings.rings[ring1].bottom_point = get_bottom_point(rings, rings.rings[ring1].points);
    }
    if rings.rings[ring2].bottom_point == NIL {
        rings.rings[ring2].bottom_point = get_bottom_point(rings, rings.rings[ring2].points);
    }
    let pt1 = rings.rings[ring1].bottom_point;
    let pt2 = rings.rings[ring2].bottom_point;
    let (x1, y1) = rings.xy(pt1);
    let (x2, y2) = rings.xy(pt2);
    if y1 > y2 {
        ring1
    } else if y1 < y2 {
        ring2
    } else if x1 < x2 {
        ring1
    } else if x1 > x2 || rings.next(pt1) == pt1 {
        ring2
    } else if rings.next(pt2) == pt2 || first_is_bottom_point(rings, pt1, pt2) {
        ring1
    } else {
        ring2
    }
}

/// `ring1_child_below_ring2`.
fn ring1_child_below_ring2(mut ring1: RingPtr, ring2: RingPtr, rings: &RingManager) -> bool {
    loop {
        ring1 = rings.rings[ring1].parent;
        if ring1 == ring2 {
            return true;
        }
        if ring1 == NIL {
            return false;
        }
    }
}

/// `update_points_ring`.
pub(crate) fn update_points_ring(ring: RingPtr, rings: &mut RingManager) {
    let start = rings.rings[ring].points;
    let mut op = start;
    loop {
        rings.points[op].ring = ring;
        op = rings.points[op].prev;
        if op == start {
            break;
        }
    }
}

/// `append_ring`.
fn append_ring(b1: BoundPtr, b2: BoundPtr, active_bounds: &[BoundPtr], b: &mut [Bound], manager: &mut RingManager) {
    // get the start and ends of both output polygons ...
    let out_rec1 = b[b1].ring;
    let out_rec2 = b[b2].ring;

    let (keep_ring, keep_bound, remove_ring, remove_bound) = if ring1_child_below_ring2(out_rec1, out_rec2, manager) {
        (out_rec2, b2, out_rec1, b1)
    } else if ring1_child_below_ring2(out_rec2, out_rec1, manager)
        || out_rec1 == get_lower_most_ring(out_rec1, out_rec2, manager)
    {
        (out_rec1, b1, out_rec2, b2)
    } else {
        (out_rec2, b2, out_rec1, b1)
    };

    // get the start and ends of both output polygons and
    // join b2 poly onto b1 poly and delete pointers to b2 ...
    let p1_lft = manager.rings[keep_ring].points;
    let p1_rt = manager.prev(p1_lft);
    let p2_lft = manager.rings[remove_ring].points;
    let p2_rt = manager.prev(p2_lft);

    // join b2 poly onto b1 poly and delete pointers to b2 ...
    if b[keep_bound].side == EdgeSide::Left {
        if b[remove_bound].side == EdgeSide::Left {
            // z y x a b c
            reverse_ring(p2_lft, manager);
            manager.points[p2_lft].next = p1_lft;
            manager.points[p1_lft].prev = p2_lft;
            manager.points[p1_rt].next = p2_rt;
            manager.points[p2_rt].prev = p1_rt;
            manager.rings[keep_ring].points = p2_rt;
        } else {
            // x y z a b c
            manager.points[p2_rt].next = p1_lft;
            manager.points[p1_lft].prev = p2_rt;
            manager.points[p2_lft].prev = p1_rt;
            manager.points[p1_rt].next = p2_lft;
            manager.rings[keep_ring].points = p2_lft;
        }
    } else if b[remove_bound].side == EdgeSide::Right {
        // a b c z y x
        reverse_ring(p2_lft, manager);
        manager.points[p1_rt].next = p2_rt;
        manager.points[p2_rt].prev = p1_rt;
        manager.points[p2_lft].next = p1_lft;
        manager.points[p1_lft].prev = p2_lft;
    } else {
        // a b c x y z
        manager.points[p1_rt].next = p2_lft;
        manager.points[p2_lft].prev = p1_rt;
        manager.points[p1_lft].prev = p2_rt;
        manager.points[p2_rt].next = p1_lft;
    }

    manager.rings[keep_ring].bottom_point = NIL;
    let keep_is_hole = ring_is_hole(keep_ring, manager);
    let remove_is_hole = ring_is_hole(remove_ring, manager);

    manager.rings[remove_ring].points = NIL;
    manager.rings[remove_ring].bottom_point = NIL;
    if keep_is_hole == remove_is_hole {
        ring1_replaces_ring2(keep_ring, remove_ring, manager);
    } else {
        let parent = manager.rings[keep_ring].parent;
        ring1_replaces_ring2(parent, remove_ring, manager);
    }

    update_points_ring(keep_ring, manager);

    // nb: safe because we only get here via AddLocalMaxPoly
    b[keep_bound].ring = NIL;
    b[remove_bound].ring = NIL;

    for &bb in active_bounds {
        if bb == NIL {
            continue;
        }
        if b[bb].ring == remove_ring {
            b[bb].ring = keep_ring;
            b[bb].side = b[keep_bound].side;
            break; // Not sure why there is a break here but was transfered logic from angus
        }
    }
}

/// `add_local_maximum_point`.
pub(crate) fn add_local_maximum_point(
    b1: BoundPtr,
    b2: BoundPtr,
    pt: GPoint,
    b: &mut [Bound],
    rings: &mut RingManager,
    active_bounds: &[BoundPtr],
) {
    insert_hot_pixels_in_path(b2, pt, b, rings, false);
    add_point(b1, active_bounds, pt, b, rings);
    if b[b1].ring == b[b2].ring {
        b[b1].ring = NIL;
        b[b2].ring = NIL;
        // I am not certain that order is important here?
    } else if rings.rings[b[b1].ring].ring_index < rings.rings[b[b2].ring].ring_index {
        append_ring(b1, b2, active_bounds, b, rings);
    } else {
        append_ring(b2, b1, active_bounds, b, rings);
    }
}

/// `enum point_in_polygon_result`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PointInPolygonResult {
    On = -1,
    Inside = 0,
    Outside = 1,
}

fn toggle(result: PointInPolygonResult) -> PointInPolygonResult {
    if result == PointInPolygonResult::Outside { PointInPolygonResult::Inside } else { PointInPolygonResult::Outside }
}

/// `point_in_polygon(point<T> const&, point_ptr<T>)`.
fn point_in_polygon(pt: (i32, i32), mut op: PointPtr, rings: &RingManager) -> PointInPolygonResult {
    // returns 0 if false, +1 if true, -1 if pt ON polygon boundary
    let mut result = PointInPolygonResult::Outside;
    let start_op = op;
    let (ptx, pty) = pt;
    loop {
        let (opx, opy) = rings.xy(op);
        let (nx, ny) = rings.xy(rings.next(op));
        if ny == pty && ((nx == ptx) || (opy == pty && ((nx > ptx) == (opx < ptx)))) {
            return PointInPolygonResult::On;
        }
        if (opy < pty) != (ny < pty) {
            if opx >= ptx {
                if nx > ptx {
                    // Switch between point outside polygon and point inside
                    // polygon
                    result = toggle(result);
                } else {
                    let d = f64::from(opx.wrapping_sub(ptx)) * f64::from(ny.wrapping_sub(pty))
                        - f64::from(nx.wrapping_sub(ptx)) * f64::from(opy.wrapping_sub(pty));
                    if value_is_zero(d) {
                        return PointInPolygonResult::On;
                    }
                    if (d > 0.0) == (ny > opy) {
                        result = toggle(result);
                    }
                }
            } else if nx > ptx {
                let d = f64::from(opx.wrapping_sub(ptx)) * f64::from(ny.wrapping_sub(pty))
                    - f64::from(nx.wrapping_sub(ptx)) * f64::from(opy.wrapping_sub(pty));
                if value_is_zero(d) {
                    return PointInPolygonResult::On;
                }
                if (d > 0.0) == (ny > opy) {
                    result = toggle(result);
                }
            }
        }
        op = rings.next(op);
        if start_op == op {
            break;
        }
    }
    result
}

/// `point_in_polygon(mapbox::geometry::point<double> const&, point_ptr<T>)`.
fn point_in_polygon_f(pt: (f64, f64), mut op: PointPtr, rings: &RingManager) -> PointInPolygonResult {
    // returns 0 if false, +1 if true, -1 if pt ON polygon boundary
    let mut result = PointInPolygonResult::Outside;
    let start_op = op;
    let (ptx, pty) = pt;
    loop {
        let (ox, oy) = rings.xy(op);
        let (nx, ny) = rings.xy(rings.next(op));
        let op_x = f64::from(ox);
        let op_y = f64::from(oy);
        let op_next_x = f64::from(nx);
        let op_next_y = f64::from(ny);
        if values_are_equal(op_next_y, pty)
            && (values_are_equal(op_next_x, ptx)
                || (values_are_equal(op_y, pty) && ((op_next_x > ptx) == (op_x < ptx))))
        {
            return PointInPolygonResult::On;
        }
        if (op_y < pty) != (op_next_y < pty) {
            if greater_than_or_equal(op_x, ptx) {
                if op_next_x > ptx {
                    result = toggle(result);
                } else {
                    let d = (op_x - ptx) * (op_next_y - pty) - (op_next_x - ptx) * (op_y - pty);
                    if value_is_zero(d) {
                        return PointInPolygonResult::On;
                    }
                    if (d > 0.0) == (op_next_y > op_y) {
                        result = toggle(result);
                    }
                }
            } else if op_next_x > ptx {
                let d = (op_x - ptx) * (op_next_y - pty) - (op_next_x - ptx) * (op_y - pty);
                if value_is_zero(d) {
                    return PointInPolygonResult::On;
                }
                if (d > 0.0) == (op_next_y > op_y) {
                    result = toggle(result);
                }
            }
        }
        op = rings.next(op);
        if start_op == op {
            break;
        }
    }
    result
}

/// `is_convex`.
fn is_convex(edge: PointPtr, rings: &mut RingManager) -> bool {
    let prev = rings.prev(edge);
    let next = rings.next(edge);
    let (ex, ey) = rings.xy(edge);
    let (px, py) = rings.xy(prev);
    let (nx, ny) = rings.xy(next);
    let v1x = ex.wrapping_sub(px);
    let v1y = ey.wrapping_sub(py);
    let v2x = nx.wrapping_sub(ex);
    let v2y = ny.wrapping_sub(ey);
    let cross = v1x.wrapping_mul(v2y).wrapping_sub(v2x.wrapping_mul(v1y));
    let ring = rings.ring_of(edge);
    if cross < 0 && rings.area(ring) > 0.0 { true } else { cross > 0 && rings.area(ring) < 0.0 }
}

/// `centroid_of_points`.
fn centroid_of_points(edge: PointPtr, rings: &RingManager) -> (f64, f64) {
    let (px, py) = rings.xy(rings.prev(edge));
    let (ex, ey) = rings.xy(edge);
    let (nx, ny) = rings.xy(rings.next(edge));
    (f64::from(px.wrapping_add(ex).wrapping_add(nx)) / 3.0, f64::from(py.wrapping_add(ey).wrapping_add(ny)) / 3.0)
}

/// `inside_or_outside_special`.
fn inside_or_outside_special(
    first_pt: PointPtr,
    other_poly: PointPtr,
    rings: &mut RingManager,
) -> R<PointInPolygonResult> {
    // We are going to loop through all the points
    // of the original triangle. The goal is to find a convex edge
    // that with its next and previous forms a triangle with its centroid
    // that is within the first ring. Then we will check the other polygon
    // to see if it is within this polygon.
    let mut itr = first_pt;
    loop {
        if is_convex(itr, rings) {
            let pt = centroid_of_points(itr, rings);
            if PointInPolygonResult::Inside == point_in_polygon_f(pt, first_pt, rings) {
                return Ok(point_in_polygon_f(pt, other_poly, rings));
            }
        }
        itr = rings.next(itr);
        if itr == first_pt {
            break;
        }
    }
    Err("Could not find a point within the polygon to test".to_owned())
}

/// `box2_contains_box1`.
fn box2_contains_box1(box1: &GBox, box2: &GBox) -> bool {
    box2.max.x >= box1.max.x && box2.max.y >= box1.max.y && box2.min.x <= box1.min.x && box2.min.y <= box1.min.y
}

/// `poly2_contains_poly1`.
pub(crate) fn poly2_contains_poly1(ring1: RingPtr, ring2: RingPtr, rings: &mut RingManager) -> R<bool> {
    if !box2_contains_box1(&rings.rings[ring1].bbox, &rings.rings[ring2].bbox) {
        return Ok(false);
    }

    if rings.area(ring2).abs() < rings.area(ring1).abs() {
        return Ok(false);
    }
    let outpt1 = rings.next(rings.rings[ring1].points);
    let outpt2 = rings.next(rings.rings[ring2].points);
    let mut op = outpt1;
    loop {
        // nb: PointInPolygon returns 0 if false, +1 if true, -1 if pt on polygon
        let res = point_in_polygon(rings.xy(op), outpt2, rings);
        if res != PointInPolygonResult::On {
            return Ok(res == PointInPolygonResult::Inside);
        }
        op = rings.next(op);
        if op == outpt1 {
            break;
        }
    }
    let res = inside_or_outside_special(outpt1, outpt2, rings)?;
    Ok(res == PointInPolygonResult::Inside)
}
