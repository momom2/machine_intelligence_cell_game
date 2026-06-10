//! **Variation operators**: how a child genome is produced from parents using the
//! parents' *own* (inherited, evolving) [`Operators`].
//!
//! This module is the operational core of the **autoconstructive** loop. The key point
//! (`02`, Spector's Pushpop) is that the rates driving these operators are **not global
//! hyperparameters** — they are read from the genome being reproduced, and the child's
//! rates are themselves produced by [`Operators::reproduce`]. So the search adapts *how
//! it searches*: a lineage that benefits from aggressive structural change will tend to
//! carry a high `p_toggle_gene`, while one that has found a good structure and only needs
//! fine-tuning will drift toward low toggle / high threshold-tweak rates. Selection sees
//! the *outcome* of an operator set (the child's fitness) and so indirectly selects the
//! operators.
//!
//! Two paths:
//! * [`recombine`] — sexual reproduction: cross two parents gene-slot by gene-slot using
//!   parent A's `crossover_bias`, inherit+meta-mutate the operators, then mutate the
//!   child's genome under the child's own (new) operators.
//! * [`mutate_only`] — asexual reproduction: clone a parent, inherit+meta-mutate its
//!   operators, and mutate under those.

use crate::genome::{catalog_len, Gene, Genome};
use crate::rng::Rng;

/// The fixed **meta-mutation rate**: how much an operator value may drift per
/// reproduction (multiplicative jitter bound in [`Operators::reproduce`]). This is the
/// *only* global evolutionary constant — and it governs the drift of the operators, not
/// the genome, so the genome's evolution is still driven entirely by the evolved
/// operators (the autoconstructive property). Kept modest so operator drift is a slow
/// second-order effect that selection can steer rather than noise that swamps it.
pub const META_RATE: f64 = 0.25;

/// Recombine two parents into a child genome.
///
/// Steps (in order, all deterministic given `rng`):
/// 1. **Operator inheritance + meta-mutation.** The child's operators come from parent A's
///    operators via [`Operators::reproduce`] (so the reproduction machinery evolves). We
///    use A as the operator parent by convention; since pairings are unordered draws from
///    the elite, both orders occur across the generation.
/// 2. **Gene-slot crossover.** For each catalog id, decide independently whether the child
///    inherits that slot's gene (if present) from A or B, biased by the child's
///    `crossover_bias`. A gene present in only one parent is taken with that same
///    per-slot coin, so structure is genuinely mixed rather than unioned.
/// 3. **Mutation under the child's own operators** (see [`mutate_genome`]).
pub fn recombine(a: &Genome, b: &Genome, rng: &mut Rng) -> Genome {
    let child_ops = a.operators.reproduce(rng, META_RATE);

    // Index each parent's genes by catalog id for slot-wise crossover.
    let mut genes: Vec<Gene> = Vec::new();
    for id in 0..catalog_len() {
        let ga = a.genes.iter().find(|g| g.template_id == id).copied();
        let gb = b.genes.iter().find(|g| g.template_id == id).copied();
        // Per-slot inheritance coin, biased toward A by the child's crossover_bias.
        let take_a = rng.chance(child_ops.crossover_bias);
        let chosen = match (ga, gb) {
            (Some(x), Some(y)) => {
                // Both have it: inherit one parent's threshold/offset, but blend the
                // threshold halfway sometimes so crossover can interpolate a setting the
                // arms race is converging on (a legible "averaging" move).
                let mut g = if take_a { x } else { y };
                if rng.chance(0.5) {
                    g.threshold = 0.5 * (x.threshold + y.threshold);
                    g.priority_offset = (x.priority_offset + y.priority_offset) / 2;
                }
                Some(g)
            }
            (Some(x), None) => {
                if take_a {
                    Some(x)
                } else {
                    None
                }
            }
            (None, Some(y)) => {
                if take_a {
                    None
                } else {
                    Some(y)
                }
            }
            (None, None) => None,
        };
        if let Some(g) = chosen {
            genes.push(g);
        }
    }

    let mut child = Genome { genes, operators: child_ops };
    child.normalize();
    mutate_genome(&mut child, rng);
    child
}

/// Asexual reproduction: clone `parent`, evolve its operators, mutate under them.
pub fn mutate_only(parent: &Genome, rng: &mut Rng) -> Genome {
    let child_ops = parent.operators.reproduce(rng, META_RATE);
    let mut child = Genome { genes: parent.genes.clone(), operators: child_ops };
    mutate_genome(&mut child, rng);
    child
}

