//! # `ai::counter` — the Counter opponent (opponent-modeling + modular best-response)
//!
//! The arc-2 opponent of `COUNTER_DESIGN.md`: it **observes** an opponent, **profiles** their
//! policy approximately in our own legible vocabulary, and **synthesizes a counter** that blends a
//! robust RPS backbone with projection-validated exploits of specific weaknesses.
//!
//! ## The three phases (COUNTER_DESIGN §3–6)
//!
//! Three submodules, all deterministic functions of the match log + world:
//!
//! * [`observe`] — the observation log. [`observe::Observer`] is the AI-vs-AI harness hook: each
//!   planning tick it records the watched seat's `(situation-bucket → choice)` samples — the
//!   discrete [`observe::SituationBucket`] (`ownership × garrison-band × neighbor-threat ×
//!   frontier`) and the abstracted move [`observe::MoveKind`] (`Colonize`/`Reinforce`/`Strike`/
//!   `Hold`, including inaction) + fraction band — into an [`observe::ObservationLog`].
//! * [`profile`] — inference. [`profile::OpponentProfile::infer`] reduces a log to the
//!   confidence-weighted profile: the strategic [`profile::StrategicMix`] (RPS placement), the
//!   per-module tendencies [`profile::Modules`] (guards-rear / over-commits / hoards / nearest-first
//!   / feeds-losing-grind), and per-bucket mean behaviour + confidence (`n_I`), with a recency-decay
//!   hook ([`profile::RecencyDecay`], default: accumulate).
//! * [`synthesize`] — the counter (COUNTER_DESIGN §5–6): [`synthesize::synthesize`] blends a robust
//!   RPS **backbone** (the countering automaton from the roster — [`profile::OpponentProfile::rps_counter`])
//!   with **projection-validated module exploits** over [`crate::vocab`], gated by `p_max` (the
//!   playstyle axis) and per-bucket DBR confidence. [`synthesize::CounterController`] is the
//!   *accumulate-then-counter* driver wired by `Roster::Counter { p_max }`: it watches the opposing
//!   seat, sharpens the profile across the match, and re-derives the counter on the decision cadence
//!   — with no mid-match policy flip (deferred per §2/§10).
//!
//! Determinism: the projection draws no RNG and the profile is a deterministic function of the log
//! (COUNTER_DESIGN §9), so a given `(seed, policies, horizon, p_max)` yields the same counter
//! bit-for-bit.

pub mod observe;
pub mod profile;
pub mod synthesize;

pub use observe::{
    Choice, FractionBand, FrontierBand, GarrisonBand, MoveKind, ObservationLog, Observer,
    OwnershipBand, Sample, SituationBucket, ThreatBand,
};
pub use profile::{
    BucketSummary, ModuleStat, Modules, OpponentProfile, RecencyDecay, StrategicAxis, StrategicMix,
};
pub use synthesize::{synthesize, CounterController, CounterPlan, Exploit};

#[cfg(test)]
mod tests;
