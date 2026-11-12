// Ports test/util/tiny_map.test.cpp.
//
// The C++ templates over the linear threshold are generic functions over a
// const parameter; `testing::ScopedTrace` is the threshold in the assertion
// messages. A C++ move leaves an empty map behind, which is `mem::take`.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::hash::Hash;
use std::time::{Duration, Instant};

use mln_tile_id::{CanonicalTileID, OverscaledTileID};
use mln_util::logging::{Event, Log};
use mln_util::tiny_unordered_map::TinyUnorderedMap;

type IntMap<const T: usize> = TinyUnorderedMap<i32, i32, T>;

fn test_init<const THRESHOLD: usize>() {
    // Construct from separate initializer lists
    let map = IntMap::<THRESHOLD>::from_keys_values([1, 2, 3], [3, 2, 1]);
    assert_eq!(3, map.size(), "threshold {THRESHOLD}");

    // Construct from list of pairs
    let map2 = IntMap::<THRESHOLD>::from_pairs([(1, 3), (2, 2), (3, 1)]);
    assert_eq!(map.size(), map2.size(), "threshold {THRESHOLD}");
    // testSetDiff(map, map2);
    assert!(map.find(&1).is_some() && map2.find(&1).is_some(), "threshold {THRESHOLD}");
    assert!(map.find(&2).is_some() && map2.find(&2).is_some(), "threshold {THRESHOLD}");
    assert!(map.find(&3).is_some() && map2.find(&3).is_some(), "threshold {THRESHOLD}");

    // Construct in a different order
    let map3 = IntMap::<THRESHOLD>::from_keys_values([2, 3, 1], [2, 1, 3]);
    assert_eq!(map.size(), map3.size(), "threshold {THRESHOLD}");
    // testSetDiff(map, map3);
    assert!(map3.find(&1).is_some(), "threshold {THRESHOLD}");
    assert!(map3.find(&2).is_some(), "threshold {THRESHOLD}");
    assert!(map3.find(&3).is_some(), "threshold {THRESHOLD}");
}

// gtest: TinyMap.Init
#[test]
fn init() {
    test_init::<0>();
    test_init::<2>();
    test_init::<5>();
    test_init::<10>();
}

fn test_copy<const THRESHOLD: usize>() {
    let map = IntMap::<THRESHOLD>::from_keys_values([1, 2, 3], [3, 2, 1]);
    let map2 = map.clone();
    let mut map3 = IntMap::<THRESHOLD>::default();
    assert!(map3.empty());
    map3 = map2.clone();
    // testSetDiff(map, map2);
    // testSetDiff(map, map3);
    assert_eq!(map.size(), map2.size(), "threshold {THRESHOLD}");
    assert_eq!(map.size(), map3.size(), "threshold {THRESHOLD}");
    for k in [1, 2, 3] {
        assert!(map.find(&k).is_some() && map2.find(&k).is_some() && map3.find(&k).is_some(), "threshold {THRESHOLD}");
    }
}

// gtest: TinyMap.Copy
#[test]
fn copy() {
    test_copy::<0>();
    test_copy::<2>();
    test_copy::<5>();
    test_copy::<10>();
}

fn test_gen_init<const THRESHOLD: usize>() {
    let input = [1, 2, 3];
    let map = IntMap::<THRESHOLD>::from_generator(input.iter(), |x| (*x, 4 - x));
    assert_eq!(3, map.size(), "threshold {THRESHOLD}");
    assert_eq!(Some(&3), map.find(&1), "threshold {THRESHOLD}");
    assert_eq!(Some(&2), map.find(&2), "threshold {THRESHOLD}");
    assert_eq!(Some(&1), map.find(&3), "threshold {THRESHOLD}");
}

// gtest: TinyMap.GenInit
#[test]
fn gen_init() {
    test_gen_init::<0>();
    test_gen_init::<2>();
    test_gen_init::<5>();
    test_gen_init::<10>();
}

