//! The **ladder of Automatons**: a curated set of fixed mixes ordered by increasing
//! centrality (the difficulty metric from `02-ai-opponents.md`).
//!
//! The ladder starts at the three *pure corners* (cleanest counters, easiest reads),
//! steps through the three *edge midpoints* (two-strategy blends), and culminates at
//! the *balanced centre* (the hardest mix to read and to counter). Ordering by
//! centrality is the design's prescription — "order the ladder by increasing
//! centrality" — and the validation in [`crate::capstone`] tests whether that ordering
//! actually predicts difficulty.

use crate::automaton::AutomatonSpec;
use crate::mix::{Corner, Mix};

/// One rung of the ladder: an Automaton plus its 1-based rung index. Rungs are sorted
/// by ascending centrality, so a higher index is (by the design's metric) harder.
pub struct Rung {
    /// 1-based position in the ladder (1 = easiest / most pure).
    pub index: usize,
    /// The Automaton at this rung (carries the hidden mix + compiled policy).
    pub spec: AutomatonSpec,
}

impl Rung {
    /// Convenience: the rung's centrality (its difficulty per the design metric).
    pub fn centrality(&self) -> f64 {
        self.spec.centrality()
    }
}

/// The default ladder: corners, edge-midpoints, lopsided two-strategy blends, and the
/// centre — ten rungs spanning the full centrality range, ordered easiest → hardest.
///
/// The specific mixes are chosen to sample the simplex from rim to centre so the
/// centrality-vs-difficulty relationship is tested across the whole range, not just at
/// the extremes:
/// * **corners** `(C, D, A)` — centrality 0 (pure, cleanest counter);
/// * **lopsided edges** like 2:1 blends — slightly central;
/// * **edge midpoints** `(CD, DA, CA)` — moderately central;
/// * **centre** `(1/3,1/3,1/3)` — centrality 1 (hardest).
pub fn default_ladder() -> Vec<Rung> {
    let mixes = vec![
        // --- Pure corners (centrality 0; cleanest counter) ---
        Corner::Colonize.as_mix(),
        Corner::Defend.as_mix(),
        Corner::Attack.as_mix(),
        // --- Lopsided two-strategy blends (mildly central): ~3:1 then ~2:1 ---
        Mix::new(3.0, 1.0, 0.0),
        Mix::new(0.0, 3.0, 1.0),
        Mix::new(1.0, 0.0, 3.0),
        Mix::new(2.0, 1.0, 0.0), // colonize-leaning, some defend
        Mix::new(0.0, 2.0, 1.0), // defend-leaning, some attack
        Mix::new(1.0, 0.0, 2.0), // attack-leaning, some colonize
        // --- Even two-strategy blends (edge midpoints, moderately central) ---
        Mix::new(1.0, 1.0, 0.0), // colonize/defend
        Mix::new(0.0, 1.0, 1.0), // defend/attack
        Mix::new(1.0, 0.0, 1.0), // colonize/attack
        // --- Three-strategy blends approaching the centre (very central) ---
        Mix::new(2.0, 1.0, 1.0), // colonize-dominant tri-mix
        Mix::new(1.0, 2.0, 1.0), // defend-dominant tri-mix
        Mix::new(1.0, 1.0, 2.0), // attack-dominant tri-mix
        // --- Balanced centre (centrality 1, hardest) ---
        Mix::centre(),
    ];
    build_ladder(mixes)
}

/// Sort an arbitrary set of mixes into a centrality-ordered ladder (easiest first) and
/// assign 1-based indices. Exposed so the validation harness (and future curricula) can
/// build a ladder from any sampling of the simplex.
pub fn build_ladder(mixes: Vec<Mix>) -> Vec<Rung> {
    let mut specs: Vec<AutomatonSpec> = mixes.into_iter().map(AutomatonSpec::from_mix).collect();
    // Ascending centrality. Stable sort + a total tie-break on the label keeps the
    // order deterministic when two mixes share a centrality (e.g. the three corners,
    // all centrality 0, come out in C, D, A order).
    specs.sort_by(|x, y| {
        x.centrality()
            .partial_cmp(&y.centrality())
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| x.name.cmp(&y.name))
    });
    specs
        .into_iter()
        .enumerate()
        .map(|(i, spec)| Rung { index: i + 1, spec })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ladder_is_ordered_by_increasing_centrality() {
        let ladder = default_ladder();
        assert!(ladder.len() >= 7, "ladder should span the simplex");
        let mut prev = -1.0;
        for rung in &ladder {
            let cen = rung.centrality();
            assert!(cen >= prev - 1e-12, "rung {} centrality {cen} < prev {prev}", rung.index);
            prev = cen;
        }
        // First rungs are pure corners (centrality ~0); last is the balanced centre.
        assert!(ladder.first().unwrap().centrality() < 1e-9, "easiest rung is pure");
        assert!((ladder.last().unwrap().centrality() - 1.0).abs() < 1e-9, "hardest is centre");
    }

    #[test]
    fn indices_are_one_based_and_contiguous() {
        let ladder = default_ladder();
        for (i, rung) in ladder.iter().enumerate() {
            assert_eq!(rung.index, i + 1);
        }
    }
}
