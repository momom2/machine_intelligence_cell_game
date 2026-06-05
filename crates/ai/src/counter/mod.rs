//! # `ai::counter` — the Counter opponent (opponent-modeling + modular best-response)
//!
//! The arc-2 opponent of `COUNTER_DESIGN.md`: it **observes** an opponent, **profiles** their
//! policy approximately in our own legible vocabulary, and (in a later phase) **synthesizes a
//! counter** that blends a robust RPS backbone with projection-validated exploits of specific
//! weaknesses.
//!
//! ## This phase — OBSERVE + INFER (COUNTER_DESIGN §3–4)
//!
//! Two submodules, both deterministic pure functions of the match log:
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
//!
//! The counter **synthesis** and the `Roster::Counter` wiring are *not* built here (COUNTER_DESIGN
//! §5/§9 — a later phase); [`profile::StrategicAxis::rps_counter`] surfaces only the backbone
//! *suggestion* the synthesis will start from.
//!
//! Determinism: the projection draws no RNG and the profile is a deterministic function of the log
//! (COUNTER_DESIGN §9), so a given `(seed, policies, horizon)` yields the same profile bit-for-bit.

pub mod observe;
pub mod profile;

pub use observe::{
    Choice, FractionBand, FrontierBand, GarrisonBand, MoveKind, ObservationLog, Observer,
    OwnershipBand, Sample, SituationBucket, ThreatBand,
};
pub use profile::{
    BucketSummary, ModuleStat, Modules, OpponentProfile, RecencyDecay, StrategicAxis, StrategicMix,
};

#[cfg(test)]
mod tests;
