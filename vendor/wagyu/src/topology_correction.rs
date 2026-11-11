// Ports vendor/wagyu/include/mapbox/geometry/wagyu/topology_correction.hpp.
//
// `std::unordered_multimap<ring_ptr, point_ptr_pair>`: the code only ever
// visits the elements of one key (`equal_range`), so the bucket order (by
// pointer hash) never matters; what matters is the order inside a key,
// which in libstdc++ puts every newly inserted element *before* the
// existing equivalent ones (`_M_insert_multi_node` without a hint) and is
// kept by rehashing. `ConnectionMap` stores each key's run in that order.
// `std::set<ring_ptr>` is only used for membership. Every
// `for (auto c : ring->children)` captures the vector's length first, as the
// range-for captures end().

use std::cmp::{Ordering, Reverse};
use std::collections::{HashMap, HashSet, VecDeque};

use crate::point::{NIL, PointPtr, RingPtr};
use crate::ring::{
    R, RingManager, assign_as_child, assign_as_sibling, create_new_ring, reassign_as_child, remove_ring,
    remove_ring_and_points, reverse_ring, ring_depth, ring_is_hole, ring1_replaces_ring2,
};
use crate::ring_util::{poly2_contains_poly1, update_points_ring};
use crate::util::value_is_zero;

/// `point_ptr_pair<int32_t>`.
#[derive(Clone, Copy, Debug)]
struct PointPtrPair {
    op1: PointPtr,
    op2: PointPtr,
}

/// `std::unordered_multimap<ring_ptr<T>, point_ptr_pair<T>>` (see the file
/// comment).
#[derive(Default)]
struct ConnectionMap {
    runs: HashMap<RingPtr, Vec<PointPtrPair>>,
}

impl ConnectionMap {
    /// `emplace` / `insert`: before the existing elements of the key.
    fn emplace(&mut self, key: RingPtr, value: PointPtrPair) {
        self.runs.entry(key).or_default().insert(0, value);
    }

    fn len(&self, key: RingPtr) -> usize {
        self.runs.get(&key).map_or(0, Vec::len)
    }

    fn get(&self, key: RingPtr, i: usize) -> PointPtrPair {
        self.runs[&key][i]
    }

    /// `erase(iterator)` of the i-th element of the key.
    fn erase_at(&mut self, key: RingPtr, i: usize) {
        let run = self.runs.get_mut(&key).expect("key present");
        run.remove(i);
        if run.is_empty() {
            self.runs.remove(&key);
        }
    }

    /// `erase(key)`.
    fn erase_key(&mut self, key: RingPtr) {
        self.runs.remove(&key);
    }
}

type IList = VecDeque<(RingPtr, PointPtrPair)>;

/// `find_intersect_loop`.
#[allow(clippy::too_many_arguments)]
fn find_intersect_loop(
    dupe_ring: &mut ConnectionMap,
    i_list: &mut IList,
    ring_parent: RingPtr,
    ring_origin: RingPtr,
    ring_search: RingPtr,
    visited: &mut HashSet<RingPtr>,
    orig_pt: PointPtr,
    prev_pt: PointPtr,
    rings: &mut RingManager,
) -> bool {
    {
        // Check for direct connection
        let mut i = 0;
        while i < dupe_ring.len(ring_search) {
            let second = dupe_ring.get(ring_search, i);
            let it_ring1 = rings.ring_of(second.op1);
            let it_ring2 = rings.ring_of(second.op2);
            if it_ring1 == NIL
                || it_ring2 == NIL
                || it_ring1 != ring_search
                || (!rings.is_hole(it_ring1) && !rings.is_hole(it_ring2))
            {
                dupe_ring.erase_at(ring_search, i);
                continue;
            }
            if it_ring2 == ring_origin
                && (ring_parent == it_ring2 || ring_parent == rings.parent(it_ring2))
                && !rings.pt_eq(prev_pt, second.op2)
                && !rings.pt_eq(orig_pt, second.op2)
            {
                i_list.push_front((ring_search, second));
                return true;
            }
            i += 1;
        }
    }
    visited.insert(ring_search);
    // Check for connection through chain of other intersections
    let mut i = 0;
    while i < dupe_ring.len(ring_search) {
        let second = dupe_ring.get(ring_search, i);
        let it_ring = rings.ring_of(second.op2);
        if visited.contains(&it_ring)
            || it_ring == NIL
            || (ring_parent != it_ring && ring_parent != rings.parent(it_ring))
            || value_is_zero(rings.area(it_ring))
            || rings.pt_eq(prev_pt, second.op2)
        {
            i += 1;
            continue;
        }
        if find_intersect_loop(
            dupe_ring,
            i_list,
            ring_parent,
            ring_origin,
            it_ring,
            visited,
            orig_pt,
            second.op2,
            rings,
        ) {
            i_list.push_front((ring_search, second));
            return true;
        }
        i += 1;
    }
    false
}

/// `point_ptr_cmp`: by descending y, ascending x, then descending ring
/// depth.
fn point_ptr_cmp(rings: &RingManager, op1: PointPtr, op2: PointPtr) -> Ordering {
    let (x1, y1) = rings.xy(op1);
    let (x2, y2) = rings.xy(op2);
    y2.cmp(&y1)
        .then(x1.cmp(&x2))
        .then_with(|| ring_depth(rings.ring_of(op2), rings).cmp(&ring_depth(rings.ring_of(op1), rings)))
}

