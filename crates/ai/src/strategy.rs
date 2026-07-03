//! The pure **strategic** policies (Layer-2, over [`world::StructAggregate`]) and the
//! **tactical** policy selector (per-struct internal play).
//!
//! Each strategic policy is a simple, legible, hand-written rule with a **clear identity** and
//! a **documented blind spot** — these are the showcase opponents the campaign levels expose,
//! and the directional rock-paper-scissors the validated triad predicts:
//!
//! * **attack beats colonize** — a timed strike takes the colonizer's undefended production,
//! * **colonize beats defend** — the colonizer out-produces the turtle that pays opportunity cost,
//! * **defend beats attack** — the defender's edge punishes the over-committed aggressor.
//!
//! All of them emit [`world::FleetOrder`]s and obey the same two world rules: a struct may only
//! be an **export source** when [`world::StructAggregate::fully_owned_uncontested`] holds, and
//! a `FleetOrder` is only valid between **lane-adjacent** structs (so a move toward a far
//! objective is routed to the first hop via [`crate::graph::next_hop`]). They are stateless
//! pure functions of the observed `&World` (no hidden per-tick state), so they are fully
//! deterministic and either seat can run any of them.
//!
//! The **tactical** policy ([`TacticalPolicy`]) governs each struct's *internal* play (its
//! sub-structures). The default is the Layer-1 greedy adapter (auto-defend/expand); `None`
//! leaves a struct's internals alone (used by the passive dummy and for isolating the
//! strategic layer in tests).

use layer1::{Faction, SimParams};
use world::{FleetOrder, StructOwner, Projection, World, WorldParams};

use crate::adapters::Layer2View;
use crate::automata::{
    Automaton, AttackParams, ColonizeParams, DefendParams, SimpleColonizerParams,
};
use crate::greedy::GreedyParams;

