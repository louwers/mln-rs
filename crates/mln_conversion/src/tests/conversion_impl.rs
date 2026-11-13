use std::cell::Cell;
use std::rc::Rc;

use feature::Value;

use crate::conversion::{ConversionTraits, Convertible, Error};

/// Counts its destructions.
struct MockConvertible {
    counter: Rc<Cell<i32>>,
}

impl Drop for MockConvertible {
    fn drop(&mut self) {
        self.counter.set(self.counter.get() + 1);
    }
}

impl ConversionTraits for MockConvertible {
    fn is_undefined(&self) -> bool {
        false
    }
    fn is_array(&self) -> bool {
        false
    }
    fn array_length(&self) -> usize {
        0
    }
    fn array_member(&self, _i: usize) -> Convertible<'_> {
        Convertible::owned(MockConvertible { counter: Rc::new(Cell::new(0)) })
    }
    fn is_object(&self) -> bool {
        false
    }
    fn object_member(&self, _name: &str) -> Option<Convertible<'_>> {
        None
    }
    fn each_member(&self, _f: &mut dyn FnMut(&str, &Convertible<'_>) -> Option<Error>) -> Option<Error> {
        None
    }
    fn to_bool(&self) -> Option<bool> {
        None
    }
    fn to_number(&self) -> Option<f32> {
        None
    }
    fn to_double(&self) -> Option<f64> {
        None
    }
    fn to_string(&self) -> Option<String> {
        None
    }
    fn to_value(&self) -> Option<Value> {
        None
    }
}

// gtest: Conversion.Move
#[test]
// `b = a; a = b` are the C++ test's two moves, not a swap.
#[allow(clippy::almost_swapped)]
fn move_() {
    let dtor_counter = Rc::new(Cell::new(0));

    {
        let mock = MockConvertible { counter: dtor_counter.clone() };
        let mut a = Convertible::owned(mock);
        let b = a;
        a = b;
        // C++ also self-move-assigns `a = std::move(*c)` with `c = &a`; safe
        // Rust cannot alias `a` that way.
        let _ = &a;
    }

    // C++ counts 4: its moves leave moved-from MockConvertibles (the local
    // `mock` and the storage of each moved-from Convertible) that are
    // destroyed too. Rust moves are bitwise and leave nothing behind, so the
    // one wrapped value is destroyed exactly once.
    assert_eq!(dtor_counter.get(), 1);
}
