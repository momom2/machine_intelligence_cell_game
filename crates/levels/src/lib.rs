//! # levels — the level / campaign system for v1 of the cell-game RTS
//!
//! **Phase 3.** A headless, deterministic, fully-tested library that defines the game's
//! **10-level campaign**: for each level, the GUI-facing **metadata** (title, blurb, on-screen
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
//! let levels: Vec<Level> = campaign();          // the 10 levels, in order
//! let lvl = &levels[0];
//! println!("{} — {}", lvl.title, lvl.objective); // metadata drives the UI
//! let (mut world, wp) = (lvl.build)(42);         // instantiate the world (seeded)
//! let sim = SimParams::default();
//! // ... the host then runs the world: player orders + lvl.enemy Automaton, World::step,
//! //     and reports a WIN when World::outcome() favours Faction::Player.
//! # let _ = (&mut world, wp, sim, &lvl.enemy, lvl.start_view, lvl.automation_available, lvl.horizon);
//! ```
//!
//! The **player** is always [`Faction::Player`]; the **enemy** seat is [`Faction::Enemy`],
//! driven by the level's [`Level::enemy`] roster entry (the GUI builds an
//! [`ai::AiController::from_roster`] for it). A level is **won** when [`world::World::outcome`]
//! reports [`Faction::Player`].
//!
//! ## The curriculum (built exactly; see `LEVELS.md` for topology + measured validation)
//!
//! 1. **First Moves** (Layer-1, Passive) — select a sub, send a fraction, capture.
//! 2. **Contact** (Layer-1, GreedyLocal) — concentration of force; layout decides who fights.
//! 3. **Two Worlds** (Layer-2, GreedyLocal) — inter-planet fleets, zoom-to-micro, automation.
//! 4. **Hold the Line** (Layer-2, GreedyLocal) — reinforce L3, lean on automation.
//! 5. **Three Fronts** (Layer-2, GreedyLocal) — multi-front concentration on a triangle.
//! 6. **The Prize** (Layer-2, GreedyLocal) — expansion-vs-defense timing around a fat neutral.
//! 7. **The Seam** (Layer-2, GreedyLocal) — exploit greedy's undefended rear (a flank).
//! 8. **Overreach** (Layer-2, Colonize) — strike undefended production (attack > colonize).
//! 9. **The Turtle** (Layer-2, Defend) — out-expand a turtle (colonize > defend).
//! 10. **The Hammer** (Layer-2, Attack) — punish the over-committed stack (defend > attack).
//!
//! ## Validation
//!
//! The real test is headless ([`validation`], asserted by the lib tests): every level **builds
//! without panic and matches its spec** (planet count, per-planet sub counts, ownership, lane
//! connectivity), is **deterministic** (identical `state_hash` across rebuilds + replays), and
//! is **sane as a curriculum** — the intended lesson actually holds when measured against AI
//! proxies (the L8/L9/L10 counters beat their pure Automaton on the level map, a rear-flank
//! beats greedy on L7, and L1-L6 are not auto-lost by a competent player proxy).

pub mod builders;
pub mod campaign;
pub mod validation;

use layer1::Faction;
use world::{PlanetId, World, WorldParams};

// Flat re-exports of the items a host (GUI / tests) consumes.
pub use campaign::campaign;
pub use validation::{validate_campaign, validate_level, LevelReport};

// Re-export the substrate types a host needs to *use* a Level without depending on the inner
// crates directly (convenience; the inner crates are still available).
pub use ai::Roster;

/// Where the camera **opens** when a level starts — the lens the player begins in.
///
/// Layer 1 is the *embodied / micro* view of a single structure's sub-structures (the tutorials
/// start here so movement and combat are taught up close); Layer 2 is the *tactical* zoomed-out
/// view over planets and lanes. The host honours this when it first shows the level; the player
/// can still zoom between layers afterwards (Layer-2 levels let you zoom *into* a planet, which
/// is the Layer-1 view of that planet).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StartView {
    /// Open zoomed **into** the given planet's Layer-1 structure (its sub-structures). The
    /// single-planet tutorials use `Layer1(0)` — the only planet.
    Layer1(PlanetId),
    /// Open in the Layer-2 tactical view (planets + lanes).
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
    /// The enemy Automaton this level fields (the [`Faction::Enemy`] seat). The GUI builds an
    /// [`ai::AiController::from_roster(Faction::Enemy, enemy)`](ai::AiController::from_roster).
    pub enemy: Roster,
    /// Which lens the camera opens in (see [`StartView`]).
    pub start_view: StartView,
    /// Whether this level offers the player the optional **basic automation** toggle (delegate a
    /// planet's internal play to the Layer-1 greedy adapter). Introduced at L3; off for the
    /// L1/L2 micro tutorials where the player must drive every move themselves.
    pub automation_available: bool,
    /// The match horizon in ticks: if neither side is eliminated by now, the winner is decided
    /// on [`world::World::outcome`]'s lead. Sized per level for fair pacing.
    pub horizon: u64,
    /// Build this level's world (and its inter-planet [`WorldParams`]) from `seed`. A bare `fn`
    /// pointer: deterministic, allocation-light, and safe to call repeatedly (each call yields a
    /// fresh, independent world). The player seat is [`Faction::Player`]; the enemy is
    /// [`Faction::Enemy`].
    pub build: fn(seed: u64) -> (World, WorldParams),
}

