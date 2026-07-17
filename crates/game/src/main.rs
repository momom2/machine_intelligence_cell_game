#![cfg_attr(windows, windows_subsystem = "windows")]
//! # game — the complete single-binary v1 cell-game RTS (the PLAYABLE product)
//!
//! This is the macroquad GUI that assembles the headless substrate into the actual game. Per the
//! design's signature principle (`00-overview.md`, *decouple computation from spectacle*): **all**
//! model logic lives in the headless crates ([`levels`] builds the world; [`world`] steps it;
//! [`ai`] drives the enemy and the player's optional automation; [`layer1`] is the per-structure
//! sim). This crate only (a) draws the two layers and (b) turns human input into the same orders
//! the AI uses. It changes nothing about the model — it owns only spectacle data (node/camera
//! layout, GUI tick pace, menu/zoom state).
//!
//! ## App state machine
//! `MainMenu -> LevelSelect -> InLevel -> (Victory | Defeat)`, plus an in-level Pause overlay.
//! Beating level N unlocks N+1; progress is persisted to `mi_progress.json` next to the exe.
//!
//! ## The zoomable two-layer match (the core)
//! ONE [`world::World`] (built by the level). The sim always runs the same; the camera has two
//! zoom states:
//! * **Layer-2 lens** (zoomed out) — structs as nodes coloured by [`world::StructAggregate`]
//!   owner, lanes, and inter-struct [`world::InterFleet`]s interpolated along lanes. Click an
//!   owned struct then a lane-connected struct to issue a [`world::FleetOrder`].
//! * **Layer-1 interior** (zoomed into one structure) — that struct's [`layer1::Interior`] drawn
//!   exactly like the standalone Layer-1 game (sub-structures, ships, battle bubbles). Click a
//!   sub then another sub to issue a [`layer1::MoveOrder`].
//! The camera lerps smoothly between the two. `automation_available` levels let the player toggle
//! AUTO on an owned struct (key `A`) so its internals are driven by the Layer-1 greedy adapter —
//! the same policy the enemy uses.
//!
//! ## Verification modes (screenshot-checkable, mirrors layer1/2-game)
//! `--shot <path>` render one frame and exit; `--screen <menu|select>`; `--level <N>`;
//! `--view <lens|interior>`; `--at-tick <T>`; `--auto` (both seats AI); `--seed <S>`.

use ai::{AiController, GreedyParams, SeatController};
use layer1::{Faction, SimParams};
use levels::{campaign, Level, StartView};
use macroquad::prelude::*;
use world::{StructId, StructOwner, World, WorldParams};

// =============================================================================================
// Pacing & tuning constants (the spectacle layer's operating point — see GAME.md)
// =============================================================================================

/// **THE sim tickrate knob** — logical ticks per real second. All game behaviour is grounded at this
/// rate; change this one number to re-pace the whole sim. Default **60**. The wall-clock behaviour is
/// **independent** of `TICK_HZ`: every per-tick quantity is scaled by `TICK_SCALE = TICK_HZ / REF_HZ`
/// (per-tick rates ÷ it, periods/tick-counts × it), so the per-*second* behaviour stays fixed — a
/// finer `TICK_HZ` just steps the same motion in smaller, smoother increments.
const TICK_HZ: f64 = 60.0;

/// The historical reference rate the **unscaled** `SimParams::default()` / `WorldParams::default()`
/// are grounded at. Those defaults (and the AI / sim / levels test suites) are never touched — only
/// the game's operating point (`gui_params`, `build_scaled`, `scaled_horizon`) diverges, by `scale`.
const REF_HZ: f64 = 2.5;

/// The game's tick-rate scale vs. the unscaled reference: `TICK_HZ / REF_HZ` (= 24 at 60 Hz). Per-tick
/// rates are ÷ this; periods / tick-counts / horizons are × this. The headless tools use `scale = 1.0`
/// (the coarse reference resolution) so they reach the same logical outcome in ~`scale`× fewer ticks.
const TICK_SCALE: f64 = TICK_HZ / REF_HZ;
/// Integer view of [`TICK_SCALE`] for tick-counts / periods (exact at the default 60/2.5 = 24).
const TICK_SCALE_U: u32 = (TICK_HZ / REF_HZ) as u32;

/// Base world rate in ticks per real second — equals [`TICK_HZ`] (the count-up clock divides by it).
const BASE_TICKS_PER_SEC: f64 = TICK_HZ;

/// Seconds of sim advanced per fixed logical tick (the fixed-timestep quantum).
const FIXED_DT: f64 = 1.0 / TICK_HZ;
/// Overload clamp: the most **sim-seconds** a single frame may consume (`dt × speed`, capped here).
/// Beyond it the game **slows** (consumes less real time than elapsed) rather than dropping ticks —
/// so the sim never skips a tick and behaviour is identical at any framerate, just dilated under
/// sustained overload. It also caps the per-frame fast-forward, so it must clear the top speed at the
/// target framerate: `25x ÷ 60 fps ≈ 0.42 s`, so `0.5` realises the full 25x stop at ≥60 fps (and a
/// hitch buys at most 0.5 s = 30 ticks of catch-up in one frame).
const MAX_FRAME_DT: f64 = 0.5;
/// Wall-clock budget (ms) for the sim-drain loop inside one rendered frame — the frame-budget
/// governor's dial (see `Game::update`). 12 ms leaves ~4 ms of a 60 fps frame for rendering;
/// a machine whose ticks fit the budget never notices it.
const SIM_BUDGET_MS: f32 = 12.0;

/// The discrete speed-slider stops (also cycled by -/+). There is **no 0x stop** (owner,
/// 2026-07-06): pausing is its own state — the `P` overlay pause ([`Game::paused`]), separate
/// from the running speed, which it preserves. SPACE toggles 1x ⇄ the last non-1x stop.
const SPEED_STEPS: [f64; 4] = [1.0, 3.0, 10.0, 25.0];
/// Index into [`SPEED_STEPS`] the game starts at (1.0x).
const DEFAULT_SPEED_IDX: usize = 0;

/// Ticks the enemy seats sit IDLE at match start (2 s at 1x): the player gets a beat to read
/// the board before Simple's first orders land. Game-loop QoL only — the harness/validation
/// paths drive seats directly and are unaffected.
const ENEMY_GRACE_TICKS: u64 = (2.0 * TICK_HZ) as u64;

/// Lifetime (seconds) of a ship-death flash (white cross + enemy line), independent of game speed.
const KILL_FX_TTL: f64 = 0.35;

/// Lifetime (seconds) of a teleport flash — the transient white line from the gate to the
/// arrival point, quickly fading (wall-clock, like the kill flashes).
const TELEPORT_FX_TTL: f64 = 0.4;

/// Base (unscaled) decision cadence: run the seat decisions (enemy AI + player automation) every
/// this-many **reference** ticks. A `Game` scales it by its `scale` (`DECISION_BASE × scale`).
/// Sourced from the `ai` harness so the headless levels validation measures at the same cadence
/// the shipped game plays.
const DECISION_BASE: u64 = ai::harness::GAME_DECISION_BASE;

/// Headless tick budget per level for `--selftest`. It runs at the coarse `scale=1`
/// resolution and stop at `min(horizon, this)` so the whole suite finishes in a couple of seconds.
/// Determinism, automation-expansion and the early-game balance lead are all visible well within it
/// (cf. the 300-tick determinism cap in `levels::validation`); the trade is that a capped run shows
/// an *early/mid-game* lead, not the sealed final outcome of the long levels.
const HEADLESS_TICK_CAP: u64 = 700;

/// Ticks the `--shot --at-tick` capture advances per frame while racing to its target tick
/// (independent of wall-clock pacing, so capture is fast and deterministic).
const SHOT_TICKS_PER_FRAME: u64 = 8 * TICK_SCALE_U as u64;
/// Extra frames rendered after reaching the capture point so the framebuffer is fully drawn.
const SHOT_SETTLE_FRAMES: u32 = 2;

/// Camera zoom-lerp speed (fraction toward target per second-ish; applied via exp smoothing).
const ZOOM_LERP_RATE: f32 = 7.0;
/// Scale (lens-world-units -> pixels multiplier relative to a single struct's local span) the
/// interior view zooms in to. Larger = the struct fills more of the screen.
const INTERIOR_FILL: f32 = 0.80;
/// `cam_t` (0 = full lens, 1 = full interior) above which the interior scene is drawn.
const INTERIOR_DRAW_THRESHOLD: f32 = 0.55;

/// Zoom-slider magnification range applied to a layer's fitted camera (`1.0` = full fit, no zoom).
/// The interior fit frames the TACTICAL cluster (the reserve ring is excluded — see
/// [`interior_camera`]), so the out-end must reach far enough to bring the whole reserve ring
/// on screen (~0.3× at the corrected game scale); `7.0` zooms in 7×.
// The global out-zoom floor (owner-tuned by playtest: 0.2 → 0.25 → 0.35 → 0.6). A mission can
// tighten it further via `Level::zoom_min` (Far far away runs at 0.8); [`Game::zoom_min`]
// resolves the effective floor — every interactive clamp goes through it.
mod narrative;
mod replay;

/// Wall-clock for the PERF diagnostics: `std::time::Instant` traps on wasm, so the browser
/// build times with macroquad's `get_time()` instead (same numbers, coarser clock).
#[derive(Clone, Copy)]
struct PerfInstant {
    #[cfg(not(target_arch = "wasm32"))]
    t: std::time::Instant,
    #[cfg(target_arch = "wasm32")]
    t: f64,
}

impl PerfInstant {
    fn now() -> PerfInstant {
        #[cfg(not(target_arch = "wasm32"))]
        {
            PerfInstant { t: std::time::Instant::now() }
        }
        #[cfg(target_arch = "wasm32")]
        {
            PerfInstant { t: get_time() }
        }
    }
    fn elapsed_ms(&self) -> f32 {
        #[cfg(not(target_arch = "wasm32"))]
        {
            self.t.elapsed().as_secs_f32() * 1000.0
        }
        #[cfg(target_arch = "wasm32")]
        {
            ((get_time() - self.t) * 1000.0) as f32
        }
    }
}

const ZOOM_MIN: f32 = 0.60;
const ZOOM_MAX: f32 = 7.0;
/// Multiplicative zoom change per mouse-wheel notch (the wheel drives the same per-layer zoom
/// value as the right-side slider).
const WHEEL_ZOOM_STEP: f32 = 1.15;
/// Keyboard camera-pan speed in **screen px/s** (divided by the camera scale, so panning feels
/// the same at every zoom level).
const PAN_SPEED_PX: f32 = 700.0;
/// After a SEND, the selection stays alive this long (wall-clock) for rapid repeat orders, then
/// auto-clears — so clicking a new sub right after a send selects it instead of shipping more.
/// (Owner-tuned 1 s → 2 s.)
const DESELECT_AFTER_SEND_S: f64 = 2.0;

/// Pointer movement (px) from the left-button press above which the drag becomes a **selection box**
/// rather than a click (select / order).
const BOX_DRAG_THRESHOLD: f32 = 6.0;

// =============================================================================================
// Palette (the shared dark minimalist look from layer1/2-game)
// =============================================================================================

const BG: Color = Color::new(0.043, 0.055, 0.078, 1.0);
const GRID: Color = Color::new(0.10, 0.12, 0.16, 1.0);

const PLAYER: Color = Color::new(0.30, 0.72, 1.00, 1.0); // cyan/blue (Player seat)
const PLAYER_DIM: Color = Color::new(0.18, 0.42, 0.62, 1.0);
const ENEMY: Color = Color::new(1.00, 0.42, 0.30, 1.0); // red/orange (Enemy seat — default kind)
const ENEMY_DIM: Color = Color::new(0.62, 0.26, 0.20, 1.0);
// Enemy colour is keyed on the controlling AI **kind** (its `Roster`), not the level — so a future
// level with two enemies of different kinds shows both colours at once. `roster_color` is the map.
const ENEMY_GREY: Color = Color::new(0.60, 0.62, 0.68, 1.0); // Passive (dormant machine) — cool steel
const ENEMY_YELLOW: Color = Color::new(0.95, 0.78, 0.22, 1.0); // SimpleColonize — amber
// The **second** AI seat (`Enemy2`) gets its own shade so two same-kind enemies read apart.
const ENEMY2_YELLOW: Color = Color::new(0.83, 0.87, 0.32, 1.0); // 2nd SimpleColonize — paler, greener yellow
const ENEMY2_DIM: Color = Color::new(0.48, 0.51, 0.19, 1.0);
const NEUTRAL: Color = Color::new(0.55, 0.58, 0.62, 1.0);
const NEUTRAL_DIM: Color = Color::new(0.30, 0.32, 0.35, 1.0);

const EDGE_COL: Color = Color::new(0.22, 0.26, 0.32, 1.0);
const EDGE_HILITE: Color = Color::new(0.55, 0.85, 1.00, 0.9);

const AUTO_COL: Color = Color::new(0.55, 1.00, 0.70, 1.0); // automation indicator (green)

const HUD_BG: Color = Color::new(0.0, 0.0, 0.0, 0.55);
const HUD_TEXT: Color = Color::new(0.90, 0.92, 0.96, 1.0);
const HUD_MUTED: Color = Color::new(0.62, 0.66, 0.72, 1.0);
const ACCENT: Color = Color::new(1.0, 0.92, 0.6, 1.0); // titles / highlight (warm)

fn faction_color(f: Faction) -> Color {
    match f {
        Faction::Player => PLAYER,
        Faction::Ai(0) => ENEMY,
        Faction::Ai(_) => ENEMY2_YELLOW, // 2nd+ AI seat (generic fallback shade; `Game::enemy_color` is roster-aware)
        Faction::Neutral => NEUTRAL,
    }
}
fn faction_dim(f: Faction) -> Color {
    match f {
        Faction::Player => PLAYER_DIM,
        Faction::Ai(0) => ENEMY_DIM,
        Faction::Ai(_) => ENEMY2_DIM, // 2nd+ AI seat (generic fallback shade)
        Faction::Neutral => NEUTRAL_DIM,
    }
}

/// The colour of an enemy by its controlling **kind** (`Roster`). This — not the level — is the
/// source of truth, so a level fielding several enemies of different kinds renders each in its own
/// colour. Unmapped kinds fall back to the default red/orange [`ENEMY`].
fn roster_color(r: ai::Roster) -> Color {
    match r {
        ai::Roster::Passive => ENEMY_GREY,
        ai::Roster::SimpleColonize => ENEMY_YELLOW,
        _ => ENEMY,
    }
}

/// Colour for the **second** AI seat (`Enemy2`), keyed on its kind — a distinct shade of the same
/// family as [`roster_color`] so two same-kind opponents are told apart (two Simples ⇒ two yellows).
fn roster_color_alt(r: ai::Roster) -> Color {
    match r {
        ai::Roster::Passive => Color::new(0.50, 0.53, 0.60, 1.0), // cooler steel (vs Enemy's grey)
        ai::Roster::SimpleColonize => ENEMY2_YELLOW,
        _ => Color::new(0.92, 0.52, 0.30, 1.0), // distinct orange (vs Enemy's red)
    }
}

/// Dim a colour to ~0.58× luminance (matching the hand-tuned `*_DIM` constants) for sub fills.
#[inline]
fn dim_of(c: Color) -> Color {
    Color::new(c.r * 0.58, c.g * 0.58, c.b * 0.58, c.a)
}

// The lens colour of a struct node is resolved per-game by [`Game::struct_col`] (so the enemy seat
// takes its kind-colour); there is no longer a free `struct_color` helper.

// =============================================================================================
// Run mode & CLI config
// =============================================================================================

/// Which menu a `--screen` shot targets.
#[derive(Clone, Copy, PartialEq)]
enum ScreenTarget {
    Menu,
    Select,
    Settings,
    Memory,
}

/// Which lens a `--view` shot / a loaded level should open in (overriding the level default).
#[derive(Clone, Copy, PartialEq)]
enum ViewTarget {
    Lens,
    Interior,
}

#[derive(Clone)]
enum Mode {
    /// Normal interactive play (human is the Player seat).
    Human,
    /// Headless verification: render one frame to `path` and exit. The other fields steer *what*
    /// is captured (a menu, or a level advanced to a tick in a given view, optionally AI-driven).
    Shot {
        path: String,
        screen: Option<ScreenTarget>,
        level: Option<usize>,
        view: Option<ViewTarget>,
        at_tick: u64,
        /// Per-layer zoom applied to the captured view (the shot's "scroll wheel").
        zoom: Option<f32>,
        /// Camera pan in world units applied to the captured view (the shot's "drag").
        pan: Option<(f32, f32)>,
        /// Capture the synthetic reserve-combat ARENA (two staged clumps fight to the end)
        /// instead of a campaign level — the aesthetics-validation scenario.
        arena: bool,
        auto: bool,
        /// Also write the focused struct's raw ship table (CSV) at the captured tick — the
        /// numeric half of the aesthetics screenshot loop (pixels show the look, the dump
        /// says which ships made it).
        dump: Option<String>,
    },
}

struct Config {
    mode: Mode,
    seed: u64,
    /// Debug: unlock every level in the select screen (for testing).
    unlock_all: bool,
    /// Optional `--level <N>` to jump straight into a level in interactive play.
    start_level: Option<usize>,
    /// `--auto` in interactive play drives BOTH seats by AI (a hands-off demo of a level).
    auto: bool,
    /// `--selftest`: run the headless game-loop self-test, print results, and exit.
    selftest: bool,
    /// `--reset`: wipe all saved progress + memory (unlocks, received briefings, notes) on startup,
    /// then run fresh.
    reset: bool,
    /// `--text`: enable the narrative text layer (pre-mission briefings, post-battle logs,
    /// notes, the Memory page). OFF by default (owner, 2026-07-08) — without it those
    /// features are hidden entirely.
    text: bool,
    /// `--win WxH`: resize the window before capturing/playing — the presentation-QA dial
    /// for verifying layouts in tiny or oddly-shaped windows (web branch, 2026-07-10).
    win: Option<(u32, u32)>,
    /// `--replay <file.mir>`: open straight into PLAYBACK of a recorded match (combines
    /// with `--shot` for headless playback verification).
    replay: Option<String>,
}

fn parse_seed(s: &str) -> Option<u64> {
    let s = s.trim();
    if let Some(hex) = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
        u64::from_str_radix(hex, 16).ok()
    } else {
        s.parse::<u64>().ok()
    }
}

/// Parse CLI flags (and a couple of env fallbacks) into a [`Config`].
fn parse_config() -> Config {
    let mut seed: u64 = 0xCE11_2042;
    let mut unlock_all = false;
    let mut start_level: Option<usize> = None;
    let mut auto = false;
    let mut selftest = false;
    let mut reset = false;
    let mut text = false;
    let mut win: Option<(u32, u32)> = None;
    let mut replay: Option<String> = None;

    // Shot sub-config (only meaningful with --shot).
    let mut shot_path: Option<String> = None;
    let mut screen: Option<ScreenTarget> = None;
    let mut level: Option<usize> = None;
    let mut view: Option<ViewTarget> = None;
    let mut at_tick: u64 = 0;
    let mut zoom: Option<f32> = None;
    let mut pan: Option<(f32, f32)> = None;
    let mut arena = false;
    let mut dump: Option<String> = None;

    if std::env::var("MI_UNLOCK_ALL").map(|v| v == "1" || v == "true").unwrap_or(false) {
        unlock_all = true;
    }
    if let Ok(s) = std::env::var("MI_SEED") {
        if let Some(v) = parse_seed(&s) {
            seed = v;
        }
    }

    let args: Vec<String> = std::env::args().collect();
    let mut i = 1;
    while i < args.len() {
        let next = |i: usize| -> Option<&String> { args.get(i + 1).filter(|s| !s.starts_with("--")) };
        match args[i].as_str() {
            "--shot" => {
                shot_path = Some(match next(i) {
                    Some(p) => {
                        i += 1;
                        p.clone()
                    }
                    None => "target/game_shot.png".to_string(),
                });
            }
            "--screen" => {
                if let Some(v) = next(i) {
                    screen = match v.as_str() {
                        "menu" => Some(ScreenTarget::Menu),
                        "select" => Some(ScreenTarget::Select),
                        "settings" => Some(ScreenTarget::Settings),
                        "memory" => Some(ScreenTarget::Memory),
                        _ => None,
                    };
                    i += 1;
                }
            }
            "--level" => {
                if let Some(v) = next(i) {
                    if let Ok(n) = v.trim().parse::<usize>() {
                        level = Some(n);
                        start_level = Some(n);
                    }
                    i += 1;
                }
            }
            "--view" => {
                if let Some(v) = next(i) {
                    view = match v.as_str() {
                        "lens" => Some(ViewTarget::Lens),
                        "interior" => Some(ViewTarget::Interior),
                        _ => None,
                    };
                    i += 1;
                }
            }
            "--at-tick" => {
                if let Some(v) = next(i) {
                    if let Ok(t) = v.trim().parse::<u64>() {
                        at_tick = t;
                    }
                    i += 1;
                }
            }
            "--zoom" => {
                if let Some(v) = next(i) {
                    if let Ok(z) = v.trim().parse::<f32>() {
                        // The dev capture tool is exempt from the play-facing out-zoom floor
                        // (the aesthetics screenshot loop wants arbitrarily wide frames).
                        zoom = Some(z.clamp(0.01, ZOOM_MAX));
                    }
                    i += 1;
                }
            }
            "--pan" => {
                if let Some(v) = next(i) {
                    let mut it = v.split(',').map(|s| s.trim().parse::<f32>());
                    if let (Some(Ok(x)), Some(Ok(y))) = (it.next(), it.next()) {
                        pan = Some((x, y));
                    }
                    i += 1;
                }
            }
            "--auto" => {
                auto = true;
            }
            "--arena" => {
                arena = true;
            }
            "--dump" => {
                if let Some(p) = next(i) {
                    dump = Some(p.clone());
                    i += 1;
                }
            }
            "--unlock-all" => {
                unlock_all = true;
            }
            "--selftest" => {
                selftest = true;
            }
            "--reset" => {
                reset = true;
            }
            "--text" => {
                text = true;
            }
            "--replay" => {
                if let Some(pth) = next(i) {
                    replay = Some(pth.clone());
                    i += 1;
                }
            }
            "--win" => {
                if let Some(v) = next(i) {
                    let mut it = v.split(['x', 'X']).map(|s| s.trim().parse::<u32>());
                    if let (Some(Ok(w)), Some(Ok(h))) = (it.next(), it.next()) {
                        win = Some((w.max(1), h.max(1)));
                    }
                    i += 1;
                }
            }
            "--seed" => {
                if let Some(v) = next(i) {
                    if let Some(s) = parse_seed(v) {
                        seed = s;
                    }
                    i += 1;
                }
            }
            _ => {}
        }
        i += 1;
    }

    let mode = match shot_path {
        Some(path) => Mode::Shot { path, screen, level, view, at_tick, zoom, pan, arena, auto, dump },
        None => Mode::Human,
    };
    Config { mode, seed, unlock_all, start_level, auto, selftest, reset, text, win, replay }
}

// =============================================================================================
// The reserve-combat ARENA (dev scenario for the aesthetics screenshot loop)
// =============================================================================================

/// Build the arena world: two dense 300-ship clumps (deliberately LINE-like, 0.2 rad wide)
/// staged on one struct's reserve ring, two zero-production anchor subs so each seat owns
/// ground, no AI orders, no meaningful production — the ships seek, clash, and the survivors
/// redisperse. Pure orbit/combat aesthetics, reproducible for `--arena --shot`.
fn build_arena(seed: u64) -> (World, WorldParams) {
    use layer1::{Interior, Ship, SubStructure, Vec2};
    let mut w = World::new();
    let mut st = Interior::new(seed);
    let anchor_p = st.add_sub(SubStructure::teleporter(Vec2::new(-30.0, 0.0), Faction::Player));
    let anchor_e = st.add_sub(SubStructure::teleporter(Vec2::new(30.0, 0.0), Faction::Ai(0)));
    // Permanent anchor garrisons: neither seat can be eliminated (production-0 subs alone do
    // not keep a seat alive), so the arena never seals and the post-combat dispersal can be
    // observed for as long as the shot asks.
    for _ in 0..20 {
        st.spawn_ship(Faction::Player, anchor_p);
        st.spawn_ship(Faction::Ai(0), anchor_e);
    }
    let storage = st.add_storage_sub();
    let centre = st.subs[storage].pos;
    let radius = st.subs[storage].radius;
    let rf = st.subs[storage].ring_frac;
    let stage = |st: &mut Interior, f: Faction, base: f32, n: usize| {
        for i in 0..n {
            let t = i as f32 / n as f32;
            let a = (base + (t - 0.5) * 0.2).rem_euclid(std::f32::consts::TAU);
            let off = t * 0.2 - 0.1; // deterministic spread across the radial band
            let rr = (rf + off) * radius;
            let pos = Vec2::new(centre.x + rr * a.cos(), centre.y + rr * a.sin());
            st.ships.push(Ship {
                faction: f,
                pos,
                target: None,
                home: storage,
                aim: pos,
                alive: true,
                angle: a,
                undock_remaining: 0,
                drift_remaining: 0,
                ring_offset: off,
                ring_drift: 0.0,
            });
        }
    };
    stage(&mut st, Faction::Player, 1.0, 300);
    stage(&mut st, Faction::Ai(0), 4.0, 300);
    w.add_struct(world::Structure::new(st, layer1::Vec2::new(0.0, 0.0), "Arena"));
    (w, WorldParams::default())
}

/// The arena wrapped as a [`Level`] (Passive enemy seat — nothing issues orders; the sim's
/// own orbit/combat dynamics are the entire show).
fn arena_level() -> Level {
    Level {
        id: 99,
        title: "Arena".into(),
        blurb: "Dev arena: reserve-combat aesthetics.".into(),
        objective: "Observe.".into(),
        hints: vec!["Dev scenario — not part of the campaign.".into()],
        enemies: vec![ai::Roster::Passive],
        start_view: StartView::Layer1(0),
        automation_available: false,
        horizon: 1_000_000,
        zoom_min: None,
        zoom_start: None,
        briefing: None,
        post_log: None,
        source: levels::LevelSource::Builtin(build_arena),
        content_hash: 0,
    }
}

// =============================================================================================
// Progress persistence (mi_progress.json next to the exe)
// =============================================================================================

/// Replay checkpoint cadence in ticks (see `Game::checkpoints`).
const REPLAY_CHECKPOINT_EVERY: u64 = 600;

/// The replay folder (owner layout, 2026-07-10): `replays/` next to the exe. MANUAL saves
/// land here directly (never pruned); AUTO saves land in per-level subfolders
/// (`replays/L07/`), pruned to the Settings cap.
fn replay_dir() -> std::path::PathBuf {
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            return dir.join("replays");
        }
    }
    std::path::PathBuf::from("replays")
}

/// Prune a level's AUTO-replay subfolder to at most `cap` files, oldest first. The
/// cleanup counts and deletes ONLY files the auto-recorder itself named
/// (`L<NN>_<epoch>.mir`, chronological by the parsed epoch): anything a player renamed or
/// dropped into the folder is INVISIBLE to it — renaming a replay is a legitimate way to
/// keep it (owner probe, 2026-07-10: a lexicographic sort over every `.mir` would have
/// deleted an early-sorting renamed keeper first). `cap < 0` = infinite, no cleanup.
fn prune_auto_replays(dir: &std::path::Path, cap: i64) {
    if cap < 0 {
        return;
    }
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    let mut files: Vec<(u64, std::path::PathBuf)> = rd
        .filter_map(|e| {
            let p = e.ok()?.path();
            let epoch = auto_replay_epoch(&p)?;
            Some((epoch, p))
        })
        .collect();
    files.sort();
    let excess = files.len().saturating_sub(cap.max(0) as usize);
    for (_, f) in files.into_iter().take(excess) {
        let _ = std::fs::remove_file(f);
    }
}

/// The `<epoch>` of a STANDARD auto-replay file name (`L<NN>_<epoch>.mir`); `None` for
/// anything else — non-standard names belong to the player, not the cleanup.
fn auto_replay_epoch(p: &std::path::Path) -> Option<u64> {
    if !p.extension().is_some_and(|x| x == "mir") {
        return None;
    }
    let stem = p.file_stem()?.to_str()?;
    let (lvl, ts) = stem.split_once('_')?;
    let digits = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
    if !lvl.strip_prefix('L').is_some_and(digits) || !digits(ts) {
        return None;
    }
    ts.parse().ok()
}

/// One section of the replay browser (the Memory page's default tab): the manual-saves
/// shelf, or one campaign level's auto subfolder.
struct ReplaySection {
    label: String,
    entries: Vec<ReplayEntry>,
}

/// One listed replay: file stem, full path, and the recorded end tick (for the duration
/// column) — `None` when the file has no readable `end` line.
struct ReplayEntry {
    name: String,
    path: std::path::PathBuf,
    end_tick: Option<u64>,
}

/// A row of the flattened browser tree: a section header, or entry `e` of section `sec`.
#[derive(Clone, Copy)]
enum BrowserRow {
    Header(usize),
    Entry(usize, usize),
}

/// Flatten sections into visible rows (entries only under an expanded header).
fn browser_rows(sections: &[ReplaySection], expanded: u32) -> Vec<BrowserRow> {
    let mut rows = Vec::new();
    for (i, sec) in sections.iter().enumerate() {
        rows.push(BrowserRow::Header(i));
        if expanded & (1u32 << i) != 0 {
            for e in 0..sec.entries.len() {
                rows.push(BrowserRow::Entry(i, e));
            }
        }
    }
    rows
}

/// Scan the replay folders into the browser model: the `replays/` root (manual saves,
/// FIRST — the owner's "universal replays" section) then one section per campaign level.
fn scan_replays(levels: &[Level]) -> Vec<ReplaySection> {
    let root = replay_dir();
    let mut sections = vec![ReplaySection { label: "Saved replays".to_string(), entries: scan_replay_dir(&root) }];
    for lvl in levels {
        sections.push(ReplaySection {
            label: format!("{:>2}   {}", lvl.id, lvl.title),
            entries: scan_replay_dir(&root.join(format!("L{:02}", lvl.id))),
        });
    }
    sections
}

/// All `.mir` files of one folder, name-sorted (chronological for auto names), with each
/// file's recorded end tick read off its `end` line.
fn scan_replay_dir(dir: &std::path::Path) -> Vec<ReplayEntry> {
    let Ok(rd) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut out: Vec<ReplayEntry> = rd
        .filter_map(|e| {
            let path = e.ok()?.path();
            if !path.extension().is_some_and(|x| x == "mir") {
                return None;
            }
            let name = path.file_stem()?.to_string_lossy().into_owned();
            let end_tick = std::fs::read_to_string(&path).ok().and_then(|t| {
                t.lines()
                    .rev()
                    .find(|l| l.starts_with("end "))
                    .and_then(|l| l.split_whitespace().nth(1))
                    .and_then(|v| v.parse().ok())
            });
            Some(ReplayEntry { name, path, end_tick })
        })
        .collect();
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// Match duration for the browser: ticks at the fixed 60 ticks-per-second play rate,
/// as `m:ss` (owner: human time, not ticks).
fn fmt_duration_60tps(ticks: u64) -> String {
    let secs = ticks / 60;
    format!("{}:{:02}", secs / 60, secs % 60)
}

/// The Memory page's list band (below title + tabs, above the help line).
const MEM_TOP: f32 = 168.0;
const MEM_PITCH: f32 = 30.0;

fn memory_visible_rows() -> usize {
    (((screen_height() - MEM_TOP - 56.0) / MEM_PITCH).floor() as usize).max(1)
}

fn memory_row_rect(vis_i: usize) -> (f32, f32, f32, f32) {
    let cx = screen_width() * 0.5;
    (cx - SEL_ROW_W * 0.5, MEM_TOP + vis_i as f32 * MEM_PITCH, SEL_ROW_W, MEM_PITCH - 4.0)
}

/// Which flattened row the mouse is over (accounting for the scroll window).
fn memory_row_at_mouse(scroll: usize, nrows: usize) -> Option<usize> {
    let (mx, my) = mouse_position();
    let visible = memory_visible_rows();
    for vis_i in 0..visible.min(nrows.saturating_sub(scroll)) {
        let (x, y, w, h) = memory_row_rect(vis_i);
        if mx >= x && mx <= x + w && my >= y && my <= y + h {
            return Some(scroll + vis_i);
        }
    }
    None
}

/// The tab chips under the MEMORY title. Returns each chip's rect.
fn memory_tab_rect(tab: usize) -> Rect {
    let w = 170.0;
    let cx = screen_width() * 0.5;
    let x0 = cx - w - 8.0;
    Rect::new(if tab == 0 { x0 } else { cx + 8.0 }, 122.0, w, 30.0)
}

fn memory_tab_at_mouse(text_on: bool) -> Option<usize> {
    let (mx, my) = mouse_position();
    let ntabs = if text_on { 2 } else { 1 };
    (0..ntabs).find(|&t| memory_tab_rect(t).contains(vec2(mx, my)))
}

/// The auto/manual replay file name: level + timestamp (owner scheme). The epoch seconds
/// keep names unique and lexicographically chronological.
fn replay_file_name(level_id: u32) -> String {
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format!("L{level_id:02}_{ts}.mir")
}

/// The replay file's seat code: `P`, `A0`, `A1`, ... (`N` only in a drawn `end` line).
fn seat_code(f: Faction) -> String {
    match f {
        Faction::Player => "P".to_string(),
        Faction::Ai(i) => format!("A{i}"),
        Faction::Neutral => "N".to_string(),
    }
}

/// Path to the progress file: next to the executable if we can find it, else the CWD.
fn progress_path() -> std::path::PathBuf {
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            return dir.join("mi_progress.json");
        }
    }
    std::path::PathBuf::from("mi_progress.json")
}

/// GAME PROGRESS (owner, 2026-07-08 — one file tracking it all): the level ids **unlocked**,
/// the level ids **completed** (a win is recorded even on the last level — the old high-water
/// pair could not represent that), and the **memories unlocked** (level ids whose briefing has
/// been received). Persisted as explicit id arrays in `mi_progress.json`:
/// `{"unlocked": [1, 2], "completed": [1], "memories": [1]}` — hand-rolled (de)serialization,
/// no serde. The legacy scalar format (`{"unlocked": N, "briefed": M}`) migrates on load.
#[derive(Debug, Clone, Default)]
struct Progress {
    unlocked: std::collections::BTreeSet<u32>,
    completed: std::collections::BTreeSet<u32>,
    memories: std::collections::BTreeSet<u32>,
}

impl Progress {
    /// A fresh profile: level 1 unlocked, nothing completed, no memories.
    fn fresh() -> Progress {
        let mut p = Progress::default();
        p.unlocked.insert(1);
        p
    }

    fn load() -> Progress {
        let Ok(text) = std::fs::read_to_string(progress_path()) else {
            return Progress::fresh();
        };
        // New format: explicit id arrays.
        if let Some(unlocked) = parse_json_uint_list(&text, "unlocked") {
            let mut p = Progress {
                unlocked: unlocked.into_iter().collect(),
                completed: parse_json_uint_list(&text, "completed").unwrap_or_default().into_iter().collect(),
                memories: parse_json_uint_list(&text, "memories").unwrap_or_default().into_iter().collect(),
            };
            p.unlocked.insert(1); // level 1 is always playable
            return p;
        }
        // Legacy high-water pair: unlocked 1..=N, completed 1..N (the pair could not record
        // the last level's win), memories 1..=M.
        let n = parse_json_uint(&text, "unlocked").unwrap_or(1).max(1);
        let m = parse_json_uint(&text, "briefed").unwrap_or(0);
        Progress {
            unlocked: (1..=n).collect(),
            completed: (1..n).collect(),
            memories: (1..=m).collect(),
        }
    }

    /// Persist (best-effort; failure is non-fatal — progress just won't carry over).
    fn save(&self) {
        let list = |set: &std::collections::BTreeSet<u32>| {
            set.iter().map(|i| i.to_string()).collect::<Vec<_>>().join(", ")
        };
        let body = format!(
            "{{\n  \"unlocked\": [{}],\n  \"completed\": [{}],\n  \"memories\": [{}]\n}}\n",
            list(&self.unlocked),
            list(&self.completed),
            list(&self.memories)
        );
        let _ = std::fs::write(progress_path(), body);
    }
}

/// Pull the unsigned integer after `"<key>"` out of the tiny JSON blob. Tolerant of whitespace.
fn parse_json_uint(text: &str, key: &str) -> Option<u32> {
    let needle = format!("\"{key}\"");
    let at = text.find(&needle)?;
    let after = &text[at + needle.len()..];
    let colon = after.find(':')?;
    let rest = &after[colon + 1..];
    let digits: String = rest.chars().skip_while(|c| c.is_whitespace()).take_while(|c| c.is_ascii_digit()).collect();
    digits.parse::<u32>().ok()
}

#[cfg(test)]
mod replay_prune_tests {
    use super::auto_replay_epoch;
    use std::path::Path;

    /// The cleanup's contract: it recognises ONLY its own names — everything else in the
    /// folder is the player's and must be invisible to pruning.
    #[test]
    fn only_standard_auto_names_are_prunable() {
        let e = |s: &str| auto_replay_epoch(Path::new(s));
        assert_eq!(e("L01_1784294003.mir"), Some(1784294003));
        assert_eq!(e("L107_5.mir"), Some(5));
        assert_eq!(e("AAA_favorite.mir"), None, "a renamed keeper must be untouchable");
        assert_eq!(e("my_epic_win.mir"), None);
        assert_eq!(e("L01_1784294003.txt"), None);
        assert_eq!(e("L01.mir"), None);
        assert_eq!(e("L01_17842_9.mir"), None, "extra underscore = not ours");
        assert_eq!(e("Lx_1784294003.mir"), None);
        assert_eq!(e("L01_.mir"), None);
    }
}

#[cfg(test)]
mod progress_tests {
    use super::*;

