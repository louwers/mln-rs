//! The hash maps of gfx and the backends, and the combination of hashes that
//! their cache keys use.

// The hashes are only used as cache keys (pipelines, shader names), so the
// exact values need not match C++; the combination formula does.

/// Combines an already-hashed `v` into `seed`, as boost does:
/// `seed ^= v + 0x9e3779b9 + (seed << 6) + (seed >> 2)`.
pub fn hash_combine(seed: &mut u64, v: u64) {
    *seed ^= v.wrapping_add(0x9e37_79b9).wrapping_add(*seed << 6).wrapping_add(*seed >> 2);
}

/// Folds `hash_combine` over `values`, from seed 0.
#[must_use]
pub fn hash(values: &[u64]) -> u64 {
    let mut seed = 0u64;
    for &v in values {
        hash_combine(&mut seed, v);
    }
    seed
}

/// The hasher of [`HashMap`] and [`HashSet`]: foldhash's, seeded for each
/// process. The benchmarks' builds (`tools/bench/profile.toml`) set the
/// `mln_bench_fixed_hash` cfg, which gives every map the same seed, so that a
/// benchmark does the same work every run.
#[cfg(not(mln_bench_fixed_hash))]
pub type RandomState = foldhash::fast::RandomState;
#[cfg(mln_bench_fixed_hash)]
pub type RandomState = foldhash::fast::FixedState;

/// A [`std::collections::HashMap`] with [`RandomState`].
#[expect(clippy::disallowed_types, reason = "the alias that names it")]
pub type HashMap<K, V> = std::collections::HashMap<K, V, RandomState>;

/// A [`std::collections::HashSet`] with [`RandomState`].
#[expect(clippy::disallowed_types, reason = "the alias that names it")]
pub type HashSet<T> = std::collections::HashSet<T, RandomState>;
