//! capstone — the arc-1 capstone validation driver (build-order step 4).
//!
//! Plays the inferring [`automaton::Capstone`] player, the fixed non-inferring
//! [`automaton::Baseline`] control, and the perfect-information [`automaton::Oracle`]
//! upper bound against the whole hidden-but-fixed Automaton **ladder** (and a dense
//! uniform simplex **grid**), then writes `CAPSTONE_RESULTS.md`, `capstone_results.csv`,
//! and `capstone_results.json` at the repo root.
//!
//! Run with `cargo run -p automaton --bin capstone --release`.
//!
//! Everything is built on the deterministic mean-field engine — no RNG — so reruns are
//! bit-identical. The match budget and operating point are documented in the report.

use std::path::{Path, PathBuf};
use std::time::Instant;

use automaton::report::{self, ReportContext};
use automaton::{default_ladder, evaluate_ladder, simplex_grid_study, InferConfig};
use cell_core::{maps::all_maps, GameState, Params};

/// R2's robust operating point (`R2_RESULTS.md`): the (r, k, l) where the triad cycle is
/// most robust. The capstone is validated at the same point so its results compose with
/// the rest of the build.
fn operating_params() -> Params {
    Params { r: 0.6, k: 2.25, l: 0.15, ..Params::default() }
}

const HORIZON: u64 = 600;
const GRID_STEPS: u32 = 6;

fn main() -> std::io::Result<()> {
    let params = operating_params();
    let cfg = InferConfig::default();

    let named_maps: Vec<(String, GameState)> =
        all_maps().into_iter().map(|m| (m.name.to_string(), m.state)).collect();
    let ladder = default_ladder();

    eprintln!(
        "capstone validation: r={:.3} k={:.3} l={:.3} | {} maps × 2 seatings | horizon {} | scout {} ticks | {} rungs",
        params.r, params.k, params.l, named_maps.len(), HORIZON, cfg.scout_ticks, ladder.len()
    );

    let t0 = Instant::now();
    let eval = evaluate_ladder(&ladder, &named_maps, cfg, &params, HORIZON);
    let study = simplex_grid_study(&named_maps, cfg, &params, HORIZON, GRID_STEPS);
    let elapsed = t0.elapsed();

    eprintln!(
        "done in {:.2}s | cap_wr {:.3} base_wr {:.3} orc_wr {:.3} | inf_err {:.3} corner_acc {:.3}",
        elapsed.as_secs_f64(),
        eval.mean_capstone_winrate,
        eval.mean_baseline_winrate,
        eval.mean_oracle_winrate,
        eval.mean_inference_error,
        eval.corner_accuracy,
    );
    eprintln!(
        "centrality→difficulty (grid Spearman): read {:+.3} counter {:+.3} combined {:+.3}",
        study.read_difficulty_rho, study.counter_difficulty_rho, study.combined_difficulty_rho
    );

    let ctx = ReportContext {
        params,
        horizon: HORIZON,
        scout_ticks: cfg.scout_ticks,
        n_maps: named_maps.len(),
        grid_steps: GRID_STEPS,
    };

    let root = std::env::current_dir()?;
    write_all(&root, &eval, &study, &ctx)?;
    Ok(())
}

/// Render and write the three artifacts at `root`. Factored out so the (blocked-binary-safe)
/// regression test can call the identical path.
fn write_all(
    root: &Path,
    eval: &automaton::LadderEvaluation,
    study: &automaton::SimplexStudy,
    ctx: &ReportContext,
) -> std::io::Result<()> {
    let md = join(root, "CAPSTONE_RESULTS.md");
    let csv = join(root, "capstone_results.csv");
    let json = join(root, "capstone_results.json");
    std::fs::write(&md, report::to_markdown(eval, study, ctx))?;
    std::fs::write(&csv, report::to_csv(eval))?;
    std::fs::write(&json, report::to_json(eval, study, ctx))?;
    eprintln!("wrote:\n  {}\n  {}\n  {}", md.display(), csv.display(), json.display());
    Ok(())
}

fn join(root: &Path, name: &str) -> PathBuf {
    let mut p = root.to_path_buf();
    p.push(name);
    p
}