/// The strategic (inter-structure) policy a seat runs each decision tick. Construct one and call
/// [`StrategicPolicy::decide`] to get the [`FleetOrder`]s for the tick.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StrategicPolicy {
    /// **Passive.** Issues nothing — the inert dummy used by Level 1's enemy seat. (Its
    /// structs may still auto-defend internally if paired with a non-`None` tactical policy.)
    Passive,

    /// **GreedyLocal.** The layer-agnostic greedy export rule lifted to the struct graph (the
    /// [`crate::adapters::greedy_layer2_orders`] behaviour): every securely-held struct ships
    /// its surplus to the nearest uncontested objective, retreating from a fight it is losing.
    /// A sensible, balanced baseline that *expands and defends reactively* but — like the
    /// greedy seam — never posts a dedicated rear guard.
    GreedyLocal,

    /// **SimpleColonizer.** The early-campaign everyman ([`crate::automata::SimpleColonizerParams`]):
    /// sizes each capture wave **proportional to the target's total foreign resistance**, fills
    /// **nearest-first**, only sends when it can crack the cheapest foothold, and STOPs once
    /// committed (present + in-transit) reaches the goal. Keeps the retreat reflex + garrison floor
    /// and the documented **thin-rear seam** (no dedicated rear guard).
    ///
    /// *Identity:* a reactive, resistance-sized colonizer. *Blind spot:* the seam — a sustained
    /// strike on its floor-only rear snowballs.
    SimpleColonize,

    /// **Colonize.** Maximize expansion via the marginal-value rule
    /// ([`crate::automata::ColonizeParams`]): send one more ship to a front **only while**
    /// `marginal_ticks_saved >= transit_cost`, running a few fronts in parallel; it barely defends.
    ///
    /// *Identity:* fastest growth of the power base.
    /// *Blind spot:* **undefended production** — it keeps only the minimum garrison and pours
    /// everything into new colonies, so a **timed strike** (the Attack policy) takes a fat,
    /// thinly-held struct before colonization compounds.
    Colonize,

    /// **Defend.** Defense-first, but productive when there is no fight. If any owned struct is
    /// **contested**, it withdraws to defend — a cautious half-commitment reinforcement of the
    /// most-outnumbered contested struct from the nearest secure rear, one per tick (never
    /// over-extending). If **nothing** of its own is contested, a turtle that merely sits pays an
    /// opportunity cost, so it commits a **portion** (half, keeping a home reserve) to colonize
    /// the nearest neutral — or, when there is no neutral left to grab, to press the enemy with
    /// that same cautious half-commitment. It snaps back to reinforcing the instant a struct is
    /// contested.
    ///
    /// *Identity:* a turtle that concentrates force on its own ground and punishes an
    /// over-committed attacker, but no longer idles when the board is quiet.
    /// *Blind spot:* **opportunity cost** — its half-commitment expansion still grows more slowly
    /// than a pure colonizer's, so an out-expander (the Colonize policy) out-produces it and wins
    /// on territory at the horizon.
    Defend,

    /// **Attack.** Mass ships and strike the enemy's **weakest / most valuable** structure,
    /// accepting over-commitment. Surplus from every exportable struct is funnelled toward a
    /// single **staging** struct (the owned struct nearest the target); once the staging
    /// struct has a stack it commits the bulk of it along the lane toward the target.
    ///
    /// *Identity:* concentrate, then strike the soft, productive target.
    /// *Blind spot:* **over-extension** — it strips its own structs to feed the assault, so a
    /// **defender** (the Defend policy) that survives the strike and counter-punches the
    /// committed, thinly-backed stack can roll up the aggressor's emptied rear.
    Attack,

    /// **HardcodedColonize** (v1). The direct hardcoded recipe ([`crate::hardcoded::colonize`]):
    /// pour every secure struct's surplus onto the nearest neutral it can take; when none remain,
    /// the same capture behaviour targets enemy ground. Never defends its own — undefended
    /// production is its blind spot.
    HardcodedColonize,
    /// **HardcodedDefend** (v1) ([`crate::hardcoded::defend`]): repel assaults by concentrating
    /// just-enough force locally; spend only genuine over-cap surplus colonizing; else keep topped.
    HardcodedDefend,
    /// **HardcodedAttack** (v1) ([`crate::hardcoded::attack`]): capture enemy ground, ranked by
    /// proximity tempered with ease of battle, concentrating; abandon a fight it projects losing.
    HardcodedAttack,

    /// **ColonizeThenAttack** (mix). Plays [`StrategicPolicy::Colonize`] until it holds a
    /// territory base (a tick threshold *or* it owns a majority of structs), then flips to
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
    /// Decide this tick's inter-struct [`FleetOrder`]s for `seat` over `world`. Deterministic;
    /// a pure function of `(world snapshot, seat, wp, self)`. Returns a (possibly empty) list
    /// the caller feeds to [`World::issue_fleet_order`].
    ///
    /// `tick` is the world tick, used only by the time-gated mixes; pass `world.tick`.
    ///
    /// This convenience builds a fresh forward [`world::Projection`] internally with default
    /// [`SimParams`]; the controller's hot path uses [`StrategicPolicy::decide_with`] to share the
    /// **one** projection it already built this tick across both layers.
    pub fn decide(&self, world: &World, seat: Faction, wp: &WorldParams, tick: u64) -> Vec<FleetOrder> {
        let sp = SimParams::default();
        let proj = world.project_forward(&sp, wp, world::DEFAULT_PROJECTION_HORIZON);
        self.decide_with(world, seat, &sp, wp, tick, &proj)
    }

    /// Decide this tick's [`FleetOrder`]s, **sharing a pre-built forward [`world::Projection`]**
    /// (the R3 "one projection per tick" path the controller uses). The four pure automatons read
    /// `proj`; the legacy mixes (`GreedyLocal`/`ColonizeThenAttack`/`Balanced`) keep their existing
    /// projection-free behaviour. Deterministic.
    pub fn decide_with(
        &self,
        world: &World,
        seat: Faction,
        sp: &SimParams,
        wp: &WorldParams,
        tick: u64,
        proj: &Projection,
    ) -> Vec<FleetOrder> {
        match self {
            StrategicPolicy::Passive => Vec::new(),
            StrategicPolicy::GreedyLocal => {
                crate::adapters::greedy_layer2_orders(world, seat, sp, wp, &GreedyParams::default())
            }
            // The four composable automatons (run over the shared projection at Layer 2).
            StrategicPolicy::SimpleColonize => run_automaton(
                Automaton::SimpleColonizer(SimpleColonizerParams::default()),
                world,
                seat,
                sp,
                wp,
                proj,
            ),
            StrategicPolicy::Colonize => {
                run_automaton(Automaton::Colonize(ColonizeParams::default()), world, seat, sp, wp, proj)
            }
            StrategicPolicy::Defend => {
                run_automaton(Automaton::Defend(DefendParams::default()), world, seat, sp, wp, proj)
            }
            StrategicPolicy::Attack => {
                run_automaton(Automaton::Attack(AttackParams::default()), world, seat, sp, wp, proj)
            }
            // The v1 HARDCODED automata: direct Layer-2 recipes (share the one projection).
            StrategicPolicy::HardcodedColonize => crate::hardcoded::colonize(world, seat, sp, wp, proj),
            StrategicPolicy::HardcodedDefend => crate::hardcoded::defend(world, seat, sp, wp, proj),
            StrategicPolicy::HardcodedAttack => crate::hardcoded::attack(world, seat, sp, wp, proj),
            StrategicPolicy::ColonizeThenAttack => {
                // Flip once we have a base: a tick threshold OR a struct majority.
                let owned = count_owned(world, seat);
                let total = world.structs.len();
                let auto = if tick >= COLONIZE_THEN_ATTACK_FLIP_TICK || owned * 2 > total {
                    Automaton::Attack(AttackParams::default())
                } else {
                    Automaton::Colonize(ColonizeParams::default())
                };
                run_automaton(auto, world, seat, sp, wp, proj)
            }
            StrategicPolicy::Balanced => {
                // Expand/defend reactively; if there is nothing uncontested to grab, press an
                // attack on the weakest enemy struct (so a stalled greedy still applies force).
                let mut orders =
                    crate::adapters::greedy_layer2_orders(world, seat, sp, wp, &GreedyParams::default());
                if orders.is_empty() && any_enemy_struct(world, seat) && !any_uncontested(world, seat) {
                    orders = run_automaton(
                        Automaton::Attack(AttackParams::default()),
                        world,
                        seat,
                        sp,
                        wp,
                        proj,
                    );
                }
                orders
            }
        }
    }

    /// Whether this policy needs the forward [`world::Projection`] to decide. Only the two
    /// **projection-free** policies the live game fields — [`StrategicPolicy::Passive`] and
    /// [`StrategicPolicy::GreedyLocal`] — return `false`; every other policy is part of the parked
    /// automata track and reads the projection (so the controller builds one only when this is `true`).
    pub fn needs_projection(&self) -> bool {
        !matches!(self, StrategicPolicy::Passive | StrategicPolicy::GreedyLocal)
    }

    /// Decide for the **projection-free** policies (the only ones the live game fields). Mirrors the
    /// `Passive`/`GreedyLocal` arms of [`decide_with`](StrategicPolicy::decide_with) without touching a
    /// projection. Panics if called for a projection-dependent policy — the controller guards this with
    /// [`needs_projection`](StrategicPolicy::needs_projection), so it is never reached in practice.
    pub fn decide_projection_free(
        &self,
        world: &World,
        seat: Faction,
        sp: &SimParams,
        wp: &WorldParams,
    ) -> Vec<FleetOrder> {
        match self {
            StrategicPolicy::Passive => Vec::new(),
            StrategicPolicy::GreedyLocal => {
                crate::adapters::greedy_layer2_orders(world, seat, sp, wp, &GreedyParams::default())
            }
            _ => unreachable!("decide_projection_free called for a projection-dependent policy"),
        }
    }

    /// A short human-readable name for the GUI/levels.
    pub fn name(&self) -> &'static str {
        match self {
            StrategicPolicy::Passive => "Passive",
            StrategicPolicy::GreedyLocal => "GreedyLocal",
            StrategicPolicy::SimpleColonize => "SimpleColonize",
            StrategicPolicy::Colonize => "Colonize",
            StrategicPolicy::Defend => "Defend",
            StrategicPolicy::Attack => "Attack",
            StrategicPolicy::HardcodedColonize => "HardcodedColonize",
            StrategicPolicy::HardcodedDefend => "HardcodedDefend",
            StrategicPolicy::HardcodedAttack => "HardcodedAttack",
            StrategicPolicy::ColonizeThenAttack => "Colonize→Attack",
            StrategicPolicy::Balanced => "Balanced",
        }
    }
}

