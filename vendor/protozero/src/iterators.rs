// Ports vendor/protozero/include/protozero/iterators.hpp.
//
// The C++ iterators are pointer pairs with operator* (decode), operator++
// (skip) and operator== (pointer identity). The ports keep those operations
// as methods (`deref`, `increment`, `post_increment`, `==`), so code like
// vector_tile.hpp's `*start_itr++` loops ports line by line, and they also
// implement Rust's `Iterator`.

use crate::exception::Exception;
use crate::varint::{decode_varint, decode_zigzag64, skip_varint};

/// A value type of `const_varint_iterator<T>`: `static_cast<T>` of the
/// decoded `uint64_t` (truncating for 32 bit types).
pub trait VarintValue: Copy {
    fn from_varint(value: u64) -> Self;
}

impl VarintValue for i32 {
    fn from_varint(value: u64) -> Self {
        value as i32
    }
}
impl VarintValue for u32 {
    fn from_varint(value: u64) -> Self {
        value as u32
    }
}
impl VarintValue for i64 {
    fn from_varint(value: u64) -> Self {
        value as i64
    }
}
impl VarintValue for u64 {
    fn from_varint(value: u64) -> Self {
        value
    }
}

/// A value type of `const_svarint_iterator<T>`: `static_cast<T>` of
/// `decode_zigzag64(...)` (truncating for `int32_t`).
pub trait SvarintValue: Copy {
    fn from_zigzag64(value: i64) -> Self;
}

impl SvarintValue for i32 {
    fn from_zigzag64(value: i64) -> Self {
        value as i32
    }
}
impl SvarintValue for i64 {
    fn from_zigzag64(value: i64) -> Self {
        value
    }
}

/// A value type of `const_fixed_iterator<T>`: `memcpy` of `sizeof(T)`
/// little-endian bytes (protozero byte-swaps on big-endian machines).
pub trait FixedValue: Copy {
    const SIZE: usize;
    fn from_bytes(bytes: &[u8]) -> Self;
}

macro_rules! fixed_value {
    ($($t:ty),*) => {$(
        impl FixedValue for $t {
            const SIZE: usize = std::mem::size_of::<$t>();
            fn from_bytes(bytes: &[u8]) -> Self {
                let mut a = [0u8; std::mem::size_of::<$t>()];
                a.copy_from_slice(&bytes[..std::mem::size_of::<$t>()]);
                <$t>::from_le_bytes(a)
            }
        }
    )*};
}
fixed_value!(u32, i32, u64, i64, f32, f64);

/// `iterator_range<T>`: a pair of iterators (C++ derives from
/// `std::pair<T, T>`; `first`/`second` are public there too).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct IteratorRange<T> {
    pub first: T,
    pub second: T,
}

/// `std::distance` for the protozero iterators (C++ specializes it).
pub trait Distance: Sized {
    fn distance(first: &Self, last: &Self) -> isize;
}

impl<T: Copy + PartialEq + Distance> IteratorRange<T> {
    pub fn new(first: T, last: T) -> Self {
        IteratorRange { first, second: last }
    }

    /// Return iterator to beginning of range.
    pub fn begin(&self) -> T {
        self.first
    }

    /// Return iterator to end of range.
    pub fn end(&self) -> T {
        self.second
    }

    pub fn cbegin(&self) -> T {
        self.first
    }

    pub fn cend(&self) -> T {
        self.second
    }

    /// Return true if this range is empty.
    pub fn empty(&self) -> bool {
        self.begin() == self.end()
    }

    /// Get the size of the range, i.e. the number of elements
    /// (`std::distance(begin(), end())`; for varint iterators the number of
    /// bytes without the continuation bit, without decoding).
    pub fn size(&self) -> usize {
        T::distance(&self.first, &self.second) as usize
    }

    /// Swap the contents of this range with the other.
    pub fn swap(&mut self, other: &mut Self) {
        std::mem::swap(self, other);
    }
}

impl<T: VarintValue> IteratorRange<ConstVarintIterator<'_, T>> {
    /// Get element at the beginning of the range (`front()`; the C++ asserts
    /// that the range is not empty).
    pub fn front(&self) -> Result<T, Exception> {
        debug_assert!(!self.empty());
        self.first.deref()
    }

    /// Advance beginning of range by one (`drop_front()`).
    pub fn drop_front(&mut self) -> Result<(), Exception> {
        debug_assert!(!self.empty());
        self.first.increment()
    }
}

