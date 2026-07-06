//! The **AI controller** and the **roster** — the clean entry point the GUI and the campaign
//! levels call.
//!
//! Given a `&World`, a `Faction` (the seat to play), a chosen **strategic** policy
//! ([`crate::strategy::StrategicPolicy`]) and a **tactical** policy
//! ([`crate::strategy::TacticalPolicy`], default greedy), the controller produces, for one
//! decision tick, **both**:
//!
//! * the inter-struct [`world::FleetOrder`]s (the strategic layer), and
//! * the per-struct [`layer1::MoveOrder`]s (each owned struct's internal play — the tactical
//!   layer; by default the Layer-1 greedy adapter auto-defends/expands every owned struct's
//!   sub-structures).
//!
//! [`AiController::apply`] then issues both against a mutable [`world::World`] in the documented
//! order (struct internals first, then fleets), so a host can do "decide → apply" each tick
//! without knowing the internals. Everything is deterministic.

use layer1::{Faction, MoveOrder, SimParams};
use world::{FleetOrder, StructId, World, WorldParams, DEFAULT_PROJECTION_HORIZON};

use crate::greedy::GreedyParams;
use crate::strategy::{StrategicPolicy, TacticalPolicy};

/// One seat's full decision for a tick: the inter-struct fleet orders plus the per-structure
/// internal move orders. Returned by [`AiController::decide`]; applied by
/// [`AiController::apply`].
#[derive(Debug, Clone, Default, PartialEq)]
pub struct AiDecision {
    /// Inter-struct orders (the strategic layer) to feed to [`World::issue_fleet_order`].
    pub fleet_orders: Vec<FleetOrder>,
    /// Per-struct internal orders (the tactical layer): for each owned structure, the
    /// [`MoveOrder`]s to feed to that struct's [`layer1::Interior::issue_order`]. Only structs
    /// with at least one order appear.
    pub struct_orders: Vec<(StructId, Vec<MoveOrder>)>,
}

impl AiDecision {
    /// Total number of concrete orders in this decision (fleets + all per-struct moves) — a
    /// convenient activity gauge for tests/diagnostics.
    pub fn order_count(&self) -> usize {
        self.fleet_orders.len() + self.struct_orders.iter().map(|(_, v)| v.len()).sum::<usize>()
    }
}

/// The AI controller for one seat: a {strategic, tactical} policy pair plus the greedy
/// tunables. Stateless across ticks (a pure function of the observed world), so a clone behaves
/// identically and re-running is deterministic.
#[derive(Debug, Clone, Copy)]
pub struct AiController {
    /// The seat this controller plays.
    pub seat: Faction,
    /// The inter-struct strategic policy.
    pub strategic: StrategicPolicy,
    /// The per-struct internal tactical policy (default [`TacticalPolicy::Greedy`]).
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
    /// * **Tactical:** for each struct the seat has a presence on, runs the chosen tactical
    ///   policy over that struct's [`layer1::Interior`] to get its internal move orders. With
    ///   [`TacticalPolicy::Greedy`] this is the Layer-1 greedy adapter; with
    ///   [`TacticalPolicy::None`] no internal orders are produced.
    ///
    /// Deterministic: a pure function of `(world, params, wp, self)`.
    ///
    /// **One projection per decision tick (R3 contract).** This builds the shared forward
    /// [`world::Projection`] **exactly once** here and hands the *same* object to both layers — the
    /// strategic policy (via [`StrategicPolicy::decide_with`]) and every per-struct tactical
    /// [`crate::adapters::Layer1View::with_projection`]. No policy re-projects; they all read this
    /// look-ahead, satisfying "call `project_forward` once and share it (both layers, via the
    /// view/adapters)".
    pub fn decide(&self, world: &World, params: &SimParams, wp: &WorldParams) -> AiDecision {
        // The forward projection is built **only** for the parked automata strategic policies. The
        // live rosters (Passive / GreedyLocal) are projection-free, so the live game builds none.
        let proj = self
            .strategic
            .needs_projection()
            .then(|| world.project_forward(params, wp, DEFAULT_PROJECTION_HORIZON));

        let fleet_orders = match &proj {
            Some(p) => self.strategic.decide_with(world, self.seat, params, wp, world.tick, p),
            None => self.strategic.decide_projection_free(world, self.seat, params, wp),
        };

        let mut struct_orders: Vec<(StructId, Vec<MoveOrder>)> = Vec::new();
        if self.tactical == TacticalPolicy::Greedy {
            for p in 0..world.structs.len() {
                // Only bother on structs where the seat actually has subs/ships to command;
                // the greedy adapter would return empty otherwise, but this keeps the result
                // tidy and avoids needless work.
                if !self.has_presence(world, p) {
                    continue;
                }
                let st = &world.structs[p].interior;
                // Share this tick's projection with the Layer-1 view when one was built (the parked
                // automata path). The greedy tactical default reads no projection, so the live path
                // uses the projection-free view — behaviour-identical, just no wasted look-ahead.
                let view = match &proj {
                    Some(pr) => crate::adapters::Layer1View::with_projection(st, params, self.seat, pr, p),
                    None => crate::adapters::Layer1View::new(st, params, self.seat),
                };
                let actions = crate::greedy::decide_greedy(&view, &self.greedy);
                let orders = view.to_move_orders(&actions);
                if !orders.is_empty() {
                    struct_orders.push((p, orders));
                }
            }
        }

        AiDecision { fleet_orders, struct_orders }
    }

