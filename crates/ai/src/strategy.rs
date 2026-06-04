//! The pure **strategic** policies (Layer-2, over [`world::PlanetAggregate`]) and the
//! **tactical** policy selector (per-planet internal play).
//!
//! Each strategic policy is a simple, legible, hand-written rule with a **clear identity** and
//! a **documented blind spot** — these are the showcase opponents the campaign levels expose,
//! and the directional rock-paper-scissors the validated triad predicts:
//!
//! * **attack beats colonize** — a timed strike takes the colonizer's undefended production,
//! * **colonize beats defend** — the colonizer out-produces the turtle that pays opportunity cost,
//! * **defend beats attack** — the defender's edge punishes the over-committed aggressor.
//!
//! All of them emit [`world::FleetOrder`]s and obey the same two world rules: a planet may only
//! be an **export source** when [`world::PlanetAggregate::fully_owned_uncontested`] holds, and
//! a `FleetOrder` is only valid between **lane-adjacent** planets (so a move toward a far
//! objective is routed to the first hop via [`crate::graph::next_hop`]). They are stateless
//! pure functions of the observed `&World` (no hidden per-tick state), so they are fully
//! deterministic and either seat can run any of them.
//!
//! The **tactical** policy ([`TacticalPolicy`]) governs each planet's *internal* play (its
//! sub-structures). The default is the Layer-1 greedy adapter (auto-defend/expand); `None`
//! leaves a planet's internals alone (used by the passive dummy and for isolating the
//! strategic layer in tests).

use layer1::{Faction, FractionBucket};
use world::{FleetOrder, PlanetAggregate, PlanetId, PlanetOwner, World, WorldParams};

use crate::graph::{next_hop, path_len};
use crate::greedy::GreedyParams;

/// The strategic (inter-planet) policy a seat runs each decision tick. Construct one and call
/// [`StrategicPolicy::decide`] to get the [`FleetOrder`]s for the tick.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StrategicPolicy {
    /// **Passive.** Issues nothing — the inert dummy used by Level 1's enemy seat. (Its
    /// planets may still auto-defend internally if paired with a non-`None` tactical policy.)
    Passive,

    /// **GreedyLocal.** The layer-agnostic greedy export rule lifted to the planet graph (the
    /// [`crate::adapters::greedy_layer2_orders`] behaviour): every securely-held planet ships
    /// its surplus to the nearest uncontested objective, retreating from a fight it is losing.
    /// A sensible, balanced baseline that *expands and defends reactively* but — like the
    /// greedy seam — never posts a dedicated rear guard.
    GreedyLocal,

    /// **Colonize.** Maximize expansion: every exportable planet ships its surplus toward the
    /// **nearest neutral/empty** planet to grab it fast; it barely defends.
    ///
    /// *Identity:* fastest growth of the power base.
    /// *Blind spot:* **undefended production** — it keeps only the minimum garrison and pours
    /// everything into new colonies, so a **timed strike** (the Attack policy) takes a fat,
    /// thinly-held planet before colonization compounds.
    Colonize,

    /// **Defend.** Defense-first, but productive when there is no fight. If any owned planet is
    /// **contested**, it withdraws to defend — a cautious half-commitment reinforcement of the
    /// most-outnumbered contested planet from the nearest secure rear, one per tick (never
    /// over-extending). If **nothing** of its own is contested, a turtle that merely sits pays an
    /// opportunity cost, so it commits a **portion** (half, keeping a home reserve) to colonize
    /// the nearest neutral — or, when there is no neutral left to grab, to press the enemy with
    /// that same cautious half-commitment. It snaps back to reinforcing the instant a planet is
    /// contested.
    ///
    /// *Identity:* a turtle that concentrates force on its own ground and punishes an
    /// over-committed attacker, but no longer idles when the board is quiet.
    /// *Blind spot:* **opportunity cost** — its half-commitment expansion still grows more slowly
    /// than a pure colonizer's, so an out-expander (the Colonize policy) out-produces it and wins
    /// on territory at the horizon.
    Defend,

    /// **Attack.** Mass ships and strike the enemy's **weakest / most valuable** planet,
    /// accepting over-commitment. Surplus from every exportable planet is funnelled toward a
    /// single **staging** planet (the owned planet nearest the target); once the staging
    /// planet has a stack it commits the bulk of it along the lane toward the target.
    ///
    /// *Identity:* concentrate, then strike the soft, productive target.
    /// *Blind spot:* **over-extension** — it strips its own planets to feed the assault, so a
    /// **defender** (the Defend policy) that survives the strike and counter-punches the
    /// committed, thinly-backed stack can roll up the aggressor's emptied rear.
    Attack,

    /// **ColonizeThenAttack** (mix). Plays [`StrategicPolicy::Colonize`] until it holds a
    /// territory base (a tick threshold *or* it owns a majority of planets), then flips to
    /// [`StrategicPolicy::Attack`]. A common human line: grab ground first, cash it in as an
    /// army second. Its blind spot is the *transition* — a defender it strikes too early (small
    /// stack) or a colonizer that out-expanded it before the flip can still beat it.
    ColonizeThenAttack,

    /// **Balanced** (mix). Runs [`StrategicPolicy::GreedyLocal`] but, when no uncontested
    /// expansion remains, leans on [`StrategicPolicy::Attack`]'s concentrate-and-strike. A
    /// hedged generalist — strong against pure lines, but master of none, so a committed pure
    /// strategy can out-focus it on the axis it under-invests in.
    Balanced,
}