impl<T: SvarintValue> IteratorRange<ConstSvarintIterator<'_, T>> {
    /// `front()`.
    pub fn front(&self) -> Result<T, Exception> {
        debug_assert!(!self.empty());
        self.first.deref()
    }

    /// `drop_front()`.
    pub fn drop_front(&mut self) -> Result<(), Exception> {
        debug_assert!(!self.empty());
        self.first.increment()
    }
}

impl<T: FixedValue> IteratorRange<ConstFixedIterator<'_, T>> {
    /// `front()`.
    #[must_use]
    pub fn front(&self) -> T {
        debug_assert!(!self.empty());
        self.first.deref()
    }

    /// `drop_front()`.
    pub fn drop_front(&mut self) {
        debug_assert!(!self.empty());
        self.first.increment();
    }
}

impl<'a, T: VarintValue> IntoIterator for IteratorRange<ConstVarintIterator<'a, T>> {
    type Item = Result<T, Exception>;
    type IntoIter = VarintRangeIter<'a, T>;
    fn into_iter(self) -> Self::IntoIter {
        VarintRangeIter { it: self.first, end: self.second, failed: false }
    }
}

impl<'a, T: SvarintValue> IntoIterator for IteratorRange<ConstSvarintIterator<'a, T>> {
    type Item = Result<T, Exception>;
    type IntoIter = SvarintRangeIter<'a, T>;
    fn into_iter(self) -> Self::IntoIter {
        SvarintRangeIter { it: self.first, end: self.second, failed: false }
    }
}

impl<'a, T: FixedValue> IntoIterator for IteratorRange<ConstFixedIterator<'a, T>> {
    type Item = T;
    type IntoIter = FixedRangeIter<'a, T>;
    fn into_iter(self) -> Self::IntoIter {
        FixedRangeIter { it: self.first, end: self.second }
    }
}

/// `const_fixed_iterator<T>`: a random-access iterator over fixed-size
/// values. C++ holds only a pointer; the port holds the packed field and a
/// byte offset into it, so dereferencing out of range panics where the C++
/// is undefined.
#[derive(Clone, Copy, Debug)]
pub struct ConstFixedIterator<'a, T> {
    buf: &'a [u8],
    pos: usize,
    _marker: std::marker::PhantomData<T>,
}

impl<T> Default for ConstFixedIterator<'_, T> {
    fn default() -> Self {
        ConstFixedIterator { buf: &[], pos: 0, _marker: std::marker::PhantomData }
    }
}

impl<'a, T: FixedValue> ConstFixedIterator<'a, T> {
    /// An iterator at byte offset `pos` of `buf`.
    #[must_use]
    pub fn new(buf: &'a [u8], pos: usize) -> Self {
        ConstFixedIterator { buf, pos, _marker: std::marker::PhantomData }
    }

    /// `operator*`.
    #[must_use]
    pub fn deref(&self) -> T {
        T::from_bytes(&self.buf[self.pos..self.pos + T::SIZE])
    }

    /// `operator++`.
    pub fn increment(&mut self) {
        self.pos += T::SIZE;
    }

    /// `operator++(int)`.
    #[must_use]
    pub fn post_increment(&mut self) -> Self {
        let tmp = *self;
        self.increment();
        tmp
    }

    /// `operator--`.
    pub fn decrement(&mut self) {
        self.pos -= T::SIZE;
    }

    /// `operator+(difference_type)` (also `+=` with `*it = it.add(n)`).
    #[must_use]
    pub fn add(&self, n: isize) -> Self {
        let mut tmp = *self;
        tmp.pos = (tmp.pos as isize + T::SIZE as isize * n) as usize;
        tmp
    }

    /// `operator[]`.
    #[must_use]
    pub fn index(&self, n: isize) -> T {
        self.add(n).deref()
    }

    /// `operator-(const_fixed_iterator, const_fixed_iterator)`.
    #[must_use]
    pub fn diff(&self, rhs: &Self) -> isize {
        (self.pos as isize - rhs.pos as isize) / T::SIZE as isize
    }

    fn addr(&self) -> usize {
        self.buf.as_ptr() as usize + self.pos
    }
}

impl<T> PartialEq for ConstFixedIterator<'_, T> {
    fn eq(&self, other: &Self) -> bool {
        self.buf.as_ptr() as usize + self.pos == other.buf.as_ptr() as usize + other.pos
    }
}