impl Level {
    /// Convenience: build this level's world with `seed` (just calls [`Level::build`]).
    pub fn world(&self, seed: u64) -> (World, WorldParams) {
        (self.build)(seed)
    }

    /// The player seat (always [`Faction::Player`]). A tiny helper so hosts/tests do not
    /// hard-code the convention.
    #[inline]
    pub fn player_seat(&self) -> Faction {
        Faction::Player
    }

    /// The enemy seat (always [`Faction::Enemy`]).
    #[inline]
    pub fn enemy_seat(&self) -> Faction {
        Faction::Enemy
    }
}

impl std::fmt::Debug for Level {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Custom Debug so the `fn` pointer does not render as an opaque address-only field.
        f.debug_struct("Level")
            .field("id", &self.id)
            .field("title", &self.title)
            .field("enemy", &self.enemy)
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

    /// Every level builds, matches its structural spec, and is deterministic; every intended
    /// lesson holds (the per-level assertions live in [`validation::validate_level`]). This is
    /// the one test that fails loudly if any level's world or lesson regresses.
    #[test]
    fn campaign_is_well_formed_and_lessons_hold() {
        let levels = campaign();
        assert_eq!(levels.len(), 10, "the campaign must have exactly 10 levels");
        for (i, lvl) in levels.iter().enumerate() {
            assert_eq!(lvl.id as usize, i + 1, "level ids must be 1..=10 in order");
            let report = validate_level(lvl);
            assert!(
                report.ok(),
                "level {} ({}) failed validation: {:#?}",
                lvl.id,
                lvl.title,
                report
            );
        }
    }

    /// Metadata sanity: titles/blurbs/objectives non-empty, every level has at least one hint,
    /// the player/enemy seat conventions hold, and only the L1/L2 micro tutorials hide
    /// automation.
    #[test]
    fn metadata_is_complete() {
        for lvl in campaign() {
            assert!(!lvl.title.is_empty(), "L{} has an empty title", lvl.id);
            assert!(lvl.blurb.len() > 10, "L{} blurb too short", lvl.id);
            assert!(lvl.objective.len() > 10, "L{} objective too short", lvl.id);
            assert!(!lvl.hints.is_empty(), "L{} has no hints", lvl.id);
            assert_eq!(lvl.player_seat(), Faction::Player);
            assert_eq!(lvl.enemy_seat(), Faction::Enemy);
            assert!(lvl.horizon >= 1000, "L{} horizon implausibly small", lvl.id);
            // The two single-structure tutorials open in Layer 1; the rest open in Layer 2.
            match lvl.id {
                1 | 2 => {
                    assert!(matches!(lvl.start_view, StartView::Layer1(_)));
                    assert!(!lvl.automation_available, "L{} should not offer automation", lvl.id);
                }
                _ => {
                    assert_eq!(lvl.start_view, StartView::Layer2);
                    assert!(lvl.automation_available, "L{} should offer automation", lvl.id);
                }
            }
        }
    }

    /// Report-only: print the full per-level validation report (the measured curriculum
    /// numbers). Not an extra set of assertions — `campaign_is_well_formed_and_lessons_hold`
    /// already asserts every level passes; this exists so `--nocapture` surfaces the exact
    /// win-loss margins quoted in `LEVELS.md`.
    #[test]
    fn print_validation_report() {
        println!("\n=== levels: campaign validation report ===");
        for r in validate_campaign() {
            println!(
                "L{:>2} {:<14} structure:{} deterministic:{} lesson:{}",
                r.id,
                r.title,
                if r.structure_ok { "ok" } else { "FAIL" },
                if r.deterministic { "ok" } else { "FAIL" },
                if r.lesson_ok { "ok" } else { "FAIL" },
            );
            println!("      lesson: {}", r.lesson_detail);
            if let Some(d) = &r.structure_detail {
                println!("      structure mismatch: {d}");
            }
        }
        println!("===========================================\n");
    }
}
