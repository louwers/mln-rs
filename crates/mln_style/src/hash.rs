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
#[cfg(not(mln_bench_fixed_hash))]
pub type RandomState = foldhash::fast::RandomState;

/// The benchmarks' builds (`tools/bench/profile.toml`) set the
/// `mln_bench_fixed_hash` cfg, which gives every map the same seed, so that
/// each process hashes alike and a benchmark does the same work every run.
#[cfg(mln_bench_fixed_hash)]
pub type RandomState = foldhash::fast::FixedState;

/// Whether [`RandomState`] has a fixed seed, as in the benchmarks' builds.
#[doc(hidden)]
pub const SEED_IS_FIXED: bool = cfg!(mln_bench_fixed_hash);

/// A [`std::collections::HashMap`] with mln-rs's hasher. Create one with
/// `default()`, or `with_capacity_and_hasher(n, RandomState::default())`.
#[expect(clippy::disallowed_types, reason = "the one place that names it")]
pub type HashMap<K, V> = std::collections::HashMap<K, V, RandomState>;

/// A [`std::collections::HashSet`] with mln-rs's hasher. Create one with
/// `default()`, or `with_capacity_and_hasher(n, RandomState::default())`.
#[expect(clippy::disallowed_types, reason = "the one place that names it")]
pub type HashSet<T> = std::collections::HashSet<T, RandomState>;

/// A hash of `bytes` that is the same for the same bytes throughout the
/// process, to tell whether data changed without keeping a copy of it.
#[must_use]
pub fn hash_bytes(bytes: &[u8]) -> u64 {
    use std::hash::BuildHasher;
    foldhash::quality::FixedState::default().hash_one(bytes)
}
