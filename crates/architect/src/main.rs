//! architect — the autoconstructive-evolution driver (build-order step 5, the headline).
//!
//! Runs the population loop, **streams the glass-genome diffs + R1 instrumentation per
//! generation** to stderr so the evolution is watchable live, then writes
//! `ARCHITECT_RESULTS.md` at the repo root.
//!
//! Run with `cargo run -p architect --release`.
//!
//! Built on the deterministic mean-field engine + one seeded PRNG, so a given seed
//! reproduces the run bit-for-bit. The operating point is R2's robust `(r, k, l)` so the
//! results compose with the rest of the build.

use std::path::{Path, PathBuf};
use std::time::Instant;

use architect::glass;
use architect::population::{run, Config};
use architect::report::{self, compact_label};
use cell_core::{maps::all_maps, GameState, Params};

/// R2's robust operating point (`R2_RESULTS.md`): the `(r, k, l)` where the triad cycle is
/// most robust. The Architect evolves at the same point so the policies it discovers live
/// in the same strategic regime the rest of the build validated.
fn operating_params() -> Params {
    Params { r: 0.6, k: 2.25, l: 0.15, ..Params::default() }
}

fn main() -> std::io::Result<()> {
    let params = operating_params();
    let cfg = Config::default();

    let maps: Vec<GameState> = all_maps().into_iter().map(|m| m.state).collect();

    eprintln!(
        "architect: autoconstructive evolution | r={:.2} k={:.2} l={:.2} | pop {} × {} gens | \
         coevo-sample {} | horizon {} | seed 0x{:X}",
        params.r, params.k, params.l, cfg.pop_size, cfg.generations, cfg.coevo_sample,
        architect::fitness::HORIZON, cfg.seed
    );
    eprintln!(
        "{:->96}\n gen   best   champ rules species intrans maxDom arch  diff (glass-genome patch-notes)",
        ""
    );

    let t0 = Instant::now();
    let result = run(cfg, &maps, &params);
    let elapsed = t0.elapsed();

    // Stream a compact live view: one line per generation + the diff when the champion
    // legibly changed (this is the "watch it mutate between matches" feel, on the console).
    for l in &result.log {
        let diff_summary = if l.candidate_rejected {
            format!("REJECTED (would regress: {})", l.regressed.join(", "))
        } else if l.champion_changed && !l.champion_diff.is_empty() {
            l.champion_diff.iter().map(|d| d.render().trim().to_string()).collect::<Vec<_>>().join("  ")
        } else {
            String::new()
        };
        eprintln!(
            " {:>3} {:+.3} {:+.3}  {:>2}     {:>2}    {:.2}   {:.2}  {:>2}   {}",
            l.gen,
            l.best_fitness,
            l.champion.fitness,
            l.champion_rule_count,
            l.genotypic.distinct_species,
            l.non_transitivity.intransitive_triads,
            l.non_transitivity.max_dominance,
            l.archive_size,
            diff_summary,
        );
    }

    eprintln!("{:->96}", "");
    let (r1, intrans, dom, species) = report::classify_r1(&result.log);
    eprintln!(
        "done in {:.2}s | final champion: {} ({} rules) fitness {:+.3}",
        elapsed.as_secs_f64(),
        compact_label(&result.final_champion),
        result.final_champion.rule_count(),
        result.final_champion_fitness.fitness,
    );
    eprintln!(
        "R1 READ: {}  (late intransitivity {:.0}%, max-dominance {:.0}%, {:.1} species, archive {})",
        r1.as_str(),
        100.0 * intrans,
        100.0 * dom,
        species,
        result.archive.len(),
    );
    eprintln!("\nfinal glass genome:\n{}", glass::render_genome(&result.final_champion));

    // Write the report at the repo root (cwd when run via `cargo run` from the workspace).
    let root = std::env::current_dir()?;
    let md = join(&root, "ARCHITECT_RESULTS.md");
    std::fs::write(&md, report::to_markdown(&result, &params, elapsed.as_secs_f64()))?;
    eprintln!("\nwrote: {}", md.display());

    Ok(())
}

fn join(root: &Path, name: &str) -> PathBuf {
    let mut p = root.to_path_buf();
    p.push(name);
    p
}
