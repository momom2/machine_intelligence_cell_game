//! **Simple** — the stateful colonizer (the campaign "Simple" enemy).
//!
//! Unlike the other rosters (pure functions of the observed world, carried by the stateless
//! [`crate::controller::AiController`]), Simple keeps a **persistent per-struct departure ledger**
//! across decision ticks, so it is driven by its own [`SimpleController`] — the same pattern the
//! [`crate::counter::CounterController`] uses.
//!
//! # Two layers
//!
//! * **Layer 1 (intra-structure, the heart):** for each struct the seat has presence on, run the four
//!   phases [`Phase::EXPIRE`]→DEFEND→PLAN+COMMIT→DISPATCH against that struct's ledger
//!   ([`simple_layer1_step`]). It sizes captures by a **minimum** (the bar to start a front) and a
//!   **maximum** (how much is worth committing), secures up to [`SimpleParams::fronts`] fronts at a
//!   time then deepens them, and *staggers* each taskforce's departures so the ships **land
//!   together** (synchronised arrival, realised purely from AI state — the engine never holds ships,
//!   so a committed-but-not-yet-due leg merely *reserves* its ships in the ledger until its tick).
//!
//! * **Layer 2 (inter-structure, simplified):** from each **fully-owned, uncontested** structure, push the
//!   storage along a **funneling DAG** toward the worlds that need ships. No ledger,
//!   no retreat, no staggering — just a steady feed toward the fight. On a single-struct level (every
//!   campaign mission Simple plays today) this is a no-op and the Layer-1 ledger is the whole game.
//!
//! # OVERWHELM
//!
//! `OVERWHELM(n) = max(ratio·n, n + add)` (default `max(1.2·n, n+20)`) — the force needed to beat `n`
//! defenders. `OVERWHELM(0) = add = min_wave`, so an undefended neutral costs exactly the floor wave.
//! Every threshold rounds **up** (`ceil`): a force floor must never round below its requirement.
//!
//! # Storage as a sub (owner redesign, 2026-07-08)
//!
//! Simple has **no storage-specific policy**. Its view (`Layer1View::direct`) presents the
//! ownerless reserve like any other position — owned by the staged-ship majority (ties/empty =
//! neutral), priced by a virtual resistance proportional to its capacity — so everything falls
//! out of the ordinary phases: a foe-held reserve is a front to fund (or STAGE FOR SIEGE toward,
//! massing until its OVERWHELM bar is fundable); an unclaimed reserve is the guaranteed
//! least-attractive colonization target, "claimed" with a floor wave only when nothing else
//! remains; the seat's own staged stock is ordinary spare for `pull`.
//!
//! # Determinism
//!
//! The ledger is `Vec`-only (never a `HashMap`); every phase iterates subs/ops/legs by ascending
//! index; every nearest/candidate sort breaks ties by ascending id; force math is done in `f32` then
//! `ceil`-ed once; the clock is the single absolute `world.tick`. So identical inputs evolve the
//! ledger identically and `World::state_hash` stays bit-identical on replay. The controller is built
//! fresh per match, so the ledger never leaks across matches.

use layer1::{Faction, SimParams};
use world::{FleetOrder, World, WorldParams};

use crate::adapters::Layer1View;
use crate::greedy::{PosOwner, PositionView};
use layer1::FractionBucket;

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
    /// FORTRESS gauntlet toll: extra attackers required per **manning ship** of each rival
    /// fortress whose overwatch zone a leg's straight path crosses (the fixed price of walking
    /// the kill zone — see `PositionView::overwatch_toll`; the target fort's own zone counts —
    /// the approach walks into it); a leg routed via an owned teleporter walks nothing and pays
    /// nothing. `0.0` disables (a fortress-naive Simple). Default 1.5 (owner retune, 2026-07-08).
    pub fort_toll: f32,
    /// FORTRESS **prod-equivalent prior** (owner formula, 2026-07-08): in the neutral ranking
    /// a full-coverage fort counts as this much production in the denominator —
    /// `prod + fort_prod_equiv_value·coverage·fort_tuning_constant + (gate term)`. Coverage is
    /// the fort's complete-graph path-coverage fraction (value ∝ how much of the map's
    /// movement space it commands).
    pub fort_prod_equiv_value: f32,
    /// TELEPORTER prod-equivalent prior: a full-savings gate counts as this much production in
    /// the same denominator (`gate_prod_equiv_value·savings·gate_tuning_constant`). Savings is
    /// the complete-graph travel fraction the gate would save — a static map property,
    /// unrelated to the live `via_gate` routing.
    pub gate_prod_equiv_value: f32,
    /// Multiplier on the fort prior's coverage term (owner: a tuning knob, expect retunes).
    pub fort_tuning_constant: f32,
    /// Multiplier on the gate prior's savings term (owner: a tuning knob, expect retunes).
    pub gate_tuning_constant: f32,
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
    /// Weight on the **distance** term when ranking neutral capture targets, per **world unit**
    /// of Euclidean distance from the nearest owned sub (owner formula, 2026-07-08 — ranking
    /// distance is raw geometry; travel *ticks* and teleporter routing still govern funding
    /// and scheduling). The full neutral priority (lower = attack first):
    /// `(res + 1800) / (prod_raw + fort_prior + gate_prior) + neutral_dist_weight · dist`.
    pub neutral_dist_weight: f32,
    /// **CONSOLIDATE** trigger margin (owner rule): with nothing to do — no fundable front, no
    /// mop-up, no ledger ops, no defensive fire — a sub more than this many ships **over its
    /// capacity** ships its whole surplus (≥ this many ships) to the nearest friendly sub,
    /// instead of letting it rot under per-sub attrition while every OVERWHELM bar sits
    /// unfundable from any single garrison.
    pub consolidate_margin: u32,
    /// **Adjacency-restricted attacks** (owner variant, 2026-07-08 — Far far away's ring):
    /// with `Some(range)` and at least one owned sub on a struct, only targets within `range`
    /// world units of an owned sub are ever planned there — expansion crawls neighbour to
    /// neighbour instead of launching waves across the middle. With no owned sub on the
    /// struct (a fresh invasion) the restriction is moot and everything is fair game.
    /// `None` (the default) = the classic unrestricted Simple.
    pub adjacency_range: Option<f32>,
}

