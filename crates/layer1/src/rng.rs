//! A tiny, **seeded**, deterministic PRNG (xorshift64*).
//!
//! ## Why a PRNG lives here (unlike `cell-core`)
//!
//! `cell-core` is the deterministic *mean-field* layer and is emphatically RNG-free:
//! randomness there would contaminate the AI fitness gate (`00`/`01`). **Layer 1 is the
//! opposite end of the "decouple computation from spectacle" axis** — it is the
//! *spectacle* combat model (`01`: "This stochastic per-ship combat is the LAYER-1
//! (spectacle) combat model"). Each ship is a *stochastic emitter* that one-shots an
//! enemy when it fires, so Layer 1 inherently needs randomness.
//!
//! To keep that randomness from breaking reproducibility, we confine it to this one
//! seeded generator. A whole simulation run is then **bit-reproducible from a seed**: the
//! determinism test pins `same seed => identical final state`. We implement the generator
//! inline (no `rand` crate) so the library stays dependency-free, exactly like the rest of
//! the workspace.
//!
//! xorshift64* is chosen because it is ~6 lines, has no dependency, and is a well-known,
//! statistically decent generator for game-flavour randomness (one-shot kill rolls, ship
//! spread) — not cryptography.

/// A xorshift64* generator. Cheap to clone (it is just a `u64` of state), so callers can
/// snapshot/fork a deterministic stream.
#[derive(Debug, Clone)]
pub struct Rng {
    state: u64,
}

impl Rng {
    /// Seed the generator. Seed `0` is remapped (xorshift cannot leave the all-zero
    /// state), so every seed — including 0 — yields a valid, non-degenerate stream.
    #[inline]
    pub fn new(seed: u64) -> Rng {
        // Avalanche the seed once (SplitMix64 finalizer) so nearby seeds (0,1,2…) start in
        // well-separated parts of the state space, and 0 maps to something non-zero.
        let mut z = seed.wrapping_add(0x9E37_79B9_7F4A_7C15);
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^= z >> 31;
        Rng { state: if z == 0 { 0x9E37_79B9_7F4A_7C15 } else { z } }
    }

    /// Next raw `u64` (the xorshift64* step).
    #[inline]
    pub fn next_u64(&mut self) -> u64 {
        let mut x = self.state;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.state = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    /// Uniform `f32` in `[0, 1)` (top 24 bits → a float, the standard construction).
    #[inline]
    pub fn next_f32(&mut self) -> f32 {
        (self.next_u64() >> 40) as f32 / (1u32 << 24) as f32
    }

    /// Uniform `f64` in `[0, 1)` (top 53 bits → a double, the standard construction).
    #[inline]
    pub fn next_f64(&mut self) -> f64 {
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

    /// Uniform `f32` in `[lo, hi]` (handles `lo >= hi` by returning `lo`).
    #[inline]
    pub fn range_f32(&mut self, lo: f32, hi: f32) -> f32 {
        if hi <= lo {
            return lo;
        }
        lo + (hi - lo) * self.next_f32()
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
        assert_ne!(a.next_u64(), b.next_u64());
    }

    #[test]
    fn seed_zero_is_nondegenerate() {
        let mut r = Rng::new(0);
        // The all-zero state would freeze a raw xorshift; ours must keep producing.
        let a = r.next_u64();
        let b = r.next_u64();
        assert_ne!(a, 0);
        assert_ne!(a, b);
    }

    #[test]
    fn floats_in_unit_interval() {
        let mut r = Rng::new(99);
        for _ in 0..10_000 {
            let x = r.next_f64();
            assert!((0.0..1.0).contains(&x), "f64 out of range: {x}");
            let y = r.next_f32();
            assert!((0.0..1.0).contains(&y), "f32 out of range: {y}");
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

    #[test]
    fn chance_is_well_calibrated() {
        // Empirical frequency of a p=0.3 event should land near 0.3 over many trials.
        let mut r = Rng::new(424242);
        let trials = 200_000;
        let hits = (0..trials).filter(|_| r.chance(0.3)).count();
        let freq = hits as f64 / trials as f64;
        assert!((freq - 0.3).abs() < 0.01, "frequency {freq} far from 0.3");
    }
}
