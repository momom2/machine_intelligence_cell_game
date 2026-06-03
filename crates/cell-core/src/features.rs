//! # features — the legible feature/HUD vocabulary (decision **D3**)
//!
//! These are the *derived legible quantities* the design (`01-mechanics.md`,
//! `04` D3) makes do **triple duty**:
//!
//! 1. **Player HUD** — what the human reads off the screen at a glance.
//! 2. **AI input features** — the only things a [`crate::dsl`] `Condition` may test.
//! 3. **Fitness shaping** — terms the Architect's evaluator can reward.
//!
//! This triple use *is* the shared-vocabulary spine from `00-overview.md`: the AI's
//! conditions, the player's mental model, and the fitness signal are written in the
//! **same words**. The hard constraint that shapes every choice here comes from the
//! arc-1 capstone (`02`): a **human must be able to infer an opponent's
//! colonize/defend/attack mix at a glance** from these numbers. So each feature
//! below is justified twice — once for *why it drives a policy* and once for *why it
//! is legible-at-a-glance*. If a quantity cannot be read off the board by eye, it
//! does not belong in this module.
//!
//! ## Two grains of feature
//!
//! * **Global / per-owner** ([`OwnerFeatures`]) — the macro picture: how big is my
//!   economy, how much of the board do I hold. These are the numbers a player scans
//!   in the top bar; they let you read *expansion rate* (the colonize signal).
//! * **Per-node** ([`NodeFeatures`]) — the micro picture at one structure: am I
//!   strong here, is this the front line, is something inbound. These are what a
//!   player reads by looking at a single planet; they let you read *aggression
//!   timing* and *frontier behavior* (the attack/defend signals).
//!
//! Together those two grains are exactly the observables the capstone names —
//! "expansion rate, aggression timing, frontier behavior" (`02`) — which is why this
//! set is sufficient to infer a mix.
//!
//! ## Determinism & cost
//!
//! Like the rest of `cell-core`, everything here is a pure function of
//! [`GameState`] with **no RNG** and no allocation on the hot per-node paths (the
//! two BFS helpers allocate a single `Vec` sized to the node count, which the
//! evaluator computes once per tick and reuses). This keeps the features cheap
//! enough to call inside the Architect's tight evaluation loop.

use crate::engine::{GameState, Params};
use crate::types::*;
use std::collections::VecDeque;

/// Macro, per-owner features — the "top bar" a player scans to read the **economy**
/// and **board-control** picture. These are what make the *colonize* signal legible:
/// a colonizer's `territory_share` and `production_rate` climb fast while its army
/// stays thin.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct OwnerFeatures {
    /// **Production rate** — ships produced per tick across all owned nodes
    /// (`sum of r * production_mult` over owned nodes). *Drives policy:* it is the
    /// economic engine; a rule can hold back aggression until production crosses a
    /// threshold. *Legible:* it is just "how fast my numbers are going up", the first
    /// thing a cell-game player feels, and the clearest tell that an opponent is
    /// playing economic (colonize).
    pub production_rate: f64,

    /// **Territory share** in [-1, 1] — `(my_nodes - enemy_nodes) /
    /// (my_nodes + enemy_nodes)`. *Drives policy:* board dominance; a rule can switch
    /// from expand to defend once ahead on territory. *Legible:* a glance at the map
    /// colors gives it immediately — count the dots. A rising territory share with a
    /// flat army is the visual signature of colonize.
    pub territory_share: f64,

    /// **Unit share** in [-1, 1] — `(my_units - enemy_units) /
    /// (my_units + enemy_units)`, counting garrisons **and** in-flight fleets.
    /// *Drives policy:* raw military balance; the precondition for a safe all-in.
    /// *Legible:* it is the army-strength bar; together with territory share it
    /// separates the three styles — high-units/low-territory reads as *attack*
    /// (massing), high-territory/low-units reads as *colonize*.
    pub unit_share: f64,

    /// Number of nodes this owner controls (the raw count behind `territory_share`,
    /// exposed because the DSL's `MyTerritoryCount` predicate is more legible as a
    /// count than a ratio for small boards).
    pub territory_count: u32,

    /// Total ships this owner controls (garrisons + fleets) — the raw count behind
    /// `unit_share`.
    pub total_units: f64,
}

