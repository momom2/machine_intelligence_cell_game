//! # layer2-game — the real-time PLAYABLE Layer-2 (tactical) renderer
//!
//! This is the macroquad GUI over the headless [`cell_core`] deterministic mean-field
//! graph engine, with a HIDDEN-mix [`automaton`] Automaton as the opponent. Per the
//! design's signature principle (`00-overview.md`, *decouple computation from
//! spectacle*): **all** model logic lives in `cell-core`/`automaton`; this crate only
//! (a) draws the graph — nodes, edges, fleets streaming along edges, combat flashes —
//! and (b) turns human input into `cell_core::Command`s through the exact same engine
//! API the AI uses ([`GameState::launch_with`]). It changes nothing about the model:
//! it constructs a layout (node coordinates the engine does not carry) and a GUI tick
//! pace, data this crate owns, exactly as the brief permits.
//!
//! The human commands seat **A** (Player, cyan); the [`automaton`] Automaton commands
//! seat **B** (Enemy, red). The Automaton is a hidden, fixed colonize/defend/attack
//! mix; the headline experience (`02-ai-opponents.md`) is to **scout it, infer its
//! lean from the legible HUD features, and counter it** — then see the *true* mix
//! REVEALED on the end screen (the "glass genome" reward).
//!
//! ## Frame structure (real-time over a fixed-tick engine)
//! Each frame we accumulate wall-clock `dt`, then step the deterministic engine a whole
//! number of fixed ticks (so the simulation stays reproducible regardless of frame
//! rate). Each tick we (1) let the human's queued `Command`s and the enemy policy's
//! commands launch via `launch_with` — exactly mirroring `cell_core`'s own
//! `run_match` order (A before B) — then (2) `step` the engine one tick. Fleet motion
//! is interpolated between ticks by `progress / length` so a slow tick rate still looks
//! fluid. Pause and a speed multiplier scale how many ticks elapse per second.
//!
//! ## Modes (mirroring layer1-game)
//! * default — human plays A, the hidden-mix Automaton plays B.
//! * `--auto` (or `LAYER2_AUTO=1`) — both seats driven by Automatons; hands-off demo.
//! * `--shot <path>` (or `LAYER2_SHOT=<path>`) — headless verification: both seats AI,
//!   race deterministically to a target tick, capture the framebuffer to a PNG, exit.

use automaton::{compile, Mix};
use cell_core::dsl::strategies;
use cell_core::maps::{all_maps, MapSpec};
use cell_core::{
    Command, DslPolicy, FractionBucket, GameState, OwnerFeatures, Owner, Params, Policy,
};
use macroquad::prelude::*;

// ---------------------------------------------------------------------------------------------
// Pacing & tuning constants (the spectacle layer's operating point — see LAYER2_GAME.md)
// ---------------------------------------------------------------------------------------------

/// Base engine rate in **ticks per (real) second** at 1x speed.
///
/// `cell-core` matches run to elimination or a horizon of [`HORIZON`] ticks. To land a
/// watchable **1-3 minute** match we step the engine at ~5 ticks/s: a full
/// 600-tick game then lasts ~120 s (2 min), elimination games are shorter, and a human
/// stalling the enemy tends to push toward the longer end. Fleet motion stays fluid via
/// **render-side interpolation** of `progress / length` between ticks (the engine's
/// determinism explicitly supports this — see `cell-core` lib docs).
const BASE_TICKS_PER_SEC: f64 = 5.0;

/// Match horizon in ticks (mirrors the headless capstone/R2 horizon). If neither side is
/// eliminated by here, the leader by combined unit+territory share wins (engine's rule).
const HORIZON: u64 = 600;

/// Available speed multipliers (cycled by -/+ and [ ]). 1x = `BASE_TICKS_PER_SEC`.
const SPEED_STEPS: [f64; 7] = [0.5, 1.0, 1.5, 2.0, 3.0, 4.0, 6.0];
/// Index into `SPEED_STEPS` the game starts at (1.0x).
const DEFAULT_SPEED_IDX: usize = 1;

/// Safety bound on engine ticks advanced in a single rendered frame, so a stall (or a
/// huge speed) can never spiral the engine arbitrarily far in one frame.
const MAX_TICKS_PER_FRAME: u32 = 8;

/// The R2-validated operating point: the region where the colonize/defend/attack triad
/// is a genuine rock-paper-scissors cycle (`R2_RESULTS.md`, reused by the capstone).
fn operating_params() -> Params {
    Params { r: 0.6, k: 2.25, l: 0.15, ..Params::default() }
}

// `--shot` capture: race deterministically to this tick, then settle a couple of frames.
/// Engine tick the `--shot` run reaches before grabbing the frame. Far enough in that
/// both sides have expanded into the neutral middle and fleets are in transit, so the
/// screenshot proves nodes + edges + fleet streams + the HUD inference readout.
const SHOT_TARGET_TICK: u64 = 90;
/// Engine ticks advanced per frame while a `--shot` run races to its target tick
/// (independent of wall-clock pacing, so capture is fast and deterministic).
const SHOT_TICKS_PER_FRAME: u64 = 6;
/// Extra frames rendered after reaching the capture tick, so the framebuffer (and the
/// pulsing effects) are fully drawn before `get_screen_data`.
const SHOT_SETTLE_FRAMES: u32 = 2;

// ---------------------------------------------------------------------------------------------
// Palette (mirrors layer1-game's dark minimalist look)
// ---------------------------------------------------------------------------------------------

const BG: Color = Color::new(0.043, 0.055, 0.078, 1.0);

const PLAYER: Color = Color::new(0.30, 0.72, 1.00, 1.0); // cyan/blue (seat A)
const PLAYER_DIM: Color = Color::new(0.18, 0.42, 0.62, 1.0);
const ENEMY: Color = Color::new(1.00, 0.42, 0.30, 1.0); // red/orange (seat B)
const ENEMY_DIM: Color = Color::new(0.62, 0.26, 0.20, 1.0);
const NEUTRAL: Color = Color::new(0.55, 0.58, 0.62, 1.0);
const NEUTRAL_DIM: Color = Color::new(0.30, 0.32, 0.35, 1.0);

