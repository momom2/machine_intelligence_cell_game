//! The **glass genome**: render a champion rule-set legibly, and print the **diff**
//! between successive champions as `+`/`-`/`~` lines.
//!
//! `02-ai-opponents.md` calls this "the single most important 'feel' mechanic in the
//! game": the player watches the Architect's policy "mutate and grow between matches as
//! diffs (`+ rule: defend rear when frontier is committed`)." Autoconstruction becomes a
//! *spectatable* process rather than a black box. Here that is literal: a champion is a
//! short list of legible gene `note`s, and the generation-to-generation change is a small
//! set of added / removed / retuned rules.
//!
//! The diff grammar (kept deliberately tiny so it reads like patch notes):
//! * `+ <tag>: <note>` — a rule the new champion **added**.
//! * `- <tag>: <note>` — a rule it **removed**.
//! * `~ <tag>: <change>` — a rule it **retuned** (threshold) or **reordered** (priority).

use std::fmt::Write as _;

use crate::genome::{Gene, Genome};

/// Render a champion genome as a legible, priority-ordered rule list (the "current glass
/// genome" view). Each line is `[prio] tag (thr=…): note`.
pub fn render_genome(g: &Genome) -> String {
    let mut genes: Vec<&Gene> = g.genes.iter().collect();
    // Show in evaluation order (highest priority first), exactly how the engine reads it.
    genes.sort_by_key(|x| std::cmp::Reverse(x.priority()));
    let mut s = String::new();
    for gene in genes {
        let t = gene.template();
        let _ = writeln!(
            s,
            "  [{:>3}] {:<10} (thr={:>5.2}): {}",
            gene.priority(),
            t.tag,
            gene.threshold,
            t.note
        );
    }
    if g.genes.is_empty() {
        s.push_str("  (empty)\n");
    }
    s
}

/// A single diff line between two champions.
#[derive(Debug, Clone)]
pub enum DiffLine {
    Added(String),
    Removed(String),
    Retuned(String),
}

impl DiffLine {
    /// The rendered patch-note line (`+`/`-`/`~`).
    pub fn render(&self) -> String {
        match self {
            DiffLine::Added(s) => format!("  + {s}"),
            DiffLine::Removed(s) => format!("  - {s}"),
            DiffLine::Retuned(s) => format!("  ~ {s}"),
        }
    }

    /// Just the sign char, for compact summaries.
    pub fn sign(&self) -> char {
        match self {
            DiffLine::Added(_) => '+',
            DiffLine::Removed(_) => '-',
            DiffLine::Retuned(_) => '~',
        }
    }
}

/// Compute the legible diff turning champion `prev` into champion `next`.
///
/// A rule (by catalog id) present only in `next` is an **add**, present only in `prev` is
/// a **remove**, present in both with a changed threshold/priority is a **retune**. The
/// thresholds that matter are reported numerically so the player sees "strike threshold
/// 1.30 → 1.12" (the AI growing more aggressive), which is the readable signal.
pub fn diff_genomes(prev: &Genome, next: &Genome) -> Vec<DiffLine> {
    let mut lines = Vec::new();
    let cat = crate::genome::catalog_len();
    for id in 0..cat {
        let p = prev.genes.iter().find(|g| g.template_id == id);
        let n = next.genes.iter().find(|g| g.template_id == id);
        match (p, n) {
            (None, Some(g)) => {
                let t = g.template();
                lines.push(DiffLine::Added(format!(
                    "{:<10} (thr={:.2}): {}",
                    t.tag, g.threshold, t.note
                )));
            }
            (Some(g), None) => {
                let t = g.template();
                lines.push(DiffLine::Removed(format!("{:<10}: {}", t.tag, t.note)));
            }
            (Some(a), Some(b)) => {
                let t = a.template();
                let dthr = (a.threshold - b.threshold).abs();
                let dprio = a.priority() != b.priority();
                // Only report a retune if something legibly changed (avoid float noise).
                if dthr > 1e-3 || dprio {
                    let mut what = String::new();
                    if dthr > 1e-3 {
                        let _ = write!(what, "threshold {:.2}->{:.2}", a.threshold, b.threshold);
                    }
                    if dprio {
                        if !what.is_empty() {
                            what.push_str(", ");
                        }
                        let _ = write!(what, "priority {}->{}", a.priority(), b.priority());
                    }
                    lines.push(DiffLine::Retuned(format!("{:<10} ({})", t.tag, what)));
                }
            }
            (None, None) => {}
        }
    }
    lines
}

/// Render a diff as a block of patch-note lines (or "(no change)" when identical).
pub fn render_diff(prev: &Genome, next: &Genome) -> String {
    let lines = diff_genomes(prev, next);
    if lines.is_empty() {
        return "  (no change to the rule-set)\n".to_string();
    }
    let mut s = String::new();
    for l in lines {
        s.push_str(&l.render());
        s.push('\n');
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::genome::{Gene, Genome, Operators};

    fn g(genes: Vec<Gene>) -> Genome {
        let mut x = Genome { genes, operators: Operators::default_seed() };
        x.normalize();
        x
    }

    #[test]
    fn diff_detects_add_remove_retune() {
        // prev: {expand+(3), flow(6)}; next: {expand+(3) retuned, strike(0) added}.
        let prev = g(vec![Gene::fresh(3), Gene::fresh(6)]);
        let mut expand_retuned = Gene::fresh(3);
        expand_retuned.threshold += 1.0;
        let next = g(vec![expand_retuned, Gene::fresh(0)]);

        let lines = diff_genomes(&prev, &next);
        let signs: Vec<char> = lines.iter().map(|l| l.sign()).collect();
        // strike added (+), expand+ retuned (~), flow removed (-) — order is by catalog id.
        assert!(signs.contains(&'+'), "should record an add (strike)");
        assert!(signs.contains(&'-'), "should record a remove (flow)");
        assert!(signs.contains(&'~'), "should record a retune (expand+)");
    }

    #[test]
    fn identical_genomes_have_empty_diff() {
        let a = g(vec![Gene::fresh(3), Gene::fresh(6)]);
        let b = g(vec![Gene::fresh(3), Gene::fresh(6)]);
        assert!(diff_genomes(&a, &b).is_empty());
        assert!(render_diff(&a, &b).contains("no change"));
    }

    #[test]
    fn render_is_priority_ordered_and_legible() {
        let champ = g(vec![Gene::fresh(0), Gene::fresh(4), Gene::fresh(5)]); // strike/expand-/mass
        let txt = render_genome(&champ);
        // Strike (Engage, prio ~300) must appear before expand- (Expand, ~100).
        let strike_pos = txt.find("strike").unwrap();
        let expand_pos = txt.find("expand-").unwrap();
        assert!(strike_pos < expand_pos, "higher-priority rule must render first");
        // Notes (human phrases) are present — the legibility property.
        assert!(txt.contains("commit all at the weakest adjacent enemy"));
    }
}
