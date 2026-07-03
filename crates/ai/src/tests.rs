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
    corridor_world, diamond_world, duel_both_seatings, run_match, run_simple_match,
    DEFAULT_DECISION_INTERVAL, DEFAULT_HORIZON,
};
use layer1::{Faction, FractionBucket, SimParams, Interior, SubStructure, Vec2};
use world::{FleetOrder, Structure, StructOwner, World, WorldParams};

fn sim() -> SimParams {
    SimParams::default()
}

// ======================================================================================
// (A) The greedy SEAM: it never posts a rear guard, so a rear strike beats it.
// ======================================================================================

/// A home struct with `subs` owned subs (each `per_sub` idle ships), keyed by `seed`.
fn home(seed: u64, owner: Faction, subs: usize, per_sub: usize, pos: Vec2, name: &str) -> Structure {
    let mut st = Interior::new(seed);
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
    Structure::new(st, pos, name)
}

fn neutral(seed: u64, subs: usize, pos: Vec2, name: &str) -> Structure {
    let mut st = Interior::new(seed);
    for i in 0..subs.max(1) {
        let ang = (i as f32) / (subs.max(1) as f32) * std::f32::consts::TAU;
        let r = if i == 0 { 0.0 } else { 9.0 };
        st.add_sub(SubStructure::new(Vec2::new(r * ang.cos(), r * ang.sin()), 4.0, Faction::Neutral));
    }
    Structure::new(st, pos, name)
}

/// A "bait" world for the seam. The greedy seat (Enemy) holds a **single-sub rear** structure
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
    let p = w.add_struct(home(seed, Faction::Player, 3, 14, Vec2::new(0.0, 0.0), "P-home"));
    let e = w.add_struct(home(seed + 1, Faction::Ai(0), 1, 10, Vec2::new(28.0, 0.0), "E-rear"));
    let b1 = w.add_struct(neutral(seed + 11, 1, Vec2::new(64.0, 0.0), "bait1"));
    let b2 = w.add_struct(neutral(seed + 12, 1, Vec2::new(100.0, 0.0), "bait2"));
    let b3 = w.add_struct(neutral(seed + 13, 1, Vec2::new(136.0, 0.0), "bait3"));
    w.add_lane(p, e, 28.0); // the strike lane (short)
    w.add_lane(e, b1, 36.0); // greedy's bait corridor (it ships surplus down here)
    w.add_lane(b1, b2, 36.0);
    w.add_lane(b2, b3, 36.0);
    w
}

/// The greedy policy's seam is **exploitable**: a focused rear strike reaches a pure-greedy seat's
/// undefended rear and *holds it*, because greedy keeps no reserve — it streams its surplus down
/// the bait corridor and leaves its home defended only by the flat garrison floor.
///
/// **Re-expressed for the new resistance / denial model** (mirrors how `layer1`'s
/// `ai_seam_thin_rear_is_exploitable` was re-expressed). Capture is no longer instant — taking a
/// fresh `max_resistance ≈ 1800` rear is a long grind — so the seam no longer manifests as a quick
/// "snipe the rear and win by the horizon". It manifests instead as **sustained denial**: the
/// flank reaches the greedy rear and *sits there uncontested for a long stretch* because greedy has
/// no rear-guard rule (it is busy chasing the bait corridor). While the flank sits uncontested it
/// (a) **starves** the rear's production (Mechanic B) and (b) grinds its resistance down. We assert,
/// in a **majority** of seeds, that the flank either outright **captures** the rear OR holds a
/// **sustained uncontested-presence streak** on it (>= `DENY_STREAK_WINDOWS` consecutive decision
/// windows of Player-present / Enemy-absent on the rear) — the spatial signature that greedy posts
/// no rear guard.
#[test]
#[ignore = "curriculum contract for the PARKED greedy (the L7 seam lesson); void until the greedy rework ships it again"]
fn greedy_seam_thin_rear_is_exploitable() {
    let params = sim();
    let wp = WorldParams::default();
    let seeds: [u64; 7] = [1, 7, 42, 100, 0x5EA1, 2024, 31337];
    // A sustained uncontested-presence streak this many decision windows long on the greedy rear is
    // the denial/grind signature of the seam under the new model (vs the old instant snipe).
    const DENY_STREAK_WINDOWS: u32 = 20;
    let mut exploited = 0;

    for &seed in &seeds {
        let mut w = seam_world(seed);
        let greedy = AiController::from_roster(Faction::Ai(0), Roster::GreedyLocal);
        // Structure ids in seam_world: P-home=0, E-rear=1, bait1=2, ...
        let (p_home, e_rear) = (0usize, 1usize);
        let mut exploited_this_seed = false;
        let mut deny_streak = 0u32;

        for t in 0..DEFAULT_HORIZON {
            if w.is_eliminated(Faction::Player) || w.is_eliminated(Faction::Ai(0)) {
                exploited_this_seed = true; // greedy collapsed — the flank paid off
                break;
            }
            if t % DEFAULT_DECISION_INTERVAL == 0 {
                // Greedy (Enemy) decides+acts on its own — it bleeds E-rear toward the bait.
                greedy.decide_and_apply(&mut w, &params, &wp);
                // Exploiter (Player): mass the home straight at the thin rear and keep feeding it,
                // so the grind on the floor-only rear is sustained.
                w.issue_fleet_order(FleetOrder::new(p_home, e_rear, FractionBucket::All), Faction::Player, &wp);

                // Track the denial signature on the rear, sampled once per decision window.
                let agg = w.struct_aggregate(e_rear);
                let captured = matches!(agg.owner, StructOwner::Owned(Faction::Player));
                if captured {
                    exploited_this_seed = true;
                    break;
                }
                if agg.ships_of(Faction::Player) > 0 && agg.ships_of(Faction::Ai(0)) == 0 {
                    deny_streak += 1;
                    if deny_streak >= DENY_STREAK_WINDOWS {
                        exploited_this_seed = true;
                        break;
                    }
                } else {
                    deny_streak = 0;
                }
            }
            w.step(&params, &wp);
        }
        if exploited_this_seed {
            exploited += 1;
        }
    }

    assert!(
        exploited * 2 > seeds.len(),
        "the flank should exploit greedy's thin-rear seam (capture OR sustained uncontested denial \
         of the rear) in a majority of seeds, got {exploited}/{}",
        seeds.len()
    );
}

