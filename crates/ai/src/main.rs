//! `ai-harness` — the headless verification binary for the AI layer.
//!
//! It runs two demonstrations on the symmetric test worlds and prints the results:
//!
//! 1. **The greedy seam** — a pure-greedy seat vs. a rear-strike line, showing the rear strike
//!    exploits greedy's "never posts a rear guard" seam.
//! 2. **The pure-strategy cycle** — the round-robin of Colonize / Defend / Attack over both
//!    seatings on the standard worlds, printing the win-rate matrix and whether the validated
//!    cycle (attack > colonize > defend > attack) closes here.
//!
//! Everything is deterministic (no RNG outside each planet's seeded sim), so reruns are
//! bit-identical. Run with `cargo run -p ai --bin ai-harness` (release recommended for speed).

use ai::controller::Roster;
use ai::harness::{corridor_world, diamond_world, duel_both_seatings, DEFAULT_HORIZON};
use layer1::SimParams;
use world::WorldParams;

fn main() {
    let params = SimParams::default();
    let wp = WorldParams::default();

    println!("== AI-vs-AI harness (deterministic; horizon {DEFAULT_HORIZON} ticks) ==\n");

    // ---- (1) The pure-strategy cycle on the corridor + diamond worlds. ----
    let pures = [Roster::Colonize, Roster::Defend, Roster::Attack];
    for (label, build) in [
        ("corridor", &corridor_world as &dyn Fn(u64) -> world::World),
        ("diamond", &diamond_world as &dyn Fn(u64) -> world::World),
    ] {
        println!("--- Pure-strategy round robin on the {label} world (both seatings) ---");
        println!("        (row beats column? wins-losses-draws over 2 seatings)\n");
        // Header
        print!("{:>10}", "");
        for c in pures {
            print!("{:>16}", c.name());
        }
        println!();
        for a in pures {
            print!("{:>10}", a.name());
            for b in pures {
                if a == b {
                    print!("{:>16}", "—");
                    continue;
                }
                let (aw, bw, dr) = duel_both_seatings(|| build(1), &params, &wp, a, b);
                print!("{:>16}", format!("{aw}-{bw}-{dr}"));
            }
            println!();
        }
        println!();

        // The three RPS edges, summed over seatings, with a verdict.
        let edges = [
            ("attack  > colonize", Roster::Attack, Roster::Colonize),
            ("colonize> defend  ", Roster::Colonize, Roster::Defend),
            ("defend  > attack  ", Roster::Defend, Roster::Attack),
        ];
        let mut closed = true;
        for (name, a, b) in edges {
            let (aw, bw, dr) = duel_both_seatings(|| build(1), &params, &wp, a, b);
            let ok = aw > bw;
            closed &= ok;
            println!(
                "  edge {name}: {aw}-{bw}-{dr}  => {}",
                if ok { "HOLDS" } else { "does NOT hold" }
            );
        }
        println!(
            "  => the attack>colonize>defend>attack cycle {} on {label}.\n",
            if closed { "CLOSES" } else { "does NOT fully close" }
        );
    }

    // ---- (2) Multi-seed robustness of the cycle on the diamond (the world that closes it). ----
    println!("--- Diamond cycle over many seeds (both seatings each) ---");
    let seeds: [u64; 8] = [1, 7, 42, 100, 0x5EA1, 2024, 31337, 9001];
    let edges = [
        ("attack > colonize", Roster::Attack, Roster::Colonize),
        ("colonize> defend ", Roster::Colonize, Roster::Defend),
        ("defend > attack  ", Roster::Defend, Roster::Attack),
    ];
    for (name, a, b) in edges {
        let (mut aw, mut bw, mut dr) = (0u32, 0u32, 0u32);
        for &s in &seeds {
            let (x, y, z) = duel_both_seatings(|| diamond_world(s), &params, &wp, a, b);
            aw += x;
            bw += y;
            dr += z;
        }
        let games = aw + bw + dr;
        println!(
            "  {name}: A={aw} B={bw} draws={dr} over {games} games  (A win-rate {:.2}) => {}",
            aw as f64 / games as f64,
            if aw > bw { "edge holds" } else { "edge does NOT hold" }
        );
    }
    println!();

    // ---- (3) Greedy vs the pure strategies (sanity: greedy is a real player). ----
    println!("--- GreedyLocal vs the pure strategies (corridor, both seatings) ---");
    for opp in [Roster::Colonize, Roster::Defend, Roster::Attack, Roster::Passive] {
        let (gw, ow, dr) =
            duel_both_seatings(|| corridor_world(1), &params, &wp, Roster::GreedyLocal, opp);
        println!("  GreedyLocal vs {:<16}: {gw}-{ow}-{dr}", opp.name());
    }
    println!("\nDone.");
}