impl OwnerFeatures {
    /// Compute the macro features for `who` from the current state and params.
    ///
    /// `params` is needed only for [`OwnerFeatures::production_rate`] (it multiplies
    /// by the swept colonization rate `r`); the share terms are pure board reads.
    pub fn compute(state: &GameState, who: Owner, params: &Params) -> OwnerFeatures {
        let opp = who.opponent();

        let production_rate: f64 = state
            .nodes
            .iter()
            .filter(|n| n.owner == who)
            .map(|n| params.r * n.production_mult)
            .sum();

        let my_t = state.territory(who);
        let op_t = state.territory(opp);
        let my_u = state.total_units(who);
        let op_u = state.total_units(opp);

        OwnerFeatures {
            production_rate,
            territory_share: signed_share(my_t as f64, op_t as f64),
            unit_share: signed_share(my_u, op_u),
            territory_count: my_t as u32,
            total_units: my_u,
        }
    }
}

/// Micro, per-node features — what a player reads by **looking at one structure**.
/// These carry the *attack* and *defend* signals: a force ratio below 1 on a
/// frontier node with hostiles inbound is exactly the picture of "about to be
/// attacked", and reading those across the board is how the capstone's mix-inference
/// is done.
///
/// All fields are from the perspective of a chosen owner `me` (the seat whose policy
/// is being evaluated, or whose HUD is being drawn).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NodeFeatures {
    /// The node this describes.
    pub node: NodeId,

    /// Who owns the node right now (so a selector can filter mine/enemy/empty).
    pub owner: Owner,

    /// Current garrison on the node (real-valued, mean-field).
    pub garrison: f64,

    /// **Per-node force ratio** — `my_effective_defense / incoming_hostile`, capped
    /// at [`F_RATIO_CAP`] when nothing is inbound (read as "overwhelmingly safe").
    /// "Effective defense" credits the defender advantage `sqrt(k)` and friendly
    /// reinforcements already inbound, matching how the battle will actually resolve.
    /// *Drives policy:* the core defend trigger — reinforce any owned node whose
    /// ratio drops below 1. *Legible:* it is the "am I winning this fight" number a
    /// player estimates by eyeballing the two stacks; the square law makes it the
    /// honest predictor because 2x ships ~ 4x power.
    pub force_ratio: f64,

    /// **Is-frontier-node** — true iff the node touches at least one node **not**
    /// owned by `me` (enemy or neutral). *Drives policy:* frontier nodes are where
    /// expansion and defense happen; interior nodes are safe to strip for
    /// reinforcements. *Legible:* the front line is the most visually obvious thing
    /// on the board — the edge between your color and everything else. Whether an
    /// opponent garrisons its frontier or leaves it thin is the single clearest
    /// defend-vs-colonize tell.
    pub is_frontier: bool,

    /// **Distance-to-nearest-enemy** — hop count (BFS over edges) from this node to
    /// the nearest enemy-owned node; `+inf` if the enemy is unreachable/eliminated.
    /// *Drives policy:* staging distance for a strike and urgency for defense; a rule
    /// can mass on the node closest to the enemy. *Legible:* "how far is the fight
    /// from here", which a player reads straight off the graph. A shrinking
    /// distance-to-enemy with a growing local stack is the visual signature of an
    /// incoming *attack*.
    pub distance_to_enemy: f64,

    /// **Incoming hostile fleet in N ticks** — total enemy ship count whose ETA to
    /// this node is `<= horizon` (the predictive term). *Drives policy:* the whole
    /// point of *predictive* conditions — defend *before* the blow lands, during the
    /// attacker's undock window. *Legible:* on screen these are the fleets visibly
    /// streaking toward the node; a player literally watches them coming. `01` calls
    /// out that the movement model's undock delay is what *makes* this readable in
    /// time to react, and notes predictive features are "exactly what make a policy
    /// look like it is thinking".
    pub incoming_hostile: f64,

    /// Friendly ships already inbound to this node (so a policy does not double-send
    /// reinforcements, and so the HUD can show "help is on the way"). Folded into
    /// [`NodeFeatures::force_ratio`] but exposed raw because it is its own legible
    /// quantity ("reinforcements en route").
    pub incoming_friendly: f64,

    /// **Attack force ratio** — `my_garrison * (1 - l) / weakest_adjacent_enemy
    /// defense`, i.e. the *offensive* mirror of [`NodeFeatures::force_ratio`]: "if I
    /// strike the weakest enemy planet next door with everything here, do I have the
    /// majority the timing push needs?" The `(1 - l)` factor pre-pays the
    /// commitment loss so a threshold near 1 means "I clear it after losses".
    /// `0.0` if no enemy is adjacent (nothing to attack). *Drives policy:* the
    /// *attack* archetype's strike trigger — commit when this crosses ~1. *Legible:*
    /// it is exactly the question a player asks before a poke — "am I bigger than
    /// that planet?" — and reading whether an opponent's frontier stacks *exceed*
    /// yours is the clearest *attack* tell (massing for a push).
    pub attack_force_ratio: f64,

    /// **Weakest-neighbor force ratio** — the minimum [`NodeFeatures::force_ratio`]
    /// over this node's adjacent **friendly** nodes (or [`F_RATIO_CAP`] if it has no
    /// friendly neighbor). *Drives policy:* lets an interior node decide "a planet
    /// next to me is losing its fight" and push help, without the rule grammar
    /// needing to peek at the target's features. *Legible:* it is "is anyone next to
    /// me in trouble?", the glance a player makes before sending reinforcements — and
    /// a player who keeps this high (never lets a neighbor get outnumbered) is
    /// visibly playing *defend*.
    pub weakest_neighbor_force_ratio: f64,
}

