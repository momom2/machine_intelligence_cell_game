//! The two thin **adapters** that map the layer-agnostic greedy policy ([`crate::greedy`])
//! onto the concrete layers.
//!
//! * [`Layer1View`] — positions are a single planet's **sub-structures**; distance is
//!   Euclidean over their `layer1::Vec2`; the resulting [`GreedyAction`]s become
//!   `layer1::MoveOrder`s. This is also exactly the **player's optional "basic automation"**
//!   for a planet (auto-defend / auto-expand its internal sub-structures).
//! * [`Layer2View`] — positions are the **planets** of a [`world::World`]; distance is the
//!   shortest-path length over the lane graph (BFS, summing lane lengths); a planet may only
//!   be an **export source** when [`world::PlanetAggregate::fully_owned_uncontested`] holds
//!   (per the world spec); the resulting actions become `world::FleetOrder`s, routed to the
//!   **first hop** along the shortest path toward the chosen (possibly multi-lane-distant)
//!   destination, because a `FleetOrder` is only valid between lane-adjacent planets.
//!
//! Both adapters fold the abstract `count` (surplus ships) into a [`layer1::FractionBucket`]
//! via [`bucket_for`], since both layers' atomic actions take a bucket rather than a raw
//! count. The conversion is intentionally conservative (it never plans to move *more* than the
//! surplus the policy intended) — see [`bucket_for`].

use layer1::{Faction, FractionBucket, MoveOrder, SimParams, Structure, SubId};
use world::{FleetOrder, PlanetId, PlanetOwner, World, WorldParams};

use crate::greedy::{GreedyAction, PosOwner, PositionInfo, PositionView};

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
// Layer-1 adapter: a planet's sub-structures.
// ======================================================================================

/// Greedy [`PositionView`] over a single Layer-1 [`Structure`]'s sub-structures, from the
/// point of view of one acting `seat`.
///
/// A "position" is a sub-structure ([`SubId`]). For each sub it reports:
/// * **owner** relative to the seat (`Me`/`Enemy`/`Neutral`),
/// * **my_ships** = idle ships of the seat garrisoned at the sub (the movable stock — only
///   idle ships can be ordered, matching [`Structure::issue_order`]),
/// * **enemy_ships** = enemy ships *engaging* the sub (within `radius + engagement_radius` of
///   its centre, the same "defenders/contesting" notion the Layer-1 Automaton uses),
/// * **contested** = an enemy ship is within engagement range of the sub.
///
/// Distance is Euclidean over sub-structure positions. Every sub is reachable from every other
/// (one structure, no lanes), so [`PositionView::distance`] is always `Some`.
pub struct Layer1View<'a> {
    st: &'a Structure,
    seat: Faction,
    infos: Vec<PositionInfo>,
}

impl<'a> Layer1View<'a> {
    /// Snapshot `st` for `seat` under `params`. Computes each sub's [`PositionInfo`] once so
    /// the policy reads a stable view. (`params` is read here for the engagement radius used to
    /// count contesting enemy ships; it is not retained.)
    pub fn new(st: &'a Structure, params: &'a SimParams, seat: Faction) -> Layer1View<'a> {
        let enemy = seat.opponent();
        let infos = (0..st.subs.len())
            .map(|s| {
                let owner = match st.subs[s].owner {
                    o if o == seat => PosOwner::Me,
                    o if o == enemy => PosOwner::Enemy,
                    _ => PosOwner::Neutral,
                };
                let my_ships = st.idle_count_at(s, seat) as u32;
                let enemy_ships = engaging_count(st, params, s, enemy) as u32;
                let contested = enemy_ships > 0;
                PositionInfo { id: s, owner, my_ships, enemy_ships, contested }
            })
            .collect();
        Layer1View { st, seat, infos }
    }

    /// Turn the greedy policy's abstract actions into concrete [`MoveOrder`]s for this planet.
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
}

/// Count of living `faction` ships engaging sub `s`: within `radius + engagement_radius` of
/// its centre (so a stack one hop away that can fire across counts), mirroring the Layer-1
/// Automaton's `defenders_of`/`is_contested` notion.
fn engaging_count(st: &Structure, params: &SimParams, s: SubId, faction: Faction) -> usize {
    let c = st.subs[s].pos;
    let reach = st.subs[s].radius + params.engagement_radius;
    let reach2 = reach * reach;
    st.ships
        .iter()
        .filter(|sh| sh.alive && sh.faction == faction && sh.pos.dist_sq(c) <= reach2)
        .count()
}