impl StrategicPolicy {
    /// Decide this tick's inter-planet [`FleetOrder`]s for `seat` over `world`. Deterministic;
    /// a pure function of `(world snapshot, seat, wp, self)`. Returns a (possibly empty) list
    /// the caller feeds to [`World::issue_fleet_order`].
    ///
    /// `tick` is the world tick, used only by the time-gated mixes; pass `world.tick`.
    pub fn decide(&self, world: &World, seat: Faction, wp: &WorldParams, tick: u64) -> Vec<FleetOrder> {
        match self {
            StrategicPolicy::Passive => Vec::new(),
            StrategicPolicy::GreedyLocal => {
                crate::adapters::greedy_layer2_orders(world, seat, wp, &GreedyParams::default())
            }
            StrategicPolicy::Colonize => colonize(world, seat, wp),
            StrategicPolicy::Defend => defend(world, seat, wp),
            StrategicPolicy::Attack => attack(world, seat, wp),
            StrategicPolicy::ColonizeThenAttack => {
                // Flip once we have a base: a tick threshold OR a planet majority.
                let owned = count_owned(world, seat);
                let total = world.planets.len();
                if tick >= COLONIZE_THEN_ATTACK_FLIP_TICK || owned * 2 > total {
                    attack(world, seat, wp)
                } else {
                    colonize(world, seat, wp)
                }
            }
            StrategicPolicy::Balanced => {
                // Expand/defend reactively; if there is nothing uncontested to grab, press an
                // attack on the weakest enemy planet (so a stalled greedy still applies force).
                let mut orders =
                    crate::adapters::greedy_layer2_orders(world, seat, wp, &GreedyParams::default());
                if orders.is_empty() && any_enemy_planet(world, seat) && !any_uncontested(world, seat) {
                    orders = attack(world, seat, wp);
                }
                orders
            }
        }
    }

    /// A short human-readable name for the GUI/levels.
    pub fn name(&self) -> &'static str {
        match self {
            StrategicPolicy::Passive => "Passive",
            StrategicPolicy::GreedyLocal => "GreedyLocal",
            StrategicPolicy::Colonize => "Colonize",
            StrategicPolicy::Defend => "Defend",
            StrategicPolicy::Attack => "Attack",
            StrategicPolicy::ColonizeThenAttack => "Colonize→Attack",
            StrategicPolicy::Balanced => "Balanced",
        }
    }
}

/// Tick at which [`StrategicPolicy::ColonizeThenAttack`] flips from colonizing to attacking if
/// it has not already secured a planet majority. Tuned so the mix gets a real opening land-grab
/// before committing to an assault on the standard test horizons (~900–1200 ticks).
pub const COLONIZE_THEN_ATTACK_FLIP_TICK: u64 = 280;

/// The bucket the colonize/attack policies use when committing surplus: the bulk of it. Big
/// enough to actually move the stack, but a bucket (not "All") so a planet is not stripped to
/// zero in one order when it has more than the launch floor.
const COMMIT_BUCKET: FractionBucket = FractionBucket::ThreeQuarter;

/// The bucket the defend policy uses to trickle **reinforcement** to a contested planet
/// (steadier, smaller commitment so it does not over-extend feeding one planet).
const REINFORCE_BUCKET: FractionBucket = FractionBucket::Half;

