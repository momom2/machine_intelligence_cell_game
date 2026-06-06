//! Unit tests for the event-driven forward [`super::Projection`].
//!
//! The load-bearing check (R3) is **fast == reference**: the closed-form, event-driven integrator
//! must match a straight tick-by-tick reference integrator (the same per-tick kernel run every
//! tick) on a battery of small hand-built worlds, within rounding. Plus: a hand-computed grind
//! ETA, determinism, no state perturbation, and the derived queries / fleet-arrival timing.

use super::*;
use crate::{FleetOrder, Lane, Planet, World, WorldParams};
use layer1::{Faction, FractionBucket, SimParams, Structure, SubStructure, Vec2};

// ---------------------------------------------------------------------------
// Builders
// ---------------------------------------------------------------------------

/// A single-sub planet owned by `owner` with `garrison` idle ships of `owner`, sub radius 5 at the
/// local origin. `max_res` overrides the (otherwise huge) fresh resistance so grinds finish inside
/// a test horizon.
fn planet_1sub(seed: u64, owner: Faction, garrison: usize, max_res: f32, map: Vec2, name: &str) -> Planet {
    let mut st = Structure::new(seed);
    let s = st.add_sub(SubStructure::new(Vec2::new(0.0, 0.0), 5.0, owner).with_max_resistance(max_res));
    for _ in 0..garrison {
        st.spawn_ship(owner, s);
    }
    let _ = s;
    Planet::new(st, map, name)
}

/// Lower a planet's sub resistance/max in place (cheap-foothold helper).
fn soften(w: &mut World, p: PlanetId, sub: usize, max: f32) {
    let s = &mut w.planets[p].structure.subs[sub];
    let m = max.max(1.0);
    s.max_resistance = m;
    s.resistance = m;
}

// ---------------------------------------------------------------------------
// fast == reference (the central correctness battery)
// ---------------------------------------------------------------------------

/// Compare every sub's fate between the fast and reference integrators for a world, allowing a
/// tiny rounding slack on the horizon resistance and an off-by-one on ETAs (closed-form ceilings
/// vs tick-by-tick can differ by at most one tick on a flip boundary).
fn assert_fast_matches_reference(w: &World, sp: &SimParams, wp: &WorldParams, horizon: u64, ctx: &str) {
    let fast = w.project_forward(sp, wp, horizon);
    let refp = w.project_reference(sp, wp, horizon);
    assert_eq!(fast.base_index, refp.base_index, "{ctx}: planet layout differs");
    for p in 0..w.planets.len() {
        for s in 0..w.planets[p].structure.subs.len() {
            let f = fast.sub_fate(p, s);
            let r = refp.sub_fate(p, s);
            assert_eq!(f.current_owner, r.current_owner, "{ctx}: p{p} s{s} current_owner");
            assert_eq!(
                f.owner_at_horizon, r.owner_at_horizon,
                "{ctx}: p{p} s{s} owner_at_horizon (fast={:?} ref={:?})",
                f.owner_at_horizon, r.owner_at_horizon
            );
            assert_eq!(
                f.owner_after_first_change, r.owner_after_first_change,
                "{ctx}: p{p} s{s} owner_after_first_change"
            );
            // ETAs: equal, or within one tick (boundary rounding between closed-form and per-tick).
            match (f.eta_first_change, r.eta_first_change) {
                (Some(a), Some(b)) => assert!(
                    a.abs_diff(b) <= 1,
                    "{ctx}: p{p} s{s} eta_first_change fast={a} ref={b} differ by >1"
                ),
                (None, None) => {}
                (a, b) => panic!("{ctx}: p{p} s{s} eta_first_change presence differs fast={a:?} ref={b:?}"),
            }
            // Horizon resistance: allow a small absolute slack (one tick of grind/heal/combat).
            let dr = (f.resistance_at_horizon - r.resistance_at_horizon).abs();
            let slack = (f.resistance_at_horizon.max(r.resistance_at_horizon) * 0.02).max(2.0);
            assert!(
                dr <= slack,
                "{ctx}: p{p} s{s} resistance_at_horizon fast={} ref={} (|Δ|={dr} > {slack})",
                f.resistance_at_horizon, r.resistance_at_horizon
            );
            assert_eq!(f.became_contested, r.became_contested, "{ctx}: p{p} s{s} became_contested");
        }
    }
}