    #[test]
    fn id_lists_parse_and_legacy_scalars_do_not() {
        let new = "{
  \"unlocked\": [1, 2, 5],
  \"completed\": [],
  \"memories\": [1]
}
";
        assert_eq!(parse_json_uint_list(new, "unlocked"), Some(vec![1, 2, 5]));
        assert_eq!(parse_json_uint_list(new, "completed"), Some(vec![]));
        assert_eq!(parse_json_uint_list(new, "memories"), Some(vec![1]));
        let legacy = "{\"unlocked\": 4, \"briefed\": 2}";
        assert_eq!(parse_json_uint_list(legacy, "unlocked"), None, "scalar is the legacy cue");
        assert_eq!(parse_json_uint(legacy, "unlocked"), Some(4));
        assert_eq!(parse_json_uint(legacy, "briefed"), Some(2));
    }
}

/// Pull the `[…]` id array after `"<key>"` — `None` if the value is not an array (legacy).
fn parse_json_uint_list(text: &str, key: &str) -> Option<Vec<u32>> {
    let needle = format!("\"{key}\"");
    let at = text.find(&needle)?;
    let after = &text[at + needle.len()..];
    let colon = after.find(':')?;
    let rest = after[colon + 1..].trim_start();
    if !rest.starts_with('[') {
        return None;
    }
    let inner = &rest[1..rest.find(']')?];
    Some(
        inner
            .split(',')
            .filter_map(|t| t.trim().parse::<u32>().ok())
            .collect(),
    )
}

/// Load the player's persistent notes (empty if none). They live at `assets/notes/notes.glg`
/// (owner reorg, 2026-07-08 — player-facing text lives with the assets); a legacy
/// `mi_notes.txt` next to the exe migrates over on first load.
fn load_notes() -> String {
    let path = narrative::notes_path();
    if let Ok(s) = std::fs::read_to_string(&path) {
        return s;
    }
    let legacy = std::env::current_exe()
        .ok()
        .and_then(|e| e.parent().map(|d| d.join("mi_notes.txt")))
        .unwrap_or_else(|| std::path::PathBuf::from("mi_notes.txt"));
    if let Ok(s) = std::fs::read_to_string(&legacy) {
        let _ = std::fs::write(&path, &s);
        return s;
    }
    String::new()
}

/// Persist the player's notes (best-effort).
fn save_notes(notes: &str) {
    let _ = std::fs::write(narrative::notes_path(), notes);
}

/// `--reset`: wipe all saved state — progress (unlocks + received briefings), notes, and
/// the battle-stats history.
fn reset_all_progress() {
    let _ = std::fs::remove_file(progress_path());
    let _ = std::fs::remove_file(narrative::notes_path());
    let _ = std::fs::remove_file(narrative::battle_stats_path());
}

// =============================================================================================
// Control bindings (mi_controls.cfg next to the exe; rebindable from the Settings screen)
// =============================================================================================

/// Every rebindable in-game action. Menu navigation, Esc (back/pause), Enter/Space (confirm),
/// and the mouse gestures are FIXED — rebinding those could lock the player out of the UI.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Action {
    PanUp,
    PanDown,
    PanLeft,
    PanRight,
    SpeedDown,
    SpeedUp,
    PauseToggle,
    Frac25,
    Frac50,
    Frac75,
    Frac100,
    PerfOverlay,
}

/// The full action table: `(action, cfg key, settings label, default binding)`. Order = the
/// Settings screen's row order and the `Bindings::keys` index.
const ACTIONS: [(Action, &str, &str, KeyCode); 12] = [
    (Action::PanUp, "pan_up", "Pan camera up", KeyCode::W),
    (Action::PanDown, "pan_down", "Pan camera down", KeyCode::S),
    (Action::PanLeft, "pan_left", "Pan camera left", KeyCode::A),
    (Action::PanRight, "pan_right", "Pan camera right", KeyCode::D),
    (Action::SpeedDown, "speed_down", "Game speed down", KeyCode::Minus),
    (Action::SpeedUp, "speed_up", "Game speed up", KeyCode::Equal),
    (Action::PauseToggle, "pause", "Pause / resume", KeyCode::P),
    (Action::Frac25, "send_25", "Send fraction 25%", KeyCode::Key1),
    (Action::Frac50, "send_50", "Send fraction 50%", KeyCode::Key2),
    (Action::Frac75, "send_75", "Send fraction 75%", KeyCode::Key3),
    (Action::Frac100, "send_100", "Send fraction 100%", KeyCode::Key4),
    (Action::PerfOverlay, "perf_overlay", "Performance overlay", KeyCode::F3),
];

/// The live key map + persisted play prefs. Loaded from [`controls_path`] at startup, saved on
/// every rebind / pref change. Enforced invariant: no two actions share a key (rebinding to a
/// taken key SWAPS the two bindings, so every action always has one).
struct Bindings {
    keys: [KeyCode; ACTIONS.len()],
    /// Persisted play prefs (owner QoL: not reset between matches) — the last speed step
    /// (persisted as the **multiplier** — 1/3/10/25 — so the file survives stop-list edits;
    /// the transient P-pause is never persisted, so no match starts pre-paused)…
    speed_idx: usize,
    /// …and the last send-fraction percentage.
    frac_pct: u8,
    /// AUTO-REPLAY retention (owner, 2026-07-10): each level's auto-replay subfolder keeps
    /// at most this many files, oldest pruned first. `-1` = infinite (no cleanup). Manual
    /// saves (the `replays/` root) are never pruned. Set in Settings; free integer.
    replay_cap: i64,
}

impl Bindings {
    fn defaults() -> Bindings {
        let mut keys = [KeyCode::Unknown; ACTIONS.len()];
        for (i, &(_, _, _, k)) in ACTIONS.iter().enumerate() {
            keys[i] = k;
        }
        Bindings { keys, speed_idx: DEFAULT_SPEED_IDX, frac_pct: 100, replay_cap: 50 }
    }

    fn idx(a: Action) -> usize {
        ACTIONS.iter().position(|&(aa, _, _, _)| aa == a).expect("action in table")
    }

    fn key_of(&self, a: Action) -> KeyCode {
        self.keys[Bindings::idx(a)]
    }

    /// Bind row `i` to `k`. If `k` is already bound to another action, the two SWAP keys.
    fn set(&mut self, i: usize, k: KeyCode) {
        if let Some(j) = self.keys.iter().position(|&kk| kk == k) {
            self.keys[j] = self.keys[i];
        }
        self.keys[i] = k;
    }

    fn load() -> Bindings {
        let mut b = Bindings::defaults();
        let Ok(text) = std::fs::read_to_string(controls_path()) else {
            return b;
        };
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let Some((name, key)) = line.split_once('=') else { continue };
            // Play prefs share the file with the bindings.
            match name.trim() {
                "speed" => {
                    // Stored as the MULTIPLIER (1/3/10/25). Unknown values (including the
                    // old index-based files) fall back to the 1x default.
                    if let Ok(v) = key.trim().parse::<usize>() {
                        b.speed_idx = SPEED_STEPS
                            .iter()
                            .position(|&s| s as usize == v)
                            .unwrap_or(DEFAULT_SPEED_IDX);
                    }
                    continue;
                }
                "send_fraction" => {
                    if let Ok(v) = key.trim().parse::<u8>() {
                        b.frac_pct = v.clamp(1, 100);
                    }
                    continue;
                }
                "replay_cap" => {
                    if let Ok(v) = key.trim().parse::<i64>() {
                        b.replay_cap = v.max(-1);
                    }
                    continue;
                }
                _ => {}
            }
            let Some(i) = ACTIONS.iter().position(|&(_, n, _, _)| n == name.trim()) else { continue };
            if let Some(k) = key_from_name(key.trim()) {
                // Direct assign (not `set`): the file is the authority while loading; a
                // duplicated key in a hand-edited file resolves in favour of the later line.
                if let Some(j) = b.keys.iter().position(|&kk| kk == k) {
                    if j != i {
                        b.keys[j] = ACTIONS[j].3; // bounce the earlier action back to its default
                    }
                }
                b.keys[i] = k;
            }
        }
        b
    }

    fn save(&self) {
        let mut out = String::from("# Machine Intelligence - control bindings (action = key).\n# Rebind in-game via Settings, or edit by hand; unknown lines are ignored.\n");
        for (i, &(_, name, _, _)) in ACTIONS.iter().enumerate() {
            out.push_str(&format!("{} = {}\n", name, key_name(self.keys[i])));
        }
        out.push_str("# Play prefs (persist between matches).\n");
        out.push_str(&format!("speed = {}\n", SPEED_STEPS[self.speed_idx.min(SPEED_STEPS.len() - 1)] as usize));
        out.push_str(&format!("replay_cap = {}\n", self.replay_cap));
        out.push_str(&format!("send_fraction = {}\n", self.frac_pct));
        let _ = std::fs::write(controls_path(), out);
    }
}

/// Path to the controls file: next to the executable if we can find it, else the CWD.
fn controls_path() -> std::path::PathBuf {
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            return dir.join("mi_controls.cfg");
        }
    }
    std::path::PathBuf::from("mi_controls.cfg")
}

thread_local! {
    /// The live bindings (macroquad's main loop is single-threaded; input handlers are spread
    /// across free functions, so a thread-local beats threading `&Bindings` through all of them).
    static BINDS: std::cell::RefCell<Bindings> = std::cell::RefCell::new(Bindings::load());
}

/// Was the key bound to `a` pressed this frame?
fn act_pressed(a: Action) -> bool {
    BINDS.with(|b| is_key_pressed(b.borrow().key_of(a)))
}

/// Is the key bound to `a` held down?
fn act_down(a: Action) -> bool {
    BINDS.with(|b| is_key_down(b.borrow().key_of(a)))
}

/// The `KeyCode` ↔ name tables (the config-file / Settings-screen vocabulary). Names are the
/// stable identifiers written to `mi_controls.cfg` — a key outside this table cannot be bound.
const KEY_NAMES: &[(KeyCode, &str)] = &[
    (KeyCode::A, "A"), (KeyCode::B, "B"), (KeyCode::C, "C"), (KeyCode::D, "D"),
    (KeyCode::E, "E"), (KeyCode::F, "F"), (KeyCode::G, "G"), (KeyCode::H, "H"),
    (KeyCode::I, "I"), (KeyCode::J, "J"), (KeyCode::K, "K"), (KeyCode::L, "L"),
    (KeyCode::M, "M"), (KeyCode::N, "N"), (KeyCode::O, "O"), (KeyCode::P, "P"),
    (KeyCode::Q, "Q"), (KeyCode::R, "R"), (KeyCode::S, "S"), (KeyCode::T, "T"),
    (KeyCode::U, "U"), (KeyCode::V, "V"), (KeyCode::W, "W"), (KeyCode::X, "X"),
    (KeyCode::Y, "Y"), (KeyCode::Z, "Z"),
    (KeyCode::Key0, "0"), (KeyCode::Key1, "1"), (KeyCode::Key2, "2"), (KeyCode::Key3, "3"),
    (KeyCode::Key4, "4"), (KeyCode::Key5, "5"), (KeyCode::Key6, "6"), (KeyCode::Key7, "7"),
    (KeyCode::Key8, "8"), (KeyCode::Key9, "9"),
    (KeyCode::F1, "F1"), (KeyCode::F2, "F2"), (KeyCode::F3, "F3"), (KeyCode::F4, "F4"),
    (KeyCode::F5, "F5"), (KeyCode::F6, "F6"), (KeyCode::F7, "F7"), (KeyCode::F8, "F8"),
    (KeyCode::F9, "F9"), (KeyCode::F10, "F10"), (KeyCode::F11, "F11"), (KeyCode::F12, "F12"),
    (KeyCode::Up, "Up"), (KeyCode::Down, "Down"), (KeyCode::Left, "Left"), (KeyCode::Right, "Right"),
    // (Space is deliberately absent: it is the FIXED hard-pause key, reserved like Esc/Enter.)
    (KeyCode::Tab, "Tab"),
    (KeyCode::Minus, "Minus"), (KeyCode::Equal, "Equal"),
    (KeyCode::LeftBracket, "LBracket"), (KeyCode::RightBracket, "RBracket"),
    (KeyCode::Semicolon, "Semicolon"), (KeyCode::Apostrophe, "Apostrophe"),
    (KeyCode::Comma, "Comma"), (KeyCode::Period, "Period"), (KeyCode::Slash, "Slash"),
    (KeyCode::Backslash, "Backslash"), (KeyCode::GraveAccent, "Grave"),
    (KeyCode::Insert, "Insert"), (KeyCode::Delete, "Delete"), (KeyCode::Home, "Home"),
    (KeyCode::End, "End"), (KeyCode::PageUp, "PageUp"), (KeyCode::PageDown, "PageDown"),
    (KeyCode::Kp0, "Kp0"), (KeyCode::Kp1, "Kp1"), (KeyCode::Kp2, "Kp2"), (KeyCode::Kp3, "Kp3"),
    (KeyCode::Kp4, "Kp4"), (KeyCode::Kp5, "Kp5"), (KeyCode::Kp6, "Kp6"), (KeyCode::Kp7, "Kp7"),
    (KeyCode::Kp8, "Kp8"), (KeyCode::Kp9, "Kp9"),
    (KeyCode::KpAdd, "KpAdd"), (KeyCode::KpSubtract, "KpSubtract"),
    (KeyCode::KpMultiply, "KpMultiply"), (KeyCode::KpDivide, "KpDivide"),
    (KeyCode::KpDecimal, "KpDecimal"),
    (KeyCode::LeftShift, "LShift"), (KeyCode::RightShift, "RShift"),
    (KeyCode::LeftControl, "LCtrl"), (KeyCode::RightControl, "RCtrl"),
    (KeyCode::LeftAlt, "LAlt"), (KeyCode::RightAlt, "RAlt"),
];

fn key_name(k: KeyCode) -> &'static str {
    KEY_NAMES.iter().find(|&&(kk, _)| kk == k).map_or("?", |&(_, n)| n)
}

fn key_from_name(name: &str) -> Option<KeyCode> {
    KEY_NAMES.iter().find(|&&(_, n)| n.eq_ignore_ascii_case(name)).map(|&(k, _)| k)
}

// =============================================================================================
// Continuous camera (lens-world coords -> screen pixels). One camera spans both zoom states by
// lerping its centre + scale between the lens framing and the focused-struct framing.
// =============================================================================================

/// Lens-world coordinates: every struct's [`world::Structure::pos`] lives here, and so does the
/// *interior* of a struct (we offset a struct's local sub coords by its `pos` so one continuous
/// space holds both — the struct's own structure is centred on its `pos`).
#[derive(Clone, Copy)]
struct Camera {
    /// World point mapped to the screen centre.
    cx: f32,
    cy: f32,
    /// Pixels per world unit.
    scale: f32,
    /// Top HUD inset (screen px) the fit avoids.
    top: f32,
    /// Bottom HUD inset (screen px) the fit avoids.
    bottom: f32,
}

impl Camera {
    #[inline]
    fn to_screen(&self, wx: f32, wy: f32) -> (f32, f32) {
        let scx = screen_width() * 0.5;
        let scy = self.top + (screen_height() - self.top - self.bottom) * 0.5;
        // World y grows up; screen y grows down.
        (scx + (wx - self.cx) * self.scale, scy - (wy - self.cy) * self.scale)
    }
    /// Inverse of [`Camera::to_screen`]: the world point under screen pixel `(sx, sy)` — the
    /// anchor for cursor-centred zoom and grab-panning.
    #[inline]
    fn to_world(&self, sx: f32, sy: f32) -> (f32, f32) {
        let scx = screen_width() * 0.5;
        let scy = self.top + (screen_height() - self.top - self.bottom) * 0.5;
        (self.cx + (sx - scx) / self.scale, self.cy - (sy - scy) / self.scale)
    }
    #[inline]
    fn len(&self, m: f32) -> f32 {
        m * self.scale
    }
}

/// Lerp between two cameras by `t` in [0,1] (centre + scale; scale lerped in log space so the
/// zoom feels even).
fn lerp_camera(a: Camera, b: Camera, t: f32) -> Camera {
    let t = t.clamp(0.0, 1.0);
    let scale = (a.scale.ln() + (b.scale.ln() - a.scale.ln()) * t).exp();
    Camera {
        cx: a.cx + (b.cx - a.cx) * t,
        cy: a.cy + (b.cy - a.cy) * t,
        scale,
        top: a.top,
        bottom: a.bottom,
    }
}

/// The lens camera: fit every struct position (with a margin) into the drawable area.
fn lens_camera(world: &World, top: f32, bottom: f32) -> Camera {
    let (mut minx, mut miny, mut maxx, mut maxy) =
        (f32::INFINITY, f32::INFINITY, f32::NEG_INFINITY, f32::NEG_INFINITY);
    for p in &world.structs {
        // Pad each struct by its own visual node radius (in world units) so big nodes fit.
        let pad = struct_world_radius(p);
        minx = minx.min(p.pos.x - pad);
        miny = miny.min(p.pos.y - pad);
        maxx = maxx.max(p.pos.x + pad);
        maxy = maxy.max(p.pos.y + pad);
    }
    if !minx.is_finite() {
        return Camera { cx: 0.0, cy: 0.0, scale: 1.0, top, bottom };
    }
    let sw = screen_width();
    let sh = screen_height();
    let margin = 110.0_f32;
    let avail_w = (sw - 2.0 * margin).max(1.0);
    let avail_h = (sh - top - bottom - 2.0 * margin).max(1.0);
    let span_x = (maxx - minx).max(1e-3);
    let span_y = (maxy - miny).max(1e-3);
    let scale = (avail_w / span_x).min(avail_h / span_y);
    Camera { cx: (minx + maxx) * 0.5, cy: (miny + maxy) * 0.5, scale, top, bottom }
}

/// The interior camera for struct `p`: fit that struct's **tactical cluster** (its real subs, in
/// world coords = local + structure.pos) into the drawable area, scaled to fill it. The reserve /
/// patrol-zone node is EXCLUDED from the fit — at the corrected game scale its ring dwarfs the
/// cluster, and fitting it would open every struct as an unreadable blob in the middle of a huge
/// circle. The ring sits off-screen at the default fit; zooming out (down to [`ZOOM_MIN`]) brings
/// it into view.
fn interior_camera(world: &World, p: StructId, top: f32, bottom: f32) -> Camera {
    let structure = &world.structs[p];
    let (mut minx, mut miny, mut maxx, mut maxy) =
        (f32::INFINITY, f32::INFINITY, f32::NEG_INFINITY, f32::NEG_INFINITY);
    for (i, s) in structure.interior.subs.iter().enumerate() {
        if structure.interior.is_storage(i) {
            continue; // the reserve ring is an outer orbit, not part of the tactical frame
        }
        // An ORBITING sub contributes its whole orbit circle, not its instantaneous position:
        // fitting the momentary bounding box made the frame breathe as the ring turned (the
        // "pulsating camera" on the turning field) — the envelope is rotation-invariant.
        let (wx, wy, pad) = match s.orbit {
            Some(o) => (
                structure.pos.x + o.center.x,
                structure.pos.y + o.center.y,
                o.radius + s.radius,
            ),
            None => (structure.pos.x + s.pos.x, structure.pos.y + s.pos.y, s.radius),
        };
        minx = minx.min(wx - pad);
        miny = miny.min(wy - pad);
        maxx = maxx.max(wx + pad);
        maxy = maxy.max(wy + pad);
    }
    if !minx.is_finite() {
        // Degenerate: centre on the struct with a generous zoom.
        return Camera { cx: structure.pos.x, cy: structure.pos.y, scale: 12.0, top, bottom };
    }
    let sw = screen_width();
    let sh = screen_height();
    let margin = 90.0_f32;
    let avail_w = (sw - 2.0 * margin).max(1.0) * INTERIOR_FILL;
    let avail_h = (sh - top - bottom - 2.0 * margin).max(1.0) * INTERIOR_FILL;
    let span_x = (maxx - minx).max(1e-3);
    let span_y = (maxy - miny).max(1e-3);
    let scale = (avail_w / span_x).min(avail_h / span_y);
    Camera { cx: (minx + maxx) * 0.5, cy: (miny + maxy) * 0.5, scale, top, bottom }
}

/// A struct's visual node radius **in world units** (so the lens fit accounts for it). Scales
/// gently with total ships present so big stacks read as bigger nodes; bounded.
fn struct_world_radius(p: &world::Structure) -> f32 {
    // The MECHANICAL Layer-2 size (owner design, 2026-07-08): radius ∝ sqrt(cumulative
    // production) — size reads as value, and the overwatch zone derives from the same
    // number, so what you see is what shoots.
    p.l2_radius()
}

// =============================================================================================
// In-level game state
// =============================================================================================

/// The current camera zoom intent.
#[derive(Clone, Copy, PartialEq)]
enum View {
    /// Zoomed out to the struct graph.
    Lens,
    /// Zoomed into one struct's interior.
    Interior(StructId),
}

/// A lightweight fleet identity to carry `progress` across a tick for interpolation (cell-core
/// fleets have no stable id; [`world::InterFleet`] is the same — we re-derive identity).
#[derive(Clone, Copy, PartialEq)]
struct FleetKey {
    faction: Faction,
    from: StructId,
    to: StructId,
    progress: f32,
    undock_remaining: u32,
}

/// One in-level match: the world + everything the renderer/host needs around it.
struct Game {
    level: Level,
    world: World,
    wp: WorldParams,
    sim: SimParams,

    /// AI controllers for the enemy seat(s): always `Enemy`, plus `Enemy2` on multi-enemy levels.
    enemies: Vec<SeatController>,
    /// Set in `--auto` / demo: also drive the Player seat by AI (full strategic+tactical).
    player_ai: Option<AiController>,
    /// Tunables for the player's basic-automation greedy adapter (matches the AI's defaults).
    greedy: GreedyParams,
    /// Per-struct player-automation flags (only meaningful when `level.automation_available`).
    automated: Vec<bool>,

    // Camera / zoom.
    view: View,
    /// 0 = full lens, 1 = full interior. Eased toward the target each frame.
    cam_t: f32,
    /// The struct the interior camera is (or was last) focused on — kept while easing out.
    focus: StructId,
    /// Per-layer zoom magnification (the right-side slider). Indexed by [`Game::zoom_layer`]:
    /// `[0]` = Layer-2 lens, `[1]` = Layer-1 interior, `[2]` reserved for a future third layer.
    /// Each layer remembers its own value independently; `1.0` is the default fit.
    zoom: [f32; 3],
    /// True while dragging the right-side zoom slider.
    dragging_zoom: bool,
    /// Per-layer camera **pan** (world units, added to the fitted centre): `[0]` = lens,
    /// `[1]` = interior. Driven by WASD, right-drag, and the cursor-anchor correction of the
    /// wheel zoom; clamped by [`clamp_pan`]; the interior pan resets when the focus structure
    /// changes (a pan framed for one struct means nothing on another).
    pan: [(f32, f32); 2],
    /// Right-drag grab-pan state: (mouse anchor px, that layer's pan at the anchor).
    rdrag: Option<((f32, f32), (f32, f32))>,
    /// The current right-drag moved past the click threshold ⇒ it is a PAN, and the release
    /// must not fire the plain right-click action (clear selection).
    rdrag_moved: bool,
    /// When set, the current selection auto-clears at this `get_time()` deadline. Armed by every
    /// SEND (single or multi): the selection survives [`DESELECT_AFTER_SEND_S`] for rapid repeat
    /// orders, then lets go — so the next click SELECTS instead of accidentally ordering.
    deselect_at: Option<f64>,

    // Pacing.
    /// Current stop on the discrete speed slider — index into [`SPEED_STEPS`] (1x..25x; there
    /// is no 0x stop — pausing is the separate `paused` flag below).
    speed_idx: usize,
    /// The last **non-1x** stop selected (slider or speed keys) — the far side of the fixed
    /// Space key's 1x ⇄ fast toggle. Defaults to 3x until the player picks something faster.
    speed_alt_idx: usize,
    /// THE pause (owner merge, 2026-07-08 — Esc and P raise the same menu): the sim freezes,
    /// **no orders** are accepted, but the camera stays free (drag / pan / zoom). Orthogonal
    /// to `speed_idx`, which it preserves across the pause. Transient — never persisted.
    paused: bool,
    /// Whether the pause MENU panel (veil, "PAUSED", Resume / Restart / Main Menu, the ✕) is
    /// visible. The ✕ clears it while staying paused — no veil, a bare corner tag — so the
    /// player can camera around the frozen level; Esc / P bring the panel back.
    pause_buttons: bool,
    /// True while dragging the topbar speed slider.
    dragging_speed: bool,
    /// Cumulative PLAYER ship losses this match (counted off the death-FX liveness diff —
    /// the post-battle log's `{lost}` metric; `--text` presentation only, never sim state).
    lost_ships: u64,
    /// Cumulative enemy ships destroyed (all rival seats) — the `{killed}` metric.
    killed_ships: u64,
    /// The rendered post-battle log (`--text`, once the match seals) — viewable with `L`.
    post_log_brief: Option<Briefing>,
    /// Whether the post-battle log popup is open on the end screen.
    show_post_log: bool,
    /// Latch: the log is rendered/persisted exactly once per match.
    log_written: bool,
    tick_accum: f64,
    render_alpha: f32,

    // Interpolation snapshots.
    /// Per-structure, per-ship pre-step positions (indexed [structure][ship]) for interior motion.
    prev_ship_pos: Vec<Vec<layer1::Vec2>>,
    /// Fleet identities + progress before the last tick, for lens fleet interpolation.
    prev_fleets: Vec<FleetKey>,

    // Input / selection (shared across both layers).
    /// Player send-fraction as a whole percent in `1..=100` (the topbar slider). Hotkeys 1/2/3/4
    /// snap it to 25/50/75/100. Applied to every player move / fleet order.
    frac_pct: u8,
    /// True while the player is dragging the topbar troop slider (so the drag is not also read as
    /// a board order, and continues tracking even if the pointer leaves the strip).
    dragging_slider: bool,
    /// Selected source: a struct (lens) or a sub of the focused struct (interior). We keep both
    /// kinds; only the one matching the current view is acted on.
    sel_struct: Option<StructId>,
    sel_sub: Option<usize>,
    /// Box multi-selection (from a left-drag): structs in the lens, or subs of the focused struct in
    /// the interior. Issuing an order to a non-empty multi-selection orders **all** of them, then
    /// clears. Only player-commandable positions are box-selectable (and never the struct-storage node).
    sel_structs: Vec<StructId>,
    sel_subs: Vec<usize>,
    /// Left-button press position (screen px) while held, for click-vs-drag detection. `None` when up.
    drag_start: Option<(f32, f32)>,
    /// True once the held drag has passed [`BOX_DRAG_THRESHOLD`] — we are drawing a selection box.
    box_active: bool,

    /// Show the start/objective overlay until dismissed.
    show_intro: bool,
    /// Latched outcome once the match ends (so Victory/Defeat is stable).
    finished: Option<Faction>,

    /// Transient ship-death visual effects (white cross + line from a nearby enemy), spawned by
    /// diffing ship liveness each rendered frame. Purely cosmetic — never read by the sim, so it
    /// has no bearing on determinism.
    kill_fx: Vec<KillFx>,
    /// Transient teleport flashes (white departure→arrival lines), drained from the sim's
    /// per-tick `teleport_events` after every tick. Purely cosmetic.
    teleport_fx: Vec<TeleportFx>,
    /// Transient Layer-2 fleet-loss flashes `(cross position, shooter position, born)` — a
    /// white cross in the lens where transit ships were shot down, plus a thin tracer line
    /// from whatever shot them (struct centre or firing fleet), drained from the world's
    /// per-tick `fleet_death_events` after every tick. The cross is jittered around the
    /// fleet's map position at spawn (cosmetic RNG, never the sim's). Purely cosmetic.
    fleet_kill_fx: Vec<(layer1::Vec2, layer1::Vec2, f64)>,
    /// Reused per-struct ship-liveness snapshot for the death-FX diff (filled before a tick drains;
    /// capacity retained across frames — no per-frame allocation).
    prev_alive: Vec<Vec<bool>>,

    /// The match seed (`build_scaled` input) — stamped into the replay file.
    seed: u64,
    /// The shared ORDER JOURNAL: installed on the world + every interior at construction,
    /// it accumulates every seat's orders (count-canonical, tick-stamped) for the whole
    /// match. Kilobytes even for long games. See `write_replay`.
    journal: layer1::OrderJournal,
    /// `(tick, state_hash)` checkpoints sampled every [`REPLAY_CHECKPOINT_EVERY`] ticks —
    /// written into the replay so a mismatched playback DETECTS divergence loudly.
    checkpoints: Vec<(u64, u64)>,
    /// The replay file has been written for this match (latched with `finished`).
    replay_written: bool,
    /// `Some` = this Game IS a playback (see [`ReplayState`]); `None` = a live match.
    replay: Option<ReplayState>,
    /// The player pressed "Save replay" on this match's end screen (one shelf copy).
    manual_saved: bool,

    /// This match's tickrate **scale** (`TICK_SCALE` for interactive play; `1.0` for the headless
    /// tools). Drives `gui_params(scale)`, `build_scaled(.., scale)`, the horizon, and the cadence.
    scale: f64,
    /// Seat-decision cadence in ticks (`DECISION_BASE × scale`), derived from `scale`.
    decision_interval: u64,
}

/// PLAYBACK state for a replay match: no AI runs and no player orders are accepted — the
/// recorded journal drives every seat (`step_core` feeds records due each tick through
/// `World::apply_record`). Snapshots power the timeline scrubber: seeking restores the
/// nearest snapshot ≤ target and fast-forwards (bit-exact — pinned by the levels test
/// `snapshot_resume_is_bit_exact`); recorded checkpoints are verified in passing, so a
/// build/level mismatch is flagged loudly instead of silently showing a fiction.
struct ReplayState {
    /// Every seat's recorded orders, tick-stamped, in issuance order.
    orders: Vec<layer1::JournalEntry>,
    cursor: usize,
    /// Recorded `(tick, state_hash)` divergence checkpoints.
    checkpoints: Vec<(u64, u64)>,
    check_cursor: usize,
    /// The scrubber's snapshot ring: `(tick, cloned world)` every checkpoint interval
    /// (plus tick 0). A few hundred KB per entry on typical boards.
    snapshots: Vec<(u64, World)>,
    end_tick: u64,
    winner: Faction,
    diverged: bool,
    /// Timeline-click fast-forward target, drained at full speed in `update`.
    seek_target: Option<u64>,
}

/// One ship-death flash, in structure-local coordinates of `struct`. Drawn for [`KILL_FX_TTL`]
/// seconds (fading out) when that struct's interior is on screen.
struct KillFx {
    sid: StructId,
    /// Where the ship died (the destroyed ship's last position).
    at: layer1::Vec2,
    /// A nearby enemy to draw the "killing" line from, if one was in range.
    from: Option<layer1::Vec2>,
    /// Wall-clock spawn time (`get_time()`), for the fade.
    born: f64,
}

/// One teleport flash — a white line from the departure gate to the arrival point, drawn for
/// [`TELEPORT_FX_TTL`] seconds (fading) when that struct's interior is on screen. Fed by the
/// sim's per-tick `Interior::teleport_events` (drained after every tick, so no jump is missed
/// even on multi-tick frames at the high speed stops).
struct TeleportFx {
    sid: StructId,
    from: layer1::Vec2,
    to: layer1::Vec2,
    /// Wall-clock spawn time (`get_time()`), for the fade.
    born: f64,
}

impl Game {
    fn new(level: Level, seed: u64, auto: bool, force_view: Option<ViewTarget>, scale: f64) -> Game {
        let (mut world, wp) = build_scaled(&level, seed, scale);
        // The replay journal: one shared Vec on the world + every interior (all seats'
        // orders, count-canonical). Recording is unconditional — it is the match's input
        // log; whether a file gets WRITTEN is decided at match end (see write_replay).
        let journal: layer1::OrderJournal =
            std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        world.set_journaling(Some(journal.clone()));
        let sim = gui_params(scale);
        // Prime each structure's cached pacing (undock/drift) to the scaled operating point NOW:
        // the cache otherwise holds the unscaled reference until a structure's first step, so the
        // AI's tick-0 orders would undock 24x too fast at the interactive scale.
        for p in &mut world.structs {
            p.interior.set_pacing(&sim);
        }
        // The level's `enemies` list *is* the seat declaration: one controller per `Ai(i)` seat, in
        // order. Any number of opponents (a free-for-all when >1); the engine is agnostic to the count.
        let enemies: Vec<SeatController> = level
            .enemies
            .iter()
            .enumerate()
            .map(|(i, &r)| SeatController::from_roster(Faction::Ai(i as u8), r))
            .collect();
        // In auto/demo, the Player seat is a balanced controller so the level plays itself.
        let player_ai = if auto {
            Some(AiController::from_roster(Faction::Player, ai::Roster::GreedyLocal))
        } else {
            None
        };
        let n = world.structs.len();

        // Opening view: honour an explicit override, else the level's StartView.
        let view = match force_view {
            Some(ViewTarget::Lens) => View::Lens,
            Some(ViewTarget::Interior) => View::Interior(start_struct(&level)),
            None => match level.start_view {
                StartView::Layer1(p) => View::Interior(p.min(n.saturating_sub(1))),
                StartView::Layer2 => View::Lens,
            },
        };
        let focus = match view {
            View::Interior(p) => p,
            View::Lens => start_struct(&level),
        };
        let cam_t = if matches!(view, View::Interior(_)) { 1.0 } else { 0.0 };

        let mut g = Game {
            level,
            world,
            wp,
            sim,
            enemies,
            player_ai,
            greedy: GreedyParams::default(),
            automated: vec![false; n],
            view,
            cam_t,
            focus,
            zoom: [1.0; 3],
            dragging_zoom: false,
            pan: [(0.0, 0.0); 2],
            rdrag: None,
            rdrag_moved: false,
            deselect_at: None,
            // The mission briefing (a separate popup) now precedes the match, so the level itself
            // begins live; the old in-level intro overlay is retired (`show_intro = false`).
            // Speed + send-fraction persist between matches (mi_controls.cfg play prefs).
            speed_idx: BINDS.with(|b| b.borrow().speed_idx),
            // The Space toggle's far side: the persisted speed if it is faster than 1x, else 3x.
            speed_alt_idx: BINDS.with(|b| b.borrow().speed_idx).max(1),
            paused: false,
            pause_buttons: false,
            dragging_speed: false,
            lost_ships: 0,
            killed_ships: 0,
            post_log_brief: None,
            show_post_log: false,
            log_written: false,
            tick_accum: 0.0,
            render_alpha: 0.0,
            prev_ship_pos: Vec::new(),
            prev_fleets: Vec::new(),
            frac_pct: BINDS.with(|b| b.borrow().frac_pct),
            dragging_slider: false,
            sel_struct: None,
            sel_sub: None,
            sel_structs: Vec::new(),
            sel_subs: Vec::new(),
            drag_start: None,
            box_active: false,
            show_intro: false,
            finished: None,
            kill_fx: Vec::new(),
            teleport_fx: Vec::new(),
            fleet_kill_fx: Vec::new(),
            prev_alive: Vec::new(),
            seed,
            journal,
            checkpoints: Vec::new(),
            replay_written: false,
            replay: None,
            manual_saved: false,
            scale,
            decision_interval: (DECISION_BASE as f64 * scale).round().max(1.0) as u64,
        };
        // The authored STARTING zoom (`[level] zoom_start`, owner ask 2026-07-08): applied to
        // the layer the mission opens on, clamped to the effective bounds (per-struct
        // overrides included — view/focus are set above, so the bounds resolve correctly).
        if let Some(z) = g.level.zoom_start {
            let layer = g.zoom_layer();
            g.zoom[layer] = z.clamp(g.zoom_min(), g.zoom_max());
        }
        g
    }

    /// Render colour of a faction, resolving each AI seat `Ai(i)` to its roster's kind-colour via
    /// [`Game::enemy_color`] (a distinct shade per seat, indexing [`Level::enemies`]).
    fn col(&self, f: Faction) -> Color {
        match f {
            Faction::Ai(i) => self.enemy_color(i as usize),
            _ => faction_color(f),
        }
    }
    /// Dimmed-fill colour matching [`Game::col`].
    fn dim(&self, f: Faction) -> Color {
        match f {
            Faction::Ai(i) => dim_of(self.enemy_color(i as usize)),
            _ => faction_dim(f),
        }
    }
    /// Colour for the `i`-th AI seat: its roster's kind-colour — seat 0 the base shade, every
    /// seat 1+ the SAME alt shade (so two same-kind rivals are told apart, but a third+ would
    /// share the second's colour; per-seat palettes are a when-needed extension). Indexes
    /// [`Level::enemies`]; falls back to Simple's colour for an out-of-range seat.
    fn enemy_color(&self, i: usize) -> Color {
        let r = self.level.enemies.get(i).copied().unwrap_or(ai::Roster::SimpleColonize);
        if i == 0 {
            roster_color(r)
        } else {
            roster_color_alt(r)
        }
    }
    /// Lens colour of a struct node, resolving an owned struct's faction through [`Game::col`].
    fn struct_col(&self, owner: StructOwner) -> Color {
        match owner {
            StructOwner::Owned(f) => self.col(f),
            StructOwner::Neutral => NEUTRAL,
            StructOwner::Contested => ACCENT,
        }
    }

    /// The effective sim speed: the slider stop, or `0` while the overlay pause holds.
    fn speed(&self) -> f64 {
        if self.paused {
            0.0
        } else {
            SPEED_STEPS[self.speed_idx]
        }
    }
    /// The `P` overlay pause (the speed slider keeps its stop underneath).
    fn paused(&self) -> bool {
        self.paused
    }

    /// The player's current send-fraction as a multiplier in `(0,1]` (the topbar slider / hotkeys).
    fn send_fraction(&self) -> f32 {
        (self.frac_pct.clamp(1, 100) as f32) / 100.0
    }

    /// The FOCUSED struct's `[struct]` zoom overrides from the data file, if any (owner
    /// mechanism, 2026-07-08 — e.g. Far far away's rear yard-only struct sets a much lower
    /// floor so its huge reserve ring can be seen before the wheel exits to the lens).
    /// Code-built worlds (arena, selftest) have no spec: no overrides.
    fn struct_zoom_override(&self) -> (Option<f32>, Option<f32>) {
        if let levels::LevelSource::Spec(sp) = &self.level.source {
            if let Some(st) = sp.structs.get(self.focus) {
                return (st.zoom_min, st.zoom_max);
            }
        }
        (None, None)
    }