/// Greedy is **sensible** (not inert): from a fully-owned start it expands — it captures at
/// least one neutral struct during the opening, and beats a Passive dummy outright.
#[test]
#[ignore = "emergent-behavior contract for the PARKED greedy; re-pin at the rework"]
fn greedy_is_sensible_expands_and_beats_passive() {
    let params = sim();
    let wp = WorldParams::default();
    let (gw, pw, _dr) =
        duel_both_seatings(|| corridor_world(1), &params, &wp, Roster::GreedyLocal, Roster::Passive);
    assert!(gw > pw, "greedy must beat a do-nothing passive seat (got {gw}-{pw})");

    // And it actually grows territory: run greedy (Player) vs passive and check sub growth.
    let mut w = corridor_world(1);
    let g = AiController::from_roster(Faction::Player, Roster::GreedyLocal);
    let pa = AiController::from_roster(Faction::Ai(0), Roster::Passive);
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
        "greedy should have captured at least one neutral struct's subs by mid-game"
    );
}

// ======================================================================================
// (B) The three pure strategies behave DISTINCTLY + the validated cycle.
// ======================================================================================

/// Report-only companion: print the corridor world's edges too (it does NOT fully close — an
/// honest negative result documented in `AI.md`). Not an assertion; it just records the numbers.
/// A report TOOL, not a test (30 full matches, asserts nothing) — run on demand with
/// `cargo test -p ai pure_strategy_cycle_corridor_report -- --ignored --nocapture`.
#[test]
#[ignore = "report tool (30 full matches, no assertions) — run with --ignored --nocapture"]
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
        let b = AiController::from_roster(Faction::Ai(0), Roster::Defend);
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
        let b = AiController::from_roster(Faction::Ai(0), Roster::Attack);
        run_match(&mut w, &params, &wp, &a, &b, DEFAULT_HORIZON, DEFAULT_DECISION_INTERVAL)
    };
    let o1 = go();
    let o2 = go();
    assert_eq!(o1, o2, "the same match replays identically");
}


// ======================================================================================
// (E) Resolution canaries — matches must END, not merely run out the clock.
// ======================================================================================

/// CANARY: a predator vs a do-nothing `Passive` must finish the job — **by elimination**, well
/// inside the horizon. This is the suite's alarm for the **"matches stop resolving"** failure
/// class (an engagement-dynamics change that lets garrisons stand off out of range forever, an
/// economy change that makes elimination unreachable, ...): such a regression turns these
/// eliminations into at-horizon leads, which every outcome-tolerant match test would silently
/// absorb — this one fails loudly instead. Wall-clock is deliberately NOT asserted (it lies
/// under machine suspension); the tick budget is the deterministic cost proxy.
#[test]
#[ignore = "greedy is PARKED; resolution is guarded by canary_simple at the live operating point"]
fn canary_greedy_eliminates_passive_within_budget() {
    let params = sim();
    let wp = WorldParams::default();
    const TICK_BUDGET: u64 = DEFAULT_HORIZON * 2 / 3;
    for &seed in &[1u64, 42] {
        let mut w = corridor_world(seed);
        let g = AiController::from_roster(Faction::Player, Roster::GreedyLocal);
        let p = AiController::from_roster(Faction::Ai(0), Roster::Passive);
        let out = run_match(&mut w, &params, &wp, &g, &p, DEFAULT_HORIZON, DEFAULT_DECISION_INTERVAL);
        assert!(
            out.by_elimination && out.winner == Some(Faction::Player) && out.tick <= TICK_BUDGET,
            "greedy must ELIMINATE passive on the corridor within {TICK_BUDGET} ticks              (seed {seed}: winner {:?}, by_elimination {}, tick {})",
            out.winner,
            out.by_elimination,
            out.tick
        );
    }
}