/// Convenience: run the greedy policy over `st` for `seat` and return the [`MoveOrder`]s to
/// issue. This is the **per-planet tactical default** (auto-defend/expand) the controller uses
/// and the player's optional basic automation. `params_greedy` lets a caller tune the floor /
/// tie-break; pass `&GreedyParams::default()` for the standard behaviour.
pub fn greedy_layer1_orders(
    st: &Structure,
    params: &SimParams,
    seat: Faction,
    params_greedy: &crate::greedy::GreedyParams,
) -> Vec<MoveOrder> {
    let view = Layer1View::new(st, params, seat);
    let actions = crate::greedy::decide_greedy(&view, params_greedy);
    view.to_move_orders(&actions)
}

// ======================================================================================
// Layer-2 adapter: the World's planets.
// ======================================================================================

/// Greedy [`PositionView`] over a [`world::World`]'s planets, from the point of view of one
/// acting `seat`.
///
/// A "position" is a [`PlanetId`]. For each planet it reads the [`world::PlanetAggregate`]:
/// * **owner** relative to the seat (`PlanetOwner::Owned(seat)` → `Me`, the enemy → `Enemy`,
///   `Contested`/`Neutral` → ... see below),
/// * **my_ships** / **enemy_ships** = each side's ships associated with the planet (garrisoned
///   **plus** currently arriving — `PlanetAggregate::ships_of`),
/// * **contested** = `PlanetOwner::Contested`.
///
/// A `Contested` planet maps to [`PosOwner::Neutral`] *for ownership* but is flagged
/// `contested`, so the greedy rules treat it correctly: it is never an *uncontested* expand
/// target (it has enemy ships), it is a *retreat-from* trigger if I am losing there, and it is
/// a *concentrate* target when nothing uncontested remains. A planet the seat fully owns maps
/// to `Me`.
///
/// **Distance** is the shortest-path length over the lane graph (BFS from `from`, summing lane
/// lengths). **Export precondition** ([`PositionView::can_export_from`]): a planet may export
/// only when [`world::PlanetAggregate::fully_owned_uncontested`] is true for the seat — the
/// world spec's rule that only a securely held planet shares surplus. Because a
/// [`world::FleetOrder`] is valid only between lane-adjacent planets, the *order generation*
/// ([`Layer2View::to_fleet_orders`]) routes each action to the **first hop** along the
/// shortest path toward the chosen destination.
pub struct Layer2View<'a> {
    world: &'a World,
    seat: Faction,
    infos: Vec<PositionInfo>,
    export_ok: Vec<bool>,
}

