// Ports vendor/wagyu/include/mapbox/geometry/wagyu/local_minimum.hpp. The
// `std::deque<local_minimum>` is a Vec; the left and right bounds are in the
// bound arena.

use std::cmp::Ordering;

use crate::bound::BoundPtr;

/// `local_minimum<int32_t>`.
#[derive(Clone, Debug)]
pub(crate) struct LocalMinimum {
    pub left_bound: BoundPtr,
    pub right_bound: BoundPtr,
    pub y: i32,
    pub minimum_has_horizontal: bool,
}

/// `local_minimum_sorter`: by descending y, then the minima with a
/// horizontal edge first.
pub(crate) fn local_minimum_sorter(lm1: &LocalMinimum, lm2: &LocalMinimum) -> Ordering {
    lm2.y.cmp(&lm1.y).then(lm2.minimum_has_horizontal.cmp(&lm1.minimum_has_horizontal))
}
