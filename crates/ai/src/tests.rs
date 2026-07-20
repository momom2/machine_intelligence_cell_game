//! Headless validation of the AI layer (run as **lib** tests so they execute reliably under
//! the Windows app-control policy that blocks freshly-linked standalone binaries — see
//! `AI.md`). Pure-L1 (owner pivot, 2026-07-20): matches run on one [`Interior`] via the
//! shared [`SeatController`] dispatch. The invariants pinned:
//!
//! * an active seat **beats the inert dummy** on the standard boards (the harness drives
//!   real progress, not stalemates),
//! * the stateful **Simple** brain plays and wins as fielded by the campaign dispatch, and
//! * **determinism**: same seed + same rosters ⇒ identical `state_hash` and outcome.

use crate::controller::{Roster, SeatController};
use crate::harness::{
    corridor_interior, diamond_interior, duel_both_seatings, run_match, DEFAULT_DECISION_INTERVAL,
    DEFAULT_HORIZON,
};
use layer1::{Faction, SimParams};

#[test]
fn greedy_beats_passive_on_the_corridor() {
    let params = SimParams::default();
    let (aw, bw, _dr) =
        duel_both_seatings(|| corridor_interior(1), &params, Roster::GreedyLocal, Roster::Passive);
    assert_eq!(aw, 2, "greedy should win both seatings against the dummy (got {aw}-{bw})");
}

#[test]
fn simple_beats_passive_on_the_diamond() {
    let params = SimParams::default();
    let (aw, bw, _dr) = duel_both_seatings(
        || diamond_interior(3),
        &params,
        Roster::SimpleColonize,
        Roster::Passive,
    );
    assert_eq!(aw, 2, "Simple should win both seatings against the dummy (got {aw}-{bw})");
}

/// The harness path is itself deterministic: two identical runs produce identical hashes
/// and outcomes.
#[test]
fn determinism_run_match_is_stable() {
    let params = SimParams::default();
    let run = || {
        let mut st = diamond_interior(7);
        let mut a = SeatController::from_roster(Faction::Player, Roster::GreedyLocal);
        let mut b = SeatController::from_roster(Faction::Ai(0), Roster::SimpleColonize);
        let out =
            run_match(&mut st, &params, &mut a, &mut b, DEFAULT_HORIZON, DEFAULT_DECISION_INTERVAL);
        (st.state_hash(), out.winner, out.tick, out.ships, out.subs)
    };
    assert_eq!(run(), run(), "same seed + rosters must replay bit-identically");
}

/// The Cycler brain runs without panicking and issues orders on a board with its shape
/// (owned subs + a foe) — a smoke pin; its full behaviour is mission-tuned by play.
#[test]
fn cycler_runs_and_acts() {
    // The Cycler is designed around PRODUCTION pushing a sub's count past its storage
    // capacity (owner clarification, 2026-07-20) — cycling starts from production
    // overflow, never from a pre-stacked over-cap garrison. So: two producing owned subs
    // starting under cap; the horizon lets production cross the capacity line, and the
    // surplus then drills between them. A far-away passive player sub keeps the match
    // two-seated.
    use layer1::{Interior, SubStructure, Vec2};
    let params = SimParams::default();
    let mut st = Interior::new(11);
    // Under REFERENCE params the legacy per-structure softcap plateaus each sub's stock
    // around ~20 — below the default per-sub capacity of 60, so cap-60 subs can never
    // overflow here. Capacity 10 puts the overflow line under the plateau, letting the
    // designed production-overflow dynamic fire. (The shipped game runs the per-sub
    // attrition economy instead, where production overflows capacity directly.)
    let a = st.add_sub(
        SubStructure::new(Vec2::new(0.0, 0.0), 0.0, Faction::Ai(0)).with_storage_capacity(10),
    );
    let _b = st.add_sub(
        SubStructure::new(Vec2::new(24.0, 0.0), 0.0, Faction::Ai(0)).with_storage_capacity(10),
    );
    let p = st.add_sub(SubStructure::new(Vec2::new(200.0, 0.0), 0.0, Faction::Player));
    for _ in 0..8 {
        st.spawn_ship(Faction::Ai(0), a);
    }
    for _ in 0..10 {
        st.spawn_ship(Faction::Player, p);
    }
    let mut cy = SeatController::from_roster(Faction::Ai(0), Roster::Cycler);
    let mut pl = SeatController::from_roster(Faction::Player, Roster::Passive);
    let mut moved_any = false;
    for _ in 0..DEFAULT_HORIZON {
        if st.tick % DEFAULT_DECISION_INTERVAL == 0 {
            pl.decide_and_apply(&mut st, &params);
            moved_any |= cy.decide_and_apply(&mut st, &params) > 0;
        }
        st.step(&params);
    }
    assert!(moved_any, "the Cycler should cycle its over-cap surplus at least once");
}
