//! # world — the Layer-2 LENS over multiple Layer-1 structs
//!
//! **"ONE WORLD, Layer 2 is a lens."** This crate makes Layer 2 (the tactical, Solarmax-like
//! zoomed-out view from `03-ui-layers.md`) a *view* over several Layer-1 battlefields, not a
//! second game. There is exactly **one** simulation substrate — the spatial Layer-1 sim in the
//! [`layer1`] crate — and every **struct** *is* a real [`layer1::Interior`] (its own
//! sub-structures and discrete ships). Layer 2 adds only:
//!
//! * **lanes** — edges between structs, and
//! * **inter-struct fleets** — ships in transit from one struct to another.
//!
//! It is **not** a second combat model. [`World::step`] simply steps every struct's
//! [`layer1::Interior`] (which fights/captures normally) and moves ships between structs
//! along lanes. When a fleet arrives it **injects** its ships into the destination struct's
//! `Interior`, spawned idle, so the ordinary Layer-1 sim then resolves the landing.
//!
//! ## The model at a glance
//!
//! * [`Structure`] — `{ structure: layer1::Interior, pos: Vec2, name: String }`. The `pos` is the
//!   struct's location on the **Layer-2 map** (a different space from the *intra*-structure
//!   `layer1` coordinates inside `structure`). All structs share one [`layer1::SimParams`].
//! * [`Lane`] — `{ a: StructId, b: StructId, length: f32 }`. Undirected; `length` sets transit
//!   time. `StructId` is the index into [`World::structs`].
//! * [`InterFleet`] — `{ faction, from, to, count, undock_remaining, progress }`. A clump of
//!   `count` ships of one faction crossing the `from`→`to` lane. It mirrors Layer-1's
//!   **undock-then-transit** movement: it first burns `undock_remaining` ticks leaving the
//!   source (like ships peeling off a sub), then advances `progress` from 0→1 along the lane.
//! * [`World`] — `{ structs, lanes, fleets, tick }` plus a lane adjacency index.
//! * [`FleetOrder`] — `{ from, to, fraction }`. The inter-struct atomic action: pull a
//!   [`layer1::FractionBucket`] of a faction's **idle** ships off the source struct (drawn from
//!   the sub-structures it owns, keeping a small per-sub floor) and launch them as one
//!   [`InterFleet`] along the connecting lane. Issued via [`World::issue_fleet_order`] with the
//!   acting faction (the data type stays faction-free, exactly like Layer-1's [`MoveOrder`]).
//!
//! ## Determinism
//!
//! The world preserves Layer-1's bit-reproducibility. All randomness still lives inside each
//! struct's `Interior` behind its own seeded PRNG; the inter-struct layer draws **none**.
//! [`World::step`] has a fixed, documented iteration order, and [`World::state_hash`] folds
//! every struct's [`layer1::Interior::state_hash`], every in-transit fleet, and the tick into
//! one 64-bit fingerprint. Same construction + same orders ⇒ identical hashes across reruns.

use layer1::rng::Rng;
use layer1::{Faction, FractionBucket, SimParams, Interior, SubId, Vec2};

pub mod projection;
pub use projection::{CombatEvent, Projection, SubFate, DEFAULT_PROJECTION_HORIZON};

/// Index of a struct into [`World::structs`].
pub type StructId = usize;

/// Tunables for the **inter-struct** (Layer-2) layer only. Intra-struct behaviour is governed
/// by [`layer1::SimParams`]; these control how fleets cross lanes and how much surplus a structure
/// will export. All are documented dials; the defaults are the operating point the tests use.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WorldParams {
    /// LAYER-2 fleet engagement range (owner design, 2026-07-08 — L2 combat works like L1):
    /// two hostile in-transit fleets within this map distance of each other fight
    /// symmetrically (a lane brawl). Struct↔fleet combat uses the struct's own geometry
    /// instead (`overwatch_reach` / `l2_radius`).
    pub fleet_range: f32,
    /// Ticks a freshly launched fleet spends **undocking** before it starts crossing the lane.
    /// The Layer-2 analog of ships peeling off a sub-structure at Layer 1 — it makes a launch
    /// feel like a commitment that takes a moment to get moving rather than an instant teleport.
    pub undock_ticks: u32,

    /// Lane-length units a fleet covers per tick once it is transiting. Per-tick `progress`
    /// gain is `transit_speed / lane.length` (clamped into `[0,1]`), so a lane of `length L`
    /// takes about `L / transit_speed` ticks to cross (after undocking). Defaults to Layer-1's
    /// `ship_speed` so inter-struct travel feels consistent with intra-struct movement.
    pub transit_speed: f32,

    /// Per-sub idle **garrison floor** kept on the source struct when a fleet launches: no
    /// owned sub is drawn below this many idle ships. A struct always keeps a little home guard
    /// rather than exporting itself empty. See [`Interior::take_idle_ships_structwide`].
    pub keep_floor: usize,
}

impl Default for WorldParams {
    fn default() -> Self {
        WorldParams { fleet_range: 6.0, undock_ticks: 6, transit_speed: 1.4, keep_floor: 2 }
    }
}

/// A structure: an independent Layer-1 battlefield placed on the Layer-2 map.
///
/// The struct *is* its [`layer1::Interior`] — all its sub-structures, ships, production,
/// combat and capture happen inside `interior` under the shared [`layer1::SimParams`]. `pos`
/// is where the struct sits on the **Layer-2** map (used to draw it and to give lanes a
/// direction); it is unrelated to the intra-struct coordinates of `interior`.
#[derive(Debug, Clone)]
pub struct Structure {
    /// The real Layer-1 sim for this struct (its sub-structures + ships + RNG).
    pub interior: Interior,
    /// The struct's position on the Layer-2 map.
    pub pos: Vec2,
    /// A human-readable name (for the renderer/logs).
    pub name: String,
    /// The Layer-2 OVERWATCH multiplier (owner design, 2026-07-08): the struct's defensive
    /// zone reaches `overwatch_mult × l2_radius()` into the lanes — the fortress-like band
    /// past the node edge where its defenders fire on hostile fleets. Default 1.5;
    /// per-struct via the `.lvl` `overwatch` key.
    pub overwatch_mult: f32,
}

/// Layer-2 node radius per √(cumulative production) — a struct's SIZE on the map reads as
/// its VALUE (owner design, 2026-07-08: radius ∝ sqrt(prod), area ∝ production).
pub const L2_RADIUS_PER_SQRT_PROD: f32 = 3.0;

impl Structure {
    /// Build a struct from an existing Layer-1 [`Interior`] at Layer-2 position `pos`.
    /// The struct's Layer-2 map radius: `L2_RADIUS_PER_SQRT_PROD × sqrt(Σ sub production)`
    /// (floored for production-less structs). This IS the mechanical size — the overwatch
    /// zone and the lens rendering both derive from it.
    pub fn l2_radius(&self) -> f32 {
        let prod: u32 = self.interior.subs.iter().map(|s| s.production).sum();
        ((prod as f32).sqrt() * L2_RADIUS_PER_SQRT_PROD).max(4.0)
    }

    /// The Layer-2 overwatch reach from the struct centre (see [`Structure::overwatch_mult`]).
    pub fn overwatch_reach(&self) -> f32 {
        self.overwatch_mult * self.l2_radius()
    }

    /// The single REAL faction owning every non-storage sub of this struct (`None` when
    /// neutral ground remains or ownership is split) — the seat whose guns the overwatch
    /// zone fires; a contested struct does not fire.
    pub fn sole_owner(&self) -> Option<Faction> {
        let mut owner: Option<Faction> = None;
        for (i, sub) in self.interior.subs.iter().enumerate() {
            if self.interior.is_storage(i) {
                continue;
            }
            if !sub.owner.is_real() {
                return None;
            }
            match owner {
                None => owner = Some(sub.owner),
                Some(f) if f == sub.owner => {}
                _ => return None,
            }
        }
        owner
    }

    pub fn new(interior: Interior, pos: Vec2, name: impl Into<String>) -> Structure {
        Structure { interior, pos, name: name.into(),
            overwatch_mult: 1.5 }
    }

    /// The struct's effective Layer-1 radius: the farthest sub-structure extent from the
    /// structure's local origin. Used to place a fleet's **entry point** on the perimeter
    /// facing the incoming lane. Zero for a structure with no sub-structures.
    fn local_radius(&self) -> f32 {
        self.interior
            .subs
            .iter()
            .map(|s| s.pos.dist(Vec2::new(0.0, 0.0)) + s.radius)
            .fold(0.0f32, f32::max)
    }
}

