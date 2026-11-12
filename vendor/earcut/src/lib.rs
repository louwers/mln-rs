// Ports vendor/earcut.hpp/include/mapbox/earcut.hpp (mapbox::earcut,
// mapbox::detail::Earcut, mapbox::util::nth).
//
// The C++ is generic over the point type, but every coordinate is converted
// to `double` when the node is created (`Node::x`/`y` are `double`), so the
// instantiations the core uses (`Point<int16_t>` read as `int64_t` in
// gfx/fill_generator.cpp and renderer/buckets/fill_extrusion_bucket.cpp,
// `Point<float>` in fill_extrusion_bucket.cpp) differ only in that
// conversion: [`Nth`] does it.
//
// The C++ nodes are pool-allocated and linked by pointers; here they live in
// a `Vec` and link by index (`NIL` = `nullptr`). Pointer comparisons become
// index comparisons, which is the same identity.

use geometry::Point;

/// `mapbox::util::nth<0, P>::get` / `nth<1, P>::get`, converted to `double`
/// as `Node`'s constructor does.
pub trait Nth {
    fn nth0(&self) -> f64;
    fn nth1(&self) -> f64;
}

impl<T: Copy + Into<f64>> Nth for Point<T> {
    fn nth0(&self) -> f64 {
        self.x.into()
    }
    fn nth1(&self) -> f64 {
        self.y.into()
    }
}

/// `std::pair<T, T>` (the generic `nth` uses `std::get`).
impl<T: Copy + Into<f64>> Nth for (T, T) {
    fn nth0(&self) -> f64 {
        self.0.into()
    }
    fn nth1(&self) -> f64 {
        self.1.into()
    }
}

/// `std::array<T, 2>`.
impl<T: Copy + Into<f64>> Nth for [T; 2] {
    fn nth0(&self) -> f64 {
        self[0].into()
    }
    fn nth1(&self) -> f64 {
        self[1].into()
    }
}

/// The index type `N` of `mapbox::earcut<N>` (`uint32_t` by default).
pub trait Index: Copy + PartialEq + std::fmt::Debug {
    /// `static_cast<N>(i)` (modular).
    fn from_usize(i: usize) -> Self;
}

macro_rules! impl_index {
    ($($t:ty),*) => {$(
        impl Index for $t {
            fn from_usize(i: usize) -> Self {
                i as $t
            }
        }
    )*};
}
impl_index!(u16, u32, u64, usize);

/// `mapbox::earcut<N>(poly)`: triangle indices into the concatenated rings.
pub fn earcut<N: Index, P: Nth, R: AsRef<[P]>>(poly: &[R]) -> Vec<N> {
    let mut earcut = Earcut::<N>::new();
    earcut.run(poly);
    earcut.indices
}

/// "`nullptr`"
const NIL: usize = usize::MAX;

struct Node<N> {
    i: N,
    x: f64,
    y: f64,

    // previous and next vertice nodes in a polygon ring
    prev: usize,
    next: usize,

    // z-order curve value
    z: i32,

    // previous and next nodes in z-order
    prev_z: usize,
    next_z: usize,

    // indicates whether this is a steiner point
    steiner: bool,
}

/// `mapbox::detail::Earcut<N>`.
pub struct Earcut<N> {
    pub indices: Vec<N>,
    pub vertices: usize,

    hashing: bool,
    min_x: f64,
    max_x: f64,
    min_y: f64,
    max_y: f64,
    inv_size: f64,

    nodes: Vec<Node<N>>,
}

impl<N: Index> Default for Earcut<N> {
    fn default() -> Self {
        Self::new()
    }
}

impl<N: Index> Earcut<N> {
    #[must_use]
    pub fn new() -> Self {
        Earcut {
            indices: Vec::new(),
            vertices: 0,
            hashing: false,
            min_x: 0.0,
            max_x: 0.0,
            min_y: 0.0,
            max_y: 0.0,
            inv_size: 0.0,
            nodes: Vec::new(),
        }
    }