fn test_move<const THRESHOLD: usize>() {
    let map = IntMap::<THRESHOLD>::from_keys_values([1, 2, 3], [3, 2, 1]);
    let mut map2 = map.clone();

    // move constructor
    let mut map3 = std::mem::take(&mut map2);
    assert!(map2.empty(), "threshold {THRESHOLD}");
    // testSetDiff(map, map3);
    assert!(map3.find(&1).is_some(), "threshold {THRESHOLD}");
    assert!(map3.find(&2).is_some(), "threshold {THRESHOLD}");
    assert!(map3.find(&3).is_some(), "threshold {THRESHOLD}");

    // move assignment
    let mut map4 = IntMap::<THRESHOLD>::default();
    assert!(map4.empty());
    map4 = std::mem::take(&mut map3);
    assert!(map3.empty(), "threshold {THRESHOLD}");
    // testSetDiff(map, map4);
    assert!(map4.find(&1).is_some(), "threshold {THRESHOLD}");
    assert!(map4.find(&2).is_some(), "threshold {THRESHOLD}");
    assert!(map4.find(&3).is_some(), "threshold {THRESHOLD}");

    map4.clear();
    assert!(map4.empty(), "threshold {THRESHOLD}");
}

// gtest: TinyMap.Move
#[test]
fn move_() {
    test_move::<0>();
    test_move::<2>();
    test_move::<5>();
    test_move::<10>();
}

fn test_const_lookup<const THRESHOLD: usize>() {
    let map = IntMap::<THRESHOLD>::from_keys_values([2, 4, 6], [3, 2, 1]);
    assert_eq!(3, *map.find(&2).unwrap(), "threshold {THRESHOLD}");
    assert_eq!(2, *map.find(&4).unwrap(), "threshold {THRESHOLD}");
    assert_eq!(1, *map.find(&6).unwrap(), "threshold {THRESHOLD}");
}

// gtest: TinyMap.ConstLookup
#[test]
fn const_lookup() {
    test_const_lookup::<0>();
    test_const_lookup::<2>();
    test_const_lookup::<5>();
    test_const_lookup::<10>();
}

fn test_mutable_lookup<const THRESHOLD: usize>() {
    let mut map = IntMap::<THRESHOLD>::from_keys_values([2, 4, 6], [3, 2, 1]);
    assert_eq!(3, *map.find_mut(&2).unwrap(), "threshold {THRESHOLD}");
    assert_eq!(2, *map.find_mut(&4).unwrap(), "threshold {THRESHOLD}");
    assert_eq!(1, *map.find_mut(&6).unwrap(), "threshold {THRESHOLD}");

    // operator[] doesn't insert... for now
    assert!(map.find_mut(&7).is_none(), "threshold {THRESHOLD}");
    assert!(map.find_mut(&5).is_none(), "threshold {THRESHOLD}");
    assert!(map.find_mut(&0).is_none(), "threshold {THRESHOLD}");
    assert!(map.find_mut(&3).is_none(), "threshold {THRESHOLD}");
    assert_eq!(3, map.size(), "threshold {THRESHOLD}");

    assert_eq!(Some(&mut 3), map.find_mut(&2), "threshold {THRESHOLD}");
    assert_eq!(Some(&mut 2), map.find_mut(&4), "threshold {THRESHOLD}");
    assert_eq!(Some(&mut 1), map.find_mut(&6), "threshold {THRESHOLD}");
}

// gtest: TinyMap.MutableLookup
#[test]
fn mutable_lookup() {
    test_mutable_lookup::<0>();
    test_mutable_lookup::<2>();
    test_mutable_lookup::<5>();
    test_mutable_lookup::<10>();
}

fn os(overscaled_z: u8, wrap: i16, z: u8, x: u32, y: u32) -> OverscaledTileID {
    OverscaledTileID::new(overscaled_z, wrap, CanonicalTileID::new(z, x, y))
}