/// A Layer-2 lane: an undirected edge between two structs, with a `length` that sets transit
/// time. `a` and `b` are [`StructId`]s (indices into [`World::structs`]).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Lane {
    pub a: StructId,
    pub b: StructId,
    /// Lane length in Layer-2 map units. Drives how long a fleet takes to cross
    /// (see [`WorldParams::transit_speed`]). A non-positive length is treated as a minimum so a
    /// fleet still arrives in finite time.
    pub length: f32,
}

impl Lane {
    /// Convenience constructor.
    pub fn new(a: StructId, b: StructId, length: f32) -> Lane {
        Lane { a, b, length }
    }

    /// True if this lane connects `x` and `y` (in either direction).
    #[inline]
    fn connects(&self, x: StructId, y: StructId) -> bool {
        (self.a == x && self.b == y) || (self.a == y && self.b == x)
    }
}

/// A fleet of ships in transit between two structs along a lane.
///
/// Mirrors Layer-1's undock-then-transit movement: it first counts `undock_remaining` down to
/// zero (leaving the source structure), then advances `progress` from `0.0` to `1.0` across the
/// lane. On reaching `1.0` it **arrives** and its `count` ships are injected into the
/// destination struct's [`layer1::Interior`] as `faction`, spawned idle.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct InterFleet {
    /// Which seat owns the fleet (always a real seat — never `Neutral`).
    pub faction: Faction,
    /// Source struct (where the ships were pulled from).
    pub from: StructId,
    /// Destination struct (where the ships will be injected on arrival).
    pub to: StructId,
    /// Number of ships carried (conserved: removed from `from`, re-spawned at `to`).
    pub count: u32,
    /// Ticks left before the fleet finishes undocking and begins crossing the lane.
    pub undock_remaining: u32,
    /// Fraction of the lane crossed so far, in `[0.0, 1.0]`. Only advances once
    /// `undock_remaining == 0`. At `>= 1.0` the fleet has arrived.
    pub progress: f32,
}

impl InterFleet {
    /// True once the fleet has finished undocking and fully crossed the lane.
    #[inline]
    pub fn arrived(&self) -> bool {
        self.undock_remaining == 0 && self.progress >= 1.0
    }
}

/// The inter-struct atomic action: launch a [`FractionBucket`] of a faction's idle ships from
/// `from` to `to` along the connecting lane.
///
/// Deliberately the same shape as Layer-1's [`layer1::MoveOrder`] (`source`/`target`/`fraction`,
/// here `from`/`to`/`fraction`) so the shared-vocabulary spine holds across layers. It carries
/// no faction — exactly like `MoveOrder` — because the *acting seat* is supplied at the call
/// site ([`World::issue_fleet_order`]). Only **connected** orders (a lane exists between `from`
/// and `to`) do anything; everything else (same structure, out-of-range ids, no lane, no idle
/// surplus) is a safe no-op.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FleetOrder {
    /// Source struct to pull idle ships from.
    pub from: StructId,
    /// Destination struct to send them to.
    pub to: StructId,
    /// How many of the source struct's idle ships to send (a fraction bucket).
    pub fraction: FractionBucket,
}

impl FleetOrder {
    /// Convenience constructor.
    pub fn new(from: StructId, to: StructId, fraction: FractionBucket) -> FleetOrder {
        FleetOrder { from, to, fraction }
    }
}

/// The Layer-2 aggregate ownership of a struct — the lens datum the renderer, the strategic AI,
/// and the greedy export rule read instead of peering at every sub-structure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StructOwner {
    /// Exactly one real faction is present and it owns **every** owned sub-structure on the
    /// struct (no enemy sub, no enemy ship). The struct flies one flag.
    Owned(Faction),
    /// Both real factions have a presence (subs and/or ships) — the struct is being fought over.
    Contested,
    /// No real owner: no faction owns any sub and no real ships are present (all-neutral, or
    /// empty). The classic up-for-grabs structure.
    Neutral,
}

/// The Layer-2 aggregate **view** of a single structure: who effectively holds it, how many ships
/// each side has there (counting both garrisoned ships and fleets currently *arriving*), and
/// whether a faction holds it cleanly enough to export surplus.
///
/// This is computed from the struct's [`layer1::Interior`] plus the in-transit fleets headed
/// to it; it adds no state. Later phases (the strategic AI and the greedy export rule) consume
/// exactly this.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StructAggregate {
    /// Aggregate ownership (see [`StructOwner`]).
    pub owner: StructOwner,
    /// Living Player ships **garrisoned on the struct** (in its `Interior`).
    pub player_ships: usize,
    /// Living Enemy ships garrisoned on the structure.
    pub enemy_ships: usize,
    /// Player ships **currently arriving** (in fleets whose `to` is this structure).
    pub player_incoming: u32,
    /// Enemy ships currently arriving.
    pub enemy_incoming: u32,
    /// Player-owned sub-structures on the structure.
    pub player_subs: usize,
    /// Enemy-owned sub-structures on the structure.
    pub enemy_subs: usize,
    /// Neutral (unowned) sub-structures on the structure.
    pub neutral_subs: usize,
}

impl StructAggregate {
    /// Total ships (garrisoned + arriving) for `faction` associated with this structure.
    #[inline]
    pub fn ships_of(&self, faction: Faction) -> u32 {
        match faction {
            Faction::Player => self.player_ships as u32 + self.player_incoming,
            // Every AI rival reads the combined "enemy" slot (the Layer-2 lens is player-vs-rivals
            // binary; per-seat Layer-2 is deferred).
            Faction::Ai(_) => self.enemy_ships as u32 + self.enemy_incoming,
            Faction::Neutral => 0,
        }
    }

    /// True iff `faction` owns **every** sub-structure on the struct AND **no enemy ship is
    /// present** (garrisoned). This is the precondition for a struct to *export surplus*: it is
    /// securely held, so shipping idle ships elsewhere will not immediately lose it. (Incoming
    /// friendly fleets do not affect this; an incoming *enemy* fleet has not landed yet, so it
    /// does not by itself make the struct non-exportable — once it lands the enemy ship is
    /// present and this flips to `false`.)
    pub fn fully_owned_uncontested(&self, faction: Faction) -> bool {
        match faction {
            Faction::Player => {
                self.player_subs > 0
                    && self.enemy_subs == 0
                    && self.neutral_subs == 0
                    && self.enemy_ships == 0
            }
            // Any AI rival: the combined non-player slot fully owns the structure, uncontested by the
            // player. On a single-AI level this is exactly that AI; per-seat Layer-2 is deferred.
            Faction::Ai(_) => {
                self.enemy_subs > 0
                    && self.player_subs == 0
                    && self.neutral_subs == 0
                    && self.player_ships == 0
            }
            Faction::Neutral => false,
        }
    }
}

/// World-level outcome — the Layer-2 mirror of [`layer1::Outcome`], aggregated over all structs
/// and all in-transit fleets.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WorldOutcome {
    /// `Some(faction)` if that faction won (the other is eliminated world-wide, or it leads on
    /// total ships + total owned subs at the horizon). `None` only for an exact tie.
    pub winner: Option<Faction>,
    /// True if the match ended by world-wide elimination rather than a horizon lead.
    pub by_elimination: bool,
    /// Tick at which the outcome was taken.
    pub tick: u64,
    /// Total ships across all structs **and fleets** `(player, enemy)`.
    pub ships: (usize, usize),
    /// Total owned sub-structures across all structs `(player, enemy)`.
    pub subs: (usize, usize),
}

/// Per-sub **in-transit influx** toward one struct's sub-structures, read directly from the current
/// in-flight state — the projection-free look-ahead the live game uses (see [`World::sub_influx_for`]).
/// Each `Vec` is indexed by [`SubId`] within the structure.
#[derive(Debug, Clone, Default)]
pub struct SubInflux {
    /// Acting seat's ships inbound to each sub (intra-structure moves + the seat's inter-struct fleets).
    pub mine: Vec<u32>,
    /// Every *other* real faction's ships inbound to each sub (the free-for-all foe in-flight force).
    pub foe: Vec<u32>,
    /// Earliest absolute tick a seat ship is projected to reach each sub (`None` if none inbound).
    pub friendly_eta: Vec<Option<u64>>,
}

/// The complete Layer-2 world: several Layer-1 structs, the lanes between them, the fleets in
/// transit, and the elapsed inter-struct tick.
///
/// One Layer-2 combat loss report: `count` ships of `faction` died at `at` (the victim
/// fleet's map position), shot from `from` — a struct's centre when overwatch fired, the
/// firing fleet's position for a lane skirmish. Enough for a GUI to draw an ATTRIBUTED
/// flash (cross at the victim, tracer from the shooter). See
/// [`World::fleet_death_events`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FleetDeathEvent {
    pub at: Vec2,
    pub from: Vec2,
    pub faction: Faction,
    pub count: u32,
}

