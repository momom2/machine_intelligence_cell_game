//! # ai — the AI layer for v1 of the cell-game RTS
//!
//! This crate is headless and deterministic, built on the Layer-2 lens ([`world`]) and the
//! Layer-1 spatial sim ([`layer1`]). It contains four things:
//!
//! 1. **The layer-agnostic GREEDY tactical policy** ([`greedy`]) — implemented **once** over
//!    an abstract [`greedy::PositionView`], then adapted ([`adapters`]) to *both* layers:
//!    Layer-1 sub-structures (emitting `layer1::MoveOrder`s) and Layer-2 planets (emitting
//!    `world::FleetOrder`s).
//! 2. **The pure strategic policies** ([`strategy`]) over the Layer-2 [`world::PlanetAggregate`]
//!    — colonize / defend / attack, each a legible hand-written policy with a documented blind
//!    spot, plus a couple of mixes and a passive that issues nothing.
//! 3. **The AI controller + roster** ([`controller`]) — given a `&World`, a `Faction`, and a
//!    chosen {strategic, tactical} policy pair, it produces BOTH the inter-planet
//!    `FleetOrder`s and the per-planet `MoveOrder`s for one decision tick, with a helper to
//!    apply them. Every owned planet's *internal* play defaults to the Layer-1 greedy adapter.
//! 4. **A headless AI-vs-AI harness** ([`harness`]) for validation.
//!
//! Determinism is inherited from the substrate: this crate draws no randomness of its own; all
//! policies are pure functions of the observed state, so a given seed + policy pair replays
//! bit-for-bit (see the determinism tests).

pub mod adapters;
pub mod automata;
pub mod controller;
pub mod counter;
pub mod graph;
pub mod greedy;
pub mod harness;
pub mod strategy;
pub mod vocab;

#[cfg(test)]
mod tests;

// Flat re-exports of the most-used items for hosts (GUI / levels / tests).
pub use adapters::{
    bucket_for, greedy_layer1_orders, greedy_layer2_orders, Layer1View, Layer2View,
};
pub use automata::{
    AttackParams, Automaton, ColonizeParams, DefendParams, SimpleColonizerParams,
};
pub use controller::{AiController, AiDecision, Roster};
pub use counter::{CounterController, CounterPlan, Exploit, OpponentProfile};
pub use greedy::{
    decide_greedy, GreedyAction, GreedyKind, GreedyParams, PosOwner, PositionInfo, PositionView,
    Side,
};
pub use strategy::{StrategicPolicy, TacticalPolicy};
