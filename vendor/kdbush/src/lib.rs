// Ports vendor/kdbush.hpp/include/kdbush.hpp (kdbush::KDBush, kdbush::nth).
//
// `KDBush<TPoint, TIndex>` is generic over the point type (read through
// `nth<0/1, TPoint>`, which also fixes the number type `TNumber`) and the
// index type. The core instantiates `KDBush<Cluster, uint32_t>` with
// `double` coordinates (vendor/supercluster); kdbush's own test uses
// `std::pair<int, int>` with `size_t` indices. Index arithmetic is the
// C++ unsigned arithmetic (wrapping), and `TIndex(double)` in `select`
// converts as clang does on x86-64.

/// `nth<0, TPoint>::get` / `nth<1, TPoint>::get`.
pub trait Nth {
    type Number: Number;
    fn nth0(&self) -> Self::Number;
    fn nth1(&self) -> Self::Number;
}

/// `std::pair<T, T>` (the generic `nth` uses `std::get`).
impl<T: Number> Nth for (T, T) {
    type Number = T;
    fn nth0(&self) -> T {
        self.0
    }
    fn nth1(&self) -> T {
        self.1
    }
}

/// `TNumber`: the arithmetic `range`/`within` use.
pub trait Number: Copy + PartialOrd + std::fmt::Debug {
    #[must_use]
    fn add(self, other: Self) -> Self;
    #[must_use]
    fn sub(self, other: Self) -> Self;
    #[must_use]
    fn mul(self, other: Self) -> Self;
}

impl Number for f64 {
    fn add(self, other: Self) -> Self {
        self + other
    }
    fn sub(self, other: Self) -> Self {
        self - other
    }
    fn mul(self, other: Self) -> Self {
        self * other
    }
}

impl Number for f32 {
    fn add(self, other: Self) -> Self {
        self + other
    }
    fn sub(self, other: Self) -> Self {
        self - other
    }
    fn mul(self, other: Self) -> Self {
        self * other
    }
}

/// `int` arithmetic (overflow is UB in C++; wraps here).
impl Number for i32 {
    fn add(self, other: Self) -> Self {
        self.wrapping_add(other)
    }
    fn sub(self, other: Self) -> Self {
        self.wrapping_sub(other)
    }
    fn mul(self, other: Self) -> Self {
        self.wrapping_mul(other)
    }
}

/// `TIndex`: an unsigned index type.
pub trait Index: Copy + Ord + std::fmt::Debug {
    const ZERO: Self;
    const ONE: Self;
    /// `static_cast<TIndex>(n)` of a `size_t` (modular).
    fn from_usize(n: usize) -> Self;
    fn to_usize(self) -> usize;
    /// `static_cast<double>(TIndex)`
    fn to_f64(self) -> f64;
    /// `TIndex(double)`, saturating out of range.
    fn from_f64(v: f64) -> Self;
    #[must_use]
    fn wrapping_add(self, other: Self) -> Self;
    #[must_use]
    fn wrapping_sub(self, other: Self) -> Self;
    #[must_use]
    fn shr1(self) -> Self;
}

impl Index for u32 {
    const ZERO: Self = 0;
    const ONE: Self = 1;
    fn from_usize(n: usize) -> Self {
        n as u32
    }
    fn to_usize(self) -> usize {
        self as usize
    }
    fn to_f64(self) -> f64 {
        f64::from(self)
    }
    fn from_f64(v: f64) -> Self {
        v as u32
    }
    fn wrapping_add(self, other: Self) -> Self {
        u32::wrapping_add(self, other)
    }
    fn wrapping_sub(self, other: Self) -> Self {
        u32::wrapping_sub(self, other)
    }
    fn shr1(self) -> Self {
        self >> 1
    }
}

/// `size_t`
impl Index for usize {
    const ZERO: Self = 0;
    const ONE: Self = 1;
    fn from_usize(n: usize) -> Self {
        n
    }
    fn to_usize(self) -> usize {
        self
    }
    fn to_f64(self) -> f64 {
        self as f64
    }
    fn from_f64(v: f64) -> Self {
        v as usize
    }
    fn wrapping_add(self, other: Self) -> Self {
        usize::wrapping_add(self, other)
    }
    fn wrapping_sub(self, other: Self) -> Self {
        usize::wrapping_sub(self, other)
    }
    fn shr1(self) -> Self {
        self >> 1
    }
}

