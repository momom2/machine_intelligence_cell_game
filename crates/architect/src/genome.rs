//! The **genome**: a legible DSL rule-set plus the organism's *own* evolvable
//! reproduction operators.
//!
//! ## Two design commitments from `02-ai-opponents.md`, made concrete
//!
//! **1. The glass genome must stay legible.** The single most important "feel" mechanic
//! is that the player *reads* the Architect's policy and *watches it mutate as diffs*. A
//! freely-mutating rule with arbitrary feature/threshold/selector cross-products would
//! be a wall of noise. So a [`Gene`] is drawn from a **fixed catalog of building-block
//! rules** ([`gene_catalog`]) phrased in the exact vocabulary of `DSL_SPEC.md` — each
//! carries a stable, human `note` ("strike: commit all at the weakest adjacent enemy
//! when we have the majority"). Evolution chooses *which* genes are present, their
//! *priority order*, and tunes a single legible *threshold* per gene. A diff is then
//! always a readable line: `+ strike …`, `- man-the-wall …`, `~ reorder reinforce`.
//! This is also the parsimony lever (`03`/`04` R3): fewer genes ⇒ a shorter, readable
//! policy, which the fitness function rewards.
//!
//! **2. Autoconstruction: each organism carries its own operators (Spector's Pushpop).**
//! `02`: "a population of rule-set organisms, where each organism carries **its own**
//! mutation/recombination operators — the reproduction machinery itself evolves, not just
//! the strategy." So a [`Genome`] bundles the rule-set with an [`Operators`] struct (its
//! mutation rates and crossover bias). When two parents recombine, the **child's operators
//! are themselves inherited and mutated** ([`Operators::reproduce`]), so selection acts on
//! *how to evolve* as well as *what to play*. This is what makes the loop
//! *autoconstructive* rather than a plain GA with fixed hyperparameters.
//!
//! ## Why a catalog rather than free-form rule synthesis
//!
//! The catalog is the union of the legible postures the whole game is built on: the
//! shared **expand** baseline plus the **attack** (strike / mass), **defend** (reinforce /
//! man-the-wall) and **colonize** (forward-flow) signature rules from
//! `cell_core::dsl::strategies` and `automaton::compile`, at a few threshold settings.
//! Restricting the search to recombinations of these is exactly the "stay inside the
//! vocabulary the player can read *and write*" constraint of `DSL_SPEC.md §4`, and it
//! guarantees every evolved genome compiles to a policy a human could have authored — the
//! point of the shared substrate.

use cell_core::dsl::{Cmp, OwnerFilter, Rank, Scope};
use cell_core::{Action, Condition, Feature, FractionBucket, NodeSelector, Rule, RulePolicy};

use crate::rng::Rng;

// ===========================================================================
// The legible gene catalog
// ===========================================================================

/// The legible role a gene plays — used for diffs and to keep priorities sane. These are
/// the same role tiers the hand-written `automaton::compile` skeleton uses (ENGAGE > WALL
/// > EXPAND > ROUTE), so an evolved policy reads top-to-bottom like a player's sentence.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    /// Emergency commitment: strike (attack) or reinforce (defend).
    Engage,
    /// Proactive consolidation: man the wall (defend).
    Wall,
    /// The shared economy: claim neutrals.
    Expand,
    /// Spend leftover ships: mass toward enemy / flow toward neutral.
    Route,
}