/// Cap applied to [`NodeFeatures::force_ratio`] when no hostiles are inbound, so the
/// value is a finite, comparable "overwhelmingly safe" sentinel rather than an
/// infinity that would poison averaging/fitness terms. Large enough that any sane
/// `>=` threshold treats it as "totally safe".
pub const F_RATIO_CAP: f64 = 1e6;

impl NodeFeatures {
    /// Compute one node's features for owner `me`.
    ///
    /// `dist_to_enemy` and `incoming_within` are passed in (rather than recomputed)
    /// so a caller scanning every node pays for the board-wide BFS **once**. Use
    /// [`distance_to_enemy_field`] to produce `dist_to_enemy` and
    /// [`Params::undock_delay`]-derived horizon for `look_ahead`. See
    /// [`NodeFeatures::compute_all`] for the batched convenience path.
    pub fn compute(
        state: &GameState,
        me: Owner,
        node: NodeId,
        dist_to_enemy: &[f64],
        look_ahead: f64,
        params: &Params,
    ) -> NodeFeatures {
        let owner = state.nodes[node].owner;
        let garrison = state.nodes[node].garrison;
        let incoming_hostile = incoming_hostile(state, me, node, look_ahead);
        let incoming_friendly = incoming_friendly(state, me, node);

        // Effective defense as the battle will resolve it: a garrison ship is worth
        // sqrt(k) attacker ships (see engine docs), plus friendlies already inbound.
        let eff_defense = garrison * params.k.sqrt() + incoming_friendly;
        let force_ratio = if incoming_hostile <= 0.0 {
            F_RATIO_CAP
        } else {
            (eff_defense / incoming_hostile).min(F_RATIO_CAP)
        };

        // Attack force ratio: my post-loss strength here vs the weakest adjacent
        // enemy's garrison. 0 if no enemy is adjacent. (Self-contained: needs only
        // this node and its neighbors, so it is final after this single pass.)
        let attack_force_ratio = {
            let mut weakest_enemy: Option<f64> = None;
            for &(_, nbr) in state.neighbors(node) {
                if state.nodes[nbr].owner == me.opponent() {
                    let d = state.nodes[nbr].garrison;
                    weakest_enemy = Some(weakest_enemy.map_or(d, |w: f64| w.min(d)));
                }
            }
            match weakest_enemy {
                None => 0.0,
                Some(def) => {
                    let attack_strength = garrison * (1.0 - params.l);
                    if def <= 0.0 {
                        // Undefended enemy planet: capturing it is free → treat as
                        // overwhelmingly favorable.
                        F_RATIO_CAP
                    } else {
                        (attack_strength / def).min(F_RATIO_CAP)
                    }
                }
            }
        };

        NodeFeatures {
            node,
            owner,
            garrison,
            force_ratio,
            is_frontier: is_frontier(state, me, node),
            distance_to_enemy: dist_to_enemy[node],
            incoming_hostile,
            incoming_friendly,
            attack_force_ratio,
            // Filled by the neighbor pass in `compute_all`; for a standalone single
            // node we report the safe sentinel (no neighbor info available cheaply).
            weakest_neighbor_force_ratio: F_RATIO_CAP,
        }
    }