/// Construct with [`World::new`], add structs with [`World::add_struct`] and lanes with
/// [`World::add_lane`], then drive it with [`World::step`]. Fully deterministic: see the crate
/// docs and [`World::state_hash`].
#[derive(Debug, Clone)]
pub struct World {
    /// The Layer-2 combat RNG (owner design, 2026-07-08 — L2 combat rolls dice like L1).
    /// Seeded by the level build ([`World::reseed`]); the arena / test worlds keep the
    /// fixed default. Like the interiors' RNGs it is NOT part of `state_hash` — the hash
    /// checks the visible state, and identical seeds + orders replay identically.
    pub rng: Rng,
    /// All structs, indexed by [`StructId`].
    pub structs: Vec<Structure>,
    /// All lanes between structs.
    pub lanes: Vec<Lane>,
    /// All fleets currently in transit (undocking or crossing a lane).
    pub fleets: Vec<InterFleet>,
    /// Fleet ships destroyed by Layer-2 combat THIS TICK — render/metrics support (the lens
    /// loss flash + tracer; the battle log's `{lost}`/`{killed}` counters, which otherwise
    /// only see interior deaths). One event per (victim fleet, shooter) pair. Cleared at the
    /// top of every [`World::step`], filled by the combat pass. Transient presentation
    /// state: deterministic but never hashed (the [`layer1::Interior::teleport_events`]
    /// pattern — a GUI host drains it after every tick; a headless host just ignores it).
    pub fleet_death_events: Vec<FleetDeathEvent>,
    /// Inter-struct ticks elapsed. Advances in lock-step with each struct's own `tick`
    /// (one `World::step` is one tick for every structure).
    pub tick: u64,
    /// Adjacency: for each structure, the [`StructId`]s it is laned to. Rebuilt whenever a lane is
    /// added so [`World::neighbors`] is O(1) and order is deterministic (lane insertion order).
    adjacency: Vec<Vec<StructId>>,
}

impl World {
    /// Create an empty world (no structs, lanes, or fleets).
    pub fn new() -> World {
        World { rng: Rng::new(0x1A7E_2C0B), structs: Vec::new(), lanes: Vec::new(), fleets: Vec::new(), fleet_death_events: Vec::new(), tick: 0, adjacency: Vec::new() }
    }

    /// Add a structure, returning its [`StructId`].
    pub fn add_struct(&mut self, structure: Structure) -> StructId {
        self.structs.push(structure);
        self.adjacency.push(Vec::new());
        self.structs.len() - 1
    }

    /// Add an undirected lane between `a` and `b` with the given `length`, returning its index
    /// in [`World::lanes`]. Out-of-range endpoints or a self-lane (`a == b`) are rejected
    /// (returns `None`); duplicate lanes are allowed (harmless — adjacency simply lists the
    /// neighbour twice, and order checks only ask whether *a* lane exists).
    pub fn add_lane(&mut self, a: StructId, b: StructId, length: f32) -> Option<usize> {
        if a == b || a >= self.structs.len() || b >= self.structs.len() {
            return None;
        }
        self.lanes.push(Lane::new(a, b, length));
        self.adjacency[a].push(b);
        self.adjacency[b].push(a);
        Some(self.lanes.len() - 1)
    }

    /// The structs directly laned to `p` (in lane-insertion order). Empty if `p` is out of range.
    pub fn neighbors(&self, p: StructId) -> &[StructId] {
        self.adjacency.get(p).map(|v| v.as_slice()).unwrap_or(&[])
    }

    /// True if a lane connects `from` and `to`.
    pub fn are_connected(&self, from: StructId, to: StructId) -> bool {
        from < self.structs.len()
            && to < self.structs.len()
            && self.lanes.iter().any(|l| l.connects(from, to))
    }

    /// The length of the (first) lane connecting `from` and `to`, if any. Convenience for a
    /// renderer/AI that wants the crossing distance (e.g. to estimate a fleet's arrival time).
    pub fn lane_length(&self, from: StructId, to: StructId) -> Option<f32> {
        self.lanes.iter().find(|l| l.connects(from, to)).map(|l| l.length)
    }

    // ----------------------------------------------------------------------
    // Inter-struct orders
    // ----------------------------------------------------------------------

    /// Issue a [`FleetOrder`] for `faction`: pull a fraction-bucket of `faction`'s **idle**
    /// ships off struct `from` (drawn from the sub-structures it owns, keeping
    /// [`WorldParams::keep_floor`] idle per sub) and launch them as one [`InterFleet`] toward
    /// `to` along the connecting lane. Returns the number of ships actually launched.
    ///
    /// It is robust to junk (safe no-op returning 0) when: `from == to`, either id is out of
    /// range, **no lane connects** `from` and `to`, `faction` is `Neutral`, or the source
    /// struct has no exportable idle surplus for `faction`. The pulled ships leave the source
    /// struct's `Interior` immediately (so they cannot be ordered again or fight there); they
    /// reappear, conserved, when the fleet arrives. Mirrors Layer-1's "commit, then it's
    /// flying" — once launched, a fleet is not redirected.
    pub fn issue_fleet_order(&mut self, order: FleetOrder, faction: Faction, wp: &WorldParams) -> u32 {
        let FleetOrder { from, to, fraction } = order;
        self.launch_fleet(from, to, faction, wp, |s, f, floor| {
            // A 100% order takes *everything* — no home-guard floor left behind.
            let floor = if fraction.as_f32() >= 1.0 { 0 } else { floor };
            s.take_idle_ships_structwide(f, fraction, floor)
        })
    }

    /// Like [`World::issue_fleet_order`] but with a **continuous** send-fraction `frac` in `(0,1]`
    /// — the GUI's free 1–100 % troop slider — instead of a [`layer1::FractionBucket`]. Same lane
    /// validation, keep-floor and determinism; the four snap positions match the buckets exactly.
    pub fn issue_fleet_order_fraction(
        &mut self,
        from: StructId,
        to: StructId,
        frac: f32,
        faction: Faction,
        wp: &WorldParams,
    ) -> u32 {
        self.launch_fleet(from, to, faction, wp, |s, f, floor| {
            // A 100% order takes *everything* — no home-guard floor left behind.
            let floor = if frac >= 1.0 { 0 } else { floor };
            s.take_idle_ships_structwide_fraction(f, frac, floor)
        })
    }

    /// Shared core of the fleet orders: reject junk (disconnected / out-of-range / `from == to` /
    /// `Neutral`), then pull the source struct's exportable surplus via `pull` (RNG-free; does not
    /// perturb determinism) and, if any, launch it as one [`InterFleet`]. Returns ships launched.
    fn launch_fleet(
        &mut self,
        from: StructId,
        to: StructId,
        faction: Faction,
        wp: &WorldParams,
        pull: impl Fn(&mut Interior, Faction, usize) -> usize,
    ) -> u32 {
        if from == to
            || from >= self.structs.len()
            || to >= self.structs.len()
            || !faction.is_real()
            || !self.are_connected(from, to)
        {
            return 0;
        }
        let taken = pull(&mut self.structs[from].interior, faction, wp.keep_floor);
        if taken == 0 {
            return 0;
        }
        self.fleets.push(InterFleet {
            faction,
            from,
            to,
            count: taken as u32,
            undock_remaining: wp.undock_ticks,
            progress: 0.0,
        });
        taken as u32
    }

    // ----------------------------------------------------------------------
    // The tick loop
    // ----------------------------------------------------------------------

    /// Advance the whole world by exactly one tick, in this **fixed** order (for determinism):
    ///
    /// 1. **Structures** — step every struct's [`layer1::Interior`] in ascending [`StructId`]
    ///    order (each does its own production → movement → combat → capture internally).
    /// 2. **Fleets** — advance every in-transit fleet in `fleets`-vector order: burn an
    ///    undock tick, else add this tick's lane progress.
    /// 3. **Arrivals** — any fleet that has now fully crossed its lane injects its ships into
    ///    the destination struct (see [`World::inject_fleet`]) and is removed; survivors keep
    ///    their relative order.
    /// 4. **tick** += 1.
    ///
    /// Injection happens *after* this tick's struct steps, so freshly landed ships first fight
    /// on the **next** tick (they arrive idle at the end of this one) — the same "no
    /// retroactive action this tick" discipline Layer-1 uses for production/capture.
    /// The Layer-2 map position of an in-flight fleet (post-undock, pre-arrival): the straight
    /// lerp between its endpoint structs by `progress`. `None` while undocking, arrived, or empty
    /// — the states in which a fleet is not a combatant on the map.
    fn fleet_map_pos(&self, f: &InterFleet) -> Option<Vec2> {
        (f.undock_remaining == 0 && f.progress < 1.0 && f.count > 0).then(|| {
            let (a, b) = (self.structs[f.from].pos, self.structs[f.to].pos);
            Vec2::new(a.x + (b.x - a.x) * f.progress, a.y + (b.y - a.y) * f.progress)
        })
    }