/// A single building-block rule the Architect may include in a policy. It is **legible
/// by construction**: a fixed condition/action shape from the DSL vocabulary, a stable
/// human `note`, and a single evolvable numeric `threshold` (the comparison constant in
/// its condition — e.g. the strike majority, the reinforce safety margin, the expand
/// keep-floor). Everything else about the rule is fixed so a diff is a one-line change.
#[derive(Debug, Clone, Copy)]
pub struct GeneTemplate {
    /// Stable identity (its index in the catalog). Two genes with the same id are "the
    /// same rule" for diffing/dedup, even at different thresholds.
    pub id: usize,
    /// The legible role tier (sets the base priority band).
    pub role: Role,
    /// Human-readable note surfaced in the glass-genome diff (`&'static`, from the
    /// catalog — no per-genome leaking).
    pub note: &'static str,
    /// A short tag used in compact diffs/labels (e.g. `strike`, `reinforce`, `expand`).
    pub tag: &'static str,
    /// Default threshold (the value the seed genomes use).
    pub default_threshold: f64,
    /// Legal threshold range `[lo, hi]` mutation stays within (keeps the rule sensible
    /// and the search bounded).
    pub threshold_lo: f64,
    pub threshold_hi: f64,
    /// Builds the concrete DSL [`Rule`] for this gene at the given `threshold` and
    /// `priority`. The closure captures only the fixed shape; the two varying parts
    /// (priority, threshold) are passed in.
    build: fn(priority: i32, threshold: f64) -> Rule,
}

impl GeneTemplate {
    /// The base priority band for this gene's role (higher = considered earlier). Genes
    /// within a band are ordered by their evolved `priority` offset.
    pub fn base_priority(&self) -> i32 {
        match self.role {
            Role::Engage => 300,
            Role::Wall => 200,
            Role::Expand => 100,
            Role::Route => 50,
        }
    }
}

// --- The catalog. Each entry's `build` closure is a fixed DSL shape lifted verbatim
// --- from `cell_core::dsl::strategies` / `automaton::compile` so evolved policies are
// --- exactly the legible vocabulary the player reads. -----------------------------

/// Convenience: an "every owned node with surplus" source selector (the common source).
fn mine_src() -> NodeSelector {
    NodeSelector::new(Scope::Anywhere, OwnerFilter::Mine, Rank::WeakestGarrison)
}

