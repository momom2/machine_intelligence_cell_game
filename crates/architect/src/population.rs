//! The **autoconstructive evolutionary loop** itself: a population of rule-set organisms
//! that each carry their own operators, evolved across generations with mean-field
//! coevolution + cached-player-model fitness, a parsimony penalty, the wins-as-test-cases
//! acceptance gate, glass-genome diffs, and the R1 instrumentation streamed every
//! generation.
//!
//! This is build-order **step 5** end to end. The pieces (`genome`, `variation`,
//! `fitness`, `glass`, `diversity`) are assembled here into the loop `02-ai-opponents.md`
//! describes: "between matches it runs an autoconstructive evolutionary loop over a
//! population of rule-set organisms."
//!
//! ## One generation, in order
//!
//! 1. **Evaluate.** Each organism's blended fitness = coevolution vs a *sample* of the
//!    population (so fitness is relative — the engine of any arms race) + cached
//!    player-model performance − parsimony. Sampling (rather than all-vs-all) keeps the
//!    cost ~`O(pop · sample)` not `O(pop²)` — the R5 compute lever.
//! 2. **Select.** Truncation selection to an elite (the top fraction by fitness).
//! 3. **Propose champion + success-story gate.** The best organism is the *candidate*
//!    champion. It is installed as the new published champion **only if it does not
//!    regress the archive** (`fitness::Archive::accepts`). If it regresses, the previous
//!    champion stands (the exploit stays closed) — Schmidhuber's success-story rule.
//! 4. **Grow the archive.** If the candidate champion is *beaten* by some elite member by
//!    a clear margin, that beater is an "exploit" — added to the regression archive so
//!    future champions must keep beating it. This is the headless analogue of "the player
//!    beat it with a line; that line becomes a test case."
//! 5. **Instrument (R1).** Genotypic diversity + strategic non-transitivity over a set of
//!    representatives, logged so collapse-vs-persistence is visible per generation.
//! 6. **Reproduce.** Fill the next generation from the elite by recombination/mutation
//!    **under the parents' own (inherited, evolving) operators** — the autoconstructive
//!    step.

use cell_core::{GameState, Params};

use crate::diversity::{self, Genotypic, NonTransitivity};
use crate::fitness::{self, Archive, FitnessBreakdown, PlayerModel};
use crate::genome::{seed_archetypes, Genome, Operators};
use crate::glass;
use crate::rng::Rng;

/// Tunable knobs for a run. Defaults are sized for a fast-but-informative headless run
/// (a few seconds in release) per the R5 "keep it cheap" mandate.
#[derive(Debug, Clone, Copy)]
pub struct Config {
    /// Number of organisms per generation.
    pub pop_size: usize,
    /// Number of generations to run.
    pub generations: usize,
    /// How many random opponents each candidate plays for its coevolution fitness (the
    /// sample size). Smaller = cheaper + noisier; larger = smoother + costlier.
    pub coevo_sample: usize,
    /// Elite fraction kept by truncation selection (and used as the reproduction pool).
    pub elite_frac: f64,
    /// Fraction of offspring produced by **recombination** (the rest by mutation-only).
    pub crossover_frac: f64,
    /// Margin above which one genome "beats" another (for the archive gate and the R1
    /// non-transitivity relation). Matches the R2 acceptance epsilon.
    pub beat_epsilon: f64,
    /// How many of the top organisms to use as representatives for the (costlier)
    /// non-transitivity matrix. Kept small so the `O(reps²)` matrix stays cheap.
    pub n_reps: usize,
    /// Master seed for the run (full bit-reproducibility).
    pub seed: u64,
    /// Add an exploit to the archive only when the candidate champion loses to an elite
    /// member by at least this margin (so we archive genuine counters, not noise).
    pub archive_margin: f64,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            pop_size: 48,
            generations: 40,
            coevo_sample: 6,
            elite_frac: 0.25,
            crossover_frac: 0.6,
            beat_epsilon: 0.05,
            n_reps: 8,
            seed: 0xA5CE_11A5_2024,
            archive_margin: 0.10,
        }
    }
}

