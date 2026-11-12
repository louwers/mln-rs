// Ports vendor/wagyu/include/mapbox/geometry/wagyu/intersect.hpp,
// intersect_util.hpp and bubble_sort.hpp.

use std::cmp::{Ordering, Reverse};

use crate::active_bound_list::{is_even_odd_fill_type, swap_rings, swap_sides};
use crate::bound::{Bound, BoundPtr};
use crate::config::{ClipType, FillType, PolygonType};
use crate::edge::{Edge, get_current_x, slopes_equal};
use crate::point::GPoint;
use crate::ring::{R, RingManager};
use crate::ring_util::{add_local_maximum_point, add_local_minimum_point, add_point, round_towards_max};
use crate::util::values_are_equal;

/// `intersect_node<int32_t>`.
#[derive(Clone, Copy, Debug)]
pub(crate) struct IntersectNode {
    pub bound1: BoundPtr,
    pub bound2: BoundPtr,
    pub pt: (f64, f64),
}

/// `std::stable_sort` with `intersect_list_sorter`: by descending y, then
/// by descending sum of the bounds' `winding_count2`, where y values within
/// the ULP tolerance of each other count as equal.
///
/// The tolerance makes `intersect_list_sorter` no strict weak ordering, and
/// Rust's sorts may panic on one. This sorts by exact y, then sorts each run
/// of neighbours within the tolerance by winding count. Where every two y
/// values of a run are within the tolerance, that is the order
/// `std::stable_sort` gives.
fn sort_intersect_list(intersects: &mut Vec<IntersectNode>, b: &[Bound]) {
    let mut order: Vec<usize> = (0..intersects.len()).collect();
    order.sort_by(|&i, &j| intersects[j].pt.1.total_cmp(&intersects[i].pt.1));
    for run in order.chunk_by_mut(|&i, &j| values_are_equal(intersects[i].pt.1, intersects[j].pt.1)) {
        run.sort_by_key(|&i| {
            let node = &intersects[i];
            (Reverse(b[node.bound1].winding_count2 + b[node.bound2].winding_count2), i)
        });
    }
    *intersects = order.into_iter().map(|i| intersects[i]).collect();
}

/// `round_point`.
pub(crate) fn round_point(pt: (f64, f64)) -> GPoint {
    GPoint::new(round_towards_max(pt.0), round_towards_max(pt.1))
}

/// `get_edge_intersection<int32_t, double>`.
pub(crate) fn get_edge_intersection(e1: &Edge, e2: &Edge) -> Option<(f64, f64)> {
    let p0_x = f64::from(e1.bot.x);
    let p0_y = f64::from(e1.bot.y);
    let p1_x = f64::from(e1.top.x);
    let p1_y = f64::from(e1.top.y);
    let p2_x = f64::from(e2.bot.x);
    let p2_y = f64::from(e2.bot.y);
    let p3_x = f64::from(e2.top.x);
    let p3_y = f64::from(e2.top.y);
    let s1_x = p1_x - p0_x;
    let s1_y = p1_y - p0_y;
    let s2_x = p3_x - p2_x;
    let s2_y = p3_y - p2_y;

    let s = (-s1_y * (p0_x - p2_x) + s1_x * (p0_y - p2_y)) / (-s2_x * s1_y + s1_x * s2_y);
    let t = (s2_x * (p0_y - p2_y) - s2_y * (p0_x - p2_x)) / (-s2_x * s1_y + s1_x * s2_y);

    if (0.0..=1.0).contains(&s) && (0.0..=1.0).contains(&t) {
        return Some((p0_x + (t * s1_x), p0_y + (t * s1_y)));
    }
    None
}

/// `intersection_compare`.
pub(crate) fn intersection_compare(b1: &Bound, b2: &Bound) -> bool {
    b1.current_x.partial_cmp(&b2.current_x) != Some(Ordering::Greater) || slopes_equal(b1.cur(), b2.cur())
}

/// `bubble_sort(begin, end, c, m)` over the active bounds; `m` is called
/// before each swap and may throw.
pub(crate) fn bubble_sort<C, M>(active_bounds: &mut [BoundPtr], mut c: C, mut m: M) -> R<()>
where
    C: FnMut(BoundPtr, BoundPtr) -> bool,
    M: FnMut(BoundPtr, BoundPtr) -> R<()>,
{
    if active_bounds.is_empty() {
        return Ok(());
    }
    let last = active_bounds.len() - 1;
    loop {
        let mut modified = false;
        for itr in 0..last {
            let next = itr + 1;
            if !c(active_bounds[itr], active_bounds[next]) {
                m(active_bounds[itr], active_bounds[next])?;
                active_bounds.swap(itr, next);
                modified = true;
            }
        }
        if !modified {
            return Ok(());
        }
    }
}