    pub fn step(&mut self, params: &SimParams, wp: &WorldParams) {
        self.fleet_death_events.clear();

        // (0) CONTESTED-STORAGE FIRE SPLIT (owner design, 2026-07-09): a defender ship must
        // not fight at full rate in BOTH arenas at once. When a sole-owned struct has foes
        // inside its interior (staged raiders, landed attackers) AND hostile fleets inside
        // its overwatch zone, the owner's fire budget is spread across the two pools by
        // head-count: the interior sim gets `foes / (foes + fleet ships)` of each ship's
        // fire probability (handed down via [`layer1::Interior::fire_scale`]) and the
        // Layer-2 volley in (2b) gets the complement. Either pool empty ⇒ the other side
        // fires at full rate (scale cleared / no discount).
        for pi in 0..self.structs.len() {
            let scale = self.structs[pi].sole_owner().and_then(|owner| {
                let st = &self.structs[pi];
                let foes = st
                    .interior
                    .ships
                    .iter()
                    .filter(|s| s.alive && s.faction.is_foe_of(owner))
                    .count();
                if foes == 0 {
                    return None;
                }
                let reach = st.overwatch_reach();
                let inbound: u32 = self
                    .fleets
                    .iter()
                    .filter(|f| f.faction.is_foe_of(owner))
                    .filter(|f| self.fleet_map_pos(f).map_or(false, |p| p.dist(st.pos) <= reach))
                    .map(|f| f.count)
                    .sum();
                (inbound > 0).then(|| (owner, foes as f64 / (foes as f64 + inbound as f64)))
            });
            self.structs[pi].interior.fire_scale = scale;
        }

        // (1) Step every struct's spatial sim.
        for structure in self.structs.iter_mut() {
            structure.interior.step(params);
        }

        // (2) Advance fleets (undock, then transit). Collect the indices that arrive.
        for f in self.fleets.iter_mut() {
            if f.undock_remaining > 0 {
                f.undock_remaining -= 1;
                continue;
            }
            let len = f_lane_len(&self.lanes, f.from, f.to);
            // Per-tick progress; a degenerate (non-positive) length arrives immediately.
            let dprog = if len > 0.0 { wp.transit_speed / len } else { 1.0 };
            f.progress = (f.progress + dprog).min(1.0);
        }

        // (2b) LAYER-2 COMBAT (owner design, 2026-07-08 — "similar to layer 1"): the
        // pseudo-random square law over ship COUNTS. Ships travel in fleets up here, so
        // individual positions don't matter: count who is in range, roll the dice — every
        // combatant ship fires with `SimParams::fire_prob` at the hostile pool it can reach.
        // * A fully-owned struct's defenders fire on hostile fleets inside its OVERWATCH
        //   zone (`overwatch_reach` = 1.5 × node radius by default — the fortress band).
        // * A fleet fires BACK only inside the node radius itself: the band between the
        //   node and the reach is one-sided overwatch, the fortress rule at Layer 2.
        // * Two hostile in-transit fleets within `WorldParams::fleet_range` brawl
        //   symmetrically (lane skirmishes).
        // Kills are computed against a start-of-tick snapshot (simultaneous, like the L1
        // sub-step: overkill on an already-dead target is wasted) and split across a
        // shooter's reachable groups by their share of the pool. A garrison also fighting
        // foes INSIDE its interior fires up here at only its fleet share of the budget —
        // see the fire split in step phase (0). RNG: the world's own seeded stream —
        // replays reproduce.
        {
            #[derive(Clone, Copy)]
            enum Tgt {
                Fleet(usize),
                Garrison(usize, Faction),
            }
            // Snapshot the combatant groups.
            let fleet_pos: Vec<Option<Vec2>> =
                self.fleets.iter().map(|f| self.fleet_map_pos(f)).collect();
            let garrisons: Vec<Option<(Faction, usize)>> = self
                .structs
                .iter()
                .map(|s| {
                    s.sole_owner().and_then(|o| {
                        let n = s.interior.ship_count(o);
                        (n > 0).then_some((o, n))
                    })
                })
                .collect();
            // (shooter count, per-shooter fire probability, the shooter's map position —
            // the tracer origin for loss events — and the reachable hostile groups with
            // their snapshot sizes)
            let mut volleys: Vec<(usize, f64, Vec2, Vec<(Tgt, usize)>)> = Vec::new();
            for (fi, f) in self.fleets.iter().enumerate() {
                let Some(pos) = fleet_pos[fi] else { continue };
                let mut pool: Vec<(Tgt, usize)> = Vec::new();
                for (gj, g) in self.fleets.iter().enumerate() {
                    let Some(gpos) = fleet_pos[gj] else { continue };
                    if gj != fi && g.faction.is_foe_of(f.faction) && pos.dist(gpos) <= wp.fleet_range
                    {
                        pool.push((Tgt::Fleet(gj), g.count as usize));
                    }
                }
                for (pi, st) in self.structs.iter().enumerate() {
                    if let Some((owner, n)) = garrisons[pi] {
                        if owner.is_foe_of(f.faction) && pos.dist(st.pos) <= st.l2_radius() {
                            pool.push((Tgt::Garrison(pi, owner), n));
                        }
                    }
                }
                if !pool.is_empty() {
                    volleys.push((f.count as usize, params.fire_prob, pos, pool));
                }
            }
            for (pi, st) in self.structs.iter().enumerate() {
                let Some((owner, defenders)) = garrisons[pi] else { continue };
                let mut pool: Vec<(Tgt, usize)> = Vec::new();
                for (fi, f) in self.fleets.iter().enumerate() {
                    let Some(pos) = fleet_pos[fi] else { continue };
                    if owner.is_foe_of(f.faction) && pos.dist(st.pos) <= st.overwatch_reach() {
                        pool.push((Tgt::Fleet(fi), f.count as usize));
                    }
                }
                if pool.is_empty() {
                    continue;
                }
                // The fire split's Layer-2 side (see step phase (0)): foes inside the
                // interior claim their head-count share of the defenders' budget — the
                // interior sim delivers that share — so only the fleet share is fired here.
                let foes_inside = st
                    .interior
                    .ships
                    .iter()
                    .filter(|s| s.alive && s.faction.is_foe_of(owner))
                    .count();
                let fleet_pool: usize = pool.iter().map(|(_, n)| *n).sum();
                let share = fleet_pool as f64 / (fleet_pool + foes_inside) as f64;
                volleys.push((defenders, params.fire_prob * share, st.pos, pool));
            }
            // Roll the dice and split each volley across its pool by share (largest first).
            let mut fleet_deaths = vec![0usize; self.fleets.len()];
            // Per (victim fleet, shooter position) hit — the attribution the loss events carry.
            let mut fleet_hits: Vec<(usize, Vec2, usize)> = Vec::new();
            let mut garrison_deaths: Vec<(usize, Faction, usize)> = Vec::new();
            for (shooters, prob, src, pool) in volleys {
                let mut kills = 0usize;
                for _ in 0..shooters {
                    if self.rng.chance(prob) {
                        kills += 1;
                    }
                }
                if kills == 0 {
                    continue;
                }
                let total: usize = pool.iter().map(|(_, n)| *n).sum::<usize>().max(1);
                let mut assigned = 0usize;
                for (k, (tgt, n)) in pool.iter().enumerate() {
                    let share = if k + 1 == pool.len() {
                        kills - assigned // remainder to the last group
                    } else {
                        kills * n / total
                    };
                    assigned += share;
                    match tgt {
                        Tgt::Fleet(j) => {
                            fleet_deaths[*j] += share;
                            if share > 0 {
                                fleet_hits.push((*j, src, share));
                            }
                        }
                        Tgt::Garrison(pj, fac) => garrison_deaths.push((*pj, *fac, share)),
                    }
                }
            }
            // Apply simultaneously (overkill wasted, like the L1 sub-step). Fleet deaths are
            // recorded as per-tick, per-shooter events so the GUI can count them into the
            // battle-log metrics and flash an ATTRIBUTED loss (they never touch an interior,
            // so the interior liveness diff cannot see them). Overkill is capped per fleet;
            // the cap is walked through that fleet's hits in volley order, so the events'
            // counts always sum to the ships actually lost.
            let mut remaining = vec![0u32; self.fleets.len()];
            for (fi, d) in fleet_deaths.iter().enumerate() {
                if *d == 0 {
                    continue;
                }
                let dead = (*d as u32).min(self.fleets[fi].count);
                self.fleets[fi].count -= dead;
                remaining[fi] = dead;
            }
            for (j, src, share) in fleet_hits {
                let take = (share as u32).min(remaining[j]);
                if take == 0 {
                    continue;
                }
                remaining[j] -= take;
                if let Some(at) = fleet_pos[j] {
                    self.fleet_death_events.push(FleetDeathEvent {
                        at,
                        from: src,
                        faction: self.fleets[j].faction,
                        count: take,
                    });
                }
            }
            for (pj, fac, d) in garrison_deaths {
                if d > 0 {
                    self.structs[pj].interior.kill_ships(fac, d);
                }
            }
            // A fleet ground to nothing in space simply ceases to exist.
            self.fleets.retain(|f| f.count > 0);
        }

        // (3) Resolve arrivals in fleet order; keep non-arrived fleets in their relative order.
        if self.fleets.iter().any(|f| f.arrived()) {
            // Take ownership of the current fleet list, partition into arrived / still-flying.
            let current = std::mem::take(&mut self.fleets);
            let mut remaining: Vec<InterFleet> = Vec::with_capacity(current.len());
            for f in current {
                if f.arrived() {
                    self.inject_fleet(&f);
                } else {
                    remaining.push(f);
                }
            }
            self.fleets = remaining;
        }

        // (4) Advance the world clock.
        self.tick += 1;
    }

