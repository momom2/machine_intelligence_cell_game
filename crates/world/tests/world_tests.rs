//! Integration tests for the Layer-2 `world` lens over multiple Layer-1 structs.
//!
//! These pin the load-bearing properties from the task spec:
//!   (i)   **multi-struct step** — every struct's Layer-1 sim advances under one `World::step`;
//!   (ii)  **inter-struct fleet** — a fleet launched from struct A arrives at struct B and the
//!         injected ships capture a neutral sub-structure there (the headline behaviour);
//!   (iii) **StructAggregate** correctness for neutral / owned / contested structs and the
//!         `fully_owned_uncontested` (exportable) flag;
//!   (iv)  **determinism** via `state_hash` — two identical runs match at every tick; an extra
//!         order diverges;
//!   (v)   a **2-struct AI-free smoke** that runs to a horizon without panicking.

use layer1::{Faction, FractionBucket, SimParams, Interior, SubStructure, Vec2};
use world::{FleetOrder, Structure, StructOwner, World, WorldParams};

/// Lower the `max_resistance` (and refill) of a single sub on struct `p` so capture-pipeline
/// tests grind through a flip quickly. Under the new model fresh resistance is `1800` (~100
/// production periods); these tests exercise the *world fleet pipeline* (launch → transit →
/// inject → capture), not the grind speed (which the `layer1` tests cover), so we make the
/// target a cheap foothold to keep the loop horizons short.
fn soften_sub(w: &mut World, p: world::StructId, sub: usize, max: f32) {
    let s = &mut w.structs[p].interior.subs[sub];
    let m = max.max(1.0);
    s.max_resistance = m;
    s.resistance = m;
}

// ---------------------------------------------------------------------------
// Builders
// ---------------------------------------------------------------------------

/// A single-sub struct owned by `owner` with `garrison` starting ships of `owner` (0 ships and
/// `Neutral` owner ⇒ an empty up-for-grabs structure). The sub sits at the structure's local
/// origin (so the struct's `local_radius` is just the sub radius).
fn one_sub_struct(seed: u64, owner: Faction, garrison: usize, map_pos: Vec2, name: &str) -> Structure {
    let mut st = Interior::new(seed);
    let s = st.add_sub(SubStructure::new(Vec2::new(0.0, 0.0), 5.0, owner));
    for _ in 0..garrison {
        // Ships can only garrison at a sub; spawn the owner's garrison there. (For a Neutral
        // owner we never pass garrison > 0.)
        st.spawn_ship(owner, s);
    }
    Structure::new(st, map_pos, name)
}

/// A struct with two subs: a `home` owned by `owner` (well garrisoned) and a separate
/// `neutral` sub, far enough apart in LOCAL space that the home garrison does not immediately
/// sit inside the neutral. Returns the structure. (Used to test exportable/aggregate logic.)
fn home_plus_neutral_struct(seed: u64, owner: Faction, garrison: usize, map_pos: Vec2, name: &str) -> Structure {
    let mut st = Interior::new(seed);
    let home = st.add_sub(SubStructure::new(Vec2::new(0.0, 0.0), 5.0, owner));
    let _neutral = st.add_sub(SubStructure::new(Vec2::new(40.0, 0.0), 5.0, Faction::Neutral));
    for _ in 0..garrison {
        st.spawn_ship(owner, home);
    }
    Structure::new(st, map_pos, name)
}

// ===========================================================================
// (i) Multi-struct step
// ===========================================================================

