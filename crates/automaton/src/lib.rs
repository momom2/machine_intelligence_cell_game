//! # automaton — the arc-1 capstone (build-order step 4)
//!
//! This crate is the **headless Layer-2 logic** of the design: the *Automaton*
//! opponent ladder and the player routine that beats it. It builds entirely on the
//! deterministic mean-field engine + the shared rule DSL from `cell-core`, so every
//! result here is bit-reproducible (no RNG, no I/O on the hot path).
//!
//! ## What the arc-1 capstone is (`02-ai-opponents.md`)
//!
//! The climax of the first arc is *solving the triad dynamic* against **Automatons
//! that implement hidden-but-fixed mixes of colonize / defend / attack**. The
//! decision the design fixed is **hidden + fixed**, which makes winning a four-step
//! routine:
//!
//! 1. **scout** — observe the opponent's *early* behaviour,
//! 2. **infer** — read its hidden mix from the legible features (online policy
//!    inference — exactly what the Apprentice later does to the player),
//! 3. **select** — choose the counter from the rock-paper-scissors cycle, and
//! 4. **execute** — play the now-*static* optimization against it.
//!
//! ## The difficulty metric (`02`): centrality in the simplex
//!
//! Each Automaton is a point in the colonize/defend/attack **simplex** (weights that
//! sum to 1). *Pure* corners have clean counters; **balanced mixes near the centre are
//! hardest** to read and to counter. So the ladder is ordered by **increasing
//! centrality** (= decreasing distance from the nearest pure corner), and the headline
//! empirical question is whether that centrality actually predicts difficulty.
//!
//! ## Module map
//! * [`mix`] — the [`Mix`] simplex weight + its geometry (centrality, nearest corner).
//! * [`automaton`] — synthesize a single fixed, legible DSL [`cell_core::RulePolicy`]
//!   from a [`Mix`]; pure corners reproduce the `cell_core::dsl::strategies` archetypes.
//! * [`ladder`] — the ordered ladder of Automatons (corners → centre).
//! * [`infer`] — online policy inference: scout an opponent and estimate its mix from
//!   the legible features.
//! * [`capstone`] — the player routines: the inferring `Capstone` player and the
//!   fixed non-inferring `Baseline`, plus the head-to-head evaluation used to validate.

pub mod automaton;
pub mod capstone;
pub mod infer;
pub mod ladder;
pub mod mix;
pub mod report;

pub use automaton::{compile, AutomatonSpec};
pub use capstone::{
    counter_mix, evaluate_ladder, pearson, simplex_grid, simplex_grid_study, spearman, Baseline,
    Capstone, LadderEvaluation, Oracle, RungReport, SimplexStudy,
};
pub use infer::{infer_mix, scout_opponent, InferConfig, Scout};
pub use ladder::{build_ladder, default_ladder, Rung};
pub use mix::{Corner, Mix};
pub use report::ReportContext;
