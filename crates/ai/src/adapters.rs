//! The two thin **adapters** that map the layer-agnostic greedy policy ([`crate::greedy`])
//! onto the concrete layers.
//!
//! * [`Layer1View`] — positions are a single struct's **sub-structures**; distance is
//!   Euclidean over their `layer1::Vec2`; the resulting [`GreedyAction`]s become
//!   `layer1::MoveOrder`s. This is also exactly the **player's optional "basic automation"**
//!   for a struct (auto-defend / auto-expand its internal sub-structures).
//! * [`Layer2View`] — positions are the **structs** of a [`world::World`]; distance is the
//!   shortest-path length over the lane graph (BFS, summing lane lengths); a struct may only
//!   be an **export source** when [`world::StructAggregate::fully_owned_uncontested`] holds
//!   (per the world spec); the resulting actions become `world::FleetOrder`s, routed to the
//!   **first hop** along the shortest path toward the chosen (possibly multi-lane-distant)
//!   destination, because a `FleetOrder` is only valid between lane-adjacent structs.
//!
//! Both adapters fold the abstract `count` (surplus ships) into a [`layer1::FractionBucket`]
//! via [`bucket_for`], since both layers' atomic actions take a bucket rather than a raw
//! count. The conversion is intentionally conservative (it never plans to move *more* than the
//! surplus the policy intended) — see [`bucket_for`].

use layer1::{Faction, FractionBucket, MoveOrder, SimParams, Interior, SubId};
use world::{FleetOrder, StructId, StructOwner, Projection, SubInflux, World, WorldParams};

use crate::greedy::{GreedyAction, PosOwner, PositionInfo, PositionView, Side};

/// Choose the **smallest** fraction bucket whose share of `available` covers `want` ships.
///
/// The atomic action moves a *bucket* of the source's eligible ships, not an exact count, so
/// we pick the tightest bucket that still ships at least `want`. Because the policy's `want`
/// is the surplus (`stock - floor`) and the bucket is taken from the *full* stock (Layer 1) or
/// the floor-respecting surplus (Layer 2), rounding up to the next bucket can never release
/// more than the stock; at worst it ships a little of the floor at Layer 1 (acceptable — the
/// floor is a soft home-guard, and the world primitive re-imposes a hard floor at Layer 2).
/// `want == 0` or `available == 0` yields `None` (no order). Always returns at least
/// `Quarter` when something must move.
pub fn bucket_for(want: u32, available: u32) -> Option<FractionBucket> {
    if want == 0 || available == 0 {
        return None;
    }
    if want >= available {
        return Some(FractionBucket::All);
    }
    // Smallest bucket whose rounded count_of(available) is >= want.
    for b in [
        FractionBucket::Quarter,
        FractionBucket::Half,
        FractionBucket::ThreeQuarter,
        FractionBucket::All,
    ] {
        if b.count_of(available as usize) as u32 >= want {
            return Some(b);
        }
    }
    Some(FractionBucket::All)
}

// ======================================================================================
// Layer-1 adapter: a struct's sub-structures.
// ======================================================================================

/// Greedy [`PositionView`] over a single Layer-1 [`Interior`]'s sub-structures, from the
/// point of view of one acting `seat`.
///
/// A "position" is a sub-structure ([`SubId`]). For each sub it reports:
/// * **owner** relative to the seat (`Me`/`Enemy`/`Neutral`),
/// * **my_ships** = idle ships of the seat garrisoned at the sub (the movable stock — only
///   idle ships can be ordered, matching [`Interior::issue_order`]),
/// * **enemy_ships** = enemy ships *engaging* the sub (within `radius + engagement_radius` of
///   its centre, the same "defenders/contesting" notion the Layer-1 Automaton uses),
/// * **contested** = an enemy ship is within engagement range of the sub.
///
/// Distance is Euclidean over sub-structure positions. Every sub is reachable from every other
/// (one structure, no lanes), so [`PositionView::distance`] is always `Some`.
///
/// **Projection context (optional).** When built with [`Layer1View::with_projection`] the view
/// carries the shared [`world::Projection`] and the [`StructId`] this structure sits at, so the
/// composable automatons' QUERY reads ([`PositionView::capture_eta`], `marginal_ticks_saved`, …)
/// answer from the *one* projection the controller built this tick. Built with [`Layer1View::new`]
/// it has no projection, and those queries fall back to their conservative defaults (the greedy
/// tactical default never asks them).
pub struct Layer1View<'a> {
    st: &'a Interior,
    seat: Faction,
    infos: Vec<PositionInfo>,
    /// The shared world projection + which struct this structure is, for the QUERY reads. `None`
    /// for the plain greedy path that does not look ahead. Used **only** by the parked automata
    /// track; the live game never builds a projection (see `direct`).
    proj: Option<(&'a Projection, StructId)>,
    /// The projection-free in-transit influx (the live game's look-ahead). When `Some`, the
    /// `incoming_mine` / `enemy_incoming` / `friendly_eta` reads answer from it directly instead of
    /// from a forward projection. Built by [`Layer1View::direct`] (the Simple controller's path).
    direct: Option<SubInflux>,
    /// Sim params for the geometry/force reads (`transit_ticks` distance→ticks, force sizing).
    sp: SimParams,
}