/// `correct_orientations`.
fn correct_orientations(manager: &mut RingManager) {
    for r in 0..manager.rings.len() {
        if manager.rings[r].points == NIL {
            continue;
        }
        manager.recalculate_stats(r);
        if manager.size(r) < 3 {
            remove_ring_and_points(r, manager, false, true);
            continue;
        }
        if ring_is_hole(r, manager) != manager.is_hole(r) {
            let points = manager.rings[r].points;
            reverse_ring(points, manager);
            manager.recalculate_stats(r);
        }
    }
}

/// `sort_ring_points`.
fn sort_ring_points(r: RingPtr, rings: &RingManager) -> Vec<PointPtr> {
    let mut sorted_points = Vec::new();
    let mut point_itr = rings.rings[r].points;
    let last_point = rings.prev(point_itr);
    while point_itr != last_point {
        sorted_points.push(point_itr);
        point_itr = rings.next(point_itr);
    }
    sorted_points.push(last_point);
    sorted_points.sort_by_key(|&pt| {
        let (x, y) = rings.xy(pt);
        (Reverse(y), x)
    });
    sorted_points
}

/// `correct_self_intersection`: splits the ring at two points of the same
/// position; returns the new ring or NIL.
fn correct_self_intersection(pt1: PointPtr, pt2: PointPtr, manager: &mut RingManager) -> RingPtr {
    if manager.ring_of(pt1) != manager.ring_of(pt2) {
        return NIL;
    }

    let ring = manager.ring_of(pt1);

    // split the polygon into two ...
    let pt3 = manager.prev(pt1);
    let pt4 = manager.prev(pt2);
    manager.points[pt1].prev = pt4;
    manager.points[pt4].next = pt1;
    manager.points[pt2].prev = pt3;
    manager.points[pt3].next = pt2;

    let new_ring = create_new_ring(manager);
    let (area_1, size_1, box1) = manager.area_from_point(pt1);
    let (area_2, size_2, box2) = manager.area_from_point(pt2);
    if area_1.abs() > area_2.abs() {
        manager.rings[ring].points = pt1;
        manager.set_stats(ring, area_1, size_1, box1);
        manager.rings[new_ring].points = pt2;
        manager.set_stats(new_ring, area_2, size_2, box2);
    } else {
        manager.rings[ring].points = pt2;
        manager.set_stats(ring, area_2, size_2, box2);
        manager.rings[new_ring].points = pt1;
        manager.set_stats(new_ring, area_1, size_1, box1);
    }
    update_points_ring(new_ring, manager);
    new_ring
}

/// `correct_repeated_points`.
fn correct_repeated_points(manager: &mut RingManager, new_rings: &mut Vec<RingPtr>, points: &[PointPtr]) {
    for itr1 in 0..points.len() {
        if manager.ring_of(points[itr1]) == NIL {
            continue;
        }
        for itr2 in itr1 + 1..points.len() {
            if manager.ring_of(points[itr2]) == NIL {
                continue;
            }
            let new_ring = correct_self_intersection(points[itr1], points[itr2], manager);
            if new_ring != NIL {
                new_rings.push(new_ring);
            }
        }
    }
}

/// The run detection shared by `find_and_correct_repeated_points`,
/// `correct_chained_rings` and `correct_collinear_edges`: calls `f` with the
/// [first, last) index range of every run of 2+ points at the same position.
fn for_each_repeated_run<F: FnMut(&mut RingManager, usize, usize) -> R<()>>(
    manager: &mut RingManager,
    points: &[PointPtr],
    mut f: F,
) -> R<()> {
    let mut count: usize = 0;
    let mut prev_itr = 0;
    let mut itr = 1;
    while itr < points.len() {
        if manager.pt_eq(points[prev_itr], points[itr]) {
            count += 1;
            prev_itr += 1;
            itr += 1;
            if itr != points.len() {
                continue;
            } else {
                prev_itr += 1;
            }
        } else {
            prev_itr += 1;
            itr += 1;
        }
        if count == 0 {
            continue;
        }
        let first = prev_itr - (count + 1);
        f(manager, first, prev_itr)?;
        count = 0;
    }
    Ok(())
}

/// `find_and_correct_repeated_points`.
fn find_and_correct_repeated_points(r: RingPtr, manager: &mut RingManager, new_rings: &mut Vec<RingPtr>) {
    let sorted_points = sort_ring_points(r, manager);
    // Find sets of repeated points
    let _ = for_each_repeated_run(manager, &sorted_points, |m, first, last| {
        correct_repeated_points(m, new_rings, &sorted_points[first..last]);
        Ok(())
    });
}

/// `reassign_children_if_necessary`.
fn reassign_children_if_necessary(
    new_ring: RingPtr,
    sibling_ring: RingPtr,
    manager: &mut RingManager,
    new_rings: &[RingPtr],
) -> R<()> {
    let n = manager.children_of(sibling_ring).len();
    for i in 0..n {
        let c = manager.children_of(sibling_ring)[i];
        if c == NIL {
            continue;
        }
        if new_rings.contains(&c) {
            continue;
        }
        if poly2_contains_poly1(c, new_ring, manager)? {
            reassign_as_child(c, new_ring, manager)?;
        }
    }
    Ok(())
}

/// `find_parent_in_tree`.
fn find_parent_in_tree(r: RingPtr, possible_parent: RingPtr, manager: &mut RingManager) -> R<bool> {
    // Before starting this we are assuming that possible_parent
    // and r have opposite signs of their areas

    // First we must search all grandchildren
    let n = manager.rings[possible_parent].children.len();
    for i in 0..n {
        let c = manager.rings[possible_parent].children[i];
        if c == NIL {
            continue;
        }
        let m = manager.rings[c].children.len();
        for j in 0..m {
            let gc = manager.rings[c].children[j];
            if gc == NIL {
                continue;
            }
            if find_parent_in_tree(r, gc, manager)? {
                return Ok(true);
            }
        }
    }

    if poly2_contains_poly1(r, possible_parent, manager)? {
        reassign_as_child(r, possible_parent, manager)?;
        return Ok(true);
    }

    Ok(false)
}