    /// Inject an arrived fleet's `count` ships into the destination struct's [`layer1::Interior`]
    /// as `faction`, spawned **idle**, so the ordinary Layer-1 sim then resolves the landing
    /// (fight / capture) on subsequent ticks.
    ///
    /// **Entry point.** Fleets land in the destination's **reserve / patrol-zone node**
    /// (`storage_sub`) — the universal inter-struct entry point. Every campaign struct has one,
    /// so this is the normal path. Only a bare structure with no reserve falls back to the
    /// lane-facing [`World::entry_sub`] rule (reinforce the owned sub nearest where the lane
    /// enters, else beachhead at the nearest sub facing the source). If the destination has no
    /// sub-structures at all, nothing is injected (the ships are dropped — a degenerate map the
    /// constructors never build).
    fn inject_fleet(&mut self, f: &InterFleet) {
        // Fleets arrive into the destination's **reserve / patrol-zone** node (the universal entry
        // point) if it has one; otherwise the lane-facing entry sub (bare structures with no reserve).
        let entry = match self.structs[f.to]
            .interior
            .storage_sub
            .or_else(|| self.entry_sub(f.to, f.from, f.faction))
        {
            Some(s) => s,
            None => return, // destination has no sub-structures; nothing to garrison at
        };
        let structure = &mut self.structs[f.to];
        for _ in 0..f.count {
            structure.interior.spawn_ship(f.faction, entry);
        }
    }

    /// Choose the destination sub-structure an arriving `faction` fleet from `from` garrisons
    /// at (see [`World::inject_fleet`] for the rule). `None` if the destination has no subs.
    ///
    /// **Public so the forward-[`projection`] schedules a fleet's arrival into the *identical*
    /// landing sub the sim would inject into** (R3 / §5) — re-deriving the rule in the AI would
    /// risk drift. Pure read; draws no randomness.
    pub fn entry_sub(&self, dest: StructId, from: StructId, faction: Faction) -> Option<SubId> {
        let d = self.structs.get(dest)?;
        if d.interior.subs.is_empty() {
            return None;
        }
        // Direction on the Layer-2 map from the destination toward the source.
        let to_src = Vec2::new(d.pos.x, d.pos.y);
        let src = &self.structs.get(from)?.pos;
        let mut dx = src.x - to_src.x;
        let mut dy = src.y - to_src.y;
        let mag = (dx * dx + dy * dy).sqrt();
        if mag > 1e-6 {
            dx /= mag;
            dy /= mag;
        } else {
            // Coincident structs: fall back to +x so the choice is still deterministic.
            dx = 1.0;
            dy = 0.0;
        }
        // Perimeter point in the destination's LOCAL space (its structure is centred on the
        // local origin) facing the source on the map.
        let r = d.local_radius();
        let perim = Vec2::new(dx * r, dy * r);

        // Prefer the faction's own subs (reinforce); else any sub (beachhead). Nearest to the
        // perimeter point wins; ties break to the lowest SubId (the `<` keeps the first seen).
        let pick = |owned_only: bool| -> Option<SubId> {
            let mut best: Option<(SubId, f32)> = None;
            for (i, s) in d.interior.subs.iter().enumerate() {
                if owned_only && s.owner != faction {
                    continue;
                }
                let dist = s.pos.dist(perim);
                match best {
                    Some((_, bd)) if bd <= dist => {}
                    _ => best = Some((i, dist)),
                }
            }
            best.map(|(i, _)| i)
        };
        pick(true).or_else(|| pick(false))
    }

    /// Per-sub [`SubInflux`] toward `struct` for `seat`, read **directly** from the current in-flight
    /// state (no forward [`projection`]). This is the live game's projection-free look-ahead: it mirrors
    /// exactly what the projection scheduled as "arrivals", but off the *present* state instead of a
    /// forward simulation —
    /// * **(a) intra-structure moving ships** are attributed to their `target` sub, with the same
    ///   undock-then-straight-line ETA the sim uses
    ///   (`undock_remaining + ceil((dist-tolerance)/ship_speed)`);
    /// * **(b) inter-struct fleets** inbound to this struct are attributed to the sub they will
    ///   actually land at — the reserve / patrol node (`storage_sub`) if present, else the lane-facing
    ///   [`World::entry_sub`] — *identical to [`World::inject_fleet`]* (this fixes the old projection's
    ///   entry-sub divergence, which always routed arrivals to the entry sub).
    ///
    /// Free-for-all: every real faction other than `seat` is counted as a foe. A foe mover already
    /// **within its target's engaging reach** (`radius + engagement_radius`) is *not* counted in
    /// `foe` — it is already present in any engaging-ships read of that sub, and counting it in
    /// both would double the threat. Deterministic: a pure function of the world state (positions,
    /// fleets) in f32 with a single `ceil`, so identical inputs give an identical influx and
    /// `state_hash` replay stays bit-identical. Out-of-range `struct` yields an empty influx.
    pub fn sub_influx_for(
        &self,
        sid: StructId,
        seat: Faction,
        sp: &SimParams,
        wp: &WorldParams,
    ) -> SubInflux {
        const EPS: f32 = 1e-6;
        let Some(struct_ref) = self.structs.get(sid) else {
            return SubInflux { mine: vec![], foe: vec![], friendly_eta: vec![] };
        };
        let st = &struct_ref.interior;
        let n = st.subs.len();
        let mut influx = SubInflux {
            mine: vec![0; n],
            foe: vec![0; n],
            friendly_eta: vec![None; n],
        };
        let now = self.tick;
        let note_eta = |slot: &mut Option<u64>, eta: u64| {
            *slot = Some(slot.map_or(eta, |e| e.min(eta)));
        };

        // (a) Intra-structure moving ships -> their target sub.
        for sh in &st.ships {
            if !sh.alive {
                continue;
            }
            let Some(tgt) = sh.target else { continue };
            if tgt >= n {
                continue;
            }
            if sh.faction == seat {
                influx.mine[tgt] += 1;
                // The sim does not move a ship until its undock delay burns, so the ETA must
                // include it (mirrors the fleet branch, which charges the fleet's undock). A
                // departure from a TELEPORTER the mover's side owns arrives the instant the
                // undock burns out — no transit leg.
                let teleporting = st
                    .subs
                    .get(sh.home)
                    .map_or(false, |s| s.kind == layer1::SubKind::Teleporter && s.owner == sh.faction);
                let eta = if teleporting {
                    now + sh.undock_remaining as u64
                } else {
                    let eff = (sh.pos.dist(sh.aim) - sp.arrival_tolerance).max(0.0);
                    now + sh.undock_remaining as u64
                        + (eff / sp.ship_speed.max(EPS)).ceil() as u64
                };
                note_eta(&mut influx.friendly_eta[tgt], eta);
            } else if sh.faction.is_real() {
                // Skip a foe mover already inside the target's engaging reach: it is already
                // counted by any "engaging ships at this sub" read, and influx must not count
                // the same ship a second time.
                let s = &st.subs[tgt];
                let reach = s.radius + sp.engagement_radius;
                if sh.pos.dist_sq(s.pos) <= reach * reach {
                    continue;
                }
                influx.foe[tgt] += 1;
            }
        }

        // (b) Inter-struct fleets inbound to this struct -> their real landing sub (reserve else entry).
        for f in &self.fleets {
            if f.to != sid || !f.faction.is_real() {
                continue;
            }
            let Some(land) = st.storage_sub.or_else(|| self.entry_sub(f.to, f.from, f.faction)) else {
                continue;
            };
            if land >= n {
                continue;
            }
            if f.faction == seat {
                influx.mine[land] += f.count;
                let ticks = fleet_arrival_ticks(self, wp, f);
                if ticks != u64::MAX {
                    note_eta(&mut influx.friendly_eta[land], now.saturating_add(ticks).saturating_add(1));
                }
            } else {
                influx.foe[land] += f.count;
            }
        }

        influx
    }

