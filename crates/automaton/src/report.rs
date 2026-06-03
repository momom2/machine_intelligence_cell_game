//! Report writers for the arc-1 capstone validation: human-readable Markdown plus
//! machine-readable CSV and JSON.
//!
//! These are **library** functions (not buried in the binary) for two reasons: (1) the
//! rendering is then unit-testable, and (2) the validation artifact can be regenerated
//! from anywhere that can build the lib. No serde — the JSON is small and emitted by hand,
//! keeping the crate dependency-free and bit-reproducible.

use std::fmt::Write as _;

use cell_core::Params;

use crate::capstone::{LadderEvaluation, SimplexStudy};

/// All the context the report needs beyond the measured results: the operating point and
/// match budget the validation ran at. Mirrors the R2 report's self-documenting header.
#[derive(Debug, Clone, Copy)]
pub struct ReportContext {
    pub params: Params,
    pub horizon: u64,
    pub scout_ticks: u64,
    pub n_maps: usize,
    pub grid_steps: u32,
}

/// Threshold below which a correlation is read as "no relationship" rather than a
/// direction. Centrality-vs-difficulty correlations inside `[-WEAK, WEAK]` are reported as
/// *not supporting* the design's monotone claim.
pub const WEAK_CORR: f64 = 0.15;

fn mean<F: Fn(&crate::capstone::RungReport) -> f64>(eval: &LadderEvaluation, f: F) -> f64 {
    if eval.rungs.is_empty() {
        return 0.0;
    }
    eval.rungs.iter().map(|r| f(r)).sum::<f64>() / eval.rungs.len() as f64
}