/// `assign_new_ring_parents`.
fn assign_new_ring_parents(manager: &mut RingManager, original_ring: RingPtr, new_rings: &mut Vec<RingPtr>) -> R<()> {
    // First lets remove any rings that have zero area
    // or have no points
    new_rings.retain(|&r| {
        if manager.rings[r].points == NIL {
            return false;
        }
        !value_is_zero(manager.area(r))
    });

    if new_rings.is_empty() {
        // No new rings created simply return;
        return Ok(());
    }

    // We should not have to re-assign the parent of the original ring
    // because we always maintained the largest ring during splitting
    // on repeated points.
    let original_ring_area = manager.area(original_ring);
    let original_positive = original_ring_area > 0.0;

    // If there is only one new ring the logic is very simple and we
    // do not have to check which ring contains, we only need to compare
    // the areas of the original ring and that of the new ring.
    if new_rings.len() == 1 {
        let new_ring_area = manager.area(new_rings[0]);
        let new_positive = new_ring_area > 0.0;
        let original_parent = manager.parent(original_ring);
        if original_positive == new_positive {
            // The rings should be siblings
            assign_as_child(new_rings[0], original_parent, manager)?;
            reassign_children_if_necessary(new_rings[0], original_ring, manager, new_rings)?;
        } else {
            // The new ring is a child of original ring
            // Check the
            assign_as_child(new_rings[0], original_ring, manager)?;
            let original_parent = manager.parent(original_ring);
            reassign_children_if_necessary(new_rings[0], original_parent, manager, new_rings)?;
        }
        return Ok(());
    }

    // Now we want to sort rings from the largest in absolute area to the smallest
    // as we will assign the rings with the largest areas first
    new_rings.sort_by(|&r1, &r2| manager.area(r2).abs().total_cmp(&manager.area(r1).abs()));

    for r_itr in 0..new_rings.len() {
        let r = new_rings[r_itr];
        let new_ring_area = manager.area(r);
        let new_positive = new_ring_area > 0.0;
        let same_orientation = new_positive == original_positive;
        let mut found = false;
        // First lets check the trees of any new_rings that might have
        // been assigned as siblings to the original ring.
        for s_itr in 0..r_itr {
            let s = new_rings[s_itr];
            if manager.parent(s) != manager.parent(original_ring) {
                continue;
            }
            if same_orientation {
                let n = manager.rings[s].children.len();
                for i in 0..n {
                    let s_child = manager.rings[s].children[i];
                    if s_child == NIL {
                        continue;
                    }
                    if find_parent_in_tree(r, s_child, manager)? {
                        reassign_children_if_necessary(r, original_ring, manager, new_rings)?;
                        found = true;
                        break;
                    }
                }
            } else if find_parent_in_tree(r, s, manager)? {
                let original_parent = manager.parent(original_ring);
                reassign_children_if_necessary(r, original_parent, manager, new_rings)?;
                found = true;
            }
            if found {
                break;
            }
        }

        if found {
            continue;
        }

        // Next lets check the tree of the original_ring
        if same_orientation {
            let n = manager.rings[original_ring].children.len();
            for i in 0..n {
                let o_child = manager.rings[original_ring].children[i];
                if o_child == NIL {
                    continue;
                }
                if find_parent_in_tree(r, o_child, manager)? {
                    reassign_children_if_necessary(r, original_ring, manager, new_rings)?;
                    found = true;
                    break;
                }
            }
            if !found {
                // If we didn't find any parent and the same orientation
                // then it must be a sibling of the original ring
                let original_parent = manager.parent(original_ring);
                assign_as_child(r, original_parent, manager)?;
                reassign_children_if_necessary(r, original_ring, manager, new_rings)?;
            }
        } else if find_parent_in_tree(r, original_ring, manager)? {
            let original_parent = manager.parent(original_ring);
            reassign_children_if_necessary(r, original_parent, manager, new_rings)?;
        } else {
            return Err("Unable to find a proper parent ring".to_owned());
        }
    }
    Ok(())
}

/// `correct_ring_self_intersections`.
fn correct_ring_self_intersections(manager: &mut RingManager, r: RingPtr, correct_tree: bool) -> R<bool> {
    if manager.rings[r].corrected || manager.rings[r].points == NIL {
        return Ok(false);
    }

    let mut new_rings: Vec<RingPtr> = Vec::new();

    find_and_correct_repeated_points(r, manager, &mut new_rings);

    if correct_tree {
        assign_new_ring_parents(manager, r, &mut new_rings)?;
    }

    manager.rings[r].corrected = true;
    Ok(true)
}