/// The catalog of legible genes. The Architect's genomes are subsets of these.
///
/// Coverage rationale: the set must be rich enough that the strategic space is
/// **non-degenerate** (R1's whole question), so it includes the signature rule of each
/// triad archetype *and* enough threshold variety that postures blend continuously:
/// * **strike** (attack engage), **mass** (attack route),
/// * **reinforce** (defend engage), **man-the-wall** (defend wall),
/// * **expand** (the shared economy, at two commitment levels),
/// * **flow-to-neutral** (colonize route),
/// * plus a couple of conditioned variants (expand-when-ahead, strike-only-when-massed)
///   that give evolution legible *compositional* moves.
pub fn gene_catalog() -> &'static [GeneTemplate] {
    // Built once; returned as a static slice. `OnceLock` keeps it dependency-free.
    use std::sync::OnceLock;
    static CATALOG: OnceLock<Vec<GeneTemplate>> = OnceLock::new();
    CATALOG
        .get_or_init(|| {
            vec![
                // 0) STRIKE — attack's signature engage rule. Threshold = the attack
                //    force ratio majority needed to commit (lower = more trigger-happy).
                GeneTemplate {
                    id: 0,
                    role: Role::Engage,
                    note: "strike: commit all at the weakest adjacent enemy when we have the majority",
                    tag: "strike",
                    default_threshold: 1.10,
                    threshold_lo: 1.00,
                    threshold_hi: 2.00,
                    build: |p, t| {
                        Rule::new(
                            p,
                            Condition::cmp(Feature::AttackForceRatio, Cmp::Ge, t),
                            Action {
                                source: mine_src(),
                                fraction: FractionBucket::All,
                                target: NodeSelector::new(Scope::Adjacent, OwnerFilter::Enemy, Rank::WeakestGarrison),
                            },
                        )
                        .noted("strike: commit all at the weakest adjacent enemy when we have the majority")
                    },
                },
                // 1) REINFORCE — defend's signature engage rule. Threshold = the
                //    weakest-neighbor force-ratio safety margin (higher = reinforce earlier).
                GeneTemplate {
                    id: 1,
                    role: Role::Engage,
                    note: "reinforce: rush the most threatened friendly neighbor",
                    tag: "reinforce",
                    default_threshold: 1.50,
                    threshold_lo: 1.05,
                    threshold_hi: 2.00,
                    build: |p, t| {
                        Rule::new(
                            p,
                            Condition::and(
                                Condition::cmp(Feature::WeakestNeighborForceRatio, Cmp::Lt, t),
                                Condition::cmp(Feature::Garrison, Cmp::Ge, 1.0),
                            ),
                            Action {
                                source: mine_src(),
                                fraction: FractionBucket::All,
                                target: NodeSelector::new(Scope::Adjacent, OwnerFilter::Mine, Rank::LowestForceRatio),
                            },
                        )
                        .noted("reinforce: rush the most threatened friendly neighbor")
                    },
                },
                // 2) MAN THE WALL — defend's proactive consolidation. Threshold = the
                //    interior garrison floor above which a calm node feeds the frontier.
                GeneTemplate {
                    id: 2,
                    role: Role::Wall,
                    note: "man the wall: feed surplus to the weakest adjacent friendly node",
                    tag: "wall",
                    default_threshold: 4.0,
                    threshold_lo: 3.0,
                    threshold_hi: 14.0,
                    build: |p, t| {
                        Rule::new(
                            p,
                            Condition::and(
                                Condition::cmp(Feature::IsFrontier, Cmp::Lt, 0.5),
                                Condition::and(
                                    Condition::cmp(Feature::Garrison, Cmp::Ge, t),
                                    Condition::cmp(Feature::IncomingHostile, Cmp::Le, 0.0),
                                ),
                            ),
                            Action {
                                source: mine_src(),
                                fraction: FractionBucket::Half,
                                target: NodeSelector::new(Scope::Adjacent, OwnerFilter::Mine, Rank::WeakestGarrison),
                            },
                        )
                        .noted("man the wall: feed surplus to the weakest adjacent friendly node")
                    },
                },
                // 3) EXPAND (aggressive) — the shared economy, colonial setting:
                //    three-quarters of the garrison to the best empty, low keep-floor.
                //    Threshold = the keep-floor (surplus a node must have to expand).
                GeneTemplate {
                    id: 3,
                    role: Role::Expand,
                    note: "expand hard: flood three-quarters of surplus into the best empty node",
                    tag: "expand+",
                    default_threshold: 2.0,
                    threshold_lo: 1.0,
                    threshold_hi: 6.0,
                    build: |p, t| {
                        Rule::new(
                            p,
                            Condition::cmp(Feature::Garrison, Cmp::Ge, t),
                            Action {
                                source: mine_src(),
                                fraction: FractionBucket::ThreeQuarter,
                                target: NodeSelector::new(Scope::Adjacent, OwnerFilter::Empty, Rank::BestColonizeValue),
                            },
                        )
                        .noted("expand hard: flood three-quarters of surplus into the best empty node")
                    },
                },
                // 4) EXPAND (medium) — the shared economy at a moderate commitment: half the
                //    garrison to the best empty node, keep-floor a few ships. This is the
                //    general-purpose expander (the funding economy an attacker runs, and the
                //    middle setting between flood and turtle). Threshold = the keep-floor.
                GeneTemplate {
                    id: 4,
                    role: Role::Expand,
                    note: "expand: send half of surplus to the best adjacent empty node",
                    tag: "expand",
                    default_threshold: 4.0,
                    threshold_lo: 2.0,
                    threshold_hi: 10.0,
                    build: |p, t| {
                        Rule::new(
                            p,
                            Condition::cmp(Feature::Garrison, Cmp::Ge, t),
                            Action {
                                source: mine_src(),
                                fraction: FractionBucket::Half,
                                target: NodeSelector::new(Scope::Adjacent, OwnerFilter::Empty, Rank::BestColonizeValue),
                            },
                        )
                        .noted("expand: send half of surplus to the best adjacent empty node")
                    },
                },
                // 5) MASS — attack's routing rule: push the army one hop toward the enemy
                //    front. Threshold = the garrison floor that participates in massing.
                GeneTemplate {
                    id: 5,
                    role: Role::Route,
                    note: "mass: push the army one hop toward the enemy front",
                    tag: "mass",
                    default_threshold: 2.0,
                    threshold_lo: 1.0,
                    threshold_hi: 8.0,
                    build: |p, t| {
                        Rule::new(
                            p,
                            Condition::cmp(Feature::Garrison, Cmp::Ge, t),
                            Action {
                                source: mine_src(),
                                fraction: FractionBucket::All,
                                target: NodeSelector::new(Scope::Adjacent, OwnerFilter::Mine, Rank::ClosestToEnemy),
                            },
                        )
                        .noted("mass: push the army one hop toward the enemy front")
                    },
                },
                // 6) FLOW (eager) — colonize's signature routing: a node with no adjacent
                //    empty trickles half its surplus one hop toward unclaimed space, *even
                //    under threat*. Keeping it ungated is exactly what keeps a colonizer thin
                //    and reaching (so a timed strike still cracks it — the `attack ≻ colonize`
                //    edge). Threshold = the garrison floor.
                GeneTemplate {
                    id: 6,
                    role: Role::Route,
                    note: "flow forward: keep pushing surplus toward unclaimed space",
                    tag: "flow",
                    default_threshold: 4.0,
                    threshold_lo: 2.0,
                    threshold_hi: 10.0,
                    build: |p, t| {
                        Rule::new(
                            p,
                            Condition::cmp(Feature::Garrison, Cmp::Ge, t),
                            Action {
                                source: mine_src(),
                                fraction: FractionBucket::Half,
                                target: NodeSelector::new(Scope::Adjacent, OwnerFilter::Mine, Rank::ClosestToNeutral),
                            },
                        )
                        .noted("flow forward: keep pushing surplus toward unclaimed space")
                    },
                },
                // 7) HOLD-WHEN-AHEAD — a legible *compositional* gene: once we lead on
                //    territory, stop expanding and consolidate (man-the-wall-like but
                //    gated on the macro feature). Threshold = the territory-share lead
                //    above which we switch to holding. Gives evolution a "close out a won
                //    game" move that reads clearly in the genome.
                GeneTemplate {
                    id: 7,
                    role: Role::Wall,
                    note: "consolidate when ahead: once leading on territory, stack the frontier",
                    tag: "hold-ahead",
                    default_threshold: 0.20,
                    threshold_lo: 0.05,
                    threshold_hi: 0.60,
                    build: |p, t| {
                        Rule::new(
                            p,
                            Condition::and(
                                Condition::cmp(Feature::TerritoryShare, Cmp::Ge, t),
                                Condition::cmp(Feature::Garrison, Cmp::Ge, 4.0),
                            ),
                            Action {
                                source: mine_src(),
                                fraction: FractionBucket::Half,
                                target: NodeSelector::new(Scope::Adjacent, OwnerFilter::Mine, Rank::ClosestToEnemy),
                            },
                        )
                        .noted("consolidate when ahead: once leading on territory, stack the frontier")
                    },
                },
                // 8) EXPAND (cautious) — the turtle's economy: a quarter of the garrison to
                //    the best empty node, only when not under threat, keeping a heavy home
                //    reserve. This is the defender's "claim a fair share but never strip the
                //    walls" expander (the `colonize ≻ defend` opportunity cost lives here).
                //    Threshold = the keep-floor.
                GeneTemplate {
                    id: 8,
                    role: Role::Expand,
                    note: "expand safely: send a quarter to the best empty node when calm",
                    tag: "expand-",
                    default_threshold: 6.0,
                    threshold_lo: 3.0,
                    threshold_hi: 12.0,
                    build: |p, t| {
                        Rule::new(
                            p,
                            Condition::and(
                                Condition::cmp(Feature::Garrison, Cmp::Ge, t),
                                Condition::cmp(Feature::IncomingHostile, Cmp::Le, 0.0),
                            ),
                            Action {
                                source: mine_src(),
                                fraction: FractionBucket::Quarter,
                                target: NodeSelector::new(Scope::Adjacent, OwnerFilter::Empty, Rank::BestColonizeValue),
                            },
                        )
                        .noted("expand safely: send a quarter to the best empty node when calm")
                    },
                },
                // 9) FLOW (cautious) — defend's routing: trickle surplus toward the contested
                //    middle **only when calm** (`IncomingHostile <= 0`). The instant a node is
                //    threatened it holds instead of shoving ships forward — the discipline that
                //    keeps a defender's wall manned (a threatened turtle that kept flowing
                //    would thin out and be overrun). The calm-gated twin of gene 6; the two
                //    routing disciplines are the legible colonize↔defend distinction.
                //    Threshold = the garrison floor.
                GeneTemplate {
                    id: 9,
                    role: Role::Route,
                    note: "flow when calm: trickle spare ships toward the contested middle, holding under threat",
                    tag: "flow-safe",
                    default_threshold: 8.0,
                    threshold_lo: 4.0,
                    threshold_hi: 12.0,
                    build: |p, t| {
                        Rule::new(
                            p,
                            Condition::and(
                                Condition::cmp(Feature::Garrison, Cmp::Ge, t),
                                Condition::cmp(Feature::IncomingHostile, Cmp::Le, 0.0),
                            ),
                            Action {
                                source: mine_src(),
                                fraction: FractionBucket::Half,
                                target: NodeSelector::new(Scope::Adjacent, OwnerFilter::Mine, Rank::ClosestToNeutral),
                            },
                        )
                        .noted("flow when calm: trickle spare ships toward the contested middle, holding under threat")
                    },
                },
            ]
        })
        .as_slice()
}

