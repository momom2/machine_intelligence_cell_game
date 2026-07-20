//! A small **headless AI-vs-AI harness**: symmetric test-interior builders plus a
//! deterministic match runner that pits two [`Roster`] entries (via the shared
//! [`SeatController`] dispatch) against each other to a horizon and reports the
//! [`layer1::Outcome`].
//!
//! Pure-L1 (owner pivot, 2026-07-20): matches run on ONE [`Interior`]. The runner draws no
//! randomness of its own (all randomness lives inside the interior's seeded sim), so a
//! given `(build seed, rosters, horizon, interval)` replays bit-for-bit — the basis of the
//! determinism test in `crates/ai/src/tests.rs`.

use layer1::{Faction, Interior, Outcome, SimParams, SubStructure, Vec2};

use crate::controller::{Roster, SeatController};

/// How often (in ticks) each seat re-plans. Both seats decide on the **same pre-step
/// snapshot** every `decision_interval` ticks (Player issues first — a fixed, documented
/// tie-break), so forces commit over time rather than re-issuing every tick.
pub const DEFAULT_DECISION_INTERVAL: u64 = 8;

/// The **shipped game's** seat-decision cadence, in unscaled reference ticks (the GUI runs
/// it scaled: `GAME_DECISION_BASE * TICK_SCALE` ticks at 60 Hz = the same per-second
/// cadence). Exported so headless drivers (the levels validation) measure at the operating
/// point the game actually plays.
pub const GAME_DECISION_BASE: u64 = 5;

/// Default match horizon (ticks) for the standard test interiors: long enough for the
/// resistance-grind economy to convert a lead (a fresh capture is ~`max_resistance /
/// present_force` ticks).
pub const DEFAULT_HORIZON: u64 = 3000;

/// Result of one match: the interior outcome plus the seat the "A" roster entry played
/// (so a caller swapping seatings can attribute wins correctly).
#[derive(Debug, Clone, Copy)]
pub struct MatchResult {
    /// The outcome at the horizon / elimination.
    pub outcome: Outcome,
    /// Which seat the "A" roster entry played this match.
    pub a_seat: Faction,
}

impl MatchResult {
    /// Did roster entry **A** win this match? (`false` on a loss *or* a draw.)
    pub fn a_won(&self) -> bool {
        self.outcome.winner == Some(self.a_seat)
    }
    /// Did roster entry **B** win this match?
    pub fn b_won(&self) -> bool {
        self.outcome.winner == Some(self.a_seat.opponent())
    }
}

/// Run a single match between two [`SeatController`]s on `st` to `horizon` ticks (or until
/// one seat is eliminated), re-planning every `decision_interval` ticks. The controllers
/// must be set to opposing seats. Decision order each planning tick: **Player-seat applies
/// first, the other second** (documented tie-break). Deterministic.
pub fn run_match(
    st: &mut Interior,
    params: &SimParams,
    a: &mut SeatController,
    b: &mut SeatController,
    horizon: u64,
    decision_interval: u64,
) -> Outcome {
    let interval = decision_interval.max(1);
    let player_first = a.seat() == Faction::Player;
    while st.tick < horizon {
        if st.is_eliminated(Faction::Player) || st.is_eliminated(Faction::Ai(0)) {
            break;
        }
        if st.tick % interval == 0 {
            if player_first {
                a.decide_and_apply(st, params);
                b.decide_and_apply(st, params);
            } else {
                b.decide_and_apply(st, params);
                a.decide_and_apply(st, params);
            }
        }
        st.step(params);
    }
    st.outcome()
}

/// Run roster entry `a` vs `b` on a freshly built interior (seat A = Player, B = `Ai(0)`)
/// to the default horizon/interval. Convenience over [`run_match`].
pub fn run_roster(mut st: Interior, params: &SimParams, a: Roster, b: Roster) -> MatchResult {
    let mut ca = SeatController::from_roster(Faction::Player, a);
    let mut cb = SeatController::from_roster(Faction::Ai(0), b);
    let outcome = run_match(
        &mut st,
        params,
        &mut ca,
        &mut cb,
        DEFAULT_HORIZON,
        DEFAULT_DECISION_INTERVAL,
    );
    MatchResult { outcome, a_seat: Faction::Player }
}

/// Run `a` vs `b` over BOTH seatings on interiors built by `build` (called twice — a fresh
/// board per seating, same seed = a fair mirror). Returns `(a_wins, b_wins, draws)` out
/// of 2.
pub fn duel_both_seatings(
    mut build: impl FnMut() -> Interior,
    params: &SimParams,
    a: Roster,
    b: Roster,
) -> (u32, u32, u32) {
    let (mut aw, mut bw, mut dr) = (0u32, 0u32, 0u32);
    for a_is_player in [true, false] {
        let mut st = build();
        let (ra, rb) = if a_is_player { (a, b) } else { (b, a) };
        let mut ca = SeatController::from_roster(Faction::Player, ra);
        let mut cb = SeatController::from_roster(Faction::Ai(0), rb);
        let outcome = run_match(
            &mut st,
            params,
            &mut ca,
            &mut cb,
            DEFAULT_HORIZON,
            DEFAULT_DECISION_INTERVAL,
        );
        let a_seat = if a_is_player { Faction::Player } else { Faction::Ai(0) };
        match outcome.winner {
            Some(w) if w == a_seat => aw += 1,
            Some(_) => bw += 1,
            None => dr += 1,
        }
    }
    (aw, bw, dr)
}

// =====================================================================================
// Standard symmetric test interiors.
// =====================================================================================

/// The **corridor**: two owned home subs at the ends, a line of neutrals between — the
/// expansion race board. Symmetric under 180° rotation.
pub fn corridor_interior(seed: u64) -> Interior {
    let mut st = Interior::new(seed);
    let p = st.add_sub(SubStructure::new(Vec2::new(-60.0, 0.0), 0.0, Faction::Player));
    let e = st.add_sub(SubStructure::new(Vec2::new(60.0, 0.0), 0.0, Faction::Ai(0)));
    for x in [-30.0f32, 0.0, 30.0] {
        st.add_sub(SubStructure::new(Vec2::new(x, 0.0), 0.0, Faction::Neutral));
    }
    for _ in 0..20 {
        st.spawn_ship(Faction::Player, p);
        st.spawn_ship(Faction::Ai(0), e);
    }
    st
}

/// The **diamond**: homes left/right, two contested neutrals above/below the midline —
/// the flanking board. Symmetric under vertical mirror.
pub fn diamond_interior(seed: u64) -> Interior {
    let mut st = Interior::new(seed);
    let p = st.add_sub(SubStructure::new(Vec2::new(-50.0, 0.0), 0.0, Faction::Player));
    let e = st.add_sub(SubStructure::new(Vec2::new(50.0, 0.0), 0.0, Faction::Ai(0)));
    st.add_sub(SubStructure::new(Vec2::new(0.0, 35.0), 0.0, Faction::Neutral));
    st.add_sub(SubStructure::new(Vec2::new(0.0, -35.0), 0.0, Faction::Neutral));
    for _ in 0..20 {
        st.spawn_ship(Faction::Player, p);
        st.spawn_ship(Faction::Ai(0), e);
    }
    st
}