    /// `operator()(const Polygon& points)`.
    pub fn run<P: Nth, R: AsRef<[P]>>(&mut self, points: &[R]) {
        // reset
        self.indices.clear();
        self.vertices = 0;

        if points.is_empty() {
            return;
        }

        let mut threshold: i32 = 80;
        let mut len: usize = 0;

        let mut i = 0;
        while threshold >= 0 && i < points.len() {
            threshold = threshold.wrapping_sub(points[i].as_ref().len() as i32);
            len += points[i].as_ref().len();
            i += 1;
        }

        // estimate size of nodes and indices
        self.nodes.clear();
        self.nodes.reserve(len * 3 / 2);
        self.indices.reserve(len + points[0].as_ref().len());

        let mut outer_node = self.linked_list(points[0].as_ref(), true);
        if outer_node == NIL || self.nodes[outer_node].prev == self.nodes[outer_node].next {
            return;
        }

        if points.len() > 1 {
            outer_node = self.eliminate_holes(points, outer_node);
        }

        // if the shape is not too simple, we'll use z-order curve hash later;
        // calculate polygon bbox
        self.hashing = threshold < 0;
        if self.hashing {
            let mut p = self.nodes[outer_node].next;
            self.min_x = self.nodes[outer_node].x;
            self.max_x = self.min_x;
            self.min_y = self.nodes[outer_node].y;
            self.max_y = self.min_y;
            loop {
                let x = self.nodes[p].x;
                let y = self.nodes[p].y;
                // std::min<double>(minX, x) / std::max<double>(maxX, x)
                self.min_x = if x < self.min_x { x } else { self.min_x };
                self.min_y = if y < self.min_y { y } else { self.min_y };
                self.max_x = if self.max_x < x { x } else { self.max_x };
                self.max_y = if self.max_y < y { y } else { self.max_y };
                p = self.nodes[p].next;
                if p == outer_node {
                    break;
                }
            }

            // minX, minY and size are later used to transform coords into
            // integers for z-order calculation
            let dx = self.max_x - self.min_x;
            let dy = self.max_y - self.min_y;
            self.inv_size = if dx < dy { dy } else { dx };
            self.inv_size = if self.inv_size == 0.0 { 0.0 } else { 1.0 / self.inv_size };
        }

        self.earcut_linked(outer_node, 0);

        self.nodes.clear();
    }

    // create a circular doubly linked list from polygon points in the
    // specified winding order
    fn linked_list<P: Nth>(&mut self, points: &[P], clockwise: bool) -> usize {
        let mut sum = 0.0;
        let len = points.len();
        let mut last = NIL;

        // calculate original winding order of a polygon ring
        let mut j = if len > 0 { len - 1 } else { 0 };
        for i in 0..len {
            let p1 = &points[i];
            let p2 = &points[j];
            let p20 = p2.nth0();
            let p10 = p1.nth0();
            let p11 = p1.nth1();
            let p21 = p2.nth1();
            sum += (p20 - p10) * (p11 + p21);
            j = i;
        }

        // link points into circular doubly-linked list in the specified
        // winding order
        if clockwise == (sum > 0.0) {
            for (i, p) in points.iter().enumerate() {
                last = self.insert_node(self.vertices + i, p, last);
            }
        } else {
            for i in (0..len).rev() {
                last = self.insert_node(self.vertices + i, &points[i], last);
            }
        }

        if last != NIL && self.equals(last, self.nodes[last].next) {
            self.remove_node(last);
            last = self.nodes[last].next;
        }

        self.vertices += len;

        last
    }

    // eliminate colinear or duplicate points
    fn filter_points(&mut self, start: usize, end: usize) -> usize {
        let mut end = if end == NIL { start } else { end };

        let mut p = start;
        loop {
            let mut again = false;

            let (steiner, prev, next) = {
                let pn = &self.nodes[p];
                (pn.steiner, pn.prev, pn.next)
            };
            if !steiner && (self.equals(p, next) || self.area(prev, p, next) == 0.0) {
                self.remove_node(p);
                p = self.nodes[p].prev;
                end = p;

                if p == self.nodes[p].next {
                    break;
                }
                again = true;
            } else {
                p = self.nodes[p].next;
            }
            if !(again || p != end) {
                break;
            }
        }

        end
    }

