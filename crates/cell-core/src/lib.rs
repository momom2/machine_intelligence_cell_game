//! # cell-core — the deterministic mean-field engine
//!
//! The headless heart of the "cell game". It implements, **with zero randomness**,
//! the four mechanics needed to answer risk **R2** (triad cycle non-degeneracy)
//! and to later serve as the Architect's cheap fitness evaluator:
//!
//! * **Economy** — per-node production governed by a colonization rate `r`
//!   ([`engine::Params::r`]).
//! * **Movement** — fleets in transit with an undock/engagement delay, plus an
//!   attack-commitment loss `l` ([`engine::Params::l`]).
//! * **Combat** — mean-field **Lanchester square law** with a defender-advantage
//!   multiplier `k` ([`engine::Params::k`]); doubling engaged ships ~quadruples
//!   effective power.
//! * **A tick loop** — [`engine::GameState::run_match`] plays two policies to
//!   elimination or a horizon and returns a scalar [`engine::MatchOutcome`].
//!
//! The design principle behind all of this (`00-overview.md`) is **"decouple
//! computation from spectacle"**: this engine is the deterministic *computation*
//! layer, reused for AI fitness and (later) the rendered game. Randomness is a
//! future render-only concern and appears nowhere here — a property the test suite
//! pins down ([determinism test](crate)).
//!
//! ## Module map
//! * [`types`] — plain value types (owners, nodes, edges, fleets, commands).
//! * [`engine`] — params, state, the tick loop, and the square-law solver.
//! * [`features`] — the legible feature/HUD vocabulary (D3): derived quantities
//!   serving triple duty (player HUD + AI input features + fitness shaping).
//! * [`dsl`] — the shared `condition -> action` rule DSL (D4): one language for the
//!   three roles (AI policy, player automation, Layer-3 command surface), plus its
//!   per-tick evaluator.
//! * [`policy`] — the three competent pure strategies (Colonize / Defend / Attack),
//!   both as hardcoded policies and (in [`dsl::strategies`]) as DSL rule-sets.
//! * [`maps`] — fixed symmetric maps with equal opposing starts.
//! * [`harness`] — run-a-matchup / evaluate-the-cycle helpers (reused by tests and
//!   the r2-sweep binary).

pub mod dsl;
pub mod engine;
pub mod features;
pub mod harness;
pub mod maps;
pub mod policy;
pub mod types;

// Convenient flat re-exports for the most-used items.
pub use dsl::{Action, Condition, DslPolicy, Feature, NodeSelector, Policy as RulePolicy, Rule};
pub use engine::{lanchester_resolve, GameState, MatchOutcome, Params};
pub use features::{NodeFeatures, OwnerFeatures};
pub use harness::{cycle_on_map, cycle_over_maps, duel, Archetype, CyclePoint, DuelResult};
pub use policy::{Attack, Colonize, Defend, Policy};
pub use types::{Command, Edge, EdgeId, FractionBucket, Node, NodeId, Owner};
