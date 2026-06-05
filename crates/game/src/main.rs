//! # game — the complete single-binary v1 cell-game RTS (the PLAYABLE product)
//!
//! This is the macroquad GUI that assembles the headless substrate into the actual game. Per the
//! design's signature principle (`00-overview.md`, *decouple computation from spectacle*): **all**
//! model logic lives in the headless crates ([`levels`] builds the world; [`world`] steps it;
//! [`ai`] drives the enemy and the player's optional automation; [`layer1`] is the per-planet
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
//! * **Layer-2 lens** (zoomed out) — planets as nodes coloured by [`world::PlanetAggregate`]
//!   owner, lanes, and inter-planet [`world::InterFleet`]s interpolated along lanes. Click an
//!   owned planet then a lane-connected planet to issue a [`world::FleetOrder`].
//! * **Layer-1 interior** (zoomed into one planet) — that planet's [`layer1::Structure`] drawn
//!   exactly like the standalone Layer-1 game (sub-structures, ships, battle bubbles). Click a
//!   sub then another sub to issue a [`layer1::MoveOrder`].
//! The camera lerps smoothly between the two. `automation_available` levels let the player toggle
//! AUTO on an owned planet (key `A`) so its internals are driven by the Layer-1 greedy adapter —
//! the same policy the enemy uses.
//!
//! ## Verification modes (screenshot-checkable, mirrors layer1/2-game)
//! `--shot <path>` render one frame and exit; `--screen <menu|select>`; `--level <N>`;
//! `--view <lens|interior>`; `--at-tick <T>`; `--auto` (both seats AI); `--seed <S>`.

use ai::{AiController, GreedyParams};
use layer1::{Faction, FractionBucket, MoveOrder, SimParams};
use levels::{campaign, Level, StartView};
use macroquad::prelude::*;
use world::{FleetOrder, PlanetId, PlanetOwner, World, WorldParams};

// =============================================================================================
// Pacing & tuning constants (the spectacle layer's operating point — see GAME.md)
// =============================================================================================

/// Base world rate in **ticks per (real) second** at 1x speed. ~5 t/s lands watchable matches
/// (mirrors layer2-game), with render-side interpolation keeping motion fluid between ticks.
const BASE_TICKS_PER_SEC: f64 = 5.0;

/// Available speed multipliers (cycled by -/+ and [ ]). 1x = [`BASE_TICKS_PER_SEC`].
const SPEED_STEPS: [f64; 7] = [0.5, 1.0, 1.5, 2.0, 3.0, 4.0, 6.0];
/// Index into [`SPEED_STEPS`] the game starts at (1.0x).
const DEFAULT_SPEED_IDX: usize = 1;

/// Safety bound on world ticks advanced in a single rendered frame (so a stall / huge speed can
/// never spiral the sim arbitrarily far in one frame).
const MAX_TICKS_PER_FRAME: u32 = 8;

/// Run the seat decisions (enemy AI + player automation) every this-many ticks (matches the
/// headless cadence). Forces commit over time instead of re-planning every tick.
const DECISION_INTERVAL: u64 = 5;

/// Ticks the `--shot --at-tick` capture advances per frame while racing to its target tick
/// (independent of wall-clock pacing, so capture is fast and deterministic).
const SHOT_TICKS_PER_FRAME: u64 = 8;
/// Extra frames rendered after reaching the capture point so the framebuffer is fully drawn.
const SHOT_SETTLE_FRAMES: u32 = 2;

/// Camera zoom-lerp speed (fraction toward target per second-ish; applied via exp smoothing).
const ZOOM_LERP_RATE: f32 = 7.0;
/// Scale (lens-world-units -> pixels multiplier relative to a single planet's local span) the
/// interior view zooms in to. Larger = the planet fills more of the screen.
const INTERIOR_FILL: f32 = 0.80;
/// `cam_t` (0 = full lens, 1 = full interior) above which the interior scene is drawn.
const INTERIOR_DRAW_THRESHOLD: f32 = 0.55;

// =============================================================================================
// Palette (the shared dark minimalist look from layer1/2-game)
// =============================================================================================

const BG: Color = Color::new(0.043, 0.055, 0.078, 1.0);
const GRID: Color = Color::new(0.10, 0.12, 0.16, 1.0);

const PLAYER: Color = Color::new(0.30, 0.72, 1.00, 1.0); // cyan/blue (Player seat)
const PLAYER_DIM: Color = Color::new(0.18, 0.42, 0.62, 1.0);
const ENEMY: Color = Color::new(1.00, 0.42, 0.30, 1.0); // red/orange (Enemy seat)
const ENEMY_DIM: Color = Color::new(0.62, 0.26, 0.20, 1.0);
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
        Faction::Enemy => ENEMY,
        Faction::Neutral => NEUTRAL,
    }
}
fn faction_dim(f: Faction) -> Color {
    match f {
        Faction::Player => PLAYER_DIM,
        Faction::Enemy => ENEMY_DIM,
        Faction::Neutral => NEUTRAL_DIM,
    }
}

/// The lens colour of a planet from its [`world::PlanetAggregate`] owner.
fn planet_color(owner: PlanetOwner) -> Color {
    match owner {
        PlanetOwner::Owned(f) => faction_color(f),
        PlanetOwner::Neutral => NEUTRAL,
        PlanetOwner::Contested => ACCENT, // striped/mixed; base drawn warm, overlaid below
    }
}

// =============================================================================================
// Run mode & CLI config
// =============================================================================================

/// Which menu a `--screen` shot targets.
#[derive(Clone, Copy, PartialEq)]
enum ScreenTarget {
    Menu,
    Select,
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
        auto: bool,
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

    // Shot sub-config (only meaningful with --shot).
    let mut shot_path: Option<String> = None;
    let mut screen: Option<ScreenTarget> = None;
    let mut level: Option<usize> = None;
    let mut view: Option<ViewTarget> = None;
    let mut at_tick: u64 = 0;

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
            "--auto" => {
                auto = true;
            }
            "--unlock-all" => {
                unlock_all = true;
            }
            "--selftest" => {
                selftest = true;
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
        Some(path) => Mode::Shot { path, screen, level, view, at_tick, auto },
        None => Mode::Human,
    };
    Config { mode, seed, unlock_all, start_level, auto, selftest }
}

// =============================================================================================
// Progress persistence (mi_progress.json next to the exe)
// =============================================================================================

/// Path to the progress file: next to the executable if we can find it, else the CWD.
fn progress_path() -> std::path::PathBuf {
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            return dir.join("mi_progress.json");
        }
    }
    std::path::PathBuf::from("mi_progress.json")
}

/// The highest unlocked level (1-based). Level 1 is always unlocked. Stored as a tiny JSON
/// object `{"unlocked": N}`. We hand-roll the (de)serialization to keep this crate's dependency
/// set to the substrate + macroquad (no serde).
fn load_unlocked() -> u32 {
    let path = progress_path();
    let Ok(text) = std::fs::read_to_string(&path) else {
        return 1;
    };
    parse_unlocked(&text).unwrap_or(1).max(1)
}

/// Pull the integer after `"unlocked"` out of the tiny JSON blob. Tolerant of whitespace.
fn parse_unlocked(text: &str) -> Option<u32> {
    let key = text.find("\"unlocked\"")?;
    let after = &text[key + "\"unlocked\"".len()..];
    let colon = after.find(':')?;
    let rest = &after[colon + 1..];
    let digits: String = rest.chars().skip_while(|c| c.is_whitespace()).take_while(|c| c.is_ascii_digit()).collect();
    digits.parse::<u32>().ok()
}

/// Persist `unlocked` (best-effort; failure is non-fatal — progress just won't carry over).
fn save_unlocked(unlocked: u32) {
    let path = progress_path();
    let body = format!("{{\n  \"unlocked\": {}\n}}\n", unlocked);
    let _ = std::fs::write(path, body);
}

// =============================================================================================
// Continuous camera (lens-world coords -> screen pixels). One camera spans both zoom states by
// lerping its centre + scale between the lens framing and the focused-planet framing.
// =============================================================================================

/// Lens-world coordinates: every planet's [`world::Planet::pos`] lives here, and so does the
/// *interior* of a planet (we offset a planet's local sub coords by its `pos` so one continuous
/// space holds both — the planet's own structure is centred on its `pos`).
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

