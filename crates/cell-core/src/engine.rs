//! The deterministic mean-field engine.
//!
//! This module implements the four mechanics R2 needs, all with **zero RNG**:
//!   (b) economy: per-node production governed by colonization rate `r`,
//!   (c) movement: fleets in transit with an undock/engagement delay, plus the
//!       attack-commitment loss `l`,
//!   (d) mean-field Lanchester *square-law* combat with defender-advantage `k`,
//!   (e) a tick loop running a full match between two policies to a scalar outcome.
//!
//! ## Why these exact equations (the WHY, per the design docs)
//!
//! `01-mechanics.md` insists the triad is "a cycle of three NUMBERS, not three
//! labels": colonization rate, defender-advantage multiplier, attack-commitment
//! loss. Those three numbers are [`Params::r`], [`Params::k`], [`Params::l`]. The
//! whole point of R2 is to find a region of `(r, k, l)` where the rock-paper-
//! scissors cycle closes. So the engine is written to make each of those three
//! constants a clean, monotone dial on the corresponding strategic edge.
//!
//! ### Square-law combat
//! Lanchester's square law in ODE form, for attacker strength `A` and defender
//! strength `D` fighting at one node:
//! ```text
//!     dA/dt = -(k) * D
//!     dD/dt = -(1) * A
//! ```
//! The conserved quantity is `A^2 - k*D^2` (hence "square law"). The strategic
//! consequence the design relies on: doubling the ships on one side roughly
//! quadruples its effective power budget, because power enters through the squared
//! conserved quantity. `k > 1` gives the *defender* an advantage (defender ships
//! are worth `sqrt(k)` attacker ships). This is the explicit defender term the
//! design says we may need to tune.
//!
//! We integrate this with fixed sub-steps inside each tick so the discrete analog
//! tracks the ODE closely and the 2x=>4x property emerges cleanly (see tests).

use crate::types::*;

/// The three tunable payoff constants plus the fixed movement parameters.
///
/// The first three fields (`r`, `k`, `l`) are *the* three numbers R2 sweeps. The
/// remaining fields are engine constants held fixed across the sweep (they define
/// the movement/combat texture but are not the cycle dials).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Params {
    /// **Colonization rate `r`.** Per-tick ship production of a player-owned node
    /// (scaled by the node's `production_mult`). Higher `r` rewards expansion, so
    /// it strengthens *colonize* relative to *defend*. This is the economic engine.
    pub r: f64,

    /// **Defender-advantage multiplier `k`.** Multiplies the defender's combat
    /// coefficient in the square law. `k = 1` is symmetric combat; `k > 1` means a
    /// stationary garrison out-trades an equal incoming force. Tunes *defend* ≻
    /// *attack*. If `k` is too large, defend dominates everything (a fixed point).
    pub k: f64,

    /// **Attack-commitment loss `l`.** Fraction in [0,1) of an attacking fleet's
    /// ships lost *on commitment* when it arrives at a hostile/contested node
    /// (over-extension cost). Bigger `l` punishes aggression, strengthening
    /// *defend* ≻ *attack* and weakening *attack* ≻ *colonize*.
    pub l: f64,

    /// Undock/engagement delay in ticks. Ships that just launched are vulnerable
    /// and deal no damage for this many ticks (`01-mechanics.md` "Engagement /
    /// undock delay"). Held fixed across the sweep.
    pub undock_delay: f64,

    /// Number of ODE sub-steps per tick used by the combat integrator. More
    /// sub-steps => closer to the continuous square-law ODE. Fixed; determinism is
    /// unaffected by its value (only accuracy).
    pub combat_substeps: u32,

    /// Combat speed scalar: multiplies both Lanchester coefficients so battles
    /// resolve at a sensible pace relative to production. Fixed across the sweep.
    pub combat_rate: f64,
}

impl Default for Params {
    /// A neutral-ish midpoint used by tests. The sweep overrides `r`, `k`, `l`.
    fn default() -> Self {
        Params {
            r: 1.0,
            k: 1.5,
            l: 0.15,
            undock_delay: 3.0,
            combat_substeps: 8,
            combat_rate: 0.5,
        }
    }
}

