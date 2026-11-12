// Ports vendor/wagyu/include/mapbox/geometry/wagyu/process_horizontal.hpp.
// Reverse iterators over the active bounds and the hot pixels are kept as
// the index of the element they point at (-1 = rend()).

use crate::active_bound_list::{current_edge_is_horizontal, get_maxima_pair, is_maxima, next_edge_in_bound};
use crate::bound::{Bound, BoundPtr};
use crate::config::{ClipType, FillType};
use crate::intersect_util::intersect_bounds;
use crate::point::{GPoint, NIL};
use crate::ring::RingManager;
use crate::ring_util::{add_local_maximum_point, add_point_to_ring};
use crate::util::{greater_than, less_than, wround};

/// `process_horizontal_left_to_right`.
#[allow(clippy::too_many_arguments)]
fn process_horizontal_left_to_right(
    scanline_y: i32,
    mut horz_bound: usize,
    active_bounds: &mut [BoundPtr],
    b: &mut [Bound],
    rings: &mut RingManager,
    scanbeam: &mut Vec<i32>,
    cliptype: ClipType,
    subject_fill_type: FillType,
    clip_fill_type: FillType,
) -> usize {
    let h = active_bounds[horz_bound];
    let mut horizontal_itr_behind = horz_bound;
    let mut shifted = false;
    let is_maxima_edge = is_maxima(&b[h], scanline_y);
    let mut bound_max_pair = active_bounds.len();
    if is_maxima_edge {
        bound_max_pair = get_maxima_pair(horz_bound, active_bounds, b);
    }

    let hp_len = rings.hot_pixels.len();
    let mut hp_itr = rings.current_hp_itr;
    while hp_itr != hp_len
        && (rings.hot_pixels[hp_itr].y > scanline_y
            || (rings.hot_pixels[hp_itr].y == scanline_y && rings.hot_pixels[hp_itr].x < b[h].cur().bot.x))
    {
        hp_itr += 1;
    }

    let mut bnd = horz_bound + 1;

    while bnd != active_bounds.len() {
        let bb = active_bounds[bnd];
        if bb == NIL {
            bnd += 1;
            continue;
        }
        // this code block inserts extra coords into horizontal edges (in output
        // polygons) wherever hot pixels touch these horizontal edges. This helps
        //'simplifying' polygons (ie if the Simplify property is set).
        while hp_itr != hp_len
            && rings.hot_pixels[hp_itr].y == scanline_y
            && rings.hot_pixels[hp_itr].x < wround(b[bb].current_x)
            && rings.hot_pixels[hp_itr].x < b[h].cur().top.x
        {
            if b[h].ring != NIL {
                let hp = rings.hot_pixels[hp_itr];
                add_point_to_ring(h, hp, b, rings);
            }
            hp_itr += 1;
        }
        if greater_than(b[bb].current_x, f64::from(b[h].cur().top.x)) {
            break;
        }
        // Also break if we've got to the end of an intermediate horizontal edge ...
        // nb: Smaller Dx's are to the right of larger Dx's ABOVE the horizontal.
        if wround(b[bb].current_x) == b[h].cur().top.x
            && b[h].next_edge != b[h].edges.len()
            && b[h].cur().dx < b[h].next().dx
        {
            break;
        }

        // note: may be done multiple times
        if b[h].ring != NIL {
            let pt = GPoint::new(wround(b[bb].current_x), scanline_y);
            add_point_to_ring(h, pt, b, rings);
        }

        // OK, so far we're still in range of the horizontal Edge  but make sure
        // we're at the last of consec. horizontals when matching with eMaxPair
        if is_maxima_edge && bnd == bound_max_pair {
            let mp = active_bounds[bound_max_pair];
            if b[h].ring != NIL && b[mp].ring != NIL {
                let top = b[h].cur().top;
                add_local_maximum_point(h, mp, top, b, rings, active_bounds);
            }
            active_bounds[bound_max_pair] = NIL;
            active_bounds[horz_bound] = NIL;
            if !shifted {
                horizontal_itr_behind += 1;
            }
            return horizontal_itr_behind;
        }

        let pt = GPoint::new(wround(b[bb].current_x), scanline_y);
        intersect_bounds(h, bb, pt, cliptype, subject_fill_type, clip_fill_type, b, rings, active_bounds);
        active_bounds.swap(horz_bound, bnd);
        horz_bound = bnd;
        bnd += 1;
        shifted = true;
    } // end while (bnd != active_bounds.end())

    if b[h].ring != NIL {
        while hp_itr != hp_len
            && rings.hot_pixels[hp_itr].y == scanline_y
            && rings.hot_pixels[hp_itr].x < b[h].cur().top.x
        {
            let hp = rings.hot_pixels[hp_itr];
            add_point_to_ring(h, hp, b, rings);
            hp_itr += 1;
        }
    }

    if b[h].ring != NIL {
        let top = b[h].cur().top;
        add_point_to_ring(h, top, b, rings);
    }

    if b[h].next_edge == b[h].edges.len() {
        active_bounds[horz_bound] = NIL;
    } else {
        next_edge_in_bound(&mut b[h], scanbeam);
    }

    if !shifted {
        horizontal_itr_behind += 1;
    }
    horizontal_itr_behind
}