/// `World::step` advances every struct's own Layer-1 sim (each struct's `tick` moves in
/// lock-step with the world tick) and does not panic with zero fleets.
#[test]
fn multi_struct_step_advances_all_structs() {
    let params = SimParams::default();
    let wp = WorldParams::default();
    let mut w = World::new();
    let a = w.add_struct(one_sub_struct(1, Faction::Player, 6, Vec2::new(0.0, 0.0), "A"));
    let b = w.add_struct(one_sub_struct(2, Faction::Ai(0), 6, Vec2::new(100.0, 0.0), "B"));
    let c = w.add_struct(one_sub_struct(3, Faction::Neutral, 0, Vec2::new(50.0, 50.0), "C"));

    for _ in 0..25 {
        w.step(&params, &wp);
    }
    assert_eq!(w.tick, 25);
    assert_eq!(w.structs[a].interior.tick, 25, "struct A's own sim advanced");
    assert_eq!(w.structs[b].interior.tick, 25, "struct B's own sim advanced");
    assert_eq!(w.structs[c].interior.tick, 25, "struct C's own sim advanced");

    // Owned structs produced ships over 25 ticks (production_period 18 ⇒ at least one spawn);
    // the neutral struct produced nothing.
    assert!(w.structs[a].interior.ship_count(Faction::Player) >= 6);
    assert!(w.structs[b].interior.ship_count(Faction::Ai(0)) >= 6);
    assert_eq!(w.structs[c].interior.ship_count(Faction::Player), 0);
    assert_eq!(w.structs[c].interior.ship_count(Faction::Ai(0)), 0);
}

// ===========================================================================
// (ii) Inter-struct fleet: launch A -> B, arrive, inject, capture a neutral
// ===========================================================================

/// A fleet launched from struct A (Player) along the A–B lane arrives at struct B (a lone
/// neutral sub) and the injected ships capture the neutral sub there — proving the full
/// pipeline: pull idle ships off A, undock + transit, inject idle at B, Layer-1 capture.
#[test]
fn fleet_arrives_and_captures_neutral_on_destination() {
    let params = SimParams::default();
    let wp = WorldParams::default();
    let mut w = World::new();
    let a = w.add_struct(one_sub_struct(10, Faction::Player, 12, Vec2::new(0.0, 0.0), "A"));
    let b = w.add_struct(one_sub_struct(11, Faction::Neutral, 0, Vec2::new(20.0, 0.0), "B"));
    w.add_lane(a, b, 20.0).expect("lane A-B");
    // Make B's sub a cheap foothold so the injected wave grinds the flip within the loop.
    soften_sub(&mut w, b, 0, 24.0);

    assert_eq!(w.structs[b].interior.subs[0].owner, Faction::Neutral);
    let a_before = w.structs[a].interior.ship_count(Faction::Player);

    // Launch half of A's idle garrison toward B.
    let launched = w.issue_fleet_order(FleetOrder::new(a, b, FractionBucket::Half), Faction::Player, &wp);
    assert!(launched > 0, "should launch at least one ship");
    assert_eq!(w.fleets.len(), 1, "one fleet in transit");

    // Those ships left A immediately (conserved in the fleet, not on A any more).
    let a_after_launch = w.structs[a].interior.ship_count(Faction::Player);
    assert_eq!(a_after_launch, a_before - launched as usize, "launched ships left struct A");

    // While undocking + transiting, B sees them only as 'incoming', not garrisoned.
    w.step(&params, &wp); // 1 undock tick consumed
    let agg_mid = w.struct_aggregate(b);
    assert_eq!(agg_mid.player_incoming, launched, "fleet counts as incoming at B");
    assert_eq!(agg_mid.player_ships, 0, "not landed yet");

    // Step until the fleet lands and captures B's neutral sub.
    let mut captured_tick = None;
    for _ in 0..80 {
        w.step(&params, &wp);
        if w.structs[b].interior.subs[0].owner == Faction::Player {
            captured_tick = Some(w.tick);
            break;
        }
    }
    assert!(captured_tick.is_some(), "the injected fleet should capture B's neutral sub");
    assert!(w.fleets.is_empty(), "fleet should have been consumed on arrival");
    // The landed ships are now garrisoned on B.
    assert!(w.structs[b].interior.ship_count(Faction::Player) > 0, "ships landed at B");
    // Ship conservation across the world: total Player ships never exceeded the start + B's
    // own production (B produced nothing until capture, so total == a_before until capture tick).
    assert!(w.total_ships(Faction::Player) >= launched as usize);
}

