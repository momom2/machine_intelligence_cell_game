//! **Simple** — the stateful colonizer (the campaign "Simple" enemy).
//!
//! Unlike the other rosters (pure functions of the observed world, carried by the stateless
//! [`crate::controller::AiController`]), Simple keeps a **persistent per-planet departure ledger**
//! across decision ticks, so it is driven by its own [`SimpleController`] — the same pattern the
//! [`crate::counter::CounterController`] uses.
//!
//! # Two layers
//!
//! * **Layer 1 (intra-planet, the heart):** for each planet the seat has presence on, run the four
//!   phases [`Phase::EXPIRE`]→DEFEND→PLAN+COMMIT→DISPATCH against that planet's ledger
//!   ([`simple_layer1_step`]). It sizes captures by a **minimum** (the bar to start a front) and a
//!   **maximum** (how much is worth committing), secures up to [`SimpleParams::fronts`] fronts at a
//!   time then deepens them, and *staggers* each taskforce's departures so the ships **land
//!   together** (synchronised arrival, realised purely from AI state — the engine never holds ships,
//!   so a committed-but-not-yet-due leg merely *reserves* its ships in the ledger until its tick).
//!
//! * **Layer 2 (inter-planet, simplified):** from each **fully-owned, uncontested** planet, push the
//!   surplus toward the nearest **frontline** planet (any planet that is not a quiet rear). No ledger,
//!   no retreat, no staggering — just a steady feed toward the fight. On a single-planet level (every
//!   campaign mission Simple plays today) this is a no-op and the Layer-1 ledger is the whole game.
//!
//! # OVERWHELM
//!
//! `OVERWHELM(n) = max(ratio·n, n + add)` (default `max(1.2·n, n+20)`) — the force needed to beat `n`
//! defenders. `OVERWHELM(0) = add = min_wave`, so an undefended neutral costs exactly the floor wave.
//! Every threshold rounds **up** (`ceil`): a force floor must never round below its requirement.
//!
//! # Determinism
//!
//! The ledger is `Vec`-only (never a `HashMap`); every phase iterates subs/ops/legs by ascending
//! index; every nearest/candidate sort breaks ties by ascending id; force math is done in `f32` then
//! `ceil`-ed once; the clock is the single absolute `world.tick`. So identical inputs evolve the
//! ledger identically and `World::state_hash` stays bit-identical on replay. The controller is built
//! fresh per match, so the ledger never leaks across matches.

use layer1::{Faction, SimParams};
use world::{World, WorldParams};

use crate::adapters::{Layer1View, Layer2View};
use crate::greedy::{GreedyAction, GreedyKind, PosOwner, PositionView};
use crate::vocab;

/// Policy dials for [`SimpleController`]. **All policy, no mechanics** — the headless/AI/test
/// reference uses these unscaled defaults; the GUI scales the *sim* params, never these.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SimpleParams {
    /// Garrison floor: every owned sub keeps this many ships home; only ships strictly above it are
    /// *surplus* eligible to move (the over-threat retreat is the one exception — it moves all).
    pub floor: u32,
    /// Smallest assault worth launching. Equals `OVERWHELM(0)` by construction (= `overwhelm_add`),
    /// documented here so the relationship is explicit.
    pub min_wave: u32,
    /// Parallel fronts: secure the minimum of up to this many targets, deepen them toward their
    /// maximum, then move to the next batch.
    pub fronts: usize,
    /// `OVERWHELM` multiplicative margin (the `1.2` in `max(1.2·n, n+20)`).
    pub overwhelm_ratio: f32,
    /// `OVERWHELM` additive margin (the `+20`; also the undefended-neutral cost / `min_wave`).
    pub overwhelm_add: u32,
    /// Divisor turning a neutral's capture **resistance** into its ship-equivalent defence for the
    /// *maximum* sizing (`OVERWHELM(resistance/this)`). Matches the sim's resistance-per-capacity, so
    /// `resistance/this` ≈ the neutral's capacity.
    pub neutral_res_divisor: f32,
    /// Multiplier on an **enemy** sub's present+incoming defenders for the *maximum* sizing
    /// (`OVERWHELM(this·foes)`) — commit up to a decisive margin, never below the minimum.
    pub non_neutral_foe_mult: f32,
    /// Weight on the **distance** term when ranking neutral capture targets. The PLAN priority of a
    /// neutral is `resistance / production + neutral_dist_weight · travel_to_nearest_owned` (lower =
    /// attack first). Kept small so cost-effectiveness (cheap to grind, high production) dominates and
    /// distance only nudges among similar-value neutrals.
    pub neutral_dist_weight: f32,
}

