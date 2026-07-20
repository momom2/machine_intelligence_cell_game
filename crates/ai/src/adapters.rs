//! The thin **adapter** that maps the layer-agnostic greedy policy ([`crate::greedy`])
//! onto the game's single Layer-1 interior.
//!
//! * [`Layer1View`] — positions are the interior's **sub-structures**; distance is
//!   Euclidean over their `layer1::Vec2`; the resulting [`GreedyAction`]s become
//!   `layer1::MoveOrder`s. This is also exactly the **player's optional "basic automation"**
//!   (auto-defend / auto-expand the sub-structures).
//!
//! The adapter folds the abstract `count` (surplus ships) into a [`layer1::FractionBucket`]
//! via [`bucket_for`], since the atomic action takes a bucket rather than a raw count. The
//! conversion is intentionally conservative (it never plans to move *more* than the surplus
//! the policy intended) — see [`bucket_for`].
//!
//! (The Layer-2 adapter over world structs died with the pure-L1 pivot, owner 2026-07-20.)

use layer1::{Faction, FractionBucket, MoveOrder, SimParams, Interior, SubId};

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
pub struct Layer1View<'a> {
    st: &'a Interior,
    seat: Faction,
    infos: Vec<PositionInfo>,
    /// Sim params for the geometry/force reads (`transit_ticks` distance→ticks, force sizing).
    sp: SimParams,
}

impl<'a> Layer1View<'a> {
    /// Snapshot `st` for `seat` under `params`: computes each sub's [`PositionInfo`] once so
    /// the policy reads a stable view. (`params` is read for the engagement radius used to
    /// count contesting enemy ships and retained for the geometry reads.) The pure-L1 pivot
    /// (owner, 2026-07-20) removed the projection and in-transit-influx variants — one
    /// interior is the whole game, so this is the only constructor.
    pub fn new(st: &'a Interior, params: &'a SimParams, seat: Faction) -> Layer1View<'a> {
        // FREE-FOR-ALL: every *other* real seat (incl. a second AI) is a foe, not just one.
        let infos = (0..st.subs.len())
            .map(|s| {
                let raw = st.subs[s].owner;
                let owner = if raw == seat {
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
        Layer1View { st, seat, infos, sp: *params }
    }

    /// The concrete faction for a seat-relative [`Side`].
    #[inline]
    fn faction_of(&self, side: Side) -> Faction {
        match side {
            Side::Me => self.seat,
            Side::Foe => self.seat.opponent(),
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

    // ---- Property signals (thin sim reads — NO mechanic re-derived). --------------------------

    fn resistance(&self, id: usize) -> f32 {
        // The grind remaining to take this sub *for the seat*: it is a foreign sub iff not mine.
        if self.st.subs[id].owner == self.seat {
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
        // Same reserve-radius crutch as `distance` (Simple path only), so pull ordering and
        // synchronized-landing schedules agree with the ranking about how far storage is.
        let d = self.distance(from, to)?;
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
        let (mut pairs, mut covered) = (0u32, 0u32);
        for u in 0..n {
            for v in (u + 1)..n {
                pairs += 1;
                // An edge is COVERED when its straight segment comes within the overwatch
                // reach of the fort's centre — which includes any edge with an endpoint in
                // the zone, and (owner fix, 2026-07-08) the fort's OWN edges, trivially: a
                // trip to or from the fort is walked under its guns from the first step.
                // (Previously pairs incident to the fort were excluded, so an isolated rear
                // fort read 0.0 despite commanding every approach to itself.)
                let hit = u == id
                    || v == id
                    || seg_point_dist(self.st.subs[u].pos, self.st.subs[v].pos, fort.pos)
                        <= reach;
                if hit {
                    covered += 1;
                }
            }
        }
        if pairs == 0 {
            0.0
        } else {
            covered as f32 / pairs as f32
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

    fn fort_reach(&self, id: usize) -> f32 {
        // Any owner's fortress — the reach is a property of the ground, and the ranking
        // discount applies to RIVAL forts (an owned fort is never a candidate anyway).
        self.st
            .subs
            .get(id)
            .filter(|s| s.kind == layer1::SubKind::Fortress)
            .map_or(0.0, fort_overwatch_reach)
    }

    fn production_raw(&self, id: usize) -> f32 {
        // As authored — a fortress reads its true 0 (`production` stays max(1) for dividers).
        self.st.subs.get(id).map_or(0.0, |s| s.production as f32)
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

#[cfg(test)]
mod tests {
    use super::*;
    use layer1::{SubStructure, Vec2};
    
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
        // Attacking the fort ITSELF pays its own toll (owner check, 2026-07-08): the path's
        // endpoint sits at zero distance from the zone centre, so the approach is a crossing.
        assert_eq!(v.overwatch_toll(1, 0), 7, "the assault walks into the target's own zone");

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
    fn fort_coverage_counts_crossed_and_incident_pairs() {
        let st = fort_world();
        let sp = layer1::SimParams::default();
        let v = Layer1View::new(&st, &sp, Faction::Player);
        // 10 pairs among the 5 subs: the fort's own 4 edges are covered by definition (owner
        // fix — its lanes are walked under its guns), plus (1,2) crossing the middle = 5/10.
        let c = v.fort_coverage(0);
        assert!((c - 0.5).abs() < 1e-6, "coverage 5/10, got {c}");
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

    /// STORAGE AS A SUB (owner redesign, 2026-07-08) — the Simple path (`direct`) presents the
    /// reserve by staged-ship majority and prices it by capacity-proportional virtual
    /// resistance; the greedy path (`new`) keeps the legacy own-ground disguise.
    /// The distance crutch (owner fix, 2026-07-08): the reserve's centre position lies — its
    /// garrison lives on the huge orbit ring — so the Simple path prices it at its RADIUS'
    /// distance from every other sub (the greedy path keeps raw positions).
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