/// The bucket the defend policy uses for its **productive** branch (colonizing a neutral, or
/// pressing the enemy, when nothing of ours is contested). Deliberately a *small* portion so the
/// turtle keeps a large home reserve and never feeds its force piecemeal into the enemy's mass —
/// this is what preserves the **defend > attack** edge while no longer idling. Tuned: `Half`
/// dispersed the defender into the contested centre and flipped defend→attack to a loss, so the
/// commitment is a quarter (keep three-quarters home).
const DEFEND_PRODUCE_BUCKET: FractionBucket = FractionBucket::Quarter;

// ======================================================================================
// COLONIZE
// ======================================================================================

/// Colonize: each exportable planet ships its surplus toward the nearest **neutral/empty**
/// planet (uncontested target the seat does not already own). If no neutral is reachable from
/// a planet, that planet holds (colonize does not pick fights). Routed first-hop.
fn colonize(world: &World, seat: Faction, wp: &WorldParams) -> Vec<FleetOrder> {
    let mut orders = Vec::new();
    for from in exportable_planets(world, seat, wp) {
        // Nearest neutral (not owned by anyone, no enemy presence) reachable from `from`.
        let target = nearest_planet(world, from, |agg| {
            matches!(agg.owner, PlanetOwner::Neutral) && agg.ships_of(seat.opponent()) == 0
        });
        if let Some(tgt) = target {
            if let Some(hop) = next_hop(world, from, tgt) {
                orders.push(FleetOrder::new(from, hop, COMMIT_BUCKET));
            }
        }
    }
    orders
}

// ======================================================================================
// DEFEND
// ======================================================================================

/// Defend: defense-first (the turtle identity), but **productive instead of idle** when there is
/// genuinely nothing of its own to hold. Three tiers, in order:
///
/// 1. **Withdraw to defend a CONTESTED planet (the reactive identity).** If any owned planet is
///    actually contested (enemy present on it), reinforce the **most urgent** one (where we are
///    most outnumbered) from the nearest secure rear — a cautious **[`REINFORCE_BUCKET`]** (Half)
///    trickle, **one reinforcement per tick**, so the turtle never over-extends. This is what
///    lets defend punish an over-committed attacker.
/// 2. **Fortify a threatened FRONTIER.** Else, if an owned planet sits on the frontier (lane-
///    adjacent to an enemy/contested planet), pour surplus into the thinnest such planet to build
///    a garrison wall (again Half, one per tick). Concentrating force on its own forward ground —
///    rather than dispersing to chase distant neutrals — is the heart of the turtle and the
///    reason it beats Attack: the fortified frontier survives the strike and counter-punches. (A
///    defender that abandoned this and spread thin to colonize lost to Attack — see the tuning
///    note; this tier is the fix.)
/// 3. **Be productive when truly quiet.** Only when **nothing is contested and nothing of ours is
///    even on the frontier** (a safe interior — the case where the old defender simply *idled* and
///    bled opportunity cost) does it expand: ship a **portion** ([`DEFEND_PRODUCE_BUCKET`]) from
///    each exportable planet to the **nearest neutral** to colonize, keeping a home reserve. If
///    there is **no neutral to colonize anywhere**, it presses the enemy via the shared attack
///    staging logic with that same small commitment (never stripping itself into a pure attacker).
///
/// It always drops back up to the defensive tiers the instant a planet becomes contested or
/// frontier — so it withdraws to defend if attacked.
fn defend(world: &World, seat: Faction, wp: &WorldParams) -> Vec<FleetOrder> {
    let enemy = seat.opponent();
    let n = world.planets.len();
    let aggs: Vec<PlanetAggregate> = (0..n).map(|p| world.planet_aggregate(p)).collect();

    // --- Tiers 1 & 2: reinforce the most threatened owned planet (contested first, then a thin --
    // frontier). A live fight (contested) outranks a merely-adjacent frontier via the score bonus.
    let mut best_target: Option<(PlanetId, i64)> = None;
    for p in 0..n {
        let agg = aggs[p];
        let owned_by_me = matches!(agg.owner, PlanetOwner::Owned(f) if f == seat);
        let contested = matches!(agg.owner, PlanetOwner::Contested);
        // Only defend ground we have a stake in (own it, or are contesting it).
        let mine_here = agg.ships_of(seat) > 0 || agg.player_or_enemy_subs(seat) > 0;
        if !(owned_by_me || (contested && mine_here)) {
            continue;
        }
        let frontier = is_frontier(world, &aggs, p, seat);
        if !contested && !frontier {
            continue; // a safe interior planet does not need reinforcement
        }
        // Threat score: enemy pressure here minus our garrison, with a big bonus when actually
        // contested (a live fight outranks a merely-adjacent frontier).
        let enemy_here = agg.ships_of(enemy) as i64;
        let mine = agg.ships_of(seat) as i64;
        let mut score = enemy_here - mine;
        if contested {
            score += 1000;
        } else if frontier {
            score += 100;
        }
        match best_target {
            Some((_, bs)) if bs >= score => {}
            _ => best_target = Some((p, score)),
        }
    }
    if let Some((target, _)) = best_target {
        let mut orders = Vec::new();
        // Reinforce from the nearest secure rear planet (exportable, not the target itself).
        if let Some(from) = nearest_exportable_to(world, seat, wp, target) {
            if let Some(hop) = next_hop(world, from, target) {
                orders.push(FleetOrder::new(from, hop, REINFORCE_BUCKET));
            }
        }
        return orders; // one reinforcement per tick — never over-extend
    }

    // --- Tier 3: nothing contested AND nothing on the frontier -> be productive (don't idle). --
    // Colonize toward the nearest SAFE neutral (no enemy presence) from every exportable planet,
    // committing only a portion so most of the force stays home.
    let mut orders = Vec::new();
    for from in exportable_planets(world, seat, wp) {
        let target = nearest_planet(world, from, |agg| {
            matches!(agg.owner, PlanetOwner::Neutral) && agg.ships_of(enemy) == 0
        });
        if let Some(tgt) = target {
            if let Some(hop) = next_hop(world, from, tgt) {
                orders.push(FleetOrder::new(from, hop, DEFEND_PRODUCE_BUCKET));
            }
        }
    }
    if !orders.is_empty() {
        return orders;
    }

    // No neutral to colonize anywhere: press the enemy, but with the same small commitment so the
    // defender retains a strong home reserve (it is not turning into a pure attacker).
    attack_with(world, seat, wp, DEFEND_PRODUCE_BUCKET)
}