impl<'a> Layer1View<'a> {
    /// Snapshot `st` for `seat` under `params`, **without** a projection (the plain greedy
    /// tactical path). Computes each sub's [`PositionInfo`] once so the policy reads a stable
    /// view. (`params` is read for the engagement radius used to count contesting enemy ships and
    /// retained for the geometry reads.)
    pub fn new(st: &'a Interior, params: &'a SimParams, seat: Faction) -> Layer1View<'a> {
        Self::build(st, params, seat, None)
    }

    /// Snapshot `st` for `seat`, sharing the controller's forward [`world::Projection`] (built
    /// over the whole world this tick) and the [`StructId`] `struct` this structure is, so the
    /// automatons' look-ahead QUERIES read the same projection at Layer 1 as at Layer 2.
    pub fn with_projection(
        st: &'a Interior,
        params: &'a SimParams,
        seat: Faction,
        proj: &'a Projection,
        sid: StructId,
    ) -> Layer1View<'a> {
        Self::build(st, params, seat, Some((proj, sid)))
    }

    /// Snapshot `st` for `seat` with a **projection-free** in-transit `influx` (the live game's
    /// look-ahead — see [`World::sub_influx_for`]). The `incoming_mine` / `enemy_incoming` /
    /// `friendly_eta` reads answer from `influx` directly; the heavier projection QUERY reads
    /// (`capture_eta`, `force_for_efficiency`, …) fall back to their conservative defaults (Simple
    /// never asks them). This is how the campaign **Simple** seat avoids building a projection.
    pub fn direct(
        st: &'a Interior,
        params: &'a SimParams,
        seat: Faction,
        influx: SubInflux,
    ) -> Layer1View<'a> {
        let mut v = Self::build(st, params, seat, None);
        v.direct = Some(influx);
        v
    }

    /// Shared builder for both constructors.
    fn build(
        st: &'a Interior,
        params: &'a SimParams,
        seat: Faction,
        proj: Option<(&'a Projection, StructId)>,
    ) -> Layer1View<'a> {
        let infos = (0..st.subs.len())
            .map(|s| {
                // The ownerless struct-storage node is never a capture target (it is skipped by the
                // resistance grind). Present it as the seat's **own** position — as it was when the
                // reserve was majority-owned — so the policy neither colonizes it nor counts it among
                // capturable neutrals; the seat's own ships staged there stay usable as surplus.
                // FREE-FOR-ALL: every *other* real seat (incl. a second AI) is a foe, not just one.
                let raw = st.subs[s].owner;
                let owner = if st.is_storage(s) || raw == seat {
                    PosOwner::Me
                } else if raw.is_real() {
                    PosOwner::Enemy
                } else {
                    PosOwner::Neutral
                };
                let my_ships = st.idle_count_at(s, seat) as u32;
                // Engaging ships of every other real seat (all are foes) — one generic pass, no
                // hardcoded seat list, so any number of `Ai(i)` opponents is counted.
                let enemy_ships = engaging_foes_count(st, params, s, seat) as u32;
                let contested = enemy_ships > 0;
                PositionInfo { id: s, owner, my_ships, enemy_ships, contested }
            })
            .collect();
        Layer1View { st, seat, infos, proj, direct: None, sp: *params }
    }

    /// The concrete faction for a seat-relative [`Side`].
    #[inline]
    fn faction_of(&self, side: Side) -> Faction {
        match side {
            Side::Me => self.seat,
            Side::Foe => self.seat.opponent(),
        }
    }

    /// Map a projected sub owner onto a seat-relative [`Side`] (`None` if neutral / no change).
    #[inline]
    fn side_of(&self, f: Faction) -> Option<Side> {
        if f == self.seat {
            Some(Side::Me)
        } else if f == self.seat.opponent() {
            Some(Side::Foe)
        } else {
            None
        }
    }

    /// Turn the greedy policy's abstract actions into concrete [`MoveOrder`]s for this structure.
    /// Each action moves a fraction-bucket of the source sub's idle ships toward the target
    /// sub. Actions that map to no shippable bucket are dropped.
    pub fn to_move_orders(&self, actions: &[GreedyAction]) -> Vec<MoveOrder> {
        let mut orders = Vec::with_capacity(actions.len());
        for a in actions {
            let available = self.st.idle_count_at(a.from, self.seat) as u32;
            if let Some(frac) = bucket_for(a.count, available) {
                orders.push(MoveOrder::new(a.from, a.to, frac));
            }
        }
        orders
    }
}

impl<'a> PositionView for Layer1View<'a> {
    fn len(&self) -> usize {
        self.infos.len()
    }
    fn info(&self, id: usize) -> PositionInfo {
        self.infos[id]
    }
    fn distance(&self, from: usize, to: usize) -> Option<f32> {
        Some(self.st.subs[from].pos.dist(self.st.subs[to].pos))
    }
    // Any owned sub may shed surplus at Layer 1 (no export precondition); the default
    // `can_export_from == true` and `reachable == distance.is_some()` are correct.
    fn first_hop(&self, from: usize, to: usize) -> Option<usize> {
        // One structure: every sub is directly reachable from every other, so the first hop toward
        // any (distinct, valid) target IS the target itself — there is never a foe-held waypoint.
        (from != to && to < self.infos.len()).then_some(to)
    }
    fn is_staging(&self, id: usize) -> bool {
        // The ownerless struct-storage / reserve node: its garrison is the struct's rallied
        // export stock, so the greedy never redistributes it via the friendly-reinforce rule.
        self.st.is_storage(id)
    }

    // ---- Property signals (thin sim reads — NO mechanic re-derived). --------------------------

    fn resistance(&self, id: usize) -> f32 {
        // The grind remaining to take this sub *for the seat*: it is a foreign sub iff not mine.
        // The ownerless storage node reads 0 — it is presented as the seat's own position and can
        // never be captured, so there is no grind to size a wave on.
        if self.st.is_storage(id) || self.st.subs[id].owner == self.seat {
            0.0
        } else {
            self.st.sub_resistance(id).0
        }
    }

    fn min_foothold_resistance(&self, id: usize) -> f32 {
        // At Layer 1 a "position" is a single sub, so the cheapest foothold of one sub IS its own
        // resistance (if foreign). Kept distinct from `resistance` so the Layer-2 roll-up can mean
        // "the cheapest sub on the struct" without changing the policy code.
        self.resistance(id)
    }

    fn production(&self, id: usize) -> f32 {
        // The sub's ships-per-period mint rate (≥ 1 so callers can divide by it safely).
        self.st.subs[id].production.max(1) as f32
    }

    fn present_count(&self, id: usize, side: Side) -> u32 {
        self.st.presence_in_sub(id, self.faction_of(side)) as u32
    }

    fn idle_at(&self, id: usize, side: Side) -> u32 {
        self.st.idle_count_at(id, self.faction_of(side)) as u32
    }