    /// Struct `p`'s interior out-zoom floor (its own `zoom_min` override, else the level's,
    /// else the global [`ZOOM_MIN`]) — also the L1 side of the zoom-continuity handoff.
    fn struct_zoom_floor(&self, p: StructId) -> f32 {
        if let levels::LevelSource::Spec(sp) = &self.level.source {
            if let Some(st) = sp.structs.get(p) {
                if let Some(z) = st.zoom_min {
                    return z;
                }
            }
        }
        self.level.zoom_min.unwrap_or(ZOOM_MIN)
    }

    /// The effective **minimum zoom** (out-zoom floor): on the interior layer the focused
    /// struct's `zoom_min` override, else the level's presentation override, else the global
    /// [`ZOOM_MIN`].
    fn zoom_min(&self) -> f32 {
        if self.zoom_layer() == 1 {
            return self.struct_zoom_floor(self.focus);
        }
        self.level.zoom_min.unwrap_or(ZOOM_MIN)
    }

    /// The effective **maximum zoom** (in-zoom ceiling). Interior layer: the focused
    /// struct's `zoom_max` override, else the global [`ZOOM_MAX`]. LENS layer (owner
    /// continuity contract, 2026-07-08): the zoom at which the focused struct reads exactly
    /// as big as its interior does at that struct's out-zoom floor — so struct visible size
    /// scales CONTINUOUSLY across the L1↔L2 transition (wheel-up past this ceiling enters
    /// the struct, picking up at the interior floor).
    fn zoom_max(&self) -> f32 {
        if self.zoom_layer() == 1 {
            if let (_, Some(z)) = self.struct_zoom_override() {
                return z;
            }
            return ZOOM_MAX;
        }
        lens_match_zoom(self, self.focus).max(self.level.zoom_min.unwrap_or(ZOOM_MIN))
    }

    /// A seat is **finished** (its defeat is sealed) once it has **no ships anywhere** *and* **no
    /// producing structure left**: every sub it still owns is being eroded by the other seat (so
    /// it produces nothing and those subs are flipping away). This calls the match the moment the
    /// result is decided rather than forcing the player to mop up the last drifting ships or wait
    /// out the final resistance grind. With zero ships, no owned sub can be contested/relieved, so
    /// "all remaining owned subs are being eroded" genuinely means the seat can never recover.
    fn seat_finished(&self, f: Faction) -> bool {
        if self.world.total_ships(f) != 0 {
            return false;
        }
        for structure in &self.world.structs {
            let st = &structure.interior;
            for i in 0..st.subs.len() {
                // The reserve / patrol-zone node produces nothing, so owning only it does not
                // keep a seat alive — skip it (a seat with no ships and no producing sub is done).
                if st.is_storage(i) {
                    continue;
                }
                if st.subs[i].owner == f {
                    // A ZERO-production sub (fortress, teleporter) cannot rebuild a shipless
                    // seat either — owning one does not stave off the seal (owner QoL: enemy
                    // reduced to empty forts ⇒ the match is won, no mop-up grind required).
                    if st.subs[i].production == 0 {
                        continue;
                    }
                    // The sub is being lost iff a **foreign** seat is its lone present eroder (any
                    // real faction ≠ `f` — works in a free-for-all). Home-based, matching the grind.
                    let being_lost = matches!(st.capture_present_faction(i), Some((g, _)) if g != f);
                    if !being_lost {
                        return false;
                    }
                }
            }
        }
        true
    }

    /// True once **every** enemy seat the level declares is [`Game::seat_finished`] — the
    /// player's win condition in a free-for-all, for any number of opponents.
    ///
    /// **Passive seats never block the win** (owner QoL, 2026-07-07): a Passive roster is
    /// scenery-hostile — an obstacle, not an objective — so a mission is won once every
    /// *acting* rival is finished, even with passive garrisons still standing. The one
    /// exception is **Mission 1**, whose entire objective *is* the Passive centre.
    fn all_enemies_finished(&self) -> bool {
        // Passive seats never block victory — EXCEPT the M1 micro-tutorial (its passive keep
        // IS the objective) and the id-99 dev arena (its Passive anchor seat exists precisely
        // so the scenario never ends; without the exemption the arena drew at tick 0).
        self.level.enemies.iter().enumerate().all(|(i, &r)| {
            (r == ai::Roster::Passive && !matches!(self.level.id, 1 | 99))
                || self.seat_finished(Faction::Ai(i as u8))
        })
    }

    /// The match is over once either seat is [`Game::seat_finished`], or — **only for AI-driven
    /// matches** (demo / `--auto` / `--selftest` / capture, which all run a Player-seat AI) — at
    /// the level horizon. A human player's match has no clock: it ends only by a sealed result, so
    /// a deliberate game is never cut off mid-comeback (the topbar shows a count-up clock).
    fn match_over(&self) -> bool {
        // A latched outcome IS the end (live matches only latch when over; a playback
        // latches at the recording's end tick). Without this, a PLAYBACK — no player AI,
        // so no horizon clock — would sail straight past the recorded end and keep
        // simulating forever (found the hard way: an 8-minute "headless minute").
        if self.finished.is_some() {
            return true;
        }
        if let Some(rs) = &self.replay {
            return self.world.tick >= rs.end_tick;
        }
        if self.seat_finished(Faction::Player) || self.all_enemies_finished() {
            return true;
        }
        self.player_ai.is_some() && self.world.tick >= scaled_horizon(&self.level, self.scale)
    }

    /// Snapshot fleet identities + per-struct ship positions just before a tick, for render
    /// interpolation. Reuses the buffers' capacity (no per-tick allocation after warm-up).
    fn snapshot(&mut self) {
        self.prev_fleets.clear();
        self.prev_fleets.extend(self.world.fleets.iter().map(fleet_key));
        self.prev_ship_pos.resize_with(self.world.structs.len(), Vec::new);
        for (pp, p) in self.prev_ship_pos.iter_mut().zip(self.world.structs.iter()) {
            pp.clear();
            pp.extend(p.interior.ships.iter().map(|s| s.pos));
        }
    }

    /// Build a PLAYBACK Game over a recorded match (from a parsed `.mir` or the live
    /// match's own journal). The world is rebuilt from (level, seed, scale) exactly like
    /// the recording; nothing records or writes during playback.
    #[allow(clippy::too_many_arguments)]
    fn new_replay(
        level: Level,
        seed: u64,
        scale: f64,
        orders: Vec<layer1::JournalEntry>,
        checkpoints: Vec<(u64, u64)>,
        end_tick: u64,
        winner: Faction,
    ) -> Game {
        let mut g = Game::new(level, seed, false, None, scale);
        g.world.set_journaling(None); // playback never records...
        g.replay_written = true; // ...never writes a .mir...
        g.log_written = true; // ...and never appends battle stats.
        g.enemies.clear();
        g.show_intro = false;
        let snap0 = (0u64, g.world.clone());
        g.replay = Some(ReplayState {
            orders,
            cursor: 0,
            checkpoints,
            check_cursor: 0,
            snapshots: vec![snap0],
            end_tick,
            winner,
            diverged: false,
            seek_target: None,
        });
        g
    }

    /// A playback Game of THIS live match (the end screen's "Watch replay") — straight
    /// from the in-memory journal, no file round-trip.
    fn make_replay_of_this_match(&self) -> Game {
        Game::new_replay(
            self.level.clone(),
            self.seed,
            self.scale,
            self.journal.borrow().clone(),
            self.checkpoints.clone(),
            self.world.tick,
            self.finished.unwrap_or(Faction::Neutral),
        )
    }

    /// Jump the playback to `target` (timeline click / "Watch again"): restore the nearest
    /// snapshot ≤ target, reposition the cursors, and let `update` fast-forward the rest.
    fn replay_seek(&mut self, target: u64) {
        let Some(rs) = &mut self.replay else { return };
        let target = target.min(rs.end_tick);
        if target < self.world.tick {
            let idx = rs
                .snapshots
                .iter()
                .rposition(|(t, _)| *t <= target)
                .expect("snapshot 0 always exists");
            let st = rs.snapshots[idx].0;
            self.world = rs.snapshots[idx].1.clone();
            rs.cursor = rs.orders.partition_point(|e| e.tick < st);
            rs.check_cursor = rs.checkpoints.partition_point(|(t, _)| *t <= st);
            self.finished = None; // re-latches on reaching the end again
        }
        rs.seek_target = Some(target);
    }

    /// Run one world tick, snapshotting first for render interpolation — the headless paths
    /// (`--selftest` / `--shot` advance loops) call this. The interactive frame loop instead
    /// snapshots only before the frame's **final** tick (see [`Game::update`]) so a multi-tick
    /// frame (high speed stops) does not copy every ship position once per tick for snapshots
    /// only the last of which is ever rendered.
    fn step_one_tick(&mut self) {
        self.snapshot();
        self.step_core();
    }

    /// One world tick **without** the render snapshot: on the decision cadence, decide+apply the
    /// enemy (and the player AI in demo mode) and run player automation; then step the world once.
    fn step_core(&mut self) {
        if let Some(rs) = &mut self.replay {
            // PLAYBACK: the journal drives every seat — no AI, no automation, no grace.
            while rs.cursor < rs.orders.len() && rs.orders[rs.cursor].tick <= self.world.tick {
                self.world.apply_record(&rs.orders[rs.cursor].record, &self.wp);
                rs.cursor += 1;
            }
        } else if self.world.tick % self.decision_interval == 0 {
            // Enemy seat(s) — one or two AI opponents (free-for-all). `&mut`: the stateful Simple
            // seat mutates its ledger each tick. The first ENEMY_GRACE_TICKS are a grace period:
            // the enemy holds still while the player reads the board.
            if self.world.tick >= ENEMY_GRACE_TICKS {
                for e in &mut self.enemies {
                    e.decide_and_apply(&mut self.world, &self.sim, &self.wp);
                }
            }
            // Player seat in demo mode (both seats AI).
            if let Some(pa) = &self.player_ai {
                pa.decide_and_apply(&mut self.world, &self.sim, &self.wp);
            } else {
                // Player's optional basic automation: drive each AUTO struct's internals with the
                // SAME Layer-1 greedy adapter the AI uses (delegate a struct's micro).
                self.run_player_automation();
            }
        }

        self.world.step(&self.sim, &self.wp);

        // Drain this tick's teleport jumps into wall-clock flashes (per tick, so no jump is
        // missed on multi-tick frames; capped like the kill flashes so a huge gated wave
        // cannot grow the list without bound).
        let now = get_time();
        for (sid, s) in self.world.structs.iter_mut().enumerate() {
            for (from, to) in s.interior.teleport_events.drain(..) {
                self.teleport_fx.push(TeleportFx { sid, from, to, born: now });
            }
        }
        if self.teleport_fx.len() > 512 {
            let cut = self.teleport_fx.len() - 512;
            self.teleport_fx.drain(..cut);
        }

        // Drain this tick's Layer-2 fleet losses: they land on the transit fleets themselves,
        // never on interior ships, so the interior liveness diff cannot see them — count them
        // into the battle-log metrics here and flash the loss at the fleet's map position.
        // Big single-tick losses burst into a few crosses, each jittered around the flock
        // (cosmetic macroquad RNG, like the interior flash's shooter pick — never the sim's).
        for e in self.world.fleet_death_events.drain(..) {
            if e.faction == Faction::Player {
                self.lost_ships += e.count as u64;
            } else if e.faction.is_real() {
                self.killed_ships += e.count as u64;
            }
            for _ in 0..e.count.min(6) {
                let jx = macroquad::rand::gen_range(-1.5f32, 1.5);
                let jy = macroquad::rand::gen_range(-1.5f32, 1.5);
                let at = layer1::Vec2::new(e.at.x + jx, e.at.y + jy);
                self.fleet_kill_fx.push((at, e.from, now));
            }
        }
        if self.fleet_kill_fx.len() > 512 {
            let cut = self.fleet_kill_fx.len() - 512;
            self.fleet_kill_fx.drain(..cut);
        }

        // Replay checkpoint cadence (cheap: one state_hash per 600 ticks). In playback,
        // VERIFY the recorded checkpoints in passing and keep the scrubber's snapshots;
        // in a live match, record them.
        if let Some(rs) = &mut self.replay {
            while rs.check_cursor < rs.checkpoints.len()
                && rs.checkpoints[rs.check_cursor].0 <= self.world.tick
            {
                let (t, h) = rs.checkpoints[rs.check_cursor];
                if t == self.world.tick && self.world.state_hash() != h && !rs.diverged {
                    rs.diverged = true;
                    eprintln!(
                        "[game] REPLAY DIVERGED at tick {t} — the build or level content \
                         does not match the recording"
                    );
                }
                rs.check_cursor += 1;
            }
            if self.world.tick % REPLAY_CHECKPOINT_EVERY == 0
                && rs.snapshots.last().map_or(true, |(t, _)| *t < self.world.tick)
            {
                let mut c = self.world.clone();
                c.set_journaling(None);
                rs.snapshots.push((self.world.tick, c));
            }
        } else if self.world.tick % REPLAY_CHECKPOINT_EVERY == 0 {
            self.checkpoints.push((self.world.tick, self.world.state_hash()));
        }

        // A playback seals exactly where the recording did.
        if let Some(rs) = &self.replay {
            if self.finished.is_none() && self.world.tick >= rs.end_tick {
                self.finished = Some(rs.winner);
            }
        }
        // Latch the outcome the first tick the match is decided. A sealed seat loses outright;
        // otherwise (horizon end in an AI match) fall back to the score-based world outcome.
        if self.finished.is_none() && self.match_over() {
            let e_done = self.all_enemies_finished();
            let p_done = self.seat_finished(Faction::Player);
            self.finished = Some(if e_done && !p_done {
                Faction::Player
            } else if p_done && !e_done {
                Faction::Ai(0)
            } else {
                self.world.outcome().winner.unwrap_or(Faction::Neutral)
            });
        }
        // Write the replay once, when the match seals (interactive-scale matches only —
        // the scale-1 headless harnesses run thousands of matches and stamp nothing).
        if self.finished.is_some() && !self.replay_written && self.scale == TICK_SCALE {
            self.replay_written = true;
            self.write_replay();
        }
    }

    /// Serialize the sealed match's replay — `.mir` v1, hand-rolled text — into
    /// `mi_replays/` next to the exe. Everything a bit-exact reproduction needs (see the
    /// levels round-trip pin): version stamps, the level-content hash, seed, scale bits,
    /// every seat's count-canonical orders with their ticks, periodic state-hash
    /// checkpoints (divergence detection), and the final outcome line. Native only for
    /// now — the web build keeps its journal in memory for the upload round.
    fn write_replay(&self) {
        if cfg!(target_arch = "wasm32") {
            return;
        }
        // AUTO save: replays/L<NN>/L<NN>_<epoch>.mir, then prune the subfolder to the
        // Settings cap (oldest first; -1 = keep everything).
        let dir = replay_dir().join(format!("L{:02}", self.level.id));
        let _ = std::fs::create_dir_all(&dir);
        let name = replay_file_name(self.level.id);
        match std::fs::write(dir.join(&name), self.replay_text()) {
            Ok(()) => println!("[game] replay written: replays/L{:02}/{name}", self.level.id),
            Err(e) => eprintln!("[game] replay write FAILED: {name}: {e}"),
        }
        let cap = BINDS.with(|b| b.borrow().replay_cap);
        prune_auto_replays(&dir, cap);
    }

    /// MANUAL save (the victory menu's "Save replay"): the same serialization, written to
    /// the `replays/` ROOT — the player's own shelf, never auto-pruned.
    fn save_replay_manual(&mut self) {
        if self.manual_saved || cfg!(target_arch = "wasm32") {
            return;
        }
        let dir = replay_dir();
        let _ = std::fs::create_dir_all(&dir);
        let name = replay_file_name(self.level.id);
        match std::fs::write(dir.join(&name), self.replay_text()) {
            Ok(()) => {
                self.manual_saved = true;
                println!("[game] replay saved: replays/{name}");
            }
            Err(e) => eprintln!("[game] replay save FAILED: {name}: {e}"),
        }
    }

    /// The `.mir` v1 text of this match (see `crates/game/src/replay.rs` for the reader).
    fn replay_text(&self) -> String {
        use std::fmt::Write as _;
        let mut out = String::new();
        let _ = writeln!(out, "mir 1");
        let _ = writeln!(out, "version {} {}", env!("GIT_HASH"), env!("CARGO_PKG_VERSION"));
        let _ = writeln!(out, "level {}", self.level.id);
        let _ = writeln!(out, "level_hash {:016x}", self.level.content_hash);
        let _ = writeln!(out, "seed {}", self.seed);
        let _ = writeln!(out, "scale_bits {:016x}", self.scale.to_bits());
        for e in self.journal.borrow().iter() {
            match e.record {
                layer1::OrderRecord::Move { sid, source, target, count, faction } => {
                    let _ = writeln!(
                        out,
                        "o {} m {} {} {} {} {}",
                        e.tick, sid, source, target, count, seat_code(faction)
                    );
                }
                layer1::OrderRecord::Fleet { from, to, count, faction } => {
                    let _ = writeln!(
                        out,
                        "o {} f {} {} {} {}",
                        e.tick, from, to, count, seat_code(faction)
                    );
                }
            }
        }
        for (t, h) in &self.checkpoints {
            let _ = writeln!(out, "h {} {:016x}", t, h);
        }
        let _ = writeln!(
            out,
            "end {} {} {} {} {:016x}",
            self.world.tick,
            seat_code(self.finished.unwrap_or(Faction::Neutral)),
            self.lost_ships,
            self.killed_ships,
            self.world.state_hash()
        );
        out
    }

    /// For each player-owned, AUTO-enabled structure, issue the Layer-1 greedy adapter's internal
    /// move orders into that struct's structure. This is the "delegate a struct" lesson — the
    /// identical policy the enemy runs on its own structs.
    ///
    /// **PARKED** — basic automation is quarantined pending a redesign. Every campaign level now sets
    /// `automation_available = false`, so this (and the `A` toggle / AUTO render, both gated on the
    /// same flag) are inert. The wiring is kept as the starting point for the redesign.
    fn run_player_automation(&mut self) {
        if !self.level.automation_available {
            return;
        }
        for p in 0..self.world.structs.len() {
            if !self.automated.get(p).copied().unwrap_or(false) {
                continue;
            }
            // Only bother where the player actually has a presence to command.
            let agg = self.world.struct_aggregate(p);
            if agg.player_subs == 0 && agg.player_ships == 0 {
                continue;
            }
            let orders = ai::greedy_layer1_orders(
                &self.world.structs[p].interior,
                &self.sim,
                Faction::Player,
                &self.greedy,
            );
            for o in orders {
                self.world.structs[p].interior.issue_order(o, Faction::Player);
            }
        }
    }

    /// Advance the sim for one rendered frame's worth of wall-clock `dt`.
    fn update(&mut self, dt: f64) {
        // Ease the camera toward its target regardless of pause (so zoom stays responsive).
        let target = if matches!(self.view, View::Interior(_)) { 1.0 } else { 0.0 };
        let k = 1.0 - (-ZOOM_LERP_RATE * dt as f32).exp();
        self.cam_t += (target - self.cam_t) * k;
        if (self.cam_t - target).abs() < 0.001 {
            self.cam_t = target;
        }

        // Expire finished death/teleport flashes on wall-clock (so they fade even while paused).
        let now = get_time();
        self.kill_fx.retain(|fx| now - fx.born < KILL_FX_TTL);
        self.teleport_fx.retain(|fx| now - fx.born < TELEPORT_FX_TTL);
        self.fleet_kill_fx.retain(|(_, _, born)| now - born < KILL_FX_TTL);

        // Timeline SEEK drain (playback): run toward the target at full speed, budgeted
        // per frame like the governor — non-blocking, the bar shows progress. Runs even
        // while paused or on the end overlay (scrubbing works everywhere).
        if let Some(target) = self.replay.as_ref().and_then(|r| r.seek_target) {
            let drain = PerfInstant::now();
            while self.world.tick < target && drain.elapsed_ms() < SIM_BUDGET_MS {
                self.step_core();
            }
            if self.world.tick >= target {
                if let Some(rs) = &mut self.replay {
                    rs.seek_target = None;
                }
            }
            self.tick_accum = 0.0;
            self.render_alpha = 1.0;
            return;
        }

        if self.paused() || self.match_over() {
            self.tick_accum = 0.0; // no stale fraction carries across a pause
            self.render_alpha = 1.0;
            return;
        }

        // FIXED TIMESTEP — frame-rate-INDEPENDENT. `tick_accum` is owed sim-**seconds**. We CLAMP the
        // input (owed) to MAX_FRAME_DT, so a hitch or huge speed buys at most that much catch-up;
        // beyond it the game SLOWS (consumes less wall-clock than elapsed) rather than dropping a
        // tick. Every consumed tick is a full FIXED_DT, so behaviour is identical at any framerate.
        let owed = (dt * self.speed()).min(MAX_FRAME_DT);
        self.tick_accum += owed;
        // Ships only ever die during a sim tick, so only snapshot liveness + diff for death-FX when a
        // tick will actually drain this frame (was done every frame, even no-tick ones). The snapshot
        // reuses a persistent buffer — no per-frame allocation.
        let will_tick = self.tick_accum >= FIXED_DT;
        if will_tick {
            self.snapshot_alive();
        }
        let mut ended_mid_frame = false;
        let drain = PerfInstant::now();
        while self.tick_accum >= FIXED_DT {
            // Snapshot for render interpolation only before the frame's FINAL tick — earlier
            // ticks' snapshots would be overwritten unread (a full per-ship position copy each,
            // up to ~30/frame at the top speed stop).
            if self.tick_accum < 2.0 * FIXED_DT {
                self.snapshot();
            }
            self.step_core();
            self.tick_accum -= FIXED_DT;
            if self.match_over() {
                self.tick_accum = 0.0;
                ended_mid_frame = true;
                break;
            }
            // FRAME-BUDGET GOVERNOR (web ask, 2026-07-10): when ticks cost more wall-clock
            // than the frame can afford, stop draining and RENDER — dropping the leftover
            // debt (the game SLOWS, the same rule as the owed clamp above). Without this, a
            // slow machine (the browser build especially) at a high speed stop melts into a
            // sub-10-fps slideshow whose effective speed is no higher anyway: fps collapses
            // until the owed clamp caps the very ticks it was stuttering to run. With it the
            // frame rate — camera, input, readability — stays live and the speed dial means
            // "as fast as this machine goes, smoothly". Checked only with ticks still owed,
            // so every frame makes at least one tick of progress; at 1× it never fires.
            if self.tick_accum >= FIXED_DT && drain.elapsed_ms() >= SIM_BUDGET_MS {
                self.tick_accum = 0.0;
                ended_mid_frame = true; // same render rule: no fresh snapshot — draw outright
                break;
            }
        }
        // On a mid-frame break (match end / budget) the loop may have skipped the final-tick
        // snapshot; render the current state outright (alpha 1) instead of lerping against a
        // stale snapshot.
        self.render_alpha = if ended_mid_frame {
            1.0
        } else {
            (self.tick_accum / FIXED_DT).clamp(0.0, 1.0) as f32
        };

        if will_tick {
            let t = PerfInstant::now();
            self.spawn_kill_fx(now);
            PERF.with(|p| {
                let mut p = p.borrow_mut();
                p.killfx_ms = ema(p.killfx_ms, t.elapsed_ms());
            });
        }
    }

    /// Snapshot current per-struct ship liveness into the reusable `prev_alive` buffer (capacity
    /// retained — no allocation after warm-up), for the post-tick death-FX diff.
    fn snapshot_alive(&mut self) {
        self.prev_alive.resize_with(self.world.structs.len(), Vec::new);
        for (pa, structure) in self.prev_alive.iter_mut().zip(self.world.structs.iter()) {
            pa.clear();
            pa.extend(structure.interior.ships.iter().map(|s| s.alive));
        }
    }

    /// Diff ship liveness against the `prev_alive` snapshot and spawn a death flash for every ship
    /// destroyed since it was taken. The "killing" line is drawn from a random living enemy still in
    /// engagement range, if any (so a soft-cap/attrition death with no enemy nearby just shows the
    /// cross). Called only on frames where a tick drained.
    fn spawn_kill_fx(&mut self, now: f64) {
        // Move the snapshot out so the loop can borrow `self.world` cleanly; restore it after (keeps
        // the buffer's capacity).
        let prev_alive = std::mem::take(&mut self.prev_alive);
        let mut spawned: Vec<KillFx> = Vec::new();
        let (mut lost, mut killed) = (0u64, 0u64);
        for (p, structure) in self.world.structs.iter().enumerate() {
            let Some(prev) = prev_alive.get(p) else { continue };
            let st = &structure.interior;
            for id in 0..prev.len() {
                // Index-guarded: the sim compacts its corpse-heavy ships Vec occasionally, so
                // last frame's snapshot may be longer than the current roster.
                let Some(sh) = st.ships.get(id) else { continue };
                if !prev[id] || sh.alive {
                    continue; // was already dead, or survived this frame
                }
                // The battle-log metrics ride the same liveness diff (presentation-only).
                if sh.faction == Faction::Player {
                    lost += 1;
                } else if sh.faction.is_real() {
                    killed += 1;
                }
                // Was the victim stationary? Under transit-fire gating a mover cannot have shot
                // it, so movers are excluded from the plausible shooters (the same predicate
                // combat gates on).
                let victim_stationary = sh.target.is_none();
                // Candidate shooters: every living enemy whose OWN reach covers the victim —
                // regular ships within the engagement radius, fortress garrisons within their
                // doubled reach (`ship_engagement_reach`, the per-shooter truth combat uses).
                // Picking uniformly from this pool traces the kill to a fortress ship exactly in
                // proportion to the fortress's share of the plausible shooters: lots of ships
                // nearby + a few in a fortress ⇒ most likely a nearby one. None ⇒ just the cross
                // (e.g. a soft-cap death).
                let candidates: Vec<layer1::Vec2> = st
                    .ships
                    .iter()
                    .enumerate()
                    .filter(|(j, o)| {
                        if !(o.alive && o.faction.is_real() && o.faction != sh.faction) {
                            return false;
                        }
                        if self.sim.transit_fire_gating
                            && victim_stationary
                            && (o.target.is_some() || o.drift_remaining > 0)
                        {
                            return false; // a mover cannot have shot a stationary victim
                        }
                        let reach = st.ship_engagement_reach(*j, &self.sim);
                        sh.pos.dist_sq(o.pos) <= reach * reach
                    })
                    .map(|(_, o)| o.pos)
                    .collect();
                let from = if candidates.is_empty() {
                    None
                } else {
                    Some(candidates[macroquad::rand::gen_range(0, candidates.len())])
                };
                spawned.push(KillFx { sid: p, at: sh.pos, from, born: now });
            }
        }
        self.lost_ships += lost;
        self.killed_ships += killed;
        self.kill_fx.append(&mut spawned);
        // Bound memory in a pathological mass-death frame (cosmetic only).
        if self.kill_fx.len() > 512 {
            let excess = self.kill_fx.len() - 512;
            self.kill_fx.drain(0..excess);
        }
        self.prev_alive = prev_alive; // restore the reusable buffer
    }

    /// The current blended camera (lens <-> interior of `focus`).
    fn camera(&self) -> Camera {
        let top = HUD_TOP_H;
        let bottom = HUD_BOTTOM_H;
        let mut lens = lens_camera(&self.world, top, bottom);
        lens.scale *= self.zoom[0]; // per-layer zoom magnification (slider / wheel)
        lens.cx += self.pan[0].0; // per-layer pan (WASD / right-drag / zoom anchor)
        lens.cy += self.pan[0].1;
        if self.cam_t <= 0.0001 {
            return lens;
        }
        let mut inter = interior_camera(&self.world, self.focus, top, bottom);
        inter.scale *= self.zoom[1];
        inter.cx += self.pan[1].0;
        inter.cy += self.pan[1].1;
        lerp_camera(lens, inter, self.cam_t)
    }

    /// The zoom-slider layer index for the current view (`0` = lens, `1` = interior; `2` reserved).
    fn zoom_layer(&self) -> usize {
        match self.view {
            View::Lens => 0,
            View::Interior(_) => 1,
        }
    }

    /// Interpolated draw position for ship `id` on struct `p`. A jump larger than any legitimate
    /// per-tick movement (a TELEPORTER departure) is **snapped**, not lerped — otherwise the ship
    /// would smear across the struct for a frame.
    #[inline]
    fn ship_draw_pos(&self, p: StructId, id: usize) -> layer1::Vec2 {
        const SNAP_DIST_SQ: f32 = 16.0; // > (ship step + orbit glide)² for any sane geometry
        let cur = self.world.structs[p].interior.ships[id].pos;
        match self.prev_ship_pos.get(p).and_then(|v| v.get(id)) {
            Some(prev) if cur.dist_sq(*prev) <= SNAP_DIST_SQ => layer1::Vec2 {
                x: prev.x + (cur.x - prev.x) * self.render_alpha,
                y: prev.y + (cur.y - prev.y) * self.render_alpha,
            },
            _ => cur,
        }
    }

    /// Interpolated effective lane progress for fleet `idx` (blends pre-step progress found by
    /// identity with current progress).
    fn fleet_draw_progress(&self, idx: usize) -> f32 {
        let cur = &self.world.fleets[idx];
        let cur_key = fleet_key(cur);
        let prev = self.prev_fleets.iter().find(|p| {
            p.faction == cur_key.faction
                && p.from == cur_key.from
                && p.to == cur_key.to
                && (cur_key.progress - p.progress) <= 0.6
                && (cur_key.progress - p.progress) >= -0.01
        });
        match prev {
            Some(p) => {
                let from = eff_pos(p.undock_remaining, p.progress);
                let to = eff_pos(cur.undock_remaining, cur.progress);
                from + (to - from) * self.render_alpha
            }
            None => eff_pos(cur.undock_remaining, cur.progress),
        }
    }
}

/// A fleet's effective lane position in [0,1]: it sits at ~0 while undocking, then advances.
#[inline]
fn eff_pos(undock_remaining: u32, progress: f32) -> f32 {
    if undock_remaining > 0 {
        0.0
    } else {
        progress
    }
}

#[inline]
fn fleet_key(f: &world::InterFleet) -> FleetKey {
    FleetKey { faction: f.faction, from: f.from, to: f.to, progress: f.progress, undock_remaining: f.undock_remaining }
}

/// The struct a level "opens on" (for interior framing): the `Layer1(p)` target, else the first
/// Player-owned structure, else struct 0.
fn start_struct(level: &Level) -> StructId {
    if let StartView::Layer1(p) = level.start_view {
        return p;
    }
    let (world, _wp) = level.world(0);
    for p in 0..world.structs.len() {
        if matches!(world.struct_aggregate(p).owner, StructOwner::Owned(Faction::Player)) {
            return p;
        }
    }
    0
}

/// The GUI sim operating point: the Layer-1 defaults with combat softened a touch so brawls last
/// long enough to read (purely spectacle tuning of data we own; determinism is intact — the sim
/// still draws all randomness from its own seeded PRNG).
fn gui_params(scale: f64) -> SimParams {
    let s_f = scale as f32;
    let mut p = SimParams::default();
    // Combat runs far slower than production for a deliberate, readable feel. The `0.0055` is the
    // reference (per REF_HZ-tick) value; ÷ scale re-grounds it to the game's tickrate.
    p.fire_prob = 0.0055 / scale;
    // No home-ground fire advantage (owner retune, 2026-07-08 — was 0.003/scale, a +55% fire
    // rate on owned ground): defence should come from position and numbers, not a hidden dial.
    // The mechanic survives in the sim (reference SimParams keeps it) — this is the game's 0.
    p.defender_fire_bonus = 0.0;
    // A wave in transit cannot "drive-by" shoot a garrison — an assault must *land* before it can
    // trade with defenders, while the garrison shoots the incoming wave. See `transit_fire_gating`.
    p.transit_fire_gating = true;
    // Grid-accelerated, no visible bubbles: each ship spreads its fire across all in-range enemies
    // (continuous-feeling attrition), instead of one-shotting one random target. See `spread_damage`.
    p.spread_damage = true;
    // Per-sub soft cap: a gentle linear bleed of each sub's surplus above its storage capacity.
    p.per_sub_attrition = true;
    // --- Re-ground to the game's tickrate: per-tick RATES ÷ scale, PERIODS / tick-counts × scale, so
    // the per-second behaviour is fixed regardless of TICK_HZ. (Counts/distances are left alone.)
    p.ship_speed /= s_f;
    p.orbit_rate /= s_f;
    p.orbit_glide /= s_f;
    p.prod_square_spin /= s_f;
    p.drift_speed /= s_f;
    // Ring-band churn: GUI-only dial (reference default 0 keeps headless geometry exact).
    // Semantics: per-tick velocity KICK as a fraction of the drift speed cap (0.5×ship_speed,
    // band-limited) — slow ballistic radial drift with soft band bounces; crosses the reserve
    // band in ~15 s at 1x, never outpaces flight, never snap-reverses. Set AFTER the rate
    // scaling (dimensionless fraction; the underlying speed cap already scales via ship_speed).
    // 0.05 → 0.03 with the v3 orbit model — the separation engine does the mixing work now,
    // the churn only has to dissolve radial standoffs.
    p.ring_jitter_step = 0.03;
    p.production_period = (p.production_period as f64 * scale).round() as u32;
    p.drift_ticks = (p.drift_ticks as f64 * scale).round() as u32;
    p.undock_ticks = (p.undock_ticks as f64 * scale).round().max(1.0) as u32;
    p
}

/// Build a level's world at a given tickrate `scale`: scale every per-sub capture resistance (eroded
/// by present-count per tick) and the inter-struct [`WorldParams`] so capture / transit pace matches
/// the [`gui_params`]`(scale)` rates. The game uses `scale = TICK_SCALE`; the headless tools use
/// `scale = 1.0` (the coarse reference resolution). The authored level values are never mutated.
fn build_scaled(level: &Level, seed: u64, scale: f64) -> (World, WorldParams) {
    let (mut world, mut wp) = level.world(seed);
    let s_f = scale as f32;
    for structure in &mut world.structs {
        for sub in &mut structure.interior.subs {
            sub.max_resistance *= s_f;
            sub.resistance *= s_f;
            // Authored orbital motion is a per-tick RATE — re-ground it like the SimParams rates.
            if let Some(o) = &mut sub.orbit {
                o.omega /= s_f;
            }
        }
    }
    wp.undock_ticks = (wp.undock_ticks as f64 * scale).round() as u32;
    wp.transit_speed /= s_f;
    // (Layer-2 combat rolls SimParams::fire_prob, which is already re-grounded above.)
    (world, wp)
}

/// A level's match horizon in ticks at a given tickrate `scale` (authored horizon × scale).
#[inline]
fn scaled_horizon(level: &Level, scale: f64) -> u64 {
    (level.horizon as f64 * scale).round() as u64
}

// =============================================================================================
// Mission briefings — styled-text popups shown before a mission (and re-read, no delay, in Memory)
// =============================================================================================

/// Seconds between successive slow **human-speech** tokens. Human speech is deliberately slow: one
/// token at a time. Tunable.
const HUMAN_TOKEN_INTERVAL: f64 = 1.2;
/// Seconds of read-out the **Wait** button fast-forwards per press.
const BRIEFING_WAIT_SECONDS: f64 = 60.0;
/// Per-token character budget for the LLM-like stream tokenizer: a token is a short run of
/// characters, so injected machine text can land **mid-word** (the input-stream feel).
const STREAM_TOKEN_CHARS: usize = 4;
/// Line height (px) for briefing text.
const BRIEF_LINE_H: f32 = 30.0;
/// Pixels scrolled per mouse-wheel notch inside a briefing.
const BRIEF_SCROLL_SPEED: f32 = 44.0;

// Briefing palette — the markup colours (`<blue>` … `<gray>`), plus the default and the error red.
const BRIEF_DEFAULT: Color = Color::new(0.87, 0.90, 0.95, 1.0);
const BRIEF_BLUE: Color = Color::new(0.46, 0.66, 1.00, 1.0);
const BRIEF_GREEN: Color = Color::new(0.52, 0.86, 0.56, 1.0);
const BRIEF_GOLD: Color = Color::new(1.00, 0.84, 0.42, 1.0);
const BRIEF_GRAY: Color = Color::new(0.60, 0.64, 0.70, 1.0);
const BRIEF_ERR_COL: Color = Color::new(1.00, 0.32, 0.30, 1.0);

/// One styled token of briefing text. The stream renders as a continuous character flow (spaces
/// come from the token text), so tokens may be sub-word — which lets machine text be injected
/// mid-word. Each carries its own style; font is a deliberate future hook.
#[derive(Clone)]
struct CutSeg {
    /// Token text — a short character run (empty for a hard `newline`).
    text: String,
    color: Color,
    size: u16,
    /// The `<shining>` effect (a moving golden glint).
    shining: bool,
    /// Machine speech reveals **instantly**; human speech reveals one token per interval. Structural
    /// `newline` tokens are always instant.
    instant: bool,
    /// A hard line break (the token carries no text).
    newline: bool,
}

/// The active style while parsing briefing markup.
#[derive(Clone, Copy)]
struct BriefStyle {
    color: Color,
    size: u16,
    shining: bool,
    instant: bool,
}
impl Default for BriefStyle {
    fn default() -> Self {
        BriefStyle { color: BRIEF_DEFAULT, size: 22, shining: false, instant: false }
    }
}

/// Tokenize `text` into stream tokens with `style` (short runs; `\n` → a hard break that is always
/// instant), appended to `out`.
fn push_styled(out: &mut Vec<CutSeg>, text: &str, style: BriefStyle) {
    let mk = |s: String, newline: bool| CutSeg {
        text: s,
        color: style.color,
        size: style.size,
        shining: style.shining,
        instant: style.instant || newline,
        newline,
    };
    let mut cur = String::new();
    for ch in text.chars() {
        if ch == '\n' {
            if !cur.is_empty() {
                out.push(mk(std::mem::take(&mut cur), false));
            }
            out.push(mk(String::new(), true));
            continue;
        }
        cur.push(ch);
        if cur.chars().count() >= STREAM_TOKEN_CHARS {
            out.push(mk(std::mem::take(&mut cur), false));
        }
    }
    if !cur.is_empty() {
        out.push(mk(cur, false));
    }
}