#[test]
fn fast_matches_reference_quiet_owned_world() {
    // Two owned planets, no enemy, no fleets: pure heal-and-produce. Fast jumps across spawns;
    // reference steps each. They must agree.
    let sp = SimParams::default();
    let wp = WorldParams::default();
    let mut w = World::new();
    // Damaged sub so the heal actually moves (start below max).
    let mut a = planet_1sub(1, Faction::Player, 5, 100.0, Vec2::new(0.0, 0.0), "A");
    a.structure.subs[0].resistance = 40.0;
    w.add_planet(a);
    w.add_planet(planet_1sub(2, Faction::Enemy, 4, 100.0, Vec2::new(80.0, 0.0), "B"));
    assert_fast_matches_reference(&w, &sp, &wp, 300, "quiet_owned");
}

#[test]
fn fast_matches_reference_lone_attacker_grind() {
    // A neutral sub with an attacker's idle ships already inside it: a pure uncontested grind to a
    // flip, then a heal as the new owner. Fast computes the flip closed-form; reference steps it.
    let sp = SimParams::default();
    let wp = WorldParams::default();
    let mut w = World::new();
    let mut st = Structure::new(3);
    let neutral = st.add_sub(SubStructure::new(Vec2::new(0.0, 0.0), 6.0, Faction::Neutral).with_max_resistance(30.0));
    // 4 Player ships sitting on the neutral sub (idle, inside radius).
    for _ in 0..4 {
        st.spawn_ship(Faction::Player, neutral);
    }
    w.add_planet(Planet::new(st, Vec2::new(0.0, 0.0), "G"));
    assert_fast_matches_reference(&w, &sp, &wp, 240, "lone_attacker");

    // Hand check: 4 attackers vs resistance 30 => flip near ceil(30/4)=8 ticks from base+1.
    let proj = w.project_forward(&sp, &wp, 240);
    let cap = proj.sub_capture(0, 0).expect("should flip");
    assert_eq!(cap.0, Faction::Player, "attacker captures the neutral");
    assert!(
        cap.1 >= proj.base_tick + 7 && cap.1 <= proj.base_tick + 9,
        "flip eta ~ ceil(30/4)=8 ticks, got {}",
        cap.1 - proj.base_tick
    );
}

#[test]
fn fast_matches_reference_contested_combat() {
    // Both factions co-present on one sub: combat runs (mean square law), grind frozen. This is the
    // tick-by-tick regime in the fast path, so it should match the reference exactly.
    let sp = SimParams::default();
    let wp = WorldParams::default();
    let mut st = Structure::new(4);
    let sub = st.add_sub(SubStructure::new(Vec2::new(0.0, 0.0), 8.0, Faction::Player).with_max_resistance(50.0));
    for _ in 0..10 {
        st.spawn_ship(Faction::Player, sub);
    }
    for _ in 0..8 {
        st.spawn_ship(Faction::Enemy, sub);
    }
    let mut w = World::new();
    w.add_planet(Planet::new(st, Vec2::new(0.0, 0.0), "C"));
    assert_fast_matches_reference(&w, &sp, &wp, 240, "contested");
    let proj = w.project_forward(&sp, &wp, 240);
    assert!(proj.sub_fate(0, 0).became_contested, "co-present => became_contested");
}

#[test]
fn fast_matches_reference_with_intra_moves_and_fleet() {
    // A world exercising both arrival sources: an intra-structure move toward a neutral sub on
    // planet A, plus an inter-planet fleet A->B that lands and grinds B's neutral. Fast schedules
    // both as events; reference replays them tick-by-tick.
    let sp = SimParams::default();
    let wp = WorldParams::default();
    let mut w = World::new();

    // Planet A: a Player home sub + a neutral sub far apart; issue an intra move home->neutral.
    let mut ast = Structure::new(5);
    let home = ast.add_sub(SubStructure::new(Vec2::new(0.0, 0.0), 5.0, Faction::Player));
    let aneut = ast.add_sub(SubStructure::new(Vec2::new(30.0, 0.0), 5.0, Faction::Neutral).with_max_resistance(20.0));
    for _ in 0..12 {
        ast.spawn_ship(Faction::Player, home);
    }
    // Move half of home's idle ships toward the neutral sub (now in intra-structure transit).
    ast.issue_order(layer1::MoveOrder::new(home, aneut, FractionBucket::Half));
    let a = w.add_planet(Planet::new(ast, Vec2::new(0.0, 0.0), "A"));

    // Planet B: a lone neutral sub to be invaded by a fleet from A.
    let b = w.add_planet(planet_1sub(6, Faction::Neutral, 0, 20.0, Vec2::new(40.0, 0.0), "B"));
    w.add_lane(a, b, 20.0).unwrap();
    soften(&mut w, b, 0, 20.0);
    // Launch a fleet A->B (pulls idle ships off A's owned subs).
    w.issue_fleet_order(FleetOrder::new(a, b, FractionBucket::Half), Faction::Player, &wp);

    assert_fast_matches_reference(&w, &sp, &wp, 300, "moves_and_fleet");
}