/// Number of distinct gene ids in the catalog.
pub fn catalog_len() -> usize {
    gene_catalog().len()
}

// ===========================================================================
// A gene instance (a catalog entry with an evolved threshold + priority offset)
// ===========================================================================

/// An instantiated gene in an organism's policy: which catalog template, plus the two
/// evolvable knobs — its `threshold` (the condition constant) and a `priority_offset`
/// (a small ± shift within its role band, so evolution can reorder same-role rules).
#[derive(Debug, Clone, Copy)]
pub struct Gene {
    pub template_id: usize,
    pub threshold: f64,
    /// Offset added to the template's base priority. Bounded to keep role bands from
    /// crossing (see [`Gene::priority`]).
    pub priority_offset: i32,
}

impl Gene {
    /// The catalog template backing this gene.
    pub fn template(&self) -> &'static GeneTemplate {
        &gene_catalog()[self.template_id]
    }

    /// The effective priority: role base + clamped offset. Offsets are kept within
    /// ±20 so a Route gene can never out-prioritize an Expand gene, etc. (bands are 50
    /// apart) — preserving the legible high→low role skeleton even as priorities evolve.
    pub fn priority(&self) -> i32 {
        let off = self.priority_offset.clamp(-20, 20);
        self.template().base_priority() + off
    }

    /// Compile this gene to its concrete DSL [`Rule`].
    pub fn to_rule(&self) -> Rule {
        let t = self.template();
        let thr = self.threshold.clamp(t.threshold_lo, t.threshold_hi);
        (t.build)(self.priority(), thr)
    }

    /// A fresh gene for catalog template `id` at its default threshold and zero offset.
    pub fn fresh(id: usize) -> Gene {
        let t = &gene_catalog()[id];
        Gene { template_id: id, threshold: t.default_threshold, priority_offset: 0 }
    }
}

