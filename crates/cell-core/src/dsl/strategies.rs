//! The three pure strategies, re-expressed as DSL rule-sets.
//!
//! This module is the **expressiveness proof** for D4: the colonize/defend/attack
//! triad — hand-written as bespoke [`crate::policy`] code for the R2 sweep — is
//! re-derived here using nothing but [`Rule`]s over the legible features. The
//! `dsl_reproduces_pure_strategies` integration test then shows these rule-sets
//! reproduce the **rock-paper-scissors cycle** at R2's recommended operating point
//! (`r=0.6, k=2.25, l=0.15`), i.e. the DSL is rich enough to carry the strategic
//! core. That this is possible is the whole bet of `02`'s "one substrate, four
//! minds": if three competent strategies fit in the language, so can the policies the
//! Architect evolves and the player writes.
//!
//! ## How each archetype maps to rules (the WHY)
//!
//! The cycle lives in **combat posture**, not in expansion (every archetype expands;
//! see `01`/`policy.rs`). So all three rule-sets share an expansion rule and differ
//! in the *priority and presence* of defend/attack rules:
//!
//! * **Colonize** — expansion only, with forward-flow toward unclaimed space. No
//!   defend, no attack: thin everywhere, out-produces a turtle, falls to a timed
//!   strike.
//! * **Attack** — a high-priority **strike** rule (commit when the attack force
//!   ratio clears the majority a timing push needs) sitting above a modest expansion
//!   and a *mass-toward-the-enemy* rule. Pre-empts expansion exactly when a target is
//!   weak.
//! * **Defend** — high-priority **reinforce-the-threatened** and **man-the-wall**
//!   rules above a cautious expansion. Holds a heavy, reinforced frontier.
//!
//! The priorities are the legible part: read top-to-bottom, each policy is a sentence
//! a player would say ("if I can take the planet next door, do it; otherwise mass
//! toward them; otherwise grab land").

use super::*;

// ---------------------------------------------------------------------------
// Shared building blocks
// ---------------------------------------------------------------------------

/// Send `frac` of any owned node's garrison to its best adjacent **empty** node
/// (rate-aware [`Rank::BestColonizeValue`]), gated so only nodes with real surplus
/// (garrison `>= keep`) spend. This is the shared economic baseline every archetype
/// uses — the DSL form of `policy::expand_step`.
fn expand_rule(priority: i32, keep: f64, frac: FractionBucket) -> Rule {
    Rule::new(
        priority,
        Condition::cmp(Feature::Garrison, Cmp::Ge, keep),
        Action {
            source: NodeSelector::new(Scope::Anywhere, OwnerFilter::Mine, Rank::WeakestGarrison),
            fraction: frac,
            target: NodeSelector::new(Scope::Adjacent, OwnerFilter::Empty, Rank::BestColonizeValue),
        },
    )
    .noted("expand: send surplus to the best adjacent empty node")
}

// ---------------------------------------------------------------------------
// Colonize
// ---------------------------------------------------------------------------

/// **Colonize** as rules: grab land fast, garrison nothing, flow ships forward toward
/// unclaimed space so captured nodes keep funding expansion. Pure economist — never
/// masses on the enemy (the forward-flow routes only toward neutral space).
pub fn colonize() -> Policy {
    Policy::new(
        "Colonize(dsl)",
        vec![
            // 1) Claim adjacent neutrals aggressively (keep almost nothing).
            expand_rule(100, 2.0, FractionBucket::ThreeQuarter),
            // 2) Forward-flow: a node with surplus but no adjacent empty pushes ships
            //    one hop toward the friendly neighbor closest to unclaimed space, so
            //    growth keeps reaching. Routes toward NEUTRAL only (Colonize must not
            //    accidentally mass against the enemy).
            Rule::new(
                50,
                Condition::cmp(Feature::Garrison, Cmp::Ge, 4.0),
                Action {
                    source: NodeSelector::new(
                        Scope::Anywhere,
                        OwnerFilter::Mine,
                        Rank::WeakestGarrison,
                    ),
                    fraction: FractionBucket::Half,
                    target: NodeSelector::new(
                        Scope::Adjacent,
                        OwnerFilter::Mine,
                        Rank::ClosestToNeutral,
                    ),
                },
            )
            .noted("flow forward: push surplus toward unclaimed space"),
        ],
    )
}