/// A fleet to a struct where the faction has NO foothold lands at the destination sub nearest
/// the perimeter facing the source (the beachhead rule), and the ships are really there.
#[test]
fn fleet_injects_at_beachhead_when_no_owned_sub() {
    let params = SimParams::default();
    let wp = WorldParams::default();
    let mut w = World::new();
    // Destination B has two neutral subs at distinct local positions; A sits to B's left on the
    // map, so the lane enters from -x and the beachhead should be the sub nearer -x locally.
    let a = w.add_struct(one_sub_struct(20, Faction::Player, 10, Vec2::new(-50.0, 0.0), "A"));
    let mut bst = Interior::new(21);
    let left = bst.add_sub(SubStructure::new(Vec2::new(-30.0, 0.0), 5.0, Faction::Neutral));
    let _right = bst.add_sub(SubStructure::new(Vec2::new(30.0, 0.0), 5.0, Faction::Neutral));
    let b = w.add_struct(Structure::new(bst, Vec2::new(50.0, 0.0), "B"));
    w.add_lane(a, b, 15.0).expect("lane");
    // Cheap foothold on the beachhead sub so the landed wave grinds the flip within the loop.
    soften_sub(&mut w, b, left, 24.0);

    w.issue_fleet_order(FleetOrder::new(a, b, FractionBucket::All), Faction::Player, &wp);
    for _ in 0..60 {
        w.step(&params, &wp);
        if w.structs[b].interior.ship_count(Faction::Player) > 0 {
            break;
        }
    }
    // Ships landed, and the LEFT sub (facing the source) is the one captured/contested first.
    assert!(w.structs[b].interior.ship_count(Faction::Player) > 0, "beachhead ships present");
    // Step a little more to let capture settle, then the left sub should be Player's.
    for _ in 0..20 {
        w.step(&params, &wp);
    }
    assert_eq!(
        w.structs[b].interior.subs[left].owner,
        Faction::Player,
        "beachhead should land at and capture the sub facing the source lane"
    );
}

// ===========================================================================
// Order validity (junk orders are safe no-ops)
// ===========================================================================

#[test]
fn unconnected_and_junk_orders_are_noops() {
    let wp = WorldParams::default();
    let mut w = World::new();
    let a = w.add_struct(one_sub_struct(30, Faction::Player, 8, Vec2::new(0.0, 0.0), "A"));
    let b = w.add_struct(one_sub_struct(31, Faction::Player, 8, Vec2::new(50.0, 0.0), "B"));
    // No lane between A and B yet.
    assert_eq!(
        w.issue_fleet_order(FleetOrder::new(a, b, FractionBucket::All), Faction::Player, &wp),
        0,
        "no lane ⇒ no-op"
    );
    // Same structure.
    assert_eq!(w.issue_fleet_order(FleetOrder::new(a, a, FractionBucket::All), Faction::Player, &wp), 0);
    // Out-of-range destination.
    assert_eq!(w.issue_fleet_order(FleetOrder::new(a, 999, FractionBucket::All), Faction::Player, &wp), 0);
    // Neutral can never issue.
    w.add_lane(a, b, 10.0);
    assert_eq!(w.issue_fleet_order(FleetOrder::new(a, b, FractionBucket::All), Faction::Neutral, &wp), 0);
    // Enemy has no idle ships on A ⇒ no-op even though the lane exists.
    assert_eq!(w.issue_fleet_order(FleetOrder::new(a, b, FractionBucket::All), Faction::Ai(0), &wp), 0);
    assert!(w.fleets.is_empty(), "no junk order created a fleet");
}

// ===========================================================================
// (iii) StructAggregate: neutral / owned / contested + exportable
// ===========================================================================

