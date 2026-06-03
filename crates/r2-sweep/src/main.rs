//! r2-sweep — headless cycle-sweep answering risk **R2** (cycle non-degeneracy).
//!
//! Sweeps the three-number space `(r, k, l)` over a documented grid, runs the
//! three ordered triad matchups on each symmetric map in **both seatings**, and
//! reports the fraction of points where the rock-paper-scissors cycle holds, the
//! shape/extent of the non-degenerate region, a robust operating point, and raw
//! numbers — to `R2_RESULTS.md`, `r2_results.csv`, and `r2_results.json` at the
//! repo root.
//!
//! Run with `cargo run -p r2-sweep --release`.

mod report;
mod sweep;

use cell_core::maps::all_maps;
use std::path::PathBuf;
use std::time::Instant;

fn main() -> std::io::Result<()> {
    let config = sweep::SweepConfig::default();

    // Build the symmetric maps once; keep names alongside states for reporting.
    let specs = all_maps();
    let map_names: Vec<String> = specs.iter().map(|m| m.name.to_string()).collect();
    let named_maps: Vec<(String, cell_core::GameState)> = specs
        .iter()
        .map(|m| (m.name.to_string(), m.state.clone()))
        .collect();
    let states: Vec<cell_core::GameState> = specs.iter().map(|m| m.state.clone()).collect();

    eprintln!("r2-sweep: {}", config.describe());
    eprintln!(
        "Maps: {} | matches/point: {} | total points: {}",
        map_names.join(", "),
        3 * 2 * map_names.len(),
        config.r_axis.steps * config.k_axis.steps * config.l_axis.steps
    );

    let t0 = Instant::now();
    let result = sweep::run_sweep(&states, &config);
    let elapsed = t0.elapsed();

    // Per-map detail at the chosen operating point.
    let per_map = if let Some(bi) = result.best_idx {
        let p = &result.points[bi];
        let params = config.params_at(p.r, p.k, p.l);
        sweep::per_map_detail(&named_maps, &params, config.horizon)
    } else {
        Vec::new()
    };

    // Summary to stderr.
    let frac = if result.total > 0 {
        result.holds_count as f64 / result.total as f64
    } else {
        0.0
    };
    eprintln!(
        "Done in {:.2}s: {}/{} points hold ({:.1}%); largest contiguous region = {} points",
        elapsed.as_secs_f64(),
        result.holds_count,
        result.total,
        frac * 100.0,
        result.largest_region_size,
    );
    if let Some(bi) = result.best_idx {
        let p = &result.points[bi];
        eprintln!(
            "Operating point: r={:.3} k={:.3} l={:.3} (holds={}, min_margin={:+.4})",
            p.r,
            p.k,
            p.l,
            p.holds(config.epsilon),
            p.min_margin(),
        );
    }

    // Resolve the repo root: the binary runs from the workspace, so use the
    // current dir. (cargo run sets cwd to the workspace root.)
    let root = std::env::current_dir()?;
    let md_path = join(&root, "R2_RESULTS.md");
    let csv_path = join(&root, "r2_results.csv");
    let json_path = join(&root, "r2_results.json");

    std::fs::write(&md_path, report::to_markdown(&result, &per_map, &map_names))?;
    std::fs::write(&csv_path, report::to_csv(&result))?;
    std::fs::write(&json_path, report::to_json(&result, &per_map))?;

    eprintln!("Wrote:\n  {}\n  {}\n  {}", md_path.display(), csv_path.display(), json_path.display());
    Ok(())
}

fn join(root: &std::path::Path, name: &str) -> PathBuf {
    let mut p = root.to_path_buf();
    p.push(name);
    p
}
