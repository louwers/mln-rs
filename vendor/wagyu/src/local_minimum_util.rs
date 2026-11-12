// Ports vendor/wagyu/include/mapbox/geometry/wagyu/local_minimum_util.hpp and
// build_local_minima_list.hpp.

use std::cmp::Ordering;

use crate::bound::{Bound, BoundPtr, EdgeSide};
use crate::build_edges::build_edge_list;
use crate::config::PolygonType;
use crate::edge::{Edge, is_horizontal};
use crate::local_minimum::LocalMinimum;
use crate::point::{GPoint, NIL};
use crate::ring::R;

/// `reverse_horizontal`: swap horizontal edges' top and bottom x's so they
/// follow the natural progression of the bounds - ie so their xbots will
/// align with the adjoining lower edge.
fn reverse_horizontal(e: &mut Edge) {
    std::mem::swap(&mut e.top.x, &mut e.bot.x);
}

/// `start_list_on_local_maximum`: make a list start on a local maximum by
/// shifting all the points not on a local maximum to the end.
fn start_list_on_local_maximum(edges: &mut [Edge]) {
    if edges.len() <= 2 {
        return;
    }
    // Find the first local maximum going forward in the list
    let mut prev_edge = edges.len() - 1;
    let mut prev_edge_is_horizontal = is_horizontal(&edges[prev_edge]);
    let mut edge = 0;
    let mut y_decreasing_before_last_horizontal = false; // assume false at start

    while edge != edges.len() {
        let edge_is_horizontal = is_horizontal(&edges[edge]);
        if !prev_edge_is_horizontal && !edge_is_horizontal && edges[edge].top == edges[prev_edge].top {
            break;
        }
        if !edge_is_horizontal && prev_edge_is_horizontal {
            if y_decreasing_before_last_horizontal
                && (edges[edge].top == edges[prev_edge].bot || edges[edge].top == edges[prev_edge].top)
            {
                break;
            }
        } else if !y_decreasing_before_last_horizontal
            && !prev_edge_is_horizontal
            && edge_is_horizontal
            && (edges[prev_edge].top == edges[edge].top || edges[prev_edge].top == edges[edge].bot)
        {
            y_decreasing_before_last_horizontal = true;
        }
        prev_edge_is_horizontal = edge_is_horizontal;
        prev_edge = edge;
        edge += 1;
    }
    edges.rotate_left(edge);
}

/// `create_bound_towards_minimum`.
fn create_bound_towards_minimum(edges: &mut Vec<Edge>) -> Bound {
    if edges.len() == 1 {
        if is_horizontal(&edges[0]) {
            reverse_horizontal(&mut edges[0]);
        }
        let mut bnd = Bound::new();
        std::mem::swap(&mut bnd.edges, edges);
        return bnd;
    }
    let mut next_edge = 1;
    let mut edge = 0;
    let mut edge_is_horizontal = is_horizontal(&edges[edge]);
    if edge_is_horizontal {
        reverse_horizontal(&mut edges[edge]);
    }
    let mut y_increasing_before_last_horizontal = false; // assume false at start

    while next_edge != edges.len() {
        let next_edge_is_horizontal = is_horizontal(&edges[next_edge]);
        if !next_edge_is_horizontal && !edge_is_horizontal && edges[edge].bot == edges[next_edge].bot {
            break;
        }
        if !next_edge_is_horizontal && edge_is_horizontal {
            if y_increasing_before_last_horizontal
                && (edges[next_edge].bot == edges[edge].bot || edges[next_edge].bot == edges[edge].top)
            {
                break;
            }
        } else if !y_increasing_before_last_horizontal
            && !edge_is_horizontal
            && next_edge_is_horizontal
            && (edges[edge].bot == edges[next_edge].top || edges[edge].bot == edges[next_edge].bot)
        {
            y_increasing_before_last_horizontal = true;
        }
        edge_is_horizontal = next_edge_is_horizontal;
        edge = next_edge;
        if edge_is_horizontal {
            reverse_horizontal(&mut edges[edge]);
        }
        next_edge += 1;
    }
    let mut bnd = Bound::new();
    if next_edge == edges.len() {
        std::mem::swap(edges, &mut bnd.edges);
    } else {
        bnd.edges = edges.drain(..next_edge).collect();
    }
    bnd.edges.reverse();
    bnd
}

