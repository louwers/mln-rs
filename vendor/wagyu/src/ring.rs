// Ports vendor/wagyu/include/mapbox/geometry/wagyu/ring.hpp. Rings and points
// live in the ring manager's arenas (`std::deque<ring>`/`std::deque<point>`
// + `storage` in C++, which only provide stable addresses); `ring_ptr` and
// `point_ptr` are indices, NIL is nullptr. The lazily computed ring stats
// (`area()`, `is_hole()`, `size()`) need the point arena, so they are
// methods of the manager taking the ring.

use crate::point::{GPoint, NIL, Point, PointPtr, RingPtr};

pub(crate) type R<T> = Result<T, String>;

/// `mapbox::geometry::box<int32_t>`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct GBox {
    pub min: GPoint,
    pub max: GPoint,
}

impl GBox {
    pub fn zero() -> GBox {
        GBox { min: GPoint::new(0, 0), max: GPoint::new(0, 0) }
    }
}

/// `ring<int32_t>`.
#[derive(Clone, Debug)]
pub(crate) struct Ring {
    /// To support unset 0 is undefined and indexes offset by 1
    pub ring_index: usize,
    /// number of points in the ring
    pub size_: usize,
    /// area of the ring
    pub area_: f64,
    /// bounding box of the ring
    pub bbox: GBox,
    pub parent: RingPtr,
    pub children: Vec<RingPtr>,
    pub points: PointPtr,
    pub bottom_point: PointPtr,
    pub is_hole_: bool,
    pub corrected: bool,
}

impl Ring {
    fn new() -> Ring {
        Ring {
            ring_index: 0,
            size_: 0,
            area_: f64::NAN,
            bbox: GBox::zero(),
            parent: NIL,
            children: Vec::new(),
            points: NIL,
            bottom_point: NIL,
            is_hole_: false,
            corrected: false,
        }
    }
}

/// `ring_manager<int32_t>`.
#[derive(Debug, Default)]
pub(crate) struct RingManager {
    pub children: Vec<RingPtr>,
    pub all_points: Vec<PointPtr>,
    pub hot_pixels: Vec<GPoint>,
    pub current_hp_itr: usize,
    pub points: Vec<Point>,
    pub rings: Vec<Ring>,
    pub index: usize,
}

impl RingManager {
    pub fn pt(&self, p: PointPtr) -> &Point {
        &self.points[p]
    }

    pub fn next(&self, p: PointPtr) -> PointPtr {
        self.points[p].next
    }

    pub fn prev(&self, p: PointPtr) -> PointPtr {
        self.points[p].prev
    }

    pub fn ring_of(&self, p: PointPtr) -> RingPtr {
        self.points[p].ring
    }

    pub fn xy(&self, p: PointPtr) -> (i32, i32) {
        (self.points[p].x, self.points[p].y)
    }

    /// `*a == *b` (`operator==` on wagyu points: the coordinates).
    pub fn pt_eq(&self, a: PointPtr, b: PointPtr) -> bool {
        self.points[a].x == self.points[b].x && self.points[a].y == self.points[b].y
    }

    pub fn parent(&self, r: RingPtr) -> RingPtr {
        self.rings[r].parent
    }

    /// `parent == nullptr ? manager.children : parent->children`.
    pub fn children_of(&mut self, parent: RingPtr) -> &mut Vec<RingPtr> {
        if parent == NIL { &mut self.children } else { &mut self.rings[parent].children }
    }

    /// `area_from_point`: (area, size, bbox).
    pub fn area_from_point(&self, mut op: PointPtr) -> (f64, usize, GBox) {
        let start_op = op;
        let mut size = 0;
        let mut a = 0.0;
        let mut min_x = self.points[op].x;
        let mut max_x = self.points[op].x;
        let mut min_y = self.points[op].y;
        let mut max_y = self.points[op].y;
        loop {
            size += 1;
            let p = &self.points[op];
            if p.x > max_x {
                max_x = p.x;
            } else if p.x < min_x {
                min_x = p.x;
            }
            if p.y > max_y {
                max_y = p.y;
            } else if p.y < min_y {
                min_y = p.y;
            }
            let prev = &self.points[p.prev];
            a += f64::from(prev.x.wrapping_add(p.x)) * f64::from(prev.y.wrapping_sub(p.y));
            op = p.next;
            if op == start_op {
                break;
            }
        }
        let bbox = GBox { min: GPoint::new(min_x, min_y), max: GPoint::new(max_x, max_y) };
        (a * 0.5, size, bbox)
    }

