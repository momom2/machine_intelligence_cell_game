//! Headless **validation** of the campaign — the real test the GUI's content rests on.
//!
//! One check per level, and it is the only gate: **determinism** — building the same level with
//! the same seed twice yields the **same [`world::World::state_hash`]**, and a short scripted
//! match replays bit-for-bit (the substrate's guarantee, re-confirmed at the level layer). A
//! `build` that panics fails here too.
//!
//! **Balance is deliberately NOT tested** (owner rule, 2026-07-06: all balancing is per-level,
//! by hand). The old winnability proxies (a concentration proxy for the Layer-1 tutorials, the
//! greedy baseline for Layer-2 maps) and the automata-track lesson checks (the RPS counters,
//! the seam flank) measured exactly the thing the owner tunes by playing, so they were removed
//! with the rest of the lesson machinery — same fate as the structural-spec gate before them
//! (a hand-maintained mirror of each `build`, removed as non-load-bearing: it only ever broke
//! on legitimate authoring changes).

use ai::harness::GAME_DECISION_BASE;
use ai::SeatController;
use layer1::{Faction, SimParams};
use layer1::Interior;

use crate::Level;

/// The outcome of validating one level.
#[derive(Debug, Clone)]
pub struct LevelReport {
    pub id: u32,
    pub title: String,
    /// Building twice (and replaying a scripted match) was deterministic.
    pub deterministic: bool,
}

impl LevelReport {
    /// The hard invariant a level must always satisfy: it builds and replays
    /// **deterministically**. Nothing else about a level is gated — balance/difficulty is the
    /// owner's hand-tuned, playtested call, per level.
    pub fn ok(&self) -> bool {
        self.deterministic
    }
}

/// Validate a single level's gate — determinism. The gated lib test
/// (`campaign_is_well_formed`) runs this for every level.
pub fn validate_level_gates(level: &Level) -> LevelReport {
    LevelReport {
        id: level.id,
        title: level.title.clone(),
        deterministic: check_determinism(level),
    }
}

// ======================================================================================
// Determinism.
// ======================================================================================

/// Build the level twice with the same seed and compare `state_hash`; then run a short scripted
/// match twice and compare per-tick hashes + outcome. Returns `true` if everything matched.
fn check_determinism(level: &Level) -> bool {
    let params = SimParams::default();

    // (a) Two fresh builds are bit-identical.
    if level.interior(42).state_hash() != level.interior(42).state_hash() {
        return false;
    }

    // (b) A scripted enemy-seats match replays identically (per-tick hashes). Every declared
    // seat is driven (`enemies[i]` → `Ai(i)`) through the same [`SeatController`] dispatch the
    // game uses — so the stateful live Simple, not the parked automaton, is what replays — at
    // the game's reference decision cadence. The PLAYER seat is passive: the old greedy
    // player-proxy was removed (owner, 2026-07-10) — proxy matches measured nothing and
    // dressed their non-information up as results; the enemy seats generate ample order
    // traffic for the determinism property, which is the only thing gated here.
    let replay = |level: &Level| -> (Vec<u64>, layer1::Outcome) {
        let mut w = level.interior(7);
        let mut enemies = enemy_seats(level);
        let mut hashes = Vec::new();
        for t in 0..300u64 {
            if w.is_eliminated(Faction::Player) || all_enemies_eliminated(&w, level) {
                break;
            }
            if t % GAME_DECISION_BASE == 0 {
                for e in &mut enemies {
                    e.decide_and_apply(&mut w, &params);
                }
            }
            w.step(&params);
            hashes.push(w.state_hash());
        }
        (hashes, w.outcome())
    };
    let (h1, o1) = replay(level);
    let (h2, o2) = replay(level);
    h1 == h2 && o1 == o2
}

/// One [`SeatController`] per declared enemy seat (`level.enemies[i]` drives `Faction::Ai(i)`),
/// through the same roster→brain dispatch the game uses.
fn enemy_seats(level: &Level) -> Vec<SeatController> {
    level
        .enemies
        .iter()
        .enumerate()
        .map(|(i, &r)| SeatController::from_roster(Faction::Ai(i as u8), r))
        .collect()
}

/// True when **every** declared enemy seat is world-wide eliminated (the multi-seat analogue of
/// the old binary `is_eliminated(Ai(0))` early-out).
fn all_enemies_eliminated(w: &Interior, level: &Level) -> bool {
    (0..level.enemies.len()).all(|i| w.is_eliminated(Faction::Ai(i as u8)))
}
