//! **R1 instrumentation**: measure whether the population converges to a single dominant
//! program (the arms race dies) or sustains **strategic diversity / a non-transitive arms
//! race**. This module is the headline analysis the task asks for — "instrument it to
//! detect DOMINANT-POLICY COLLAPSE."
//!
//! ## Why two very different kinds of diversity (and why the second is the real test)
//!
//! `04` R1 is precise about what "collapse" means: it is **not** primarily about the
//! population looking genetically uniform — it is about the *strategy space* admitting a
//! **dominant meta-policy** ("every policy must have a counter-policy … otherwise both the
//! player's iteration loop and the Architect's evolution converge to a fixed point and
//! die"). So we measure two things:
//!
//! 1. **Genotypic diversity** ([`genotypic`]) — how varied the *rule-sets* are: the number
//!    of distinct gene-set "species", the Shannon entropy over those species, and the mean
//!    pairwise structural distance. Cheap, and a useful coarse signal (a population that has
//!    collapsed to one genome trivially has zero here), but **not sufficient**: many
//!    genetically-distinct genomes could still be strategically equivalent.
//!
//! 2. **Strategic non-transitivity** ([`non_transitivity`]) — the load-bearing R1 measure.
//!    Among a set of representative genomes we build the round-robin **payoff matrix**
//!    (who-beats-whom, mean over both seatings and all maps) and ask: is the beat-relation
//!    **transitive** (a clean dominance ranking ⇒ a top dog that beats everyone ⇒ collapse
//!    is possible) or does it contain **3-cycles** (A≻B≻C≻A ⇒ rock-paper-scissors in policy
//!    space ⇒ no dominant policy ⇒ the arms race can persist)? The fraction of intransitive
//!    triads is the single number that most directly answers R1.
//!
//! A *dominant-policy collapse* reads as: genotypic diversity → low **and** non-transitivity
//! → 0 (a strict dominance order emerges). *Diversity persists* reads as: non-transitivity
//! stays well above 0 (cycles survive) even if the population clusters, because then no
//! single genome can dominate the cycle.

use cell_core::{GameState, Params};

use crate::fitness::duel_over_maps;
use crate::genome::{catalog_len, Genome};

/// A compact, order-independent signature of a genome's *structure*: the sorted set of
/// catalog gene ids it contains. Two genomes with the same signature play the same rules
/// (possibly at different thresholds), so this is the right grain for counting strategic
/// "species" without overcounting threshold jitter.
pub fn gene_signature(g: &Genome) -> u64 {
    let mut bits: u64 = 0;
    for gene in &g.genes {
        if gene.template_id < 64 {
            bits |= 1u64 << gene.template_id;
        }
    }
    bits
}

/// Structural (genotypic) diversity of a population.
#[derive(Debug, Clone, Copy)]
pub struct Genotypic {
    /// Number of **distinct gene-set signatures** present (distinct "species" by rule
    /// composition). 1 ⇒ every organism runs the same rules (structural monoculture).
    pub distinct_species: usize,
    /// Shannon entropy (in bits) over the signature distribution, normalized to `[0,1]`
    /// by `log2(distinct_species_max)`. 0 ⇒ one species dominates the whole population;
    /// 1 ⇒ signatures are uniformly spread. The cleanest scalar "how varied is the gene
    /// pool".
    pub species_entropy: f64,
    /// Mean pairwise structural distance (Hamming distance over the gene-presence bitset,
    /// normalized by catalog size) — a continuous companion to the species count.
    pub mean_pairwise_distance: f64,
}