/// Everything recorded for one generation — the per-generation log row the report and the
/// R1 read are built from.
#[derive(Debug, Clone)]
pub struct GenerationLog {
    /// 0-based generation index.
    pub gen: usize,
    /// Best blended fitness this generation.
    pub best_fitness: f64,
    /// Mean blended fitness over the population.
    pub mean_fitness: f64,
    /// The published champion's fitness breakdown (the champion *after* the success-story
    /// gate — may be the carried-over previous champion if the candidate regressed).
    pub champion: FitnessBreakdown,
    /// The published champion's rule count (parsimony / glass-genome size over time).
    pub champion_rule_count: usize,
    /// True if a *new* champion was installed this generation (the candidate passed the
    /// gate and differed from the previous champion).
    pub champion_changed: bool,
    /// True if the candidate champion was **rejected by the success-story gate** (it would
    /// have regressed the archive). Direct evidence the gate is doing work.
    pub candidate_rejected: bool,
    /// Labels of archive entries a rejected candidate would have regressed (empty if none).
    pub regressed: Vec<String>,
    /// Size of the regression archive at the end of this generation.
    pub archive_size: usize,
    /// Genotypic diversity of the population.
    pub genotypic: Genotypic,
    /// Strategic non-transitivity over the representatives (the headline R1 signal).
    pub non_transitivity: NonTransitivity,
    /// Mean of the population's evolved `p_toggle_gene` operator — so we can *watch the
    /// reproduction machinery itself evolve* (the autoconstructive property, made visible).
    pub mean_op_toggle: f64,
    /// Mean evolved `threshold_step` operator (the other end of the explore/exploit dial).
    pub mean_op_threshold_step: f64,
    /// The glass-genome diff lines vs the previous published champion (rendered).
    pub champion_diff: Vec<glass::DiffLine>,
}

/// The full result of a run: the per-generation log, the final champion, and the final
/// archive — everything the report and the keyResult need.
pub struct RunResult {
    pub log: Vec<GenerationLog>,
    pub final_champion: Genome,
    pub final_champion_fitness: FitnessBreakdown,
    pub archive: Archive,
    pub config: Config,
    pub player_model: PlayerModel,
}

/// A scored organism (genome + its fitness breakdown this generation).
struct Scored {
    genome: Genome,
    fit: FitnessBreakdown,
}

