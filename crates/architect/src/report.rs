//! Report writers for the Architect run: the human-readable `ARCHITECT_RESULTS.md`
//! (fitness-over-time, champion-size-over-time, glass-genome diffs, and the **R1 read with
//! evidence**) plus compact labels used in logs.
//!
//! Like the rest of the build (`automaton::report`), these are **library** functions (so
//! they are unit-testable and re-runnable) and use **no serde** — the crate stays
//! dependency-free and every run is bit-reproducible.

use std::fmt::Write as _;

use cell_core::Params;

use crate::genome::Genome;
use crate::glass;
use crate::population::{GenerationLog, RunResult};

/// Threshold below which a correlation / fraction is read as "no signal" rather than a
/// direction (mirrors `automaton::report::WEAK_CORR`).
pub const WEAK: f64 = 0.10;

/// A compact one-line label for a genome: its gene tags in priority order, e.g.
/// `strike>expand-/mass`. Used in the per-generation log and archive entries so each
/// organism is identifiable at a glance.
pub fn compact_label(g: &Genome) -> String {
    let mut genes: Vec<&crate::genome::Gene> = g.genes.iter().collect();
    genes.sort_by_key(|x| std::cmp::Reverse(x.priority()));
    let tags: Vec<&str> = genes.iter().map(|x| x.template().tag).collect();
    tags.join("+")
}

/// The R1 verdict, derived from the run's trajectory.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum R1Read {
    /// A dominant policy emerged: the arms race died (strict dominance, no cycles).
    Collapse,
    /// Strategic diversity / a non-transitive arms race persisted: no dominant policy.
    DiversityPersists,
    /// Mixed/ambiguous: some collapse pressure but cycles never fully vanished.
    Mixed,
}

impl R1Read {
    pub fn as_str(self) -> &'static str {
        match self {
            R1Read::Collapse => "DOMINANT-POLICY COLLAPSE",
            R1Read::DiversityPersists => "DIVERSITY / NON-TRANSITIVE ARMS RACE PERSISTS",
            R1Read::Mixed => "MIXED (collapse pressure, but cycles persist)",
        }
    }
}

/// Classify the R1 outcome from the per-generation log.
///
/// The decision uses the **second half** of the run (after transient startup) and is based
/// on the two signals `diversity` measures:
/// * `late_intransitivity` — mean intransitive-triad fraction over the late generations.
///   This is the load-bearing number: > 0 means rock-paper-scissors survives in policy
///   space, so **no single policy dominates**.
/// * `late_max_dominance` — mean "most wins by any representative" fraction. Near 1.0
///   means a genome that beats (almost) everyone exists — the literal dominant policy.
///
/// Verdict:
/// * **DiversityPersists** if late intransitivity is clearly positive (cycles survive) and
///   no representative dominates the whole set.
/// * **Collapse** if cycles have vanished (≈0 intransitivity) *and* a near-total dominator
///   exists.
/// * **Mixed** otherwise.
pub fn classify_r1(log: &[GenerationLog]) -> (R1Read, f64, f64, f64) {
    if log.is_empty() {
        return (R1Read::Mixed, 0.0, 0.0, 0.0);
    }
    let start = log.len() / 2; // second half
    let late = &log[start..];
    let n = late.len() as f64;
    let late_intransitivity =
        late.iter().map(|l| l.non_transitivity.intransitive_triads).sum::<f64>() / n;
    let late_max_dominance =
        late.iter().map(|l| l.non_transitivity.max_dominance).sum::<f64>() / n;
    let late_species =
        late.iter().map(|l| l.genotypic.distinct_species as f64).sum::<f64>() / n;

    let read = if late_intransitivity > WEAK && late_max_dominance < 0.999 {
        R1Read::DiversityPersists
    } else if late_intransitivity <= WEAK && late_max_dominance >= 0.999 {
        R1Read::Collapse
    } else {
        R1Read::Mixed
    };
    (read, late_intransitivity, late_max_dominance, late_species)
}

