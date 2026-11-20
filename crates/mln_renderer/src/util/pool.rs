//! Ranges into flat pools of a bucket's per-symbol data.

/// `len` items of a pool from `start`.
///
/// A symbol keeps ranges into a few long `Vec`s that its bucket owns, rather
/// than many short `Vec`s of its own, so that building, moving and freeing a
/// bucket takes a few allocations rather than several per symbol. A range is
/// valid only against the pool it was made for: another pool of the same type
/// gives wrong items, which debug builds catch only when they are out of
/// bounds.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, mln_wire::Wire)]
pub(crate) struct PoolRange {
    start: u32,
    len: u32,
}

impl PoolRange {
    /// An empty range at the end of `pool`, which [`push`](Self::push) grows.
    pub(crate) fn at_end<T>(pool: &[T]) -> Self {
        debug_assert!(u32::try_from(pool.len()).is_ok());
        PoolRange { start: pool.len() as u32, len: 0 }
    }

    /// Appends `items` to `pool`, and returns their range.
    pub(crate) fn extend<T>(pool: &mut Vec<T>, items: impl IntoIterator<Item = T>) -> Self {
        let mut range = PoolRange::at_end(pool);
        pool.extend(items);
        debug_assert!(u32::try_from(pool.len()).is_ok());
        range.len = pool.len() as u32 - range.start;
        range
    }

    /// Appends `item` to `pool` as this range's last item. The range must end
    /// where `pool` does.
    pub(crate) fn push<T>(&mut self, pool: &mut Vec<T>, item: T) {
        debug_assert_eq!(self.start as usize + self.len as usize, pool.len());
        pool.push(item);
        self.len += 1;
    }

    pub(crate) fn len(self) -> usize {
        self.len as usize
    }

    /// This range's items of `pool`.
    pub(crate) fn of<T>(self, pool: &[T]) -> &[T] {
        debug_assert!(self.start as usize + self.len as usize <= pool.len());
        &pool[self.start as usize..][..self.len as usize]
    }
}
