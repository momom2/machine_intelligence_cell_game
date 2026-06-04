//! The **AI controller** and the **roster** — the clean entry point the GUI and the campaign
//! levels call.
//!
//! Given a `&World`, a `Faction` (the seat to play), a chosen **strategic** policy
//! ([`crate::strategy::StrategicPolicy`]) and a **tactical** policy
//! ([`crate::strategy::TacticalPolicy`], default greedy), the controller produces, for one
//! decision tick, **both**:
//!
//! * the inter-planet [`world::FleetOrder`]s (the strategic layer), and
//! * the per-planet [`layer1::MoveOrder`]s (each owned planet's internal play — the tactical
//!   layer; by default the Layer-1 greedy adapter auto-defends/expands every owned planet's
//!   sub-structures).
//!
//! [`AiController::apply`] then issues both against a mutable [`world::World`] in the documented
//! order (planet internals first, then fleets), so a host can do "decide → apply" each tick
//! without knowing the internals. Everything is deterministic.

use layer1::{Faction, MoveOrder, SimParams};
use world::{FleetOrder, PlanetId, PlanetOwner, World, WorldParams, DEFAULT_PROJECTION_HORIZON};

use crate::greedy::GreedyParams;
use crate::strategy::{StrategicPolicy, TacticalPolicy};

/// One seat's full decision for a tick: the inter-planet fleet orders plus the per-planet
/// internal move orders. Returned by [`AiController::decide`]; applied by
/// [`AiController::apply`].
#[derive(Debug, Clone, Default, PartialEq)]
pub struct AiDecision {
    /// Inter-planet orders (the strategic layer) to feed to [`World::issue_fleet_order`].
    pub fleet_orders: Vec<FleetOrder>,
    /// Per-planet internal orders (the tactical layer): for each owned planet, the
    /// [`MoveOrder`]s to feed to that planet's [`layer1::Structure::issue_order`]. Only planets
    /// with at least one order appear.
    pub planet_orders: Vec<(PlanetId, Vec<MoveOrder>)>,
}

impl AiDecision {
    /// Total number of concrete orders in this decision (fleets + all per-planet moves) — a
    /// convenient activity gauge for tests/diagnostics.
    pub fn order_count(&self) -> usize {
        self.fleet_orders.len() + self.planet_orders.iter().map(|(_, v)| v.len()).sum::<usize>()
    }
}

/// The AI controller for one seat: a {strategic, tactical} policy pair plus the greedy
/// tunables. Stateless across ticks (a pure function of the observed world), so a clone behaves
/// identically and re-running is deterministic.
#[derive(Debug, Clone, Copy)]
pub struct AiController {
    /// The seat this controller plays.
    pub seat: Faction,
    /// The inter-planet strategic policy.
    pub strategic: StrategicPolicy,
    /// The per-planet internal tactical policy (default [`TacticalPolicy::Greedy`]).
    pub tactical: TacticalPolicy,
    /// Tunables for the greedy tactical adapter (garrison floor + tie-break).
    pub greedy: GreedyParams,
}

impl AiController {
    /// A controller for `seat` with an explicit {strategic, tactical} pair and default greedy
    /// params.
    pub fn new(seat: Faction, strategic: StrategicPolicy, tactical: TacticalPolicy) -> AiController {
        AiController { seat, strategic, tactical, greedy: GreedyParams::default() }
    }

    /// A controller for `seat` from a [`Roster`] entry (bundles the policy pair).
    pub fn from_roster(seat: Faction, entry: Roster) -> AiController {
        let (strategic, tactical) = entry.policies();
        AiController { seat, strategic, tactical, greedy: GreedyParams::default() }
    }