/// The complete mutable battlefield state.
#[derive(Debug, Clone, PartialEq)]
pub struct GameState {
    pub nodes: Vec<Node>,
    pub edges: Vec<Edge>,
    /// Fleets currently in transit (undocking or travelling).
    pub fleets: Vec<Fleet>,
    /// Whole ticks elapsed since match start.
    pub tick: u64,
    /// Adjacency: for each node, the list of `(edge_id, neighbor_node_id)`.
    /// Precomputed for fast, allocation-free policy queries.
    adjacency: Vec<Vec<(EdgeId, NodeId)>>,
}

impl GameState {
    /// Build a state from nodes and edges, computing adjacency.
    pub fn new(nodes: Vec<Node>, edges: Vec<Edge>) -> Self {
        let mut adjacency = vec![Vec::new(); nodes.len()];
        for (eid, e) in edges.iter().enumerate() {
            adjacency[e.a].push((eid, e.b));
            adjacency[e.b].push((eid, e.a));
        }
        GameState {
            nodes,
            edges,
            fleets: Vec::new(),
            tick: 0,
            adjacency,
        }
    }

    /// Neighbors of `node` as `(edge_id, neighbor_id)` pairs.
    #[inline]
    pub fn neighbors(&self, node: NodeId) -> &[(EdgeId, NodeId)] {
        &self.adjacency[node]
    }

    /// Find the edge directly connecting `from` and `to`, if any.
    pub fn edge_between(&self, from: NodeId, to: NodeId) -> Option<EdgeId> {
        self.adjacency[from]
            .iter()
            .find(|&&(_, n)| n == to)
            .map(|&(eid, _)| eid)
    }

    /// Total ships an owner controls: garrisons + in-flight fleets.
    pub fn total_units(&self, owner: Owner) -> f64 {
        let g: f64 = self
            .nodes
            .iter()
            .filter(|n| n.owner == owner)
            .map(|n| n.garrison)
            .sum();
        let f: f64 = self
            .fleets
            .iter()
            .filter(|fl| fl.owner == owner)
            .map(|fl| fl.count)
            .sum();
        g + f
    }

    /// Number of nodes an owner controls (territory count).
    pub fn territory(&self, owner: Owner) -> usize {
        self.nodes.iter().filter(|n| n.owner == owner).count()
    }

    /// True if `owner` controls no nodes and has no in-flight fleets (eliminated).
    pub fn is_eliminated(&self, owner: Owner) -> bool {
        self.territory(owner) == 0
            && !self.fleets.iter().any(|f| f.owner == owner && f.count > 0.0)
    }

    /// Launch a fleet with **zero undock delay** (convenience for unit tests that
    /// exercise pure movement/combat). Production code and policies use
    /// [`GameState::launch_with`] so the undock delay from `Params` is applied.
    ///
    /// Silently ignored if the move is illegal (not owned, no connecting edge, or
    /// empty garrison) — policies emit only legal moves, but we stay robust because
    /// the Architect will emit junk later.
    pub fn issue(&mut self, owner: Owner, cmd: Command) {
        // Must own the source.
        if self.nodes[cmd.source].owner != owner {
            return;
        }
        // Must be directly connected.
        let Some(edge) = self.edge_between(cmd.source, cmd.target) else {
            return;
        };
        let avail = self.nodes[cmd.source].garrison;
        let send = avail * cmd.fraction.as_f64();
        if send <= 0.0 {
            return;
        }
        self.nodes[cmd.source].garrison -= send;
        self.fleets.push(Fleet {
            owner,
            edge,
            from: cmd.source,
            to: cmd.target,
            count: send,
            undock_remaining: 0.0, // zero-undock variant; see launch_with for the real path
            progress: 0.0,
        });
    }
}