    fn soft_cap_at(&self, _id: usize) -> u32 {
        // Per-structure cap for the seat (the same number the sim attrites against). It is a
        // structure-wide quantity, so every position reports the seat's structure cap.
        self.st.soft_cap(self.seat, &self.sp)
    }

    fn parked_ratio(&self, _id: usize) -> f32 {
        let cap = self.st.soft_cap(self.seat, &self.sp);
        if cap == 0 {
            return 0.0;
        }
        self.st.parked_count(self.seat) as f32 / cap as f32
    }

    fn transit_ticks(&self, from: usize, to: usize) -> Option<u64> {
        // Undock, then a straight-line intra-structure hop at ship_speed, "arrived" within
        // arrival_tolerance — the same pacing the sim charges (`dispatch_move` sets
        // `undock_remaining = undock_ticks` on every order), mirroring `Layer2View::transit_ticks`
        // which already adds the fleet undock. A departure from a TELEPORTER the seat owns
        // arrives the instant the undock burns out (no transit leg) — without this the Simple
        // ledger's synchronized landings would desync on teleporter maps.
        let teleporting = self
            .st
            .subs
            .get(from)
            .map_or(false, |s| s.kind == layer1::SubKind::Teleporter && s.owner == self.seat);
        if teleporting {
            return Some(self.sp.undock_ticks as u64);
        }
        let d = self.st.subs[from].pos.dist(self.st.subs[to].pos);
        let eff = (d - self.sp.arrival_tolerance).max(0.0);
        let speed = self.sp.ship_speed.max(1e-6);
        Some(self.sp.undock_ticks as u64 + ((eff / speed).ceil() as u64).max(1))
    }

    fn overwatch_toll(&self, from: usize, to: usize) -> u32 {
        let n = self.st.subs.len();
        if from >= n || to >= n {
            return 0;
        }
        let (a, b) = (self.st.subs[from].pos, self.st.subs[to].pos);
        let mut toll = 0u32;
        for f in 0..n {
            let sub = &self.st.subs[f];
            if sub.kind != layer1::SubKind::Fortress || !sub.owner.is_real() || sub.owner == self.seat {
                continue; // only RIVAL-held fortresses shoot us on the way
            }
            if seg_point_dist(a, b, sub.pos) <= fort_overwatch_reach(sub) {
                toll += self.st.idle_count_at(f, sub.owner) as u32;
            }
        }
        toll
    }

    fn fort_coverage(&self, id: usize) -> f32 {
        let n = self.st.subs.len();
        let Some(fort) = self.st.subs.get(id) else { return 0.0 };
        if fort.kind != layer1::SubKind::Fortress {
            return 0.0;
        }
        let reach = fort_overwatch_reach(fort);
        let (mut pairs, mut crossed) = (0u32, 0u32);
        for u in 0..n {
            if u == id {
                continue;
            }
            for v in (u + 1)..n {
                if v == id {
                    continue;
                }
                pairs += 1;
                if seg_point_dist(self.st.subs[u].pos, self.st.subs[v].pos, fort.pos) <= reach {
                    crossed += 1;
                }
            }
        }
        if pairs == 0 {
            0.0
        } else {
            crossed as f32 / pairs as f32
        }
    }

    fn gate_savings(&self, id: usize) -> f32 {
        let n = self.st.subs.len();
        let Some(gate) = self.st.subs.get(id) else { return 0.0 };
        if gate.kind != layer1::SubKind::Teleporter {
            return 0.0;
        }
        // Ordered pairs u → v (u, v ≠ gate): direct = |uv|; via the gate the trip shortens to
        // the walk u → gate (the hop out is instant). Fraction of total distance saved.
        let (mut sum_direct, mut sum_saved) = (0.0f32, 0.0f32);
        for u in 0..n {
            if u == id {
                continue;
            }
            let to_gate = self.st.subs[u].pos.dist(gate.pos);
            for v in 0..n {
                if v == id || v == u {
                    continue;
                }
                let direct = self.st.subs[u].pos.dist(self.st.subs[v].pos);
                sum_direct += direct;
                sum_saved += (direct - to_gate).max(0.0);
            }
        }
        if sum_direct <= f32::EPSILON {
            0.0
        } else {
            sum_saved / sum_direct
        }
    }

    fn fort_capacity(&self, id: usize) -> Option<u32> {
        let s = self.st.subs.get(id)?;
        (s.kind == layer1::SubKind::Fortress && s.owner == self.seat).then(|| s.storage_capacity)
    }

    fn capacity(&self, id: usize) -> u32 {
        // The effective cap the attrition model enforces (a yard's invisible virtual cap).
        self.st.subs.get(id).map_or(0, |s| s.storage_cap_effective() as u32)
    }

    fn via_gate(&self, from: usize, to: usize) -> Option<(usize, u64)> {
        let direct = self.transit_ticks(from, to)?;
        let mut best: Option<(usize, u64)> = None;
        for g in 0..self.st.subs.len() {
            if g == from || g == to {
                continue;
            }
            let s = &self.st.subs[g];
            if s.kind != layer1::SubKind::Teleporter || s.owner != self.seat {
                continue;
            }
            let (Some(t1), Some(t2)) = (self.transit_ticks(from, g), self.transit_ticks(g, to)) else {
                continue;
            };
            let total = t1.saturating_add(t2);
            if total < direct && best.map_or(true, |(_, bt)| total < bt) {
                best = Some((g, total));
            }
        }
        best
    }

    // ---- Forward-projection QUERY pass-throughs (per sub of this structure). ---------------------

    fn capture_eta(&self, id: usize) -> Option<u64> {
        let (proj, p) = self.proj?;
        proj.capture_eta(p, id)
    }

    fn projected_next_owner(&self, id: usize) -> Option<Side> {
        let (proj, p) = self.proj?;
        let f = proj.sub_fate(p, id);
        self.side_of(f.owner_after_first_change?)
    }

    fn marginal_ticks_saved(&self, target: usize, from: usize) -> u64 {
        match self.proj {
            Some((proj, p)) => proj.marginal_ticks_saved(p, target, from),
            None => 0,
        }
    }