#[test]
fn fast_matches_reference_mid_run_states() {
    // Step a live world a few times (creating partial progress, mid-transit fleets, damaged subs),
    // then re-project from several mid-run states. Each must still match the reference.
    let sp = SimParams::default();
    let wp = WorldParams::default();
    let mut w = World::new();
    let a = w.add_planet(planet_1sub(7, Faction::Player, 14, 100.0, Vec2::new(0.0, 0.0), "A"));
    let b = w.add_planet(planet_1sub(8, Faction::Enemy, 14, 100.0, Vec2::new(60.0, 0.0), "B"));
    let c = w.add_planet(planet_1sub(9, Faction::Neutral, 0, 40.0, Vec2::new(30.0, 40.0), "C"));
    w.add_lane(a, c, 25.0).unwrap();
    w.add_lane(b, c, 25.0).unwrap();
    w.add_lane(a, b, 50.0).unwrap();
    soften(&mut w, c, 0, 40.0);

    for t in 0..120u64 {
        if t == 3 {
            w.issue_fleet_order(FleetOrder::new(a, c, FractionBucket::Half), Faction::Player, &wp);
            w.issue_fleet_order(FleetOrder::new(b, c, FractionBucket::Quarter), Faction::Enemy, &wp);
        }
        w.step(&sp, &wp);
        if t % 17 == 0 {
            assert_fast_matches_reference(&w, &sp, &wp, 200, &format!("mid_run@{}", w.tick));
        }
    }
}

// ---------------------------------------------------------------------------
// Determinism + no state perturbation
// ---------------------------------------------------------------------------

#[test]
fn projection_is_deterministic() {
    let sp = SimParams::default();
    let wp = WorldParams::default();
    let mut w = World::new();
    let a = w.add_planet(planet_1sub(11, Faction::Player, 10, 60.0, Vec2::new(0.0, 0.0), "A"));
    let b = w.add_planet(planet_1sub(12, Faction::Neutral, 0, 30.0, Vec2::new(30.0, 0.0), "B"));
    w.add_lane(a, b, 20.0).unwrap();
    w.issue_fleet_order(FleetOrder::new(a, b, FractionBucket::Half), Faction::Player, &wp);
    w.step(&sp, &wp);

    let p1 = w.project_forward(&sp, &wp, 240);
    let p2 = w.project_forward(&sp, &wp, 240);
    assert_eq!(p1.base_tick, p2.base_tick);
    assert_eq!(p1.horizon, p2.horizon);
    for p in 0..w.planets.len() {
        for s in 0..w.planets[p].structure.subs.len() {
            assert_eq!(p1.sub_fate(p, s), p2.sub_fate(p, s), "fate p{p} s{s} not deterministic");
        }
    }
}

#[test]
fn projection_does_not_perturb_state_hash() {
    // The projection is a pure read: it must not touch any planet RNG or mutate state, so the world
    // hash is identical before and after a project (the determinism contract / §5 note).
    let sp = SimParams::default();
    let wp = WorldParams::default();
    let mut w = World::new();
    let a = w.add_planet(planet_1sub(13, Faction::Player, 8, 100.0, Vec2::new(0.0, 0.0), "A"));
    let b = w.add_planet(planet_1sub(14, Faction::Enemy, 8, 100.0, Vec2::new(40.0, 0.0), "B"));
    w.add_lane(a, b, 40.0).unwrap();
    w.issue_fleet_order(FleetOrder::new(a, b, FractionBucket::Half), Faction::Player, &wp);
    for _ in 0..10 {
        w.step(&sp, &wp);
    }
    let before = w.state_hash();
    let _ = w.project_forward(&sp, &wp, 240);
    let _ = w.project_forward(&sp, &wp, 1000);
    let after = w.state_hash();
    assert_eq!(before, after, "project_forward perturbed the world state hash");
}

// ---------------------------------------------------------------------------
// Derived queries + fleet-arrival timing
// ---------------------------------------------------------------------------

