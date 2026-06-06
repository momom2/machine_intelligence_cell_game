//! # `ai::bestresponse` — the projection-scored best-response optimizer
//!
//! The principled core the three strategic automata share. Each is the **same** optimizer:
//! enumerate a handful of candidate force-allocations, score each by the projected production
//! OBJECTIVE ([`world::Projection::expected_production`]), and issue the best one. The three pure
//! identities are **not** different programs — they are this one optimizer under a different
//! `(objective × enemy-belief)`:
//!
//! | strategy | objective | enemy belief (the distortion) |
//! |---|---|---|
//! | **Colonize** | maximise **OWN** future production | optimistic (plain projection) |
//! | **Attack**   | minimise **ENEMY** future production | optimistic (plain projection) |
//! | **Defend**   | maximise **(own − enemy)** | pessimistic (a competent adversary answers) |
//!
//! We *specify* only the objective and the distortion. Whether the RPS cycle and the documented
//! blind spots actually **emerge** is something the harness **measures** — never asserted here.
//!
//! ## Contract
//! Nothing here names a raw mechanic: the value is a [`world::Projection`] query, the moves go
//! through the world's own [`World::issue_fleet_order`] primitive, and "what the future looks like"
//! is the projection oracle. The optimizer clones the world, applies a candidate's orders (and, for
//! the pessimist, the adversary's answer), projects, and reads the objective — a pure what-if.

use std::collections::HashSet;

use layer1::{Faction, FractionBucket, SimParams};
use world::{FleetOrder, PlanetId, PlanetOwner, World, WorldParams};

/// What an agent VALUES — a partition of the production-delta value-to-go. The identity's objective.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Objective {
    /// Maximise own future production (Colonize): grow, and find neutral ground cheapest.
    MaxOwn,
    /// Minimise enemy future production (Attack): hurt the opponent's economy, even at worse
    /// ship-efficiency than grabbing neutral ground.
    MinEnemy,
    /// Maximise (own − enemy) future production (Defend): don't fall behind.
    Delta,
    /// Maximise `own − DENY_BIAS·enemy` (Attack): still values its own production (so it doesn't
    /// suicide like pure `MinEnemy`) but weights *hurting the enemy* above growing — the explicit
    /// bias that makes it strike instead of converging on Colonize's rational opener.
    DenyLeaning,
}

/// The enemy BELIEF used when scoring a plan — the controlled distortion that (we hope) gives an
/// identity its blind spot. `Optimistic` scores against the plain, **non-reacting** projection (the
/// proactive strategies' blind spot is exactly that they don't anticipate a reaction). `Pessimistic`
/// assumes a **competent adversary** issues its own best counter-orders before the future is
/// projected (the reactive strategy plans against the worst case).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum EnemyModel {
    /// Plain projection — the enemy does not react to my orders this tick.
    Optimistic,
    /// A competent adversary answers with its own production-minimising best response first.
    Pessimistic,
}

/// The identity's **structural commitment** — which *class* of target it will ever send force at.
/// This is the irreducible bit of identity that a pure optimizer cannot have (an optimizer converges
/// on one line); the projection-integral still decides *how much / where* WITHIN the lens, so there
/// are no hand-tuned priority numbers or thresholds — only "what kind of ground do I pursue".
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Lens {
    /// **Colonize**: grab *neutral* ground (and thicken/recover own). Never initiates on the enemy —
    /// its blind spot.
    Expand,
    /// **Attack**: drive force at *enemy* ground (and mass on own staging). Never spreads to colonise
    /// its own flank — it over-commits forward; that is its blind spot.
    Assault,
    /// **Defend**: hold *own* ground and take only *safe* neutral expansion. Never presses the enemy —
    /// its opportunity-cost blind spot.
    Fortify,
}

/// Decide `seat`'s inter-planet [`FleetOrder`]s this tick by scoring candidate allocations against
/// `obj` under the enemy belief `enemy`, projecting `horizon` ticks. Deterministic: candidate
/// generation and the argmax tie-break (keep the first maximum, HOLD first) are id-ordered, and the
/// projection draws no RNG.
pub fn decide(
    world: &World,
    seat: Faction,
    sp: &SimParams,
    wp: &WorldParams,
    lens: Lens,
    obj: Objective,
    enemy: EnemyModel,
    horizon: u64,
) -> Vec<FleetOrder> {
    let candidates = candidate_allocations(world, seat, wp, lens);
    let mut best_score = f64::NEG_INFINITY;
    let mut best: Vec<FleetOrder> = Vec::new();
    for cand in candidates {
        let s = score(world, seat, &cand, sp, wp, obj, enemy, horizon);
        if s > best_score {
            best_score = s;
            best = cand;
        }
    }
    best
}