    /// Apply a previously-[`AiController::decide`]d decision to `world` for this seat, in the
    /// documented order: **per-struct internal moves first**, then **inter-struct fleets**.
    /// Returns `(ships moved internally, ships launched in fleets)`.
    ///
    /// Internals-first matches the world's own tick discipline (a struct's spatial sim resolves
    /// before fleets move) and means a fleet launched this tick draws from the surplus *after*
    /// any internal reshuffling the same tick requested. Note the layers **do** contend within a
    /// tick: an internal `MoveOrder` de-idles the ships it moves immediately (they gain a
    /// `target`), so a same-tick fleet order can only draw what the internal moves left idle —
    /// deterministic either way, but the ordering is load-bearing.
    pub fn apply(&self, world: &mut World, decision: &AiDecision, wp: &WorldParams) -> (usize, usize) {
        let mut moved = 0usize;
        for (p, orders) in &decision.struct_orders {
            if *p < world.structs.len() {
                for o in orders {
                    // Faction-scoped: this seat's order can only move this seat's own idle ships,
                    // never an opponent's ships sitting on the same (e.g. contested) sub.
                    moved += world.structs[*p].interior.issue_order(*o, self.seat);
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

    /// True if `seat` has any sub or ship on struct `p` (so its internals are worth deciding). Reads
    /// the structure directly (not the binary Layer-2 aggregate) so it is correct for **any** seat,
    /// including a second AI (`Enemy2`).
    fn has_presence(&self, world: &World, p: StructId) -> bool {
        let st = &world.structs[p].interior;
        st.sub_count(self.seat) > 0 || st.ship_count(self.seat) > 0
    }
}

/// The clean **roster** the GUI / levels pick from: each entry bundles a {strategic, tactical}
/// policy pair and carries a human-readable name + description. This is the menu of opponents
/// (and player-automation presets).
///
/// (Not `Eq`/`Hash`: the parameterized [`Roster::Counter`] carries an `f32` playstyle dial. Nothing
/// keys a map on a roster or compares two for exact equality, so `PartialEq` is enough.)
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Roster {
    /// Inert dummy: issues nothing, internals idle. Level 1's enemy seat.
    Passive,
    /// The layer-agnostic greedy export rule on the struct graph + greedy struct internals — a
    /// balanced expand/defend baseline that never posts a rear guard (the diagnosable seam).
    GreedyLocal,
    /// The resistance-sized, nearest-first everyman colonizer (the [`StrategicPolicy::SimpleColonize`]
    /// automaton). Identity: reactive resistance-proportional expansion; blind spot: the thin-rear seam.
    SimpleColonize,
    /// The scripted "Command and Control" drillmaster (owner-designed, mission-specific;
    /// stateful — driven by [`crate::cycler::CyclerController`]). Identity: cycles its surplus
    /// between its subs (the rotating column dodges idle attrition — the mission clock),
    /// masses its pool on an attacked sub, and launches one telegraphed all-in strike when
    /// the pool can overwhelm a target's defenders (`max(3F, F+60)`). Units standing on
    /// ground it does not own are **committed sieges** — they hold while they outnumber the
    /// enemy there (else retreat to the nearest owned sub) and never count for cycling or
    /// gathering. Blind spot: ships staged in — or flying to — the reserve are invisible to
    /// it until no foe sub remains.
    Cycler,
    /// Pure colonizer (greedy struct internals). Identity: fastest expansion; blind spot:
    /// undefended production.
    Colonize,
    /// Pure attacker (greedy struct internals). Identity: mass-and-strike; blind spot:
    /// over-extension.
    Attack,
    /// Pure defender (greedy struct internals). Identity: hold/reinforce; blind spot:
    /// opportunity cost.
    Defend,
    /// Mix: colonize, then flip to attack once a base is held.
    ColonizeThenAttack,
    /// Mix: balanced generalist (greedy, pressing when expansion dries up).
    Balanced,
    /// **HardcodedColonize** (v1): the direct hardcoded colonizer ([`crate::hardcoded::colonize`]).
    HardcodedColonize,
    /// **HardcodedDefend** (v1): the direct hardcoded defender ([`crate::hardcoded::defend`]).
    HardcodedDefend,
    /// **HardcodedAttack** (v1): the direct hardcoded attacker ([`crate::hardcoded::attack`]).
    HardcodedAttack,
    /// **The Counter** (arc-2): observes the opposing seat, profiles it in the legible vocabulary,
    /// and plays a counter = RPS backbone + projection-validated module exploits, with character set
    /// by the `p_max` **playstyle** dial in `[0, 1]` (backbone ↔ exploits; *not* difficulty —
    /// COUNTER_DESIGN §2). Unlike the fixed entries it is **stateful** (it accumulates a profile
    /// across the match), so it is driven by a [`crate::counter::CounterController`], not the
    /// stateless [`AiController`]; see [`AiController::from_roster`]'s fallback note and
    /// [`Roster::counter_p_max`].
    Counter {
        /// The playstyle dial in `[0, 1]`.
        p_max: f32,
    },
}

impl Roster {
    /// Every **fixed (parameterless)** roster entry, in a stable display order. The
    /// parameterized [`Roster::Counter`] is excluded (it has no canonical `p_max`). Currently
    /// consumed only by the name/description smoke test.
    pub const ALL: [Roster; 12] = [
        Roster::Passive,
        Roster::GreedyLocal,
        Roster::SimpleColonize,
        Roster::Cycler,
        Roster::Colonize,
        Roster::Defend,
        Roster::Attack,
        Roster::ColonizeThenAttack,
        Roster::Balanced,
        Roster::HardcodedColonize,
        Roster::HardcodedDefend,
        Roster::HardcodedAttack,
    ];

    /// The {strategic, tactical} policy pair this entry bundles. Internals default to greedy
    /// for every entry except [`Roster::Passive`] (which idles internally too, so its dummy is
    /// truly inert).
    ///
    /// [`Roster::Counter`] is **stateful** and has no single fixed pair, so it reports a
    /// **generalist fallback** ([`StrategicPolicy::Balanced`] + greedy) here — the never-worse
    /// pure-strategy a stateless [`AiController`] plays for it when the observation hook is not
    /// driven. The real accumulate-then-counter behaviour comes from
    /// [`crate::counter::CounterController`] (use [`Roster::counter_p_max`] to detect a Counter and
    /// build one).
    pub fn policies(self) -> (StrategicPolicy, TacticalPolicy) {
        match self {
            Roster::Passive => (StrategicPolicy::Passive, TacticalPolicy::None),
            Roster::GreedyLocal => (StrategicPolicy::GreedyLocal, TacticalPolicy::Greedy),
            Roster::SimpleColonize => (StrategicPolicy::SimpleColonize, TacticalPolicy::Greedy),
            // Stateless fallback for the stateful Cycler (same pattern as Counter below): inert
            // if a host drives it without the real `CyclerController`.
            Roster::Cycler => (StrategicPolicy::Passive, TacticalPolicy::None),
            Roster::Colonize => (StrategicPolicy::Colonize, TacticalPolicy::Greedy),
            Roster::Defend => (StrategicPolicy::Defend, TacticalPolicy::Greedy),
            Roster::Attack => (StrategicPolicy::Attack, TacticalPolicy::Greedy),
            Roster::ColonizeThenAttack => (StrategicPolicy::ColonizeThenAttack, TacticalPolicy::Greedy),
            Roster::Balanced => (StrategicPolicy::Balanced, TacticalPolicy::Greedy),
            Roster::HardcodedColonize => (StrategicPolicy::HardcodedColonize, TacticalPolicy::Greedy),
            Roster::HardcodedDefend => (StrategicPolicy::HardcodedDefend, TacticalPolicy::Greedy),
            Roster::HardcodedAttack => (StrategicPolicy::HardcodedAttack, TacticalPolicy::Greedy),
            // Stateless fallback for the stateful Counter (see the doc above).
            Roster::Counter { .. } => (StrategicPolicy::Balanced, TacticalPolicy::Greedy),
        }
    }

    /// The Counter's `p_max` playstyle dial if this entry is a [`Roster::Counter`], else `None`.
    /// A host uses this to detect a Counter seat and build the stateful
    /// [`crate::counter::CounterController`] for it instead of a stateless [`AiController`].
    pub fn counter_p_max(self) -> Option<f32> {
        match self {
            Roster::Counter { p_max } => Some(p_max),
            _ => None,
        }
    }

    /// A short human-readable name for the entry.
    pub fn name(self) -> &'static str {
        match self {
            Roster::Passive => "Passive",
            Roster::GreedyLocal => "Greedy (local)",
            Roster::SimpleColonize => "Simple",
            Roster::Cycler => "Cycler",
            Roster::Colonize => "Colonize",
            Roster::Defend => "Defend",
            Roster::Attack => "Attack",
            Roster::ColonizeThenAttack => "Colonize→Attack",
            Roster::Balanced => "Balanced",
            Roster::HardcodedColonize => "HardcodedColonize",
            Roster::HardcodedDefend => "HardcodedDefend",
            Roster::HardcodedAttack => "HardcodedAttack",
            Roster::Counter { .. } => "Counter",
        }
    }

    /// A one-line description (identity + blind spot where relevant) for tooltips / level text.
    pub fn description(self) -> &'static str {
        match self {
            Roster::Passive => "Issues no orders — the inert dummy for Level 1's enemy seat.",
            Roster::GreedyLocal => {
                "Every secure struct ships surplus to the nearest objective; retreats from a \
                 losing fight. Balanced, but never posts a rear guard (its exploitable seam)."
            }
            Roster::Cycler => {
                "Drills its surplus between its subs, masses everything on an attacked one, and \
                 strikes all-in only with crushing force — after a visible muster. Blind to \
                 ships staged in the reserve."
            }
            Roster::SimpleColonize => {
                "Sizes each capture wave to the target's total resistance and fills nearest-first; \
                 keeps only a garrison floor. Blind spot: the thin-rear seam."
            }
            Roster::Colonize => {
                "Maximizes expansion to neutral structs; barely defends. Blind spot: undefended \
                 production loses to a timed strike."
            }
            Roster::Defend => {
                "Holds and reinforces owned structs; minimal expansion. Blind spot: opportunity \
                 cost loses to an out-expander."
            }
            Roster::Attack => {
                "Masses ships and strikes the enemy's weakest/most valuable structure. Blind spot: \
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
            Roster::HardcodedColonize => {
                "v1 hardcoded: pours surplus onto the nearest neutral it can take, fastest first; \
                 attacks once neutrals run out; never defends its own ground."
            }
            Roster::HardcodedDefend => {
                "v1 hardcoded: repels assaults by concentrating just-enough force locally; spends \
                 only genuine over-cap surplus colonizing; otherwise keeps its structures topped."
            }
            Roster::HardcodedAttack => {
                "v1 hardcoded: captures enemy ground, choosing by proximity tempered with ease of \
                 battle; concentrates, and abandons a fight it projects losing."
            }
            Roster::Counter { .. } => {
                "Observes and profiles the opponent, then plays the RPS counter plus \
                 projection-validated exploits of its weak spots. Character set by the p_max \
                 playstyle dial (robust generalist ↔ vulnerability hunter)."
            }
        }
    }
}

/// A seat's AI driver — the roster→brain dispatch **the game and the headless validation
/// share**, so both field the *same* brain for a roster entry. Most rosters map to the
/// stateless [`AiController`]; [`Roster::SimpleColonize`] maps to the stateful
/// [`crate::SimpleController`] (it carries a per-struct departure ledger across ticks), so
/// hosts hold one `SeatController` per enemy seat and step it with `&mut`.
///
/// (Before this dispatch was shared, the levels validation built every enemy through
/// `AiController::from_roster`, whose `SimpleColonize` arm routes to the *parked* projection-
/// driven automaton — certifying a different enemy than the game ships.)
#[derive(Clone)]
pub enum SeatController {
    /// The stateless controller (pure function of the observed world).
    Stateless(AiController),
    /// The stateful campaign **Simple** ([`crate::SimpleController`]).
    Simple(crate::simple::SimpleController),
    /// The stateful scripted **Cycler** ([`crate::cycler::CyclerController`] — the
    /// "Command and Control" mission enemy).
    Cycler(crate::cycler::CyclerController),
}

impl SeatController {
    /// Build the right driver for `seat` from a roster entry: the stateful Simple for
    /// [`Roster::SimpleColonize`], the stateful Cycler for [`Roster::Cycler`], else the
    /// stateless controller.
    pub fn from_roster(seat: Faction, r: Roster) -> SeatController {
        match r {
            Roster::SimpleColonize => SeatController::Simple(crate::simple::SimpleController::new(seat)),
            Roster::Cycler => SeatController::Cycler(crate::cycler::CyclerController::new(seat)),
            _ => SeatController::Stateless(AiController::from_roster(seat, r)),
        }
    }

    /// The seat this controller plays.
    pub fn seat(&self) -> Faction {
        match self {
            SeatController::Stateless(c) => c.seat,
            SeatController::Simple(c) => c.seat,
            SeatController::Cycler(c) => c.seat,
        }
    }

    /// Decide and apply this seat's turn (mutates the ledger for the stateful variants).
    /// Returns `(ships moved internally, ships launched in fleets)`.
    pub fn decide_and_apply(
        &mut self,
        world: &mut World,
        sim: &SimParams,
        wp: &WorldParams,
    ) -> (usize, usize) {
        match self {
            SeatController::Stateless(c) => c.decide_and_apply(world, sim, wp),
            SeatController::Simple(c) => c.decide_and_apply(world, sim, wp),
            SeatController::Cycler(c) => c.decide_and_apply(world, sim, wp),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use layer1::{Interior, SubStructure, Vec2};
    use world::Structure;

    fn stocked_player_struct(seed: u64, ships: usize, pos: Vec2, name: &str) -> Structure {
        let mut st = Interior::new(seed);
        let s = st.add_sub(SubStructure::new(Vec2::new(0.0, 0.0), 4.0, Faction::Player));
        for _ in 0..ships {
            st.spawn_ship(Faction::Player, s);
        }
        Structure::new(st, pos, name)
    }
    fn neutral(seed: u64, pos: Vec2, name: &str) -> Structure {
        let mut st = Interior::new(seed);
        st.add_sub(SubStructure::new(Vec2::new(0.0, 0.0), 4.0, Faction::Neutral));
        Structure::new(st, pos, name)
    }

    #[test]
    fn controller_produces_both_layers() {
        // A Player home with a neutral sub inside it (so the LAYER-1 greedy has something to
        // expand to internally) AND a neutral struct next door (so the LAYER-2 strategy has an
        // export target). The decision should carry both kinds of order.
        let mut home = Interior::new(1);
        let h = home.add_sub(SubStructure::new(Vec2::new(0.0, 0.0), 4.0, Faction::Player));
        let _hn = home.add_sub(SubStructure::new(Vec2::new(8.0, 0.0), 4.0, Faction::Neutral));
        for _ in 0..12 {
            home.spawn_ship(Faction::Player, h);
        }
        let mut w = World::new();
        let p = w.add_struct(Structure::new(home, Vec2::new(0.0, 0.0), "P"));
        let nbr = w.add_struct(neutral(2, Vec2::new(30.0, 0.0), "N"));
        w.add_lane(p, nbr, 30.0);

        let params = SimParams::default();
        let wp = WorldParams::default();
        let ctrl = AiController::from_roster(Faction::Player, Roster::Colonize);
        let dec = ctrl.decide(&w, &params, &wp);

        // Tactical: the home should issue an internal MoveOrder toward its own neutral sub.
        assert!(
            dec.struct_orders.iter().any(|(pp, ords)| *pp == p && !ords.is_empty()),
            "greedy internals should issue a per-struct order, got {dec:?}"
        );
        // Strategic: P is NOT fully owned (it has a neutral sub) so colonize cannot export yet.
        // That's correct — assert the controller still returns a well-formed decision.
        let _ = dec.fleet_orders;
        assert!(dec.order_count() >= 1);
    }

    #[test]
    fn passive_controller_is_inert() {
        let mut w = World::new();
        let p = w.add_struct(stocked_player_struct(1, 12, Vec2::new(0.0, 0.0), "P"));
        let nbr = w.add_struct(neutral(2, Vec2::new(30.0, 0.0), "N"));
        w.add_lane(p, nbr, 30.0);
        let params = SimParams::default();
        let wp = WorldParams::default();
        let ctrl = AiController::from_roster(Faction::Player, Roster::Passive);
        let dec = ctrl.decide(&w, &params, &wp);
        assert_eq!(dec.order_count(), 0, "passive issues nothing on either layer");
    }

    #[test]
    fn roster_names_and_descriptions_present() {
        for r in Roster::ALL {
            assert!(!r.name().is_empty());
            assert!(r.description().len() > 10);
        }
    }
}