/// Run a composable [`Automaton`] at **Layer 2** over `world` for `seat`, sharing the pre-built
/// forward `proj`, and convert its abstract [`crate::greedy::GreedyAction`]s into concrete
/// [`FleetOrder`]s (first-hop routed, fraction-bucketed) via the existing adapter. This is the
/// single bridge from the layer-agnostic automaton programs to the Layer-2 order primitive.
fn run_automaton(
    auto: Automaton,
    world: &World,
    seat: Faction,
    sp: &SimParams,
    wp: &WorldParams,
    proj: &Projection,
) -> Vec<FleetOrder> {
    let view = Layer2View::with_projection(world, seat, proj, sp, wp);
    let actions = auto.decide(&view);
    view.to_fleet_orders(&actions, wp)
}

/// Tick at which [`StrategicPolicy::ColonizeThenAttack`] flips from colonizing to attacking if
/// it has not already secured a struct majority. Tuned so the mix gets a real opening land-grab
/// before committing to an assault on the standard test horizons (~900–1200 ticks).
pub const COLONIZE_THEN_ATTACK_FLIP_TICK: u64 = 280;

// ======================================================================================
// COLONIZE / DEFEND / ATTACK / SimpleColonizer are now the COMPOSABLE AUTOMATONS in
// `crate::automata`, run at Layer 2 via `run_automaton` above. The previous hand-written
// per-struct bodies (which inlined nearest-neutral / staging heuristics with no look-ahead)
// were superseded by those projection-driven programs over `crate::vocab`; only the small
// world-reads the mixes still use remain below.
// ======================================================================================

