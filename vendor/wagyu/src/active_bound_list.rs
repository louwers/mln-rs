// Ports vendor/wagyu/include/mapbox/geometry/wagyu/active_bound_list.hpp. The
// active bound list is a `Vec<BoundPtr>` (NIL entries are the C++ nullptrs
// set during a scanline); iterators are indices (len = end()).

use crate::bound::{Bound, BoundPtr, EdgeSide};
use crate::config::{ClipType, FillType, PolygonType};
use crate::edge::{get_current_x, is_horizontal};
use crate::local_minimum::LocalMinimum;
use crate::local_minimum_util::initialize_lm;
use crate::ring::RingManager;
use crate::ring_util::add_local_minimum_point;
use crate::scanbeam::insert_sorted_scanbeam;
use crate::util::{greater_than, less_than, values_are_equal};

pub(crate) fn is_even_odd_fill_type(bound: &Bound, subject_fill_type: FillType, clip_fill_type: FillType) -> bool {
    if bound.poly_type == PolygonType::Subject {
        subject_fill_type == FillType::EvenOdd
    } else {
        clip_fill_type == FillType::EvenOdd
    }
}

fn is_even_odd_alt_fill_type(bound: &Bound, subject_fill_type: FillType, clip_fill_type: FillType) -> bool {
    if bound.poly_type == PolygonType::Subject {
        clip_fill_type == FillType::EvenOdd
    } else {
        subject_fill_type == FillType::EvenOdd
    }
}

/// `bound_insert_location` (bound2 = the bound being inserted).
fn bound_insert_location(bound2: &Bound, bound1: &Bound) -> bool {
    if values_are_equal(bound2.current_x, bound1.current_x) {
        if bound2.cur().top.y > bound1.cur().top.y {
            less_than(f64::from(bound2.cur().top.x), get_current_x(bound1.cur(), bound2.cur().top.y))
        } else {
            greater_than(f64::from(bound1.cur().top.x), get_current_x(bound2.cur(), bound1.cur().top.y))
        }
    } else {
        bound2.current_x < bound1.current_x
    }
}

/// `insert_bound_into_ABL`: returns the position of `left` (`right` follows).
pub(crate) fn insert_bound_into_abl(
    left: BoundPtr,
    right: BoundPtr,
    active_bounds: &mut Vec<BoundPtr>,
    b: &[Bound],
) -> usize {
    let itr = active_bounds.iter().position(|&x| bound_insert_location(&b[left], &b[x])).unwrap_or(active_bounds.len());
    active_bounds.insert(itr, right);
    active_bounds.insert(itr, left);
    itr
}

/// `is_maxima(bound, y)`.
pub(crate) fn is_maxima(bnd: &Bound, y: i32) -> bool {
    bnd.next_edge == bnd.edges.len() && bnd.cur().top.y == y
}

/// `is_intermediate(bound, y)`.
pub(crate) fn is_intermediate(bnd: &Bound, y: i32) -> bool {
    bnd.next_edge != bnd.edges.len() && bnd.cur().top.y == y
}

pub(crate) fn current_edge_is_horizontal(bnd: &Bound) -> bool {
    is_horizontal(bnd.cur())
}

pub(crate) fn next_edge_is_horizontal(bnd: &Bound) -> bool {
    is_horizontal(bnd.next())
}

/// `next_edge_in_bound`.
pub(crate) fn next_edge_in_bound(bnd: &mut Bound, scanbeam: &mut Vec<i32>) {
    bnd.current_edge += 1;
    if bnd.current_edge != bnd.edges.len() {
        bnd.next_edge += 1;
        bnd.current_x = f64::from(bnd.cur().bot.x);
        if !is_horizontal(bnd.cur()) {
            insert_sorted_scanbeam(scanbeam, bnd.cur().top.y);
        }
    }
}

/// `get_maxima_pair`: the position of the bound's maximum bound (len if not
/// found).
pub(crate) fn get_maxima_pair(bnd: usize, active_bounds: &[BoundPtr], b: &[Bound]) -> usize {
    let maximum = b[active_bounds[bnd]].maximum_bound;
    active_bounds.iter().position(|&x| x == maximum).unwrap_or(active_bounds.len())
}

