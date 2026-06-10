//! A tiny, **seeded**, deterministic PRNG (SplitMix64).
//!
//! ## Why a PRNG lives here but nowhere in `cell-core`
//!
//! `cell-core` is the *computation* layer and is emphatically RNG-free (`01`,
//! `00`-overview "decouple computation from spectacle"): a policy is a pure function of
//! [`cell_core::GameState`], which is the precondition for the empirical fitness gate.
//! Evolution, however, is a **search**, and a search needs variation. We confine that
//! variation to this one seeded generator so the whole Architect run is still
//! **bit-reproducible from a seed** — the same property the rest of the build relies on,
//! just parameterized by a seed instead of being parameter-free. Fitness *evaluation*
//! (the mean-field matches) remains perfectly deterministic; only the *proposal* of new
//! genomes is stochastic.
//!
//! SplitMix64 is chosen because it is ~10 lines, has no external dependency (the crate is
//! deliberately dependency-free, like `cell-core`/`automaton`), and is a well-known,
//! statistically decent generator for this use (variation operators, not cryptography).

/// A SplitMix64 generator. Cheap to clone (it is just a `u64` state), so each organism
/// can carry/advance its own stream deterministically.
#[derive(Debug, Clone)]
pub struct Rng {
    state: u64,
}

impl Rng {
    /// Seed the generator. Any seed is valid (including 0).
    pub fn new(seed: u64) -> Rng {
        Rng { state: seed }
    }

    /// Next raw `u64` (the SplitMix64 step).
    #[inline]
    pub fn next_u64(&mut self) -> u64 {
        // SplitMix64: advance by the golden-ratio odd constant, then avalanche.
        self.state = self.state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Uniform `f64` in `[0, 1)`.
    #[inline]
    pub fn next_f64(&mut self) -> f64 {
        // Top 53 bits → a double in [0,1), the standard construction.
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }

    /// `true` with probability `p` (clamped to `[0,1]`).
    #[inline]
    pub fn chance(&mut self, p: f64) -> bool {
        self.next_f64() < p.clamp(0.0, 1.0)
    }

    /// Uniform integer in `[0, n)`. Returns 0 if `n == 0`.
    #[inline]
    pub fn below(&mut self, n: usize) -> usize {
        if n == 0 {
            return 0;
        }
        (self.next_u64() % n as u64) as usize
    }

    /// Uniform `f64` in `[lo, hi]` (handles `lo == hi`).
    #[inline]
    pub fn range(&mut self, lo: f64, hi: f64) -> f64 {
        if hi <= lo {
            return lo;
        }
        lo + (hi - lo) * self.next_f64()
    }

    /// A fresh independent-ish substream seed derived from this generator. Used to give
    /// each organism its own deterministic stream from the parent's.
    #[inline]
    pub fn fork_seed(&mut self) -> u64 {
        self.next_u64()
    }

    /// Pick an index in `[0, len)` — convenience for choosing a slice element.
    #[inline]
    pub fn choose(&mut self, len: usize) -> usize {
        self.below(len)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deterministic_from_seed() {
        let mut a = Rng::new(12345);
        let mut b = Rng::new(12345);
        for _ in 0..1000 {
            assert_eq!(a.next_u64(), b.next_u64());
        }
    }

    #[test]
    fn different_seeds_diverge() {
        let mut a = Rng::new(1);
        let mut b = Rng::new(2);
        // Astronomically unlikely to match on the first draw; pins that the seed matters.
        assert_ne!(a.next_u64(), b.next_u64());
    }

    #[test]
    fn f64_in_unit_interval() {
        let mut r = Rng::new(99);
        for _ in 0..10_000 {
            let x = r.next_f64();
            assert!((0.0..1.0).contains(&x), "f64 out of range: {x}");
        }
    }

    #[test]
    fn below_respects_bound() {
        let mut r = Rng::new(7);
        for _ in 0..10_000 {
            assert!(r.below(5) < 5);
        }
        assert_eq!(r.below(0), 0);
    }
}