#[test]
fn aggregate_neutral_struct() {
    let mut w = World::new();
    let n = w.add_struct(one_sub_struct(40, Faction::Neutral, 0, Vec2::new(0.0, 0.0), "N"));
    let agg = w.struct_aggregate(n);
    assert_eq!(agg.owner, StructOwner::Neutral);
    assert_eq!(agg.player_ships, 0);
    assert_eq!(agg.enemy_ships, 0);
    assert_eq!(agg.neutral_subs, 1);
    assert!(!agg.fully_owned_uncontested(Faction::Player));
    assert!(!agg.fully_owned_uncontested(Faction::Ai(0)));
}

#[test]
fn aggregate_owned_struct_and_exportable() {
    let mut w = World::new();
    // A struct where Player owns the only sub and garrisons it, no enemy anywhere.
    let p = w.add_struct(one_sub_struct(41, Faction::Player, 5, Vec2::new(0.0, 0.0), "P"));
    let agg = w.struct_aggregate(p);
    assert_eq!(agg.owner, StructOwner::Owned(Faction::Player));
    assert_eq!(agg.player_subs, 1);
    assert_eq!(agg.enemy_subs, 0);
    assert_eq!(agg.neutral_subs, 0);
    assert_eq!(agg.player_ships, 5);
    assert!(agg.fully_owned_uncontested(Faction::Player), "all subs owned, no enemy ⇒ exportable");
    assert!(!agg.fully_owned_uncontested(Faction::Ai(0)));

    // A struct that still has a neutral sub is owned-but-NOT-fully (cannot export surplus yet).
    let q = w.add_struct(home_plus_neutral_struct(42, Faction::Player, 8, Vec2::new(60.0, 0.0), "Q"));
    let aggq = w.struct_aggregate(q);
    assert_eq!(aggq.owner, StructOwner::Owned(Faction::Player), "no enemy present ⇒ owned by Player");
    assert_eq!(aggq.neutral_subs, 1);
    assert!(
        !aggq.fully_owned_uncontested(Faction::Player),
        "a remaining neutral sub means not fully owned ⇒ not exportable"
    );
}

#[test]
fn aggregate_contested_struct() {
    // One structure, two subs: Player owns one, Enemy owns the other, both garrisoned ⇒ Contested.
    let mut st = Interior::new(43);
    let ps = st.add_sub(SubStructure::new(Vec2::new(0.0, 0.0), 5.0, Faction::Player));
    let es = st.add_sub(SubStructure::new(Vec2::new(60.0, 0.0), 5.0, Faction::Ai(0)));
    for _ in 0..3 {
        st.spawn_ship(Faction::Player, ps);
        st.spawn_ship(Faction::Ai(0), es);
    }
    let mut w = World::new();
    let c = w.add_struct(Structure::new(st, Vec2::new(0.0, 0.0), "C"));
    let agg = w.struct_aggregate(c);
    assert_eq!(agg.owner, StructOwner::Contested);
    assert_eq!(agg.player_subs, 1);
    assert_eq!(agg.enemy_subs, 1);
    assert_eq!(agg.player_ships, 3);
    assert_eq!(agg.enemy_ships, 3);
    assert!(!agg.fully_owned_uncontested(Faction::Player));
    assert!(!agg.fully_owned_uncontested(Faction::Ai(0)));
}