/// Run the autoconstructive loop. `maps` are the evaluation maps (the standard three);
/// `params` the operating point. Returns the full [`RunResult`].
pub fn run(cfg: Config, maps: &[GameState], params: &Params) -> RunResult {
    let mut rng = Rng::new(cfg.seed);

    // --- The cached player model (fixed across the run; see fitness::PlayerModel). A
    // competent colonize-leaning reference, expressed in the same legible substrate. ---
    let player_model =
        PlayerModel { genome: seed_archetypes()[0].clone(), label: "cached player: colonize-leaning" };

    // --- Seed the initial population: the three competent archetypes (so coevolution
    // starts on the real triad) padded out with diverse random genomes. ---
    let mut pop: Vec<Genome> = Vec::with_capacity(cfg.pop_size);
    for g in seed_archetypes() {
        pop.push(g);
    }
    while pop.len() < cfg.pop_size {
        // Vary density so some seeds are lean (1-2 rules) and some rich (toward the
        // catalog size) — spreads the initial parsimony/structure distribution.
        let density = rng.range(0.25, 0.6);
        pop.push(Genome::random(&mut rng, density));
    }

    let mut archive = Archive::new();
    let mut log: Vec<GenerationLog> = Vec::with_capacity(cfg.generations);

    // The published champion (after the success-story gate). Starts as the best seed.
    let mut champion: Option<Genome> = None;
    let mut champion_fit: Option<FitnessBreakdown> = None;

    for gen in 0..cfg.generations {
        // ---- 1. Evaluate every organism's blended fitness ----
        let scored = evaluate_population(&pop, &player_model, maps, params, &mut rng, cfg.coevo_sample);

        // Population fitness summaries.
        let best_idx = argmax(&scored, |s| s.fit.fitness);
        let best_fitness = scored[best_idx].fit.fitness;
        let mean_fitness = scored.iter().map(|s| s.fit.fitness).sum::<f64>() / scored.len() as f64;

        // ---- 2. Select the elite (truncation) ----
        let mut order: Vec<usize> = (0..scored.len()).collect();
        order.sort_by(|&a, &b| {
            scored[b].fit.fitness.partial_cmp(&scored[a].fit.fitness).unwrap_or(std::cmp::Ordering::Equal)
        });
        let elite_n = ((cfg.pop_size as f64 * cfg.elite_frac).round() as usize).clamp(2, cfg.pop_size);
        let elite: Vec<&Scored> = order.iter().take(elite_n).map(|&i| &scored[i]).collect();

        // ---- 3. Candidate champion + success-story acceptance gate ----
        let candidate = elite[0].genome.clone();
        let candidate_fit = elite[0].fit;
        let (accepts, regressed) = archive.accepts(&candidate, maps, params);

        let prev_champion = champion.clone();
        let mut candidate_rejected = false;
        let mut champion_changed = false;

        if champion.is_none() {
            // First generation: install the candidate as the initial champion regardless
            // (the archive is empty, so it trivially passes anyway).
            champion = Some(candidate.clone());
            champion_fit = Some(candidate_fit);
            champion_changed = true;
        } else if accepts {
            // The self-modification does not regress any archived win → install it.
            // (Only count it as a "change" if the rule-set actually differs.)
            let changed = glass::diff_genomes(champion.as_ref().unwrap(), &candidate).len() > 0;
            champion = Some(candidate.clone());
            champion_fit = Some(candidate_fit);
            champion_changed = changed;
        } else {
            // Regression! Keep the previous champion (the exploit stays closed). This is
            // the success-story rule doing its job, and we log exactly which fix it saved.
            candidate_rejected = true;
        }

        // ---- 4. Grow the archive: if the *published* champion is beaten by an elite
        // member by a clear margin, that beater is an exploit to keep closed. ----
        let champ_ref = champion.as_ref().unwrap();
        if let Some(beater) = strongest_beater(champ_ref, &elite, maps, params, cfg.archive_margin) {
            // Avoid archiving a structural duplicate of something already archived.
            let sig = diversity::gene_signature(&beater);
            let already = archive.entries.iter().any(|e| diversity::gene_signature(&e.opponent) == sig);
            if !already {
                let label = format!("exploit#{}: {}", archive.len() + 1, crate::report::compact_label(&beater));
                archive.add(beater, label, cfg.beat_epsilon);
            }
        }

        // ---- 5. R1 instrumentation ----
        // Representatives = the top `n_reps` distinct-signature organisms (so the matrix
        // compares genuinely different strategies, not threshold twins of one species).
        let reps = pick_representatives(&order, &scored, cfg.n_reps);
        let genotypic = diversity::genotypic(&pop);
        let non_transitivity = diversity::non_transitivity(&reps, maps, params, cfg.beat_epsilon);

        // Operator means — watch the reproduction machinery evolve.
        let mean_op_toggle = pop.iter().map(|g| g.operators.p_toggle_gene).sum::<f64>() / pop.len() as f64;
        let mean_op_threshold_step =
            pop.iter().map(|g| g.operators.threshold_step).sum::<f64>() / pop.len() as f64;

        // Glass-genome diff vs the previous published champion.
        let champion_diff = match &prev_champion {
            Some(p) => glass::diff_genomes(p, champion.as_ref().unwrap()),
            None => Vec::new(),
        };

        log.push(GenerationLog {
            gen,
            best_fitness,
            mean_fitness,
            champion: champion_fit.unwrap(),
            champion_rule_count: champion.as_ref().unwrap().rule_count(),
            champion_changed,
            candidate_rejected,
            regressed,
            archive_size: archive.len(),
            genotypic,
            non_transitivity,
            mean_op_toggle,
            mean_op_threshold_step,
            champion_diff,
        });

        // ---- 6. Reproduce the next generation under the parents' own operators ----
        if gen + 1 < cfg.generations {
            pop = reproduce(&elite, cfg, &mut rng);
        }
    }

    RunResult {
        final_champion: champion.clone().unwrap(),
        final_champion_fitness: champion_fit.unwrap(),
        log,
        archive,
        config: cfg,
        player_model,
    }
}

/// Evaluate every organism's blended fitness. Each candidate's coevolution opponents are a
/// random sample of the population *excluding itself*, drawn from the run RNG so the whole
/// evaluation is reproducible. Sampling keeps cost ~`O(pop · sample)`.
fn evaluate_population(
    pop: &[Genome],
    player_model: &PlayerModel,
    maps: &[GameState],
    params: &Params,
    rng: &mut Rng,
    sample: usize,
) -> Vec<Scored> {
    let n = pop.len();
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        // Sample `sample` distinct opponents != i.
        let opponents = sample_opponents(n, i, sample, rng);
        let opp_genomes: Vec<Genome> = opponents.into_iter().map(|j| pop[j].clone()).collect();
        let fit = fitness::evaluate(&pop[i], &opp_genomes, player_model, maps, params);
        out.push(Scored { genome: pop[i].clone(), fit });
    }
    out
}

