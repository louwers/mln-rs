// Ports include/mln/util/immutable.hpp.
//
// C++ `Immutable<T>` is a shared pointer to a const T, `Mutable<T>` a unique,
// writable one that is frozen by moving it into an `Immutable<T>`. In Rust
// the mutable phase is a plain owned `T` (or `Box<T>` for trait objects) and
// the frozen form is an `Arc<T>`; identity diffing is `Arc::ptr_eq`.

pub type Immutable<T> = std::sync::Arc<T>;