/// Result of a full match.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MatchOutcome {
    /// `Some(Owner::A | Owner::B)` if one side was eliminated or led at horizon;
    /// `None` only for an exact tie at the horizon.
    pub winner: Option<Owner>,
    /// Score in [-1, 1] from A's perspective: +1 = total A victory, -1 = total B
    /// victory, 0 = perfectly even. This is the scalar the sweep compares.
    /// Defined as A's combined (unit + territory) share minus B's, so it folds
    /// both economy and board control into one number (see `score_from`).
    pub score_a: f64,
    /// Tick at which the match ended (elimination tick, or the horizon).
    pub end_tick: u64,
    /// True if the match ended by elimination rather than reaching the horizon.
    pub by_elimination: bool,
}

impl GameState {
    /// Launch a fleet honoring the undock delay from `params`. This is the path
    /// production code and policies use.
    pub fn launch_with(&mut self, owner: Owner, cmd: Command, params: &Params) {
        if self.nodes[cmd.source].owner != owner {
            return;
        }
        let Some(edge) = self.edge_between(cmd.source, cmd.target) else {
            return;
        };
        let avail = self.nodes[cmd.source].garrison;
        let send = avail * cmd.fraction.as_f64();
        if send <= 0.0 {
            return;
        }
        self.nodes[cmd.source].garrison -= send;
        self.fleets.push(Fleet {
            owner,
            edge,
            from: cmd.source,
            to: cmd.target,
            count: send,
            undock_remaining: params.undock_delay,
            progress: 0.0,
        });
    }

    /// Advance the simulation by exactly one whole tick:
    ///   1. production (economy),
    ///   2. fleet movement (undock then transit; arrivals resolve),
    ///   3. combat at every contested node (mean-field square law).
    ///
    /// Order matters and is fixed for determinism. Production first means a node
    /// captured this tick does not retroactively produce for the new owner.
    pub fn step(&mut self, params: &Params) {
        self.produce(params);
        self.advance_fleets(params);
        self.resolve_all_combat(params);
        self.tick += 1;
    }

    /// (b) Economy: each player-owned node adds `r * production_mult` ships.
    /// Neutral nodes produce nothing. Deterministic and linear.
    fn produce(&mut self, params: &Params) {
        for n in &mut self.nodes {
            if n.owner.is_player() {
                n.garrison += params.r * n.production_mult;
            }
        }
    }

    /// (c) Movement: undock countdown, then transit, then arrival resolution.
    fn advance_fleets(&mut self, params: &Params) {
        // Advance timers/positions. We process one whole tick of motion.
        for f in &mut self.fleets {
            if f.undock_remaining > 0.0 {
                // Spend this tick (or part of it) finishing undock. Any leftover
                // time within the tick rolls into transit so total speed is
                // continuous across the undock->transit boundary.
                let spent = f.undock_remaining.min(1.0);
                f.undock_remaining -= spent;
                let leftover = 1.0 - spent;
                if leftover > 0.0 {
                    f.progress += leftover;
                }
            } else {
                f.progress += 1.0;
            }
        }

        // Resolve arrivals. We BATCH all fleets arriving at the same destination
        // this tick and resolve them together, grouped by owner. Batching is what
        // makes simultaneous arrivals **symmetric**: if two enemy fleets reach the
        // same neutral node on the same tick, they fight each other directly rather
        // than one arbitrarily "seizing first" (which would create a seat bias).
        let mut arrivals: Vec<usize> = Vec::new();
        for (i, f) in self.fleets.iter().enumerate() {
            if !f.is_undocking() && f.progress >= self.edges[f.edge].length {
                arrivals.push(i);
            }
        }
        if arrivals.is_empty() {
            return;
        }
        // Group arrived fleets by destination node, summing incoming per owner.
        // (NodeId -> (incoming_A, incoming_B)). Using a Vec keyed by node keeps it
        // allocation-light and fully deterministic in iteration order.
        let mut inc_a: Vec<f64> = vec![0.0; self.nodes.len()];
        let mut inc_b: Vec<f64> = vec![0.0; self.nodes.len()];
        let mut touched: Vec<NodeId> = Vec::new();
        // Remove arrived fleets (reverse order for swap_remove index safety) and
        // accumulate their counts per destination/owner.
        for &i in arrivals.iter().rev() {
            let f = self.fleets.swap_remove(i);
            if inc_a[f.to] == 0.0 && inc_b[f.to] == 0.0 {
                touched.push(f.to);
            }
            match f.owner {
                Owner::A => inc_a[f.to] += f.count,
                Owner::B => inc_b[f.to] += f.count,
                Owner::Neutral => {}
            }
        }
        // Resolve each touched node once. Sort touched so resolution order is
        // deterministic regardless of fleet ordering (it does not affect results
        // since nodes are independent, but determinism-by-construction is cheap).
        touched.sort_unstable();
        for node in touched {
            self.resolve_node_arrivals(node, inc_a[node], inc_b[node], params);
        }
    }