/// Score one candidate allocation: clone the world, apply my `orders`, (for the pessimist) let a
/// competent adversary answer, project, and read the objective. Higher is better for all three
/// objectives (MinEnemy is negated so "more is better" holds uniformly).
fn score(
    world: &World,
    seat: Faction,
    orders: &[FleetOrder],
    sp: &SimParams,
    wp: &WorldParams,
    obj: Objective,
    enemy: EnemyModel,
    horizon: u64,
) -> f64 {
    let mut w = world.clone();
    for o in orders {
        w.issue_fleet_order(*o, seat, wp);
    }
    if enemy == EnemyModel::Pessimistic {
        inject_competent_adversary(&mut w, seat, wp);
    }
    let proj = w.project_forward(sp, wp, horizon);
    let own = proj.expected_production(seat);
    let enemy = proj.expected_production(seat.opponent());
    match obj {
        Objective::MaxOwn => own,
        Objective::MinEnemy => -enemy,
        Objective::Delta => own - enemy,
        // Denial-leaning: still values its own production (so it does not suicide) but weights
        // hurting the enemy above growing — the controlled bias that makes Attack *strike* rather
        // than converge on the same rational opener as Colonize. `DENY_BIAS` is its one legible dial.
        Objective::DenyLeaning => own - DENY_BIAS * enemy,
    }
}

/// How much more an Attack values *denying enemy* production than *growing its own* (the weight on
/// the enemy term in [`Objective::DenyLeaning`]). `> 1` makes it prefer striking enemy ground over
/// grabbing neutrals; this is the explicit, single suboptimality that gives Attack its identity.
const DENY_BIAS: f64 = 3.0;

/// Is planet `t` a legal target for `seat` under `lens` (its committed target class)? `Expand`
/// pursues foe-free neutral ground + own (grow/recover); `Assault` pursues enemy-bearing ground +
/// own (strike/stage); `Fortify` pursues own + neutral (hold + safe expansion, never the enemy).
fn is_target(world: &World, t: PlanetId, seat: Faction, lens: Lens) -> bool {
    let agg = world.planet_aggregate(t);
    let foe = seat.opponent();
    let neutral = matches!(agg.owner, PlanetOwner::Neutral) && agg.ships_of(foe) == 0;
    let enemy = matches!(agg.owner, PlanetOwner::Owned(f) if f == foe)
        || (matches!(agg.owner, PlanetOwner::Contested) && agg.ships_of(foe) > 0);
    let own = matches!(agg.owner, PlanetOwner::Owned(f) if f == seat);
    match lens {
        Lens::Expand => neutral || own,
        Lens::Assault => enemy || own,
        // Defend considers ALL ground, but its pessimism (a competent adversary answers) gates it to
        // *safe* moves: hold own, expand only where the foe can't punish, counter-attack only an
        // over-extended enemy's exposed ground. The caution is the identity, not the target filter.
        Lens::Fortify => own || neutral || enemy,
    }
}

/// Generate the candidate force-allocations to score. Deliberately a SMALL, legible set (the maps
/// are small, so this is far from the NP-hard general assignment): HOLD, a **concentrate-on-T**
/// option per reachable target (every source ships its surplus toward `T` — this is what lets the
/// optimizer discover the wave/threshold effect: under-concentrating simply scores worse), and a
/// **spread** option (each source to its nearest distinct neutral, for parallel expansion).
fn candidate_allocations(world: &World, seat: Faction, wp: &WorldParams, lens: Lens) -> Vec<Vec<FleetOrder>> {
    let n = world.planets.len();

    // Sources: a planet `seat` securely holds (export precondition) with surplus above the floor.
    let sources: Vec<PlanetId> = (0..n)
        .filter(|&p| {
            let agg = world.planet_aggregate(p);
            matches!(agg.owner, PlanetOwner::Owned(f) if f == seat)
                && agg.fully_owned_uncontested(seat)
                && exportable_surplus(world, p, seat, wp.keep_floor) > 0
        })
        .collect();

    // Always include HOLD (issue nothing) — a real choice (amass / not over-extend).
    let mut cands: Vec<Vec<FleetOrder>> = vec![Vec::new()];
    if sources.is_empty() {
        return cands;
    }

    // Targets: planets reachable from a source AND in the identity's LENS (its committed target
    // class). The lens is the structural identity; the integral optimises within it.
    let targets: Vec<PlanetId> = (0..n)
        .filter(|&t| {
            is_target(world, t, seat, lens)
                && sources.iter().any(|&s| s != t && crate::graph::next_hop(world, s, t).is_some())
        })
        .collect();

    // CONCENTRATE on each target: every source ships its whole surplus toward T.
    for &t in &targets {
        let mut orders = Vec::new();
        for &s in &sources {
            if let Some(hop) = crate::graph::next_hop(world, s, t) {
                orders.push(FleetOrder::new(s, hop, FractionBucket::All));
            }
        }
        if !orders.is_empty() {
            cands.push(orders);
        }
    }

    // SPREAD: assign each source to its nearest distinct reachable NEUTRAL target (parallel grab).
    {
        let mut orders = Vec::new();
        let mut taken: HashSet<PlanetId> = HashSet::new();
        for &s in &sources {
            let mut best: Option<(PlanetId, f32)> = None;
            for &t in &targets {
                if taken.contains(&t) {
                    continue;
                }
                if !matches!(world.planet_aggregate(t).owner, PlanetOwner::Neutral) {
                    continue;
                }
                if let Some(d) = crate::graph::path_len(world, s, t) {
                    if best.map_or(true, |(_, bd)| d < bd) {
                        best = Some((t, d));
                    }
                }
            }
            if let Some((t, _)) = best {
                taken.insert(t);
                if let Some(hop) = crate::graph::next_hop(world, s, t) {
                    orders.push(FleetOrder::new(s, hop, FractionBucket::All));
                }
            }
        }
        if !orders.is_empty() {
            cands.push(orders);
        }
    }

    cands
}

