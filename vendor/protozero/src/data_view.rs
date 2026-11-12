// Ports vendor/protozero/include/protozero/data_view.hpp.

/// `protozero::data_view`: a non-owning view of a byte range (pointer +
/// size). A Rust slice is exactly that: `data()` is `as_ptr()`, `size()` is
/// `len()`, `empty()` is `is_empty()`, `to_string()`/`std::string(view)` is
/// `to_vec()`, and `==`, `<`, `compare` are the slice comparisons (memcmp,
/// then the shorter view first), as in C++.
pub type DataView<'a> = &'a [u8];

/// `data_view::compare`: memcmp of the common prefix, then size; returns
/// <0, 0 or >0 as in C++.
#[must_use]
pub fn compare(lhs: DataView<'_>, rhs: DataView<'_>) -> i32 {
    match lhs.cmp(rhs) {
        std::cmp::Ordering::Less => -1,
        std::cmp::Ordering::Equal => 0,
        std::cmp::Ordering::Greater => 1,
    }
}

#[cfg(test)]
mod tests {
    // Ports the comparison cases of vendor/protozero/test/unit/test_data_view.cpp.
    use super::*;

    #[test]
    fn comparing_data_views() {
        let v1: DataView = b"foo";
        let v2: DataView = b"bar";
        let v3: DataView = b"foox";
        let v4: DataView = b"foo";
        let v5: DataView = b"fooooooo";
        let v6: DataView = b"f";

        assert_eq!(v1, v1);
        assert_ne!(v1, v2);
        assert_ne!(v1, v3);
        assert_eq!(v1, v4);
        assert_ne!(v1, v5);
        assert_ne!(v1, v6);

        assert_eq!(compare(v1, v1), 0);
        assert!(compare(v1, v2) > 0);
        assert!(compare(v1, v3) < 0);
        assert_eq!(compare(v1, v4), 0);
        assert!(compare(v1, v5) < 0);
        assert!(compare(v1, v6) > 0);
        assert!(compare(v2, v1) < 0);
        assert!(compare(v3, v1) > 0);
        assert!(compare(v6, v1) < 0);
        assert!(v1 > v2 && v1 < v3 && v1 <= v4 && v1 >= v4);
    }

    #[test]
    fn default_constructed_data_view() {
        let view: DataView = Default::default();
        assert_eq!(view, []);
        assert_eq!(view.len(), 0);
        assert_eq!(compare(view, view), 0);
    }
}
