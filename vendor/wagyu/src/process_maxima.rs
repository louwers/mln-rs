// Ports vendor/wagyu/include/mapbox/geometry/wagyu/process_maxima.hpp.

use crate::active_bound_list::{
    current_edge_is_horizontal, get_maxima_pair, insert_horizontal_local_minima_into_abl, is_intermediate, is_maxima,
    next_edge_in_bound, next_edge_is_horizontal,
};
use crate::bound::{Bound, BoundPtr};
use crate::config::{ClipType, FillType};
use crate::edge::get_current_x;
use crate::intersect_util::intersect_bounds;
use crate::local_minimum::LocalMinimum;
use crate::point::NIL;
use crate::process_horizontal::process_horizontals;
use crate::ring::{R, RingManager};
use crate::ring_util::{add_local_maximum_point, add_point_to_ring, insert_hot_pixels_in_path};

/// `do_maxima`.
#[allow(clippy::too_many_arguments)]
fn do_maxima(
    mut bnd: usize,
    bnd_max_pair: usize,
    cliptype: ClipType,
    subject_fill_type: FillType,
    clip_fill_type: FillType,
    b: &mut [Bound],
    manager: &mut RingManager,
    active_bounds: &mut [BoundPtr],
) -> R<usize> {
    let me = active_bounds[bnd];
    let mut bnd_next = bnd + 1;
    let mut return_bnd = bnd;
    let mut skipped = false;
    while bnd_next != active_bounds.len() && bnd_next != bnd_max_pair {
        let next = active_bounds[bnd_next];
        if next == NIL {
            bnd_next += 1;
            continue;
        }
        skipped = true;
        let top = b[me].cur().top;
        intersect_bounds(me, next, top, cliptype, subject_fill_type, clip_fill_type, b, manager, active_bounds);
        active_bounds.swap(bnd, bnd_next);
        bnd = bnd_next;
        bnd_next += 1;
    }

    let mp = active_bounds[bnd_max_pair];
    if b[me].ring != NIL && b[mp].ring != NIL {
        let top = b[me].cur().top;
        add_local_maximum_point(me, mp, top, b, manager, active_bounds);
    } else if b[me].ring != NIL || b[mp].ring != NIL {
        return Err("DoMaxima error".to_owned());
    }
    active_bounds[bnd_max_pair] = NIL;
    active_bounds[bnd] = NIL;
    if !skipped {
        return_bnd += 1;
    }
    Ok(return_bnd)
}

/// `process_edges_at_top_of_scanbeam`.
#[allow(clippy::too_many_arguments)]
pub(crate) fn process_edges_at_top_of_scanbeam(
    top_y: i32,
    active_bounds: &mut Vec<BoundPtr>,
    scanbeam: &mut Vec<i32>,
    minima: &[LocalMinimum],
    minima_sorted: &[usize],
    current_lm: &mut usize,
    b: &mut [Bound],
    manager: &mut RingManager,
    cliptype: ClipType,
    subject_fill_type: FillType,
    clip_fill_type: FillType,
) -> R<()> {
    let mut bnd = 0;
    while bnd != active_bounds.len() {
        let bb = active_bounds[bnd];
        if bb == NIL {
            bnd += 1;
            continue;
        }
        // 1. Process maxima, treating them as if they are "bent" horizontal edges,
        // but exclude maxima with horizontal edges.
        let mut is_maxima_edge = is_maxima(&b[bb], top_y);

        if is_maxima_edge {
            let bnd_max_pair = get_maxima_pair(bnd, active_bounds, b);
            // C++ dereferences end() when the pair is missing (undefined);
            // treated as not a maxima.
            is_maxima_edge = bnd_max_pair != active_bounds.len()
                && !current_edge_is_horizontal(&b[active_bounds[bnd_max_pair]])
                && is_maxima(&b[active_bounds[bnd_max_pair]], top_y);
            if is_maxima_edge {
                bnd = do_maxima(
                    bnd,
                    bnd_max_pair,
                    cliptype,
                    subject_fill_type,
                    clip_fill_type,
                    b,
                    manager,
                    active_bounds,
                )?;
                continue;
            }
        }

        // 2. Promote horizontal edges.
        if is_intermediate(&b[bb], top_y) && next_edge_is_horizontal(&b[bb]) {
            if b[bb].ring != NIL {
                let top = b[bb].cur().top;
                insert_hot_pixels_in_path(bb, top, b, manager, false);
            }
            next_edge_in_bound(&mut b[bb], scanbeam);
            if b[bb].ring != NIL {
                let bot = b[bb].cur().bot;
                add_point_to_ring(bb, bot, b, manager);
            }
        } else {
            b[bb].current_x = get_current_x(b[bb].cur(), top_y);
        }

        bnd += 1;
    }

    active_bounds.retain(|&x| x != NIL);

    insert_horizontal_local_minima_into_abl(
        top_y,
        minima,
        minima_sorted,
        current_lm,
        active_bounds,
        b,
        manager,
        scanbeam,
        cliptype,
        subject_fill_type,
        clip_fill_type,
    );

    process_horizontals(top_y, active_bounds, b, manager, scanbeam, cliptype, subject_fill_type, clip_fill_type);

    // 4. Promote intermediate vertices
    for &bb in active_bounds.iter() {
        if is_intermediate(&b[bb], top_y) {
            if b[bb].ring != NIL {
                let top = b[bb].cur().top;
                add_point_to_ring(bb, top, b, manager);
            }
            next_edge_in_bound(&mut b[bb], scanbeam);
        }
    }
    Ok(())
}