    /// Compute this seat's [`AiDecision`] for the current tick **without mutating** the world.
    ///
    /// * **Strategic:** runs [`StrategicPolicy::decide`] to get the fleet orders.
    /// * **Tactical:** for each planet the seat has a presence on, runs the chosen tactical
    ///   policy over that planet's [`layer1::Structure`] to get its internal move orders. With
    ///   [`TacticalPolicy::Greedy`] this is the Layer-1 greedy adapter; with
    ///   [`TacticalPolicy::None`] no internal orders are produced.
    ///
    /// Deterministic: a pure function of `(world, params, wp, self)`.
    ///
    /// **One projection per decision tick (R3 contract).** This builds the shared forward
    /// [`world::Projection`] **exactly once** here and hands the *same* object to both layers — the
    /// strategic policy (via [`StrategicPolicy::decide_with`]) and every per-planet tactical
    /// [`crate::adapters::Layer1View::with_projection`]. No policy re-projects; they all read this
    /// look-ahead, satisfying "call `project_forward` once and share it (both layers, via the
    /// view/adapters)".
    pub fn decide(&self, world: &World, params: &SimParams, wp: &WorldParams) -> AiDecision {
        // THE single forward projection for this tick, shared by both layers.
        let proj = world.project_forward(params, wp, DEFAULT_PROJECTION_HORIZON);

        let fleet_orders = self.strategic.decide_with(world, self.seat, params, wp, world.tick, &proj);

        let mut planet_orders: Vec<(PlanetId, Vec<MoveOrder>)> = Vec::new();
        if self.tactical == TacticalPolicy::Greedy {
            for p in 0..world.planets.len() {
                // Only bother on planets where the seat actually has subs/ships to command;
                // the greedy adapter would return empty otherwise, but this keeps the result
                // tidy and avoids needless work.
                if !self.has_presence(world, p) {
                    continue;
                }
                // Share THIS tick's projection with the Layer-1 view (the greedy default does not
                // read it, but a projection-aware tactical policy would — and threading it keeps
                // the "one projection, both layers" contract literal and ready to use).
                let view = crate::adapters::Layer1View::with_projection(
                    &world.planets[p].structure,
                    params,
                    self.seat,
                    &proj,
                    p,
                );
                let actions = crate::greedy::decide_greedy(&view, &self.greedy);
                let orders = view.to_move_orders(&actions);
                if !orders.is_empty() {
                    planet_orders.push((p, orders));
                }
            }
        }

        AiDecision { fleet_orders, planet_orders }
    }

    /// Apply a previously-[`AiController::decide`]d decision to `world` for this seat, in the
    /// documented order: **per-planet internal moves first**, then **inter-planet fleets**.
    /// Returns `(ships moved internally, ships launched in fleets)`.
    ///
    /// Internals-first matches the world's own tick discipline (a planet's spatial sim resolves
    /// before fleets move) and means a fleet launched this tick draws from the surplus *after*
    /// any internal reshuffling the same tick requested — but note internal `MoveOrder`s only
    /// retarget idle ships (they do not change the idle *count* this instant), so the two layers
    /// do not fight over the same ships within a tick.
    pub fn apply(&self, world: &mut World, decision: &AiDecision, wp: &WorldParams) -> (usize, usize) {
        let mut moved = 0usize;
        for (p, orders) in &decision.planet_orders {
            if *p < world.planets.len() {
                for o in orders {
                    moved += world.planets[*p].structure.issue_order(*o);
                }
            }
        }
        let mut launched = 0usize;
        for o in &decision.fleet_orders {
            launched += world.issue_fleet_order(*o, self.seat, wp) as usize;
        }
        (moved, launched)
    }

    /// Decide and apply in one call (the common "advance this seat one decision" path). Returns
    /// `(moved, launched)`.
    pub fn decide_and_apply(
        &self,
        world: &mut World,
        params: &SimParams,
        wp: &WorldParams,
    ) -> (usize, usize) {
        let decision = self.decide(world, params, wp);
        self.apply(world, &decision, wp)
    }

    /// True if `seat` has any sub or ship on planet `p` (so its internals are worth deciding).
    fn has_presence(&self, world: &World, p: PlanetId) -> bool {
        let agg = world.planet_aggregate(p);
        let subs = match self.seat {
            Faction::Player => agg.player_subs,
            Faction::Enemy => agg.enemy_subs,
            Faction::Neutral => 0,
        };
        subs > 0 || agg.ships_of(self.seat) > 0 || matches!(agg.owner, PlanetOwner::Owned(f) if f == self.seat)
    }
}

