//! `counter-diag` — the headless **Counter diagnostic** binary (COUNTER_DESIGN §7).
//!
//! Runs `Roster::Counter { p_max }` against each of the four diagnostic targets
//! (`Colonize` / `Defend` / `Attack` / `SimpleColonize`), sweeping `p_max` over the playstyle range
//! `{0.2, 0.6, 0.95}`, on **both seatings** of several seeds, to the standard match horizon. For each
//! `(target, p_max)` it prints the Counter's inferred profile vs the target's ground truth (does the
//! mix point at the right RPS corner? does the thin-rear seam fire?), the Counter's win-rate, and
//! which backbone/exploit it converged on.
//!
//! The Counter is a **diagnostic for the automata**: a brittle exploit that fires flags "PATCH that
//! automaton"; a mixed line that beats the intended RPS counter flags "REFINE it"; and the `p_max`
//! sweep traces the robust-generalist ↔ vulnerability-hunter playstyle axis (its win-rate effect is
//! non-monotonic across targets). Everything is deterministic (the projection draws no RNG, the
//! profile is a deterministic function of the observation log), so reruns are bit-identical.
//!
//! Run with `cargo run -p ai --bin counter-diag --release` (release strongly recommended — it runs
//! ~120 full matches). The exact output is pasted into `COUNTER_RESULTS.md`.

use ai::harness::{
    counter_diag_targets, counter_diagnostic, format_counter_diagnostic, COUNTER_DIAG_P_MAX,
    COUNTER_DIAG_SEEDS, DEFAULT_HORIZON,
};

fn main() {
    let targets = counter_diag_targets();
    let rows = counter_diagnostic(&targets, &COUNTER_DIAG_P_MAX, &COUNTER_DIAG_SEEDS, DEFAULT_HORIZON);
    print!("{}", format_counter_diagnostic(&rows, COUNTER_DIAG_SEEDS.len()));

    // A compact machine-readable summary line per cell (handy for diffing / regression eyeballing).
    println!("--- summary (target,p_max -> counter_wins/games, inferred(ok?), seam, exploit_rate) ---");
    for row in &rows {
        for c in &row.cells {
            println!(
                "  {:<14} p={:<4} {:>2}/{:<2}  inferred={}{}  seam={}  exploit_rate={:.2}",
                row.target,
                c.p_max,
                c.counter_wins,
                c.games(),
                c.inferred_dominant,
                if c.inferred_matches_truth { "(ok)" } else { "(X)" },
                if c.seam_fired { "Y" } else { "n" },
                c.exploit_rate(),
            );
        }
    }
    println!("\nDone.");
}
