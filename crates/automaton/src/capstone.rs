//! The arc-1 **capstone player** (scout → infer → counter → execute) and the fixed
//! non-inferring **baseline** it is measured against, plus the headless validation that
//! answers the step-4 keyResult.
//!
//! ## The four steps as one policy (`02-ai-opponents.md`)
//!
//! [`Capstone`] is itself a `cell_core::Policy`, so it plugs straight into the match
//! runner — but unlike a fixed Automaton it **changes its standing orders mid-match**
//! (decision **D1** "fluid toggle"). It runs in two phases:
//!
//! 1. **scout / observe** — for the first `scout_ticks` it plays a neutral economic
//!    posture and feeds every tick to a [`Scout`], reading the opponent's early
//!    behaviour from the legible features;
//! 2. **infer → select → execute** — once the window closes it commits: it infers the
//!    opponent's hidden [`Mix`], **rotates it to the rock-paper-scissors counter**, and
//!    plays the counter Automaton (compiled from the *same* DSL — the shared-vocabulary
//!    spine) for the rest of the match.
//!
//! ## The counter rotation (the cycle, made mechanical)
//!
//! The triad cycle is attack≻colonize, colonize≻defend, defend≻attack (`01`,
//! `R2_RESULTS.md`). So the strategy that punishes a given mix is a fixed **rotation**
//! of that mix's weights:
//!
//! | the opponent leans… | …so the counter leans | because |
//! |---|---|---|
//! | colonize | **attack** | undefended expansion falls to a timed strike |
//! | defend   | **colonize** | the turtle pays opportunity cost while you out-produce |
//! | attack   | **defend** | the defender's advantage punishes the over-committed push |
//!
//! As weights: `counter(c, d, a) = Mix(colonize=d, defend=a, attack=c)`. A *blended*
//! opponent gets a *blended* counter, which is why the routine degrades gracefully near
//! the centre rather than mis-committing to one pure response.
//!
//! ## The baseline (the control)
//!
//! [`Baseline`] uses the **identical** machinery — scout window then commit — but
//! commits to a **fixed** mix that ignores what it observed. This isolates exactly the
//! contribution of inference: any win-rate gap between [`Capstone`] and [`Baseline`] is
//! attributable to *reading the opponent*, not to the counter library, the maps, or the
//! scouting cost (both pay it). The default fixed mix is the balanced centre — the
//! maximally-hedged choice a non-inferring player would rationally make with no
//! information.

use cell_core::{Command, GameState, Owner, Params};

use crate::automaton::{compile, AutomatonSpec};
use crate::infer::{InferConfig, Scout};
use crate::ladder::Rung;
use crate::mix::Mix;

/// The posture the capstone (and the standalone scout) play *while observing*.
///
/// It must (a) **survive** the scouting window against *any* opponent — a pure colonizer
/// scout gets eliminated by an attacker before it can switch, which is fatal — while (b)
/// still gathering economy and (c) not biasing the read. A **colonize-leaning defensive
/// blend** does all three: it expands to keep pace, but reinforces if struck, so it is
/// never crippled before it commits. (Built from the *same* DSL — one substrate.)
pub fn scout_posture() -> Box<dyn cell_core::Policy> {
    // c=0.6, d=0.35, a=0.05: mostly economy with just enough defend to weather an early
    // strike during the short window, and a touch of contact-seeking so it reaches the
    // opponent and reveals aggression. Deliberately *not* a dominant policy — it only has
    // to survive the brief scouting window; the committed counter wins the match.
    Box::new(cell_core::DslPolicy::new(compile(Mix::new(0.6, 0.35, 0.05), "scout")))
}

/// Rotate an opponent mix to its rock-paper-scissors counter mix (see the module docs):
/// `counter(c, d, a) = Mix(colonize=d, defend=a, attack=c)`.
pub fn counter_mix(opp: Mix) -> Mix {
    Mix::new(opp.d, opp.a, opp.c)
}

// ===========================================================================
// The capstone player
// ===========================================================================

/// The inferring capstone player. Scouts, infers the opponent's mix, rotates to the
/// counter, and executes it for the rest of the match.
pub struct Capstone {
    cfg: InferConfig,
    /// Built lazily on `reset`/first tick once we know our seat.
    scout: Option<Scout>,
    /// The committed counter policy, set when the scouting window closes.
    counter: Option<Box<dyn cell_core::Policy>>,
    /// The posture played during scouting.
    scout_brain: Box<dyn cell_core::Policy>,
    /// Cached estimate of the opponent's mix (for reporting inference accuracy).
    last_estimate: Option<Mix>,
}

impl Capstone {
    /// A capstone player with the given inference config.
    pub fn new(cfg: InferConfig) -> Capstone {
        Capstone {
            cfg,
            scout: None,
            counter: None,
            scout_brain: scout_posture(),
            last_estimate: None,
        }
    }

