//! The **AI controller** and the **roster** — the clean entry point the GUI and the campaign
//! levels call.
//!
//! Pure-L1 (owner pivot, 2026-07-20): the game is ONE [`layer1::Interior`]; a controller
//! decides and applies that interior's [`layer1::MoveOrder`]s for its seat each decision
//! tick. The stateless [`AiController`] runs the Layer-1 greedy adapter; the stateful
//! campaign brains ([`crate::SimpleController`], [`crate::CyclerController`]) carry their
//! ledgers across ticks. [`SeatController`] is the roster→brain dispatch **the game and the
//! headless validation share**, so both field the same brain for a roster entry.
//! Everything is deterministic (pure functions of the observed interior; no RNG).

use layer1::{Faction, Interior, SimParams};

use crate::greedy::GreedyParams;
use crate::simple::SimpleVersion;

/// The stateless controller for one seat: the Layer-1 greedy adapter over the interior
/// (or nothing, for the inert dummy). A pure function of the observed state, so a clone
/// behaves identically and re-running is deterministic.
#[derive(Debug, Clone, Copy)]
pub struct AiController {
    /// The seat this controller plays.
    pub seat: Faction,
    /// Whether the seat plays at all (`false` = the Passive dummy).
    pub active: bool,
    /// Tunables for the greedy adapter (garrison floor + tie-break).
    pub greedy: GreedyParams,
}

impl AiController {
    /// A controller for `seat` from a [`Roster`] entry.
    pub fn from_roster(seat: Faction, entry: Roster) -> AiController {
        AiController {
            seat,
            active: !matches!(entry, Roster::Passive),
            greedy: GreedyParams::default(),
        }
    }

    /// Decide and apply this seat's interior orders for one decision tick. Returns the
    /// number of ships actually moved.
    pub fn decide_and_apply(&self, st: &mut Interior, params: &SimParams) -> usize {
        if !self.active {
            return 0;
        }
        // Only bother when the seat has something to command.
        if st.sub_count(self.seat) == 0 && st.ship_count(self.seat) == 0 {
            return 0;
        }
        let orders = crate::adapters::greedy_layer1_orders(st, params, self.seat, &self.greedy);
        let mut moved = 0usize;
        for o in orders {
            // Faction-scoped: this seat's order can only move this seat's own idle ships,
            // never an opponent's ships sitting on the same (e.g. contested) sub.
            moved += st.issue_order(o, self.seat);
        }
        moved
    }
}

/// The clean **roster** the GUI / levels pick from — the menu of campaign opponents. The
/// parked pure-strategy and Counter entries died with the Layer-2 excision (they live on
/// the `layer2` branch).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Roster {
    /// Inert dummy: issues nothing. Level 1's enemy seat.
    Passive,
    /// The Layer-1 greedy adapter — a balanced expand/defend baseline.
    GreedyLocal,
    /// The stateful campaign everyman: the ledger-driven **Simple** colonizer
    /// ([`crate::SimpleController`]). Identity: resistance-sized, nearest-first capture
    /// waves; keeps only a garrison floor. Blind spot: the thin-rear seam.
    SimpleColonize {
        /// The FROZEN brain snapshot this level was balanced against (`.lvl`: `simple v1`).
        /// See [`crate::simple`] — a shipped mission's version never changes.
        version: SimpleVersion,
    },
    /// The scripted "Command and Control" drillmaster (owner-designed, mission-specific;
    /// stateful — [`crate::cycler::CyclerController`]). Identity: cycles its surplus
    /// between its subs (the rotating column dodges idle attrition — the mission clock),
    /// masses its pool on an attacked sub, and launches one telegraphed all-in strike when
    /// the pool can overwhelm a target's defenders (`max(3F, F+60)`). Units standing on
    /// ground it does not own are **committed sieges** — they hold while they outnumber
    /// the enemy there (else retreat to the nearest owned sub) and never count for
    /// cycling or gathering.
    Cycler,
    /// The **adjacency-restricted** Simple (stateful, mission-specific — Far far away's
    /// turning ring): the full [`crate::SimpleController`] brain, but its attack targets
    /// are limited to within `range` world units of an owned sub — expansion crawls
    /// neighbour to neighbour and never launches waves across the middle.
    SimpleAdjacent {
        /// The adjacency reach in world units (set per level to the ring's neighbour
        /// chord plus margin).
        range: f32,
        /// The FROZEN brain snapshot this level was balanced against (`.lvl`:
        /// `simple_adjacent 90 v1`). See [`crate::simple`].
        version: SimpleVersion,
    },
}

impl Roster {
    /// Every **fixed (parameterless)** roster entry, in a stable display order (the
    /// parameterized [`Roster::SimpleAdjacent`] is excluded). Consumed by the
    /// name/description smoke test.
    pub const ALL: [Roster; 4] = [
        Roster::Passive,
        Roster::GreedyLocal,
        Roster::SimpleColonize { version: SimpleVersion::V1 },
        Roster::Cycler,
    ];

