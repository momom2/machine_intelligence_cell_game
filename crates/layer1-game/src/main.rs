//! # layer1-game — the real-time PLAYABLE Layer-1 renderer (the *spectacle* layer)
//!
//! This is the macroquad GUI over the headless [`layer1`] spatial sim. Per the design's
//! signature principle (`00-overview.md`, *decouple computation from spectacle*): **all** model
//! logic lives in the `layer1` library; this crate only (a) draws the structure, ships, and
//! battle bubbles, and (b) turns human input into `MoveOrder`s through the same public API the
//! AI uses. It changes nothing about the model — it constructs a GUI-specific [`SimParams`]
//! operating point (data it owns) to pace a human match, exactly as the brief permits.
//!
//! The human commands the **Player** seat; the Layer-1 [`Automaton`] commands the **Enemy** seat.
//!
//! ## Frame structure (real-time over a fixed-tick sim)
//! Each frame we accumulate wall-clock `dt`, then step the deterministic sim a whole number of
//! fixed ticks (so physics stays reproducible regardless of frame rate), running the Enemy
//! Automaton (and, in auto/shot modes, a Player Automaton) on a decision interval. Then we
//! handle input and draw. Pause and a speed multiplier scale how many ticks elapse per second.
//!
//! ## Modes
//! * default — human plays Player, Automaton plays Enemy.
//! * `--auto` (or `LAYER1_AUTO=1`) — both seats driven by the Automaton; hands-off demo.
//! * `--shot <path>` (or `LAYER1_SHOT=<path>`) — headless verification: both seats AI, run a
//!   few seconds of real-time play, capture the framebuffer to a PNG, and exit cleanly.

use layer1::{
    ai::Automaton,
    sample_params, sample_structure,
    types::{Faction, FractionBucket, MoveOrder},
    SampleLayout, SimParams, Structure,
};
use macroquad::prelude::*;

// ---------------------------------------------------------------------------------------------
// Pacing & tuning constants (the spectacle layer's operating point — see LAYER1_GAME.md)
// ---------------------------------------------------------------------------------------------

/// Base simulation rate in **ticks per (real) second** at 1x speed.
///
/// Pacing is the subtle part of this brief. Empirically (measured across 8 seeds) an
/// Automaton-vs-Automaton match on the sample structure resolves in ~50-170 ticks — *short*,
/// and dominated by the AI's flank/elimination seam rather than by combat lethality, so
/// softening combat barely lengthens it. To land a watchable **1-3 minute** match we therefore
/// step the sim *slowly* (≈1.8 ticks/s ⇒ ~90 ticks ≈ 50 s, longer games ≈ 60-100 s; a human
/// reacting tends to extend it) and keep motion fluid with **render-side interpolation** of
/// ship positions between ticks (the sim's determinism explicitly supports this — see
/// `LAYER1_SIM.md`). The brief's own guidance: tune the tick rate and/or scale the scenario.
const BASE_TICKS_PER_SEC: f64 = 1.8;

/// Available speed multipliers (cycled by -/+ and [ ]). 1x = `BASE_TICKS_PER_SEC`. Up to 4x for
/// impatient play; 0.5x to study a brawl in slow motion.
const SPEED_STEPS: [f64; 7] = [0.5, 1.0, 1.5, 2.0, 3.0, 4.0, 6.0];
/// Index into `SPEED_STEPS` the game starts at (1.0x).
const DEFAULT_SPEED_IDX: usize = 1;

/// Run the seat Automata every this-many ticks (matches the headless runner's cadence). Lets
/// forces commit over time instead of re-planning every tick.
const DECISION_INTERVAL: u64 = 5;

/// Safety bound on how many sim ticks we will advance in a single rendered frame, so a long
/// stall (or a huge speed) can never spiral the sim arbitrarily far in one frame. At 6x and
/// 1.8 base that is ~11 t/s, well under this cap on a normal frame.
const MAX_TICKS_PER_FRAME: u32 = 6;

/// `--shot` capture point: the sim **tick** to reach before grabbing the frame. Around here the
/// opening waves have collided at the central keep and a battle bubble is live (the AI-vs-AI
/// sample shows 2 bubbles by tick ~50), so the screenshot proves subs + ships + a bubble.
const SHOT_TARGET_TICK: u64 = 48;
/// Sim ticks to advance per frame while a `--shot` run races to `SHOT_TARGET_TICK` (independent
/// of wall-clock pacing, so the capture is fast and deterministic). A handful of frames render
/// before capture, enough for the framebuffer + pulsing effects to be mid-animation.
const SHOT_TICKS_PER_FRAME: u64 = 6;
/// Extra frames rendered after reaching the capture tick, so the framebuffer is fully drawn
/// before `get_screen_data`.
const SHOT_SETTLE_FRAMES: u32 = 2;