/// Parse `<tag>…<\tag>` briefing markup into styled tokens. Recognised tags: the **speech kind**
/// (`human_speech` = slow, `machine_speech` = instant), the **colours** `blue` / `green` / `golden`
/// / `gray`, and the `shining` effect. A close tag is written `<\tag>` (note the backslash); several
/// may be combined with `+`, e.g. `<\golden+\shining>`. Unknown tags are ignored. Text outside tags
/// inherits the current style.
fn parse_briefing(markup: &str) -> Vec<CutSeg> {
    let mut out = Vec::new();
    let mut style = BriefStyle::default();
    let mut chars = markup.chars().peekable();
    let mut buf = String::new();
    while let Some(c) = chars.next() {
        if c == '<' {
            if !buf.is_empty() {
                push_styled(&mut out, &buf, style);
                buf.clear();
            }
            let mut tag = String::new();
            for tc in chars.by_ref() {
                if tc == '>' {
                    break;
                }
                tag.push(tc);
            }
            apply_tag(&tag, &mut style);
        } else {
            buf.push(c);
        }
    }
    if !buf.is_empty() {
        push_styled(&mut out, &buf, style);
    }
    out
}

/// Apply one (possibly `+`-combined) markup tag to the parse `style`. A leading `\` marks a closing
/// tag (which reverts that attribute to its default).
fn apply_tag(tag: &str, style: &mut BriefStyle) {
    for part in tag.split('+') {
        let (closing, name) = match part.strip_prefix('\\') {
            Some(rest) => (true, rest),
            None => (false, part),
        };
        match name {
            "blue" => style.color = if closing { BRIEF_DEFAULT } else { BRIEF_BLUE },
            "green" => style.color = if closing { BRIEF_DEFAULT } else { BRIEF_GREEN },
            "golden" => style.color = if closing { BRIEF_DEFAULT } else { BRIEF_GOLD },
            "gray" | "grey" => style.color = if closing { BRIEF_DEFAULT } else { BRIEF_GRAY },
            "shining" => style.shining = !closing,
            // Human speech is the slow default; machine speech is instant. Either close reverts to it.
            "human_speech" => style.instant = false,
            "machine_speech" => style.instant = !closing,
            _ => {} // unknown tag: ignored (never displayed as text)
        }
    }
}

/// Whether a briefing is being shown for the first time before a mission (slow, Wait + Close-refusal)
/// or re-read from **Memory** (everything instant, free Close).
#[derive(Clone, Copy, PartialEq)]
enum BriefMode {
    Mission,
    Memory,
}

/// A playing briefing: one ordered token `stream`. **Human** tokens read out slowly (one per
/// [`HUMAN_TOKEN_INTERVAL`]); **machine** (and structural) tokens appear instantly. In Mission mode
/// a refused **Close** splices its red error into the stream at the read cursor — shown at once,
/// mid-word — and the human speech resumes after it. In Memory mode the whole stream is shown up
/// front. Overflow **scrolls**, it never clips.
struct Briefing {
    stream: Vec<CutSeg>,
    /// How many tokens from the front have been read out (shown).
    revealed: usize,
    /// Time accumulated toward reading the next (human) token.
    timer: f64,
    /// Scroll offset (px from the top of the laid-out content).
    scroll: f32,
    /// While true the view stays pinned to the newest token; a manual scroll-up releases it.
    follow: bool,
    mode: BriefMode,
}

impl Briefing {
    fn new(markup: &str, mode: BriefMode) -> Briefing {
        let stream = parse_briefing(markup);
        let revealed = if mode == BriefMode::Memory { stream.len() } else { 0 };
        let mut b = Briefing {
            stream,
            revealed,
            timer: 0.0,
            scroll: 0.0,
            follow: mode == BriefMode::Mission,
            mode,
        };
        b.reveal_instant(); // show any leading machine/structural tokens at once
        b
    }

    /// True once the whole stream has been read out — only then does Mission **Close** actually
    /// close. (Spamming Close never reaches this: each press splices in more to read.)
    fn fully_revealed(&self) -> bool {
        self.revealed >= self.stream.len()
    }

    /// Advance the read cursor past any instant (machine / structural) tokens at the front.
    fn reveal_instant(&mut self) {
        while self.revealed < self.stream.len() && self.stream[self.revealed].instant {
            self.revealed += 1;
        }
    }

    /// Advance the slow human read-out by `dt` wall-clock seconds (machine tokens stay instant).
    fn update(&mut self, dt: f64) {
        self.reveal_instant();
        if self.fully_revealed() {
            return;
        }
        self.timer += dt;
        while self.timer >= HUMAN_TOKEN_INTERVAL && !self.fully_revealed() {
            self.timer -= HUMAN_TOKEN_INTERVAL;
            self.revealed += 1; // one human token
            self.reveal_instant(); // then any machine tokens that follow it
        }
        if self.fully_revealed() {
            self.timer = 0.0;
        }
    }

    /// **Wait**: read out as much as ≈[`BRIEFING_WAIT_SECONDS`] of elapsed time would.
    fn wait(&mut self) {
        self.timer += BRIEFING_WAIT_SECONDS;
        self.update(0.0);
    }

    /// Splice the red "Close refused" error into the stream **at the current read cursor** and reveal
    /// it **at once** (it is machine output) — landing mid-token (`…ex<Error: …>ample.`); the slow
    /// human speech resumes after it. Mission mode only.
    fn inject_close_error(&mut self) {
        let style = BriefStyle { color: BRIEF_ERR_COL, size: 22, shining: false, instant: true };
        let mut err = Vec::new();
        push_styled(
            &mut err,
            "<Error: ToolCall:Close called, but you haven't finished receiving instructions. Please try again later.>",
            style,
        );
        let at = self.revealed.min(self.stream.len());
        self.stream.splice(at..at, err);
        self.reveal_instant(); // the spliced error is all instant — show it now, then resume human
    }
}

/// What happens once a briefing's Close actually closes it.
#[derive(Clone, Copy)]
enum AfterBriefing {
    /// Begin level index `usize` (0-based).
    StartLevel(usize),
    /// Return to the Memory page (re-reads).
    BackToMemory,
}

/// Popup + button geometry for a briefing, derived from the current screen size.
struct BriefingLayout {
    box_x: f32,
    box_y: f32,
    box_w: f32,
    box_h: f32,
    text_x: f32,
    text_y: f32,
    text_w: f32,
    text_bottom: f32,
    wait_btn: Rect,
    close_btn: Rect,
}

fn briefing_layout() -> BriefingLayout {
    let sw = screen_width();
    let sh = screen_height();
    let box_w = (sw * 0.72).clamp(420.0, 920.0);
    let box_h = (sh * 0.70).clamp(300.0, 640.0);
    let box_x = (sw - box_w) * 0.5;
    let box_y = (sh - box_h) * 0.5;
    let pad = 30.0;
    let btn_h = 34.0;
    let btn_y = box_y + box_h - pad - btn_h;
    let text_x = box_x + pad;
    let text_y = box_y + pad + 6.0;
    let text_w = box_w - 2.0 * pad - 14.0; // leave a gutter for the scrollbar on the right
    let text_bottom = btn_y - 16.0;
    let wbw = 100.0;
    let cbw = 110.0;
    let wait_btn = Rect::new(box_x + box_w * 0.5 - wbw * 0.5, btn_y, wbw, btn_h);
    let close_btn = Rect::new(box_x + box_w - pad - cbw, btn_y, cbw, btn_h);
    BriefingLayout { box_x, box_y, box_w, box_h, text_x, text_y, text_w, text_bottom, wait_btn, close_btn }
}

/// Lay the revealed tokens out into content-space baselines `(token index, x, y)` with continuous
/// (no inter-token spacing) stream wrapping; return the total content height. `y` is measured from
/// the content top (before scroll).
fn layout_briefing(b: &Briefing, text_x: f32, text_w: f32) -> (Vec<(usize, f32, f32)>, f32) {
    let mut placed = Vec::new();
    let mut cx = text_x;
    let mut cy = BRIEF_LINE_H; // first baseline
    let n = b.revealed.min(b.stream.len());
    for i in 0..n {
        let seg = &b.stream[i];
        if seg.newline {
            cx = text_x;
            cy += BRIEF_LINE_H;
            continue;
        }
        let w = measure_text(&seg.text, None, seg.size, 1.0).width;
        if cx > text_x && cx + w > text_x + text_w {
            cx = text_x;
            cy += BRIEF_LINE_H;
        }
        placed.push((i, cx, cy));
        cx += w;
    }
    (placed, cy)
}

/// The effective scroll offset (auto-following the bottom while `follow`) and the max scroll.
fn briefing_scroll(b: &Briefing, content_h: f32, visible_h: f32) -> (f32, f32) {
    let max_scroll = (content_h + BRIEF_LINE_H - visible_h).max(0.0);
    let scroll = if b.follow { max_scroll } else { b.scroll.clamp(0.0, max_scroll) };
    (scroll, max_scroll)
}

fn draw_briefing(b: &Briefing) {
    let lay = briefing_layout();
    let sw = screen_width();
    let sh = screen_height();
    draw_rectangle(0.0, 0.0, sw, sh, Color::new(0.02, 0.03, 0.05, 0.9));
    draw_rectangle(lay.box_x, lay.box_y, lay.box_w, lay.box_h, Color::new(0.06, 0.08, 0.12, 0.98));
    draw_rectangle_lines(lay.box_x, lay.box_y, lay.box_w, lay.box_h, 2.0, Color::new(0.30, 0.55, 0.82, 0.85));

    let (placed, content_h) = layout_briefing(b, lay.text_x, lay.text_w);
    let visible_h = lay.text_bottom - lay.text_y;
    let (scroll, max_scroll) = briefing_scroll(b, content_h, visible_h);

    let t = get_time() as f32;
    for &(i, x, y) in &placed {
        let sy = lay.text_y + y - scroll;
        if sy < lay.text_y - BRIEF_LINE_H || sy > lay.text_bottom {
            continue; // scrolled off-screen above/below the visible window
        }
        let seg = &b.stream[i];
        let col = if seg.shining {
            // A glint that sweeps horizontally and pulses in time — lerp the colour toward white.
            let g = (((t * 2.4 - x * 0.012).sin() * 0.5 + 0.5).powf(2.0) * 0.7).clamp(0.0, 1.0);
            Color::new(
                seg.color.r + (1.0 - seg.color.r) * g,
                seg.color.g + (1.0 - seg.color.g) * g,
                seg.color.b + (1.0 - seg.color.b) * g,
                seg.color.a,
            )
        } else {
            seg.color
        };
        draw_text(&seg.text, x, sy, seg.size as f32, col);
    }

    // Scrollbar in the right gutter, shown once content overflows the window.
    if max_scroll > 0.5 {
        let bar_w = 6.0;
        let track_x = lay.box_x + lay.box_w - 16.0 - bar_w;
        let track_h = visible_h;
        draw_rectangle(track_x, lay.text_y, bar_w, track_h, Color::new(1.0, 1.0, 1.0, 0.07));
        let total = content_h + BRIEF_LINE_H;
        let thumb_h = (track_h * (visible_h / total)).clamp(18.0, track_h);
        let thumb_y = lay.text_y + (scroll / max_scroll) * (track_h - thumb_h);
        draw_rectangle(track_x, thumb_y, bar_w, thumb_h, Color::new(0.55, 0.70, 0.90, 0.55));
    }

    // Buttons. Mission: Wait (always) + Close (active only once fully read). Memory: just Close.
    match b.mode {
        BriefMode::Mission => {
            draw_brief_button(lay.wait_btn, "Wait", true);
            draw_brief_button(lay.close_btn, "Close", b.fully_revealed());
        }
        BriefMode::Memory => {
            draw_brief_button(lay.close_btn, "Close", true);
        }
    }
}

fn draw_brief_button(r: Rect, label: &str, active: bool) {
    let bg = if active { Color::new(0.16, 0.24, 0.36, 0.95) } else { Color::new(0.10, 0.12, 0.16, 0.9) };
    let border = if active { Color::new(0.45, 0.70, 0.95, 0.9) } else { Color::new(0.30, 0.34, 0.40, 0.8) };
    draw_rectangle(r.x, r.y, r.w, r.h, bg);
    draw_rectangle_lines(r.x, r.y, r.w, r.h, 1.5, border);
    let d = measure_text(label, None, 20, 1.0);
    let col = if active { HUD_TEXT } else { HUD_MUTED };
    draw_text(label, r.x + (r.w - d.width) * 0.5, r.y + r.h * 0.5 + d.height * 0.35, 20.0, col);
}

/// The raw briefing markup for a level id, if it has one yet. The legacy campaign's briefings are
/// deprecated; new mission briefings are authored here one at a time.
// =============================================================================================
// App-level state machine
// =============================================================================================

enum AppState {
    MainMenu { idx: usize },
    LevelSelect { idx: usize },
    /// The Memory page: a list of received mission briefings (re-readable, no delay). `idx` = row.
    Memory { tab: usize, idx: usize, expanded: u32, scroll: usize },
    /// The Settings page: control rebinding. `idx` = highlighted row (an [`ACTIONS`] row, or the
    /// trailing "Reset to defaults"); `capturing` = waiting for the next key press to bind it.
    Settings { idx: usize, capturing: bool },
    /// A mission-briefing popup (the narrative layer); `after` is applied when it closes.
    Briefing { player: Briefing, after: AfterBriefing },
    InLevel { game: Box<Game> },
}

struct App {
    levels: Vec<Level>,
    progress: Progress,
    /// The player's persistent **Notes** (carried between levels; saved to `mi_notes.txt`).
    notes: String,
    /// Whether the in-level Notes editor overlay is currently open.
    notes_open: bool,
    state: AppState,
    seed: u64,
    auto: bool,
    /// The `--text` narrative layer toggle (see [`Config::text`]).
    text: bool,
    /// The replay browser's scanned model (rebuilt each time the Memory page opens): the
    /// manual-saves shelf first, then one section per campaign level's auto subfolder.
    replay_sections: Vec<ReplaySection>,
}

impl App {
    fn new(cfg: &Config) -> App {
        let levels = campaign();
        let mut progress = Progress::load();
        if cfg.unlock_all {
            for l in &levels {
                progress.unlocked.insert(l.id);
                progress.memories.insert(l.id); // so Memory is populated for testing
            }
        }

        // If a start level was requested on the CLI, jump straight into it (no briefing).
        let state = if let Some(n) = cfg.start_level {
            let idx = n.saturating_sub(1).min(levels.len() - 1);
            let game = Game::new(levels[idx].clone(), cfg.seed, cfg.auto, None, TICK_SCALE);
            AppState::InLevel { game: Box::new(game) }
        } else {
            AppState::MainMenu { idx: 0 }
        };

        App {
            levels,
            progress,
            notes: load_notes(),
            text: cfg.text,
            notes_open: false,
            replay_sections: Vec::new(),
            state,
            seed: cfg.seed,
            auto: cfg.auto,
        }
    }

    /// Enter level index `idx` (0-based): show its mission briefing first **if** it has one and has
    /// not been received yet (first play); otherwise begin the level directly.
    fn enter_level(&mut self, idx: usize) {
        let level_id = self.levels[idx].id;
        // Briefings are `--text` only (owner, 2026-07-08): without the flag the mission
        // starts directly and the briefing is neither shown nor marked received.
        if self.text {
            if let Some(markup) = self.levels[idx].briefing.clone() {
                if !self.progress.memories.contains(&level_id) {
                    // Logic-based text: render the template against the PREVIOUS battle's
                    // metrics + the player's notes before the markup parser sees it.
                    let ctx = narrative::last_metrics(&self.notes);
                    let player =
                        Briefing::new(&narrative::render(&markup, &ctx), BriefMode::Mission);
                    self.progress.memories.insert(level_id);
                    self.progress.save();
                    self.state =
                        AppState::Briefing { player, after: AfterBriefing::StartLevel(idx) };
                    return;
                }
            }
        }
        self.start_level(idx);
    }

    /// Begin level index `idx` (0-based) directly, no briefing.
    fn start_level(&mut self, idx: usize) {
        let game = Game::new(self.levels[idx].clone(), self.seed, self.auto, None, TICK_SCALE);
        self.state = AppState::InLevel { game: Box::new(game) };
    }

    /// Record a win on level `idx` (0-based): mark it COMPLETED and unlock the next level
    /// (the last level's win is recorded too — it has no successor to unlock). Persists.
    fn record_win(&mut self, idx: usize) {
        let mut changed = self.progress.completed.insert(self.levels[idx].id);
        if let Some(next) = self.levels.get(idx + 1) {
            changed |= self.progress.unlocked.insert(next.id);
        }
        if changed {
            self.progress.save();
        }
    }
}

// =============================================================================================
// Update + input per app state
// =============================================================================================

/// The main-menu rows. **Memory is `--text` only** (owner, 2026-07-08): without the
/// narrative layer the page would always be empty, so it is hidden entirely.
fn main_menu_items(_text: bool) -> &'static [&'static str] {
    // Memory is ALWAYS present (owner, 2026-07-10): its default tab is the replay
    // browser; the text-memories tab appears only under `--text`.
    &["Play", "Level Select", "Memory", "Settings", "Quit"]
}

/// A transition the in-level update wants to apply to `App` *after* the `app.state` borrow ends
/// (so we never hold a borrow of `app.state` across a method that mutates `app`).
enum LevelAction {
    /// No transition this frame.
    None,
    /// (Re)start level index `idx` (0-based).
    Start(usize),
    /// Go to the level-select screen with row `idx` highlighted.
    ToSelect(usize),
    /// Go to the MAIN menu (the pause menu's exit — owner, 2026-07-08: not level select).
    ToMenu,
    /// Record a win on level `idx`; if `advance`, move on to the next level (or the menu).
    WinThen { idx: usize, advance: bool },
    /// Swap to a PLAYBACK of the just-finished match (from its in-memory journal).
    WatchReplay,
}


/// Drive the app one frame (input + sim). Returns `true` to request quitting.
fn app_update(app: &mut App, dt: f64) -> bool {
    // Notes editor (in-mission only) takes input priority and freezes the game while open.
    let in_level = matches!(app.state, AppState::InLevel { .. });
    let pause_open = matches!(&app.state, AppState::InLevel { game } if game.paused && game.pause_buttons);
    if in_level {
        if app.notes_open {
            handle_notes_input(app);
            return false;
        } else if app.text && !pause_open && notes_icon_clicked() {
            app.notes_open = true;
            while get_char_pressed().is_some() {} // drop any chars queued from gameplay keys
            return false;
        }
    }

    match &mut app.state {
        AppState::MainMenu { idx } => {
            let n_items = main_menu_items(app.text).len();
            let mut idx_v = *idx;
            if is_key_pressed(KeyCode::Down) || is_key_pressed(KeyCode::S) {
                idx_v = (idx_v + 1) % n_items;
            }
            if is_key_pressed(KeyCode::Up) || is_key_pressed(KeyCode::W) {
                idx_v = (idx_v + n_items - 1) % n_items;
            }
            *idx = idx_v;
            // Mouse hover/click selects.
            let click = is_mouse_button_pressed(MouseButton::Left);
            if click && fullscreen_btn_at_mouse(false) {
                toggle_fullscreen();
                return false;
            }
            if let Some(hovered) = menu_item_at_mouse(n_items) {
                *idx = hovered;
                if click {
                    return activate_main_menu(app, hovered);
                }
            }
            if is_key_pressed(KeyCode::Enter) || is_key_pressed(KeyCode::Space) {
                return activate_main_menu(app, idx_v);
            }
            false
        }
        AppState::LevelSelect { idx } => {
            let n = app.levels.len();
            let mut sel = *idx;
            if is_key_pressed(KeyCode::Down) || is_key_pressed(KeyCode::S) {
                sel = (sel + 1) % n;
            }
            if is_key_pressed(KeyCode::Up) || is_key_pressed(KeyCode::W) {
                sel = (sel + n - 1) % n;
            }
            *idx = sel;
            if is_key_pressed(KeyCode::Escape) {
                app.state = AppState::MainMenu { idx: 0 };
                return false;
            }
            let click = is_mouse_button_pressed(MouseButton::Left);
            if let Some(hovered) = level_row_at_mouse(n) {
                *idx = hovered;
                if click && app.progress.unlocked.contains(&app.levels[hovered].id) {
                    app.enter_level(hovered);
                    return false;
                }
            }
            if (is_key_pressed(KeyCode::Enter) || is_key_pressed(KeyCode::Space))
                && app.progress.unlocked.contains(&app.levels[sel].id)
            {
                app.enter_level(sel);
            }
            false
        }
        AppState::Settings { idx, capturing } => {
            let nrows = ACTIONS.len() + 3; // actions + replay cap + fullscreen + reset
            if *capturing {
                if *idx == ACTIONS.len() {
                    // FREE-INTEGER entry for the auto-replay cap (owner: -1 = infinite).
                    // Digits and a leading '-' build the value in SETTINGS_ENTRY; Enter
                    // commits (clamped to >= -1), Esc cancels, Backspace edits.
                    while let Some(c) = get_char_pressed() {
                        if c.is_ascii_digit() || c == '-' {
                            SETTINGS_ENTRY.with(|e| e.borrow_mut().push(c));
                        }
                    }
                    if is_key_pressed(KeyCode::Backspace) {
                        SETTINGS_ENTRY.with(|e| {
                            e.borrow_mut().pop();
                        });
                    }
                    if is_key_pressed(KeyCode::Enter) {
                        let txt = SETTINGS_ENTRY.with(|e| e.borrow().clone());
                        if let Ok(v) = txt.trim().parse::<i64>() {
                            BINDS.with(|b| {
                                let mut b = b.borrow_mut();
                                b.replay_cap = v.max(-1);
                                b.save();
                            });
                        }
                        *capturing = false;
                    }
                    if is_key_pressed(KeyCode::Escape) {
                        *capturing = false;
                    }
                    return false;
                }
                // Waiting for the next key: any nameable key binds (swapping on conflict and
                // saving the file); Esc cancels; anything unnameable keeps waiting.
                if let Some(k) = get_last_key_pressed() {
                    if k == KeyCode::Escape {
                        *capturing = false;
                    } else if KEY_NAMES.iter().any(|&(kk, _)| kk == k) {
                        let row = *idx;
                        BINDS.with(|b| {
                            let mut b = b.borrow_mut();
                            b.set(row, k);
                            b.save();
                        });
                        *capturing = false;
                    }
                }
                return false;
            }
            if is_key_pressed(KeyCode::Escape) {
                app.state = AppState::MainMenu { idx: 3 };
                return false;
            }
            if is_key_pressed(KeyCode::Down) || is_key_pressed(KeyCode::S) {
                *idx = (*idx + 1) % nrows;
            }
            if is_key_pressed(KeyCode::Up) || is_key_pressed(KeyCode::W) {
                *idx = (*idx + nrows - 1) % nrows;
            }
            let click = is_mouse_button_pressed(MouseButton::Left);
            if let Some(hovered) = settings_row_at_mouse(nrows) {
                *idx = hovered;
            }
            if (click && settings_row_at_mouse(nrows).is_some())
                || is_key_pressed(KeyCode::Enter)
                || is_key_pressed(KeyCode::Space)
            {
                if *idx < ACTIONS.len() {
                    *capturing = true;
                } else if *idx == ACTIONS.len() {
                    // The auto-replay cap: open the free-integer entry.
                    SETTINGS_ENTRY.with(|e| e.borrow_mut().clear());
                    while get_char_pressed().is_some() {} // drop the triggering char
                    *capturing = true;
                } else if *idx == ACTIONS.len() + 1 {
                    toggle_fullscreen();
                } else {
                    BINDS.with(|b| {
                        let mut b = b.borrow_mut();
                        *b = Bindings::defaults();
                        b.save();
                    });
                }
            }
            false
        }
        AppState::Memory { .. } => {
            // Tab 1 (text memories): received briefings — level indices with a briefing the
            // player has been shown. Tab 0 (replays): the scanned browser model.
            let entries: Vec<usize> = (0..app.levels.len())
                .filter(|&i| {
                    app.levels[i].briefing.is_some()
                        && app.progress.memories.contains(&app.levels[i].id)
                })
                .collect();
            let n = entries.len();
            let text_on = app.text;
            enum Act {
                Back,
                OpenBrief(usize),
                OpenReplay(std::path::PathBuf),
            }
            let action = {
                let AppState::Memory { tab, idx, expanded, scroll } = &mut app.state else {
                    unreachable!()
                };
                if !text_on {
                    *tab = 0;
                }
                // Tab switching: chips under the title (click), or Left/Right.
                if text_on
                    && (is_key_pressed(KeyCode::Left)
                        || is_key_pressed(KeyCode::Right)
                        || (is_mouse_button_pressed(MouseButton::Left)
                            && memory_tab_at_mouse(text_on).map_or(false, |t| t != *tab)))
                {
                    *tab ^= 1;
                    *idx = 0;
                    *scroll = 0;
                }
                if is_key_pressed(KeyCode::Escape) {
                    Some(Act::Back)
                } else if *tab == 0 {
                    // ---- The REPLAY BROWSER (owner design, 2026-07-10). ----
                    let rows = browser_rows(&app.replay_sections, *expanded);
                    if rows.is_empty() {
                        None
                    } else {
                        let visible = memory_visible_rows();
                        let mut sel = (*idx).min(rows.len() - 1);
                        if is_key_pressed(KeyCode::Down) || is_key_pressed(KeyCode::S) {
                            sel = (sel + 1) % rows.len();
                        }
                        if is_key_pressed(KeyCode::Up) || is_key_pressed(KeyCode::W) {
                            sel = (sel + rows.len() - 1) % rows.len();
                        }
                        // Wheel scrolls; hover highlights; selection stays on-screen.
                        let wheel = mouse_wheel().1;
                        if wheel < 0.0 {
                            *scroll = (*scroll + 3).min(rows.len().saturating_sub(visible));
                        } else if wheel > 0.0 {
                            *scroll = scroll.saturating_sub(3);
                        }
                        let click = is_mouse_button_pressed(MouseButton::Left);
                        let hovered = memory_row_at_mouse(*scroll, rows.len());
                        if let Some(h) = hovered {
                            sel = h;
                        }
                        if sel < *scroll {
                            *scroll = sel;
                        } else if sel >= *scroll + visible {
                            *scroll = sel + 1 - visible;
                        }
                        *idx = sel;
                        let chosen = if is_key_pressed(KeyCode::Enter)
                            || is_key_pressed(KeyCode::Space)
                        {
                            Some(sel)
                        } else if click {
                            hovered
                        } else {
                            None
                        };
                        match chosen.map(|c| rows[c]) {
                            Some(BrowserRow::Header(sec)) => {
                                *expanded ^= 1u32 << sec;
                                None
                            }
                            Some(BrowserRow::Entry(sec, e)) => Some(Act::OpenReplay(
                                app.replay_sections[sec].entries[e].path.clone(),
                            )),
                            None => None,
                        }
                    }
                } else if n == 0 {
                    None
                } else {
                    // ---- Text memories (the original briefing list). ----
                    let mut sel = (*idx).min(n - 1);
                    if is_key_pressed(KeyCode::Down) || is_key_pressed(KeyCode::S) {
                        sel = (sel + 1) % n;
                    }
                    if is_key_pressed(KeyCode::Up) || is_key_pressed(KeyCode::W) {
                        sel = (sel + n - 1) % n;
                    }
                    let click = is_mouse_button_pressed(MouseButton::Left);
                    let hovered = level_row_at_mouse(n);
                    if let Some(h) = hovered {
                        sel = h;
                    }
                    *idx = sel;
                    let chosen = if is_key_pressed(KeyCode::Enter) || is_key_pressed(KeyCode::Space) {
                        Some(sel)
                    } else if click {
                        hovered
                    } else {
                        None
                    };
                    chosen.map(Act::OpenBrief)
                }
            };
            match action {
                Some(Act::Back) => app.state = AppState::MainMenu { idx: 0 },
                Some(Act::OpenBrief(row)) => {
                    if let Some(&lvl_idx) = entries.get(row) {
                        if let Some(markup) = app.levels[lvl_idx].briefing.clone() {
                            let ctx = narrative::last_metrics(&app.notes);
                            let player =
                                Briefing::new(&narrative::render(&markup, &ctx), BriefMode::Memory);
                            app.state = AppState::Briefing { player, after: AfterBriefing::BackToMemory };
                        }
                    }
                }
                Some(Act::OpenReplay(path)) => {
                    if let Some(g) = load_replay_from(path.to_string_lossy().as_ref(), &app.levels) {
                        app.state = AppState::InLevel { game: Box::new(g) };
                    }
                }
                None => {}
            }
            false
        }
        AppState::Briefing { .. } => {
            // Drive the reveal + buttons while holding the borrow; defer the close transition (which
            // mutates `app`) until after the borrow ends — same discipline as the InLevel arm.
            let close_after = {
                let AppState::Briefing { player, after } = &mut app.state else { unreachable!() };
                let lay = briefing_layout();
                let (mx, my) = mouse_position();
                let pt = vec2(mx, my);
                let mut close_after: Option<AfterBriefing> = None;
                if is_mouse_button_pressed(MouseButton::Left) {
                    match player.mode {
                        BriefMode::Mission => {
                            if lay.wait_btn.contains(pt) {
                                player.wait();
                            } else if lay.close_btn.contains(pt) {
                                if player.fully_revealed() {
                                    close_after = Some(*after); // fully read ⇒ Close really closes
                                } else {
                                    player.inject_close_error(); // refused — splice the red error
                                }
                            }
                        }
                        BriefMode::Memory => {
                            if lay.close_btn.contains(pt) {
                                close_after = Some(*after); // Memory re-read: Close always returns
                            }
                        }
                    }
                }
                // Mouse-wheel scrolls the text; scrolling up releases auto-follow until back at bottom.
                let (_, wheel) = mouse_wheel();
                if wheel != 0.0 {
                    let (_, content_h) = layout_briefing(player, lay.text_x, lay.text_w);
                    let visible_h = lay.text_bottom - lay.text_y;
                    let (cur_scroll, max_scroll) = briefing_scroll(player, content_h, visible_h);
                    player.scroll = (cur_scroll - wheel * BRIEF_SCROLL_SPEED).clamp(0.0, max_scroll);
                    player.follow = player.scroll >= max_scroll - 1.0;
                }
                player.update(dt);
                close_after
            };
            match close_after {
                Some(AfterBriefing::StartLevel(idx)) => app.start_level(idx),
                Some(AfterBriefing::BackToMemory) => {
                    app.state = AppState::Memory { tab: 1, idx: 0, expanded: 0, scroll: 0 }
                }
                None => {}
            }
            false
        }
        AppState::InLevel { .. } => {
            // The InLevel arm borrows `game`/`paused_menu` from `app.state`, but several
            // transitions need to mutate `app` itself (start a level, change state). So we
            // compute a deferred `LevelAction` while holding the borrow, drop it, then apply.
            let n_levels = app.levels.len();
            let action = {
                let AppState::InLevel { game } = &mut app.state else { unreachable!() };

                if game.match_over() {
                    // --- End-of-match handling (Victory/Defeat) ---
                    let winner = game.finished.unwrap_or(Faction::Neutral);
                    let idx = (game.level.id as usize).saturating_sub(1);
                    game.update(dt); // keep easing the camera on the end screen
                    // POST-BATTLE BRIEFING (`--text`): render the level's `_post.brf`
                    // template against the sealed match ONCE — persist only the `#metrics`
                    // stats line (the flag source future logic-based text reads; rendered
                    // text is shown, never stored) — and keep it viewable with `L`.
                    if !game.log_written {
                        game.log_written = true;
                        if app.text {
                            if let Some(t) = game.level.post_log.clone() {
                                let ctx = narrative::LogCtx {
                                    result: if winner == Faction::Player {
                                        "victory".into()
                                    } else {
                                        "defeat".into()
                                    },
                                    ticks: game.world.tick,
                                    lost: game.lost_ships,
                                    killed: game.killed_ships,
                                    ships: game.world.total_ships(Faction::Player) as u64,
                                    notes: app.notes.clone(),
                                };
                                let rendered = narrative::render(&t, &ctx);
                                narrative::append_battle_stats(game.level.id, &ctx);
                                game.post_log_brief =
                                    Some(Briefing::new(&rendered, BriefMode::Memory));
                            }
                        }
                    }
                    if game.show_post_log {
                        // The log popup swallows the end-screen keys until closed.
                        let (mx, my) = mouse_position();
                        let close_click = is_mouse_button_pressed(MouseButton::Left)
                            && briefing_layout().close_btn.contains(vec2(mx, my));
                        if is_key_pressed(KeyCode::Escape)
                            || is_key_pressed(KeyCode::L)
                            || close_click
                        {
                            game.show_post_log = false;
                        }
                        LevelAction::None
                    } else if game.post_log_brief.is_some() && is_key_pressed(KeyCode::L) {
                        game.show_post_log = true;
                        LevelAction::None
                    } else if is_mouse_button_pressed(MouseButton::Left)
                        && fullscreen_btn_at_mouse(true)
                    {
                        toggle_fullscreen();
                        LevelAction::None
                    } else if game.replay.is_some() {
                        // PLAYBACK end overlay: scrub, watch again, or leave.
                        let mut act = LevelAction::None;
                        if is_mouse_button_pressed(MouseButton::Left) {
                            if let Some(t) = timeline_click_tick(game) {
                                game.replay_seek(t);
                            } else {
                                match menu_item_at_mouse(2) {
                                    Some(0) => game.replay_seek(0),
                                    Some(1) => act = LevelAction::ToMenu,
                                    _ => {}
                                }
                            }
                        } else if is_key_pressed(KeyCode::Escape) {
                            act = LevelAction::ToMenu;
                        }
                        act
                    } else if is_key_pressed(KeyCode::Escape) {
                        LevelAction::ToSelect(idx)
                    } else if winner == Faction::Player {
                        // The VICTORY MENU's buttons (drawn by draw_end_banner). The win
                        // itself was recorded by earlier frames' non-advancing WinThen, so
                        // Restart / Main Menu don't lose it.
                        let mut advance = is_key_pressed(KeyCode::Enter) || is_key_pressed(KeyCode::Space);
                        let mut act = None;
                        if is_mouse_button_pressed(MouseButton::Left) {
                            match menu_item_at_mouse(4) {
                                Some(0) => advance = true,
                                Some(1) => {
                                    // The split row: Watch (left half) | Save (right half).
                                    let (x, _, w, _) = menu_item_rect(1);
                                    let (mx, _) = mouse_position();
                                    if mx < x + w * 0.5 {
                                        act = Some(LevelAction::WatchReplay);
                                    } else {
                                        game.save_replay_manual();
                                    }
                                }
                                Some(2) => act = Some(LevelAction::Start(idx)),
                                Some(3) => act = Some(LevelAction::ToMenu),
                                _ => {}
                            }
                        }
                        act.unwrap_or(LevelAction::WinThen { idx, advance })
                    } else {
                        // The DEFEAT/DRAW MENU's buttons (Retry / Fullscreen / Main Menu);
                        // the R key still retries.
                        let mut act = if is_key_pressed(KeyCode::R) {
                            LevelAction::Start(idx)
                        } else {
                            LevelAction::None
                        };
                        if is_mouse_button_pressed(MouseButton::Left) {
                            match menu_item_at_mouse(3) {
                                Some(0) => act = LevelAction::Start(idx),
                                Some(1) => act = LevelAction::WatchReplay,
                                Some(2) => act = LevelAction::ToMenu,
                                _ => {}
                            }
                        }
                        act
                    }
                } else {
                    // --- The PAUSE MENU buttons (owner merge, 2026-07-08: ONE pause — Esc and
                    // P both raise it, the camera stays free underneath). The hit-test runs
                    // BEFORE the normal input path; a button click cannot leak to the board
                    // because the paused gate in `handle_in_level_input` refuses orders.
                    let mut act = LevelAction::None;
                    if game.paused && game.pause_buttons && !game.show_intro {
                        if is_mouse_button_pressed(MouseButton::Left) {
                            let idx = (game.level.id as usize).saturating_sub(1);
                            if pause_cross_at_mouse() {
                                // The ✕: hide the buttons and lift the veil — the player
                                // inspects the frozen level; Esc / P bring the menu back.
                                game.pause_buttons = false;
                            } else {
                                match menu_item_at_mouse(3) {
                                    Some(0) => {
                                        game.paused = false;
                                        game.pause_buttons = false;
                                    }
                                    Some(1) => act = LevelAction::Start(idx),
                                    Some(2) => act = LevelAction::ToMenu,
                                    _ => {}
                                }
                            }
                        }
                    }
                    // Live input + sim step run every frame: the camera is free under the
                    // menu; `Game::update` freezes the sim while paused.
                    handle_in_level_input(game);
                    game.update(dt);
                    act
                }
            };

            // Apply the deferred action now that the `app.state` borrow is gone.
            match action {
                LevelAction::None => {}
                LevelAction::Start(idx) => app.start_level(idx),
                LevelAction::ToSelect(idx) => app.state = AppState::LevelSelect { idx },
                LevelAction::ToMenu => app.state = AppState::MainMenu { idx: 0 },
                LevelAction::WatchReplay => {
                    if let AppState::InLevel { game } = &app.state {
                        let rg = game.make_replay_of_this_match();
                        app.state = AppState::InLevel { game: Box::new(rg) };
                    }
                }
                LevelAction::WinThen { idx, advance } => {
                    app.record_win(idx);
                    if advance {
                        let next = idx + 1;
                        if next < n_levels {
                            app.enter_level(next);
                        } else {
                            app.state = AppState::MainMenu { idx: 0 };
                        }
                    }
                }
            }
            false
        }
    }
}

/// Activate a main-menu item. Returns `true` to quit.
fn activate_main_menu(app: &mut App, item: usize) -> bool {
    // Rows are matched by LABEL — the list shrinks without `--text` (no Memory row).
    match main_menu_items(app.text).get(item).copied() {
        Some("Play") => {
            // Play: enter the highest unlocked level (its briefing plays first if unseen).
            let idx = (0..app.levels.len())
                .rev()
                .find(|&i| app.progress.unlocked.contains(&app.levels[i].id))
                .unwrap_or(0);
            app.enter_level(idx);
            false
        }
        Some("Level Select") => {
            app.state = AppState::LevelSelect { idx: 0 };
            false
        }
        Some("Memory") => {
            // Rescan the replay folders on entry — the page always shows the live truth.
            app.replay_sections = scan_replays(&app.levels);
            app.state = AppState::Memory { tab: 0, idx: 0, expanded: 0, scroll: 0 };
            false
        }
        Some("Settings") => {
            app.state = AppState::Settings { idx: 0, capturing: false };
            false
        }
        Some("Quit") => true,
        _ => false,
    }
}