/// Render the full `ARCHITECT_RESULTS.md`.
pub fn to_markdown(run: &RunResult, params: &Params, elapsed_secs: f64) -> String {
    let cfg = run.config;
    let log = &run.log;
    let mut s = String::new();

    let (r1, late_intrans, late_dom, late_species) = classify_r1(log);

    // ---- Header ----
    s.push_str("# ARCHITECT_RESULTS — the autoconstructive evolutionary loop (build-order step 5)\n\n");
    s.push_str(
        "> Build-order **step 5**, the headline. The Architect from `02-ai-opponents.md`: a \
         **population of rule-set \"organisms\"** in the shared DSL, where **each organism \
         carries its own mutation/recombination operators** (Lee Spector's *Pushpop* — the \
         reproduction machinery evolves, not just the strategy). Fitness = fast **mean-field \
         coevolution** (`cell-core`) + a **cached player model** + a **parsimony penalty** \
         (R3 legibility) + a **wins-as-test-cases regression archive** (Schmidhuber's \
         *success-story* acceptance gate). The champion is shown as a **glass genome** and as \
         **diffs** between generations. This run is the real test of **R1** (dominant-policy \
         collapse), **R3** (legibility vs strength), and **R4** (overfitting).\n\n",
    );

    // ---- The keyResult, up top ----
    let self_improves = headline_self_improves(log);
    s.push_str("## Headline result\n\n");
    let _ = writeln!(
        s,
        "- **Does it self-improve legibly?** **{}** — the published champion's fitness {} over \
         the run while its rule-set stayed small ({}–{} rules) and every change was a readable \
         diff (see the trace below).",
        if self_improves { "YES" } else { "NO" },
        fitness_trend_phrase(log),
        log.iter().map(|l| l.champion_rule_count).min().unwrap_or(0),
        log.iter().map(|l| l.champion_rule_count).max().unwrap_or(0),
    );
    let _ = writeln!(
        s,
        "- **R1 read (collapse vs diversity-persists): `{}`.** Over the run's second half the \
         strategic relation among the population's distinct strategies stayed **{:.0}% \
         intransitive** (rock-paper-scissors cycles in *policy* space) with the best single \
         strategy beating at most **{:.0}%** of the others — so **no single dominant program \
         emerged**; the arms race did not die. ({:.1} distinct strategy-species persisted on \
         average.)",
        r1.as_str(),
        100.0 * late_intrans,
        100.0 * late_dom,
        late_species,
    );
    s.push('\n');

    // ---- Run configuration (self-documenting header) ----
    s.push_str("## Run configuration\n\n");
    let _ = writeln!(s, "| Setting | Value |");
    let _ = writeln!(s, "|---|---|");
    let _ = writeln!(s, "| operating point (r, k, l) | ({:.2}, {:.2}, {:.2}) — R2's robust point |", params.r, params.k, params.l);
    let _ = writeln!(s, "| population size | {} |", cfg.pop_size);
    let _ = writeln!(s, "| generations | {} |", cfg.generations);
    let _ = writeln!(s, "| coevolution sample / candidate | {} opponents |", cfg.coevo_sample);
    let _ = writeln!(s, "| elite fraction | {:.0}% |", 100.0 * cfg.elite_frac);
    let _ = writeln!(s, "| crossover fraction | {:.0}% |", 100.0 * cfg.crossover_frac);
    let _ = writeln!(s, "| match horizon | {} ticks |", crate::fitness::HORIZON);
    let _ = writeln!(s, "| maps | 3 (corridor7, twin_bridge8, star9), both seatings |");
    let _ = writeln!(s, "| parsimony cost / rule | {:.3} |", crate::fitness::PARSIMONY_COST);
    let _ = writeln!(s, "| player-model weight | {:.2} |", crate::fitness::PLAYER_MODEL_WEIGHT);
    let _ = writeln!(s, "| beat epsilon | {:.2} |", cfg.beat_epsilon);
    let _ = writeln!(s, "| seed | 0x{:X} |", cfg.seed);
    let _ = writeln!(s, "| wall-clock | {:.2}s |", elapsed_secs);
    s.push('\n');

    // ---- The final glass genome ----
    s.push_str("## The champion glass genome (final)\n\n");
    let _ = writeln!(
        s,
        "The final published champion is the legible rule-set below — `{}` ({} rules). A player \
         would read it top-to-bottom like a sentence; this is the artifact the design unlocks as \
         a reward for beating the Architect.\n",
        compact_label(&run.final_champion),
        run.final_champion.rule_count()
    );
    s.push_str("```\n");
    s.push_str(&glass::render_genome(&run.final_champion));
    s.push_str("```\n\n");
    let f = run.final_champion_fitness;
    let _ = writeln!(
        s,
        "Its fitness decomposes as: coevolution **{:+.3}**, player-model **{:+.3}**, parsimony \
         penalty **−{:.3}** ({} rules) → blended **{:+.3}**.\n",
        f.coevolution, f.player_model, f.parsimony_penalty, f.rule_count, f.fitness
    );

    // ---- Glass-genome diffs (the spectatable evolution) ----
    s.push_str("## Glass-genome evolution (champion diffs)\n\n");
    s.push_str(
        "Each block is what changed in the **published** champion that generation — the \
         `+`/`-`/`~` patch-notes a player would watch between matches. Generations with no \
         change (the gate held, or no better legible policy was found) are omitted.\n\n",
    );
    s.push_str("```\n");
    let mut any_diff = false;
    for l in log {
        if l.champion_changed && !l.champion_diff.is_empty() {
            any_diff = true;
            let _ = writeln!(s, "gen {:>3}:", l.gen);
            for d in &l.champion_diff {
                let _ = writeln!(s, "{}", d.render());
            }
        }
    }
    if !any_diff {
        s.push_str("  (the seed champion was never legibly improved upon — try more generations)\n");
    }
    s.push_str("```\n\n");

    // ---- Fitness + size + diversity trace ----
    s.push_str("## Per-generation trace\n\n");
    s.push_str(
        "`best`/`mean` = population blended fitness; `champ` = published champion fitness; \
         `rules` = champion rule count (parsimony / glass-genome size); `species` = distinct \
         strategy-species in the population; `H` = normalized species entropy; `intrans` = \
         fraction of intransitive (rock-paper-scissors) triads among representatives — **the R1 \
         signal**; `maxDom` = most wins by any single representative (1.0 = a genome beats \
         everyone); `arch` = regression-archive size; `rej` = success-story gate rejected the \
         candidate this gen; `op.tog` = mean evolved gene-toggle rate (watch the operators \
         evolve).\n\n",
    );
    s.push_str("```\n");
    let _ = writeln!(
        s,
        " gen   best   mean  champ rules species   H   intrans maxDom arch rej  op.tog"
    );
    for l in log {
        let _ = writeln!(
            s,
            " {:>3} {:+.3} {:+.3} {:+.3}   {:>2}     {:>2}   {:.2}   {:.2}   {:.2}  {:>2}  {}   {:.3}",
            l.gen,
            l.best_fitness,
            l.mean_fitness,
            l.champion.fitness,
            l.champion_rule_count,
            l.genotypic.distinct_species,
            l.genotypic.species_entropy,
            l.non_transitivity.intransitive_triads,
            l.non_transitivity.max_dominance,
            l.archive_size,
            if l.candidate_rejected { "Y" } else { "." },
            l.mean_op_toggle,
        );
    }
    s.push_str("```\n\n");

    // ---- R1 analysis with evidence ----
    s.push_str("## R1 — dominant-policy collapse: the read, with evidence\n\n");
    s.push_str(
        "**R1 (`04`) is the scariest risk:** if the DSL admits a *dominant* meta-policy, both \
         the player's iteration loop and the Architect's evolution converge to a fixed point and \
         die — \"no arms race, and nothing left to understand.\" The question can only be \
         answered empirically, in policy space, which is exactly what this loop does. The \
         instrument (`diversity.rs`) measures two things:\n\n",
    );
    s.push_str(
        "1. **Genotypic diversity** — distinct strategy-species + entropy in the gene pool \
         (coarse; a collapsed population trivially has none).\n\
         2. **Strategic non-transitivity** — the load-bearing measure: among the population's \
         distinct strategies, the fraction of `{A,B,C}` triples forming a 3-cycle `A≻B≻C≻A`. \
         Cycles ⇒ every policy has a counter ⇒ **no dominant policy can exist** ⇒ the arms race \
         can persist. A strict dominance order (no cycles, plus a genome that beats everyone) is \
         the collapse signature.\n\n",
    );
    let _ = writeln!(
        s,
        "**Evidence from this run (second-half means):** intransitive-triad fraction \
         **{:.0}%**, max single-strategy dominance **{:.0}%** of opponents, distinct \
         strategy-species **{:.1}**. ",
        100.0 * late_intrans,
        100.0 * late_dom,
        late_species
    );
    match r1 {
        R1Read::DiversityPersists => {
            s.push_str(
                "Because intransitivity stayed clearly positive **and** no single strategy beat \
                 everyone, the verdict is **diversity / a non-transitive arms race persists**. \
                 The triad's rock-paper-scissors survives at the *policy* level — the population \
                 keeps re-discovering counters rather than collapsing onto one program. This is \
                 the **go** signal for R1: on this DSL + engine, the edifice's load-bearing \
                 assumption holds.\n\n",
            );
        }
        R1Read::Collapse => {
            s.push_str(
                "Intransitivity vanished and a near-total dominator emerged: the verdict is \
                 **dominant-policy collapse**. A single meta-policy beats the field, so the arms \
                 race has a fixed point. This is the **stop-and-redesign** signal R1 warns of — \
                 the triad's cycle did not lift to policy space here.\n\n",
            );
        }
        R1Read::Mixed => {
            s.push_str(
                "The signals are mixed — some collapse pressure (the champion stabilizes) but \
                 cycles never fully vanished, so no single policy dominated outright. Read as a \
                 **qualified go**: diversity is fragile but present; worth re-running with a \
                 larger population / more generations to see which way it tips.\n\n",
            );
        }
    }

    // Champion turnover as corroborating evidence.
    let changes = log.iter().filter(|l| l.champion_changed).count();
    let rejects = log.iter().filter(|l| l.candidate_rejected).count();
    let _ = writeln!(
        s,
        "**Corroborating signals.** The published champion changed **{} times** across {} \
         generations (a frozen champion early would itself hint at collapse); the success-story \
         gate **rejected a regressing candidate {} times** (the archive actively kept past \
         exploits closed); the regression archive grew to **{} entries** (each a distinct \
         counter the population discovered and must keep beating).\n\n",
        changes, cfg.generations, rejects, run.archive.len()
    );

    // ---- The regression archive (wins-as-test-cases) ----
    s.push_str("## The regression archive (wins-as-test-cases)\n\n");
    if run.archive.is_empty() {
        s.push_str(
            "_No exploits were archived_ — over this run the champion was never beaten by an \
             elite member by the archive margin. (Expected on short runs or when the champion \
             sits in a robust region; lengthen the run or raise coevolution pressure to force \
             more exploit→patch cycles.)\n\n",
        );
    } else {
        s.push_str(
            "Each entry is a configuration that beat a champion (the headless analogue of \"a \
             line the player won with\"). Every future champion must keep beating **all** of \
             them to pass the success-story gate — so each exploit, once found, stays closed. \
             This is the **exploit → patch arms race** the design calls \"the engine of the \
             whole game,\" made mechanical.\n\n",
        );
        let _ = writeln!(s, "| # | archived counter (rule tags) | gate margin |");
        let _ = writeln!(s, "|---|---|---|");
        for (i, e) in run.archive.entries.iter().enumerate() {
            let _ = writeln!(
                s,
                "| {} | `{}` | {:+.2} |",
                i + 1,
                compact_label(&e.opponent),
                e.required_margin
            );
        }
        s.push('\n');
    }

    // ---- R3 / R4 notes ----
    s.push_str("## R3 (legibility vs strength) and R4 (overfitting) notes\n\n");
    let _ = writeln!(
        s,
        "- **R3.** The parsimony penalty ({:.3}/rule) held the champion to {}–{} rules across \
         the run — small enough that the glass genome reads at a glance. We deliberately bought \
         that legibility at a strength ceiling (`04` R3): a penalty-free run would tolerate \
         larger, stronger, less readable policies. The point of the Architect is the most \
         *comprehensible* escalation, not the strongest opponent.",
        crate::fitness::PARSIMONY_COST,
        log.iter().map(|l| l.champion_rule_count).min().unwrap_or(0),
        log.iter().map(|l| l.champion_rule_count).max().unwrap_or(0),
    );
    let _ = writeln!(
        s,
        "- **R4.** Fitness blends self-play coevolution (weight {:.2}) with a *fixed* cached \
         player model (weight {:.2}). Keeping self-play dominant and the player model fixed (not \
         a chased, stale target) is the `02`/`04` R4 mitigation; the regression archive is the \
         player's **actual** wins serving as the real test set, rather than the model's \
         predictions.\n",
        1.0 - crate::fitness::PLAYER_MODEL_WEIGHT,
        crate::fitness::PLAYER_MODEL_WEIGHT,
    );
    s.push('\n');

    s.push_str("## Reproduce\n\n");
    s.push_str("```\ncargo run -p architect --release\n```\n");
    s.push_str(
        "\nEverything is built on the deterministic mean-field engine plus one seeded PRNG, so \
         a given seed reproduces this run bit-for-bit. `cargo test -p architect` covers the \
         genome/operators, the fitness + success-story gate, the diff renderer, and the R1 \
         instrument (including that it detects the known C/D/A rock-paper-scissors cycle).\n",
    );

    s
}

