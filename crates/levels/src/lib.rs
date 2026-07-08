//! # levels — the level / campaign system for v1 of the cell-game RTS
//!
//! **Phase 3.** A headless, deterministic, fully-tested library that defines the game's
//! **7-level campaign**: for each level, the GUI-facing **metadata** (title, blurb, on-screen
//! objective, tutorial hints, the chosen enemy [`ai::Roster`], where the camera opens, whether
//! basic automation is offered, and the match horizon) plus a bare
//! `build: fn(seed) -> (World, WorldParams)` **world-builder** that instantiates the level's
//! [`world::World`] on demand. No graphics — this crate is the *content + its validation*, sat
//! on top of the substrate ([`world`], [`ai`], [`layer1`]).
//!
//! ## What the GUI consumes
//!
//! ```no_run
//! use levels::{campaign, Level, StartView};
//! use layer1::{Faction, SimParams};
//!
//! let levels: Vec<Level> = campaign();          // the 7 levels, in order
//! let lvl = &levels[0];
//! println!("{} — {}", lvl.title, lvl.objective); // metadata drives the UI
//! let (mut world, wp) = (lvl.build)(42);         // instantiate the world (seeded)
//! let sim = SimParams::default();
//! // ... the host then runs the world: player orders + the lvl.enemies Automata, World::step,
//! //     and reports a WIN when World::outcome() favours Faction::Player.
//! # let _ = (&mut world, wp, sim, &lvl.enemies, lvl.start_view, lvl.automation_available, lvl.horizon);
//! ```
//!
//! The **player** is always [`Faction::Player`]; the **enemy seats** are `Faction::Ai(i)`, one
//! per [`Level::enemies`] entry, driven through the shared [`ai::SeatController`] dispatch. A
//! level is **won** when [`world::World::outcome`] reports [`Faction::Player`].
//!
//! ## The curriculum
//!
//! See the table in [`campaign`](crate::campaign)'s module doc and `LEVELS.md` (the tutorial-arc
//! plan + the as-built topology table). All seven missions are hand-authored (the old L8-L13
//! placeholders were deleted, 2026-07-07).
//!
//! ## Validation
//!
//! The real test is headless ([`validation`], asserted by the lib tests): every level **builds
//! without panic** and is **deterministic** (identical `state_hash` across rebuilds + replays).
//! That is the only gate — **balance is never tested** (owner rule: all balancing is per-level,
//! by hand; the old winnability/lesson proxies were removed 2026-07-06).

pub mod builders;
pub mod campaign;
pub mod spec;
pub mod validation;

use layer1::Faction;
use world::{StructId, World, WorldParams};

// Flat re-exports of the items a host (GUI / tests) consumes.
pub use campaign::campaign;
pub use validation::{validate_level_gates, LevelReport};

// Re-export the substrate types a host needs to *use* a Level without depending on the inner
// crates directly (convenience; the inner crates are still available).
pub use ai::Roster;

/// Where the camera **opens** when a level starts — the lens the player begins in.
///
/// Layer 1 is the *embodied / micro* view of a single structure's sub-structures (the tutorials
/// start here so movement and combat are taught up close); Layer 2 is the *tactical* zoomed-out
/// view over structs and lanes. The host honours this when it first shows the level; the player
/// can still zoom between layers afterwards (Layer-2 levels let you zoom *into* a structure, which
/// is the Layer-1 view of that structure).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StartView {
    /// Open zoomed **into** the given struct's Layer-1 structure (its sub-structures). The
    /// single-struct tutorials use `Layer1(0)` — the only structure.
    Layer1(StructId),
    /// Open in the Layer-2 tactical view (structs + lanes).
    Layer2,
}