/// `process_horizontal_right_to_left`.
#[allow(clippy::too_many_arguments)]
fn process_horizontal_right_to_left(
    scanline_y: i32,
    horz_bound_fwd: usize,
    active_bounds: &mut [BoundPtr],
    b: &mut [Bound],
    rings: &mut RingManager,
    scanbeam: &mut Vec<i32>,
    cliptype: ClipType,
    subject_fill_type: FillType,
    clip_fill_type: FillType,
) -> usize {
    let h = active_bounds[horz_bound_fwd];
    let next_bnd_itr = horz_bound_fwd + 1;
    let is_maxima_edge = is_maxima(&b[h], scanline_y);
    // rend() is -1; a reverse iterator made from end() and decremented is len.
    let mut bound_max_pair: isize = -1;
    if is_maxima_edge {
        bound_max_pair = get_maxima_pair(horz_bound_fwd, active_bounds, b) as isize;
    }
    let hp_len = rings.hot_pixels.len();
    let mut hp_itr_fwd = rings.current_hp_itr;
    while hp_itr_fwd != hp_len
        && (rings.hot_pixels[hp_itr_fwd].y < scanline_y
            || (rings.hot_pixels[hp_itr_fwd].y == scanline_y && rings.hot_pixels[hp_itr_fwd].x < b[h].cur().top.x))
    {
        hp_itr_fwd += 1;
    }
    let mut hp_itr: isize = hp_itr_fwd as isize - 1;

    let mut bnd: isize = horz_bound_fwd as isize - 1;
    let mut horz_bound: isize = horz_bound_fwd as isize;
    while bnd >= 0 {
        let bb = active_bounds[bnd as usize];
        if bb == NIL {
            bnd -= 1;
            continue;
        }
        // this code block inserts extra coords into horizontal edges (in output
        // polygons) wherever hot pixels touch these horizontal edges.
        while hp_itr >= 0
            && rings.hot_pixels[hp_itr as usize].y == scanline_y
            && rings.hot_pixels[hp_itr as usize].x > wround(b[bb].current_x)
            && rings.hot_pixels[hp_itr as usize].x > b[h].cur().top.x
        {
            if b[h].ring != NIL {
                let hp = rings.hot_pixels[hp_itr as usize];
                add_point_to_ring(h, hp, b, rings);
            }
            hp_itr -= 1;
        }

        if less_than(b[bb].current_x, f64::from(b[h].cur().top.x)) {
            break;
        }

        // Also break if we've got to the end of an intermediate horizontal edge ...
        // nb: Smaller Dx's are to the right of larger Dx's ABOVE the horizontal.
        if wround(b[bb].current_x) == b[h].cur().top.x
            && b[h].next_edge != b[h].edges.len()
            && b[h].cur().dx < b[h].next().dx
        {
            break;
        }

        // note: may be done multiple times
        if b[h].ring != NIL {
            let pt = GPoint::new(wround(b[bb].current_x), scanline_y);
            add_point_to_ring(h, pt, b, rings);
        }

        // OK, so far we're still in range of the horizontal Edge  but make sure
        // we're at the last of consec. horizontals when matching with eMaxPair
        if is_maxima_edge && bnd == bound_max_pair {
            let mp = active_bounds[bound_max_pair as usize];
            if b[h].ring != NIL && b[mp].ring != NIL {
                let top = b[h].cur().top;
                add_local_maximum_point(h, mp, top, b, rings, active_bounds);
            }
            active_bounds[bound_max_pair as usize] = NIL;
            active_bounds[horz_bound as usize] = NIL;
            return next_bnd_itr;
        }

        let pt = GPoint::new(wround(b[bb].current_x), scanline_y);
        intersect_bounds(bb, h, pt, cliptype, subject_fill_type, clip_fill_type, b, rings, active_bounds);
        active_bounds.swap(horz_bound as usize, bnd as usize);
        horz_bound = bnd;
        bnd -= 1;
    } // end while (bnd != active_bounds.rend())

    if b[h].ring != NIL {
        while hp_itr >= 0
            && rings.hot_pixels[hp_itr as usize].y == scanline_y
            && rings.hot_pixels[hp_itr as usize].x > b[h].cur().top.x
        {
            let hp = rings.hot_pixels[hp_itr as usize];
            add_point_to_ring(h, hp, b, rings);
            hp_itr -= 1;
        }
    }

    if b[h].ring != NIL {
        let top = b[h].cur().top;
        add_point_to_ring(h, top, b, rings);
    }

    if b[h].next_edge == b[h].edges.len() {
        active_bounds[horz_bound as usize] = NIL;
    } else {
        next_edge_in_bound(&mut b[h], scanbeam);
    }
    next_bnd_itr
}