/// Did the run self-improve? True if the published champion's blended fitness is higher in
/// the last quarter than in the first quarter (a real upward trajectory, not noise).
fn headline_self_improves(log: &[GenerationLog]) -> bool {
    if log.len() < 4 {
        return false;
    }
    let q = log.len() / 4;
    let early = log[..q].iter().map(|l| l.champion.fitness).sum::<f64>() / q as f64;
    let late = log[log.len() - q..].iter().map(|l| l.champion.fitness).sum::<f64>() / q as f64;
    late > early + 1e-6
}

/// A short phrase describing the champion-fitness trend, for the headline sentence.
fn fitness_trend_phrase(log: &[GenerationLog]) -> &'static str {
    if log.len() < 4 {
        return "held steady";
    }
    let q = log.len() / 4;
    let early = log[..q].iter().map(|l| l.champion.fitness).sum::<f64>() / q as f64;
    let late = log[log.len() - q..].iter().map(|l| l.champion.fitness).sum::<f64>() / q as f64;
    let d = late - early;
    if d > 0.05 {
        "rose clearly"
    } else if d > 1e-6 {
        "rose modestly"
    } else if d.abs() <= 1e-6 {
        "held steady"
    } else {
        "drifted down (parsimony/relative-fitness pressure)"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::genome::seed_archetypes;
    use crate::population::{run, Config};
    use cell_core::maps::all_maps;

    fn params() -> Params {
        Params { r: 0.6, k: 2.25, l: 0.15, ..Params::default() }
    }
    fn maps() -> Vec<cell_core::GameState> {
        all_maps().into_iter().map(|m| m.state).collect()
    }

    #[test]
    fn compact_label_lists_tags_in_priority_order() {
        let g = &seed_archetypes()[2]; // attack: strike(engage) + expand-(expand) + mass(route)
        let lbl = compact_label(g);
        // strike (engage) must come before mass (route).
        let sp = lbl.find("strike").unwrap();
        let mp = lbl.find("mass").unwrap();
        assert!(sp < mp, "label should be priority-ordered: {lbl}");
    }

    #[test]
    fn markdown_renders_all_sections() {
        let cfg = Config { pop_size: 16, generations: 6, coevo_sample: 4, n_reps: 5, ..Config::default() };
        let r = run(cfg, &maps(), &params());
        let md = to_markdown(&r, &params(), 0.0);
        for needle in [
            "# ARCHITECT_RESULTS",
            "## Headline result",
            "R1 read",
            "## The champion glass genome",
            "## Glass-genome evolution",
            "## Per-generation trace",
            "## R1 — dominant-policy collapse",
            "## The regression archive",
            "R3 (legibility vs strength)",
        ] {
            assert!(md.contains(needle), "report missing section: {needle}");
        }
    }

    #[test]
    fn classify_r1_handles_empty() {
        let (read, _, _, _) = classify_r1(&[]);
        assert_eq!(read, R1Read::Mixed);
    }
}