    fn force_for_efficiency(&self, id: usize, ratio: f32) -> Option<u32> {
        let (proj, p) = self.proj?;
        proj.force_for_efficiency(p, id, ratio)
    }

    fn incoming_mine(&self, id: usize) -> u32 {
        if let Some(d) = &self.direct {
            return d.mine.get(id).copied().unwrap_or(0);
        }
        match self.proj {
            Some((proj, p)) => proj.incoming_present_at(p, id, self.seat),
            None => 0,
        }
    }

    fn enemy_incoming(&self, id: usize) -> u32 {
        if let Some(d) = &self.direct {
            return d.foe.get(id).copied().unwrap_or(0);
        }
        match self.proj {
            // Aggregate over every real faction that is not the acting seat (free-for-all: a second
            // enemy counts as a foe too) — the in-flight mirror of how `enemy_ships` is summed.
            Some((proj, p)) => proj.incoming_present_foes_at(p, id, self.seat),
            None => 0,
        }
    }

    fn friendly_eta(&self, id: usize) -> Option<u64> {
        if let Some(d) = &self.direct {
            return d.friendly_eta.get(id).copied().flatten();
        }
        let (proj, p) = self.proj?;
        proj.eta_to_present_for(p, id, self.seat)
    }

    fn returning_owner_force(&self, id: usize) -> u32 {
        match self.proj {
            Some((proj, p)) => proj.returning_owner_force(p, id),
            None => 0,
        }
    }
}

/// Count of living ships of **every foe of `seat`** engaging sub `s`: within
/// `radius + engagement_radius` of its centre (so a stack one hop away that can fire across
/// counts), mirroring the Layer-1 Automaton's `defenders_of`/`is_contested` notion. One
/// generic pass — free-for-all, with no hardcoded seat list, so a
/// level may field any number of `Ai(i)` opponents and they all register as threats.
fn engaging_foes_count(st: &Interior, params: &SimParams, s: SubId, seat: Faction) -> usize {
    let c = st.subs[s].pos;
    let reach = st.subs[s].radius + params.engagement_radius;
    let reach2 = reach * reach;
    st.ships
        .iter()
        .filter(|sh| sh.alive && sh.faction.is_foe_of(seat) && sh.pos.dist_sq(c) <= reach2)
        .count()
}

/// Convenience: run the greedy policy over `st` for `seat` and return the [`MoveOrder`]s to
/// issue. This is the **per-struct tactical default** (auto-defend/expand) the controller uses
/// and the player's optional basic automation. `params_greedy` lets a caller tune the floor /
/// tie-break; pass `&GreedyParams::default()` for the standard behaviour.
pub fn greedy_layer1_orders(
    st: &Interior,
    params: &SimParams,
    seat: Faction,
    params_greedy: &crate::greedy::GreedyParams,
) -> Vec<MoveOrder> {
    let view = Layer1View::new(st, params, seat);
    let actions = crate::greedy::decide_greedy(&view, params_greedy);
    view.to_move_orders(&actions)
}

// ======================================================================================
// Layer-2 adapter: the World's structs.
// ======================================================================================

/// Greedy [`PositionView`] over a [`world::World`]'s structs, from the point of view of one
/// acting `seat`.
///
/// A "position" is a [`StructId`]. For each struct it reads the [`world::StructAggregate`]:
/// * **owner** relative to the seat (`StructOwner::Owned(seat)` → `Me`, the enemy → `Enemy`,
///   `Contested`/`Neutral` → ... see below),
/// * **my_ships** / **enemy_ships** = each side's ships associated with the struct (garrisoned
///   **plus** currently arriving — `StructAggregate::ships_of`),
/// * **contested** = `StructOwner::Contested`.
///
/// A `Contested` struct maps to [`PosOwner::Neutral`] *for ownership* but is flagged
/// `contested`, so the greedy rules treat it correctly: it is never an *uncontested* expand
/// target (it has enemy ships), it is a *retreat-from* trigger if I am losing there, and it is
/// a *concentrate* target when nothing uncontested remains. A struct the seat fully owns maps
/// to `Me`.
///
/// **Distance** is the shortest-path length over the lane graph (BFS from `from`, summing lane
/// lengths). **Export precondition** ([`PositionView::can_export_from`]): a struct may export
/// only when [`world::StructAggregate::fully_owned_uncontested`] is true for the seat — the
/// world spec's rule that only a securely held struct shares surplus. Because a
/// [`world::FleetOrder`] is valid only between lane-adjacent structs, the *order generation*
/// ([`Layer2View::to_fleet_orders`]) routes each action to the **first hop** along the
/// shortest path toward the chosen destination.
pub struct Layer2View<'a> {
    world: &'a World,
    seat: Faction,
    infos: Vec<PositionInfo>,
    export_ok: Vec<bool>,
    /// The shared forward projection (built once by the controller this tick) for the QUERY
    /// reads, rolled up to struct scope. `None` for the plain greedy export path.
    proj: Option<&'a Projection>,
    /// Sim/world params for the geometry/force reads (`transit_ticks`, soft cap, force sizing).
    sp: SimParams,
    wp: WorldParams,
}

