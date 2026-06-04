//! A small **headless AI-vs-AI harness**: symmetric test-world builders plus a deterministic
//! match runner that pits two [`Roster`] entries (or raw [`AiController`]s) against each other
//! to a horizon and reports the [`world::WorldOutcome`].
//!
//! This is the real test of the AI layer: the strategy tests in
//! `crates/ai/tests/ai_tests.rs` and the `ai-harness` binary both drive it. The runner draws
//! no randomness of its own (all randomness lives inside each planet's `layer1` Structure), so
//! a given `(world build seed, policies, horizon, interval)` replays bit-for-bit — the basis of
//! the determinism test.

use layer1::{Faction, SimParams, Structure, SubStructure, Vec2};
use world::{Planet, World, WorldOutcome, WorldParams};

use crate::controller::{AiController, Roster};

/// How often (in ticks) each seat re-plans. Both seats decide on the **same pre-step snapshot**
/// every `decision_interval` ticks (Player issues first — a fixed, documented tie-break,
/// matching `layer1::run_auto_vs_auto`), so forces commit over time rather than re-issuing
/// every tick.
pub const DEFAULT_DECISION_INTERVAL: u64 = 8;

/// Default match horizon (ticks) for the standard test worlds — long enough for an economic
/// lead to convert and for an assault to land, short enough to keep the suite fast.
pub const DEFAULT_HORIZON: u64 = 1200;