impl Default for SimpleParams {
    fn default() -> Self {
        SimpleParams {
            floor: 10,
            min_wave: 20,
            fronts: 3,
            overwhelm_ratio: 1.2,
            overwhelm_add: 20,
            neutral_res_divisor: 60.0,
            non_neutral_foe_mult: 2.0,
            neutral_dist_weight: 5.0,
        }
    }
}

impl SimpleParams {
    /// `OVERWHELM(n) = max(ratio·n, n + add)`, rounded **up** to a whole ship count (a force floor
    /// must never round below its requirement). `n` is an `f32` so callers can pass `resistance/div`.
    ///
    /// The `ceil` uses a small tolerance so f32 imprecision in `ratio·n` (e.g. `1.2 * 100.0` is
    /// `120.0000048`, not `120.0`) does not spuriously round a whole-number force **up** by one ship.
    /// Deterministic: the same f32 ops every call.
    #[inline]
    pub fn overwhelm(&self, n: f32) -> u32 {
        const EPS: f32 = 1e-3;
        let a = (self.overwhelm_ratio * n - EPS).ceil();
        let b = (n + self.overwhelm_add as f32 - EPS).ceil();
        a.max(b).max(0.0) as u32
    }
}

// =====================================================================================
// The persistent departure ledger (Vec-only — determinism).
// =====================================================================================

/// One leg of a synchronised taskforce: `count` ships from `src`, scheduled to **depart** at
/// `depart_at` (so the whole op lands together). `sent` flips once the real move has been issued.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Leg {
    pub(crate) src: usize,
    pub(crate) count: u32,
    pub(crate) depart_at: u64,
    pub(crate) sent: bool,
}

/// A committed assault on `target`: a set of staggered [`Leg`]s timed to all arrive by `land_at`.
/// The op is dropped once `now >= land_at` (its ships have landed).
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Op {
    pub(crate) target: usize,
    pub(crate) land_at: u64,
    pub(crate) legs: Vec<Leg>,
}

/// A concrete move to issue this tick (a DISPATCH leg coming due, or a retreat). `count == u32::MAX`
/// means "everything idle" (the over-threat full evacuation — issue_order_count clamps to idle).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Move {
    pub(crate) src: usize,
    pub(crate) tgt: usize,
    pub(crate) count: u32,
}

// =====================================================================================
// Derived quantities (pure reads of the view + ledger).
// =====================================================================================

/// Present + incoming foe force at `t` (the defence a taskforce must beat).
fn foes<V: PositionView>(view: &V, t: usize) -> u32 {
    view.info(t).enemy_ships + view.enemy_incoming(t)
}

/// The **bar to start a front** on `t`: an undefended neutral costs `OVERWHELM(0)`; defended ground
/// costs `OVERWHELM(present+incoming foes)`.
fn minimum<V: PositionView>(view: &V, t: usize, p: &SimpleParams) -> u32 {
    if view.info(t).owner == PosOwner::Neutral {
        p.overwhelm(0.0)
    } else {
        p.overwhelm(foes(view, t) as f32)
    }
}

/// The **most worth committing** to `t`: enough to grind a neutral's resistance out
/// (`OVERWHELM(resistance/div)`), or to crush an enemy sub with margin (`OVERWHELM(mult·foes)`).
fn maximum<V: PositionView>(view: &V, t: usize, p: &SimpleParams) -> u32 {
    if view.info(t).owner == PosOwner::Neutral {
        p.overwhelm(view.resistance(t) / p.neutral_res_divisor)
    } else {
        p.overwhelm(p.non_neutral_foe_mult * foes(view, t) as f32)
    }
}

/// Ships from `s` reserved by UNSENT legs (the departure ledger — they have not left yet but are
/// spoken for, so `spare` must not re-spend them).
fn committed_out(ops: &[Op], s: usize) -> u32 {
    ops.iter().flat_map(|op| &op.legs).filter(|l| !l.sent && l.src == s).map(|l| l.count).sum()
}

/// My force already committed toward `t` by UNSENT legs (in-transit, sent legs are counted by
/// `incoming_mine` instead, so there is no double count).
fn committed_in(ops: &[Op], t: usize) -> u32 {
    ops.iter()
        .filter(|op| op.target == t)
        .flat_map(|op| &op.legs)
        .filter(|l| !l.sent)
        .map(|l| l.count)
        .sum()
}

