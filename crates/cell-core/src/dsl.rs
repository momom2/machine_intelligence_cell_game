//! # dsl — the shared `condition -> action` rule language (decision **D4**)
//!
//! This is the design's **one language for three roles** (`03-ui-layers.md`,
//! `04` D4):
//!
//! 1. the **AI policy representation** — every opponent in `02` (Automaton, Counter,
//!    Apprentice, Architect) is a [`Policy`] in this DSL;
//! 2. the **player's automation language** — the player authors the *same* [`Rule`]s
//!    to shed micromanagement (`03` "player-authored automation");
//! 3. the **Layer-3 command surface** — issuing a standing order at the abstract zoom
//!    level *is* writing a rule.
//!
//! Because all three are the same object, "reading the Architect's glass genome" and
//! "writing your own subroutine" are the same skill — the shared-vocabulary spine of
//! `00-overview.md`. Conditions test only the legible [`crate::features`] (D3), so a
//! rule is always phrased in words the player already reads off the HUD.
//!
//! ## The shape of a policy
//!
//! ```text
//! Policy = prioritized Vec<Rule>          // evaluated top-to-bottom each tick
//! Rule   = { priority, condition, action }
//! Condition = predicate over features (with And/Or/Not and comparisons)
//! Action = { source: NodeSelector, fraction: FractionBucket, target: NodeSelector }
//! ```
//!
//! The atomic action is exactly the design's `(source, fraction-bucket in
//! {25,50,75,100}%, target)` — there is deliberately no sub-action skill; **all**
//! intelligence lives in which rules get composed (`01`, `00`).
//!
//! ## How evaluation works (and why this shape)
//!
//! Evaluation is **source-driven**: a rule is "*for each* of my nodes that matches
//! the source selector *and* satisfies the condition (evaluated at that node), send
//! `fraction` of its garrison to the node its target selector resolves to." This
//! mirrors how the hardcoded policies in [`crate::policy`] loop over owned nodes and
//! pick a destination, so the DSL can reproduce them (proven in
//! [`strategies`] + the `dsl_reproduces_pure_strategies` test).
//!
//! Higher-priority rules win the right to spend a node's garrison: within a tick each
//! source node is committed by **at most one** rule (the highest-priority one that
//! both matches and finds a target), so a defend rule placed above an expand rule
//! pre-empts expansion exactly the way a prioritized heuristic list should. This
//! "first matching rule per node wins" semantics is what makes priority *mean*
//! something and keeps a policy legible: you read it top-down like the player would.
//!
//! ## Determinism
//!
//! Every selector resolves with a fixed, total tie-break (lowest [`NodeId`] wins a
//! tie on the ranking key), and nodes are visited in ascending id order. So a
//! [`Policy`] is a pure, deterministic function of [`GameState`] — the precondition
//! for the empirical fitness gate (`00`, `04`).

use crate::engine::{GameState, Params};
use crate::features::{self, NodeFeatures, OwnerFeatures};
use crate::types::*;

pub mod strategies;

// ===========================================================================
// Features the DSL can test
// ===========================================================================

/// The legible quantities a [`Condition`] may compare against a threshold.
///
/// This enum *is* the D3 feature vocabulary surfaced into the language: each variant
/// is one of the documented [`crate::features`] quantities. Per-node features are
/// read **at the candidate source node** currently under consideration; global
/// features are read for the acting seat. Keeping this list short is deliberate —
/// `03`/`04` make the DSL the game's cognitive-load pacing mechanism, so each
/// feature is one concept the campaign unlocks.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Feature {
    // ---- per-node (evaluated at the candidate source node) ----
    /// The source node's current garrison.
    Garrison,
    /// 1.0 if the source node is a frontier node, else 0.0 (is-frontier-node).
    IsFrontier,
    /// Hop distance from the source node to the nearest enemy node.
    DistanceToEnemy,
    /// Enemy ships inbound to the source node within the default look-ahead
    /// (incoming-hostile-in-N-ticks, the predictive feature).
    IncomingHostile,
    /// The source node's per-node force ratio (effective defense / incoming hostile).
    ForceRatio,
    /// Friendly ships already inbound to the source node.
    IncomingFriendly,
    /// The source node's attack force ratio (my post-loss strength here vs the
    /// weakest adjacent enemy). The *attack* archetype's strike trigger.
    AttackForceRatio,
    /// The minimum force ratio over the source node's adjacent **friendly** nodes
    /// ("is a neighbor of mine in trouble?"). The *defend* archetype's reinforce
    /// trigger.
    WeakestNeighborForceRatio,

    // ---- global / per-owner (evaluated for the acting seat) ----
    /// My production rate (ships/tick across owned nodes).
    ProductionRate,
    /// My territory share in [-1, 1].
    TerritoryShare,
    /// My unit share in [-1, 1].
    UnitShare,
    /// My raw territory count (number of nodes owned).
    MyTerritoryCount,
    /// My total units (garrisons + fleets).
    MyTotalUnits,

    /// A literal constant, so conditions can compare a feature to a fixed number or
    /// two features to each other without a special-case grammar. (Most rules use
    /// the [`Condition`] comparison helpers, which wrap this.)
    Const(f64),
}