    // main ear slicing loop which triangulates a polygon (given as a linked
    // list)
    fn earcut_linked(&mut self, ear: usize, pass: i32) {
        let mut ear = ear;
        if ear == NIL {
            return;
        }

        // interlink polygon nodes in z-order
        if pass == 0 && self.hashing {
            self.index_curve(ear);
        }

        let mut stop = ear;

        // iterate through ears, slicing them one by one
        while self.nodes[ear].prev != self.nodes[ear].next {
            let prev = self.nodes[ear].prev;
            let next = self.nodes[ear].next;

            let is_ear = if self.hashing { self.is_ear_hashed(ear) } else { self.is_ear(ear) };
            if is_ear {
                // cut off the triangle
                self.indices.push(self.nodes[prev].i);
                self.indices.push(self.nodes[ear].i);
                self.indices.push(self.nodes[next].i);

                self.remove_node(ear);

                // skipping the next vertice leads to less sliver triangles
                ear = self.nodes[next].next;
                stop = self.nodes[next].next;

                continue;
            }

            ear = next;

            // if we looped through the whole remaining polygon and can't find
            // any more ears
            if ear == stop {
                if pass == 0 {
                    // try filtering points and slicing again
                    let filtered = self.filter_points(ear, NIL);
                    self.earcut_linked(filtered, 1);
                } else if pass == 1 {
                    // if this didn't work, try curing all small
                    // self-intersections locally
                    let filtered = self.filter_points(ear, NIL);
                    ear = self.cure_local_intersections(filtered);
                    self.earcut_linked(ear, 2);
                } else if pass == 2 {
                    // as a last resort, try splitting the remaining polygon
                    // into two
                    self.split_earcut(ear);
                }

                break;
            }
        }
    }

    // check whether a polygon node forms a valid ear with adjacent nodes
    fn is_ear(&self, ear: usize) -> bool {
        let a = self.nodes[ear].prev;
        let b = ear;
        let c = self.nodes[ear].next;

        if self.area(a, b, c) >= 0.0 {
            return false; // reflex, can't be an ear
        }

        let (an, bn, cn) = (&self.nodes[a], &self.nodes[b], &self.nodes[c]);

        // now make sure we don't have other points inside the potential ear
        let mut p = self.nodes[self.nodes[ear].next].next;

        while p != self.nodes[ear].prev {
            let pn = &self.nodes[p];
            if point_in_triangle(an.x, an.y, bn.x, bn.y, cn.x, cn.y, pn.x, pn.y)
                && self.area(pn.prev, p, pn.next) >= 0.0
            {
                return false;
            }
            p = pn.next;
        }

        true
    }

    fn is_ear_hashed(&self, ear: usize) -> bool {
        let a = self.nodes[ear].prev;
        let b = ear;
        let c = self.nodes[ear].next;

        if self.area(a, b, c) >= 0.0 {
            return false; // reflex, can't be an ear
        }

        let (an, bn, cn) = (&self.nodes[a], &self.nodes[b], &self.nodes[c]);

        // triangle bbox; min & max are calculated like this for speed
        let min_tx = min_f64(an.x, min_f64(bn.x, cn.x));
        let min_ty = min_f64(an.y, min_f64(bn.y, cn.y));
        let max_tx = max_f64(an.x, max_f64(bn.x, cn.x));
        let max_ty = max_f64(an.y, max_f64(bn.y, cn.y));

        // z-order range for the current triangle bbox;
        let min_z = self.z_order(min_tx, min_ty);
        let max_z = self.z_order(max_tx, max_ty);

        // first look for points inside the triangle in increasing z-order
        let mut p = self.nodes[ear].next_z;

        while p != NIL && self.nodes[p].z <= max_z {
            let pn = &self.nodes[p];
            if p != self.nodes[ear].prev
                && p != self.nodes[ear].next
                && point_in_triangle(an.x, an.y, bn.x, bn.y, cn.x, cn.y, pn.x, pn.y)
                && self.area(pn.prev, p, pn.next) >= 0.0
            {
                return false;
            }
            p = pn.next_z;
        }

        // then look for points in decreasing z-order
        p = self.nodes[ear].prev_z;

        while p != NIL && self.nodes[p].z >= min_z {
            let pn = &self.nodes[p];
            if p != self.nodes[ear].prev
                && p != self.nodes[ear].next
                && point_in_triangle(an.x, an.y, bn.x, bn.y, cn.x, cn.y, pn.x, pn.y)
                && self.area(pn.prev, p, pn.next) >= 0.0
            {
                return false;
            }
            p = pn.prev_z;
        }

        true
    }

