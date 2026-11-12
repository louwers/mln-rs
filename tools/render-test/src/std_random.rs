// Ports the parts of the C++ standard library that `Manifest::doShuffle`
// (render-test/manifest_parser.cpp) uses, as libstdc++ implements them on
// x86-64 Linux:
//
//   std::seed_seq sequence{seed};
//   std::mt19937 shuffler(sequence);
//   std::shuffle(testPaths.begin(), testPaths.end(), shuffler);
//
// `std::mt19937` and `std::seed_seq::generate` are fully specified by the
// standard ([rand.eng.mers], [rand.util.seedseq]). `std::shuffle` and
// `std::uniform_int_distribution` are not: this is libstdc++'s
// (bits/stl_algo.h, bits/uniform_int_dist.h, GCC 11 and later, which draw
// two swap positions per random number and use Lemire's nearly divisionless
// reduction). `mt19937::result_type` is `uint_fast32_t`, 64 bits wide there,
// which selects the 32-bit-range reduction path. libc++ (Apple) shuffles
// differently. //difftest/render_test checks this against the C++ library.

/// `std::seed_seq` over 32-bit seeds.
pub struct SeedSeq {
    v: Vec<u32>,
}

impl SeedSeq {
    #[must_use]
    pub fn new(seeds: &[u32]) -> Self {
        SeedSeq { v: seeds.to_vec() }
    }

    /// `seed_seq::generate(begin, end)`.
    pub fn generate(&self, out: &mut [u32]) {
        let n = out.len();
        if n == 0 {
            return;
        }
        out.fill(0x8b8b_8b8b);
        let s = self.v.len();
        let t = if n >= 623 {
            11
        } else if n >= 68 {
            7
        } else if n >= 39 {
            5
        } else if n >= 7 {
            3
        } else {
            (n - 1) / 2
        };
        let p = (n - t) / 2;
        let q = p + t;
        let m = std::cmp::max(s + 1, n);
        let tt = |x: u32| x ^ (x >> 27);

        for k in 0..m {
            let r1 = 1_664_525_u32.wrapping_mul(tt(out[k % n] ^ out[(k + p) % n] ^ out[(k + n - 1) % n]));
            let r2 = if k == 0 {
                r1.wrapping_add(s as u32)
            } else if k <= s {
                r1.wrapping_add((k % n) as u32).wrapping_add(self.v[k - 1])
            } else {
                r1.wrapping_add((k % n) as u32)
            };
            out[(k + p) % n] = out[(k + p) % n].wrapping_add(r1);
            out[(k + q) % n] = out[(k + q) % n].wrapping_add(r2);
            out[k % n] = r2;
        }
        for k in m..m + n {
            let r3 = 1_566_083_941_u32
                .wrapping_mul(tt(out[k % n].wrapping_add(out[(k + p) % n]).wrapping_add(out[(k + n - 1) % n])));
            let r4 = r3.wrapping_sub((k % n) as u32);
            out[(k + p) % n] ^= r3;
            out[(k + q) % n] ^= r4;
            out[k % n] = r4;
        }
    }
}

/// `std::mt19937`.
pub struct Mt19937 {
    state: [u32; Self::N],
    index: usize,
}

impl Mt19937 {
    const N: usize = 624;
    const M: usize = 397;
    const MATRIX_A: u32 = 0x9908_b0df;
    const UPPER_MASK: u32 = 0x8000_0000;
    const LOWER_MASK: u32 = 0x7fff_ffff;

    pub const MIN: u32 = 0;
    pub const MAX: u32 = u32::MAX;

    /// `mersenne_twister_engine(result_type value)`.
    #[must_use]
    pub fn new(value: u32) -> Self {
        let mut state = [0u32; Self::N];
        state[0] = value;
        for i in 1..Self::N {
            state[i] = 1_812_433_253_u32.wrapping_mul(state[i - 1] ^ (state[i - 1] >> 30)).wrapping_add(i as u32);
        }
        Mt19937 { state, index: Self::N }
    }

    /// `mersenne_twister_engine(Sseq& q)`.
    #[must_use]
    pub fn from_seed_seq(q: &SeedSeq) -> Self {
        let mut state = [0u32; Self::N];
        q.generate(&mut state);
        // If the upper w-r bits of x[0] and all of x[1..] are zero, use 2^(w-1).
        let zero = state[0] & Self::UPPER_MASK == 0 && state[1..].iter().all(|&x| x == 0);
        if zero {
            state[0] = Self::UPPER_MASK;
        }
        Mt19937 { state, index: Self::N }
    }

