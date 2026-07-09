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

/// The directory holding the mission files, resolved RELATIVE-first (owner requirement,
/// 2026-07-08 - a downloaded repo must play wherever it lands): (1) next to the executable
/// (the shipped layout: game.exe beside assets/), (2) under the **current working
/// directory** (a dev shortcut whose WorkingDirectory is the repo root reaches the assets
/// even when the exe sits in target/release/), (3) the workspace tree baked at compile
/// time from this crate's manifest dir (dev runs and tests - machine-specific, last resort).
fn assets_dir() -> std::path::PathBuf {
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let p = dir.join("assets").join("levels");
            if p.is_dir() {
                return p;
            }
        }
    }
    if let Ok(cwd) = std::env::current_dir() {
        let p = cwd.join("assets").join("levels");
        if p.is_dir() {
            return p;
        }
    }
    let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/levels");
    if p.is_dir() {
        return p;
    }
    panic!("assets/levels not found (looked next to the executable, under the current directory, and in the workspace tree)")
}

/// The campaign levels, in play order — **loaded from the mission files** (owner ask,
/// 2026-07-08: tweaking a level must not cost a recompile). Each mission lives in its own
/// DIRECTORY under `assets/levels/` (owner reorg, 2026-07-08 — per-level asset bundles):
///
/// ```text
/// assets/levels/01_first_steps/01_first_steps.lvl        the world (required)
///                              01_first_steps_pre.brf    pre-mission briefing (optional)
///                              01_first_steps_post.brf   post-battle briefing template (optional)
/// ```
///
/// Directories are sorted by name (the `NN_` prefix is the play order); a loose `*.lvl`
/// directly under `assets/levels/` still loads (sorted among the directories by name). A
/// malformed or missing world file panics with the file name and line — a content bug must
/// be loud, and the levels validation suite exercises every file.
pub fn campaign() -> Vec<Level> {
    let dir = assets_dir();
    // (sort key, .lvl path) — one entry per mission directory (its single .lvl) or loose file.
    let mut files: Vec<(String, std::path::PathBuf)> = Vec::new();
    for entry in std::fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", dir.display()))
        .filter_map(|e| e.ok().map(|e| e.path()))
    {
        let key = entry.file_name().unwrap_or_default().to_string_lossy().into_owned();
        if entry.is_dir() {
            let mut lvls: Vec<std::path::PathBuf> = std::fs::read_dir(&entry)
                .unwrap_or_else(|e| panic!("cannot read {}: {e}", entry.display()))
                .filter_map(|e| e.ok().map(|e| e.path()))
                .filter(|p| p.extension().is_some_and(|x| x == "lvl"))
                .collect();
            match lvls.len() {
                0 => {} // not a mission directory (e.g. future shared assets) — skip
                1 => files.push((key, lvls.remove(0))),
                _ => panic!("{}: a mission directory must hold exactly one .lvl", entry.display()),
            }
        } else if entry.extension().is_some_and(|x| x == "lvl") {
            files.push((key, entry));
        }
    }
    files.sort_by(|a, b| a.0.cmp(&b.0));
    files
        .iter()
        .map(|(_, f)| {
            let text = std::fs::read_to_string(f)
                .unwrap_or_else(|e| panic!("cannot read {}: {e}", f.display()));
            let sp = spec::parse(&text)
                .unwrap_or_else(|e| panic!("{}: {e}", f.display()));
            // Narrative companions, next to the world file: `<stem>_pre.brf` / `<stem>_post.brf`
            // (`.brf` = authored briefing templates, pre and post — the format contract;
            // `.glg` is reserved for game-generated stats). Loaded verbatim; the game
            // renders them — and only under `--text`.
            let stem = f.file_stem().unwrap_or_default().to_string_lossy().into_owned();
            let companion = |suffix: &str| -> Option<String> {
                let p = f.with_file_name(format!("{stem}{suffix}"));
                std::fs::read_to_string(p).ok()
            };
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
                briefing: companion("_pre.brf"),
                post_log: companion("_post.brf"),
                source: LevelSource::Spec(std::sync::Arc::new(sp)),
            }
        })
        .collect()
}