/// Render the full Markdown report.
pub fn to_markdown(eval: &LadderEvaluation, study: &SimplexStudy, ctx: &ReportContext) -> String {
    let mut s = String::new();
    let p = &ctx.params;
    let matches_per_rung = ctx.n_maps * 2;

    let mean_cap_sc = mean(eval, |r| r.capstone_score);
    let mean_base_sc = mean(eval, |r| r.baseline_score);
    let mean_orc_sc = mean(eval, |r| r.oracle_score);

    // Verdicts.
    let infer_works = eval.mean_capstone_winrate > eval.mean_baseline_winrate
        && mean_cap_sc > mean_base_sc;
    // The design predicts difficulty *rises* with centrality, i.e. a clearly POSITIVE
    // correlation. We test the monotone claim on the dense grid (Spearman), which is the
    // statistically sound sampling; the curated ladder is corroborating.
    let centrality_supported = study.read_difficulty_rho > WEAK_CORR
        || study.combined_difficulty_rho > WEAK_CORR;

    s.push_str("# CAPSTONE_RESULTS — arc-1 capstone (scout → infer → counter → execute)\n\n");
    s.push_str(
        "> Build-order **step 4** deliverable. Headless validation of the arc-1 capstone \
         from `02-ai-opponents.md`: a ladder of **hidden-but-fixed** Automatons spanning the \
         colonize/defend/attack simplex, and a player routine that **scouts** an opponent, \
         **infers** its hidden mix from the legible features (`01`/`DSL_SPEC.md`), selects the \
         rock-paper-scissors **counter**, and **executes** it. Everything is built on the \
         deterministic mean-field engine + the shared rule DSL from `cell-core`, so every \
         number here is bit-reproducible (no RNG).\n\n",
    );

    // ----- Verdict -----
    s.push_str("## Verdict\n\n");
    let _ = writeln!(
        s,
        "1. **Does infer-then-counter beat a fixed non-inferring baseline?** **{}**",
        if infer_works { "YES" } else { "NO" }
    );
    let _ = writeln!(
        s,
        "   - Capstone mean win-rate **{:.3}** vs. baseline **{:.3}** (Δ = {:+.3}); mean score \
         margin **{:+.3}** vs. **{:+.3}** (Δ = {:+.3}).",
        eval.mean_capstone_winrate,
        eval.mean_baseline_winrate,
        eval.mean_capstone_winrate - eval.mean_baseline_winrate,
        mean_cap_sc,
        mean_base_sc,
        mean_cap_sc - mean_base_sc,
    );
    let _ = writeln!(
        s,
        "2. **Does difficulty rise with simplex centrality, as the design predicts?** **{}**",
        if centrality_supported { "YES" } else { "NO — not supported by measurement" }
    );
    let _ = writeln!(
        s,
        "   - Dense-grid Spearman rank correlation of centrality vs. *read*-difficulty = \
         **{:+.3}**, vs. *counter*-difficulty = **{:+.3}**, vs. *combined* capstone difficulty = \
         **{:+.3}** ({} grid mixes). A positive value would mean central mixes are harder; \
         the measured values are {}.",
        study.read_difficulty_rho,
        study.counter_difficulty_rho,
        study.combined_difficulty_rho,
        study.n_points,
        if centrality_supported { "positive" } else { "≈0 or negative" },
    );
    s.push('\n');
    s.push_str(
        "**Bottom line.** The capstone *mechanism* works: reading the opponent and committing \
         to the matched counter is worth a real, repeatable edge over the same machinery that \
         skips the read. The design's *a-priori difficulty metric*, however, is **not** \
         validated: on this engine and feature set, difficulty is governed by **which axis the \
         mix loads on** — defend is the least legible — not by distance-from-pure. This is \
         exactly the kind of empirically-decided question `04` flags (\"cannot be proven up \
         front … must be measured\"), and the measurement comes back negative for centrality.\n\n",
    );

    // ----- Setup -----
    s.push_str("## Setup\n\n");
    let _ = writeln!(
        s,
        "- **Operating point** (R2's robust point): `r = {:.3}`, `k = {:.3}`, `l = {:.3}`; \
         `undock_delay = {:.0}`, `combat_rate = {:.2}`, `combat_substeps = {}`.",
        p.r, p.k, p.l, p.undock_delay, p.combat_rate, p.combat_substeps
    );
    let _ = writeln!(
        s,
        "- **Maps**: {} symmetric maps (corridor7, twin_bridge8, star9), **both seatings** → \
         {} matches per rung. Horizon {} ticks. Scout window **{} ticks**, then commit.",
        ctx.n_maps, matches_per_rung, ctx.horizon, ctx.scout_ticks
    );
    let _ = writeln!(
        s,
        "- **Ladder**: {} rungs, ordered by increasing **centrality** (distance-from-pure, \
         normalized so a pure corner = 0 and the balanced centre = 1).",
        eval.rungs.len()
    );
    s.push_str(
        "- **Players, all sharing one machinery** (scout-the-same-window, then commit):\n\
         \u{20}  - **Capstone** — commits to the counter of the *inferred* mix.\n\
         \u{20}  - **Baseline** (control) — commits to a **fixed balanced** mix, ignoring the \
         observation. Any gap vs. Capstone is attributable to *reading the opponent*.\n\
         \u{20}  - **Oracle** (perfect *read*) — commits to the counter of the opponent's \
         **true** mix. It isolates reading quality, but note it is **not** an absolute \
         performance ceiling: the counter *rotation* is the theoretical RPS response, which \
         is optimal at the pure corners but only approximate for blends — so on some blended \
         rungs the capstone's (mis-)read lands on a *practically stronger* response than the \
         exact rotation, and beats the oracle. See the note below the headline table.\n\n",
    );

    // ----- Headline numbers -----
    s.push_str("## Headline numbers\n\n");
    s.push_str("| Metric | Value |\n|---|---|\n");
    let _ = writeln!(s, "| Capstone mean win-rate | **{:.3}** |", eval.mean_capstone_winrate);
    let _ = writeln!(s, "| Baseline mean win-rate | {:.3} |", eval.mean_baseline_winrate);
    let _ = writeln!(s, "| Oracle mean win-rate (ceiling) | {:.3} |", eval.mean_oracle_winrate);
    let _ = writeln!(s, "| Capstone mean score margin | **{:+.3}** |", mean_cap_sc);
    let _ = writeln!(s, "| Baseline mean score margin | {:+.3} |", mean_base_sc);
    let _ = writeln!(s, "| Oracle mean score margin | {:+.3} |", mean_orc_sc);
    let _ = writeln!(
        s,
        "| Mean inference error (simplex distance) | {:.3} |",
        eval.mean_inference_error
    );
    let _ = writeln!(
        s,
        "| Dominant-lean classification accuracy | {:.3} |",
        eval.corner_accuracy
    );
    s.push('\n');
    if mean_orc_sc < mean_cap_sc {
        s.push_str(
            "> **On the Oracle scoring *below* the Capstone here:** the Oracle plays the exact \
             RPS *rotation* of the true mix, which is optimal only at the pure corners. For \
             *blended* opponents the rotation is merely approximate, so the capstone's read — \
             which leans toward attack/colonize, the strategies that happen to be strongest on \
             these maps — sometimes lands on a practically better response than the exact \
             rotation. So the Oracle isolates *reading* quality, but is not an absolute ceiling; \
             improving the counter *selection* for blends (beyond a fixed rotation) is the clear \
             next lever, and is what the arc-2 Counter opponent will need anyway.\n\n",
        );
    }

    // ----- Per-rung table -----
    s.push_str("## Per-rung results (ladder ordered easiest → hardest by centrality)\n\n");
    s.push_str(
        "`cen` = centrality (0 = pure corner, 1 = balanced centre). `inf` = mean inferred mix; \
         `infErr` = its simplex distance to the truth; `corner?` = was the dominant lean read \
         correctly. Win-rates and score margins are over all maps × both seatings.\n\n",
    );
    s.push_str(
        "| # | mix | cen | true (c,d,a) | inferred (c,d,a) | infErr | corner? | cap WR | base WR | orc WR | cap sc | base sc | orc sc |\n",
    );
    s.push_str(
        "|---|---|---|---|---|---|---|---|---|---|---|---|---|\n",
    );
    for r in &eval.rungs {
        let _ = writeln!(
            s,
            "| {} | {} | {:.2} | {:.2},{:.2},{:.2} | {:.2},{:.2},{:.2} | {:.2} | {} | {:.2} | {:.2} | {:.2} | {:+.2} | {:+.2} | {:+.2} |",
            r.index,
            r.label,
            r.centrality,
            r.true_mix.c, r.true_mix.d, r.true_mix.a,
            r.inferred_mix.c, r.inferred_mix.d, r.inferred_mix.a,
            r.inference_error,
            if r.corner_correct { "Y" } else { "·" },
            r.capstone_winrate, r.baseline_winrate, r.oracle_winrate,
            r.capstone_score, r.baseline_score, r.oracle_score,
        );
    }
    s.push('\n');
    s.push_str(
        "Read the gap between **cap** and **base**: on the rungs where the baseline's fixed \
         balanced mix *loses* (negative `base sc` — e.g. the colonize/defend blends `C5D2`, \
         `C4D2`, `C3D3`), the inferring capstone *wins*. That is precisely the value of \
         reading the opponent, and it is concentrated exactly where a fixed hedge is wrong.\n\n",
    );

    // ----- Centrality study -----
    s.push_str("## Does centrality predict difficulty? (the dense-grid study)\n\n");
    s.push_str(
        "The 16-rung ladder is a curated *campaign* path; its small size lets per-matchup \
         idiosyncrasy dominate a correlation. The statistically sound test samples the simplex \
         **uniformly** and densely and correlates centrality against difficulty over all of it. \
         The design's claim is *monotone* (\"more central ⇒ harder\"), so the headline is the \
         **Spearman rank** correlation; Pearson is shown for reference.\n\n",
    );
    let _ = writeln!(
        s,
        "Grid: **{} mixes** at 1/{} resolution (every corner, edge, and interior point), same \
         maps × seatings.\n",
        study.n_points, ctx.grid_steps
    );
    s.push_str("| Difficulty channel | Spearman ρ | Pearson r | Interpretation |\n|---|---|---|---|\n");
    let _ = writeln!(
        s,
        "| **read** (inference error) | {:+.3} | {:+.3} | {} |",
        study.read_difficulty_rho,
        study.read_difficulty_pearson,
        verdict_phrase(study.read_difficulty_rho),
    );
    let _ = writeln!(
        s,
        "| **counter** (oracle score-difficulty) | {:+.3} | {:+.3} | {} |",
        study.counter_difficulty_rho,
        study.counter_difficulty_pearson,
        verdict_phrase(study.counter_difficulty_rho),
    );
    let _ = writeln!(
        s,
        "| **combined** (capstone score-difficulty) | {:+.3} | {:+.3} | {} |",
        study.combined_difficulty_rho,
        study.combined_difficulty_pearson,
        verdict_phrase(study.combined_difficulty_rho),
    );
    s.push('\n');

    // Band table — the concrete shape.
    s.push_str(
        "Difficulty by centrality band (the concrete shape — if the design held, every column \
         would rise top-to-bottom):\n\n",
    );
    s.push_str("| centrality band | mean read err | mean counter diff | mean combined diff | n |\n|---|---|---|---|---|\n");
    for (c, re, cd, md, n) in &study.bands {
        let _ = writeln!(s, "| ~{:.1} | {:.3} | {:.3} | {:.3} | {} |", c, re, cd, md, n);
    }
    s.push('\n');

    // ----- Mechanism / why -----
    s.push_str("## Why centrality fails — and what *does* drive difficulty\n\n");
    s.push_str(
        "The read-difficulty column actually **falls** toward the centre. The cause is visible \
         in the per-rung `inferred` column: the **defend** weight is systematically \
         under-read (a heavy turtle's `d` is recovered only partially; a defend+attack mix \
         reads as almost pure attack). Defend is intrinsically the hardest axis to see from \
         these observables, because:\n\n\
         - a defender's signature act (reinforce) only fires when *threatened*, so a neutral \
           scout barely provokes it inside the short window;\n\
         - every Automaton runs the shared *expand* economy, so a turtle still keeps ships \
           moving — its mobility is not as low as a pure-defend caricature would be;\n\
         - the cleanest defend tell (a stocked **rear/interior garrison**) was implemented and \
           tested as a second signal, but on these 7–9-node maps it *washed out* the cleaner \
           mobility gap and mis-read pure defenders as colonizers, so it is excluded from the \
           score (and documented in `infer.rs`). The honest conclusion is that defend is the \
           least legible axis here.\n\n\
         Because the ladder/grid place defend-heavy mixes across the whole centrality range \
         (not concentrated at the centre), the defend-legibility penalty does **not** line up \
         with centrality — so centrality does not predict difficulty. Difficulty is better \
         predicted by *defend content* than by *distance-from-pure*.\n\n\
         This does not weaken the capstone as a game mechanism — inference+counter still pays \
         off — but it means an honest difficulty ramp for arc 1 should be ordered by \
         **measured** win-rate (or by defend content), not by the geometric centrality metric \
         the design assumed up front.\n\n",
    );

    // ----- Reproduce -----
    s.push_str("## How to reproduce\n\n");
    s.push_str("```\ncargo run -p automaton --bin capstone --release\n```\n\n");
    s.push_str(
        "Writes `CAPSTONE_RESULTS.md` (this file), `capstone_results.csv`, and \
         `capstone_results.json` at the repo root. The engine and both players are \
         deterministic (no RNG), so reruns are bit-identical. The asserted regression tests \
         live in `crates/automaton` (`cargo test -p automaton`); the per-rung table and the \
         grid study are also printed by the ignored diagnostics \
         `cargo test -p automaton diag_results -- --ignored --nocapture` and \
         `… diag_inference …`.\n\n",
    );
    s.push_str("---\n\n");
    s.push_str(
        "*Score margin is in [-1, 1] = A's combined (unit + territory) share minus B's, \
         averaged over both seatings; a win is a strictly positive end score (eliminations \
         score ±1). Centrality, inference error, and the correlations are defined in \
         `crates/automaton/src/{mix,infer,capstone}.rs`.*\n",
    );

    s
}