/// `process_single_intersection`.
fn process_single_intersection(
    connection_map: &mut ConnectionMap,
    op_j: PointPtr,
    op_k: PointPtr,
    manager: &mut RingManager,
) -> R<()> {
    let ring_j = manager.ring_of(op_j);
    let ring_k = manager.ring_of(op_k);
    if ring_j == ring_k {
        return Ok(());
    }

    if !manager.is_hole(ring_j) && !manager.is_hole(ring_k) {
        // Both are not holes, return nothing to do.
        return Ok(());
    }

    let mut ring_origin;
    let ring_search;
    let mut ring_parent;
    let mut op_origin_1;
    let mut op_origin_2;

    if !manager.is_hole(ring_j) {
        ring_origin = ring_j;
        ring_parent = ring_origin;
        ring_search = ring_k;
        op_origin_1 = op_j;
        op_origin_2 = op_k;
    } else if !manager.is_hole(ring_k) {
        ring_origin = ring_k;
        ring_parent = ring_origin;
        ring_search = ring_j;
        op_origin_1 = op_k;
        op_origin_2 = op_j;
    } else {
        // both are holes
        // Order doesn't matter
        ring_origin = ring_j;
        ring_parent = manager.parent(ring_origin);
        ring_search = ring_k;
        op_origin_1 = op_j;
        op_origin_2 = op_k;
    }
    if ring_parent != manager.parent(ring_search) {
        // The two holes do not have the same parent, do not add them
        // simply return!
        return Ok(());
    }
    let mut found = false;
    let mut i_list: IList = VecDeque::new();
    {
        // Check for direct connection
        let mut i = 0;
        while i < connection_map.len(ring_search) {
            let second = connection_map.get(ring_search, i);
            if manager.ring_of(second.op1) == NIL {
                connection_map.erase_at(ring_search, i);
                continue;
            }
            if manager.ring_of(second.op2) == NIL {
                connection_map.erase_at(ring_search, i);
                continue;
            }
            let it_ring2 = manager.ring_of(second.op2);
            if it_ring2 == ring_origin {
                found = true;
                if !manager.pt_eq(op_origin_1, second.op2) {
                    i_list.push_back((ring_search, second));
                    break;
                }
            }
            i += 1;
        }
    }
    if i_list.is_empty() {
        let mut visited: HashSet<RingPtr> = HashSet::new();
        visited.insert(ring_search);
        // Check for connection through chain of other intersections
        let mut i = 0;
        while i < connection_map.len(ring_search) {
            let second = connection_map.get(ring_search, i);
            let it_ring = manager.ring_of(second.op2);
            if it_ring != ring_search
                && !manager.pt_eq(op_origin_2, second.op2)
                && it_ring != NIL
                && (ring_parent == it_ring || ring_parent == manager.parent(it_ring))
                && !value_is_zero(manager.area(it_ring))
                && find_intersect_loop(
                    connection_map,
                    &mut i_list,
                    ring_parent,
                    ring_origin,
                    it_ring,
                    &mut visited,
                    op_origin_2,
                    second.op2,
                    manager,
                )
            {
                found = true;
                i_list.push_front((ring_search, second));
                break;
            }
            i += 1;
        }
    }
    if !found {
        connection_map.emplace(ring_origin, PointPtrPair { op1: op_origin_1, op2: op_origin_2 });
        connection_map.emplace(ring_search, PointPtrPair { op1: op_origin_2, op2: op_origin_1 });
        return Ok(());
    }

    if i_list.is_empty() {
        // The situation where both origin and search are holes might have a missing
        // search condition, we must check if a new pair must be added.
        let mut missing = true;
        // Check for direct connection
        for i in 0..connection_map.len(ring_origin) {
            let second = connection_map.get(ring_origin, i);
            let it_ring2 = manager.ring_of(second.op2);
            if it_ring2 == ring_search {
                missing = false;
            }
        }
        if missing {
            connection_map.emplace(ring_origin, PointPtrPair { op1: op_origin_1, op2: op_origin_2 });
        }
        return Ok(());
    }

    if manager.is_hole(ring_origin) {
        for i_ring in i_list.iter_mut() {
            let ring_itr = i_ring.0;
            if !manager.is_hole(ring_itr) {
                // Make the hole the origin!
                std::mem::swap(&mut op_origin_1, &mut i_ring.1.op1);
                std::mem::swap(&mut op_origin_2, &mut i_ring.1.op2);
                i_ring.0 = ring_origin;
                ring_origin = ring_itr;
                ring_parent = ring_origin;
                break;
            }
        }
    }
    let origin_is_hole = manager.is_hole(ring_origin);

    // Switch
    let op_origin_1_next = manager.next(op_origin_1);
    let op_origin_2_next = manager.next(op_origin_2);
    manager.points[op_origin_1].next = op_origin_2_next;
    manager.points[op_origin_2].next = op_origin_1_next;
    manager.points[op_origin_1_next].prev = op_origin_2;
    manager.points[op_origin_2_next].prev = op_origin_1;

    for i_ring in &i_list {
        let op_search_1 = i_ring.1.op1;
        let op_search_2 = i_ring.1.op2;
        let op_search_1_next = manager.next(op_search_1);
        let op_search_2_next = manager.next(op_search_2);
        manager.points[op_search_1].next = op_search_2_next;
        manager.points[op_search_2].next = op_search_1_next;
        manager.points[op_search_1_next].prev = op_search_2;
        manager.points[op_search_2_next].prev = op_search_1;
    }

    let ring_new = create_new_ring(manager);
    manager.rings[ring_origin].corrected = false;
    let (area_1, size_1, box1) = manager.area_from_point(op_origin_1);
    let (area_2, size_2, box2) = manager.area_from_point(op_origin_2);
    if origin_is_hole && (area_1 < 0.0) {
        manager.rings[ring_origin].points = op_origin_1;
        manager.set_stats(ring_origin, area_1, size_1, box1);
        manager.rings[ring_new].points = op_origin_2;
        manager.set_stats(ring_new, area_2, size_2, box2);
    } else {
        manager.rings[ring_origin].points = op_origin_2;
        manager.set_stats(ring_origin, area_2, size_2, box2);
        manager.rings[ring_new].points = op_origin_1;
        manager.set_stats(ring_new, area_1, size_1, box1);
    }

    update_points_ring(ring_origin, manager);
    update_points_ring(ring_new, manager);

    manager.rings[ring_origin].bottom_point = NIL;

    for i_ring in &i_list {
        let ring_itr = i_ring.0;
        manager.rings[ring_itr].bottom_point = NIL;
        if origin_is_hole {
            ring1_replaces_ring2(ring_origin, ring_itr, manager);
        } else {
            let parent = manager.parent(ring_origin);
            ring1_replaces_ring2(parent, ring_itr, manager);
        }
    }
    if origin_is_hole {
        assign_as_child(ring_new, ring_origin, manager)?;
        // The parent ring in this situation might need to give up children
        // to the new ring.
        let n = manager.rings[ring_parent].children.len();
        for i in 0..n {
            let c = manager.rings[ring_parent].children[i];
            if c == NIL {
                continue;
            }
            if poly2_contains_poly1(c, ring_new, manager)? {
                reassign_as_child(c, ring_new, manager)?;
            }
        }
    } else {
        // The new ring and the origin ring need to be siblings
        // however some children ring from the ring origin might
        // need to be re-assigned to the new ring
        assign_as_sibling(ring_new, ring_origin, manager)?;
        let n = manager.rings[ring_origin].children.len();
        for i in 0..n {
            let c = manager.rings[ring_origin].children[i];
            if c == NIL {
                continue;
            }
            if poly2_contains_poly1(c, ring_new, manager)? {
                reassign_as_child(c, ring_new, manager)?;
            }
        }
    }

    let mut move_list: Vec<(RingPtr, PointPtrPair)> = Vec::new();

    for i_ring in &i_list {
        let key = i_ring.0;
        if connection_map.len(key) > 0 {
            for i in 0..connection_map.len(key) {
                let second = connection_map.get(key, i);
                let it_ring = manager.ring_of(second.op1);
                let it_ring2 = manager.ring_of(second.op2);
                if it_ring == NIL || it_ring2 == NIL || it_ring == it_ring2 {
                    continue;
                }
                if manager.is_hole(it_ring) || manager.is_hole(it_ring2) {
                    move_list.push((it_ring, second));
                }
            }
            connection_map.erase_key(key);
        }
    }

    let mut i = 0;
    while i < connection_map.len(ring_origin) {
        let second = connection_map.get(ring_origin, i);
        let it_ring = manager.ring_of(second.op1);
        let it_ring2 = manager.ring_of(second.op2);
        if it_ring == NIL || it_ring2 == NIL || it_ring == it_ring2 {
            connection_map.erase_at(ring_origin, i);
            continue;
        }
        if it_ring != ring_origin {
            if manager.is_hole(it_ring) || manager.is_hole(it_ring2) {
                move_list.push((it_ring, second));
            }
            connection_map.erase_at(ring_origin, i);
        } else if manager.is_hole(it_ring) || manager.is_hole(it_ring2) {
            i += 1;
        } else {
            connection_map.erase_at(ring_origin, i);
        }
    }

    for (key, value) in move_list {
        connection_map.emplace(key, value);
    }
    Ok(())
}