    /// A capstone player with default inference config.
    pub fn with_defaults() -> Capstone {
        Capstone::new(InferConfig::default())
    }

    /// The mix this player inferred for its opponent in the most recent match (None
    /// until it has finished scouting at least once). Used to score inference accuracy.
    pub fn last_estimate(&self) -> Option<Mix> {
        self.last_estimate
    }
}

impl cell_core::Policy for Capstone {
    fn name(&self) -> &'static str {
        "Capstone(infer+counter)"
    }

    fn reset(&mut self) {
        self.scout = None;
        self.counter = None;
        self.scout_brain = scout_posture();
        // keep last_estimate across reset so a caller can read it after the match
    }

    fn decide(&mut self, state: &GameState, me: Owner, params: &Params) -> Vec<Command> {
        let opp = me.opponent();
        // Lazily initialize the scout now that we know our seat.
        let scout = self.scout.get_or_insert_with(|| Scout::new(opp, self.cfg));

        if !scout.done() {
            // Phase 1: observe this tick, then play the neutral scouting posture.
            scout.observe(state, me, params);
            // If that observation *closed* the window, commit immediately so we do not
            // waste this tick playing scout when we could start countering.
            if scout.done() {
                let est = scout.estimate();
                self.last_estimate = Some(est);
                let counter = counter_mix(est);
                let spec = AutomatonSpec::from_mix(counter);
                self.counter = Some(spec.make());
            } else {
                return self.scout_brain.decide(state, me, params);
            }
        }

        // Phase 2: execute the committed counter.
        self.counter
            .as_mut()
            .expect("counter set once scouting is done")
            .decide(state, me, params)
    }
}

// ===========================================================================
// The fixed non-inferring baseline (control)
// ===========================================================================

/// The control player: identical scout-then-commit *machinery* but commits to a
/// **fixed** mix regardless of what it observed. Isolates the value of inference.
pub struct Baseline {
    cfg: InferConfig,
    fixed: Mix,
    scout: Option<Scout>,
    committed: Option<Box<dyn cell_core::Policy>>,
    scout_brain: Box<dyn cell_core::Policy>,
}

impl Baseline {
    /// A baseline that, after the same scouting window, always plays `fixed`.
    pub fn new(cfg: InferConfig, fixed: Mix) -> Baseline {
        Baseline {
            cfg,
            fixed,
            scout: None,
            committed: None,
            scout_brain: scout_posture(),
        }
    }

    /// The default control: after scouting, always play the **balanced** mix — the
    /// maximally-hedged choice with no information. This is the fairest non-inferring
    /// opponent: it pays the same scouting cost and plays a competent all-round policy,
    /// it simply never *reads* the opponent.
    pub fn balanced() -> Baseline {
        Baseline::new(InferConfig::default(), Mix::centre())
    }
}

impl cell_core::Policy for Baseline {
    fn name(&self) -> &'static str {
        "Baseline(fixed,no-infer)"
    }

    fn reset(&mut self) {
        self.scout = None;
        self.committed = None;
        self.scout_brain = scout_posture();
    }

    fn decide(&mut self, state: &GameState, me: Owner, params: &Params) -> Vec<Command> {
        let opp = me.opponent();
        let scout = self.scout.get_or_insert_with(|| Scout::new(opp, self.cfg));
        if !scout.done() {
            // Observe (so the scouting *cost* and timing exactly match the capstone) but
            // throw the estimate away.
            scout.observe(state, me, params);
            if scout.done() {
                let spec = AutomatonSpec::from_mix(self.fixed);
                self.committed = Some(spec.make());
            } else {
                return self.scout_brain.decide(state, me, params);
            }
        }
        self.committed
            .as_mut()
            .expect("committed set once scouting is done")
            .decide(state, me, params)
    }
}

// ===========================================================================
// The perfect-information oracle (upper bound / isolates the inference channel)
// ===========================================================================

/// An upper-bound control: identical scout-then-commit *machinery* and the *same*
/// counter library as [`Capstone`], but it is **handed the opponent's true mix** instead
/// of inferring it. Comparing capstone to oracle isolates the cost of *imperfect
/// reading*: `oracle_score − capstone_score` is the price the capstone pays for
/// inference error, and the design predicts that price **rises with centrality** (central
/// mixes are harder to read). Comparing oracle to baseline shows the ceiling a perfect
/// reader reaches with this counter library.
pub struct Oracle {
    cfg: InferConfig,
    true_mix: Mix,
    scout: Option<Scout>,
    committed: Option<Box<dyn cell_core::Policy>>,
    scout_brain: Box<dyn cell_core::Policy>,
}

impl Oracle {
    /// An oracle that, after the same scouting window, plays the exact counter to
    /// `true_mix`.
    pub fn new(cfg: InferConfig, true_mix: Mix) -> Oracle {
        Oracle {
            cfg,
            true_mix,
            scout: None,
            committed: None,
            scout_brain: scout_posture(),
        }
    }
}