/// `set_winding_count`. The reverse iterator is kept as its `base()` index.
fn set_winding_count(
    bnd_itr: usize,
    active_bounds: &[BoundPtr],
    b: &mut [Bound],
    subject_fill_type: FillType,
    clip_fill_type: FillType,
) {
    let me = active_bounds[bnd_itr];
    let mut rev = bnd_itr;
    if rev == 0 {
        b[me].winding_count = i32::from(b[me].winding_delta);
        b[me].winding_count2 = 0;
        return;
    }

    // find the edge of the same polytype that immediately preceeds 'edge' in
    // AEL
    while rev != 0 && b[active_bounds[rev - 1]].poly_type != b[me].poly_type {
        rev -= 1;
    }
    if rev == 0 {
        b[me].winding_count = i32::from(b[me].winding_delta);
        b[me].winding_count2 = 0;
    } else if is_even_odd_fill_type(&b[me], subject_fill_type, clip_fill_type) {
        // EvenOdd filling ...
        let prev = active_bounds[rev - 1];
        b[me].winding_count = i32::from(b[me].winding_delta);
        b[me].winding_count2 = b[prev].winding_count2;
    } else {
        // nonZero, Positive or Negative filling ...
        let prev = active_bounds[rev - 1];
        let prev_wc = b[prev].winding_count;
        let prev_delta = i32::from(b[prev].winding_delta);
        let my_delta = i32::from(b[me].winding_delta);
        if prev_wc * prev_delta < 0 {
            // prev edge is 'decreasing' WindCount (WC) toward zero
            // so we're outside the previous polygon ...
            if prev_wc.abs() > 1 {
                // outside prev poly but still inside another.
                // when reversing direction of prev poly use the same WC
                if prev_delta * my_delta < 0 {
                    b[me].winding_count = prev_wc;
                } else {
                    // otherwise continue to 'decrease' WC ...
                    b[me].winding_count = prev_wc + my_delta;
                }
            } else {
                // now outside all polys of same polytype so set own WC ...
                b[me].winding_count = my_delta;
            }
        } else {
            // prev edge is 'increasing' WindCount (WC) away from zero
            // so we're inside the previous polygon ...
            if prev_delta * my_delta < 0 {
                // if wind direction is reversing prev then use same WC
                b[me].winding_count = prev_wc;
            } else {
                // otherwise add to WC ...
                b[me].winding_count = prev_wc + my_delta;
            }
        }
        b[me].winding_count2 = b[prev].winding_count2;
    }

    // update winding_count2 ...
    let mut fwd = rev;
    if is_even_odd_alt_fill_type(&b[me], subject_fill_type, clip_fill_type) {
        // EvenOdd filling ...
        while fwd != bnd_itr {
            b[me].winding_count2 = i32::from(b[me].winding_count2 == 0);
            fwd += 1;
        }
    } else {
        // nonZero, Positive or Negative filling ...
        while fwd != bnd_itr {
            b[me].winding_count2 += i32::from(b[active_bounds[fwd]].winding_delta);
            fwd += 1;
        }
    }
}

/// `is_contributing`.
pub(crate) fn is_contributing(
    bnd: &Bound,
    cliptype: ClipType,
    subject_fill_type: FillType,
    clip_fill_type: FillType,
) -> bool {
    let mut pft = subject_fill_type;
    let mut pft2 = clip_fill_type;
    if bnd.poly_type != PolygonType::Subject {
        pft = clip_fill_type;
        pft2 = subject_fill_type;
    }

    match pft {
        FillType::EvenOdd => {}
        FillType::NonZero => {
            if bnd.winding_count.abs() != 1 {
                return false;
            }
        }
        FillType::Positive => {
            if bnd.winding_count != 1 {
                return false;
            }
        }
        FillType::Negative => {
            if bnd.winding_count != -1 {
                return false;
            }
        }
    }

    let wc2 = bnd.winding_count2;
    match cliptype {
        ClipType::Intersection => match pft2 {
            FillType::EvenOdd | FillType::NonZero => wc2 != 0,
            FillType::Positive => wc2 > 0,
            FillType::Negative => wc2 < 0,
        },
        ClipType::Union => match pft2 {
            FillType::EvenOdd | FillType::NonZero => wc2 == 0,
            FillType::Positive => wc2 <= 0,
            FillType::Negative => wc2 >= 0,
        },
        ClipType::Difference => {
            if bnd.poly_type == PolygonType::Subject {
                match pft2 {
                    FillType::EvenOdd | FillType::NonZero => wc2 == 0,
                    FillType::Positive => wc2 <= 0,
                    FillType::Negative => wc2 >= 0,
                }
            } else {
                match pft2 {
                    FillType::EvenOdd | FillType::NonZero => wc2 != 0,
                    FillType::Positive => wc2 > 0,
                    FillType::Negative => wc2 < 0,
                }
            }
        }
        ClipType::XOr => true,
    }
}