/// Compute genotypic diversity over a population of genomes.
pub fn genotypic(pop: &[Genome]) -> Genotypic {
    let n = pop.len();
    if n == 0 {
        return Genotypic { distinct_species: 0, species_entropy: 0.0, mean_pairwise_distance: 0.0 };
    }

    // --- species counts over gene-set signatures ---
    let sigs: Vec<u64> = pop.iter().map(gene_signature).collect();
    let mut uniq: Vec<u64> = sigs.clone();
    uniq.sort_unstable();
    uniq.dedup();
    let distinct_species = uniq.len();

    // Shannon entropy over the signature distribution.
    let mut entropy = 0.0;
    for &u in &uniq {
        let count = sigs.iter().filter(|&&s| s == u).count() as f64;
        let p = count / n as f64;
        if p > 0.0 {
            entropy -= p * p.log2();
        }
    }
    // Normalize by the max possible entropy given the number of species observed (so a
    // population that *could* only have k species and spreads evenly across them reads as
    // maximally diverse). Guard the single-species case.
    let species_entropy = if distinct_species > 1 {
        entropy / (distinct_species as f64).log2()
    } else {
        0.0
    };

    // --- mean pairwise Hamming distance over gene-presence bitsets ---
    let cat = catalog_len() as f64;
    let mut sum = 0.0;
    let mut pairs = 0.0;
    for i in 0..n {
        for j in (i + 1)..n {
            let xor = sigs[i] ^ sigs[j];
            sum += (xor.count_ones() as f64) / cat;
            pairs += 1.0;
        }
    }
    let mean_pairwise_distance = if pairs > 0.0 { sum / pairs } else { 0.0 };

    Genotypic { distinct_species, species_entropy, mean_pairwise_distance }
}

/// The strategic non-transitivity analysis — the core R1 measure.
#[derive(Debug, Clone)]
pub struct NonTransitivity {
    /// Number of representatives compared (matrix is `n × n`).
    pub n: usize,
    /// Fraction of unordered triples `{i,j,k}` whose beat-relation is **intransitive**
    /// (a 3-cycle A≻B≻C≻A). **The headline R1 number**: > 0 ⇒ rock-paper-scissors persists
    /// in policy space ⇒ no dominant policy ⇒ arms race can sustain. 0 ⇒ a strict dominance
    /// order ⇒ collapse is admissible.
    pub intransitive_triads: f64,
    /// The maximum number of opponents any single representative beats, as a fraction of
    /// `n-1`. **1.0 ⇒ a genome that beats everyone exists (a literal dominant policy).**
    /// Below 1.0 ⇒ even the best representative loses to someone (a counter exists).
    pub max_dominance: f64,
    /// Number of distinct representatives that are **unbeaten** by everyone else (no one in
    /// the set beats them). 1 unbeaten genome that also beats all others = collapse; a
    /// "top tier" with internal cycles = persistence.
    pub unbeaten_count: usize,
    /// The fraction of decisive (non-near-tie) ordered pairs — context for how meaningful
    /// the relation is (if almost everything is a draw, the other numbers are weak).
    pub decisiveness: f64,
}

/// Build the round-robin payoff matrix among `reps` and analyze its (in)transitivity.
///
/// `epsilon` is the margin above which one genome is said to "beat" another (so near-ties
/// do not spuriously create or break cycles). The matrix uses the same fair both-seatings
/// `duel_over_maps` the fitness uses, so the strategic relation measured here is exactly
/// the one selection acts on.
pub fn non_transitivity(
    reps: &[Genome],
    maps: &[GameState],
    params: &Params,
    epsilon: f64,
) -> NonTransitivity {
    let n = reps.len();
    if n < 2 {
        return NonTransitivity {
            n,
            intransitive_triads: 0.0,
            max_dominance: 0.0,
            unbeaten_count: n,
            decisiveness: 0.0,
        };
    }

    // beats[i][j] = true iff i beats j by > epsilon (mean over maps + both seatings).
    // Because duel_over_maps already averages both seatings, score(i vs j) = -score(j vs i)
    // up to float symmetry, so we compute the upper triangle and mirror it.
    let mut beats = vec![vec![false; n]; n];
    let mut decisive_pairs = 0.0;
    let mut total_pairs = 0.0;
    for i in 0..n {
        for j in (i + 1)..n {
            let gj = reps[j].clone();
            let s = duel_over_maps(maps, &reps[i], &move || gj.make_player(), params);
            total_pairs += 1.0;
            if s > epsilon {
                beats[i][j] = true;
                decisive_pairs += 1.0;
            } else if s < -epsilon {
                beats[j][i] = true;
                decisive_pairs += 1.0;
            }
            // |s| <= epsilon ⇒ a near-tie: neither beats the other.
        }
    }

    // --- intransitive triads: of all C(n,3) triples, how many contain a 3-cycle? ---
    let mut intransitive = 0.0;
    let mut triples = 0.0;
    for i in 0..n {
        for j in (i + 1)..n {
            for k in (j + 1)..n {
                triples += 1.0;
                if is_cyclic_triad(&beats, i, j, k) {
                    intransitive += 1.0;
                }
            }
        }
    }
    let intransitive_triads = if triples > 0.0 { intransitive / triples } else { 0.0 };

    // --- dominance: most wins by any single rep, and count of unbeaten reps ---
    let mut max_wins = 0usize;
    let mut unbeaten = 0usize;
    for i in 0..n {
        let wins = (0..n).filter(|&j| beats[i][j]).count();
        max_wins = max_wins.max(wins);
        let is_beaten = (0..n).any(|j| beats[j][i]);
        if !is_beaten {
            unbeaten += 1;
        }
    }
    let max_dominance = max_wins as f64 / (n - 1) as f64;
    let decisiveness = if total_pairs > 0.0 { decisive_pairs / total_pairs } else { 0.0 };

    NonTransitivity {
        n,
        intransitive_triads,
        max_dominance,
        unbeaten_count: unbeaten,
        decisiveness,
    }
}

