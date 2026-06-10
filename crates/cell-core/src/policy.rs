//! The three pure strategies as hardcoded policies.
//!
//! `01-mechanics.md` is emphatic that the triad must be **"a cycle of three
//! NUMBERS, not three labels."** For the R2 sweep to mean anything, each policy
//! must be a *competent, sensible* embodiment of its archetype — a weak strawman
//! would make the cycle appear/disappear for the wrong reasons. So each policy
//! below plays its style well and is documented with the reasoning behind it.
//!
//! All policies are **pure functions of the observed state** (no internal RNG, no
//! hidden history). This keeps the whole engine deterministic, which is the
//! precondition for the empirical R2 gate.
//!
//! ## The common substrate and where the archetypes diverge
//!
//! Every archetype must build an economy by expanding into the neutral territory
//! between the two homes — otherwise "the cycle" would only measure *who bothered
//! to expand*, which is not the strategic question. So all three share a
//! rate-aware [`expand_step`] baseline. The archetypes then differ in **combat
//! posture**, which is exactly where the timing structure creates the cycle:
//!
//! * **Colonize** — maximize economy: grab the most nodes fastest, garrison
//!   nothing, never mass. Out-produces everyone but is thin everywhere.
//! * **Attack** — expand to contact, then **mass and strike** enemy production.
//!   Beats Colonize because the colonizer's forward nodes are thinly held and fall
//!   to a concentrated blow that then snowballs (square law). Loses to Defend
//!   because the defender's `k` advantage + reinforcement repels the committed
//!   strike and the aggressor bleeds (commitment loss `l`).
//! * **Defend** — expand modestly, then **hold**: garrison frontier nodes and
//!   reinforce threatened ones. Beats Attack (repels strikes); loses to Colonize
//!   (out-expanded, pays the opportunity cost of ships sitting idle).

use crate::engine::{GameState, Params};
use crate::types::*;

/// A strategy: given the current state, the seat it plays, and the engine params,
/// return the commands to issue this tick.
///
/// `reset` is called once at the start of each match so a policy can clear any
/// internal state and remain reusable across the sweep's many matches.
pub trait Policy {
    /// Decide this tick's commands. Must be deterministic in its inputs.
    fn decide(&mut self, state: &GameState, me: Owner, params: &Params) -> Vec<Command>;

    /// Reset any per-match internal state. Default: no-op.
    fn reset(&mut self) {}

    /// A short human-readable name (for reports/logs).
    fn name(&self) -> &'static str;
}

// ============================================================================
// Shared derived quantities (the same legible features the HUD / AI would use)
// ============================================================================

/// Direct edge travel time `from -> to` (edge length), or +inf if not adjacent.
fn direct_time(state: &GameState, from: NodeId, to: NodeId) -> f64 {
    match state.edge_between(from, to) {
        Some(eid) => state.edges[eid].length,
        None => f64::INFINITY,
    }
}