/// Surplus `s` can still give an attack: idle above the floor, minus what is already reserved.
fn spare<V: PositionView>(view: &V, ops: &[Op], s: usize, p: &SimpleParams) -> u32 {
    view.info(s).my_ships.saturating_sub(p.floor).saturating_sub(committed_out(ops, s))
}

/// Everything already working on `t`: present + in-flight (`incoming_mine`) + my still-undeparted
/// committed legs. Subtracting this from the minimum/maximum is what makes the planner self-throttle
/// (a target with enough inbound needs no new wave) and replaces the old binary "ignore" flag.
fn our_force<V: PositionView>(view: &V, ops: &[Op], t: usize) -> u32 {
    view.info(t).my_ships + view.incoming_mine(t) + committed_in(ops, t)
}

/// `true` when a foe (present **or** incoming) can overwhelm my garrison here — flee.
fn over_threat<V: PositionView>(view: &V, s: usize, p: &SimpleParams) -> bool {
    let info = view.info(s);
    info.enemy_ships + view.enemy_incoming(s) >= p.overwhelm(info.my_ships as f32)
}

/// `true` when a foe is present and eroding me but not (yet) overwhelming — hold (pin) the garrison.
fn being_captured<V: PositionView>(view: &V, s: usize, p: &SimpleParams) -> bool {
    view.info(s).enemy_ships > 0 && !over_threat(view, s, p)
}

/// Travel `a`→`b` in **ticks** (deterministic `transit_ticks`), or `u64::MAX` if unreachable.
fn travel<V: PositionView>(view: &V, a: usize, b: usize) -> u64 {
    view.transit_ticks(a, b).unwrap_or(u64::MAX)
}

/// Least travel from any owned sub to `t` (the candidate's "nearest-owned distance" sort key).
fn nearest_owned_travel<V: PositionView>(view: &V, t: usize) -> u64 {
    (0..view.len())
        .filter(|&s| view.info(s).owner == PosOwner::Me && view.reachable(s, t))
        .map(|s| travel(view, s, t))
        .min()
        .unwrap_or(u64::MAX)
}

/// PLAN ordering score for a candidate `t` (**lower = attack first**). A **neutral** is ranked by
/// capture cost-effectiveness `resistance / production` plus a small term proportional to its travel
/// distance from the nearest owned sub (`neutral_dist_weight`) — so Simple prefers cheap-to-grind,
/// high-production, nearby neutrals rather than purely the nearest one. An **enemy** keeps the plain
/// nearest-owned-first ordering (its capture is sized by force, not resistance).
fn neutral_priority<V: PositionView>(view: &V, t: usize, p: &SimpleParams) -> f32 {
    let dist = nearest_owned_travel(view, t) as f32;
    if view.info(t).owner == PosOwner::Neutral {
        view.resistance(t) / view.production(t).max(1.0) + p.neutral_dist_weight * dist
    } else {
        dist
    }
}

/// Nearest **safe owned** sub to flee to (owned, uncontested, not itself over-threatened). Lowest
/// travel, ties by lowest id.
fn nearest_safe<V: PositionView>(view: &V, from: usize, p: &SimpleParams) -> Option<usize> {
    let mut best: Option<(u64, usize)> = None;
    for to in 0..view.len() {
        if to == from {
            continue;
        }
        let info = view.info(to);
        if info.owner != PosOwner::Me || info.contested || over_threat(view, to, p) {
            continue;
        }
        if !view.reachable(from, to) {
            continue;
        }
        let d = travel(view, from, to);
        match best {
            Some((bd, _)) if bd <= d => {}
            _ => best = Some((d, to)),
        }
    }
    best.map(|(_, id)| id)
}

/// Pull up to `want` ships toward `t`, nearest-source-first (ties by id), reserving from `avail` into
/// `legs`. Returns how much was actually gathered (may be `< want` if sources run dry).
fn pull<V: PositionView>(view: &V, t: usize, want: u32, avail: &mut [u32], legs: &mut Vec<(usize, u32)>) -> u32 {
    if want == 0 {
        return 0;
    }
    let mut srcs: Vec<usize> = (0..view.len()).filter(|&s| avail[s] > 0 && view.reachable(s, t)).collect();
    srcs.sort_by(|&a, &b| travel(view, a, t).cmp(&travel(view, b, t)).then(a.cmp(&b)));
    let mut got = 0u32;
    for s in srcs {
        if got >= want {
            break;
        }
        let take = avail[s].min(want - got);
        if take == 0 {
            continue;
        }
        avail[s] -= take;
        legs.push((s, take));
        got += take;
    }
    got
}