// ---------------------------------------------------------------------------------------------
// GUI-specific sim parameters
// ---------------------------------------------------------------------------------------------

/// The operating point the GUI paces a human match at. We start from the library's
/// [`sample_params`] (the canonical defaults) and only soften lethality a touch so fights last
/// long enough to *watch* (more simultaneous on-screen brawling, more reinforcement decisions)
/// — this is spectacle tuning of data we own, not a change to the model. Determinism is intact:
/// the sim still draws all randomness from its own seeded PRNG.
fn gui_params() -> SimParams {
    let mut p = sample_params();
    // Soften combat a little (default 0.035) so individual brawls last long enough to *read*
    // and battle bubbles linger on screen — purely spectacle. (It does not meaningfully change
    // match length, which is governed by the AI's capture/elimination dynamics, not lethality.)
    p.fire_prob = 0.020;
    p
}

// ---------------------------------------------------------------------------------------------
// Palette
// ---------------------------------------------------------------------------------------------

const BG: Color = Color::new(0.043, 0.055, 0.078, 1.0);
const GRID: Color = Color::new(0.10, 0.12, 0.16, 1.0);

const PLAYER: Color = Color::new(0.30, 0.72, 1.00, 1.0); // cyan/blue
const PLAYER_DIM: Color = Color::new(0.18, 0.42, 0.62, 1.0);
const ENEMY: Color = Color::new(1.00, 0.42, 0.30, 1.0); // red/orange
const ENEMY_DIM: Color = Color::new(0.62, 0.26, 0.20, 1.0);
const NEUTRAL: Color = Color::new(0.55, 0.58, 0.62, 1.0);
const NEUTRAL_DIM: Color = Color::new(0.30, 0.32, 0.35, 1.0);

const HUD_BG: Color = Color::new(0.0, 0.0, 0.0, 0.55);
const HUD_TEXT: Color = Color::new(0.90, 0.92, 0.96, 1.0);
const HUD_MUTED: Color = Color::new(0.62, 0.66, 0.72, 1.0);

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

// ---------------------------------------------------------------------------------------------
// World <-> screen camera (fits the structure's metre bounds to the window with margins)
// ---------------------------------------------------------------------------------------------

/// Maps the structure's arbitrary "metre" coordinates to screen pixels, fitting all
/// sub-structures (plus their radii) into the drawable area below the HUD with a margin.
/// Recomputed every frame, so window resizes are handled for free.
#[derive(Clone, Copy)]
struct Camera {
    scale: f32,
    off_x: f32,
    off_y: f32,
}

impl Camera {
    /// Fit `subs` bounds into the rect `[0,sw] x [top, sh]` (top leaves room for the HUD bar).
    fn fit(st: &Structure, sw: f32, sh: f32, top: f32) -> Camera {
        let (mut minx, mut miny, mut maxx, mut maxy) =
            (f32::INFINITY, f32::INFINITY, f32::NEG_INFINITY, f32::NEG_INFINITY);
        for s in &st.subs {
            minx = minx.min(s.pos.x - s.radius);
            miny = miny.min(s.pos.y - s.radius);
            maxx = maxx.max(s.pos.x + s.radius);
            maxy = maxy.max(s.pos.y + s.radius);
        }
        if !minx.is_finite() {
            // Degenerate (no subs): identity-ish camera centred on the screen.
            return Camera { scale: 1.0, off_x: sw * 0.5, off_y: sh * 0.5 };
        }
        let margin = 70.0_f32;
        let avail_w = (sw - 2.0 * margin).max(1.0);
        let avail_h = (sh - top - 2.0 * margin).max(1.0);
        let span_x = (maxx - minx).max(1e-3);
        let span_y = (maxy - miny).max(1e-3);
        let scale = (avail_w / span_x).min(avail_h / span_y);
        // Centre the fitted world inside the available rect.
        let world_cx = (minx + maxx) * 0.5;
        let world_cy = (miny + maxy) * 0.5;
        let screen_cx = sw * 0.5;
        let screen_cy = top + (sh - top) * 0.5;
        Camera {
            scale,
            // Note y is flipped: world y grows up, screen y grows down.
            off_x: screen_cx - world_cx * scale,
            off_y: screen_cy + world_cy * scale,
        }
    }

    #[inline]
    fn to_screen(&self, p: layer1::types::Vec2) -> (f32, f32) {
        (self.off_x + p.x * self.scale, self.off_y - p.y * self.scale)
    }

    /// Scale a metre length to pixels.
    #[inline]
    fn len(&self, m: f32) -> f32 {
        m * self.scale
    }
}

