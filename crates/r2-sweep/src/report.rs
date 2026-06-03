//! Report writers: human-readable Markdown, plus machine-readable CSV and JSON.
//!
//! No serde dependency — the JSON is small and structured, so we emit it by hand.
//! This keeps cell-core/r2-sweep dependency-free and fast to compile.

use crate::sweep::{PerMapDetail, SweepResult};
use std::fmt::Write as _;

/// Build the CSV string: one row per grid point with the three margins and a
/// holds flag. Header documents the columns.
pub fn to_csv(res: &SweepResult) -> String {
    let mut s = String::new();
    s.push_str("r,k,l,attack_beats_colonize,colonize_beats_defend,defend_beats_attack,min_margin,cycle_holds\n");
    for p in &res.points {
        let c = &p.cycle;
        let _ = writeln!(
            s,
            "{:.4},{:.4},{:.4},{:.6},{:.6},{:.6},{:.6},{}",
            p.r,
            p.k,
            p.l,
            c.attack_beats_colonize,
            c.colonize_beats_defend,
            c.defend_beats_attack,
            p.min_margin(),
            p.holds(res.config.epsilon) as u8
        );
    }
    s
}

/// Escape a string for embedding in JSON.
fn json_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for ch in s.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Build the JSON string: config, summary stats, operating point, and the full
/// grid as an array of point objects.
pub fn to_json(res: &SweepResult, per_map: &[PerMapDetail]) -> String {
    let cfg = &res.config;
    let mut s = String::new();
    s.push_str("{\n");

    // Config block.
    let _ = write!(
        s,
        "  \"config\": {{\n\
         \x20   \"r\": {{\"min\": {}, \"max\": {}, \"steps\": {}}},\n\
         \x20   \"k\": {{\"min\": {}, \"max\": {}, \"steps\": {}}},\n\
         \x20   \"l\": {{\"min\": {}, \"max\": {}, \"steps\": {}}},\n\
         \x20   \"horizon\": {},\n\
         \x20   \"epsilon\": {},\n\
         \x20   \"fixed_undock\": {}, \"fixed_combat_rate\": {}, \"fixed_substeps\": {}\n\
         \x20 }},\n",
        cfg.r_axis.min, cfg.r_axis.max, cfg.r_axis.steps,
        cfg.k_axis.min, cfg.k_axis.max, cfg.k_axis.steps,
        cfg.l_axis.min, cfg.l_axis.max, cfg.l_axis.steps,
        cfg.horizon, cfg.epsilon,
        cfg.fixed_undock, cfg.fixed_combat_rate, cfg.fixed_substeps,
    );

    // Summary.
    let frac = if res.total > 0 {
        res.holds_count as f64 / res.total as f64
    } else {
        0.0
    };
    let _ = write!(
        s,
        "  \"summary\": {{\n\
         \x20   \"total_points\": {},\n\
         \x20   \"holds_count\": {},\n\
         \x20   \"holds_fraction\": {:.6},\n\
         \x20   \"largest_contiguous_region_size\": {},\n\
         \x20   \"region_exists\": {}\n\
         \x20 }},\n",
        res.total,
        res.holds_count,
        frac,
        res.largest_region_size,
        res.largest_region_size > 0,
    );

    // Operating point.
    if let Some(bi) = res.best_idx {
        let p = &res.points[bi];
        let c = &p.cycle;
        let _ = write!(
            s,
            "  \"operating_point\": {{\n\
             \x20   \"r\": {:.4}, \"k\": {:.4}, \"l\": {:.4},\n\
             \x20   \"holds\": {},\n\
             \x20   \"attack_beats_colonize\": {:.6},\n\
             \x20   \"colonize_beats_defend\": {:.6},\n\
             \x20   \"defend_beats_attack\": {:.6},\n\
             \x20   \"min_margin\": {:.6},\n",
            p.r, p.k, p.l,
            p.holds(cfg.epsilon),
            c.attack_beats_colonize, c.colonize_beats_defend, c.defend_beats_attack,
            p.min_margin(),
        );
        s.push_str("    \"per_map\": [\n");
        for (i, d) in per_map.iter().enumerate() {
            let c = &d.cycle;
            let _ = write!(
                s,
                "      {{\"map\": {}, \"attack_beats_colonize\": {:.6}, \
                 \"colonize_beats_defend\": {:.6}, \"defend_beats_attack\": {:.6}}}{}\n",
                json_str(&d.map_name),
                c.attack_beats_colonize, c.colonize_beats_defend, c.defend_beats_attack,
                if i + 1 < per_map.len() { "," } else { "" },
            );
        }
        s.push_str("    ]\n  },\n");
    } else {
        s.push_str("  \"operating_point\": null,\n");
    }

    // Full grid.
    s.push_str("  \"grid\": [\n");
    for (i, p) in res.points.iter().enumerate() {
        let c = &p.cycle;
        let _ = write!(
            s,
            "    {{\"r\": {:.4}, \"k\": {:.4}, \"l\": {:.4}, \
             \"attack_beats_colonize\": {:.6}, \"colonize_beats_defend\": {:.6}, \
             \"defend_beats_attack\": {:.6}, \"min_margin\": {:.6}, \"holds\": {}}}{}\n",
            p.r, p.k, p.l,
            c.attack_beats_colonize, c.colonize_beats_defend, c.defend_beats_attack,
            p.min_margin(), p.holds(cfg.epsilon),
            if i + 1 < res.points.len() { "," } else { "" },
        );
    }
    s.push_str("  ]\n");

    s.push_str("}\n");
    s
}

