//! Validation of the SYNTHESIS phase (COUNTER_DESIGN §5–6).
//!
//! Three tiers:
//!
//! * **Backbone** — the RPS backbone is the right countering automaton for the inferred mix
//!   (colonize⇒Attack, attack⇒Defend, defend⇒Colonize), and an empty/agnostic profile issues
//!   nothing; `p_max == 0` is the pure robust generalist (never deviates).
//! * **Exploit gating** — a fired module only ships its candidate when the **projection** confirms
//!   it beats the backbone (an exploit whose forecast loses is dropped); the DBR confidence gates
//!   trust; everything is deterministic.
//! * **Wiring** — the [`CounterController`] accumulates the opposing seat's profile across a match
//!   (the observation hook) and re-derives the counter on the decision cadence, with no mid-match
//!   policy flip.

use layer1::{Faction, SimParams, Structure, SubStructure, Vec2};
use world::{Planet, World, WorldParams};

use crate::controller::{AiController, Roster};
use crate::counter::observe::{
    Choice, FractionBand, FrontierBand, GarrisonBand, MoveKind, ObservationLog, OwnershipBand,
    Sample, SituationBucket, ThreatBand,
};
use crate::counter::profile::OpponentProfile;
use crate::counter::synthesize::{synthesize, CounterController, Exploit};
use crate::harness::{corridor_world, diamond_world, run_counter_match, DEFAULT_DECISION_INTERVAL};
use crate::strategy::StrategicPolicy;

fn sim() -> SimParams {
    SimParams::default()
}
fn wparams() -> WorldParams {
    WorldParams::default()
}

// ======================================================================================
// Synthetic-log helpers (mirror counter::tests) — build a profile with a known dominant axis.
// ======================================================================================

fn bucket(o: OwnershipBand, g: GarrisonBand, t: ThreatBand, f: FrontierBand) -> SituationBucket {
    SituationBucket { ownership: o, garrison: g, threat: t, frontier: f }
}
fn sample(tick: u64, b: SituationBucket, kind: MoveKind, band: FractionBand) -> Sample {
    Sample { tick, bucket: b, choice: Choice { kind, band } }
}

/// A colonize-dominant profile (many Colonize choices) — should counter with **Attack**.
fn colonize_profile() -> OpponentProfile {
    let mut log = ObservationLog::new();
    let b = bucket(OwnershipBand::Even, GarrisonBand::Low, ThreatBand::Calm, FrontierBand::Front);
    for t in 0..30 {
        log.push(Faction::Player, sample(t, b, MoveKind::Colonize, FractionBand::Half));
    }
    OpponentProfile::infer(&log)
}

/// An attack-dominant profile — should counter with **Defend**.
fn attack_profile() -> OpponentProfile {
    let mut log = ObservationLog::new();
    let b = bucket(OwnershipBand::Even, GarrisonBand::Low, ThreatBand::Calm, FrontierBand::Front);
    for t in 0..30 {
        log.push(Faction::Player, sample(t, b, MoveKind::Strike, FractionBand::Heavy));
    }
    OpponentProfile::infer(&log)
}

/// A defend-dominant profile (all holds fold into defend) — should counter with **Colonize**.
fn defend_profile() -> OpponentProfile {
    let mut log = ObservationLog::new();
    let b = bucket(OwnershipBand::Even, GarrisonBand::Low, ThreatBand::Calm, FrontierBand::Rear);
    for t in 0..30 {
        log.push(Faction::Player, sample(t, b, MoveKind::Hold, FractionBand::None));
    }
    OpponentProfile::infer(&log)
}

// ======================================================================================
// (A) Backbone selection — the RPS counter to the inferred mix.
// ======================================================================================

#[test]
fn backbone_counters_colonize_with_attack() {
    let w = diamond_world(1);
    let p = colonize_profile();
    // p_max = 0: pure backbone, no exploits — isolate the backbone choice.
    let plan = synthesize(&p, &w, Faction::Ai(0), &sim(), &wparams(), 0.0);
    assert_eq!(plan.backbone, Some(StrategicPolicy::Attack), "infer-Colonize => backbone Attack");
    assert_eq!(plan.exploit, None, "p_max=0 never ships an exploit");
}