const EDGE_COL: Color = Color::new(0.22, 0.26, 0.32, 1.0);
const EDGE_HILITE: Color = Color::new(0.55, 0.85, 1.00, 0.9); // valid-neighbor edge

const HUD_BG: Color = Color::new(0.0, 0.0, 0.0, 0.55);
const HUD_TEXT: Color = Color::new(0.90, 0.92, 0.96, 1.0);
const HUD_MUTED: Color = Color::new(0.62, 0.66, 0.72, 1.0);

fn owner_color(o: Owner) -> Color {
    match o {
        Owner::A => PLAYER,
        Owner::B => ENEMY,
        Owner::Neutral => NEUTRAL,
    }
}
fn owner_dim(o: Owner) -> Color {
    match o {
        Owner::A => PLAYER_DIM,
        Owner::B => ENEMY_DIM,
        Owner::Neutral => NEUTRAL_DIM,
    }
}

// ---------------------------------------------------------------------------------------------
// Node layout — the engine carries graph topology + production_mult but NO coordinates.
// We supply per-node (x, y) in an abstract space matching each map's ASCII diagram in
// `cell_core::maps`; the camera fits this space to the window. This is pure spectacle
// data this crate owns; it never feeds back into the deterministic model.
// ---------------------------------------------------------------------------------------------

/// 2D coordinates (abstract units) for every node of a map, indexed by `NodeId`.
fn layout_for(name: &str, n_nodes: usize) -> Vec<(f32, f32)> {
    match name {
        // corridor7:  A(0)-P(1)-N(2)-N(3)-N(4)-P(5)-B(6)  — a straight line.
        "corridor7" => (0..n_nodes).map(|i| (i as f32, 0.0)).collect(),

        // twin_bridge8:
        //          N(2) -- N(3)
        //         /          \
        //    A(0)-P(1)        P(6)-B(7)
        //         \          /
        //          N(4) -- N(5)
        "twin_bridge8" => vec![
            (0.0, 0.0),  // 0 home A
            (1.0, 0.0),  // 1 A private
            (2.0, -1.0), // 2 top near A
            (3.0, -1.0), // 3 top near B
            (2.0, 1.0),  // 4 bottom near A
            (3.0, 1.0),  // 5 bottom near B
            (4.0, 0.0),  // 6 B private
            (5.0, 0.0),  // 7 home B
        ],

        // star9:
        //    P(1)            P(7)
        //       \            /
        //    A(0)-- C(3)-C(4)-C(5) --B(8)
        //       /            \
        //    P(2)            P(6)
        "star9" => vec![
            (0.0, 0.0),   // 0 home A
            (1.0, -1.1),  // 1 A spoke
            (1.0, 1.1),   // 2 A spoke
            (2.0, 0.0),   // 3 core left
            (3.0, 0.0),   // 4 core center
            (4.0, 0.0),   // 5 core right
            (5.0, 1.1),   // 6 B spoke
            (5.0, -1.1),  // 7 B spoke
            (6.0, 0.0),   // 8 home B
        ],

        // Fallback: lay unknown maps out on a circle so we never panic on a new map.
        _ => (0..n_nodes)
            .map(|i| {
                let a = (i as f32 / n_nodes.max(1) as f32) * std::f32::consts::TAU;
                (a.cos(), a.sin())
            })
            .collect(),
    }
}

// ---------------------------------------------------------------------------------------------
// World <-> screen camera (fits the abstract node coordinates to the window with margins)
// ---------------------------------------------------------------------------------------------

/// Maps abstract layout coordinates to screen pixels, fitting all nodes into the drawable
/// area below the HUD with a margin. Recomputed every frame, so window resizes are free.
#[derive(Clone, Copy)]
struct Camera {
    scale: f32,
    off_x: f32,
    off_y: f32,
}

impl Camera {
    fn fit(layout: &[(f32, f32)], sw: f32, sh: f32, top: f32, bottom: f32) -> Camera {
        let (mut minx, mut miny, mut maxx, mut maxy) =
            (f32::INFINITY, f32::INFINITY, f32::NEG_INFINITY, f32::NEG_INFINITY);
        for &(x, y) in layout {
            minx = minx.min(x);
            miny = miny.min(y);
            maxx = maxx.max(x);
            maxy = maxy.max(y);
        }
        if !minx.is_finite() {
            return Camera { scale: 1.0, off_x: sw * 0.5, off_y: sh * 0.5 };
        }
        let margin = 90.0_f32;
        let avail_w = (sw - 2.0 * margin).max(1.0);
        let avail_h = (sh - top - bottom - 2.0 * margin).max(1.0);
        let span_x = (maxx - minx).max(1e-3);
        let span_y = (maxy - miny).max(1e-3);
        let scale = (avail_w / span_x).min(avail_h / span_y);
        let world_cx = (minx + maxx) * 0.5;
        let world_cy = (miny + maxy) * 0.5;
        let screen_cx = sw * 0.5;
        let screen_cy = top + (sh - top - bottom) * 0.5;
        Camera {
            scale,
            off_x: screen_cx - world_cx * scale,
            off_y: screen_cy - world_cy * scale,
        }
    }

    #[inline]
    fn to_screen(&self, p: (f32, f32)) -> (f32, f32) {
        (self.off_x + p.0 * self.scale, self.off_y + p.1 * self.scale)
    }
}

// ---------------------------------------------------------------------------------------------
// Run mode & config (mirrors layer1-game)
// ---------------------------------------------------------------------------------------------

#[derive(Clone)]
enum Mode {
    /// Human plays seat A; the hidden-mix Automaton plays seat B.
    Human,
    /// Both seats driven by Automatons; never exits.
    Auto,
    /// Both seats AI; after racing to the capture tick, write a PNG to `path` and exit.
    Shot { path: String },
}

struct Config {
    mode: Mode,
    seed: u64,
    /// Optional override for the `--shot` capture tick (env `LAYER2_SHOT_TICK`). A very
    /// large value captures the end-of-match REVEAL banner. Defaults to `SHOT_TARGET_TICK`.
    shot_tick: u64,
}