    // ----------------------------------------------------------------------
    // Layer-2 aggregate (the lens datum)
    // ----------------------------------------------------------------------

    /// Compute the [`StructAggregate`] for struct `p`: aggregate ownership, per-faction ship
    /// counts (garrisoned **plus** currently arriving), sub-structure tallies, and the
    /// exportable flag. Reads the struct's `Interior` and the in-transit `fleets`; adds no
    /// state. Out-of-range `p` yields an all-empty `Neutral` aggregate.
    ///
    /// **Owner rule:**
    /// * `Owned(faction)` — `faction` owns at least one sub, owns **all** owned subs (the enemy
    ///   owns none), and **no enemy ship is present** (garrisoned). Neutral subs may remain.
    /// * `Contested` — both real factions have a presence (a sub or a garrisoned ship each).
    /// * `Neutral` — neither real faction owns a sub and neither has a garrisoned ship.
    ///
    /// (Arriving fleets are counted in the ship tallies but do **not** by themselves set
    /// `Contested`: a fleet that has not landed is not yet "present" for ownership. It flips the
    /// aggregate the tick after it lands and its ships are real garrisoned ships.)
    pub fn struct_aggregate(&self, p: StructId) -> StructAggregate {
        if p >= self.structs.len() {
            return StructAggregate {
                owner: StructOwner::Neutral,
                player_ships: 0,
                enemy_ships: 0,
                player_incoming: 0,
                enemy_incoming: 0,
                player_subs: 0,
                enemy_subs: 0,
                neutral_subs: 0,
            };
        }
        let st = &self.structs[p].interior;
        let player_ships = st.ship_count(Faction::Player);
        // Layer-2 lens: every non-player rival is aggregated into the binary "enemy" slot (summed over
        // any number of AI seats — no hardcoded count). Per-seat Layer-2 (telling rivals apart in the
        // lens / pie chart) is deferred; no current level fields a multi-struct free-for-all.
        let enemy_ships = st.foreign_ship_count(Faction::Player);
        let player_subs = st.sub_count(Faction::Player);
        let enemy_subs = st.foreign_sub_count(Faction::Player);
        let neutral_subs = st.sub_count(Faction::Neutral);

        let mut player_incoming = 0u32;
        let mut enemy_incoming = 0u32;
        for f in &self.fleets {
            if f.to != p {
                continue;
            }
            match f.faction {
                Faction::Player => player_incoming += f.count,
                // Any AI rival's inbound fleet feeds the combined enemy slot.
                Faction::Ai(_) => enemy_incoming += f.count,
                Faction::Neutral => {}
            }
        }

        // Aggregate ownership from garrisoned presence (subs + garrisoned ships).
        let player_present = player_subs > 0 || player_ships > 0;
        let enemy_present = enemy_subs > 0 || enemy_ships > 0;
        let owner = match (player_present, enemy_present) {
            (true, true) => StructOwner::Contested,
            (true, false) => StructOwner::Owned(Faction::Player),
            (false, true) => StructOwner::Owned(Faction::Ai(0)),
            (false, false) => StructOwner::Neutral,
        };

        StructAggregate {
            owner,
            player_ships,
            enemy_ships,
            player_incoming,
            enemy_incoming,
            player_subs,
            enemy_subs,
            neutral_subs,
        }
    }

    // ----------------------------------------------------------------------
    // Layer-2 wrappers of the per-structure capture / soft-cap read signals
    // ----------------------------------------------------------------------
    //
    // These lift the new `layer1::Interior` reads (resistance, parked count, soft cap) to
    // struct scope so the strategic AI does not reach into a struct's `Interior`. All are
    // pure, deterministic reads that add no state and draw no randomness.

    /// Total foreign capture resistance on struct `p` from `seat`'s point of view: the sum of
    /// `resistance` over every sub on `p` **not** owned by `seat` (neutral and enemy subs). This
    /// is the quantity a resistance-proportional colonizer sizes a capture wave on. Out-of-range
    /// `p` yields `0.0`.
    pub fn struct_total_resistance_vs(&self, p: StructId, seat: Faction) -> f32 {
        match self.structs.get(p) {
            Some(structure) => structure.interior.total_foreign_resistance(seat),
            None => 0.0,
        }
    }

    /// Parked ships of `seat` on struct `p` (living ships in the struct's `Interior` — idle or
    /// intra-structure transit). Inter-struct fleets to/from `p` are **not** counted (they live
    /// in [`World::fleets`], exempt from the soft cap). Out-of-range `p` yields `0`. Mirrors
    /// [`layer1::Interior::parked_count`].
    pub fn parked_count(&self, p: StructId, seat: Faction) -> u32 {
        match self.structs.get(p) {
            Some(structure) => structure.interior.parked_count(seat),
            None => 0,
        }
    }

    /// The soft cap for `seat` on struct `p` — `softcap_free` plus the **sum of per-sub
    /// capacities** of the subs `seat` owns there (see [`layer1::SubStructure::soft_cap_capacity`];
    /// numerically `softcap_free + softcap_per_sub * owned_subs` today). When [`World::parked_count`]
    /// exceeds this, the struct's `Interior` bleeds the overflow with `sqrt` attrition.
    /// Out-of-range `p` yields `0`. Mirrors [`layer1::Interior::soft_cap`].
    pub fn soft_cap(&self, p: StructId, seat: Faction, sp: &SimParams) -> u32 {
        match self.structs.get(p) {
            Some(structure) => structure.interior.soft_cap(seat, sp),
            None => 0,
        }
    }

    // ----------------------------------------------------------------------
    // World-level outcome
    // ----------------------------------------------------------------------

    /// Total living ships of `faction` across **all** structs and **all** in-transit fleets.
    pub fn total_ships(&self, faction: Faction) -> usize {
        let garrisoned: usize = self.structs.iter().map(|p| p.interior.ship_count(faction)).sum();
        let flying: usize = self
            .fleets
            .iter()
            .filter(|f| f.faction == faction)
            .map(|f| f.count as usize)
            .sum();
        garrisoned + flying
    }

    /// Total sub-structures owned by `faction` across all structs.
    pub fn total_subs(&self, faction: Faction) -> usize {
        self.structs.iter().map(|p| p.interior.sub_count(faction)).sum()
    }

    /// Total living ships of every real seat **other than** `seat`, across all structs and fleets —
    /// the free-for-all "all my rivals" ship total, summed over any number of AI opponents.
    pub fn total_foreign_ships(&self, seat: Faction) -> usize {
        let garrisoned: usize = self.structs.iter().map(|p| p.interior.foreign_ship_count(seat)).sum();
        let flying: usize = self
            .fleets
            .iter()
            .filter(|f| f.faction.is_real() && f.faction != seat)
            .map(|f| f.count as usize)
            .sum();
        garrisoned + flying
    }

    /// Total sub-structures owned by every real seat **other than** `seat`, across all structs.
    pub fn total_foreign_subs(&self, seat: Faction) -> usize {
        self.structs.iter().map(|p| p.interior.foreign_sub_count(seat)).sum()
    }

    /// Total owned **producing** subs (`production > 0`) — the elimination-relevant territory
    /// count (owner QoL: fortresses/teleporters alone cannot rebuild a dead seat).
    pub fn total_productive_subs(&self, faction: Faction) -> usize {
        self.structs.iter().map(|p| p.interior.productive_sub_count(faction)).sum()
    }

    /// [`World::total_productive_subs`] summed over every real seat other than `seat`.
    pub fn total_foreign_productive_subs(&self, seat: Faction) -> usize {
        self.structs.iter().map(|p| p.interior.productive_foreign_sub_count(seat)).sum()
    }

    /// True if `faction` is **world-wide eliminated**: it has no ships anywhere (garrisoned or
    /// in transit) **and** owns no PRODUCING sub on any struct (a seat holding only
    /// zero-production specials — fortresses, teleporters — can never rebuild, so they do not
    /// keep it alive; owner QoL). Mirrors Layer-1's elimination, lifted to the whole world.
    pub fn is_eliminated(&self, faction: Faction) -> bool {
        self.total_ships(faction) == 0 && self.total_productive_subs(faction) == 0
    }

