// Ports test/util/hash.test.cpp (`util::order_independent_hash` is in
// mln_gfx::util_hash, next to the other util/hash.hpp functions gfx uses).
// Its Metal shader test is in crates/mln_mtl/tests/gtest_hash.rs.
//
// The C++ draws the values with `std::default_random_engine` seeded from a
// `std::seed_seq` and `uniform_int_distribution<size_t>(0, maxVal)`; the
// values only decide which collections are tried, so a small LCG stands in
// (same seeds, same counts). `std::next_permutation` is ported below.

use mln_gfx::util_hash::order_independent_hash;

/// `std::next_permutation`: rearranges into the next lexicographically
/// greater permutation; false (and sorted ascending) after the last one.
fn next_permutation(values: &mut [u64]) -> bool {
    if values.len() < 2 {
        return false;
    }
    let mut i = values.len() - 1;
    while i > 0 && values[i - 1] >= values[i] {
        i -= 1;
    }
    if i == 0 {
        values.reverse();
        return false;
    }
    let mut j = values.len() - 1;
    while values[j] <= values[i - 1] {
        j -= 1;
    }
    values.swap(i - 1, j);
    values[i..].reverse();
    true
}

// gtest: OrderIndependentHash.Permutations
#[test]
fn permutations() {
    // Try collections of up to this many elements
    const MAX_SIZE: usize = 6;
    // Try this many different sets of values for each size
    const ITERATIONS: usize = 10;
    // Use values up to this large
    const MAX_VAL: u64 = 10;
    // Repeat all that with different random seeds
    const SEEDS: u64 = 5;

    for cur_seed in 0..SEEDS {
        let mut engine: u64 = 0xf00d + cur_seed;
        let mut distribution = move || {
            engine = engine.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
            (engine >> 33) % (MAX_VAL + 1)
        };

        for cur_size in 1..=MAX_SIZE {
            for _ in 0..ITERATIONS {
                // Generate `curSize` random values
                let mut values: Vec<u64> = (0..cur_size).map(|_| distribution()).collect();

                // Generate the hash for each permutation, comparing subsequent values to the first one.
                let mut is_first = true;
                let mut initial_value = 0;
                loop {
                    let cur_value = order_independent_hash(values.iter().copied());
                    if is_first {
                        initial_value = cur_value;
                        is_first = false;
                    } else {
                        assert_eq!(initial_value, cur_value, "{values:?}");
                    }
                    if !next_permutation(&mut values) {
                        break;
                    }
                }
            }
        }
    }
}