/// Parse CLI flags and env vars. Recognised: `--shot <path>` / `LAYER2_SHOT=<path>`,
/// `--auto` / `LAYER2_AUTO=1`, `--seed <hex|dec>` / `LAYER2_SEED`.
fn parse_config() -> Config {
    let mut mode = Mode::Human;
    let mut seed: u64 = 0xCE11_2042;
    let mut shot_tick: u64 = SHOT_TARGET_TICK;

    if let Ok(s) = std::env::var("LAYER2_SHOT_TICK") {
        if let Ok(v) = s.trim().parse::<u64>() {
            shot_tick = v;
        }
    }
    if let Ok(p) = std::env::var("LAYER2_SHOT") {
        if !p.is_empty() {
            mode = Mode::Shot { path: p };
        }
    }
    if std::env::var("LAYER2_AUTO").map(|v| v == "1" || v == "true").unwrap_or(false)
        && !matches!(mode, Mode::Shot { .. })
    {
        mode = Mode::Auto;
    }
    if let Ok(s) = std::env::var("LAYER2_SEED") {
        if let Some(v) = parse_seed(&s) {
            seed = v;
        }
    }

    let args: Vec<String> = std::env::args().collect();
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--shot" => {
                if i + 1 < args.len() && !args[i + 1].starts_with("--") {
                    mode = Mode::Shot { path: args[i + 1].clone() };
                    i += 1;
                } else {
                    mode = Mode::Shot { path: "target/layer2_shot.png".to_string() };
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
// Choosing the hidden enemy mix (the "glass genome" the player must read)
// ---------------------------------------------------------------------------------------------

/// The roster of hidden mixes the enemy is drawn from, cycled by seed. We span the
/// colonize/defend/attack simplex from the three pure corners through lopsided and even
/// two-strategy blends to the balanced centre — the same rim-to-centre sampling the
/// capstone ladder uses (`automaton::ladder`), so every match poses a genuinely different
/// inference problem (easy pure corners up to the hard balanced centre).
fn mix_roster() -> Vec<Mix> {
    vec![
        Mix::new(1.0, 0.0, 0.0), // pure colonize
        Mix::new(0.0, 1.0, 0.0), // pure defend
        Mix::new(0.0, 0.0, 1.0), // pure attack
        Mix::new(2.0, 1.0, 0.0), // colonize-leaning, some defend
        Mix::new(0.0, 2.0, 1.0), // defend-leaning, some attack
        Mix::new(1.0, 0.0, 2.0), // attack-leaning, some colonize
        Mix::new(1.0, 1.0, 0.0), // colonize/defend
        Mix::new(0.0, 1.0, 1.0), // defend/attack
        Mix::new(1.0, 0.0, 1.0), // colonize/attack
        Mix::new(1.0, 1.0, 1.0), // balanced centre (hardest)
    ]
}

/// Deterministically pick a (map, hidden mix) for a given seed. The map cycles through the
/// three symmetric maps and the mix cycles through the roster, on independent strides, so
/// successive restarts (seed += 1) vary both. The mix stays HIDDEN during play.
fn pick_match(seed: u64) -> (MapSpec, Mix, &'static str) {
    let maps = all_maps();
    let map_idx = (seed as usize) % maps.len();
    let roster = mix_roster();
    let mix_idx = ((seed / maps.len() as u64) as usize) % roster.len();
    let mix = roster[mix_idx];
    // A short label for the underlying ladder-style mix (only used for logging in shot mode;
    // the on-screen reveal recomputes a friendly percentage string at end of match).
    let label: &'static str = Box::leak(mix.label().into_boxed_str());
    // `MapSpec` is not Clone; rebuild the chosen one by index from a fresh `all_maps()`.
    let chosen = all_maps().into_iter().nth(map_idx).unwrap();
    let _ = maps; // (the first `all_maps()` was only for the length / index)
    (chosen, mix, label)
}

/// Build the enemy engine policy for a hidden mix: the Automaton compiler turns the mix
/// into a single legible prioritized DSL rule-set (`automaton::compile`), wrapped in the
/// engine's `DslPolicy` so it plays exactly as it does headlessly in the capstone.
fn enemy_policy(mix: Mix, label: &str) -> DslPolicy {
    DslPolicy::new(compile(mix, label))
}

// ---------------------------------------------------------------------------------------------
// Per-node render bookkeeping (for combat-resolution flashes — pure spectacle)
// ---------------------------------------------------------------------------------------------

/// A transient flash drawn over a node when a fleet arrives and combat/colonization
/// resolves there (count change or owner change). Decays over `FLASH_TTL` seconds.
#[derive(Clone, Copy)]
struct Flash {
    /// Remaining lifetime in seconds.
    ttl: f32,
    /// True if the node changed owner this resolution (a capture) — drawn bigger/brighter.
    capture: bool,
    /// Color to flash (the new owner's color).
    col: Color,
}

const FLASH_TTL: f32 = 0.6;

// ---------------------------------------------------------------------------------------------
// Game state (the renderer's own state; the *engine* state lives in `GameState`)
// ---------------------------------------------------------------------------------------------

struct Game {
    state: GameState,
    layout: Vec<(f32, f32)>,
    map_name: &'static str,
    params: Params,
    seed: u64,

    /// The HIDDEN enemy mix (revealed only on the end screen).
    hidden_mix: Mix,
    mix_label: &'static str,
    enemy_ai: DslPolicy,
    /// Present in Auto/Shot modes (both seats AI). `None` => human plays seat A.
    player_ai: Option<DslPolicy>,

    paused: bool,
    speed_idx: usize,
    tick_accum: f64,

    /// Fleet `progress` snapshot just before the most recent tick, indexed positionally.
    /// Fleets are matched best-effort by identity (owner/edge/from/to) so motion between
    /// ticks interpolates smoothly. (cell-core fleets have no stable id; we re-derive.)
    prev_progress: Vec<FleetKey>,
    /// Fractional progress toward the next tick in `[0,1)` — the interpolation alpha.
    render_alpha: f32,

    /// Per-node combat/arrival flashes (spectacle only).
    flashes: Vec<Option<Flash>>,
    /// Garrison snapshot at the previous tick boundary, to detect resolution events.
    prev_garrison: Vec<f64>,
    prev_owner: Vec<Owner>,
    /// Fleet count inbound to each node at the previous tick (to detect arrivals).
    prev_incoming: Vec<f64>,

    /// Inference telemetry on the ENEMY (seat B): cumulative early-behavior counters that
    /// make the hidden lean legible (expansion / aggression / frontier). Pure read-outs.
    enemy_captures: u32,   // neutral nodes the enemy has colonized (expansion signal)
    enemy_strikes: u32,    // fleets the enemy has sent at player-owned nodes (aggression)
    last_enemy_terr: usize,

    /// Current fraction bucket for human orders (default Half).
    fraction: FractionBucket,
    /// Currently selected source node (must be seat-A owned). `None` => nothing selected.
    selected: Option<usize>,
    /// While the left button is held after pressing on an A node: the drag source.
    drag_source: Option<usize>,

    /// Engine ticks of play elapsed (for the clock).
    play_ticks: u64,
    shot_state: ShotState,
}

/// A lightweight fleet identity used to carry `progress` across a tick for interpolation.
#[derive(Clone, Copy, PartialEq)]
struct FleetKey {
    owner: Owner,
    edge: usize,
    from: usize,
    to: usize,
    progress: f64,
    undock_remaining: f64,
}

#[derive(Clone, Copy, PartialEq)]
enum ShotState {
    Playing,
    Settling(u32),
    Done,
}

impl Game {
    fn new(seed: u64, mode: &Mode) -> Game {
        let (map, hidden_mix, mix_label) = pick_match(seed);
        let layout = layout_for(map.name, map.state.nodes.len());
        let params = operating_params();
        let enemy_ai = enemy_policy(hidden_mix, mix_label);
        // In auto/shot, the Player seat is driven by a fixed competent strategy (DSL
        // colonize) so the demo/capture plays a real game without a human.
        let player_ai = match mode {
            Mode::Human => None,
            Mode::Auto | Mode::Shot { .. } => {
                Some(DslPolicy::new(strategies::colonize()))
            }
        };
        let n = map.state.nodes.len();
        let mut g = Game {
            state: map.state,
            layout,
            map_name: map.name,
            params,
            seed,
            hidden_mix,
            mix_label,
            enemy_ai,
            player_ai,
            paused: false,
            speed_idx: DEFAULT_SPEED_IDX,
            tick_accum: 0.0,
            prev_progress: Vec::new(),
            render_alpha: 0.0,
            flashes: vec![None; n],
            prev_garrison: vec![0.0; n],
            prev_owner: vec![Owner::Neutral; n],
            prev_incoming: vec![0.0; n],
            enemy_captures: 0,
            enemy_strikes: 0,
            last_enemy_terr: 0,
            fraction: FractionBucket::Half,
            selected: None,
            drag_source: None,
            play_ticks: 0,
            shot_state: if matches!(mode, Mode::Shot { .. }) {
                ShotState::Playing
            } else {
                ShotState::Done
            },
        };
        g.snapshot_for_events();
        g.last_enemy_terr = g.state.territory(Owner::B);
        g
    }

    /// Restart with a fresh seed: new map and a NEW hidden mix (a new inference problem),
    /// while preserving the run mode and the human's chosen fraction/speed.
    fn restart(&mut self) {
        let mode = if self.player_ai.is_some() { Mode::Auto } else { Mode::Human };
        let seed = self.seed.wrapping_add(1);
        let fraction = self.fraction;
        let speed_idx = self.speed_idx;
        let shot_state = self.shot_state;
        *self = Game::new(seed, &mode);
        self.fraction = fraction;
        self.speed_idx = speed_idx;
        self.shot_state = shot_state;
    }

    fn speed(&self) -> f64 {
        SPEED_STEPS[self.speed_idx]
    }

    fn match_over(&self) -> bool {
        self.state.is_eliminated(Owner::A)
            || self.state.is_eliminated(Owner::B)
            || self.state.tick >= HORIZON
    }

    /// Capture per-node garrison/owner/incoming so the next tick can detect resolution
    /// events (arrivals, combat, captures) for flashes and enemy-behavior telemetry.
    fn snapshot_for_events(&mut self) {
        for (i, n) in self.state.nodes.iter().enumerate() {
            self.prev_garrison[i] = n.garrison;
            self.prev_owner[i] = n.owner;
        }
        for i in 0..self.state.nodes.len() {
            self.prev_incoming[i] = 0.0;
        }
        for f in &self.state.fleets {
            self.prev_incoming[f.to] += f.count;
        }
    }

    /// Snapshot fleet identities+progress so the renderer can interpolate fleet motion.
    fn snapshot_fleets(&mut self) {
        self.prev_progress = self.state.fleets.iter().map(fleet_key).collect();
    }

    /// Decide+launch both seats' commands for this tick, then `step` the engine once.
    /// Mirrors `cell_core::GameState::run_match`: collect both seats' decisions on the
    /// same pre-tick snapshot, apply A before B, then advance. Human commands (when seat
    /// A is human) are launched directly at input time into `pending_player`.
    fn step_one_tick(&mut self, pending_player: &mut Vec<Command>) {
        // Snapshot fleets BEFORE motion so interpolation has a "from" progress.
        self.snapshot_fleets();
        self.snapshot_for_events();

        // Seat A: either the human's queued commands, or the player Automaton.
        if let Some(pa) = &mut self.player_ai {
            let cmds = pa.decide(&self.state, Owner::A, &self.params);
            for c in cmds {
                self.state.launch_with(Owner::A, c, &self.params);
            }
        } else {
            for c in pending_player.drain(..) {
                self.state.launch_with(Owner::A, c, &self.params);
            }
        }

        // Seat B: the hidden-mix enemy Automaton, decided on the same pre-launch snapshot
        // is not strictly required for a live game, but we keep A-before-B to match the
        // engine's documented order.
        let cmds_b = self.enemy_ai.decide(&self.state, Owner::B, &self.params);
        // Count enemy aggression (fleets aimed at player-owned nodes) for the HUD readout.
        for c in &cmds_b {
            if self.state.nodes[c.target].owner == Owner::A {
                self.enemy_strikes += 1;
            }
        }
        for c in cmds_b {
            self.state.launch_with(Owner::B, c, &self.params);
        }

        self.state.step(&self.params);
        self.play_ticks += 1;

        self.detect_events();
    }

    /// After a `step`, compare against the pre-tick snapshot to (a) spawn combat/arrival
    /// flashes and (b) update enemy expansion telemetry. Pure spectacle/read-out.
    fn detect_events(&mut self) {
        for i in 0..self.state.nodes.len() {
            let now_owner = self.state.nodes[i].owner;
            let now_g = self.state.nodes[i].garrison;
            let was_owner = self.prev_owner[i];
            let was_incoming = self.prev_incoming[i];

            let captured = now_owner != was_owner;
            // A "resolution" worth flashing: a capture, or a meaningful garrison change at
            // a node that had hostile/owned fleets inbound last tick (arrival/combat).
            let big_change = (now_g - self.prev_garrison[i]).abs() > 0.5;
            if captured || (was_incoming > 0.0 && big_change) {
                self.flashes[i] = Some(Flash {
                    ttl: FLASH_TTL,
                    capture: captured,
                    col: owner_color(now_owner),
                });
            }
        }
        // Enemy expansion signal: rises whenever seat B's node count grows.
        let et = self.state.territory(Owner::B);
        if et > self.last_enemy_terr {
            self.enemy_captures += (et - self.last_enemy_terr) as u32;
        }
        self.last_enemy_terr = et;
    }

    /// Advance simulation for one rendered frame's worth of wall-clock `dt` (seconds).
    fn update(&mut self, dt: f64, pending_player: &mut Vec<Command>) {
        // Decay flashes regardless of pause (so they fade out, not freeze).
        for f in &mut self.flashes {
            if let Some(fl) = f {
                fl.ttl -= dt as f32;
                if fl.ttl <= 0.0 {
                    *f = None;
                }
            }
        }
        if self.paused || self.match_over() {
            self.render_alpha = 1.0;
            return;
        }
        self.tick_accum += dt * BASE_TICKS_PER_SEC * self.speed();
        let mut budget = MAX_TICKS_PER_FRAME;
        while self.tick_accum >= 1.0 && budget > 0 {
            self.step_one_tick(pending_player);
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

    /// Interpolated progress for a currently-live fleet: blend its pre-step progress (found
    /// by identity in `prev_progress`) with its current progress by `render_alpha`. Falls
    /// back to current progress for fleets with no match (spawned this tick).
    fn fleet_draw_progress(&self, idx: usize) -> f64 {
        let cur = &self.state.fleets[idx];
        let cur_key = fleet_key(cur);
        // Best-effort identity match: same owner/edge/from/to and progress within one tick.
        let prev = self
            .prev_progress
            .iter()
            .find(|p| {
                p.owner == cur_key.owner
                    && p.edge == cur_key.edge
                    && p.from == cur_key.from
                    && p.to == cur_key.to
                    && (cur_key.progress - p.progress) <= 1.5
                    && (cur_key.progress - p.progress) >= -0.01
            });
        match prev {
            Some(p) => {
                // Interpolate undock (still 0 progress) then transit.
                let from = effective_position(p.undock_remaining, p.progress);
                let to = effective_position(cur.undock_remaining, cur.progress);
                from + (to - from) * self.render_alpha as f64
            }
            None => effective_position(cur.undock_remaining, cur.progress),
        }
    }
}

/// A fleet's "effective position along the edge" in [0, length]: while undocking it sits
/// at ~0 (just leaving the source) and only advances once `undock_remaining` hits 0.
#[inline]
fn effective_position(undock_remaining: f64, progress: f64) -> f64 {
    if undock_remaining > 0.0 {
        0.0
    } else {
        progress
    }
}

#[inline]
fn fleet_key(f: &cell_core::types::Fleet) -> FleetKey {
    FleetKey {
        owner: f.owner,
        edge: f.edge,
        from: f.from,
        to: f.to,
        progress: f.progress,
        undock_remaining: f.undock_remaining,
    }
}

// ---------------------------------------------------------------------------------------------
// Input (human seat only)
// ---------------------------------------------------------------------------------------------

/// Node under a screen point (within its drawn radius). Picks the nearest centre on overlap.
fn node_at_screen(game: &Game, cam: &Camera, mx: f32, my: f32) -> Option<usize> {
    let mut best: Option<(usize, f32)> = None;
    for i in 0..game.state.nodes.len() {
        let (sx, sy) = cam.to_screen(game.layout[i]);
        let r = node_radius(game, i).max(12.0) + 4.0;
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

fn handle_input(game: &mut Game, cam: &Camera, pending_player: &mut Vec<Command>) {
    // --- Global controls (work in every mode) ---
    if is_key_pressed(KeyCode::Space) {
        game.paused = !game.paused;
    }
    if is_key_pressed(KeyCode::R) {
        game.restart();
    }
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

    if is_key_pressed(KeyCode::Escape) || is_mouse_button_pressed(MouseButton::Right) {
        game.selected = None;
        game.drag_source = None;
    }

    // Pointer orders are disabled while an Automaton drives seat A, or when the match ended.
    if game.player_ai.is_some() || game.match_over() {
        return;
    }

    let (mx, my) = mouse_position();

    // Press: select an A-owned node as source (and arm a drag), or — if a source is
    // already selected — issue a command to a CONNECTED clicked node.
    if is_mouse_button_pressed(MouseButton::Left) {
        if let Some(node) = node_at_screen(game, cam, mx, my) {
            if game.state.nodes[node].owner == Owner::A {
                game.selected = Some(node);
                game.drag_source = Some(node);
            } else if let Some(src) = game.selected {
                // Only legal along a direct edge (the atomic action travels one edge).
                if game.state.edge_between(src, node).is_some() {
                    pending_player.push(Command { source: src, target: node, fraction: game.fraction });
                }
                game.drag_source = None;
            }
        } else {
            game.selected = None;
            game.drag_source = None;
        }
    }

    // Release: a drag from an A node onto a *different, connected* node issues a command.
    if is_mouse_button_released(MouseButton::Left) {
        if let Some(src) = game.drag_source {
            if let Some(tgt) = node_at_screen(game, cam, mx, my) {
                if tgt != src && game.state.edge_between(src, tgt).is_some() {
                    pending_player.push(Command { source: src, target: tgt, fraction: game.fraction });
                }
            }
            game.drag_source = None;
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Rendering
// ---------------------------------------------------------------------------------------------

const HUD_TOP_H: f32 = 96.0;
const HUD_BOTTOM_H: f32 = 26.0;

/// Node draw radius (pixels), scaled gently by garrison so big stacks read as bigger.
fn node_radius(game: &Game, i: usize) -> f32 {
    let g = game.state.nodes[i].garrison.max(0.0);
    let base = 16.0_f32;
    let bonus = (g.sqrt() as f32) * 2.2;
    (base + bonus).min(46.0)
}

fn draw_scene(game: &Game, cam: &Camera) {
    let t = get_time() as f32;

    // --- Edges first (so nodes/fleets draw on top) ---
    let valid_neighbors = current_valid_neighbors(game);
    for e in &game.state.edges {
        let (ax, ay) = cam.to_screen(game.layout[e.a]);
        let (bx, by) = cam.to_screen(game.layout[e.b]);
        let highlight = valid_neighbors
            .as_ref()
            .map(|(src, set)| {
                (e.a == *src && set.contains(&e.b)) || (e.b == *src && set.contains(&e.a))
            })
            .unwrap_or(false);
        if highlight {
            draw_line(ax, ay, bx, by, 3.0, EDGE_HILITE);
        } else {
            draw_line(ax, ay, bx, by, 1.5, EDGE_COL);
        }
    }

    // --- Fleets (interpolated streams along their edge) ---
    for idx in 0..game.state.fleets.len() {
        let f = &game.state.fleets[idx];
        let len = game.state.edges[f.edge].length.max(1e-3);
        let pos = (game.fleet_draw_progress(idx) / len).clamp(0.0, 1.0) as f32;
        // Endpoints oriented from->to so the stream moves the right way.
        let (fx, fy) = cam.to_screen(game.layout[f.from]);
        let (tx, ty) = cam.to_screen(game.layout[f.to]);
        let cx = fx + (tx - fx) * pos;
        let cy = fy + (ty - fy) * pos;
        let dir = {
            let dx = tx - fx;
            let dy = ty - fy;
            let l = (dx * dx + dy * dy).sqrt().max(1e-3);
            (dx / l, dy / l)
        };
        let col = owner_color(f.owner);
        let undock = f.undock_remaining > 0.0;
        draw_fleet_cluster(cx, cy, dir, f.count, col, undock, t);
    }

    // --- Nodes ---
    for i in 0..game.state.nodes.len() {
        let n = &game.state.nodes[i];
        let (sx, sy) = cam.to_screen(game.layout[i]);
        let r = node_radius(game, i);
        let base = owner_color(n.owner);
        let dim = owner_dim(n.owner);

        // Combat/arrival flash halo under the node.
        if let Some(fl) = game.flashes[i] {
            let k = (fl.ttl / FLASH_TTL).clamp(0.0, 1.0);
            let extra = if fl.capture { 18.0 } else { 9.0 };
            draw_circle(sx, sy, r + extra * (1.0 - k) + 6.0, Color::new(fl.col.r, fl.col.g, fl.col.b, 0.30 * k));
            if fl.capture {
                draw_circle_lines(sx, sy, r + 6.0 + 16.0 * (1.0 - k), 2.5, Color::new(1.0, 1.0, 1.0, 0.7 * k));
            }
        }

        // Filled disk (dim) + ring (bright).
        draw_circle(sx, sy, r, Color::new(dim.r, dim.g, dim.b, 0.40));
        draw_circle_lines(sx, sy, r, 2.5, base);

        // Production tick indicator: a small pulsing pip on owned (producing) nodes.
        if n.owner.is_player() {
            let pulse = 0.5 + 0.5 * (t * 2.2 + i as f32 * 0.6).sin();
            let pr = 3.0 + 1.6 * pulse;
            draw_circle(sx + r * 0.72, sy - r * 0.72, pr, Color::new(base.r, base.g, base.b, 0.85));
        }

        // Selected source: a bright animated outline + valid-neighbor cue handled on edges.
        if game.selected == Some(i) {
            let pulse = 0.5 + 0.5 * (t * 4.0).sin();
            draw_circle_lines(sx, sy, r + 7.0, 2.5, Color::new(1.0, 1.0, 1.0, 0.5 + 0.4 * pulse));
            draw_circle_lines(sx, sy, r + 11.0, 1.5, Color::new(1.0, 1.0, 1.0, 0.25 * pulse));
        }

        // Rounded garrison count, centred.
        let label = (n.garrison.round() as i64).to_string();
        let fs = (r * 0.85).clamp(14.0, 34.0) as u16;
        let dims = measure_text(&label, None, fs, 1.0);
        draw_text(
            &label,
            sx - dims.width * 0.5,
            sy + dims.height * 0.35,
            fs as f32,
            Color::new(0.97, 0.98, 1.0, 0.96),
        );
    }
}

/// If a source is selected (human seat, match live), return it plus the set of its
/// directly-connected neighbor node ids — used to highlight valid command targets.
fn current_valid_neighbors(game: &Game) -> Option<(usize, std::collections::HashSet<usize>)> {
    if game.player_ai.is_some() || game.match_over() {
        return None;
    }
    let src = game.selected?;
    let set: std::collections::HashSet<usize> =
        game.state.neighbors(src).iter().map(|&(_, n)| n).collect();
    Some((src, set))
}

/// A small ship cluster/stream for a fleet: a few triangles pointed along the edge
/// direction, count scaling how many we draw. Undocking fleets are drawn fainter (they
/// have just left the source and deal no damage yet).
fn draw_fleet_cluster(cx: f32, cy: f32, dir: (f32, f32), count: f64, col: Color, undock: bool, t: f32) {
    let n = (count.sqrt().round() as i32).clamp(1, 6);
    let (ux, uy) = dir;
    let (px, py) = (-uy, ux); // perpendicular, for spreading the cluster
    let alpha = if undock { 0.45 } else { 0.95 };
    let c = Color::new(col.r, col.g, col.b, alpha);
    for k in 0..n {
        // Spread along and across the travel direction; a touch of shimmer over time.
        let along = (k as f32 - (n as f32 - 1.0) * 0.5) * 6.0;
        let across = ((k as f32 * 1.7 + t * 2.0).sin()) * 3.0;
        let bx = cx + ux * along + px * across;
        let by = cy + uy * along + py * across;
        draw_ship_triangle(bx, by, ux, uy, c);
    }
    // A faint count label trailing the cluster for legibility on bigger fleets.
    if count >= 4.0 {
        let label = (count.round() as i64).to_string();
        draw_text(&label, cx + 8.0, cy - 8.0, 15.0, Color::new(col.r, col.g, col.b, 0.8));
    }
}

/// A small isoceles triangle centred at (cx,cy) pointing along unit vector (ux,uy).
fn draw_ship_triangle(cx: f32, cy: f32, ux: f32, uy: f32, col: Color) {
    let nose = 6.5_f32;
    let back = 3.8_f32;
    let (px, py) = (-uy, ux);
    let tip = Vec2::new(cx + ux * nose, cy + uy * nose);
    let l = Vec2::new(cx - ux * back + px * back, cy - uy * back + py * back);
    let r = Vec2::new(cx - ux * back - px * back, cy - uy * back - py * back);
    draw_triangle(tip, l, r, col);
}

fn draw_hud(game: &Game) {
    let sw = screen_width();
    let sh = screen_height();

    // --- Top bar ---
    draw_rectangle(0.0, 0.0, sw, HUD_TOP_H, HUD_BG);

    let pf = OwnerFeatures::compute(&game.state, Owner::A, &game.params);
    let ef = OwnerFeatures::compute(&game.state, Owner::B, &game.params);

    // Player block (left).
    draw_text("PLAYER (you)", 16.0, 22.0, 22.0, PLAYER);
    draw_text(
        &format!(
            "nodes {}   ships {:>4}   prod {:.1}/t",
            pf.territory_count,
            pf.total_units.round() as i64,
            pf.production_rate
        ),
        16.0,
        46.0,
        20.0,
        HUD_TEXT,
    );

    // Enemy block (right).
    let etitle = "ENEMY (hidden mix)";
    let e1 = measure_text(etitle, None, 22, 1.0);
    draw_text(etitle, sw - e1.width - 16.0, 22.0, 22.0, ENEMY);
    let etxt = format!(
        "nodes {}   ships {:>4}   prod {:.1}/t",
        ef.territory_count,
        ef.total_units.round() as i64,
        ef.production_rate
    );
    let e2 = measure_text(&etxt, None, 20, 1.0);
    draw_text(&etxt, sw - e2.width - 16.0, 46.0, 20.0, HUD_TEXT);

    // Enemy lean read-out (the inference HUD), row 3 right: the legible tells that let a
    // human guess colonize/defend/attack. Expansion = neutral nodes the enemy has taken;
    // Aggression = fleets it has thrown at the player; Frontier-hold = its mean frontier
    // garrison (does it man its wall?). High aggression + thin frontier reads as attack; a
    // climbing node count with a thin frontier reads as colonize; a heavy frontier reads as
    // defend. All read straight off the board, like every feature in `cell_core::features`.
    let frontier_hold = enemy_frontier_hold(game);
    let lean = format!(
        "read enemy:  expansion {}   aggression {}   frontier-hold {:.1}",
        game.enemy_captures, game.enemy_strikes, frontier_hold
    );
    let ld = measure_text(&lean, None, 18, 1.0);
    draw_text(&lean, sw - ld.width - 16.0, 78.0, 18.0, HUD_MUTED);

    // Scout hint, row 3 left (the design's headline experience in one line).
    draw_text(
        "scout the enemy's lean, then counter it",
        16.0,
        78.0,
        18.0,
        HUD_MUTED,
    );

    // Centre block (rows 1-2): map, clock, fraction, speed/pause, tick.
    let secs = game.play_ticks as f64 / BASE_TICKS_PER_SEC;
    let clock = format!("{:02}:{:02}", (secs as u64) / 60, (secs as u64) % 60);
    let frac = match game.fraction {
        FractionBucket::Quarter => "25%",
        FractionBucket::Half => "50%",
        FractionBucket::ThreeQuarter => "75%",
        FractionBucket::All => "100%",
    };
    let st = if game.paused { "PAUSED".to_string() } else { format!("{}x", fmt_speed(game.speed())) };
    let center = format!(
        "{}  |  tick {}/{}  |  {}",
        game.map_name, game.state.tick, HORIZON, clock
    );
    let cd = measure_text(&center, None, 20, 1.0);
    draw_text(&center, sw * 0.5 - cd.width * 0.5, 28.0, 20.0, HUD_MUTED);
    let center2 = format!("send {}   |   {}", frac, st);
    let c2d = measure_text(&center2, None, 20, 1.0);
    draw_text(&center2, sw * 0.5 - c2d.width * 0.5, 52.0, 20.0, HUD_TEXT);

    // --- Bottom help line ---
    let help = "L-click your node then a linked node (or drag): send  |  R-click/Esc: clear  |  1-4: 25/50/75/100%  |  Space: pause  |  -/+ or [ ]: speed  |  R: new match";
    let hbd = measure_text(help, None, 16, 1.0);
    draw_rectangle(0.0, sh - HUD_BOTTOM_H, sw, HUD_BOTTOM_H, HUD_BG);
    draw_text(help, (sw - hbd.width) * 0.5, sh - 8.0, 16.0, HUD_MUTED);

    // --- End-of-match banner (with the REVEAL) ---
    if game.match_over() {
        draw_end_banner(game, sw, sh);
    }
}

/// The enemy's mean frontier garrison (a legible "does it man its wall?" number): average
/// garrison over enemy-owned nodes that touch a non-enemy node. High = defend-leaning;
/// low/thin = colonize-leaning. Read straight off the board, like the other features.
fn enemy_frontier_hold(game: &Game) -> f64 {
    let me = Owner::B;
    let mut sum = 0.0;
    let mut count = 0;
    for i in 0..game.state.nodes.len() {
        if game.state.nodes[i].owner != me {
            continue;
        }
        let frontier = game.state.neighbors(i).iter().any(|&(_, nb)| game.state.nodes[nb].owner != me);
        if frontier {
            sum += game.state.nodes[i].garrison;
            count += 1;
        }
    }
    if count == 0 {
        0.0
    } else {
        sum / count as f64
    }
}

/// Outcome + the hidden-mix REVEAL (the "glass genome" reward) + restart prompt.
fn draw_end_banner(game: &Game, sw: f32, sh: f32) {
    let a_dead = game.state.is_eliminated(Owner::A);
    let b_dead = game.state.is_eliminated(Owner::B);
    let score = game.state.score_from(Owner::A);
    let (text, col) = if b_dead && !a_dead {
        ("VICTORY", PLAYER)
    } else if a_dead && !b_dead {
        ("DEFEAT", ENEMY)
    } else if score > 0.0 {
        ("VICTORY", PLAYER)
    } else if score < 0.0 {
        ("DEFEAT", ENEMY)
    } else {
        ("DRAW", NEUTRAL)
    };

    draw_rectangle(0.0, 0.0, sw, sh, Color::new(0.0, 0.0, 0.0, 0.55));
    let fs = 92u16;
    let d = measure_text(text, None, fs, 1.0);
    draw_text(text, sw * 0.5 - d.width * 0.5, sh * 0.42, fs as f32, col);

    let how = if a_dead || b_dead {
        "by elimination".to_string()
    } else {
        format!("by lead at horizon (score {:+.2})", score)
    };
    let hd = measure_text(&how, None, 24, 1.0);
    draw_text(&how, sw * 0.5 - hd.width * 0.5, sh * 0.42 + 36.0, 24.0, HUD_TEXT);

    // THE REVEAL: the enemy's true colonize/defend/attack mix as integer percentages.
    let m = game.hidden_mix;
    let (c, dd, a) = pct3(m.c, m.d, m.a);
    let reveal = format!("Enemy was: colonize {} / defend {} / attack {}", c, dd, a);
    let rd = measure_text(&reveal, None, 30, 1.0);
    // A subtle box behind the reveal so it reads as the payoff.
    draw_rectangle(
        sw * 0.5 - rd.width * 0.5 - 18.0,
        sh * 0.42 + 58.0,
        rd.width + 36.0,
        46.0,
        Color::new(0.10, 0.12, 0.16, 0.9),
    );
    draw_text(&reveal, sw * 0.5 - rd.width * 0.5, sh * 0.42 + 88.0, 30.0, Color::new(1.0, 0.92, 0.6, 1.0));

    let nearest = format!(
        "(lean: {}, centrality {:.2})",
        m.nearest_corner().name(),
        m.centrality()
    );
    let nd = measure_text(&nearest, None, 20, 1.0);
    draw_text(&nearest, sw * 0.5 - nd.width * 0.5, sh * 0.42 + 120.0, 20.0, HUD_MUTED);

    let r = "press R for a NEW match (new hidden mix)";
    let rrd = measure_text(r, None, 26, 1.0);
    draw_text(r, sw * 0.5 - rrd.width * 0.5, sh * 0.42 + 160.0, 26.0, HUD_MUTED);
}

/// Render three weights summing to ~1 as integer percentages that sum to exactly 100
/// (largest-remainder rounding), so the reveal always reads cleanly.
fn pct3(c: f64, d: f64, a: f64) -> (i32, i32, i32) {
    let raw = [c * 100.0, d * 100.0, a * 100.0];
    let mut floor = [raw[0].floor() as i32, raw[1].floor() as i32, raw[2].floor() as i32];
    let mut rem = 100 - (floor[0] + floor[1] + floor[2]);
    // Distribute the leftover to the largest fractional parts.
    let mut fracs: [(usize, f64); 3] =
        [(0, raw[0].fract()), (1, raw[1].fract()), (2, raw[2].fract())];
    fracs.sort_by(|x, y| y.1.partial_cmp(&x.1).unwrap_or(std::cmp::Ordering::Equal));
    let mut k = 0;
    while rem > 0 {
        floor[fracs[k % 3].0] += 1;
        rem -= 1;
        k += 1;
    }
    (floor[0], floor[1], floor[2])
}

fn fmt_speed(s: f64) -> String {
    if s.fract().abs() < 1e-6 {
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
        window_title: "Layer 2 — tactical".to_owned(),
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
    // Commands the human queued this frame, launched at the next tick (seat A).
    let mut pending_player: Vec<Command> = Vec::new();

    loop {
        let sw = screen_width();
        let sh = screen_height();
        let cam = Camera::fit(&game.layout, sw, sh, HUD_TOP_H, HUD_BOTTOM_H);

        handle_input(&mut game, &cam, &mut pending_player);

        // Advance the engine. In `--shot` we race to a fixed target tick independent of
        // frame timing (deterministic, fast capture); otherwise we pace by wall-clock dt.
        if matches!(cfg.mode, Mode::Shot { .. }) && game.shot_state == ShotState::Playing {
            let mut n = SHOT_TICKS_PER_FRAME;
            while n > 0 && game.state.tick < cfg.shot_tick && !game.match_over() {
                game.step_one_tick(&mut pending_player);
                n -= 1;
            }
            game.render_alpha = 1.0;
        } else {
            let dt = get_frame_time() as f64;
            game.update(dt, &mut pending_player);
        }

        clear_background(BG);
        draw_scene(&game, &cam);
        draw_hud(&game);

        // --- Shot-mode capture state machine (mirrors layer1-game) ---
        if let Mode::Shot { path } = &cfg.mode {
            match game.shot_state {
                ShotState::Playing => {
                    if game.state.tick >= cfg.shot_tick || game.match_over() {
                        game.shot_state = ShotState::Settling(SHOT_SETTLE_FRAMES);
                    }
                }
                ShotState::Settling(0) => {
                    let img = get_screen_data();
                    if let Some(parent) = std::path::Path::new(path).parent() {
                        let _ = std::fs::create_dir_all(parent);
                    }
                    img.export_png(path);
                    println!(
                        "[layer2-game] shot written: {} ({}x{})  map={} tick={} hidden_mix={}",
                        path,
                        img.width(),
                        img.height(),
                        game.map_name,
                        game.state.tick,
                        game.mix_label,
                    );
                    game.shot_state = ShotState::Done;
                }
                ShotState::Settling(n) => {
                    game.shot_state = ShotState::Settling(n - 1);
                }
                ShotState::Done => {
                    break;
                }
            }
        }

        next_frame().await;
    }
}
