//! **Simple** — the stateful campaign colonizer, kept as FROZEN VERSIONS.
//!
//! Owner ruling (2026-07-24): Simple had grown into one 1700-line brain serving five missions, so
//! every tuning pass silently re-balanced missions it was never meant to touch (the teleporter
//! leash fix that prompted this changed Deliberation as well as Far Far Away). Each version is
//! therefore a **complete, frozen snapshot** in its own module, and a level pins the version it
//! was balanced against — a shipped mission never changes again.
//!
//! ## Working with versions
//!
//! * **Never edit a version a shipped level pins.** Fixing or retuning the brain = COPY the
//!   newest module to the next `vN`, change *that*, and re-pin only the mission being tuned.
//!   (Copy-on-write: versions fork when a change would disturb a balanced mission, not before.)
//! * A version owns its whole brain — decision program, dials, and its own tests. `v1`'s tests
//!   live in `v1.rs` and pin `v1` forever.
//! * [`SimpleVersion`] is the identity a level declares (`.lvl`: `enemy = simple v1`), carried on
//!   the [`crate::Roster`] entry and dispatched here.

pub mod v1;

use layer1::{Faction, Interior, SimParams};

/// Which frozen snapshot of the Simple brain a seat fields. Declared per level and never changed
/// for a shipped mission — see the module docs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SimpleVersion {
    /// The original brain (everything through 2026-07-24, incl. the unbound-teleporter leash).
    #[default]
    V1,
}

impl SimpleVersion {
    /// The `.lvl` token / display name (`"v1"`).
    pub fn name(self) -> &'static str {
        match self {
            SimpleVersion::V1 => "v1",
        }
    }

    /// Parse a `.lvl` version token; `None` if unknown (the parser reports it with file+line).
    pub fn parse(s: &str) -> Option<SimpleVersion> {
        match s {
            "v1" => Some(SimpleVersion::V1),
            _ => None,
        }
    }
}

/// The version-dispatching Simple controller: one variant per frozen brain. Hosts hold this and
/// never see which version they are driving.
#[derive(Clone)]
pub enum SimpleController {
    V1(v1::SimpleController),
}

impl SimpleController {
    /// The plain colonizer of `version`.
    pub fn new(seat: Faction, version: SimpleVersion) -> SimpleController {
        match version {
            SimpleVersion::V1 => SimpleController::V1(v1::SimpleController::new(seat)),
        }
    }

    /// The adjacency-leashed variant of `version` (targets/funding restricted to within `range`
    /// of owned ground; an owned teleporter is exempt).
    pub fn new_adjacent(seat: Faction, range: f32, version: SimpleVersion) -> SimpleController {
        match version {
            SimpleVersion::V1 => {
                SimpleController::V1(v1::SimpleController::new_adjacent(seat, range))
            }
        }
    }

    /// The seat this brain plays.
    pub fn seat(&self) -> Faction {
        match self {
            SimpleController::V1(c) => c.seat,
        }
    }

    /// Run one decision tick for this seat, applying the orders. Returns the ships moved.
    pub fn decide_and_apply(&mut self, st: &mut Interior, sp: &SimParams) -> usize {
        match self {
            SimpleController::V1(c) => c.decide_and_apply(st, sp),
        }
    }
}

/// The dial set of the CURRENT (newest) version — what hosts and the harness mean by "Simple's
/// params". A future version with a different dial set gets its own type; this alias then names
/// whichever is current.
pub use v1::SimpleParams;