/// `correct_chained_rings`.
fn correct_chained_rings(manager: &mut RingManager) -> R<()> {
    if manager.all_points.len() < 2 {
        return Ok(());
    }
    // Setup connection map which is a map of rings and their
    // connection point pairs with other rings.
    let mut connection_map = ConnectionMap::default();

    // Now lets find and process any points
    // that overlap -- we should have solved
    // all situations where these points
    // would be self intersections of a ring with
    // earlier processing so this should just be
    // points where different rings are touching.
    let all_points = manager.all_points.clone();
    for_each_repeated_run(manager, &all_points, |m, first, last| {
        // correct_chained_repeats
        for itr1 in first..last {
            if m.ring_of(all_points[itr1]) == NIL {
                continue;
            }
            for itr2 in itr1 + 1..last {
                if m.ring_of(all_points[itr2]) == NIL {
                    continue;
                }
                process_single_intersection(&mut connection_map, all_points[itr1], all_points[itr2], m)?;
            }
        }
        Ok(())
    })
}

/// `sort_rings_largest_to_smallest`: the rings without points last.
fn sort_rings_largest_to_smallest(manager: &mut RingManager) -> Vec<RingPtr> {
    let mut sorted_rings: Vec<RingPtr> = (0..manager.rings.len()).collect();
    sorted_rings.sort_by(|&r1, &r2| {
        let (empty_1, empty_2) = (manager.rings[r1].points == NIL, manager.rings[r2].points == NIL);
        if empty_1 || empty_2 {
            return empty_1.cmp(&empty_2);
        }
        manager.area(r2).abs().total_cmp(&manager.area(r1).abs())
    });
    sorted_rings
}

/// `sort_rings_smallest_to_largest`: the rings without points last.
fn sort_rings_smallest_to_largest(manager: &mut RingManager) -> Vec<RingPtr> {
    let mut sorted_rings: Vec<RingPtr> = (0..manager.rings.len()).collect();
    sorted_rings.sort_by(|&r1, &r2| {
        let (empty_1, empty_2) = (manager.rings[r1].points == NIL, manager.rings[r2].points == NIL);
        if empty_1 || empty_2 {
            return empty_1.cmp(&empty_2);
        }
        manager.area(r1).abs().total_cmp(&manager.area(r2).abs())
    });
    sorted_rings
}

/// `collinear_path<int32_t>`: collinear edges are in opposite directions
/// such that start_1 is at the same position of end_2 and vise versa. Start
/// to end is always forward (next) on path.
struct CollinearPath {
    start_1: PointPtr,
    end_1: PointPtr,
    start_2: PointPtr,
    end_2: PointPtr,
}

