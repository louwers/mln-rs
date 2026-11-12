// Ports the libstdc++ (GCC's bits/stl_algo.h and bits/stl_heap.h)
// implementation of `std::nth_element` with a comparator, which the C++ core
// gets on Linux. The standard leaves the order of the elements other than
// the nth unspecified; `limitHoles` keeps whatever order this leaves, so the
// port reproduces libstdc++'s introselect step by step.
//
// Moves are modelled with `std::mem::take` (the moved-from element is left
// as `T::default()` and always overwritten before it is read, as in C++).

/// `std::__lg`: floor(log2(n)) for n > 0.
fn lg(n: usize) -> usize {
    (usize::BITS - 1 - n.leading_zeros()) as usize
}

/// `std::nth_element(first, nth, last, comp)` on `v` with `nth` an index.
pub fn nth_element<T: Default, F: FnMut(&T, &T) -> bool>(v: &mut [T], nth: usize, mut comp: F) {
    let len = v.len();
    if len == 0 || nth == len {
        return;
    }
    introselect(v, 0, nth, len, lg(len) * 2, &mut comp);
}

fn introselect<T: Default, F: FnMut(&T, &T) -> bool>(
    v: &mut [T],
    mut first: usize,
    nth: usize,
    mut last: usize,
    mut depth_limit: usize,
    comp: &mut F,
) {
    while last - first > 3 {
        if depth_limit == 0 {
            heap_select(v, first, nth + 1, last, comp);
            // Place the nth largest element in its final position.
            v.swap(first, nth);
            return;
        }
        depth_limit -= 1;
        let cut = unguarded_partition_pivot(v, first, last, comp);
        if cut <= nth {
            first = cut;
        } else {
            last = cut;
        }
    }
    insertion_sort(v, first, last, comp);
}

/// `std::__move_median_to_first`: swaps the median of `a`, `b`, `c` to
/// `result`.
fn move_median_to_first<T, F: FnMut(&T, &T) -> bool>(
    v: &mut [T],
    result: usize,
    a: usize,
    b: usize,
    c: usize,
    comp: &mut F,
) {
    if comp(&v[a], &v[b]) {
        if comp(&v[b], &v[c]) {
            v.swap(result, b);
        } else if comp(&v[a], &v[c]) {
            v.swap(result, c);
        } else {
            v.swap(result, a);
        }
    } else if comp(&v[a], &v[c]) {
        v.swap(result, a);
    } else if comp(&v[b], &v[c]) {
        v.swap(result, c);
    } else {
        v.swap(result, b);
    }
}

/// `std::__unguarded_partition`.
fn unguarded_partition<T, F: FnMut(&T, &T) -> bool>(
    v: &mut [T],
    mut first: usize,
    mut last: usize,
    pivot: usize,
    comp: &mut F,
) -> usize {
    loop {
        while comp(&v[first], &v[pivot]) {
            first += 1;
        }
        last -= 1;
        while comp(&v[pivot], &v[last]) {
            last -= 1;
        }
        if first >= last {
            return first;
        }
        v.swap(first, last);
        first += 1;
    }
}

/// `std::__unguarded_partition_pivot`.
fn unguarded_partition_pivot<T, F: FnMut(&T, &T) -> bool>(
    v: &mut [T],
    first: usize,
    last: usize,
    comp: &mut F,
) -> usize {
    let mid = first + (last - first) / 2;
    move_median_to_first(v, first, first + 1, mid, last - 1, comp);
    unguarded_partition(v, first + 1, last, first, comp)
}

/// `std::__insertion_sort`.
fn insertion_sort<T: Default, F: FnMut(&T, &T) -> bool>(v: &mut [T], first: usize, last: usize, comp: &mut F) {
    if first == last {
        return;
    }
    for i in first + 1..last {
        if comp(&v[i], &v[first]) {
            // move_backward(first, i, i + 1); *first = val
            v[first..=i].rotate_right(1);
        } else {
            unguarded_linear_insert(v, i, comp);
        }
    }
}