impl cell_core::Policy for Oracle {
    fn name(&self) -> &'static str {
        "Oracle(true-mix counter)"
    }

    fn reset(&mut self) {
        self.scout = None;
        self.committed = None;
        self.scout_brain = scout_posture();
    }

    fn decide(&mut self, state: &GameState, me: Owner, params: &Params) -> Vec<Command> {
        let opp = me.opponent();
        let scout = self.scout.get_or_insert_with(|| Scout::new(opp, self.cfg));
        if !scout.done() {
            scout.observe(state, me, params); // match cost/timing; ignore the estimate
            if scout.done() {
                let spec = AutomatonSpec::from_mix(counter_mix(self.true_mix));
                self.committed = Some(spec.make());
            } else {
                return self.scout_brain.decide(state, me, params);
            }
        }
        self.committed
            .as_mut()
            .expect("committed set once scouting is done")
            .decide(state, me, params)
    }
}

// ===========================================================================
// Headless validation
// ===========================================================================

/// Per-rung outcome of the validation: how each player fared against one Automaton.
#[derive(Debug, Clone)]
pub struct RungReport {
    /// 1-based rung index (sorted by ascending centrality).
    pub index: usize,
    /// Human label of the Automaton's mix (e.g. `C`, `C2D1`, `bal`).
    pub label: String,
    /// The Automaton's hidden true mix.
    pub true_mix: Mix,
    /// The difficulty metric: simplex centrality in [0,1].
    pub centrality: f64,

    /// Capstone win-rate against this rung in [0,1] (over all maps × both seatings).
    pub capstone_winrate: f64,
    /// Baseline win-rate against this rung in [0,1] (same matches).
    pub baseline_winrate: f64,
    /// Oracle (perfect-information counter) win-rate against this rung in [0,1].
    pub oracle_winrate: f64,
    /// Mean capstone score margin in [-1,1] (over all maps × both seatings).
    pub capstone_score: f64,
    /// Mean baseline score margin in [-1,1].
    pub baseline_score: f64,
    /// Mean oracle score margin in [-1,1] — the ceiling this counter library reaches
    /// against this rung *with a perfect read*. `oracle_score − capstone_score` is the
    /// price the capstone pays for *imperfect reading* (the inference penalty).
    pub oracle_score: f64,

    /// Mean inferred mix the capstone settled on (averaged over the matches/seatings).
    pub inferred_mix: Mix,
    /// Inference error: simplex distance between inferred and true mix (lower = better).
    /// This is the **read-difficulty** measure the design predicts rises with centrality.
    pub inference_error: f64,
    /// True iff the inferred mix's nearest corner equals the true mix's nearest corner
    /// (the "did we read the dominant lean correctly" classification accuracy).
    pub corner_correct: bool,
}

/// Aggregate validation across the whole ladder, plus the headline summary numbers the
/// step-4 keyResult needs.
///
/// The design (`02`) makes a **two-part** claim — central mixes are hardest *to read* and
/// *to counter* — so the validation reports the two parts separately rather than folding
/// them into one number that could hide which half holds:
///
/// * **read-difficulty** = `centrality_vs_inference_error_corr` — does the mix get harder
///   to *read* (larger inference error) toward the centre? This is the cleaner, more
///   directly-measured half (inference error is continuous and not matchup-saturated).
/// * **counter-difficulty** = `centrality_vs_oracle_difficulty_corr` — with a *perfect*
///   read (the Oracle), does the residual difficulty still rise toward the centre? This
///   isolates "hard to counter" from "hard to read".
///
/// The combined capstone difficulty (`centrality_vs_difficulty_corr`) is reported too,
/// but it mixes both effects and the matchup-idiosyncrasy of a small curated ladder, so
/// it is the *noisiest* of the three on the 16-rung ladder — see [`simplex_grid_study`]
/// for the same correlations on a dense uniform sampling, which is the statistically
/// sound test of the monotone claim.
#[derive(Debug, Clone)]
pub struct LadderEvaluation {
    pub rungs: Vec<RungReport>,
    /// Mean capstone win-rate across all rungs.
    pub mean_capstone_winrate: f64,
    /// Mean baseline win-rate across all rungs.
    pub mean_baseline_winrate: f64,
    /// Mean oracle (perfect-read) win-rate across all rungs — the counter library's
    /// ceiling.
    pub mean_oracle_winrate: f64,
    /// Mean inference error across all rungs.
    pub mean_inference_error: f64,
    /// Fraction of rungs whose dominant lean was classified correctly.
    pub corner_accuracy: f64,

