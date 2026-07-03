//! Match-running and cycle-evaluation helpers, used by tests (the `r2-sweep` binary that
//! also drove them was deleted). Kept in the library because cell-core is also the
//! future Architect fitness evaluator, which needs exactly this "run a matchup,
//! get a scalar" capability.

use crate::engine::{GameState, MatchOutcome, Params};
use crate::policy::{Attack, Colonize, Defend, Policy};
use crate::types::Owner;

/// The three pure archetypes, as an enum, so the sweep can iterate matchups
/// generically and name them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Archetype {
    Colonize,
    Defend,
    Attack,
}

impl Archetype {
    pub fn name(self) -> &'static str {
        match self {
            Archetype::Colonize => "Colonize",
            Archetype::Defend => "Defend",
            Archetype::Attack => "Attack",
        }
    }

    /// Construct a fresh boxed policy for this archetype.
    pub fn make(self) -> Box<dyn Policy> {
        match self {
            Archetype::Colonize => Box::new(Colonize),
            Archetype::Defend => Box::new(Defend),
            Archetype::Attack => Box::new(Attack),
        }
    }
}

/// Outcome of playing a matchup on one map across **both seatings**.
///
/// `score` is the mean score *for the first-named archetype* across the two
/// seatings, in [-1, 1]. Playing both seatings and averaging cancels the fixed
/// "A acts before B" tie-break and any residual positional asymmetry, so a
/// positive `score` means the first archetype genuinely beat the second.
#[derive(Debug, Clone, Copy)]
pub struct DuelResult {
    pub first: Archetype,
    pub second: Archetype,
    /// Mean score for `first` across both seatings, in [-1, 1].
    pub score: f64,
    /// The two raw per-seating scores (first-as-A, first-as-B), for reporting.
    pub score_first_as_a: f64,
    pub score_first_as_b: f64,
}

/// Run a single match of `first` vs `second` on a fresh copy of `base`, returning
/// the outcome from A's perspective.
fn one_match(
    base: &GameState,
    a: Archetype,
    b: Archetype,
    params: &Params,
    horizon: u64,
) -> MatchOutcome {
    let mut pa = a.make();
    let mut pb = b.make();
    base.clone()
        .run_match(pa.as_mut(), pb.as_mut(), params, horizon)
}

/// Play `first` vs `second` on `base` in **both seatings** and average the score
/// for `first`. This is the core "who beats whom, fairly" primitive.
pub fn duel(
    base: &GameState,
    first: Archetype,
    second: Archetype,
    params: &Params,
    horizon: u64,
) -> DuelResult {
    // Seating 1: first = A, second = B. score_a is first's score directly.
    let m1 = one_match(base, first, second, params, horizon);
    let first_as_a = m1.score_a;

    // Seating 2: second = A, first = B. first's score is -score_a.
    let m2 = one_match(base, second, first, params, horizon);
    let first_as_b = -m2.score_a;

    DuelResult {
        first,
        second,
        score: 0.5 * (first_as_a + first_as_b),
        score_first_as_a: first_as_a,
        score_first_as_b: first_as_b,
    }
}

/// The three directed edges the cycle requires, evaluated at one `(map, params)`
/// point. Each value is the margin by which the intended winner beats the intended
/// loser (mean over both seatings). The cycle "holds" iff all three exceed
/// `epsilon`.
#[derive(Debug, Clone, Copy)]
pub struct CyclePoint {
    /// attack ≻ colonize margin (score for Attack vs Colonize).
    pub attack_beats_colonize: f64,
    /// colonize ≻ defend margin (score for Colonize vs Defend).
    pub colonize_beats_defend: f64,
    /// defend ≻ attack margin (score for Defend vs Attack).
    pub defend_beats_attack: f64,
}

impl CyclePoint {
    /// The weakest of the three edges — the margin by which the cycle holds (or
    /// fails, if negative). A robust operating point maximizes this.
    pub fn min_margin(&self) -> f64 {
        self.attack_beats_colonize
            .min(self.colonize_beats_defend)
            .min(self.defend_beats_attack)
    }

    /// True iff every intended counter wins by more than `epsilon`.
    pub fn holds(&self, epsilon: f64) -> bool {
        self.attack_beats_colonize > epsilon
            && self.colonize_beats_defend > epsilon
            && self.defend_beats_attack > epsilon
    }
}

/// Evaluate the three cycle edges on a single map at one params point, averaging
/// both seatings for each edge.
pub fn cycle_on_map(base: &GameState, params: &Params, horizon: u64) -> CyclePoint {
    let ac = duel(base, Archetype::Attack, Archetype::Colonize, params, horizon).score;
    let cd = duel(base, Archetype::Colonize, Archetype::Defend, params, horizon).score;
    let da = duel(base, Archetype::Defend, Archetype::Attack, params, horizon).score;
    CyclePoint {
        attack_beats_colonize: ac,
        colonize_beats_defend: cd,
        defend_beats_attack: da,
    }
}

/// Evaluate the cycle across several maps and **average each edge across maps**.
/// Averaging (rather than requiring all maps to agree) gives a robust aggregate
/// signal; the sweep also reports per-map detail at the operating point.
pub fn cycle_over_maps(maps: &[GameState], params: &Params, horizon: u64) -> CyclePoint {
    let mut ac = 0.0;
    let mut cd = 0.0;
    let mut da = 0.0;
    for m in maps {
        let c = cycle_on_map(m, params, horizon);
        ac += c.attack_beats_colonize;
        cd += c.colonize_beats_defend;
        da += c.defend_beats_attack;
    }
    let n = maps.len().max(1) as f64;
    CyclePoint {
        attack_beats_colonize: ac / n,
        colonize_beats_defend: cd / n,
        defend_beats_attack: da / n,
    }
}

/// Convenience accessor for the standard players (used in a couple of tests).
pub fn standard_players() -> (Owner, Owner) {
    (Owner::A, Owner::B)
}