/// `std::__unguarded_linear_insert`.
fn unguarded_linear_insert<T: Default, F: FnMut(&T, &T) -> bool>(v: &mut [T], mut last: usize, comp: &mut F) {
    let val = std::mem::take(&mut v[last]);
    let mut next = last - 1;
    while comp(&val, &v[next]) {
        v[last] = std::mem::take(&mut v[next]);
        last = next;
        next = next.wrapping_sub(1);
    }
    v[last] = val;
}

/// `std::__heap_select`.
fn heap_select<T: Default, F: FnMut(&T, &T) -> bool>(
    v: &mut [T],
    first: usize,
    middle: usize,
    last: usize,
    comp: &mut F,
) {
    make_heap(v, first, middle, comp);
    let mut i = middle;
    while i < last {
        if comp(&v[i], &v[first]) {
            pop_heap(v, first, middle, i, comp);
        }
        i += 1;
    }
}

/// `std::__make_heap`.
fn make_heap<T: Default, F: FnMut(&T, &T) -> bool>(v: &mut [T], first: usize, last: usize, comp: &mut F) {
    if last - first < 2 {
        return;
    }
    let len = last - first;
    let mut parent = (len - 2) / 2;
    loop {
        let value = std::mem::take(&mut v[first + parent]);
        adjust_heap(v, first, parent, len, value, comp);
        if parent == 0 {
            return;
        }
        parent -= 1;
    }
}

/// `std::__pop_heap`.
fn pop_heap<T: Default, F: FnMut(&T, &T) -> bool>(v: &mut [T], first: usize, last: usize, result: usize, comp: &mut F) {
    let value = std::mem::take(&mut v[result]);
    v[result] = std::mem::take(&mut v[first]);
    adjust_heap(v, first, 0, last - first, value, comp);
}

/// `std::__adjust_heap` (indices relative to `first`).
fn adjust_heap<T: Default, F: FnMut(&T, &T) -> bool>(
    v: &mut [T],
    first: usize,
    mut hole_index: usize,
    len: usize,
    value: T,
    comp: &mut F,
) {
    let top_index = hole_index;
    let mut second_child = hole_index;
    while second_child < (len - 1) / 2 {
        second_child = 2 * (second_child + 1);
        if comp(&v[first + second_child], &v[first + (second_child - 1)]) {
            second_child -= 1;
        }
        v[first + hole_index] = std::mem::take(&mut v[first + second_child]);
        hole_index = second_child;
    }
    if (len & 1) == 0 && second_child == (len - 2) / 2 {
        second_child = 2 * (second_child + 1);
        v[first + hole_index] = std::mem::take(&mut v[first + (second_child - 1)]);
        hole_index = second_child - 1;
    }
    push_heap(v, first, hole_index, top_index, value, comp);
}

/// `std::__push_heap` (indices relative to `first`).
fn push_heap<T, F: FnMut(&T, &T) -> bool>(
    v: &mut [T],
    first: usize,
    mut hole_index: usize,
    top_index: usize,
    value: T,
    comp: &mut F,
) {
    // C++ `(__holeIndex - 1) / 2` on a signed distance: 0 for hole 0.
    let parent_of = |i: usize| if i == 0 { 0 } else { (i - 1) / 2 };
    let mut parent = parent_of(hole_index);
    while hole_index > top_index && comp(&v[first + parent], &value) {
        v.swap(first + hole_index, first + parent);
        hole_index = parent;
        parent = parent_of(hole_index);
    }
    v[first + hole_index] = value;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nth_element_places_the_nth() {
        for n in 0..40usize {
            let base: Vec<i32> = (0..n as i32).map(|i| (i * 7919) % 31 - 15).collect();
            for nth in 0..n {
                let mut v = base.clone();
                nth_element(&mut v, nth, |a, b| a < b);
                let mut sorted = base.clone();
                sorted.sort_unstable();
                assert_eq!(v[nth], sorted[nth]);
                assert!(v[..nth].iter().all(|x| *x <= v[nth]));
                assert!(v[nth + 1..].iter().all(|x| *x >= v[nth]));
            }
        }
    }
}