impl Feature {
    /// Evaluate this feature at a candidate source node, given precomputed per-tick
    /// context (so the board-wide BFS and owner features are paid for once).
    fn eval(self, node: NodeId, ctx: &EvalContext) -> f64 {
        let nf = &ctx.node_feats[node];
        match self {
            Feature::Garrison => nf.garrison,
            Feature::IsFrontier => {
                if nf.is_frontier {
                    1.0
                } else {
                    0.0
                }
            }
            Feature::DistanceToEnemy => nf.distance_to_enemy,
            Feature::IncomingHostile => nf.incoming_hostile,
            Feature::ForceRatio => nf.force_ratio,
            Feature::IncomingFriendly => nf.incoming_friendly,
            Feature::AttackForceRatio => nf.attack_force_ratio,
            Feature::WeakestNeighborForceRatio => nf.weakest_neighbor_force_ratio,
            Feature::ProductionRate => ctx.owner_feats.production_rate,
            Feature::TerritoryShare => ctx.owner_feats.territory_share,
            Feature::UnitShare => ctx.owner_feats.unit_share,
            Feature::MyTerritoryCount => ctx.owner_feats.territory_count as f64,
            Feature::MyTotalUnits => ctx.owner_feats.total_units,
            Feature::Const(c) => c,
        }
    }
}

/// The comparison operators a [`Condition`] uses. Deliberately the four a player
/// reads naturally ("force ratio **below** 1", "production **at least** 5").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cmp {
    Lt,
    Le,
    Gt,
    Ge,
}

impl Cmp {
    #[inline]
    fn apply(self, a: f64, b: f64) -> bool {
        match self {
            Cmp::Lt => a < b,
            Cmp::Le => a <= b,
            Cmp::Gt => a > b,
            Cmp::Ge => a >= b,
        }
    }
}

// ===========================================================================
// Conditions
// ===========================================================================

/// A predicate over the legible features, evaluated at a candidate source node.
///
/// The leaf is [`Condition::Compare`] (one feature vs. one threshold/feature); the
/// combinators [`Condition::And`]/[`Condition::Or`]/[`Condition::Not`] compose leaves
/// without leaving the legible vocabulary. [`Condition::Always`] is the trivial
/// "this rule always applies to a matching source".
#[derive(Debug, Clone, PartialEq)]
pub enum Condition {
    /// Always true (the rule's applicability is then governed purely by its source
    /// selector finding a node + the action finding a target).
    Always,
    /// `lhs <cmp> rhs`, both evaluated at the candidate source node.
    Compare {
        lhs: Feature,
        cmp: Cmp,
        rhs: Feature,
    },
    And(Box<Condition>, Box<Condition>),
    Or(Box<Condition>, Box<Condition>),
    Not(Box<Condition>),
}

impl Condition {
    /// `feature <cmp> threshold` — the common case (compare a feature to a constant).
    pub fn cmp(feature: Feature, cmp: Cmp, threshold: f64) -> Condition {
        Condition::Compare {
            lhs: feature,
            cmp,
            rhs: Feature::Const(threshold),
        }
    }

    /// `a AND b`.
    pub fn and(a: Condition, b: Condition) -> Condition {
        Condition::And(Box::new(a), Box::new(b))
    }

    /// `a OR b`.
    pub fn or(a: Condition, b: Condition) -> Condition {
        Condition::Or(Box::new(a), Box::new(b))
    }

    /// `NOT a`. (Named `not` for legibility as a DSL constructor; this is not the
    /// `std::ops::Not` trait method.)
    #[allow(clippy::should_implement_trait)]
    pub fn not(a: Condition) -> Condition {
        Condition::Not(Box::new(a))
    }

    /// Evaluate the predicate at `node`.
    fn eval(&self, node: NodeId, ctx: &EvalContext) -> bool {
        match self {
            Condition::Always => true,
            Condition::Compare { lhs, cmp, rhs } => {
                cmp.apply(lhs.eval(node, ctx), rhs.eval(node, ctx))
            }
            Condition::And(a, b) => a.eval(node, ctx) && b.eval(node, ctx),
            Condition::Or(a, b) => a.eval(node, ctx) || b.eval(node, ctx),
            Condition::Not(a) => !a.eval(node, ctx),
        }
    }
}