// ======================================================================================
// ATTACK
// ======================================================================================

/// Attack: pick the enemy's weakest/most valuable planet, stage surplus at the owned planet
/// nearest it, and commit the staging stack along the lane toward the target.
///
/// Target value = production (sub count) high, garrison (enemy ships) low. We minimize
/// `enemy_ships - W * enemy_subs` so a fat, thinly-held planet is chosen. Every exportable
/// planet ships toward the staging planet; the staging planet (if it has a real stack) commits
/// the bulk toward the target's first hop.
fn attack(world: &World, seat: Faction, wp: &WorldParams) -> Vec<FleetOrder> {
    attack_with(world, seat, wp, COMMIT_BUCKET)
}

/// The shared attack staging logic, parameterized by the commit bucket so a cautious caller can
/// strike with a **smaller** fraction (keeping a home reserve). [`StrategicPolicy::Attack`] uses
/// the full [`COMMIT_BUCKET`]; [`defend`]'s "nothing to colonize, so press" fallback uses the
/// smaller [`REINFORCE_BUCKET`] so the turtle does not strip itself bare.
fn attack_with(world: &World, seat: Faction, wp: &WorldParams, commit: FractionBucket) -> Vec<FleetOrder> {
    let enemy = seat.opponent();
    let n = world.planets.len();
    let aggs: Vec<PlanetAggregate> = (0..n).map(|p| world.planet_aggregate(p)).collect();

    // Choose the target enemy planet: weakest defence, most production.
    let mut target: Option<(PlanetId, f32)> = None; // (id, cost lower = better)
    for p in 0..n {
        let agg = aggs[p];
        let is_enemy = matches!(agg.owner, PlanetOwner::Owned(f) if f == enemy)
            || (matches!(agg.owner, PlanetOwner::Contested) && agg.ships_of(enemy) > 0);
        if !is_enemy {
            continue;
        }
        // Must be reachable from at least one of our planets.
        if !any_owned_can_reach(world, seat, p) {
            continue;
        }
        let cost = agg.ships_of(enemy) as f32 - ATTACK_VALUE_WEIGHT * agg.player_or_enemy_subs(enemy) as f32;
        match target {
            Some((_, bc)) if bc <= cost => {}
            _ => target = Some((p, cost)),
        }
    }
    let Some((target, _)) = target else {
        // No reachable enemy planet (e.g. all neutral so far): fall back to colonizing so the
        // attacker still develops rather than idling before contact.
        return colonize(world, seat, wp);
    };

    // Staging planet: our exportable planet nearest the target (the spearhead).
    let staging = nearest_exportable_to(world, seat, wp, target);

    let mut orders = Vec::new();
    // Everyone funnels surplus toward the staging planet (mass), except the staging planet
    // itself which commits forward toward the target.
    for from in exportable_planets(world, seat, wp) {
        if Some(from) == staging {
            // Spearhead: commit along the lane toward the target.
            if let Some(hop) = next_hop(world, from, target) {
                orders.push(FleetOrder::new(from, hop, commit));
            }
            continue;
        }
        if let Some(stage) = staging {
            if let Some(hop) = next_hop(world, from, stage) {
                orders.push(FleetOrder::new(from, hop, commit));
            }
        }
    }
    orders
}