// ---------------------------------------------------------------------------------------------
// Run mode
// ---------------------------------------------------------------------------------------------

#[derive(Clone)]
enum Mode {
    /// Human plays Player, Automaton plays Enemy.
    Human,
    /// Both seats driven by the Automaton; never exits.
    Auto,
    /// Both seats AI; after `SHOT_PLAY_SECONDS` of play, write a PNG to `path` and exit.
    Shot { path: String },
}

struct Config {
    mode: Mode,
    seed: u64,
    /// Optional override for the `--shot` capture tick (env `LAYER1_SHOT_TICK`). A very large
    /// value captures the end-of-match banner. Defaults to [`SHOT_TARGET_TICK`].
    shot_tick: u64,
}

/// Parse CLI flags and env vars into a [`Config`]. Recognised:
/// `--shot <path>` / `LAYER1_SHOT=<path>`, `--auto` / `LAYER1_AUTO=1`, `--seed <hex|dec>`.
fn parse_config() -> Config {
    let mut mode = Mode::Human;
    let mut seed: u64 = 0xC0FFEE_1234;
    let mut shot_tick: u64 = SHOT_TARGET_TICK;
    if let Ok(s) = std::env::var("LAYER1_SHOT_TICK") {
        if let Ok(v) = s.trim().parse::<u64>() {
            shot_tick = v;
        }
    }

    // Env first (so a CLI flag can still override).
    if let Ok(p) = std::env::var("LAYER1_SHOT") {
        if !p.is_empty() {
            mode = Mode::Shot { path: p };
        }
    }
    if std::env::var("LAYER1_AUTO").map(|v| v == "1" || v == "true").unwrap_or(false) {
        if !matches!(mode, Mode::Shot { .. }) {
            mode = Mode::Auto;
        }
    }
    if let Ok(s) = std::env::var("LAYER1_SEED") {
        if let Some(v) = parse_seed(&s) {
            seed = v;
        }
    }

    let args: Vec<String> = std::env::args().collect();
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--shot" => {
                if i + 1 < args.len() {
                    mode = Mode::Shot { path: args[i + 1].clone() };
                    i += 1;
                } else {
                    // Default path if none supplied.
                    mode = Mode::Shot { path: "target/layer1_shot.png".to_string() };
                }
            }
            "--auto" => {
                if !matches!(mode, Mode::Shot { .. }) {
                    mode = Mode::Auto;
                }
            }
            "--seed" => {
                if i + 1 < args.len() {
                    if let Some(v) = parse_seed(&args[i + 1]) {
                        seed = v;
                    }
                    i += 1;
                }
            }
            _ => {}
        }
        i += 1;
    }
    Config { mode, seed, shot_tick }
}

fn parse_seed(s: &str) -> Option<u64> {
    let s = s.trim();
    if let Some(hex) = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
        u64::from_str_radix(hex, 16).ok()
    } else {
        s.parse::<u64>().ok()
    }
}

// ---------------------------------------------------------------------------------------------
// Game state (the renderer's own state; the *sim* state lives in `Structure`)
// ---------------------------------------------------------------------------------------------

struct Game {
    st: Structure,
    layout: SampleLayout,
    params: SimParams,
    seed: u64,

    enemy_ai: Automaton,
    /// Present in Auto/Shot modes (both seats AI). `None` => human plays Player.
    player_ai: Option<Automaton>,

    paused: bool,
    speed_idx: usize,
    tick_accum: f64,

    /// Ship positions captured **just before** the most recent tick, indexed by `ShipId`
    /// (indices are stable for the run). Used to interpolate motion between ticks so a slow
    /// sim rate still looks fluid. Ships with no entry (spawned this tick) draw at current pos.
    prev_pos: Vec<layer1::types::Vec2>,
    /// Fractional progress toward the next tick in `[0,1)` — the interpolation alpha for drawing.
    render_alpha: f32,

    /// Current fraction bucket for human orders (default Half).
    fraction: FractionBucket,
    /// Currently selected source sub (must be Player-owned). `None` => nothing selected.
    selected: Option<usize>,
    /// While the left button is held after pressing on a Player sub: the drag source.
    drag_source: Option<usize>,

    /// Wall-clock seconds of *simulated* play (advances only while unpaused), for the shot clock.
    play_seconds: f64,
    /// Frame countdown used by `--shot` to settle the framebuffer before capture.
    shot_state: ShotState,
}

#[derive(Clone, Copy, PartialEq)]
enum ShotState {
    /// Not a shot run, or still playing toward the capture point.
    Playing,
    /// Reached the capture tick; render `n` more frames then grab.
    Settling(u32),
    /// Captured; main loop should exit.
    Done,
}