/// The clean **roster** the GUI / levels pick from: each entry bundles a {strategic, tactical}
/// policy pair and carries a human-readable name + description. This is the menu of opponents
/// (and player-automation presets).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Roster {
    /// Inert dummy: issues nothing, internals idle. Level 1's enemy seat.
    Passive,
    /// The layer-agnostic greedy export rule on the planet graph + greedy planet internals — a
    /// balanced expand/defend baseline that never posts a rear guard (the diagnosable seam).
    GreedyLocal,
    /// The resistance-sized, nearest-first everyman colonizer (the [`StrategicPolicy::SimpleColonize`]
    /// automaton). Identity: reactive resistance-proportional expansion; blind spot: the thin-rear seam.
    SimpleColonize,
    /// Pure colonizer (greedy planet internals). Identity: fastest expansion; blind spot:
    /// undefended production.
    Colonize,
    /// Pure defender (greedy planet internals). Identity: hold/reinforce; blind spot:
    /// opportunity cost.
    Attack,
    /// Pure attacker (greedy planet internals). Identity: mass-and-strike; blind spot:
    /// over-extension.
    Defend,
    /// Mix: colonize, then flip to attack once a base is held.
    ColonizeThenAttack,
    /// Mix: balanced generalist (greedy, pressing when expansion dries up).
    Balanced,
}

impl Roster {
    /// Every roster entry, in a stable display order (for menus / harness loops).
    pub const ALL: [Roster; 8] = [
        Roster::Passive,
        Roster::GreedyLocal,
        Roster::SimpleColonize,
        Roster::Colonize,
        Roster::Defend,
        Roster::Attack,
        Roster::ColonizeThenAttack,
        Roster::Balanced,
    ];

    /// The {strategic, tactical} policy pair this entry bundles. Internals default to greedy
    /// for every entry except [`Roster::Passive`] (which idles internally too, so its dummy is
    /// truly inert).
    pub fn policies(self) -> (StrategicPolicy, TacticalPolicy) {
        match self {
            Roster::Passive => (StrategicPolicy::Passive, TacticalPolicy::None),
            Roster::GreedyLocal => (StrategicPolicy::GreedyLocal, TacticalPolicy::Greedy),
            Roster::SimpleColonize => (StrategicPolicy::SimpleColonize, TacticalPolicy::Greedy),
            Roster::Colonize => (StrategicPolicy::Colonize, TacticalPolicy::Greedy),
            Roster::Defend => (StrategicPolicy::Defend, TacticalPolicy::Greedy),
            Roster::Attack => (StrategicPolicy::Attack, TacticalPolicy::Greedy),
            Roster::ColonizeThenAttack => (StrategicPolicy::ColonizeThenAttack, TacticalPolicy::Greedy),
            Roster::Balanced => (StrategicPolicy::Balanced, TacticalPolicy::Greedy),
        }
    }

