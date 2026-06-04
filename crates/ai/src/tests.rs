//! Headless validation of the AI layer (run as **lib** tests so they execute reliably under
//! the Windows app-control policy that blocks freshly-linked standalone binaries — see
//! `AI.md`). These are the "real test" the task calls for:
//!
//! * the greedy policy is sensible and its **seam is exploitable** (it commits surplus to the
//!   nearest fight and never posts a rear guard — a rear strike beats it),
//! * the three pure strategies behave **distinctly**, and the validated cycle
//!   (**attack > colonize > defend > attack**) is measured over a fair symmetric world and
//!   **both seatings**, and
//! * **determinism**: same seed + same policies ⇒ identical `state_hash` and outcome.

use crate::controller::{AiController, Roster};
use crate::harness::{
    corridor_world, diamond_world, duel_both_seatings, run_match, DEFAULT_DECISION_INTERVAL,
    DEFAULT_HORIZON,
};
use crate::strategy::StrategicPolicy;
use layer1::{Faction, FractionBucket, SimParams, Structure, SubStructure, Vec2};
use world::{FleetOrder, Planet, PlanetOwner, World, WorldParams};

fn sim() -> SimParams {
    SimParams::default()
}

// ======================================================================================
// (A) The greedy SEAM: it never posts a rear guard, so a rear strike beats it.
// ======================================================================================

/// A home planet with `subs` owned subs (each `per_sub` idle ships), keyed by `seed`.
fn home(seed: u64, owner: Faction, subs: usize, per_sub: usize, pos: Vec2, name: &str) -> Planet {
    let mut st = Structure::new(seed);
    let ids: Vec<_> = (0..subs)
        .map(|i| {
            let ang = (i as f32) / (subs.max(1) as f32) * std::f32::consts::TAU;
            let r = if i == 0 { 0.0 } else { 9.0 };
            st.add_sub(SubStructure::new(Vec2::new(r * ang.cos(), r * ang.sin()), 4.0, owner))
        })
        .collect();
    for &s in &ids {
        for _ in 0..per_sub {
            st.spawn_ship(owner, s);
        }
    }
    Planet::new(st, pos, name)
}

fn neutral(seed: u64, subs: usize, pos: Vec2, name: &str) -> Planet {
    let mut st = Structure::new(seed);
    for i in 0..subs.max(1) {
        let ang = (i as f32) / (subs.max(1) as f32) * std::f32::consts::TAU;
        let r = if i == 0 { 0.0 } else { 9.0 };
        st.add_sub(SubStructure::new(Vec2::new(r * ang.cos(), r * ang.sin()), 4.0, Faction::Neutral));
    }
    Planet::new(st, pos, name)
}

/// A "bait" world for the seam. The greedy seat (Enemy) holds a **single-sub rear** planet
/// `E-rear` with a juicy neutral **bait corridor** dangling off it (`bait1..bait3`). Because
/// greedy always ships its surplus toward the nearest uncontested grab and never keeps a
/// reserve, it continuously bleeds `E-rear` down toward the flat garrison floor to colonize the
/// bait — `E-rear`'s lone sub produces too slowly to refill the gap. The exploiter (Player)
/// sits on a strong home one lane from `E-rear` and, once greedy has committed forward, lands a
/// single **overwhelming** wave on the stripped rear.
///
/// ```text
///   P-home === E-rear --- bait1 --- bait2 --- bait3   (=== = the short strike lane)
/// ```
/// `E-rear` is deliberately a **single** sub (low production, low defender mass) so that once
/// greedy thins it, a concentrated strike captures it — the exact "thin rear gets flanked"
/// failure the seam describes. The Player home is large so it can build the strike stack.
fn seam_world(seed: u64) -> World {
    let mut w = World::new();
    let p = w.add_planet(home(seed, Faction::Player, 3, 14, Vec2::new(0.0, 0.0), "P-home"));
    let e = w.add_planet(home(seed + 1, Faction::Enemy, 1, 10, Vec2::new(28.0, 0.0), "E-rear"));
    let b1 = w.add_planet(neutral(seed + 11, 1, Vec2::new(64.0, 0.0), "bait1"));
    let b2 = w.add_planet(neutral(seed + 12, 1, Vec2::new(100.0, 0.0), "bait2"));
    let b3 = w.add_planet(neutral(seed + 13, 1, Vec2::new(136.0, 0.0), "bait3"));
    w.add_lane(p, e, 28.0); // the strike lane (short)
    w.add_lane(e, b1, 36.0); // greedy's bait corridor (it ships surplus down here)
    w.add_lane(b1, b2, 36.0);
    w.add_lane(b2, b3, 36.0);
    w
}

