//! # world — the Layer-2 LENS over multiple Layer-1 planets
//!
//! **"ONE WORLD, Layer 2 is a lens."** This crate makes Layer 2 (the tactical, Solarmax-like
//! zoomed-out view from `03-ui-layers.md`) a *view* over several Layer-1 battlefields, not a
//! second game. There is exactly **one** simulation substrate — the spatial Layer-1 sim in the
//! [`layer1`] crate — and every **planet** *is* a real [`layer1::Structure`] (its own
//! sub-structures and discrete ships). Layer 2 adds only:
//!
//! * **lanes** — edges between planets, and
//! * **inter-planet fleets** — ships in transit from one planet to another.
//!
//! It is **not** a second combat model. [`World::step`] simply steps every planet's
//! [`layer1::Structure`] (which fights/captures normally) and moves ships between planets
//! along lanes. When a fleet arrives it **injects** its ships into the destination planet's
//! `Structure`, spawned idle, so the ordinary Layer-1 sim then resolves the landing.
//!
//! ## The model at a glance
//!
//! * [`Planet`] — `{ structure: layer1::Structure, pos: Vec2, name: String }`. The `pos` is the
//!   planet's location on the **Layer-2 map** (a different space from the *intra*-planet
//!   `layer1` coordinates inside `structure`). All planets share one [`layer1::SimParams`].
//! * [`Lane`] — `{ a: PlanetId, b: PlanetId, length: f32 }`. Undirected; `length` sets transit
//!   time. `PlanetId` is the index into [`World::planets`].
//! * [`InterFleet`] — `{ faction, from, to, count, undock_remaining, progress }`. A clump of
//!   `count` ships of one faction crossing the `from`→`to` lane. It mirrors Layer-1's
//!   **undock-then-transit** movement: it first burns `undock_remaining` ticks leaving the
//!   source (like ships peeling off a sub), then advances `progress` from 0→1 along the lane.
//! * [`World`] — `{ planets, lanes, fleets, tick }` plus a lane adjacency index.
//! * [`FleetOrder`] — `{ from, to, fraction }`. The inter-planet atomic action: pull a
//!   [`layer1::FractionBucket`] of a faction's **idle** ships off the source planet (drawn from
//!   the sub-structures it owns, keeping a small per-sub floor) and launch them as one
//!   [`InterFleet`] along the connecting lane. Issued via [`World::issue_fleet_order`] with the
//!   acting faction (the data type stays faction-free, exactly like Layer-1's [`MoveOrder`]).
//!
//! ## Determinism
//!
//! The world preserves Layer-1's bit-reproducibility. All randomness still lives inside each
//! planet's `Structure` behind its own seeded PRNG; the inter-planet layer draws **none**.
//! [`World::step`] has a fixed, documented iteration order, and [`World::state_hash`] folds
//! every planet's [`layer1::Structure::state_hash`], every in-transit fleet, and the tick into
//! one 64-bit fingerprint. Same construction + same orders ⇒ identical hashes across reruns.

use layer1::{Faction, FractionBucket, SimParams, Structure, SubId, Vec2};

pub mod projection;
pub use projection::{CombatEvent, Projection, SubFate, DEFAULT_PROJECTION_HORIZON};

/// Index of a planet into [`World::planets`].
pub type PlanetId = usize;

/// Tunables for the **inter-planet** (Layer-2) layer only. Intra-planet behaviour is governed
/// by [`layer1::SimParams`]; these control how fleets cross lanes and how much surplus a planet
/// will export. All are documented dials; the defaults are the operating point the tests use.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WorldParams {
    /// Ticks a freshly launched fleet spends **undocking** before it starts crossing the lane.
    /// The Layer-2 analog of ships peeling off a sub-structure at Layer 1 — it makes a launch
    /// feel like a commitment that takes a moment to get moving rather than an instant teleport.
    pub undock_ticks: u32,

    /// Lane-length units a fleet covers per tick once it is transiting. Per-tick `progress`
    /// gain is `transit_speed / lane.length` (clamped into `[0,1]`), so a lane of `length L`
    /// takes about `L / transit_speed` ticks to cross (after undocking). Defaults to Layer-1's
    /// `ship_speed` so inter-planet travel feels consistent with intra-planet movement.
    pub transit_speed: f32,

    /// Per-sub idle **garrison floor** kept on the source planet when a fleet launches: no
    /// owned sub is drawn below this many idle ships. A planet always keeps a little home guard
    /// rather than exporting itself empty. See [`Structure::take_idle_ships_planetwide`].
    pub keep_floor: usize,
}