    /// The world outcome **as of now** — the Layer-2 mirror of [`layer1::Interior::outcome`].
    /// If exactly one real faction is world-wide eliminated, the other wins by elimination;
    /// otherwise the winner leads on `total ships + total owned subs` at the horizon (an exact
    /// tie ⇒ `None`).
    pub fn outcome(&self) -> WorldOutcome {
        // Player-perspective outcome in a free-for-all: VICTORY iff **all** enemy seats are
        // eliminated, DEFEAT iff the player is, else a horizon lead on player-vs-combined-enemies.
        // Every `Ai(i)` seat is aggregated into the binary "enemy" slot (rivals may also have
        // whittled each other down — which simply helps the player).
        let p_ships = self.total_ships(Faction::Player);
        let p_subs = self.total_subs(Faction::Player);
        // All rivals combined, summed over every non-player real seat (any number of AI opponents).
        let e_ships = self.total_foreign_ships(Faction::Player);
        let e_subs = self.total_foreign_subs(Faction::Player);
        let p_dead = self.is_eliminated(Faction::Player);
        // Every rival eliminated ⇔ no non-player real ship or owned PRODUCING sub remains
        // anywhere (leftover fortresses/teleporters cannot rebuild a wiped-out seat — owner
        // QoL). The horizon-lead tie-break below still counts ALL owned subs as territory.
        let enemies_dead = e_ships == 0 && self.total_foreign_productive_subs(Faction::Player) == 0;

        let (winner, by_elim) = if enemies_dead && !p_dead {
            (Some(Faction::Player), true)
        } else if p_dead && !enemies_dead {
            (Some(Faction::Ai(0)), true) // an enemy stands ⇒ player defeated
        } else if p_dead && enemies_dead {
            (None, true) // everyone wiped out (degenerate) ⇒ draw
        } else {
            let p_score = p_ships + p_subs;
            let e_score = e_ships + e_subs;
            let w = if p_score > e_score {
                Some(Faction::Player)
            } else if e_score > p_score {
                Some(Faction::Ai(0))
            } else {
                None
            };
            (w, false)
        };
        WorldOutcome {
            winner,
            by_elimination: by_elim,
            tick: self.tick,
            ships: (p_ships, e_ships),
            subs: (p_subs, e_subs),
        }
    }

    // ----------------------------------------------------------------------
    // Determinism fingerprint
    // ----------------------------------------------------------------------

    /// A 64-bit fingerprint of the world's state: every struct's
    /// [`layer1::Interior::state_hash`] (which already folds that struct's full sim state and
    /// RNG position), every in-transit fleet, every lane, and the world tick. Two worlds built
    /// identically and driven with the same orders produce identical hashes at every tick — the
    /// determinism tests assert on this. The params (`SimParams` / [`WorldParams`]) are **not**
    /// folded: the hash compares identically-parameterised runs, not configurations.
    ///
    /// Implemented as an inline FNV-1a, the same construction `layer1` uses; floats are folded
    /// by bit pattern so the comparison is exact.
    /// Reseed the Layer-2 combat RNG (called by the level build with the match seed).
    pub fn reseed(&mut self, seed: u64) {
        self.rng = Rng::new(seed ^ 0x1A7E_2C0B_5EED_D1CE);
    }

    pub fn state_hash(&self) -> u64 {
        let mut h: u64 = 0xcbf2_9ce4_8422_2325;
        #[inline]
        fn mix(h: &mut u64, b: u8) {
            *h ^= b as u64;
            *h = h.wrapping_mul(0x0000_0100_0000_01b3);
        }
        #[inline]
        fn mix_u64(h: &mut u64, v: u64) {
            for b in v.to_le_bytes() {
                mix(h, b);
            }
        }
        #[inline]
        fn mix_f32(h: &mut u64, v: f32) {
            for b in v.to_bits().to_le_bytes() {
                mix(h, b);
            }
        }
        mix_u64(&mut h, self.tick);
        // Structures, in StructId order: fold each struct's own state hash and its map position.
        mix_u64(&mut h, self.structs.len() as u64);
        for p in &self.structs {
            mix_u64(&mut h, p.interior.state_hash());
            mix_f32(&mut h, p.pos.x);
            mix_f32(&mut h, p.pos.y);
        }
        // Lanes, in insertion order (construction-static, but they drive fleet dynamics —
        // `dprog = transit_speed / length` — so two differently-laned worlds hash differently).
        mix_u64(&mut h, self.lanes.len() as u64);
        for l in &self.lanes {
            mix_u64(&mut h, l.a as u64);
            mix_u64(&mut h, l.b as u64);
            mix_f32(&mut h, l.length);
        }
        // Fleets, in fleet-vector order.
        mix_u64(&mut h, self.fleets.len() as u64);
        for f in &self.fleets {
            mix(&mut h, faction_byte(f.faction));
            mix_u64(&mut h, f.from as u64);
            mix_u64(&mut h, f.to as u64);
            mix_u64(&mut h, f.count as u64);
            mix_u64(&mut h, f.undock_remaining as u64);
            mix_f32(&mut h, f.progress);
        }
        h
    }
}

impl Default for World {
    fn default() -> Self {
        World::new()
    }
}

/// Lane length lookup used inside `step` (free function so the borrow of `self.lanes` does not
/// collide with the mutable borrow of `self.fleets`).
#[inline]
fn f_lane_len(lanes: &[Lane], from: StructId, to: StructId) -> f32 {
    lanes
        .iter()
        .find(|l| l.connects(from, to))
        .map(|l| if l.length > 0.0 { l.length } else { 1.0 })
        .unwrap_or(1.0)
}

/// Fleet arrival timing: ticks until an in-transit `fleet`'s ships are **injected** into its
/// destination. This reproduces [`World::step`] exactly: it burns the remaining undock delay,
/// then crosses the lane at `transit_speed / lane_len` progress per tick, and `World::step`
/// injects the ships at the **end** of the arriving tick (so they are first present the *next*
/// tick — the `+1` a scheduler adds). A degenerate (non-positive) lane length arrives the first
/// transiting tick. Pure, deterministic, RNG-free. Lives here — next to the `step` loop whose
/// arithmetic it mirrors — because the **live** [`World::sub_influx_for`] reads it every Simple
/// decision (the parked [`projection`] shares it).
pub fn fleet_arrival_ticks(world: &World, wp: &WorldParams, fleet: &InterFleet) -> u64 {
    let undock = fleet.undock_remaining as u64;
    // The same lane-length clamp `World::step` uses: missing/degenerate => 1.
    let len = f_lane_len(&world.lanes, fleet.from, fleet.to);
    let dprog = if len > 0.0 { wp.transit_speed / len } else { 1.0 };
    let remaining = (1.0 - fleet.progress).max(0.0);
    let cross = if dprog > 0.0 {
        (remaining / dprog).ceil() as u64
    } else {
        // No progress possible (zero transit speed): never arrives within any finite horizon.
        u64::MAX
    };
    // While undocking, progress does not advance; the two phases are sequential.
    undock.saturating_add(cross)
}

#[inline]
fn faction_byte(f: Faction) -> u8 {
    match f {
        Faction::Neutral => 0,
        Faction::Player => 1,
        // Ai(0)=2, Ai(1)=3, … — preserves the old Enemy/Enemy2 codes so existing levels' hashes hold.
        Faction::Ai(i) => 2u8.saturating_add(i),
    }
}

#[cfg(test)]
mod overwatch_tests {
    use super::*;
    use layer1::{SubStructure, Vec2 as V};

    /// A hostile fleet crossing a fully-owned struct's overwatch zone bleeds under the
    /// stochastic square law; with the fire probability zeroed it lands whole (owner
    /// design, 2026-07-08 — Layer-2 combat rolls dice like Layer 1).
    fn survivors(fire_prob: f64) -> bool {
        let mut params = layer1::SimParams::default();
        params.fire_prob = fire_prob;
        let wp = WorldParams::default();
        let mut w = World::new();
        let mut a = layer1::Interior::new(1);
        let sa = a.add_sub(SubStructure::new(V::new(0.0, 0.0), 0.0, Faction::Player));
        for _ in 0..52 {
            a.spawn_ship(Faction::Player, sa);
        }
        let mut b = layer1::Interior::new(2);
        // A fat producer: big l2_radius => a wide zone; a big garrison => strong fire.
        let sb = b.add_sub(
            SubStructure::new(V::new(0.0, 0.0), 0.0, Faction::Ai(0)).with_production(100),
        );
        for _ in 0..1000 {
            b.spawn_ship(Faction::Ai(0), sb);
        }
        let pa = w.add_struct(Structure::new(a, V::new(0.0, 0.0), "A"));
        let pb = w.add_struct(Structure::new(b, V::new(100.0, 0.0), "B"));
        w.add_lane(pa, pb, 100.0);
        assert!(w.issue_fleet_order(FleetOrder::new(pa, pb, layer1::FractionBucket::All), Faction::Player, &wp) > 0);
        for _ in 0..400 {
            w.step(&params, &wp);
            if w.structs[pb].interior.ship_count(Faction::Player) > 0 {
                return true; // some of the fleet landed
            }
            if w.fleets.is_empty() {
                break;
            }
        }
        false
    }