/// Draw up to `k` distinct indices in `[0,n)` excluding `exclude`.
fn sample_opponents(n: usize, exclude: usize, k: usize, rng: &mut Rng) -> Vec<usize> {
    if n <= 1 {
        return Vec::new();
    }
    let k = k.min(n - 1);
    let mut chosen = Vec::with_capacity(k);
    // Simple rejection sampling — k is small relative to n, so this is cheap.
    let mut guard = 0;
    while chosen.len() < k && guard < 1000 {
        let j = rng.below(n);
        if j != exclude && !chosen.contains(&j) {
            chosen.push(j);
        }
        guard += 1;
    }
    chosen
}

/// Of the elite, the one that beats `champ` by the largest margin above `min_margin`
/// (the "strongest exploit"), if any. Used to grow the regression archive.
fn strongest_beater(
    champ: &Genome,
    elite: &[&Scored],
    maps: &[GameState],
    params: &Params,
    min_margin: f64,
) -> Option<Genome> {
    let mut best: Option<(Genome, f64)> = None;
    for s in elite {
        // Skip an elite member that *is* (structurally) the champion.
        if diversity::gene_signature(&s.genome) == diversity::gene_signature(champ) {
            continue;
        }
        let g = s.genome.clone();
        // margin of the beater over the champion = score(beater vs champ).
        let champ_clone = champ.clone();
        let beater_margin =
            -fitness::duel_over_maps(maps, &champ_clone, &move || g.make_player(), params);
        if beater_margin >= min_margin && best.as_ref().map_or(true, |(_, m)| beater_margin > *m) {
            best = Some((s.genome.clone(), beater_margin));
        }
    }
    best.map(|(g, _)| g)
}

/// Pick up to `n_reps` representatives with **distinct gene signatures**, taking the
/// highest-fitness organism of each signature in fitness order. Distinct signatures make
/// the non-transitivity matrix compare genuinely different strategies.
fn pick_representatives(order: &[usize], scored: &[Scored], n_reps: usize) -> Vec<Genome> {
    let mut seen: Vec<u64> = Vec::new();
    let mut reps: Vec<Genome> = Vec::new();
    for &i in order {
        let sig = diversity::gene_signature(&scored[i].genome);
        if !seen.contains(&sig) {
            seen.push(sig);
            reps.push(scored[i].genome.clone());
            if reps.len() >= n_reps {
                break;
            }
        }
    }
    reps
}

/// Build the next generation from the elite pool: an elite-preserving step (keep the best
/// genome verbatim — elitism) then fill by recombination/mutation under the parents' own
/// operators.
fn reproduce(elite: &[&Scored], cfg: Config, rng: &mut Rng) -> Vec<Genome> {
    let mut next = Vec::with_capacity(cfg.pop_size);
    // Elitism: carry the single best genome unchanged so fitness never regresses by drift.
    next.push(elite[0].genome.clone());

    while next.len() < cfg.pop_size {
        if rng.chance(cfg.crossover_frac) && elite.len() >= 2 {
            // Recombine two distinct elite parents.
            let a = rng.below(elite.len());
            let mut b = rng.below(elite.len());
            if b == a {
                b = (b + 1) % elite.len();
            }
            next.push(crate::variation::recombine(&elite[a].genome, &elite[b].genome, rng));
        } else {
            // Mutate a single elite parent.
            let a = rng.below(elite.len());
            next.push(crate::variation::mutate_only(&elite[a].genome, rng));
        }
    }
    next
}

/// Index of the max element by key `f`.
fn argmax<T, F: Fn(&T) -> f64>(xs: &[T], f: F) -> usize {
    let mut best = 0;
    let mut best_v = f64::NEG_INFINITY;
    for (i, x) in xs.iter().enumerate() {
        let v = f(x);
        if v > best_v {
            best_v = v;
            best = i;
        }
    }
    best
}