// ===========================================================================
// Node selectors
// ===========================================================================

/// Which nodes a rule acts on, for both the **source** (which owned nodes spend
/// ships) and the **target** (where those ships go, resolved relative to the source).
///
/// A selector has three parts a player can read as one phrase:
/// *scope* (where to look) + *owner filter* (whose nodes) + *ranking* (which one to
/// pick when several qualify). E.g. "the **weakest** (ranking) **enemy** (filter)
/// node **adjacent** (scope) to here" is `weakest enemy frontier` in the prose of
/// the design.
///
/// As a **source** selector, the ranking is unused (every matching owned node is a
/// candidate); as a **target** selector, the ranking chooses the single destination.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NodeSelector {
    /// Where to look for candidate nodes.
    pub scope: Scope,
    /// Restrict to nodes with this ownership relation to the acting seat.
    pub filter: OwnerFilter,
    /// When used as a target, pick the candidate that extremizes this key.
    pub rank: Rank,
}

impl NodeSelector {
    /// Shorthand constructor.
    pub const fn new(scope: Scope, filter: OwnerFilter, rank: Rank) -> NodeSelector {
        NodeSelector { scope, filter, rank }
    }
}

/// Where a selector looks for candidates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    /// The source node itself (used by a target selector to mean "stay here" — e.g.
    /// reinforce *this* node from elsewhere is expressed the other way around, so
    /// this is mainly for completeness/future rules).
    Here,
    /// Nodes directly adjacent to the source node (one hop). The common case: the
    /// atomic action only moves along a single edge.
    Adjacent,
    /// Any node on the board (used by source selectors that scan all owned nodes, and
    /// by target selectors that pick a global extremum like "my weakest frontier").
    Anywhere,
}

/// Ownership relation to the **acting seat** (`me`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OwnerFilter {
    /// Owned by me.
    Mine,
    /// Owned by the enemy.
    Enemy,
    /// Unowned / neutral (a colonization target).
    Empty,
    /// Not owned by me (enemy or neutral) — i.e. an expansion/attack target.
    NotMine,
    /// Any owner (no filter).
    Any,
}

impl OwnerFilter {
    fn matches(self, owner: Owner, me: Owner) -> bool {
        match self {
            OwnerFilter::Mine => owner == me,
            OwnerFilter::Enemy => owner == me.opponent(),
            OwnerFilter::Empty => owner == Owner::Neutral,
            OwnerFilter::NotMine => owner != me,
            OwnerFilter::Any => true,
        }
    }
}

/// How a **target** selector breaks ties among candidates: pick the one that
/// minimizes/maximizes a legible quantity. Each variant names a concept the player
/// already uses ("the weakest enemy", "the nearest empty", "the most threatened of
/// mine").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rank {
    /// Smallest garrison (the *weakest* node) — e.g. "weakest enemy" for a strike,
    /// "weakest frontier" for reinforcement.
    WeakestGarrison,
    /// Largest garrison (the *strongest* node) — e.g. a staging node to mass on.
    StrongestGarrison,
    /// Shortest edge from the source (the *nearest* node).
    Nearest,
    /// Best colonization value `production_mult / (edge_len + 1)` — the rate-aware
    /// "best empty to grab" from the `01` scheduling discussion (not naive nearest).
    BestColonizeValue,
    /// Lowest force ratio (the *most threatened* node) — for defensive targeting.
    LowestForceRatio,
    /// Closest to the enemy by hop distance (drive a spearhead toward contact / the
    /// best staging node).
    ClosestToEnemy,
    /// Closest to unclaimed (neutral) space by hop distance — route ships toward
    /// economy without massing on the enemy (keeps Colonize a pure economist).
    ClosestToNeutral,
}

// ===========================================================================
// Action & Rule
// ===========================================================================

/// An atomic action: send `fraction` of the **source** node's garrison to the
/// **target** node. Exactly the design's `(source, fraction-bucket, target)`.
///
/// `source` enumerates candidate owned nodes; `target` is resolved **relative to
/// each candidate source** (so an `Adjacent` target means "adjacent to that source").
/// If the target selector finds no node for a given source, that source emits nothing
/// for this rule (and remains free for a lower-priority rule).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Action {
    pub source: NodeSelector,
    pub fraction: FractionBucket,
    pub target: NodeSelector,
}