    // go through all polygon nodes and cure small local self-intersections
    fn cure_local_intersections(&mut self, start: usize) -> usize {
        let mut start = start;
        let mut p = start;
        loop {
            let a = self.nodes[p].prev;
            let b = self.nodes[self.nodes[p].next].next;

            // a self-intersection where edge (v[i-1],v[i]) intersects
            // (v[i+1],v[i+2])
            if !self.equals(a, b)
                && self.intersects(a, p, self.nodes[p].next, b)
                && self.locally_inside(a, b)
                && self.locally_inside(b, a)
            {
                self.indices.push(self.nodes[a].i);
                self.indices.push(self.nodes[p].i);
                self.indices.push(self.nodes[b].i);

                // remove two nodes involved
                self.remove_node(p);
                let pn = self.nodes[p].next;
                self.remove_node(pn);

                p = b;
                start = b;
            }
            p = self.nodes[p].next;
            if p == start {
                break;
            }
        }

        self.filter_points(p, NIL)
    }

    // try splitting polygon into two and triangulate them independently
    fn split_earcut(&mut self, start: usize) {
        // look for a valid diagonal that divides the polygon into two
        let mut a = start;
        loop {
            let mut b = self.nodes[self.nodes[a].next].next;
            while b != self.nodes[a].prev {
                if self.nodes[a].i != self.nodes[b].i && self.is_valid_diagonal(a, b) {
                    // split the polygon in two by the diagonal
                    let mut c = self.split_polygon(a, b);

                    // filter colinear points around the cuts
                    a = self.filter_points(a, self.nodes[a].next);
                    c = self.filter_points(c, self.nodes[c].next);

                    // run earcut on each half
                    self.earcut_linked(a, 0);
                    self.earcut_linked(c, 0);
                    return;
                }
                b = self.nodes[b].next;
            }
            a = self.nodes[a].next;
            if a == start {
                break;
            }
        }
    }

    // link every hole into the outer loop, producing a single-ring polygon
    // without holes
    fn eliminate_holes<P: Nth, R: AsRef<[P]>>(&mut self, points: &[R], outer_node: usize) -> usize {
        let mut outer_node = outer_node;
        let len = points.len();

        let mut queue: Vec<usize> = Vec::new();
        for ring in &points[1..len] {
            let list = self.linked_list(ring.as_ref(), false);
            if list != NIL {
                if list == self.nodes[list].next {
                    self.nodes[list].steiner = true;
                }
                queue.push(self.get_leftmost(list));
            }
        }
        let nodes = &self.nodes;
        queue.sort_by(|a, b| nodes[*a].x.total_cmp(&nodes[*b].x));

        // process holes from left to right
        for hole in queue {
            self.eliminate_hole(hole, outer_node);
            outer_node = self.filter_points(outer_node, self.nodes[outer_node].next);
        }

        outer_node
    }

    // find a bridge between vertices that connects hole with an outer ring
    // and and link it
    fn eliminate_hole(&mut self, hole: usize, outer_node: usize) {
        let outer_node = self.find_hole_bridge(hole, outer_node);
        if outer_node != NIL {
            let b = self.split_polygon(outer_node, hole);
            self.filter_points(b, self.nodes[b].next);
        }
    }

    // David Eberly's algorithm for finding a bridge between hole and outer
    // polygon
    fn find_hole_bridge(&self, hole: usize, outer_node: usize) -> usize {
        let n = &self.nodes;
        let mut p = outer_node;
        let hx = n[hole].x;
        let hy = n[hole].y;
        let mut qx = f64::NEG_INFINITY;
        let mut m = NIL;

        // find a segment intersected by a ray from the hole's leftmost Vertex
        // to the left; segment's endpoint with lesser x will be potential
        // connection Vertex
        loop {
            let pn = &n[p];
            let pnext = &n[pn.next];
            if hy <= pn.y && hy >= pnext.y && pnext.y != pn.y {
                let x = pn.x + (hy - pn.y) * (pnext.x - pn.x) / (pnext.y - pn.y);
                if x <= hx && x > qx {
                    qx = x;
                    if x == hx {
                        if hy == pn.y {
                            return p;
                        }
                        if hy == pnext.y {
                            return pn.next;
                        }
                    }
                    m = if pn.x < pnext.x { p } else { pn.next };
                }
            }
            p = pn.next;
            if p == outer_node {
                break;
            }
        }

        if m == NIL {
            return NIL;
        }

        if hx == qx {
            return m; // hole touches outer segment; pick leftmost endpoint
        }

        // look for points inside the triangle of hole Vertex, segment
        // intersection and endpoint; if there are no points found, we have a
        // valid connection; otherwise choose the Vertex of the minimum angle
        // with the ray as connection Vertex

        let stop = m;
        let mut tan_min = f64::INFINITY;

        p = m;
        let mx = n[m].x;
        let my = n[m].y;

        loop {
            let pn = &n[p];
            if hx >= pn.x
                && pn.x >= mx
                && hx != pn.x
                && point_in_triangle(
                    if hy < my { hx } else { qx },
                    hy,
                    mx,
                    my,
                    if hy < my { qx } else { hx },
                    hy,
                    pn.x,
                    pn.y,
                )
            {
                let tan_cur = (hy - pn.y).abs() / (hx - pn.x); // tangential

                if self.locally_inside(p, hole)
                    && (tan_cur < tan_min
                        || (tan_cur == tan_min && (pn.x > n[m].x || self.sector_contains_sector(m, p))))
                {
                    m = p;
                    tan_min = tan_cur;
                }
            }

            p = pn.next;
            if p == stop {
                break;
            }
        }

        m
    }