    /// Compute [`NodeFeatures`] for **every** node from `me`'s perspective in one
    /// pass, sharing the single distance-to-enemy BFS. This is the path the DSL
    /// evaluator uses each tick: O(V + E) for the BFS plus O(V + fleets) for the
    /// per-node terms.
    ///
    /// A short second pass fills [`NodeFeatures::weakest_neighbor_force_ratio`], which
    /// is the only field that depends on *other* nodes' computed features.
    pub fn compute_all(state: &GameState, me: Owner, params: &Params) -> Vec<NodeFeatures> {
        let dist = distance_to_enemy_field(state, me);
        // The same look-ahead the Defend archetype uses: enough to react during the
        // attacker's undock window plus a couple of hops of travel. Centralized here
        // so HUD and AI agree on "incoming in N ticks".
        let look = default_look_ahead(params);
        let mut feats: Vec<NodeFeatures> = (0..state.nodes.len())
            .map(|n| NodeFeatures::compute(state, me, n, &dist, look, params))
            .collect();

        // Second pass: weakest *friendly* neighbor's force ratio. Read from the
        // already-computed `feats` so the credited defender advantage etc. is
        // consistent with the per-node force ratios.
        for n in 0..state.nodes.len() {
            let mut weakest = F_RATIO_CAP;
            for &(_, nbr) in state.neighbors(n) {
                if state.nodes[nbr].owner == me {
                    weakest = weakest.min(feats[nbr].force_ratio);
                }
            }
            feats[n].weakest_neighbor_force_ratio = weakest;
        }
        feats
    }
}

/// The default predictive horizon for "incoming hostile in N ticks", in ticks.
///
/// Chosen as `undock_delay + a few hops` so a defender reacts within the window the
/// movement model opens (the undock delay is what makes the threat readable in time;
/// see `01-mechanics.md` "Movement"). Centralized so the HUD and every policy use the
/// same N — otherwise "incoming" would mean different things to player and AI, which
/// would break the shared-vocabulary guarantee.
#[inline]
pub fn default_look_ahead(params: &Params) -> f64 {
    params.undock_delay + 14.0
}

// ===========================================================================
// Primitive legible quantities (free functions).
//
// These are the atoms the structs above are built from. They are public because
// the DSL's selectors/conditions and (historically) the hardcoded policies read
// them directly, and because they read well one at a time on the HUD.
// ===========================================================================

/// Signed share in [-1, 1]: `(me - op) / (me + op)`, with both-zero defined as 0
/// (perfectly even). The canonical "who is ahead, and by how much" shape used by
/// both territory and unit share. Mirrors the engine's internal `share` so the HUD
/// and the match score speak the same units.
#[inline]
pub fn signed_share(me: f64, op: f64) -> f64 {
    let total = me + op;
    if total <= 0.0 {
        0.0
    } else {
        (me - op) / total
    }
}