/// One `condition -> action` heuristic with a priority. Higher `priority` is
/// considered first; the first rule (per source node) whose condition holds *and*
/// whose action resolves a target commits that node.
#[derive(Debug, Clone, PartialEq)]
pub struct Rule {
    /// Higher = considered earlier. Ties broken by position in the policy `Vec`.
    pub priority: i32,
    /// When this rule applies (tested at each candidate source node).
    pub condition: Condition,
    /// What to do.
    pub action: Action,
    /// Optional human label, surfaced in the glass-genome diff view (`02`).
    pub note: &'static str,
}

impl Rule {
    /// Construct a rule (note defaults to empty).
    pub fn new(priority: i32, condition: Condition, action: Action) -> Rule {
        Rule {
            priority,
            condition,
            action,
            note: "",
        }
    }

    /// Builder: attach a human-readable note (for legible diffs/logs).
    pub fn noted(mut self, note: &'static str) -> Rule {
        self.note = note;
        self
    }
}

/// A complete strategy: a prioritized list of rules evaluated each tick.
///
/// This is the single object every opponent and the player share. Construct one with
/// [`Policy::new`] (which sorts by priority) and run it through [`evaluate`] or wrap
/// it in [`DslPolicy`] to plug into the engine's [`crate::policy::Policy`] trait.
#[derive(Debug, Clone, PartialEq)]
pub struct Policy {
    /// Rules in evaluation order (highest priority first). Kept sorted by [`new`].
    pub rules: Vec<Rule>,
    /// Optional name for reports/diffs.
    pub name: &'static str,
}

impl Policy {
    /// Build a policy from rules, sorting by descending priority (stable, so equal
    /// priorities keep author order). Sorting once here means [`evaluate`] can assume
    /// the list is already in evaluation order.
    pub fn new(name: &'static str, mut rules: Vec<Rule>) -> Policy {
        // Descending priority; `sort_by_key` is stable, so equal priorities keep the
        // author's order (the documented tie-break the evaluator relies on).
        rules.sort_by_key(|r| std::cmp::Reverse(r.priority));
        Policy { rules, name }
    }
}

// ===========================================================================
// The evaluator
// ===========================================================================

/// Per-tick context shared across all rule evaluations: the legible features computed
/// **once** for the acting seat. Building this is the only non-trivial cost of a
/// tick's evaluation (one board-wide BFS + a pass over fleets).
struct EvalContext {
    me: Owner,
    node_feats: Vec<NodeFeatures>,
    owner_feats: OwnerFeatures,
    /// Distance-to-neutral field, computed lazily only if a rule needs it.
    dist_neutral: Option<Vec<f64>>,
}

impl EvalContext {
    fn build(state: &GameState, me: Owner, params: &Params) -> EvalContext {
        EvalContext {
            me,
            node_feats: NodeFeatures::compute_all(state, me, params),
            owner_feats: OwnerFeatures::compute(state, me, params),
            dist_neutral: None,
        }
    }

    /// Distance-to-neutral field (memoized): only the [`Rank::ClosestToNeutral`]
    /// ranking needs it, so most ticks never pay for it.
    fn dist_neutral(&mut self, state: &GameState) -> &[f64] {
        if self.dist_neutral.is_none() {
            self.dist_neutral = Some(features::distance_to_neutral_field(state));
        }
        self.dist_neutral.as_ref().unwrap()
    }
}

/// Evaluate a [`Policy`] against `state` for seat `me`, returning this tick's
/// commands.
///
/// Semantics (see the module docs): rules are tried highest-priority first; each
/// owned node is committed by **at most one** rule per tick (the first that matches
/// and resolves a target). The result is deterministic.
pub fn evaluate(policy: &Policy, state: &GameState, me: Owner, params: &Params) -> Vec<Command> {
    let mut ctx = EvalContext::build(state, me, params);
    let mut cmds = Vec::new();
    // Which owned nodes have already committed their garrison this tick.
    let mut committed = vec![false; state.nodes.len()];
    // Track friendly inbound *we are creating this tick* so two rules don't both pile
    // onto the same target, and so target selection respects pending sends. We fold
    // these into a per-node "pending friendly" map seeded from real in-flight fleets.
    let mut pending_friendly = vec![0.0; state.nodes.len()];
    for f in &state.fleets {
        if f.owner == me {
            pending_friendly[f.to] += f.count;
        }
    }

    for rule in &policy.rules {
        // Enumerate candidate source nodes for this rule.
        let sources = resolve_sources(&rule.action.source, state, &ctx);
        for src in sources {
            if committed[src] {
                continue;
            }
            if !rule.condition.eval(src, &ctx) {
                continue;
            }
            // Resolve the target relative to this source.
            let Some(tgt) =
                resolve_target(&rule.action.target, src, state, &mut ctx, &pending_friendly)
            else {
                continue;
            };
            if tgt == src {
                continue; // never send to self
            }
            cmds.push(Command {
                source: src,
                target: tgt,
                fraction: rule.action.fraction,
            });
            committed[src] = true;
            // Record the friendly ships now inbound to `tgt` so later rules this tick
            // see them — this both credits reinforcements (for force-ratio targeting)
            // and prevents two sources from claiming the *same* empty node. We record
            // for every target owner, including empty: an empty target we just sent a
            // fleet at now has friendly ships inbound, which is exactly what the
            // empty-target de-dup in `resolve_target` checks.
            let sent = state.nodes[src].garrison * rule.action.fraction.as_f64();
            pending_friendly[tgt] += sent;
        }
    }
    cmds
}