fn test_tile_id_key<const THRESHOLD: usize>() {
    let keys = vec![
        os(1, 0, 1, 0, 0),
        os(1, 0, 1, 1, 0),
        os(1, 0, 1, 0, 1),
        os(1, 0, 1, 1, 1),
        os(2, 0, 2, 0, 0),
        os(2, 0, 2, 1, 0),
        os(2, 0, 2, 0, 1),
        os(2, 0, 2, 1, 1),
        os(2, 0, 2, 2, 1),
        os(2, 0, 2, 3, 1),
        os(2, 0, 2, 2, 2),
        os(2, 0, 2, 3, 2),
        os(0, 0, 0, 0, 0),
    ];
    let values = vec![0usize; keys.len()];
    let map = TinyUnorderedMap::<OverscaledTileID, usize, THRESHOLD>::from_keys_values(keys.clone(), values);
    for k in &keys {
        assert!(map.find(k).is_some(), "threshold {THRESHOLD}");
        assert!(map.find(&OverscaledTileID::new(3, k.wrap, k.canonical)).is_none(), "threshold {THRESHOLD}");
        assert!(map.find(&OverscaledTileID::new(k.overscaled_z, 1, k.canonical)).is_none(), "threshold {THRESHOLD}");
    }
}

// gtest: TinyMap.TileIDKey
#[test]
fn tile_id_key() {
    test_tile_id_key::<0>();
    test_tile_id_key::<2>();
    test_tile_id_key::<5>();
    test_tile_id_key::<10>();
}

fn test_custom_comp<const THRESHOLD: usize>() {
    let map = IntMap::<THRESHOLD>::from_keys_values([1, 2, 3], [3, 2, 1]);
    // C++: TinyUnorderedMap<int, int, Threshold, std::hash<int>, std::equal_to<>>; the
    // port's map always compares keys with `Eq`.
    let map2 = IntMap::<THRESHOLD>::from_keys_values([2, 3, 1], [2, 1, 3]);
    // testSetDiff(map, map2);
    assert_eq!(map.size(), map2.size(), "threshold {THRESHOLD}");
    assert!(map2.find(&1).is_some(), "threshold {THRESHOLD}");
    assert!(map2.find(&2).is_some(), "threshold {THRESHOLD}");
    assert!(map2.find(&3).is_some(), "threshold {THRESHOLD}");
}

// gtest: TinyMap.CustomComp
#[test]
fn custom_comp() {
    test_custom_comp::<0>();
    test_custom_comp::<2>();
    test_custom_comp::<5>();
    test_custom_comp::<10>();
}

// --- Benchmarks ---

fn inc_tile_id(tid: &mut OverscaledTileID) {
    tid.canonical.y += 1;
    if tid.canonical.y == 1u32 << tid.canonical.z {
        tid.canonical.y = 0;
        tid.canonical.x += 1;
    }
    if tid.canonical.x == 1u32 << tid.canonical.z {
        tid.canonical.x = 0;
        tid.canonical.z += 1;
    }
}

/// A function that generates successive tile IDs.
fn make_gen_tile_id() -> impl FnMut() -> OverscaledTileID {
    let mut tid = os(0, 0, 0, 0, 0);
    move || {
        let v = tid;
        inc_tile_id(&mut tid);
        v
    }
}

fn make_gen_int_id() -> impl FnMut() -> usize {
    let mut n = 0;
    move || {
        n += 1;
        n
    }
}

fn make_gen_string_id() -> impl FnMut() -> String {
    let mut n = 0;
    move || {
        n += 1;
        format!("someprefix_{n}")
    }
}

/// `std::shuffle` with a small LCG (`std::default_random_engine` is
/// libstdc++'s `minstd_rand0`; the order only spreads the lookups).
fn shuffle<T>(items: &mut [T], state: &mut u64) {
    for i in (1..items.len()).rev() {
        *state = *state * 16807 % 2_147_483_647;
        let j = (*state as usize) % (i + 1);
        items.swap(i, j);
    }
}

trait LookupMap<K> {
    fn count(&self, key: &K) -> usize;
}

impl<K: Eq + Hash> LookupMap<K> for HashMap<K, usize> {
    fn count(&self, key: &K) -> usize {
        usize::from(self.contains_key(key))
    }
}

impl<K: Eq + Hash, const T: usize> LookupMap<K> for TinyUnorderedMap<K, usize, T> {
    fn count(&self, key: &K) -> usize {
        TinyUnorderedMap::count(self, key)
    }
}