/// Direct edge travel time `from -> to` (the edge length), or `+inf` if the two
/// nodes are not directly connected. The legible "how long to send ships there".
#[inline]
pub fn direct_time(state: &GameState, from: NodeId, to: NodeId) -> f64 {
    match state.edge_between(from, to) {
        Some(eid) => state.edges[eid].length,
        None => f64::INFINITY,
    }
}

/// Ticks until a fleet reaches its destination: remaining undock time plus remaining
/// transit. The atom behind every "incoming in N ticks" read.
#[inline]
pub fn fleet_eta(state: &GameState, f: &Fleet) -> f64 {
    let len = state.edges[f.edge].length;
    f.undock_remaining + (len - f.progress).max(0.0)
}

/// **Incoming hostile fleet in N ticks** (the predictive feature) — total count of
/// enemy ships, from `me`'s perspective, inbound to `node` with ETA `<= within`.
pub fn incoming_hostile(state: &GameState, me: Owner, node: NodeId, within: f64) -> f64 {
    let mut sum = 0.0;
    for f in &state.fleets {
        if f.owner == me || f.to != node {
            continue;
        }
        if fleet_eta(state, f) <= within {
            sum += f.count;
        }
    }
    sum
}

/// Friendly ships already inbound to `node` (any ETA). Used to avoid double-sending
/// and to credit pending reinforcements in the force ratio.
pub fn incoming_friendly(state: &GameState, me: Owner, node: NodeId) -> f64 {
    state
        .fleets
        .iter()
        .filter(|f| f.owner == me && f.to == node)
        .map(|f| f.count)
        .sum()
}

/// **Is-frontier-node**: true iff `node` touches any node not owned by `me`.
pub fn is_frontier(state: &GameState, me: Owner, node: NodeId) -> bool {
    state
        .neighbors(node)
        .iter()
        .any(|&(_, n)| state.nodes[n].owner != me)
}

/// Ids of nodes owned by `me`, in ascending node order (deterministic).
pub fn my_nodes(state: &GameState, me: Owner) -> Vec<NodeId> {
    (0..state.nodes.len())
        .filter(|&i| state.nodes[i].owner == me)
        .collect()
}

/// **Distance-to-nearest-enemy** field: hop distance from *every* node to the
/// nearest enemy-owned node (multi-source BFS seeded at all enemy nodes). `+inf`
/// where the enemy is unreachable. Computed once and indexed per node.
pub fn distance_to_enemy_field(state: &GameState, me: Owner) -> Vec<f64> {
    let enemy = me.opponent();
    multi_source_bfs(state, |i| state.nodes[i].owner == enemy)
}

/// Distance field to the nearest **neutral** node (used by colonize-style routing so
/// ships flow toward unclaimed economy). `+inf` where no neutral is reachable.
pub fn distance_to_neutral_field(state: &GameState) -> Vec<f64> {
    multi_source_bfs(state, |i| state.nodes[i].owner == Owner::Neutral)
}

/// Single-source hop-distance BFS from `source` to all nodes.
pub fn distance_from(state: &GameState, source: NodeId) -> Vec<f64> {
    let mut dist = vec![f64::INFINITY; state.nodes.len()];
    dist[source] = 0.0;
    let mut q = VecDeque::new();
    q.push_back(source);
    while let Some(u) = q.pop_front() {
        for &(_, v) in state.neighbors(u) {
            if dist[v] > dist[u] + 1.0 {
                dist[v] = dist[u] + 1.0;
                q.push_back(v);
            }
        }
    }
    dist
}