// =====================================================================================
// Layer 1 — the four phases (the heart). Pure: mutates `ops`, returns the moves to ISSUE.
// =====================================================================================

/// Run one decision tick of the Layer-1 program for a single planet against its `ops` ledger,
/// returning the concrete [`Move`]s to issue this tick (retreats first, then dispatched legs). `now`
/// is the absolute `world.tick`.
pub(crate) fn simple_layer1_step<V: PositionView>(
    view: &V,
    ops: &mut Vec<Op>,
    now: u64,
    p: &SimpleParams,
) -> Vec<Move> {
    let n = view.len();
    let mut moves: Vec<Move> = Vec::new();
    if n == 0 {
        return moves;
    }

    // ---- (0) EXPIRE: an assault that has landed stops reserving anything. ----
    ops.retain(|op| now < op.land_at);

    // ---- (1) DEFEND: flee the overwhelmed, pin the contested. ----
    let mut fleeing = vec![false; n];
    let mut pinned = vec![false; n];
    for s in 0..n {
        let info = view.info(s);
        if info.owner != PosOwner::Me {
            continue;
        }
        if over_threat(view, s, p) {
            fleeing[s] = true;
            if let Some(dst) = nearest_safe(view, s, p) {
                // Move EVERYTHING — ignore the floor (u32::MAX is clamped to idle by issue_order_count).
                moves.push(Move { src: s, tgt: dst, count: u32::MAX });
            }
        } else if being_captured(view, s, p) {
            pinned[s] = true;
        }
    }
    // Cancel any op fed by a fleeing/pinned source's UNSENT leg (already-sent ships have flown).
    ops.retain(|op| !op.legs.iter().any(|l| !l.sent && (fleeing[l.src] || pinned[l.src])));

    // ---- (2) PLAN + COMMIT: secure FRONTS minimums, then deepen toward maximums. ----
    let mut avail: Vec<u32> = (0..n)
        .map(|s| {
            let info = view.info(s);
            if info.owner == PosOwner::Me && !fleeing[s] && !pinned[s] {
                spare(view, ops, s, p)
            } else {
                0
            }
        })
        .collect();

    let mut candidates: Vec<usize> = (0..n)
        .filter(|&t| {
            view.info(t).owner != PosOwner::Me
                && (0..n).any(|s| view.info(s).owner == PosOwner::Me && view.reachable(s, t))
        })
        .collect();
    // Neutral before Enemy; within a group ascending by `neutral_priority` (a neutral by
    // resistance/production + a small distance term, an enemy by nearest-owned); ties break by id.
    candidates.sort_by(|&a, &b| {
        let na = (view.info(a).owner != PosOwner::Neutral) as u8;
        let nb = (view.info(b).owner != PosOwner::Neutral) as u8;
        na.cmp(&nb)
            .then(
                neutral_priority(view, a, p)
                    .partial_cmp(&neutral_priority(view, b, p))
                    .unwrap_or(std::cmp::Ordering::Equal),
            )
            .then(a.cmp(&b))
    });

    let mut plan: Vec<(usize, Vec<(usize, u32)>)> = Vec::new();
    let mut qi = 0;
    while qi < candidates.len() {
        // Phase A: fill up to FRONTS fronts we can secure the minimum of.
        let mut batch: Vec<usize> = Vec::new();
        while qi < candidates.len() && batch.len() < p.fronts {
            let t = candidates[qi];
            qi += 1;
            let of = our_force(view, ops, t);
            if maximum(view, t, p).saturating_sub(of) == 0 {
                continue; // already maxed/covered — don't burn a front slot.
            }
            let to_min = minimum(view, t, p).saturating_sub(of);
            let mut legs: Vec<(usize, u32)> = Vec::new();
            let got = pull(view, t, to_min, &mut avail, &mut legs);
            if got < to_min {
                for (s, c) in &legs {
                    avail[*s] += *c; // roll back — couldn't field the minimum.
                }
                continue;
            }
            plan.push((t, legs));
            batch.push(t);
        }
        if batch.is_empty() {
            break;
        }
        // Phase B: push each secured front toward its maximum.
        for &t in &batch {
            let of = our_force(view, ops, t);
            let to_max = maximum(view, t, p).saturating_sub(of);
            let entry = plan.iter_mut().find(|(tt, _)| *tt == t).expect("front planned in phase A");
            let already: u32 = entry.1.iter().map(|(_, c)| *c).sum();
            let want_more = to_max.saturating_sub(already);
            if want_more > 0 {
                pull(view, t, want_more, &mut avail, &mut entry.1);
            }
        }
    }

    // Commit one staggered op per planned target.
    for (t, legs) in plan {
        if legs.is_empty() {
            continue;
        }
        // Coalesce legs sharing a source (one op leg per src — stable RNG draw count on dispatch).
        let mut coalesced: Vec<(usize, u32)> = Vec::new();
        for (s, c) in legs {
            if let Some(e) = coalesced.iter_mut().find(|(ss, _)| *ss == s) {
                e.1 += c;
            } else {
                coalesced.push((s, c));
            }
        }
        // Land no earlier than the farthest leg's travel, any inbound friendly ETA, or a prior op's
        // landing for the same target.
        let mut land_at = now.saturating_add(coalesced.iter().map(|(s, _)| travel(view, *s, t)).max().unwrap_or(0));
        if let Some(eta) = view.friendly_eta(t) {
            land_at = land_at.max(eta);
        }
        for op in ops.iter() {
            if op.target == t {
                land_at = land_at.max(op.land_at);
            }
        }
        let op_legs: Vec<Leg> = coalesced
            .into_iter()
            .map(|(s, c)| Leg { src: s, count: c, depart_at: land_at.saturating_sub(travel(view, s, t)), sent: false })
            .collect();
        ops.push(Op { target: t, land_at, legs: op_legs });
    }

    // ---- (3) DISPATCH: fire the staggered legs that have come due. ----
    for op in ops.iter_mut() {
        for leg in op.legs.iter_mut() {
            if !leg.sent && now >= leg.depart_at && !fleeing[leg.src] && !pinned[leg.src] {
                moves.push(Move { src: leg.src, tgt: op.target, count: leg.count });
                leg.sent = true;
            }
        }
    }

    moves
}