    fn twist(&mut self) {
        for i in 0..Self::N {
            let y = (self.state[i] & Self::UPPER_MASK) | (self.state[(i + 1) % Self::N] & Self::LOWER_MASK);
            let mut next = self.state[(i + Self::M) % Self::N] ^ (y >> 1);
            if y & 1 != 0 {
                next ^= Self::MATRIX_A;
            }
            self.state[i] = next;
        }
        self.index = 0;
    }

    /// `operator()()`.
    pub fn next_u32(&mut self) -> u32 {
        if self.index >= Self::N {
            self.twist();
        }
        let mut z = self.state[self.index];
        self.index += 1;
        z ^= z >> 11;
        z ^= (z << 7) & 0x9d2c_5680;
        z ^= (z << 15) & 0xefc6_0000;
        z ^= z >> 18;
        z
    }
}

/// libstdc++ `uniform_int_distribution<unsigned long>{a, b}(g)` with a
/// 32-bit engine and `b - a < 2^32 - 1` (the "downscaling" case, the only
/// one `shuffle` reaches for fewer than 2^32 - 1 elements).
fn uniform_int(g: &mut Mt19937, a: u64, b: u64) -> u64 {
    let urngrange = u64::from(Mt19937::MAX - Mt19937::MIN);
    let urange = b - a;
    // TODO(port): the "upscaling" case (b - a >= 2^32 - 1).
    assert!(urngrange > urange, "uniform_int_distribution upscaling is not ported");
    // `__urngrange == __UINT32_MAX__`: Lemire's reduction in 64 bits.
    let uerange = (urange + 1) as u32;
    let mut product = u64::from(g.next_u32()) * u64::from(uerange);
    let mut low = product as u32;
    if low < uerange {
        let threshold = uerange.wrapping_neg() % uerange;
        while low < threshold {
            product = u64::from(g.next_u32()) * u64::from(uerange);
            low = product as u32;
        }
    }
    (product >> 32) + a
}

/// libstdc++ `std::shuffle(first, last, g)`.
pub fn shuffle<T>(v: &mut [T], g: &mut Mt19937) {
    if v.is_empty() {
        return;
    }
    let urngrange = u64::from(Mt19937::MAX - Mt19937::MIN);
    let urange = v.len() as u64;

    if urngrange / urange >= urange {
        // I.e. (urngrange >= urange * urange) but without wrap issues.
        let mut i = 1usize;

        // Since we know the range isn't empty, an even number of elements
        // means an uneven number of elements /to swap/, in which case we do
        // the first one up front:
        if urange.is_multiple_of(2) {
            let j = uniform_int(g, 0, 1) as usize;
            v.swap(i, j);
            i += 1;
        }

        // Now we know that last - i is even, so we do the rest in pairs,
        // using a single distribution invocation to produce swap positions
        // for two successive elements at a time:
        while i != v.len() {
            let swap_range = i as u64 + 1;
            // __gen_two_uniform_ints(swap_range, swap_range + 1, g)
            let b1 = swap_range + 1;
            let x = uniform_int(g, 0, swap_range * b1 - 1);
            let (first, second) = ((x / b1) as usize, (x % b1) as usize);
            v.swap(i, first);
            i += 1;
            v.swap(i, second);
            i += 1;
        }
        return;
    }

    for i in 1..v.len() {
        let j = uniform_int(g, 0, i as u64) as usize;
        v.swap(i, j);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mt19937_default_seed_10000th_output() {
        // [rand.predef]: the 10000th consecutive invocation of a
        // default-constructed mt19937 (seed 5489) produces 4123659995.
        let mut g = Mt19937::new(5489);
        let mut x = 0;
        for _ in 0..10000 {
            x = g.next_u32();
        }
        assert_eq!(x, 4_123_659_995);
    }

    #[test]
    fn shuffle_is_a_permutation() {
        for n in [0usize, 1, 2, 3, 10, 1476] {
            let mut v: Vec<usize> = (0..n).collect();
            shuffle(&mut v, &mut Mt19937::from_seed_seq(&SeedSeq::new(&[1])));
            let mut sorted = v.clone();
            sorted.sort_unstable();
            assert_eq!(sorted, (0..n).collect::<Vec<_>>());
        }
    }
}