// ---------------------------------------------------------------------------
// Attack
// ---------------------------------------------------------------------------

/// **Attack** as rules: a timing push. Strike the weakest adjacent enemy the moment
/// the attack force ratio clears the majority the push needs; otherwise expand
/// modestly to fund the army and mass the rest of the army toward the enemy.
///
/// The strike threshold mirrors `policy::Attack::commit_threshold`: commit on a
/// post-loss majority (~1.1x), letting the engine's defender advantage `k` decide the
/// fight. Against a thin colonizer the ratio clears trivially (early conquest →
/// snowball); against a massed defender it rarely clears (so Attack stays back and
/// under-invests — the cost it pays losing to Defend).
pub fn attack() -> Policy {
    Policy::new(
        "Attack(dsl)",
        vec![
            // 1) STRIKE (highest): if this node can take the weakest enemy planet next
            //    door with a majority, send everything at it.
            Rule::new(
                100,
                Condition::cmp(Feature::AttackForceRatio, Cmp::Ge, 1.10),
                Action {
                    source: NodeSelector::new(
                        Scope::Anywhere,
                        OwnerFilter::Mine,
                        Rank::WeakestGarrison,
                    ),
                    fraction: FractionBucket::All,
                    target: NodeSelector::new(
                        Scope::Adjacent,
                        OwnerFilter::Enemy,
                        Rank::WeakestGarrison,
                    ),
                },
            )
            .noted("strike: commit all at the weakest adjacent enemy when we have the majority"),
            // 2) EXPAND modestly to fund the army (Attack under-invests on purpose).
            expand_rule(60, 4.0, FractionBucket::Half),
            // 3) MASS: every other node with ships pushes one hop toward the enemy,
            //    routing through the friendly neighbor nearest the enemy so the army
            //    concentrates at the front (expansion via rule 2 grabs neutrals on the
            //    way; this rule only moves between owned nodes).
            Rule::new(
                40,
                Condition::cmp(Feature::Garrison, Cmp::Ge, 2.0),
                Action {
                    source: NodeSelector::new(
                        Scope::Anywhere,
                        OwnerFilter::Mine,
                        Rank::WeakestGarrison,
                    ),
                    fraction: FractionBucket::All,
                    target: NodeSelector::new(
                        Scope::Adjacent,
                        OwnerFilter::Mine,
                        Rank::ClosestToEnemy,
                    ),
                },
            )
            .noted("mass: push the army one hop toward the enemy front"),
        ],
    )
}

// ---------------------------------------------------------------------------
// Defend
// ---------------------------------------------------------------------------

