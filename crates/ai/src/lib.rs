//! # ai — the AI layer for the cell-game RTS
//!
//! This crate is headless and deterministic, built on the Layer-1 spatial sim ([`layer1`]).
//! The whole game is one [`layer1::Interior`] (sub-structures + ships), and every AI seat
//! decides and applies that interior's `layer1::MoveOrder`s for one decision tick. It
//! contains five things:
//!
//! 1. **The GREEDY tactical policy** ([`greedy`]) — the project owner's expand/defend/assault
//!    rule, implemented **once** over an abstract [`greedy::PositionView`] and then adapted
//!    ([`adapters`]) to the interior: positions are its **sub-structures**, emitting
//!    `layer1::MoveOrder`s. This is also the player's optional "basic automation".
//! 2. **The AI controller + roster** ([`controller`]) — the menu the GUI / levels pick from,
//!    and the [`controller::SeatController`] dispatch that fields the right brain for a roster
//!    entry (the game and the headless validation share it). The stateless default runs the
//!    greedy adapter.
//! 3. **The live campaign "Simple" enemy** ([`simple`]) — a stateful, ledger-driven colonizer
//!    (`SimpleController`) fielded for `Roster::SimpleColonize` / `Roster::SimpleAdjacent`; it
//!    sizes each capture wave by resistance and staggers departures so they land together.
//! 4. **The scripted "Command and Control" enemy** ([`cycler`], `Roster::Cycler`) — a
//!    stateful, telegraphed drillmaster (`CyclerController`).
//! 5. **A headless AI-vs-AI harness** ([`harness`]) for validation.
//!
//! Determinism is inherited from the substrate: this crate draws no randomness of its own; all
//! policies are pure functions of the observed state, so a given seed + policy pair replays
//! bit-for-bit (see the determinism tests).

pub mod adapters;
pub mod controller;
pub mod cycler;
pub mod greedy;
pub mod harness;
pub mod simple;

#[cfg(test)]
mod tests;

// Flat re-exports of the most-used items for hosts (GUI / levels / tests).
pub use adapters::{bucket_for, greedy_layer1_orders, Layer1View};
pub use controller::{AiController, Roster, SeatController};
pub use greedy::{
    decide_greedy, GreedyAction, GreedyKind, GreedyParams, PosOwner, PositionInfo, PositionView,
    Side,
};
pub use cycler::CyclerController;
pub use simple::{SimpleController, SimpleParams};