/// `process_horizontal`.
#[allow(clippy::too_many_arguments)]
fn process_horizontal(
    scanline_y: i32,
    horz_bound: usize,
    active_bounds: &mut [BoundPtr],
    b: &mut [Bound],
    rings: &mut RingManager,
    scanbeam: &mut Vec<i32>,
    cliptype: ClipType,
    subject_fill_type: FillType,
    clip_fill_type: FillType,
) -> usize {
    let h = active_bounds[horz_bound];
    if b[h].cur().bot.x < b[h].cur().top.x {
        process_horizontal_left_to_right(
            scanline_y,
            horz_bound,
            active_bounds,
            b,
            rings,
            scanbeam,
            cliptype,
            subject_fill_type,
            clip_fill_type,
        )
    } else {
        process_horizontal_right_to_left(
            scanline_y,
            horz_bound,
            active_bounds,
            b,
            rings,
            scanbeam,
            cliptype,
            subject_fill_type,
            clip_fill_type,
        )
    }
}

/// `process_horizontals`.
#[allow(clippy::too_many_arguments)]
pub(crate) fn process_horizontals(
    scanline_y: i32,
    active_bounds: &mut Vec<BoundPtr>,
    b: &mut [Bound],
    rings: &mut RingManager,
    scanbeam: &mut Vec<i32>,
    cliptype: ClipType,
    subject_fill_type: FillType,
    clip_fill_type: FillType,
) {
    let mut bnd_itr = 0;
    while bnd_itr != active_bounds.len() {
        let bb = active_bounds[bnd_itr];
        if bb != NIL && current_edge_is_horizontal(&b[bb]) {
            bnd_itr = process_horizontal(
                scanline_y,
                bnd_itr,
                active_bounds,
                b,
                rings,
                scanbeam,
                cliptype,
                subject_fill_type,
                clip_fill_type,
            );
        } else {
            bnd_itr += 1;
        }
    }
    active_bounds.retain(|&x| x != NIL);
}