/// **Defend** as rules: build a modest economy, then hold it. Reinforce any node
/// whose neighbor is losing its fight, keep the frontier manned from the interior,
/// and expand only cautiously from safe nodes.
///
/// Holding ships (rather than expanding maximally) pays an opportunity cost vs a
/// colonizer (losing that edge) but the massed, reinforced, `k`-advantaged frontier
/// repels an aggressor (winning that edge).
pub fn defend() -> Policy {
    Policy::new(
        "Defend(dsl)",
        vec![
            // 1) REINFORCE (highest): if a friendly neighbor is under pressure
            //    (weakest-neighbor force ratio below a safety margin), rush help to the
            //    most threatened adjacent friendly node. The margin > 1 means we
            //    reinforce *before* the fight is a coin flip — predictive defense, fed
            //    by "incoming in N ticks", so it fires during the attacker's undock
            //    window (exactly what the hardcoded Defend's deficit check does).
            Rule::new(
                100,
                Condition::and(
                    Condition::cmp(Feature::WeakestNeighborForceRatio, Cmp::Lt, 1.5),
                    Condition::cmp(Feature::Garrison, Cmp::Ge, 1.0),
                ),
                Action {
                    source: NodeSelector::new(
                        Scope::Anywhere,
                        OwnerFilter::Mine,
                        Rank::WeakestGarrison,
                    ),
                    fraction: FractionBucket::All,
                    target: NodeSelector::new(
                        Scope::Adjacent,
                        OwnerFilter::Mine,
                        Rank::LowestForceRatio,
                    ),
                },
            )
            .noted("reinforce: rush the most threatened friendly neighbor"),
            // 2) MAN THE WALL: an interior (non-frontier) node with surplus pushes
            //    half its ships to its weakest adjacent friendly node, keeping the
            //    frontier heavy *proactively* (so the wall is already massed when the
            //    attacker arrives — this is what turns the defender advantage `k` into
            //    a repelled assault). Gated only on being interior + calm + having a
            //    modest surplus; deliberately low floor so consolidation is constant.
            Rule::new(
                70,
                Condition::and(
                    Condition::cmp(Feature::IsFrontier, Cmp::Lt, 0.5), // interior only
                    Condition::and(
                        Condition::cmp(Feature::Garrison, Cmp::Ge, 4.0),
                        Condition::cmp(Feature::IncomingHostile, Cmp::Le, 0.0),
                    ),
                ),
                Action {
                    source: NodeSelector::new(
                        Scope::Anywhere,
                        OwnerFilter::Mine,
                        Rank::WeakestGarrison,
                    ),
                    fraction: FractionBucket::Half,
                    // Feed the weakest adjacent friendly node. Across maps this keeps
                    // every door manned rather than over-committing to the single lane
                    // nearest the enemy (which leaves a parallel bridge open).
                    target: NodeSelector::new(
                        Scope::Adjacent,
                        OwnerFilter::Mine,
                        Rank::WeakestGarrison,
                    ),
                },
            )
            .noted("man the wall: feed surplus to the weakest adjacent friendly node"),
            // 3) EXPAND to a fair share: from calm nodes with a solid floor, claim the
            //    best adjacent neutral with a quarter of the garrison. A turtle that
            //    *cedes* the land grab loses on territory before any fight (it would
            //    hold a smaller half of the map forever); Defend must contest neutrals
            //    at a competitive pace, then hold — "modest", not "passive". The
            //    defensive rules above out-prioritize this, so a threatened node never
            //    expands.
            Rule::new(
                50,
                Condition::and(
                    Condition::cmp(Feature::Garrison, Cmp::Ge, 6.0),
                    Condition::cmp(Feature::IncomingHostile, Cmp::Le, 0.0),
                ),
                Action {
                    source: NodeSelector::new(
                        Scope::Anywhere,
                        OwnerFilter::Mine,
                        Rank::WeakestGarrison,
                    ),
                    // A quarter (vs Colonize's three-quarters): Defend keeps a heavy
                    // home reserve, so it claims a fair share but is out-raced by a
                    // pure colonizer's flood — exactly the colonize ≻ defend edge.
                    fraction: FractionBucket::Quarter,
                    target: NodeSelector::new(
                        Scope::Adjacent,
                        OwnerFilter::Empty,
                        Rank::BestColonizeValue,
                    ),
                },
            )
            .noted("expand: claim a fair share of neutrals, keeping a heavy home"),
            // 4) FLOW TO NEUTRAL: a calm node with no adjacent empty trickles ships one
            //    hop toward unclaimed space, so expansion keeps reaching the contested
            //    middle (e.g. the center of a corridor) instead of stalling at home.
            //    Deliberately gentle (small fraction, high floor) and lowest priority:
            //    enough to claim a fair share, but slow enough that a *pure* colonizer
            //    still out-produces the defender (preserving colonize ≻ defend).
            Rule::new(
                30,
                Condition::and(
                    Condition::cmp(Feature::Garrison, Cmp::Ge, 8.0),
                    Condition::cmp(Feature::IncomingHostile, Cmp::Le, 0.0),
                ),
                Action {
                    source: NodeSelector::new(
                        Scope::Anywhere,
                        OwnerFilter::Mine,
                        Rank::WeakestGarrison,
                    ),
                    fraction: FractionBucket::Half,
                    target: NodeSelector::new(
                        Scope::Adjacent,
                        OwnerFilter::Mine,
                        Rank::ClosestToNeutral,
                    ),
                },
            )
            .noted("flow: trickle spare ships toward the contested middle"),
        ],
    )
}

/// All three pure-strategy rule-sets, in canonical order.
pub fn all() -> [Policy; 3] {
    [colonize(), defend(), attack()]
}