/// A short phrase classifying a correlation against the design's *positive* prediction.
fn verdict_phrase(rho: f64) -> &'static str {
    if rho > WEAK_CORR {
        "rises with centrality (supports design)"
    } else if rho < -WEAK_CORR {
        "**falls** with centrality (contradicts design)"
    } else {
        "no relationship (does not support design)"
    }
}

/// CSV: one row per ladder rung with the full per-rung numbers. Header documents columns.
pub fn to_csv(eval: &LadderEvaluation) -> String {
    let mut s = String::new();
    s.push_str(
        "index,label,centrality,true_c,true_d,true_a,inf_c,inf_d,inf_a,inference_error,corner_correct,\
         capstone_winrate,baseline_winrate,oracle_winrate,capstone_score,baseline_score,oracle_score\n",
    );
    for r in &eval.rungs {
        let _ = writeln!(
            s,
            "{},{},{:.6},{:.6},{:.6},{:.6},{:.6},{:.6},{:.6},{:.6},{},{:.6},{:.6},{:.6},{:.6},{:.6},{:.6}",
            r.index,
            r.label,
            r.centrality,
            r.true_mix.c, r.true_mix.d, r.true_mix.a,
            r.inferred_mix.c, r.inferred_mix.d, r.inferred_mix.a,
            r.inference_error,
            r.corner_correct as u8,
            r.capstone_winrate, r.baseline_winrate, r.oracle_winrate,
            r.capstone_score, r.baseline_score, r.oracle_score,
        );
    }
    s
}