// ===========================================================================
// The self-carried reproduction operators (Spector's Pushpop twist)
// ===========================================================================

/// An organism's **own** mutation/recombination parameters. These are part of the genome
/// and are *themselves inherited and mutated* on reproduction, so the population evolves
/// *how it evolves*, not just what it plays — the defining property of autoconstructive
/// evolution (`02`, Lee Spector's Pushpop).
///
/// Each field is a probability/rate in a bounded range; [`Operators::reproduce`] perturbs
/// them with a small meta-mutation so the operator distribution itself drifts under
/// selection.
#[derive(Debug, Clone, Copy)]
pub struct Operators {
    /// Probability of toggling a gene's presence (add a missing gene / drop a present
    /// one) during mutation. The main structural-change rate.
    pub p_toggle_gene: f64,
    /// Probability of nudging a present gene's threshold.
    pub p_tweak_threshold: f64,
    /// Probability of nudging a gene's priority offset (reordering within a role band).
    pub p_tweak_priority: f64,
    /// Std-dev (as a fraction of the gene's legal threshold range) of a threshold nudge.
    pub threshold_step: f64,
    /// Crossover bias in `[0,1]`: probability of taking each gene-slot from parent A
    /// rather than parent B during recombination. Drifts so lineages can favor one
    /// parent's structure.
    pub crossover_bias: f64,
}

