//! A tiny deterministic generator (SplitMix64) so the catalog never depends on a RNG crate's
//! output stability.

/// SplitMix64 state.
pub(crate) struct Rng(u64);

impl Rng {
    pub(crate) fn new(seed: u64) -> Self {
        Self(seed)
    }

    fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Uniform in `0..n` (`n` must be non-zero; the modulo bias is irrelevant at this size).
    pub(crate) fn below(&mut self, n: usize) -> usize {
        usize::try_from(self.next_u64() % n.max(1) as u64).unwrap_or(0)
    }

    /// Uniform in `lo..=hi`.
    pub(crate) fn range(&mut self, lo: u32, hi: u32) -> u32 {
        lo + u32::try_from(self.below((hi - lo + 1) as usize)).unwrap_or(0)
    }

    /// A random element of a non-empty slice.
    pub(crate) fn pick<'a, T>(&mut self, items: &'a [T]) -> &'a T {
        &items[self.below(items.len())]
    }

    /// True with the given probability in percent.
    pub(crate) fn percent(&mut self, p: u32) -> bool {
        self.below(100) < p as usize
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_seed_same_sequence() {
        let mut a = Rng::new(7);
        let mut b = Rng::new(7);
        let xs: Vec<u32> = (0..8).map(|_| a.range(0, 1000)).collect();
        let ys: Vec<u32> = (0..8).map(|_| b.range(0, 1000)).collect();
        assert_eq!(xs, ys);
    }

    #[test]
    fn range_is_inclusive_and_bounded() {
        let mut rng = Rng::new(1);
        for _ in 0..1000 {
            let v = rng.range(3, 5);
            assert!((3..=5).contains(&v));
        }
    }
}