/// JSON: summary + per-rung array + the grid study. Hand-emitted (no serde).
pub fn to_json(eval: &LadderEvaluation, study: &SimplexStudy, ctx: &ReportContext) -> String {
    let p = &ctx.params;
    let mut s = String::new();
    s.push_str("{\n");

    let _ = write!(
        s,
        "  \"context\": {{ \"r\": {:.4}, \"k\": {:.4}, \"l\": {:.4}, \"horizon\": {}, \
         \"scout_ticks\": {}, \"n_maps\": {}, \"grid_steps\": {} }},\n",
        p.r, p.k, p.l, ctx.horizon, ctx.scout_ticks, ctx.n_maps, ctx.grid_steps
    );

    let _ = write!(
        s,
        "  \"summary\": {{ \"mean_capstone_winrate\": {:.6}, \"mean_baseline_winrate\": {:.6}, \
         \"mean_oracle_winrate\": {:.6}, \"mean_inference_error\": {:.6}, \"corner_accuracy\": {:.6}, \
         \"centrality_vs_inference_error_corr\": {:.6}, \"centrality_vs_oracle_difficulty_corr\": {:.6}, \
         \"centrality_vs_difficulty_corr\": {:.6}, \"centrality_vs_difficulty_corr_baseline\": {:.6} }},\n",
        eval.mean_capstone_winrate,
        eval.mean_baseline_winrate,
        eval.mean_oracle_winrate,
        eval.mean_inference_error,
        eval.corner_accuracy,
        eval.centrality_vs_inference_error_corr,
        eval.centrality_vs_oracle_difficulty_corr,
        eval.centrality_vs_difficulty_corr,
        eval.centrality_vs_difficulty_corr_baseline,
    );

    let _ = write!(
        s,
        "  \"grid_study\": {{ \"n_points\": {}, \"read_rho\": {:.6}, \"counter_rho\": {:.6}, \
         \"combined_rho\": {:.6}, \"read_pearson\": {:.6}, \"counter_pearson\": {:.6}, \
         \"combined_pearson\": {:.6}, \"mean_inference_error\": {:.6} }},\n",
        study.n_points,
        study.read_difficulty_rho,
        study.counter_difficulty_rho,
        study.combined_difficulty_rho,
        study.read_difficulty_pearson,
        study.counter_difficulty_pearson,
        study.combined_difficulty_pearson,
        study.mean_inference_error,
    );

    s.push_str("  \"rungs\": [\n");
    for (i, r) in eval.rungs.iter().enumerate() {
        let comma = if i + 1 < eval.rungs.len() { "," } else { "" };
        let _ = write!(
            s,
            "    {{ \"index\": {}, \"label\": {}, \"centrality\": {:.6}, \
             \"true\": [{:.6}, {:.6}, {:.6}], \"inferred\": [{:.6}, {:.6}, {:.6}], \
             \"inference_error\": {:.6}, \"corner_correct\": {}, \
             \"capstone_winrate\": {:.6}, \"baseline_winrate\": {:.6}, \"oracle_winrate\": {:.6}, \
             \"capstone_score\": {:.6}, \"baseline_score\": {:.6}, \"oracle_score\": {:.6} }}{}\n",
            r.index,
            json_str(&r.label),
            r.centrality,
            r.true_mix.c, r.true_mix.d, r.true_mix.a,
            r.inferred_mix.c, r.inferred_mix.d, r.inferred_mix.a,
            r.inference_error,
            r.corner_correct,
            r.capstone_winrate, r.baseline_winrate, r.oracle_winrate,
            r.capstone_score, r.baseline_score, r.oracle_score,
            comma,
        );
    }
    s.push_str("  ]\n}\n");
    s
}

/// Minimal JSON string escaping.
fn json_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for ch in s.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}