/// One campaign level: GUI-facing metadata plus a world-builder.
///
/// This is the unit the GUI reads to drive its UI (title/blurb/objective/hints, which opponent
/// to instantiate, where to open the camera, whether to offer the automation toggle, and the
/// match horizon) and calls [`Level::build`] on to instantiate the playable
/// [`world::World`]. The struct is deliberately plain data + one `fn` pointer so it is trivially
/// `Clone`/inspectable and carries no hidden state.
#[derive(Clone)]
pub struct Level {
    /// Stable 1-based level number (also its position in [`campaign`]).
    pub id: u32,
    /// Short display title (e.g. `"First Moves"`).
    pub title: String,
    /// A 1-2 sentence intro shown before / framing the level (flavour + setup).
    pub blurb: String,
    /// The short on-screen goal (what the player must achieve to win this level).
    pub objective: String,
    /// Tutorial pointers — concrete things to teach (controls, tactics). Shown as a tip list.
    pub hints: Vec<String>,
    /// The AI opponents this level fields, in seat order: `enemies[i]` drives the `Faction::Ai(i)`
    /// seat. One entry is the usual single-enemy duel; two-plus make a **free-for-all** (every real
    /// seat fights the others). **This list is where the number of enemies is decided** — the engine
    /// reads it and is otherwise agnostic to seat count; nothing below the level layer hardcodes it.
    /// The GUI builds one controller per entry.
    pub enemies: Vec<Roster>,
    /// Which lens the camera opens in (see [`StartView`]).
    pub start_view: StartView,
    /// Whether this level offers the player the optional **basic automation** toggle (delegate a
    /// struct's internal play to the Layer-1 greedy adapter). **PARKED — currently `false` on every
    /// level**: the basic-automation feature is quarantined pending a proper redesign. The `game`
    /// wiring (toggle / per-tick run / AUTO render) is all gated on this flag, so it stays inert. The
    /// Layer-1 greedy adapter it used (`ai::greedy_layer1_orders`) is still live as the AI's tactical.
    pub automation_available: bool,
    /// The match horizon in ticks: if neither side is eliminated by now, the winner is decided
    /// on [`world::World::outcome`]'s lead. Sized per level for fair pacing.
    pub horizon: u64,
    /// Presentation dial: this mission's **minimum zoom** (the farthest out-zoom multiplier on
    /// the fitted camera), overriding the game's global floor. `None` = the game default.
    /// Purely visual — the sim never reads it.
    pub zoom_min: Option<f32>,
    /// Where this level's world comes from (see [`LevelSource`]): a parsed data-file spec
    /// (the campaign) or a built-in `fn` (dev scenarios like the arena / selftest worlds).
    /// Either way [`Level::world`] is deterministic and safe to call repeatedly.
    pub source: LevelSource,
}

/// The origin of a level's world-builder.
#[derive(Clone)]
pub enum LevelSource {
    /// A parsed `.lvl` data file (see [`spec`]) — the campaign path: tweaking the file needs
    /// no recompile. `Arc` keeps `Level: Clone` cheap.
    Spec(std::sync::Arc<spec::LevelSpec>),
    /// A hand-written builder `fn` — the dev scenarios the game constructs in code (the
    /// reserve-combat arena, the selftest automation world).
    Builtin(fn(seed: u64) -> (World, WorldParams)),
}

impl Level {
    /// Build this level's world with `seed` (dispatching on [`Level::source`]).
    pub fn world(&self, seed: u64) -> (World, WorldParams) {
        match &self.source {
            LevelSource::Spec(sp) => sp.build(seed),
            LevelSource::Builtin(f) => f(seed),
        }
    }

    /// The player seat (always [`Faction::Player`]). A tiny helper so hosts/tests do not
    /// hard-code the convention.
    #[inline]
    pub fn player_seat(&self) -> Faction {
        Faction::Player
    }

    /// The first enemy seat (always [`Faction::Ai(0)`]).
    #[inline]
    pub fn enemy_seat(&self) -> Faction {
        Faction::Ai(0)
    }

}

impl std::fmt::Debug for Level {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Custom Debug so the `fn` pointer does not render as an opaque address-only field.
        f.debug_struct("Level")
            .field("id", &self.id)
            .field("title", &self.title)
            .field("enemies", &self.enemies)
            .field("start_view", &self.start_view)
            .field("automation_available", &self.automation_available)
            .field("horizon", &self.horizon)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    //! Headless lib tests — the real validation of the campaign. Kept as **lib** tests (not a
    //! separate `tests/` integration target) so they run reliably under the Windows app-control
    //! policy that can block freshly-linked standalone binaries (see `LEVELS.md`).
    use super::*;

    /// Every level builds and replays deterministically — the one gate a level must pass
    /// ([`validation::validate_level_gates`]). Balance is never tested (owner rule: all
    /// balancing is per-level, by hand).
    #[test]
    fn campaign_is_well_formed() {
        let levels = campaign();
        assert_eq!(levels.len(), 7, "the campaign must have exactly 7 levels");
        for (i, lvl) in levels.iter().enumerate() {
            assert_eq!(lvl.id as usize, i + 1, "level ids must be 1..=7 in order");
            let report = validation::validate_level_gates(lvl);
            assert!(
                report.ok(),
                "level {} ({}) failed validation: {:#?}",
                lvl.id,
                lvl.title,
                report
            );
        }
    }

    /// Engine-facing INVARIANTS only (owner rule: tests never pin content a designer edits —
    /// titles, blurbs, hints, horizons and start views are the .lvl files' business): the
    /// player/enemy seat conventions hold, and basic automation stays parked.
    #[test]
    fn seat_and_automation_invariants_hold() {
        for lvl in campaign() {
            assert_eq!(lvl.player_seat(), Faction::Player);
            assert_eq!(lvl.enemy_seat(), Faction::Ai(0));
            // Basic automation is PARKED — no level offers it (quarantined pending redesign).
            assert!(!lvl.automation_available, "automation is parked; L{} must not offer it", lvl.id);
        }
    }

}