/// Result of one match: the world outcome plus the seat that each roster entry played (so a
/// caller swapping seatings can attribute wins correctly).
#[derive(Debug, Clone, Copy)]
pub struct MatchResult {
    /// The world outcome at the horizon / elimination.
    pub outcome: WorldOutcome,
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

/// Run a single match between two [`AiController`]s on `world` to `horizon` ticks (or until one
/// seat is world-eliminated), re-planning every `decision_interval` ticks. Both controllers
/// must already be set to opposing seats. Returns the final [`WorldOutcome`].
///
/// Decision order each planning tick: both decide on the same snapshot, then **Player applies
/// first, Enemy second** (documented tie-break). Deterministic.
pub fn run_match(
    world: &mut World,
    params: &SimParams,
    wp: &WorldParams,
    a: &AiController,
    b: &AiController,
    horizon: u64,
    decision_interval: u64,
) -> WorldOutcome {
    let interval = decision_interval.max(1);
    // Order the two controllers so Player-seat applies first regardless of which is A.
    let (first, second) = if a.seat == Faction::Player { (a, b) } else { (b, a) };
    while world.tick < horizon {
        if world.is_eliminated(Faction::Player) || world.is_eliminated(Faction::Enemy) {
            break;
        }
        if world.tick % interval == 0 {
            // Decide both on the SAME pre-step snapshot (decide is a pure read), then apply
            // Player-first. We compute both decisions before applying either.
            let d_first = first.decide(world, params, wp);
            let d_second = second.decide(world, params, wp);
            first.apply(world, &d_first, wp);
            second.apply(world, &d_second, wp);
        }
        world.step(params, wp);
    }
    world.outcome()
}

/// Run roster entry `a` vs `b` on a freshly built `world` (seat A = Player, B = Enemy) to the
/// default horizon/interval. Convenience over [`run_match`].
pub fn run_roster(
    mut world: World,
    params: &SimParams,
    wp: &WorldParams,
    a: Roster,
    b: Roster,
) -> MatchResult {
    let ca = AiController::from_roster(Faction::Player, a);
    let cb = AiController::from_roster(Faction::Enemy, b);
    let outcome = run_match(
        &mut world,
        params,
        wp,
        &ca,
        &cb,
        DEFAULT_HORIZON,
        DEFAULT_DECISION_INTERVAL,
    );
    MatchResult { outcome, a_seat: Faction::Player }
}

/// Play `a` vs `b` on **both seatings** of a freshly built world (built by `build`, called
/// twice so each match starts fresh) and return `(a_wins, b_wins, draws)` over the two games.
/// This is the fair, seat-symmetric comparison the strategy tests use.
pub fn duel_both_seatings(
    build: impl Fn() -> World,
    params: &SimParams,
    wp: &WorldParams,
    a: Roster,
    b: Roster,
) -> (u32, u32, u32) {
    let mut a_wins = 0;
    let mut b_wins = 0;
    let mut draws = 0;

    // Seating 1: A=Player, B=Enemy.
    {
        let mut w = build();
        let ca = AiController::from_roster(Faction::Player, a);
        let cb = AiController::from_roster(Faction::Enemy, b);
        let o = run_match(&mut w, params, wp, &ca, &cb, DEFAULT_HORIZON, DEFAULT_DECISION_INTERVAL);
        match o.winner {
            Some(Faction::Player) => a_wins += 1,
            Some(Faction::Enemy) => b_wins += 1,
            _ => draws += 1,
        }
    }
    // Seating 2: A=Enemy, B=Player (swap seats; same map).
    {
        let mut w = build();
        let ca = AiController::from_roster(Faction::Enemy, a);
        let cb = AiController::from_roster(Faction::Player, b);
        let o = run_match(&mut w, params, wp, &cb, &ca, DEFAULT_HORIZON, DEFAULT_DECISION_INTERVAL);
        match o.winner {
            Some(Faction::Enemy) => a_wins += 1,
            Some(Faction::Player) => b_wins += 1,
            _ => draws += 1,
        }
    }
    (a_wins, b_wins, draws)
}

// ======================================================================================
// Symmetric test-world builders.
// ======================================================================================

/// Build a sub-structure planet for a home base: `subs` Player/Enemy-owned subs in a small
/// cluster (so the Layer-1 greedy internals have several positions to play), each seeded with
/// `per_sub` idle ships. `seed` keys the planet's RNG.
fn home_planet(seed: u64, owner: Faction, subs: usize, per_sub: usize, pos: Vec2, name: &str) -> Planet {
    let mut st = Structure::new(seed);
    // Lay the subs out in a tight ring so they are within fighting/engagement proximity and the
    // internal greedy can shuffle between them, but capture stays clean at setup.
    let ids: Vec<_> = (0..subs)
        .map(|i| {
            let ang = (i as f32) / (subs.max(1) as f32) * std::f32::consts::TAU;
            let r = if i == 0 { 0.0 } else { 9.0 };
            st.add_sub(SubStructure::new(
                Vec2::new(r * ang.cos(), r * ang.sin()),
                4.0,
                owner,
            ))
        })
        .collect();
    for &s in &ids {
        for _ in 0..per_sub {
            st.spawn_ship(owner, s);
        }
    }
    Planet::new(st, pos, name)
}

/// Build a neutral planet with `subs` empty neutral sub-structures (capturable production).
fn neutral_planet(seed: u64, subs: usize, pos: Vec2, name: &str) -> Planet {
    let mut st = Structure::new(seed);
    for i in 0..subs.max(1) {
        let ang = (i as f32) / (subs.max(1) as f32) * std::f32::consts::TAU;
        let r = if i == 0 { 0.0 } else { 9.0 };
        st.add_sub(SubStructure::new(Vec2::new(r * ang.cos(), r * ang.sin()), 4.0, Faction::Neutral));
    }
    Planet::new(st, pos, name)
}

/// The standard **symmetric corridor** test world: two homes at the ends of a chain of neutral
/// planets, mirror-symmetric so swapping seats is perfectly fair.
///
/// ```text
///   P(home) — n1 — n2(centre) — n3 — E(home)
/// ```
/// Both homes start with 3 owned subs at ~10 idle ships each (a real army to spend), and there
/// are three single-sub neutral planets between them: two flanking colonization targets and a
/// contested centre. Lane lengths are symmetric. This is the world the cycle measurement and
/// the seam test use; `seed` keys both planets' RNG (offset so the two homes differ).
pub fn corridor_world(seed: u64) -> World {
    let mut w = World::new();
    let p = w.add_planet(home_planet(seed, Faction::Player, 3, 10, Vec2::new(0.0, 0.0), "P-home"));
    let n1 = w.add_planet(neutral_planet(seed + 11, 1, Vec2::new(35.0, 0.0), "n1"));
    let n2 = w.add_planet(neutral_planet(seed + 12, 1, Vec2::new(70.0, 0.0), "n2-centre"));
    let n3 = w.add_planet(neutral_planet(seed + 13, 1, Vec2::new(105.0, 0.0), "n3"));
    let e = w.add_planet(home_planet(seed + 1, Faction::Enemy, 3, 10, Vec2::new(140.0, 0.0), "E-home"));
    // Symmetric chain of equal-length lanes.
    let l = 35.0;
    w.add_lane(p, n1, l);
    w.add_lane(n1, n2, l);
    w.add_lane(n2, n3, l);
    w.add_lane(n3, e, l);
    w
}

/// A slightly richer symmetric world: two homes, each with a **private** flank neutral (close,
/// safe to colonize), plus a single shared **contested centre** neutral. A diamond:
///
/// ```text
///        fP          fE          (flank neutrals, one per side)
///       /  \        /  \
///   P-home   centre   E-home
/// ```
/// `fP`/`fE` are each adjacent only to their own home and the centre; the centre bridges both
/// sides. Mirror-symmetric. Good for separating colonize (grab the easy flank) from attack
/// (push through the centre) and defend (hold the flank+home).
pub fn diamond_world(seed: u64) -> World {
    let mut w = World::new();
    let p = w.add_planet(home_planet(seed, Faction::Player, 3, 10, Vec2::new(0.0, 0.0), "P-home"));
    let e = w.add_planet(home_planet(seed + 1, Faction::Enemy, 3, 10, Vec2::new(120.0, 0.0), "E-home"));
    let fp = w.add_planet(neutral_planet(seed + 11, 1, Vec2::new(30.0, 40.0), "fP"));
    let fe = w.add_planet(neutral_planet(seed + 12, 1, Vec2::new(90.0, 40.0), "fE"));
    let centre = w.add_planet(neutral_planet(seed + 13, 2, Vec2::new(60.0, 0.0), "centre"));
    w.add_lane(p, fp, 35.0);
    w.add_lane(e, fe, 35.0);
    w.add_lane(p, centre, 45.0);
    w.add_lane(e, centre, 45.0);
    w.add_lane(fp, centre, 40.0);
    w.add_lane(fe, centre, 40.0);
    w
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn corridor_is_symmetric_in_subs_and_ships() {
        let w = corridor_world(7);
        assert_eq!(w.total_subs(Faction::Player), w.total_subs(Faction::Enemy));
        assert_eq!(w.total_ships(Faction::Player), w.total_ships(Faction::Enemy));
        assert_eq!(w.planets.len(), 5);
    }

    #[test]
    fn diamond_is_symmetric() {
        let w = diamond_world(3);
        assert_eq!(w.total_subs(Faction::Player), w.total_subs(Faction::Enemy));
        assert_eq!(w.total_ships(Faction::Player), w.total_ships(Faction::Enemy));
    }

    #[test]
    fn a_match_runs_to_a_self_consistent_outcome() {
        let params = SimParams::default();
        let wp = WorldParams::default();
        let r = run_roster(corridor_world(1), &params, &wp, Roster::Colonize, Roster::Defend);
        // The outcome's ship/sub tallies must be internally consistent (non-negative; winner
        // matches the lead unless by elimination).
        let o = r.outcome;
        assert!(o.tick <= DEFAULT_HORIZON);
        let _ = (r.a_won(), r.b_won());
        // A well-formed outcome: someone leads or it's a draw.
        let p_score = o.ships.0 + o.subs.0;
        let e_score = o.ships.1 + o.subs.1;
        if !o.by_elimination {
            match o.winner {
                Some(Faction::Player) => assert!(p_score >= e_score),
                Some(Faction::Enemy) => assert!(e_score >= p_score),
                _ => {}
            }
        }
    }
}
