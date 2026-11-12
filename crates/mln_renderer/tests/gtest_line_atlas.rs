// Ports test/geometry/line_atlas.test.cpp.
//
// The C++ test draws its dash arrays from `std::mt19937`,
// `std::uniform_int_distribution<size_t>` and
// `std::normal_distribution<float>`. The engine is fully specified by the
// standard; the distributions are libstdc++'s (x86-64 Linux, GCC 11 and
// later), ported below so the atlas sees the same dash arrays as in C++.
// The mt19937 and uniform_int code is the one of
// tools/render-test/src/std_random.rs (checked there against the C++
// library by //difftest/render_test).

use mln_renderer::line_atlas::{LineAtlas, LinePatternCap};

/// `std::mt19937`.
struct Mt19937 {
    state: [u32; Self::N],
    index: usize,
}

impl Mt19937 {
    const N: usize = 624;
    const M: usize = 397;
    const MATRIX_A: u32 = 0x9908_b0df;
    const UPPER_MASK: u32 = 0x8000_0000;
    const LOWER_MASK: u32 = 0x7fff_ffff;

    /// `mersenne_twister_engine(result_type value)`.
    fn new(value: u32) -> Self {
        let mut state = [0u32; Self::N];
        state[0] = value;
        for i in 1..Self::N {
            state[i] = 1_812_433_253_u32.wrapping_mul(state[i - 1] ^ (state[i - 1] >> 30)).wrapping_add(i as u32);
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
    fn next_u32(&mut self) -> u32 {
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

/// libstdc++ `uniform_int_distribution<size_t>{a, b}(g)` with a 32-bit engine
/// and `b - a < 2^32 - 1` (Lemire's reduction in 64 bits).
fn uniform_int(g: &mut Mt19937, a: u64, b: u64) -> u64 {
    let urange = b - a;
    assert!(urange < u64::from(u32::MAX));
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

/// libstdc++ `generate_canonical<float, 24>(g)` for a 32-bit engine: one
/// draw, `float(g() - min) / 2^32`, clamped below 1.
fn generate_canonical_f32(g: &mut Mt19937) -> f32 {
    let sum = g.next_u32() as f32;
    let ret = sum / 4_294_967_296.0_f32;
    // `std::nextafter(1.0f, 0.0f)`.
    if ret >= 1.0 { f32::from_bits(1.0f32.to_bits() - 1) } else { ret }
}

/// libstdc++ `normal_distribution<float>` (Marsaglia's polar method; keeps
/// the second value of each pair for the next call).
struct NormalDistribution {
    mean: f32,
    stddev: f32,
    saved: f32,
    saved_available: bool,
}

impl NormalDistribution {
    fn new(mean: f32, stddev: f32) -> Self {
        NormalDistribution { mean, stddev, saved: 0.0, saved_available: false }
    }

    fn sample(&mut self, g: &mut Mt19937) -> f32 {
        let ret = if self.saved_available {
            self.saved_available = false;
            self.saved
        } else {
            let (mut x, mut y, mut r2);
            loop {
                x = 2.0f32 * generate_canonical_f32(g) - 1.0;
                y = 2.0f32 * generate_canonical_f32(g) - 1.0;
                r2 = x * x + y * y;
                if !(r2 > 1.0 || r2 == 0.0) {
                    break;
                }
            }
            let mult = (-2.0 * r2.ln() / r2).sqrt();
            self.saved = x * mult;
            self.saved_available = true;
            y * mult
        };
        ret * self.stddev + self.mean
    }
}

// gtest: LineAtlas.Random
#[test]
fn random() {
    let mut generator = Mt19937::new(42); // Use the same seed for reproducible tests.
    let count_distribution = |g: &mut Mt19937| uniform_int(g, 0, 12) as usize;
    let cap_style_distribution = |g: &mut Mt19937| uniform_int(g, 0, 1);
    let mut length_distribution = NormalDistribution::new(3.0, 5.0);

    for _it in 0..100 {
        let mut atlas = LineAtlas::new();
        let mut dasharray: Vec<f32> = Vec::with_capacity(8);
        for _j in 0..100 {
            dasharray.clear();
            let count = count_distribution(&mut generator);
            for _i in 0..count {
                dasharray.push(length_distribution.sample(&mut generator));
            }
            let pattern_cap =
                if cap_style_distribution(&mut generator) > 0 { LinePatternCap::Round } else { LinePatternCap::Square };

            atlas.get_dash_pattern_texture(&dasharray, &dasharray, pattern_cap);
        }
    }
}