impl<T: FixedValue> PartialOrd for ConstFixedIterator<'_, T> {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        self.addr().partial_cmp(&other.addr())
    }
}

impl<T: FixedValue> Distance for ConstFixedIterator<'_, T> {
    fn distance(first: &Self, last: &Self) -> isize {
        last.diff(first)
    }
}

/// Rust iterator over an `iterator_range<const_fixed_iterator<T>>`.
pub struct FixedRangeIter<'a, T> {
    it: ConstFixedIterator<'a, T>,
    end: ConstFixedIterator<'a, T>,
}

impl<T: FixedValue> Iterator for FixedRangeIter<'_, T> {
    type Item = T;
    fn next(&mut self) -> Option<T> {
        if self.it == self.end {
            return None;
        }
        Some(self.it.post_increment().deref())
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let n = self.end.diff(&self.it).max(0) as usize;
        (n, Some(n))
    }
}

impl<T: FixedValue> ExactSizeIterator for FixedRangeIter<'_, T> {}

/// `const_varint_iterator<T>`: a forward iterator over varints. `data` is
/// the C++ [`m_data`, `m_end`).
#[derive(Clone, Copy, Debug)]
pub struct ConstVarintIterator<'a, T> {
    data: &'a [u8],
    _marker: std::marker::PhantomData<T>,
}

impl<T> Default for ConstVarintIterator<'_, T> {
    fn default() -> Self {
        ConstVarintIterator { data: &[], _marker: std::marker::PhantomData }
    }
}

impl<'a, T: VarintValue> ConstVarintIterator<'a, T> {
    /// `const_varint_iterator(data, end)` with `data` = [data, end).
    #[must_use]
    pub fn new(data: &'a [u8]) -> Self {
        ConstVarintIterator { data, _marker: std::marker::PhantomData }
    }

    /// `operator*`: decode the varint at the current position.
    pub fn deref(&self) -> Result<T, Exception> {
        let mut d = self.data; // will be thrown away
        Ok(T::from_varint(decode_varint(&mut d)?))
    }

    /// `operator++`: skip the varint at the current position.
    pub fn increment(&mut self) -> Result<(), Exception> {
        skip_varint(&mut self.data)
    }

    /// `operator++(int)`: returns the iterator before the increment.
    pub fn post_increment(&mut self) -> Result<Self, Exception> {
        let tmp = *self;
        self.increment()?;
        Ok(tmp)
    }

    /// The remaining bytes [`m_data`, `m_end`).
    #[must_use]
    pub fn data(&self) -> &'a [u8] {
        self.data
    }
}

impl<T> PartialEq for ConstVarintIterator<'_, T> {
    /// `m_data == rhs.m_data && m_end == rhs.m_end`.
    fn eq(&self, other: &Self) -> bool {
        std::ptr::eq(self.data.as_ptr(), other.data.as_ptr()) && self.data.len() == other.data.len()
    }
}

/// `const_varint_iterator::distance`: the number of bytes without the most
/// significant bit in [first, last), i.e. the number of varints if they are
/// well-formed, computed without decoding.
fn varint_distance(first: &[u8], last: &[u8]) -> isize {
    let start = first.as_ptr() as usize;
    let stop = last.as_ptr() as usize;
    let n = stop.saturating_sub(start).min(first.len());
    first[..n].iter().filter(|&&c| c & 0x80 == 0).count() as isize
}

impl<T: VarintValue> Distance for ConstVarintIterator<'_, T> {
    fn distance(first: &Self, last: &Self) -> isize {
        varint_distance(first.data, last.data)
    }
}

/// Rust iterator over an `iterator_range<const_varint_iterator<T>>`: each
/// step is the C++ `*it++` (skip, then decode the skipped varint). After an
/// error it ends.
pub struct VarintRangeIter<'a, T> {
    it: ConstVarintIterator<'a, T>,
    end: ConstVarintIterator<'a, T>,
    failed: bool,
}

impl<T: VarintValue> Iterator for VarintRangeIter<'_, T> {
    type Item = Result<T, Exception>;
    fn next(&mut self) -> Option<Self::Item> {
        if self.failed || self.it == self.end {
            return None;
        }
        let r = self.it.post_increment().and_then(|tmp| tmp.deref());
        self.failed = r.is_err();
        Some(r)
    }
}

/// `const_svarint_iterator<T>`: like `const_varint_iterator`, but
/// dereferencing zigzag-decodes.
#[derive(Clone, Copy, Debug)]
pub struct ConstSvarintIterator<'a, T> {
    data: &'a [u8],
    _marker: std::marker::PhantomData<T>,
}