impl Operators {
    /// A reasonable default operator set for the initial population. Deliberately
    /// moderate rates so early evolution explores without thrashing; selection then
    /// tunes them per-lineage.
    pub fn default_seed() -> Operators {
        Operators {
            p_toggle_gene: 0.20,
            p_tweak_threshold: 0.50,
            p_tweak_priority: 0.20,
            threshold_step: 0.15,
            crossover_bias: 0.50,
        }
    }

    /// A randomized operator set (for diversity in the initial population), each field
    /// drawn uniformly from a sensible band.
    pub fn random(rng: &mut Rng) -> Operators {
        Operators {
            p_toggle_gene: rng.range(0.05, 0.40),
            p_tweak_threshold: rng.range(0.20, 0.80),
            p_tweak_priority: rng.range(0.05, 0.40),
            threshold_step: rng.range(0.05, 0.30),
            crossover_bias: rng.range(0.30, 0.70),
        }
    }

    /// Produce the child's operators from a parent's, applying a small **meta-mutation**
    /// (the operators evolve too). Each rate takes a multiplicative jitter and is clamped
    /// back into its legal band. `meta` scales the jitter (the population's fixed
    /// meta-learning-rate; the *operators* are what evolve, `meta` just bounds drift).
    pub fn reproduce(&self, rng: &mut Rng, meta: f64) -> Operators {
        let jitter = |r: &mut Rng, v: f64, lo: f64, hi: f64| {
            // Multiplicative jitter in [1-meta, 1+meta], then clamp.
            let f = 1.0 + r.range(-meta, meta);
            (v * f).clamp(lo, hi)
        };
        Operators {
            p_toggle_gene: jitter(rng, self.p_toggle_gene, 0.02, 0.60),
            p_tweak_threshold: jitter(rng, self.p_tweak_threshold, 0.05, 0.95),
            p_tweak_priority: jitter(rng, self.p_tweak_priority, 0.02, 0.60),
            threshold_step: jitter(rng, self.threshold_step, 0.02, 0.50),
            crossover_bias: jitter(rng, self.crossover_bias, 0.15, 0.85),
        }
    }
}

// ===========================================================================
// The genome
// ===========================================================================