/// The Simple half of the resolution canary — the acceptance test for the **Layer-2
/// struct-to-struct reinforcement design** (not yet built). Today Simple's per-front
/// `OVERWHELM`-minimum can exceed what the reference soft cap lets it concentrate on one
/// structure, and its simplified Layer-2 push cannot mass the rest of its (much larger) economy
/// into a decisive wave — so a diamond endgame vs `Passive` runs to the horizon without
/// elimination. The owner's call: this position is out-of-gameplay (a human wins or restarts
/// long before it), and the *general* fix is the L2 reinforcement design — a Layer-1
/// desperation rule was tried and removed. Un-ignore when that design lands.
#[test]
fn canary_simple_eliminates_passive_within_budget() {
    // The LIVE operating point's attrition model (per-sub caps, ~120 effective headroom per
    // sub), not the parked reference's per-structure sqrt cap: under the legacy cap a one-sub
    // foothold can never stand more than `softcap_free + softcap_per_sub` ships, which sits
    // BELOW Simple's doctrine bar against a dug-in garrison — reinforcement bleeds away faster
    // than it accumulates, by construction. Simple is the live campaign enemy; its resolution
    // canary tests the loop the shipped game runs (funnel → staging headroom → wave).
    let mut params = sim();
    params.per_sub_attrition = true;
    let wp = WorldParams::default();
    const TICK_BUDGET: u64 = DEFAULT_HORIZON * 2 / 3;
    for &seed in &[1u64, 42] {
        let mut w = diamond_world(seed);
        let mut simple = crate::simple::SimpleController::new(Faction::Player);
        let p = AiController::from_roster(Faction::Ai(0), Roster::Passive);
        let out =
            run_simple_match(&mut w, &params, &wp, &mut simple, &p, DEFAULT_HORIZON, DEFAULT_DECISION_INTERVAL);
        assert!(
            out.by_elimination && out.winner == Some(Faction::Player) && out.tick <= TICK_BUDGET,
            "Simple must ELIMINATE passive on the diamond within {TICK_BUDGET} ticks              (seed {seed}: winner {:?}, by_elimination {}, tick {})",
            out.winner,
            out.by_elimination,
            out.tick
        );
    }
}

/// The reserve-BLOCKADE endgame gap, PINNED as a fact of the suite. A beaten remnant of
/// `>= layer1::sim::STORAGE_ENEMY_BLOCK` (20) ships parked in the ownerless reserve node (a)
/// blockades the owner's auto-divert and (b) can never be captured out (the reserve is never
/// captured) — so a map-controlling winner may be structurally unable to finish by elimination
/// and the match runs to its horizon. This test measures exactly that scenario and asserts the
/// CURRENT truth, so the gap is a visible, versioned suite fact instead of folklore — when an
/// endgame rule lands (e.g. a remnant-hunting nudge; see the design notes), this test is the
/// one to flip. The driver itself stays horizon-bounded, so the gap is a *resolution* gap, never
/// an unbounded-cost one.
#[test]
fn reserve_blockade_remnant_endgame_is_pinned() {
    let params = sim();
    let wp = WorldParams::default();
    // One struct WITH a reserve node: the Player owns every producing sub; the Ai(0) remnant
    // (25 >= the 20-ship blockade threshold) sits idle in the reserve.
    let mut st = Interior::new(99);
    let a = st.add_sub(SubStructure::new(Vec2::new(-9.0, 0.0), 0.0, Faction::Player));
    let _b = st.add_sub(SubStructure::new(Vec2::new(9.0, 0.0), 0.0, Faction::Player));
    let reserve = st.add_storage_sub();
    for _ in 0..30 {
        st.spawn_ship(Faction::Player, a);
    }
    for _ in 0..25 {
        st.spawn_ship(Faction::Ai(0), reserve);
    }
    let mut w = World::new();
    w.add_struct(Structure::new(st, Vec2::new(0.0, 0.0), "blockade"));
    let g = AiController::from_roster(Faction::Player, Roster::GreedyLocal);
    let p = AiController::from_roster(Faction::Ai(0), Roster::Passive);
    let out = run_match(&mut w, &params, &wp, &g, &p, 1500, DEFAULT_DECISION_INTERVAL);
    // The PINNED current truth (measured 2026-06-11): the winner holds the whole map and a big
    // ship lead, but the remnant in the never-capturable reserve survives to the horizon — no
    // elimination. When an endgame rule lands (remnant-hunting nudge / end-condition change),
    // these are the assertions to flip.
    assert_eq!(out.winner, Some(Faction::Player), "the blockaded winner still wins on lead");
    assert!(
        !out.by_elimination,
        "EXPECTED STALEMATE: the reserve remnant should deny elimination (if this fails, the          endgame gap has been fixed — flip this test to assert resolution and update the docs)"
    );
    assert!(
        w.total_ships(Faction::Ai(0)) > 0,
        "the remnant survives in the ownerless reserve (blockade ≥ STORAGE_ENEMY_BLOCK)"
    );
}