impl Game {
    fn new(seed: u64, mode: &Mode) -> Game {
        let (st, layout) = sample_structure(seed);
        let params = gui_params();
        let enemy_ai = Automaton::new(Faction::Enemy);
        let player_ai = match mode {
            Mode::Human => None,
            Mode::Auto | Mode::Shot { .. } => Some(Automaton::new(Faction::Player)),
        };
        Game {
            st,
            layout,
            params,
            seed,
            enemy_ai,
            player_ai,
            paused: false,
            speed_idx: DEFAULT_SPEED_IDX,
            tick_accum: 0.0,
            prev_pos: Vec::new(),
            render_alpha: 0.0,
            fraction: FractionBucket::Half,
            selected: None,
            drag_source: None,
            play_seconds: 0.0,
            shot_state: if matches!(mode, Mode::Shot { .. }) {
                ShotState::Playing
            } else {
                ShotState::Done // not used outside shot mode
            },
        }
    }

    /// Rebuild the scenario with a fresh seed (R restart): each game differs but stays
    /// reproducible. Preserves the human's chosen fraction/speed and the run mode's AI seats.
    fn restart(&mut self) {
        self.seed = self.seed.wrapping_add(1);
        let (st, layout) = sample_structure(self.seed);
        self.st = st;
        self.layout = layout;
        self.tick_accum = 0.0;
        self.prev_pos = Vec::new();
        self.render_alpha = 0.0;
        self.play_seconds = 0.0;
        self.selected = None;
        self.drag_source = None;
        self.paused = false;
        // Keep enemy_ai/player_ai (Automaton is stateless) and fraction/speed as-is.
    }

    fn speed(&self) -> f64 {
        SPEED_STEPS[self.speed_idx]
    }

    fn match_over(&self) -> bool {
        self.st.is_eliminated(Faction::Player) || self.st.is_eliminated(Faction::Enemy)
    }

    /// Issue both seats' Automaton orders for one decision tick. Player issues first (the
    /// library's documented tie-break). Only used when a seat is AI-controlled.
    fn ai_decide_and_issue(&mut self) {
        if let Some(pa) = &self.player_ai {
            let orders = pa.decide(&self.st, &self.params);
            for o in orders {
                self.st.issue_order(o);
            }
        }
        let orders = self.enemy_ai.decide(&self.st, &self.params);
        for o in orders {
            self.st.issue_order(o);
        }
    }

    /// Advance the sim by exactly one tick, running the decision step on the cadence first.
    /// Snapshots ship positions just before stepping so the renderer can interpolate motion.
    fn step_one_tick(&mut self) {
        if self.st.tick % DECISION_INTERVAL == 0 {
            self.ai_decide_and_issue();
        }
        // Capture pre-step positions (indexed by ShipId) for render interpolation.
        self.prev_pos.clear();
        self.prev_pos.extend(self.st.ships.iter().map(|s| s.pos));
        self.st.step(&self.params);
    }

    /// Advance simulation for one rendered frame's worth of wall-clock `dt` (seconds).
    fn update(&mut self, dt: f64) {
        if self.paused || self.match_over() {
            // Freeze on the exact current positions (no interpolation past the last tick).
            self.render_alpha = 1.0;
            return;
        }
        self.tick_accum += dt * BASE_TICKS_PER_SEC * self.speed();
        let mut budget = MAX_TICKS_PER_FRAME;
        while self.tick_accum >= 1.0 && budget > 0 {
            self.step_one_tick();
            self.tick_accum -= 1.0;
            budget -= 1;
            self.play_seconds += 1.0 / BASE_TICKS_PER_SEC;
            if self.match_over() {
                break;
            }
        }
        // If we hit the per-frame budget, drop the backlog so we don't fast-forward later.
        if budget == 0 {
            self.tick_accum = self.tick_accum.min(1.0);
        }
        // Leftover fraction toward the next tick = interpolation alpha for this frame's draw.
        self.render_alpha = (self.tick_accum.clamp(0.0, 1.0)) as f32;
    }