/// Enumerate the candidate **source** nodes for a selector: owned nodes (almost
/// always `OwnerFilter::Mine`) in the selector's scope, in ascending id order. The
/// ranking is ignored for sources — every matching node is a candidate the rule may
/// fire on. We additionally require a positive garrison (an empty node can spend
/// nothing) to avoid emitting no-op commands.
fn resolve_sources(sel: &NodeSelector, state: &GameState, ctx: &EvalContext) -> Vec<NodeId> {
    let me = ctx.me;
    match sel.scope {
        // A source selector with Anywhere scope scans the whole board (the usual
        // case: "every owned node with surplus"). Adjacent/Here source scopes are
        // unusual but supported for symmetry.
        Scope::Anywhere => (0..state.nodes.len())
            .filter(|&i| sel.filter.matches(state.nodes[i].owner, me) && state.nodes[i].garrison > 0.0)
            .collect(),
        Scope::Here => Vec::new(), // a source has no "here" without a prior source
        Scope::Adjacent => Vec::new(), // not meaningful for source enumeration
    }
}

/// Resolve a **target** selector relative to source node `src`: among the candidates
/// in scope passing the owner filter, return the one extremizing the ranking key
/// (lowest [`NodeId`] breaks ties). `None` if no candidate qualifies.
fn resolve_target(
    sel: &NodeSelector,
    src: NodeId,
    state: &GameState,
    ctx: &mut EvalContext,
    pending_friendly: &[f64],
) -> Option<NodeId> {
    let me = ctx.me;

    // Gather candidate node ids per scope.
    let candidates: Vec<NodeId> = match sel.scope {
        Scope::Here => vec![src],
        Scope::Adjacent => state.neighbors(src).iter().map(|&(_, n)| n).collect(),
        Scope::Anywhere => (0..state.nodes.len()).collect(),
    };

    // For ClosestToNeutral we need the (memoized) field; compute before the loop to
    // avoid borrow issues, only when that ranking is used.
    let dist_neutral: Option<Vec<f64>> = if matches!(sel.rank, Rank::ClosestToNeutral) {
        Some(ctx.dist_neutral(state).to_vec())
    } else {
        None
    };

    let mut best: Option<(NodeId, f64)> = None;
    for n in candidates {
        if n == src {
            // A target equal to the source is only valid for Scope::Here; for the
            // movement scopes skip it (no self-edge).
            if !matches!(sel.scope, Scope::Here) {
                continue;
            }
        }
        if !sel.filter.matches(state.nodes[n].owner, me) {
            continue;
        }
        // Skip targets a friendly fleet is already saturating, for colonization
        // targets, so we don't dribble multiple fleets onto one empty node. (Only
        // applies to Empty targets; reinforcement deliberately stacks.)
        if matches!(sel.filter, OwnerFilter::Empty) && pending_friendly[n] > 0.0 {
            continue;
        }
        let key = rank_key(sel.rank, src, n, state, ctx, dist_neutral.as_deref());
        // We always *minimize* `key`; rank_key negates where "largest wins".
        match best {
            Some((_, bk)) if key >= bk => {}
            _ => best = Some((n, key)),
        }
    }
    best.map(|(n, _)| n)
}