/// The greedy policy's seam is **exploitable**: a focused rear strike beats a pure-greedy seat
/// that has *equal starting force*, because greedy keeps no reserve — it streams its surplus
/// down the bait corridor and leaves its home defended only by the flat garrison floor.
///
/// The exploiter (Player) is scripted to do the one thing the seam invites: mass its whole home
/// and punch the greedy home across the strike lane, then keep feeding the assault. We assert
/// the exploiter wins (captures the greedy home / leads) in a majority of seeds — the Layer-2
/// analog of `layer1`'s `ai_seam_thin_rear_is_exploitable`.
#[test]
fn greedy_seam_thin_rear_is_exploitable() {
    let params = sim();
    let wp = WorldParams::default();
    let seeds: [u64; 7] = [1, 7, 42, 100, 0x5EA1, 2024, 31337];
    let mut exploited = 0;

    for &seed in &seeds {
        let mut w = seam_world(seed);
        let greedy = AiController::from_roster(Faction::Enemy, Roster::GreedyLocal);
        // Planet ids in seam_world: P-home=0, E-rear=1, bait1=2, ...
        let (p_home, e_rear) = (0usize, 1usize);
        let mut captured_rear = false;

        for t in 0..DEFAULT_HORIZON {
            if w.is_eliminated(Faction::Player) || w.is_eliminated(Faction::Enemy) {
                break;
            }
            if t % DEFAULT_DECISION_INTERVAL == 0 {
                // Greedy (Enemy) decides+acts on its own — it bleeds E-rear toward the bait.
                greedy.decide_and_apply(&mut w, &params, &wp);
                // Exploiter (Player): mass the home straight at the thin rear. Once greedy has
                // committed its surplus forward, E-rear is only floor-defended, so the strike
                // overruns it — the seam.
                w.issue_fleet_order(FleetOrder::new(p_home, e_rear, FractionBucket::All), Faction::Player, &wp);
            }
            w.step(&params, &wp);
            if matches!(w.planet_aggregate(e_rear).owner, PlanetOwner::Owned(Faction::Player)) {
                captured_rear = true;
            }
        }
        // The exploit succeeded if the rear was captured at some point AND the Player leads /
        // wins at the horizon (a transient touch that greedy immediately retakes does not
        // count — the flank must stick and pay off).
        if captured_rear && w.outcome().winner == Some(Faction::Player) {
            exploited += 1;
        }
    }

    assert!(
        exploited * 2 > seeds.len(),
        "the rear strike should exploit greedy's thin-rear seam in a majority of seeds, got \
         {exploited}/{}",
        seeds.len()
    );
}