/// Total hostile fleet count inbound to `node` arriving within `within` ticks,
/// from `me`'s perspective. The design's predictive "incoming in N ticks" feature.
fn incoming_hostile(state: &GameState, me: Owner, node: NodeId, within: f64) -> f64 {
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

/// Friendly count already inbound to `node` (so policies don't double-send).
fn incoming_friendly(state: &GameState, me: Owner, node: NodeId) -> f64 {
    state
        .fleets
        .iter()
        .filter(|f| f.owner == me && f.to == node)
        .map(|f| f.count)
        .sum()
}

/// Ticks until a fleet reaches its destination (undock remaining + transit left).
fn fleet_eta(state: &GameState, f: &Fleet) -> f64 {
    let len = state.edges[f.edge].length;
    f.undock_remaining + (len - f.progress).max(0.0)
}

/// Ids of nodes owned by `me`.
fn my_nodes(state: &GameState, me: Owner) -> Vec<NodeId> {
    (0..state.nodes.len())
        .filter(|&i| state.nodes[i].owner == me)
        .collect()
}

/// A node is "frontier" for `me` if it touches any non-`me` node.
fn is_frontier(state: &GameState, me: Owner, node: NodeId) -> bool {
    state.neighbors(node).iter().any(|&(_, n)| state.nodes[n].owner != me)
}

/// Hop-distance from a single source node to all others (BFS over edges).
fn bfs_from(state: &GameState, source: NodeId) -> Vec<f64> {
    let n = state.nodes.len();
    let mut dist = vec![f64::INFINITY; n];
    dist[source] = 0.0;
    let mut q = std::collections::VecDeque::new();
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

/// Hop-distance from every node to the nearest enemy-owned node.
fn bfs_to_enemy(state: &GameState, me: Owner) -> Vec<f64> {
    let enemy = me.opponent();
    let n = state.nodes.len();
    let mut dist = vec![f64::INFINITY; n];
    let mut q = std::collections::VecDeque::new();
    for i in 0..n {
        if state.nodes[i].owner == enemy {
            dist[i] = 0.0;
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

/// Map a desired ship count to the smallest fraction bucket of `avail` that covers
/// it (so we don't over-commit). `All` if even that is short.
fn bucket_for(avail: f64, need: f64) -> FractionBucket {
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

/// Shared rate-aware expansion: for each owned node with surplus above `keep`,
/// colonize its best adjacent **empty** node. "Best" is rate-aware (not naive
/// nearest-first): value = `production_mult / (travel + 1)` so a cheap near node
/// that starts compounding now is preferred, but a rich far node still wins when
/// its richness justifies the trip (the `01` scheduling problem). Returns commands
/// and does not send if a friendly fleet is already inbound to that target.
///
/// `keep` is the garrison floor a node retains (archetype-specific: colonizers keep
/// almost nothing; defenders keep more). `frac` is the bucket to send.
fn expand_step(
    state: &GameState,
    me: Owner,
    keep: f64,
    frac: FractionBucket,
    cmds: &mut Vec<Command>,
) {
    for src in my_nodes(state, me) {
        let g = state.nodes[src].garrison;
        if g <= keep + 0.5 {
            continue;
        }
        let mut best: Option<(NodeId, f64)> = None;
        for &(_, nbr) in state.neighbors(src) {
            if state.nodes[nbr].owner != Owner::Neutral {
                continue;
            }
            if incoming_friendly(state, me, nbr) > 0.0 {
                continue;
            }
            let value = state.nodes[nbr].production_mult / (direct_time(state, src, nbr) + 1.0);
            if best.map_or(true, |(_, bv)| value > bv) {
                best = Some((nbr, value));
            }
        }
        if let Some((tgt, _)) = best {
            cmds.push(Command { source: src, target: tgt, fraction: frac });
        }
    }
}

// ============================================================================
// Colonize
// ============================================================================

/// **Colonize**: rate-aware expansion, garrison nothing, never mass.
///
/// It expands with large fractions and a near-zero keep-floor so it claims the most
/// territory fastest, and it *flows ships forward* through already-owned nodes
/// toward unclaimed space so captured nodes keep funding further expansion rather
/// than piling up idle garrisons. This is the pure economic engine: it out-produces
/// a turtle (beating Defend) but leaves every node thinly held (losing to a timed
/// Attack).
pub struct Colonize;

impl Policy for Colonize {
    fn name(&self) -> &'static str {
        "Colonize"
    }

    fn decide(&mut self, state: &GameState, me: Owner, _params: &Params) -> Vec<Command> {
        let mut cmds = Vec::new();
        // 1) Claim adjacent neutral nodes aggressively (keep almost nothing).
        expand_step(state, me, 1.0, FractionBucket::ThreeQuarter, &mut cmds);

        // 2) Flow forward: any owned node with NO adjacent neutral (its local
        // frontier is exhausted) but that still has a path to unclaimed space pushes
        // ships one hop toward the nearest *neutral* node, so growth keeps reaching.
        // We deliberately route toward NEUTRAL space only (not enemy nodes): Colonize
        // is a pure economist and must never accidentally mass against the enemy —
        // doing so would make it defend, blurring the Colonize/Defend distinction.
        let dist_neutral = bfs_to_neutral(state, me);
        for src in my_nodes(state, me) {
            let g = state.nodes[src].garrison;
            if g < 4.0 {
                continue;
            }
            let has_adjacent_neutral = state
                .neighbors(src)
                .iter()
                .any(|&(_, n)| state.nodes[n].owner == Owner::Neutral);
            if has_adjacent_neutral {
                continue; // step 1 already handled it
            }
            // Push toward the neighbor closest to unclaimed space.
            let mut best: Option<(NodeId, f64)> = None;
            for &(_, nbr) in state.neighbors(src) {
                if state.nodes[nbr].owner != me {
                    continue;
                }
                let d = dist_neutral[nbr];
                if d.is_finite() && best.map_or(true, |(_, bd)| d < bd) {
                    best = Some((nbr, d));
                }
            }
            if let Some((tgt, d)) = best {
                if d < dist_neutral[src] {
                    cmds.push(Command { source: src, target: tgt, fraction: FractionBucket::Half });
                }
            }
        }
        cmds
    }
}

/// Hop-distance from every node to the nearest **neutral** node. Used by Colonize's
/// forward-flow so it routes ships only toward unclaimed economy, never toward the
/// enemy (Colonize must stay a pure economist).
fn bfs_to_neutral(state: &GameState, me: Owner) -> Vec<f64> {
    let _ = me; // signature parity with the other BFS helpers
    let n = state.nodes.len();
    let mut dist = vec![f64::INFINITY; n];
    let mut q = std::collections::VecDeque::new();
    for i in 0..n {
        if state.nodes[i].owner == Owner::Neutral {
            dist[i] = 0.0;
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

// ============================================================================
// Defend
// ============================================================================

/// **Defend**: build a modest economy, then hold it.
///
/// Defend expands more cautiously than Colonize (it keeps a larger garrison floor
/// and only claims *safe* neutral nodes near home), then switches to a defensive
/// posture: it reads the predictive "incoming hostile in N ticks" feature and
/// funnels reinforcements to threatened nodes ahead of the attacker, and otherwise
/// consolidates ships onto frontier nodes so its wall is manned and `k`-advantaged.
///
/// Because it holds ships rather than expanding maximally, it pays an opportunity
/// cost against a colonizer (losing that edge) — but a massed garrison plus the
/// defender advantage repels an aggressor (winning that edge).
pub struct Defend;

impl Policy for Defend {
    fn name(&self) -> &'static str {
        "Defend"
    }

    fn decide(&mut self, state: &GameState, me: Owner, params: &Params) -> Vec<Command> {
        let mut cmds = Vec::new();
        let mine = my_nodes(state, me);
        let look = 14.0 + params.undock_delay;

        // 1) Modest expansion: only claim neutral nodes from interior nodes that are
        // not currently threatened, keeping a solid home garrison. This gives Defend
        // an economy without stripping its defenses.
        for src in &mine {
            if incoming_hostile(state, me, *src, look) > 0.0 {
                continue; // don't expand from a node under threat
            }
            let g = state.nodes[*src].garrison;
            if g < 8.0 {
                continue; // keep a floor; only spend genuine surplus
            }
            let mut best: Option<(NodeId, f64)> = None;
            for &(_, nbr) in state.neighbors(*src) {
                if state.nodes[nbr].owner != Owner::Neutral {
                    continue;
                }
                if incoming_friendly(state, me, nbr) > 0.0 {
                    continue;
                }
                let value = state.nodes[nbr].production_mult / (direct_time(state, *src, nbr) + 1.0);
                if best.map_or(true, |(_, bv)| value > bv) {
                    best = Some((nbr, value));
                }
            }
            if let Some((tgt, _)) = best {
                // Spend only a quarter so the defender stays heavy at home.
                cmds.push(Command { source: *src, target: tgt, fraction: FractionBucket::Quarter });
            }
        }

        // 2) Emergency reinforcement of threatened owned nodes.
        for &node in &mine {
            let threat = incoming_hostile(state, me, node, look);
            if threat <= 0.0 {
                continue;
            }
            // Effective present defense ~ garrison * sqrt(k) + inbound friendlies.
            let have = state.nodes[node].garrison * params.k.sqrt() + incoming_friendly(state, me, node);
            if have >= threat {
                continue;
            }
            let deficit = threat - have;
            // Pull from neighbors with spare ships, interior first.
            let mut donors: Vec<NodeId> = state
                .neighbors(node)
                .iter()
                .map(|&(_, n)| n)
                .filter(|&n| state.nodes[n].owner == me && state.nodes[n].garrison > 1.0)
                .collect();
            donors.sort_by(|&x, &y| {
                let fx = is_frontier(state, me, x);
                let fy = is_frontier(state, me, y);
                fx.cmp(&fy).then(
                    state.nodes[y]
                        .garrison
                        .partial_cmp(&state.nodes[x].garrison)
                        .unwrap_or(std::cmp::Ordering::Equal),
                )
            });
            let mut need = deficit;
            for d in donors {
                if need <= 0.0 {
                    break;
                }
                let avail = state.nodes[d].garrison;
                let frac = bucket_for(avail, need);
                cmds.push(Command { source: d, target: node, fraction: frac });
                need -= avail * frac.as_f64();
            }
        }

        // 3) Consolidate when calm: push surplus from deep-interior nodes to the
        // weakest adjacent frontier node so the wall is manned.
        for &node in &mine {
            if is_frontier(state, me, node) {
                continue;
            }
            if incoming_hostile(state, me, node, look) > 0.0 {
                continue;
            }
            let g = state.nodes[node].garrison;
            if g < 10.0 {
                continue;
            }
            let mut target: Option<(NodeId, f64)> = None;
            for &(_, nbr) in state.neighbors(node) {
                if state.nodes[nbr].owner != me || !is_frontier(state, me, nbr) {
                    continue;
                }
                let def = state.nodes[nbr].garrison;
                if target.map_or(true, |(_, bd)| def < bd) {
                    target = Some((nbr, def));
                }
            }
            if let Some((tgt, _)) = target {
                cmds.push(Command { source: node, target: tgt, fraction: FractionBucket::Half });
            }
        }

        cmds
    }
}

// ============================================================================
// Attack
// ============================================================================

/// **Attack**: a **timing push** — build an army quickly, expand only enough to
/// fund it, and strike the enemy's weakest node as soon as the spearhead holds a
/// numeric **majority** over the *currently visible* defense (plus a buffer for the
/// commitment loss `l`).
///
/// The design's RPS cycle is a timing story, so the crucial choice is the strike
/// trigger. Attack does **not** wait for the `sqrt(k)`-sized surplus the square law
/// would demand for a *guaranteed* win — that surplus is unreachable against an
/// economy that grows as fast as its own, which would make Attack passive. Instead
/// it commits on a raw majority. The *engine* then applies the defender advantage
/// `k`, which decides the outcome:
///
/// * vs **Colonize** — the colonizer's forward nodes are nearly empty, so Attack
///   trivially has a majority, commits early, conquers, and snowballs (square law).
///   **Attack wins.**
/// * vs **Defend** — the defender massed its frontier and reinforces, so Attack
///   rarely gets a clean majority; when it does commit, `k` plus the reinforcement
///   landing during the undock delay turn the trade against it, and Attack has
///   under-invested in economy. **Defend wins.**
///
/// Concentration of force is ~a theorem under the square law, so Attack funnels its
/// whole army to one staging node and strikes with all of it (never dribbles).
pub struct Attack;

impl Attack {
    /// Pre-loss force at which Attack will commit against a *visible* defense `def`:
    /// a raw numeric majority plus a buffer covering the commitment loss `l` and a
    /// small constant so it does not commit into near-parity. Deliberately **not**
    /// scaled by `sqrt(k)` — committing on majority (and letting `k` decide) is what
    /// produces the timing cycle (see the type-level docs).
    fn commit_threshold(def: f64, l: f64) -> f64 {
        // Need post-loss strength a bit above the defender: A*(1-l) > def*margin.
        let margin = 1.10;
        def * margin / (1.0 - l).max(1e-3) + 3.0
    }
}

impl Policy for Attack {
    fn name(&self) -> &'static str {
        "Attack"
    }

    fn decide(&mut self, state: &GameState, me: Owner, params: &Params) -> Vec<Command> {
        let mut cmds = Vec::new();
        let enemy = me.opponent();
        let mine = my_nodes(state, me);
        if mine.is_empty() {
            return cmds;
        }
        let dist_enemy = bfs_to_enemy(state, me);

        // Best staging->target strike: an owned node adjacent to an enemy node,
        // targeting the weakest reachable enemy node (lowest visible defense).
        let mut best_strike: Option<(NodeId, NodeId, f64)> = None; // (stage, target, def)
        for &stage in &mine {
            for &(_, nbr) in state.neighbors(stage) {
                if state.nodes[nbr].owner != enemy {
                    continue;
                }
                let edge_len = direct_time(state, stage, nbr);
                // Visible defense = its garrison + reinforcements that clearly land
                // before our strike arrives (undock + transit). We do NOT try to
                // model every future reinforcement — Attack bets on tempo.
                let def = state.nodes[nbr].garrison
                    + incoming_hostile(state, enemy, nbr, params.undock_delay + edge_len);
                if best_strike.map_or(true, |(_, _, bd)| def < bd) {
                    best_strike = Some((stage, nbr, def));
                }
            }
        }

        // Expand modestly to fund the army (Attack under-invests in economy on
        // purpose — that is the cost it pays when its push fails vs Defend).
        expand_step(state, me, 3.0, FractionBucket::Half, &mut cmds);

        match best_strike {
            Some((stage, target, def)) => {
                let staged = state.nodes[stage].garrison;
                let threshold = Attack::commit_threshold(def, params.l);
                if staged >= threshold {
                    // Commit the whole staged force at the weakest enemy node.
                    cmds.push(Command { source: stage, target, fraction: FractionBucket::All });
                } else {
                    // Concentrate: funnel every other owned node's ships toward the
                    // staging node so the spearhead reaches the threshold fast.
                    let to_stage = bfs_from(state, stage);
                    for &src in &mine {
                        if src == stage || state.nodes[src].garrison < 2.0 {
                            continue;
                        }
                        push_one_hop_toward(state, me, src, &to_stage, FractionBucket::All, &mut cmds);
                    }
                }
            }
            None => {
                // Phase 1: no contact yet. Drive the spearhead toward the enemy,
                // colonizing intermediate neutrals on the way.
                for &src in &mine {
                    if state.nodes[src].garrison < 2.0 {
                        continue;
                    }
                    let has_adj_neutral = state
                        .neighbors(src)
                        .iter()
                        .any(|&(_, n)| state.nodes[n].owner == Owner::Neutral);
                    if has_adj_neutral {
                        continue; // expand_step grabs it; cheaper than marching
                    }
                    push_one_hop_toward(state, me, src, &dist_enemy, FractionBucket::All, &mut cmds);
                }
            }
        }

        cmds
    }
}

/// Push all (a bucket of) `src`'s ships one hop toward the goal described by the
/// distance field `dist` (smaller = closer to goal). Prefers stepping onto an owned
/// or neutral neighbor that strictly decreases distance. No-op if none qualifies.
fn push_one_hop_toward(
    state: &GameState,
    me: Owner,
    src: NodeId,
    dist: &[f64],
    frac: FractionBucket,
    cmds: &mut Vec<Command>,
) {
    let here = dist[src];
    let mut best: Option<(NodeId, f64)> = None;
    for &(_, nbr) in state.neighbors(src) {
        let d = dist[nbr];
        if !d.is_finite() || d >= here {
            continue; // must move closer
        }
        // Prefer owned (free move) then neutral (colonize on arrival). Never route a
        // plain hop *into* an enemy node here (that is the strike logic's job, kept
        // separate); skip enemy neighbors entirely.
        let owner = state.nodes[nbr].owner;
        let penalty = match owner {
            o if o == me => 0.0,
            Owner::Neutral => 0.25,
            _ => continue, // enemy node: not a routing hop
        };
        let key = d + penalty;
        if best.map_or(true, |(_, bk)| key < bk) {
            best = Some((nbr, key));
        }
    }
    if let Some((nbr, _)) = best {
        cmds.push(Command { source: src, target: nbr, fraction: frac });
    }
}
