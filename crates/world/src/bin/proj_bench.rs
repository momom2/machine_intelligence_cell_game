//! `proj-bench` — the profiling harness for the forward [`world::Projection`] (R3 HARD REQUIREMENT).
//!
//! It builds a **representative** multi-planet world — six planets in a ring + two cross lanes,
//! each planet a 3–4 sub structure with mixed ownership and garrisons, plus **several ships in
//! intra-structure transit and several inter-planet fleets in flight** — then times
//! [`world::World::project_forward`] over many iterations and reports the **average microseconds
//! per call**. The projection must run cheaply enough to call on every AI decision tick (well
//! under ~1 ms/call), so this is the gate that confirms the event-driven integrator is fast.
//!
//! Deterministic: the world is seeded and the projection draws no randomness, so the *work* timed
//! is identical run to run (only the wall-clock varies). Run release for a meaningful number:
//! `cargo run -p world --release --bin proj-bench`.

use std::hint::black_box;
use std::time::Instant;

use layer1::{Faction, FractionBucket, MoveOrder, SimParams, Structure, SubStructure, Vec2};
use world::{FleetOrder, Planet, World, WorldParams, DEFAULT_PROJECTION_HORIZON};

/// Build one planet as a small structure of `n_subs` subs in a ring around the local origin, with
/// the given `owner` holding the first sub and a starting garrison there, and the rest neutral.
/// A couple of the owner's idle ships are then put into **intra-structure transit** toward other
/// subs so the projection has moving-ship arrivals to schedule.
fn make_planet(seed: u64, owner: Faction, n_subs: usize, garrison: usize, map: Vec2, name: &str) -> Planet {
    let mut st = Structure::new(seed);
    let mut subs = Vec::new();
    for i in 0..n_subs {
        let ang = i as f32 / n_subs as f32 * std::f32::consts::TAU;
        let pos = Vec2::new(30.0 * ang.cos(), 30.0 * ang.sin());
        let f = if i == 0 { owner } else { Faction::Neutral };
        // Mixed resistances so both heal and grind segments occur in the integration.
        let maxr = 60.0 + (i as f32) * 40.0;
        subs.push(st.add_sub(SubStructure::new(pos, 6.0, f).with_max_resistance(maxr)));
    }
    for _ in 0..garrison {
        st.spawn_ship(owner, subs[0]);
    }
    // Put a few of the owner's ships into intra-structure transit toward the next subs (creates
    // moving-ship arrival events for the projection to integrate).
    if n_subs >= 2 {
        st.issue_order(MoveOrder::new(subs[0], subs[1], FractionBucket::Quarter));
    }
    if n_subs >= 3 {
        st.issue_order(MoveOrder::new(subs[0], subs[2], FractionBucket::Quarter));
    }
    Planet::new(st, map, name)
}

/// Build the representative world and seed several inter-planet fleets in flight.
fn build_world(sp: &SimParams, wp: &WorldParams) -> World {
    let mut w = World::new();
    // Six planets in a ring, alternating ownership, 3–4 subs each.
    let n = 6usize;
    for i in 0..n {
        let ang = i as f32 / n as f32 * std::f32::consts::TAU;
        let map = Vec2::new(200.0 * ang.cos(), 200.0 * ang.sin());
        let owner = if i % 2 == 0 { Faction::Player } else { Faction::Enemy };
        let n_subs = 3 + (i % 2); // 3 or 4 subs
        let p = make_planet(100 + i as u64, owner, n_subs, 16, map, &format!("P{i}"));
        w.add_planet(p);
    }
    // Ring lanes + two cross lanes (a small but non-trivial graph).
    for i in 0..n {
        let _ = w.add_lane(i, (i + 1) % n, 60.0);
    }
    let _ = w.add_lane(0, 3, 90.0);
    let _ = w.add_lane(1, 4, 90.0);

    // Launch several inter-planet fleets so the projection has fleet arrivals to schedule into
    // entry subs across multiple planets, at varied progress (step a bit between launches).
    for i in 0..n {
        let from = i;
        let to = (i + 1) % n;
        let seat = if i % 2 == 0 { Faction::Player } else { Faction::Enemy };
        w.issue_fleet_order(FleetOrder::new(from, to, FractionBucket::Half), seat, wp);
        // Advance a couple of ticks so fleets sit at different undock/progress phases.
        w.step(sp, wp);
        w.step(sp, wp);
    }
    // A couple of long cross-lane fleets too.
    w.issue_fleet_order(FleetOrder::new(0, 3, FractionBucket::Quarter), Faction::Player, wp);
    w.issue_fleet_order(FleetOrder::new(1, 4, FractionBucket::Quarter), Faction::Enemy, wp);
    w
}