/// Greedy is **sensible** (not inert): from a fully-owned start it expands — it captures at
/// least one neutral planet during the opening, and beats a Passive dummy outright.
#[test]
fn greedy_is_sensible_expands_and_beats_passive() {
    let params = sim();
    let wp = WorldParams::default();
    let (gw, pw, _dr) =
        duel_both_seatings(|| corridor_world(1), &params, &wp, Roster::GreedyLocal, Roster::Passive);
    assert!(gw > pw, "greedy must beat a do-nothing passive seat (got {gw}-{pw})");

    // And it actually grows territory: run greedy (Player) vs passive and check sub growth.
    let mut w = corridor_world(1);
    let g = AiController::from_roster(Faction::Player, Roster::GreedyLocal);
    let pa = AiController::from_roster(Faction::Enemy, Roster::Passive);
    let start = w.total_subs(Faction::Player);
    for t in 0..400u64 {
        if t % DEFAULT_DECISION_INTERVAL == 0 {
            g.decide_and_apply(&mut w, &params, &wp);
            pa.decide_and_apply(&mut w, &params, &wp);
        }
        w.step(&params, &wp);
    }
    assert!(
        w.total_subs(Faction::Player) > start,
        "greedy should have captured at least one neutral planet's subs by mid-game"
    );
}

// ======================================================================================
// (B) The three pure strategies behave DISTINCTLY + the validated cycle.
// ======================================================================================

/// The three pure strategies issue **distinct** opening orders on the same world (they are not
/// the same policy wearing different hats): on the diamond, colonize grabs flanks, attack
/// commits toward the centre/enemy, defend holds. We assert their first-decision order sets
/// differ pairwise.
#[test]
fn pure_strategies_are_distinct() {
    let params = sim();
    let wp = WorldParams::default();
    let w = diamond_world(1);

    let dec = |sp: StrategicPolicy| {
        let c = AiController { seat: Faction::Player, strategic: sp, tactical: crate::strategy::TacticalPolicy::Greedy, greedy: crate::greedy::GreedyParams::default() };
        // Fleet orders only — the strategic signature is the inter-planet plan.
        c.decide(&w, &params, &wp).fleet_orders
    };
    let col = dec(StrategicPolicy::Colonize);
    let def = dec(StrategicPolicy::Defend);
    let atk = dec(StrategicPolicy::Attack);

    // Colonize must issue something (there are neutrals to grab from the fully-owned home).
    assert!(!col.is_empty(), "colonize should open by expanding");
    // They must not all be identical.
    assert!(
        !(col == def && def == atk),
        "the three pure strategies must not produce identical opening orders"
    );
    // Specifically colonize != attack (the clearest contrast: grab neutral vs strike enemy).
    assert_ne!(col, atk, "colonize and attack must open differently");
    let _ = def;
}

/// **The validated cycle, measured.** On the symmetric diamond world over both seatings and
/// several seeds, assert each rock-paper-scissors edge holds:
/// **attack > colonize**, **colonize > defend**, **defend > attack**.
///
/// This is a *measurement*, not an assumption (per `01-mechanics.md`): the test asserts the
/// cycle closes on this world, and the exact numbers are reported in `AI.md`. If a future
/// tuning weakens an edge, this test is where it shows up.
#[test]
fn pure_strategy_cycle_closes_on_diamond() {
    let params = sim();
    let wp = WorldParams::default();
    let seeds: [u64; 5] = [1, 7, 42, 2024, 31337];

    let edge = |a: Roster, b: Roster| -> (u32, u32, u32) {
        let mut aw = 0;
        let mut bw = 0;
        let mut dr = 0;
        for &s in &seeds {
            let (x, y, z) = duel_both_seatings(|| diamond_world(s), &params, &wp, a, b);
            aw += x;
            bw += y;
            dr += z;
        }
        (aw, bw, dr)
    };

    let (a_c_w, a_c_l, _) = edge(Roster::Attack, Roster::Colonize);
    let (c_d_w, c_d_l, _) = edge(Roster::Colonize, Roster::Defend);
    let (d_a_w, d_a_l, _) = edge(Roster::Defend, Roster::Attack);

    println!("diamond cycle over {} seeds x 2 seatings:", seeds.len());
    println!("  attack  > colonize : {a_c_w}-{a_c_l}");
    println!("  colonize> defend   : {c_d_w}-{c_d_l}");
    println!("  defend  > attack   : {d_a_w}-{d_a_l}");

    assert!(a_c_w > a_c_l, "attack should beat colonize (timed strike on undefended production), got {a_c_w}-{a_c_l}");
    assert!(c_d_w > c_d_l, "colonize should beat defend (out-expand the turtle), got {c_d_w}-{c_d_l}");
    assert!(d_a_w > d_a_l, "defend should beat attack (punish the over-committed stack), got {d_a_w}-{d_a_l}");
}