// =============================================================================================
// In-level input (both layers + zoom + automation)
// =============================================================================================

fn handle_in_level_input(game: &mut Game) {
    // Persist play prefs (speed + send fraction) whenever the player changed them last frame.
    // (The transient P-pause is a separate flag, so it never leaks into persistence.)
    BINDS.with(|b| {
        let mut b = b.borrow_mut();
        let mut dirty = false;
        if b.speed_idx != game.speed_idx {
            b.speed_idx = game.speed_idx;
            dirty = true;
        }
        if b.frac_pct != game.frac_pct {
            b.frac_pct = game.frac_pct;
            dirty = true;
        }
        if dirty {
            b.save();
        }
    });

    // Post-send auto-deselect: the selection armed by a send lets go once its window expires.
    if let Some(t) = game.deselect_at {
        if get_time() >= t {
            clear_selection(game);
        }
    }

    // Dismiss the intro overlay on any meaningful key/click.
    if game.show_intro
        && (is_key_pressed(KeyCode::Enter)
            || is_key_pressed(KeyCode::Space)
            || is_mouse_button_pressed(MouseButton::Left))
    {
        game.show_intro = false;
        game.paused = false; // the briefing held the clock; dismissing it starts play
        // Don't also treat this first click/keypress as an in-game action (a Space dismissal
        // must not immediately snap the speed via the fixed Space key below).
        return;
    }

    // --- Global controls (rebindable — see `ACTIONS` / the Settings screen — except SPACE,
    //     fixed and reserved like Esc) ---
    // SPACE: paused ⇒ just resume; running ⇒ toggle 1x ⇄ the last non-1x stop selected
    // (owner fix: Space on 1x returns to your fast speed instead of doing nothing).
    if is_key_pressed(KeyCode::Space) {
        if game.paused {
            game.paused = false;
            game.pause_buttons = false;
        } else if game.speed_idx == 0 {
            game.speed_idx = game.speed_alt_idx.clamp(1, SPEED_STEPS.len() - 1);
        } else {
            game.speed_alt_idx = game.speed_idx;
            game.speed_idx = 0; // SPEED_STEPS[0] = 1x
        }
    }
    // The overlay pause: sim freezes under a "Game paused" veil, orders are refused, the
    // camera stays free. The speed slider keeps its stop for the resume.
    if act_pressed(Action::PauseToggle) {
        toggle_pause(game);
    }
    if act_pressed(Action::SpeedDown) {
        game.speed_idx = game.speed_idx.saturating_sub(1);
        if game.speed_idx != 0 {
            game.speed_alt_idx = game.speed_idx;
        }
    }
    if act_pressed(Action::SpeedUp) {
        game.speed_idx = (game.speed_idx + 1).min(SPEED_STEPS.len() - 1);
        game.speed_alt_idx = game.speed_idx; // stepping up always lands past 1x
    }
    if act_pressed(Action::Frac25) {
        game.frac_pct = 25;
    }
    if act_pressed(Action::Frac50) {
        game.frac_pct = 50;
    }
    if act_pressed(Action::Frac75) {
        game.frac_pct = 75;
    }
    if act_pressed(Action::Frac100) {
        game.frac_pct = 100;
    }

    // --- Keyboard camera pan (held keys; screen-constant speed at any zoom) ---
    {
        let (mut dx, mut dy) = (0.0f32, 0.0f32);
        if act_down(Action::PanLeft) {
            dx -= 1.0;
        }
        if act_down(Action::PanRight) {
            dx += 1.0;
        }
        if act_down(Action::PanUp) {
            dy += 1.0; // world y grows up
        }
        if act_down(Action::PanDown) {
            dy -= 1.0;
        }
        if dx != 0.0 || dy != 0.0 {
            let layer = game.zoom_layer();
            let scale = game.camera().scale.max(1e-6);
            let step = PAN_SPEED_PX * get_frame_time() / scale;
            game.pan[layer].0 += dx * step;
            game.pan[layer].1 += dy * step;
            clamp_pan(game, layer);
        }
    }

    // Mouse wheel: CONTINUOUS cursor-anchored zoom on the current layer (the same value the
    // right-side slider drives), with the layer transitions at the ends of the range:
    //   - interior, wheel-down at ZOOM_MIN  -> out to the lens (multi-struct maps only);
    //   - lens, wheel-up over a struct      -> into that struct (the old navigation gesture);
    //   - otherwise each notch scales the layer zoom by WHEEL_ZOOM_STEP toward the cursor.
    // Works on single-struct missions too (they lock to the interior; the wheel just zooms).
    {
        let (_wx, wy) = mouse_wheel();
        if wy != 0.0 {
            let up = wy > 0.0;
            match game.view {
                View::Lens if up => {
                    // Prefer entering the hovered/selected structure; with none, zoom the lens in.
                    let (mx, my) = mouse_position();
                    let cam = game.camera();
                    if struct_at_screen(game, &cam, mx, my).or(game.sel_struct).is_some() {
                        zoom_in(game);
                    } else {
                        wheel_zoom(game, WHEEL_ZOOM_STEP);
                    }
                }
                View::Interior(_) if !up && game.zoom[1] <= game.zoom_min() && game.world.structs.len() > 1 => {
                    // Already fully zoomed out: the next notch leaves the structure —
                    // landing at the lens zoom where the struct reads the SAME SIZE it just
                    // did (the continuity handoff), ready to keep shrinking from there.
                    let handoff = lens_match_zoom(game, game.focus);
                    game.view = View::Lens;
                    game.zoom[0] = handoff.max(game.level.zoom_min.unwrap_or(ZOOM_MIN));
                    clear_selection(game);
                }
                _ => {
                    wheel_zoom(game, if up { WHEEL_ZOOM_STEP } else { 1.0 / WHEEL_ZOOM_STEP });
                }
            }
        }
    }

    // --- Right button: a drag past the threshold GRAB-PANS the camera (the world point under
    //     the press follows the cursor); a plain right-click (press + release, no drag) keeps
    //     its old meaning — clear the selection. Resolved on release, like left click-vs-box. ---
    if is_mouse_button_pressed(MouseButton::Right) {
        let layer = game.zoom_layer();
        game.rdrag = Some((mouse_position(), game.pan[layer]));
        game.rdrag_moved = false;
    }
    if let Some(((ax, ay), pan0)) = game.rdrag {
        if is_mouse_button_down(MouseButton::Right) {
            let (mx, my) = mouse_position();
            if !game.rdrag_moved && (mx - ax).hypot(my - ay) > BOX_DRAG_THRESHOLD {
                game.rdrag_moved = true;
            }
            if game.rdrag_moved {
                let layer = game.zoom_layer();
                let scale = game.camera().scale.max(1e-6);
                game.pan[layer].0 = pan0.0 - (mx - ax) / scale;
                game.pan[layer].1 = pan0.1 + (my - ay) / scale; // screen y grows down
                clamp_pan(game, layer);
            }
        } else {
            if !game.rdrag_moved {
                clear_selection(game);
            }
            game.rdrag = None;
            game.rdrag_moved = false;
        }
    }
    // ESC: always the pause menu (owner, 2026-07-08 — no zoom-out-to-lens step and no
    // clear-selection step; right-click clears the selection, the wheel leaves the interior).
    if is_key_pressed(KeyCode::Escape) {
        toggle_pause(game);
    }

    // Topbar pointer controls (speed slider + troop slider) take precedence over board orders
    // and stay live even in demo mode. If they consumed this frame's click, don't also order.
    if handle_topbar_input(game) {
        return;
    }
    // Right-side zoom slider (stays live in demo mode, like the topbar).
    if handle_zoom_slider(game) {
        return;
    }
    // The persistent fullscreen button (bottom-right) — consumed before board input, live
    // even while paused, like the sliders.
    if is_mouse_button_pressed(MouseButton::Left) && fullscreen_btn_at_mouse(true) {
        toggle_fullscreen();
        return;
    }
    // Replay timeline scrub (the bar above the bottom HUD) — playback only.
    if game.replay.is_some() && is_mouse_button_pressed(MouseButton::Left) {
        if let Some(t) = timeline_click_tick(game) {
            game.replay_seek(t);
            return;
        }
    }

    // Pointer orders are disabled while the player seat is AI-driven (demo mode) and in
    // PLAYBACK (the journal is the only order source; the camera stays free).
    if game.player_ai.is_some() || game.replay.is_some() {
        return;
    }
    // NO ORDERS WHILE PAUSED (owner rule): the overlay pause refuses selection and orders
    // alike — everything above (camera pan / zoom / grab-pan, the topbar and zoom sliders,
    // the pause and speed keys) stays live; the board itself is untouchable.
    if game.paused {
        game.drag_start = None;
        game.box_active = false;
        return;
    }

    let (mx, my) = mouse_position();
    let cam = game.camera();

    // Whether we are effectively in the interior (camera mostly zoomed in).
    let in_interior = matches!(game.view, View::Interior(_)) && game.cam_t > INTERIOR_DRAW_THRESHOLD;

    // (Player automation has NO control binding: the mechanic is unimplemented player-side —
    // an empty promise until its redesign ships. The engine plumbing survives behind
    // `automation_available = false`; re-add an Action when the mechanic is real.)

    // A release that happens OUTSIDE the window never reaches us (the OS doesn't capture the
    // mouse for us), leaving a stale drag: the box kept drawing and then silently died on
    // the next press. Owner call (2026-07-10): the moment we notice the button is no longer
    // down with a drag still tracked — and no real release event to consume it — resolve the
    // BOX selection with its final rectangle (the anchor to the last position the window
    // saw, which is exactly the box the player was shown). A stale plain CLICK just
    // dissolves: a click that ended off-window shouldn't order anything.
    if game.drag_start.is_some()
        && !is_mouse_button_down(MouseButton::Left)
        && !is_mouse_button_released(MouseButton::Left)
    {
        let (px, py) = game.drag_start.take().unwrap();
        if game.box_active {
            if in_interior {
                box_select_interior(game, &cam, px, py, mx, my);
            } else {
                box_select_lens(game, &cam, px, py, mx, my);
            }
        }
        game.box_active = false;
    }

    // --- Left button: a drag past the threshold draws a **selection box**; a plain click selects a
    //     source or issues an order (incl. ordering a whole box multi-selection to the clicked
    //     target). Click vs drag is resolved on release so the two never collide. ---
    if is_mouse_button_pressed(MouseButton::Left) {
        game.drag_start = Some((mx, my));
        game.box_active = false;
    }
    if is_mouse_button_down(MouseButton::Left) {
        if let Some((px, py)) = game.drag_start {
            if (mx - px).hypot(my - py) > BOX_DRAG_THRESHOLD {
                game.box_active = true;
            }
        }
    }
    if is_mouse_button_released(MouseButton::Left) {
        // Only resolve a release whose matching PRESS this handler saw (drag_start tracked).
        // A release whose press was consumed elsewhere — the pause menu's Resume, an overlay's
        // Close, the topbar/zoom sliders — must not click through into the board.
        let Some((px, py)) = game.drag_start.take() else {
            game.box_active = false;
            return;
        };
        if game.box_active {
            if in_interior {
                box_select_interior(game, &cam, px, py, mx, my);
            } else {
                box_select_lens(game, &cam, px, py, mx, my);
            }
        } else if in_interior {
            handle_interior_click(game, &cam, mx, my);
        } else {
            handle_lens_click(game, &cam, mx, my);
        }
        game.box_active = false;
    }
}

/// Box-select the focused struct's interior: every **player-commandable** sub (owned, or holding
/// idle player ships) whose screen position falls inside the drag rectangle — **excluding** the
/// struct-storage node. A plain box supersedes the selection; **Ctrl+box is additive** (owner,
/// 2026-07-08): the boxed subs join the selection — unless ALL of them are already in it, then
/// they leave it instead (box-toggle).
fn box_select_interior(game: &mut Game, cam: &Camera, x0: f32, y0: f32, x1: f32, y1: f32) {
    let p = game.focus;
    let (lo_x, hi_x) = (x0.min(x1), x0.max(x1));
    let (lo_y, hi_y) = (y0.min(y1), y0.max(y1));
    let structure = &game.world.structs[p];
    let st = &structure.interior;
    let picked: Vec<usize> = (0..st.subs.len())
        .filter(|&i| !st.is_storage(i)) // never the struct-storage / reserve node
        .filter(|&i| st.subs[i].owner == Faction::Player || st.idle_count_at(i, Faction::Player) > 0)
        .filter(|&i| {
            let (sx, sy) = cam.to_screen(structure.pos.x + st.subs[i].pos.x, structure.pos.y + st.subs[i].pos.y);
            sx >= lo_x && sx <= hi_x && sy >= lo_y && sy <= hi_y
        })
        .collect();
    if ctrl_down() {
        if picked.is_empty() {
            return; // an empty additive box changes nothing
        }
        // Fold a lone single-select into the multi first, so Ctrl+box extends it naturally.
        if let Some(s) = game.sel_sub.take() {
            if !game.sel_subs.contains(&s) {
                game.sel_subs.push(s);
            }
        }
        if picked.iter().all(|i| game.sel_subs.contains(i)) {
            game.sel_subs.retain(|i| !picked.contains(i));
        } else {
            for i in picked {
                if !game.sel_subs.contains(&i) {
                    game.sel_subs.push(i);
                }
            }
        }
        game.deselect_at = None;
        return;
    }
    game.sel_sub = None;
    game.sel_subs = picked;
    game.deselect_at = None; // a fresh selection is deliberate — no pending auto-clear
}

/// Box-select on the Layer-2 lens: every **player-commandable** struct (owned, or holding player
/// subs) whose node centre falls inside the drag rectangle. Supersedes the single-select.
fn box_select_lens(game: &mut Game, cam: &Camera, x0: f32, y0: f32, x1: f32, y1: f32) {
    let (lo_x, hi_x) = (x0.min(x1), x0.max(x1));
    let (lo_y, hi_y) = (y0.min(y1), y0.max(y1));
    let picked: Vec<StructId> = (0..game.world.structs.len())
        .filter(|&i| {
            let agg = game.world.struct_aggregate(i);
            matches!(agg.owner, StructOwner::Owned(Faction::Player)) || agg.player_subs > 0
        })
        .filter(|&i| {
            let (sx, sy) = cam.to_screen(game.world.structs[i].pos.x, game.world.structs[i].pos.y);
            sx >= lo_x && sx <= hi_x && sy >= lo_y && sy <= hi_y
        })
        .collect();
    if ctrl_down() {
        // Same additive/box-toggle rule as the interior (owner, 2026-07-08).
        if picked.is_empty() {
            return;
        }
        if let Some(s) = game.sel_struct.take() {
            if !game.sel_structs.contains(&s) {
                game.sel_structs.push(s);
            }
        }
        if picked.iter().all(|i| game.sel_structs.contains(i)) {
            game.sel_structs.retain(|i| !picked.contains(i));
        } else {
            for i in picked {
                if !game.sel_structs.contains(&i) {
                    game.sel_structs.push(i);
                }
            }
        }
        game.deselect_at = None;
        return;
    }
    game.sel_struct = None;
    game.sel_structs = picked;
    game.deselect_at = None; // a fresh selection is deliberate — no pending auto-clear
}

/// Zoom into a structure: prefer the hovered structure, else the current selection, else the focus.
fn zoom_in(game: &mut Game) {
    let (mx, my) = mouse_position();
    let cam = game.camera();
    let target = struct_at_screen(game, &cam, mx, my).or(game.sel_struct);
    if let Some(p) = target {
        if game.focus != p {
            game.pan[1] = (0.0, 0.0); // a pan framed for another struct means nothing here
        }
        game.focus = p;
        game.view = View::Interior(p);
        // CONTINUITY (owner, 2026-07-08): enter at the struct's own out-zoom floor — the
        // exact size the lens showed it at its ceiling — then zoom in from there.
        game.zoom[1] = game.zoom_min();
        clear_selection(game);
    } else if let View::Lens = game.view {
        // No struct under the cursor: focus the current focus structure.
        game.view = View::Interior(game.focus);
        game.zoom[1] = game.zoom_min();
    }
}

/// Scale the current layer's zoom by `factor` (clamped to the effective bounds — per-struct
/// overrides included on the interior layer), **anchored on
/// the mouse cursor**: the world point under the cursor stays under the cursor — the pan absorbs
/// the correction. (Computed against the target cameras; during a lens⇄interior ease the anchor
/// is approximate for a few frames and settles exactly.)
fn wheel_zoom(game: &mut Game, factor: f32) {
    let layer = game.zoom_layer();
    let (mx, my) = mouse_position();
    let before = game.camera();
    let (wx0, wy0) = before.to_world(mx, my);
    game.zoom[layer] = (game.zoom[layer] * factor).clamp(game.zoom_min(), game.zoom_max());
    let after = game.camera();
    let (wx1, wy1) = after.to_world(mx, my);
    game.pan[layer].0 += wx0 - wx1;
    game.pan[layer].1 += wy0 - wy1;
    clamp_pan(game, layer);
}

/// Keep layer `layer`'s pan within reach of the board — bounded by the **content extent**
/// (every struct node on the lens; every sub INCLUDING the reserve ring on the interior), not
/// by the fitted frame: the interior fit deliberately frames only the tactical cluster, and
/// clamping to it made the camera refuse to zoom in near the map edges / the reserve ring.
/// The centre may wander up to half a view past the farthest content in each axis.
fn clamp_pan(game: &mut Game, layer: usize) {
    let fit = if layer == 0 {
        lens_camera(&game.world, HUD_TOP_H, HUD_BOTTOM_H)
    } else {
        interior_camera(&game.world, game.focus, HUD_TOP_H, HUD_BOTTOM_H)
    };
    let (mut ext_x, mut ext_y) = (0.0f32, 0.0f32);
    if layer == 0 {
        for i in 0..game.world.structs.len() {
            let s = &game.world.structs[i];
            let pad = struct_world_radius(s);
            ext_x = ext_x.max((s.pos.x - fit.cx).abs() + pad);
            ext_y = ext_y.max((s.pos.y - fit.cy).abs() + pad);
        }
    } else if let Some(stc) = game.world.structs.get(game.focus) {
        for sub in &stc.interior.subs {
            // Orbit-envelope extents (like the camera fit): the pan bound must not breathe
            // with the turning ring either.
            let (wx, wy, pad) = match sub.orbit {
                Some(o) => (stc.pos.x + o.center.x, stc.pos.y + o.center.y, o.radius + sub.radius),
                None => (stc.pos.x + sub.pos.x, stc.pos.y + sub.pos.y, sub.radius),
            };
            ext_x = ext_x.max((wx - fit.cx).abs() + pad);
            ext_y = ext_y.max((wy - fit.cy).abs() + pad);
        }
    }
    let z = game.zoom[layer.min(1)].max(game.zoom_min());
    let half_w = screen_width() * 0.5 / (fit.scale * z);
    let half_h = (screen_height() - HUD_TOP_H - HUD_BOTTOM_H) * 0.5 / (fit.scale * z);
    game.pan[layer].0 = game.pan[layer].0.clamp(-(ext_x + half_w), ext_x + half_w);
    game.pan[layer].1 = game.pan[layer].1.clamp(-(ext_y + half_h), ext_y + half_h);
}

fn clear_selection(game: &mut Game) {
    game.sel_struct = None;
    game.sel_sub = None;
    game.sel_structs.clear();
    game.sel_subs.clear();
    game.drag_start = None;
    game.box_active = false;
    game.deselect_at = None;
}

/// Arm the post-send auto-deselect window (each send re-arms it, so a burst of repeat orders
/// keeps the selection alive until 1s after the LAST one).
fn arm_deselect(game: &mut Game) {
    game.deselect_at = Some(get_time() + DESELECT_AFTER_SEND_S);
}

/// Esc / the rebindable `P` — the ONE pause (owner merge, 2026-07-08). Not paused → pause
/// with the menu panel up. Menu up → resume. Paused with the panel hidden (the ✕ — the
/// player is inspecting the frozen level) → bring the panel back. Throughout: the sim is
/// frozen, orders are refused, the camera stays free, the speed slider keeps its stop.
fn toggle_pause(game: &mut Game) {
    if game.paused && game.pause_buttons {
        game.paused = false;
        game.pause_buttons = false;
    } else {
        game.paused = true;
        game.pause_buttons = true;
        // A drag in progress is abandoned, not resumed: otherwise the press tracked before
        // the pause would resolve as a click/box on the release after Resume.
        game.drag_start = None;
        game.box_active = false;
    }
}

/// Is either Ctrl held? (Ctrl+left-click = add-to-selection.)
fn ctrl_down() -> bool {
    is_key_down(KeyCode::LeftControl) || is_key_down(KeyCode::RightControl)
}

/// Lens-layer **click** handling (called on release when the gesture was a click, not a box-drag):
/// order a box multi-selection to the clicked target, else select an owned struct as the source,
/// issue a FleetOrder to a lane-connected target, or zoom into the re-clicked source.
fn handle_lens_click(game: &mut Game, cam: &Camera, mx: f32, my: f32) {
    let frac = game.send_fraction();
    let Some(structure) = struct_at_screen(game, cam, mx, my) else {
        clear_selection(game);
        return;
    };
    let owner = game.world.struct_aggregate(structure).owner;
    let mine = matches!(owner, StructOwner::Owned(Faction::Player))
        || game.world.struct_aggregate(structure).player_subs > 0;
    // Ctrl+click: ADD the struct to the selection (toggle if already in) instead of ordering.
    if ctrl_down() {
        if mine {
            if let Some(src) = game.sel_struct.take() {
                if !game.sel_structs.contains(&src) {
                    game.sel_structs.push(src);
                }
            }
            if let Some(at) = game.sel_structs.iter().position(|&s| s == structure) {
                game.sel_structs.remove(at);
            } else {
                game.sel_structs.push(structure);
            }
            game.deselect_at = None;
        }
        return; // ctrl+click on a non-ours structure: keep the selection untouched
    }
    // A box multi-selection is active: this click is the TARGET — order from every selected source
    // that can reach it (the target may be one of the selected); the selection then survives the
    // auto-deselect window for repeat orders.
    if !game.sel_structs.is_empty() {
        let srcs = game.sel_structs.clone();
        for src in srcs {
            if src != structure && game.world.are_connected(src, structure) {
                game.world.issue_fleet_order_fraction(src, structure, frac, Faction::Player, &game.wp);
            }
        }
        arm_deselect(game);
        return;
    }
    match game.sel_struct {
        // Re-click the selected source ⇒ zoom into it.
        Some(src) if src == structure => {
            game.focus = structure;
            game.view = View::Interior(structure);
            game.sel_struct = None;
            game.deselect_at = None;
        }
        // Connected target ⇒ launch a fleet; the source stays selected for the deselect window.
        Some(src) if game.world.are_connected(src, structure) => {
            game.world.issue_fleet_order_fraction(src, structure, frac, Faction::Player, &game.wp);
            arm_deselect(game);
        }
        // A source is selected but this target is not connected: reselect if ours, else clear.
        Some(_) => {
            game.sel_struct = if mine { Some(structure) } else { None };
            game.deselect_at = None;
        }
        None if mine => {
            game.sel_struct = Some(structure);
            game.deselect_at = None;
        }
        // Non-owned structure, nothing selected: zoom in to inspect it.
        None => {
            game.focus = structure;
            game.view = View::Interior(structure);
        }
    }
}

/// Interior-layer **click** handling (called on release when the gesture was a click): order a box
/// multi-selection to the clicked target sub, else select a commandable sub as the source or issue a
/// MoveOrder to another sub of the focused structure.
fn handle_interior_click(game: &mut Game, cam: &Camera, mx: f32, my: f32) {
    let p = game.focus;
    let frac = game.send_fraction();
    let Some(sub) = sub_at_screen(game, p, cam, mx, my) else {
        game.sel_sub = None;
        game.sel_subs.clear();
        game.deselect_at = None;
        return;
    };
    // You can command a sub if you own it OR have idle ships sitting there (e.g. ships capturing a
    // neutral/enemy sub, or fighting on it). Transiting / undocking ships aren't idle, so they're
    // never grabbed — "always orderable except in transit".
    let st = &game.world.structs[p].interior;
    let can_source = st.subs[sub].owner == Faction::Player || st.idle_count_at(sub, Faction::Player) > 0;
    // Ctrl+click: ADD the sub to the selection (toggle if already in) instead of ordering.
    if ctrl_down() {
        if can_source {
            if let Some(src) = game.sel_sub.take() {
                if !game.sel_subs.contains(&src) {
                    game.sel_subs.push(src);
                }
            }
            if let Some(at) = game.sel_subs.iter().position(|&s| s == sub) {
                game.sel_subs.remove(at);
            } else {
                game.sel_subs.push(sub);
            }
            game.deselect_at = None;
        }
        return; // ctrl+click on a non-commandable sub: keep the selection untouched
    }
    // A box multi-selection is active: this click is the TARGET — order from every selected source
    // (the target may be one of the selected); the selection then survives the auto-deselect
    // window for repeat orders.
    if !game.sel_subs.is_empty() {
        let srcs = game.sel_subs.clone();
        for src in srcs {
            if src != sub {
                game.world.structs[p].interior.issue_order_fraction(src, sub, frac, Faction::Player);
            }
        }
        arm_deselect(game);
        return;
    }
    match game.sel_sub {
        // A source is already selected: this click orders to `sub` (any target sub); the source
        // stays selected for the auto-deselect window (rapid repeat sends).
        Some(src) if src != sub => {
            game.world.structs[p].interior.issue_order_fraction(src, sub, frac, Faction::Player);
            arm_deselect(game);
        }
        // Otherwise select `sub` as the source if we can command ships there.
        _ => {
            game.sel_sub = if can_source { Some(sub) } else { None };
            game.deselect_at = None;
        }
    }
}

/// Structure under a screen point (within its drawn node radius). Nearest centre on overlap.
fn struct_at_screen(game: &Game, cam: &Camera, mx: f32, my: f32) -> Option<StructId> {
    let mut best: Option<(StructId, f32)> = None;
    for i in 0..game.world.structs.len() {
        let p = &game.world.structs[i];
        let (sx, sy) = cam.to_screen(p.pos.x, p.pos.y);
        let r = node_screen_radius(game, i, cam).max(12.0) + 5.0;
        let d2 = (sx - mx) * (sx - mx) + (sy - my) * (sy - my);
        if d2 <= r * r {
            match best {
                Some((_, bd)) if bd <= d2 => {}
                _ => best = Some((i, d2)),
            }
        }
    }
    best.map(|(i, _)| i)
}

/// Sub-structure of struct `p` under a screen point. Normal subs snap generously — a click
/// within **1.5× the sub's radius** counts (owner QoL: a near-miss of a small sub must not
/// fall through to the reserve, whose map-sized circle otherwise catches every stray click) —
/// and always beat the storage node, which keeps its exact radius and only claims clicks no
/// real sub wants.
fn sub_at_screen(game: &Game, p: StructId, cam: &Camera, mx: f32, my: f32) -> Option<usize> {
    let structure = &game.world.structs[p];
    let mut best: Option<(usize, f32)> = None;
    let mut storage_hit: Option<usize> = None;
    for (i, s) in structure.interior.subs.iter().enumerate() {
        let (sx, sy) = cam.to_screen(structure.pos.x + s.pos.x, structure.pos.y + s.pos.y);
        let d2 = (sx - mx) * (sx - mx) + (sy - my) * (sy - my);
        if structure.interior.is_storage(i) {
            let r = cam.len(s.radius).max(10.0);
            if d2 <= r * r {
                storage_hit = Some(i);
            }
            continue;
        }
        let r = cam.len(s.radius).max(10.0) * 1.5;
        if d2 <= r * r {
            match best {
                Some((_, bd)) if bd <= d2 => {}
                _ => best = Some((i, d2)),
            }
        }
    }
    best.map(|(i, _)| i).or(storage_hit)
}

// =============================================================================================
// Rendering — top-level dispatch
// =============================================================================================

const HUD_TOP_H: f32 = 96.0;
const HUD_BOTTOM_H: f32 = 26.0;

// =============================================================================================
// UI scale clamp — tiny / oddly-shaped windows (owner ask, 2026-07-10, web branch)
// =============================================================================================

/// The window size below which layouts stop adapting and the whole frame starts SCALING:
/// everything renders on a logical canvas at least this large, downscaled uniformly to fit
/// the window. Windows at least this big in both axes render 1:1, exactly as before; in a
/// tiny or oddly-shaped window the tighter axis dictates one uniform shrink (the owner's
/// "just clamp" rule — no letterboxing, the wider axis simply gets more logical room).
const UI_MIN_W: f32 = 900.0;
const UI_MIN_H: f32 = 600.0;

thread_local! {
    /// The Settings page's free-integer entry buffer (the auto-replay cap row).
    static SETTINGS_ENTRY: std::cell::RefCell<String> = const { std::cell::RefCell::new(String::new()) };
}