/// Is the triple `{a,b,c}` a 3-cycle under the `beats` relation (in either rotational
/// direction)? A triad is intransitive iff each member beats exactly one other and loses
/// to the other — i.e. `a≻b≻c≻a` or `a≻c≻b≻a`.
fn is_cyclic_triad(beats: &[Vec<bool>], a: usize, b: usize, c: usize) -> bool {
    let ab = beats[a][b];
    let bc = beats[b][c];
    let ca = beats[c][a];
    let ba = beats[b][a];
    let cb = beats[c][b];
    let ac = beats[a][c];
    // Forward cycle a->b->c->a, or the reverse a->c->b->a.
    (ab && bc && ca) || (ac && cb && ba)
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

    #[test]
    fn monoculture_has_zero_genotypic_diversity() {
        let g = seed_archetypes()[0].clone();
        let pop = vec![g.clone(), g.clone(), g.clone()];
        let d = genotypic(&pop);
        assert_eq!(d.distinct_species, 1);
        assert_eq!(d.species_entropy, 0.0);
        assert_eq!(d.mean_pairwise_distance, 0.0);
    }

    #[test]
    fn varied_population_has_positive_diversity() {
        let pop = seed_archetypes(); // colonize/defend/attack: 3 distinct signatures
        let d = genotypic(&pop);
        assert_eq!(d.distinct_species, 3);
        assert!(d.species_entropy > 0.9, "3 distinct, evenly spread ⇒ near-max entropy");
        assert!(d.mean_pairwise_distance > 0.0);
    }

    /// **The key R1 sanity check**: the three triad archetypes form a rock-paper-scissors
    /// cycle, so the non-transitivity analysis must detect the single triad as cyclic
    /// (intransitive fraction = 1.0) and report that **no genome dominates** (max_dominance
    /// = 0.5: each beats exactly one of the other two). This proves the instrument actually
    /// detects a non-transitive arms race when one exists.
    #[test]
    fn detects_rps_cycle_among_archetypes() {
        let reps = seed_archetypes();
        let nt = non_transitivity(&reps, &maps(), &params(), 0.05);
        assert_eq!(nt.n, 3);
        assert!(
            (nt.intransitive_triads - 1.0).abs() < 1e-9,
            "the C/D/A triad is one cyclic triad, got {}",
            nt.intransitive_triads
        );
        // Each archetype beats exactly one of the other two ⇒ max wins = 1 of 2 = 0.5.
        assert!((nt.max_dominance - 0.5).abs() < 1e-9, "no dominant policy, got {}", nt.max_dominance);
        // In a perfect 3-cycle everyone is beaten by someone ⇒ zero unbeaten.
        assert_eq!(nt.unbeaten_count, 0, "a 3-cycle has no unbeaten member");
    }

    #[test]
    fn detects_dominance_when_no_cycle() {
        // Construct a transitive set by hand: a fabricated beats matrix where 0>1>2.
        // (Done at the matrix level via a tiny helper to avoid needing dominant genomes.)
        let beats = vec![
            vec![false, true, true],
            vec![false, false, true],
            vec![false, false, false],
        ];
        assert!(!is_cyclic_triad(&beats, 0, 1, 2), "0>1>2 is transitive, not a cycle");
    }

    #[test]
    fn non_transitivity_is_deterministic() {
        let reps = seed_archetypes();
        let a = non_transitivity(&reps, &maps(), &params(), 0.05);
        let b = non_transitivity(&reps, &maps(), &params(), 0.05);
        assert_eq!(a.intransitive_triads, b.intransitive_triads);
        assert_eq!(a.max_dominance, b.max_dominance);
    }
}