    #[test]
    fn overwatch_bleeds_hostile_fleets() {
        assert!(survivors(0.0), "with the fire probability zeroed the fleet must land");
        assert!(!survivors(0.5), "a hot zone must grind the fleet before it lands");
    }

    /// The one-sided band and the return fire: a fleet inside the NODE radius trades with
    /// the garrison (defenders die too); the same fight in the outer band is one-sided.
    #[test]
    fn return_fire_only_inside_the_node() {
        let mut params = layer1::SimParams::default();
        params.fire_prob = 0.5;
        // Hermetic: no soft-cap bleed and no production — every count change in this test
        // is Layer-2 combat and nothing else.
        params.softcap_attrition = 0.0;
        params.production_period = 1_000_000;
        let wp = WorldParams::default();
        let mut w = World::new();
        let mut a = layer1::Interior::new(1);
        let sa = a.add_sub(SubStructure::new(V::new(0.0, 0.0), 0.0, Faction::Player));
        for _ in 0..300 {
            a.spawn_ship(Faction::Player, sa);
        }
        let mut b = layer1::Interior::new(2);
        let sb = b.add_sub(
            SubStructure::new(V::new(0.0, 0.0), 0.0, Faction::Ai(0)).with_production(100),
        );
        // A THIN garrison: strong enough to prove deaths, weak enough that the big fleet
        // survives the one-sided band and actually reaches the mutual core.
        for _ in 0..10 {
            b.spawn_ship(Faction::Ai(0), sb);
        }
        let pa = w.add_struct(Structure::new(a, V::new(0.0, 0.0), "A"));
        let pb = w.add_struct(Structure::new(b, V::new(100.0, 0.0), "B"));
        w.add_lane(pa, pb, 100.0);
        let before = w.structs[pb].interior.ship_count(Faction::Ai(0));
        assert!(
            w.issue_fleet_order(FleetOrder::new(pa, pb, layer1::FractionBucket::All), Faction::Player, &wp) > 0
        );
        // Step only while the fleet is strictly OUTSIDE B's node radius: one-sided band.
        let node = w.structs[pb].l2_radius();
        loop {
            let Some(f) = w.fleets.first() else { break };
            if f.undock_remaining == 0 {
                let pos_x = f.progress * 100.0;
                // Margin of one transit step: fleets ADVANCE before combat inside a tick,
                // so break before a step could carry the fleet into the core and trade.
                if 100.0 - pos_x <= node + wp.transit_speed * 2.0 {
                    break;
                }
            }
            w.step(&params, &wp);
        }
        let pre_core = w.structs[pb].interior.ship_count(Faction::Ai(0));
        assert!(
            pre_core >= before,
            "no defender may die while the fleet is outside the node (one-sided band):              {pre_core} < {before}"
        );
        // Let the fight run into the core: now the garrison bleeds too.
        for _ in 0..200 {
            w.step(&params, &wp);
            if w.fleets.is_empty() {
                break;
            }
        }
        assert!(
            w.structs[pb].interior.ship_count(Faction::Ai(0)) < pre_core,
            "inside the node the fleet's return fire must land"
        );
    }

    /// Transit losses are REPORTED: every fleet ship destroyed by Layer-2 combat lands in
    /// `fleet_death_events` that tick — the GUI's only window on them, since they never touch
    /// an interior's liveness. When the zone grinds a fleet to nothing, the events must
    /// account for every launched ship.
    #[test]
    fn fleet_deaths_are_reported_as_events() {
        let mut params = layer1::SimParams::default();
        params.fire_prob = 1.0;
        params.softcap_attrition = 0.0;
        params.production_period = 1_000_000;
        let wp = WorldParams::default();
        let mut w = World::new();
        let mut a = layer1::Interior::new(1);
        let sa = a.add_sub(SubStructure::new(V::new(0.0, 0.0), 0.0, Faction::Player));
        for _ in 0..52 {
            a.spawn_ship(Faction::Player, sa);
        }
        let mut b = layer1::Interior::new(2);
        let sb = b.add_sub(
            SubStructure::new(V::new(0.0, 0.0), 0.0, Faction::Ai(0)).with_production(100),
        );
        for _ in 0..100 {
            b.spawn_ship(Faction::Ai(0), sb);
        }
        let pa = w.add_struct(Structure::new(a, V::new(0.0, 0.0), "A"));
        let pb = w.add_struct(Structure::new(b, V::new(100.0, 0.0), "B"));
        w.add_lane(pa, pb, 100.0);
        let launched =
            w.issue_fleet_order(FleetOrder::new(pa, pb, layer1::FractionBucket::All), Faction::Player, &wp);
        assert!(launched > 0);
        let mut reported = 0u32;
        for _ in 0..400 {
            w.step(&params, &wp);
            for e in &w.fleet_death_events {
                assert_eq!(e.faction, Faction::Player, "only the fleet side dies out there");
                assert_eq!(e.from, w.structs[pb].pos, "overwatch losses trace back to the struct");
                reported += e.count;
            }
            if w.fleets.is_empty() {
                break;
            }
        }
        assert_eq!(
            w.structs[pb].interior.ship_count(Faction::Player),
            0,
            "at fire 1.0 the zone lets nothing land"
        );
        assert_eq!(reported, launched, "every transit death must be reported as an event");
    }

    /// CONTESTED-STORAGE FIRE SPLIT (owner design, 2026-07-09): foes inside the interior
    /// claim their head-count share of the defenders' fire budget, so the overwatch volley
    /// fires at only the fleet share. Undistracted at fire 1.0, 100 defenders annihilate a
    /// 50-ship fleet the very tick it enters the band (100 sure hits ≥ 50 ships); with 300
    /// foes camped inside the interior the fleet share is 50/350, and the same first volley
    /// must leave survivors.
    #[test]
    fn interior_foes_claim_their_share_of_the_overwatch_budget() {
        fn first_blood_survivors(foes_inside: usize) -> u32 {
            let mut params = layer1::SimParams::default();
            params.fire_prob = 1.0;
            params.softcap_attrition = 0.0;
            params.production_period = 1_000_000;
            let wp = WorldParams::default();
            let mut w = World::new();
            let mut a = layer1::Interior::new(1);
            let sa = a.add_sub(SubStructure::new(V::new(0.0, 0.0), 0.0, Faction::Player));
            for _ in 0..52 {
                a.spawn_ship(Faction::Player, sa);
            }
            let mut b = layer1::Interior::new(2);
            let sb = b.add_sub(
                SubStructure::new(V::new(0.0, 0.0), 0.0, Faction::Ai(0)).with_production(100),
            );
            for _ in 0..100 {
                b.spawn_ship(Faction::Ai(0), sb);
            }
            // The distraction: hostile ships STAGED IN B'S STORAGE — the contested-storage
            // case itself. The reserve node is ownerless and never captured, so the camp
            // neither flips ground under the sole owner nor (out on the far reserve ring)
            // engages anyone in Layer-1 combat: only its head-count matters to the split.
            let far = b.add_storage_sub();
            for _ in 0..foes_inside {
                b.spawn_ship(Faction::Player, far);
            }
            let pa = w.add_struct(Structure::new(a, V::new(0.0, 0.0), "A"));
            let pb = w.add_struct(Structure::new(b, V::new(100.0, 0.0), "B"));
            w.add_lane(pa, pb, 100.0);
            let launched =
                w.issue_fleet_order(FleetOrder::new(pa, pb, layer1::FractionBucket::All), Faction::Player, &wp);
            assert!(launched > 0);
            for _ in 0..400 {
                w.step(&params, &wp);
                let dead: u32 = w.fleet_death_events.iter().map(|e| e.count).sum();
                if dead > 0 {
                    return launched - dead; // first blood: what the first volley left standing
                }
                assert!(!w.fleets.is_empty(), "the fleet must not arrive unharmed");
            }
            panic!("the zone never drew blood");
        }
        assert_eq!(
            first_blood_survivors(0),
            0,
            "an undistracted zone's first volley annihilates the fleet"
        );
        assert!(
            first_blood_survivors(300) > 0,
            "with interior foes claiming their share, the first volley is only the fleet share"
        );
    }
}