/// `create_bound_towards_maximum`.
fn create_bound_towards_maximum(edges: &mut Vec<Edge>) -> Bound {
    if edges.len() == 1 {
        let mut bnd = Bound::new();
        std::mem::swap(&mut bnd.edges, edges);
        return bnd;
    }
    let mut next_edge = 1;
    let mut edge = 0;
    let mut edge_is_horizontal = is_horizontal(&edges[edge]);
    let mut y_decreasing_before_last_horizontal = false; // assume false at start

    while next_edge != edges.len() {
        let next_edge_is_horizontal = is_horizontal(&edges[next_edge]);
        if !next_edge_is_horizontal && !edge_is_horizontal && edges[edge].top == edges[next_edge].top {
            break;
        }
        if !next_edge_is_horizontal && edge_is_horizontal {
            if y_decreasing_before_last_horizontal
                && (edges[next_edge].top == edges[edge].bot || edges[next_edge].top == edges[edge].top)
            {
                break;
            }
        } else if !y_decreasing_before_last_horizontal
            && !edge_is_horizontal
            && next_edge_is_horizontal
            && (edges[edge].top == edges[next_edge].top || edges[edge].top == edges[next_edge].bot)
        {
            y_decreasing_before_last_horizontal = true;
        }
        edge_is_horizontal = next_edge_is_horizontal;
        edge = next_edge;
        next_edge += 1;
    }
    let mut bnd = Bound::new();
    if next_edge == edges.len() {
        std::mem::swap(&mut bnd.edges, edges);
    } else {
        bnd.edges = edges.drain(..next_edge).collect();
    }
    bnd
}

/// `fix_horizontals`.
fn fix_horizontals(bnd: &mut Bound) {
    if bnd.edges.len() < 2 {
        return;
    }
    let mut edge_itr = 0;
    let next_itr = 1;
    if is_horizontal(&bnd.edges[edge_itr]) && bnd.edges[next_itr].bot != bnd.edges[edge_itr].top {
        reverse_horizontal(&mut bnd.edges[edge_itr]);
    }
    let mut prev_itr = edge_itr;
    edge_itr += 1;
    while edge_itr != bnd.edges.len() {
        if is_horizontal(&bnd.edges[edge_itr]) && bnd.edges[prev_itr].top != bnd.edges[edge_itr].bot {
            reverse_horizontal(&mut bnd.edges[edge_itr]);
        }
        prev_itr = edge_itr;
        edge_itr += 1;
    }
}

/// `move_horizontals_on_left_to_right`: we want all the horizontal segments
/// that are at the same Y as the minimum to be on the right bound.
fn move_horizontals_on_left_to_right(left_bound: &mut Bound, right_bound: &mut Bound) {
    let mut edge_itr = 0;
    while edge_itr != left_bound.edges.len() {
        if !is_horizontal(&left_bound.edges[edge_itr]) {
            break;
        }
        reverse_horizontal(&mut left_bound.edges[edge_itr]);
        edge_itr += 1;
    }
    if edge_itr == 0 {
        return;
    }
    left_bound.edges[..edge_itr].reverse();
    let dist = edge_itr;
    right_bound.edges.extend(left_bound.edges.drain(..edge_itr));
    right_bound.edges.rotate_right(dist);
}