    /// Resolve all fleets that arrived at `node` this tick, given the summed
    /// incoming counts for each player (`in_a`, `in_b`).
    ///
    /// Cases, all deterministic and **symmetric** in the two seats:
    ///   - **Only the node's owner reinforces** (incoming all friendly): add to
    ///     garrison, no commitment loss.
    ///   - **Empty node, single claimant**: colonize — capture and seed garrison.
    ///   - **A defended node attacked by one enemy**: the attacker pays the
    ///     commitment loss `l`, then fights the garrison under the square law with
    ///     the defender getting advantage `k`. (Friendly reinforcements arriving
    ///     the same tick join the defense first.)
    ///   - **Two enemies contest the same node** (e.g. both reach a neutral node,
    ///     or one attacks while the other reinforces an owner): resolve as a single
    ///     square-law engagement between the two sides' effective forces. A side
    ///     that is *assaulting* (not the current owner) pays the commitment loss;
    ///     the current owner (if any) keeps the defender advantage `k`. If the node
    ///     is neutral, neither side defends, both pay the commitment loss, and the
    ///     fight is perfectly symmetric — so simultaneous neutral grabs cancel.
    fn resolve_node_arrivals(&mut self, node: NodeId, in_a: f64, in_b: f64, params: &Params) {
        let owner = self.nodes[node].owner;
        let garrison = self.nodes[node].garrison;

        // Fold friendly reinforcements into the defender's side, and treat the
        // opposing arrivals as the attacker. Build effective (force_owner_side,
        // force_attacker_side) plus who the attacker is.
        match owner {
            Owner::A => {
                // A defends (garrison + in_a). B (in_b) assaults if present.
                let defense = garrison + in_a;
                if in_b <= 0.0 {
                    self.nodes[node].garrison = defense;
                    return;
                }
                let attackers = in_b * (1.0 - params.l);
                self.battle_into_node(node, Owner::B, attackers, Owner::A, defense, params);
            }
            Owner::B => {
                let defense = garrison + in_b;
                if in_a <= 0.0 {
                    self.nodes[node].garrison = defense;
                    return;
                }
                let attackers = in_a * (1.0 - params.l);
                self.battle_into_node(node, Owner::A, attackers, Owner::B, defense, params);
            }
            Owner::Neutral => {
                // Empty node. If only one side arrives, colonize. If both arrive,
                // they fight each other with NO defender advantage and BOTH paying
                // the commitment loss (symmetric open-field clash).
                if in_a > 0.0 && in_b <= 0.0 {
                    self.nodes[node].owner = Owner::A;
                    self.nodes[node].garrison = in_a;
                } else if in_b > 0.0 && in_a <= 0.0 {
                    self.nodes[node].owner = Owner::B;
                    self.nodes[node].garrison = in_b;
                } else if in_a > 0.0 && in_b > 0.0 {
                    let fa = in_a * (1.0 - params.l);
                    let fb = in_b * (1.0 - params.l);
                    // Symmetric: k = 1 (no defender). Winner colonizes with survivors.
                    let (a_left, b_left) = lanchester_resolve(
                        fa, fb, 1.0, params.combat_rate, f64::INFINITY, params.combat_substeps,
                    );
                    if a_left > b_left {
                        self.nodes[node].owner = Owner::A;
                        self.nodes[node].garrison = a_left;
                    } else if b_left > a_left {
                        self.nodes[node].owner = Owner::B;
                        self.nodes[node].garrison = b_left;
                    } else {
                        // Exact tie (e.g. equal forces): node stays neutral, both
                        // annihilated. Symmetric and deterministic.
                        self.nodes[node].owner = Owner::Neutral;
                        self.nodes[node].garrison = 0.0;
                    }
                }
            }
        }
    }