/// Count of structs fully/owned by `seat` (by aggregate owner).
fn count_owned(world: &World, seat: Faction) -> usize {
    (0..world.structs.len())
        .filter(|&p| matches!(world.struct_aggregate(p).owner, StructOwner::Owned(f) if f == seat))
        .count()
}

/// True if `seat` faces at least one enemy-held or enemy-contested struct anywhere.
fn any_enemy_struct(world: &World, seat: Faction) -> bool {
    let enemy = seat.opponent();
    (0..world.structs.len()).any(|p| {
        let agg = world.struct_aggregate(p);
        matches!(agg.owner, StructOwner::Owned(f) if f == enemy)
            || (matches!(agg.owner, StructOwner::Contested) && agg.ships_of(enemy) > 0)
    })
}

/// True if any struct is an uncontested expansion target for `seat` (neutral with no enemy
/// presence). Used by [`StrategicPolicy::Balanced`] to decide when to switch to pressing.
fn any_uncontested(world: &World, seat: Faction) -> bool {
    let enemy = seat.opponent();
    (0..world.structs.len()).any(|p| {
        let agg = world.struct_aggregate(p);
        matches!(agg.owner, StructOwner::Neutral) && agg.ships_of(enemy) == 0
    })
}

/// The per-struct **tactical** policy: how each owned struct plays its *internal*
/// sub-structures each decision tick.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TacticalPolicy {
    /// **Greedy** (the default): each owned struct runs the Layer-1 greedy adapter
    /// ([`crate::adapters::greedy_layer1_orders`]) — auto-defend / auto-expand its subs. This
    /// is also the player's optional "basic automation" for a structure.
    Greedy,
    /// **None**: leave struct internals alone (issue no `MoveOrder`s). Used by the passive
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

#[cfg(test)]
mod tests {
    use super::*;
    use layer1::{Interior, SubStructure, Vec2};
    use world::Structure;