/// Multi-source hop-distance BFS: every node for which `seed(i)` is true starts at
/// distance 0. The shared core of the distance-to-enemy / distance-to-neutral fields.
fn multi_source_bfs(state: &GameState, seed: impl Fn(NodeId) -> bool) -> Vec<f64> {
    let n = state.nodes.len();
    let mut dist = vec![f64::INFINITY; n];
    let mut q = VecDeque::new();
    for (i, d) in dist.iter_mut().enumerate() {
        if seed(i) {
            *d = 0.0;
            q.push_back(i);
        }
    }
    while let Some(u) = q.pop_front() {
        for &(_, v) in state.neighbors(u) {
            if dist[v] > dist[u] + 1.0 {
                dist[v] = dist[u] + 1.0;
                q.push_back(v);
            }
        }
    }
    dist
}

/// Map a desired ship count to the smallest [`FractionBucket`] of `avail` that
/// covers it, so a policy/HUD action does not over-commit. `All` if even that is
/// short. The bridge between continuous "I need ~N ships" reasoning and the discrete
/// 25/50/75/100% action space the design fixes.
pub fn bucket_for(avail: f64, need: f64) -> FractionBucket {
    if avail <= 0.0 {
        return FractionBucket::All;
    }
    let frac = (need / avail).clamp(0.0, 1.0);
    if frac <= 0.25 {
        FractionBucket::Quarter
    } else if frac <= 0.50 {
        FractionBucket::Half
    } else if frac <= 0.75 {
        FractionBucket::ThreeQuarter
    } else {
        FractionBucket::All
    }
}

#[cfg(test)]
mod tests {
    //! Tests for the D3 feature computations. These live inside the crate (rather
    //! than in `tests/`) so they compile into the library's own test runner.
    use super::*;
    use crate::engine::GameState;

    /// Force ratio, is-frontier, distance-to-enemy, incoming-hostile, and the
    /// attack force ratio on a hand-built board with a known answer.
    #[test]
    fn node_features_basic() {
        let mut params = Params::default();
        params.k = 4.0; // sqrt(k) = 2, makes the force-ratio arithmetic clean
        params.undock_delay = 0.0;

        // A(0) -- A(1) -- B(2), plus a neutral 3 hanging off node 1.
        //   node 0: interior (only friendly neighbor 1)
        //   node 1: frontier (touches enemy 2 and neutral 3)
        let nodes = vec![
            Node { owner: Owner::A, garrison: 10.0, production_mult: 1.0 },
            Node { owner: Owner::A, garrison: 8.0, production_mult: 1.0 },
            Node { owner: Owner::B, garrison: 5.0, production_mult: 1.0 },
            Node { owner: Owner::Neutral, garrison: 0.0, production_mult: 1.0 },
        ];
        let edges = vec![
            Edge { a: 0, b: 1, length: 1.0 },
            Edge { a: 1, b: 2, length: 1.0 },
            Edge { a: 1, b: 3, length: 1.0 },
        ];
        let state = GameState::new(nodes, edges);
        let feats = NodeFeatures::compute_all(&state, Owner::A, &params);

        assert!(!feats[0].is_frontier, "node 0 is interior");
        assert!(feats[1].is_frontier, "node 1 touches enemy/neutral");

        // distance-to-enemy (hops): enemy node 2 = 0, node 1 = 1, node 0 = 2.
        assert_eq!(feats[2].distance_to_enemy, 0.0);
        assert_eq!(feats[1].distance_to_enemy, 1.0);
        assert_eq!(feats[0].distance_to_enemy, 2.0);

        // No fleets → no incoming hostile → force ratio is the safe sentinel.
        assert_eq!(feats[1].incoming_hostile, 0.0);
        assert_eq!(feats[1].force_ratio, F_RATIO_CAP);

        // attack force ratio at node 1: garrison 8 * (1-l) / weakest enemy def 5.
        let expected = 8.0 * (1.0 - params.l) / 5.0;
        assert!((feats[1].attack_force_ratio - expected).abs() < 1e-9);
        // Node 0 has no adjacent enemy → attack force ratio 0.
        assert_eq!(feats[0].attack_force_ratio, 0.0);
    }