/// Mutate a genome **in place** using *its own* operators (the rates already inherited
/// onto it). This is where `p_toggle_gene` / `p_tweak_threshold` / `p_tweak_priority` /
/// `threshold_step` actually act, so a lineage's evolved rates shape its own offspring.
///
/// The three legible moves, matching the diff grammar (`+`/`-`/`~`):
/// * **toggle a gene** (`+`/`-`): with `p_toggle_gene`, flip the presence of one random
///   catalog id — the structural move that adds or removes a readable rule.
/// * **tweak a threshold** (`~`): with `p_tweak_threshold`, nudge a present gene's
///   condition constant by a Gaussian-ish step scaled by `threshold_step` × its range.
/// * **reorder** (`~`): with `p_tweak_priority`, nudge a present gene's priority offset by
///   ±1 (a within-band reordering — the legible "I considered this rule sooner" move).
pub fn mutate_genome(g: &mut Genome, rng: &mut Rng) {
    let ops = g.operators;

    // --- structural: toggle one random catalog id ---
    if rng.chance(ops.p_toggle_gene) {
        let id = rng.below(catalog_len());
        if let Some(pos) = g.genes.iter().position(|x| x.template_id == id) {
            // Drop it — but never below one rule (keep the genome a real player).
            if g.genes.len() > 1 {
                g.genes.remove(pos);
            }
        } else {
            // Add it fresh (default threshold, small random offset).
            let mut gene = Gene::fresh(id);
            gene.priority_offset = (rng.below(9) as i32) - 4;
            g.genes.push(gene);
        }
    }

    // --- threshold tweak on each present gene (independently) ---
    for gene in &mut g.genes {
        if rng.chance(ops.p_tweak_threshold) {
            let t = gene.template();
            let span = t.threshold_hi - t.threshold_lo;
            // Two uniforms make a rough triangular step centered at 0 (a cheap
            // Gaussian-ish kick, no external dependency).
            let step = (rng.next_f64() - rng.next_f64()) * ops.threshold_step * span;
            gene.threshold = (gene.threshold + step).clamp(t.threshold_lo, t.threshold_hi);
        }
        if rng.chance(ops.p_tweak_priority) {
            let delta = if rng.chance(0.5) { 1 } else { -1 };
            gene.priority_offset = (gene.priority_offset + delta).clamp(-20, 20);
        }
    }

    g.normalize();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::genome::seed_archetypes;

    #[test]
    fn recombination_is_deterministic_from_seed() {
        let arches = seed_archetypes();
        let mut r1 = Rng::new(42);
        let mut r2 = Rng::new(42);
        let c1 = recombine(&arches[0], &arches[2], &mut r1);
        let c2 = recombine(&arches[0], &arches[2], &mut r2);
        // Same seed ⇒ identical structure (gene ids, thresholds, operators).
        let ids1: Vec<usize> = c1.genes.iter().map(|g| g.template_id).collect();
        let ids2: Vec<usize> = c2.genes.iter().map(|g| g.template_id).collect();
        assert_eq!(ids1, ids2);
        assert_eq!(c1.operators.p_toggle_gene, c2.operators.p_toggle_gene);
    }

    #[test]
    fn child_inherits_only_catalog_genes() {
        let arches = seed_archetypes();
        let mut rng = Rng::new(7);
        for _ in 0..200 {
            let child = recombine(&arches[0], &arches[1], &mut rng);
            for g in &child.genes {
                assert!(g.template_id < catalog_len(), "child gene id in catalog");
            }
            // Invariant: at most one gene per id.
            let mut ids: Vec<usize> = child.genes.iter().map(|g| g.template_id).collect();
            ids.sort_unstable();
            ids.dedup();
            assert_eq!(ids.len(), child.genes.len(), "no duplicate gene ids");
            assert!(child.rule_count() >= 1, "child is a real player");
        }
    }

    #[test]
    fn operators_drift_but_stay_bounded_over_many_generations() {
        // Simulate a long asexual lineage: operators must never leave their legal bands,
        // proving the meta-mutation is bounded (no runaway rates).
        let mut g = seed_archetypes()[1].clone();
        let mut rng = Rng::new(2024);
        for _ in 0..5000 {
            g = mutate_only(&g, &mut rng);
            let o = g.operators;
            assert!((0.02..=0.60).contains(&o.p_toggle_gene));
            assert!((0.05..=0.95).contains(&o.p_tweak_threshold));
            assert!((0.02..=0.60).contains(&o.p_tweak_priority));
            assert!((0.02..=0.50).contains(&o.threshold_step));
            assert!((0.15..=0.85).contains(&o.crossover_bias));
            assert!(g.rule_count() >= 1);
        }
    }

    #[test]
    fn mutation_actually_changes_genomes_sometimes() {
        // Over many trials with default operators, mutation should produce *some* change
        // (otherwise evolution is inert). We check the ensemble, not a single draw.
        let parent = seed_archetypes()[2].clone();
        let mut rng = Rng::new(5);
        let mut changed = 0;
        for _ in 0..100 {
            let child = mutate_only(&parent, &mut rng);
            let same_len = child.rule_count() == parent.rule_count();
            let same_ids = {
                let mut a: Vec<usize> = child.genes.iter().map(|g| g.template_id).collect();
                let mut b: Vec<usize> = parent.genes.iter().map(|g| g.template_id).collect();
                a.sort_unstable();
                b.sort_unstable();
                a == b
            };
            if !(same_len && same_ids) {
                changed += 1;
            }
        }
        assert!(changed > 0, "mutation never changed structure in 100 tries");
    }
}