// =====================================================================================
// Layer 2 — the simplified push (no ledger / retreat / stagger).
// =====================================================================================

/// From each fully-owned, uncontested planet, send the surplus toward the nearest **frontline**
/// planet (any reachable planet that is not a quiet Me rear: a foe present, contested, or not mine).
/// Returns layer-agnostic [`GreedyAction`]s for [`Layer2View::to_fleet_orders`] to route + bucket.
fn simple_layer2_push<V: PositionView>(view: &V, p: &SimpleParams) -> Vec<GreedyAction> {
    let mut actions = Vec::new();
    for from in 0..view.len() {
        let info = view.info(from);
        if info.owner != PosOwner::Me || info.contested || !view.can_export_from(from) {
            continue;
        }
        let surplus = info.my_ships.saturating_sub(p.floor);
        if surplus == 0 {
            continue;
        }
        let target = vocab::nearest(view, from, |i| {
            i.id != from && (i.owner != PosOwner::Me || i.contested || i.enemy_ships > 0)
        });
        if let Some(to) = target {
            actions.push(GreedyAction { from, to, count: surplus, kind: GreedyKind::Wave });
        }
    }
    actions
}

// =====================================================================================
// The controller (the stateful host — mirrors CounterController).
// =====================================================================================

/// The stateful driver for the **Simple** seat: owns the per-planet departure ledger and runs both
/// layers each decision tick. Non-`Copy` (it accumulates state), built once per match.
#[derive(Debug, Clone)]
pub struct SimpleController {
    /// The seat Simple plays.
    pub seat: Faction,
    /// Policy dials.
    p: SimpleParams,
    /// The persistent departure ledger, indexed by planet id. Resized to the world's planet count on
    /// first use; an entry is cleared if the seat loses all presence on that planet.
    operations: Vec<Vec<Op>>,
}

impl SimpleController {
    /// A fresh Simple controller for `seat` (default policy dials, empty ledger).
    pub fn new(seat: Faction) -> SimpleController {
        SimpleController { seat, p: SimpleParams::default(), operations: Vec::new() }
    }