#[test]
fn fleet_arrival_ticks_matches_world_step() {
    // The closed-form fleet arrival timing must equal the tick on which `World::step` actually
    // injects the fleet's ships. Launch a fleet, read the predicted tick, then step until the
    // ships appear on the destination and compare.
    let sp = SimParams::default();
    let wp = WorldParams::default();
    let mut w = World::new();
    let a = w.add_planet(planet_1sub(15, Faction::Player, 12, 100.0, Vec2::new(0.0, 0.0), "A"));
    let b = w.add_planet(planet_1sub(16, Faction::Neutral, 0, 100.0, Vec2::new(33.0, 0.0), "B"));
    w.add_lane(a, b, 33.0).unwrap();
    w.issue_fleet_order(FleetOrder::new(a, b, FractionBucket::Half), Faction::Player, &wp);
    let f = w.fleets[0];
    let predicted_inject_tick = w.tick + fleet_arrival_ticks(&w, &wp, &f);

    // Step until B has Player ships garrisoned (the injection happened at end of that tick).
    let mut injected_at = None;
    for _ in 0..200 {
        w.step(&sp, &wp);
        if w.planets[b].structure.ship_count(Faction::Player) > 0 {
            injected_at = Some(w.tick);
            break;
        }
    }
    let injected_at = injected_at.expect("fleet should have landed");
    // `World::step` injects at the END of `predicted_inject_tick`; the world tick is then that
    // value (tick++ happens after injection within the same step), so they match exactly.
    assert_eq!(injected_at, predicted_inject_tick, "fleet_arrival_ticks mis-predicted injection tick");
}

#[test]
fn incoming_and_returning_force_count_arrivals() {
    // A fleet of known size toward a neutral sub: incoming_present_at should report exactly the
    // fleet count for the sender, and returning_owner_force should be 0 (neutral owner is not a
    // real seat / has no in-flight ships of its own).
    let sp = SimParams::default();
    let wp = WorldParams::default();
    let mut w = World::new();
    let a = w.add_planet(planet_1sub(17, Faction::Player, 12, 100.0, Vec2::new(0.0, 0.0), "A"));
    let b = w.add_planet(planet_1sub(18, Faction::Neutral, 0, 100.0, Vec2::new(20.0, 0.0), "B"));
    w.add_lane(a, b, 20.0).unwrap();
    let launched = w.issue_fleet_order(FleetOrder::new(a, b, FractionBucket::Half), Faction::Player, &wp);
    assert!(launched > 0);

    let proj = w.project_forward(&sp, &wp, 300);
    let entry = w.entry_sub(b, a, Faction::Player).unwrap();
    assert_eq!(
        proj.incoming_present_at(b, entry, Faction::Player),
        launched,
        "all launched ships counted as incoming at the entry sub"
    );
    assert_eq!(proj.returning_owner_force(b, entry), 0, "neutral owner has no returning force");
}

#[test]
fn planet_capture_rolls_up_subs() {
    // A single-sub neutral planet with an attacker on it flips => planet_capture reports the
    // attacker and a tick equal to the sub's flip eta.
    let sp = SimParams::default();
    let wp = WorldParams::default();
    let mut st = Structure::new(19);
    let neut = st.add_sub(SubStructure::new(Vec2::new(0.0, 0.0), 6.0, Faction::Neutral).with_max_resistance(24.0));
    for _ in 0..6 {
        st.spawn_ship(Faction::Player, neut);
    }
    let mut w = World::new();
    let p = w.add_planet(Planet::new(st, Vec2::new(0.0, 0.0), "P"));

    let proj = w.project_forward(&sp, &wp, 240);
    let pc = proj.planet_capture(p).expect("planet should flip to the attacker");
    assert_eq!(pc.0, Faction::Player);
    let sc = proj.sub_capture(p, 0).unwrap();
    assert_eq!(pc.1, sc.1, "planet flip tick == its only sub's flip tick");
}

#[test]
fn planet_capture_none_when_neutral_remains() {
    // A two-sub planet: Player owns one, the other is neutral with NO attacker. The planet never
    // becomes fully one faction's, so planet_capture is None even though one sub is owned.
    let sp = SimParams::default();
    let wp = WorldParams::default();
    let mut st = Structure::new(20);
    let owned = st.add_sub(SubStructure::new(Vec2::new(0.0, 0.0), 5.0, Faction::Player));
    let _neut = st.add_sub(SubStructure::new(Vec2::new(40.0, 0.0), 5.0, Faction::Neutral));
    for _ in 0..6 {
        st.spawn_ship(Faction::Player, owned);
    }
    let mut w = World::new();
    let p = w.add_planet(Planet::new(st, Vec2::new(0.0, 0.0), "P"));
    let proj = w.project_forward(&sp, &wp, 240);
    assert!(proj.planet_capture(p).is_none(), "a remaining un-attacked neutral blocks a clean roll-up");
}