impl Default for WorldParams {
    fn default() -> Self {
        WorldParams { undock_ticks: 6, transit_speed: 1.4, keep_floor: 2 }
    }
}

/// A planet: an independent Layer-1 battlefield placed on the Layer-2 map.
///
/// The planet *is* its [`layer1::Structure`] — all its sub-structures, ships, production,
/// combat and capture happen inside `structure` under the shared [`layer1::SimParams`]. `pos`
/// is where the planet sits on the **Layer-2** map (used to draw it and to give lanes a
/// direction); it is unrelated to the intra-planet coordinates of `structure`.
#[derive(Debug, Clone)]
pub struct Planet {
    /// The real Layer-1 sim for this planet (its sub-structures + ships + RNG).
    pub structure: Structure,
    /// The planet's position on the Layer-2 map.
    pub pos: Vec2,
    /// A human-readable name (for the renderer/logs).
    pub name: String,
}

impl Planet {
    /// Build a planet from an existing Layer-1 [`Structure`] at Layer-2 position `pos`.
    pub fn new(structure: Structure, pos: Vec2, name: impl Into<String>) -> Planet {
        Planet { structure, pos, name: name.into() }
    }

    /// The planet's effective Layer-1 radius: the farthest sub-structure extent from the
    /// structure's local origin. Used to place a fleet's **entry point** on the perimeter
    /// facing the incoming lane. Zero for a structure with no sub-structures.
    fn local_radius(&self) -> f32 {
        self.structure
            .subs
            .iter()
            .map(|s| s.pos.dist(Vec2::new(0.0, 0.0)) + s.radius)
            .fold(0.0f32, f32::max)
    }
}

/// A Layer-2 lane: an undirected edge between two planets, with a `length` that sets transit
/// time. `a` and `b` are [`PlanetId`]s (indices into [`World::planets`]).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Lane {
    pub a: PlanetId,
    pub b: PlanetId,
    /// Lane length in Layer-2 map units. Drives how long a fleet takes to cross
    /// (see [`WorldParams::transit_speed`]). A non-positive length is treated as a minimum so a
    /// fleet still arrives in finite time.
    pub length: f32,
}

impl Lane {
    /// Convenience constructor.
    pub fn new(a: PlanetId, b: PlanetId, length: f32) -> Lane {
        Lane { a, b, length }
    }

    /// True if this lane connects `x` and `y` (in either direction).
    #[inline]
    fn connects(&self, x: PlanetId, y: PlanetId) -> bool {
        (self.a == x && self.b == y) || (self.a == y && self.b == x)
    }
}

/// A fleet of ships in transit between two planets along a lane.
///
/// Mirrors Layer-1's undock-then-transit movement: it first counts `undock_remaining` down to
/// zero (leaving the source planet), then advances `progress` from `0.0` to `1.0` across the
/// lane. On reaching `1.0` it **arrives** and its `count` ships are injected into the
/// destination planet's [`layer1::Structure`] as `faction`, spawned idle.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct InterFleet {
    /// Which seat owns the fleet (always a real seat — never `Neutral`).
    pub faction: Faction,
    /// Source planet (where the ships were pulled from).
    pub from: PlanetId,
    /// Destination planet (where the ships will be injected on arrival).
    pub to: PlanetId,
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