/// `KDBush<TPoint, TIndex>` (the points are copied in, as in C++).
#[derive(Clone, Debug)]
pub struct KDBush<N: Number, I: Index = usize> {
    pub(crate) ids: Vec<I>,
    pub(crate) points: Vec<(N, N)>,
    node_size: u8,
}

/// `defaultNodeSize`
pub(crate) const DEFAULT_NODE_SIZE: u8 = 64;

impl<N: Number, I: Index> Default for KDBush<N, I> {
    fn default() -> Self {
        Self::new(DEFAULT_NODE_SIZE)
    }
}

impl<N: Number, I: Index> KDBush<N, I> {
    /// `KDBush(nodeSize)`
    #[must_use]
    pub(crate) fn new(node_size: u8) -> Self {
        KDBush { ids: Vec::new(), points: Vec::new(), node_size }
    }

    /// `KDBush(points, nodeSize)`
    #[cfg(test)]
    pub(crate) fn from_points<P: Nth<Number = N>>(points: &[P], node_size: u8) -> Self {
        let mut index = Self::new(node_size);
        index.fill(points);
        index
    }

    /// `fill(points)`
    pub fn fill<P: Nth<Number = N>>(&mut self, points: &[P]) {
        debug_assert_eq!(self.points, Vec::<(N, N)>::new());
        let size = I::from_usize(points.len());

        if size == I::ZERO {
            return;
        }

        self.points.reserve(points.len());
        self.ids.reserve(points.len());

        let mut i = I::ZERO;
        for p in points {
            self.points.push((p.nth0(), p.nth1()));
            self.ids.push(i);
            i = i.wrapping_add(I::ONE);
        }

        self.sort_kd(I::ZERO, size.wrapping_sub(I::ONE), 0);
    }

    /// `range(minX, minY, maxX, maxY, visitor)`
    pub fn range(&self, min_x: N, min_y: N, max_x: N, max_y: N, visitor: &mut impl FnMut(I)) {
        self.range_impl(
            min_x,
            min_y,
            max_x,
            max_y,
            visitor,
            I::ZERO,
            I::from_usize(self.ids.len()).wrapping_sub(I::ONE),
            0,
        );
    }

    /// `within(qx, qy, r, visitor)`
    pub fn within(&self, qx: N, qy: N, r: N, visitor: &mut impl FnMut(I)) {
        self.within_impl(qx, qy, r, visitor, I::ZERO, I::from_usize(self.ids.len()).wrapping_sub(I::ONE), 0);
    }

    fn point(&self, i: I) -> (N, N) {
        self.points[i.to_usize()]
    }

    fn node_size(&self) -> I {
        I::from_usize(usize::from(self.node_size))
    }