#[test]
fn planet_first_fall_picks_earliest_owned_loss() {
    // Two Player-owned subs on one planet, each with an enemy detachment eroding it; the sub with
    // the larger enemy force (faster grind) falls first, and planet_first_fall reports it.
    let sp = SimParams::default();
    let wp = WorldParams::default();
    let mut st = Structure::new(21);
    // Far apart so the two erosions are independent (no cross-radius combat).
    let s0 = st.add_sub(SubStructure::new(Vec2::new(0.0, 0.0), 5.0, Faction::Player).with_max_resistance(40.0));
    let s1 = st.add_sub(SubStructure::new(Vec2::new(200.0, 0.0), 5.0, Faction::Player).with_max_resistance(40.0));
    // Enemy-only presence on each owned sub (owner absent => pure erosion, no combat, no heal).
    for _ in 0..2 {
        st.spawn_ship(Faction::Enemy, s0); // 2 attackers => slower
    }
    for _ in 0..8 {
        st.spawn_ship(Faction::Enemy, s1); // 8 attackers => faster
    }
    let mut w = World::new();
    let p = w.add_planet(Planet::new(st, Vec2::new(0.0, 0.0), "P"));

    let proj = w.project_forward(&sp, &wp, 240);
    let (fall_sub, _t) = proj.planet_first_fall(p, Faction::Player).expect("an owned sub falls");
    assert_eq!(fall_sub, s1, "the more-heavily-eroded sub falls first");
}

// ---------------------------------------------------------------------------
// Horizon + OOB edge cases
// ---------------------------------------------------------------------------

#[test]
fn zero_horizon_and_oob_are_trivial() {
    let sp = SimParams::default();
    let wp = WorldParams::default();
    let mut w = World::new();
    let p = w.add_planet(planet_1sub(22, Faction::Player, 3, 100.0, Vec2::new(0.0, 0.0), "A"));
    let proj = w.project_forward(&sp, &wp, 0);
    let f = proj.sub_fate(p, 0);
    assert_eq!(f.eta_first_change, None, "zero horizon never sees a change");
    assert_eq!(f.owner_at_horizon, Faction::Player);
    // Out-of-range ids yield the trivial unchanged fate / empty queries.
    assert_eq!(proj.sub_fate(999, 0).owner_at_horizon, Faction::Neutral);
    assert_eq!(proj.sub_capture(999, 0), None);
    assert_eq!(proj.planet_capture(999), None);
    assert_eq!(proj.incoming_present_at(999, 0, Faction::Player), 0);
}

#[test]
fn lane_helpers_and_arrival_for_degenerate_lane() {
    // A degenerate (non-positive) lane length still yields a finite arrival, mirroring the sim's
    // f_lane_len clamp; a missing lane likewise clamps to length 1.
    let wp = WorldParams::default();
    let mut w = World::new();
    let a = w.add_planet(planet_1sub(23, Faction::Player, 6, 100.0, Vec2::new(0.0, 0.0), "A"));
    let b = w.add_planet(planet_1sub(24, Faction::Neutral, 0, 100.0, Vec2::new(5.0, 0.0), "B"));
    w.add_lane(a, b, 0.0).unwrap(); // degenerate length
    w.issue_fleet_order(FleetOrder::new(a, b, FractionBucket::All), Faction::Player, &wp);
    let f = w.fleets[0];
    let t = fleet_arrival_ticks(&w, &wp, &f);
    assert!(t < u64::MAX, "degenerate lane still arrives in finite time");
    // undock(6) + 1 transit tick (dprog clamps to 1.0 for non-positive length).
    assert_eq!(t, wp.undock_ticks as u64 + 1);
    let _ = Lane::new(a, b, 1.0); // touch the type so the import is used in all cfgs
}

// ---------------------------------------------------------------------------
// R3 composable query vocabulary
// ---------------------------------------------------------------------------

/// A one-planet world: `garrison` Player ships sitting on a neutral sub of resistance `max_res`,
/// plus an empty Player-owned home sub at `home_pos` to act as a `from_position` for marginal
/// reasoning. Returns `(world, capturing_sub, home_sub)`.
fn lone_grind_world(seed: u64, garrison: usize, max_res: f32, home_pos: Vec2) -> (World, usize, usize) {
    let mut st = Structure::new(seed);
    let target = st.add_sub(SubStructure::new(Vec2::new(0.0, 0.0), 6.0, Faction::Neutral).with_max_resistance(max_res));
    let home = st.add_sub(SubStructure::new(home_pos, 5.0, Faction::Player));
    for _ in 0..garrison {
        st.spawn_ship(Faction::Player, target);
    }
    let mut w = World::new();
    w.add_planet(Planet::new(st, Vec2::new(0.0, 0.0), "G"));
    (w, target, home)
}