/// The inter-planet atomic action: launch a [`FractionBucket`] of a faction's idle ships from
/// `from` to `to` along the connecting lane.
///
/// Deliberately the same shape as Layer-1's [`layer1::MoveOrder`] (`source`/`target`/`fraction`,
/// here `from`/`to`/`fraction`) so the shared-vocabulary spine holds across layers. It carries
/// no faction — exactly like `MoveOrder` — because the *acting seat* is supplied at the call
/// site ([`World::issue_fleet_order`]). Only **connected** orders (a lane exists between `from`
/// and `to`) do anything; everything else (same planet, out-of-range ids, no lane, no idle
/// surplus) is a safe no-op.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FleetOrder {
    /// Source planet to pull idle ships from.
    pub from: PlanetId,
    /// Destination planet to send them to.
    pub to: PlanetId,
    /// How many of the source planet's idle ships to send (a fraction bucket).
    pub fraction: FractionBucket,
}

impl FleetOrder {
    /// Convenience constructor.
    pub fn new(from: PlanetId, to: PlanetId, fraction: FractionBucket) -> FleetOrder {
        FleetOrder { from, to, fraction }
    }
}

/// The Layer-2 aggregate ownership of a planet — the lens datum the renderer, the strategic AI,
/// and the greedy export rule read instead of peering at every sub-structure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlanetOwner {
    /// Exactly one real faction is present and it owns **every** owned sub-structure on the
    /// planet (no enemy sub, no enemy ship). The planet flies one flag.
    Owned(Faction),
    /// Both real factions have a presence (subs and/or ships) — the planet is being fought over.
    Contested,
    /// No real owner: no faction owns any sub and no real ships are present (all-neutral, or
    /// empty). The classic up-for-grabs planet.
    Neutral,
}

/// The Layer-2 aggregate **view** of a single planet: who effectively holds it, how many ships
/// each side has there (counting both garrisoned ships and fleets currently *arriving*), and
/// whether a faction holds it cleanly enough to export surplus.
///
/// This is computed from the planet's [`layer1::Structure`] plus the in-transit fleets headed
/// to it; it adds no state. Later phases (the strategic AI and the greedy export rule) consume
/// exactly this.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PlanetAggregate {
    /// Aggregate ownership (see [`PlanetOwner`]).
    pub owner: PlanetOwner,
    /// Living Player ships **garrisoned on the planet** (in its `Structure`).
    pub player_ships: usize,
    /// Living Enemy ships garrisoned on the planet.
    pub enemy_ships: usize,
    /// Player ships **currently arriving** (in fleets whose `to` is this planet).
    pub player_incoming: u32,
    /// Enemy ships currently arriving.
    pub enemy_incoming: u32,
    /// Player-owned sub-structures on the planet.
    pub player_subs: usize,
    /// Enemy-owned sub-structures on the planet.
    pub enemy_subs: usize,
    /// Neutral (unowned) sub-structures on the planet.
    pub neutral_subs: usize,
}

impl PlanetAggregate {
    /// Total ships (garrisoned + arriving) for `faction` associated with this planet.
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

