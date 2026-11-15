//! The hash maps and sets of mln-rs.
//!
//! Their keys include layer IDs, source IDs and feature properties from
//! styles and tiles off the network, so the hasher has to resist keys crafted
//! to collide. foldhash's `RandomState` seeds each process from its address
//! space layout and the clock, and hashes short keys much faster than std's
//! default hasher. In the browser's wasm32-unknown-unknown, where there is no
//! clock or address randomization to read, its seed comes from addresses
//! alone, as std's does there.

/// The [`BuildHasher`](std::hash::BuildHasher) of [`HashMap`] and [`HashSet`].
pub type RandomState = foldhash::fast::RandomState;

/// A [`std::collections::HashMap`] with mln-rs's hasher. Create one with
/// `default()`, or `with_capacity_and_hasher(n, RandomState::default())`.
#[expect(clippy::disallowed_types, reason = "the one place that names it")]
pub type HashMap<K, V> = std::collections::HashMap<K, V, RandomState>;

/// A [`std::collections::HashSet`] with mln-rs's hasher. Create one with
/// `default()`, or `with_capacity_and_hasher(n, RandomState::default())`.
#[expect(clippy::disallowed_types, reason = "the one place that names it")]
pub type HashSet<T> = std::collections::HashSet<T, RandomState>;