/// `insert_lm_left_and_right_bound`.
#[allow(clippy::too_many_arguments)]
fn insert_lm_left_and_right_bound(
    left_bound: BoundPtr,
    right_bound: BoundPtr,
    active_bounds: &mut Vec<BoundPtr>,
    b: &mut [Bound],
    rings: &mut RingManager,
    scanbeam: &mut Vec<i32>,
    cliptype: ClipType,
    subject_fill_type: FillType,
    clip_fill_type: FillType,
) {
    // Both left and right bound
    let lb_abl_itr = insert_bound_into_abl(left_bound, right_bound, active_bounds, b);
    let rb_abl_itr = lb_abl_itr + 1;
    set_winding_count(lb_abl_itr, active_bounds, b, subject_fill_type, clip_fill_type);
    let lb = active_bounds[lb_abl_itr];
    let rb = active_bounds[rb_abl_itr];
    b[rb].winding_count = b[lb].winding_count;
    b[rb].winding_count2 = b[lb].winding_count2;
    if is_contributing(&b[left_bound], cliptype, subject_fill_type, clip_fill_type) {
        let pt = b[lb].cur().bot;
        add_local_minimum_point(lb, rb, active_bounds, pt, b, rings);
    }

    // Add top of edges to scanbeam
    insert_sorted_scanbeam(scanbeam, b[lb].cur().top.y);

    if !current_edge_is_horizontal(&b[rb]) {
        insert_sorted_scanbeam(scanbeam, b[rb].cur().top.y);
    }
}

/// `insert_local_minima_into_ABL`.
#[allow(clippy::too_many_arguments)]
pub(crate) fn insert_local_minima_into_abl(
    bot_y: i32,
    minima: &[LocalMinimum],
    minima_sorted: &[usize],
    current_lm: &mut usize,
    active_bounds: &mut Vec<BoundPtr>,
    b: &mut [Bound],
    rings: &mut RingManager,
    scanbeam: &mut Vec<i32>,
    cliptype: ClipType,
    subject_fill_type: FillType,
    clip_fill_type: FillType,
) {
    while *current_lm != minima_sorted.len() && bot_y == minima[minima_sorted[*current_lm]].y {
        let lm = &minima[minima_sorted[*current_lm]];
        initialize_lm(lm, b);
        insert_lm_left_and_right_bound(
            lm.left_bound,
            lm.right_bound,
            active_bounds,
            b,
            rings,
            scanbeam,
            cliptype,
            subject_fill_type,
            clip_fill_type,
        );
        *current_lm += 1;
    }
}

/// `insert_horizontal_local_minima_into_ABL`.
#[allow(clippy::too_many_arguments)]
pub(crate) fn insert_horizontal_local_minima_into_abl(
    top_y: i32,
    minima: &[LocalMinimum],
    minima_sorted: &[usize],
    current_lm: &mut usize,
    active_bounds: &mut Vec<BoundPtr>,
    b: &mut [Bound],
    rings: &mut RingManager,
    scanbeam: &mut Vec<i32>,
    cliptype: ClipType,
    subject_fill_type: FillType,
    clip_fill_type: FillType,
) {
    while *current_lm != minima_sorted.len()
        && top_y == minima[minima_sorted[*current_lm]].y
        && minima[minima_sorted[*current_lm]].minimum_has_horizontal
    {
        let lm = &minima[minima_sorted[*current_lm]];
        initialize_lm(lm, b);
        insert_lm_left_and_right_bound(
            lm.left_bound,
            lm.right_bound,
            active_bounds,
            b,
            rings,
            scanbeam,
            cliptype,
            subject_fill_type,
            clip_fill_type,
        );
        *current_lm += 1;
    }
}

/// `swap_sides`.
pub(crate) fn swap_sides(b: &mut [Bound], b1: BoundPtr, b2: BoundPtr) {
    let side: EdgeSide = b[b1].side;
    b[b1].side = b[b2].side;
    b[b2].side = side;
}

/// `swap_rings`.
pub(crate) fn swap_rings(b: &mut [Bound], b1: BoundPtr, b2: BoundPtr) {
    let ring = b[b1].ring;
    b[b1].ring = b[b2].ring;
    b[b2].ring = ring;
}