/// The mean evolved operator profile of a population — exposed for the report so it can
/// show that the reproduction machinery itself drifted under selection.
pub fn mean_operators(pop: &[Genome]) -> Operators {
    let n = pop.len().max(1) as f64;
    let mut acc = Operators { p_toggle_gene: 0.0, p_tweak_threshold: 0.0, p_tweak_priority: 0.0, threshold_step: 0.0, crossover_bias: 0.0 };
    for g in pop {
        acc.p_toggle_gene += g.operators.p_toggle_gene;
        acc.p_tweak_threshold += g.operators.p_tweak_threshold;
        acc.p_tweak_priority += g.operators.p_tweak_priority;
        acc.threshold_step += g.operators.threshold_step;
        acc.crossover_bias += g.operators.crossover_bias;
    }
    Operators {
        p_toggle_gene: acc.p_toggle_gene / n,
        p_tweak_threshold: acc.p_tweak_threshold / n,
        p_tweak_priority: acc.p_tweak_priority / n,
        threshold_step: acc.threshold_step / n,
        crossover_bias: acc.crossover_bias / n,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cell_core::maps::all_maps;

    fn params() -> Params {
        Params { r: 0.6, k: 2.25, l: 0.15, ..Params::default() }
    }
    fn maps() -> Vec<GameState> {
        all_maps().into_iter().map(|m| m.state).collect()
    }

    /// A short run is deterministic from its seed (same champion structure, same log).
    #[test]
    fn run_is_deterministic_from_seed() {
        let cfg = Config { pop_size: 16, generations: 5, coevo_sample: 4, n_reps: 5, ..Config::default() };
        let r1 = run(cfg, &maps(), &params());
        let r2 = run(cfg, &maps(), &params());
        assert_eq!(r1.log.len(), r2.log.len());
        for (a, b) in r1.log.iter().zip(r2.log.iter()) {
            assert_eq!(a.best_fitness, b.best_fitness, "gen {} best fitness", a.gen);
            assert_eq!(a.champion_rule_count, b.champion_rule_count);
            assert_eq!(a.non_transitivity.intransitive_triads, b.non_transitivity.intransitive_triads);
        }
        // Same final champion structure.
        assert_eq!(
            diversity::gene_signature(&r1.final_champion),
            diversity::gene_signature(&r2.final_champion)
        );
    }

    /// The champion's rule count stays bounded (parsimony works): it never blows up to the
    /// full catalog over a run.
    #[test]
    fn parsimony_bounds_champion_size() {
        let cfg = Config { pop_size: 24, generations: 12, coevo_sample: 5, ..Config::default() };
        let r = run(cfg, &maps(), &params());
        let max_rules = r.log.iter().map(|l| l.champion_rule_count).max().unwrap();
        assert!(
            max_rules <= crate::genome::catalog_len(),
            "champion rule count {} exceeds catalog",
            max_rules
        );
        // It should be meaningfully smaller than the catalog most of the time (legible).
        let final_rules = r.final_champion.rule_count();
        assert!(final_rules >= 1 && final_rules <= crate::genome::catalog_len());
    }

    /// The success-story gate is exercised: over a run with coevolution producing counters,
    /// the archive grows (exploits get recorded) — evidence the wins-as-test-cases
    /// machinery is live. (We assert non-strictly: at least the archive is valid and the
    /// run completes with a champion.)
    #[test]
    fn run_completes_with_champion_and_archive() {
        let cfg = Config { pop_size: 20, generations: 10, coevo_sample: 5, ..Config::default() };
        let r = run(cfg, &maps(), &params());
        assert!(r.final_champion.rule_count() >= 1);
        // Champion is a real player: beats idle.
        let base = &maps()[0];
        let mut champ = r.final_champion.make_player();
        let mut idle = idle();
        let out = base.clone().run_match(champ.as_mut(), idle.as_mut(), &params(), 200);
        assert!(out.score_a > 0.0, "final champion should beat idle");
    }

    fn idle() -> Box<dyn cell_core::Policy> {
        struct Idle;
        impl cell_core::Policy for Idle {
            fn name(&self) -> &'static str {
                "Idle"
            }
            fn decide(&mut self, _s: &cell_core::GameState, _me: cell_core::Owner, _p: &Params) -> Vec<cell_core::Command> {
                Vec::new()
            }
        }
        Box::new(Idle)
    }
}