    /// Build a struct whose single sub is owned by `owner` and seeded with `ships` idle ships
    /// of `garrison` faction (often == owner). A far-apart radius keeps internal combat out of
    /// these strategic unit tests.
    fn structure(seed: u64, owner: Faction, garrison: Faction, ships: usize, pos: Vec2, name: &str) -> Structure {
        let mut st = Interior::new(seed);
        let s = st.add_sub(SubStructure::new(Vec2::new(0.0, 0.0), 4.0, owner));
        for _ in 0..ships {
            st.spawn_ship(garrison, s);
        }
        Structure::new(st, pos, name)
    }

    fn neutral_struct(seed: u64, pos: Vec2, name: &str) -> Structure {
        let mut st = Interior::new(seed);
        st.add_sub(SubStructure::new(Vec2::new(0.0, 0.0), 4.0, Faction::Neutral));
        Structure::new(st, pos, name)
    }

    /// A 3-struct line: Player home (stocked) -- neutral mid -- enemy home.
    fn line_world() -> World {
        let mut w = World::new();
        let p = w.add_struct(structure(1, Faction::Player, Faction::Player, 14, Vec2::new(0.0, 0.0), "P"));
        let m = w.add_struct(neutral_struct(2, Vec2::new(30.0, 0.0), "M"));
        let e = w.add_struct(structure(3, Faction::Ai(0), Faction::Ai(0), 6, Vec2::new(60.0, 0.0), "E"));
        w.add_lane(p, m, 30.0);
        w.add_lane(m, e, 30.0);
        w
    }

    #[test]
    fn passive_issues_nothing() {
        let w = line_world();
        let wp = WorldParams::default();
        assert!(StrategicPolicy::Passive.decide(&w, Faction::Player, &wp, 0).is_empty());
    }

    #[test]
    fn attack_stages_toward_the_enemy_struct() {
        // The new siege Attack ([`crate::automata`]) only COMMITS when the spearhead can win the
        // firefight efficiently and out-last the heal; until then it HOLDS and amasses. A 14-ship
        // home vs a defended enemy two hops away is below that bar, so to see the staging→target
        // commit we stock the home heavily (so `ready_to_commit` is satisfied). The point of the
        // test is that Attack targets the enemy E and routes the spearhead's first hop toward it.
        let mut w = World::new();
        let p = w.add_struct(structure(1, Faction::Player, Faction::Player, 120, Vec2::new(0.0, 0.0), "P"));
        let m = w.add_struct(neutral_struct(2, Vec2::new(30.0, 0.0), "M"));
        let e = w.add_struct(structure(3, Faction::Ai(0), Faction::Ai(0), 4, Vec2::new(60.0, 0.0), "E"));
        w.add_lane(p, m, 30.0);
        w.add_lane(m, e, 30.0);
        let _ = (m, e);
        let wp = WorldParams::default();
        let orders = StrategicPolicy::Attack.decide(&w, Faction::Player, &wp, 0);
        assert!(!orders.is_empty(), "a well-stocked attacker should commit toward the enemy");
        // P is the only exportable struct so it IS the staging; it commits along the lane toward E,
        // whose first hop is the neutral M (id 1).
        assert!(
            orders.iter().any(|o| o.from == 0 && o.to == 1),
            "the spearhead routes P->M toward E, got {orders:?}"
        );
    }