    /// Decide and apply this seat's full turn for the decision tick, in the documented order
    /// (per-planet internals first, then inter-planet fleets). Mutates the ledger and the world.
    /// Returns `(ships moved internally, ships launched in fleets)`.
    pub fn decide_and_apply(&mut self, world: &mut World, sp: &SimParams, wp: &WorldParams) -> (usize, usize) {
        let seat = self.seat;
        let params = self.p;
        let np = world.planets.len();
        if self.operations.len() != np {
            self.operations.resize(np, Vec::new());
        }

        let now = world.tick;

        // ---- Layer 1: per-planet ledger -> internal moves (decided against the pre-apply world). ----
        // Look-ahead is the projection-free in-transit influx: `World::sub_influx_for` reads who is
        // inbound to each sub directly off the *current* state (no forward projection is built).
        let mut planet_moves: Vec<(usize, Vec<Move>)> = Vec::new();
        for p in 0..np {
            let st = &world.planets[p].structure;
            if st.sub_count(seat) == 0 && st.ship_count(seat) == 0 {
                self.operations[p].clear(); // lost the planet — drop its stale ledger.
                continue;
            }
            let influx = world.sub_influx_for(p, seat, sp, wp);
            let view = Layer1View::direct(st, sp, seat, influx);
            let moves = simple_layer1_step(&view, &mut self.operations[p], now, &params);
            if !moves.is_empty() {
                planet_moves.push((p, moves));
            }
        }

        // ---- Layer 2: simplified push toward the frontlines -> fleet orders (projection-free). ----
        let l2 = Layer2View::without_projection(world, seat, sp, wp);
        let fleet_orders = l2.to_fleet_orders(&simple_layer2_push(&l2, &params), wp);
        drop(l2);

        // ---- Apply: internals first (exact counts), then fleets. ----
        let mut moved = 0usize;
        for (p, mvs) in planet_moves {
            for m in mvs {
                moved += world.planets[p].structure.issue_order_count(m.src, m.tgt, m.count as usize, seat);
            }
        }
        let mut launched = 0usize;
        for o in fleet_orders {
            launched += world.issue_fleet_order(o, seat, wp) as usize;
        }
        (moved, launched)
    }
}