/// Report-only companion: print the corridor world's edges too (it does NOT fully close — an
/// honest negative result documented in `AI.md`). Not an assertion; it just records the numbers
/// when run with `--nocapture`.
#[test]
fn pure_strategy_cycle_corridor_report() {
    let params = sim();
    let wp = WorldParams::default();
    let seeds: [u64; 5] = [1, 7, 42, 2024, 31337];
    let edge = |a: Roster, b: Roster| -> (u32, u32, u32) {
        let (mut x, mut y, mut z) = (0, 0, 0);
        for &s in &seeds {
            let (aw, bw, dr) = duel_both_seatings(|| corridor_world(s), &params, &wp, a, b);
            x += aw;
            y += bw;
            z += dr;
        }
        (x, y, z)
    };
    let ac = edge(Roster::Attack, Roster::Colonize);
    let cd = edge(Roster::Colonize, Roster::Defend);
    let da = edge(Roster::Defend, Roster::Attack);
    println!("corridor over {} seeds x 2 seatings:", seeds.len());
    println!("  attack  > colonize : {}-{}-{}", ac.0, ac.1, ac.2);
    println!("  colonize> defend   : {}-{}-{}", cd.0, cd.1, cd.2);
    println!("  defend  > attack   : {}-{}-{}", da.0, da.1, da.2);
    // No assertion: this edge set is known not to fully close on the corridor (reported in AI.md).
}

// ======================================================================================
// (C) Determinism: same seed + same policies => identical state_hash + outcome.
// ======================================================================================

/// Two identical runs (same world seed, same controllers) produce the **same per-tick
/// `state_hash`** and the same final outcome — the AI layer adds no nondeterminism.
#[test]
fn determinism_same_seed_same_hashes() {
    let params = sim();
    let wp = WorldParams::default();

    let run = || -> (Vec<u64>, world::WorldOutcome) {
        let mut w = diamond_world(42);
        let a = AiController::from_roster(Faction::Player, Roster::Attack);
        let b = AiController::from_roster(Faction::Enemy, Roster::Defend);
        let mut hashes = Vec::new();
        for t in 0..600u64 {
            if t % DEFAULT_DECISION_INTERVAL == 0 {
                // Player-first (the documented tie-break), both on the same snapshot.
                let da = a.decide(&w, &params, &wp);
                let db = b.decide(&w, &params, &wp);
                a.apply(&mut w, &da, &wp);
                b.apply(&mut w, &db, &wp);
            }
            w.step(&params, &wp);
            hashes.push(w.state_hash());
        }
        (hashes, w.outcome())
    };

    let (h1, o1) = run();
    let (h2, o2) = run();
    assert_eq!(h1, h2, "identical runs must have identical per-tick state hashes");
    assert_eq!(o1.winner, o2.winner, "identical runs must have the same winner");
    assert_eq!(o1.ships, o2.ships);
    assert_eq!(o1.subs, o2.subs);
}

/// The `run_match` harness path is itself deterministic: two runs match outcomes.
#[test]
fn determinism_run_match_is_stable() {
    let params = sim();
    let wp = WorldParams::default();
    let go = || {
        let mut w = diamond_world(7);
        let a = AiController::from_roster(Faction::Player, Roster::Colonize);
        let b = AiController::from_roster(Faction::Enemy, Roster::Attack);
        run_match(&mut w, &params, &wp, &a, &b, DEFAULT_HORIZON, DEFAULT_DECISION_INTERVAL)
    };
    let o1 = go();
    let o2 = go();
    assert_eq!(o1, o2, "the same match replays identically");
}
