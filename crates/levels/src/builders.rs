//! Shared **world-authoring helpers** for the campaign `build` functions.
//!
//! Every mission now authors its subs inline in [`crate::campaign`] (explicit local
//! coordinates — the proximity geometry *is* the level design), so only the shared
//! parameter constructor lives here. The old multi-struct cluster helpers
//! (`stocked_struct` / `neutral_struct` / `diamond`) were deleted with the L8-L13
//! placeholders they served (owner, 2026-07-07).

use world::WorldParams;

/// The default inter-struct dials for every level (the operating point the `world`/`ai` suites
/// validated). Levels return this alongside their `World` so the host drives both with one
/// consistent parameter set.
pub fn default_world_params() -> WorldParams {
    WorldParams::default()
}
