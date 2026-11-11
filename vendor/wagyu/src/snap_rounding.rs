// Ports vendor/wagyu/include/mapbox/geometry/wagyu/snap_rounding.hpp.

use crate::active_bound_list::{current_edge_is_horizontal, insert_bound_into_abl, next_edge_in_bound};
use crate::bound::{Bound, BoundPtr};
use crate::edge::is_horizontal;
use crate::intersect_util::{bubble_sort, get_edge_intersection, intersection_compare, round_point, update_current_x};
use crate::local_minimum::{LocalMinimum, local_minimum_sorter};
use crate::point::{GPoint, NIL};
use crate::ring::{R, RingManager};
use crate::ring_util::{add_to_hot_pixels, sort_hot_pixels};
use crate::scanbeam::{insert_sorted_scanbeam, pop_from_scanbeam, setup_scanbeam};
use crate::util::wround;

/// `process_hot_pixel_intersections`.
fn process_hot_pixel_intersections(
    top_y: i32,
    active_bounds: &mut [BoundPtr],
    b: &mut [Bound],
    manager: &mut RingManager,
) -> R<()> {
    if active_bounds.is_empty() {
        return Ok(());
    }
    update_current_x(active_bounds, b, top_y);
    let b: &[Bound] = b;
    bubble_sort(
        active_bounds,
        |b1, b2| intersection_compare(&b[b1], &b[b2]),
        |b1, b2| match get_edge_intersection(b[b1].cur(), b[b2].cur()) {
            Some(pt) => {
                add_to_hot_pixels(round_point(pt), manager);
                Ok(())
            }
            None => Err("Trying to find intersection of lines that do not intersect".to_owned()),
        },
    )
}

/// `horizontals_at_top_scanbeam`.
fn horizontals_at_top_scanbeam(
    top_y: i32,
    bnd_curr: &mut usize,
    active_bounds: &mut [BoundPtr],
    b: &mut [Bound],
    manager: &mut RingManager,
) -> bool {
    let me = active_bounds[*bnd_curr];
    let mut shifted = false;
    b[me].current_x = f64::from(b[me].cur().top.x);
    if b[me].cur().bot.x < b[me].cur().top.x {
        // left to right
        let mut bnd_next = *bnd_curr + 1;
        while bnd_next != active_bounds.len()
            && (active_bounds[bnd_next] == NIL || b[active_bounds[bnd_next]].current_x < b[me].current_x)
        {
            let n = active_bounds[bnd_next];
            if n != NIL && b[n].cur().top.y != top_y && b[n].cur().bot.y != top_y {
                let pt = GPoint::new(wround(b[n].current_x), top_y);
                add_to_hot_pixels(pt, manager);
            }
            active_bounds.swap(*bnd_curr, bnd_next);
            *bnd_curr += 1;
            bnd_next += 1;
            shifted = true;
        }
    } else {
        // right to left
        if *bnd_curr != 0 {
            let mut bnd_prev = *bnd_curr - 1;
            while *bnd_curr != 0
                && (active_bounds[bnd_prev] == NIL || b[active_bounds[bnd_prev]].current_x > b[me].current_x)
            {
                let p = active_bounds[bnd_prev];
                if p != NIL && b[p].cur().top.y != top_y && b[p].cur().bot.y != top_y {
                    let pt = GPoint::new(wround(b[p].current_x), top_y);
                    add_to_hot_pixels(pt, manager);
                }
                active_bounds.swap(*bnd_curr, bnd_prev);
                *bnd_curr -= 1;
                if *bnd_curr != 0 {
                    bnd_prev -= 1;
                }
            }
        }
    }
    shifted
}