/// The sort key for a ranking, arranged so the evaluator always picks the **minimum**
/// (ties → lowest id, handled by the caller's `>=` test). Rankings whose intent is
/// "largest" return a negated value.
fn rank_key(
    rank: Rank,
    src: NodeId,
    n: NodeId,
    state: &GameState,
    ctx: &EvalContext,
    dist_neutral: Option<&[f64]>,
) -> f64 {
    match rank {
        Rank::WeakestGarrison => state.nodes[n].garrison,
        Rank::StrongestGarrison => -state.nodes[n].garrison,
        Rank::Nearest => features::direct_time(state, src, n),
        Rank::BestColonizeValue => {
            // Larger value is better → negate. value = mult / (edge_len + 1).
            let v = state.nodes[n].production_mult / (features::direct_time(state, src, n) + 1.0);
            -v
        }
        Rank::LowestForceRatio => ctx.node_feats[n].force_ratio,
        Rank::ClosestToEnemy => ctx.node_feats[n].distance_to_enemy,
        Rank::ClosestToNeutral => dist_neutral.map(|d| d[n]).unwrap_or(f64::INFINITY),
    }
}

// ===========================================================================
// Adapter to the engine's Policy trait
// ===========================================================================

/// Wraps a DSL [`Policy`] so it can be played by the engine wherever a
/// [`crate::policy::Policy`] is expected (the match runner, the harness, the sweep).
///
/// This is the bridge that lets the *same* rule-set used by the Architect also drive
/// a live match — the computation/spectacle decoupling at the policy layer.
pub struct DslPolicy {
    policy: Policy,
}

impl DslPolicy {
    pub fn new(policy: Policy) -> DslPolicy {
        DslPolicy { policy }
    }

    /// Borrow the underlying rule-set (for diffing / display).
    pub fn rules(&self) -> &Policy {
        &self.policy
    }
}

impl crate::policy::Policy for DslPolicy {
    fn decide(&mut self, state: &GameState, me: Owner, params: &Params) -> Vec<Command> {
        evaluate(&self.policy, state, me, params)
    }

    fn name(&self) -> &'static str {
        self.policy.name
    }
}

#[cfg(test)]
mod tests {
    //! DSL evaluator + expressiveness tests. These live in the crate so they compile
    //! into the library's own test runner.
    use super::*;
    use crate::maps::{all_maps, corridor7};
    use crate::policy::Policy as EnginePolicy;

    /// R2's recommended operating point (R2_RESULTS.md): the most robust point where
    /// the hardcoded triad's cycle holds. The DSL must reproduce the cycle here.
    fn operating_params() -> Params {
        Params {
            r: 0.6,
            k: 2.25,
            l: 0.15,
            ..Params::default()
        }
    }

    const HORIZON: u64 = 600;

    fn score_a(
        base: &GameState,
        a: &mut dyn EnginePolicy,
        b: &mut dyn EnginePolicy,
        params: &Params,
    ) -> f64 {
        base.clone().run_match(a, b, params, HORIZON).score_a
    }

    /// Mean score for `make_first`'s policy across **both seatings** (cancels the
    /// fixed "A acts before B" tie-break + positional bias) — generic over any boxed
    /// policy so it works for DSL policies, like `harness::duel` does for archetypes.
    fn duel_score(
        base: &GameState,
        make_first: &dyn Fn() -> Box<dyn EnginePolicy>,
        make_second: &dyn Fn() -> Box<dyn EnginePolicy>,
        params: &Params,
    ) -> f64 {
        let s1 = score_a(base, make_first().as_mut(), make_second().as_mut(), params);
        let s2 = -score_a(base, make_second().as_mut(), make_first().as_mut(), params);
        0.5 * (s1 + s2)
    }

    fn duel_over_maps(
        make_first: &dyn Fn() -> Box<dyn EnginePolicy>,
        make_second: &dyn Fn() -> Box<dyn EnginePolicy>,
        params: &Params,
    ) -> f64 {
        let maps = all_maps();
        let n = maps.len() as f64;
        maps.iter()
            .map(|m| duel_score(&m.state, make_first, make_second, params))
            .sum::<f64>()
            / n
    }

    fn dsl_colonize() -> Box<dyn EnginePolicy> {
        Box::new(DslPolicy::new(strategies::colonize()))
    }
    fn dsl_defend() -> Box<dyn EnginePolicy> {
        Box::new(DslPolicy::new(strategies::defend()))
    }
    fn dsl_attack() -> Box<dyn EnginePolicy> {
        Box::new(DslPolicy::new(strategies::attack()))
    }

    // -------------------------------------------------------------------
    // THE expressiveness proof: the DSL reproduces the triad cycle.
    // -------------------------------------------------------------------