/// Unlinks `itr` (one step of the deletion loops of `fix_collinear_path`).
fn unlink(itr: PointPtr, m: &mut RingManager) -> PointPtr {
    let prev = m.prev(itr);
    m.points[prev].next = NIL;
    let p = &mut m.points[itr];
    p.prev = NIL;
    p.ring = NIL;
    p.next
}

/// `fix_collinear_path`: returns `collinear_result { pt1, pt2 }`.
fn fix_collinear_path(path: &CollinearPath, m: &mut RingManager) -> (PointPtr, PointPtr) {
    // Left and right are just the opposite ends of the
    // collinear path, they may not be actually left
    // and right of each other.
    // The left end is start_1 and end_2
    // The right end is start_2 and end_1

    // NOTE spike detection is checking that the
    // pointers are the same values, not position!
    // additionally duplicate points we will treat
    // if they are a spike left.
    let spike_left = path.start_1 == path.end_2;
    let spike_right = path.start_2 == path.end_1;

    if spike_left && spike_right {
        // If both ends are spikes we should simply
        // delete all the points. (they should be in a loop)
        let mut itr = path.start_1;
        while itr != NIL {
            itr = unlink(itr, m);
        }
        (NIL, NIL)
    } else if spike_left {
        let prev = m.prev(path.start_2);
        let mut itr = path.start_2;
        while itr != path.end_1 {
            itr = unlink(itr, m);
        }
        m.points[prev].next = path.end_1;
        m.points[path.end_1].prev = prev;
        (path.end_1, NIL)
    } else if spike_right {
        let prev = m.prev(path.start_1);
        let mut itr = path.start_1;
        while itr != path.end_2 {
            itr = unlink(itr, m);
        }
        m.points[prev].next = path.end_2;
        m.points[path.end_2].prev = prev;
        (path.end_2, NIL)
    } else {
        let prev_1 = m.prev(path.start_1);
        let prev_2 = m.prev(path.start_2);
        let mut itr = path.start_1;
        loop {
            itr = unlink(itr, m);
            if !(itr != path.end_1 && itr != NIL) {
                break;
            }
        }
        itr = path.start_2;
        loop {
            itr = unlink(itr, m);
            if !(itr != path.end_2 && itr != NIL) {
                break;
            }
        }
        if path.start_1 == path.end_1 && path.start_2 == path.end_2 {
            (NIL, NIL)
        } else if path.start_1 == path.end_1 {
            m.points[prev_2].next = path.end_2;
            m.points[path.end_2].prev = prev_2;
            (path.end_2, NIL)
        } else if path.start_2 == path.end_2 {
            m.points[prev_1].next = path.end_1;
            m.points[path.end_1].prev = prev_1;
            (path.end_1, NIL)
        } else {
            m.points[prev_1].next = path.end_2;
            m.points[path.end_2].prev = prev_1;
            m.points[prev_2].next = path.end_1;
            m.points[path.end_1].prev = prev_2;
            (path.end_1, path.end_2)
        }
    }
}

/// `find_start_and_end_of_collinear_edges`.
fn find_start_and_end_of_collinear_edges(pt_a: PointPtr, pt_b: PointPtr, m: &RingManager) -> CollinearPath {
    // Search backward on A, forwards on B first
    let same_ring = m.ring_of(pt_a) == m.ring_of(pt_b);
    let mut back = pt_a;
    let mut forward = pt_b;
    let mut first = true;
    loop {
        while m.pt_eq(m.prev(back), back) && back != forward {
            back = m.prev(back);
            if back == pt_a {
                break;
            }
        }
        if back == forward {
            back = m.prev(back);
            forward = m.next(forward);
            break;
        }
        while m.pt_eq(m.next(forward), forward) && back != forward {
            forward = m.next(forward);
            if forward == pt_b {
                break;
            }
        }
        if !first && (back == pt_a || forward == pt_b) {
            break;
        }
        if back == forward {
            back = m.prev(back);
            forward = m.next(forward);
            break;
        }
        back = m.prev(back);
        forward = m.next(forward);
        first = false;
        if !m.pt_eq(back, forward) {
            break;
        }
    }
    let mut start_a = m.next(back);
    // If there are repeated points at the diverge we want to select
    // only the first of those repeated points.
    while !same_ring && m.pt_eq(start_a, m.next(start_a)) && start_a != pt_a {
        start_a = m.next(start_a);
    }
    let mut end_b = m.prev(forward);
    while !same_ring && m.pt_eq(end_b, m.prev(end_b)) && end_b != pt_b {
        end_b = m.prev(end_b);
    }

    // Search backward on B, forwards on A next
    back = pt_b;
    forward = pt_a;
    first = true;
    loop {
        while m.pt_eq(m.prev(back), back) && back != forward {
            back = m.prev(back);
            if back == pt_b {
                break;
            }
        }
        if back == forward {
            back = m.prev(back);
            forward = m.next(forward);
            break;
        }
        while m.pt_eq(m.next(forward), forward) && back != forward {
            forward = m.next(forward);
            if forward == pt_a {
                break;
            }
        }
        if !first && (back == pt_b || forward == pt_a) {
            break;
        }
        if back == forward || (!first && (back == end_b || forward == start_a)) {
            back = m.prev(back);
            forward = m.next(forward);
            break;
        }
        back = m.prev(back);
        forward = m.next(forward);
        first = false;
        if !m.pt_eq(back, forward) {
            break;
        }
    }
    let mut start_b = m.next(back);
    while !same_ring && m.pt_eq(start_b, m.next(start_b)) && start_b != pt_b {
        start_b = m.next(start_b);
    }
    let mut end_a = m.prev(forward);
    while !same_ring && m.pt_eq(end_a, m.prev(end_a)) && end_a != pt_a {
        end_a = m.prev(end_a);
    }
    CollinearPath { start_1: start_a, end_1: end_a, start_2: start_b, end_2: end_b }
}