/// A fast, deterministic stand-in for a **competent adversary's answer** (the pessimist's
/// distortion). The foe concentrates its exportable surplus onto `seat`'s most valuable *reachable*
/// producer — the one it would hurt me most by taking — injected before the projection. So the
/// defender plans against "if I expose this, a capable foe punishes it," without the cost of a full
/// nested best-response. (One heuristic counter-allocation; not the optimal adversary, but a sound
/// worst-case the reactive identity prepares for.)
fn inject_competent_adversary(w: &mut World, seat: Faction, wp: &WorldParams) {
    let foe = seat.opponent();
    let n = w.planets.len();
    let foe_sources: Vec<PlanetId> = (0..n)
        .filter(|&p| {
            let agg = w.planet_aggregate(p);
            matches!(agg.owner, PlanetOwner::Owned(f) if f == foe)
                && agg.fully_owned_uncontested(foe)
                && exportable_surplus(w, p, foe, wp.keep_floor) > 0
        })
        .collect();
    if foe_sources.is_empty() {
        return;
    }
    // My **softest** reachable producer: fewest present defenders (most cheaply taken), tie-broken
    // by most subs (most damaging), then lowest id (determinism). Targeting the soft underbelly —
    // rather than the best-defended jewel — is what makes a pessimistic defender wary of *exposing*
    // thin ground (a fresh expedition, a stripped rear), so it keeps force home instead of
    // over-expanding.
    let mut best: Option<(PlanetId, u32, usize)> = None;
    for p in 0..n {
        let agg = w.planet_aggregate(p);
        if !matches!(agg.owner, PlanetOwner::Owned(f) if f == seat) {
            continue;
        }
        if !foe_sources.iter().any(|&s| s != p && crate::graph::next_hop(w, s, p).is_some()) {
            continue;
        }
        let defenders = agg.ships_of(seat);
        let value = w.planets[p].structure.subs.iter().filter(|s| s.owner == seat).count();
        let better = match best {
            None => true,
            Some((_, bd, bv)) => defenders < bd || (defenders == bd && value > bv),
        };
        if better {
            best = Some((p, defenders, value));
        }
    }
    let Some((target, _, _)) = best else { return };
    for &s in &foe_sources {
        if let Some(hop) = crate::graph::next_hop(w, s, target) {
            w.issue_fleet_order(FleetOrder::new(s, hop, FractionBucket::All), foe, wp);
        }
    }
}

/// Exportable surplus of `seat` on planet `p`: idle ships above `keep_floor` on the subs `seat`
/// owns (the pool [`World::issue_fleet_order`] draws from). Mirrors the Layer-2 adapter's
/// `exportable_idle` so a candidate's force estimate matches what the primitive will actually move.
fn exportable_surplus(world: &World, p: PlanetId, seat: Faction, keep_floor: usize) -> u32 {
    let Some(planet) = world.planets.get(p) else { return 0 };
    let st = &planet.structure;
    let mut total = 0u32;
    for s in 0..st.subs.len() {
        if st.subs[s].owner != seat {
            continue;
        }
        let idle = st.idle_count_at(s, seat);
        if idle > keep_floor {
            total += (idle - keep_floor) as u32;
        }
    }
    total
}