/// Build the human-readable Markdown report.
pub fn to_markdown(res: &SweepResult, per_map: &[PerMapDetail], map_names: &[String]) -> String {
    let cfg = &res.config;
    let frac = if res.total > 0 {
        res.holds_count as f64 / res.total as f64
    } else {
        0.0
    };
    let region_exists = res.largest_region_size > 0;

    let mut s = String::new();
    let _ = writeln!(s, "# R2 — Cycle Non-Degeneracy: Sweep Results\n");
    let _ = writeln!(
        s,
        "This report answers risk **R2** from `04-open-questions-and-next-steps.md`: \
         does a region of the three-number space `(r, k, l)` exist where the \
         colonize/defend/attack triad is genuine rock-paper-scissors? \
         **\"The cycle holds\"** at a point means all three intended counters win by \
         margin `> epsilon` (mean over all maps and **both seatings**):\n");
    let _ = writeln!(s, "- **attack ≻ colonize**");
    let _ = writeln!(s, "- **colonize ≻ defend**");
    let _ = writeln!(s, "- **defend ≻ attack**\n");

    // Verdict.
    let _ = writeln!(s, "## Verdict\n");
    if region_exists {
        let _ = writeln!(
            s,
            "**A non-degenerate region EXISTS.** The strategic core is viable: \
             go ahead with the triad.\n"
        );
    } else {
        let _ = writeln!(
            s,
            "**NO region where all three counters hold was found in the swept ranges.** \
             Per `04`, this is a stop-and-redesign signal for the triad. See the \
             near-miss analysis below before concluding — widening ranges or \
             rebalancing the policies may be warranted.\n"
        );
    }

    // Headline numbers.
    let _ = writeln!(s, "## Headline numbers\n");
    let _ = writeln!(s, "| Metric | Value |");
    let _ = writeln!(s, "|---|---|");
    let _ = writeln!(s, "| Grid points evaluated | {} |", res.total);
    let _ = writeln!(s, "| Points where cycle holds | {} |", res.holds_count);
    let _ = writeln!(s, "| Fraction holding | {:.1}% ({:.4}) |", frac * 100.0, frac);
    let _ = writeln!(
        s,
        "| Largest contiguous holding region | {} points |",
        res.largest_region_size
    );
    let _ = writeln!(s, "| Maps | {} ({}) |", map_names.len(), map_names.join(", "));
    let _ = writeln!(s, "| Matches per grid point | {} |", 3 * 2 * map_names.len());
    let _ = writeln!(s);

    // Sweep config.
    let _ = writeln!(s, "## Sweep configuration\n");
    let _ = writeln!(s, "```\n{}\n```\n", cfg.describe());

    // Region shape.
    let _ = writeln!(s, "## Shape & extent of the non-degenerate region\n");
    if region_exists {
        let contiguous = res.largest_region_size == res.holds_count;
        let _ = writeln!(
            s,
            "The holding points form {}. The largest **6-connected contiguous** \
             block (neighbors differ by one grid step on a single axis) contains \
             **{} of the {} holders** ({:.0}%).\n",
            if contiguous {
                "a single contiguous region"
            } else {
                "one or more clusters"
            },
            res.largest_region_size,
            res.holds_count,
            if res.holds_count > 0 {
                100.0 * res.largest_region_size as f64 / res.holds_count as f64
            } else {
                0.0
            }
        );
        // Bounding box of the largest region in (r,k,l).
        if !res.largest_region.is_empty() {
            let mut rmin = f64::INFINITY; let mut rmax = f64::NEG_INFINITY;
            let mut kmin = f64::INFINITY; let mut kmax = f64::NEG_INFINITY;
            let mut lmin = f64::INFINITY; let mut lmax = f64::NEG_INFINITY;
            for &i in &res.largest_region {
                let p = &res.points[i];
                rmin = rmin.min(p.r); rmax = rmax.max(p.r);
                kmin = kmin.min(p.k); kmax = kmax.max(p.k);
                lmin = lmin.min(p.l); lmax = lmax.max(p.l);
            }
            let _ = writeln!(
                s,
                "Bounding box of the largest contiguous region:\n\n\
                 - `r` in [{:.3}, {:.3}]\n- `k` in [{:.3}, {:.3}]\n- `l` in [{:.3}, {:.3}]\n",
                rmin, rmax, kmin, kmax, lmin, lmax
            );
            let _ = writeln!(
                s,
                "A fat contiguous block (rather than scattered isolated points) means \
                 the cycle is a robust property of a whole regime, not a knife-edge \
                 coincidence.\n"
            );
        }
    } else {
        let _ = writeln!(s, "No holding region in the swept ranges (see near-miss below).\n");
    }

    // Operating point.
    let _ = writeln!(s, "## Recommended operating point\n");
    if let Some(bi) = res.best_idx {
        let p = &res.points[bi];
        let c = &p.cycle;
        if p.holds(cfg.epsilon) {
            let _ = writeln!(
                s,
                "The most **robust** holding point: it maximizes the *weakest* of the \
                 three edges (furthest from any counter failing), with a small \
                 tie-break favoring points whose grid-neighbors also hold (so the \
                 choice sits in the interior of the region, not on its rim):\n"
            );
        } else {
            let _ = writeln!(
                s,
                "No point holds; the **closest** point (largest minimum margin) is \
                 reported instead:\n"
            );
        }
        let _ = writeln!(s, "| Param | Value |");
        let _ = writeln!(s, "|---|---|");
        let _ = writeln!(s, "| colonization rate `r` | **{:.3}** |", p.r);
        let _ = writeln!(s, "| defender advantage `k` | **{:.3}** |", p.k);
        let _ = writeln!(s, "| attack-commitment loss `l` | **{:.3}** |", p.l);
        let _ = writeln!(s);
        let _ = writeln!(s, "Cycle-edge margins at this point (mean over maps + both seatings):\n");
        let _ = writeln!(s, "| Edge | Margin | Wins? |");
        let _ = writeln!(s, "|---|---|---|");
        let _ = writeln!(
            s, "| attack ≻ colonize | {:+.4} | {} |",
            c.attack_beats_colonize, yn(c.attack_beats_colonize > cfg.epsilon)
        );
        let _ = writeln!(
            s, "| colonize ≻ defend | {:+.4} | {} |",
            c.colonize_beats_defend, yn(c.colonize_beats_defend > cfg.epsilon)
        );
        let _ = writeln!(
            s, "| defend ≻ attack | {:+.4} | {} |",
            c.defend_beats_attack, yn(c.defend_beats_attack > cfg.epsilon)
        );
        let _ = writeln!(s, "\nWeakest edge (cycle margin): **{:+.4}** (epsilon = {:.3}).\n", p.min_margin(), cfg.epsilon);

        // Per-map breakdown at the operating point.
        let _ = writeln!(s, "### Per-map detail at the operating point\n");
        let _ = writeln!(s, "| Map | attack≻colonize | colonize≻defend | defend≻attack | all hold? |");
        let _ = writeln!(s, "|---|---|---|---|---|");
        for d in per_map {
            let c = &d.cycle;
            let all = c.attack_beats_colonize > cfg.epsilon
                && c.colonize_beats_defend > cfg.epsilon
                && c.defend_beats_attack > cfg.epsilon;
            let _ = writeln!(
                s, "| {} | {:+.4} | {:+.4} | {:+.4} | {} |",
                d.map_name, c.attack_beats_colonize, c.colonize_beats_defend,
                c.defend_beats_attack, yn(all)
            );
        }
        let _ = writeln!(s);
    } else {
        let _ = writeln!(s, "No points evaluated.\n");
    }

    // Near-miss / diagnostics: which edge fails most often.
    let _ = writeln!(s, "## Diagnostics — which edge is the binding constraint?\n");
    let mut fail_ac = 0usize;
    let mut fail_cd = 0usize;
    let mut fail_da = 0usize;
    for p in &res.points {
        let c = &p.cycle;
        if c.attack_beats_colonize <= cfg.epsilon { fail_ac += 1; }
        if c.colonize_beats_defend <= cfg.epsilon { fail_cd += 1; }
        if c.defend_beats_attack <= cfg.epsilon { fail_da += 1; }
    }
    let _ = writeln!(
        s,
        "Across all {} points, how often each edge fails to clear epsilon \
         (the most-failing edge is the one limiting the region):\n",
        res.total
    );
    let _ = writeln!(s, "| Edge | Points failing | % |");
    let _ = writeln!(s, "|---|---|---|");
    let pc = |x: usize| if res.total > 0 { 100.0 * x as f64 / res.total as f64 } else { 0.0 };
    let _ = writeln!(s, "| attack ≻ colonize | {} | {:.0}% |", fail_ac, pc(fail_ac));
    let _ = writeln!(s, "| colonize ≻ defend | {} | {:.0}% |", fail_cd, pc(fail_cd));
    let _ = writeln!(s, "| defend ≻ attack | {} | {:.0}% |", fail_da, pc(fail_da));
    let _ = writeln!(s);

    // Per-axis hold-rate: shows the actual *band* the cycle lives in along each of
    // the three numbers (more informative than the contiguous region's bounding
    // box, which can span the whole grid when the region is large but irregular).
    let _ = writeln!(
        s,
        "### Where the cycle lives along each axis\n\nHold-rate per axis value \
         (fraction of points at that value, holding over the *other* two axes). \
         This is the concrete shape of the non-degenerate region:\n"
    );
    write_axis_holdrate(&mut s, res, "colonization rate `r`", |p| p.r, cfg.epsilon);
    write_axis_holdrate(&mut s, res, "defender advantage `k`", |p| p.k, cfg.epsilon);
    write_axis_holdrate(&mut s, res, "attack-commitment loss `l`", |p| p.l, cfg.epsilon);

    // How to reproduce.
    let _ = writeln!(s, "## How to reproduce\n");
    let _ = writeln!(
        s,
        "```\ncargo run -p r2-sweep --release\n```\n\n\
         Writes `R2_RESULTS.md` (this file), `r2_results.csv`, and `r2_results.json` \
         at the repo root. The engine is fully deterministic (no RNG), so reruns are \
         bit-identical.\n"
    );

    let _ = writeln!(s, "---\n");
    let _ = writeln!(
        s,
        "*Generated by `r2-sweep`. Margins are score units in [-1, 1] where score = \
         0.5·(unit share) + 0.5·(territory share), averaged over both seatings.*"
    );

    s
}