    // whether sector in vertex m contains sector in vertex p in the same
    // coordinates
    fn sector_contains_sector(&self, m: usize, p: usize) -> bool {
        let (mn, pn) = (&self.nodes[m], &self.nodes[p]);
        (self.area(mn.prev, m, pn.prev) < 0.0 || self.area(pn.prev, m, mn.next) < 0.0)
            && (self.area(mn.prev, m, pn.next) < 0.0 || self.area(pn.next, m, mn.next) < 0.0)
    }

    // interlink polygon nodes in z-order
    fn index_curve(&mut self, start: usize) {
        debug_assert!(start != NIL);
        let mut p = start;

        loop {
            let z = self.nodes[p].z;
            let z = if z != 0 { z } else { self.z_order(self.nodes[p].x, self.nodes[p].y) };
            let pn = &mut self.nodes[p];
            pn.z = z;
            pn.prev_z = pn.prev;
            pn.next_z = pn.next;
            p = pn.next;
            if p == start {
                break;
            }
        }

        let prev_z = self.nodes[p].prev_z;
        self.nodes[prev_z].next_z = NIL;
        self.nodes[p].prev_z = NIL;

        self.sort_linked(p);
    }

    // Simon Tatham's linked list merge sort algorithm
    // http://www.chiark.greenend.org.uk/~sgtatham/algorithms/listsort.html
    fn sort_linked(&mut self, list: usize) -> usize {
        debug_assert!(list != NIL);
        let mut list = list;
        let mut in_size: i32 = 1;

        loop {
            let mut p = list;
            list = NIL;
            let mut tail = NIL;
            let mut num_merges = 0;

            while p != NIL {
                num_merges += 1;
                let mut q = p;
                let mut p_size = 0;
                for _ in 0..in_size {
                    p_size += 1;
                    q = self.nodes[q].next_z;
                    if q == NIL {
                        break;
                    }
                }

                let mut q_size = in_size;

                while p_size > 0 || (q_size > 0 && q != NIL) {
                    let e;
                    if p_size == 0 {
                        e = q;
                        q = self.nodes[q].next_z;
                        q_size -= 1;
                    } else if q_size == 0 || q == NIL || self.nodes[p].z <= self.nodes[q].z {
                        e = p;
                        p = self.nodes[p].next_z;
                        p_size -= 1;
                    } else {
                        e = q;
                        q = self.nodes[q].next_z;
                        q_size -= 1;
                    }

                    if tail == NIL {
                        list = e;
                    } else {
                        self.nodes[tail].next_z = e;
                    }

                    self.nodes[e].prev_z = tail;
                    tail = e;
                }

                p = q;
            }

            self.nodes[tail].next_z = NIL;

            if num_merges <= 1 {
                return list;
            }

            in_size *= 2;
        }
    }

    // z-order of a Vertex given coords and size of the data bounding box
    fn z_order(&self, x_: f64, y_: f64) -> i32 {
        // coords are transformed into non-negative 15-bit integer range
        let mut x = (32767.0 * (x_ - self.min_x) * self.inv_size) as i32;
        let mut y = (32767.0 * (y_ - self.min_y) * self.inv_size) as i32;

        x = (x | (x << 8)) & 0x00FF_00FF;
        x = (x | (x << 4)) & 0x0F0F_0F0F;
        x = (x | (x << 2)) & 0x3333_3333;
        x = (x | (x << 1)) & 0x5555_5555;

        y = (y | (y << 8)) & 0x00FF_00FF;
        y = (y | (y << 4)) & 0x0F0F_0F0F;
        y = (y | (y << 2)) & 0x3333_3333;
        y = (y | (y << 1)) & 0x5555_5555;

        x | (y << 1)
    }