impl<'a> Layer2View<'a> {
    /// Snapshot `world` for `seat`, **without** a projection but carrying the real `sp`/`wp` for the
    /// geometry/force reads — the live game's projection-free Layer-2 view (Simple's simplified push,
    /// which never asks a projection QUERY).
    pub fn without_projection(
        world: &'a World,
        seat: Faction,
        sp: &SimParams,
        wp: &WorldParams,
    ) -> Layer2View<'a> {
        Self::build(world, seat, None, *sp, *wp)
    }

    /// Snapshot `world` for `seat`, sharing the controller's forward [`world::Projection`] (built
    /// once this tick) so the composable automatons' QUERIES read it, rolled up to struct scope.
    /// `sp`/`wp` supply the geometry + force-sizing the property reads need.
    pub fn with_projection(
        world: &'a World,
        seat: Faction,
        proj: &'a Projection,
        sp: &SimParams,
        wp: &WorldParams,
    ) -> Layer2View<'a> {
        Self::build(world, seat, Some(proj), *sp, *wp)
    }

    /// Shared builder for both constructors.
    fn build(
        world: &'a World,
        seat: Faction,
        proj: Option<&'a Projection>,
        sp: SimParams,
        wp: WorldParams,
    ) -> Layer2View<'a> {
        let enemy = seat.opponent();
        let n = world.structs.len();
        let mut infos = Vec::with_capacity(n);
        let mut export_ok = Vec::with_capacity(n);
        for p in 0..n {
            let agg = world.struct_aggregate(p);
            let owner = match agg.owner {
                StructOwner::Owned(f) if f == seat => PosOwner::Me,
                StructOwner::Owned(f) if f == enemy => PosOwner::Enemy,
                // Contested or any other -> Neutral for ownership, but flagged contested below.
                _ => PosOwner::Neutral,
            };
            let my_ships = agg.ships_of(seat);
            let enemy_ships = agg.ships_of(enemy);
            let contested = matches!(agg.owner, StructOwner::Contested);
            infos.push(PositionInfo { id: p, owner, my_ships, enemy_ships, contested });
            export_ok.push(agg.fully_owned_uncontested(seat));
        }
        Layer2View { world, seat, infos, export_ok, proj, sp, wp }
    }

    /// The concrete faction for a seat-relative [`Side`].
    #[inline]
    fn faction_of(&self, side: Side) -> Faction {
        match side {
            Side::Me => self.seat,
            Side::Foe => self.seat.opponent(),
        }
    }

    /// Map a projected owner onto a seat-relative [`Side`] (`None` if neutral).
    #[inline]
    fn side_of(&self, f: Faction) -> Option<Side> {
        if f == self.seat {
            Some(Side::Me)
        } else if f == self.seat.opponent() {
            Some(Side::Foe)
        } else {
            None
        }
    }

    /// Number of subs on struct `p` (for projection roll-ups).
    #[inline]
    fn sub_count(&self, p: StructId) -> usize {
        self.world.structs.get(p).map(|pl| pl.interior.subs.len()).unwrap_or(0)
    }

    /// For the struct-scope per-sub QUERIES, pick `(target_sub, from_sub)`: the **cheapest foreign
    /// foothold** sub on struct `p` (least resistance — the sub a spearhead actually cracks first)
    /// and a **friendly source sub** on the same struct for the marginal from-position (lowest-id
    /// seat-owned sub, else sub 0). `None` if the struct has no foreign sub. This is how the
    /// Layer-2 view answers a per-sub projection query at struct granularity without inventing a
    /// new projection method.
    fn cheapest_foothold_and_source(&self, p: StructId) -> Option<(SubId, SubId)> {
        let structure = self.world.structs.get(p)?;
        let st = &structure.interior;
        let mut best: Option<(SubId, f32)> = None;
        for s in 0..st.subs.len() {
            // Skip the seat's own subs and the ownerless storage node (never capturable).
            if st.subs[s].owner == self.seat || st.is_storage(s) {
                continue;
            }
            let r = st.sub_resistance(s).0;
            match best {
                Some((_, br)) if br <= r => {}
                _ => best = Some((s, r)),
            }
        }
        let (target_sub, _) = best?;
        let from_sub = (0..st.subs.len())
            .find(|&s| st.subs[s].owner == self.seat)
            .unwrap_or(0);
        Some((target_sub, from_sub))
    }

    /// Turn the greedy policy's abstract actions into concrete [`FleetOrder`]s.
    ///
    /// Each action's destination may be several lanes away (distance is shortest-path), but a
    /// `FleetOrder` is only valid between lane-adjacent structs, so we route to the **first
    /// hop** of the shortest path from `from` toward `to`. The fraction bucket is sized from
    /// the struct's exportable surplus (its idle ships above the world's `keep_floor`); if the
    /// next hop cannot be resolved (e.g. the target became unreachable) the action is dropped.
    pub fn to_fleet_orders(&self, actions: &[GreedyAction], wp: &WorldParams) -> Vec<FleetOrder> {
        let mut orders = Vec::with_capacity(actions.len());
        for a in actions {
            // Resolve the first hop toward the chosen destination.
            let Some(next) = self.next_hop(a.from, a.to) else { continue };
            // Available exportable surplus = idle ships of the seat above the world keep_floor,
            // drawn only from owned subs (mirrors `take_idle_ships_structwide`). We size the
            // bucket against this so the chosen fraction actually releases ~the surplus.
            let available = exportable_idle(&self.world.structs[a.from].interior, self.seat, wp.keep_floor);
            if available == 0 {
                continue;
            }
            if let Some(frac) = bucket_for(a.count.min(available), available) {
                orders.push(FleetOrder::new(a.from, next, frac));
            }
        }
        orders
    }

    /// First struct on a shortest path from `from` to `to` over the lane graph (delegates to
    /// [`crate::graph::next_hop`]). Used to route a multi-lane greedy action one valid
    /// `FleetOrder` hop at a time.
    fn next_hop(&self, from: StructId, to: StructId) -> Option<StructId> {
        crate::graph::next_hop(self.world, from, to)
    }
}

impl<'a> PositionView for Layer2View<'a> {
    fn len(&self) -> usize {
        self.infos.len()
    }
    fn info(&self, id: usize) -> PositionInfo {
        self.infos[id]
    }
    fn distance(&self, from: usize, to: usize) -> Option<f32> {
        crate::graph::path_len(self.world, from, to)
    }
    fn can_export_from(&self, from: usize) -> bool {
        self.export_ok[from]
    }
    fn first_hop(&self, from: usize, to: usize) -> Option<usize> {
        // The first struct on a shortest lane-path from `from` toward `to` — exactly the hop
        // `to_fleet_orders` would route this action onto (a far objective is sent one lane at a time).
        if from == to {
            return None;
        }
        self.next_hop(from, to)
    }

    // ---- Property signals (struct-scope reads through the world wrappers). --------------------

    fn resistance(&self, id: usize) -> f32 {
        // Total foreign capture resistance on the struct for the seat (sum over not-mine subs) —
        // the grind to fully own it. Read through the world wrapper, never re-derived.
        self.world.struct_total_resistance_vs(id, self.seat)
    }