// =====================================================================================
// Tests — the PURE Layer-1 program over a tiny in-memory view (no sim needed).
// =====================================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::greedy::{PosOwner, PositionInfo, PositionView};

    /// Minimal line view: travel(a,b) = |xa - xb| (≥1) ticks, settable resistance / incoming / eta.
    struct TV {
        infos: Vec<PositionInfo>,
        xs: Vec<i64>,
        resist: Vec<f32>,
        inc_mine: Vec<u32>,
        inc_enemy: Vec<u32>,
        fr_eta: Vec<Option<u64>>,
    }

    impl TV {
        fn new(rows: &[(PosOwner, u32, u32)]) -> TV {
            let n = rows.len();
            let infos = rows
                .iter()
                .enumerate()
                .map(|(i, &(owner, my, en))| PositionInfo {
                    id: i,
                    owner,
                    my_ships: my,
                    enemy_ships: en,
                    contested: my > 0 && en > 0,
                })
                .collect();
            TV {
                infos,
                xs: (0..n as i64).collect(),
                resist: vec![0.0; n],
                inc_mine: vec![0; n],
                inc_enemy: vec![0; n],
                fr_eta: vec![None; n],
            }
        }
    }

    impl PositionView for TV {
        fn len(&self) -> usize {
            self.infos.len()
        }
        fn info(&self, id: usize) -> PositionInfo {
            self.infos[id]
        }
        fn distance(&self, a: usize, b: usize) -> Option<f32> {
            Some((self.xs[a] - self.xs[b]).abs() as f32)
        }
        fn transit_ticks(&self, a: usize, b: usize) -> Option<u64> {
            Some((self.xs[a] - self.xs[b]).unsigned_abs().max(1))
        }
        fn resistance(&self, id: usize) -> f32 {
            self.resist[id]
        }
        fn incoming_mine(&self, id: usize) -> u32 {
            self.inc_mine[id]
        }
        fn enemy_incoming(&self, id: usize) -> u32 {
            self.inc_enemy[id]
        }
        fn friendly_eta(&self, id: usize) -> Option<u64> {
            self.fr_eta[id]
        }
    }

    fn leg_for<'a>(op: &'a Op, src: usize) -> &'a Leg {
        op.legs.iter().find(|l| l.src == src).expect("a leg from that source")
    }

    #[test]
    fn overwhelm_formula() {
        let p = SimpleParams::default();
        assert_eq!(p.overwhelm(0.0), 20); // OVERWHELM(0) == min_wave == add
        assert_eq!(p.overwhelm(10.0), 30); // max(ceil(12), 30)
        assert_eq!(p.overwhelm(50.0), 70); // max(60, 70)
        assert_eq!(p.overwhelm(100.0), 120); // max(120, 120)
        assert_eq!(p.overwhelm(200.0), 240); // max(240, 220)
    }

    #[test]
    fn minimum_never_exceeds_maximum() {
        let p = SimpleParams::default();
        // Neutral with a fat resistance.
        let mut v = TV::new(&[(PosOwner::Me, 100, 0), (PosOwner::Neutral, 0, 0)]);
        v.resist[1] = 1800.0;
        assert_eq!(minimum(&v, 1, &p), 20);
        assert_eq!(maximum(&v, 1, &p), 50); // OVERWHELM(30)
        assert!(minimum(&v, 1, &p) <= maximum(&v, 1, &p));
        // Enemy sub with present+incoming foes.
        let mut e = TV::new(&[(PosOwner::Me, 100, 0), (PosOwner::Enemy, 0, 10)]);
        e.inc_enemy[1] = 0;
        assert_eq!(minimum(&e, 1, &p), 30); // OVERWHELM(10)
        assert_eq!(maximum(&e, 1, &p), 40); // OVERWHELM(20)
        assert!(minimum(&e, 1, &p) <= maximum(&e, 1, &p));
    }

    #[test]
    fn enemy_incoming_counts_toward_foes() {
        let p = SimpleParams::default();
        let mut v = TV::new(&[(PosOwner::Me, 100, 0), (PosOwner::Enemy, 0, 5)]);
        v.inc_enemy[1] = 7; // 5 present + 7 inbound = 12 defenders
        assert_eq!(foes(&v, 1), 12);
        assert_eq!(minimum(&v, 1, &p), p.overwhelm(12.0));
    }

    #[test]
    fn secures_minimum_then_caps_at_maximum() {
        let p = SimpleParams::default();
        let mut v = TV::new(&[(PosOwner::Me, 100, 0), (PosOwner::Neutral, 0, 0)]);
        v.resist[1] = 600.0; // capacity-equiv 10 -> max = OVERWHELM(10) = 30
        let mut ops = Vec::new();
        let moves = simple_layer1_step(&v, &mut ops, 0, &p);
        assert_eq!(ops.len(), 1);
        assert_eq!(ops[0].target, 1);
        let total: u32 = ops[0].legs.iter().map(|l| l.count).sum();
        assert_eq!(total, 30, "secured min(20) then deepened to max(30), not more");
        // Single near source -> the whole wave departs now.
        assert_eq!(moves, vec![Move { src: 0, tgt: 1, count: 30 }]);
    }

    #[test]
    fn does_not_oversend_when_already_covered() {
        let p = SimpleParams::default();
        let mut v = TV::new(&[(PosOwner::Me, 100, 0), (PosOwner::Neutral, 0, 0)]);
        v.resist[1] = 600.0; // max = 30
        v.inc_mine[1] = 40; // already 40 of my ships inbound (> max) -> nothing to add
        let mut ops = Vec::new();
        let moves = simple_layer1_step(&v, &mut ops, 0, &p);
        assert!(ops.is_empty(), "covered target gets no new op");
        assert!(moves.is_empty());
    }

    #[test]
    fn fronts_batching_secures_nearest_first_under_budget() {
        // One source with spare 60 (my 70 - floor 10); four equal neutrals at x=1..4 each needing
        // min 20. FRONTS=3 so the nearest three get secured; the fourth can't (budget spent).
        let p = SimpleParams::default();
        let mut v = TV::new(&[
            (PosOwner::Me, 70, 0),
            (PosOwner::Neutral, 0, 0),
            (PosOwner::Neutral, 0, 0),
            (PosOwner::Neutral, 0, 0),
            (PosOwner::Neutral, 0, 0),
        ]);
        for t in 1..=4 {
            v.resist[t] = 1200.0; // max = OVERWHELM(20) = 40, min = 20
        }
        let mut ops = Vec::new();
        let _ = simple_layer1_step(&v, &mut ops, 0, &p);
        let mut targets: Vec<usize> = ops.iter().map(|o| o.target).collect();
        targets.sort();
        assert_eq!(targets, vec![1, 2, 3], "3 nearest fronts secured, the 4th left for later");
    }

    #[test]
    fn defend_evacuates_an_overwhelmed_sub() {
        let p = SimpleParams::default();
        // id0 (10 ships) faces 50 foes -> over_threat -> flee EVERYTHING to the safe id1.
        let v = TV::new(&[(PosOwner::Me, 10, 50), (PosOwner::Me, 20, 0)]);
        let mut ops = Vec::new();
        let moves = simple_layer1_step(&v, &mut ops, 0, &p);
        assert_eq!(moves, vec![Move { src: 0, tgt: 1, count: u32::MAX }]);
    }

    #[test]
    fn being_captured_pins_the_garrison() {
        let p = SimpleParams::default();
        // id0 has 100 ships and 5 foes present: not overwhelmed, but being captured -> pinned, so it
        // is NOT used to source the neutral id1, and no op is committed.
        let mut v = TV::new(&[(PosOwner::Me, 100, 5), (PosOwner::Neutral, 0, 0)]);
        v.resist[1] = 600.0;
        let mut ops = Vec::new();
        let moves = simple_layer1_step(&v, &mut ops, 0, &p);
        assert!(ops.is_empty(), "pinned sole source cannot fund a front");
        assert!(moves.is_empty());
    }

    #[test]
    fn staggers_departures_to_land_together() {
        let p = SimpleParams::default();
        // Two sources at x0,x1; a fat neutral at x2 needing both. Legs stagger so both LAND at land_at.
        let mut v = TV::new(&[(PosOwner::Me, 100, 0), (PosOwner::Me, 100, 0), (PosOwner::Neutral, 0, 0)]);
        v.resist[2] = 12000.0; // max = OVERWHELM(200) = 240 -> needs both sources (90 + 90)
        let mut ops = Vec::new();
        let moves = simple_layer1_step(&v, &mut ops, 0, &p);
        assert_eq!(ops.len(), 1);
        let op = &ops[0];
        assert_eq!(op.target, 2);
        assert_eq!(op.land_at, 2, "land = now + farthest travel (x0->x2 = 2)");
        // Farthest source (id0, travel 2) departs now; nearer source (id1, travel 1) departs at +1.
        assert_eq!(leg_for(op, 0).depart_at, 0);
        assert_eq!(leg_for(op, 1).depart_at, 1);
        // Only the due leg fires this tick.
        assert_eq!(moves, vec![Move { src: 0, tgt: 2, count: leg_for(op, 0).count }]);
    }

    #[test]
    fn reservation_persists_and_prevents_double_spend() {
        let p = SimpleParams::default();
        // A friendly wave is inbound to the neutral (eta now+10), so the new wave is scheduled to land
        // with it (depart in the future) — its ships are RESERVED, not dispatched this tick.
        let mut v = TV::new(&[(PosOwner::Me, 100, 0), (PosOwner::Neutral, 0, 0)]);
        v.resist[1] = 600.0; // max 30
        v.fr_eta[1] = Some(10);
        let mut ops = Vec::new();
        let moves_a = simple_layer1_step(&v, &mut ops, 0, &p);
        assert!(moves_a.is_empty(), "leg departs in the future -> nothing issued yet");
        assert_eq!(committed_out(&ops, 0), 30, "the wave's ships are reserved on the source");
        assert_eq!(spare(&v, &ops, 0, &p), 60, "spare = 100 - floor(10) - reserved(30)");
        // Re-decide on the same tick/state: the reserved force already covers the target, so NO second
        // wave is committed (the committed ledger replaces the old binary ignore flag).
        let moves_b = simple_layer1_step(&v, &mut ops, 0, &p);
        assert_eq!(ops.len(), 1, "no double-commit");
        assert!(moves_b.is_empty());
    }

    #[test]
    fn deterministic_same_input_same_output() {
        let p = SimpleParams::default();
        let mut v = TV::new(&[
            (PosOwner::Me, 80, 0),
            (PosOwner::Neutral, 0, 0),
            (PosOwner::Enemy, 0, 12),
            (PosOwner::Me, 40, 0),
        ]);
        v.resist[1] = 900.0;
        v.resist[2] = 1800.0;
        let mut ops_a = Vec::new();
        let mut ops_b = Vec::new();
        let ma = simple_layer1_step(&v, &mut ops_a, 7, &p);
        let mb = simple_layer1_step(&v, &mut ops_b, 7, &p);
        assert_eq!(ops_a, ops_b);
        assert_eq!(ma, mb);
    }
}