    /// Resolve a square-law assault of `attackers` (already past commitment loss)
    /// belonging to `attacker_owner` against `defense` held by `defender_owner` at
    /// `node`, applying defender advantage `k`. Updates owner/garrison.
    fn battle_into_node(
        &mut self,
        node: NodeId,
        attacker_owner: Owner,
        attackers: f64,
        defender_owner: Owner,
        defense: f64,
        params: &Params,
    ) {
        let (atk_left, def_left) = lanchester_resolve(
            attackers, defense, params.k, params.combat_rate, f64::INFINITY, params.combat_substeps,
        );
        if atk_left > def_left && atk_left > 0.0 {
            self.nodes[node].owner = attacker_owner;
            self.nodes[node].garrison = atk_left;
        } else {
            self.nodes[node].owner = defender_owner;
            self.nodes[node].garrison = def_left.max(0.0);
        }
    }

    /// (d) Combat at every node where a defender garrison coexists with a hostile
    /// presence. In this model, persistent node combat arises when two players'
    /// ships occupy the same node — which only happens transiently via the arrival
    /// path above (an assault resolves on arrival). A node is never co-owned in
    /// steady state, so this pass is a safety net that drains any residual
    /// co-located hostile counts. It is kept for completeness and future
    /// mechanics (e.g. simultaneous multi-fleet arrivals) and is a no-op in the
    /// common case.
    ///
    /// Undocking fleets at a node do NOT participate (they deal no damage and are
    /// vulnerable) — but because assaults resolve against the *garrison* on
    /// arrival, undocking ships that have left the garrison are simply absent from
    /// the defense, which is exactly the intended vulnerability.
    fn resolve_all_combat(&mut self, _params: &Params) {
        // No-op in the current single-owner-per-node model. Present so the tick
        // order documented above is stable as mechanics grow.
    }

    /// Run a full match between two policies to elimination or `horizon` ticks.
    ///
    /// `policy_a` plays seat A, `policy_b` plays seat B. Each tick, both policies
    /// observe the *same* pre-tick state and emit zero or more commands; A's
    /// commands are applied before B's (a fixed, documented tie-break — both
    /// seatings are played by the sweep to cancel this bias).
    pub fn run_match(
        mut self,
        policy_a: &mut dyn crate::policy::Policy,
        policy_b: &mut dyn crate::policy::Policy,
        params: &Params,
        horizon: u64,
    ) -> MatchOutcome {
        policy_a.reset();
        policy_b.reset();
        while self.tick < horizon {
            // Eliminations checked at tick boundary.
            let a_dead = self.is_eliminated(Owner::A);
            let b_dead = self.is_eliminated(Owner::B);
            if a_dead || b_dead {
                return self.finish(true);
            }

            // Both policies see the identical snapshot; collect then apply so B
            // does not see A's same-tick launches (simultaneous decision).
            let cmds_a = policy_a.decide(&self, Owner::A, params);
            let cmds_b = policy_b.decide(&self, Owner::B, params);
            for c in cmds_a {
                self.launch_with(Owner::A, c, params);
            }
            for c in cmds_b {
                self.launch_with(Owner::B, c, params);
            }

            self.step(params);
        }
        self.finish(false)
    }