#[test]
fn backbone_counters_attack_with_defend() {
    let w = diamond_world(1);
    let p = attack_profile();
    let plan = synthesize(&p, &w, Faction::Ai(0), &sim(), &wparams(), 0.0);
    assert_eq!(plan.backbone, Some(StrategicPolicy::Defend), "infer-Attack => backbone Defend");
}

#[test]
fn backbone_counters_defend_with_colonize() {
    let w = diamond_world(1);
    let p = defend_profile();
    let plan = synthesize(&p, &w, Faction::Ai(0), &sim(), &wparams(), 0.0);
    assert_eq!(plan.backbone, Some(StrategicPolicy::Colonize), "infer-Defend => backbone Colonize");
}

#[test]
fn empty_profile_is_agnostic_no_orders() {
    let w = diamond_world(1);
    let p = OpponentProfile::infer(&ObservationLog::new());
    let plan = synthesize(&p, &w, Faction::Ai(0), &sim(), &wparams(), 1.0);
    assert_eq!(plan.backbone, None, "no read yet => no backbone");
    assert!(plan.fleet_orders.is_empty(), "agnostic Counter issues nothing");
    assert_eq!(plan.exploit, None);
    assert_eq!(plan.gift, 0.0);
}

#[test]
fn p_max_zero_is_the_pure_backbone_even_with_a_fired_seam() {
    // A colonize profile that ALSO fires the thin-rear seam (rear surplus shipped forward). At
    // p_max = 0 the synthesis must still ship ONLY the backbone — the playstyle dial at the robust
    // end deviates for nothing (COUNTER_DESIGN §2: p_max is character, the robust end is backbone).
    let mut log = ObservationLog::new();
    let rear = bucket(OwnershipBand::Ahead, GarrisonBand::Low, ThreatBand::Calm, FrontierBand::Rear);
    for t in 0..20 {
        log.push(Faction::Player, sample(t, rear, MoveKind::Colonize, FractionBand::Heavy));
    }
    let p = OpponentProfile::infer(&log);
    assert!(p.modules.never_guards_rear().fires, "the seam must fire for this test to be meaningful");

    let w = diamond_world(7);
    let plan = synthesize(&p, &w, Faction::Ai(0), &sim(), &wparams(), 0.0);
    assert_eq!(plan.exploit, None, "p_max=0 ships no exploit even when the seam fires");
}

// ======================================================================================
// (B) Exploit gating — the projection decides; a losing exploit is dropped.
// ======================================================================================

/// A small world where the Counter (Enemy) is boxed in with NO foe-bearing target reachable, so the
/// flank/counterpunch candidates produce no orders (or orders that cannot improve the forecast).
/// Either way the synthesis must fall back to the safe backbone — never ship an empty/losing exploit.
#[test]
fn exploit_dropped_when_it_produces_no_winning_orders() {
    // Build a profile that fires the seam (so a FlankRear candidate is *considered*), then run the
    // synthesis on a world where the Counter has nothing to flank — the candidate cannot beat the
    // backbone in the projection, so it is dropped and the backbone (Attack) drives the tick.
    let mut log = ObservationLog::new();
    let rear = bucket(OwnershipBand::Ahead, GarrisonBand::Low, ThreatBand::Calm, FrontierBand::Rear);
    for t in 0..20 {
        log.push(Faction::Player, sample(t, rear, MoveKind::Colonize, FractionBand::Heavy));
    }
    let p = OpponentProfile::infer(&log);
    assert!(p.modules.never_guards_rear().fires);

    // A lone Counter home with only a neutral neighbour — no enemy presence to flank at all.
    let mut w = World::new();
    let mut st = Structure::new(1);
    let h = st.add_sub(SubStructure::new(Vec2::new(0.0, 0.0), 4.0, Faction::Ai(0)));
    for _ in 0..40 {
        st.spawn_ship(Faction::Ai(0), h);
    }
    let home = w.add_planet(Planet::new(st, Vec2::new(0.0, 0.0), "C-home"));
    let mut nst = Structure::new(2);
    nst.add_sub(SubStructure::new(Vec2::new(0.0, 0.0), 4.0, Faction::Neutral));
    let nbr = w.add_planet(Planet::new(nst, Vec2::new(30.0, 0.0), "N"));
    w.add_lane(home, nbr, 30.0);

    let plan = synthesize(&p, &w, Faction::Ai(0), &sim(), &wparams(), 1.0);
    // The flank/counterpunch candidates have no foe to hit here, so no exploit can be confirmed.
    assert_eq!(plan.exploit, None, "no reachable rear to flank => exploit dropped, backbone drives");
    assert_eq!(plan.backbone, Some(StrategicPolicy::Attack), "infer-Colonize backbone is Attack");
}

