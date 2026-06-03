//! # architect — the autoconstructive evolutionary loop (build-order **step 5**, the headline)
//!
//! The Architect / Gardener of `02-ai-opponents.md`: between matches it runs an
//! **autoconstructive evolutionary loop** over a *population* of rule-set "organisms" in
//! the shared `cell_core` DSL, where **each organism carries its own
//! mutation/recombination operators** (Lee Spector's *Pushpop* twist — the reproduction
//! machinery evolves, not just the strategy). It is built entirely on the deterministic
//! mean-field engine + DSL from `cell-core`, plus one seeded PRNG, so a run is
//! bit-reproducible from its seed.
//!
//! ## The four fitness pressures (`02`, `04`)
//!
//! 1. **Mean-field coevolution** — cheap deterministic matches vs a *sample* of the
//!    population, so fitness is **relative** (the engine of the arms race).
//! 2. **Cached player model** — performance vs a fixed cached opponent (blended with, not
//!    replacing, self-play — the R4 overfitting mitigation).
//! 3. **Parsimony penalty** — a per-rule cost that keeps the **glass genome** small and
//!    readable (R3 legibility; an accepted strength ceiling).
//! 4. **Wins-as-test-cases regression archive** — Schmidhuber's *success-story* gate: a
//!    new champion is installed **only if it does not regress** any archived past-beaten
//!    configuration, so every exploit the population finds stays closed.
//!
//! ## The two "feel" mechanics, headless
//!
//! * **Glass genome** ([`glass`]) — the champion is a short legible rule-set, and its
//!   change each generation is a readable `+`/`-`/`~` **diff**. Autoconstruction is
//!   spectatable, not a black box.
//! * **R1 instrumentation** ([`diversity`]) — the loop measures, every generation,
//!   whether the population collapses to a **single dominant program** (the arms race
//!   dies) or sustains **strategic diversity / a non-transitive arms race**. This is the
//!   real test of `04`'s scariest risk, R1.
//!
//! ## Module map
//! * [`rng`] — a tiny seeded SplitMix64 (the only stochastic element; evaluation stays
//!   deterministic).
//! * [`genome`] — the legible gene catalog, a [`genome::Gene`], an organism's own
//!   [`genome::Operators`], and the [`genome::Genome`] (rule-set + operators).
//! * [`variation`] — recombination/mutation **driven by each organism's own operators**
//!   (the autoconstructive step; operators are inherited + meta-mutated).
//! * [`fitness`] — the blended objective + the [`fitness::Archive`] success-story gate.
//! * [`diversity`] — the R1 instrument (genotypic diversity + strategic non-transitivity).
//! * [`glass`] — the glass-genome renderer + champion diffs.
//! * [`population`] — the loop that assembles all of the above across generations.
//! * [`report`] — `ARCHITECT_RESULTS.md` (fitness/size/diffs + the R1 read with evidence).

pub mod diversity;
pub mod fitness;
pub mod genome;
pub mod glass;
pub mod population;
pub mod report;
pub mod rng;
pub mod variation;

pub use diversity::{genotypic, non_transitivity, Genotypic, NonTransitivity};
pub use fitness::{Archive, FitnessBreakdown, PlayerModel};
pub use genome::{gene_catalog, seed_archetypes, Gene, GeneTemplate, Genome, Operators, Role};
pub use glass::{diff_genomes, render_diff, render_genome, DiffLine};
pub use population::{run, Config, GenerationLog, RunResult};
pub use report::{classify_r1, to_markdown, R1Read};
pub use rng::Rng;