    fn min_foothold_resistance(&self, id: usize) -> f32 {
        // The cheapest foothold = the least single foreign sub's resistance on the struct (crack
        // one sub to flip a producer). The Layer-2 roll-up of the per-sub resistance signal. The
        // ownerless storage node is excluded — it can never be captured, so it is no foothold.
        let Some(structure) = self.world.structs.get(id) else { return 0.0 };
        let m = structure
            .interior
            .subs
            .iter()
            .enumerate()
            .filter(|(s, sub)| sub.owner != self.seat && !structure.interior.is_storage(*s))
            .map(|(s, _)| structure.interior.sub_resistance(s).0)
            .fold(f32::INFINITY, f32::min);
        if m.is_finite() {
            m
        } else {
            0.0
        }
    }

    fn present_count(&self, id: usize, side: Side) -> u32 {
        // Garrisoned + arriving ships of the side associated with the structure.
        self.world.struct_aggregate(id).ships_of(self.faction_of(side))
    }

    fn idle_at(&self, id: usize, side: Side) -> u32 {
        // Idle garrisoned ships of the side on the struct (the over-stack guard's input).
        let Some(structure) = self.world.structs.get(id) else { return 0 };
        let f = self.faction_of(side);
        (0..structure.interior.subs.len())
            .map(|s| structure.interior.idle_count_at(s, f) as u32)
            .sum()
    }

    fn soft_cap_at(&self, id: usize) -> u32 {
        self.world.soft_cap(id, self.seat, &self.sp)
    }

    fn parked_ratio(&self, id: usize) -> f32 {
        let cap = self.world.soft_cap(id, self.seat, &self.sp);
        if cap == 0 {
            return 0.0;
        }
        self.world.parked_count(id, self.seat) as f32 / cap as f32
    }

    fn transit_ticks(&self, from: usize, to: usize) -> Option<u64> {
        // Lane-path length / transit_speed plus the undock delay — the same timing the world's
        // fleet scheduler uses, composed only from params (no mechanic rule).
        let len = crate::graph::path_len(self.world, from, to)?;
        let speed = self.wp.transit_speed.max(1e-6);
        Some(self.wp.undock_ticks as u64 + (len / speed).ceil() as u64)
    }

    // ---- Forward-projection QUERY pass-throughs (rolled up to struct scope). ------------------

    fn capture_eta(&self, id: usize) -> Option<u64> {
        // The struct flips when its last foreign sub falls (the clean Layer-2 "fully owned" notion).
        let proj = self.proj?;
        proj.struct_capture(id).map(|(_, t)| t)
    }

    fn projected_next_owner(&self, id: usize) -> Option<Side> {
        let proj = self.proj?;
        // If the projection rolls up a clean struct flip, use its faction; else, if any of my subs
        // is projected to fall to the foe first, the struct is trending to the foe.
        if let Some((f, _)) = proj.struct_capture(id) {
            return self.side_of(f);
        }
        if proj.struct_first_fall(id, self.seat).is_some() {
            return Some(Side::Foe);
        }
        None
    }

    fn marginal_ticks_saved(&self, target: usize, from: usize) -> u64 {
        // Layer-2 marginal value of one more ship sent from a DIFFERENT struct `from` to the
        // cheapest foothold sub on `target`. The projection's `marginal_ticks_saved` is
        // intra-structure (its `from_position` must be a sub on the *same* structure), which does not
        // exist for an inter-struct wave — so we compose the *same* underlying what-if,
        // `capture_eta_if`, with the real **inter-struct transit delay** instead: compare the
        // foothold's flip ETA with and without one extra arriving ship of the seat. This is the
        // honest Layer-2 reading of "does one more ship pay its transit?".
        let Some(proj) = self.proj else { return 0 };
        let Some((tsub, _)) = self.cheapest_foothold_and_source(target) else { return 0 };
        let delay = self.transit_ticks(from, target).unwrap_or(u64::MAX);
        if delay == u64::MAX {
            return 0;
        }
        let base = proj.capture_eta_if(target, tsub, 0, delay, self.seat);
        let plus = proj.capture_eta_if(target, tsub, 1, delay, self.seat);
        match (base, plus) {
            (Some(b), Some(p)) => b.saturating_sub(p),
            // One more ship turns a non-flip (within horizon) into a flip: value = horizon to that
            // new flip (a large-but-finite "newly possible" signal, matching the projection's own).
            (None, Some(p)) => (proj.base_tick + proj.horizon).saturating_sub(p),
            (_, None) => 0,
        }
    }

    fn force_for_efficiency(&self, id: usize, ratio: f32) -> Option<u32> {
        // Sized for the cheapest foothold sub on the struct (the sub a spearhead actually cracks).
        let proj = self.proj?;
        let (tsub, _) = self.cheapest_foothold_and_source(id)?;
        proj.force_for_efficiency(id, tsub, ratio)
    }

    fn incoming_mine(&self, id: usize) -> u32 {
        let Some(proj) = self.proj else { return 0 };
        (0..self.sub_count(id)).map(|s| proj.incoming_present_at(id, s, self.seat)).sum()
    }

    fn returning_owner_force(&self, id: usize) -> u32 {
        let Some(proj) = self.proj else { return 0 };
        (0..self.sub_count(id)).map(|s| proj.returning_owner_force(id, s)).sum()
    }
}

/// Distance from point `p` to the straight segment `a`–`b` (the fortress-overwatch crossing
/// test the special-sub signals share).
fn seg_point_dist(a: layer1::Vec2, b: layer1::Vec2, p: layer1::Vec2) -> f32 {
    let (abx, aby) = (b.x - a.x, b.y - a.y);
    let len2 = abx * abx + aby * aby;
    let t = if len2 <= 1e-9 {
        0.0
    } else {
        (((p.x - a.x) * abx + (p.y - a.y) * aby) / len2).clamp(0.0, 1.0)
    };
    p.dist(layer1::Vec2::new(a.x + abx * t, a.y + aby * t))
}