/// `add_ring_to_local_minima_list`: the bounds go into the arena `bounds`.
fn add_ring_to_local_minima_list(
    edges: &mut Vec<Edge>,
    minima_list: &mut Vec<LocalMinimum>,
    bounds: &mut Vec<Bound>,
    poly_type: PolygonType,
) -> R<()> {
    if edges.is_empty() {
        return Ok(());
    }
    // Adjust the order of the ring so we start on a local maximum
    // therefore we start right away on a bound.
    start_list_on_local_maximum(edges);

    let mut first_minimum: BoundPtr = NIL;
    let mut last_maximum: BoundPtr = NIL;
    while !edges.is_empty() {
        let mut lm_minimum_has_horizontal = false;
        let mut to_minimum = create_bound_towards_minimum(edges);
        if edges.is_empty() {
            return Err("Edges is empty after only creating a single bound.".to_owned());
        }
        let mut to_maximum = create_bound_towards_maximum(edges);
        fix_horizontals(&mut to_minimum);
        fix_horizontals(&mut to_maximum);
        let mut to_max_first_non_horizontal = 0;
        let mut to_min_first_non_horizontal = 0;
        let minimum_is_left;
        while to_max_first_non_horizontal != to_maximum.edges.len()
            && is_horizontal(&to_maximum.edges[to_max_first_non_horizontal])
        {
            lm_minimum_has_horizontal = true;
            to_max_first_non_horizontal += 1;
        }
        while to_min_first_non_horizontal != to_minimum.edges.len()
            && is_horizontal(&to_minimum.edges[to_min_first_non_horizontal])
        {
            lm_minimum_has_horizontal = true;
            to_min_first_non_horizontal += 1;
        }

        if to_max_first_non_horizontal == to_maximum.edges.len()
            || to_min_first_non_horizontal == to_minimum.edges.len()
        {
            return Err("should not have a horizontal only bound for a ring".to_owned());
        }

        if lm_minimum_has_horizontal {
            if to_maximum.edges[to_max_first_non_horizontal].bot.x > to_minimum.edges[to_min_first_non_horizontal].bot.x
            {
                minimum_is_left = true;
                move_horizontals_on_left_to_right(&mut to_minimum, &mut to_maximum);
            } else {
                minimum_is_left = false;
                move_horizontals_on_left_to_right(&mut to_maximum, &mut to_minimum);
            }
        } else {
            let max_dx = to_maximum.edges[to_max_first_non_horizontal].dx;
            let min_dx = to_minimum.edges[to_min_first_non_horizontal].dx;
            minimum_is_left = max_dx.partial_cmp(&min_dx) != Some(Ordering::Greater);
        }
        debug_assert!(!to_minimum.edges.is_empty());
        debug_assert!(!to_maximum.edges.is_empty());
        let min_front_bot_y = to_minimum.edges[0].bot.y;
        if last_maximum != NIL {
            to_minimum.maximum_bound = last_maximum;
        }
        to_minimum.poly_type = poly_type;
        to_maximum.poly_type = poly_type;
        if minimum_is_left {
            to_minimum.side = EdgeSide::Left;
            to_maximum.side = EdgeSide::Right;
            to_minimum.winding_delta = -1;
            to_maximum.winding_delta = 1;
            bounds.push(to_minimum);
            let left = bounds.len() - 1;
            bounds.push(to_maximum);
            let right = bounds.len() - 1;
            minima_list.push(LocalMinimum {
                left_bound: left,
                right_bound: right,
                y: min_front_bot_y,
                minimum_has_horizontal: lm_minimum_has_horizontal,
            });
            if last_maximum == NIL {
                first_minimum = left;
            } else {
                bounds[last_maximum].maximum_bound = left;
            }
            last_maximum = right;
        } else {
            to_minimum.side = EdgeSide::Right;
            to_maximum.side = EdgeSide::Left;
            to_minimum.winding_delta = -1;
            to_maximum.winding_delta = 1;
            bounds.push(to_maximum);
            let left = bounds.len() - 1;
            bounds.push(to_minimum);
            let right = bounds.len() - 1;
            minima_list.push(LocalMinimum {
                left_bound: left,
                right_bound: right,
                y: min_front_bot_y,
                minimum_has_horizontal: lm_minimum_has_horizontal,
            });
            if last_maximum == NIL {
                first_minimum = right;
            } else {
                bounds[last_maximum].maximum_bound = right;
            }
            last_maximum = left;
        }
    }
    bounds[last_maximum].maximum_bound = first_minimum;
    bounds[first_minimum].maximum_bound = last_maximum;
    Ok(())
}

/// `initialize_lm`.
pub(crate) fn initialize_lm(lm: &LocalMinimum, bounds: &mut [Bound]) {
    for (b, side) in [(lm.left_bound, EdgeSide::Left), (lm.right_bound, EdgeSide::Right)] {
        let bnd = &mut bounds[b];
        if !bnd.edges.is_empty() {
            bnd.current_edge = 0;
            bnd.next_edge = 1;
            bnd.current_x = f64::from(bnd.edges[0].bot.x);
            bnd.winding_count = 0;
            bnd.winding_count2 = 0;
            bnd.side = side;
            bnd.ring = NIL;
        }
    }
}

/// `add_linear_ring` (`build_local_minima_list.hpp`).
pub(crate) fn add_linear_ring(
    path_geometry: &[GPoint],
    minima_list: &mut Vec<LocalMinimum>,
    bounds: &mut Vec<Bound>,
    p_type: PolygonType,
) -> R<bool> {
    let mut new_edges = Vec::with_capacity(path_geometry.len());
    if !build_edge_list(path_geometry, &mut new_edges) || new_edges.is_empty() {
        return Ok(false);
    }
    add_ring_to_local_minima_list(&mut new_edges, minima_list, bounds, p_type)?;
    Ok(true)
}
