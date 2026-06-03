//! Tests for the sweep machinery (axis spacing, region analysis) and an end-to-end
//! smoke test that a small sweep runs deterministically and finds a holding region.

use cell_core::maps::all_maps;

// Pull in the sweep modules by path. They live in the binary crate, so we re-declare
// them here as part of the integration test target.
#[path = "../src/sweep.rs"]
mod sweep;
#[path = "../src/report.rs"]
mod report;

use sweep::{Axis, SweepConfig};

#[test]
fn axis_values_are_evenly_spaced_inclusive() {
    let a = Axis { name: "x", min: 0.0, max: 1.0, steps: 5 };
    let v = a.values();
    assert_eq!(v.len(), 5);
    assert!((v[0] - 0.0).abs() < 1e-12);
    assert!((v[4] - 1.0).abs() < 1e-12);
    assert!((v[2] - 0.5).abs() < 1e-12);
}

#[test]
fn axis_single_step_returns_min() {
    let a = Axis { name: "x", min: 0.7, max: 2.0, steps: 1 };
    assert_eq!(a.values(), vec![0.7]);
}

/// A small but real sweep should run deterministically and (given the engine is
/// tuned correctly) find at least one holding point with a contiguous region.
/// This is an end-to-end guard: if the cycle ever stops existing, this fails loudly.
#[test]
fn small_sweep_finds_holding_region_and_is_deterministic() {
    let states: Vec<_> = all_maps().into_iter().map(|m| m.state).collect();
    // A coarse but meaningful grid for speed.
    let cfg = SweepConfig {
        r_axis: Axis { name: "r", min: 0.5, max: 1.5, steps: 4 },
        k_axis: Axis { name: "k", min: 1.0, max: 2.5, steps: 4 },
        l_axis: Axis { name: "l", min: 0.0, max: 0.4, steps: 5 },
        horizon: 400,
        epsilon: 0.05,
        fixed_undock: 3.0,
        fixed_combat_rate: 0.5,
        fixed_substeps: 8,
    };

    let r1 = sweep::run_sweep(&states, &cfg);
    let r2 = sweep::run_sweep(&states, &cfg);

    // Determinism: same holds_count and same operating point.
    assert_eq!(r1.holds_count, r2.holds_count, "sweep not deterministic (holds_count)");
    assert_eq!(r1.best_idx, r2.best_idx, "sweep not deterministic (operating point)");

    // The whole point of R2: a region must exist.
    assert!(
        r1.holds_count > 0,
        "expected at least one holding grid point; none found (cycle degenerate?)"
    );
    assert!(
        r1.largest_region_size >= 1,
        "expected a contiguous holding region"
    );

    // The operating point, if it holds, must clear epsilon on every edge.
    if let Some(bi) = r1.best_idx {
        let p = &r1.points[bi];
        if p.holds(cfg.epsilon) {
            assert!(p.cycle.attack_beats_colonize > cfg.epsilon);
            assert!(p.cycle.colonize_beats_defend > cfg.epsilon);
            assert!(p.cycle.defend_beats_attack > cfg.epsilon);
        }
    }
}

/// Run the FULL production sweep and write `R2_RESULTS.md`, `r2_results.csv`, and
/// `r2_results.json` to the repo root. This mirrors the `r2-sweep` binary's `main`
/// and exists as an `#[ignore]`d test purely so the reports can be regenerated via
/// the cargo *test* harness on machines where Windows Smart App Control blocks
/// running freshly-built application binaries (`cargo run`). Invoke explicitly:
///
/// ```text
/// cargo test -p r2-sweep --release --test sweep_tests -- --ignored generate_reports
/// ```
///
/// It is `#[ignore]`d so the normal `cargo test` stays fast and side-effect-free.
#[test]
#[ignore = "side-effectful, slow; run explicitly to (re)generate R2 reports"]
fn generate_reports() {
    let config = SweepConfig::default();
    let specs = all_maps();
    let map_names: Vec<String> = specs.iter().map(|m| m.name.to_string()).collect();
    let named_maps: Vec<(String, cell_core::GameState)> =
        specs.iter().map(|m| (m.name.to_string(), m.state.clone())).collect();
    let states: Vec<cell_core::GameState> = specs.iter().map(|m| m.state.clone()).collect();

    let result = sweep::run_sweep(&states, &config);
    let per_map = if let Some(bi) = result.best_idx {
        let p = &result.points[bi];
        let params = config.params_at(p.r, p.k, p.l);
        sweep::per_map_detail(&named_maps, &params, config.horizon)
    } else {
        Vec::new()
    };

    // Repo root = two levels up from this crate's manifest dir.
    let manifest = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let root = manifest
        .parent()
        .and_then(|p| p.parent())
        .expect("repo root")
        .to_path_buf();

    let md = root.join("R2_RESULTS.md");
    let csv = root.join("r2_results.csv");
    let json = root.join("r2_results.json");
    std::fs::write(&md, report::to_markdown(&result, &per_map, &map_names)).unwrap();
    std::fs::write(&csv, report::to_csv(&result)).unwrap();
    std::fs::write(&json, report::to_json(&result, &per_map)).unwrap();

    // Assert the headline result so this also serves as a regression guard.
    assert!(result.largest_region_size > 0, "R2: no holding region found!");
    eprintln!(
        "Wrote reports to {}: {}/{} hold, largest region {}",
        root.display(),
        result.holds_count,
        result.total,
        result.largest_region_size
    );
}