    /// **keyResult.** The three pure strategies, written purely as DSL rule-sets,
    /// reproduce the rock-paper-scissors cycle at R2's operating point:
    ///   attack ≻ colonize, colonize ≻ defend, defend ≻ attack,
    /// each winning by a clear margin (mean over all maps and both seatings). This is
    /// the evidence the DSL is expressive enough to carry the strategic core.
    #[test]
    fn dsl_reproduces_pure_strategies() {
        let params = operating_params();
        let ac = duel_over_maps(&dsl_attack, &dsl_colonize, &params); // attack ≻ colonize
        let cd = duel_over_maps(&dsl_colonize, &dsl_defend, &params); // colonize ≻ defend
        let da = duel_over_maps(&dsl_defend, &dsl_attack, &params); // defend ≻ attack

        // Above the R2 epsilon (0.05) so the cycle is a genuine property, not noise.
        let epsilon = 0.05;
        assert!(ac > epsilon, "DSL attack should beat DSL colonize, margin {ac}");
        assert!(cd > epsilon, "DSL colonize should beat DSL defend, margin {cd}");
        assert!(da > epsilon, "DSL defend should beat DSL attack, margin {da}");
    }

    /// Cross-check: each DSL archetype captures the *same* strategy as its hardcoded
    /// twin (it does not lose badly head-to-head), so the cycle above is the triad's
    /// and not some unrelated three-way that merely happens to cycle.
    #[test]
    fn dsl_archetypes_match_their_hardcoded_twins() {
        use crate::policy::{Attack, Colonize, Defend};
        let params = operating_params();
        let col =
            duel_over_maps(&dsl_colonize, &(|| Box::new(Colonize) as Box<dyn EnginePolicy>), &params);
        let def =
            duel_over_maps(&dsl_defend, &(|| Box::new(Defend) as Box<dyn EnginePolicy>), &params);
        let atk =
            duel_over_maps(&dsl_attack, &(|| Box::new(Attack) as Box<dyn EnginePolicy>), &params);
        // A loose floor: the point is that each DSL archetype plays the *same*
        // strategy as its hardcoded twin (competitive head-to-head), not that it is
        // identically tuned. The hardcoded versions are hand-optimized, so the DSL
        // re-expression may be somewhat weaker without being a *different* strategy.
        let floor = -0.45;
        assert!(col > floor, "DSL colonize too far from hardcoded: {col}");
        assert!(def > floor, "DSL defend too far from hardcoded: {def}");
        assert!(atk > floor, "DSL attack too far from hardcoded: {atk}");
    }

