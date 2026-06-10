//! layer1-headless — the Layer-1 verification artifact (no GUI).
//!
//! Builds the sample structure, runs the [`layer1::Automaton`] against itself from a FIXED
//! seed to elimination or a horizon, and prints a readable per-~50-tick summary: tick, ship
//! count per faction, sub-structures owned per faction, and the number of active battle
//! bubbles. Then prints the final outcome. This proves the sim works without any display.
//!
//! Run with `cargo run -p layer1 --bin layer1-headless`.
//!
//! Everything is deterministic from the seed (the library's only randomness is its embedded
//! seeded PRNG), so reruns are bit-identical.

use layer1::scenario::{sample_params, sample_structure};
use layer1::{run_auto_vs_auto, Automaton, Faction, SimParams, Structure};

/// Fixed seed so the headless run is reproducible.
const SEED: u64 = 0xC0FFEE_1234;
/// Hard horizon; the match usually ends earlier by elimination.
const HORIZON: u64 = 4000;
/// How often the Automatons re-plan (ticks). They commit gradually between decisions.
const DECISION_INTERVAL: u64 = 4;
/// Print a summary line every this-many ticks.
const SUMMARY_EVERY: u64 = 50;

fn main() {
    let params = sample_params();
    let (mut st, layout) = sample_structure(SEED);
    let player = Automaton::new(Faction::Player);
    let enemy = Automaton::new(Faction::Ai(0));

    println!("== Layer-1 headless: Automaton vs Automaton ==");
    println!(
        "seed 0x{SEED:X} | horizon {HORIZON} | decision interval {DECISION_INTERVAL} ticks | summary every {SUMMARY_EVERY} ticks"
    );
    print_params(&params);
    println!(
        "structure: {} sub-structures (player_home={}, enemy_home={}, player_post={}, enemy_post={}, neutral_left={}, neutral_right={}, neutral_keep={})",
        st.subs.len(),
        layout.player_home,
        layout.enemy_home,
        layout.player_post,
        layout.enemy_post,
        layout.neutral_left,
        layout.neutral_right,
        layout.neutral_keep,
    );
    println!();
    print_header();
    // Print the opening state (tick 0) before any stepping.
    print_summary(0, &st, &params);

    let outcome = run_auto_vs_auto(
        &mut st,
        &params,
        &player,
        &enemy,
        HORIZON,
        DECISION_INTERVAL,
        |tick, st| {
            if tick % SUMMARY_EVERY == 0 {
                print_summary(tick, st, &params);
            }
        },
    );

    // Always print the final tick's line if it wasn't a summary boundary.
    if outcome.tick % SUMMARY_EVERY != 0 {
        print_summary(outcome.tick, &st, &params);
    }

    println!();
    let winner = match outcome.winner {
        Some(Faction::Player) => "PLAYER".to_string(),
        Some(Faction::Ai(i)) => format!("AI#{i}"),
        Some(Faction::Neutral) => "NEUTRAL?".to_string(),
        None => "DRAW".to_string(),
    };
    let how = if outcome.by_elimination {
        "by elimination"
    } else {
        "by lead at horizon"
    };
    println!(
        "FINAL @ tick {}: winner = {} ({}) | ships P={} E={} | subs P={} E={}",
        outcome.tick,
        winner,
        how,
        outcome.ships.0,
        outcome.ships.1,
        outcome.subs.0,
        outcome.subs.1,
    );
    println!(
        "final state hash: 0x{:016X} (identical on every rerun with this seed)",
        st.state_hash()
    );
}

fn print_params(p: &SimParams) {
    println!(
        "params: R={:.1} fire_p={:.3} substeps={} speed={:.2} prod_period={} defender_bonus={:.3} spread={:.1}",
        p.engagement_radius,
        p.fire_prob,
        p.combat_substeps,
        p.ship_speed,
        p.production_period,
        p.defender_fire_bonus,
        p.spread_radius,
    );
}

fn print_header() {
    println!("{:>6} | {:>9} | {:>9} | {:>7}", "tick", "ships P/E", "subs P/E", "bubbles");
    println!("{:->6}-+-{:->9}-+-{:->9}-+-{:->7}", "", "", "", "");
}

fn print_summary(tick: u64, st: &Structure, params: &SimParams) {
    let p_ships = st.ship_count(Faction::Player);
    let e_ships = st.ship_count(Faction::Ai(0));
    let p_subs = st.sub_count(Faction::Player);
    let e_subs = st.sub_count(Faction::Ai(0));
    let bubbles = st.bubble_count(params);
    println!(
        "{:>6} | {:>4}/{:<4} | {:>4}/{:<4} | {:>7}",
        tick,
        p_ships,
        e_ships,
        p_subs,
        e_subs,
        bubbles,
    );
}