    #[test]
    fn defend_reinforces_a_contested_struct_over_expanding() {
        // P (stocked) adjacent to a CONTESTED struct C (both sides have a sub there) AND a far
        // neutral N. C is a fight on the Player's own ground that the Player is **holding** (Player
        // present-majority), so the new turtle's reactive-defense priority reinforces C rather than
        // wandering off to colonize N.
        //
        // **Recalibrated for the new resistance/soft-cap model.** The pre-grind test put the Player
        // in the *minority* on C (2 vs 3); the new Defender only reinforces a contested fight it is
        // *winning locally* (present-majority) — pouring reinforcement off its own ground into a
        // losing brawl just drains the home wall and was measured to collapse the defend>attack edge
        // (see AUTOMATA_DESIGN §6). So C is set up Player-majority here.
        let mut w = World::new();
        let p = w.add_struct(structure(1, Faction::Player, Faction::Player, 14, Vec2::new(0.0, 0.0), "P"));
        // Contested: a Player sub (with garrison) AND an Enemy sub (fewer ships) on the same structure,
        // so the aggregate is Contested with the Player holding the present-majority.
        let mut cst = Interior::new(2);
        let cps = cst.add_sub(SubStructure::new(Vec2::new(0.0, 0.0), 4.0, Faction::Player));
        let ces = cst.add_sub(SubStructure::new(Vec2::new(9.0, 0.0), 4.0, Faction::Ai(0)));
        // Even presence: Player is at parity (the `>=` majority gate accepts it — a fight on its own
        // ground it is holding) but still below the force an *efficient* hold needs, so the turtle
        // tops it up from the rear rather than wandering off to colonize.
        for _ in 0..6 {
            cst.spawn_ship(Faction::Player, cps);
        }
        for _ in 0..6 {
            cst.spawn_ship(Faction::Ai(0), ces);
        }
        let c = w.add_struct(Structure::new(cst, Vec2::new(20.0, 0.0), "C"));
        let _far = w.add_struct(neutral_struct(3, Vec2::new(80.0, 0.0), "N"));
        w.add_lane(p, c, 20.0);
        w.add_lane(c, w.structs.len() - 1, 60.0);

        let wp = WorldParams::default();
        let agg_c = w.struct_aggregate(c);
        assert!(matches!(agg_c.owner, StructOwner::Contested), "C must be contested for this test");
        assert!(
            agg_c.ships_of(Faction::Player) >= agg_c.ships_of(Faction::Ai(0)),
            "C must be a fight the Player is holding (present-majority) for the new turtle to reinforce it"
        );
        let orders = StrategicPolicy::Defend.decide(&w, Faction::Player, &wp, 0);
        assert!(!orders.is_empty(), "defend should reinforce the contested struct it is holding");
        assert!(
            orders.iter().any(|o| o.from == p && o.to == c),
            "defend reinforces the contested struct C from the rear P, got {orders:?}"
        );
    }

    #[test]
    fn defend_holds_the_reserve_below_cap_and_spends_only_the_cap_surplus() {
        // The new turtle ([`crate::automata`] Defend) is "stay productive but ONLY spend the
        // genuine surplus the soft cap would otherwise destroy". With nothing contested:
        //   * a home BELOW its soft cap keeps its reserve home, healing (it does NOT colonize); but
        //   * a home stocked OVER its soft cap spends the over-cap surplus on the nearest neutral.
        let wp = WorldParams::default();
        let sp = SimParams::default();

        // Below cap: a 14-ship single-sub home (soft cap = softcap_free + per_sub = 30) holds.
        let w_low = line_world(); // P(14) -- M(neutral) -- E(enemy)
        let low = StrategicPolicy::Defend.decide(&w_low, Faction::Player, &wp, 0);
        assert!(
            low.iter().all(|o| o.from != 0),
            "below the soft cap the turtle holds its healing reserve, got {low:?}"
        );

        // Over cap: stock the home well past its soft cap so it has genuine surplus to spend.
        let mut w_hi = World::new();
        let p = w_hi.add_struct(structure(1, Faction::Player, Faction::Player, 60, Vec2::new(0.0, 0.0), "P"));
        let _m = w_hi.add_struct(neutral_struct(2, Vec2::new(30.0, 0.0), "M"));
        w_hi.add_lane(p, 1, 30.0);
        assert!(
            w_hi.parked_count(0, Faction::Player) > w_hi.soft_cap(0, Faction::Player, &sp),
            "the test home must start OVER its soft cap"
        );
        let hi = StrategicPolicy::Defend.decide(&w_hi, Faction::Player, &wp, 0);
        assert!(
            hi.iter().any(|o| o.from == 0 && o.to == 1),
            "over the soft cap the turtle spends its genuine surplus colonizing M, got {hi:?}"
        );
    }
}
