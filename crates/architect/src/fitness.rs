//! **Fitness**: the four pressures the design (`02-ai-opponents.md`, `04` R3/R4) puts on
//! the Architect, plus the Schmidhuber **success-story acceptance gate**.
//!
//! A candidate genome's fitness blends four legible terms, each tracing to the docs:
//!
//! 1. **Coevolution (mean-field self-play).** `02`: "fitness from fast simulated
//!    coevolution (cheap mean-field matches)." The candidate plays a *sample of the
//!    current population* on the standard maps, both seatings. This is what makes fitness
//!    **relative** — beating today's population, not an absolute yardstick — and so is the
//!    term that can sustain (or fail to sustain) an **arms race** (R1).
//! 2. **Cached player model.** `02`: "plus performance against a cached model of the
//!    player." The candidate also plays a fixed [`PlayerModel`] (a cached opponent). Per
//!    R4 (overfitting), this is *blended* with — never replaces — self-play, and the
//!    model is a fixed competent policy rather than a stale moving target.
//! 3. **Parsimony penalty.** `02`/`04` R3: "a parsimony penalty favoring small rule-sets
//!    for legibility." A small per-rule cost. This is the legibility lever — it bounds the
//!    champion's size so the glass genome stays readable, at an accepted strength ceiling.
//! 4. **(Acceptance gate, not a score term) Wins-as-test-cases regression archive.** `02`:
//!    "every line the player beats it with is added to a regression archive the next genome
//!    must still survive … accept a self-modification only if it does not regress past
//!    performance (Schmidhuber's success-story algorithm)." Implemented as [`Archive`] +
//!    [`Archive::accepts`]: a *new champion* is installed only if it still beats every
//!    archived past-beaten configuration. Exploits the player found stay closed.
//!
//! Everything here calls only the deterministic `cell_core` engine, so a genome's fitness
//! is a pure function of `(genome, opponents, maps, params)` — bit-reproducible.

use cell_core::{GameState, Params};

use crate::genome::Genome;

/// The match horizon for fitness matches. Long enough that the triad's timing structure
/// resolves (an Attack reaches contact, a Colonize's economy compounds), matching the
/// horizon the R2/capstone work validated the cycle at.
pub const HORIZON: u64 = 600;

/// Per-rule parsimony cost subtracted from fitness. Tuned (see `ARCHITECT_RESULTS.md`) so
/// it is a real but gentle pressure: it visibly bounds champion size and breaks ties
/// toward shorter policies, without being so harsh that it forbids a genuinely useful
/// fourth/fifth rule. This is the R3 legibility/strength dial.
pub const PARSIMONY_COST: f64 = 0.015;

/// Weight on the cached-player-model term in the blended objective (the rest is
/// coevolution). Per R4 we keep self-play dominant so the population does not overfit a
/// single cached opponent; the player model is a meaningful minority of the signal.
pub const PLAYER_MODEL_WEIGHT: f64 = 0.35;

/// A cached **player model**: a fixed opponent standing in for "the player," whom the
/// Architect partly optimizes against (`02`). Kept as a [`Genome`] so it is expressed in
/// the *same* legible substrate as the organisms (one substrate, four minds) and so the
/// model itself could later be swapped for an Apprentice-induced rule-set without changing
/// this code. It is **fixed** across the run on purpose: chasing a moving/stale model is
/// exactly the R4 failure mode, so the model is a stable, competent reference policy.
#[derive(Clone)]
pub struct PlayerModel {
    pub genome: Genome,
    pub label: &'static str,
}

impl PlayerModel {
    /// A fresh boxed engine policy for the cached player.
    pub fn make(&self) -> Box<dyn cell_core::Policy> {
        self.genome.make_player()
    }
}

/// Mean score for `first` over **both seatings** on one base state, in [-1, 1] (cancels
/// the engine's "A acts before B" tie-break + positional bias). The fair-duel primitive
/// reused throughout the build (`harness::duel`, the DSL/automaton tests).
pub fn duel_score(
    base: &GameState,
    first: &Genome,
    second: &dyn Fn() -> Box<dyn cell_core::Policy>,
    params: &Params,
) -> f64 {
    let s1 = base
        .clone()
        .run_match(first.make_player().as_mut(), second().as_mut(), params, HORIZON)
        .score_a;
    let s2 = -base
        .clone()
        .run_match(second().as_mut(), first.make_player().as_mut(), params, HORIZON)
        .score_a;
    0.5 * (s1 + s2)
}