    pub fn reset_stats(&mut self, r: RingPtr) {
        let ring = &mut self.rings[r];
        ring.area_ = f64::NAN;
        ring.is_hole_ = false;
        ring.bbox = GBox::zero();
        ring.size_ = 0;
    }

    pub fn recalculate_stats(&mut self, r: RingPtr) {
        let points = self.rings[r].points;
        if points != NIL {
            let (area, size, bbox) = self.area_from_point(points);
            let ring = &mut self.rings[r];
            ring.area_ = area;
            ring.size_ = size;
            ring.bbox = bbox;
            ring.is_hole_ = area.is_nan() || area <= 0.0;
        }
    }

    pub fn set_stats(&mut self, r: RingPtr, a: f64, s: usize, b: GBox) {
        let ring = &mut self.rings[r];
        ring.bbox = b;
        ring.area_ = a;
        ring.size_ = s;
        ring.is_hole_ = a.is_nan() || a <= 0.0;
    }

    /// `ring::area()`.
    pub fn area(&mut self, r: RingPtr) -> f64 {
        if self.rings[r].area_.is_nan() {
            self.recalculate_stats(r);
        }
        self.rings[r].area_
    }

    /// `ring::is_hole()` (from the area; see also `ring_is_hole`).
    pub fn is_hole(&mut self, r: RingPtr) -> bool {
        if self.rings[r].area_.is_nan() {
            self.recalculate_stats(r);
        }
        self.rings[r].is_hole_
    }

    /// `ring::size()`.
    pub fn size(&mut self, r: RingPtr) -> usize {
        if self.rings[r].area_.is_nan() {
            self.recalculate_stats(r);
        }
        self.rings[r].size_
    }
}

/// `create_new_ring`.
pub(crate) fn create_new_ring(manager: &mut RingManager) -> RingPtr {
    let mut ring = Ring::new();
    ring.ring_index = manager.index;
    manager.index += 1;
    manager.rings.push(ring);
    manager.rings.len() - 1
}

/// `create_new_point(r, pt, rings)`: a single-point list.
pub(crate) fn create_new_point(r: RingPtr, pt: GPoint, rings: &mut RingManager) -> PointPtr {
    let id = rings.points.len();
    rings.points.push(Point { ring: r, x: pt.x, y: pt.y, next: id, prev: id });
    rings.all_points.push(id);
    id
}

/// `create_new_point(r, pt, before_this_point, rings)`.
pub(crate) fn create_new_point_before(
    r: RingPtr,
    pt: GPoint,
    before_this_point: PointPtr,
    rings: &mut RingManager,
) -> PointPtr {
    let id = rings.points.len();
    let prev = rings.points[before_this_point].prev;
    rings.points.push(Point { ring: r, x: pt.x, y: pt.y, next: before_this_point, prev });
    rings.points[before_this_point].prev = id;
    rings.points[prev].next = id;
    rings.all_points.push(id);
    id
}

/// `set_to_children`.
pub(crate) fn set_to_children(r: RingPtr, children: &mut Vec<RingPtr>) {
    for c in children.iter_mut() {
        if *c == NIL {
            *c = r;
            return;
        }
    }
    children.push(r);
}

/// `remove_from_children`.
pub(crate) fn remove_from_children(r: RingPtr, children: &mut [RingPtr]) {
    for c in children.iter_mut() {
        if *c == r {
            *c = NIL;
            return;
        }
    }
}

/// `assign_as_child`.
pub(crate) fn assign_as_child(new_ring: RingPtr, parent: RingPtr, manager: &mut RingManager) -> R<()> {
    if (parent == NIL && manager.is_hole(new_ring))
        || (parent != NIL && manager.is_hole(new_ring) == manager.is_hole(parent))
    {
        return Err("Trying to assign a child that is the same orientation as the parent".to_owned());
    }
    set_to_children(new_ring, manager.children_of(parent));
    manager.rings[new_ring].parent = parent;
    Ok(())
}

/// `reassign_as_child`.
pub(crate) fn reassign_as_child(ring: RingPtr, parent: RingPtr, manager: &mut RingManager) -> R<()> {
    if (parent == NIL && manager.is_hole(ring)) || (parent != NIL && manager.is_hole(ring) == manager.is_hole(parent)) {
        return Err("Trying to re-assign a child that is the same orientation as the parent".to_owned());
    }
    let old_parent = manager.rings[ring].parent;
    remove_from_children(ring, manager.children_of(old_parent));
    set_to_children(ring, manager.children_of(parent));
    manager.rings[ring].parent = parent;
    Ok(())
}

