//! The campaign **loader**: the 7 missions are DATA — one `.lvl` file each under
//! `assets/levels/`, parsed by [`crate::spec`] at startup and interpreted into a
//! [`world::World`] on demand (owner ask, 2026-07-08: tweaking a level must not cost a
//! recompile — edit the file, restart the game). The old hand-written `build_*` functions
//! were deleted after a structural migration check verified the files reproduce them
//! sub-for-sub; `git log` has both.
//!
//! The campaign (see `LEVELS.md` for the full table and the tutorial-arc plan). L1 = Passive,
//! L2 = Simple, L3 = the scripted Cycler, L4/L5 = Simple, L6 = Simple ×2 (free-for-all),
//! L7 = the adjacency-leashed Simple + a Passive fort seat. All seven are hand-authored;
//! every mission opens in Layer 1:
//!
//! | # | Title | Enemies | File |
//! |---|---|---|---|
//! | 1 | First steps | Passive | `01_first_steps.lvl` |
//! | 2 | Fire in the sky | Simple | `02_fire_in_the_sky.lvl` |
//! | 3 | Command and Control | Cycler | `03_command_and_control.lvl` |
//! | 4 | The Sinews of War | Simple | `04_the_sinews_of_war.lvl` |
//! | 5 | Head of the Snake | Simple | `05_head_of_the_snake.lvl` |
//! | 6 | Deliberation | Simple ×2 | `06_deliberation.lvl` |
//! | 7 | Far far away | SimpleAdjacent + Passive | `07_far_far_away.lvl` |

use crate::{spec, Level, LevelSource};

// ======================================================================================
// The campaign LOADER (data-driven, 2026-07-08).
// ======================================================================================

/// The directory holding the mission files: `assets/levels/*.lvl`, next to the executable
/// (a shipped layout) or in the workspace tree (dev runs and tests — the path is baked at
/// compile time from this crate's manifest dir).
fn assets_dir() -> std::path::PathBuf {
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let p = dir.join("assets").join("levels");
            if p.is_dir() {
                return p;
            }
        }
    }
    let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/levels");
    if p.is_dir() {
        return p;
    }
    panic!("assets/levels not found (looked next to the executable and in the workspace tree)")
}

/// The campaign levels, in play order — **loaded from the mission files** (owner ask,
/// 2026-07-08: tweaking a level must not cost a recompile). Files are read from
/// [`assets_dir`], sorted by filename (the `NN_` prefix is the play order), parsed by
/// [`spec::parse`], and interpreted at build time by [`spec::LevelSpec::build`]. A malformed
/// or missing file panics with the file name and line — a content bug must be loud, and the
/// levels validation suite exercises every file.
pub fn campaign() -> Vec<Level> {
    let dir = assets_dir();
    let mut files: Vec<std::path::PathBuf> = std::fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", dir.display()))
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "lvl"))
        .collect();
    files.sort();
    files
        .iter()
        .map(|f| {
            let text = std::fs::read_to_string(f)
                .unwrap_or_else(|e| panic!("cannot read {}: {e}", f.display()));
            let sp = spec::parse(&text)
                .unwrap_or_else(|e| panic!("{}: {e}", f.display()));
            Level {
                id: sp.id,
                title: sp.title.clone(),
                blurb: sp.blurb.clone(),
                objective: sp.objective.clone(),
                hints: sp.hints.clone(),
                enemies: sp.enemies.clone(),
                start_view: sp.start_view,
                automation_available: false, // PARKED: quarantined pending redesign
                horizon: sp.horizon,
                zoom_min: sp.zoom_min,
                source: LevelSource::Spec(std::sync::Arc::new(sp)),
            }
        })
        .collect()
}