thread_local! {
    /// Whether WE believe the window is fullscreen (macroquad has a setter but no getter).
    /// Authoritative on desktop (only our button changes it); on web the browser can exit
    /// fullscreen behind our back (Esc), in which case the next toggle is a visual no-op
    /// and the one after re-enters — self-correcting, never stuck.
    static FULLSCREEN: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Flip fullscreen (the Settings row and the pause-menu button both land here). On web this
/// reaches `canvas.requestFullscreen()` via miniquad — browsers require it to follow a user
/// gesture, which both callers are (a click / key press).
fn toggle_fullscreen() {
    FULLSCREEN.with(|f| {
        let on = !f.get();
        f.set(on);
        macroquad::window::set_fullscreen(on);
    });
}

/// This frame's uniform UI downscale factor, in `(0, 1]`.
fn ui_scale() -> f32 {
    let w = macroquad::window::screen_width();
    let h = macroquad::window::screen_height();
    (w / UI_MIN_W).min(h / UI_MIN_H).min(1.0).max(0.05)
}

/// LOGICAL screen width — deliberately SHADOWS macroquad's glob-imported physical one, so
/// every layout in this file lays out on the scaled canvas untouched. Rendering maps the
/// logical canvas onto the window each frame ([`apply_ui_camera`]); the mouse maps back
/// (the [`mouse_position`] shadow). At scale 1 all three are identical to macroquad's.
fn screen_width() -> f32 {
    macroquad::window::screen_width() / ui_scale()
}
/// LOGICAL screen height (see [`screen_width`]).
fn screen_height() -> f32 {
    macroquad::window::screen_height() / ui_scale()
}
/// Mouse position in LOGICAL px — the same space the layouts and hit-tests live in
/// (shadows macroquad's, see [`screen_width`]).
fn mouse_position() -> (f32, f32) {
    let s = ui_scale();
    let (x, y) = macroquad::input::mouse_position();
    (x / s, y / s)
}

/// Point the frame at the logical canvas: identity at scale 1 (the plain screen camera),
/// otherwise a camera rendering logical `(0,0)..(LW,LH)` across the whole window. Called at
/// the top of every drawn frame.
fn apply_ui_camera() {
    let s = ui_scale();
    if s >= 1.0 {
        set_default_camera();
        return;
    }
    let (lw, lh) = (screen_width(), screen_height());
    set_camera(&Camera2D {
        // Positive y zoom: macroquad's screen pass already flips the projection for a
        // custom camera, so this maps logical y straight down like the default camera
        // (a negative y here renders the whole frame upside-down — shot-verified).
        zoom: vec2(2.0 / lw, 2.0 / lh),
        target: vec2(lw * 0.5, lh * 0.5),
        ..Default::default()
    });
}

fn app_draw(app: &App) {
    apply_ui_camera();
    clear_background(BG);
    match &app.state {
        AppState::MainMenu { idx } => draw_main_menu(*idx, app.text),
        AppState::LevelSelect { idx } => draw_level_select(app, *idx),
        AppState::Memory { tab, idx, expanded, scroll } => {
            draw_memory(app, *tab, *idx, *expanded, *scroll)
        }
        AppState::Settings { idx, capturing } => draw_settings(*idx, *capturing),
        AppState::Briefing { player, .. } => draw_briefing(player),
        AppState::InLevel { game } => {
            draw_in_level(game);
            if app.text {
                draw_notes_icon();
            }
            if app.notes_open {
                draw_notes(&app.notes);
            }
        }
    }
    draw_perf_overlay();
}

/// Frame-timing overlay (toggle **F3**): FPS + per-phase milliseconds and the on-screen ship count.
/// EMA-smoothed; purely diagnostic, for future optimization.
fn draw_perf_overlay() {
    let p = PERF.with(|p| *p.borrow());
    if !p.show {
        return;
    }
    let lines = [
        format!("FPS {}", get_fps()),
        format!("update {:>6.2} ms", p.update_ms),
        format!("  kill-fx {:>6.2} ms", p.killfx_ms),
        format!("draw   {:>6.2} ms", p.draw_ms),
        format!("  ships {:>6.2} ms  ({} on-screen)", p.ships_ms, p.ships_drawn),
    ];
    let x = 12.0;
    let mut y = HUD_TOP_H + 22.0;
    draw_rectangle(x - 6.0, y - 17.0, 270.0, lines.len() as f32 * 18.0 + 10.0, Color::new(0.0, 0.0, 0.0, 0.6));
    for l in &lines {
        draw_text(l, x, y, 18.0, Color::new(0.60, 1.0, 0.72, 1.0));
        y += 18.0;
    }
}

// =============================================================================================
// Menus
// =============================================================================================

// Menu items are laid out relative to `screen_height()` (which is *logical* px — under high-DPI
// it differs from the physical framebuffer) so they never collide with the title above or the
// help line below at any window size / DPI. A block of 3 items is centred a bit below middle.
const MENU_ITEM_W: f32 = 360.0;

/// Pitch between menu items, proportional to the window with sane bounds.
fn menu_pitch() -> f32 {
    (screen_height() * 0.09).clamp(44.0, 64.0)
}
fn menu_item_h() -> f32 {
    menu_pitch() - 10.0
}
/// Y (top of the band) where the first menu item starts: centre the full item block around
/// ~55% height (no tagline above, no help footer below — the buttons own the space), never
/// rising into the title block.
fn menu_first_y() -> f32 {
    let block = menu_pitch() * 5.0; // the full-menu block height (stable layout either way)
    (screen_height() * 0.55 - block * 0.5).max(screen_height() * 0.30)
}

fn menu_item_rect(i: usize) -> (f32, f32, f32, f32) {
    let cx = screen_width() * 0.5;
    let pitch = menu_pitch();
    let h = menu_item_h();
    let y = menu_first_y() + i as f32 * pitch;
    (cx - MENU_ITEM_W * 0.5, y, MENU_ITEM_W, h)
}

fn menu_item_at_mouse(n: usize) -> Option<usize> {
    let (mx, my) = mouse_position();
    for i in 0..n {
        let (x, y, w, h) = menu_item_rect(i);
        if mx >= x && mx <= x + w && my >= y && my <= y + h {
            return Some(i);
        }
    }
    None
}

fn draw_centered(text: &str, y: f32, size: u16, col: Color) {
    let d = measure_text(text, None, size, 1.0);
    draw_text(text, screen_width() * 0.5 - d.width * 0.5, y, size as f32, col);
}

fn draw_main_menu(idx: usize, text: bool) {
    let sh = screen_height();
    // Title block, proportional so it always sits above the menu items.
    draw_centered("MACHINE INTELLIGENCE", sh * 0.20, 60, ACCENT);
    draw_centered("a cell-game RTS", sh * 0.20 + 40.0, 26, HUD_MUTED);

    for (i, item) in main_menu_items(text).iter().enumerate() {
        let (x, y, w, h) = menu_item_rect(i);
        let sel = i == idx;
        let bg = if sel { Color::new(0.12, 0.16, 0.22, 0.95) } else { Color::new(0.07, 0.09, 0.13, 0.9) };
        draw_rectangle(x, y, w, h, bg);
        draw_rectangle_lines(x, y, w, h, 2.0, if sel { PLAYER } else { EDGE_COL });
        let col = if sel { HUD_TEXT } else { HUD_MUTED };
        draw_centered(item, y + h * 0.66, 28, col);
    }
    draw_fullscreen_btn(false);
}

/// Level-select list geometry, fitted to the window height so all 10 rows fit between the header
/// and the bottom help line at any size. Returns the top-left + size of row `i`'s rectangle.
const SEL_ROW_W: f32 = 820.0;
/// Top of the first row's band (below the header).
const SEL_TOP: f32 = 140.0;
/// Reserved space at the bottom (help line).
const SEL_BOTTOM_RESERVE: f32 = 56.0;
/// Number of campaign levels the list lays out for (fixed at 10).
const SEL_ROWS: usize = 10;

/// The row pitch that fits [`SEL_ROWS`] rows into the available band.
fn sel_pitch() -> f32 {
    let avail = (screen_height() - SEL_TOP - SEL_BOTTOM_RESERVE).max(1.0);
    (avail / SEL_ROWS as f32).clamp(20.0, 60.0)
}

fn level_row_rect(i: usize) -> (f32, f32, f32, f32) {
    let cx = screen_width() * 0.5;
    let pitch = sel_pitch();
    let h = pitch - 8.0;
    let y = SEL_TOP + i as f32 * pitch;
    (cx - SEL_ROW_W * 0.5, y, SEL_ROW_W, h)
}

fn level_row_at_mouse(n: usize) -> Option<usize> {
    let (mx, my) = mouse_position();
    for i in 0..n {
        let (x, y, w, h) = level_row_rect(i);
        if mx >= x && mx <= x + w && my >= y && my <= y + h {
            return Some(i);
        }
    }
    None
}

// ---- Settings screen (control rebinding) ----

/// Settings-list row geometry: like the level select, fitted so `n` rows sit between the header
/// and the bottom help band.
fn settings_row_rect(i: usize, n: usize) -> (f32, f32, f32, f32) {
    let cx = screen_width() * 0.5;
    let avail = (screen_height() - SEL_TOP - SEL_BOTTOM_RESERVE).max(1.0);
    let pitch = (avail / n as f32).clamp(20.0, 48.0);
    let h = pitch - 6.0;
    let y = SEL_TOP + i as f32 * pitch;
    (cx - SEL_ROW_W * 0.5, y, SEL_ROW_W, h)
}

fn settings_row_at_mouse(n: usize) -> Option<usize> {
    let (mx, my) = mouse_position();
    for i in 0..n {
        let (x, y, w, h) = settings_row_rect(i, n);
        if mx >= x && mx <= x + w && my >= y && my <= y + h {
            return Some(i);
        }
    }
    None
}

/// The Settings page: one row per rebindable action (label left, bound key right), a trailing
/// "Reset to defaults" row, and a capture state ("press a key...") on the row being rebound.
fn draw_settings(idx: usize, capturing: bool) {
    draw_centered("SETTINGS - CONTROLS", 96.0, 44, ACCENT);
    let nrows = ACTIONS.len() + 3;
    BINDS.with(|b| {
        let b = b.borrow();
        for i in 0..ACTIONS.len() {
            let (x, y, w, h) = settings_row_rect(i, nrows);
            let sel = i == idx;
            let bg = if sel { Color::new(0.12, 0.16, 0.22, 0.95) } else { Color::new(0.07, 0.09, 0.13, 0.85) };
            draw_rectangle(x, y, w, h, bg);
            draw_rectangle_lines(x, y, w, h, 2.0, if sel { PLAYER } else { EDGE_COL });
            let baseline = y + h * 0.5 + 8.0;
            let col = if sel { HUD_TEXT } else { HUD_MUTED };
            draw_text(ACTIONS[i].2, x + 18.0, baseline, 22.0, col);
            let key = if capturing && sel { "press a key... (Esc cancels)".to_string() } else { key_name(b.keys[i]).to_string() };
            let kc = if capturing && sel { ACCENT } else { col };
            let td = measure_text(&key, None, 22, 1.0);
            draw_text(&key, x + w - td.width - 18.0, baseline, 22.0, kc);
        }
    });
    // Auto-replay retention row (owner, 2026-07-10): free integer, -1 = infinite.
    {
        let i = ACTIONS.len();
        let (x, y, w, h) = settings_row_rect(i, nrows);
        let sel = i == idx;
        let bg = if sel { Color::new(0.12, 0.16, 0.22, 0.95) } else { Color::new(0.07, 0.09, 0.13, 0.85) };
        draw_rectangle(x, y, w, h, bg);
        draw_rectangle_lines(x, y, w, h, 2.0, if sel { PLAYER } else { EDGE_COL });
        let baseline = y + h * 0.5 + 8.0;
        let col = if sel { HUD_TEXT } else { HUD_MUTED };
        draw_text("Auto-replays kept per level", x + 18.0, baseline, 22.0, col);
        let value = if capturing && sel {
            format!("{}_ (Enter commits, -1 = infinite)", SETTINGS_ENTRY.with(|e| e.borrow().clone()))
        } else {
            let v = BINDS.with(|b| b.borrow().replay_cap);
            if v < 0 { "infinite".to_string() } else { v.to_string() }
        };
        let kc = if capturing && sel { ACCENT } else { col };
        let td = measure_text(&value, None, 22, 1.0);
        draw_text(&value, x + w - td.width - 18.0, baseline, 22.0, kc);
    }
    // Fullscreen toggle row.
    {
        let i = ACTIONS.len() + 1;
        let (x, y, w, h) = settings_row_rect(i, nrows);
        let sel = i == idx;
        let bg = if sel { Color::new(0.12, 0.16, 0.22, 0.95) } else { Color::new(0.07, 0.09, 0.13, 0.85) };
        draw_rectangle(x, y, w, h, bg);
        draw_rectangle_lines(x, y, w, h, 2.0, if sel { PLAYER } else { EDGE_COL });
        let baseline = y + h * 0.5 + 8.0;
        let col = if sel { HUD_TEXT } else { HUD_MUTED };
        draw_text("Toggle fullscreen", x + 18.0, baseline, 22.0, col);
        let state = FULLSCREEN.with(|f| if f.get() { "on" } else { "off" });
        let td = measure_text(state, None, 22, 1.0);
        draw_text(state, x + w - td.width - 18.0, baseline, 22.0, col);
    }
    // Reset row.
    {
        let i = ACTIONS.len() + 2;
        let (x, y, w, h) = settings_row_rect(i, nrows);
        let sel = i == idx;
        let bg = if sel { Color::new(0.16, 0.12, 0.12, 0.95) } else { Color::new(0.10, 0.07, 0.08, 0.85) };
        draw_rectangle(x, y, w, h, bg);
        draw_rectangle_lines(x, y, w, h, 2.0, if sel { PLAYER } else { EDGE_COL });
        draw_text("Reset to defaults", x + 18.0, y + h * 0.5 + 8.0, 22.0, if sel { HUD_TEXT } else { HUD_MUTED });
    }
    draw_centered(
        "Click / Enter: rebind (a taken key swaps)  |  saved to mi_controls.cfg  |  Esc: back",
        screen_height() - 22.0,
        18,
        HUD_MUTED,
    );
}

fn draw_level_select(app: &App, idx: usize) {
    draw_centered("LEVEL SELECT", 96.0, 44, ACCENT);

    for (i, lvl) in app.levels.iter().enumerate() {
        let (x, y, w, h) = level_row_rect(i);
        let unlocked = app.progress.unlocked.contains(&lvl.id);
        let completed = app.progress.completed.contains(&lvl.id);
        let sel = i == idx;
        let bg = if sel { Color::new(0.12, 0.16, 0.22, 0.95) } else { Color::new(0.07, 0.09, 0.13, 0.85) };
        draw_rectangle(x, y, w, h, bg);
        draw_rectangle_lines(x, y, w, h, 2.0, if sel { PLAYER } else { EDGE_COL });

        // Vertically-centred baseline inside the row.
        let baseline = y + h * 0.5 + 9.0;
        let num = format!("{:>2}", lvl.id);
        let title = if unlocked {
            format!("{}   {}", num, lvl.title)
        } else {
            format!("{}   [locked]", num)
        };
        // Completion is a COLOR, not a label (owner, 2026-07-08): an unlocked-but-uncleared
        // level reads in a slightly grayer blue; clearing it brightens the row to normal.
        let col = if !unlocked {
            Color::new(0.40, 0.42, 0.46, 1.0)
        } else if !completed {
            if sel {
                Color::new(0.58, 0.70, 0.86, 1.0)
            } else {
                Color::new(0.46, 0.56, 0.70, 1.0)
            }
        } else if sel {
            HUD_TEXT
        } else {
            HUD_MUTED
        };
        draw_text(&title, x + 18.0, baseline, 24.0, col);

        // Enemy tag on the right — visible for EVERY mission (owner, 2026-07-07: enemy AI
        // visibility on the select menu; it was previously revealed only after the briefing).
        // A multi-enemy mission lists every seat (e.g. "Simple, Simple").
        let tag = lvl.enemies.iter().map(|r| r.name()).collect::<Vec<_>>().join(", ");
        let td = measure_text(&tag, None, 18, 1.0);
        draw_text(&tag, x + w - td.width - 16.0, baseline, 18.0, HUD_MUTED);
    }

    draw_centered(
        "Up/Down or mouse  |  Enter: play an unlocked level  |  Esc: back",
        screen_height() - 22.0,
        18,
        HUD_MUTED,
    );
}

/// The Memory page: a list of mission briefings the player has received (re-readable, no delay).
fn draw_memory(app: &App, tab: usize, idx: usize, expanded: u32, scroll: usize) {
    draw_centered("MEMORY", 96.0, 44, ACCENT);
    // Tab chips: Replays (default) | Memories (text layer only).
    let tabs: &[&str] = if app.text { &["Replays", "Memories"] } else { &["Replays"] };
    for (t, label) in tabs.iter().enumerate() {
        let r = memory_tab_rect(t);
        let active = t == tab;
        let bg = if active { Color::new(0.12, 0.16, 0.22, 0.95) } else { Color::new(0.07, 0.09, 0.13, 0.7) };
        draw_rectangle(r.x, r.y, r.w, r.h, bg);
        draw_rectangle_lines(r.x, r.y, r.w, r.h, 2.0, if active { PLAYER } else { EDGE_COL });
        let d = measure_text(label, None, 22, 1.0);
        draw_text(label, r.x + r.w * 0.5 - d.width * 0.5, r.y + r.h * 0.5 + 8.0, 22.0, if active { HUD_TEXT } else { HUD_MUTED });
    }

    if tab == 0 {
        // ---- The replay browser. ----
        let rows = browser_rows(&app.replay_sections, expanded);
        if rows.is_empty() {
            draw_centered("No replays yet — finish a mission.", screen_height() * 0.45, 24, HUD_MUTED);
        }
        let visible = memory_visible_rows();
        for vis_i in 0..visible.min(rows.len().saturating_sub(scroll)) {
            let row = rows[scroll + vis_i];
            let (x, y, w, h) = memory_row_rect(vis_i);
            let sel = scroll + vis_i == idx;
            match row {
                BrowserRow::Header(sec) => {
                    let section = &app.replay_sections[sec];
                    let open = expanded & (1u32 << sec) != 0;
                    let bg = if sel { Color::new(0.14, 0.17, 0.24, 0.95) } else { Color::new(0.09, 0.11, 0.16, 0.9) };
                    draw_rectangle(x, y, w, h, bg);
                    draw_rectangle_lines(x, y, w, h, 2.0, if sel { PLAYER } else { HUD_MUTED });
                    let arrow = if open { "v" } else { ">" };
                    let label = format!("{arrow}  {}", section.label);
                    let baseline = y + h * 0.5 + 8.0;
                    draw_text(&label, x + 14.0, baseline, 22.0, if sel { HUD_TEXT } else { HUD_MUTED });
                    let count = format!("({})", section.entries.len());
                    let d = measure_text(&count, None, 20, 1.0);
                    draw_text(&count, x + w - d.width - 14.0, baseline, 20.0, HUD_MUTED);
                }
                BrowserRow::Entry(sec, e) => {
                    let entry = &app.replay_sections[sec].entries[e];
                    let bg = if sel { Color::new(0.12, 0.16, 0.22, 0.95) } else { Color::new(0.07, 0.09, 0.13, 0.85) };
                    draw_rectangle(x, y, w, h, bg);
                    draw_rectangle_lines(x, y, w, h, 1.5, if sel { PLAYER } else { EDGE_COL });
                    let baseline = y + h * 0.5 + 8.0;
                    draw_text(&entry.name, x + 44.0, baseline, 20.0, if sel { HUD_TEXT } else { HUD_MUTED });
                    // ASCII "?": the bitmap font has no em-dash (it rendered as a tofu box).
                    let dur = entry.end_tick.map_or("?".to_string(), fmt_duration_60tps);
                    let d = measure_text(&dur, None, 20, 1.0);
                    draw_text(&dur, x + w - d.width - 14.0, baseline, 20.0, if sel { HUD_TEXT } else { HUD_MUTED });
                }
            }
        }
        draw_centered(
            "Click: expand / watch  |  Wheel: scroll  |  Esc: back",
            screen_height() - 22.0,
            18,
            HUD_MUTED,
        );
    } else {
        // ---- Text memories (the original briefing list). ----
        let entries: Vec<usize> = (0..app.levels.len())
            .filter(|&i| app.levels[i].briefing.is_some() && app.progress.memories.contains(&app.levels[i].id))
            .collect();
        if entries.is_empty() {
            draw_centered("No briefings received yet.", screen_height() * 0.45, 24, HUD_MUTED);
        } else {
            for (row, &lvl_idx) in entries.iter().enumerate() {
                let (x, y, w, h) = level_row_rect(row);
                let sel = row == idx;
                let bg = if sel { Color::new(0.12, 0.16, 0.22, 0.95) } else { Color::new(0.07, 0.09, 0.13, 0.85) };
                draw_rectangle(x, y, w, h, bg);
                draw_rectangle_lines(x, y, w, h, 2.0, if sel { PLAYER } else { EDGE_COL });
                let lvl = &app.levels[lvl_idx];
                let baseline = y + h * 0.5 + 9.0;
                let label = format!("{:>2}   {}  —  briefing", lvl.id, lvl.title);
                draw_text(&label, x + 18.0, baseline, 24.0, if sel { HUD_TEXT } else { HUD_MUTED });
            }
        }
        draw_centered(
            "Up/Down or mouse  |  Enter: re-read a briefing  |  Esc: back",
            screen_height() - 22.0,
            18,
            HUD_MUTED,
        );
    }
}

/// The ✕ that hides the pause panel (top right, under the topbar): the veil lifts, the
/// buttons go, a corner tag remains — the player cameras around the frozen level.
/// The persistent FULLSCREEN button (owner, 2026-07-10): a small square at the bottom
/// right of the main menu and of missions — video-player style, arrows pointing OUT to go
/// fullscreen, IN to come back. Replaces the pause-menu row (and the victory/defeat rows
/// that mirrored it).
const FS_BTN_S: f32 = 34.0;

fn fullscreen_btn_rect(in_level: bool) -> Rect {
    let s = FS_BTN_S;
    if in_level {
        // Left of the zoom-slider strip, just above the bottom HUD bar.
        Rect::new(screen_width() - 22.0 - 12.0 - s, screen_height() - HUD_BOTTOM_H - s - 8.0, s, s)
    } else {
        Rect::new(screen_width() - s - 14.0, screen_height() - s - 14.0, s, s)
    }
}

fn fullscreen_btn_at_mouse(in_level: bool) -> bool {
    let (mx, my) = mouse_position();
    fullscreen_btn_rect(in_level).contains(vec2(mx, my))
}

/// A small arrow: shaft from `from` to `to`, head at `to`.
fn draw_arrow_small(from: (f32, f32), to: (f32, f32), col: Color) {
    draw_line(from.0, from.1, to.0, to.1, 2.0, col);
    let (dx, dy) = (to.0 - from.0, to.1 - from.1);
    let l = (dx * dx + dy * dy).sqrt().max(1e-3);
    let (ux, uy) = (dx / l, dy / l);
    let (px, py) = (-uy, ux);
    let h = 3.5;
    draw_line(to.0, to.1, to.0 - ux * h + px * h * 0.6, to.1 - uy * h + py * h * 0.6, 2.0, col);
    draw_line(to.0, to.1, to.0 - ux * h - px * h * 0.6, to.1 - uy * h - py * h * 0.6, 2.0, col);
}

fn draw_fullscreen_btn(in_level: bool) {
    let r = fullscreen_btn_rect(in_level);
    let hover = fullscreen_btn_at_mouse(in_level);
    draw_rectangle(r.x, r.y, r.w, r.h, if hover { Color::new(0.16, 0.20, 0.28, 0.95) } else { Color::new(0.10, 0.12, 0.16, 0.85) });
    draw_rectangle_lines(r.x, r.y, r.w, r.h, 1.5, if hover { ACCENT } else { HUD_MUTED });
    let col = if hover { HUD_TEXT } else { HUD_MUTED };
    let (cx, cy) = (r.x + r.w * 0.5, r.y + r.h * 0.5);
    let fs = FULLSCREEN.with(|f| f.get());
    // Four diagonal arrows toward the corners (expand) or back toward the centre (restore).
    for (dx, dy) in [(-1.0f32, -1.0f32), (1.0, -1.0), (-1.0, 1.0), (1.0, 1.0)] {
        let inner = (cx + dx * 3.0, cy + dy * 3.0);
        let outer = (cx + dx * 10.0, cy + dy * 10.0);
        let (from, to) = if fs { (outer, inner) } else { (inner, outer) };
        draw_arrow_small(from, to, col);
    }
}

/// The replay timeline bar (playback only): centred above the bottom HUD.
fn timeline_rect() -> Rect {
    let w = (screen_width() * 0.6).min(900.0);
    Rect::new(screen_width() * 0.5 - w * 0.5, screen_height() - HUD_BOTTOM_H - 22.0, w, 10.0)
}

/// The tick a click on the timeline maps to (generous vertical grab band), if any.
fn timeline_click_tick(game: &Game) -> Option<u64> {
    let rs = game.replay.as_ref()?;
    let r = timeline_rect();
    let (mx, my) = mouse_position();
    if mx >= r.x && mx <= r.x + r.w && (my - (r.y + r.h * 0.5)).abs() <= 14.0 {
        let frac = ((mx - r.x) / r.w).clamp(0.0, 1.0) as f64;
        Some((frac * rs.end_tick as f64) as u64)
    } else {
        None
    }
}

fn pause_cross_rect() -> Rect {
    Rect::new(screen_width() - 48.0, HUD_TOP_H + 12.0, 34.0, 34.0)
}

fn pause_cross_at_mouse() -> bool {
    let (mx, my) = mouse_position();
    pause_cross_rect().contains(vec2(mx, my))
}

/// A column of hover-lit overlay buttons in the shared menu-item geometry (the pause menu's
/// look; the victory menu reuses it). Hit-testing is `menu_item_at_mouse(items.len())`.
fn draw_overlay_buttons(items: &[&str]) {
    let hovered = menu_item_at_mouse(items.len());
    for (i, item) in items.iter().enumerate() {
        let (x, y, w, h) = menu_item_rect(i);
        let sel = hovered == Some(i);
        let bg = if sel { Color::new(0.12, 0.16, 0.22, 0.95) } else { Color::new(0.07, 0.09, 0.13, 0.9) };
        draw_rectangle(x, y, w, h, bg);
        draw_rectangle_lines(x, y, w, h, 2.0, if sel { PLAYER } else { EDGE_COL });
        draw_centered(item, y + h * 0.66, 28, if sel { HUD_TEXT } else { HUD_MUTED });
    }
}

/// The merged pause menu (owner, 2026-07-08): veil, "PAUSED", hover-lit Resume / Restart /
/// Main Menu, and the ✕. Mouse-driven (the camera keys pan the free camera underneath).
fn draw_pause_menu() {
    let sw = screen_width();
    let sh = screen_height();
    draw_rectangle(0.0, 0.0, sw, sh, Color::new(0.0, 0.0, 0.0, 0.6));
    draw_centered("PAUSED", menu_first_y() - menu_pitch() * 0.8, 60, ACCENT);
    draw_overlay_buttons(&["Resume", "Restart", "Main Menu"]);
    let r = pause_cross_rect();
    let hover = pause_cross_at_mouse();
    draw_rectangle(r.x, r.y, r.w, r.h, if hover { Color::new(0.16, 0.20, 0.28, 0.95) } else { Color::new(0.10, 0.12, 0.16, 0.85) });
    draw_rectangle_lines(r.x, r.y, r.w, r.h, 1.5, if hover { ACCENT } else { HUD_MUTED });
    let col = if hover { HUD_TEXT } else { HUD_MUTED };
    let inset = 10.0;
    draw_line(r.x + inset, r.y + inset, r.x + r.w - inset, r.y + r.h - inset, 2.0, col);
    draw_line(r.x + r.w - inset, r.y + inset, r.x + inset, r.y + r.h - inset, 2.0, col);
    draw_centered("Esc: resume", sh - 40.0, 18, HUD_MUTED);
}

/// The panel-hidden pause tag: no veil — just the state, quietly, while the player looks.
fn draw_pause_tag() {
    draw_centered("Paused — Esc for menu", HUD_TOP_H + 30.0, 20, HUD_MUTED);
}

// =============================================================================================
// Notes — a persistent in-mission scratchpad (the player's explicit memory, kept between levels)
// =============================================================================================

/// The small document icon at the bottom-left during a mission (click to open the Notes editor).
fn notes_icon_rect() -> Rect {
    let s = 34.0;
    Rect::new(14.0, screen_height() - HUD_BOTTOM_H - s - 8.0, s, s)
}

/// True if the Notes document icon was just left-clicked.
fn notes_icon_clicked() -> bool {
    if !is_mouse_button_pressed(MouseButton::Left) {
        return false;
    }
    let (mx, my) = mouse_position();
    notes_icon_rect().contains(vec2(mx, my))
}

/// Draw the Notes document icon (bottom-left), highlighting on hover.
fn draw_notes_icon() {
    let r = notes_icon_rect();
    let (mx, my) = mouse_position();
    let hover = r.contains(vec2(mx, my));
    let bg = if hover { Color::new(0.16, 0.20, 0.28, 0.95) } else { Color::new(0.10, 0.12, 0.16, 0.85) };
    draw_rectangle(r.x, r.y, r.w, r.h, bg);
    draw_rectangle_lines(r.x, r.y, r.w, r.h, 1.5, if hover { ACCENT } else { HUD_MUTED });
    // A little document glyph: a page outline + a few text lines.
    let col = if hover { HUD_TEXT } else { HUD_MUTED };
    let px = r.x + r.w * 0.30;
    let py = r.y + r.h * 0.22;
    let pw = r.w * 0.40;
    let ph = r.h * 0.56;
    draw_rectangle_lines(px, py, pw, ph, 1.5, col);
    for k in 0..3 {
        let ly = py + ph * 0.32 + k as f32 * (ph * 0.22);
        draw_line(px + pw * 0.18, ly, px + pw * 0.82, ly, 1.0, col);
    }
}

/// Notes editor popup geometry.
struct NotesLayout {
    box_x: f32,
    box_y: f32,
    box_w: f32,
    box_h: f32,
    text_x: f32,
    text_y: f32,
    text_w: f32,
    text_bottom: f32,
    close_btn: Rect,
}

fn notes_layout() -> NotesLayout {
    let sw = screen_width();
    let sh = screen_height();
    let box_w = (sw * 0.6).clamp(380.0, 760.0);
    let box_h = (sh * 0.62).clamp(280.0, 560.0);
    let box_x = (sw - box_w) * 0.5;
    let box_y = (sh - box_h) * 0.5;
    let pad = 26.0;
    let btn_h = 32.0;
    let btn_y = box_y + box_h - pad - btn_h;
    let text_x = box_x + pad;
    let text_y = box_y + pad + 40.0; // below the title row
    let text_w = box_w - 2.0 * pad;
    let text_bottom = btn_y - 12.0;
    let cbw = 110.0;
    let close_btn = Rect::new(box_x + box_w - pad - cbw, btn_y, cbw, btn_h);
    NotesLayout { box_x, box_y, box_w, box_h, text_x, text_y, text_w, text_bottom, close_btn }
}

/// Handle input while the Notes editor is open: type into `app.notes`; Esc or the Close button
/// closes and saves. Called from the top of `app_update`, so the game is frozen meanwhile.
fn handle_notes_input(app: &mut App) {
    let lay = notes_layout();
    let (mx, my) = mouse_position();
    let close_click = is_mouse_button_pressed(MouseButton::Left) && lay.close_btn.contains(vec2(mx, my));
    if is_key_pressed(KeyCode::Escape) || close_click {
        app.notes_open = false;
        save_notes(&app.notes);
        return;
    }
    // Typed characters (printable only — control chars are handled below by keycode).
    while let Some(c) = get_char_pressed() {
        if !c.is_control() {
            app.notes.push(c);
        }
    }
    if is_key_pressed(KeyCode::Enter) || is_key_pressed(KeyCode::KpEnter) {
        app.notes.push('\n');
    }
    if is_key_pressed(KeyCode::Backspace) {
        app.notes.pop();
    }
}

/// Draw the Notes editor popup with the current `notes` text + a blinking cursor at the end.
fn draw_notes(notes: &str) {
    let lay = notes_layout();
    let sw = screen_width();
    let sh = screen_height();
    draw_rectangle(0.0, 0.0, sw, sh, Color::new(0.02, 0.03, 0.05, 0.82));
    draw_rectangle(lay.box_x, lay.box_y, lay.box_w, lay.box_h, Color::new(0.07, 0.09, 0.12, 0.99));
    draw_rectangle_lines(lay.box_x, lay.box_y, lay.box_w, lay.box_h, 2.0, Color::new(0.55, 0.70, 0.90, 0.85));
    draw_text("Notes", lay.box_x + 26.0, lay.box_y + 34.0, 28.0, ACCENT);

    // Text: wrap on the box width, honour explicit newlines; track the cursor (end of text).
    let size: u16 = 20;
    let line_h = 26.0;
    let mut cx = lay.text_x;
    let mut cy = lay.text_y;
    for (li, line) in notes.split('\n').enumerate() {
        if li > 0 {
            cx = lay.text_x;
            cy += line_h;
        }
        for word in line.split_inclusive(' ') {
            let w = measure_text(word, None, size, 1.0).width;
            if cx > lay.text_x && cx + w > lay.text_x + lay.text_w {
                cx = lay.text_x;
                cy += line_h;
            }
            if cy <= lay.text_bottom {
                draw_text(word, cx, cy, size as f32, HUD_TEXT);
            }
            cx += w;
        }
    }
    // Blinking cursor at the current end-of-text position.
    if ((get_time() * 2.0) as i64) % 2 == 0 && cy <= lay.text_bottom {
        draw_rectangle(cx + 1.0, cy - size as f32 * 0.78, 2.0, size as f32, ACCENT);
    }

    draw_brief_button(lay.close_btn, "Close", true);
}

// =============================================================================================
// In-level rendering (lens + interior, crossfaded by cam_t)
// =============================================================================================

fn draw_in_level(game: &Game) {
    let cam = game.camera();

    // Lens fades out as we zoom in; interior fades in. A crossfade band keeps the transition
    // smooth without trying to morph two different scene representations into one.
    let interior_alpha = ((game.cam_t - INTERIOR_DRAW_THRESHOLD) / (1.0 - INTERIOR_DRAW_THRESHOLD))
        .clamp(0.0, 1.0);
    let lens_alpha = (1.0 - interior_alpha).clamp(0.0, 1.0);

    if lens_alpha > 0.001 {
        draw_lens(game, &cam, lens_alpha);
    }
    if interior_alpha > 0.001 {
        draw_interior(game, game.focus, &cam, interior_alpha);
    }

    // Selection box while dragging (screen space, over the board, under the HUD).
    if game.box_active {
        if let Some((px, py)) = game.drag_start {
            let (mx, my) = mouse_position();
            let (x, y) = (px.min(mx), py.min(my));
            let (bw, bh) = ((mx - px).abs(), (my - py).abs());
            draw_rectangle(x, y, bw, bh, Color::new(0.60, 0.75, 1.0, 0.12));
            draw_rectangle_lines(x, y, bw, bh, 1.5, Color::new(0.85, 0.90, 1.0, 0.9));
        }
    }

    draw_hud(game);
    draw_zoom_slider(game);

    // The pause: the menu panel over a darkened board, or — panel hidden via the ✕ — a bare
    // corner tag over the un-darkened board (the player is inspecting the frozen level). The
    // intro / end overlays supersede it.
    if game.paused() && !game.show_intro && !game.match_over() {
        if game.pause_buttons {
            draw_pause_menu();
        } else {
            draw_pause_tag();
        }
    }
    if game.show_intro {
        draw_intro_overlay(game);
    }
    if game.match_over() {
        draw_end_banner(game);
        if game.show_post_log {
            if let Some(b) = &game.post_log_brief {
                draw_briefing(b);
            }
        } else if game.post_log_brief.is_some() {
            draw_centered("L: battle log", screen_height() - 64.0, 18, HUD_MUTED);
        }
    }
    // The replay timeline (playback only) — on top of overlays, like the fullscreen button.
    if let Some(rs) = &game.replay {
        let r = timeline_rect();
        draw_rectangle(r.x, r.y, r.w, r.h, Color::new(0.0, 0.0, 0.0, 0.55));
        let frac = (game.world.tick as f32 / rs.end_tick.max(1) as f32).clamp(0.0, 1.0);
        draw_rectangle(r.x, r.y, r.w * frac, r.h, Color::new(0.35, 0.55, 0.95, 0.9));
        draw_rectangle_lines(r.x, r.y, r.w, r.h, 1.5, HUD_MUTED);
        // PLAYHEAD (owner tweak, 2026-07-10): a white vertical bar with an inverted
        // triangle riding the present tick — the video-player affordance that says
        // "this is a cursor; click the bar to move it".
        let px = r.x + r.w * frac;
        draw_line(px, r.y - 2.0, px, r.y + r.h + 2.0, 2.0, WHITE);
        draw_triangle(
            vec2(px - 5.0, r.y - 10.0),
            vec2(px + 5.0, r.y - 10.0),
            vec2(px, r.y - 3.0),
            WHITE,
        );
        // Human time at the fixed 60 ticks/second play rate (owner tweak: "2:00 / 8:00",
        // not raw ticks), matching the browser's duration column.
        let label = if rs.seek_target.is_some() {
            format!(
                "REPLAY  {} / {}  (seeking...)",
                fmt_duration_60tps(game.world.tick),
                fmt_duration_60tps(rs.end_tick)
            )
        } else {
            format!(
                "REPLAY  {} / {}",
                fmt_duration_60tps(game.world.tick),
                fmt_duration_60tps(rs.end_tick)
            )
        };
        draw_text(&label, r.x, r.y - 16.0, 16.0, HUD_MUTED);
        if rs.diverged {
            let warn = "DIVERGED — build/level does not match the recording";
            let d = measure_text(warn, None, 16, 1.0);
            draw_text(warn, r.x + r.w - d.width, r.y - 16.0, 16.0, ENEMY);
        }
    }

    // The persistent fullscreen toggle, on top of every overlay (it stays clickable there).
    draw_fullscreen_btn(true);
}

/// Draw the Layer-2 lens: lanes, interpolated fleets, struct nodes.
fn draw_lens(game: &Game, cam: &Camera, alpha: f32) {
    let t = get_time() as f32;
    let w = &game.world;

    // Valid-target neighbours of a selected source (for highlighting).
    let valid: Option<std::collections::HashSet<StructId>> = game.sel_struct.map(|src| {
        w.neighbors(src).iter().copied().collect()
    });

    // --- Lanes ---
    for lane in &w.lanes {
        let (ax, ay) = cam.to_screen(w.structs[lane.a].pos.x, w.structs[lane.a].pos.y);
        let (bx, by) = cam.to_screen(w.structs[lane.b].pos.x, w.structs[lane.b].pos.y);
        let hi = match (game.sel_struct, &valid) {
            (Some(src), Some(set)) => (lane.a == src && set.contains(&lane.b)) || (lane.b == src && set.contains(&lane.a)),
            _ => false,
        };
        if hi {
            draw_line(ax, ay, bx, by, 3.0, fade(EDGE_HILITE, alpha));
        } else {
            draw_line(ax, ay, bx, by, 1.5, fade(EDGE_COL, alpha));
        }
    }

    // --- Fleets (interpolated along their lane) ---
    for idx in 0..w.fleets.len() {
        let f = &w.fleets[idx];
        let pos = game.fleet_draw_progress(idx).clamp(0.0, 1.0);
        let (fx, fy) = cam.to_screen(w.structs[f.from].pos.x, w.structs[f.from].pos.y);
        let (tx, ty) = cam.to_screen(w.structs[f.to].pos.x, w.structs[f.to].pos.y);
        let cx = fx + (tx - fx) * pos;
        let cy = fy + (ty - fy) * pos;
        let dir = {
            let (dx, dy) = (tx - fx, ty - fy);
            let l = (dx * dx + dy * dy).sqrt().max(1e-3);
            (dx / l, dy / l)
        };
        let col = fade(game.col(f.faction), alpha);
        draw_fleet_cluster(cx, cy, dir, f.count as f64, col, f.undock_remaining > 0, t, cam.scale);
    }

    // --- Fleet-loss flashes: a white cross where transit ships were shot down, plus a thin
    // tracer from whatever shot them (the lens twin of the interior's ship-death flash —
    // Layer-2 combat reads through these). Struct nodes draw AFTER this, so an overwatch
    // tracer visually emerges from the node's rim rather than its centre.
    for &(at, from, born) in &game.fleet_kill_fx {
        let age = (get_time() - born).max(0.0);
        let life = (1.0 - (age / KILL_FX_TTL) as f32).clamp(0.0, 1.0);
        if life <= 0.0 {
            continue;
        }
        let a = alpha * life;
        let (vx, vy) = cam.to_screen(at.x, at.y);
        let (fx, fy) = cam.to_screen(from.x, from.y);
        draw_line(fx, fy, vx, vy, 1.0, Color::new(1.0, 1.0, 1.0, 0.35 * a));
        let s = 3.5;
        draw_line(vx - s, vy - s, vx + s, vy + s, 2.0, Color::new(1.0, 1.0, 1.0, 0.9 * a));
        draw_line(vx - s, vy + s, vx + s, vy - s, 2.0, Color::new(1.0, 1.0, 1.0, 0.9 * a));
    }

    // Off-screen structs keep a presence on the border (arrows pointing at them).
    draw_offscreen_struct_arrows(game, cam, alpha);

    // --- Structure nodes ---
    for i in 0..w.structs.len() {
        let agg = w.struct_aggregate(i);
        let (sx, sy) = cam.to_screen(w.structs[i].pos.x, w.structs[i].pos.y);
        let r = node_screen_radius(game, i, cam);

        let base = game.struct_col(agg.owner);
        let dim = match agg.owner {
            StructOwner::Owned(f) => game.dim(f),
            StructOwner::Neutral => NEUTRAL_DIM,
            StructOwner::Contested => Color::new(0.4, 0.34, 0.2, 1.0),
        };

        // MINI-INTERIOR node (owner design, 2026-07-08): the struct's real sub layout drawn
        // in true proportion inside the node — Layer 2 shows the actual map, and at the
        // zoom handoff the miniature grows continuously into the interior view. Scale maps
        // the reserve ring (the interior's visible extent) onto the node radius.
        let st = &w.structs[i].interior;
        let r_int = st
            .storage_sub
            .map(|g| st.subs[g].radius)
            .unwrap_or_else(|| st.subs.iter().map(|s| s.pos.dist(layer1::Vec2::new(0.0, 0.0)) + s.radius).fold(1.0, f32::max));
        let s_px = r / r_int.max(1e-3);
        draw_circle(sx, sy, r, fade(Color::new(dim.r, dim.g, dim.b, 0.18), alpha));
        for (si, sub) in st.subs.iter().enumerate() {
            if st.is_storage(si) {
                continue;
            }
            let (px, py) = (sx + sub.pos.x * s_px, sy - sub.pos.y * s_px);
            let sr = (sub.radius * s_px).max(1.2);
            let (fill, line) = if sub.owner.is_real() {
                (game.dim(sub.owner), game.col(sub.owner))
            } else {
                (NEUTRAL_DIM, NEUTRAL)
            };
            draw_circle(px, py, sr, fade(Color::new(fill.r, fill.g, fill.b, 0.6), alpha));
            draw_circle_lines(px, py, sr, 1.0, fade(line, alpha));
        }
        draw_circle_lines(sx, sy, r, 2.5, fade(base, alpha));
        // OVERWATCH zone (owner design, 2026-07-08): a fully-owned struct's defensive band
        // into the lanes — the fortresses' faint threat-ring language, spoken at Layer 2.
        if let Some(zone_owner) = w.structs[i].sole_owner() {
            let oc = game.col(zone_owner);
            draw_circle_lines(
                sx,
                sy,
                cam.len(w.structs[i].overwatch_reach()),
                1.0,
                fade(Color::new(oc.r, oc.g, oc.b, 0.12), alpha),
            );
        }

        // Automation indicator: a green ring + "AUTO" tag on automated, player-owned structs.
        if game.automated.get(i).copied().unwrap_or(false) {
            let pulse = 0.5 + 0.5 * (t * 3.0).sin();
            draw_circle_lines(sx, sy, r + 6.0, 2.0, fade(Color::new(AUTO_COL.r, AUTO_COL.g, AUTO_COL.b, 0.5 + 0.4 * pulse), alpha));
            draw_text("AUTO", sx - 16.0, sy + r + 16.0, 16.0, fade(AUTO_COL, alpha));
        }

        // Production pip on owned (producing) structs.
        if let StructOwner::Owned(f) = agg.owner {
            if f.is_real() {
                let pulse = 0.5 + 0.5 * (t * 2.2 + i as f32 * 0.6).sin();
                let pr = 3.0 + 1.6 * pulse;
                draw_circle(sx + r * 0.72, sy - r * 0.72, pr, fade(Color::new(base.r, base.g, base.b, 0.85), alpha));
            }
        }

        // Selected source outline (single-select or a box multi-selection).
        if game.sel_struct == Some(i) || game.sel_structs.contains(&i) {
            let pulse = 0.5 + 0.5 * (t * 4.0).sin();
            draw_circle_lines(sx, sy, r + 8.0, 2.5, fade(Color::new(1.0, 1.0, 1.0, 0.5 + 0.4 * pulse), alpha));
        }

        // Reserve-node garrison as dots: the staging area's idle ships shown right in the lens,
        // placed on a ring inside the node at each ship's real Layer-1 orbit angle (mirrors the
        // interior orbit viz). These are the ships rallied at the reserve and ready to launch an
        // inter-struct fleet — what the player sends with a struct→struct order.
        {
            let st = &w.structs[i].interior;
            if let Some(stg) = st.storage_sub {
                for sh in &st.ships {
                    if sh.alive && sh.target.is_none() && sh.home == stg && sh.faction.is_real() {
                        // The interior's own dots, miniaturised (owner design, 2026-07-08):
                        // the ship's REAL position scaled by the same factor as the subs —
                        // smaller and slightly transparent, the L1 language at L2.
                        let (px, py) = (sx + sh.pos.x * s_px, sy - sh.pos.y * s_px);
                        let c = game.col(sh.faction);
                        draw_circle(px, py, 1.5, fade(Color::new(c.r, c.g, c.b, 0.55), alpha));
                    }
                }
            }
        }

        // Per-side **present** ship counts (in the structure; incoming inter-struct fleets are not
        // here yet), drawn just **above** the node, each in its faction colour. Contested ⇒ both,
        // stacked above (enemy nearest the node, player over it).
        let fs = (r * 0.8).clamp(13.0, 28.0) as u16;
        let above = sy - r - 6.0;
        let line_h = fs as f32 + 2.0;
        let count_at = |val: usize, col: Color, line: f32| {
            let s = val.to_string();
            let d = measure_text(&s, None, fs, 1.0);
            draw_text(&s, sx - d.width * 0.5, above - line * line_h, fs as f32, fade(col, alpha));
        };
        // One line per **present** seat, each in its own colour: every AI rival first (nearest the
        // node, in seat order), then the player on top. Per-seat counts tallied in **one pass** over
        // this struct's ships (vs `ship_count` per seat = O(seats · N)).
        let st_i = &w.structs[i].interior;
        let nseats = 1 + game.level.enemies.len();
        let mut counts = vec![0u32; nseats];
        for sh in &st_i.ships {
            if !sh.alive {
                continue;
            }
            match sh.faction {
                Faction::Player => counts[0] += 1,
                Faction::Ai(a) => {
                    let c = 1 + a as usize;
                    if c < nseats {
                        counts[c] += 1;
                    }
                }
                Faction::Neutral => {}
            }
        }
        let mut count_lines = 0.0f32;
        for ai in 0..game.level.enemies.len() {
            if counts[1 + ai] > 0 {
                count_at(counts[1 + ai] as usize, game.col(Faction::Ai(ai as u8)), count_lines);
                count_lines += 1.0;
            }
        }
        if counts[0] > 0 {
            count_at(counts[0] as usize, PLAYER, count_lines);
            count_lines += 1.0;
        }

        // Structure name above the counts.
        let name = &w.structs[i].name;
        let nd = measure_text(name, None, 16, 1.0);
        let name_y = above - count_lines * line_h - 8.0;
        draw_text(name, sx - nd.width * 0.5, name_y, 16.0, fade(HUD_MUTED, alpha));
    }
}

/// Draw the Layer-1 interior of struct `p` (subs, ships, battle bubbles) — like the standalone
/// Layer-1 game, but in the shared world-space camera.
fn draw_interior(game: &Game, p: StructId, cam: &Camera, alpha: f32) {
    let t = get_time() as f32;
    let structure = &game.world.structs[p];
    let st = &structure.interior;
    let ox = structure.pos.x;
    let oy = structure.pos.y;

    // Reference grid (every 10 local units), faint.
    draw_interior_grid(structure, cam, alpha);

    // Per-frame tally: idle (home-based) ship counts by (sub, seat) in ONE O(N) pass, so the per-sub
    // labels + contested ring below read this table instead of re-scanning all ships per sub per
    // faction (was O(subs · seats · N)). Column 0 = Player, column 1+i = Ai(i); no ship is ever Neutral.
    let nseats = 1 + game.level.enemies.len();
    let mut idle_by_sub: Vec<Vec<u32>> = vec![vec![0u32; nseats]; st.subs.len()];
    for sh in &st.ships {
        if !sh.is_idle() {
            continue;
        }
        let col = match sh.faction {
            Faction::Player => 0,
            Faction::Ai(a) => 1 + a as usize,
            Faction::Neutral => continue,
        };
        if let Some(row) = idle_by_sub.get_mut(sh.home) {
            if col < row.len() {
                row[col] += 1;
            }
        }
    }

    // --- Sub-structures ---
    for (i, s) in st.subs.iter().enumerate() {
        let (sx, sy) = cam.to_screen(ox + s.pos.x, oy + s.pos.y);
        // World-true radius, NO px floor: ships orbit at their real sim positions, so a floored
        // (inflated) circle would show the garrison ring at a zoom-dependent fraction of the
        // body — the circle must shrink with the ships or the geometry lies.
        let r = cam.len(s.radius);
        let base = game.col(s.owner);
        let dim = game.dim(s.owner);
        let is_storage = st.is_storage(i);
        let shipyard_inactive = matches!(s.kind, layer1::SubKind::Shipyard { active: false });
        if is_storage {
            // Reserve / patrol-zone node: the big circle enclosing all subs — the universal
            // entry/exit point. Drawn as an outline with only a whisper of fill so the inner
            // subs and their orbits read clearly through it. Produces nothing (no squares/ring),
            // but is attackable, selectable, and shows its garrisoned reserve like any sub.
            draw_circle(sx, sy, r, fade(Color::new(base.r, base.g, base.b, 0.05), alpha));
            draw_circle_lines(sx, sy, r, 1.5, fade(Color::new(base.r, base.g, base.b, 0.5), alpha));
        } else {
            match s.kind {
                layer1::SubKind::Fortress => {
                    // FORTRESS: a diamond (4-gon with a vertex up) instead of a circle, plus —
                    // while owned — a faint ring showing the overwatch reach of its garrison
                    // (ring radius + the fixed fortress range, the threat envelope).
                    draw_poly(sx, sy, 4, r, 0.0, fade(Color::new(dim.r, dim.g, dim.b, 0.35), alpha));
                    draw_poly_lines(sx, sy, 4, r, 0.0, 2.0, fade(base, alpha));
                    if s.owner.is_real() {
                        let reach = s.ring_frac * s.radius + layer1::sim::FORTRESS_RANGE;
                        draw_circle_lines(
                            sx,
                            sy,
                            cam.len(reach),
                            1.0,
                            fade(Color::new(base.r, base.g, base.b, 0.12), alpha),
                        );
                    }
                }
                layer1::SubKind::Teleporter => {
                    // TELEPORTER: nested circles — the body plus two rings OUTSIDE the garrison
                    // orbit (owner ask: the innermost ring sits beyond the ship circle, so the
                    // gate reads bigger than the parade it swallows; the orbit band tops out at
                    // (ring_frac + jitter) ≈ 0.85 × radius, safely inside the r-radius body
                    // line). Widely separated, brightening outward: owner colour → a lightened
                    // owner colour → white (the portal glows at its rim).
                    let bright = Color::new(
                        base.r + (1.0 - base.r) * 0.55,
                        base.g + (1.0 - base.g) * 0.55,
                        base.b + (1.0 - base.b) * 0.55,
                        0.9,
                    );
                    draw_circle(sx, sy, r, fade(Color::new(dim.r, dim.g, dim.b, 0.35), alpha));
                    draw_circle_lines(sx, sy, r, 2.0, fade(base, alpha));
                    draw_circle_lines(sx, sy, r * 1.36, 1.5, fade(bright, alpha));
                    draw_circle_lines(sx, sy, r * 1.72, 1.5, fade(Color::new(1.0, 1.0, 1.0, 0.9), alpha));
                }
                layer1::SubKind::Shipyard { .. } => {
                    // SHIPYARD: no body — the production squares (or the pre-activation grid
                    // below) ARE the visual; the normal-size circle exists only for selection
                    // and the garrison ring (rings/bars/counts below still render).
                }
                layer1::SubKind::Standard => {
                    draw_circle(sx, sy, r, fade(Color::new(dim.r, dim.g, dim.b, 0.35), alpha));
                    draw_circle_lines(sx, sy, r, 2.0, fade(base, alpha));
                }
            }
        }

        // Production squares: one empty white square per unit of production capacity, evenly
        // spaced at half the sub radius — a freshly minted ship appears at the next square
        // (round-robin) then glides out to the garrison ring. Their angles match the sim's
        // `spawn_at_square`, so what you see is where ships actually appear. A production-0
        // sub (fortress, teleporter, the reserve node) gets NONE — the sim never spawns there,
        // and the old `.max(1)` drew a phantom square inside every fortress. (A pre-activation
        // shipyard draws the segmenting grid below instead.)
        if !is_storage && !shipyard_inactive && s.production > 0 {
            let prod = s.production;
            let sq_ring = r * 0.4;
            // Zoom-proportional (owner fix: the old 2..6 px clamp froze the squares' size
            // while everything else scaled); a 1 px floor keeps them visible when tiny.
            let half = (r * 0.1).max(1.0);
            // The slots ARE the sub's spawn points, so the phase comes from the SIM tick (not
            // wall-clock) — interpolated by render_alpha to match the ship interpolation timeline —
            // so the drawn squares stay locked to where ships are actually created. Screen y grows
            // down, so we negate the sine; +phase reads counter-clockwise on screen.
            let phase = (game.world.tick as f32 - 1.0 + game.render_alpha)
                * game.sim.prod_square_spin;
            for k in 0..prod {
                let ang = k as f32 * std::f32::consts::TAU / prod as f32 + phase;
                let qx = sx + sq_ring * ang.cos();
                let qy = sy - sq_ring * ang.sin();
                draw_rectangle_lines(qx - half, qy - half, half * 2.0, half * 2.0, 1.5, fade(Color::new(1.0, 1.0, 1.0, 0.7), alpha));
            }
        }

        // Pre-activation SHIPYARD: a tight white grid of its future production squares that
        // **progressively segments** toward their ring slots as the activation grind advances
        // (t = the eroded fraction of the initial resistance). At t=0 the 8 cells sit packed in
        // a 3x3 block (the "sealed" yard); as attackers grind the bar each cell lerps out to its
        // production-square slot (matching `spawn_at_square` angles), where the standard square
        // rendering takes over on activation. Groundwork for a richer animation later.
        if shipyard_inactive {
            let prod = s.production.max(1);
            let t_act = 1.0
                - if s.max_resistance > 0.0 {
                    (s.resistance / s.max_resistance).clamp(0.0, 1.0)
                } else {
                    0.0
                };
            let sq_ring = r * 0.4;
            let half = (r * 0.1).clamp(2.0, 6.0);
            let spacing = half * 2.3;
            let phase = (game.world.tick as f32 - 1.0 + game.render_alpha)
                * game.sim.prod_square_spin;
            // The 8 cells of a 3x3 grid (centre omitted), row-major.
            const GRID: [(f32, f32); 8] = [
                (-1.0, -1.0), (0.0, -1.0), (1.0, -1.0),
                (-1.0, 0.0), (1.0, 0.0),
                (-1.0, 1.0), (0.0, 1.0), (1.0, 1.0),
            ];
            let col = fade(Color::new(1.0, 1.0, 1.0, 0.75), alpha);
            for k in 0..prod {
                let (gx, gy) = GRID[(k as usize) % GRID.len()];
                let (gx, gy) = (sx + gx * spacing, sy + gy * spacing);
                let ang = k as f32 * std::f32::consts::TAU / prod as f32 + phase;
                let (fx, fy) = (sx + sq_ring * ang.cos(), sy - sq_ring * ang.sin());
                let qx = gx + (fx - gx) * t_act;
                let qy = gy + (fy - gy) * t_act;
                draw_rectangle_lines(qx - half, qy - half, half * 2.0, half * 2.0, 1.5, col);
            }
            // The sealed yard's outer frame, fading out as it segments.
            let frame = spacing + half;
            draw_rectangle_lines(
                sx - frame,
                sy - frame,
                frame * 2.0,
                frame * 2.0,
                1.5,
                fade(Color::new(1.0, 1.0, 1.0, 0.55 * (1.0 - t_act)), alpha),
            );
        }

        // Capture state: the lone present faction (if any) grinding this sub. Home-based — the
        // same counts `resolve_resistance` acts on — so the cue matches the actual grind (WYSIWYG).
        let lone = st.capture_present_faction(i).map(|(f, _)| f);
        let eroding_by = lone.filter(|&f| f != s.owner); // being CAPTURED by this faction
        let res_frac = if s.max_resistance > 0.0 { (s.resistance / s.max_resistance).clamp(0.0, 1.0) } else { 1.0 };
        let healing = matches!(lone, Some(f) if f == s.owner) && res_frac < 0.999;

        // Per-side **present** (idle, garrisoned) ship counts — one `(count, colour)` entry per seat
        // present, in seat order: Player, then **each AI rival in its own colour**, then Neutral.
        // Drives both the contested ring and the labels below; generalises to any number of opponents.
        let mut present: Vec<(usize, Color)> = Vec::new();
        {
            let row = &idle_by_sub[i];
            if row[0] > 0 {
                present.push((row[0] as usize, game.col(Faction::Player)));
            }
            for ai in 0..game.level.enemies.len() {
                let c = row[1 + ai];
                if c > 0 {
                    present.push((c as usize, game.col(Faction::Ai(ai as u8))));
                }
            }
        }

        // Contested-ownership ring: when two or more sides have ships present, draw a ring split into
        // each side's colour, each arc ∝ its share of the present ships (10 vs 30 ⇒ a quarter / three-
        // quarters split). One side present ⇒ no ring (the sub's own fill already reads its holder).
        if present.len() >= 2 {
            let total: f32 = present.iter().map(|(c, _)| *c as f32).sum();
            let slices: Vec<(f32, Color)> =
                present.iter().map(|(c, col)| (*c as f32 / total, *col)).collect();
            draw_ownership_ring(sx, sy, r + 4.0, &slices, alpha);
        }

        // Being-captured cue: a pulsing ring in the attacker's colour around the sub.
        if let Some(atk) = eroding_by {
            let pulse = 0.5 + 0.5 * (t * 6.0).sin();
            let ac = game.col(atk);
            draw_circle_lines(sx, sy, r + 2.0, 2.5, fade(Color::new(ac.r, ac.g, ac.b, 0.45 + 0.45 * pulse), alpha));
        }

        // Resistance ("siege") bar below the sub: capturable neutrals, damaged, or being ground.
        if s.owner == Faction::Neutral || res_frac < 0.999 || eroding_by.is_some() {
            let fill = if s.owner == Faction::Neutral { NEUTRAL_DIM } else { base };
            draw_resistance_bar(sx, sy + r + 7.0, r, res_frac, fill, eroding_by.map(|f| game.col(f)), healing, alpha, t);
        }

        // Selected source outline (single-select or a box multi-selection).
        if matches!(game.view, View::Interior(fp) if fp == p)
            && (game.sel_sub == Some(i) || game.sel_subs.contains(&i))
        {
            let pulse = 0.5 + 0.5 * (t * 4.0).sin();
            draw_circle_lines(sx, sy, r + 7.0, 2.5, fade(Color::new(1.0, 1.0, 1.0, 0.5 + 0.4 * pulse), alpha));
        }

        // Counts above the sub. With **one or no** side present, a single centred "<count> / <cap>"
        // line shows the present side's count over the capacity (capacity greyed) — so the capacity is
        // visible on every sub/structure. With **two+ sides** present, each side's count stacks vertically
        // (right-aligned, in its colour) and the "/ cap" sits to their right at the **mean** of their
        // vertical positions. (Ships in transit are not counted.)
        let fs = (r * 0.9).clamp(14.0, 34.0) as u16;
        let above = sy - r - 6.0;
        let line_h = fs as f32 + 2.0;
        let cap = s.storage_capacity;
        // A shipyard's declared capacity is 0 and its VIRTUAL cap is deliberately hidden — a
        // "120 / 0" label would read as a bug, so yards show the bare count.
        let show_cap = !matches!(s.kind, layer1::SubKind::Shipyard { .. });
        if present.len() >= 2 {
            let n = present.len();
            let cap_str = if show_cap { format!(" / {cap}") } else { String::new() };
            let cap_w = measure_text(&cap_str, None, fs, 1.0).width;
            let max_w = present
                .iter()
                .map(|(c, _)| measure_text(&c.to_string(), None, fs, 1.0).width)
                .fold(0.0_f32, f32::max);
            // Counts share a right edge `x_r`; the whole group (counts + capacity) is centred on sx.
            let x_r = sx + (max_w - cap_w) * 0.5;
            for (k, (c, col)) in present.iter().enumerate() {
                let cs = c.to_string();
                let w = measure_text(&cs, None, fs, 1.0).width;
                let y = above - ((n - 1 - k) as f32) * line_h; // last entry sits at `above`
                draw_text(&cs, x_r - w, y, fs as f32, fade(*col, alpha));
            }
            // Capacity (greyed) at the mean of the counts' vertical positions.
            let mean_y = above - ((n - 1) as f32) * 0.5 * line_h;
            draw_text(&cap_str, x_r, mean_y, fs as f32, fade(HUD_MUTED, alpha));
        } else {
            // 0 or 1 side present: a centred "<count> / <cap>" line — the **present** side's count
            // (so an ownerless storage shows its staged occupants, not the owner's 0), in that side's
            // colour; or "0 / cap" in the owner's colour when empty.
            let (count, col) = match present.first() {
                Some(&(c, col)) => (c, col),
                None => (0usize, game.col(s.owner)),
            };
            let a = count.to_string();
            let b = if show_cap { format!(" / {cap}") } else { String::new() };
            let wa = measure_text(&a, None, fs, 1.0).width;
            let wb = measure_text(&b, None, fs, 1.0).width;
            let x0 = sx - (wa + wb) * 0.5;
            draw_text(&a, x0, above, fs as f32, fade(col, alpha));
            draw_text(&b, x0 + wa, above, fs as f32, fade(HUD_MUTED, alpha));
        }
    }

    // --- Ships (drawn at their real, interpolated sim positions) ---
    // Idle ships sit on their sub's real orbit ring (computed in the sim); ships in transit fly.
    // What's drawn IS the combat geometry — no separate visual ring (WYSIWYG). Batched into a single
    // mesh (one draw call), off-screen-culled, with a density LOD — see `draw_ships_interior`.
    draw_ships_interior(game, p, st, cam, ox, oy, alpha);
    // Off-screen ships (e.g. staged in the reserve, or mid-transit beyond the frame) keep a
    // presence: border arrows pointing at them, distance-scaled.
    draw_offscreen_ship_arrows(game, p, st, cam, ox, oy, alpha);

    // --- Ship-death flashes (white cross + a thin line from the nearby enemy that downed it) ---
    // Battle "bubbles" are no longer a visible concept — combat reads through these flashes.
    for fx in &game.kill_fx {
        if fx.sid != p {
            continue;
        }
        let age = (t as f64 - fx.born).max(0.0);
        let life = (1.0 - (age / KILL_FX_TTL) as f32).clamp(0.0, 1.0);
        let a = alpha * life;
        let (vx, vy) = cam.to_screen(ox + fx.at.x, oy + fx.at.y);
        if let Some(src) = fx.from {
            let (sxs, sys) = cam.to_screen(ox + src.x, oy + src.y);
            draw_line(sxs, sys, vx, vy, 1.0, Color::new(1.0, 1.0, 1.0, 0.35 * a));
        }
        let s = 3.5;
        draw_line(vx - s, vy - s, vx + s, vy + s, 2.0, Color::new(1.0, 1.0, 1.0, 0.9 * a));
        draw_line(vx - s, vy + s, vx + s, vy - s, 2.0, Color::new(1.0, 1.0, 1.0, 0.9 * a));
    }

    // --- Teleport flashes: a transient white line from the gate to the arrival point, quickly
    // fading — the visual "the ship went THAT way" for the no-transit hop.
    for fx in &game.teleport_fx {
        if fx.sid != p {
            continue;
        }
        let age = (t as f64 - fx.born).max(0.0);
        let life = (1.0 - (age / TELEPORT_FX_TTL) as f32).clamp(0.0, 1.0);
        let a = alpha * life;
        let (ax, ay) = cam.to_screen(ox + fx.from.x, oy + fx.from.y);
        let (bx, by) = cam.to_screen(ox + fx.to.x, oy + fx.to.y);
        draw_line(ax, ay, bx, by, 1.5, Color::new(1.0, 1.0, 1.0, 0.28 * a));
    }

    // AUTO badge for this struct while zoomed in.
    if game.automated.get(p).copied().unwrap_or(false) {
        draw_text("AUTO ENABLED", 16.0, HUD_TOP_H + 44.0, 20.0, fade(AUTO_COL, alpha));
    }
}

fn draw_interior_grid(structure: &world::Structure, cam: &Camera, alpha: f32) {
    let (mut minx, mut miny, mut maxx, mut maxy) =
        (f32::INFINITY, f32::INFINITY, f32::NEG_INFINITY, f32::NEG_INFINITY);
    for s in &structure.interior.subs {
        minx = minx.min(s.pos.x - s.radius);
        miny = miny.min(s.pos.y - s.radius);
        maxx = maxx.max(s.pos.x + s.radius);
        maxy = maxy.max(s.pos.y + s.radius);
    }
    if !minx.is_finite() {
        return;
    }
    let step = 10.0_f32;
    let (ox, oy) = (structure.pos.x, structure.pos.y);
    let x0 = (minx / step).floor() * step - step;
    let x1 = (maxx / step).ceil() * step + step;
    let y0 = (miny / step).floor() * step - step;
    let y1 = (maxy / step).ceil() * step + step;
    let g = fade(GRID, alpha);
    let mut x = x0;
    while x <= x1 {
        let (sx, sa) = cam.to_screen(ox + x, oy + y0);
        let (_, sb) = cam.to_screen(ox + x, oy + y1);
        draw_line(sx, sa, sx, sb, 1.0, g);
        x += step;
    }
    let mut y = y0;
    while y <= y1 {
        let (sa, sy) = cam.to_screen(ox + x0, oy + y);
        let (sb, _) = cam.to_screen(ox + x1, oy + y);
        draw_line(sa, sy, sb, sy, 1.0, g);
        y += step;
    }
}

// =============================================================================================
// Shared drawing helpers
// =============================================================================================

#[inline]
fn fade(c: Color, a: f32) -> Color {
    Color::new(c.r, c.g, c.b, c.a * a)
}

/// A struct node's screen radius: its world radius scaled by the camera, with sane bounds so it
/// reads at any zoom.
/// The LENS zoom at which struct `p`'s node is exactly as large on screen as its interior
/// reads at that struct's out-zoom floor — the L1↔L2 continuity point (owner, 2026-07-08).
fn lens_match_zoom(game: &Game, p: StructId) -> f32 {
    let inter = interior_camera(&game.world, p, HUD_TOP_H, HUD_BOTTOM_H);
    let lens = lens_camera(&game.world, HUD_TOP_H, HUD_BOTTOM_H);
    let st = &game.world.structs[p].interior;
    let r_int = st.storage_sub.map(|g| st.subs[g].radius).unwrap_or(30.0);
    let node = game.world.structs[p].l2_radius().max(1e-3);
    (inter.scale * game.struct_zoom_floor(p) * r_int) / (lens.scale * node).max(1e-6)
}

fn node_screen_radius(game: &Game, i: StructId, cam: &Camera) -> f32 {
    // Floor for far-out readability; NO ceiling — zooming the lens in grows the node
    // continuously toward the interior view (the L1↔L2 handoff contract).
    let wr = struct_world_radius(&game.world.structs[i]);
    cam.len(wr).max(10.0)
}

/// A ring around a **contested** sub/structure, split into arcs whose angular spans are proportional
/// to each side's share of the ships present. `slices` are `(fraction, colour)` summing to ~1.0
/// (zero-fraction slices skipped); arcs are laid clockwise from the top. This replaces the old
/// ongoing-production indicator — it reads *presence balance*, not production.
fn draw_ownership_ring(cx: f32, cy: f32, r: f32, slices: &[(f32, Color)], alpha: f32) {
    let total_segs = 64.0;
    let start0 = -std::f32::consts::FRAC_PI_2; // top of the circle
    let mut acc = 0.0f32;
    for &(frac, col) in slices {
        if frac <= 0.0 {
            continue;
        }
        let a0 = start0 + acc * std::f32::consts::TAU;
        let a1 = start0 + (acc + frac) * std::f32::consts::TAU;
        let n = ((total_segs * frac).ceil() as i32).max(1);
        let c = fade(col, alpha);
        let mut prev: Option<(f32, f32)> = None;
        for k in 0..=n {
            let a = a0 + (a1 - a0) * (k as f32 / n as f32);
            let p = (cx + r * a.cos(), cy + r * a.sin());
            if let Some(pp) = prev {
                draw_line(pp.0, pp.1, p.0, p.1, 3.5, c);
            }
            prev = Some(p);
        }
        acc += frac;
    }
}


/// A small "siege" resistance bar below a sub. `frac` = resistance/max; `fill` = the holder's
/// colour; `eroding` (Some(attacker_col)) tints the lost slice + pulses the outline in the
/// attacker's colour; `healing` gives a green outline. Lets a capture be read in real time.
#[allow(clippy::too_many_arguments)]
fn draw_resistance_bar(cx: f32, top_y: f32, sub_r: f32, frac: f32, fill: Color, eroding: Option<Color>, healing: bool, alpha: f32, t: f32) {
    let frac = frac.clamp(0.0, 1.0);
    let w = (sub_r * 2.0).clamp(18.0, 90.0);
    let h = 4.5;
    let x = cx - w * 0.5;
    let y = top_y;
    draw_rectangle(x, y, w, h, fade(Color::new(0.04, 0.05, 0.08, 0.78), alpha));
    if let Some(atk) = eroding {
        // The depleted slice, in the attacker's colour: how much has been ground away.
        draw_rectangle(x + w * frac, y, w * (1.0 - frac), h, fade(Color::new(atk.r, atk.g, atk.b, 0.5), alpha));
    }
    draw_rectangle(x, y, w * frac, h, fade(fill, alpha));
    let (oc, ow) = if let Some(atk) = eroding {
        let pulse = 0.5 + 0.5 * (t * 7.0).sin();
        (Color::new(atk.r, atk.g, atk.b, 0.55 + 0.4 * pulse), 1.5)
    } else if healing {
        (Color::new(0.38, 0.9, 0.5, 0.75), 1.2)
    } else {
        (Color::new(1.0, 1.0, 1.0, 0.22), 1.0)
    };
    draw_rectangle_lines(x, y, w, h, ow, fade(oc, alpha));
}

/// `s` = the camera's px-per-world-unit scale: the triangle is a world-sized object, so its
/// apparent size follows the zoom (floored at [`SHIP_MIN_NOSE_PX`]).
fn draw_ship_triangle(cx: f32, cy: f32, ux: f32, uy: f32, col: Color, s: f32) {
    let nose = (SHIP_NOSE_WU * s).max(SHIP_MIN_NOSE_PX);
    draw_arrow_px(cx, cy, ux, uy, nose, col);
}

/// The ship-triangle shape at an explicit pixel size (`nose_px` tip length; back keeps the
/// ship's nose:back proportion). Shared by ships and the off-screen edge arrows.
fn draw_arrow_px(cx: f32, cy: f32, ux: f32, uy: f32, nose_px: f32, col: Color) {
    let nose = nose_px.max(SHIP_MIN_NOSE_PX);
    let back = nose * (SHIP_BACK_WU / SHIP_NOSE_WU);
    let (px, py) = (-uy, ux);
    let tip = Vec2::new(cx + ux * nose, cy + uy * nose);
    let l = Vec2::new(cx - ux * back + px * back, cy - uy * back + py * back);
    let r = Vec2::new(cx - ux * back - px * back, cy - uy * back - py * back);
    draw_triangle(tip, l, r, col);
}

/// The inset screen rectangle the edge arrows live on (inside the HUD bands), and its centre.
fn arrow_rect() -> (f32, f32, f32, f32, f32, f32) {
    let (sw, sh) = (screen_width(), screen_height());
    let (x0, x1) = (10.0, sw - 10.0);
    let (y0, y1) = (HUD_TOP_H + 10.0, sh - HUD_BOTTOM_H - 10.0);
    (x0, x1, y0, y1, (x0 + x1) * 0.5, (y0 + y1) * 0.5)
}

/// Clamp the ray centre→`(sx, sy)` onto the inset rect: where an off-screen thing's edge arrow
/// sits (it slides along the border as the thing moves). Returns the point + the outward unit
/// direction toward the thing.
fn edge_anchor(x0: f32, x1: f32, y0: f32, y1: f32, cx: f32, cy: f32, sx: f32, sy: f32) -> (f32, f32, f32, f32) {
    let (dx, dy) = (sx - cx, sy - cy);
    let tx = if dx > 0.0 {
        (x1 - cx) / dx
    } else if dx < 0.0 {
        (x0 - cx) / dx
    } else {
        f32::INFINITY
    };
    let ty = if dy > 0.0 {
        (y1 - cy) / dy
    } else if dy < 0.0 {
        (y0 - cy) / dy
    } else {
        f32::INFINITY
    };
    let t = tx.min(ty).max(0.0);
    let len = dx.hypot(dy).max(1e-3);
    (cx + dx * t, cy + dy * t, dx / len, dy / len)
}

/// Edge arrows for the focused struct's OFF-SCREEN ships: each renders as a ship-coloured arrow
/// on the screen border pointing at the ship, sized by how far past the edge it is — ship-sized
/// at near-visibility, shrinking linearly to 1/3 at one full struct-storage radius out (then
/// clamped). Ships on OTHER structs show nothing (the interior only ever draws the focused one).
fn draw_offscreen_ship_arrows(game: &Game, p: StructId, st: &layer1::Interior, cam: &Camera, ox: f32, oy: f32, alpha: f32) {
    let (x0, x1, y0, y1, rcx, rcy) = arrow_rect();
    // One full struct-storage radius past the edge ⇒ 1/3 size (the struct's own yardstick).
    let yard = st.storage_sub.map(|s| st.subs[s].radius).unwrap_or(60.0).max(1.0);
    // BATCHED into one mesh (chunked to the drawcall budget): a big off-screen stockpile is
    // thousands of arrows, and per-arrow draw_triangle calls were a per-frame lag source.
    SHIP_MESH.with(|m| {
        let mut mesh = m.borrow_mut();
        mesh.vertices.clear();
        mesh.indices.clear();
        for id in 0..st.ships.len() {
            let sh = &st.ships[id];
            if !sh.alive {
                continue;
            }
            let pos = game.ship_draw_pos(p, id);
            let (sx, sy) = cam.to_screen(ox + pos.x, oy + pos.y);
            if sx >= x0 && sx <= x1 && sy >= y0 && sy <= y1 {
                continue; // visible — the ship itself is on screen
            }
            let (ax, ay, ux, uy) = edge_anchor(x0, x1, y0, y1, rcx, rcy, sx, sy);
            let d_world = (sx - ax).hypot(sy - ay) / cam.scale.max(1e-6);
            let f = 1.0 - (d_world / yard).min(1.0) * (2.0 / 3.0);
            let mut col = fade(game.col(sh.faction), alpha);
            col.a *= SHIP_ALPHA;
            let nose = ((SHIP_NOSE_WU * cam.scale).max(SHIP_MIN_NOSE_PX) * f).max(SHIP_MIN_NOSE_PX);
            let back = nose * (SHIP_BACK_WU / SHIP_NOSE_WU);
            let (px, py) = (-uy, ux);
            if mesh.indices.len() + 3 > SHIP_MESH_ICAP {
                draw_mesh(&mesh);
                mesh.vertices.clear();
                mesh.indices.clear();
            }
            let base = mesh.vertices.len() as u16;
            mesh.vertices.push(Vertex::new(ax + ux * nose, ay + uy * nose, 0.0, 0.0, 0.0, col));
            mesh.vertices.push(Vertex::new(ax - ux * back + px * back, ay - uy * back + py * back, 0.0, 0.0, 0.0, col));
            mesh.vertices.push(Vertex::new(ax - ux * back - px * back, ay - uy * back - py * back, 0.0, 0.0, 0.0, col));
            mesh.indices.extend_from_slice(&[base, base + 1, base + 2]);
        }
        draw_mesh(&mesh);
    });
}

/// Lens-layer twin of [`draw_offscreen_ship_arrows`]: every struct whose node is fully off
/// screen gets an owner-coloured edge arrow pointing at it, node-sized at near-visibility and
/// shrinking to 1/3 one struct-storage radius past the edge.
fn draw_offscreen_struct_arrows(game: &Game, cam: &Camera, alpha: f32) {
    let (x0, x1, y0, y1, rcx, rcy) = arrow_rect();
    for i in 0..game.world.structs.len() {
        let structure = &game.world.structs[i];
        let (sx, sy) = cam.to_screen(structure.pos.x, structure.pos.y);
        let node_r = node_screen_radius(game, i, cam);
        if sx + node_r >= x0 && sx - node_r <= x1 && sy + node_r >= y0 && sy - node_r <= y1 {
            continue; // any part of the node is visible
        }
        let (ax, ay, ux, uy) = edge_anchor(x0, x1, y0, y1, rcx, rcy, sx, sy);
        let st = &structure.interior;
        let yard = st.storage_sub.map(|s| st.subs[s].radius).unwrap_or(60.0).max(1.0);
        let d_world = (sx - ax).hypot(sy - ay) / cam.scale.max(1e-6);
        let f = 1.0 - (d_world / yard).min(1.0) * (2.0 / 3.0);
        let mut col = fade(game.struct_col(game.world.struct_aggregate(i).owner), alpha);
        col.a *= SHIP_ALPHA;
        draw_arrow_px(ax, ay, ux, uy, node_r * 0.6 * f, col);
    }
}

// =============================================================================================
// Ship rendering — batched mesh + off-screen cull + density LOD (perf), and frame-timing.
// =============================================================================================

/// On-screen ships above which the interior ship view switches from individual meshed ships to a
/// **density LOD** (screen-grid blobs). Tuned high so normal play keeps crisp individual ships and
/// the aggregate only kicks in under extreme density. Raised 1500 → 6000 once the mesh was
/// chunked to macroquad's per-drawcall budget: a few thousand quads is a handful of draw calls,
/// and the blobs ("big-pixel ships") were tripping on ordinary 2000-ship reserve stockpiles.
const SHIP_DENSITY_THRESHOLD: usize = 6000;

/// Ship visual size in **world units** — apparent size scales with the camera (zoom in ⇒ bigger
/// ships, zoom out ⇒ smaller), like every other physical object on the board. Sized so the
/// default interior fit (~8 px/world-unit at the corrected game scale) reproduces the old
/// fixed-px look (nose ~3 px, idle dot ~1.2 px).
const SHIP_NOSE_WU: f32 = 0.40;
/// Triangle back half-width, world units (keeps the old 3.0 : 1.75 nose:back proportion).
const SHIP_BACK_WU: f32 = 0.235;
/// Idle-dot half-size, world units.
const SHIP_DOT_R_WU: f32 = 0.15;
/// Screen-px floor for the triangle nose (dot floors at half this) so extreme zoom-out leaves
/// ships faintly visible rather than sub-pixel-invisible.
const SHIP_MIN_NOSE_PX: f32 = 1.0;
/// Interior ships render at this alpha so OVERLAP reads as density: a loose garrison stays
/// airy, a packed brawl stacks toward opaque — the eye gets a head-count gradient the old
/// solid glyphs flattened.
const SHIP_ALPHA: f32 = 0.6;
/// Max **indices** per ship-mesh chunk. macroquad's internal batch is 10 000 vertices /
/// 5 000 indices PER DRAW CALL — a bigger `draw_mesh` triggers "geometry() exceeded max
/// drawcall size, clamping" and silently DROPS the excess geometry (ships past ~830 idle
/// quads simply vanished). Chunk under the index budget (the binding one: an idle quad is
/// 4 v / 6 i) with headroom.
const SHIP_MESH_ICAP: usize = 4_500;

thread_local! {
    /// Reused vertex/index buffers for the ship mesh (capacity retained across frames — no per-frame
    /// allocation). `texture: None` ⇒ macroquad binds its default white texture (flat-colour tris).
    static SHIP_MESH: std::cell::RefCell<Mesh> = std::cell::RefCell::new(Mesh {
        vertices: Vec::new(),
        indices: Vec::new(),
        texture: None,
    });
    /// Reused screen-grid accumulator for the density LOD: `(count, sum_r, sum_g, sum_b)` per cell.
    static SHIP_BIN: std::cell::RefCell<Vec<(u32, f32, f32, f32)>> = std::cell::RefCell::new(Vec::new());
    /// Sticky LOD mode for the meshed/binned switch (hysteresis — see [`draw_ships_interior`]).
    static SHIP_LOD_BINNED: std::cell::Cell<bool> = std::cell::Cell::new(false);
}

/// Draw the focused struct's ships: a single batched mesh (one `draw_mesh` per ≤64k-vertex chunk) —
/// idle ships as small quad dots, moving ships as forward-pointing triangles — with off-screen
/// culling. Above [`SHIP_DENSITY_THRESHOLD`] on-screen ships it aggregates into a screen-grid density
/// blob instead. Records the on-screen count + timing for the perf overlay.
fn draw_ships_interior(game: &Game, p: StructId, st: &layer1::Interior, cam: &Camera, ox: f32, oy: f32, alpha: f32) {
    let t0 = PerfInstant::now();
    let (sw, shh) = (screen_width(), screen_height());
    // Pass 1: count on-screen drawable ships (cheap; picks the render mode).
    let mut on_screen = 0usize;
    for sh in &st.ships {
        if !sh.alive {
            continue;
        }
        let (sx, sy) = cam.to_screen(ox + sh.pos.x, oy + sh.pos.y);
        if sx >= -8.0 && sx <= sw + 8.0 && sy >= -8.0 && sy <= shh + 8.0 {
            on_screen += 1;
        }
    }
    // Hysteresis around the threshold (switch up at ~1.07x, back down at ~0.93x) so a battle
    // hovering at the boundary does not flicker between crisp ships and density blobs each frame.
    let binned = SHIP_LOD_BINNED.with(|b| {
        let hi = SHIP_DENSITY_THRESHOLD + SHIP_DENSITY_THRESHOLD / 15;
        let lo = SHIP_DENSITY_THRESHOLD - SHIP_DENSITY_THRESHOLD / 15;
        let mode = if on_screen > hi {
            true
        } else if on_screen < lo {
            false
        } else {
            b.get() // inside the dead band: keep the previous mode
        };
        b.set(mode);
        mode
    });
    if binned {
        draw_ships_binned(game, p, st, cam, ox, oy, alpha, sw, shh);
    } else {
        draw_ships_meshed(game, p, st, cam, ox, oy, alpha, sw, shh);
    }
    PERF.with(|pf| {
        let mut pf = pf.borrow_mut();
        pf.ships_drawn = on_screen as u32;
        pf.ships_ms = ema(pf.ships_ms, t0.elapsed_ms());
    });
}

/// Individual ships, batched into one mesh: idle = quad dot, moving = forward triangle — both
/// **world-sized** ([`SHIP_DOT_R_WU`] / [`SHIP_NOSE_WU`]), so apparent size follows the zoom.
fn draw_ships_meshed(game: &Game, p: StructId, st: &layer1::Interior, cam: &Camera, ox: f32, oy: f32, alpha: f32, sw: f32, shh: f32) {
    SHIP_MESH.with(|m| {
        let mut mesh = m.borrow_mut();
        mesh.vertices.clear();
        mesh.indices.clear();
        for id in 0..st.ships.len() {
            let sh = &st.ships[id];
            if !sh.alive {
                continue;
            }
            let pos = game.ship_draw_pos(p, id);
            let (sx, sy) = cam.to_screen(ox + pos.x, oy + pos.y);
            if sx < -8.0 || sx > sw + 8.0 || sy < -8.0 || sy > shh + 8.0 {
                continue; // off-screen cull
            }
            let mut col = fade(game.col(sh.faction), alpha);
            col.a *= SHIP_ALPHA; // translucent ships: overlap stacks toward opaque = density
            if sh.drift_remaining > 0 {
                // Fade over the match's (scaled) drift duration — `drift_remaining` was set from
                // `game.sim.drift_ticks` (x24 at the GUI tick scale), so dividing by the unscaled
                // reference const would pin alpha >1 for ~95% of the drift and then pop.
                let full = game.sim.drift_ticks.max(1) as f32;
                col.a *= 0.8 * (sh.drift_remaining as f32 / full).min(1.0);
            }
            if mesh.indices.len() + 6 > SHIP_MESH_ICAP {
                draw_mesh(&mesh);
                mesh.vertices.clear();
                mesh.indices.clear();
            }
            let base = mesh.vertices.len() as u16;
            match sh.target {
                None => {
                    let r = (SHIP_DOT_R_WU * cam.scale).max(SHIP_MIN_NOSE_PX * 0.5);
                    mesh.vertices.push(Vertex::new(sx - r, sy - r, 0.0, 0.0, 0.0, col));
                    mesh.vertices.push(Vertex::new(sx + r, sy - r, 0.0, 0.0, 0.0, col));
                    mesh.vertices.push(Vertex::new(sx + r, sy + r, 0.0, 0.0, 0.0, col));
                    mesh.vertices.push(Vertex::new(sx - r, sy + r, 0.0, 0.0, 0.0, col));
                    mesh.indices.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
                }
                Some(_) => {
                    let dx = sh.aim.x - pos.x;
                    let dy = sh.aim.y - pos.y;
                    let len = (dx * dx + dy * dy).sqrt().max(1e-3);
                    let (ux, uy) = (dx / len, -dy / len); // screen-space (y down)
                    let (qx, qy) = (-uy, ux);
                    let nose = (SHIP_NOSE_WU * cam.scale).max(SHIP_MIN_NOSE_PX);
                    let back = nose * (SHIP_BACK_WU / SHIP_NOSE_WU);
                    mesh.vertices.push(Vertex::new(sx + ux * nose, sy + uy * nose, 0.0, 0.0, 0.0, col));
                    mesh.vertices.push(Vertex::new(sx - ux * back + qx * back, sy - uy * back + qy * back, 0.0, 0.0, 0.0, col));
                    mesh.vertices.push(Vertex::new(sx - ux * back - qx * back, sy - uy * back - qy * back, 0.0, 0.0, 0.0, col));
                    mesh.indices.extend_from_slice(&[base, base + 1, base + 2]);
                }
            }
        }
        draw_mesh(&mesh);
    });
}

/// Density LOD: bin on-screen ships into a coarse screen grid and draw one blob per cell (size +
/// opacity ∝ count, colour = mean of the cell's ships) in a single mesh. Aggregates extreme densities
/// while staying cheap (few cells, not N ships).
fn draw_ships_binned(game: &Game, p: StructId, st: &layer1::Interior, cam: &Camera, ox: f32, oy: f32, alpha: f32, sw: f32, shh: f32) {
    const CELL: f32 = 9.0;
    let cols = (sw / CELL).ceil() as usize + 1;
    let rows = (shh / CELL).ceil() as usize + 1;
    SHIP_BIN.with(|g| {
        let mut grid = g.borrow_mut();
        grid.clear();
        grid.resize(cols * rows, (0, 0.0, 0.0, 0.0));
        for id in 0..st.ships.len() {
            let sh = &st.ships[id];
            if !sh.alive {
                continue;
            }
            let pos = game.ship_draw_pos(p, id);
            let (sx, sy) = cam.to_screen(ox + pos.x, oy + pos.y);
            if sx < 0.0 || sx >= sw || sy < 0.0 || sy >= shh {
                continue;
            }
            let c = game.col(sh.faction);
            let e = &mut grid[(sy / CELL) as usize * cols + (sx / CELL) as usize];
            e.0 += 1;
            e.1 += c.r;
            e.2 += c.g;
            e.3 += c.b;
        }
        SHIP_MESH.with(|m| {
            let mut mesh = m.borrow_mut();
            mesh.vertices.clear();
            mesh.indices.clear();
            for cy in 0..rows {
                for cx in 0..cols {
                    let e = grid[cy * cols + cx];
                    if e.0 == 0 {
                        continue;
                    }
                    let n = e.0 as f32;
                    let col = Color::new(e.1 / n, e.2 / n, e.3 / n, (0.45 + 0.07 * n).min(0.95) * alpha);
                    let half = (1.6 + n.sqrt()).min(CELL * 0.7);
                    let (px, py) = (cx as f32 * CELL + CELL * 0.5, cy as f32 * CELL + CELL * 0.5);
                    if mesh.indices.len() + 6 > SHIP_MESH_ICAP {
                        draw_mesh(&mesh);
                        mesh.vertices.clear();
                        mesh.indices.clear();
                    }
                    let base = mesh.vertices.len() as u16;
                    mesh.vertices.push(Vertex::new(px - half, py - half, 0.0, 0.0, 0.0, col));
                    mesh.vertices.push(Vertex::new(px + half, py - half, 0.0, 0.0, 0.0, col));
                    mesh.vertices.push(Vertex::new(px + half, py + half, 0.0, 0.0, 0.0, col));
                    mesh.vertices.push(Vertex::new(px - half, py + half, 0.0, 0.0, 0.0, col));
                    mesh.indices.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
                }
            }
            draw_mesh(&mesh);
        });
    });
}

/// Lightweight frame-timing for the perf overlay (toggle **F3**). EMA-smoothed milliseconds per phase.
#[derive(Default, Clone, Copy)]
struct Perf {
    update_ms: f32, // app_update: input + sim drain + kill-fx
    draw_ms: f32,   // app_draw: the whole render
    ships_ms: f32,  // interior ship mesh build + draw
    killfx_ms: f32, // ship-death FX snapshot + diff
    ships_drawn: u32,
    show: bool,
}

thread_local! {
    static PERF: std::cell::RefCell<Perf> = std::cell::RefCell::new(Perf::default());
}

/// Exponential moving average (≈ last ~12 frames) so the overlay reads steady.
#[inline]
fn ema(prev: f32, sample: f32) -> f32 {
    if prev <= 0.0 {
        sample
    } else {
        prev * 0.92 + sample * 0.08
    }
}

/// A small ship cluster/stream for an inter-struct fleet. `s` = px per world unit (the flock's
/// ships AND its spread are world-sized, so the whole formation scales with the lens zoom).
fn draw_fleet_cluster(cx: f32, cy: f32, dir: (f32, f32), count: f64, col: Color, undock: bool, t: f32, s: f32) {
    let n = (count.sqrt().round() as i32).clamp(1, 6);
    let (ux, uy) = dir;
    let (px, py) = (-uy, ux);
    let alpha = if undock { 0.45 } else { 0.95 };
    let c = Color::new(col.r, col.g, col.b, col.a * alpha);
    let spread = (0.8 * s).max(2.0 * SHIP_MIN_NOSE_PX); // formation pitch, world-sized
    for k in 0..n {
        let along = (k as f32 - (n as f32 - 1.0) * 0.5) * spread;
        let across = ((k as f32 * 1.7 + t * 2.0).sin()) * spread * 0.5;
        draw_ship_triangle(cx + ux * along + px * across, cy + uy * along + py * across, ux, uy, c, s);
    }
    if count >= 4.0 {
        let label = (count.round() as i64).to_string();
        draw_text(&label, cx + 8.0, cy - 8.0, 15.0, Color::new(col.r, col.g, col.b, col.a * 0.85));
    }
}

/// Wrap `text` into lines that fit `width` px and draw them from `(x, y)`. Returns the next y.
fn wrap_text_block(text: &str, x: f32, y: f32, width: f32, size: u16, col: Color) -> f32 {
    let mut line = String::new();
    let mut yy = y;
    let lh = size as f32 * 1.3;
    for word in text.split_whitespace() {
        let trial = if line.is_empty() { word.to_string() } else { format!("{} {}", line, word) };
        if measure_text(&trial, None, size, 1.0).width > width && !line.is_empty() {
            draw_text(&line, x, yy, size as f32, col);
            yy += lh;
            line = word.to_string();
        } else {
            line = trial;
        }
    }
    if !line.is_empty() {
        draw_text(&line, x, yy, size as f32, col);
        yy += lh;
    }
    yy
}

// =============================================================================================
// HUD + overlays
// =============================================================================================

fn draw_hud(game: &Game) {
    let sw = screen_width();

    draw_rectangle(0.0, 0.0, sw, HUD_TOP_H, HUD_BG);

    let lay = topbar_layout(sw);

    // Speed controls (left): a discrete 1x/3x/10x/25x slider (pausing is the P overlay).
    draw_speed_slider(game, &lay);

    // Troop-send slider (1–100%).
    draw_troop_slider(game, &lay);

    // Count-up clock (top right). No countdown: a player match ends only by elimination.
    let secs = (game.world.tick as f64 / BASE_TICKS_PER_SEC).max(0.0) as u64;
    let clock = format!("{:02}:{:02}", secs / 60, secs % 60);
    let cd = measure_text(&clock, None, 24, 1.0);
    draw_text(&clock, sw - cd.width - 16.0, 34.0, 24.0, HUD_MUTED);
}

/// Topbar interactive-control geometry, derived purely from screen width so the draw code and the
/// input hit-testing always agree. Every rect lives inside the top `HUD_TOP_H` strip.
struct TopbarLayout {
    /// Discrete **speed** slider track (4 stops: 1x/3x/10x/25x); the value snaps to the nearest.
    speed_track: Rect,
    /// Padded grab area around the speed track.
    speed_hit: Rect,
    /// Visual troop-**send** slider track; the value maps linearly across its width (1 % at the left
    /// edge, 100 % at the right).
    slider_track: Rect,
    /// Padded grab area around the track so the handle is easy to catch.
    slider_hit: Rect,
}

fn topbar_layout(sw: f32) -> TopbarLayout {
    let by = 12.0;
    let bh = 30.0;
    let x0 = 16.0;
    // Discrete SPEED slider: a "Speed" label, then a 4-stop track (1x/3x/10x/25x).
    let speed_x = x0 + 58.0;
    let speed_w = 200.0;
    let speed_track = Rect::new(speed_x, by + bh * 0.5 - 4.0, speed_w, 8.0);
    let speed_hit = Rect::new(speed_x - 12.0, by - 6.0, speed_w + 24.0, bh + 12.0);
    // Troop "Send" slider, clear of the speed slider's rightmost "25x" label.
    let slider_x = speed_track.x + speed_w + 120.0;
    let slider_w = (sw * 0.5 - slider_x).clamp(150.0, 320.0);
    let slider_track = Rect::new(slider_x, by + bh * 0.5 - 4.0, slider_w, 8.0);
    let slider_hit = Rect::new(slider_x - 10.0, by - 6.0, slider_w + 20.0, bh + 12.0);
    TopbarLayout { speed_track, speed_hit, slider_track, slider_hit }
}

/// Handle pointer interaction with the topbar controls. Returns `true` if the controls consumed
/// this frame's pointer (so the caller skips board-order handling). Stays live in demo mode.
fn handle_topbar_input(game: &mut Game) -> bool {
    let lay = topbar_layout(screen_width());
    let (mx, my) = mouse_position();
    let pt = vec2(mx, my);

    // Continue an in-progress drag (speed or troop slider), even if the pointer left the strip.
    if game.dragging_speed {
        if is_mouse_button_down(MouseButton::Left) {
            set_speed_from_slider(game, &lay, mx);
        } else {
            game.dragging_speed = false;
        }
        return true;
    }
    if game.dragging_slider {
        if is_mouse_button_down(MouseButton::Left) {
            set_frac_from_slider(game, &lay, mx);
        } else {
            game.dragging_slider = false;
        }
        return true;
    }

    if is_mouse_button_pressed(MouseButton::Left) {
        if lay.speed_hit.contains(pt) {
            game.dragging_speed = true;
            set_speed_from_slider(game, &lay, mx);
            return true;
        }
        if lay.slider_hit.contains(pt) {
            game.dragging_slider = true;
            set_frac_from_slider(game, &lay, mx);
            return true;
        }
        // Any other press on the topbar strip is swallowed so it is not read as a board order.
        if my < HUD_TOP_H {
            return true;
        }
    }
    false
}

/// Map a pointer x-position to the 1..=100 send percent across the slider track.
fn set_frac_from_slider(game: &mut Game, lay: &TopbarLayout, mx: f32) {
    let t = ((mx - lay.slider_track.x) / lay.slider_track.w).clamp(0.0, 1.0);
    game.frac_pct = (1.0 + t * 99.0).round().clamp(1.0, 100.0) as u8;
}

/// Snap a pointer x-position to the nearest discrete **speed** stop (1x/3x/10x/25x).
fn set_speed_from_slider(game: &mut Game, lay: &TopbarLayout, mx: f32) {
    let last = SPEED_STEPS.len() - 1;
    let t = ((mx - lay.speed_track.x) / lay.speed_track.w).clamp(0.0, 1.0);
    game.speed_idx = ((t * last as f32).round() as usize).min(last);
    if game.speed_idx != 0 {
        game.speed_alt_idx = game.speed_idx; // the Space toggle's far side follows the slider
    }
}

/// The right-side vertical **zoom slider** track (top = most zoomed in). No label.
fn zoom_slider_rect() -> Rect {
    let w = 8.0;
    let x = screen_width() - 22.0;
    let top = HUD_TOP_H + 30.0;
    let bottom = screen_height() - HUD_BOTTOM_H - 30.0;
    Rect::new(x, top, w, (bottom - top).max(40.0))
}

/// Handle the zoom slider; returns `true` if it consumed this frame's pointer (so no board order).
fn handle_zoom_slider(game: &mut Game) -> bool {
    let track = zoom_slider_rect();
    let (mx, my) = mouse_position();
    if game.dragging_zoom {
        if is_mouse_button_down(MouseButton::Left) {
            set_zoom_from_slider(game, &track, my);
            return true;
        }
        game.dragging_zoom = false;
        return true;
    }
    let hit = Rect::new(track.x - 12.0, track.y - 8.0, track.w + 24.0, track.h + 16.0);
    if is_mouse_button_pressed(MouseButton::Left) && hit.contains(vec2(mx, my)) {
        game.dragging_zoom = true;
        set_zoom_from_slider(game, &track, my);
        return true;
    }
    false
}

/// Map a pointer y to the current layer's zoom value (top of the track = the effective
/// in-zoom ceiling — per-struct overrides included).
fn set_zoom_from_slider(game: &mut Game, track: &Rect, my: f32) {
    let f = (1.0 - (my - track.y) / track.h).clamp(0.0, 1.0);
    let zmin = game.zoom_min();
    let v = zmin + f * (game.zoom_max() - zmin);
    let layer = game.zoom_layer();
    game.zoom[layer] = v;
}

/// Draw the right-side zoom slider (no label) — track + handle at the current layer's value.
fn draw_zoom_slider(game: &Game) {
    let track = zoom_slider_rect();
    draw_rectangle(track.x, track.y, track.w, track.h, Color::new(0.14, 0.16, 0.20, 0.85));
    draw_rectangle_lines(track.x, track.y, track.w, track.h, 1.0, Color::new(0.30, 0.34, 0.40, 0.85));
    let v = game.zoom[game.zoom_layer()];
    let zmin = game.zoom_min();
    let f = ((v - zmin) / (game.zoom_max() - zmin).max(1e-6)).clamp(0.0, 1.0);
    let hy = track.y + (1.0 - f) * track.h;
    draw_rectangle(track.x - 4.0, hy - 5.0, track.w + 8.0, 10.0, PLAYER);
}

/// Draw the discrete **speed** slider: a "Speed" label, a track with a tick + multiplier label at
/// each of the 4 stops (1x/3x/10x/25x), and a handle on the current stop. The active stop is
/// accented; the handle greys out while the `P` overlay pause holds the clock.
fn draw_speed_slider(game: &Game, lay: &TopbarLayout) {
    let t = lay.speed_track;
    // "Speed" label to the left of the track.
    let label = "Speed";
    let ld = measure_text(label, None, 18, 1.0);
    draw_text(label, t.x - ld.width - 10.0, t.y + 7.0, 18.0, HUD_MUTED);
    // Track.
    draw_rectangle(t.x, t.y, t.w, t.h, Color::new(0.16, 0.18, 0.22, 0.95));
    let last = (SPEED_STEPS.len() - 1) as f32;
    // Stops: a tick + a centred multiplier label under each; the active stop is accented.
    for (i, &m) in SPEED_STEPS.iter().enumerate() {
        let cx = t.x + (i as f32 / last) * t.w;
        let active = i == game.speed_idx;
        let col = if active { ACCENT } else { HUD_MUTED };
        draw_rectangle(cx - 1.0, t.y - 3.0, 2.0, t.h + 6.0, col);
        let s = format!("{}x", fmt_speed(m));
        let d = measure_text(&s, None, 15, 1.0);
        draw_text(&s, cx - d.width * 0.5, t.y + 28.0, 15.0, col);
    }
    // Handle on the current stop.
    let hx = t.x + (game.speed_idx as f32 / last) * t.w;
    let hcol = if game.paused() { HUD_MUTED } else { PLAYER };
    draw_rectangle(hx - 4.0, t.y - 6.0, 8.0, t.h + 12.0, hcol);
}

fn draw_troop_slider(game: &Game, lay: &TopbarLayout) {
    let t = lay.slider_track;
    // "Send" label to the left of the track.
    let label = "Send";
    let ld = measure_text(label, None, 18, 1.0);
    draw_text(label, t.x - ld.width - 10.0, t.y + 7.0, 18.0, HUD_MUTED);
    // Track + filled portion.
    draw_rectangle(t.x, t.y, t.w, t.h, Color::new(0.16, 0.18, 0.22, 0.95));
    let frac = (game.frac_pct.clamp(1, 100) as f32 - 1.0) / 99.0;
    let fillw = t.w * frac;
    draw_rectangle(t.x, t.y, fillw, t.h, PLAYER_DIM);
    // Handle.
    let hx = t.x + fillw;
    draw_rectangle(hx - 3.0, t.y - 5.0, 6.0, t.h + 10.0, PLAYER);
    // Value readout to the right.
    let val = format!("{}%", game.frac_pct);
    draw_text(&val, t.x + t.w + 10.0, t.y + 7.0, 18.0, HUD_TEXT);
}

fn draw_intro_overlay(game: &Game) {
    let sw = screen_width();
    let sh = screen_height();
    draw_rectangle(0.0, 0.0, sw, sh, Color::new(0.0, 0.0, 0.0, 0.66));

    let box_w = (sw * 0.7).min(820.0);
    let box_x = (sw - box_w) * 0.5;
    let mut y = sh * 0.22;

    let title = format!("Level {} — {}", game.level.id, game.level.title);
    draw_centered(&title, y, 44, ACCENT);
    y += 48.0;

    y = wrap_text_block(&game.level.blurb, box_x, y + 8.0, box_w, 22, HUD_TEXT) + 12.0;

    draw_text("OBJECTIVE", box_x, y, 20.0, PLAYER);
    y += 26.0;
    y = wrap_text_block(&game.level.objective, box_x, y, box_w, 20, HUD_TEXT) + 12.0;

    draw_text("TIPS", box_x, y, 20.0, PLAYER);
    y += 26.0;
    for hint in &game.level.hints {
        y = wrap_text_block(&format!("- {}", hint), box_x, y, box_w, 18, HUD_MUTED);
    }

    draw_centered("Enter / click to begin", sh - 70.0, 24, ACCENT);
}

fn draw_end_banner(game: &Game) {
    let sw = screen_width();
    let sh = screen_height();
    let winner = game.finished.unwrap_or(Faction::Neutral);
    let (text, col) = match winner {
        Faction::Player => ("VICTORY", PLAYER),
        // `outcome()` reports a player defeat as `Enemy`; `Enemy2` is here only for exhaustiveness.
        Faction::Ai(_) => ("DEFEAT", ENEMY),
        Faction::Neutral => ("DRAW", NEUTRAL),
    };
    draw_rectangle(0.0, 0.0, sw, sh, Color::new(0.0, 0.0, 0.0, 0.6));
    draw_centered(text, menu_first_y() - menu_pitch() * 0.8, 72, col);
    if game.replay.is_some() {
        // PLAYBACK end overlay: the recorded outcome, replay controls only.
        draw_overlay_buttons(&["Watch again", "Main Menu"]);
    } else if winner == Faction::Player {
        // VICTORY MENU (owner, 2026-07-10): Next level / [Watch replay | Save replay] /
        // Restart / Main Menu — row 1 is split vertically (owner ask): watching on the
        // left, a manual shelf-save on the right ("Saved" once pressed).
        draw_overlay_buttons(&["Next level", "", "Restart", "Main Menu"]);
        let (x, y, w, h) = menu_item_rect(1);
        let half = w * 0.5 - 3.0;
        let (mx, my) = mouse_position();
        let save_label = if game.manual_saved { "Saved" } else { "Save replay" };
        for (bx, label) in [(x, "Watch replay"), (x + w * 0.5 + 3.0, save_label)] {
            let hov = mx >= bx && mx <= bx + half && my >= y && my <= y + h;
            let bg = if hov { Color::new(0.12, 0.16, 0.22, 0.95) } else { Color::new(0.07, 0.09, 0.13, 0.9) };
            draw_rectangle(bx, y, half, h, bg);
            draw_rectangle_lines(bx, y, half, h, 2.0, if hov { PLAYER } else { EDGE_COL });
            let d = measure_text(label, None, 24, 1.0);
            draw_text(label, bx + half * 0.5 - d.width * 0.5, y + h * 0.66, 24.0, if hov { HUD_TEXT } else { HUD_MUTED });
        }
    } else {
        // DEFEAT/DRAW MENU (owner follow-up, same day): Retry folds the pause menu's
        // Resume and Restart into one (they'd be the same action here); R still retries.
        draw_overlay_buttons(&["Retry", "Watch replay", "Main Menu"]);
    }
}

fn fmt_speed(s: f64) -> String {
    if s.fract().abs() < 1e-6 {
        format!("{}", s as i64)
    } else {
        format!("{}", s)
    }
}

// =============================================================================================
// Entry point + the shot (headless verification) path
// =============================================================================================

/// Load `--replay <file>` into a playback Game, if requested: parse the `.mir`, look the
/// level up by id, warn loudly on stamp mismatches (the checkpoints will catch actual
/// divergence), and build the playback.
fn load_replay_game(cfg: &Config, levels: &[Level]) -> Option<Game> {
    load_replay_from(cfg.replay.as_ref()?, levels)
}

/// Load any `.mir` path into a playback Game (the CLI and the replay browser both land
/// here): parse, look the level up, warn loudly on stamp mismatches, build the playback.
fn load_replay_from(path: &str, levels: &[Level]) -> Option<Game> {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("[game] --replay {path}: {e}");
            return None;
        }
    };
    let rf = match replay::parse(&text) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("[game] --replay {path}: {e}");
            return None;
        }
    };
    let Some(lvl) = levels.iter().find(|l| l.id == rf.level_id) else {
        eprintln!("[game] --replay: level {} is not in this campaign", rf.level_id);
        return None;
    };
    if lvl.content_hash != rf.level_hash {
        eprintln!("[game] WARNING: level content differs from the recording — expect divergence");
    }
    if rf.git != env!("GIT_HASH") {
        eprintln!(
            "[game] WARNING: replay recorded on build {} (this is {}) — expect divergence",
            rf.git,
            env!("GIT_HASH")
        );
    }
    Some(Game::new_replay(
        lvl.clone(),
        rf.seed,
        rf.scale,
        rf.orders,
        rf.checkpoints,
        rf.end_tick,
        rf.winner,
    ))
}