/// Mean `duel_score` of `first` vs `second` across all `maps`.
pub fn duel_over_maps(
    maps: &[GameState],
    first: &Genome,
    second: &dyn Fn() -> Box<dyn cell_core::Policy>,
    params: &Params,
) -> f64 {
    if maps.is_empty() {
        return 0.0;
    }
    maps.iter().map(|m| duel_score(m, first, second, params)).sum::<f64>() / maps.len() as f64
}

/// The decomposed fitness of a candidate, so the loop/report can show *why* a genome
/// scored what it did (the glass-genome spirit extends to the fitness breakdown).
#[derive(Debug, Clone, Copy)]
pub struct FitnessBreakdown {
    /// Mean coevolution score vs the sampled opponents (both seatings, all maps), [-1,1].
    pub coevolution: f64,
    /// Mean score vs the cached player model (both seatings, all maps), [-1,1].
    pub player_model: f64,
    /// The parsimony penalty actually subtracted (= `PARSIMONY_COST * rule_count`).
    pub parsimony_penalty: f64,
    /// Rule count (parsimony measure).
    pub rule_count: usize,
    /// The final blended fitness:
    /// `(1-w)·coevolution + w·player_model − parsimony_penalty`.
    pub fitness: f64,
}

/// Evaluate a candidate's blended fitness against a set of `opponents` (the coevolution
/// sample) and the cached `player_model`, on `maps` at `params`.
///
/// `opponents` are passed as genomes (a sample of the current population — see
/// [`crate::population`]); a candidate never plays *itself* (the caller excludes it).
pub fn evaluate(
    candidate: &Genome,
    opponents: &[Genome],
    player_model: &PlayerModel,
    maps: &[GameState],
    params: &Params,
) -> FitnessBreakdown {
    // --- coevolution: mean score vs each sampled opponent over all maps/seatings ---
    let coevolution = if opponents.is_empty() {
        0.0
    } else {
        let mut sum = 0.0;
        for opp in opponents {
            // Clone the opponent genome into the duel closure (cheap: a small Vec<Gene>).
            let opp = opp.clone();
            sum += duel_over_maps(maps, candidate, &move || opp.make_player(), params);
        }
        sum / opponents.len() as f64
    };

    // --- cached player model term ---
    let pm = player_model.clone();
    let player_model_score = duel_over_maps(maps, candidate, &move || pm.make(), params);

    // --- parsimony ---
    let rule_count = candidate.rule_count();
    let parsimony_penalty = PARSIMONY_COST * rule_count as f64;

    let w = PLAYER_MODEL_WEIGHT;
    let fitness = (1.0 - w) * coevolution + w * player_model_score - parsimony_penalty;

    FitnessBreakdown {
        coevolution,
        player_model: player_model_score,
        parsimony_penalty,
        rule_count,
        fitness,
    }
}

// ===========================================================================
// The wins-as-test-cases regression archive (Schmidhuber success-story gate)
// ===========================================================================

/// One archived "win": a past configuration the Architect was beaten by (in the game, a
/// line the *player* won with). The next champion must still beat it. We store the
/// opponent as a [`Genome`] (the legible substrate) plus the margin the *then-champion*
/// failed to clear, for reporting.
#[derive(Clone)]
pub struct ArchiveEntry {
    /// The configuration that must remain beaten.
    pub opponent: Genome,
    /// A human label (e.g. "exploit#3: attack-heavy").
    pub label: String,
    /// The minimum margin a champion must achieve vs this entry to "still beat it".
    /// Usually a small positive epsilon — the entry was added because a champion *failed*
    /// to beat it, so re-clearing it by a hair is enough to count as "did not regress".
    pub required_margin: f64,
}

/// The growing regression archive. A candidate is **accepted as the new champion only if
/// it does not regress** any archived entry — the success-story acceptance test (`02`).
///
/// This is deliberately a *gate*, not a fitness term: the design wants exploits to be
/// **visibly closed and to stay closed**, which a soft penalty cannot guarantee (a strong
/// genome could pay the penalty and still regress an old fix). A hard gate makes "every
/// exploit the player finds is closed forever" a structural property of the loop.
#[derive(Clone, Default)]
pub struct Archive {
    pub entries: Vec<ArchiveEntry>,
}