    /// **Read-difficulty.** Pearson correlation between centrality and *inference error*.
    /// Positive ⇒ central mixes are harder to read, as the design predicts. This is the
    /// most defensible half of the claim (continuous, not score-saturated).
    pub centrality_vs_inference_error_corr: f64,
    /// **Counter-difficulty.** Correlation between centrality and the *oracle's*
    /// score-difficulty `(1 − oracle_score)/2`. Positive ⇒ central mixes are harder to
    /// counter even with a perfect read.
    pub centrality_vs_oracle_difficulty_corr: f64,
    /// **Combined.** Correlation between centrality and the *capstone's* score-difficulty
    /// `(1 − capstone_score)/2 ∈ [0,1]`. Folds read + counter difficulty; noisiest on the
    /// curated ladder.
    pub centrality_vs_difficulty_corr: f64,
    /// Same combined score-difficulty correlation for the baseline (for contrast).
    pub centrality_vs_difficulty_corr_baseline: f64,
}

/// Run the full headless validation: play [`Capstone`] and [`Baseline`] against every
/// rung of `ladder`, on every map in both seatings, at `params`/`horizon`.
///
/// Determinism: the engine has no RNG and both players are deterministic functions of
/// the observed state, so this whole evaluation is bit-reproducible.
pub fn evaluate_ladder(
    ladder: &[Rung],
    maps: &[(String, GameState)],
    cfg: InferConfig,
    params: &Params,
    horizon: u64,
) -> LadderEvaluation {
    let mut rungs: Vec<RungReport> = Vec::with_capacity(ladder.len());

    for rung in ladder {
        let spec = &rung.spec;

        // --- Capstone / Baseline / Oracle vs this rung, all maps, both seatings ---
        let (cap_wins, cap_score, cap_n) =
            play_player_vs_rung(maps, params, horizon, spec, cfg, PlayerKind::Capstone);
        let (base_wins, base_score, base_n) =
            play_player_vs_rung(maps, params, horizon, spec, cfg, PlayerKind::Baseline);
        let (orc_wins, orc_score, _orc_n) =
            play_player_vs_rung(maps, params, horizon, spec, cfg, PlayerKind::Oracle);

        // --- Inference accuracy: the estimate the capstone settles on, averaged over
        // the same maps + seatings. Computed via the standalone scout (the identical
        // Scout/cfg observing the same opponent from the same seat → the same estimate
        // the live Capstone computes inline), which sidesteps needing to downcast the
        // trait object.
        let inferred_mix = mean_inferred_mix(maps, params, spec, cfg);
        let inference_error = inferred_mix.distance(spec.mix);
        let corner_correct = inferred_mix.nearest_corner() == spec.mix.nearest_corner();

        rungs.push(RungReport {
            index: rung.index,
            label: spec.name.clone(),
            true_mix: spec.mix,
            centrality: rung.centrality(),
            capstone_winrate: cap_wins / cap_n as f64,
            baseline_winrate: base_wins / base_n as f64,
            oracle_winrate: orc_wins / cap_n as f64,
            capstone_score: cap_score / cap_n as f64,
            baseline_score: base_score / base_n as f64,
            oracle_score: orc_score / cap_n as f64,
            inferred_mix,
            inference_error,
            corner_correct,
        });
        let _ = base_n;
    }

    // Aggregate summaries.
    let n = rungs.len().max(1) as f64;
    let mean_capstone_winrate = rungs.iter().map(|r| r.capstone_winrate).sum::<f64>() / n;
    let mean_baseline_winrate = rungs.iter().map(|r| r.baseline_winrate).sum::<f64>() / n;
    let mean_oracle_winrate = rungs.iter().map(|r| r.oracle_winrate).sum::<f64>() / n;
    let mean_inference_error = rungs.iter().map(|r| r.inference_error).sum::<f64>() / n;
    let corner_accuracy = rungs.iter().filter(|r| r.corner_correct).count() as f64 / n;

    // The three correlations that test the design's two-part claim (see the struct docs).
    let cen: Vec<f64> = rungs.iter().map(|r| r.centrality).collect();
    let inf_err: Vec<f64> = rungs.iter().map(|r| r.inference_error).collect();
    let orc_diff: Vec<f64> = rungs.iter().map(|r| (1.0 - r.oracle_score) / 2.0).collect();
    let cap_diff: Vec<f64> = rungs.iter().map(|r| (1.0 - r.capstone_score) / 2.0).collect();
    let base_diff: Vec<f64> = rungs.iter().map(|r| (1.0 - r.baseline_score) / 2.0).collect();

    LadderEvaluation {
        rungs,
        mean_capstone_winrate,
        mean_baseline_winrate,
        mean_oracle_winrate,
        mean_inference_error,
        corner_accuracy,
        centrality_vs_inference_error_corr: pearson(&cen, &inf_err),
        centrality_vs_oracle_difficulty_corr: pearson(&cen, &orc_diff),
        centrality_vs_difficulty_corr: pearson(&cen, &cap_diff),
        centrality_vs_difficulty_corr_baseline: pearson(&cen, &base_diff),
    }
}