#[test]
fn capture_eta_equals_sub_capture_tick() {
    // capture_eta is just the first-change tick; it must equal sub_capture's tick.
    let sp = SimParams::default();
    let wp = WorldParams::default();
    let (w, tgt, _home) = lone_grind_world(40, 5, 30.0, Vec2::new(50.0, 0.0));
    let proj = w.project_forward(&sp, &wp, 240);
    let eta = proj.capture_eta(0, tgt).expect("neutral with attackers flips");
    let (_who, cap_tick) = proj.sub_capture(0, tgt).unwrap();
    assert_eq!(eta, cap_tick, "capture_eta must equal sub_capture's tick");
    // OOB ids: no eta.
    assert_eq!(proj.capture_eta(999, 0), None);
}

#[test]
fn capture_eta_if_more_ships_never_later() {
    // MONOTONICITY: adding more attacking ships (same arrival time) must never push the flip
    // *later*; it should be the same or sooner. Sweep extra = 0..8.
    let sp = SimParams::default();
    let wp = WorldParams::default();
    let (w, tgt, _home) = lone_grind_world(41, 3, 60.0, Vec2::new(40.0, 0.0));
    let proj = w.project_forward(&sp, &wp, 400);

    let mut prev: Option<u64> = None;
    for extra in 0..=8u32 {
        let eta = proj.capture_eta_if(0, tgt, extra, 1, Faction::Player);
        if let (Some(p), Some(e)) = (prev, eta) {
            assert!(e <= p, "extra={extra}: eta {e} should be <= previous {p} (more ships, not slower)");
        }
        // Once a flip exists it must keep existing as we add ships (monotone reachability).
        if prev.is_some() {
            assert!(eta.is_some(), "extra={extra}: adding ships removed a previously-reachable flip");
        }
        if eta.is_some() {
            prev = eta;
        }
    }
    assert!(prev.is_some(), "with up to 8 extra attackers the sub must flip within the horizon");
}

#[test]
fn capture_eta_if_earlier_arrival_never_later() {
    // A fixed extra force arriving SOONER must flip the sub no later than arriving later.
    let sp = SimParams::default();
    let wp = WorldParams::default();
    let (w, tgt, _home) = lone_grind_world(42, 2, 80.0, Vec2::new(40.0, 0.0));
    let proj = w.project_forward(&sp, &wp, 400);

    let early = proj.capture_eta_if(0, tgt, 6, 5, Faction::Player);
    let late = proj.capture_eta_if(0, tgt, 6, 60, Faction::Player);
    if let (Some(e), Some(l)) = (early, late) {
        assert!(e <= l, "earlier reinforcement {e} should flip no later than later {l}");
    }
}

#[test]
fn capture_eta_if_matches_a_real_extra_arrival() {
    // capture_eta_if(extra, delay) should agree (within the integrator's 1-tick boundary slack)
    // with actually seeding those extra ships as a scheduled arrival. We approximate "a real
    // arrival" by comparing two projections: baseline vs one where we manually add idle ships that
    // are already present (delay 0) — the if-query with delay 0 must match present-from-the-start.
    let sp = SimParams::default();
    let wp = WorldParams::default();

    // Baseline: 3 attackers on a res-40 neutral.
    let (w0, tgt, _h) = lone_grind_world(43, 3, 40.0, Vec2::new(40.0, 0.0));
    let p0 = w0.project_forward(&sp, &wp, 300);
    let eta_if = p0.capture_eta_if(0, tgt, 4, 0, Faction::Player); // +4 present now => 7 total

    // Ground truth: 7 attackers present from the start.
    let (w1, tgt1, _h1) = lone_grind_world(43, 7, 40.0, Vec2::new(40.0, 0.0));
    let p1 = w1.project_forward(&sp, &wp, 300);
    let eta_real = p1.capture_eta(0, tgt1);

    match (eta_if, eta_real) {
        (Some(a), Some(b)) => assert!(
            a.abs_diff(b) <= 1,
            "capture_eta_if(+4 @0) = {a} should match 7-present flip {b} within 1 tick"
        ),
        other => panic!("both should flip; got {other:?}"),
    }
}

#[test]
fn marginal_ticks_saved_is_nonnegative_and_diminishes() {
    // The value of one more ship is >= 0 and (square-law) larger when the current force is small.
    let sp = SimParams::default();
    let wp = WorldParams::default();

    // Few attackers => one more ship saves more ticks; many => it saves less (diminishing).
    let (w_small, tgt_s, home_s) = lone_grind_world(44, 2, 100.0, Vec2::new(20.0, 0.0));
    let proj_s = w_small.project_forward(&sp, &wp, 600);
    let saved_small = proj_s.marginal_ticks_saved(0, tgt_s, home_s);

    let (w_big, tgt_b, home_b) = lone_grind_world(44, 20, 100.0, Vec2::new(20.0, 0.0));
    let proj_b = w_big.project_forward(&sp, &wp, 600);
    let saved_big = proj_b.marginal_ticks_saved(0, tgt_b, home_b);

    // Both non-negative by construction (saturating_sub) — assert the diminishing-returns shape.
    assert!(
        saved_small >= saved_big,
        "one more ship should save at least as many ticks when the force is small ({saved_small}) \
         as when it is large ({saved_big})"
    );
    // From a neutral position there is nothing to send: zero value.
    // (home is Player-owned here, so use an explicit neutral source: the target itself is neutral.)
    let from_neutral = proj_s.marginal_ticks_saved(0, tgt_s, tgt_s);
    assert_eq!(from_neutral, 0, "a neutral from_position contributes no marginal ship");
}