    /// True iff `faction` owns **every** sub-structure on the planet AND **no enemy ship is
    /// present** (garrisoned). This is the precondition for a planet to *export surplus*: it is
    /// securely held, so shipping idle ships elsewhere will not immediately lose it. (Incoming
    /// friendly fleets do not affect this; an incoming *enemy* fleet has not landed yet, so it
    /// does not by itself make the planet non-exportable — once it lands the enemy ship is
    /// present and this flips to `false`.)
    pub fn fully_owned_uncontested(&self, faction: Faction) -> bool {
        match faction {
            Faction::Player => {
                self.player_subs > 0
                    && self.enemy_subs == 0
                    && self.neutral_subs == 0
                    && self.enemy_ships == 0
            }
            // Any AI rival: the combined non-player slot fully owns the planet, uncontested by the
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

/// World-level outcome — the Layer-2 mirror of [`layer1::Outcome`], aggregated over all planets
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
    /// Total ships across all planets **and fleets** `(player, enemy)`.
    pub ships: (usize, usize),
    /// Total owned sub-structures across all planets `(player, enemy)`.
    pub subs: (usize, usize),
}

/// Per-sub **in-transit influx** toward one planet's sub-structures, read directly from the current
/// in-flight state — the projection-free look-ahead the live game uses (see [`World::sub_influx_for`]).
/// Each `Vec` is indexed by [`SubId`] within the planet.
#[derive(Debug, Clone, Default)]
pub struct SubInflux {
    /// Acting seat's ships inbound to each sub (intra-structure moves + the seat's inter-planet fleets).
    pub mine: Vec<u32>,
    /// Every *other* real faction's ships inbound to each sub (the free-for-all foe in-flight force).
    pub foe: Vec<u32>,
    /// Earliest absolute tick a seat ship is projected to reach each sub (`None` if none inbound).
    pub friendly_eta: Vec<Option<u64>>,
}

/// The complete Layer-2 world: several Layer-1 planets, the lanes between them, the fleets in
/// transit, and the elapsed inter-planet tick.
///
/// Construct with [`World::new`], add planets with [`World::add_planet`] and lanes with
/// [`World::add_lane`], then drive it with [`World::step`]. Fully deterministic: see the crate
/// docs and [`World::state_hash`].
#[derive(Debug, Clone)]
pub struct World {
    /// All planets, indexed by [`PlanetId`].
    pub planets: Vec<Planet>,
    /// All lanes between planets.
    pub lanes: Vec<Lane>,
    /// All fleets currently in transit (undocking or crossing a lane).
    pub fleets: Vec<InterFleet>,
    /// Inter-planet ticks elapsed. Advances in lock-step with each planet's own `tick`
    /// (one `World::step` is one tick for every planet).
    pub tick: u64,
    /// Adjacency: for each planet, the [`PlanetId`]s it is laned to. Rebuilt whenever a lane is
    /// added so [`World::neighbors`] is O(1) and order is deterministic (lane insertion order).
    adjacency: Vec<Vec<PlanetId>>,
}

impl World {
    /// Create an empty world (no planets, lanes, or fleets).
    pub fn new() -> World {
        World { planets: Vec::new(), lanes: Vec::new(), fleets: Vec::new(), tick: 0, adjacency: Vec::new() }
    }

    /// Add a planet, returning its [`PlanetId`].
    pub fn add_planet(&mut self, planet: Planet) -> PlanetId {
        self.planets.push(planet);
        self.adjacency.push(Vec::new());
        self.planets.len() - 1
    }

    /// Add an undirected lane between `a` and `b` with the given `length`, returning its index
    /// in [`World::lanes`]. Out-of-range endpoints or a self-lane (`a == b`) are rejected
    /// (returns `None`); duplicate lanes are allowed (harmless — adjacency simply lists the
    /// neighbour twice, and order checks only ask whether *a* lane exists).
    pub fn add_lane(&mut self, a: PlanetId, b: PlanetId, length: f32) -> Option<usize> {
        if a == b || a >= self.planets.len() || b >= self.planets.len() {
            return None;
        }
        self.lanes.push(Lane::new(a, b, length));
        self.adjacency[a].push(b);
        self.adjacency[b].push(a);
        Some(self.lanes.len() - 1)
    }

    /// The planets directly laned to `p` (in lane-insertion order). Empty if `p` is out of range.
    pub fn neighbors(&self, p: PlanetId) -> &[PlanetId] {
        self.adjacency.get(p).map(|v| v.as_slice()).unwrap_or(&[])
    }

    /// True if a lane connects `from` and `to`.
    pub fn are_connected(&self, from: PlanetId, to: PlanetId) -> bool {
        from < self.planets.len()
            && to < self.planets.len()
            && self.lanes.iter().any(|l| l.connects(from, to))
    }

    /// The length of the (first) lane connecting `from` and `to`, if any. Convenience for a
    /// renderer/AI that wants the crossing distance (e.g. to estimate a fleet's arrival time).
    pub fn lane_length(&self, from: PlanetId, to: PlanetId) -> Option<f32> {
        self.lanes.iter().find(|l| l.connects(from, to)).map(|l| l.length)
    }

    // ----------------------------------------------------------------------
    // Inter-planet orders
    // ----------------------------------------------------------------------

    /// Issue a [`FleetOrder`] for `faction`: pull a fraction-bucket of `faction`'s **idle**
    /// ships off planet `from` (drawn from the sub-structures it owns, keeping
    /// [`WorldParams::keep_floor`] idle per sub) and launch them as one [`InterFleet`] toward
    /// `to` along the connecting lane. Returns the number of ships actually launched.
    ///
    /// It is robust to junk (safe no-op returning 0) when: `from == to`, either id is out of
    /// range, **no lane connects** `from` and `to`, `faction` is `Neutral`, or the source
    /// planet has no exportable idle surplus for `faction`. The pulled ships leave the source
    /// planet's `Structure` immediately (so they cannot be ordered again or fight there); they
    /// reappear, conserved, when the fleet arrives. Mirrors Layer-1's "commit, then it's
    /// flying" — once launched, a fleet is not redirected.
    pub fn issue_fleet_order(&mut self, order: FleetOrder, faction: Faction, wp: &WorldParams) -> u32 {
        let FleetOrder { from, to, fraction } = order;
        self.launch_fleet(from, to, faction, wp, |s, f, floor| {
            // A 100% order takes *everything* — no home-guard floor left behind.
            let floor = if fraction.as_f32() >= 1.0 { 0 } else { floor };
            s.take_idle_ships_planetwide(f, fraction, floor)
        })
    }

    /// Like [`World::issue_fleet_order`] but with a **continuous** send-fraction `frac` in `(0,1]`
    /// — the GUI's free 1–100 % troop slider — instead of a [`layer1::FractionBucket`]. Same lane
    /// validation, keep-floor and determinism; the four snap positions match the buckets exactly.
    pub fn issue_fleet_order_fraction(
        &mut self,
        from: PlanetId,
        to: PlanetId,
        frac: f32,
        faction: Faction,
        wp: &WorldParams,
    ) -> u32 {
        self.launch_fleet(from, to, faction, wp, |s, f, floor| {
            // A 100% order takes *everything* — no home-guard floor left behind.
            let floor = if frac >= 1.0 { 0 } else { floor };
            s.take_idle_ships_planetwide_fraction(f, frac, floor)
        })
    }

    /// Shared core of the fleet orders: reject junk (disconnected / out-of-range / `from == to` /
    /// `Neutral`), then pull the source planet's exportable surplus via `pull` (RNG-free; does not
    /// perturb determinism) and, if any, launch it as one [`InterFleet`]. Returns ships launched.
    fn launch_fleet(
        &mut self,
        from: PlanetId,
        to: PlanetId,
        faction: Faction,
        wp: &WorldParams,
        pull: impl Fn(&mut Structure, Faction, usize) -> usize,
    ) -> u32 {
        if from == to
            || from >= self.planets.len()
            || to >= self.planets.len()
            || !faction.is_real()
            || !self.are_connected(from, to)
        {
            return 0;
        }
        let taken = pull(&mut self.planets[from].structure, faction, wp.keep_floor);
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
    /// 1. **Planets** — step every planet's [`layer1::Structure`] in ascending [`PlanetId`]
    ///    order (each does its own production → movement → combat → capture internally).
    /// 2. **Fleets** — advance every in-transit fleet in `fleets`-vector order: burn an
    ///    undock tick, else add this tick's lane progress.
    /// 3. **Arrivals** — any fleet that has now fully crossed its lane injects its ships into
    ///    the destination planet (see [`World::inject_fleet`]) and is removed; survivors keep
    ///    their relative order.
    /// 4. **tick** += 1.
    ///
    /// Injection happens *after* this tick's planet steps, so freshly landed ships first fight
    /// on the **next** tick (they arrive idle at the end of this one) — the same "no
    /// retroactive action this tick" discipline Layer-1 uses for production/capture.
    pub fn step(&mut self, params: &SimParams, wp: &WorldParams) {
        // (1) Step every planet's spatial sim.
        for planet in self.planets.iter_mut() {
            planet.structure.step(params);
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

    /// Inject an arrived fleet's `count` ships into the destination planet's [`layer1::Structure`]
    /// as `faction`, spawned **idle**, so the ordinary Layer-1 sim then resolves the landing
    /// (fight / capture) on subsequent ticks.
    ///
    /// **Entry point.** Fleets land in the destination's **reserve / patrol-zone node**
    /// (`storage_sub`) — the universal inter-planet entry point. Every campaign planet has one,
    /// so this is the normal path. Only a bare structure with no reserve falls back to the
    /// lane-facing [`World::entry_sub`] rule (reinforce the owned sub nearest where the lane
    /// enters, else beachhead at the nearest sub facing the source). If the destination has no
    /// sub-structures at all, nothing is injected (the ships are dropped — a degenerate map the
    /// constructors never build).
    fn inject_fleet(&mut self, f: &InterFleet) {
        // Fleets arrive into the destination's **reserve / patrol-zone** node (the universal entry
        // point) if it has one; otherwise the lane-facing entry sub (bare structures with no reserve).
        let entry = match self.planets[f.to]
            .structure
            .storage_sub
            .or_else(|| self.entry_sub(f.to, f.from, f.faction))
        {
            Some(s) => s,
            None => return, // destination has no sub-structures; nothing to garrison at
        };
        let planet = &mut self.planets[f.to];
        for _ in 0..f.count {
            planet.structure.spawn_ship(f.faction, entry);
        }
    }

    /// Choose the destination sub-structure an arriving `faction` fleet from `from` garrisons
    /// at (see [`World::inject_fleet`] for the rule). `None` if the destination has no subs.
    ///
    /// **Public so the forward-[`projection`] schedules a fleet's arrival into the *identical*
    /// landing sub the sim would inject into** (R3 / §5) — re-deriving the rule in the AI would
    /// risk drift. Pure read; draws no randomness.
    pub fn entry_sub(&self, dest: PlanetId, from: PlanetId, faction: Faction) -> Option<SubId> {
        let d = self.planets.get(dest)?;
        if d.structure.subs.is_empty() {
            return None;
        }
        // Direction on the Layer-2 map from the destination toward the source.
        let to_src = Vec2::new(d.pos.x, d.pos.y);
        let src = &self.planets.get(from)?.pos;
        let mut dx = src.x - to_src.x;
        let mut dy = src.y - to_src.y;
        let mag = (dx * dx + dy * dy).sqrt();
        if mag > 1e-6 {
            dx /= mag;
            dy /= mag;
        } else {
            // Coincident planets: fall back to +x so the choice is still deterministic.
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
            for (i, s) in d.structure.subs.iter().enumerate() {
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

    /// Per-sub [`SubInflux`] toward `planet` for `seat`, read **directly** from the current in-flight
    /// state (no forward [`projection`]). This is the live game's projection-free look-ahead: it mirrors
    /// exactly what the projection scheduled as "arrivals", but off the *present* state instead of a
    /// forward simulation —
    /// * **(a) intra-structure moving ships** are attributed to their `target` sub, with the same
    ///   undock-then-straight-line ETA the sim uses
    ///   (`undock_remaining + ceil((dist-tolerance)/ship_speed)`);
    /// * **(b) inter-planet fleets** inbound to this planet are attributed to the sub they will
    ///   actually land at — the reserve / patrol node (`storage_sub`) if present, else the lane-facing
    ///   [`World::entry_sub`] — *identical to [`World::inject_fleet`]* (this fixes the old projection's
    ///   entry-sub divergence, which always routed arrivals to the entry sub).
    ///
    /// Free-for-all: every real faction other than `seat` is counted as a foe. A foe mover already
    /// **within its target's engaging reach** (`radius + engagement_radius`) is *not* counted in
    /// `foe` — it is already present in any engaging-ships read of that sub, and counting it in
    /// both would double the threat. Deterministic: a pure function of the world state (positions,
    /// fleets) in f32 with a single `ceil`, so identical inputs give an identical influx and
    /// `state_hash` replay stays bit-identical. Out-of-range `planet` yields an empty influx.
    pub fn sub_influx_for(
        &self,
        planet: PlanetId,
        seat: Faction,
        sp: &SimParams,
        wp: &WorldParams,
    ) -> SubInflux {
        const EPS: f32 = 1e-6;
        let Some(planet_ref) = self.planets.get(planet) else {
            return SubInflux { mine: vec![], foe: vec![], friendly_eta: vec![] };
        };
        let st = &planet_ref.structure;
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

        // (b) Inter-planet fleets inbound to this planet -> their real landing sub (reserve else entry).
        for f in &self.fleets {
            if f.to != planet || !f.faction.is_real() {
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

    /// Compute the [`PlanetAggregate`] for planet `p`: aggregate ownership, per-faction ship
    /// counts (garrisoned **plus** currently arriving), sub-structure tallies, and the
    /// exportable flag. Reads the planet's `Structure` and the in-transit `fleets`; adds no
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
    pub fn planet_aggregate(&self, p: PlanetId) -> PlanetAggregate {
        if p >= self.planets.len() {
            return PlanetAggregate {
                owner: PlanetOwner::Neutral,
                player_ships: 0,
                enemy_ships: 0,
                player_incoming: 0,
                enemy_incoming: 0,
                player_subs: 0,
                enemy_subs: 0,
                neutral_subs: 0,
            };
        }
        let st = &self.planets[p].structure;
        let player_ships = st.ship_count(Faction::Player);
        // Layer-2 lens: every non-player rival is aggregated into the binary "enemy" slot (summed over
        // any number of AI seats — no hardcoded count). Per-seat Layer-2 (telling rivals apart in the
        // lens / pie chart) is deferred; no current level fields a multi-planet free-for-all.
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
            (true, true) => PlanetOwner::Contested,
            (true, false) => PlanetOwner::Owned(Faction::Player),
            (false, true) => PlanetOwner::Owned(Faction::Ai(0)),
            (false, false) => PlanetOwner::Neutral,
        };

        PlanetAggregate {
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
    // These lift the new `layer1::Structure` reads (resistance, parked count, soft cap) to
    // planet scope so the strategic AI does not reach into a planet's `Structure`. All are
    // pure, deterministic reads that add no state and draw no randomness.

    /// Total foreign capture resistance on planet `p` from `seat`'s point of view: the sum of
    /// `resistance` over every sub on `p` **not** owned by `seat` (neutral and enemy subs). This
    /// is the quantity a resistance-proportional colonizer sizes a capture wave on. Out-of-range
    /// `p` yields `0.0`.
    pub fn planet_total_resistance_vs(&self, p: PlanetId, seat: Faction) -> f32 {
        match self.planets.get(p) {
            Some(planet) => planet.structure.total_foreign_resistance(seat),
            None => 0.0,
        }
    }

    /// Parked ships of `seat` on planet `p` (living ships in the planet's `Structure` — idle or
    /// intra-structure transit). Inter-planet fleets to/from `p` are **not** counted (they live
    /// in [`World::fleets`], exempt from the soft cap). Out-of-range `p` yields `0`. Mirrors
    /// [`layer1::Structure::parked_count`].
    pub fn parked_count(&self, p: PlanetId, seat: Faction) -> u32 {
        match self.planets.get(p) {
            Some(planet) => planet.structure.parked_count(seat),
            None => 0,
        }
    }

    /// The soft cap for `seat` on planet `p` — `softcap_free` plus the **sum of per-sub
    /// capacities** of the subs `seat` owns there (see [`layer1::SubStructure::soft_cap_capacity`];
    /// numerically `softcap_free + softcap_per_sub * owned_subs` today). When [`World::parked_count`]
    /// exceeds this, the planet's `Structure` bleeds the overflow with `sqrt` attrition.
    /// Out-of-range `p` yields `0`. Mirrors [`layer1::Structure::soft_cap`].
    pub fn soft_cap(&self, p: PlanetId, seat: Faction, sp: &SimParams) -> u32 {
        match self.planets.get(p) {
            Some(planet) => planet.structure.soft_cap(seat, sp),
            None => 0,
        }
    }

    // ----------------------------------------------------------------------
    // World-level outcome
    // ----------------------------------------------------------------------

    /// Total living ships of `faction` across **all** planets and **all** in-transit fleets.
    pub fn total_ships(&self, faction: Faction) -> usize {
        let garrisoned: usize = self.planets.iter().map(|p| p.structure.ship_count(faction)).sum();
        let flying: usize = self
            .fleets
            .iter()
            .filter(|f| f.faction == faction)
            .map(|f| f.count as usize)
            .sum();
        garrisoned + flying
    }

    /// Total sub-structures owned by `faction` across all planets.
    pub fn total_subs(&self, faction: Faction) -> usize {
        self.planets.iter().map(|p| p.structure.sub_count(faction)).sum()
    }

    /// Total living ships of every real seat **other than** `seat`, across all planets and fleets —
    /// the free-for-all "all my rivals" ship total, summed over any number of AI opponents.
    pub fn total_foreign_ships(&self, seat: Faction) -> usize {
        let garrisoned: usize = self.planets.iter().map(|p| p.structure.foreign_ship_count(seat)).sum();
        let flying: usize = self
            .fleets
            .iter()
            .filter(|f| f.faction.is_real() && f.faction != seat)
            .map(|f| f.count as usize)
            .sum();
        garrisoned + flying
    }

    /// Total sub-structures owned by every real seat **other than** `seat`, across all planets.
    pub fn total_foreign_subs(&self, seat: Faction) -> usize {
        self.planets.iter().map(|p| p.structure.foreign_sub_count(seat)).sum()
    }

    /// True if `faction` is **world-wide eliminated**: it owns no sub on any planet **and** has
    /// no ships anywhere (garrisoned or in transit). Mirrors Layer-1's elimination, lifted to
    /// the whole world.
    pub fn is_eliminated(&self, faction: Faction) -> bool {
        self.total_ships(faction) == 0 && self.total_subs(faction) == 0
    }

    /// The world outcome **as of now** — the Layer-2 mirror of [`layer1::Structure::outcome`].
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
        // Every rival eliminated ⇔ no non-player real ship or owned sub remains anywhere.
        let enemies_dead = e_ships == 0 && e_subs == 0;

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

    /// A 64-bit fingerprint of the world's state: every planet's
    /// [`layer1::Structure::state_hash`] (which already folds that planet's full sim state and
    /// RNG position), every in-transit fleet, every lane, and the world tick. Two worlds built
    /// identically and driven with the same orders produce identical hashes at every tick — the
    /// determinism tests assert on this. The params (`SimParams` / [`WorldParams`]) are **not**
    /// folded: the hash compares identically-parameterised runs, not configurations.
    ///
    /// Implemented as an inline FNV-1a, the same construction `layer1` uses; floats are folded
    /// by bit pattern so the comparison is exact.
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
        // Planets, in PlanetId order: fold each planet's own state hash and its map position.
        mix_u64(&mut h, self.planets.len() as u64);
        for p in &self.planets {
            mix_u64(&mut h, p.structure.state_hash());
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
fn f_lane_len(lanes: &[Lane], from: PlanetId, to: PlanetId) -> f32 {
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