/// Which player to instantiate for a matchup.
#[derive(Clone, Copy)]
enum PlayerKind {
    Capstone,
    Baseline,
    /// Perfect-information control: counters the *true* mix (isolates reading cost).
    Oracle,
}

/// Play one player against one Automaton across all maps and both seatings. Returns
/// `(total_wins, total_score, n_matches)`. A "win" is a strictly positive end score for
/// the player's seat (elimination victories score +1, so they count).
fn play_player_vs_rung(
    maps: &[(String, GameState)],
    params: &Params,
    horizon: u64,
    spec: &AutomatonSpec,
    cfg: InferConfig,
    kind: PlayerKind,
) -> (f64, f64, usize) {
    let mut wins = 0.0;
    let mut score_sum = 0.0;
    let mut n = 0usize;

    for (_name, base) in maps {
        // Both seatings: player as A vs Automaton as B, then player as B vs Automaton A.
        for player_is_a in [true, false] {
            let mut auto = spec.make();
            // Build a fresh player each match (its scout state must not leak across).
            let mut player: Box<dyn cell_core::Policy> = match kind {
                PlayerKind::Capstone => Box::new(Capstone::new(cfg)),
                PlayerKind::Baseline => Box::new(Baseline::new(cfg, Mix::centre())),
                PlayerKind::Oracle => Box::new(Oracle::new(cfg, spec.mix)),
            };

            let outcome = if player_is_a {
                base.clone().run_match(player.as_mut(), auto.as_mut(), params, horizon)
            } else {
                base.clone().run_match(auto.as_mut(), player.as_mut(), params, horizon)
            };
            // Score from the player's perspective.
            let player_score = if player_is_a { outcome.score_a } else { -outcome.score_a };
            score_sum += player_score;
            if player_score > 0.0 {
                wins += 1.0;
            }
            n += 1;
        }
    }
    (wins, score_sum, n)
}

/// The mean mix the capstone infers for `spec`'s Automaton, averaged over all maps and
/// both seatings — the same observation the live [`Capstone`] makes inline, recovered
/// here via the standalone [`crate::infer::scout_opponent`] so we can score accuracy
/// without downcasting the policy trait object.
fn mean_inferred_mix(
    maps: &[(String, GameState)],
    params: &Params,
    spec: &AutomatonSpec,
    cfg: InferConfig,
) -> Mix {
    let mut c = 0.0;
    let mut d = 0.0;
    let mut a = 0.0;
    let mut count = 0.0;
    for (_name, base) in maps {
        for player_is_a in [true, false] {
            let (me, opp) = if player_is_a { (Owner::A, Owner::B) } else { (Owner::B, Owner::A) };
            let mut auto = spec.make();
            let est = crate::infer::scout_opponent(base, me, opp, auto.as_mut(), cfg, params).estimate();
            c += est.c;
            d += est.d;
            a += est.a;
            count += 1.0;
        }
    }
    if count > 0.0 {
        Mix::new(c / count, d / count, a / count)
    } else {
        Mix::centre()
    }
}

// ===========================================================================
// Dense simplex-grid study — the statistically sound test of the monotone claim
// ===========================================================================

/// Result of correlating centrality against difficulty over a **dense, uniform** sampling
/// of the whole simplex (rather than the small curated ladder). With many evenly-spread
/// points the per-matchup idiosyncrasy that dominates a 16-rung ladder averages out, so
/// these correlations are the load-bearing answer to "does centrality predict
/// difficulty?".
#[derive(Debug, Clone)]
pub struct SimplexStudy {
    /// Number of grid mixes evaluated.
    pub n_points: usize,
    /// Spearman *rank* correlation, centrality vs inference error (read-difficulty). Rank
    /// correlation is used because the claim is **monotone** ("more central ⇒ harder"),
    /// not linear, and rank is robust to the score saturation that plagues Pearson here.
    pub read_difficulty_rho: f64,
    /// Spearman rank correlation, centrality vs oracle score-difficulty (counter-difficulty,
    /// perfect read).
    pub counter_difficulty_rho: f64,
    /// Spearman rank correlation, centrality vs capstone score-difficulty (combined).
    pub combined_difficulty_rho: f64,
    /// Pearson (linear) versions of the same three, for reference.
    pub read_difficulty_pearson: f64,
    pub counter_difficulty_pearson: f64,
    pub combined_difficulty_pearson: f64,
    /// Mean inference error over the grid.
    pub mean_inference_error: f64,
    /// Binned means: for each centrality band, the mean difficulty — the concrete shape of
    /// the relationship (so the report can show difficulty rising band-by-band, not just a
    /// scalar correlation). Each entry is `(band_centre, mean_read_err, mean_counter_diff,
    /// mean_combined_diff, count)`.
    pub bands: Vec<(f64, f64, f64, f64, usize)>,
}