    // find the leftmost node of a polygon ring
    fn get_leftmost(&self, start: usize) -> usize {
        let mut p = start;
        let mut leftmost = start;
        loop {
            let (pn, ln) = (&self.nodes[p], &self.nodes[leftmost]);
            if pn.x < ln.x || (pn.x == ln.x && pn.y < ln.y) {
                leftmost = p;
            }
            p = pn.next;
            if p == start {
                break;
            }
        }

        leftmost
    }

    // check if a diagonal between two polygon nodes is valid (lies in polygon
    // interior)
    fn is_valid_diagonal(&self, a: usize, b: usize) -> bool {
        let (an, bn) = (&self.nodes[a], &self.nodes[b]);
        self.nodes[an.next].i != bn.i
            && self.nodes[an.prev].i != bn.i
            && !self.intersects_polygon(a, b) // dones't intersect other edges
            && ((self.locally_inside(a, b)
                && self.locally_inside(b, a)
                && self.middle_inside(a, b) // locally visible
                && (self.area(an.prev, a, bn.prev) != 0.0 || self.area(a, bn.prev, b) != 0.0)) // does not create opposite-facing sectors
                || (self.equals(a, b)
                    && self.area(an.prev, a, an.next) > 0.0
                    && self.area(bn.prev, b, bn.next) > 0.0)) // special zero-length case
    }

    // signed area of a triangle
    fn area(&self, p: usize, q: usize, r: usize) -> f64 {
        let (p, q, r) = (&self.nodes[p], &self.nodes[q], &self.nodes[r]);
        (q.y - p.y) * (r.x - q.x) - (q.x - p.x) * (r.y - q.y)
    }

    // check if two points are equal
    fn equals(&self, p1: usize, p2: usize) -> bool {
        let (p1, p2) = (&self.nodes[p1], &self.nodes[p2]);
        p1.x == p2.x && p1.y == p2.y
    }

    // check if two segments intersect
    fn intersects(&self, p1: usize, q1: usize, p2: usize, q2: usize) -> bool {
        let o1 = sign(self.area(p1, q1, p2));
        let o2 = sign(self.area(p1, q1, q2));
        let o3 = sign(self.area(p2, q2, p1));
        let o4 = sign(self.area(p2, q2, q1));

        if o1 != o2 && o3 != o4 {
            return true; // general case
        }

        if o1 == 0 && self.on_segment(p1, p2, q1) {
            return true; // p1, q1 and p2 are collinear and p2 lies on p1q1
        }
        if o2 == 0 && self.on_segment(p1, q2, q1) {
            return true; // p1, q1 and q2 are collinear and q2 lies on p1q1
        }
        if o3 == 0 && self.on_segment(p2, p1, q2) {
            return true; // p2, q2 and p1 are collinear and p1 lies on p2q2
        }
        if o4 == 0 && self.on_segment(p2, q1, q2) {
            return true; // p2, q2 and q1 are collinear and q1 lies on p2q2
        }

        false
    }

    // for collinear points p, q, r, check if point q lies on segment pr
    fn on_segment(&self, p: usize, q: usize, r: usize) -> bool {
        let (p, q, r) = (&self.nodes[p], &self.nodes[q], &self.nodes[r]);
        q.x <= max_f64(p.x, r.x) && q.x >= min_f64(p.x, r.x) && q.y <= max_f64(p.y, r.y) && q.y >= min_f64(p.y, r.y)
    }

    // check if a polygon diagonal intersects any polygon segments
    fn intersects_polygon(&self, a: usize, b: usize) -> bool {
        let (ai, bi) = (self.nodes[a].i, self.nodes[b].i);
        let mut p = a;
        loop {
            let pn = &self.nodes[p];
            let pni = self.nodes[pn.next].i;
            if pn.i != ai && pni != ai && pn.i != bi && pni != bi && self.intersects(p, pn.next, a, b) {
                return true;
            }
            p = pn.next;
            if p == a {
                break;
            }
        }

        false
    }