impl<T> Default for ConstSvarintIterator<'_, T> {
    fn default() -> Self {
        ConstSvarintIterator { data: &[], _marker: std::marker::PhantomData }
    }
}

impl<'a, T: SvarintValue> ConstSvarintIterator<'a, T> {
    /// `const_svarint_iterator(data, end)` with `data` = [data, end).
    #[must_use]
    pub fn new(data: &'a [u8]) -> Self {
        ConstSvarintIterator { data, _marker: std::marker::PhantomData }
    }

    /// `operator*`.
    pub fn deref(&self) -> Result<T, Exception> {
        let mut d = self.data; // will be thrown away
        Ok(T::from_zigzag64(decode_zigzag64(decode_varint(&mut d)?)))
    }

    /// `operator++`.
    pub fn increment(&mut self) -> Result<(), Exception> {
        skip_varint(&mut self.data)
    }

    /// `operator++(int)`.
    pub fn post_increment(&mut self) -> Result<Self, Exception> {
        let tmp = *self;
        self.increment()?;
        Ok(tmp)
    }

    /// The remaining bytes [`m_data`, `m_end`).
    #[must_use]
    pub fn data(&self) -> &'a [u8] {
        self.data
    }
}

impl<T> PartialEq for ConstSvarintIterator<'_, T> {
    fn eq(&self, other: &Self) -> bool {
        std::ptr::eq(self.data.as_ptr(), other.data.as_ptr()) && self.data.len() == other.data.len()
    }
}

impl<T: SvarintValue> Distance for ConstSvarintIterator<'_, T> {
    fn distance(first: &Self, last: &Self) -> isize {
        varint_distance(first.data, last.data)
    }
}

/// Rust iterator over an `iterator_range<const_svarint_iterator<T>>`.
pub struct SvarintRangeIter<'a, T> {
    it: ConstSvarintIterator<'a, T>,
    end: ConstSvarintIterator<'a, T>,
    failed: bool,
}

impl<T: SvarintValue> Iterator for SvarintRangeIter<'_, T> {
    type Item = Result<T, Exception>;
    fn next(&mut self) -> Option<Self::Item> {
        if self.failed || self.it == self.end {
            return None;
        }
        let r = self.it.post_increment().and_then(|tmp| tmp.deref());
        self.failed = r.is_err();
        Some(r)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Ports vendor/protozero/test/unit/test_iterators.cpp.
    #[test]
    fn default_constructed_varint_iterators_are_equal() {
        let a = ConstVarintIterator::<u32>::default();
        let b = ConstVarintIterator::<u32>::default();
        let r = IteratorRange::<ConstVarintIterator<u32>>::default();
        assert_eq!(a, a);
        assert_eq!(a, b);
        assert_eq!(a, r.begin());
        assert_eq!(a, r.end());
        assert!(r.empty());
        assert_eq!(r.size(), 0);
        assert_eq!(r.begin(), r.end());
    }

    #[test]
    fn varint_range_size_counts_terminating_bytes() {
        let data = [0x01u8, 0x81, 0x01, 0xff, 0xff, 0x03, 0x80];
        let r = IteratorRange::new(
            ConstVarintIterator::<u32>::new(&data),
            ConstVarintIterator::<u32>::new(&data[data.len()..]),
        );
        // 0x80 at the end has the continuation bit: not counted.
        assert_eq!(r.size(), 3);
        let values: Vec<_> = r.into_iter().collect();
        assert_eq!(
            values,
            vec![Ok(1), Ok(0x81 & 0x7f | 1 << 7), Ok(0x7f | 0x7f << 7 | 3 << 14), Err(Exception::EndOfBuffer)]
        );
    }

    #[test]
    fn fixed_iterator_arithmetic() {
        let data = [1u8, 0, 0, 0, 2, 0, 0, 0, 3, 0, 0, 0];
        let begin = ConstFixedIterator::<u32>::new(&data, 0);
        let end = ConstFixedIterator::<u32>::new(&data, data.len());
        let r = IteratorRange::new(begin, end);
        assert_eq!(r.size(), 3);
        assert_eq!(begin.index(2), 3);
        assert_eq!(end.diff(&begin), 3);
        assert!(begin < end);
        assert_eq!(r.into_iter().collect::<Vec<_>>(), vec![1, 2, 3]);
    }
}