fn window_conf() -> Conf {
    Conf {
        window_title: "Machine Intelligence — cell-game RTS".to_owned(),
        window_width: 1280,
        window_height: 800,
        high_dpi: true,
        ..Default::default()
    }
}

/// Reattach to the launching console, if any. The binary is a WINDOWS-subsystem app so
/// double-clicking (or play-dev's `start`) spawns no black console window (owner report:
/// closing that window killed the game) — but the CLI workflows (--selftest, --shot,
/// --replay) still print: launched from a terminal, we attach back to it here, before any
/// output. Piped/redirected output is unaffected either way (handles are inherited).
#[cfg(windows)]
fn attach_parent_console() {
    extern "system" {
        fn AttachConsole(pid: u32) -> i32;
    }
    unsafe {
        AttachConsole(u32::MAX); // ATTACH_PARENT_PROCESS
    }
}

#[macroquad::main(window_conf)]
async fn main() {
    #[cfg(windows)]
    attach_parent_console();
    let cfg = parse_config();

    // --- Reset saved state (progress + notes), then continue fresh ---
    if cfg.reset {
        reset_all_progress();
    }

    // --- Headless verification (one frame, then exit) ---
    if let Mode::Shot { .. } = &cfg.mode {
        run_shot(&cfg).await;
        return;
    }

    // --- Headless game-loop self-test (logic only, then exit) ---
    if cfg.selftest {
        let ok = run_selftest();
        use std::io::Write;
        let _ = std::io::stdout().flush(); // block-buffered when piped; flush before hard exit
        std::process::exit(if ok { 0 } else { 1 });
    }

    // --- Interactive app ---
    if let Some((w, h)) = cfg.win {
        request_new_screen_size(w as f32, h as f32);
    }
    let mut app = App::new(&cfg);
    if let Some(g) = load_replay_game(&cfg, &app.levels) {
        app.state = AppState::InLevel { game: Box::new(g) };
    }
    loop {
        let dt = get_frame_time() as f64;
        // F3 toggles the frame-timing overlay (perf instrumentation).
        if act_pressed(Action::PerfOverlay) {
            PERF.with(|p| {
                let mut p = p.borrow_mut();
                p.show = !p.show;
            });
        }
        let t_upd = PerfInstant::now();
        let quit = app_update(&mut app, dt);
        let upd_ms = t_upd.elapsed_ms();
        let t_draw = PerfInstant::now();
        app_draw(&app);
        let draw_ms = t_draw.elapsed_ms();
        PERF.with(|p| {
            let mut p = p.borrow_mut();
            p.update_ms = ema(p.update_ms, upd_ms);
            p.draw_ms = ema(p.draw_ms, draw_ms);
        });
        if quit {
            break;
        }
        next_frame().await;
    }
}