/// A complete organism genome: a set of legible [`Gene`]s (its rule-set) plus its own
/// [`Operators`]. Compiles to a `cell_core::RulePolicy` for evaluation.
///
/// Invariant: **at most one gene per catalog id** (a policy never lists the same building
/// block twice — that would be illegible and redundant). [`Genome::normalize`] enforces
/// it, keeping the gene with the most "central" threshold if duplicates ever arise from
/// crossover.
#[derive(Debug, Clone)]
pub struct Genome {
    pub genes: Vec<Gene>,
    pub operators: Operators,
}

impl Genome {
    /// Number of rules in the compiled policy = the parsimony measure the fitness penalizes.
    pub fn rule_count(&self) -> usize {
        self.genes.len()
    }

    /// True if catalog id `id` is present.
    pub fn has(&self, id: usize) -> bool {
        self.genes.iter().any(|g| g.template_id == id)
    }

    /// Enforce the "≤ one gene per id" invariant (dedup, keeping the first occurrence)
    /// and guarantee the genome is non-empty (a policy with no rules does nothing and is
    /// never useful) by seeding a cautious expand gene if it somehow became empty.
    pub fn normalize(&mut self) {
        let mut seen = [false; 64];
        self.genes.retain(|g| {
            let id = g.template_id;
            if id >= 64 || seen[id] {
                return false;
            }
            seen[id] = true;
            true
        });
        if self.genes.is_empty() {
            // Never let a genome be a no-op; an organism must at least try to play.
            self.genes.push(Gene::fresh(4)); // cautious expand
        }
    }

    /// Compile to a runnable DSL policy. The `name` is a stable label (e.g. "champion");
    /// it is leaked once (bounded — champions/log labels are few and live for the run),
    /// matching the pattern `automaton::compile` uses for the engine's `&'static` name.
    pub fn to_policy(&self, name: &str) -> RulePolicy {
        let rules: Vec<Rule> = self.genes.iter().map(|g| g.to_rule()).collect();
        let static_name: &'static str = Box::leak(name.to_string().into_boxed_str());
        RulePolicy::new(static_name, rules)
    }

    /// A fresh boxed engine policy for this genome (the match runner consumes a
    /// `&mut dyn Policy`). The compiled rule-set is independent per call.
    pub fn make_player(&self) -> Box<dyn cell_core::Policy> {
        // The compiled policy carries no per-match mutable state, so a fixed throwaway
        // name is fine here (only the *logged champion* needs a meaningful name).
        Box::new(cell_core::DslPolicy::new(self.to_policy("organism")))
    }

    /// Build a random genome: each catalog gene is included with probability `density`,
    /// thresholds jittered around their defaults, with randomized operators. Always
    /// normalized + non-empty. Used to seed the initial population with diverse policies
    /// so the coevolutionary arms race has somewhere to start.
    pub fn random(rng: &mut Rng, density: f64) -> Genome {
        let mut genes = Vec::new();
        for id in 0..catalog_len() {
            if rng.chance(density) {
                let t = &gene_catalog()[id];
                let span = t.threshold_hi - t.threshold_lo;
                let thr = (t.default_threshold + rng.range(-0.25, 0.25) * span)
                    .clamp(t.threshold_lo, t.threshold_hi);
                genes.push(Gene {
                    template_id: id,
                    threshold: thr,
                    priority_offset: (rng.below(9) as i32) - 4, // -4..=4
                });
            }
        }
        let mut g = Genome { genes, operators: Operators::random(rng) };
        g.normalize();
        g
    }
}

// ===========================================================================
// Seed archetypes (so the population starts from legible, competent policies)
// ===========================================================================