impl Archive {
    pub fn new() -> Archive {
        Archive { entries: Vec::new() }
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Add a configuration to the archive (an exploit to be kept closed). `label`
    /// describes it for the report; `required_margin` is the bar future champions must
    /// clear against it.
    pub fn add(&mut self, opponent: Genome, label: String, required_margin: f64) {
        self.entries.push(ArchiveEntry { opponent, label, required_margin });
    }

    /// Does `candidate` still beat **every** archived entry (by at least each entry's
    /// required margin)? This is the acceptance predicate: `true` ⇒ no regression ⇒ the
    /// self-modification may be installed.
    ///
    /// Returns `(accepts, regressed)` where `regressed` lists the labels of entries the
    /// candidate fails — so the loop can *show* which past fix a rejected candidate would
    /// have broken (legibility of the gate itself).
    pub fn accepts(
        &self,
        candidate: &Genome,
        maps: &[GameState],
        params: &Params,
    ) -> (bool, Vec<String>) {
        let mut regressed = Vec::new();
        for e in &self.entries {
            let opp = e.opponent.clone();
            let margin = duel_over_maps(maps, candidate, &move || opp.make_player(), params);
            if margin < e.required_margin {
                regressed.push(e.label.clone());
            }
        }
        (regressed.is_empty(), regressed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::genome::seed_archetypes;
    use cell_core::maps::all_maps;

    fn params() -> Params {
        Params { r: 0.6, k: 2.25, l: 0.15, ..Params::default() }
    }

    fn maps() -> Vec<GameState> {
        all_maps().into_iter().map(|m| m.state).collect()
    }

    fn player_model() -> PlayerModel {
        // A competent colonize-leaning reference (the seed colonize archetype).
        PlayerModel { genome: seed_archetypes()[0].clone(), label: "cached: colonize" }
    }

    #[test]
    fn fitness_is_deterministic() {
        let m = maps();
        let cand = seed_archetypes()[2].clone(); // attack
        let opps = vec![seed_archetypes()[0].clone(), seed_archetypes()[1].clone()];
        let f1 = evaluate(&cand, &opps, &player_model(), &m, &params());
        let f2 = evaluate(&cand, &opps, &player_model(), &m, &params());
        assert_eq!(f1.fitness, f2.fitness, "fitness must be deterministic");
        assert_eq!(f1.coevolution, f2.coevolution);
    }

    #[test]
    fn parsimony_penalizes_larger_genomes() {
        // Two genomes that play similarly but differ in rule count: the larger pays more.
        let m = maps();
        let opps = vec![seed_archetypes()[1].clone()];
        let small = seed_archetypes()[0].clone(); // colonize: 2 rules
        let big = seed_archetypes()[1].clone(); // defend: 4 rules
        let fs = evaluate(&small, &opps, &player_model(), &m, &params());
        let fb = evaluate(&big, &opps, &player_model(), &m, &params());
        assert!(fb.parsimony_penalty > fs.parsimony_penalty, "more rules ⇒ bigger penalty");
        assert_eq!(fs.parsimony_penalty, PARSIMONY_COST * 2.0);
    }

    #[test]
    fn triad_shows_up_in_coevolution_scores() {
        // Sanity that the coevolution term reflects the real cycle: attack scores well
        // against a colonize-only opponent sample (attack ≻ colonize).
        let m = maps();
        let attack = seed_archetypes()[2].clone();
        let colonize_opp = vec![seed_archetypes()[0].clone()];
        let f = evaluate(&attack, &colonize_opp, &player_model(), &m, &params());
        assert!(f.coevolution > 0.0, "attack should beat a colonize sample, got {}", f.coevolution);
    }

    #[test]
    fn archive_gate_blocks_regression() {
        // Build an archive containing a defender (which beats attack). An attack-heavy
        // candidate must FAIL the gate (it regresses the defend fix); a defender PASSES.
        let m = maps();
        let mut arc = Archive::new();
        arc.add(seed_archetypes()[1].clone(), "exploit: defend".to_string(), 0.02);

        let attack = seed_archetypes()[2].clone();
        let (acc_atk, regressed) = arc.accepts(&attack, &m, &params());
        assert!(!acc_atk, "attack should regress the archived defender");
        assert_eq!(regressed, vec!["exploit: defend".to_string()]);

        // A colonizer beats a defender (colonize ≻ defend), so it should pass the gate.
        let colonize = seed_archetypes()[0].clone();
        let (acc_col, _) = arc.accepts(&colonize, &m, &params());
        assert!(acc_col, "colonize should still beat the archived defender (no regression)");
    }

    #[test]
    fn empty_archive_accepts_everything() {
        let m = maps();
        let arc = Archive::new();
        let (acc, regressed) = arc.accepts(&seed_archetypes()[0].clone(), &m, &params());
        assert!(acc && regressed.is_empty());
    }
}