fn yn(b: bool) -> &'static str {
    if b { "yes" } else { "no" }
}

/// Write a small table of hold-rate grouped by the distinct values of one axis
/// (extracted by `key`). Values are bucketed by rounding to 4 decimals so the grid
/// points group cleanly. Shows, for each axis value, how many points hold out of
/// the total at that value — i.e. the cycle's extent along that axis.
fn write_axis_holdrate(
    s: &mut String,
    res: &SweepResult,
    label: &str,
    key: impl Fn(&crate::sweep::GridPoint) -> f64,
    epsilon: f64,
) {
    // Collect distinct axis values in sorted order with (holds, total) counts.
    let mut buckets: Vec<(f64, usize, usize)> = Vec::new();
    let round = |x: f64| (x * 1e4).round() / 1e4;
    for p in &res.points {
        let v = round(key(p));
        let holds = p.holds(epsilon) as usize;
        if let Some(b) = buckets.iter_mut().find(|b| (b.0 - v).abs() < 1e-9) {
            b.1 += holds;
            b.2 += 1;
        } else {
            buckets.push((v, holds, 1));
        }
    }
    buckets.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));

    let _ = writeln!(s, "\n**{}**\n", label);
    let _ = writeln!(s, "| value | holds / total | rate |");
    let _ = writeln!(s, "|---|---|---|");
    for (v, h, t) in buckets {
        let rate = if t > 0 { 100.0 * h as f64 / t as f64 } else { 0.0 };
        let _ = writeln!(s, "| {:.3} | {} / {} | {:.0}% |", v, h, t, rate);
    }
    let _ = writeln!(s);
}