/// An incoming fleet shows up in the ship tally but does NOT, on its own, make a securely-held
/// struct non-exportable or contested (it has not landed). This is the documented rule.
#[test]
fn aggregate_incoming_counts_but_does_not_flip_owner() {
    let params = SimParams::default();
    let wp = WorldParams::default();
    let mut w = World::new();
    // A: Player home (export source). B: Player-owned, fully held (exportable).
    let a = w.add_struct(one_sub_struct(44, Faction::Player, 12, Vec2::new(0.0, 0.0), "A"));
    let b = w.add_struct(one_sub_struct(45, Faction::Player, 4, Vec2::new(20.0, 0.0), "B"));
    w.add_lane(a, b, 20.0);
    // Launch a Player fleet A->B; B is friendly, so incoming friendly must not flip anything.
    let launched = w.issue_fleet_order(FleetOrder::new(a, b, FractionBucket::Half), Faction::Player, &wp);
    assert!(launched > 0);
    w.step(&params, &wp); // still undocking/transiting
    let agg = w.struct_aggregate(b);
    assert_eq!(agg.owner, StructOwner::Owned(Faction::Player));
    assert_eq!(agg.player_incoming, launched, "incoming counted");
    assert!(agg.fully_owned_uncontested(Faction::Player), "friendly incoming does not block export");
    assert_eq!(agg.ships_of(Faction::Player), (agg.player_ships as u32) + launched);
}

// ===========================================================================
// (iii-b) Layer-2 wrappers of the new capture / soft-cap read signals
// ===========================================================================

/// The struct-scope wrappers of the new per-structure reads agree with the underlying
/// `Interior` reads: total foreign resistance vs a seat, parked count, and soft cap.
#[test]
fn struct_signal_wrappers_match_structure() {
    let params = SimParams::default();
    let mut w = World::new();
    // Structure with a Player home and a neutral sub (the neutral is "foreign" to Player).
    let p = w.add_struct(home_plus_neutral_struct(50, Faction::Player, 7, Vec2::new(0.0, 0.0), "P"));

    // Total foreign resistance vs Player = the neutral sub's resistance (default fresh value).
    let direct: f32 = w.structs[p].interior.total_foreign_resistance(Faction::Player);
    assert_eq!(w.struct_total_resistance_vs(p, Faction::Player), direct);
    assert!(direct > 0.0, "a remaining neutral sub contributes foreign resistance");

    // Parked count = living Player ships in the struct's structure.
    assert_eq!(
        w.parked_count(p, Faction::Player),
        w.structs[p].interior.parked_count(Faction::Player)
    );
    assert_eq!(w.parked_count(p, Faction::Player), 7);

    // Soft cap = softcap_free + softcap_per_sub * owned_subs (1 owned sub here).
    assert_eq!(
        w.soft_cap(p, Faction::Player, &params),
        w.structs[p].interior.soft_cap(Faction::Player, &params)
    );
    assert_eq!(w.soft_cap(p, Faction::Player, &params), params.softcap_free + params.softcap_per_sub);

    // Out-of-range struct ids yield zero (defensive).
    assert_eq!(w.struct_total_resistance_vs(999, Faction::Player), 0.0);
    assert_eq!(w.parked_count(999, Faction::Player), 0);
    assert_eq!(w.soft_cap(999, Faction::Player, &params), 0);
}

// ===========================================================================
// (iv) Determinism via state_hash
// ===========================================================================

/// Build a fixed 3-struct world and run a fixed script of fleet orders. Returns the per-tick
/// hash trace and the final hash.
fn run_scripted(extra_order: bool) -> (Vec<u64>, u64) {
    let params = SimParams::default();
    let wp = WorldParams::default();
    let mut w = World::new();
    let a = w.add_struct(one_sub_struct(0xA, Faction::Player, 12, Vec2::new(0.0, 0.0), "A"));
    let b = w.add_struct(one_sub_struct(0xB, Faction::Ai(0), 12, Vec2::new(40.0, 0.0), "B"));
    let c = w.add_struct(one_sub_struct(0xC, Faction::Neutral, 0, Vec2::new(20.0, 30.0), "C"));
    w.add_lane(a, c, 25.0);
    w.add_lane(b, c, 25.0);
    w.add_lane(a, b, 50.0);

    let mut hashes = Vec::new();
    for _ in 0..200 {
        if w.tick == 5 {
            w.issue_fleet_order(FleetOrder::new(a, c, FractionBucket::Half), Faction::Player, &wp);
            w.issue_fleet_order(FleetOrder::new(b, c, FractionBucket::Half), Faction::Ai(0), &wp);
        }
        if w.tick == 60 {
            w.issue_fleet_order(FleetOrder::new(a, b, FractionBucket::Quarter), Faction::Player, &wp);
        }
        if extra_order && w.tick == 80 {
            // One additional order that the baseline run does not issue.
            w.issue_fleet_order(FleetOrder::new(a, c, FractionBucket::Quarter), Faction::Player, &wp);
        }
        w.step(&params, &wp);
        hashes.push(w.state_hash());
    }
    (hashes, w.state_hash())
}