/// `build_intersect_list`.
fn build_intersect_list(active_bounds: &mut [BoundPtr], b: &[Bound], intersects: &mut Vec<IntersectNode>) -> R<()> {
    bubble_sort(
        active_bounds,
        |b1, b2| intersection_compare(&b[b1], &b[b2]),
        |b1, b2| match get_edge_intersection(b[b1].cur(), b[b2].cur()) {
            Some(pt) => {
                intersects.push(IntersectNode { bound1: b1, bound2: b2, pt });
                Ok(())
            }
            None => Err("Trying to find intersection of lines that do not intersect".to_owned()),
        },
    )
}

/// The winding count used by `intersect_bounds` for a fill type.
fn winding_for_fill(fill: FillType, wc: i32) -> i32 {
    match fill {
        FillType::Positive => wc,
        FillType::Negative => -wc,
        FillType::EvenOdd | FillType::NonZero => wc.abs(),
    }
}

/// `intersect_bounds`: assumes that b1 will be to the Right of b2 ABOVE the
/// intersection.
#[allow(clippy::too_many_arguments)]
pub(crate) fn intersect_bounds(
    b1: BoundPtr,
    b2: BoundPtr,
    pt: GPoint,
    cliptype: ClipType,
    subject_fill_type: FillType,
    clip_fill_type: FillType,
    b: &mut [Bound],
    rings: &mut RingManager,
    active_bounds: &[BoundPtr],
) {
    let b1_contributing = b[b1].ring != crate::point::NIL;
    let b2_contributing = b[b2].ring != crate::point::NIL;

    // update winding counts...
    // assumes that b1 will be to the Right of b2 ABOVE the intersection
    if b[b1].poly_type == b[b2].poly_type {
        if is_even_odd_fill_type(&b[b1], subject_fill_type, clip_fill_type) {
            let wc = b[b1].winding_count;
            b[b1].winding_count = b[b2].winding_count;
            b[b2].winding_count = wc;
        } else {
            if b[b1].winding_count + i32::from(b[b2].winding_delta) == 0 {
                b[b1].winding_count = -b[b1].winding_count;
            } else {
                b[b1].winding_count += i32::from(b[b2].winding_delta);
            }
            if b[b2].winding_count - i32::from(b[b1].winding_delta) == 0 {
                b[b2].winding_count = -b[b2].winding_count;
            } else {
                b[b2].winding_count -= i32::from(b[b1].winding_delta);
            }
        }
    } else {
        if is_even_odd_fill_type(&b[b2], subject_fill_type, clip_fill_type) {
            b[b1].winding_count2 = i32::from(b[b1].winding_count2 == 0);
        } else {
            b[b1].winding_count2 += i32::from(b[b2].winding_delta);
        }
        if is_even_odd_fill_type(&b[b1], subject_fill_type, clip_fill_type) {
            b[b2].winding_count2 = i32::from(b[b2].winding_count2 == 0);
        } else {
            b[b2].winding_count2 -= i32::from(b[b1].winding_delta);
        }
    }

    let (b1_fill_type, b1_fill_type2) = if b[b1].poly_type == PolygonType::Subject {
        (subject_fill_type, clip_fill_type)
    } else {
        (clip_fill_type, subject_fill_type)
    };
    let (b2_fill_type, b2_fill_type2) = if b[b2].poly_type == PolygonType::Subject {
        (subject_fill_type, clip_fill_type)
    } else {
        (clip_fill_type, subject_fill_type)
    };

    let b1_wc = winding_for_fill(b1_fill_type, b[b1].winding_count);
    let b2_wc = winding_for_fill(b2_fill_type, b[b2].winding_count);

    if b1_contributing && b2_contributing {
        if (b1_wc != 0 && b1_wc != 1)
            || (b2_wc != 0 && b2_wc != 1)
            || (b[b1].poly_type != b[b2].poly_type && cliptype != ClipType::XOr)
        {
            add_local_maximum_point(b1, b2, pt, b, rings, active_bounds);
        } else {
            add_point(b1, active_bounds, pt, b, rings);
            add_point(b2, active_bounds, pt, b, rings);
            swap_sides(b, b1, b2);
            swap_rings(b, b1, b2);
        }
    } else if b1_contributing {
        if b2_wc == 0 || b2_wc == 1 {
            add_point(b1, active_bounds, pt, b, rings);
            b[b2].last_point = pt;
            swap_sides(b, b1, b2);
            swap_rings(b, b1, b2);
        }
    } else if b2_contributing {
        if b1_wc == 0 || b1_wc == 1 {
            b[b1].last_point = pt;
            add_point(b2, active_bounds, pt, b, rings);
            swap_sides(b, b1, b2);
            swap_rings(b, b1, b2);
        }
    } else if (b1_wc == 0 || b1_wc == 1) && (b2_wc == 0 || b2_wc == 1) {
        // neither bound is currently contributing ...
        let b1_wc2 = winding_for_fill(b1_fill_type2, b[b1].winding_count2);
        let b2_wc2 = winding_for_fill(b2_fill_type2, b[b2].winding_count2);

        if b[b1].poly_type != b[b2].poly_type {
            add_local_minimum_point(b1, b2, active_bounds, pt, b, rings);
        } else if b1_wc == 1 && b2_wc == 1 {
            match cliptype {
                ClipType::Intersection => {
                    if b1_wc2 > 0 && b2_wc2 > 0 {
                        add_local_minimum_point(b1, b2, active_bounds, pt, b, rings);
                    }
                }
                ClipType::Union => {
                    if b1_wc2 <= 0 && b2_wc2 <= 0 {
                        add_local_minimum_point(b1, b2, active_bounds, pt, b, rings);
                    }
                }
                ClipType::Difference => {
                    if ((b[b1].poly_type == PolygonType::Clip) && (b1_wc2 > 0) && (b2_wc2 > 0))
                        || ((b[b1].poly_type == PolygonType::Subject) && (b1_wc2 <= 0) && (b2_wc2 <= 0))
                    {
                        add_local_minimum_point(b1, b2, active_bounds, pt, b, rings);
                    }
                }
                ClipType::XOr => {
                    add_local_minimum_point(b1, b2, active_bounds, pt, b, rings);
                }
            }
        } else {
            swap_sides(b, b1, b2);
        }
    }
}