#[test]
fn expected_combat_square_law_sanity() {
    // The combat-model query: the larger side wins (other side wiped), and survivors grow with
    // the winning margin. Also the defender edge helps the defender.
    let sp = SimParams::default();
    let wp = WorldParams::default();
    let (w, _t, _h) = lone_grind_world(45, 1, 100.0, Vec2::new(10.0, 0.0));
    let proj = w.project_forward(&sp, &wp, 1);

    // 20 attackers vs 10 defenders, no edge: attacker wins decisively, defender wiped.
    let (a_surv, d_surv) = proj.expected_combat(20, 10, false);
    assert!(a_surv > 0 && d_surv == 0, "2:1 attacker should win and wipe the defender (got {a_surv},{d_surv})");
    // Square law: attacker keeps a healthy fraction (well more than the naive 20-10=10 linear).
    assert!(a_surv >= 15, "square-law: 20 vs 10 should leave many attackers, got {a_surv}");

    // Even fight, defender on its own sub: the defender edge tips it to the defender.
    let (a_e, d_e) = proj.expected_combat(10, 10, true);
    assert!(d_e >= a_e, "the on-sub defender edge should favour the defender in an even fight ({a_e} vs {d_e})");

    // No defenders: nobody dies, attacker fully survives.
    assert_eq!(proj.expected_combat(7, 0, false), (7, 0));
}

#[test]
fn force_for_efficiency_is_monotone_and_wins() {
    // force_for_efficiency: 0 when undefended; a winning force when defended; monotone
    // non-decreasing in the desired exchange ratio.
    let sp = SimParams::default();
    let wp = WorldParams::default();

    // Undefended neutral: no force needed.
    let (w0, tgt0, _h0) = lone_grind_world(46, 0, 50.0, Vec2::new(20.0, 0.0));
    let proj0 = w0.project_forward(&sp, &wp, 1);
    assert_eq!(proj0.force_for_efficiency(0, tgt0, 1.0), Some(0), "undefended => 0 ships");

    // A sub defended by its owner: build an Enemy-owned sub with a garrison, ask as the attacker.
    let mut st = Structure::new(47);
    let def = st.add_sub(SubStructure::new(Vec2::new(0.0, 0.0), 6.0, Faction::Enemy).with_max_resistance(100.0));
    for _ in 0..8 {
        st.spawn_ship(Faction::Enemy, def);
    }
    let mut w = World::new();
    w.add_planet(Planet::new(st, Vec2::new(0.0, 0.0), "D"));
    let proj = w.project_forward(&sp, &wp, 1);

    let f_win = proj.force_for_efficiency(0, def, 1.0).expect("some force wins at 1:1");
    let (a_surv, d_surv) = proj.expected_combat(f_win, 8, true);
    assert!(a_surv > 0 && d_surv == 0, "the returned force must actually win the firefight");

    // Monotone in desired_ratio: a harsher trade demands at least as many ships.
    let f1 = proj.force_for_efficiency(0, def, 1.0).unwrap();
    let f2 = proj.force_for_efficiency(0, def, 2.0).unwrap();
    let f3 = proj.force_for_efficiency(0, def, 4.0).unwrap();
    assert!(f1 <= f2 && f2 <= f3, "force must be monotone non-decreasing in desired_ratio: {f1},{f2},{f3}");
}

#[test]
fn property_reads_match_seed_state() {
    // The per-element property accessors must report the call-time sub state.
    let sp = SimParams::default();
    let wp = WorldParams::default();
    let (w, tgt, home) = lone_grind_world(48, 5, 30.0, Vec2::new(40.0, 0.0));
    let proj = w.project_forward(&sp, &wp, 1);
    assert_eq!(proj.current_owner(0, tgt), Faction::Neutral);
    assert_eq!(proj.current_owner(0, home), Faction::Player);
    assert_eq!(proj.sub_resistance(0, tgt), (30.0, 30.0));
    assert_eq!(proj.present_now(0, tgt), (5, 0), "5 idle Player ships seeded on the target");
    assert_eq!(proj.present_now(0, home), (0, 0));
    // OOB ids stay trivial.
    assert_eq!(proj.sub_resistance(9, 9), (0.0, 0.0));
    assert_eq!(proj.present_now(9, 9), (0, 0));
}