#[test]
fn deterministic_same_construction_same_orders() {
    let (trace_a, final_a) = run_scripted(false);
    let (trace_b, final_b) = run_scripted(false);
    assert_eq!(final_a, final_b, "final world hash diverged across identical runs");
    assert_eq!(trace_a, trace_b, "per-tick world hash trace diverged across identical runs");
}

#[test]
fn extra_order_diverges_hash() {
    let (_trace_base, final_base) = run_scripted(false);
    let (_trace_extra, final_extra) = run_scripted(true);
    assert_ne!(final_base, final_extra, "an extra order must change the world hash");
}

/// Cloning a world and stepping both identically keeps them bit-identical (each struct's RNG is
/// cloned) — the property a renderer relies on for replay/prediction.
#[test]
fn clone_replays_identically() {
    let params = SimParams::default();
    let wp = WorldParams::default();
    let mut a = World::new();
    let pa = a.add_struct(one_sub_struct(7, Faction::Player, 10, Vec2::new(0.0, 0.0), "A"));
    let pb = a.add_struct(one_sub_struct(8, Faction::Ai(0), 10, Vec2::new(30.0, 0.0), "B"));
    a.add_lane(pa, pb, 30.0);
    a.issue_fleet_order(FleetOrder::new(pa, pb, FractionBucket::Half), Faction::Player, &wp);
    for _ in 0..20 {
        a.step(&params, &wp);
    }
    let mut b = a.clone();
    for _ in 0..40 {
        a.step(&params, &wp);
        b.step(&params, &wp);
    }
    assert_eq!(a.state_hash(), b.state_hash(), "clone diverged from original");
}

// ===========================================================================
// (v) AI-free 2-struct smoke to a horizon
// ===========================================================================

/// A 2-struct world with both sides launching periodic fleets runs to a horizon without
/// panicking and yields a well-formed outcome. (No AI; a fixed cadence of orders.)
#[test]
fn two_struct_smoke_runs_to_horizon() {
    let params = SimParams::default();
    let wp = WorldParams::default();
    let mut w = World::new();
    let a = w.add_struct(home_plus_neutral_struct(100, Faction::Player, 14, Vec2::new(0.0, 0.0), "A"));
    let b = w.add_struct(home_plus_neutral_struct(101, Faction::Ai(0), 14, Vec2::new(60.0, 0.0), "B"));
    w.add_lane(a, b, 60.0).expect("lane");

    let horizon = 1500u64;
    while w.tick < horizon {
        if w.is_eliminated(Faction::Player) || w.is_eliminated(Faction::Ai(0)) {
            break;
        }
        // Both sides periodically push surplus at each other along the lane.
        if w.tick % 40 == 0 {
            w.issue_fleet_order(FleetOrder::new(a, b, FractionBucket::Half), Faction::Player, &wp);
            w.issue_fleet_order(FleetOrder::new(b, a, FractionBucket::Half), Faction::Ai(0), &wp);
        }
        w.step(&params, &wp);
    }
    let outcome = w.outcome();
    // Well-formed: tick within bounds, totals are self-consistent with the per-struct tallies.
    assert!(outcome.tick <= horizon);
    let p_total = w.total_ships(Faction::Player) + w.total_subs(Faction::Player);
    let e_total = w.total_ships(Faction::Ai(0)) + w.total_subs(Faction::Ai(0));
    assert_eq!(outcome.ships.0 + outcome.subs.0, p_total);
    assert_eq!(outcome.ships.1 + outcome.subs.1, e_total);
    // The world is not stuck with a phantom never-arriving fleet: any fleet present is mid-flight
    // with finite progress.
    for f in &w.fleets {
        assert!(f.progress <= 1.0 && f.count > 0);
    }
}

