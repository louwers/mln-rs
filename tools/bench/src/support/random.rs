//! Data that an application would supply, generated from a fixed seed so
//! that every run and every machine gets the same.

/// `SplitMix64`: a small generator whose sequence is fixed by its seed.
pub(crate) struct Random(u64);

impl Random {
    pub(crate) fn new(seed: u64) -> Random {
        Random(seed)
    }

    pub(crate) fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    /// A number in `[low, high)`.
    pub(crate) fn range(&mut self, low: f64, high: f64) -> f64 {
        // The top 53 bits, as a fraction of 1.
        let fraction = (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64;
        low + fraction * (high - low)
    }

    /// An index below `len`.
    pub(crate) fn below(&mut self, len: usize) -> usize {
        (self.next_u64() % len as u64) as usize
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_seed_fixes_the_sequence() {
        let a: Vec<u64> = std::iter::repeat_with({
            let mut random = Random::new(7);
            move || random.next_u64()
        })
        .take(4)
        .collect();
        let mut random = Random::new(7);
        assert!(a.iter().all(|&value| value == random.next_u64()));
        let value = Random::new(1).range(2.0, 3.0);
        assert!((2.0..3.0).contains(&value));
    }
}