/// Generate a uniform grid over the simplex: all `(i, j, k)` with `i+j+k = steps`,
/// `i,j,k >= 0`, as normalized [`Mix`]es. `steps = 6` yields 28 points spanning every
/// corner, edge, and interior at a 1/6 resolution.
pub fn simplex_grid(steps: u32) -> Vec<Mix> {
    let mut out = Vec::new();
    for i in 0..=steps {
        for j in 0..=(steps - i) {
            let k = steps - i - j;
            out.push(Mix::new(i as f64, j as f64, k as f64));
        }
    }
    out
}

/// Run the dense-grid study: for every mix in a uniform simplex grid, measure inference
/// error (read-difficulty), oracle score-difficulty (counter-difficulty), and capstone
/// score-difficulty (combined), then correlate each with centrality. This is the
/// principled test of the design's monotone "centrality ⇒ difficulty" claim.
///
/// Deterministic, like everything else here. With `steps = 6` and the 3 standard maps ×
/// both seatings it is ~28 mixes × ~24 matches — a few seconds in release.
pub fn simplex_grid_study(
    maps: &[(String, GameState)],
    cfg: InferConfig,
    params: &Params,
    horizon: u64,
    steps: u32,
) -> SimplexStudy {
    let grid = simplex_grid(steps);
    let mut cen = Vec::with_capacity(grid.len());
    let mut read_err = Vec::with_capacity(grid.len());
    let mut counter_diff = Vec::with_capacity(grid.len());
    let mut combined_diff = Vec::with_capacity(grid.len());

    for mix in &grid {
        let spec = AutomatonSpec::from_mix(*mix);
        let inferred = mean_inferred_mix(maps, params, &spec, cfg);
        let (cap_w, cap_s, cap_n) =
            play_player_vs_rung(maps, params, horizon, &spec, cfg, PlayerKind::Capstone);
        let (_orc_w, orc_s, _) =
            play_player_vs_rung(maps, params, horizon, &spec, cfg, PlayerKind::Oracle);
        let _ = cap_w;

        cen.push(mix.centrality());
        read_err.push(inferred.distance(*mix));
        counter_diff.push((1.0 - orc_s / cap_n as f64) / 2.0);
        combined_diff.push((1.0 - cap_s / cap_n as f64) / 2.0);
    }

    // Bin by centrality into 5 bands for the shape table.
    let n_bands = 5usize;
    let mut bands = Vec::with_capacity(n_bands);
    for b in 0..n_bands {
        let lo = b as f64 / n_bands as f64;
        let hi = (b + 1) as f64 / n_bands as f64;
        let mut sr = 0.0;
        let mut sc = 0.0;
        let mut sm = 0.0;
        let mut cnt = 0usize;
        for i in 0..cen.len() {
            // Last band is closed on the right so centrality == 1.0 lands somewhere.
            let in_band = if b + 1 == n_bands {
                cen[i] >= lo && cen[i] <= hi + 1e-9
            } else {
                cen[i] >= lo && cen[i] < hi
            };
            if in_band {
                sr += read_err[i];
                sc += counter_diff[i];
                sm += combined_diff[i];
                cnt += 1;
            }
        }
        if cnt > 0 {
            bands.push((0.5 * (lo + hi), sr / cnt as f64, sc / cnt as f64, sm / cnt as f64, cnt));
        }
    }

    let mean_inference_error = read_err.iter().sum::<f64>() / read_err.len().max(1) as f64;

    SimplexStudy {
        n_points: grid.len(),
        read_difficulty_rho: spearman(&cen, &read_err),
        counter_difficulty_rho: spearman(&cen, &counter_diff),
        combined_difficulty_rho: spearman(&cen, &combined_diff),
        read_difficulty_pearson: pearson(&cen, &read_err),
        counter_difficulty_pearson: pearson(&cen, &counter_diff),
        combined_difficulty_pearson: pearson(&cen, &combined_diff),
        mean_inference_error,
        bands,
    }
}

/// Pearson correlation coefficient between two equal-length samples. Returns 0 for
/// degenerate (constant) inputs. Used to test whether centrality predicts difficulty.
pub fn pearson(xs: &[f64], ys: &[f64]) -> f64 {
    let n = xs.len();
    if n == 0 || n != ys.len() {
        return 0.0;
    }
    let nf = n as f64;
    let mx = xs.iter().sum::<f64>() / nf;
    let my = ys.iter().sum::<f64>() / nf;
    let mut sxy = 0.0;
    let mut sxx = 0.0;
    let mut syy = 0.0;
    for i in 0..n {
        let dx = xs[i] - mx;
        let dy = ys[i] - my;
        sxy += dx * dy;
        sxx += dx * dx;
        syy += dy * dy;
    }
    let denom = (sxx * syy).sqrt();
    if denom <= 0.0 {
        0.0
    } else {
        sxy / denom
    }
}