    /// Diagnostic (ignored by default): per-map cycle margins + DSL-vs-hardcoded.
    /// Run with: `cargo test -p cell-core diag_margins -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn diag_margins() {
        use crate::policy::{Attack, Colonize, Defend};
        let params = operating_params();
        println!("\n== DSL cycle, per map ==");
        for m in all_maps() {
            let ac = duel_score(&m.state, &dsl_attack, &dsl_colonize, &params);
            let cd = duel_score(&m.state, &dsl_colonize, &dsl_defend, &params);
            let da = duel_score(&m.state, &dsl_defend, &dsl_attack, &params);
            println!("{:>13}: A>C {:+.4}  C>D {:+.4}  D>A {:+.4}", m.name, ac, cd, da);
        }
        let ac = duel_over_maps(&dsl_attack, &dsl_colonize, &params);
        let cd = duel_over_maps(&dsl_colonize, &dsl_defend, &params);
        let da = duel_over_maps(&dsl_defend, &dsl_attack, &params);
        println!("{:>13}: A>C {:+.4}  C>D {:+.4}  D>A {:+.4}", "MEAN", ac, cd, da);
        let col =
            duel_over_maps(&dsl_colonize, &(|| Box::new(Colonize) as Box<dyn EnginePolicy>), &params);
        let def =
            duel_over_maps(&dsl_defend, &(|| Box::new(Defend) as Box<dyn EnginePolicy>), &params);
        let atk =
            duel_over_maps(&dsl_attack, &(|| Box::new(Attack) as Box<dyn EnginePolicy>), &params);
        println!("vs hardcoded twin: colonize {:+.4}  defend {:+.4}  attack {:+.4}", col, def, atk);
    }

    // -------------------------------------------------------------------
    // Evaluator semantics
    // -------------------------------------------------------------------

    /// Identical inputs → identical commands.
    #[test]
    fn evaluator_is_deterministic() {
        let params = operating_params();
        let state = corridor7().state;
        let pol = strategies::attack();
        let c1 = evaluate(&pol, &state, Owner::A, &params);
        let c2 = evaluate(&pol, &state, Owner::A, &params);
        assert_eq!(c1, c2, "evaluation must be deterministic");
    }

    /// Higher-priority rules pre-empt lower ones on the same source node.
    #[test]
    fn priority_preempts_per_node() {
        let params = Params::default();
        let nodes = vec![
            Node { owner: Owner::A, garrison: 20.0, production_mult: 1.0 },
            Node { owner: Owner::Neutral, garrison: 0.0, production_mult: 1.0 },
            Node { owner: Owner::Neutral, garrison: 0.0, production_mult: 1.0 },
        ];
        let edges = vec![
            Edge { a: 0, b: 1, length: 1.0 },
            Edge { a: 0, b: 2, length: 1.0 },
        ];
        let state = GameState::new(nodes, edges);
        let hi = Rule::new(
            100,
            Condition::Always,
            Action {
                source: NodeSelector::new(Scope::Anywhere, OwnerFilter::Mine, Rank::WeakestGarrison),
                fraction: FractionBucket::All,
                target: NodeSelector::new(Scope::Adjacent, OwnerFilter::Empty, Rank::Nearest),
            },
        );
        let lo = Rule::new(
            10,
            Condition::Always,
            Action {
                source: NodeSelector::new(Scope::Anywhere, OwnerFilter::Mine, Rank::WeakestGarrison),
                fraction: FractionBucket::Half,
                target: NodeSelector::new(Scope::Adjacent, OwnerFilter::Empty, Rank::StrongestGarrison),
            },
        );
        let pol = Policy::new("t", vec![lo, hi]); // out of order on purpose
        let cmds = evaluate(&pol, &state, Owner::A, &params);
        assert_eq!(cmds.len(), 1, "node 0 must commit once, got {cmds:?}");
        assert_eq!(cmds[0].fraction, FractionBucket::All, "high-priority rule must win");
    }

    /// A false condition emits nothing; flipping the threshold makes it fire.
    #[test]
    fn condition_gates_firing() {
        let params = Params::default();
        let nodes = vec![
            Node { owner: Owner::A, garrison: 5.0, production_mult: 1.0 },
            Node { owner: Owner::Neutral, garrison: 0.0, production_mult: 1.0 },
        ];
        let edges = vec![Edge { a: 0, b: 1, length: 1.0 }];
        let state = GameState::new(nodes, edges);
        let mk = |threshold: f64| {
            Policy::new(
                "g",
                vec![Rule::new(
                    10,
                    Condition::cmp(Feature::Garrison, Cmp::Ge, threshold),
                    Action {
                        source: NodeSelector::new(Scope::Anywhere, OwnerFilter::Mine, Rank::WeakestGarrison),
                        fraction: FractionBucket::All,
                        target: NodeSelector::new(Scope::Adjacent, OwnerFilter::Empty, Rank::Nearest),
                    },
                )],
            )
        };
        assert_eq!(evaluate(&mk(3.0), &state, Owner::A, &params).len(), 1);
        assert_eq!(evaluate(&mk(50.0), &state, Owner::A, &params).len(), 0);
    }

    /// A DSL policy plugged into the engine actually plays: DSL Colonize beats idle.
    #[test]
    fn dsl_policy_plays_through_engine() {
        let params = operating_params();
        let base = corridor7().state;
        let mut col = dsl_colonize();
        let mut idle = Box::new(Idle) as Box<dyn EnginePolicy>;
        let out = base.run_match(col.as_mut(), idle.as_mut(), &params, 200);
        assert!(out.score_a > 0.0, "DSL colonize should beat idle, score_a={}", out.score_a);
    }

    struct Idle;
    impl EnginePolicy for Idle {
        fn name(&self) -> &'static str {
            "Idle"
        }
        fn decide(&mut self, _s: &GameState, _me: Owner, _p: &Params) -> Vec<Command> {
            Vec::new()
        }
    }

    /// Two owned nodes adjacent to the same lone empty node must not both colonize it
    /// in one tick (the evaluator respects pending sends to empty targets).
    #[test]
    fn empty_target_not_double_claimed() {
        let params = Params::default();
        let nodes = vec![
            Node { owner: Owner::A, garrison: 20.0, production_mult: 1.0 },
            Node { owner: Owner::A, garrison: 20.0, production_mult: 1.0 },
            Node { owner: Owner::Neutral, garrison: 0.0, production_mult: 1.0 },
        ];
        let edges = vec![
            Edge { a: 0, b: 2, length: 1.0 },
            Edge { a: 1, b: 2, length: 1.0 },
        ];
        let state = GameState::new(nodes, edges);
        let pol = strategies::colonize();
        let cmds = evaluate(&pol, &state, Owner::A, &params);
        let to_empty = cmds.iter().filter(|c| c.target == 2).count();
        assert_eq!(to_empty, 1, "only one fleet should target the lone empty, got {cmds:?}");
    }
}