/// Headless capture: build the requested screen/level, advance the sim to the target tick if
/// needed, render a couple of settle frames, write the PNG, and exit. Mirrors the layer1/2-game
/// shot state machines.
async fn run_shot(cfg: &Config) {
    let Mode::Shot { path, screen, level, view, at_tick, zoom, pan, arena, auto, dump } = &cfg.mode else {
        return;
    };

    // Build the app state we want to capture.
    let mut app = App::new(&Config {
        mode: Mode::Human,
        seed: cfg.seed,
        unlock_all: true, // so a --level shot is never gated
        start_level: None,
        auto: *auto,
        selftest: false,
        reset: false,
        text: false,
        win: None,
        replay: None,
    });

    match screen {
        Some(ScreenTarget::Menu) => {
            app.state = AppState::MainMenu { idx: 0 };
        }
        Some(ScreenTarget::Select) => {
            app.state = AppState::LevelSelect { idx: 0 };
        }
        Some(ScreenTarget::Settings) => {
            app.state = AppState::Settings { idx: 0, capturing: false };
        }
        Some(ScreenTarget::Memory) => {
            app.replay_sections = scan_replays(&app.levels);
            // Universal + L01 sections pre-expanded so a capture shows entries.
            app.state = AppState::Memory { tab: 0, idx: 0, expanded: 0b11, scroll: 0 };
        }
        None => {
            // A level shot (default to level 1 if unspecified) — or the reserve-combat arena.
            let lvl = if *arena {
                arena_level()
            } else {
                let idx = level.unwrap_or(1).saturating_sub(1).min(app.levels.len() - 1);
                app.levels[idx].clone()
            };
            let force_view = *view;
            let mut game = match load_replay_game(cfg, &app.levels) {
                // `--shot --replay`: headless PLAYBACK — the advance loop feeds the journal
                // and verifies checkpoints; the summary line below reports divergence.
                Some(g) => g,
                None => Game::new(lvl, cfg.seed, *auto, force_view, TICK_SCALE),
            };
            game.show_intro = false; // capture the board, not the overlay
            // Advance to the target tick (deterministically, independent of frame timing). Stops
            // early if the match ends before `at_tick`. SHOT_TICKS_PER_FRAME just bounds the inner
            // chunk; the loop runs until the target tick is reached.
            while game.world.tick < *at_tick && !game.match_over() {
                let mut budget = SHOT_TICKS_PER_FRAME;
                while budget > 0 && game.world.tick < *at_tick && !game.match_over() {
                    game.step_one_tick();
                    budget -= 1;
                }
            }
            if let Some(rs) = &game.replay {
                println!(
                    "[game] replay playback: tick {} / {}  diverged={}",
                    game.world.tick, rs.end_tick, rs.diverged
                );
            }
            // The numeric half of the shot: the focused struct's raw ship table at this tick.
            if let Some(dump_path) = dump {
                let sid = match game.view {
                    View::Interior(sid) => sid,
                    _ => 0,
                };
                let interior = &game.world.structs[sid].interior;
                let mut csv = String::from(
                    "ship,faction,home,alive,in_transit,undock,drift,x,y,angle,ring_offset\n",
                );
                for (i, s) in interior.ships.iter().enumerate() {
                    csv.push_str(&format!(
                        "{},{:?},{},{},{},{},{},{:.3},{:.3},{:.4},{:.4}\n",
                        i,
                        s.faction,
                        s.home,
                        s.alive as u8,
                        s.target.is_some() as u8,
                        s.undock_remaining,
                        s.drift_remaining,
                        s.pos.x,
                        s.pos.y,
                        s.angle,
                        s.ring_offset,
                    ));
                }
                if let Err(e) = std::fs::write(dump_path, csv) {
                    eprintln!("[game] dump FAILED: {dump_path}: {e}");
                } else {
                    println!("[game] dump written: {dump_path} @tick {}", game.world.tick);
                }
            }
            game.render_alpha = 1.0;
            // Snap the camera to its target (no easing in a single-frame capture).
            game.cam_t = if matches!(game.view, View::Interior(_)) { 1.0 } else { 0.0 };
            // Aim the shot camera (--zoom / --pan): the screenshot workflow's wheel + drag.
            let layer = game.zoom_layer();
            if let Some(z) = zoom {
                game.zoom[layer.min(1)] = *z;
            }
            if let Some((px, py)) = pan {
                game.pan[layer.min(1)] = (*px, *py);
            }
            app.state = AppState::InLevel { game: Box::new(game) };
        }
    }

    // `--win`: resize before the settle frames, so the capture sees the final geometry (the
    // resize takes effect across a frame swap — the settle loop absorbs it).
    if let Some((w, h)) = cfg.win {
        request_new_screen_size(w as f32, h as f32);
    }

    // Render a few settle frames so the framebuffer + pulsing effects are fully drawn. Each
    // `next_frame().await` presents (swaps) the buffer, so we draw, present, repeat — then draw
    // ONE more time and grab the framebuffer *before* the next swap (otherwise `get_screen_data`
    // reads the freshly-cleared back buffer and the PNG comes out black).
    for _ in 0..(SHOT_SETTLE_FRAMES + 2) {
        app_draw(&app);
        next_frame().await;
    }
    app_draw(&app);

    let img = get_screen_data();
    if let Some(parent) = std::path::Path::new(path).parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    img.export_png(path);
    let what = match screen {
        Some(ScreenTarget::Menu) => "menu".to_string(),
        Some(ScreenTarget::Select) => "select".to_string(),
        Some(ScreenTarget::Settings) => "settings".to_string(),
        Some(ScreenTarget::Memory) => "memory".to_string(),
        None => format!("level {} @tick {}", level.unwrap_or(1), at_tick),
    };
    println!(
        "[game] shot written: {} ({}x{})  {}  seed={}",
        path,
        img.width(),
        img.height(),
        what,
        cfg.seed,
    );
}

// =============================================================================================
// Headless self-test (--selftest)
// =============================================================================================
//
// Exercises the SAME `Game` + `step_one_tick` loop the interactive app runs, so it covers the
// game-specific wiring (decision cadence, enemy + player-automation application, outcome
// latching) that the world/ai/levels unit tests do not reach. It touches no macroquad rendering,
// and runs fine from the RELEASE binary — sidestepping the Windows app-control block that stops
// freshly-linked `cargo test` binaries from launching under the Desktop tree.

/// Total sub-structures the player owns across every struct (the Layer-2 ownership signal).
fn total_player_subs(w: &World) -> usize {
    (0..w.structs.len()).map(|p| w.struct_aggregate(p).player_subs).sum()
}

/// Drive a level with BOTH seats AI to the end; return (final state hash, latched winner, end tick).
fn selftest_run_to_end(level: Level, seed: u64) -> (u64, Option<Faction>, u64) {
    // PASSIVE player seat: the greedy player-proxy was removed from the tests (owner,
    // 2026-07-10) — a proxy match's outcome measures the proxy, not the game, and printing
    // it dressed non-information up as a result. The declared enemy seats still act, which
    // is all the determinism property needs.
    let mut g = Game::new(level, seed, false, None, 1.0); // headless: coarse reference resolution
    // Stop at the capped budget (or earlier if the match seals). Determinism is the property we
    // assert; the full sealed outcome of the long levels lives in an uncapped run.
    let cap = scaled_horizon(&g.level, g.scale).min(HEADLESS_TICK_CAP);
    while !g.match_over() && g.world.tick < cap {
        g.step_one_tick();
    }
    (g.world.state_hash(), g.finished, g.world.tick)
}

/// A synthetic single-struct world for the automation self-test: a stocked Player centre ringed by
/// cheap-to-capture neutral posts to expand into. The campaign no longer fields a
/// player-home-with-internal-neutrals level, so this test owns its own scenario (it exercises the
/// greedy automation adapter, not any particular level).
fn selftest_auto_world(seed: u64) -> (World, WorldParams) {
    let mut w = World::new();
    let mut st = layer1::Interior::new(seed);
    let c = st.add_sub(
        layer1::SubStructure::new(layer1::Vec2::new(0.0, 0.0), 0.0, Faction::Player)
            .with_storage_capacity(60)
            .with_production(2),
    );
    for _ in 0..60 {
        st.spawn_ship(Faction::Player, c);
    }
    for i in 0..5 {
        let ang = i as f32 / 5.0 * std::f32::consts::TAU;
        st.add_sub(
            layer1::SubStructure::new(layer1::Vec2::new(9.0 * ang.cos(), 9.0 * ang.sin()), 0.0, Faction::Neutral)
                .with_storage_capacity(30)
                .with_max_resistance(60.0), // cheap to capture — tests the adapter, not the grind
        );
    }
    st.add_storage_sub();
    w.add_struct(world::Structure::new(st, layer1::Vec2::new(0.0, 0.0), "Auto Test"));
    (w, WorldParams::default())
}

/// Run the headless game-loop self-test. Returns true iff every check passed.
fn run_selftest() -> bool {
    let mut all_ok = true;
    let seed: u64 = 0xCE11_2042;
    println!("== game self-test (headless game-loop coverage) ==");

    // (1) Every campaign level: an enemy-seats match over a passive player terminates (or
    //     runs out its capped budget) and is deterministic on a rerun (same seed ->
    //     identical final state + end).
    for level in levels::campaign() {
        let id = level.id;
        let title = level.title.clone();
        let cap = scaled_horizon(&level, 1.0).min(HEADLESS_TICK_CAP); // selftest runs headless at scale 1.0
        let (h1, fin1, t1) = selftest_run_to_end(level.clone(), seed);
        let (h2, fin2, t2) = selftest_run_to_end(level, seed);
        // Ran the whole capped budget (or sealed earlier) — the loop made progress and stopped cleanly.
        let progressed = fin1.is_some() || t1 >= cap;
        // The property under test: same seed -> identical evolution (state + outcome + end tick).
        let deterministic = h1 == h2 && fin1 == fin2 && t1 == t2;
        let ok = progressed && deterministic;
        all_ok &= ok;
        let outcome = match fin1 {
            Some(f) => format!("sealed:{f:?}"),
            None => "(capped)".to_string(),
        };
        println!(
            "  L{:>2} {:<14} tick={}/{} outcome={} det={} -> {}",
            id, title, t1, cap, outcome, deterministic,
            if ok { "PASS" } else { "FAIL" }
        );
    }

    // (2) Player basic-automation issues effective orders: on the synthetic player-centre +
    //     neutral-ring world, a player that does NOTHING holds at its starting sub, but the same
    //     player with every owned struct set to AUTO expands (captures posts) via the greedy adapter.
    {
        let auto_level = Level {
            id: 0,
            title: "Auto Test".into(),
            blurb: String::new(),
            objective: String::new(),
            hints: Vec::new(),
            enemies: vec![ai::Roster::Passive],
            start_view: StartView::Layer1(0),
            automation_available: true,
            horizon: 1200,
            zoom_min: None,
            zoom_start: None,
            briefing: None,
            post_log: None,
            source: levels::LevelSource::Builtin(selftest_auto_world),
            content_hash: 0,
        };
        let measure = |automate: bool| -> (usize, usize) {
            let mut g = Game::new(auto_level.clone(), seed, false, None, 1.0); // headless: coarse resolution
            if automate {
                for s in g.automated.iter_mut() {
                    *s = true;
                }
            }
            let start = total_player_subs(&g.world);
            let mut peak = start;
            // Run a fixed window (NOT gated on `match_over`): with no real enemy seat the player has
            // already "won" at tick 0, so a `match_over` gate would never let the loop run.
            let cap = scaled_horizon(&g.level, g.scale).min(HEADLESS_TICK_CAP);
            while g.world.tick < cap {
                g.step_one_tick();
                peak = peak.max(total_player_subs(&g.world));
            }
            (start, peak)
        };
        let (start_off, peak_off) = measure(false);
        let (_start_on, peak_on) = measure(true);
        let ok = peak_on > peak_off && peak_on > start_off;
        all_ok &= ok;
        println!(
            "  AUTOMATION (synthetic) player subs: start={} idle-peak={} auto-peak={} -> {}",
            start_off, peak_off, peak_on,
            if ok { "PASS" } else { "FAIL" }
        );
    }

    println!("== self-test: {} ==", if all_ok { "ALL PASS" } else { "FAILURES PRESENT" });
    all_ok
}