    /// Position to draw ship `id` at this frame: linear interp between its pre-step position
    /// and current position by `render_alpha`. Falls back to current pos for ships with no
    /// snapshot (spawned this tick) or when interpolation is disabled.
    #[inline]
    fn draw_pos(&self, id: usize) -> layer1::types::Vec2 {
        let cur = self.st.ships[id].pos;
        match self.prev_pos.get(id) {
            Some(prev) => layer1::types::Vec2 {
                x: prev.x + (cur.x - prev.x) * self.render_alpha,
                y: prev.y + (cur.y - prev.y) * self.render_alpha,
            },
            None => cur,
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Input (human seat only)
// ---------------------------------------------------------------------------------------------

/// Find the sub-structure under a screen point (within its drawn radius). Picks the nearest
/// centre if several overlap. Returns `None` if the click is outside every sub.
fn sub_at_screen(st: &Structure, cam: &Camera, mx: f32, my: f32) -> Option<usize> {
    let mut best: Option<(usize, f32)> = None;
    for (i, s) in st.subs.iter().enumerate() {
        let (sx, sy) = cam.to_screen(s.pos);
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

fn handle_input(game: &mut Game, cam: &Camera) {
    // --- Global controls (work in every mode) ---
    if is_key_pressed(KeyCode::Space) {
        game.paused = !game.paused;
    }
    if is_key_pressed(KeyCode::R) {
        game.restart();
    }
    // Speed down/up: '-' or '[' slower; '=' (the +/= key) or ']' faster.
    if is_key_pressed(KeyCode::Minus)
        || is_key_pressed(KeyCode::KpSubtract)
        || is_key_pressed(KeyCode::LeftBracket)
    {
        game.speed_idx = game.speed_idx.saturating_sub(1);
    }
    if is_key_pressed(KeyCode::Equal)
        || is_key_pressed(KeyCode::KpAdd)
        || is_key_pressed(KeyCode::RightBracket)
    {
        game.speed_idx = (game.speed_idx + 1).min(SPEED_STEPS.len() - 1);
    }

    // Fraction buckets (only meaningful for the human, but harmless otherwise).
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

    // Clear selection.
    if is_key_pressed(KeyCode::Escape) || is_mouse_button_pressed(MouseButton::Right) {
        game.selected = None;
        game.drag_source = None;
    }

    // Human-seat pointer orders are disabled while an Automaton drives the Player seat.
    if game.player_ai.is_some() {
        return;
    }

    let (mx, my) = mouse_position();

    // Press: begin a possible drag from a Player-owned sub, and/or set selection.
    if is_mouse_button_pressed(MouseButton::Left) {
        if let Some(sub) = sub_at_screen(&game.st, cam, mx, my) {
            if game.st.subs[sub].owner == Faction::Player {
                // Pressing a Player sub (re)selects it as the source and arms a drag.
                game.selected = Some(sub);
                game.drag_source = Some(sub);
            } else if let Some(src) = game.selected {
                // A source is already selected and we clicked a non-Player sub: issue the order.
                game.st.issue_order(MoveOrder::new(src, sub, game.fraction));
                // Keep the source selected for rapid repeat orders.
                game.drag_source = None;
            }
        } else {
            // Click on empty space clears selection.
            game.selected = None;
            game.drag_source = None;
        }
    }

    // Release: if we were dragging from a Player sub and land on a *different* sub, order it.
    if is_mouse_button_released(MouseButton::Left) {
        if let Some(src) = game.drag_source {
            if let Some(tgt) = sub_at_screen(&game.st, cam, mx, my) {
                if tgt != src {
                    game.st.issue_order(MoveOrder::new(src, tgt, game.fraction));
                    // Leave `src` selected so click+click and drag both feel consistent.
                }
            }
            game.drag_source = None;
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Rendering
// ---------------------------------------------------------------------------------------------

const HUD_TOP_H: f32 = 56.0;

fn draw_scene(game: &Game, cam: &Camera) {
    let t = get_time() as f32;

    // Subtle reference grid in metre space (every 10 m), so motion reads clearly.
    draw_metre_grid(&game.st, cam);

    // --- Sub-structures ---
    for (i, s) in game.st.subs.iter().enumerate() {
        let (sx, sy) = cam.to_screen(s.pos);
        let r = cam.len(s.radius).max(8.0);
        let base = faction_color(s.owner);
        let dim = faction_dim(s.owner);

        // Filled disk (dim) + ring (bright) so it reads as a "structure", not a blob.
        draw_circle(sx, sy, r, Color::new(dim.r, dim.g, dim.b, 0.35));
        draw_circle_lines(sx, sy, r, 2.0, base);

        // Production indicator: a thin arc filling toward the next spawn (owned subs only).
        if s.owner.is_real() {
            let frac = production_fraction(s.production_timer, game.params.production_period);
            draw_progress_ring(sx, sy, r + 4.0, frac, base);
        }

        // Selected source: a bright animated outline.
        if game.selected == Some(i) {
            let pulse = 0.5 + 0.5 * (t * 4.0).sin();
            draw_circle_lines(sx, sy, r + 7.0, 2.5, Color::new(1.0, 1.0, 1.0, 0.5 + 0.4 * pulse));
            draw_circle_lines(sx, sy, r + 11.0, 1.5, Color::new(1.0, 1.0, 1.0, 0.25 * pulse));
        }

        // Idle ship count, big and centred.
        let idle = game.st.idle_count_at(i, s.owner);
        let label = idle.to_string();
        let fs = (r * 0.95).clamp(16.0, 40.0) as u16;
        let dims = measure_text(&label, None, fs, 1.0);
        draw_text(
            &label,
            sx - dims.width * 0.5,
            sy + dims.height * 0.35,
            fs as f32,
            Color::new(0.97, 0.98, 1.0, 0.95),
        );
    }

    // --- Ships (interpolated positions for fluid motion at a low sim rate) ---
    // Idle ships as small dots; moving ships as triangles pointed along velocity.
    for id in 0..game.st.ships.len() {
        let sh = &game.st.ships[id];
        if !sh.alive {
            continue;
        }
        let pos = game.draw_pos(id);
        let (sx, sy) = cam.to_screen(pos);
        let col = faction_color(sh.faction);
        match sh.target {
            None => {
                draw_circle(sx, sy, 2.4, col);
            }
            Some(_) => {
                // Direction = unit vector toward the aim point (screen space, y flipped).
                let dx = sh.aim.x - pos.x;
                let dy = sh.aim.y - pos.y;
                let len = (dx * dx + dy * dy).sqrt().max(1e-3);
                let (ux, uy) = (dx / len, -dy / len); // flip y for screen
                draw_ship_triangle(sx, sy, ux, uy, col);
            }
        }
    }

    // --- Battle bubbles ---
    let bubbles = game.st.battle_bubbles(&game.params);
    for b in &bubbles {
        let (cx, cy) = cam.to_screen(b.center);
        let rad = cam.len(b.radius).max(14.0);
        // Pulsing translucent disk + ring.
        let pulse = 0.5 + 0.5 * (t * 6.0 + cx * 0.01).sin();
        let fill = 0.08 + 0.06 * pulse;
        draw_circle(cx, cy, rad + 6.0, Color::new(1.0, 0.85, 0.35, fill));
        draw_circle_lines(cx, cy, rad + 6.0, 2.0, Color::new(1.0, 0.8, 0.3, 0.5 + 0.3 * pulse));

        // Per-side engaged counts above the bubble.
        let txt = format!("{} vs {}", b.player_count, b.enemy_count);
        let dims = measure_text(&txt, None, 20, 1.0);
        draw_text(&txt, cx - dims.width * 0.5, cy - rad - 10.0, 20.0, Color::new(1.0, 0.95, 0.8, 0.95));
    }
    // Cheap cosmetic "tracer" flashes between nearby opposing ships inside bubbles.
    draw_tracers(game, cam, &bubbles, t);
}

/// Faint metre grid (every 10 units) to give the eye a frame of reference.
fn draw_metre_grid(st: &Structure, cam: &Camera) {
    let (mut minx, mut miny, mut maxx, mut maxy) =
        (f32::INFINITY, f32::INFINITY, f32::NEG_INFINITY, f32::NEG_INFINITY);
    for s in &st.subs {
        minx = minx.min(s.pos.x - s.radius);
        miny = miny.min(s.pos.y - s.radius);
        maxx = maxx.max(s.pos.x + s.radius);
        maxy = maxy.max(s.pos.y + s.radius);
    }
    if !minx.is_finite() {
        return;
    }
    let step = 10.0_f32;
    let x0 = (minx / step).floor() * step - step;
    let x1 = (maxx / step).ceil() * step + step;
    let y0 = (miny / step).floor() * step - step;
    let y1 = (maxy / step).ceil() * step + step;
    let mut x = x0;
    while x <= x1 {
        let (sx, sa) = cam.to_screen(layer1::types::Vec2 { x, y: y0 });
        let (_, sb) = cam.to_screen(layer1::types::Vec2 { x, y: y1 });
        draw_line(sx, sa, sx, sb, 1.0, GRID);
        x += step;
    }
    let mut y = y0;
    while y <= y1 {
        let (sa, sy) = cam.to_screen(layer1::types::Vec2 { x: x0, y });
        let (sb, _) = cam.to_screen(layer1::types::Vec2 { x: x1, y });
        draw_line(sa, sy, sb, sy, 1.0, GRID);
        y += step;
    }
}

/// 0.0 just spawned -> 1.0 about to spawn. The timer counts DOWN from `period` to 0.
fn production_fraction(timer: u32, period: u32) -> f32 {
    if period == 0 {
        return 0.0;
    }
    let remaining = timer as f32 / period as f32;
    (1.0 - remaining).clamp(0.0, 1.0)
}

/// Draw a partial ring (arc) from the top, clockwise, filling `frac` of the circle.
fn draw_progress_ring(cx: f32, cy: f32, r: f32, frac: f32, col: Color) {
    let frac = frac.clamp(0.0, 1.0);
    if frac <= 0.0 {
        return;
    }
    let segs = 48;
    let n = (segs as f32 * frac).ceil() as i32;
    let start = -std::f32::consts::FRAC_PI_2; // top
    let c = Color::new(col.r, col.g, col.b, 0.85);
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

/// A small isoceles triangle centred at (cx,cy) pointing along unit vector (ux,uy).
fn draw_ship_triangle(cx: f32, cy: f32, ux: f32, uy: f32, col: Color) {
    let nose = 6.0_f32;
    let back = 3.5_f32;
    // Perpendicular for the two rear corners.
    let (px, py) = (-uy, ux);
    let tip = Vec2::new(cx + ux * nose, cy + uy * nose);
    let l = Vec2::new(cx - ux * back + px * back, cy - uy * back + py * back);
    let r = Vec2::new(cx - ux * back - px * back, cy - uy * back - py * back);
    draw_triangle(tip, l, r, col);
}

/// Cheap combat feel: for each bubble, flash a few short lines between random-ish nearby
/// opposing ship pairs. Purely cosmetic — derived from positions, no sim mutation.
fn draw_tracers(game: &Game, cam: &Camera, bubbles: &[layer1::BattleBubble], t: f32) {
    let r = game.params.engagement_radius;
    let r2 = r * r;
    for (bi, b) in bubbles.iter().enumerate() {
        // A small per-bubble time-varying "fire rate": only draw on some frames per bubble.
        let phase = (t * 9.0 + bi as f32 * 1.7).sin();
        if phase < 0.2 {
            continue;
        }
        let mut drawn = 0;
        // Walk member ids; connect the first few opposing in-range pairs.
        for (ai, &a) in b.ships.iter().enumerate() {
            if drawn >= 6 {
                break;
            }
            let sa = &game.st.ships[a];
            if !sa.alive || sa.faction != Faction::Player {
                continue;
            }
            for &c in b.ships.iter().skip(ai + 1) {
                let sc = &game.st.ships[c];
                if !sc.alive || sc.faction == sa.faction {
                    continue;
                }
                if sa.pos.dist_sq(sc.pos) <= r2 {
                    let (x0, y0) = cam.to_screen(game.draw_pos(a));
                    let (x1, y1) = cam.to_screen(game.draw_pos(c));
                    let a = 0.20 + 0.30 * (phase);
                    draw_line(x0, y0, x1, y1, 1.0, Color::new(1.0, 0.95, 0.7, a.clamp(0.0, 0.6)));
                    drawn += 1;
                    if drawn >= 6 {
                        break;
                    }
                }
            }
        }
    }
}

fn draw_hud(game: &Game) {
    let sw = screen_width();
    let sh = screen_height();

    // --- Top bar ---
    draw_rectangle(0.0, 0.0, sw, HUD_TOP_H, HUD_BG);

    let p_ships = game.st.ship_count(Faction::Player);
    let e_ships = game.st.ship_count(Faction::Enemy);
    let p_subs = game.st.sub_count(Faction::Player);
    let e_subs = game.st.sub_count(Faction::Enemy);

    // Player block (left).
    draw_text("PLAYER", 16.0, 22.0, 22.0, PLAYER);
    draw_text(
        &format!("ships {:>3}   subs {}", p_ships, p_subs),
        16.0,
        44.0,
        20.0,
        HUD_TEXT,
    );

    // Enemy block (right).
    let etxt1 = "ENEMY";
    let e1 = measure_text(etxt1, None, 22, 1.0);
    draw_text(etxt1, sw - e1.width - 16.0, 22.0, 22.0, ENEMY);
    let etxt2 = format!("ships {:>3}   subs {}", e_ships, e_subs);
    let e2 = measure_text(&etxt2, None, 20, 1.0);
    draw_text(&etxt2, sw - e2.width - 16.0, 44.0, 20.0, HUD_TEXT);

    // Centre block: clock, fraction, speed/pause.
    let secs = game.play_seconds;
    let clock = format!("{:02}:{:02}", (secs as u64) / 60, (secs as u64) % 60);
    let frac = match game.fraction {
        FractionBucket::Quarter => "25%",
        FractionBucket::Half => "50%",
        FractionBucket::ThreeQuarter => "75%",
        FractionBucket::All => "100%",
    };
    let state = if game.paused { "PAUSED".to_string() } else { format!("{}x", fmt_speed(game.speed())) };
    let center = format!("tick {}  |  {}  |  send {}  |  {}", game.st.tick, clock, frac, state);
    let cd = measure_text(&center, None, 20, 1.0);
    draw_text(&center, sw * 0.5 - cd.width * 0.5, 30.0, 20.0, HUD_MUTED);

    // --- Bottom help line ---
    let help = "L-click/drag: select & send  |  R-click/Esc: clear  |  1-4: 25/50/75/100%  |  Space: pause  |  -/+ or [ ]: speed  |  R: restart";
    let hd = measure_text(help, None, 18, 1.0);
    let bar_h = 26.0;
    draw_rectangle(0.0, sh - bar_h, sw, bar_h, HUD_BG);
    draw_text(help, (sw - hd.width) * 0.5, sh - 8.0, 18.0, HUD_MUTED);

    // --- End-of-match banner ---
    if game.match_over() {
        let out = game.st.outcome();
        let (text, col) = match out.winner {
            Some(Faction::Player) => ("VICTORY", PLAYER),
            Some(Faction::Enemy) => ("DEFEAT", ENEMY),
            _ => ("DRAW", NEUTRAL),
        };
        // Dim the field.
        draw_rectangle(0.0, 0.0, sw, sh, Color::new(0.0, 0.0, 0.0, 0.45));
        let fs = 96u16;
        let d = measure_text(text, None, fs, 1.0);
        draw_text(text, sw * 0.5 - d.width * 0.5, sh * 0.5, fs as f32, col);
        let sub = if out.by_elimination { "by elimination" } else { "by lead at horizon" };
        let sd = measure_text(sub, None, 26, 1.0);
        draw_text(sub, sw * 0.5 - sd.width * 0.5, sh * 0.5 + 36.0, 26.0, HUD_TEXT);
        let r = "press R to restart";
        let rd = measure_text(r, None, 28, 1.0);
        draw_text(r, sw * 0.5 - rd.width * 0.5, sh * 0.5 + 80.0, 28.0, HUD_MUTED);
    }
}

fn fmt_speed(s: f64) -> String {
    if (s.fract()).abs() < 1e-6 {
        format!("{}", s as i64)
    } else {
        format!("{}", s)
    }
}

// ---------------------------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------------------------

fn window_conf() -> Conf {
    Conf {
        window_title: "Layer 1".to_owned(),
        window_width: 1280,
        window_height: 800,
        high_dpi: true,
        ..Default::default()
    }
}

#[macroquad::main(window_conf)]
async fn main() {
    let cfg = parse_config();
    let mut game = Game::new(cfg.seed, &cfg.mode);

    loop {
        let sw = screen_width();
        let sh = screen_height();
        let cam = Camera::fit(&game.st, sw, sh, HUD_TOP_H);

        // Input (no-op for pointer in auto/shot, but keeps Space/speed working everywhere).
        handle_input(&mut game, &cam);

        // Advance the sim. In `--shot` we race to a fixed target tick independent of frame
        // timing (deterministic, fast capture); otherwise we pace by wall-clock dt.
        if matches!(cfg.mode, Mode::Shot { .. }) && game.shot_state == ShotState::Playing {
            let mut n = SHOT_TICKS_PER_FRAME;
            while n > 0 && game.st.tick < cfg.shot_tick && !game.match_over() {
                game.step_one_tick();
                n -= 1;
            }
            game.render_alpha = 1.0; // exact positions for the capture
        } else {
            let dt = get_frame_time() as f64;
            game.update(dt);
        }

        // Draw.
        clear_background(BG);
        draw_scene(&game, &cam);
        draw_hud(&game);

        // --- Shot-mode capture state machine ---
        if let Mode::Shot { path } = &cfg.mode {
            match game.shot_state {
                ShotState::Playing => {
                    if game.st.tick >= cfg.shot_tick || game.match_over() {
                        game.shot_state = ShotState::Settling(SHOT_SETTLE_FRAMES);
                    }
                }
                ShotState::Settling(0) => {
                    // Capture the fully-rendered frame, write PNG, then exit next loop.
                    let img = get_screen_data();
                    // Best-effort: make sure the parent dir exists.
                    if let Some(parent) = std::path::Path::new(path).parent() {
                        let _ = std::fs::create_dir_all(parent);
                    }
                    img.export_png(path);
                    println!(
                        "[layer1-game] shot written: {} ({}x{})  tick={}",
                        path,
                        img.width(),
                        img.height(),
                        game.st.tick
                    );
                    game.shot_state = ShotState::Done;
                }
                ShotState::Settling(n) => {
                    game.shot_state = ShotState::Settling(n - 1);
                }
                ShotState::Done => {
                    // Exit cleanly after the capture frame has been presented.
                    break;
                }
            }
        }

        next_frame().await;
    }
}