/// `has_collinear_edge`: it is assumed pt_a and pt_b are at the same
/// location.
fn has_collinear_edge(pt_a: PointPtr, pt_b: PointPtr, m: &RingManager) -> bool {
    m.pt_eq(m.next(pt_a), m.prev(pt_b)) || m.pt_eq(m.next(pt_b), m.prev(pt_a))
}

/// `process_collinear_edges_same_ring`.
fn process_collinear_edges_same_ring(pt_a: PointPtr, pt_b: PointPtr, manager: &mut RingManager) {
    let original_ring = manager.ring_of(pt_a);
    // As they are the same ring that are forming a collinear edge
    // we should expect the creation of two different rings.
    let path = find_start_and_end_of_collinear_edges(pt_a, pt_b, manager);
    let results = fix_collinear_path(&path, manager);
    if results.0 == NIL {
        // If pt1 is a nullptr, both values
        // are nullptrs. This mean the ring was
        // removed during this processing.
        remove_ring(original_ring, manager, false, true);
    } else if results.1 == NIL {
        // If a single point is only returned, we simply removed a spike.
        // In this case, we don't need to worry about parent or children
        // and we simply need to set the points and clear the area
        manager.rings[original_ring].points = results.0;
        manager.recalculate_stats(original_ring);
    } else {
        // If we have two seperate points, the ring has split into
        // two different rings.
        let ring_new = create_new_ring(manager);
        manager.rings[ring_new].points = results.1;
        manager.recalculate_stats(ring_new);
        update_points_ring(ring_new, manager);
        manager.rings[original_ring].points = results.0;
        manager.recalculate_stats(original_ring);
    }
}

/// `process_collinear_edges_different_rings`.
fn process_collinear_edges_different_rings(pt_a: PointPtr, pt_b: PointPtr, manager: &mut RingManager) {
    let ring_a = manager.ring_of(pt_a);
    let ring_b = manager.ring_of(pt_b);
    let ring_a_larger = manager.area(ring_a).abs() > manager.area(ring_b).abs();
    let path = find_start_and_end_of_collinear_edges(pt_a, pt_b, manager);
    // This should result in two rings becoming one.
    let results = fix_collinear_path(&path, manager);
    if results.0 == NIL {
        remove_ring(ring_a, manager, false, true);
        remove_ring(ring_b, manager, false, true);
        return;
    }
    // Rings should merge into a single ring of the same orientation.
    // Therefore, we we will need to replace one ring with the other
    let merged_ring = if ring_a_larger { ring_a } else { ring_b };
    let deleted_ring = if ring_a_larger { ring_b } else { ring_a };

    manager.rings[merged_ring].points = results.0;
    update_points_ring(merged_ring, manager);
    manager.recalculate_stats(merged_ring);
    if manager.size(merged_ring) < 3 {
        remove_ring_and_points(merged_ring, manager, false, true);
    }
    remove_ring(deleted_ring, manager, false, true);
}

/// Removes `remove` from its list after `keep` (the bodies of the
/// `while (*pt->next == *pt ...)` loops of `remove_duplicate_points`).
fn drop_point(keep: PointPtr, remove: PointPtr, manager: &mut RingManager) {
    let p = &mut manager.points[remove];
    p.next = NIL;
    p.prev = NIL;
    p.ring = NIL;
    let ring = manager.ring_of(keep);
    if manager.rings[ring].points == remove {
        manager.rings[ring].points = keep;
    }
}

/// `remove_duplicate_points`.
fn remove_duplicate_points(pt_a: PointPtr, pt_b: PointPtr, manager: &mut RingManager) -> bool {
    if manager.ring_of(pt_a) == manager.ring_of(pt_b) {
        if manager.next(pt_a) == pt_b {
            let next = manager.next(pt_b);
            manager.points[pt_a].next = next;
            manager.points[next].prev = pt_a;
            drop_point(pt_a, pt_b, manager);
            return true;
        } else if manager.next(pt_b) == pt_a {
            let prev = manager.prev(pt_b);
            manager.points[pt_a].prev = prev;
            manager.points[prev].next = pt_a;
            drop_point(pt_a, pt_b, manager);
            return true;
        }
    }
    while manager.pt_eq(manager.next(pt_a), pt_a) && manager.next(pt_a) != pt_a {
        let remove = manager.next(pt_a);
        let next = manager.next(remove);
        manager.points[pt_a].next = next;
        manager.points[next].prev = pt_a;
        drop_point(pt_a, remove, manager);
    }
    while manager.pt_eq(manager.prev(pt_a), pt_a) && manager.prev(pt_a) != pt_a {
        let remove = manager.prev(pt_a);
        let prev = manager.prev(remove);
        manager.points[pt_a].prev = prev;
        manager.points[prev].next = pt_a;
        drop_point(pt_a, remove, manager);
    }
    if manager.next(pt_a) == pt_a {
        let ring = manager.ring_of(pt_a);
        remove_ring_and_points(ring, manager, false, true);
        return true;
    }
    if manager.ring_of(pt_b) == NIL {
        return true;
    }
    while manager.pt_eq(manager.next(pt_b), pt_b) && manager.next(pt_b) != pt_b {
        let remove = manager.next(pt_b);
        let next = manager.next(remove);
        manager.points[pt_b].next = next;
        manager.points[next].prev = pt_b;
        drop_point(pt_b, remove, manager);
    }
    while manager.pt_eq(manager.prev(pt_b), pt_b) && manager.prev(pt_b) != pt_b {
        let remove = manager.prev(pt_b);
        let prev = manager.prev(remove);
        manager.points[pt_b].prev = prev;
        manager.points[prev].next = pt_b;
        drop_point(pt_b, remove, manager);
    }
    if manager.next(pt_b) == pt_b {
        let ring = manager.ring_of(pt_b);
        remove_ring_and_points(ring, manager, false, true);
        return true;
    }
    if manager.ring_of(pt_a) == NIL {
        return true;
    }
    false
}