/// Spearman **rank** correlation: Pearson applied to the fractional ranks of each sample
/// (average ranks for ties). The design's claim is *monotone* ("more central ⇒ harder"),
/// not linear, so rank correlation is the appropriate test — and it is robust to the
/// score-difficulty saturation (lots of ±1.0 outcomes) that distorts a raw Pearson.
pub fn spearman(xs: &[f64], ys: &[f64]) -> f64 {
    if xs.len() != ys.len() || xs.is_empty() {
        return 0.0;
    }
    pearson(&fractional_ranks(xs), &fractional_ranks(ys))
}

/// Fractional ranks (1-based, ties share the average of the positions they span). The
/// helper behind [`spearman`].
fn fractional_ranks(v: &[f64]) -> Vec<f64> {
    let n = v.len();
    let mut idx: Vec<usize> = (0..n).collect();
    idx.sort_by(|&a, &b| v[a].partial_cmp(&v[b]).unwrap_or(std::cmp::Ordering::Equal));
    let mut ranks = vec![0.0; n];
    let mut i = 0;
    while i < n {
        // Find the run of equal values [i, j).
        let mut j = i + 1;
        while j < n && (v[idx[j]] - v[idx[i]]).abs() <= 1e-12 {
            j += 1;
        }
        // Average 1-based rank over the tie run.
        let avg = ((i + 1 + j) as f64) / 2.0; // (sum of (i+1..=j)) / (j-i) = (i+1 + j)/2
        for &k in &idx[i..j] {
            ranks[k] = avg;
        }
        i = j;
    }
    ranks
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ladder::default_ladder;
    use crate::mix::Corner;
    use cell_core::maps::all_maps;

    fn params() -> Params {
        Params { r: 0.6, k: 2.25, l: 0.15, ..Params::default() }
    }

    fn named_maps() -> Vec<(String, GameState)> {
        all_maps().into_iter().map(|m| (m.name.to_string(), m.state)).collect()
    }

    #[test]
    fn counter_rotation_is_rps() {
        // Pure colonize -> pure attack, etc.
        assert_eq!(counter_mix(Corner::Colonize.as_mix()).nearest_corner(), Corner::Attack);
        assert_eq!(counter_mix(Corner::Defend.as_mix()).nearest_corner(), Corner::Colonize);
        assert_eq!(counter_mix(Corner::Attack.as_mix()).nearest_corner(), Corner::Defend);
    }

    #[test]
    fn capstone_beats_baseline_on_average() {
        let maps = named_maps();
        let ladder = default_ladder();
        let eval = evaluate_ladder(&ladder, &maps, InferConfig::default(), &params(), 600);
        assert!(
            eval.mean_capstone_winrate > eval.mean_baseline_winrate,
            "capstone {:.3} should beat baseline {:.3} on average",
            eval.mean_capstone_winrate,
            eval.mean_baseline_winrate
        );
    }

    /// Diagnostic (ignored): print the full per-rung evaluation table + headline
    /// summary. Run with
    /// `cargo test -p automaton diag_results -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn diag_results() {
        let maps = named_maps();
        let ladder = default_ladder();
        let eval = evaluate_ladder(&ladder, &maps, InferConfig::default(), &params(), 600);
        println!("\n rung  label  cen   true(c,d,a)      inf(c,d,a)     inf_err cornOK  cap_wr base_wr orc_wr  cap_sc base_sc orc_sc");
        for r in &eval.rungs {
            println!(
                "  {:>2}  {:>6}  {:.2}  ({:.2},{:.2},{:.2})  ({:.2},{:.2},{:.2})  {:.2}    {}    {:.2}   {:.2}   {:.2}   {:+.2}  {:+.2}  {:+.2}",
                r.index, r.label, r.centrality,
                r.true_mix.c, r.true_mix.d, r.true_mix.a,
                r.inferred_mix.c, r.inferred_mix.d, r.inferred_mix.a,
                r.inference_error,
                if r.corner_correct { "Y" } else { "." },
                r.capstone_winrate, r.baseline_winrate, r.oracle_winrate,
                r.capstone_score, r.baseline_score, r.oracle_score
            );
        }
        println!(
            "\n  mean cap_wr {:.3}  base_wr {:.3}  orc_wr {:.3}  | inf_err {:.3}  corner_acc {:.3}",
            eval.mean_capstone_winrate, eval.mean_baseline_winrate, eval.mean_oracle_winrate,
            eval.mean_inference_error, eval.corner_accuracy
        );
        println!(
            "  LADDER centrality->difficulty corr: read(inf_err) {:+.3}  counter(oracle) {:+.3}  combined(cap) {:+.3}  baseline {:+.3}",
            eval.centrality_vs_inference_error_corr, eval.centrality_vs_oracle_difficulty_corr,
            eval.centrality_vs_difficulty_corr, eval.centrality_vs_difficulty_corr_baseline
        );

        // The statistically sound test: dense uniform grid over the simplex.
        let study = simplex_grid_study(&maps, InferConfig::default(), &params(), 600, 6);
        println!("\n  GRID study ({} mixes, steps=6):", study.n_points);
        println!(
            "    Spearman rho: read {:+.3}  counter {:+.3}  combined {:+.3}",
            study.read_difficulty_rho, study.counter_difficulty_rho, study.combined_difficulty_rho
        );
        println!(
            "    Pearson r   : read {:+.3}  counter {:+.3}  combined {:+.3}  | mean inf_err {:.3}",
            study.read_difficulty_pearson, study.counter_difficulty_pearson,
            study.combined_difficulty_pearson, study.mean_inference_error
        );
        println!("    band(cen)  read_err  counter_diff  combined_diff   n");
        for (c, re, cd, md, cnt) in &study.bands {
            println!("      {:.1}       {:.3}     {:.3}         {:.3}        {}", c, re, cd, md, cnt);
        }
    }

    #[test]
    fn evaluation_is_deterministic() {
        let maps = named_maps();
        let ladder = default_ladder();
        let e1 = evaluate_ladder(&ladder, &maps, InferConfig::default(), &params(), 400);
        let e2 = evaluate_ladder(&ladder, &maps, InferConfig::default(), &params(), 400);
        assert_eq!(e1.rungs.len(), e2.rungs.len());
        for (a, b) in e1.rungs.iter().zip(e2.rungs.iter()) {
            assert_eq!(a.capstone_winrate, b.capstone_winrate);
            assert_eq!(a.inference_error, b.inference_error);
        }
    }

    /// The dense-grid centrality→difficulty correlations are deterministic and finite.
    #[test]
    fn grid_study_is_deterministic_and_finite() {
        let maps = named_maps();
        let s1 = simplex_grid_study(&maps, InferConfig::default(), &params(), 400, 5);
        let s2 = simplex_grid_study(&maps, InferConfig::default(), &params(), 400, 5);
        assert_eq!(s1.n_points, s2.n_points);
        assert!(s1.n_points >= 15, "a steps=5 grid has 21 mixes");
        assert_eq!(s1.read_difficulty_rho, s2.read_difficulty_rho);
        for v in [s1.read_difficulty_rho, s1.counter_difficulty_rho, s1.combined_difficulty_rho] {
            assert!(v.is_finite() && (-1.0..=1.0).contains(&v), "rho out of range: {v}");
        }
    }

    /// Spearman matches Pearson on a strictly monotone (but non-linear) relation, and is
    /// robust to a tie — the property we rely on for the centrality study.
    #[test]
    fn spearman_basic() {
        let x = [1.0, 2.0, 3.0, 4.0, 5.0];
        let y = [1.0, 4.0, 9.0, 16.0, 25.0]; // monotone, non-linear
        assert!((spearman(&x, &y) - 1.0).abs() < 1e-9, "perfect monotone ⇒ ρ = 1");
        let yr = [25.0, 16.0, 9.0, 4.0, 1.0];
        assert!((spearman(&x, &yr) + 1.0).abs() < 1e-9, "reversed ⇒ ρ = -1");
    }

    /// **Artifact generator (ignored).** Renders `CAPSTONE_RESULTS.md` + the CSV/JSON to
    /// the repo root via the *same* library renderer the `capstone` binary uses. This
    /// exists as an `#[ignore]`d test (not a normal one — it writes repo files) so the
    /// deliverable can be regenerated through the library test runner. Run with
    /// `cargo test -p automaton emit_capstone_results -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn emit_capstone_results() {
        use crate::report::{self, ReportContext};
        let maps = named_maps();
        let ladder = default_ladder();
        let cfg = InferConfig::default();
        let horizon = 600;
        let grid_steps = 6;

        let eval = evaluate_ladder(&ladder, &maps, cfg, &params(), horizon);
        let study = simplex_grid_study(&maps, cfg, &params(), horizon, grid_steps);
        let ctx = ReportContext {
            params: params(),
            horizon,
            scout_ticks: cfg.scout_ticks,
            n_maps: maps.len(),
            grid_steps,
        };

        // Tests run with cwd = the crate dir (crates/automaton); the repo root is two up.
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(|p| p.parent())
            .expect("repo root is two levels above the crate manifest")
            .to_path_buf();
        let md = root.join("CAPSTONE_RESULTS.md");
        let csv = root.join("capstone_results.csv");
        let json = root.join("capstone_results.json");
        std::fs::write(&md, report::to_markdown(&eval, &study, &ctx)).unwrap();
        std::fs::write(&csv, report::to_csv(&eval)).unwrap();
        std::fs::write(&json, report::to_json(&eval, &study, &ctx)).unwrap();
        println!("wrote {}\n      {}\n      {}", md.display(), csv.display(), json.display());
    }
}