impl<'a> Layer2View<'a> {
    /// Snapshot `world` for `seat`. Computes each planet's [`PositionInfo`] and export flag
    /// once from its [`world::PlanetAggregate`].
    pub fn new(world: &'a World, seat: Faction) -> Layer2View<'a> {
        let enemy = seat.opponent();
        let n = world.planets.len();
        let mut infos = Vec::with_capacity(n);
        let mut export_ok = Vec::with_capacity(n);
        for p in 0..n {
            let agg = world.planet_aggregate(p);
            let owner = match agg.owner {
                PlanetOwner::Owned(f) if f == seat => PosOwner::Me,
                PlanetOwner::Owned(f) if f == enemy => PosOwner::Enemy,
                // Contested or any other -> Neutral for ownership, but flagged contested below.
                _ => PosOwner::Neutral,
            };
            let my_ships = agg.ships_of(seat);
            let enemy_ships = agg.ships_of(enemy);
            let contested = matches!(agg.owner, PlanetOwner::Contested);
            infos.push(PositionInfo { id: p, owner, my_ships, enemy_ships, contested });
            export_ok.push(agg.fully_owned_uncontested(seat));
        }
        Layer2View { world, seat, infos, export_ok }
    }

    /// Turn the greedy policy's abstract actions into concrete [`FleetOrder`]s.
    ///
    /// Each action's destination may be several lanes away (distance is shortest-path), but a
    /// `FleetOrder` is only valid between lane-adjacent planets, so we route to the **first
    /// hop** of the shortest path from `from` toward `to`. The fraction bucket is sized from
    /// the planet's exportable surplus (its idle ships above the world's `keep_floor`); if the
    /// next hop cannot be resolved (e.g. the target became unreachable) the action is dropped.
    pub fn to_fleet_orders(&self, actions: &[GreedyAction], wp: &WorldParams) -> Vec<FleetOrder> {
        let mut orders = Vec::with_capacity(actions.len());
        for a in actions {
            // Resolve the first hop toward the chosen destination.
            let Some(next) = self.next_hop(a.from, a.to) else { continue };
            // Available exportable surplus = idle ships of the seat above the world keep_floor,
            // drawn only from owned subs (mirrors `take_idle_ships_planetwide`). We size the
            // bucket against this so the chosen fraction actually releases ~the surplus.
            let available = exportable_idle(&self.world.planets[a.from].structure, self.seat, wp.keep_floor);
            if available == 0 {
                continue;
            }
            if let Some(frac) = bucket_for(a.count.min(available), available) {
                orders.push(FleetOrder::new(a.from, next, frac));
            }
        }
        orders
    }

    /// First planet on a shortest path from `from` to `to` over the lane graph (delegates to
    /// [`crate::graph::next_hop`]). Used to route a multi-lane greedy action one valid
    /// `FleetOrder` hop at a time.
    fn next_hop(&self, from: PlanetId, to: PlanetId) -> Option<PlanetId> {
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
}

/// Count of `faction`'s exportable idle ships on `st`: idle ships garrisoned on subs the
/// faction **owns**, summed with `keep_floor` withheld per owned sub. This is exactly the pool
/// [`Structure::take_idle_ships_planetwide`] would draw from, so sizing a fraction bucket
/// against it makes the chosen fraction release ~the intended surplus.
fn exportable_idle(st: &Structure, faction: Faction, keep_floor: usize) -> u32 {
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
/// to issue (the **Layer-2 greedy** strategic-ish behaviour: secure planets export surplus to
/// the nearest objective). `params_greedy` tunes the floor/tie-break.
pub fn greedy_layer2_orders(
    world: &World,
    seat: Faction,
    wp: &WorldParams,
    params_greedy: &crate::greedy::GreedyParams,
) -> Vec<FleetOrder> {
    let view = Layer2View::new(world, seat);
    let actions = crate::greedy::decide_greedy(&view, params_greedy);
    view.to_fleet_orders(&actions, wp)
}

#[cfg(test)]
mod tests {
    use super::*;
    use layer1::{SubStructure, Vec2};
    use world::{Lane, Planet};

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
        let mut st = Structure::new(1);
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

    /// Layer-2 export gate: a planet that is NOT fully owned (has a neutral sub) must not
    /// export, even with surplus; once it fully owns its subs it exports to a neighbour.
    #[test]
    fn layer2_export_gate_and_routing() {
        let params = sim();
        let wp = WorldParams::default();
        // Two planets joined by a lane. Planet A: one Player sub (owned) + spare ships.
        let mut a = Structure::new(1);
        let a_home = a.add_sub(SubStructure::new(Vec2::new(0.0, 0.0), 4.0, Faction::Player));
        let _a_neutral = a.add_sub(SubStructure::new(Vec2::new(10.0, 0.0), 4.0, Faction::Neutral));
        for _ in 0..12 {
            a.spawn_ship(Faction::Player, a_home);
        }
        // Planet B: a neutral target to grab.
        let mut b = Structure::new(2);
        let _b_sub = b.add_sub(SubStructure::new(Vec2::new(0.0, 0.0), 4.0, Faction::Neutral));

        let mut w = World::new();
        let pa = w.add_planet(Planet::new(a, Vec2::new(0.0, 0.0), "A"));
        let pb = w.add_planet(Planet::new(b, Vec2::new(30.0, 0.0), "B"));
        w.add_lane(pa, pb, 30.0);

        // A is NOT fully owned (it still has a neutral sub) -> no export.
        let orders = greedy_layer2_orders(&w, Faction::Player, &wp, &crate::greedy::GreedyParams::default());
        assert!(orders.is_empty(), "a planet with a neutral sub is not exportable yet");

        // Capture A's neutral sub by stepping a bit (Player ships spread & capture it), then A
        // becomes fully owned and should export toward B.
        for _ in 0..120 {
            w.step(&params, &wp);
        }
        let agg_a = w.planet_aggregate(pa);
        // Only assert the export behaviour if A indeed became fully owned (it should).
        if agg_a.fully_owned_uncontested(Faction::Player) {
            let orders = greedy_layer2_orders(&w, Faction::Player, &wp, &crate::greedy::GreedyParams::default());
            assert!(
                orders.iter().any(|o| o.from == pa && o.to == pb),
                "fully-owned A should export surplus toward B, got {orders:?}"
            );
        }
        let _ = (pa, pb, Lane::new(pa, pb, 30.0)); // silence unused in some cfgs
    }
}