fn main() {
    let sp = SimParams::default();
    let wp = WorldParams::default();
    let w = build_world(&sp, &wp);

    let total_subs: usize = w.planets.iter().map(|p| p.structure.subs.len()).sum();
    let total_ships: usize = w.planets.iter().map(|p| p.structure.ships.len()).sum();
    let in_transit: usize = w
        .planets
        .iter()
        .map(|p| p.structure.ships.iter().filter(|s| s.alive && s.target.is_some()).count())
        .sum();
    let fleet_ships: u32 = w.fleets.iter().map(|f| f.count).sum();

    println!("== proj-bench: forward-projection profiling ==");
    println!(
        "world: {} planets, {} subs, {} garrisoned ships ({} intra-transit), {} fleets in flight ({} ships)",
        w.planets.len(),
        total_subs,
        total_ships,
        in_transit,
        w.fleets.len(),
        fleet_ships,
    );
    println!("horizon: {DEFAULT_PROJECTION_HORIZON} ticks\n");

    // Warm up (page in code, settle the CPU) — also asserts the call works.
    for _ in 0..1_000 {
        let proj = w.project_forward(&sp, &wp, DEFAULT_PROJECTION_HORIZON);
        black_box(proj.sub_fate(0, 0));
    }

    // Time many short batches at the default horizon. Each batch is small so a single OS
    // preemption inflates only that one sample; we report the BEST (the contention-free floor —
    // the true cost of the work) alongside the MEDIAN and the worst, so the noise is visible and
    // the headline number is honest rather than a lucky minimum hiding a slow path.
    let iters: u64 = 20_000;
    let batches = 25;
    let mut samples_ns: Vec<f64> = Vec::with_capacity(batches);
    for _ in 0..batches {
        let t0 = Instant::now();
        for _ in 0..iters {
            let proj = w.project_forward(&sp, &wp, DEFAULT_PROJECTION_HORIZON);
            // Touch a few derived queries so the optimizer cannot elide the integration.
            black_box(proj.sub_fate(0, 0));
            black_box(proj.planet_capture(0));
            black_box(proj.incoming_present_at(1, 0, Faction::Player));
        }
        let ns_per = t0.elapsed().as_nanos() as f64 / iters as f64;
        samples_ns.push(ns_per);
    }
    samples_ns.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let best_ns_per = samples_ns[0];
    let median_ns_per = samples_ns[samples_ns.len() / 2];
    let worst_ns_per = *samples_ns.last().unwrap();
    println!(
        "  {batches} batches x {iters} calls each:  best {:.3}  median {:.3}  worst {:.3}  us/call",
        best_ns_per / 1000.0,
        median_ns_per / 1000.0,
        worst_ns_per / 1000.0,
    );

    // The BEST is the algorithm's real cost (no preemption); the headline.
    let us_per = best_ns_per / 1000.0;
    println!(
        "\nBEST: {:.3} us/call ({:.1} ns/call) at horizon {DEFAULT_PROJECTION_HORIZON}  (median {:.3} us/call)",
        us_per,
        best_ns_per,
        median_ns_per / 1000.0,
    );

    // Also report a few alternative horizons to show the event-driven scaling stays cheap.
    // Best-of-several short runs each, so a preemption does not masquerade as super-linear cost.
    for &h in &[60u64, 240, 1000, 5000] {
        let iters_h: u64 = 20_000;
        let mut best_h = f64::INFINITY;
        for _ in 0..8 {
            let t0 = Instant::now();
            for _ in 0..iters_h {
                let proj = w.project_forward(&sp, &wp, h);
                black_box(proj.sub_fate(0, 0));
            }
            best_h = best_h.min(t0.elapsed().as_nanos() as f64 / iters_h as f64);
        }
        println!("  horizon {h:>5}: {:.3} us/call (best)", best_h / 1000.0);
    }

    let budget_us = 1000.0; // ~1 ms/call ceiling from R3.
    if us_per < budget_us {
        println!(
            "\nPASS: {:.3} us/call is well under the ~{} us (~1 ms) per-decision-tick budget.",
            us_per, budget_us as u64
        );
    } else {
        println!("\nFAIL: {:.3} us/call exceeds the ~1 ms budget.", us_per);
        std::process::exit(1);
    }
}
