// Ports the parts of include/mln/util/hash.hpp that gfx uses (`util::hash`,
// `util::hash_combine`): boost's hash_combine over the values' hashes.
//
// The hashes are only used as cache keys (pipelines, shader names), so the
// exact values need not match C++; the combination formula does.

/// `util::hash_combine(seed, v)` for an already-hashed `v`
/// (boost: `seed ^= v + 0x9e3779b9 + (seed << 6) + (seed >> 2)`).
pub fn hash_combine(seed: &mut u64, v: u64) {
    *seed ^= v.wrapping_add(0x9e37_79b9).wrapping_add(*seed << 6).wrapping_add(*seed >> 2);
}

/// `util::hash(args...)`: fold `hash_combine` over the arguments from seed 0.
#[must_use]
pub fn hash(values: &[u64]) -> u64 {
    let mut seed = 0u64;
    for &v in values {
        hash_combine(&mut seed, v);
    }
    seed
}

/// `std::hash<std::string>` stand-in (FNV-1a; deterministic across runs).
#[must_use]
pub fn hash_str(s: &str) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in s.bytes() {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}