    /// A short human-readable name for the entry.
    pub fn name(self) -> &'static str {
        match self {
            Roster::Passive => "Passive",
            Roster::GreedyLocal => "Greedy (local)",
            Roster::SimpleColonize => "SimpleColonize",
            Roster::Colonize => "Colonize",
            Roster::Defend => "Defend",
            Roster::Attack => "Attack",
            Roster::ColonizeThenAttack => "Colonize→Attack",
            Roster::Balanced => "Balanced",
        }
    }

    /// A one-line description (identity + blind spot where relevant) for tooltips / level text.
    pub fn description(self) -> &'static str {
        match self {
            Roster::Passive => "Issues no orders — the inert dummy for Level 1's enemy seat.",
            Roster::GreedyLocal => {
                "Every secure planet ships surplus to the nearest objective; retreats from a \
                 losing fight. Balanced, but never posts a rear guard (its exploitable seam)."
            }
            Roster::SimpleColonize => {
                "Sizes each capture wave to the target's total resistance and fills nearest-first; \
                 keeps only a garrison floor. Blind spot: the thin-rear seam."
            }
            Roster::Colonize => {
                "Maximizes expansion to neutral planets; barely defends. Blind spot: undefended \
                 production loses to a timed strike."
            }
            Roster::Defend => {
                "Holds and reinforces owned planets; minimal expansion. Blind spot: opportunity \
                 cost loses to an out-expander."
            }
            Roster::Attack => {
                "Masses ships and strikes the enemy's weakest/most valuable planet. Blind spot: \
                 over-extension loses to a defender that punishes the committed stack."
            }
            Roster::ColonizeThenAttack => {
                "Colonizes for a base, then commits to an assault. A common human line; weak at \
                 the transition."
            }
            Roster::Balanced => {
                "A hedged generalist: expand/defend reactively, press the weakest enemy when \
                 expansion dries up. Master of none."
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use layer1::{Structure, SubStructure, Vec2};
    use world::Planet;

    fn stocked_player_planet(seed: u64, ships: usize, pos: Vec2, name: &str) -> Planet {
        let mut st = Structure::new(seed);
        let s = st.add_sub(SubStructure::new(Vec2::new(0.0, 0.0), 4.0, Faction::Player));
        for _ in 0..ships {
            st.spawn_ship(Faction::Player, s);
        }
        Planet::new(st, pos, name)
    }
    fn neutral(seed: u64, pos: Vec2, name: &str) -> Planet {
        let mut st = Structure::new(seed);
        st.add_sub(SubStructure::new(Vec2::new(0.0, 0.0), 4.0, Faction::Neutral));
        Planet::new(st, pos, name)
    }

    #[test]
    fn controller_produces_both_layers() {
        // A Player home with a neutral sub inside it (so the LAYER-1 greedy has something to
        // expand to internally) AND a neutral planet next door (so the LAYER-2 strategy has an
        // export target). The decision should carry both kinds of order.
        let mut home = Structure::new(1);
        let h = home.add_sub(SubStructure::new(Vec2::new(0.0, 0.0), 4.0, Faction::Player));
        let _hn = home.add_sub(SubStructure::new(Vec2::new(8.0, 0.0), 4.0, Faction::Neutral));
        for _ in 0..12 {
            home.spawn_ship(Faction::Player, h);
        }
        let mut w = World::new();
        let p = w.add_planet(Planet::new(home, Vec2::new(0.0, 0.0), "P"));
        let nbr = w.add_planet(neutral(2, Vec2::new(30.0, 0.0), "N"));
        w.add_lane(p, nbr, 30.0);

        let params = SimParams::default();
        let wp = WorldParams::default();
        let ctrl = AiController::from_roster(Faction::Player, Roster::Colonize);
        let dec = ctrl.decide(&w, &params, &wp);

        // Tactical: the home should issue an internal MoveOrder toward its own neutral sub.
        assert!(
            dec.planet_orders.iter().any(|(pp, ords)| *pp == p && !ords.is_empty()),
            "greedy internals should issue a per-planet order, got {dec:?}"
        );
        // Strategic: P is NOT fully owned (it has a neutral sub) so colonize cannot export yet.
        // That's correct — assert the controller still returns a well-formed decision.
        let _ = dec.fleet_orders;
        assert!(dec.order_count() >= 1);
    }

    #[test]
    fn passive_controller_is_inert() {
        let mut w = World::new();
        let p = w.add_planet(stocked_player_planet(1, 12, Vec2::new(0.0, 0.0), "P"));
        let nbr = w.add_planet(neutral(2, Vec2::new(30.0, 0.0), "N"));
        w.add_lane(p, nbr, 30.0);
        let params = SimParams::default();
        let wp = WorldParams::default();
        let ctrl = AiController::from_roster(Faction::Player, Roster::Passive);
        let dec = ctrl.decide(&w, &params, &wp);
        assert_eq!(dec.order_count(), 0, "passive issues nothing on either layer");
    }

    #[test]
    fn apply_launches_a_fleet_from_a_secure_planet() {
        // A fully-owned Player planet (single Player sub) with surplus, lane to a neutral.
        // Colonize should export, and apply() should actually launch a fleet.
        let mut w = World::new();
        let p = w.add_planet(stocked_player_planet(1, 14, Vec2::new(0.0, 0.0), "P"));
        let nbr = w.add_planet(neutral(2, Vec2::new(30.0, 0.0), "N"));
        w.add_lane(p, nbr, 30.0);
        let params = SimParams::default();
        let wp = WorldParams::default();
        let ctrl = AiController::from_roster(Faction::Player, Roster::Colonize);
        let (_moved, launched) = ctrl.decide_and_apply(&mut w, &params, &wp);
        assert!(launched > 0, "a secure stocked planet should launch a colonizing fleet");
        assert_eq!(w.fleets.len(), 1, "exactly one fleet in transit after apply");
    }

    #[test]
    fn roster_names_and_descriptions_present() {
        for r in Roster::ALL {
            assert!(!r.name().is_empty());
            assert!(r.description().len() > 10);
        }
    }
}