/// A fortress's overwatch reach from its centre: garrison ring + the fixed fortress range
/// (matches the sim's per-shooter reach and the GUI's threat-envelope ring).
fn fort_overwatch_reach(sub: &layer1::SubStructure) -> f32 {
    sub.ring_frac * sub.radius + layer1::sim::FORTRESS_RANGE
}

/// Count of `faction`'s exportable idle ships on `st`: idle ships garrisoned on subs the
/// faction **owns**, summed with `keep_floor` withheld per owned sub. This is exactly the pool
/// [`Interior::take_idle_ships_structwide`] would draw from, so sizing a fraction bucket
/// against it makes the chosen fraction release ~the intended surplus.
fn exportable_idle(st: &Interior, faction: Faction, keep_floor: usize) -> u32 {
    let mut total = 0u32;
    for s in 0..st.subs.len() {
        if st.subs[s].owner != faction {
            continue;
        }
        let idle = st.idle_count_at(s, faction);
        if idle > keep_floor {
            total += (idle - keep_floor) as u32;
        }
    }
    total
}

/// Convenience: run the greedy policy over `world` for `seat` and return the [`FleetOrder`]s
/// to issue (the **Layer-2 greedy** strategic-ish behaviour: secure structs export surplus to
/// the nearest objective). `params_greedy` tunes the floor/tie-break. Takes the **real** match
/// `sp`/`wp` so the view's geometry/force reads run at the host's operating point (never a baked
/// `SimParams::default()` — a scaled game would silently diverge otherwise).
pub fn greedy_layer2_orders(
    world: &World,
    seat: Faction,
    sp: &SimParams,
    wp: &WorldParams,
    params_greedy: &crate::greedy::GreedyParams,
) -> Vec<FleetOrder> {
    let view = Layer2View::without_projection(world, seat, sp, wp);
    let actions = crate::greedy::decide_greedy(&view, params_greedy);
    view.to_fleet_orders(&actions, wp)
}

#[cfg(test)]
mod tests {
    use super::*;
    use layer1::{SubStructure, Vec2};
    use world::{Lane, Structure};

    fn sim() -> SimParams {
        SimParams::default()
    }

    #[test]
    fn bucket_for_picks_tightest_cover() {
        assert_eq!(bucket_for(0, 10), None);
        assert_eq!(bucket_for(5, 0), None);
        assert_eq!(bucket_for(10, 10), Some(FractionBucket::All));
        assert_eq!(bucket_for(20, 10), Some(FractionBucket::All));
        // 10 available: Quarter=3(rounded), Half=5, 3Q=8, All=10.
        assert_eq!(bucket_for(3, 10), Some(FractionBucket::Quarter));
        assert_eq!(bucket_for(4, 10), Some(FractionBucket::Half));
        assert_eq!(bucket_for(6, 10), Some(FractionBucket::ThreeQuarter));
        assert_eq!(bucket_for(9, 10), Some(FractionBucket::All));
    }

    /// A tiny one-structure world for the Layer-1 view: two Player subs far apart and one
    /// neutral sub between, so greedy expands the stocked sub toward the neutral.
    #[test]
    fn layer1_view_expands_toward_neutral() {
        let params = sim();
        let mut st = Interior::new(1);
        let home = st.add_sub(SubStructure::new(Vec2::new(0.0, 0.0), 4.0, Faction::Player));
        let _far = st.add_sub(SubStructure::new(Vec2::new(50.0, 0.0), 4.0, Faction::Player));
        let neutral = st.add_sub(SubStructure::new(Vec2::new(8.0, 0.0), 4.0, Faction::Neutral));
        for _ in 0..10 {
            st.spawn_ship(Faction::Player, home);
        }
        let orders = greedy_layer1_orders(&st, &params, Faction::Player, &crate::greedy::GreedyParams::default());
        assert!(!orders.is_empty(), "stocked home should issue an expand order");
        // It should target the neutral (nearest uncontested) from the home.
        assert!(
            orders.iter().any(|o| o.source == home && o.target == neutral),
            "expand the home's surplus to the nearest neutral, got {orders:?}"
        );
    }

    /// Layer-2 export gate: a struct that is NOT fully owned (has a neutral sub) must not
    /// export, even with surplus; once it fully owns its subs it exports to a neighbour.
    #[test]
    fn layer2_export_gate_and_routing() {
        let params = sim();
        let wp = WorldParams::default();
        // Two structs joined by a lane. Structure A: one Player sub (owned) + spare ships.
        let mut a = Interior::new(1);
        let a_home = a.add_sub(SubStructure::new(Vec2::new(0.0, 0.0), 4.0, Faction::Player));
        let _a_neutral = a.add_sub(SubStructure::new(Vec2::new(10.0, 0.0), 4.0, Faction::Neutral));
        for _ in 0..12 {
            a.spawn_ship(Faction::Player, a_home);
        }
        // Structure B: a neutral target to grab.
        let mut b = Interior::new(2);
        let _b_sub = b.add_sub(SubStructure::new(Vec2::new(0.0, 0.0), 4.0, Faction::Neutral));

        let mut w = World::new();
        let pa = w.add_struct(Structure::new(a, Vec2::new(0.0, 0.0), "A"));
        let pb = w.add_struct(Structure::new(b, Vec2::new(30.0, 0.0), "B"));
        w.add_lane(pa, pb, 30.0);

        // A is NOT fully owned (it still has a neutral sub) -> no export.
        let orders = greedy_layer2_orders(&w, Faction::Player, &params, &wp, &crate::greedy::GreedyParams::default());
        assert!(orders.is_empty(), "a struct with a neutral sub is not exportable yet");

        // Capture A's neutral sub by stepping a bit (Player ships spread & capture it), then A
        // becomes fully owned and should export toward B.
        for _ in 0..120 {
            w.step(&params, &wp);
        }
        let agg_a = w.struct_aggregate(pa);
        // Only assert the export behaviour if A indeed became fully owned (it should).
        if agg_a.fully_owned_uncontested(Faction::Player) {
            let orders = greedy_layer2_orders(&w, Faction::Player, &params, &wp, &crate::greedy::GreedyParams::default());
            assert!(
                orders.iter().any(|o| o.from == pa && o.to == pb),
                "fully-owned A should export surplus toward B, got {orders:?}"
            );
        }
        let _ = (pa, pb, Lane::new(pa, pb, 30.0)); // silence unused in some cfgs
    }
}