/// `bounds_adjacent`.
fn bounds_adjacent(inode: &IntersectNode, next: Option<BoundPtr>) -> bool {
    next == Some(inode.bound2) || next == Some(inode.bound1)
}

/// `std::find_if(active_bounds, find_first_bound(inode))`.
fn find_first_bound(active_bounds: &[BoundPtr], inode: &IntersectNode) -> usize {
    active_bounds.iter().position(|&x| x == inode.bound1 || x == inode.bound2).unwrap_or(active_bounds.len())
}

/// `process_intersect_list`.
fn process_intersect_list(
    intersects: &mut [IntersectNode],
    cliptype: ClipType,
    subject_fill_type: FillType,
    clip_fill_type: FillType,
    b: &mut [Bound],
    rings: &mut RingManager,
    active_bounds: &mut [BoundPtr],
) -> R<()> {
    for node_itr in 0..intersects.len() {
        let mut b1 = find_first_bound(active_bounds, &intersects[node_itr]);
        let mut b2 = b1 + 1;
        if !bounds_adjacent(&intersects[node_itr], active_bounds.get(b2).copied()) {
            let mut next_itr = node_itr + 1;
            while next_itr != intersects.len() {
                let n1 = find_first_bound(active_bounds, &intersects[next_itr]);
                let n2 = n1 + 1;
                if bounds_adjacent(&intersects[next_itr], active_bounds.get(n2).copied()) {
                    b1 = n1;
                    b2 = n2;
                    break;
                }
                next_itr += 1;
            }
            if next_itr == intersects.len() {
                return Err("Could not properly correct intersection order.".to_owned());
            }
            intersects.swap(node_itr, next_itr);
        }
        let node = intersects[node_itr];
        let pt = round_point(node.pt);
        intersect_bounds(
            node.bound1,
            node.bound2,
            pt,
            cliptype,
            subject_fill_type,
            clip_fill_type,
            b,
            rings,
            active_bounds,
        );
        active_bounds.swap(b1, b2);
    }
    Ok(())
}

/// `update_current_x`.
pub(crate) fn update_current_x(active_bounds: &[BoundPtr], b: &mut [Bound], top_y: i32) {
    for (pos, &bnd) in active_bounds.iter().enumerate() {
        b[bnd].pos = pos;
        b[bnd].current_x = get_current_x(b[bnd].cur(), top_y);
    }
}

/// `process_intersections`.
pub(crate) fn process_intersections(
    top_y: i32,
    active_bounds: &mut [BoundPtr],
    cliptype: ClipType,
    subject_fill_type: FillType,
    clip_fill_type: FillType,
    b: &mut [Bound],
    rings: &mut RingManager,
) -> R<()> {
    if active_bounds.is_empty() {
        return Ok(());
    }
    update_current_x(active_bounds, b, top_y);
    let mut intersects: Vec<IntersectNode> = Vec::new();
    build_intersect_list(active_bounds, b, &mut intersects)?;

    if intersects.is_empty() {
        return Ok(());
    }

    // Restore order of active bounds list
    active_bounds.sort_by_key(|&bnd| b[bnd].pos);

    // Sort the intersection list
    sort_intersect_list(&mut intersects, b);

    process_intersect_list(&mut intersects, cliptype, subject_fill_type, clip_fill_type, b, rings, active_bounds)
}