/// World elimination + outcome: a faction with no subs and no ships anywhere is eliminated and
/// the other wins by elimination.
#[test]
fn world_outcome_by_elimination() {
    let mut w = World::new();
    // Player holds a struct with a ship; Enemy holds nothing anywhere.
    w.add_struct(one_sub_struct(200, Faction::Player, 3, Vec2::new(0.0, 0.0), "P"));
    w.add_struct(one_sub_struct(201, Faction::Neutral, 0, Vec2::new(40.0, 0.0), "N"));
    assert!(w.is_eliminated(Faction::Ai(0)));
    assert!(!w.is_eliminated(Faction::Player));
    let o = w.outcome();
    assert_eq!(o.winner, Some(Faction::Player));
    assert!(o.by_elimination);
}

/// World outcome by lead at the horizon and exact-tie draw, including ships still in transit
/// (in-transit ships count toward the owner's total).
#[test]
fn world_outcome_by_lead_counts_in_transit() {
    let params = SimParams::default();
    let wp = WorldParams::default();
    let mut w = World::new();
    let a = w.add_struct(one_sub_struct(210, Faction::Player, 10, Vec2::new(0.0, 0.0), "A"));
    let b = w.add_struct(one_sub_struct(211, Faction::Player, 0, Vec2::new(20.0, 0.0), "B"));
    w.add_lane(a, b, 20.0);
    // Launch a fleet so some Player ships are mid-transit, then check totals include them.
    let launched = w.issue_fleet_order(FleetOrder::new(a, b, FractionBucket::Half), Faction::Player, &wp);
    assert!(launched > 0);
    w.step(&params, &wp); // mid-undock; fleet still in transit
    assert!(!w.fleets.is_empty(), "fleet should still be flying");
    let o = w.outcome();
    assert_eq!(o.winner, Some(Faction::Player), "Player leads (Enemy has nothing)");
    // In-transit ships are part of the Player total.
    assert!(o.ships.0 as u32 >= launched, "in-transit ships counted in the world total");
}

#[test]
fn empty_fortresses_do_not_prevent_elimination() {
    // Owner QoL: a seat with NO ships whose only holdings are zero-production specials
    // (fortresses) can never rebuild � it is eliminated, and the match is won. The horizon
    // territory count still sees the fort; only the ELIMINATION checks ignore it.
    let mut w = World::new();
    let mut ps = Interior::new(1);
    let home = ps.add_sub(SubStructure::new(Vec2::new(0.0, 0.0), 0.0, Faction::Player));
    ps.spawn_ship(Faction::Player, home);
    ps.add_storage_sub();
    w.add_struct(Structure::new(ps, Vec2::new(0.0, 0.0), "P"));
    let mut es = Interior::new(2);
    es.add_sub(SubStructure::fortress(Vec2::new(0.0, 0.0), Faction::Ai(0))); // fort, NO ships
    es.add_storage_sub();
    w.add_struct(Structure::new(es, Vec2::new(60.0, 0.0), "E"));

    assert!(w.is_eliminated(Faction::Ai(0)), "a shipless seat holding only a fort is dead");
    assert_eq!(w.total_subs(Faction::Ai(0)), 1, "the fort still counts as plain territory");
    let o = w.outcome();
    assert_eq!(o.winner, Some(Faction::Player));
    assert!(o.by_elimination, "victory seals by elimination, no mop-up of empty forts");
}