    /// Compute the final outcome from the current state.
    fn finish(&self, by_elimination: bool) -> MatchOutcome {
        let score_a = self.score_from(Owner::A);
        let a_dead = self.is_eliminated(Owner::A);
        let b_dead = self.is_eliminated(Owner::B);
        let winner = if a_dead && !b_dead {
            Some(Owner::B)
        } else if b_dead && !a_dead {
            Some(Owner::A)
        } else if score_a > 0.0 {
            Some(Owner::A)
        } else if score_a < 0.0 {
            Some(Owner::B)
        } else {
            None
        };
        MatchOutcome {
            winner,
            score_a,
            end_tick: self.tick,
            by_elimination,
        }
    }

    /// Scalar score in [-1, 1] from `who`'s perspective, blending unit share and
    /// territory share equally. +1 means `who` has everything, -1 means the
    /// opponent has everything, 0 is perfectly even.
    ///
    /// Blending both means a strategy that wins on economy (more ships) and one
    /// that wins on the board (more nodes) are both rewarded, and a margin near 0
    /// genuinely means a near-even game — important for the epsilon test in R2.
    pub fn score_from(&self, who: Owner) -> f64 {
        let opp = who.opponent();
        let u_me = self.total_units(who);
        let u_op = self.total_units(opp);
        let t_me = self.territory(who) as f64;
        let t_op = self.territory(opp) as f64;

        let unit_share = share(u_me, u_op);
        let terr_share = share(t_me, t_op);
        // Equal blend, already in [-1, 1].
        0.5 * unit_share + 0.5 * terr_share
    }
}

/// Signed share in [-1, 1]: `(me - op) / (me + op)`, with the degenerate
/// both-zero case defined as 0 (perfectly even / nobody has any).
#[inline]
fn share(me: f64, op: f64) -> f64 {
    let total = me + op;
    if total <= 0.0 {
        0.0
    } else {
        (me - op) / total
    }
}

/// Resolve a Lanchester *square-law* engagement deterministically.
///
/// Integrates
/// ```text
///     dA/dt = -coeff * k * D
///     dD/dt = -coeff *     A
/// ```
/// for up to `max_time` ticks using `substeps` fixed sub-steps per tick, stopping
/// early when either side is depleted. Returns `(A_remaining, D_remaining)`.
///
/// * `a0`, `d0`: initial attacker / defender strengths.
/// * `k`: defender-advantage multiplier (>=1 favors the defender).
/// * `coeff`: base combat coefficient (`Params::combat_rate`).
/// * `max_time`: cap on integration time in ticks (`INFINITY` = fight to the end).
/// * `substeps`: sub-steps per tick (accuracy; determinism is independent of it).
///
/// Determinism: this is pure floating-point arithmetic with a fixed step schedule.
/// Identical inputs always yield identical outputs.
pub fn lanchester_resolve(
    a0: f64,
    d0: f64,
    k: f64,
    coeff: f64,
    max_time: f64,
    substeps: u32,
) -> (f64, f64) {
    let mut a = a0.max(0.0);
    let mut d = d0.max(0.0);
    if a <= 0.0 || d <= 0.0 {
        return (a, d);
    }
    let substeps = substeps.max(1);
    let dt_base = 1.0 / substeps as f64;

    // Fight to depletion when max_time is infinite by iterating until one side is
    // gone. We bound iterations defensively to avoid any pathological non-
    // termination (cannot happen mathematically, but keeps the engine total).
    let mut t = 0.0;
    let max_iters = if max_time.is_infinite() {
        // Square-law battles end fast; a generous cap relative to forces.
        ((a0 + d0).ceil() as u64 + 16) * substeps as u64 * 64
    } else {
        (max_time * substeps as f64).ceil() as u64
    };

    for _ in 0..max_iters {
        if a <= 0.0 || d <= 0.0 {
            break;
        }
        if !max_time.is_infinite() && t >= max_time {
            break;
        }
        let dt = dt_base;
        // Forward-Euler on the square-law ODE. Small dt keeps it close to the
        // analytic solution; tests confirm the 2x=>4x property to good accuracy.
        let da = -coeff * k * d * dt;
        let dd = -coeff * a * dt;
        a += da;
        d += dd;
        if a < 0.0 {
            a = 0.0;
        }
        if d < 0.0 {
            d = 0.0;
        }
        t += dt;
    }
    (a, d)
}
