//! The shipped **lineages** of the Evolved brain: frozen champions bred by
//! `cargo run -p ai --release --example evolve`. A mission pins a lineage by number
//! (`enemy = evolved 2`) and it never changes — re-running the tool writes new numbers.
//!
//! GENERATED — see `crates/ai/examples/evolve.rs`; the table is empty until the first run.

use crate::evolved::{Genome, GENES};

/// `(generation reached, genome)` per lineage, oldest first.
pub const LINEAGES: &[(u32, [f32; GENES])] = &[];

/// The genome of lineage `n`, if it exists.
pub fn genome(n: usize) -> Option<Genome> {
    LINEAGES.get(n).map(|&(_, g)| Genome(g))
}

/// The generation count lineage `n` was bred for, if it exists.
pub fn generation(n: usize) -> Option<u32> {
    LINEAGES.get(n).map(|&(g, _)| g)
}