/// `process_hot_pixel_edges_at_top_of_scanbeam`.
fn process_hot_pixel_edges_at_top_of_scanbeam(
    top_y: i32,
    scanbeam: &mut Vec<i32>,
    active_bounds: &mut Vec<BoundPtr>,
    b: &mut [Bound],
    manager: &mut RingManager,
) {
    let mut bnd = 0;
    while bnd != active_bounds.len() {
        let current_bound = active_bounds[bnd];
        if current_bound == NIL {
            bnd += 1;
            continue;
        }
        let mut bnd_curr = bnd;
        let mut shifted = false;
        while b[current_bound].current_edge != b[current_bound].edges.len() && b[current_bound].cur().top.y == top_y {
            let top = b[current_bound].cur().top;
            add_to_hot_pixels(top, manager);
            if is_horizontal(b[current_bound].cur())
                && horizontals_at_top_scanbeam(top_y, &mut bnd_curr, active_bounds, b, manager)
            {
                shifted = true;
            }
            next_edge_in_bound(&mut b[current_bound], scanbeam);
        }
        if b[current_bound].current_edge == b[current_bound].edges.len() {
            active_bounds[bnd_curr] = NIL;
        }
        if !shifted {
            bnd += 1;
        }
    }
    active_bounds.retain(|&x| x != NIL);
}

/// `insert_local_minima_into_ABL_hot_pixel`.
fn insert_local_minima_into_abl_hot_pixel(
    top_y: i32,
    minima: &[LocalMinimum],
    minima_sorted: &[usize],
    lm: &mut usize,
    active_bounds: &mut Vec<BoundPtr>,
    b: &mut [Bound],
    manager: &mut RingManager,
    scanbeam: &mut Vec<i32>,
) {
    while *lm != minima_sorted.len() && minima[minima_sorted[*lm]].y == top_y {
        let m = &minima[minima_sorted[*lm]];
        add_to_hot_pixels(b[m.left_bound].edges[0].bot, manager);
        for bound in [m.left_bound, m.right_bound] {
            let bnd = &mut b[bound];
            bnd.current_edge = 0;
            bnd.next_edge = 1;
            bnd.current_x = f64::from(bnd.edges[0].bot.x);
        }
        let lb_abl_itr = insert_bound_into_abl(m.left_bound, m.right_bound, active_bounds, b);
        let lb = active_bounds[lb_abl_itr];
        if !current_edge_is_horizontal(&b[lb]) {
            insert_sorted_scanbeam(scanbeam, b[lb].cur().top.y);
        }
        let rb = active_bounds[lb_abl_itr + 1];
        if !current_edge_is_horizontal(&b[rb]) {
            insert_sorted_scanbeam(scanbeam, b[rb].cur().top.y);
        }
        *lm += 1;
    }
}

/// `build_hot_pixels`.
pub(crate) fn build_hot_pixels(minima: &[LocalMinimum], b: &mut [Bound], manager: &mut RingManager) -> R<()> {
    let mut active_bounds: Vec<BoundPtr> = Vec::new();
    let mut scanbeam: Vec<i32> = Vec::new();
    let mut scanline_y = i32::MAX;

    let mut minima_sorted: Vec<usize> = (0..minima.len()).collect();
    minima_sorted.sort_by(|&a, &c| local_minimum_sorter(&minima[a], &minima[c]));
    let mut current_lm = 0;

    setup_scanbeam(minima, &mut scanbeam);

    // Estimate size for reserving hot pixels
    let mut reserve = 0;
    for lm in minima {
        reserve += b[lm.left_bound].edges.len() + 2;
        reserve += b[lm.right_bound].edges.len() + 2;
    }
    manager.hot_pixels.reserve(reserve);

    loop {
        let popped = pop_from_scanbeam(&mut scanline_y, &mut scanbeam);
        if !(popped || current_lm != minima_sorted.len()) {
            break;
        }
        process_hot_pixel_intersections(scanline_y, &mut active_bounds, b, manager)?;

        insert_local_minima_into_abl_hot_pixel(
            scanline_y,
            minima,
            &minima_sorted,
            &mut current_lm,
            &mut active_bounds,
            b,
            manager,
            &mut scanbeam,
        );

        process_hot_pixel_edges_at_top_of_scanbeam(scanline_y, &mut scanbeam, &mut active_bounds, b, manager);
    }
    // preallocate_point_memory: capacity only.
    let n = manager.hot_pixels.len();
    manager.points.reserve(n);
    manager.all_points.reserve(n);
    sort_hot_pixels(manager);
    Ok(())
}