#[test]
fn marginal_queries_do_not_perturb_state_hash() {
    // The what-if queries re-integrate locally; they must not touch world state.
    let sp = SimParams::default();
    let wp = WorldParams::default();
    let (mut w, tgt, home) = lone_grind_world(49, 4, 60.0, Vec2::new(30.0, 0.0));
    // Add a lane-less second planet just to have some state, then step.
    w.add_planet(planet_1sub(50, Faction::Enemy, 6, 100.0, Vec2::new(500.0, 0.0), "X"));
    for _ in 0..5 {
        w.step(&sp, &wp);
    }
    let before = w.state_hash();
    let proj = w.project_forward(&sp, &wp, 240);
    let _ = proj.capture_eta_if(0, tgt, 10, 3, Faction::Player);
    let _ = proj.marginal_ticks_saved(0, tgt, home);
    let _ = proj.force_for_efficiency(0, tgt, 2.0);
    let _ = proj.expected_combat(30, 12, true);
    assert_eq!(before, w.state_hash(), "R3 queries must not perturb world state");
}

#[test]
fn combat_timeline_matches_static_combat_with_no_events() {
    // With an empty event list the timeline is just a fight to extinction — its (my, foe) LOSSES
    // must equal the static `expected_combat` survivors subtracted from the inputs.
    let sp = SimParams::default();
    let wp = WorldParams::default();
    let (w, _t, _h) = lone_grind_world(70, 1, 10.0, Vec2::new(30.0, 0.0));
    let proj = w.project_forward(&sp, &wp, 1);

    let (my0, foe0) = (20u32, 12u32);
    // In the timeline, MY side is the on-sub defender. Mirror that with `expected_combat` by making
    // the FOE the attacker and my side the on-sub defender: (atk_surv=foe_surv, def_surv=my_surv).
    let (foe_surv, my_surv) = proj.expected_combat(foe0, my0, true);
    let (my_loss, foe_loss) = proj.expected_combat_timeline(my0, foe0, true, &[]);
    // Allow a 1-ship rounding slack: the timeline floors *accumulated losses* while expected_combat
    // floors *survivors* first — they can differ by at most one on a fractional boundary (the same
    // off-by-one the fast/reference battery tolerates).
    let dm = (my_loss as i64 - (my0 - my_surv) as i64).abs();
    let df = (foe_loss as i64 - (foe0 - foe_surv) as i64).abs();
    assert!(dm <= 1, "no-event my-losses ~= static (timeline={my_loss}, static={})", my0 - my_surv);
    assert!(df <= 1, "no-event foe-losses ~= static (timeline={foe_loss}, static={})", foe0 - foe_surv);
}

#[test]
fn combat_timeline_reinforcement_improves_kill_efficiency() {
    // A mid-fight reinforcement (MyArrival) should let my side trade BETTER (kill more foe per my
    // loss) than the same fight without it — the signal L_defend's tier-2 reads.
    let sp = SimParams::default();
    let wp = WorldParams::default();
    let (w, _t, _h) = lone_grind_world(71, 1, 10.0, Vec2::new(30.0, 0.0));
    let proj = w.project_forward(&sp, &wp, 1);

    let eff = |events: &[(u64, CombatEvent)]| -> f64 {
        let (ml, fl) = proj.expected_combat_timeline(8, 14, true, events);
        fl as f64 / (ml.max(1)) as f64
    };
    let without = eff(&[]);
    let with_reinf = eff(&[(2, CombatEvent::MyArrival(10))]);
    assert!(
        with_reinf >= without,
        "a reinforcement must not WORSEN kill-efficiency (with={with_reinf}, without={without})"
    );
}

#[test]
fn combat_timeline_is_deterministic() {
    // Same inputs => bit-identical result (the query draws no RNG).
    let sp = SimParams::default();
    let wp = WorldParams::default();
    let (w, _t, _h) = lone_grind_world(72, 1, 10.0, Vec2::new(30.0, 0.0));
    let proj = w.project_forward(&sp, &wp, 1);
    let events = [(1u64, CombatEvent::FoeArrival(5)), (4, CombatEvent::MyArrival(6)), (6, CombatEvent::MyRetreat(3))];
    let a = proj.expected_combat_timeline(10, 9, true, &events);
    let b = proj.expected_combat_timeline(10, 9, true, &events);
    assert_eq!(a, b, "the timeline query must be deterministic");
}