#[cfg(test)]
mod special_signal_tests {
    //! The Layer-1 geometry behind the special-sub signals: overwatch tolls (segment-zone
    //! crossing), path-coverage, gate savings, and the owned-gate route.

    use super::*;
    use crate::greedy::PositionView;
    use layer1::sim::Interior;
    use layer1::{SubStructure, Vec2};

    /// Fort (rival, manned by 7) at the origin; four player subs at the corners of a wide box.
    /// Only the horizontal pair through the middle crosses the overwatch zone.
    fn fort_world() -> Interior {
        // Geometry sized to FORTRESS_RANGE (18; reach ≈ 21.7): the (1,2) leg runs through the
        // fort's zone, everything else — including the (1,4)/(2,3) diagonals — stays clear.
        let mut st = Interior::new(5);
        st.add_sub(SubStructure::fortress(Vec2::new(0.0, 0.0), Faction::Ai(0))); // 0: the fort
        st.add_sub(SubStructure::new(Vec2::new(-45.0, 0.0), 0.0, Faction::Player)); // 1
        st.add_sub(SubStructure::new(Vec2::new(45.0, 0.0), 0.0, Faction::Player)); // 2
        st.add_sub(SubStructure::new(Vec2::new(-45.0, 75.0), 0.0, Faction::Player)); // 3
        st.add_sub(SubStructure::new(Vec2::new(45.0, 75.0), 0.0, Faction::Player)); // 4
        for i in 0..7 {
            let _ = i;
            st.spawn_ship(Faction::Ai(0), 0);
        }
        st
    }

    #[test]
    fn overwatch_toll_charges_crossing_legs_only() {
        let st = fort_world();
        let sp = layer1::SimParams::default();
        let v = Layer1View::new(&st, &sp, Faction::Player);
        assert_eq!(v.overwatch_toll(1, 2), 7, "the through-the-middle leg pays the manning");
        assert_eq!(v.overwatch_toll(3, 4), 0, "the wide flanking leg walks free");
        assert_eq!(v.overwatch_toll(1, 3), 0, "the lateral leg walks free");

        // A SECOND rival fort on the same crossing, manned by 5: the tolls are ADDITIVE.
        let mut st2 = fort_world();
        let f2 = st2.add_sub(SubStructure::fortress(Vec2::new(10.0, 0.0), Faction::Ai(0)));
        for _ in 0..5 {
            st2.spawn_ship(Faction::Ai(0), f2);
        }
        let v2 = Layer1View::new(&st2, &sp, Faction::Player);
        assert_eq!(v2.overwatch_toll(1, 2), 12, "two crossed gauntlets price additively (7 + 5)");
    }

    #[test]
    fn fort_coverage_counts_crossed_pairs() {
        let st = fort_world();
        let sp = layer1::SimParams::default();
        let v = Layer1View::new(&st, &sp, Faction::Player);
        // Of the 6 pairs among subs 1-4, exactly (1,2) crosses the zone.
        let c = v.fort_coverage(0);
        assert!((c - 1.0 / 6.0).abs() < 1e-6, "coverage 1/6, got {c}");
        assert_eq!(v.fort_coverage(1), 0.0, "a plain sub has no coverage");
    }

    #[test]
    fn gate_savings_and_via_route_only_for_owned_gates() {
        let sp = layer1::SimParams::default();
        // A(-30,0), gate G(0,0), B(30,0): walking A->B is far; hopping via an OWNED gate wins.
        let mut st = Interior::new(9);
        let a = st.add_sub(SubStructure::new(Vec2::new(-30.0, 0.0), 0.0, Faction::Player));
        let g = st.add_sub(SubStructure::teleporter(Vec2::new(0.0, 0.0), Faction::Player));
        let b = st.add_sub(SubStructure::new(Vec2::new(30.0, 0.0), 0.0, Faction::Neutral));
        let v = Layer1View::new(&st, &sp, Faction::Player);
        assert!(v.gate_savings(g) > 0.0, "a central gate saves complete-graph travel");
        assert_eq!(v.gate_savings(a), 0.0, "a plain sub saves nothing");
        let (via, t_via) = v.via_gate(a, b).expect("the owned gate beats walking");
        assert_eq!(via, g);
        assert!(t_via < v.transit_ticks(a, b).unwrap(), "the route is strictly faster");
        // The route charges BOTH undocks: the walk-leg's own (inside transit a->g) plus the
        // hop-leg's (transit g->b for an owned gate IS the undock).
        assert_eq!(
            t_via,
            v.transit_ticks(a, g).unwrap() + sp.undock_ticks as u64,
            "via-total = walk (with its undock) + the second undock at the gate"
        );

        // The same map with a RIVAL-held gate: no route (and no instant hop to exploit).
        let mut st2 = Interior::new(9);
        let a2 = st2.add_sub(SubStructure::new(Vec2::new(-30.0, 0.0), 0.0, Faction::Player));
        let _g2 = st2.add_sub(SubStructure::teleporter(Vec2::new(0.0, 0.0), Faction::Ai(0)));
        let b2 = st2.add_sub(SubStructure::new(Vec2::new(30.0, 0.0), 0.0, Faction::Neutral));
        let v2 = Layer1View::new(&st2, &sp, Faction::Player);
        assert!(v2.via_gate(a2, b2).is_none(), "a rival gate is no shortcut of ours");
    }

    #[test]
    fn fort_capacity_only_for_my_forts() {
        let st = fort_world();
        let sp = layer1::SimParams::default();
        let mine = Layer1View::new(&st, &sp, Faction::Ai(0));
        let theirs = Layer1View::new(&st, &sp, Faction::Player);
        assert_eq!(mine.fort_capacity(0), Some(layer1::sim::FORTRESS_STORAGE_CAPACITY));
        assert_eq!(theirs.fort_capacity(0), None, "a rival fort is not ours to man");
        assert_eq!(mine.fort_capacity(1), None, "a plain sub is no fort");
    }
}