/// Weight on a target planet's production (sub count) when choosing the Attack target — how
/// much one extra owned sub is worth in "defender ships avoided" terms. `1.5` makes a planet
/// with one more producing sub as attractive as one with ~1.5 fewer defenders.
const ATTACK_VALUE_WEIGHT: f32 = 1.5;

// ======================================================================================
// Shared helpers (pure reads of the world).
// ======================================================================================

/// Planets the seat may export from this tick: those where
/// [`PlanetAggregate::fully_owned_uncontested`] holds **and** there is real exportable surplus
/// (idle ships above the world keep-floor). Ascending [`PlanetId`] order (deterministic).
fn exportable_planets(world: &World, seat: Faction, wp: &WorldParams) -> Vec<PlanetId> {
    (0..world.planets.len())
        .filter(|&p| {
            let agg = world.planet_aggregate(p);
            agg.fully_owned_uncontested(seat)
                && exportable_surplus(world, p, seat, wp.keep_floor) > 0
        })
        .collect()
}

/// Exportable surplus on planet `p` for `seat`: idle ships on owned subs above `keep_floor`
/// (the pool [`world::World::issue_fleet_order`] would draw from).
fn exportable_surplus(world: &World, p: PlanetId, seat: Faction, keep_floor: usize) -> u32 {
    let st = &world.planets[p].structure;
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

/// The reachable planet from `from` (lowest id on distance ties) whose aggregate matches
/// `pred`, minimizing lane-path distance. `None` if none match / reachable.
fn nearest_planet(
    world: &World,
    from: PlanetId,
    pred: impl Fn(&PlanetAggregate) -> bool,
) -> Option<PlanetId> {
    let mut best: Option<(PlanetId, f32)> = None;
    for to in 0..world.planets.len() {
        if to == from {
            continue;
        }
        let agg = world.planet_aggregate(to);
        if !pred(&agg) {
            continue;
        }
        let Some(d) = path_len(world, from, to) else { continue };
        match best {
            Some((_, bd)) if bd <= d => {}
            _ => best = Some((to, d)),
        }
    }
    best.map(|(id, _)| id)
}

/// The seat's exportable planet nearest `target` by lane-path distance (the natural staging /
/// reinforcing source). `None` if the seat has no exportable planet that can reach `target`.
fn nearest_exportable_to(
    world: &World,
    seat: Faction,
    wp: &WorldParams,
    target: PlanetId,
) -> Option<PlanetId> {
    let mut best: Option<(PlanetId, f32)> = None;
    for from in exportable_planets(world, seat, wp) {
        if from == target {
            continue;
        }
        let Some(d) = path_len(world, from, target) else { continue };
        match best {
            Some((_, bd)) if bd <= d => {}
            _ => best = Some((from, d)),
        }
    }
    best.map(|(id, _)| id)
}

/// Is planet `p` a **frontier** for `seat`: owned/contested by the seat with at least one lane
/// neighbour that is enemy-owned or contested? (The places a defender wants a garrison wall.)
fn is_frontier(world: &World, aggs: &[PlanetAggregate], p: PlanetId, seat: Faction) -> bool {
    let enemy = seat.opponent();
    world.neighbors(p).iter().any(|&nb| {
        let a = aggs[nb];
        matches!(a.owner, PlanetOwner::Owned(f) if f == enemy)
            || matches!(a.owner, PlanetOwner::Contested)
    })
}

/// True if any owned (or fully-owned) planet of `seat` can reach planet `to` over lanes.
fn any_owned_can_reach(world: &World, seat: Faction, to: PlanetId) -> bool {
    (0..world.planets.len()).any(|p| {
        let agg = world.planet_aggregate(p);
        let mine = matches!(agg.owner, PlanetOwner::Owned(f) if f == seat);
        mine && path_len(world, p, to).is_some()
    })
}

/// Count of planets fully/owned by `seat` (by aggregate owner).
fn count_owned(world: &World, seat: Faction) -> usize {
    (0..world.planets.len())
        .filter(|&p| matches!(world.planet_aggregate(p).owner, PlanetOwner::Owned(f) if f == seat))
        .count()
}

/// True if `seat` faces at least one enemy-held or enemy-contested planet anywhere.
fn any_enemy_planet(world: &World, seat: Faction) -> bool {
    let enemy = seat.opponent();
    (0..world.planets.len()).any(|p| {
        let agg = world.planet_aggregate(p);
        matches!(agg.owner, PlanetOwner::Owned(f) if f == enemy)
            || (matches!(agg.owner, PlanetOwner::Contested) && agg.ships_of(enemy) > 0)
    })
}

/// True if any planet is an uncontested expansion target for `seat` (neutral with no enemy
/// presence). Used by [`StrategicPolicy::Balanced`] to decide when to switch to pressing.
fn any_uncontested(world: &World, seat: Faction) -> bool {
    let enemy = seat.opponent();
    (0..world.planets.len()).any(|p| {
        let agg = world.planet_aggregate(p);
        matches!(agg.owner, PlanetOwner::Neutral) && agg.ships_of(enemy) == 0
    })
}

/// The per-planet **tactical** policy: how each owned planet plays its *internal*
/// sub-structures each decision tick.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TacticalPolicy {
    /// **Greedy** (the default): each owned planet runs the Layer-1 greedy adapter
    /// ([`crate::adapters::greedy_layer1_orders`]) — auto-defend / auto-expand its subs. This
    /// is also the player's optional "basic automation" for a planet.
    Greedy,
    /// **None**: leave planet internals alone (issue no `MoveOrder`s). Used by the passive
    /// dummy and to isolate the strategic layer in tests.
    None,
}