/// `process_collinear_edges`.
fn process_collinear_edges(pt_a: PointPtr, pt_b: PointPtr, manager: &mut RingManager) -> bool {
    // Neither point assigned to a ring (deleted points)
    if manager.ring_of(pt_a) == NIL || manager.ring_of(pt_b) == NIL {
        return false;
    }

    if remove_duplicate_points(pt_a, pt_b, manager) {
        return true;
    }

    if !has_collinear_edge(pt_a, pt_b, manager) {
        if manager.ring_of(pt_a) == manager.ring_of(pt_b) {
            correct_self_intersection(pt_a, pt_b, manager);
            return true;
        }
        return false;
    }

    if manager.ring_of(pt_a) == manager.ring_of(pt_b) {
        process_collinear_edges_same_ring(pt_a, pt_b, manager);
    } else {
        process_collinear_edges_different_rings(pt_a, pt_b, manager);
    }
    true
}

/// `correct_collinear_repeats`.
fn correct_collinear_repeats(manager: &mut RingManager, points: &[PointPtr]) {
    for itr1 in 0..points.len() {
        if manager.ring_of(points[itr1]) == NIL {
            continue;
        }

        let mut itr2 = 0;
        while itr2 != points.len() {
            if manager.ring_of(points[itr1]) == NIL {
                break;
            }
            if manager.ring_of(points[itr2]) == NIL || points[itr2] == points[itr1] {
                itr2 += 1;
                continue;
            }
            if process_collinear_edges(points[itr1], points[itr2], manager) {
                itr2 = 0;
            } else {
                itr2 += 1;
            }
        }
    }
}

/// `correct_collinear_edges`.
fn correct_collinear_edges(manager: &mut RingManager) {
    if manager.all_points.len() < 2 {
        return;
    }
    let all_points = manager.all_points.clone();
    let _ = for_each_repeated_run(manager, &all_points, |m, first, last| {
        correct_collinear_repeats(m, &all_points[first..last]);
        Ok(())
    });
}

/// `correct_tree`: it is possible that vatti resulted in some parent child
/// relationships that are not quite right, therefore, we need to just
/// rebuild the entire tree of rings.
fn correct_tree(manager: &mut RingManager) -> R<()> {
    // First lets process the rings in order of size from largest
    // area to smallest, we know right away that no smaller ring could ever
    // contain a larger ring so we can use this to our advantage
    // as we iterate over the rings.
    let sorted_rings = sort_rings_largest_to_smallest(manager);
    for itr in 0..sorted_rings.len() {
        let r = sorted_rings[itr];
        if manager.rings[r].points == NIL {
            continue;
        }
        if manager.size(r) < 3 || value_is_zero(manager.area(r)) {
            remove_ring_and_points(r, manager, false, true);
            continue;
        }
        manager.rings[r].corrected = true;
        let mut found = false;
        // Search in reverse from the current iterator back to the begining
        // to see if any of those rings might be its parent.
        for r_itr in (0..itr).rev() {
            let rr = sorted_rings[r_itr];
            // If orientations are not different, this can't be its parent.
            if manager.is_hole(rr) == manager.is_hole(r) {
                continue;
            }
            if poly2_contains_poly1(r, rr, manager)? {
                reassign_as_child(r, rr, manager)?;
                found = true;
                break;
            }
        }
        if !found {
            if manager.is_hole(r) {
                return Err("Could not properly place hole to a parent.".to_owned());
            } else {
                // Assign to base of tree by passing nullptr
                reassign_as_child(r, NIL, manager)?;
            }
        }
    }
    Ok(())
}

/// `correct_self_intersections`.
fn correct_self_intersections(manager: &mut RingManager, correct_tree: bool) -> R<bool> {
    let mut fixed_intersections = false;
    let sorted_rings = sort_rings_smallest_to_largest(manager);
    for r in sorted_rings {
        if correct_ring_self_intersections(manager, r, correct_tree)? {
            fixed_intersections = true;
        }
    }
    Ok(fixed_intersections)
}

/// `correct_topology`.
pub(crate) fn correct_topology(manager: &mut RingManager) -> R<()> {
    // Sort all the points, this will be used for the locating of chained rings
    // and the collinear edges and only needs to be done once.
    let mut all_points = std::mem::take(&mut manager.all_points);
    all_points.sort_by(|&a, &b| point_ptr_cmp(manager, a, b));
    manager.all_points = all_points;

    // Initially the orientations of the rings
    // could be incorrect, we need to adjust them
    correct_orientations(manager);

    // We should only have to fix collinear edges once.
    // During this we also correct self intersections
    correct_collinear_edges(manager);

    correct_self_intersections(manager, false)?;

    correct_tree(manager)?;

    let mut fixed_intersections = true;
    while fixed_intersections {
        correct_chained_rings(manager)?;
        fixed_intersections = correct_self_intersections(manager, true)?;
    }
    Ok(())
}