    // check if a polygon diagonal is locally inside the polygon
    fn locally_inside(&self, a: usize, b: usize) -> bool {
        let an = &self.nodes[a];
        if self.area(an.prev, a, an.next) < 0.0 {
            self.area(a, b, an.next) >= 0.0 && self.area(a, an.prev, b) >= 0.0
        } else {
            self.area(a, b, an.prev) < 0.0 || self.area(a, an.next, b) < 0.0
        }
    }

    // check if the middle Vertex of a polygon diagonal is inside the polygon
    fn middle_inside(&self, a: usize, b: usize) -> bool {
        let (an, bn) = (&self.nodes[a], &self.nodes[b]);
        let mut p = a;
        let mut inside = false;
        let px = f64::midpoint(an.x, bn.x);
        let py = f64::midpoint(an.y, bn.y);
        loop {
            let pn = &self.nodes[p];
            let nn = &self.nodes[pn.next];
            if ((pn.y > py) != (nn.y > py)) && nn.y != pn.y && (px < (nn.x - pn.x) * (py - pn.y) / (nn.y - pn.y) + pn.x)
            {
                inside = !inside;
            }
            p = pn.next;
            if p == a {
                break;
            }
        }

        inside
    }

    // link two polygon vertices with a bridge; if the vertices belong to the
    // same ring, it splits polygon into two; if one belongs to the outer ring
    // and another to a hole, it merges it into a single ring
    fn split_polygon(&mut self, a: usize, b: usize) -> usize {
        let a2 = self.new_node(self.nodes[a].i, self.nodes[a].x, self.nodes[a].y);
        let b2 = self.new_node(self.nodes[b].i, self.nodes[b].x, self.nodes[b].y);
        let an = self.nodes[a].next;
        let bp = self.nodes[b].prev;

        self.nodes[a].next = b;
        self.nodes[b].prev = a;

        self.nodes[a2].next = an;
        self.nodes[an].prev = a2;

        self.nodes[b2].next = a2;
        self.nodes[a2].prev = b2;

        self.nodes[bp].next = b2;
        self.nodes[b2].prev = bp;

        b2
    }

    fn new_node(&mut self, i: N, x: f64, y: f64) -> usize {
        self.nodes.push(Node { i, x, y, prev: NIL, next: NIL, z: 0, prev_z: NIL, next_z: NIL, steiner: false });
        self.nodes.len() - 1
    }

    // create a node and optionally link it with previous one (in a circular
    // doubly linked list)
    fn insert_node<P: Nth>(&mut self, i: usize, pt: &P, last: usize) -> usize {
        let p = self.new_node(N::from_usize(i), pt.nth0(), pt.nth1());

        if last == NIL {
            self.nodes[p].prev = p;
            self.nodes[p].next = p;
        } else {
            let last_next = self.nodes[last].next;
            self.nodes[p].next = last_next;
            self.nodes[p].prev = last;
            self.nodes[last_next].prev = p;
            self.nodes[last].next = p;
        }
        p
    }

    fn remove_node(&mut self, p: usize) {
        let Node { prev, next, prev_z, next_z, .. } = self.nodes[p];
        self.nodes[next].prev = prev;
        self.nodes[prev].next = next;

        if prev_z != NIL {
            self.nodes[prev_z].next_z = next_z;
        }
        if next_z != NIL {
            self.nodes[next_z].prev_z = prev_z;
        }
    }
}

// check if a point lies within a convex triangle
#[allow(clippy::too_many_arguments)]
fn point_in_triangle(ax: f64, ay: f64, bx: f64, by: f64, cx: f64, cy: f64, px: f64, py: f64) -> bool {
    (cx - px) * (ay - py) - (ax - px) * (cy - py) >= 0.0
        && (ax - px) * (by - py) - (bx - px) * (ay - py) >= 0.0
        && (bx - px) * (cy - py) - (cx - px) * (by - py) >= 0.0
}

fn sign(val: f64) -> i32 {
    i32::from(0.0 < val) - i32::from(val < 0.0)
}

/// `std::min<double>(a, b)`
fn min_f64(a: f64, b: f64) -> f64 {
    if b < a { b } else { a }
}

/// `std::max<double>(a, b)`
fn max_f64(a: f64, b: f64) -> f64 {
    if a < b { b } else { a }
}