impl TacticalPolicy {
    /// A short human-readable name.
    pub fn name(&self) -> &'static str {
        match self {
            TacticalPolicy::Greedy => "Greedy(local)",
            TacticalPolicy::None => "None",
        }
    }
}

/// A small extension on [`PlanetAggregate`] so the strategies can ask for a seat's sub count
/// without re-deriving it (the aggregate already carries per-faction sub tallies).
trait AggExt {
    /// Owned sub-structures of `faction` on this planet.
    fn player_or_enemy_subs(&self, faction: Faction) -> usize;
}
impl AggExt for PlanetAggregate {
    fn player_or_enemy_subs(&self, faction: Faction) -> usize {
        match faction {
            Faction::Player => self.player_subs,
            Faction::Enemy => self.enemy_subs,
            Faction::Neutral => self.neutral_subs,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use layer1::{Structure, SubStructure, Vec2};
    use world::Planet;

    /// Build a planet whose single sub is owned by `owner` and seeded with `ships` idle ships
    /// of `garrison` faction (often == owner). A far-apart radius keeps internal combat out of
    /// these strategic unit tests.
    fn planet(seed: u64, owner: Faction, garrison: Faction, ships: usize, pos: Vec2, name: &str) -> Planet {
        let mut st = Structure::new(seed);
        let s = st.add_sub(SubStructure::new(Vec2::new(0.0, 0.0), 4.0, owner));
        for _ in 0..ships {
            st.spawn_ship(garrison, s);
        }
        Planet::new(st, pos, name)
    }

    fn neutral_planet(seed: u64, pos: Vec2, name: &str) -> Planet {
        let mut st = Structure::new(seed);
        st.add_sub(SubStructure::new(Vec2::new(0.0, 0.0), 4.0, Faction::Neutral));
        Planet::new(st, pos, name)
    }

    /// A 3-planet line: Player home (stocked) -- neutral mid -- enemy home.
    fn line_world() -> World {
        let mut w = World::new();
        let p = w.add_planet(planet(1, Faction::Player, Faction::Player, 14, Vec2::new(0.0, 0.0), "P"));
        let m = w.add_planet(neutral_planet(2, Vec2::new(30.0, 0.0), "M"));
        let e = w.add_planet(planet(3, Faction::Enemy, Faction::Enemy, 6, Vec2::new(60.0, 0.0), "E"));
        w.add_lane(p, m, 30.0);
        w.add_lane(m, e, 30.0);
        w
    }

    #[test]
    fn colonize_targets_a_neutral() {
        let w = line_world();
        let wp = WorldParams::default();
        let orders = StrategicPolicy::Colonize.decide(&w, Faction::Player, &wp, 0);
        assert!(!orders.is_empty(), "colonize should move surplus toward the neutral");
        // First hop from P toward the neutral M is M itself (adjacent).
        assert!(orders.iter().all(|o| o.from == 0), "only the stocked Player home exports");
        assert!(orders.iter().any(|o| o.to == 1), "routes toward the neutral M");
    }

    #[test]
    fn passive_issues_nothing() {
        let w = line_world();
        let wp = WorldParams::default();
        assert!(StrategicPolicy::Passive.decide(&w, Faction::Player, &wp, 0).is_empty());
    }

    #[test]
    fn attack_targets_the_enemy_planet() {
        let w = line_world();
        let wp = WorldParams::default();
        // Player home is stocked; the only enemy planet is E (2 hops away via M). Attack stages
        // toward it. Since P is the only exportable planet it IS the staging planet, and it
        // commits along the lane toward E -> first hop is M.
        let orders = StrategicPolicy::Attack.decide(&w, Faction::Player, &wp, 0);
        assert!(!orders.is_empty(), "attack should commit toward the enemy");
        assert!(orders.iter().any(|o| o.from == 0 && o.to == 1), "spearhead routes P->M toward E");
    }

    #[test]
    fn defend_reinforces_a_contested_planet_over_expanding() {
        // P (stocked) adjacent to a CONTESTED planet C (both sides present) AND a far neutral N.
        // Defend's reactive-defense priority must reinforce C rather than wander off to colonize
        // N — the contested planet outranks the productive branch.
        let mut w = World::new();
        let p = w.add_planet(planet(1, Faction::Player, Faction::Player, 14, Vec2::new(0.0, 0.0), "P"));
        // Contested: enemy-owned sub but Player ships present too.
        let mut cst = Structure::new(2);
        let cs = cst.add_sub(SubStructure::new(Vec2::new(0.0, 0.0), 4.0, Faction::Enemy));
        for _ in 0..3 {
            cst.spawn_ship(Faction::Enemy, cs);
        }
        for _ in 0..2 {
            cst.spawn_ship(Faction::Player, cs); // Player contesting presence
        }
        let c = w.add_planet(Planet::new(cst, Vec2::new(20.0, 0.0), "C"));
        let _far = w.add_planet(neutral_planet(3, Vec2::new(80.0, 0.0), "N"));
        w.add_lane(p, c, 20.0);
        w.add_lane(c, w.planets.len() - 1, 60.0);

        let wp = WorldParams::default();
        let agg_c = w.planet_aggregate(c);
        assert!(matches!(agg_c.owner, PlanetOwner::Contested), "C must be contested for this test");
        let orders = StrategicPolicy::Defend.decide(&w, Faction::Player, &wp, 0);
        assert!(!orders.is_empty(), "defend should reinforce the contested planet");
        assert!(
            orders.iter().any(|o| o.from == p && o.to == c),
            "defend reinforces the contested planet C from the rear P, got {orders:?}"
        );
    }

    #[test]
    fn defend_colonizes_a_portion_when_nothing_is_contested() {
        // Nothing of ours is contested: the new productive defend must NOT idle — it ships a
        // PORTION (Half bucket) toward the nearest neutral to grab ground, while keeping a home
        // reserve. P (stocked, fully owned) -- neutral M. Defend should export P->M with a Half
        // bucket (not All, so it retains a reserve), the cautious-but-productive behaviour.
        let w = line_world(); // P(14) -- M(neutral) -- E(enemy), nothing contested at t=0.
        let wp = WorldParams::default();
        let orders = StrategicPolicy::Defend.decide(&w, Faction::Player, &wp, 0);
        assert!(!orders.is_empty(), "idle defend is the bug: it must colonize a portion");
        assert!(
            orders.iter().any(|o| o.from == 0 && o.to == 1 && o.fraction == DEFEND_PRODUCE_BUCKET),
            "defend ships a (small) portion from P toward the neutral M, got {orders:?}"
        );
    }
}