    /// A short human-readable name for the entry.
    pub fn name(self) -> &'static str {
        match self {
            Roster::Passive => "Passive",
            Roster::GreedyLocal => "Greedy (local)",
            Roster::SimpleColonize { .. } => "Simple",
            Roster::Cycler => "Cycler",
            Roster::SimpleAdjacent { .. } => "Simple (adjacent)",
        }
    }

    /// A one-line description (identity + blind spot where relevant) for tooltips / level text.
    pub fn description(self) -> &'static str {
        match self {
            Roster::Passive => "Issues no orders — the inert dummy for Level 1's enemy seat.",
            Roster::GreedyLocal => {
                "Expands to the nearest takeable position and defends reactively; never posts \
                 a rear guard (its exploitable seam)."
            }
            Roster::Cycler => {
                "Drills its surplus between its subs, masses everything on an attacked one, and \
                 strikes all-in only with crushing force — after a visible muster."
            }
            Roster::SimpleAdjacent { .. } => {
                "The Simple colonizer, leashed to its neighbourhood: it only attacks positions \
                 adjacent to its own ground — it crawls outward and never strikes across the \
                 middle."
            }
            Roster::SimpleColonize { .. } => {
                "Sizes each capture wave to the target's total resistance and fills nearest-first; \
                 keeps only a garrison floor. Blind spot: the thin-rear seam."
            }
        }
    }
}

/// A seat's AI driver — the roster→brain dispatch **the game and the headless validation
/// share**, so both field the *same* brain for a roster entry. [`Roster::SimpleColonize`] /
/// [`Roster::SimpleAdjacent`] map to the stateful [`crate::SimpleController`] (it carries a
/// departure ledger across ticks) and [`Roster::Cycler`] to the stateful
/// [`crate::cycler::CyclerController`]; hosts hold one `SeatController` per enemy seat and
/// step it with `&mut`.
#[derive(Clone)]
pub enum SeatController {
    /// The stateless controller (pure function of the observed interior).
    Stateless(AiController),
    /// The stateful campaign **Simple** ([`crate::SimpleController`]).
    Simple(crate::simple::SimpleController),
    /// The stateful scripted **Cycler** ([`crate::cycler::CyclerController`]).
    Cycler(crate::cycler::CyclerController),
}

impl SeatController {
    /// Build the right driver for `seat` from a roster entry.
    pub fn from_roster(seat: Faction, r: Roster) -> SeatController {
        match r {
            Roster::SimpleColonize { version } => {
                SeatController::Simple(crate::simple::SimpleController::new(seat, version))
            }
            Roster::Cycler => SeatController::Cycler(crate::cycler::CyclerController::new(seat)),
            Roster::SimpleAdjacent { range, version } => {
                SeatController::Simple(crate::simple::SimpleController::new_adjacent(
                    seat, range, version,
                ))
            }
            _ => SeatController::Stateless(AiController::from_roster(seat, r)),
        }
    }

    /// The seat this controller plays.
    pub fn seat(&self) -> Faction {
        match self {
            SeatController::Stateless(c) => c.seat,
            SeatController::Simple(c) => c.seat(),
            SeatController::Cycler(c) => c.seat,
        }
    }

    /// Decide and apply this seat's turn on the interior (mutates the ledger for the
    /// stateful variants). Returns the number of ships moved.
    pub fn decide_and_apply(&mut self, st: &mut Interior, sim: &SimParams) -> usize {
        match self {
            SeatController::Stateless(c) => c.decide_and_apply(st, sim),
            SeatController::Simple(c) => c.decide_and_apply(st, sim),
            SeatController::Cycler(c) => c.decide_and_apply(st, sim),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use layer1::{SubStructure, Vec2};

    #[test]
    fn greedy_controller_expands_toward_neutral() {
        // A player sub with surplus and a neutral sub nearby: the greedy should move ships.
        let mut st = Interior::new(1);
        let h = st.add_sub(SubStructure::new(Vec2::new(0.0, 0.0), 4.0, Faction::Player));
        let _n = st.add_sub(SubStructure::new(Vec2::new(8.0, 0.0), 4.0, Faction::Neutral));
        for _ in 0..12 {
            st.spawn_ship(Faction::Player, h);
        }
        let params = SimParams::default();
        let ctrl = AiController::from_roster(Faction::Player, Roster::GreedyLocal);
        let moved = ctrl.decide_and_apply(&mut st, &params);
        assert!(moved > 0, "greedy should issue an internal order toward the neutral");
    }

    #[test]
    fn passive_controller_is_inert() {
        let mut st = Interior::new(1);
        let h = st.add_sub(SubStructure::new(Vec2::new(0.0, 0.0), 4.0, Faction::Player));
        for _ in 0..12 {
            st.spawn_ship(Faction::Player, h);
        }
        let params = SimParams::default();
        let ctrl = AiController::from_roster(Faction::Player, Roster::Passive);
        assert_eq!(ctrl.decide_and_apply(&mut st, &params), 0, "passive issues nothing");
    }

    #[test]
    fn roster_names_and_descriptions_present() {
        for r in Roster::ALL {
            assert!(!r.name().is_empty());
            assert!(r.description().len() > 10);
        }
    }
}