#[allow(clippy::too_many_arguments)]
fn benchmark<K: Clone, M: LookupMap<K>>(
    label: &str,
    threshold: usize,
    max: usize,     // largest number of elements to consider
    lookups: usize, // number of searches for each item (~ read/write ratio)
    reports: usize, // number of items reported from `max` (should divide evenly)
    make: impl Fn(Vec<K>, Vec<usize>) -> M,
    mut generate: impl FnMut() -> K,
) {
    const TIMING_ITERATIONS: u32 = 50;
    assert_eq!((max / reports) * reports, max);
    let mut engine = 0xf00d_u64;

    // build keys
    let keys: Vec<K> = (0..max).map(|_| generate()).collect();
    // build values
    let values: Vec<usize> = (0..keys.len()).collect();

    let mut times = vec![0.0f64; reports];
    let mut hits = 0usize;

    // test each size from 1 to max
    for i in 1..=max {
        // Build and lookup keys in different random orders
        let mut build_keys = keys[..i].to_vec();
        let mut test_keys = keys[..i].to_vec();
        shuffle(&mut build_keys, &mut engine);
        shuffle(&mut test_keys, &mut engine);

        // (build a map and look up the keys many times) many times
        let start_time = Instant::now();
        for _ in 0..TIMING_ITERATIONS {
            let map = make(build_keys.clone(), values[..i].to_vec());
            for _ in 0..lookups {
                for k in &test_keys {
                    hits += map.count(k);
                }
            }
        }
        if i % (max / reports) == 0 {
            let total_elapsed: Duration = start_time.elapsed();
            let elapsed = total_elapsed.as_micros() as f64 / f64::from(TIMING_ITERATIONS);
            times[i / (max / reports) - 1] = elapsed;
        }
    }
    std::hint::black_box(hits);

    let mut ss = format!("{label:<10} threshold={threshold:03} (x{:>2}): ", max / reports);
    for time in times {
        let _ = write!(ss, "{:>6}", (time * 10.0).round());
    }
    Log::info(Event::Timing, &ss);
}

const LOOKUPS: usize = 100;

fn hash_map<K: Eq + Hash>(keys: Vec<K>, values: Vec<usize>) -> HashMap<K, usize> {
    let mut m = HashMap::with_capacity(keys.len());
    for (k, v) in keys.into_iter().zip(values) {
        m.entry(k).or_insert(v);
    }
    m
}

// gtest: TinyMap.BenchmarkRef
#[test]
#[ignore = "TEST_REQUIRES_ACCURATE_TIMING (DISABLED_ on the C++ CI): a benchmark that only logs timings"]
fn benchmark_ref() {
    Log::info(Event::General, if cfg!(debug_assertions) { "Build Type: Debug" } else { "Build Type: Release" });
    // std::unordered_map with size_t keys
    benchmark("int:stl", 0, 20, LOOKUPS, 10, hash_map, make_gen_int_id());
    // std::unordered_map with string keys
    benchmark("str:stl", 0, 10, LOOKUPS, 10, hash_map, make_gen_string_id());
    benchmark("tile:stl", 0, 20, LOOKUPS, 10, hash_map, make_gen_tile_id());
}

fn test_benchmark_tiny_map<const THRESHOLD: usize>() {
    // TinyMap with int keys
    benchmark(
        "int:tiny",
        THRESHOLD,
        20,
        LOOKUPS,
        10,
        TinyUnorderedMap::<usize, usize, THRESHOLD>::from_keys_values,
        make_gen_int_id(),
    );
    // TinyMap with string keys
    benchmark(
        "str:tiny",
        THRESHOLD,
        10,
        LOOKUPS,
        10,
        TinyUnorderedMap::<String, usize, THRESHOLD>::from_keys_values,
        make_gen_string_id(),
    );
    benchmark(
        "tile:tiny",
        THRESHOLD,
        20,
        LOOKUPS,
        10,
        TinyUnorderedMap::<OverscaledTileID, usize, THRESHOLD>::from_keys_values,
        make_gen_tile_id(),
    );
}

// gtest: TinyMap.BenchmarkTinyMap
#[test]
#[ignore = "TEST_REQUIRES_ACCURATE_TIMING (DISABLED_ on the C++ CI): a benchmark that only logs timings"]
fn benchmark_tiny_map() {
    test_benchmark_tiny_map::<4>();
    test_benchmark_tiny_map::<8>();
    test_benchmark_tiny_map::<16>();
}