    /// The predictive "incoming hostile in N ticks" feature and the resulting force
    /// ratio / weakest-neighbor ratio react to an in-flight enemy fleet.
    #[test]
    fn force_ratio_reacts_to_incoming() {
        let mut params = Params::default();
        params.k = 4.0; // sqrt(k) = 2
        params.undock_delay = 0.0;
        params.r = 0.0;

        // B(0) attacks A(1); A(1) has a friendly neighbor A(2).
        let nodes = vec![
            Node { owner: Owner::B, garrison: 40.0, production_mult: 1.0 },
            Node { owner: Owner::A, garrison: 10.0, production_mult: 1.0 },
            Node { owner: Owner::A, garrison: 30.0, production_mult: 1.0 },
        ];
        let edges = vec![
            Edge { a: 0, b: 1, length: 5.0 },
            Edge { a: 1, b: 2, length: 1.0 },
        ];
        let mut state = GameState::new(nodes, edges);
        state.issue(Owner::B, Command { source: 0, target: 1, fraction: FractionBucket::All });

        let feats = NodeFeatures::compute_all(&state, Owner::A, &params);
        // Node 1 sees 40 incoming hostile within the look-ahead.
        assert!((feats[1].incoming_hostile - 40.0).abs() < 1e-9);
        // Force ratio at node 1: eff defense 10*sqrt(4)=20 over 40 = 0.5 (< 1).
        assert!((feats[1].force_ratio - 0.5).abs() < 1e-9);
        // Node 2's weakest friendly neighbor (node 1) is at 0.5 — the reinforce trigger.
        assert!((feats[2].weakest_neighbor_force_ratio - 0.5).abs() < 1e-9);
    }

    /// Owner-level features: production rate scales with r and node count; territory
    /// and unit shares are signed and in [-1, 1].
    #[test]
    fn owner_features_basic() {
        let mut params = Params::default();
        params.r = 0.5;
        let nodes = vec![
            Node { owner: Owner::A, garrison: 30.0, production_mult: 1.0 },
            Node { owner: Owner::A, garrison: 0.0, production_mult: 2.0 },
            Node { owner: Owner::B, garrison: 10.0, production_mult: 1.0 },
        ];
        let edges = vec![Edge { a: 0, b: 2, length: 1.0 }, Edge { a: 1, b: 0, length: 1.0 }];
        let state = GameState::new(nodes, edges);
        let of = OwnerFeatures::compute(&state, Owner::A, &params);

        // production rate = r*(1) + r*(2) = 0.5*3 = 1.5
        assert!((of.production_rate - 1.5).abs() < 1e-9);
        assert_eq!(of.territory_count, 2);
        // territory share = (2 - 1)/(2 + 1) = 1/3
        assert!((of.territory_share - (1.0 / 3.0)).abs() < 1e-9);
        // unit share = (30 - 10)/(30 + 10) = 0.5
        assert!((of.total_units - 30.0).abs() < 1e-9);
        assert!((of.unit_share - 0.5).abs() < 1e-9);
    }

    /// Distance fields: multi-source BFS to enemy and to neutral give the right hops.
    #[test]
    fn distance_fields() {
        // A(0) -- N(1) -- N(2) -- B(3)
        let nodes = vec![
            Node { owner: Owner::A, garrison: 5.0, production_mult: 1.0 },
            Node { owner: Owner::Neutral, garrison: 0.0, production_mult: 1.0 },
            Node { owner: Owner::Neutral, garrison: 0.0, production_mult: 1.0 },
            Node { owner: Owner::B, garrison: 5.0, production_mult: 1.0 },
        ];
        let edges = vec![
            Edge { a: 0, b: 1, length: 1.0 },
            Edge { a: 1, b: 2, length: 1.0 },
            Edge { a: 2, b: 3, length: 1.0 },
        ];
        let state = GameState::new(nodes, edges);
        let de = distance_to_enemy_field(&state, Owner::A);
        assert_eq!(de, vec![3.0, 2.0, 1.0, 0.0]);
        let dn = distance_to_neutral_field(&state);
        assert_eq!(dn, vec![1.0, 0.0, 0.0, 1.0]);
    }
}