/// The gift margin guards against a *wash*: when an exploit's forecast merely ties the backbone, the
/// safe backbone is kept. We assert the invariant directly: whenever an exploit IS shipped, its
/// recorded `gift` strictly cleared the margin (the safe-exploitation rule), across a sweep.
#[test]
fn a_shipped_exploit_always_cleared_the_gift_margin() {
    use crate::counter::synthesize::GIFT_MARGIN;
    for seed in [1u64, 7, 42] {
        for &build in &[corridor_world as fn(u64) -> World, diamond_world] {
            let w = build(seed);
            // A seam-firing colonize profile (the case most likely to find a real flank gift).
            let mut log = ObservationLog::new();
            let rear = bucket(OwnershipBand::Ahead, GarrisonBand::Low, ThreatBand::Calm, FrontierBand::Rear);
            for t in 0..24 {
                log.push(Faction::Player, sample(t, rear, MoveKind::Colonize, FractionBand::Heavy));
            }
            let p = OpponentProfile::infer(&log);
            for &seat in &[Faction::Player, Faction::Ai(0)] {
                let plan = synthesize(&p, &w, seat, &sim(), &wparams(), 1.0);
                if plan.exploit.is_some() {
                    assert!(
                        plan.gift >= GIFT_MARGIN,
                        "a shipped exploit must clear the gift margin: gift={} margin={}",
                        plan.gift,
                        GIFT_MARGIN
                    );
                }
            }
        }
    }
}

// ======================================================================================
// (C) Determinism — same inputs => identical plan.
// ======================================================================================

#[test]
fn synthesis_is_deterministic() {
    let w = diamond_world(42);
    let p = colonize_profile();
    let a = synthesize(&p, &w, Faction::Ai(0), &sim(), &wparams(), 0.7);
    let b = synthesize(&p, &w, Faction::Ai(0), &sim(), &wparams(), 0.7);
    assert_eq!(a, b, "synthesis is a deterministic function of (profile, world, seat, p_max)");
}

// ======================================================================================
// (D) Wiring — accumulate-then-counter via the CounterController + observation hook.
// ======================================================================================

#[test]
fn counter_controller_run_is_deterministic() {
    // Two identical Counter runs produce the same world fingerprint + the same accumulated profile.
    let params = sim();
    let wp = wparams();
    let run = || {
        let mut counter = CounterController::new(Faction::Ai(0), 0.6, params, wp);
        let opp = AiController::from_roster(Faction::Player, Roster::SimpleColonize);
        let mut w = corridor_world(7);
        run_counter_match(&mut w, &params, &wp, &mut counter, &opp, 400, DEFAULT_DECISION_INTERVAL);
        (w.state_hash(), counter.profile().total_samples, counter.profile().mix)
    };
    let a = run();
    let b = run();
    assert_eq!(a.0, b.0, "identical Counter runs must yield identical world hashes");
    assert_eq!(a.1, b.1, "identical sample counts");
    assert_eq!(a.2, b.2, "identical inferred mix");
}

#[test]
fn from_roster_builds_a_counter_only_for_counter_entries() {
    let params = sim();
    let wp = wparams();
    assert!(CounterController::from_roster(Faction::Ai(0), Roster::Colonize, params, wp).is_none());
    let c = CounterController::from_roster(Faction::Ai(0), Roster::Counter { p_max: 0.4 }, params, wp);
    assert!(c.is_some(), "a Counter roster entry builds a CounterController");
    assert_eq!(c.unwrap().p_max, 0.4);
}

#[test]
fn counter_roster_p_max_is_clamped() {
    let params = sim();
    let wp = wparams();
    let hi = CounterController::new(Faction::Player, 5.0, params, wp);
    assert_eq!(hi.p_max, 1.0, "p_max clamps to [0,1]");
    let lo = CounterController::new(Faction::Player, -1.0, params, wp);
    assert_eq!(lo.p_max, 0.0);
}

#[test]
fn exploit_names_are_present() {
    for e in [Exploit::FlankRear, Exploit::CounterPunch, Exploit::OutTempo] {
        assert!(!e.name().is_empty());
    }
}