    #[allow(clippy::too_many_arguments)]
    fn range_impl(
        &self,
        min_x: N,
        min_y: N,
        max_x: N,
        max_y: N,
        visitor: &mut impl FnMut(I),
        left: I,
        right: I,
        axis: u8,
    ) {
        if self.points.is_empty() {
            return;
        }

        if right.wrapping_sub(left) <= self.node_size() {
            let mut i = left;
            while i <= right {
                let (x, y) = self.point(i);
                if x >= min_x && x <= max_x && y >= min_y && y <= max_y {
                    visitor(self.ids[i.to_usize()]);
                }
                if i == right {
                    break; // `i++` past the end of a full-width range
                }
                i = i.wrapping_add(I::ONE);
            }
            return;
        }

        let m = left.wrapping_add(right).shr1();
        let (x, y) = self.point(m);

        if x >= min_x && x <= max_x && y >= min_y && y <= max_y {
            visitor(self.ids[m.to_usize()]);
        }

        if if axis == 0 { min_x <= x } else { min_y <= y } {
            self.range_impl(min_x, min_y, max_x, max_y, visitor, left, m.wrapping_sub(I::ONE), (axis + 1) % 2);
        }

        if if axis == 0 { max_x >= x } else { max_y >= y } {
            self.range_impl(min_x, min_y, max_x, max_y, visitor, m.wrapping_add(I::ONE), right, (axis + 1) % 2);
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn within_impl(&self, qx: N, qy: N, r: N, visitor: &mut impl FnMut(I), left: I, right: I, axis: u8) {
        if self.points.is_empty() {
            return;
        }

        let r2 = r.mul(r);

        if right.wrapping_sub(left) <= self.node_size() {
            let mut i = left;
            while i <= right {
                let (x, y) = self.point(i);
                if sq_dist(x, y, qx, qy) <= r2 {
                    visitor(self.ids[i.to_usize()]);
                }
                if i == right {
                    break;
                }
                i = i.wrapping_add(I::ONE);
            }
            return;
        }

        let m = left.wrapping_add(right).shr1();
        let (x, y) = self.point(m);

        if sq_dist(x, y, qx, qy) <= r2 {
            visitor(self.ids[m.to_usize()]);
        }

        if if axis == 0 { qx.sub(r) <= x } else { qy.sub(r) <= y } {
            self.within_impl(qx, qy, r, visitor, left, m.wrapping_sub(I::ONE), (axis + 1) % 2);
        }

        if if axis == 0 { qx.add(r) >= x } else { qy.add(r) >= y } {
            self.within_impl(qx, qy, r, visitor, m.wrapping_add(I::ONE), right, (axis + 1) % 2);
        }
    }

    fn sort_kd(&mut self, left: I, right: I, axis: u8) {
        if right.wrapping_sub(left) <= self.node_size() {
            return;
        }
        let m = left.wrapping_add(right).shr1();
        if axis == 0 {
            self.select::<0>(m, left, right);
        } else {
            self.select::<1>(m, left, right);
        }
        self.sort_kd(left, m.wrapping_sub(I::ONE), (axis + 1) % 2);
        self.sort_kd(m.wrapping_add(I::ONE), right, (axis + 1) % 2);
    }

    fn get<const A: u8>(&self, i: I) -> N {
        let p = self.point(i);
        if A == 0 { p.0 } else { p.1 }
    }

    fn select<const A: u8>(&mut self, k: I, mut left: I, mut right: I) {
        while right > left {
            if right.wrapping_sub(left).to_usize() > 600 {
                let n = right.wrapping_sub(left).wrapping_add(I::ONE).to_f64();
                let m = k.wrapping_sub(left).wrapping_add(I::ONE).to_f64();
                let z = n.ln();
                let s = 0.5 * (2.0 * z / 3.0).exp();
                let sign = if 2.0 * m < n { -1.0 } else { 1.0 };
                let r = k.to_f64() - m * s / n + 0.5 * (z * s * (1.0 - s / n)).sqrt() * sign;
                // std::max(left, TIndex(r)), std::min(right, TIndex(r + s))
                let new_left = {
                    let b = I::from_f64(r);
                    if left < b { b } else { left }
                };
                let new_right = {
                    let b = I::from_f64(r + s);
                    if b < right { b } else { right }
                };
                self.select::<A>(k, new_left, new_right);
            }

            let t = self.get::<A>(k);
            let mut i = left;
            let mut j = right;

            self.swap_item(left, k);
            if self.get::<A>(right) > t {
                self.swap_item(left, right);
            }

            while i < j {
                self.swap_item(i, j);
                i = i.wrapping_add(I::ONE);
                j = j.wrapping_sub(I::ONE);
                while self.get::<A>(i) < t {
                    i = i.wrapping_add(I::ONE);
                }
                while self.get::<A>(j) > t {
                    j = j.wrapping_sub(I::ONE);
                }
            }

            if self.get::<A>(left) == t {
                self.swap_item(left, j);
            } else {
                j = j.wrapping_add(I::ONE);
                self.swap_item(j, right);
            }

            if j <= k {
                left = j.wrapping_add(I::ONE);
            }
            if k <= j {
                right = j.wrapping_sub(I::ONE);
            }
        }
    }

    fn swap_item(&mut self, i: I, j: I) {
        self.ids.swap(i.to_usize(), j.to_usize());
        self.points.swap(i.to_usize(), j.to_usize());
    }
}

fn sq_dist<N: Number>(ax: N, ay: N, bx: N, by: N) -> N {
    let dx = ax.sub(bx);
    let dy = ay.sub(by);
    dx.mul(dx).add(dy.mul(dy))
}

#[cfg(test)]
mod tests;