/// The three pure-strategy genomes, expressed in catalog genes. Seeding a few of these
/// into the initial population guarantees the coevolution starts from *competent* play
/// (not random flailing), so the very first generations already exhibit the triad's
/// counter-dynamics — the substrate R1 is tested on. They are the genome equivalents of
/// `cell_core::dsl::strategies`.
pub fn seed_archetypes() -> Vec<Genome> {
    let ops = Operators::default_seed();
    // Colonize: expand hard + flow.
    let colonize = Genome {
        genes: vec![
            Gene::fresh(3), // expand+
            Gene::fresh(6), // flow
        ],
        operators: ops,
    };
    // Defend: reinforce + man-the-wall + cautious (quarter) expand + flow.
    let defend = Genome {
        genes: vec![
            Gene::fresh(1), // reinforce
            Gene::fresh(2), // man the wall
            Gene::fresh(8), // expand- (cautious quarter, calm-gated)
            Gene::fresh(9), // flow-safe (calm-gated; holds under threat)
        ],
        operators: ops,
    };
    // Attack: strike + medium (half) funding expand + mass.
    let attack = Genome {
        genes: vec![
            Gene::fresh(0), // strike
            Gene::fresh(4), // expand (medium half — the funding economy)
            Gene::fresh(5), // mass
        ],
        operators: ops,
    };
    let mut out = vec![colonize, defend, attack];
    for g in &mut out {
        g.normalize();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use cell_core::maps::corridor7;
    use cell_core::{Owner, Params};

    fn params() -> Params {
        Params { r: 0.6, k: 2.25, l: 0.15, ..Params::default() }
    }

    #[test]
    fn catalog_ids_are_contiguous_and_self_consistent() {
        for (i, g) in gene_catalog().iter().enumerate() {
            assert_eq!(g.id, i, "catalog id must equal its index");
            assert!(g.threshold_lo <= g.default_threshold && g.default_threshold <= g.threshold_hi);
        }
        assert!(catalog_len() >= 7, "catalog must cover the triad signatures");
    }

    #[test]
    fn genome_compiles_and_plays() {
        // A seed archetype compiles to a legal policy that beats idle (it actually plays).
        let g = &seed_archetypes()[0]; // colonize
        let base = corridor7().state;
        let mut p = g.make_player();
        let mut idle = idle();
        let out = base.run_match(p.as_mut(), idle.as_mut(), &params(), 200);
        assert!(out.score_a > 0.0, "a colonize genome should beat idle, got {}", out.score_a);
    }

    #[test]
    fn normalize_dedups_and_never_empty() {
        let mut g = Genome {
            genes: vec![Gene::fresh(3), Gene::fresh(3), Gene::fresh(6)],
            operators: Operators::default_seed(),
        };
        g.normalize();
        assert_eq!(g.rule_count(), 2, "duplicate id 3 should be removed");
        let mut empty = Genome { genes: vec![], operators: Operators::default_seed() };
        empty.normalize();
        assert!(empty.rule_count() >= 1, "an empty genome must be seeded with a rule");
    }

    #[test]
    fn priority_offset_cannot_cross_role_bands() {
        // Even a maxed-out Route offset stays below an Expand gene's base priority.
        let route = Gene { template_id: 5, threshold: 2.0, priority_offset: 1000 };
        let expand = Gene { template_id: 4, threshold: 6.0, priority_offset: -1000 };
        assert!(route.priority() < expand.priority(), "role skeleton must be preserved");
    }

    #[test]
    fn operators_reproduce_in_bounds() {
        let mut rng = Rng::new(1);
        let parent = Operators::random(&mut rng);
        for _ in 0..1000 {
            let child = parent.reproduce(&mut rng, 0.3);
            assert!((0.02..=0.60).contains(&child.p_toggle_gene));
            assert!((0.05..=0.95).contains(&child.p_tweak_threshold));
            assert!((0.15..=0.85).contains(&child.crossover_bias));
        }
    }

    fn idle() -> Box<dyn cell_core::Policy> {
        struct Idle;
        impl cell_core::Policy for Idle {
            fn name(&self) -> &'static str {
                "Idle"
            }
            fn decide(&mut self, _s: &cell_core::GameState, _me: Owner, _p: &Params) -> Vec<cell_core::Command> {
                Vec::new()
            }
        }
        Box::new(Idle)
    }
}