/// The lens camera: fit every planet position (with a margin) into the drawable area.
fn lens_camera(world: &World, top: f32, bottom: f32) -> Camera {
    let (mut minx, mut miny, mut maxx, mut maxy) =
        (f32::INFINITY, f32::INFINITY, f32::NEG_INFINITY, f32::NEG_INFINITY);
    for p in &world.planets {
        // Pad each planet by its own visual node radius (in world units) so big nodes fit.
        let pad = planet_world_radius(p);
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

/// The interior camera for planet `p`: fit that planet's structure (its subs, in world coords =
/// local + planet.pos) into the drawable area, scaled to fill it.
fn interior_camera(world: &World, p: PlanetId, top: f32, bottom: f32) -> Camera {
    let planet = &world.planets[p];
    let (mut minx, mut miny, mut maxx, mut maxy) =
        (f32::INFINITY, f32::INFINITY, f32::NEG_INFINITY, f32::NEG_INFINITY);
    for s in &planet.structure.subs {
        let wx = planet.pos.x + s.pos.x;
        let wy = planet.pos.y + s.pos.y;
        minx = minx.min(wx - s.radius);
        miny = miny.min(wy - s.radius);
        maxx = maxx.max(wx + s.radius);
        maxy = maxy.max(wy + s.radius);
    }
    if !minx.is_finite() {
        // Degenerate: centre on the planet with a generous zoom.
        return Camera { cx: planet.pos.x, cy: planet.pos.y, scale: 12.0, top, bottom };
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

/// A planet's visual node radius **in world units** (so the lens fit accounts for it). Scales
/// gently with total ships present so big stacks read as bigger nodes; bounded.
fn planet_world_radius(p: &world::Planet) -> f32 {
    // A modest fraction of a typical lane so nodes never overlap their neighbours badly. Ships
    // are read from the structure directly (cheap; small N at Layer 1).
    let ships = p.structure.ship_count(Faction::Player) + p.structure.ship_count(Faction::Enemy);
    let base = 7.0_f32;
    base + (ships as f32).sqrt() * 1.1
}

// =============================================================================================
// In-level game state
// =============================================================================================

/// The current camera zoom intent.
#[derive(Clone, Copy, PartialEq)]
enum View {
    /// Zoomed out to the planet graph.
    Lens,
    /// Zoomed into one planet's interior.
    Interior(PlanetId),
}

/// A lightweight fleet identity to carry `progress` across a tick for interpolation (cell-core
/// fleets have no stable id; [`world::InterFleet`] is the same — we re-derive identity).
#[derive(Clone, Copy, PartialEq)]
struct FleetKey {
    faction: Faction,
    from: PlanetId,
    to: PlanetId,
    progress: f32,
    undock_remaining: u32,
}

/// One in-level match: the world + everything the renderer/host needs around it.
struct Game {
    level: Level,
    world: World,
    wp: WorldParams,
    sim: SimParams,

    enemy: AiController,
    /// Set in `--auto` / demo: also drive the Player seat by AI (full strategic+tactical).
    player_ai: Option<AiController>,
    /// Tunables for the player's basic-automation greedy adapter (matches the AI's defaults).
    greedy: GreedyParams,
    /// Per-planet player-automation flags (only meaningful when `level.automation_available`).
    automated: Vec<bool>,

    // Camera / zoom.
    view: View,
    /// 0 = full lens, 1 = full interior. Eased toward the target each frame.
    cam_t: f32,
    /// The planet the interior camera is (or was last) focused on — kept while easing out.
    focus: PlanetId,

    // Pacing.
    paused: bool,
    speed_idx: usize,
    tick_accum: f64,
    render_alpha: f32,

    // Interpolation snapshots.
    /// Per-planet, per-ship pre-step positions (indexed [planet][ship]) for interior motion.
    prev_ship_pos: Vec<Vec<layer1::Vec2>>,
    /// Fleet identities + progress before the last tick, for lens fleet interpolation.
    prev_fleets: Vec<FleetKey>,

    // Input / selection (shared across both layers).
    fraction: FractionBucket,
    /// Selected source: a planet (lens) or a sub of the focused planet (interior). We keep both
    /// kinds; only the one matching the current view is acted on.
    sel_planet: Option<PlanetId>,
    sel_sub: Option<usize>,
    /// Drag source (planet in lens, sub in interior) while the left button is held.
    drag_planet: Option<PlanetId>,
    drag_sub: Option<usize>,

    /// Show the start/objective overlay until dismissed.
    show_intro: bool,
    /// Latched outcome once the match ends (so Victory/Defeat is stable).
    finished: Option<Faction>,
}

impl Game {
    fn new(level: Level, seed: u64, auto: bool, force_view: Option<ViewTarget>) -> Game {
        let (world, wp) = (level.build)(seed);
        let sim = gui_params();
        let enemy = AiController::from_roster(Faction::Enemy, level.enemy);
        // In auto/demo, the Player seat is a balanced controller so the level plays itself.
        let player_ai = if auto {
            Some(AiController::from_roster(Faction::Player, ai::Roster::GreedyLocal))
        } else {
            None
        };
        let n = world.planets.len();

        // Opening view: honour an explicit override, else the level's StartView.
        let view = match force_view {
            Some(ViewTarget::Lens) => View::Lens,
            Some(ViewTarget::Interior) => View::Interior(start_planet(&level)),
            None => match level.start_view {
                StartView::Layer1(p) => View::Interior(p.min(n.saturating_sub(1))),
                StartView::Layer2 => View::Lens,
            },
        };
        let focus = match view {
            View::Interior(p) => p,
            View::Lens => start_planet(&level),
        };
        let cam_t = if matches!(view, View::Interior(_)) { 1.0 } else { 0.0 };

        Game {
            level,
            world,
            wp,
            sim,
            enemy,
            player_ai,
            greedy: GreedyParams::default(),
            automated: vec![false; n],
            view,
            cam_t,
            focus,
            paused: false,
            speed_idx: DEFAULT_SPEED_IDX,
            tick_accum: 0.0,
            render_alpha: 0.0,
            prev_ship_pos: Vec::new(),
            prev_fleets: Vec::new(),
            fraction: FractionBucket::Half,
            sel_planet: None,
            sel_sub: None,
            drag_planet: None,
            drag_sub: None,
            show_intro: true,
            finished: None,
        }
    }

    fn speed(&self) -> f64 {
        SPEED_STEPS[self.speed_idx]
    }

    /// The match is over at world-wide elimination or the level horizon.
    fn match_over(&self) -> bool {
        self.world.is_eliminated(Faction::Player)
            || self.world.is_eliminated(Faction::Enemy)
            || self.world.tick >= self.level.horizon
    }

    /// Snapshot fleet identities + per-planet ship positions just before a tick, for render
    /// interpolation.
    fn snapshot(&mut self) {
        self.prev_fleets = self.world.fleets.iter().map(fleet_key).collect();
        self.prev_ship_pos.clear();
        for p in &self.world.planets {
            self.prev_ship_pos.push(p.structure.ships.iter().map(|s| s.pos).collect());
        }
    }

    /// Run one world tick: on the decision cadence, decide+apply the enemy (and the player AI in
    /// demo mode) and run player automation; then step the world once.
    fn step_one_tick(&mut self) {
        self.snapshot();

        if self.world.tick % DECISION_INTERVAL == 0 {
            // Enemy seat.
            self.enemy.decide_and_apply(&mut self.world, &self.sim, &self.wp);
            // Player seat in demo mode (both seats AI).
            if let Some(pa) = &self.player_ai {
                pa.decide_and_apply(&mut self.world, &self.sim, &self.wp);
            } else {
                // Player's optional basic automation: drive each AUTO planet's internals with the
                // SAME Layer-1 greedy adapter the AI uses (delegate a planet's micro).
                self.run_player_automation();
            }
        }

        self.world.step(&self.sim, &self.wp);

        // Latch the outcome the first tick the match is decided.
        if self.finished.is_none() && self.match_over() {
            self.finished = self.world.outcome().winner.or(Some(Faction::Neutral));
        }
    }

    /// For each player-owned, AUTO-enabled planet, issue the Layer-1 greedy adapter's internal
    /// move orders into that planet's structure. This is the "delegate a planet" lesson — the
    /// identical policy the enemy runs on its own planets.
    fn run_player_automation(&mut self) {
        if !self.level.automation_available {
            return;
        }
        for p in 0..self.world.planets.len() {
            if !self.automated.get(p).copied().unwrap_or(false) {
                continue;
            }
            // Only bother where the player actually has a presence to command.
            let agg = self.world.planet_aggregate(p);
            if agg.player_subs == 0 && agg.player_ships == 0 {
                continue;
            }
            let orders = ai::greedy_layer1_orders(
                &self.world.planets[p].structure,
                &self.sim,
                Faction::Player,
                &self.greedy,
            );
            for o in orders {
                self.world.planets[p].structure.issue_order(o);
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

        if self.paused || self.match_over() {
            self.render_alpha = 1.0;
            return;
        }
        self.tick_accum += dt * BASE_TICKS_PER_SEC * self.speed();
        let mut budget = MAX_TICKS_PER_FRAME;
        while self.tick_accum >= 1.0 && budget > 0 {
            self.step_one_tick();
            self.tick_accum -= 1.0;
            budget -= 1;
            if self.match_over() {
                break;
            }
        }
        if budget == 0 {
            self.tick_accum = self.tick_accum.min(1.0);
        }
        self.render_alpha = self.tick_accum.clamp(0.0, 1.0) as f32;
    }

    /// The current blended camera (lens <-> interior of `focus`).
    fn camera(&self) -> Camera {
        let top = HUD_TOP_H;
        let bottom = HUD_BOTTOM_H;
        let lens = lens_camera(&self.world, top, bottom);
        if self.cam_t <= 0.0001 {
            return lens;
        }
        let inter = interior_camera(&self.world, self.focus, top, bottom);
        lerp_camera(lens, inter, self.cam_t)
    }

    /// Interpolated draw position for ship `id` on planet `p`.
    #[inline]
    fn ship_draw_pos(&self, p: PlanetId, id: usize) -> layer1::Vec2 {
        let cur = self.world.planets[p].structure.ships[id].pos;
        match self.prev_ship_pos.get(p).and_then(|v| v.get(id)) {
            Some(prev) => layer1::Vec2 {
                x: prev.x + (cur.x - prev.x) * self.render_alpha,
                y: prev.y + (cur.y - prev.y) * self.render_alpha,
            },
            None => cur,
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

/// The planet a level "opens on" (for interior framing): the `Layer1(p)` target, else the first
/// Player-owned planet, else planet 0.
fn start_planet(level: &Level) -> PlanetId {
    if let StartView::Layer1(p) = level.start_view {
        return p;
    }
    let (world, _wp) = (level.build)(0);
    for p in 0..world.planets.len() {
        if matches!(world.planet_aggregate(p).owner, PlanetOwner::Owned(Faction::Player)) {
            return p;
        }
    }
    0
}

/// The GUI sim operating point: the Layer-1 defaults with combat softened a touch so brawls last
/// long enough to read (purely spectacle tuning of data we own; determinism is intact — the sim
/// still draws all randomness from its own seeded PRNG).
fn gui_params() -> SimParams {
    let mut p = SimParams::default();
    p.fire_prob = 0.022;
    p
}

// =============================================================================================
// App-level state machine
// =============================================================================================

enum AppState {
    MainMenu { idx: usize },
    LevelSelect { idx: usize },
    InLevel { game: Box<Game>, paused_menu: Option<usize> },
}

struct App {
    levels: Vec<Level>,
    unlocked: u32,
    state: AppState,
    seed: u64,
    auto: bool,
}

impl App {
    fn new(cfg: &Config) -> App {
        let levels = campaign();
        let mut unlocked = if cfg.unlock_all { levels.len() as u32 } else { load_unlocked() };
        unlocked = unlocked.clamp(1, levels.len() as u32);

        // If a start level was requested on the CLI, jump straight into it.
        let state = if let Some(n) = cfg.start_level {
            let idx = n.saturating_sub(1).min(levels.len() - 1);
            let game = Game::new(levels[idx].clone(), cfg.seed, cfg.auto, None);
            AppState::InLevel { game: Box::new(game), paused_menu: None }
        } else {
            AppState::MainMenu { idx: 0 }
        };

        App { levels, unlocked, state, seed: cfg.seed, auto: cfg.auto }
    }

    /// Begin level index `idx` (0-based).
    fn start_level(&mut self, idx: usize) {
        let game = Game::new(self.levels[idx].clone(), self.seed, self.auto, None);
        self.state = AppState::InLevel { game: Box::new(game), paused_menu: None };
    }

    /// Record a win on level `idx` (0-based): unlock the next level + persist.
    fn record_win(&mut self, idx: usize) {
        let next = (idx as u32 + 2).min(self.levels.len() as u32); // unlock idx+1 (1-based idx+2)
        if next > self.unlocked {
            self.unlocked = next;
            save_unlocked(self.unlocked);
        }
    }
}

// =============================================================================================
// Update + input per app state
// =============================================================================================

const MENU_ITEMS: [&str; 3] = ["Play", "Level Select", "Quit"];

/// A transition the in-level update wants to apply to `App` *after* the `app.state` borrow ends
/// (so we never hold a borrow of `app.state` across a method that mutates `app`).
enum LevelAction {
    /// No transition this frame.
    None,
    /// (Re)start level index `idx` (0-based).
    Start(usize),
    /// Go to the level-select screen with row `idx` highlighted.
    ToSelect(usize),
    /// Record a win on level `idx`; if `advance`, move on to the next level (or the menu).
    WinThen { idx: usize, advance: bool },
}

/// Drive the app one frame (input + sim). Returns `true` to request quitting.
fn app_update(app: &mut App, dt: f64) -> bool {
    match &mut app.state {
        AppState::MainMenu { idx } => {
            let mut idx_v = *idx;
            if is_key_pressed(KeyCode::Down) || is_key_pressed(KeyCode::S) {
                idx_v = (idx_v + 1) % MENU_ITEMS.len();
            }
            if is_key_pressed(KeyCode::Up) || is_key_pressed(KeyCode::W) {
                idx_v = (idx_v + MENU_ITEMS.len() - 1) % MENU_ITEMS.len();
            }
            *idx = idx_v;
            // Mouse hover/click selects.
            let click = is_mouse_button_pressed(MouseButton::Left);
            if let Some(hovered) = menu_item_at_mouse(MENU_ITEMS.len()) {
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
                if click && (hovered as u32) < app.unlocked {
                    app.start_level(hovered);
                    return false;
                }
            }
            if (is_key_pressed(KeyCode::Enter) || is_key_pressed(KeyCode::Space))
                && (sel as u32) < app.unlocked
            {
                app.start_level(sel);
            }
            false
        }
        AppState::InLevel { .. } => {
            // The InLevel arm borrows `game`/`paused_menu` from `app.state`, but several
            // transitions need to mutate `app` itself (start a level, change state). So we
            // compute a deferred `LevelAction` while holding the borrow, drop it, then apply.
            let n_levels = app.levels.len();
            let action = {
                let AppState::InLevel { game, paused_menu } = &mut app.state else { unreachable!() };

                // --- Pause menu overlay takes priority ---
                if let Some(pmi) = paused_menu {
                    let items = ["Resume", "Restart", "Back to Menu"];
                    let mut sel = *pmi;
                    if is_key_pressed(KeyCode::Down) || is_key_pressed(KeyCode::S) {
                        sel = (sel + 1) % items.len();
                    }
                    if is_key_pressed(KeyCode::Up) || is_key_pressed(KeyCode::W) {
                        sel = (sel + items.len() - 1) % items.len();
                    }
                    *pmi = sel;
                    let click = is_mouse_button_pressed(MouseButton::Left);
                    let hovered = menu_item_at_mouse(items.len());
                    if let Some(h) = hovered {
                        *pmi = h;
                    }
                    let chosen = if is_key_pressed(KeyCode::Enter) || is_key_pressed(KeyCode::Space) {
                        Some(sel)
                    } else if click {
                        hovered
                    } else {
                        None
                    };
                    let idx = (game.level.id as usize).saturating_sub(1);
                    if is_key_pressed(KeyCode::Escape) {
                        *paused_menu = None;
                        LevelAction::None
                    } else {
                        match chosen {
                            Some(0) => {
                                *paused_menu = None;
                                LevelAction::None
                            }
                            Some(1) => LevelAction::Start(idx),
                            Some(2) => LevelAction::ToSelect(idx),
                            _ => LevelAction::None,
                        }
                    }
                } else if game.match_over() {
                    // --- End-of-match handling (Victory/Defeat) ---
                    let winner = game.finished.unwrap_or(Faction::Neutral);
                    let idx = (game.level.id as usize).saturating_sub(1);
                    game.update(dt); // keep easing the camera on the end screen
                    if is_key_pressed(KeyCode::Escape) {
                        LevelAction::ToSelect(idx)
                    } else if winner == Faction::Player {
                        let advance = is_key_pressed(KeyCode::Enter) || is_key_pressed(KeyCode::Space);
                        LevelAction::WinThen { idx, advance }
                    } else if is_key_pressed(KeyCode::R) {
                        LevelAction::Start(idx)
                    } else {
                        LevelAction::None
                    }
                } else {
                    // --- Live in-level input ---
                    handle_in_level_input(game);
                    game.update(dt);
                    LevelAction::None
                }
            };

            // Apply the deferred action now that the `app.state` borrow is gone.
            match action {
                LevelAction::None => {}
                LevelAction::Start(idx) => app.start_level(idx),
                LevelAction::ToSelect(idx) => app.state = AppState::LevelSelect { idx },
                LevelAction::WinThen { idx, advance } => {
                    app.record_win(idx);
                    if advance {
                        let next = idx + 1;
                        if next < n_levels {
                            app.start_level(next);
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
    match item {
        0 => {
            // Play: jump to the highest unlocked level (continue), else level 1.
            let idx = (app.unlocked as usize).saturating_sub(1).min(app.levels.len() - 1);
            app.start_level(idx);
            false
        }
        1 => {
            app.state = AppState::LevelSelect { idx: 0 };
            false
        }
        2 => true,
        _ => false,
    }
}

// =============================================================================================
// In-level input (both layers + zoom + automation)
// =============================================================================================

fn handle_in_level_input(game: &mut Game) {
    // Dismiss the intro overlay on any meaningful key/click.
    if game.show_intro
        && (is_key_pressed(KeyCode::Enter)
            || is_key_pressed(KeyCode::Space)
            || is_mouse_button_pressed(MouseButton::Left))
    {
        game.show_intro = false;
        // Don't also treat this first click as an order.
        if is_mouse_button_pressed(MouseButton::Left) {
            return;
        }
    }

    // --- Global controls ---
    if is_key_pressed(KeyCode::P) {
        game.paused = !game.paused;
    }
    if is_key_pressed(KeyCode::Minus) || is_key_pressed(KeyCode::KpSubtract) || is_key_pressed(KeyCode::LeftBracket) {
        game.speed_idx = game.speed_idx.saturating_sub(1);
    }
    if is_key_pressed(KeyCode::Equal) || is_key_pressed(KeyCode::KpAdd) || is_key_pressed(KeyCode::RightBracket) {
        game.speed_idx = (game.speed_idx + 1).min(SPEED_STEPS.len() - 1);
    }
    if is_key_pressed(KeyCode::Key1) {
        game.fraction = FractionBucket::Quarter;
    }
    if is_key_pressed(KeyCode::Key2) {
        game.fraction = FractionBucket::Half;
    }
    if is_key_pressed(KeyCode::Key3) {
        game.fraction = FractionBucket::ThreeQuarter;
    }
    if is_key_pressed(KeyCode::Key4) {
        game.fraction = FractionBucket::All;
    }

    // Mouse-wheel zoom: scroll up zooms into the hovered/focused planet; down zooms out.
    let (_wx, wy) = mouse_wheel();
    if wy > 0.0 {
        zoom_in(game);
    } else if wy < 0.0 {
        game.view = View::Lens;
        clear_selection(game);
    }

    // Clear selection (right-click / Esc handled; Esc also opens pause when nothing selected).
    if is_mouse_button_pressed(MouseButton::Right) {
        clear_selection(game);
    }
    if is_key_pressed(KeyCode::Escape) {
        if game.sel_planet.is_some() || game.sel_sub.is_some() {
            clear_selection(game);
        } else if matches!(game.view, View::Interior(_)) {
            // Esc zooms out first.
            game.view = View::Lens;
        } else {
            // Open the pause menu (the caller reads this sentinel). The menu itself freezes the
            // sim while it is up, so we do NOT also set `game.paused` (that would leave the sim
            // paused after Resume).
            OPEN_PAUSE.with(|c| c.set(true));
        }
    }

    // Pointer orders are disabled while the player seat is AI-driven (demo mode).
    if game.player_ai.is_some() {
        return;
    }

    let (mx, my) = mouse_position();
    let cam = game.camera();

    // Whether we are effectively in the interior (camera mostly zoomed in).
    let in_interior = matches!(game.view, View::Interior(_)) && game.cam_t > INTERIOR_DRAW_THRESHOLD;

    // --- AUTO toggle (key A) on the focused/selected planet ---
    if is_key_pressed(KeyCode::A) && game.level.automation_available {
        let target = if in_interior {
            Some(game.focus)
        } else {
            game.sel_planet.or_else(|| planet_at_screen(game, &cam, mx, my))
        };
        if let Some(p) = target {
            if matches!(game.world.planet_aggregate(p).owner, PlanetOwner::Owned(Faction::Player))
                || game.world.planet_aggregate(p).player_subs > 0
            {
                if let Some(slot) = game.automated.get_mut(p) {
                    *slot = !*slot;
                }
            }
        }
    }

    if in_interior {
        handle_interior_click(game, &cam, mx, my);
    } else {
        handle_lens_click(game, &cam, mx, my);
    }
}

/// Zoom into a planet: prefer the hovered planet, else the current selection, else the focus.
fn zoom_in(game: &mut Game) {
    let (mx, my) = mouse_position();
    let cam = game.camera();
    let target = planet_at_screen(game, &cam, mx, my).or(game.sel_planet);
    if let Some(p) = target {
        game.focus = p;
        game.view = View::Interior(p);
        clear_selection(game);
    } else if let View::Lens = game.view {
        // No planet under the cursor: focus the current focus planet.
        game.view = View::Interior(game.focus);
    }
}

fn clear_selection(game: &mut Game) {
    game.sel_planet = None;
    game.sel_sub = None;
    game.drag_planet = None;
    game.drag_sub = None;
}

/// Lens-layer click handling: select an owned planet (source), or issue a FleetOrder to a
/// lane-connected planet; double-purpose: clicking a planet with no source selected, where the
/// planet is owned, selects it; clicking it again (or a different already-selected) zooms.
fn handle_lens_click(game: &mut Game, cam: &Camera, mx: f32, my: f32) {
    if is_mouse_button_pressed(MouseButton::Left) {
        if let Some(planet) = planet_at_screen(game, cam, mx, my) {
            let owner = game.world.planet_aggregate(planet).owner;
            let mine = matches!(owner, PlanetOwner::Owned(Faction::Player))
                || game.world.planet_aggregate(planet).player_subs > 0;
            if let Some(src) = game.sel_planet {
                if planet == src {
                    // Click the already-selected source again => zoom into it.
                    game.focus = planet;
                    game.view = View::Interior(planet);
                    clear_selection(game);
                } else if game.world.are_connected(src, planet) {
                    // Issue an inter-planet fleet order to a connected target.
                    game.world.issue_fleet_order(
                        FleetOrder::new(src, planet, game.fraction),
                        Faction::Player,
                        &game.wp,
                    );
                    // Keep the source selected for rapid repeat orders.
                    game.drag_planet = None;
                } else {
                    // Not connected: reselect if it's ours, else clear.
                    if mine {
                        game.sel_planet = Some(planet);
                        game.drag_planet = Some(planet);
                    } else {
                        clear_selection(game);
                    }
                }
            } else if mine {
                game.sel_planet = Some(planet);
                game.drag_planet = Some(planet);
            } else {
                // Clicking a non-owned planet with nothing selected: zoom in to inspect it.
                game.focus = planet;
                game.view = View::Interior(planet);
            }
        } else {
            clear_selection(game);
        }
    }

    // Drag-release from an owned planet onto a connected planet issues a fleet.
    if is_mouse_button_released(MouseButton::Left) {
        if let Some(src) = game.drag_planet {
            if let Some(tgt) = planet_at_screen(game, cam, mx, my) {
                if tgt != src && game.world.are_connected(src, tgt) {
                    game.world.issue_fleet_order(
                        FleetOrder::new(src, tgt, game.fraction),
                        Faction::Player,
                        &game.wp,
                    );
                }
            }
            game.drag_planet = None;
        }
    }
}

/// Interior-layer click handling: select a Player sub (source) or issue a MoveOrder to another
/// sub of the focused planet.
fn handle_interior_click(game: &mut Game, cam: &Camera, mx: f32, my: f32) {
    let p = game.focus;
    if is_mouse_button_pressed(MouseButton::Left) {
        if let Some(sub) = sub_at_screen(game, p, cam, mx, my) {
            let owner = game.world.planets[p].structure.subs[sub].owner;
            if owner == Faction::Player {
                game.sel_sub = Some(sub);
                game.drag_sub = Some(sub);
            } else if let Some(src) = game.sel_sub {
                game.world.planets[p].structure.issue_order(MoveOrder::new(src, sub, game.fraction));
                game.drag_sub = None;
            }
        } else {
            game.sel_sub = None;
            game.drag_sub = None;
        }
    }
    if is_mouse_button_released(MouseButton::Left) {
        if let Some(src) = game.drag_sub {
            if let Some(tgt) = sub_at_screen(game, p, cam, mx, my) {
                if tgt != src {
                    game.world.planets[p].structure.issue_order(MoveOrder::new(src, tgt, game.fraction));
                }
            }
            game.drag_sub = None;
        }
    }
}

thread_local! {
    /// Set by in-level input when Esc should open the pause menu (so the borrow of `game` inside
    /// input handling does not need to reach back into `App`).
    static OPEN_PAUSE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Planet under a screen point (within its drawn node radius). Nearest centre on overlap.
fn planet_at_screen(game: &Game, cam: &Camera, mx: f32, my: f32) -> Option<PlanetId> {
    let mut best: Option<(PlanetId, f32)> = None;
    for i in 0..game.world.planets.len() {
        let p = &game.world.planets[i];
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

/// Sub-structure of planet `p` under a screen point.
fn sub_at_screen(game: &Game, p: PlanetId, cam: &Camera, mx: f32, my: f32) -> Option<usize> {
    let planet = &game.world.planets[p];
    let mut best: Option<(usize, f32)> = None;
    for (i, s) in planet.structure.subs.iter().enumerate() {
        let (sx, sy) = cam.to_screen(planet.pos.x + s.pos.x, planet.pos.y + s.pos.y);
        let r = cam.len(s.radius).max(10.0);
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

// =============================================================================================
// Rendering — top-level dispatch
// =============================================================================================

const HUD_TOP_H: f32 = 96.0;
const HUD_BOTTOM_H: f32 = 26.0;

fn app_draw(app: &App) {
    clear_background(BG);
    match &app.state {
        AppState::MainMenu { idx } => draw_main_menu(*idx),
        AppState::LevelSelect { idx } => draw_level_select(app, *idx),
        AppState::InLevel { game, paused_menu } => {
            draw_in_level(game);
            if let Some(pmi) = paused_menu {
                draw_pause_menu(*pmi);
            }
        }
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
/// Y (top of the band) where the first menu item starts: centre a 3-item block around ~58% height.
fn menu_first_y() -> f32 {
    let pitch = menu_pitch();
    (screen_height() * 0.58 - pitch).max(screen_height() * 0.34)
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

fn draw_main_menu(idx: usize) {
    let cx = screen_width() * 0.5;
    let sh = screen_height();
    // Title block, proportional so it always sits above the menu items.
    draw_centered("MACHINE INTELLIGENCE", sh * 0.20, 60, ACCENT);
    draw_centered("a cell-game RTS", sh * 0.20 + 40.0, 26, HUD_MUTED);
    draw_centered("operator -> programmer -> meta-programmer", sh * 0.20 + 72.0, 18, HUD_MUTED);

    for (i, item) in MENU_ITEMS.iter().enumerate() {
        let (x, y, w, h) = menu_item_rect(i);
        let sel = i == idx;
        let bg = if sel { Color::new(0.12, 0.16, 0.22, 0.95) } else { Color::new(0.07, 0.09, 0.13, 0.9) };
        draw_rectangle(x, y, w, h, bg);
        draw_rectangle_lines(x, y, w, h, 2.0, if sel { PLAYER } else { EDGE_COL });
        let col = if sel { HUD_TEXT } else { HUD_MUTED };
        let _ = cx;
        draw_centered(item, y + h * 0.66, 28, col);
    }

    draw_centered(
        "Up/Down or mouse to choose  |  Enter to select  |  Esc to go back",
        screen_height() - 40.0,
        18,
        HUD_MUTED,
    );
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

fn draw_level_select(app: &App, idx: usize) {
    draw_centered("LEVEL SELECT", 96.0, 44, ACCENT);

    for (i, lvl) in app.levels.iter().enumerate() {
        let (x, y, w, h) = level_row_rect(i);
        let unlocked = (i as u32) < app.unlocked;
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
        let col = if !unlocked {
            Color::new(0.40, 0.42, 0.46, 1.0)
        } else if sel {
            HUD_TEXT
        } else {
            HUD_MUTED
        };
        draw_text(&title, x + 18.0, baseline, 24.0, col);

        // Enemy tag on the right.
        let tag = lvl.enemy.name();
        let td = measure_text(tag, None, 18, 1.0);
        draw_text(tag, x + w - td.width - 16.0, baseline, 18.0, if unlocked { HUD_MUTED } else { Color::new(0.3, 0.32, 0.36, 1.0) });
    }

    draw_centered(
        "Up/Down or mouse  |  Enter: play an unlocked level  |  Esc: back",
        screen_height() - 22.0,
        18,
        HUD_MUTED,
    );
}

fn draw_pause_menu(idx: usize) {
    let sw = screen_width();
    let sh = screen_height();
    draw_rectangle(0.0, 0.0, sw, sh, Color::new(0.0, 0.0, 0.0, 0.6));
    draw_centered("PAUSED", menu_first_y() - menu_pitch() * 0.8, 60, ACCENT);
    let items = ["Resume", "Restart", "Back to Menu"];
    for (i, item) in items.iter().enumerate() {
        let (x, y, w, h) = menu_item_rect(i);
        let sel = i == idx;
        let bg = if sel { Color::new(0.12, 0.16, 0.22, 0.95) } else { Color::new(0.07, 0.09, 0.13, 0.9) };
        draw_rectangle(x, y, w, h, bg);
        draw_rectangle_lines(x, y, w, h, 2.0, if sel { PLAYER } else { EDGE_COL });
        draw_centered(item, y + h * 0.66, 28, if sel { HUD_TEXT } else { HUD_MUTED });
    }
    draw_centered("Esc: resume", sh - 40.0, 18, HUD_MUTED);
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

    draw_hud(game);

    if game.show_intro {
        draw_intro_overlay(game);
    }
    if game.match_over() {
        draw_end_banner(game);
    }
}

/// Draw the Layer-2 lens: lanes, interpolated fleets, planet nodes.
fn draw_lens(game: &Game, cam: &Camera, alpha: f32) {
    let t = get_time() as f32;
    let w = &game.world;

    // Valid-target neighbours of a selected source (for highlighting).
    let valid: Option<std::collections::HashSet<PlanetId>> = game.sel_planet.map(|src| {
        w.neighbors(src).iter().copied().collect()
    });

    // --- Lanes ---
    for lane in &w.lanes {
        let (ax, ay) = cam.to_screen(w.planets[lane.a].pos.x, w.planets[lane.a].pos.y);
        let (bx, by) = cam.to_screen(w.planets[lane.b].pos.x, w.planets[lane.b].pos.y);
        let hi = match (game.sel_planet, &valid) {
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
        let (fx, fy) = cam.to_screen(w.planets[f.from].pos.x, w.planets[f.from].pos.y);
        let (tx, ty) = cam.to_screen(w.planets[f.to].pos.x, w.planets[f.to].pos.y);
        let cx = fx + (tx - fx) * pos;
        let cy = fy + (ty - fy) * pos;
        let dir = {
            let (dx, dy) = (tx - fx, ty - fy);
            let l = (dx * dx + dy * dy).sqrt().max(1e-3);
            (dx / l, dy / l)
        };
        let col = fade(faction_color(f.faction), alpha);
        draw_fleet_cluster(cx, cy, dir, f.count as f64, col, f.undock_remaining > 0, t);
    }

    // --- Planet nodes ---
    for i in 0..w.planets.len() {
        let agg = w.planet_aggregate(i);
        let (sx, sy) = cam.to_screen(w.planets[i].pos.x, w.planets[i].pos.y);
        let r = node_screen_radius(game, i, cam);

        let base = planet_color(agg.owner);
        let dim = match agg.owner {
            PlanetOwner::Owned(f) => faction_dim(f),
            PlanetOwner::Neutral => NEUTRAL_DIM,
            PlanetOwner::Contested => Color::new(0.4, 0.34, 0.2, 1.0),
        };

        // Filled disk + ring.
        draw_circle(sx, sy, r, fade(Color::new(dim.r, dim.g, dim.b, 0.42), alpha));
        draw_circle_lines(sx, sy, r, 2.5, fade(base, alpha));

        // Contested indicator: a second ring + a small split-colour cue.
        if agg.owner == PlanetOwner::Contested {
            draw_circle_lines(sx, sy, r + 4.0, 2.0, fade(Color::new(1.0, 0.85, 0.4, 0.8), alpha));
            // Two small arcs hinting both sides are present.
            draw_circle(sx - r * 0.5, sy, 3.5, fade(PLAYER, alpha));
            draw_circle(sx + r * 0.5, sy, 3.5, fade(ENEMY, alpha));
        }

        // Automation indicator: a green ring + "AUTO" tag on automated, player-owned planets.
        if game.automated.get(i).copied().unwrap_or(false) {
            let pulse = 0.5 + 0.5 * (t * 3.0).sin();
            draw_circle_lines(sx, sy, r + 6.0, 2.0, fade(Color::new(AUTO_COL.r, AUTO_COL.g, AUTO_COL.b, 0.5 + 0.4 * pulse), alpha));
            draw_text("AUTO", sx - 16.0, sy + r + 16.0, 16.0, fade(AUTO_COL, alpha));
        }

        // Production pip on owned (producing) planets.
        if let PlanetOwner::Owned(f) = agg.owner {
            if f.is_real() {
                let pulse = 0.5 + 0.5 * (t * 2.2 + i as f32 * 0.6).sin();
                let pr = 3.0 + 1.6 * pulse;
                draw_circle(sx + r * 0.72, sy - r * 0.72, pr, fade(Color::new(base.r, base.g, base.b, 0.85), alpha));
            }
        }

        // Selected source outline.
        if game.sel_planet == Some(i) {
            let pulse = 0.5 + 0.5 * (t * 4.0).sin();
            draw_circle_lines(sx, sy, r + 8.0, 2.5, fade(Color::new(1.0, 1.0, 1.0, 0.5 + 0.4 * pulse), alpha));
        }

        // Ship count (Player+Enemy garrisoned + arriving) centred.
        let total = agg.ships_of(Faction::Player) + agg.ships_of(Faction::Enemy);
        let label = total.to_string();
        let fs = (r * 0.85).clamp(14.0, 30.0) as u16;
        let d = measure_text(&label, None, fs, 1.0);
        draw_text(&label, sx - d.width * 0.5, sy + d.height * 0.35, fs as f32, fade(Color::new(0.97, 0.98, 1.0, 0.96), alpha));

        // Planet name above the node.
        let name = &w.planets[i].name;
        let nd = measure_text(name, None, 16, 1.0);
        draw_text(name, sx - nd.width * 0.5, sy - r - 8.0, 16.0, fade(HUD_MUTED, alpha));
    }
}

/// Draw the Layer-1 interior of planet `p` (subs, ships, battle bubbles) — like the standalone
/// Layer-1 game, but in the shared world-space camera.
fn draw_interior(game: &Game, p: PlanetId, cam: &Camera, alpha: f32) {
    let t = get_time() as f32;
    let planet = &game.world.planets[p];
    let st = &planet.structure;
    let ox = planet.pos.x;
    let oy = planet.pos.y;

    // Reference grid (every 10 local units), faint.
    draw_interior_grid(planet, cam, alpha);

    // --- Sub-structures ---
    for (i, s) in st.subs.iter().enumerate() {
        let (sx, sy) = cam.to_screen(ox + s.pos.x, oy + s.pos.y);
        let r = cam.len(s.radius).max(8.0);
        let base = faction_color(s.owner);
        let dim = faction_dim(s.owner);
        draw_circle(sx, sy, r, fade(Color::new(dim.r, dim.g, dim.b, 0.35), alpha));
        draw_circle_lines(sx, sy, r, 2.0, fade(base, alpha));

        // Capture state: the lone present faction (if any) grinding this sub.
        let lone = st.single_present_faction(i).map(|(f, _)| f);
        let eroding_by = lone.filter(|&f| f != s.owner); // being CAPTURED by this faction
        let res_frac = if s.max_resistance > 0.0 { (s.resistance / s.max_resistance).clamp(0.0, 1.0) } else { 1.0 };
        let healing = matches!(lone, Some(f) if f == s.owner) && res_frac < 0.999;

        // Production progress ring — only when owned AND not being eroded (denial = no production).
        if s.owner.is_real() && eroding_by.is_none() {
            let pf = production_fraction(s.production_timer, game.sim.production_period);
            draw_progress_ring(sx, sy, r + 4.0, pf, fade(base, alpha));
        }

        // Being-captured cue: a pulsing ring in the attacker's colour around the sub.
        if let Some(atk) = eroding_by {
            let pulse = 0.5 + 0.5 * (t * 6.0).sin();
            let ac = faction_color(atk);
            draw_circle_lines(sx, sy, r + 2.0, 2.5, fade(Color::new(ac.r, ac.g, ac.b, 0.45 + 0.45 * pulse), alpha));
        }

        // Resistance ("siege") bar below the sub: capturable neutrals, damaged, or being ground.
        if s.owner == Faction::Neutral || res_frac < 0.999 || eroding_by.is_some() {
            let fill = if s.owner == Faction::Neutral { NEUTRAL_DIM } else { base };
            draw_resistance_bar(sx, sy + r + 7.0, r, res_frac, fill, eroding_by.map(faction_color), healing, alpha, t);
        }

        // Selected source outline.
        if matches!(game.view, View::Interior(fp) if fp == p) && game.sel_sub == Some(i) {
            let pulse = 0.5 + 0.5 * (t * 4.0).sin();
            draw_circle_lines(sx, sy, r + 7.0, 2.5, fade(Color::new(1.0, 1.0, 1.0, 0.5 + 0.4 * pulse), alpha));
        }

        // Idle ship count, centred.
        let idle = st.idle_count_at(i, s.owner);
        let label = idle.to_string();
        let fs = (r * 0.95).clamp(14.0, 36.0) as u16;
        let d = measure_text(&label, None, fs, 1.0);
        draw_text(&label, sx - d.width * 0.5, sy + d.height * 0.35, fs as f32, fade(Color::new(0.97, 0.98, 1.0, 0.95), alpha));
    }

    // --- Ships (interpolated) ---
    for id in 0..st.ships.len() {
        let sh = &st.ships[id];
        if !sh.alive {
            continue;
        }
        let pos = game.ship_draw_pos(p, id);
        let (sx, sy) = cam.to_screen(ox + pos.x, oy + pos.y);
        let col = fade(faction_color(sh.faction), alpha);
        match sh.target {
            None => draw_circle(sx, sy, 2.4, col),
            Some(_) => {
                let dx = sh.aim.x - pos.x;
                let dy = sh.aim.y - pos.y;
                let len = (dx * dx + dy * dy).sqrt().max(1e-3);
                draw_ship_triangle(sx, sy, dx / len, -dy / len, col);
            }
        }
    }

    // --- Battle bubbles ---
    let bubbles = st.battle_bubbles(&game.sim);
    for b in &bubbles {
        let (cx, cy) = cam.to_screen(ox + b.center.x, oy + b.center.y);
        let rad = cam.len(b.radius).max(14.0);
        let pulse = 0.5 + 0.5 * (t * 6.0 + cx * 0.01).sin();
        let fill = 0.08 + 0.06 * pulse;
        draw_circle(cx, cy, rad + 6.0, fade(Color::new(1.0, 0.85, 0.35, fill), alpha));
        draw_circle_lines(cx, cy, rad + 6.0, 2.0, fade(Color::new(1.0, 0.8, 0.3, 0.5 + 0.3 * pulse), alpha));
        let txt = format!("{} vs {}", b.player_count, b.enemy_count);
        let d = measure_text(&txt, None, 20, 1.0);
        draw_text(&txt, cx - d.width * 0.5, cy - rad - 10.0, 20.0, fade(Color::new(1.0, 0.95, 0.8, 0.95), alpha));
    }

    // Soft-cap (garrison headroom) for the player on this planet — the anti-hoard signal.
    let parked = st.parked_count(Faction::Player);
    let cap = st.soft_cap(Faction::Player, &game.sim);
    if parked > 0 || cap > 0 {
        let frac = if cap > 0 { parked as f32 / cap as f32 } else { 0.0 };
        let (col, note): (Color, &str) = if parked > cap {
            (Color::new(1.0, 0.42, 0.36, 1.0), "  OVER CAP - ships bleeding")
        } else if frac > 0.8 {
            (Color::new(1.0, 0.82, 0.34, 1.0), "  near cap")
        } else {
            (HUD_MUTED, "")
        };
        draw_text(&format!("garrison {}/{}{}", parked, cap, note), 16.0, HUD_TOP_H + 22.0, 18.0, fade(col, alpha));
    }

    // AUTO badge for this planet while zoomed in.
    if game.automated.get(p).copied().unwrap_or(false) {
        draw_text("AUTO ENABLED", 16.0, HUD_TOP_H + 44.0, 20.0, fade(AUTO_COL, alpha));
    }
}

fn draw_interior_grid(planet: &world::Planet, cam: &Camera, alpha: f32) {
    let (mut minx, mut miny, mut maxx, mut maxy) =
        (f32::INFINITY, f32::INFINITY, f32::NEG_INFINITY, f32::NEG_INFINITY);
    for s in &planet.structure.subs {
        minx = minx.min(s.pos.x - s.radius);
        miny = miny.min(s.pos.y - s.radius);
        maxx = maxx.max(s.pos.x + s.radius);
        maxy = maxy.max(s.pos.y + s.radius);
    }
    if !minx.is_finite() {
        return;
    }
    let step = 10.0_f32;
    let (ox, oy) = (planet.pos.x, planet.pos.y);
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

/// A planet node's screen radius: its world radius scaled by the camera, with sane bounds so it
/// reads at any zoom.
fn node_screen_radius(game: &Game, i: PlanetId, cam: &Camera) -> f32 {
    let wr = planet_world_radius(&game.world.planets[i]);
    cam.len(wr).clamp(14.0, 60.0)
}

/// 0.0 just spawned -> 1.0 about to spawn.
fn production_fraction(timer: u32, period: u32) -> f32 {
    if period == 0 {
        return 0.0;
    }
    (1.0 - timer as f32 / period as f32).clamp(0.0, 1.0)
}

fn draw_progress_ring(cx: f32, cy: f32, r: f32, frac: f32, col: Color) {
    let frac = frac.clamp(0.0, 1.0);
    if frac <= 0.0 {
        return;
    }
    let segs = 48;
    let n = (segs as f32 * frac).ceil() as i32;
    let start = -std::f32::consts::FRAC_PI_2;
    let c = Color::new(col.r, col.g, col.b, col.a * 0.85);
    let mut prev: Option<(f32, f32)> = None;
    for k in 0..=n {
        let a = start + (k as f32 / segs as f32) * std::f32::consts::TAU;
        let p = (cx + r * a.cos(), cy + r * a.sin());
        if let Some(pp) = prev {
            draw_line(pp.0, pp.1, p.0, p.1, 2.5, c);
        }
        prev = Some(p);
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

fn draw_ship_triangle(cx: f32, cy: f32, ux: f32, uy: f32, col: Color) {
    let nose = 6.0_f32;
    let back = 3.5_f32;
    let (px, py) = (-uy, ux);
    let tip = Vec2::new(cx + ux * nose, cy + uy * nose);
    let l = Vec2::new(cx - ux * back + px * back, cy - uy * back + py * back);
    let r = Vec2::new(cx - ux * back - px * back, cy - uy * back - py * back);
    draw_triangle(tip, l, r, col);
}

/// A small ship cluster/stream for an inter-planet fleet.
fn draw_fleet_cluster(cx: f32, cy: f32, dir: (f32, f32), count: f64, col: Color, undock: bool, t: f32) {
    let n = (count.sqrt().round() as i32).clamp(1, 6);
    let (ux, uy) = dir;
    let (px, py) = (-uy, ux);
    let alpha = if undock { 0.45 } else { 0.95 };
    let c = Color::new(col.r, col.g, col.b, col.a * alpha);
    for k in 0..n {
        let along = (k as f32 - (n as f32 - 1.0) * 0.5) * 6.0;
        let across = ((k as f32 * 1.7 + t * 2.0).sin()) * 3.0;
        draw_ship_triangle(cx + ux * along + px * across, cy + uy * along + py * across, ux, uy, c);
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
    let sh = screen_height();
    let w = &game.world;

    draw_rectangle(0.0, 0.0, sw, HUD_TOP_H, HUD_BG);

    let p_ships = w.total_ships(Faction::Player);
    let e_ships = w.total_ships(Faction::Enemy);
    let p_subs = w.total_subs(Faction::Player);
    let e_subs = w.total_subs(Faction::Enemy);
    let (p_planets, e_planets) = planet_tallies(w);

    // Player (left).
    draw_text("PLAYER (you)", 16.0, 22.0, 22.0, PLAYER);
    draw_text(
        &format!("ships {:>4}   planets {}   subs {}", p_ships, p_planets, p_subs),
        16.0, 46.0, 20.0, HUD_TEXT,
    );

    // Enemy (right).
    let etitle = format!("ENEMY [{}]", game.level.enemy.name());
    let e1 = measure_text(&etitle, None, 22, 1.0);
    draw_text(&etitle, sw - e1.width - 16.0, 22.0, 22.0, ENEMY);
    let etxt = format!("ships {:>4}   planets {}   subs {}", e_ships, e_planets, e_subs);
    let e2 = measure_text(&etxt, None, 20, 1.0);
    draw_text(&etxt, sw - e2.width - 16.0, 46.0, 20.0, HUD_TEXT);

    // Centre rows 1-2: layer + focus, tick/clock, fraction, speed.
    let layer = if matches!(game.view, View::Interior(_)) && game.cam_t > 0.5 {
        format!("INTERIOR  ::  {}", w.planets[game.focus].name)
    } else {
        "LENS  ::  planet graph".to_string()
    };
    let secs = game.world.tick as f64 / BASE_TICKS_PER_SEC;
    let clock = format!("{:02}:{:02}", (secs as u64) / 60, (secs as u64) % 60);
    let frac = match game.fraction {
        FractionBucket::Quarter => "25%",
        FractionBucket::Half => "50%",
        FractionBucket::ThreeQuarter => "75%",
        FractionBucket::All => "100%",
    };
    let st = if game.paused { "PAUSED".to_string() } else { format!("{}x", fmt_speed(game.speed())) };
    let center = format!("{}   |   tick {}/{}   |   {}", layer, game.world.tick, game.level.horizon, clock);
    let cd = measure_text(&center, None, 20, 1.0);
    draw_text(&center, sw * 0.5 - cd.width * 0.5, 28.0, 20.0, HUD_MUTED);
    let center2 = format!("send {}   |   {}", frac, st);
    let c2d = measure_text(&center2, None, 20, 1.0);
    draw_text(&center2, sw * 0.5 - c2d.width * 0.5, 52.0, 20.0, HUD_TEXT);

    // Row 3 left: the level objective.
    let obj = format!("Goal: {}", game.level.objective);
    draw_text(&truncate_to(&obj, sw * 0.62), 16.0, 78.0, 18.0, ACCENT);

    // Row 3 right: automation status.
    if game.level.automation_available {
        let auto_n = game.automated.iter().filter(|&&a| a).count();
        let atxt = format!("automation: {} planet(s)  [A] toggle", auto_n);
        let ad = measure_text(&atxt, None, 18, 1.0);
        draw_text(&atxt, sw - ad.width - 16.0, 78.0, 18.0, if auto_n > 0 { AUTO_COL } else { HUD_MUTED });
    }

    // Bottom help line (context-sensitive).
    let in_interior = matches!(game.view, View::Interior(_)) && game.cam_t > 0.5;
    let help = if game.player_ai.is_some() {
        "DEMO (both seats AI)  |  wheel/Enter: zoom in  |  Esc/wheel: zoom out  |  P: pause  |  -/+: speed".to_string()
    } else if in_interior {
        "INTERIOR: click your sub then a sub: move  |  wheel/Esc: zoom out  |  A: automate  |  1-4: fraction  |  P: pause".to_string()
    } else {
        "LENS: click your planet then a linked planet: fleet  |  click/wheel: zoom in  |  A: automate  |  1-4: fraction  |  P: pause".to_string()
    };
    let hd = measure_text(&help, None, 16, 1.0);
    draw_rectangle(0.0, sh - HUD_BOTTOM_H, sw, HUD_BOTTOM_H, HUD_BG);
    draw_text(&help, (sw - hd.width) * 0.5, sh - 8.0, 16.0, HUD_MUTED);
}

/// (player_planets, enemy_planets): planets whose aggregate owner is each seat.
fn planet_tallies(w: &World) -> (usize, usize) {
    let mut p = 0;
    let mut e = 0;
    for i in 0..w.planets.len() {
        match w.planet_aggregate(i).owner {
            PlanetOwner::Owned(Faction::Player) => p += 1,
            PlanetOwner::Owned(Faction::Enemy) => e += 1,
            _ => {}
        }
    }
    (p, e)
}

/// Truncate `text` so it fits `width` px (appending an ellipsis if cut).
fn truncate_to(text: &str, width: f32) -> String {
    if measure_text(text, None, 18, 1.0).width <= width {
        return text.to_string();
    }
    let mut s = String::new();
    for ch in text.chars() {
        let trial = format!("{}{}", s, ch);
        if measure_text(&format!("{}...", trial), None, 18, 1.0).width > width {
            break;
        }
        s.push(ch);
    }
    format!("{}...", s)
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
    let out = game.world.outcome();
    let (text, col) = match winner {
        Faction::Player => ("VICTORY", PLAYER),
        Faction::Enemy => ("DEFEAT", ENEMY),
        Faction::Neutral => ("DRAW", NEUTRAL),
    };
    draw_rectangle(0.0, 0.0, sw, sh, Color::new(0.0, 0.0, 0.0, 0.6));
    draw_centered(text, sh * 0.40, 92, col);

    let how = if out.by_elimination {
        "by elimination".to_string()
    } else {
        format!("by lead at horizon (ships {}-{}, subs {}-{})", out.ships.0, out.ships.1, out.subs.0, out.subs.1)
    };
    draw_centered(&how, sh * 0.40 + 40.0, 24, HUD_TEXT);

    let prompt = match winner {
        Faction::Player => {
            if (game.level.id as usize) < 10 {
                "Enter: next level    |    Esc: level select"
            } else {
                "Campaign complete!  Enter: menu    |    Esc: level select"
            }
        }
        _ => "R: retry    |    Esc: level select",
    };
    draw_centered(prompt, sh * 0.40 + 92.0, 26, ACCENT);
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

fn window_conf() -> Conf {
    Conf {
        window_title: "Machine Intelligence — cell-game RTS".to_owned(),
        window_width: 1280,
        window_height: 800,
        high_dpi: true,
        ..Default::default()
    }
}

#[macroquad::main(window_conf)]
async fn main() {
    let cfg = parse_config();

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
    let mut app = App::new(&cfg);
    loop {
        let dt = get_frame_time() as f64;
        let quit = app_update(&mut app, dt);
        // The in-level Esc -> pause signal (set from input handling).
        if OPEN_PAUSE.with(|c| c.replace(false)) {
            if let AppState::InLevel { paused_menu, .. } = &mut app.state {
                *paused_menu = Some(0);
            }
        }
        app_draw(&app);
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
    let Mode::Shot { path, screen, level, view, at_tick, auto } = &cfg.mode else {
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
    });

    match screen {
        Some(ScreenTarget::Menu) => {
            app.state = AppState::MainMenu { idx: 0 };
        }
        Some(ScreenTarget::Select) => {
            app.state = AppState::LevelSelect { idx: 0 };
        }
        None => {
            // A level shot (default to level 1 if unspecified).
            let idx = level.unwrap_or(1).saturating_sub(1).min(app.levels.len() - 1);
            let force_view = *view;
            let mut game = Game::new(app.levels[idx].clone(), cfg.seed, *auto, force_view);
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
            game.render_alpha = 1.0;
            // Snap the camera to its target (no easing in a single-frame capture).
            game.cam_t = if matches!(game.view, View::Interior(_)) { 1.0 } else { 0.0 };
            app.state = AppState::InLevel { game: Box::new(game), paused_menu: None };
        }
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

/// Total sub-structures the player owns across every planet (the Layer-2 ownership signal).
fn total_player_subs(w: &World) -> usize {
    (0..w.planets.len()).map(|p| w.planet_aggregate(p).player_subs).sum()
}

/// Drive a level with BOTH seats AI to the end; return (final state hash, latched winner, end tick).
fn selftest_auto_to_end(level: Level, seed: u64) -> (u64, Option<Faction>, u64) {
    let mut g = Game::new(level, seed, true, None);
    let cap = g.level.horizon + 8; // safety bound; the match must end by `horizon`
    while !g.match_over() && g.world.tick < cap {
        g.step_one_tick();
    }
    (g.world.state_hash(), g.finished, g.world.tick)
}

/// Run the headless game-loop self-test. Returns true iff every check passed.
fn run_selftest() -> bool {
    let mut all_ok = true;
    let seed: u64 = 0xCE11_2042;
    println!("== game self-test (headless game-loop coverage) ==");

    // (1) Every campaign level: an auto match terminates, latches an outcome, and is
    //     deterministic on a rerun (same seed -> identical final state + winner).
    for level in levels::campaign() {
        let id = level.id;
        let title = level.title.clone();
        let horizon = level.horizon;
        let (h1, fin1, t1) = selftest_auto_to_end(level.clone(), seed);
        let (h2, fin2, _t2) = selftest_auto_to_end(level, seed);
        let ended = fin1.is_some();
        let within = t1 <= horizon;
        let deterministic = h1 == h2 && fin1 == fin2;
        let ok = ended && within && deterministic;
        all_ok &= ok;
        println!(
            "  L{:>2} {:<14} ended={} tick={}/{} winner={:?} det={} -> {}",
            id, title, ended, t1, horizon, fin1, deterministic,
            if ok { "PASS" } else { "FAIL" }
        );
    }

    // (2) Player basic-automation issues effective orders: on an automation level, a player that
    //     does NOTHING should not gain ground, but the same player with every owned planet set to
    //     AUTO should expand (capture sub-structures) via the greedy adapter.
    if let Some(level) = levels::campaign().into_iter().find(|l| l.automation_available) {
        let title = level.title.clone();
        let measure = |automate: bool| -> (usize, usize) {
            let mut g = Game::new(level.clone(), seed, false, None);
            if automate {
                for s in g.automated.iter_mut() {
                    *s = true;
                }
            }
            let start = total_player_subs(&g.world);
            let mut peak = start;
            let cap = g.level.horizon;
            while !g.match_over() && g.world.tick < cap {
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
            "  AUTOMATION '{}' player subs: start={} idle-peak={} auto-peak={} -> {}",
            title, start_off, peak_off, peak_on,
            if ok { "PASS" } else { "FAIL" }
        );
    } else {
        println!("  AUTOMATION: no automation-available level found -> FAIL");
        all_ok = false;
    }

    println!("== self-test: {} ==", if all_ok { "ALL PASS" } else { "FAILURES PRESENT" });
    all_ok
}