impl Default for SimpleParams {
    fn default() -> Self {
        SimpleParams {
            floor: 10,
            min_wave: 20,
            fronts: 3,
            fort_toll: 1.5,
            fort_prod_equiv_value: 5.0,
            gate_prod_equiv_value: 5.0,
            fort_tuning_constant: 2.0,
            gate_tuning_constant: 1.0,
            overwhelm_ratio: 1.2,
            overwhelm_add: 20,
            neutral_res_divisor: 60.0,
            non_neutral_foe_mult: 2.0,
            neutral_dist_weight: 20.0,
            consolidate_margin: 20,
            adjacency_range: None,
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
    /// This leg's own destination: the op's target for a plain leg; the **owned teleporter**
    /// for the walk-leg of a gate-routed pair (its partner hop-leg then runs gate → target).
    pub(crate) tgt: usize,
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

/// The **bar to start a front** on `t`: an undefended target costs `OVERWHELM(0)`; defended
/// ground — including a **contested neutral** another seat is camping or racing for — costs
/// `OVERWHELM(present+incoming foes)`. (`foes == 0` on an undefended neutral, so `OVERWHELM(0)`
/// falls out naturally; pricing a camped neutral as undefended was a bug.)
fn minimum<V: PositionView>(view: &V, t: usize, p: &SimpleParams) -> u32 {
    p.overwhelm(foes(view, t) as f32)
}

/// The **most worth committing** to `t`: enough to grind a neutral's resistance out
/// (`OVERWHELM(resistance/div)`) — or, if rivals are camped on it, enough to also crush them
/// (the `max` keeps `minimum <= maximum` for any `non_neutral_foe_mult >= 1`); an enemy sub
/// costs `OVERWHELM(mult·foes)`.
fn maximum<V: PositionView>(view: &V, t: usize, p: &SimpleParams) -> u32 {
    if view.info(t).owner == PosOwner::Neutral {
        let grind = view.resistance(t) / p.neutral_res_divisor;
        let camped = p.non_neutral_foe_mult * foes(view, t) as f32;
        p.overwhelm(grind.max(camped))
    } else {
        p.overwhelm(p.non_neutral_foe_mult * foes(view, t) as f32)
    }
}

/// Ships from `s` reserved by UNSENT legs (the departure ledger — they have not left yet but are
/// spoken for, so `spare` must not re-spend them).
fn committed_out(ops: &[Op], s: usize) -> u32 {
    ops.iter().flat_map(|op| &op.legs).filter(|l| !l.sent && l.src == s).map(|l| l.count).sum()
}

/// My force already committed toward `t` by UNSENT legs — counted by each leg's **own**
/// destination, so a gate-routed op contributes its ships once (via the final hop-leg), never
/// once per leg. (In-transit, sent legs are counted by `incoming_mine` instead — no double
/// count.)
fn committed_in(ops: &[Op], t: usize) -> u32 {
    ops.iter().flat_map(|op| &op.legs).filter(|l| !l.sent && l.tgt == t).map(|l| l.count).sum()
}

/// Surplus `s` can still give an attack: idle above the floor, minus what is already reserved.
/// **A fortress's floor IS its capacity** (owner rule): troops never leave a fort unless it is
/// over capacity — and then no more than the surplus. The wall is never milked to fund fronts.
fn spare<V: PositionView>(view: &V, ops: &[Op], s: usize, p: &SimpleParams) -> u32 {
    let floor = view.fort_capacity(s).unwrap_or(p.floor);
    view.info(s).my_ships.saturating_sub(floor).saturating_sub(committed_out(ops, s))
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

/// Travel `a`→`b` in **ticks**: the direct `transit_ticks`, or faster **via an owned
/// teleporter** when one beats walking (`via_gate`); `u64::MAX` if unreachable. The same
/// routing the commit step turns into two chained legs, so planned times are realised times.
fn travel<V: PositionView>(view: &V, a: usize, b: usize) -> u64 {
    let direct = view.transit_ticks(a, b).unwrap_or(u64::MAX);
    match view.via_gate(a, b) {
        Some((_, vt)) => direct.min(vt),
        None => direct,
    }
}

/// The owned teleporter a leg `s → t` would route through (strictly faster than walking), or
/// `None` for a direct leg — shared by the toll pricing and the two-leg commit so they can
/// never disagree.
fn gate_route<V: PositionView>(view: &V, s: usize, t: usize) -> Option<usize> {
    let direct = view.transit_ticks(s, t).unwrap_or(u64::MAX);
    view.via_gate(s, t).and_then(|(g, vt)| (vt < direct).then_some(g))
}

/// Least travel from any owned sub to `t` (mop-up's holdout ordering; funding/scheduling use
/// per-leg `travel` directly).
fn nearest_owned_travel<V: PositionView>(view: &V, t: usize) -> u64 {
    (0..view.len())
        .filter(|&s| view.info(s).owner == PosOwner::Me && view.reachable(s, t))
        .map(|s| travel(view, s, t))
        .min()
        .unwrap_or(u64::MAX)
}

/// Least **Euclidean distance** (world units) from any owned sub to `t` — the PLAN ranking's
/// distance term (owner formula, 2026-07-08). Deliberately raw geometry, NOT `travel`: travel
/// is in ticks (undock + speed) and teleporter-aware, which would clash units with
/// `fort_reach` in the enemy ranking; routing still shapes funding and landing times.
fn nearest_owned_distance<V: PositionView>(view: &V, t: usize) -> f32 {
    (0..view.len())
        .filter(|&s| view.info(s).owner == PosOwner::Me && view.reachable(s, t))
        .filter_map(|s| view.distance(s, t))
        .fold(f32::MAX, f32::min)
}

/// PLAN ordering score for a candidate `t` (**lower = attack first**; owner formulas,
/// 2026-07-08). A **neutral** is ranked by capture cost-effectiveness with prod-equivalent
/// priors for the special subs:
/// `(res + 1800) / (prod_raw + fort_pe·coverage·fort_tc + gate_pe·savings·gate_tc) + w·dist`
/// — the `+1800` (half a default sub's grind) keeps a resistance-0 site from reading as free,
/// and a commanding fort / shortening gate earns its keep as *virtual production*. An
/// **enemy** ranks by `distance − fort_reach`: nearest first, a fort counted as if the
/// attacker already stood at its zone edge (its guns reach out — so does its urgency).
fn candidate_priority<V: PositionView>(view: &V, t: usize, p: &SimpleParams) -> f32 {
    let dist = nearest_owned_distance(view, t);
    if view.info(t).owner == PosOwner::Neutral {
        let worth = view.production_raw(t)
            + p.fort_prod_equiv_value * view.fort_coverage(t) * p.fort_tuning_constant
            + p.gate_prod_equiv_value * view.gate_savings(t) * p.gate_tuning_constant;
        // Floor keeps a 0-prod, no-quality site finite but astronomically last (it IS
        // worthless ground — the storage node's virtual resistance lands here too).
        (view.resistance(t) + 1800.0) / worth.max(1e-3) + p.neutral_dist_weight * dist
    } else {
        dist - view.fort_reach(t)
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

/// Pull toward `t` until the gathered count covers `base` **plus the fortress gauntlet tolls**
/// of the chosen legs: a leg whose straight path crosses a rival fortress's overwatch zone is
/// oversized by [`SimpleParams::fort_toll`] × that zone's manning ships (a fixed price — the
/// fort fires for the crossing regardless of wave size); a leg routed via an owned teleporter
/// walks nothing and pays nothing. A deepen pass (phase B) re-charges the toll for a source the
/// secure pass already used — deliberately: its `base` was computed net of the earlier legs'
/// GROSS counts (toll ships included), so re-adding the toll makes the coalesced leg's total
/// come out to exactly `target + one toll` — the phase-A toll cancels out of the algebra.
/// Sources nearest-travel-first (ties by id; gate routes
/// already shorten `travel`), reserving from `avail` into `legs`. Returns
/// `(gathered, required)` — the caller secures the front iff `gathered >= required`.
/// `charge_toll: false` skips the gauntlet pricing (manning moves onto our own ground).
fn pull<V: PositionView>(
    view: &V,
    t: usize,
    base: u32,
    avail: &mut [u32],
    legs: &mut Vec<(usize, u32)>,
    p: &SimpleParams,
    charge_toll: bool,
) -> (u32, u32) {
    if base == 0 {
        return (0, 0);
    }
    let mut srcs: Vec<usize> = (0..view.len())
        .filter(|&s| {
            s != t
                && avail[s] > 0
                && view.reachable(s, t)
                // ADJACENCY-RESTRICTED variant: LEGS obey the range too (owner fix,
                // 2026-07-08 — restricting only the target let far garrisons fund a front
                // by flying straight across the middle). Funding comes from the target's
                // neighbourhood; distant surplus relays in via STAGE FOR SIEGE instead.
                && p.adjacency_range
                    .is_none_or(|r| view.distance(s, t).is_some_and(|d| d <= r))
        })
        .collect();
    srcs.sort_by(|&a, &b| travel(view, a, t).cmp(&travel(view, b, t)).then(a.cmp(&b)));
    let mut got = 0u32;
    let mut need = base;
    for s in srcs {
        if got >= need {
            break;
        }
        let toll = if charge_toll {
            // A routed leg teleports the dangerous stretch — but the WALK to the gate still
            // runs whatever gauntlets lie on it (the hop itself crosses nothing). Direct legs
            // pay for the full path. `overwatch_toll` already sums every crossing fort, and
            // tolls accumulate per leg, so multiple forts price additively throughout.
            let walked = match gate_route(view, s, t) {
                Some(g) => view.overwatch_toll(s, g),
                None => view.overwatch_toll(s, t),
            };
            (p.fort_toll * walked as f32).ceil() as u32
        } else {
            0
        };
        let take = avail[s].min((need + toll).saturating_sub(got));
        if take == 0 {
            continue;
        }
        need += toll;
        avail[s] -= take;
        legs.push((s, take));
        got += take;
    }
    (got, need)
}

// =====================================================================================
// Layer 1 — the four phases (the heart). Pure: mutates `ops`, returns the moves to ISSUE.
// =====================================================================================

/// Run one decision tick of the Layer-1 program for a single struct against its `ops` ledger,
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
    // An op with UNSENT legs is retained even past `land_at`: decision ticks are sparse, so a
    // leg whose `[depart_at, land_at)` window fell entirely between two decision ticks has not
    // fired yet — DISPATCH (phase 3) fires it (late) this tick instead of silently evaporating
    // it; the op is dropped on the next decision tick once fully sent.
    ops.retain(|op| now < op.land_at || op.legs.iter().any(|l| !l.sent));

    // ---- (1) DEFEND: flee the overwhelmed, pin the contested. ----
    let mut fleeing = vec![false; n];
    let mut pinned = vec![false; n];
    for s in 0..n {
        let info = view.info(s);
        if info.owner != PosOwner::Me {
            continue;
        }
        // A fortress NEVER evacuates (its floor is its capacity; troops leave only as
        // surplus): under threat the wall garrison is PINNED instead — it holds its
        // extended-range ground and fights, and its unsent outbound legs are cancelled below.
        if view.fort_capacity(s).is_some() {
            if info.enemy_ships > 0 || (over_threat(view, s, p) && info.my_ships > 0) {
                pinned[s] = true;
            }
            continue;
        }
        if over_threat(view, s, p) && info.my_ships > 0 {
            // A "flee" with no garrison is meaningless — and marking an EMPTY position as
            // fleeing would permanently veto the mop-up against a holdout massing there.
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

    // ---- (1b) MAN THE FORTS. While an expansion is ongoing (the ledger holds an active op —
    // Simple is not in dire need of ships), every owned fortress is topped up to its capacity
    // BEFORE the fronts are funded (a fort manned only after the conquest is manned too late).
    // Quiet or starved structs (no ops) keep only the regular floor and the wall stands down.
    // Immediate moves — reinforcing our own ground needs no staggering — and no gauntlet toll.
    if !ops.is_empty() {
        for f in 0..n {
            let Some(cap) = view.fort_capacity(f) else { continue };
            if fleeing[f] {
                continue;
            }
            // While manned, the wall is wall-duty, not wave spare.
            avail[f] = 0;
            let have = view.info(f).my_ships + view.incoming_mine(f) + committed_in(ops, f);
            let deficit = cap.saturating_sub(have);
            if deficit == 0 {
                continue;
            }
            let mut man_legs: Vec<(usize, u32)> = Vec::new();
            pull(view, f, deficit, &mut avail, &mut man_legs, p, false);
            for (s, c) in man_legs {
                moves.push(Move { src: s, tgt: f, count: c });
            }
        }
    }

    // ---- (2) PLAN + COMMIT: secure FRONTS minimums, then deepen toward maximums. ----

    let mut candidates: Vec<usize> = (0..n)
        .filter(|&t| {
            view.info(t).owner != PosOwner::Me
                && (0..n).any(|s| view.info(s).owner == PosOwner::Me && view.reachable(s, t))
        })
        .collect();
    // ADJACENCY-RESTRICTED variant (owner, 2026-07-08 — see `SimpleParams::adjacency_range`):
    // with at least one owned sub here, only targets within range of an owned sub are ever
    // planned — the expansion crawls neighbour to neighbour, never across the middle.
    if let Some(range) = p.adjacency_range {
        let owned: Vec<usize> = (0..n).filter(|&s| view.info(s).owner == PosOwner::Me).collect();
        if !owned.is_empty() {
            candidates.retain(|&t| {
                owned.iter().any(|&s| view.distance(s, t).is_some_and(|d| d <= range))
            });
        }
    }
    // Neutral before Enemy; within a group ascending by `candidate_priority` (a neutral by
    // prod-equiv cost-effectiveness + distance, an enemy by distance − fort reach); ties by id.
    candidates.sort_by(|&a, &b| {
        let na = (view.info(a).owner != PosOwner::Neutral) as u8;
        let nb = (view.info(b).owner != PosOwner::Neutral) as u8;
        na.cmp(&nb)
            .then(
                candidate_priority(view, a, p)
                    .partial_cmp(&candidate_priority(view, b, p))
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
            let (got, required) = pull(view, t, to_min, &mut avail, &mut legs, p, true);
            if got < required {
                for (s, c) in &legs {
                    avail[*s] += *c; // roll back — couldn't field the minimum (+ gauntlet tolls).
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
                pull(view, t, want_more, &mut avail, &mut entry.1, p, true);
            }
        }
    }

    // ---- (2b) MOP-UP: no capturable work left, but enemy ships remain — stragglers brawling
    // on my ground, or a remnant in a reserve I still out-own (a foe-MAJORITY reserve is a
    // regular PLAN candidate instead — see STORAGE AS A SUB in the adapter). Designate where
    // they reside as a target to OVERWHELM; when even the whole struct's spare cannot fund
    // that bar, mass everything available and send it anyway — one big wave is the least
    // inefficient battle the square law allows (per the design: Simple finishes the job, it
    // does not besiege forever; the Layer-2 funnel keeps feeding this struct meanwhile).
    if plan.is_empty() && !fleeing.iter().any(|&f| f) {
        // (Mop-up waits while an evacuation is in progress this tick: consolidate first,
        // counter-attack from strength next decision — never evacuate a position and feed
        // ships back into the same grinder in one breath.)
        let any_eligible = candidates
            .iter()
            .any(|&t| maximum(view, t, p).saturating_sub(our_force(view, ops, t)) > 0);
        if !any_eligible {
            // Holdouts, nearest-to-my-ground first (ties by id; same ranking flavour as enemy
            // fronts).
            let mut holdouts: Vec<usize> = (0..n).filter(|&s| foes(view, s) > 0).collect();
            holdouts.sort_by_key(|&s| (nearest_owned_travel(view, s), s));
            if let Some(&t) = holdouts.first() {
                let of = our_force(view, ops, t);
                let bar = minimum(view, t, p).saturating_sub(of);
                if bar > 0 {
                    let total: u32 = avail.iter().sum();
                    let want = bar.min(total); // all of capacity when the bar is unfundable
                    if want > 0 {
                        let mut legs: Vec<(usize, u32)> = Vec::new();
                        let (got, _) = pull(view, t, want, &mut avail, &mut legs, p, true);
                        if got > 0 {
                            plan.push((t, legs));
                        }
                    }
                }
            }
        }
    }

    // ---- (2c) STAGE FOR SIEGE (owner fix, 2026-07-08 — "Simple does not concentrate"):
    // capturable candidates EXIST but nothing was fundable this decision (the top target's
    // OVERWHELM bar exceeds what the allowed sources can field — e.g. a big player garrison,
    // doubly so under the adjacency leash, which shrinks the funding pool to the target's
    // neighbourhood). Rather than stalling forever, every quiet garrison's surplus above the
    // floor RELAYS toward the **mustering ground** — the owned sub nearest the top target —
    // one adjacency hop at a time (directly, in the unrestricted variant). Force accumulates
    // there decision over decision until the front's minimum becomes fundable and the normal
    // planner takes over. Supersedes the old wandering consolidate in this case (nearest-
    // friendly hops massed nowhere in particular).
    if plan.is_empty() && !candidates.is_empty() && ops.is_empty() && !fleeing.iter().any(|&f| f)
    {
        let target = candidates[0]; // the sort above: cheapest / nearest first
        let muster = (0..n)
            .filter(|&s| {
                view.info(s).owner == PosOwner::Me
                    && view.fort_capacity(s).is_none() // the wall is wall-duty, not a rally
            })
            .min_by_key(|&s| (travel(view, s, target), s));
        if let Some(m) = muster {
            for s in 0..n {
                let info = view.info(s);
                if s == m
                    || info.owner != PosOwner::Me
                    || view.fort_capacity(s).is_some()
                    || fleeing[s]
                    || pinned[s]
                {
                    continue;
                }
                let surplus = info.my_ships.saturating_sub(p.floor);
                if surplus == 0 {
                    continue;
                }
                // Next hop toward the muster: with the adjacency leash, the reachable owned
                // sub within range that is strictly closer to the muster (nearest to it, ties
                // by id) — the surplus relays ring-wise; unrestricted, straight to the muster.
                let hop = match p.adjacency_range {
                    None => Some(m),
                    Some(range) => {
                        let dm = view.distance(s, m).unwrap_or(f32::MAX);
                        if dm <= range {
                            Some(m)
                        } else {
                            (0..n)
                                .filter(|&h| {
                                    h != s
                                        && view.info(h).owner == PosOwner::Me
                                        && view.fort_capacity(h).is_none()
                                        && view.reachable(s, h)
                                        && view.distance(s, h).is_some_and(|d| d <= range)
                                        && view.distance(h, m).is_some_and(|d| d < dm)
                                })
                                .min_by(|&a, &b| {
                                    let da = view.distance(a, m).unwrap_or(f32::MAX);
                                    let db = view.distance(b, m).unwrap_or(f32::MAX);
                                    da.partial_cmp(&db)
                                        .unwrap_or(std::cmp::Ordering::Equal)
                                        .then(a.cmp(&b))
                                })
                        }
                    }
                };
                if let Some(h) = hop {
                    moves.push(Move { src: s, tgt: h, count: surplus });
                }
            }
        }
    }

    // ---- (2d) CONSOLIDATE (owner rule): NOTHING to do — no capture candidates at all, no
    // mop-up, no ledger ops, no defensive fire — yet garrisons keep growing past their caps.
    // Any owned sub more than `consolidate_margin` over its capacity ships its whole surplus
    // (≥ the margin) to the NEAREST friendly sub. (The unfundable-front case that used to
    // land here is now the directed STAGE FOR SIEGE above.)
    if plan.is_empty()
        && candidates.is_empty()
        && ops.is_empty()
        && !fleeing.iter().any(|&f| f)
        && !pinned.iter().any(|&x| x)
    {
        for s in 0..n {
            let info = view.info(s);
            if info.owner != PosOwner::Me {
                continue;
            }
            let cap = view.capacity(s);
            if info.my_ships <= cap + p.consolidate_margin {
                continue;
            }
            let surplus = info.my_ships - cap;
            // Nearest friendly sub; lowest travel, ties by id. (A majority-owned reserve is a
            // legitimate destination — storage IS the rally stock; its huge capacity keeps it
            // from ever being a consolidation SOURCE.)
            let mut best: Option<(u64, usize)> = None;
            for t in 0..n {
                if t == s
                    || view.info(t).owner != PosOwner::Me
                    || !view.reachable(s, t)
                {
                    continue;
                }
                let d = travel(view, s, t);
                if best.map_or(true, |(bd, bt)| (d, t) < (bd, bt)) {
                    best = Some((d, t));
                }
            }
            if let Some((_, t)) = best {
                moves.push(Move { src: s, tgt: t, count: surplus });
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
        let mut op_legs: Vec<Leg> = Vec::new();
        for (s, c) in coalesced {
            if let Some(g) = gate_route(view, s, t) {
                // TWO chained legs — walk to the owned gate, then the instant hop — so the
                // plan's travel times (both undocks included: the walk-leg's own, then the
                // hop-leg's) are the times the ships actually realise.
                let hop_departs = land_at.saturating_sub(travel(view, g, t));
                op_legs.push(Leg {
                    src: s,
                    tgt: g,
                    count: c,
                    depart_at: hop_departs.saturating_sub(travel(view, s, g)),
                    sent: false,
                });
                op_legs.push(Leg { src: g, tgt: t, count: c, depart_at: hop_departs, sent: false });
            } else {
                op_legs.push(Leg {
                    src: s,
                    tgt: t,
                    count: c,
                    depart_at: land_at.saturating_sub(travel(view, s, t)),
                    sent: false,
                });
            }
        }
        ops.push(Op { target: t, land_at, legs: op_legs });
    }

    // ---- (3) DISPATCH: fire the staggered legs that have come due. ----
    for op in ops.iter_mut() {
        for leg in op.legs.iter_mut() {
            if !leg.sent && now >= leg.depart_at && !fleeing[leg.src] && !pinned[leg.src] {
                moves.push(Move { src: leg.src, tgt: leg.tgt, count: leg.count });
                leg.sent = true;
            }
        }
    }

    moves
}

// =====================================================================================
// Layer 2 — the simplified push (no ledger / retreat / stagger).
// =====================================================================================

/// From each fully-owned, uncontested structure, send the surplus toward the nearest **frontline**
/// struct (any reachable struct that is not a quiet Me rear: a foe present, contested, or not mine).
/// Is this struct a Layer-2 funnel **sink** — a world that still *needs* ships? True when any
/// position is not the seat's (ground left to take: a neutral or rival sub — and, under
/// STORAGE AS A SUB, an UNCLAIMED or foe-majority reserve: the world demands ships until the
/// seat's staged plurality "claims" the stock) or any foe is present/incoming (a fight in
/// progress, a holdout in the reserve). A fully-owned, quiet world is UNDEMANDING:
/// its Layer-1 program idles, its production surplus auto-diverts into struct storage, and the
/// funnel sends that storage onward. Pure read of the view.
fn struct_is_sink<V: PositionView>(view: &V) -> bool {
    let n = view.len();
    (0..n).any(|s| view.info(s).owner != PosOwner::Me) || (0..n).any(|s| foes(view, s) > 0)
}

/// Layer 2 — the **funneling DAG** (replaces the old fully-owned→frontline surplus push, which
/// was also blind to reserve-staged ships). Every sink ([`struct_is_sink`]) is a BFS source;
/// every undemanding world points one hop "downhill" along the lane graph toward its nearest
/// sink (hop-count distance, ascending-id tie-breaks — a DAG, since distance strictly falls)
/// and each decision tick ships **100% of its staged storage** along that edge:
/// `FractionBucket::All` and the reserve-first fleet draw mean exactly "everything in structure
/// storage, inner garrisons untouched" (a bare no-reserve structure falls back to the legacy
/// whole-struct draw — harness fixtures only; every campaign struct has a reserve). Relay
/// worlds receive into their reserve and pass it on next decision; ships pool in the sink's
/// reserve where its Layer-1 planner spends them. No demand arithmetic and no ledger — demand
/// fluctuates on Layer-1 timescales, so Layer 2 just keeps the rivers flowing (per the design:
/// no optimality requirement). Deterministic; zero-launch orders are junk-safe no-ops.
fn funnel_orders(world: &World, seat: layer1::Faction, sinks: &[bool]) -> Vec<FleetOrder> {
    let n = sinks.len();
    let mut dist: Vec<u32> = sinks.iter().map(|&s| if s { 0 } else { u32::MAX }).collect();
    let mut frontier: Vec<usize> = (0..n).filter(|&i| sinks[i]).collect();
    let mut d = 0u32;
    while !frontier.is_empty() {
        d += 1;
        let mut next: Vec<usize> = Vec::new();
        for i in 0..n {
            if dist[i] == u32::MAX && frontier.iter().any(|&f| world.are_connected(i, f)) {
                dist[i] = d;
                next.push(i);
            }
        }
        frontier = next;
    }
    let mut orders = Vec::new();
    for i in 0..n {
        if sinks[i] || dist[i] == u32::MAX || dist[i] == 0 {
            continue; // sinks consume; unreachable-from-any-sink worlds have nowhere to send
        }
        if world.structs[i].interior.ship_count(seat) == 0 {
            continue; // nothing of ours to funnel from here
        }
        let hop = (0..n).find(|&j| dist[j] != u32::MAX && dist[j] + 1 == dist[i] && world.are_connected(i, j));
        if let Some(j) = hop {
            orders.push(FleetOrder::new(i, j, FractionBucket::All));
        }
    }
    orders
}

// =====================================================================================
// The controller (the stateful host — mirrors CounterController).
// =====================================================================================

/// The stateful driver for the **Simple** seat: owns the per-struct departure ledger and runs both
/// layers each decision tick. Non-`Copy` (it accumulates state), built once per match.
#[derive(Debug, Clone)]
pub struct SimpleController {
    /// The seat Simple plays.
    pub seat: Faction,
    /// Policy dials.
    p: SimpleParams,
    /// The persistent departure ledger, indexed by struct id. Resized to the world's struct count on
    /// first use; an entry is cleared if the seat loses all presence on that structure.
    operations: Vec<Vec<Op>>,
}

/// The last-stand sweep (see `decide_and_apply`), owner-specced targeting (2026-07-07): in
/// every struct holding foes, `seat`'s idle stacks — wherever they sit: the reserve,
/// fortresses, teleporters, captured ground — attack **overwhelmable** targets
/// (`OVERWHELM(F) = max(ratio·F, F + add)` against each target's foes present + inbound):
///
/// 1. **Distinct first**: stacks (largest first) each take the nearest still-unclaimed target
///    they can overwhelm *alone* — one stack per target where possible.
/// 2. **Merge if needed**: stacks that can solo nothing pool up and jointly hit the biggest
///    target the pool overwhelms (preferring an unclaimed one); if even the pool overwhelms
///    nothing but solo attacks are under way, the pool reinforces the easiest claimed kill.
/// 3. **All-in if hopeless**: when *no* target is overwhelmable at all, everything charges
///    one target picked pseudo-randomly (a pure hash of tick × seat × pool — no RNG drawn).
///
/// Targets are foe-owned subs, or foe-staged subs when no foe ground remains; stacks already
/// on foe ground or sharing their sub with staged foes are in contact and left to their work.
/// Deterministic; re-issued each decision tick so stragglers keep joining. Returns ships ordered.
fn last_stand_moves(world: &mut World, seat: Faction, dials: &SimpleParams) -> usize {
    let overwhelms = |m: usize, f: usize| -> bool {
        m as f32 >= (dials.overwhelm_ratio * f as f32).max((f + dials.overwhelm_add as usize) as f32)
    };
    let tick = world.tick;
    let mut moved = 0usize;
    for p in 0..world.structs.len() {
        let orders: Vec<(usize, usize)> = {
            let st = &world.structs[p].interior;
            let n = st.subs.len();
            let mut my_idle = vec![0usize; n];
            let mut foe_at = vec![0usize; n]; // foes present + inbound, per sub
            for sh in &st.ships {
                if !sh.alive || sh.drift_remaining > 0 {
                    continue;
                }
                if sh.faction == seat {
                    if sh.target.is_none() && sh.home < n {
                        my_idle[sh.home] += 1;
                    }
                } else if sh.faction.is_foe_of(seat) {
                    let at = sh.target.unwrap_or(sh.home);
                    if at < n {
                        foe_at[at] += 1;
                    }
                }
            }
            // Targets: foe ground, else foe-staged subs (e.g. a reserve remnant).
            let mut targets: Vec<usize> =
                (0..n).filter(|&t| st.subs[t].owner.is_foe_of(seat)).collect();
            if targets.is_empty() {
                targets = (0..n).filter(|&t| foe_at[t] > 0).collect();
            }
            // Stacks: idle, not already in contact (on foe ground / sharing with staged foes).
            let mut stacks: Vec<(usize, usize)> = (0..n)
                .filter(|&s| {
                    my_idle[s] > 0 && !st.subs[s].owner.is_foe_of(seat) && foe_at[s] == 0
                })
                .map(|s| (s, my_idle[s]))
                .collect();
            if targets.is_empty() || stacks.is_empty() {
                Vec::new()
            } else {
                stacks.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0))); // largest first
                let mut orders: Vec<(usize, usize)> = Vec::new();
                let mut taken: Vec<usize> = Vec::new();
                let mut leftovers: Vec<usize> = Vec::new();
                // (1) Distinct solo assignments: nearest unclaimed overwhelmable target.
                for &(s, m) in &stacks {
                    let pick = targets
                        .iter()
                        .copied()
                        .filter(|t| !taken.contains(t) && *t != s && overwhelms(m, foe_at[*t]))
                        .min_by(|&a, &b| {
                            st.subs[a]
                                .pos
                                .dist(st.subs[s].pos)
                                .partial_cmp(&st.subs[b].pos.dist(st.subs[s].pos))
                                .unwrap_or(std::cmp::Ordering::Equal)
                                .then(a.cmp(&b))
                        });
                    match pick {
                        Some(t) => {
                            orders.push((s, t));
                            taken.push(t);
                        }
                        None => leftovers.push(s),
                    }
                }
                // (2) Merge the leftovers onto one jointly-overwhelmable target.
                if !leftovers.is_empty() {
                    let pool: usize = leftovers.iter().map(|&s| my_idle[s]).sum();
                    let joint = |claimed: bool| {
                        targets
                            .iter()
                            .copied()
                            .filter(|t| taken.contains(t) == claimed && overwhelms(pool, foe_at[*t]))
                            .max_by_key(|&t| (foe_at[t], std::cmp::Reverse(t)))
                    };
                    if let Some(t) = joint(false).or_else(|| joint(true)) {
                        orders.extend(leftovers.iter().filter(|&&s| s != t).map(|&s| (s, t)));
                    } else if !taken.is_empty() {
                        // Solo attacks are running — reinforce the easiest claimed kill.
                        let t = *taken
                            .iter()
                            .min_by_key(|&&t| (foe_at[t], t))
                            .expect("taken is non-empty");
                        orders.extend(leftovers.iter().filter(|&&s| s != t).map(|&s| (s, t)));
                    } else {
                        // (3) Nothing overwhelmable anywhere: all-in on a hash-picked target.
                        let seat_byte = match seat {
                            Faction::Player => 1u64,
                            Faction::Ai(i) => 2 + i as u64,
                            _ => 0,
                        };
                        let mut x = tick ^ (seat_byte << 56) ^ ((pool as u64) << 24);
                        x ^= x >> 33;
                        x = x.wrapping_mul(0xff51_afd7_ed55_8ccd);
                        x ^= x >> 33;
                        let t = targets[(x as usize) % targets.len()];
                        orders.extend(leftovers.iter().filter(|&&s| s != t).map(|&s| (s, t)));
                    }
                }
                orders
            }
        };
        for (src, tgt) in orders {
            moved += world.structs[p].interior.issue_order(
                layer1::MoveOrder::new(src, tgt, layer1::FractionBucket::All),
                seat,
            );
        }
    }
    moved
}

impl SimpleController {
    /// A fresh Simple controller for `seat` (default policy dials, empty ledger).
    pub fn new(seat: Faction) -> SimpleController {
        SimpleController { seat, p: SimpleParams::default(), operations: Vec::new() }
    }

    /// The **adjacency-restricted** Simple (see [`SimpleParams::adjacency_range`]): default
    /// dials, but attack targets on a struct where it owns ground are limited to `range`
    /// world units of an owned sub — the [`crate::Roster::SimpleAdjacent`] brain.
    pub fn new_adjacent(seat: Faction, range: f32) -> SimpleController {
        let mut p = SimpleParams::default();
        p.adjacency_range = Some(range);
        SimpleController { seat, p, operations: Vec::new() }
    }

    /// Decide and apply this seat's full turn for the decision tick, in the documented order
    /// (per-struct internals first, then inter-struct fleets). Mutates the ledger and the world.
    /// Returns `(ships moved internally, ships launched in fleets)`.
    pub fn decide_and_apply(&mut self, world: &mut World, sp: &SimParams, wp: &WorldParams) -> (usize, usize) {
        let seat = self.seat;
        let params = self.p;
        let np = world.structs.len();
        if self.operations.len() != np {
            self.operations.resize(np, Vec::new());
        }

        // LAST STAND (owner QoL, 2026-07-07): with no producing sub left anywhere, Simple has
        // no economy to plan around — and its remnants (reserve stockpiles, fort garrisons,
        // gate posts) would otherwise camp behind their floors and doctrines forever, forcing
        // a tedious mop-up. Instead the planner is bypassed wholesale: every idle stack,
        // everywhere — the fort doctrine explicitly included — attacks overwhelmable targets
        // (see `last_stand_moves`). It cannot win the long game anyway; it can still make the
        // ending.
        if !world
            .structs
            .iter()
            .any(|s| s.interior.subs.iter().any(|sub| sub.owner == seat && sub.production > 0))
        {
            for ops in &mut self.operations {
                ops.clear(); // the ledger is meaningless without an economy
            }
            return (last_stand_moves(world, seat, &params), 0);
        }

        let now = world.tick;

        // ---- Layer 1: per-struct ledger -> internal moves (decided against the pre-apply world). ----
        // Look-ahead is the projection-free in-transit influx: `World::sub_influx_for` reads who is
        // inbound to each sub directly off the *current* state (no forward projection is built).
        let mut struct_moves: Vec<(usize, Vec<Move>)> = Vec::new();
        let mut sinks: Vec<bool> = vec![false; np];
        for p in 0..np {
            let st = &world.structs[p].interior;
            let influx = world.sub_influx_for(p, seat, sp, wp);
            let view = Layer1View::direct(st, sp, seat, influx);
            // Sink classification covers EVERY struct (a world we hold nothing on is still a
            // sink — that is how the funnel stages invasions: fleets land in its reserve).
            sinks[p] = struct_is_sink(&view);
            if st.sub_count(seat) == 0 && st.ship_count(seat) == 0 {
                self.operations[p].clear(); // lost the struct — drop its stale ledger.
                continue;
            }
            let moves = simple_layer1_step(&view, &mut self.operations[p], now, &params);
            if !moves.is_empty() {
                struct_moves.push((p, moves));
            }
        }

        // ---- Layer 2: funnel storage from undemanding worlds toward the sinks (the DAG). ----
        let fleet_orders = funnel_orders(world, seat, &sinks);

        // ---- Apply: internals first (exact counts), then fleets. ----
        let mut moved = 0usize;
        for (p, mvs) in struct_moves {
            for m in mvs {
                moved += world.structs[p].interior.issue_order_count(m.src, m.tgt, m.count as usize, seat);
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
        // Special-sub signal overrides (inert by default).
        fort_caps: Vec<Option<u32>>,
        coverage: Vec<f32>,
        savings: Vec<f32>,
        reaches: Vec<f32>,
        tolls: Vec<Vec<u32>>,
        vias: Vec<((usize, usize), (usize, u64))>,
        transit_over: Vec<((usize, usize), u64)>,
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
                fort_caps: vec![None; n],
                coverage: vec![0.0; n],
                savings: vec![0.0; n],
                reaches: vec![0.0; n],
                tolls: vec![vec![0; n]; n],
                vias: Vec::new(),
                transit_over: Vec::new(),
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
            if let Some((_, t)) = self.transit_over.iter().find(|(k, _)| *k == (a, b)) {
                return Some(*t);
            }
            Some((self.xs[a] - self.xs[b]).unsigned_abs().max(1))
        }
        fn overwatch_toll(&self, from: usize, to: usize) -> u32 {
            self.tolls[from][to]
        }
        fn fort_coverage(&self, id: usize) -> f32 {
            self.coverage[id]
        }
        fn gate_savings(&self, id: usize) -> f32 {
            self.savings[id]
        }
        fn fort_capacity(&self, id: usize) -> Option<u32> {
            self.fort_caps[id]
        }
        fn fort_reach(&self, id: usize) -> f32 {
            self.reaches[id]
        }
        fn via_gate(&self, from: usize, to: usize) -> Option<(usize, u64)> {
            self.vias.iter().find(|(k, _)| *k == (from, to)).map(|(_, v)| *v)
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

    // ================= Special subs: fortress / teleporter planner behavior =================

    #[test]
    fn manning_tops_up_the_fort_before_fronts() {
        let p = SimpleParams::default();
        // id0: big source. id1: MY fortress (cap 90, garrison 10). id2: a neutral front.
        let mut v = TV::new(&[(PosOwner::Me, 100, 0), (PosOwner::Me, 10, 0), (PosOwner::Neutral, 0, 0)]);
        v.fort_caps[1] = Some(90);
        // An ongoing expansion (an active op) => not in dire need => man the wall FIRST.
        let mut ops = vec![Op {
            target: 2,
            land_at: 100,
            legs: vec![Leg { src: 0, tgt: 2, count: 5, depart_at: 60, sent: false }],
        }];
        let moves = simple_layer1_step(&v, &mut ops, 0, &p);
        // Manning: deficit 80, pulled from id0 BEFORE any new front is funded.
        assert!(
            moves.contains(&Move { src: 0, tgt: 1, count: 80 }),
            "the fort is topped up to capacity first, got {moves:?}"
        );
        // id0 spare was 100 - floor(10) - committed(5) = 85; manning took 80 => 5 left < min 20:
        // no new front could be funded after the wall was manned.
        assert_eq!(ops.len(), 1, "no new op on top of the seeded one: {ops:?}");
    }

    #[test]
    fn fort_floor_is_capacity_only_surplus_leaves() {
        let p = SimpleParams::default();
        // id0: MY fortress (cap 90) holding 120 — only the 30 surplus is spendable.
        let mut v = TV::new(&[(PosOwner::Me, 120, 0), (PosOwner::Neutral, 0, 0)]);
        v.fort_caps[0] = Some(90);
        assert_eq!(spare(&v, &[], 0, &p), 30, "spare = ships above CAPACITY, not above the floor");
        // At exactly capacity, the fort gives nothing (even though 90 >> the regular floor).
        let mut v2 = TV::new(&[(PosOwner::Me, 90, 0), (PosOwner::Neutral, 0, 0)]);
        v2.fort_caps[0] = Some(90);
        assert_eq!(spare(&v2, &[], 0, &p), 0, "a fort at capacity is never milked");
    }

    #[test]
    fn fort_never_flees_it_pins_and_holds() {
        let p = SimpleParams::default();
        // id0: MY fortress, 50 ships, 200 foes present (over_threat by any margin); id1: a safe
        // owned refuge a plain sub would evacuate to.
        let mut v = TV::new(&[(PosOwner::Me, 50, 200), (PosOwner::Me, 40, 0)]);
        v.fort_caps[0] = Some(90);
        let mut ops = Vec::new();
        let moves = simple_layer1_step(&v, &mut ops, 0, &p);
        assert!(
            !moves.iter().any(|m| m.src == 0),
            "the fort garrison holds — no evacuation, no sourcing: {moves:?}"
        );
    }

    #[test]
    fn consolidates_surplus_when_nothing_to_do() {
        let p = SimpleParams::default();
        // Two owned subs and NO candidates at all. Sub 0 sits 25 over its capacity (default
        // 60) and past the +20 margin: its whole surplus ships to the nearest friendly sub.
        // (The unfundable-front case that used to land here is now STAGE FOR SIEGE below.)
        let v = TV::new(&[(PosOwner::Me, 85, 0), (PosOwner::Me, 40, 0)]);
        let mut ops = Vec::new();
        let moves = simple_layer1_step(&v, &mut ops, 0, &p);
        assert!(ops.is_empty(), "nothing to plan — no op: {ops:?}");
        assert_eq!(moves, vec![Move { src: 0, tgt: 1, count: 25 }], "surplus (my - cap) consolidates");
    }

    #[test]
    fn stages_toward_an_unfundable_front() {
        let p = SimpleParams::default();
        // The only enemy is massively defended (min = OVERWHELM(500), far beyond the spare):
        // instead of stalling (or the old wandering consolidate), every quiet garrison's
        // surplus above the FLOOR relays toward the MUSTER — the owned sub nearest the target
        // (owner rule, 2026-07-08: concentrate force against big garrisons, don't stall).
        let v = TV::new(&[(PosOwner::Me, 85, 0), (PosOwner::Me, 40, 0), (PosOwner::Enemy, 0, 500)]);
        let mut ops = Vec::new();
        let moves = simple_layer1_step(&v, &mut ops, 0, &p);
        assert!(ops.is_empty(), "nothing fundable — no op: {ops:?}");
        assert_eq!(
            moves,
            vec![Move { src: 0, tgt: 1, count: 75 }],
            "surplus above the floor stages at the muster (sub 1, nearest the target)"
        );
    }

    #[test]
    fn no_consolidation_while_a_front_is_fundable() {
        let p = SimpleParams::default();
        // Same surplus, but a cheap neutral exists: the planner commits a front instead —
        // consolidation must stay silent while there is an objective.
        let mut v = TV::new(&[(PosOwner::Me, 85, 0), (PosOwner::Me, 40, 0), (PosOwner::Neutral, 0, 0)]);
        v.resist[2] = 600.0;
        let mut ops = Vec::new();
        let moves = simple_layer1_step(&v, &mut ops, 0, &p);
        assert_eq!(ops.len(), 1, "the neutral front is the objective");
        assert!(
            !moves.iter().any(|m| m.tgt == 1),
            "no friendly consolidation while a front is live: {moves:?}"
        );
    }

    #[test]
    fn no_manning_without_an_ongoing_op() {
        let p = SimpleParams::default();
        let mut v = TV::new(&[(PosOwner::Me, 100, 0), (PosOwner::Me, 10, 0), (PosOwner::Neutral, 0, 0)]);
        v.fort_caps[1] = Some(90);
        let mut ops = Vec::new();
        let moves = simple_layer1_step(&v, &mut ops, 0, &p);
        // No expansion ongoing => the wall keeps only the floor; the planner funds the front
        // instead (and the fort is never topped up this tick).
        assert!(
            !moves.iter().any(|m| m.tgt == 1),
            "no manning while the struct has no active op: {moves:?}"
        );
        assert_eq!(ops.len(), 1, "the neutral front is committed instead");
        assert_eq!(ops[0].target, 2);
    }

    #[test]
    fn gauntlet_toll_inflates_the_wave_or_blocks_it() {
        let mut p = SimpleParams::default();
        p.fronts = 1;
        p.fort_toll = 1.0; // pin the RATE — these tests pin the toll mechanism, not the dial
        // id0 -> id1 walks a rival fortress gauntlet manned by 10 (toll 10 at fort_toll 1.0).
        // The neutral has resistance 0 => min == max == OVERWHELM(0) == 20, so without the toll
        // the wave would be exactly 20; with it, 30.
        let mut v = TV::new(&[(PosOwner::Me, 100, 0), (PosOwner::Neutral, 0, 0)]);
        v.tolls[0][1] = 10;
        let mut ops = Vec::new();
        simple_layer1_step(&v, &mut ops, 0, &p);
        assert_eq!(ops.len(), 1);
        let total: u32 = ops[0].legs.iter().map(|l| l.count).sum();
        assert_eq!(total, 30, "the wave is oversized by the gauntlet toll");

        // Too poor to pay the toll => the front is NOT opened at all (rollback).
        let mut v2 = TV::new(&[(PosOwner::Me, 35, 0), (PosOwner::Neutral, 0, 0)]);
        v2.tolls[0][1] = 10; // need 30, spare = 35 - 10 = 25
        let mut ops2 = Vec::new();
        let moves2 = simple_layer1_step(&v2, &mut ops2, 0, &p);
        assert!(ops2.is_empty(), "can't cover base + toll => no half-priced assault");
        assert!(moves2.is_empty());
    }

    #[test]
    fn gauntlet_toll_is_paid_once_when_the_deepen_pass_reuses_a_source() {
        // Phase A secures min (20) gross 25 (toll 5); phase B deepens toward max (40) from the
        // SAME source. Its pull re-charges the toll, but `want_more` was net of phase A's GROSS
        // legs — the algebra nets to exactly max + ONE toll: the coalesced leg sends 45, the
        // crossing eats 5, and 40 (the maximum) lands. Neither 40 (toll swallowed by the
        // deepening) nor 50 (a genuinely double-charged toll).
        let mut p = SimpleParams::default();
        p.fronts = 1;
        p.fort_toll = 1.0; // pin the rate; the test pins the pay-once algebra
        let mut v = TV::new(&[(PosOwner::Me, 100, 0), (PosOwner::Neutral, 0, 0)]);
        v.resist[1] = 1200.0; // min = OVERWHELM(0) = 20, max = OVERWHELM(20) = 40
        v.tolls[0][1] = 5;
        let mut ops = Vec::new();
        let moves = simple_layer1_step(&v, &mut ops, 0, &p);
        assert_eq!(ops.len(), 1);
        let total: u32 = ops[0].legs.iter().map(|l| l.count).sum();
        assert_eq!(total, 45, "max(40) + one toll(5), the phase-A toll cancels out");
        assert_eq!(moves, vec![Move { src: 0, tgt: 1, count: 45 }]);
    }

    #[test]
    fn gate_route_commits_two_chained_legs() {
        let p = SimpleParams::default();
        // id0 source at x=0; id1 = my gate at x=5; id2 neutral at x=10. The gate hop id1->id2
        // is instant-ish (1 tick) and the route 0->1->2 (5 + 1 = 6) beats walking (10).
        let mut v = TV::new(&[(PosOwner::Me, 100, 0), (PosOwner::Me, 0, 0), (PosOwner::Neutral, 0, 0)]);
        v.xs = vec![0, 5, 10];
        v.transit_over.push(((1, 2), 1));
        v.vias.push(((0, 2), (1, 6)));
        let mut ops = Vec::new();
        let moves = simple_layer1_step(&v, &mut ops, 0, &p);
        assert_eq!(ops.len(), 1);
        let legs = &ops[0].legs;
        assert_eq!(legs.len(), 2, "a routed wave is TWO chained legs: {legs:?}");
        assert_eq!((legs[0].src, legs[0].tgt, legs[0].count), (0, 1, 20), "walk to the gate");
        assert_eq!((legs[1].src, legs[1].tgt, legs[1].count), (1, 2, 20), "the instant hop out");
        assert!(legs[0].depart_at <= legs[1].depart_at, "the walk departs first");
        // land_at = travel(0,2) = 6 (via); hop departs at 6 - 1 = 5; walk at 5 - 5 = 0 => NOW.
        assert_eq!(legs[1].depart_at, 5);
        assert!(moves.contains(&Move { src: 0, tgt: 1, count: 20 }), "the walk-leg fires now");
    }

    #[test]
    fn special_value_bonus_reorders_candidates() {
        let mut p = SimpleParams::default();
        p.fronts = 1; // one front per batch => the ranking decides who is funded first
        // Two equal res-0 neutrals: id1 near (x=1), id2 far (x=3) but commanding the map.
        // Owner formula: priority = (res+1800)/(prod + fort_pe·cov·fort_tc + ...) + 20·dist.
        // id1 = 1800/1 + 20 = 1820; id2 = 1800/(1 + 5·0.5·2) + 60 = 360 -> the fort first.
        let mut v = TV::new(&[(PosOwner::Me, 100, 0), (PosOwner::Neutral, 0, 0), (PosOwner::Neutral, 0, 0)]);
        v.xs = vec![0, 1, 3];
        v.coverage[2] = 0.5;
        let mut ops = Vec::new();
        simple_layer1_step(&v, &mut ops, 0, &p);
        assert_eq!(ops[0].target, 2, "the commanding fort outranks the nearer plain neutral");
    }

    #[test]
    fn enemy_fort_ranks_as_if_at_its_zone_edge() {
        let mut p = SimpleParams::default();
        p.fronts = 1;
        p.fort_toll = 0.0; // isolate the RANKING signal from the gauntlet pricing
        // Two undefended enemy subs: id1 plain at x=10; id2 a fort at x=25 whose overwatch
        // reaches 20 -> effective 5 < 10: the fort's guns make it the more urgent target.
        let mut v = TV::new(&[(PosOwner::Me, 100, 0), (PosOwner::Enemy, 0, 0), (PosOwner::Enemy, 0, 0)]);
        v.xs = vec![0, 10, 25];
        v.reaches[2] = 20.0;
        let mut ops = Vec::new();
        simple_layer1_step(&v, &mut ops, 0, &p);
        assert_eq!(ops[0].target, 2, "distance − fort_reach ranks the fort first");
    }

    #[test]
    fn routed_leg_pays_the_walk_to_gate_toll_only() {
        let mut p = SimpleParams::default();
        p.fronts = 1;
        p.fort_toll = 1.0; // pin the rate; the test pins the routing waiver
        // id0 source, id1 = my gate, id2 = res-0 neutral (min == max == 20). The DIRECT path
        // 0->2 crosses a monstrous gauntlet (toll 50); the WALK to the gate 0->1 crosses a
        // smaller one (toll 10). Routing waives the direct gauntlet but NOT the walk's.
        let mut v = TV::new(&[(PosOwner::Me, 100, 0), (PosOwner::Me, 0, 0), (PosOwner::Neutral, 0, 0)]);
        v.xs = vec![0, 5, 10];
        v.transit_over.push(((1, 2), 1));
        v.vias.push(((0, 2), (1, 6)));
        v.tolls[0][2] = 50;
        v.tolls[0][1] = 10;
        let mut ops = Vec::new();
        simple_layer1_step(&v, &mut ops, 0, &p);
        assert_eq!(ops.len(), 1);
        let total: u32 = ops[0].legs.iter().map(|l| l.count).sum();
        // Two chained legs of 30 each (20 base + 10 walk toll) — NOT 20 (toll fully waived)
        // and NOT 70 (direct gauntlet charged despite the teleport).
        assert_eq!(ops[0].legs.len(), 2);
        assert_eq!(total, 60, "30 ships through both legs: base 20 + the walk's toll 10");
    }
}