/// `assign_as_sibling`.
pub(crate) fn assign_as_sibling(new_ring: RingPtr, sibling: RingPtr, manager: &mut RingManager) -> R<()> {
    if manager.is_hole(new_ring) != manager.is_hole(sibling) {
        return Err("Trying to assign to be a sibling that is not the same orientation as the sibling".to_owned());
    }
    let parent = manager.rings[sibling].parent;
    set_to_children(new_ring, manager.children_of(parent));
    manager.rings[new_ring].parent = parent;
    Ok(())
}

/// `ring1_replaces_ring2`.
pub(crate) fn ring1_replaces_ring2(ring1: RingPtr, ring2: RingPtr, manager: &mut RingManager) {
    debug_assert_ne!(ring1, ring2);
    for i in 0..manager.rings[ring2].children.len() {
        let c = manager.rings[ring2].children[i];
        if c == NIL {
            continue;
        }
        manager.rings[c].parent = ring1;
        set_to_children(c, manager.children_of(ring1));
        manager.rings[ring2].children[i] = NIL;
    }
    let old_parent = manager.rings[ring2].parent;
    remove_from_children(ring2, manager.children_of(old_parent));
    manager.rings[ring2].points = NIL;
    manager.reset_stats(ring2);
}

/// Detaches the circular list starting at `pt` (the loop of
/// `remove_ring_and_points`).
fn detach_points(mut pt: PointPtr, manager: &mut RingManager) {
    if pt != NIL {
        let prev = manager.points[pt].prev;
        manager.points[prev].next = NIL;
        while pt != NIL {
            let tmp = pt;
            pt = manager.points[pt].next;
            let p = &mut manager.points[tmp];
            p.next = NIL;
            p.prev = NIL;
            p.ring = NIL;
        }
    }
}

/// `remove_ring_and_points`.
pub(crate) fn remove_ring_and_points(
    r: RingPtr,
    manager: &mut RingManager,
    remove_children: bool,
    remove_from_parent: bool,
) {
    for i in 0..manager.rings[r].children.len() {
        let c = manager.rings[r].children[i];
        if c == NIL {
            continue;
        }
        if remove_children {
            remove_ring_and_points(c, manager, true, false);
        }
        manager.rings[r].children[i] = NIL;
    }
    if remove_from_parent {
        let parent = manager.rings[r].parent;
        remove_from_children(r, manager.children_of(parent));
    }
    let pt = manager.rings[r].points;
    detach_points(pt, manager);
    manager.rings[r].points = NIL;
    manager.reset_stats(r);
}

/// `remove_ring`.
pub(crate) fn remove_ring(r: RingPtr, manager: &mut RingManager, remove_children: bool, remove_from_parent: bool) {
    for i in 0..manager.rings[r].children.len() {
        let c = manager.rings[r].children[i];
        if c == NIL {
            continue;
        }
        if remove_children {
            remove_ring(c, manager, true, false);
        }
        manager.rings[r].children[i] = NIL;
    }
    if remove_from_parent {
        let parent = manager.rings[r].parent;
        remove_from_children(r, manager.children_of(parent));
    }
    manager.rings[r].points = NIL;
    manager.reset_stats(r);
}

/// `ring_depth`.
pub(crate) fn ring_depth(mut r: RingPtr, manager: &RingManager) -> usize {
    let mut depth = 0;
    if r == NIL {
        return depth;
    }
    while manager.rings[r].parent != NIL {
        depth += 1;
        r = manager.rings[r].parent;
    }
    depth
}

/// `ring_is_hole` (from the depth in the tree).
pub(crate) fn ring_is_hole(r: RingPtr, manager: &RingManager) -> bool {
    ring_depth(r, manager) & 1 == 1
}

/// `reverse_ring`.
pub(crate) fn reverse_ring(pp: PointPtr, manager: &mut RingManager) {
    if pp == NIL {
        return;
    }
    let mut pp1 = pp;
    loop {
        let p = &mut manager.points[pp1];
        let pp2 = p.next;
        std::mem::swap(&mut p.next, &mut p.prev);
        pp1 = pp2;
        if pp1 == pp {
            break;
        }
    }
}
