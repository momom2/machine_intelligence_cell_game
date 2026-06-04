//! The Layer-1 spatial simulation: structure, ships, movement, proximity battle bubbles,
//! stochastic square-law combat, capture, and the outcome.
//!
//! # The model (implements the project owner's Layer-1 spec exactly)
//!
//! > "Layer 1 is a single structure composed of multiple sub-structures, and ships can be
//! > moved from one sub-structure to another. When within a close enough distance, ships
//! > are engaged in a battle bubble. Depending on the layout of the structure, ships may
//! > not need to be in the same sub-structure to battle."
//!
//! Concretely:
//! * A [`Structure`] is **one** structure = several [`SubStructure`]s at 2D positions, plus
//!   a flat pool of discrete [`Ship`]s.
//! * Ships **garrison** at a sub-structure (idle) or **move** to another at a fixed speed,
//!   with a little per-ship spread so they do not perfectly overlap.
//! * Combat is **purely proximity-based on individual ship positions**: any ship with a
//!   living enemy ship within the [`SimParams::engagement_radius`] is *engaged*. Ships near
//!   the border between two close sub-structures therefore fight across them — being in the
//!   same sub-structure is **not** required. The layout (positions + radii) decides who can
//!   fight whom.
//!
//! # Combat — stochastic Lanchester square law (the Layer-1 / spectacle model)
//!
//! `01-mechanics.md`: each engaged ship is a **stochastic emitter** that destroys one enemy
//! ship when it fires, with expected damage-per-tick proportional to the number of engaged
//! ships on its side. Per combat sub-step, every engaged ship fires with probability
//! [`SimParams::fire_prob`]; on firing it one-shots a random living enemy within range.
//! Because each side's shooter count is proportional to its engaged ship count, the enemy's
//! loss rate is proportional to *your* engaged count — i.e. the stochastic **square law**
//! (`2x ships => ~4x relative advantage`; the test suite verifies this emerges). Large
//! battles trend deterministic (~`1/sqrt(N)` spread), small skirmishes feel chancy.
//!
//! Everything is deterministic **given a seed**: the only randomness is drawn from the
//! seeded [`crate::rng::Rng`] threaded through [`Structure::step`].

use crate::rng::Rng;
use crate::types::*;

/// Fresh-sub resistance (= starting value = the value a captured sub refills to). The master
/// **grind dial**: clearing a fresh sub with `F` present, uncontested attackers takes
/// `ceil(DEFAULT_MAX_RESISTANCE / F)` ticks. Per the v1-polish design review this is
/// `1800` (~100 production periods at the default `production_period = 18`), a Solarmax-paced
/// grind. Per-sub overridable via [`SubStructure::with_max_resistance`].
pub const DEFAULT_MAX_RESISTANCE: f32 = 1800.0;

/// A sub-structure: a placed module of the single Layer-1 structure where ships garrison.
///
/// Owned by a [`Faction`] (or `Neutral`), it slowly **produces** one new ship for its owner
/// every [`SimParams::production_period`] ticks (`Neutral` produces nothing). Production is
/// the reason to hold ground — it feeds the square-law snowball.
///
/// # Capture is a grind, not an instant flip
///
/// Each sub carries a [`SubStructure::resistance`] bar in `[0, max_resistance]`, starting full.
/// Capture is the slow erosion of that bar by the *single* uncontested foreign faction present
/// (see [`Structure::resolve_resistance`] / the pure [`SubStructure::capture_step`]): the owner
/// healing it back while present, an attacker grinding it down, a flip + refill at zero. The old
/// instant "uncontested presence flips it" rule is gone.
#[derive(Debug, Clone, PartialEq)]
pub struct SubStructure {
    /// Centre position in the structure's local plane.
    pub pos: Vec2,
    /// Physical extent. A ship "sits inside" this sub-structure when within `radius` of
    /// `pos`; that confers the optional defender edge and matters for capture.
    pub radius: f32,
    /// Current owner (or `Neutral`).
    pub owner: Faction,
    /// Ticks until this sub-structure next spawns a ship for its owner. Counts down each
    /// tick; on reaching 0 it spawns and resets to [`SimParams::production_period`].
    /// Held at the period while `Neutral`.
    pub production_timer: u32,
    /// **Capture resistance**, in `[0, max_resistance]`. Starts at `max_resistance`. An
    /// uncontested foreign faction with `E` ships present erodes it by `E`/tick; the owner
    /// present and uncontested heals it by its present count/tick (capped at `max_resistance`).
    /// On reaching `<= 0` the sub flips to the eroding faction and refills to `max_resistance`.
    pub resistance: f32,
    /// The cap (and refill value) of [`SubStructure::resistance`]. Default
    /// [`DEFAULT_MAX_RESISTANCE`]; override per sub with [`SubStructure::with_max_resistance`].
    /// Always `>= 1.0`.
    pub max_resistance: f32,
}

impl SubStructure {
    /// Create a sub-structure at `pos` with `radius`, owned by `owner`. Its resistance starts
    /// full at [`DEFAULT_MAX_RESISTANCE`].
    pub fn new(pos: Vec2, radius: f32, owner: Faction) -> SubStructure {
        SubStructure {
            pos,
            radius,
            owner,
            production_timer: 0,
            resistance: DEFAULT_MAX_RESISTANCE,
            max_resistance: DEFAULT_MAX_RESISTANCE,
        }
    }

    /// Builder: set this sub's `max_resistance` (clamped to `>= 1.0`) and refill its current
    /// resistance to that max. Lets a scenario make a sub a cheap foothold (low max) or a
    /// fortress (high max) without touching the global [`DEFAULT_MAX_RESISTANCE`].
    pub fn with_max_resistance(mut self, max: f32) -> SubStructure {
        let m = max.max(1.0);
        self.max_resistance = m;
        self.resistance = m;
        self
    }

    /// This sub's contribution to its **owner's** per-structure soft-cap headroom, in ships —
    /// the per-element capacity that [`Structure::soft_cap`] sums over a faction's owned subs
    /// (`soft = softcap_free + Σ sub_capacity`). Uniform today (every owned sub returns
    /// [`SimParams::softcap_per_sub`]), but expressing the cap as a **sum of per-sub capacities**
    /// rather than `softcap_per_sub * count` is what lets a future sub *type* (a "warehouse" sub
    /// with extra storage, a thin "entry/exit" sub with none, …) change the cap purely by
    /// returning a different value here — no projection/AI code changes. Modularity hinge for the
    /// forward-projection's soft-cap reads.
    #[inline]
    pub fn soft_cap_capacity(&self, params: &SimParams) -> u32 {
        // Uniform per-sub allowance today. A future warehouse/factory sub would branch on a sub
        // `kind` field here and return a larger/smaller capacity; everything downstream (the
        // structure roll-up, the projection's overstack guard) already sums this accessor.
        params.softcap_per_sub
    }

    /// The pure capture rule for one sub over one tick — the **single source of truth** the sim
    /// ([`Structure::resolve_resistance`]) and the forward-projection (in the `world` crate)
    /// both call, so the grind can never drift between them. Given the current `owner`,
    /// `resistance`, `max_resistance`, and the living present counts of each real seat, return
    /// `(new_owner, new_resistance, flipped)`:
    ///
    /// * **Frozen** — zero present, or *both* seats present (contested): no change.
    /// * **Heal** — only the owner present: `resistance` rises by its present count, capped at
    ///   `max_resistance`.
    /// * **Erode** — only a *foreign* seat present: `resistance` falls by that seat's count;
    ///   on reaching `<= 0` the sub flips to that seat and refills to `max_resistance`
    ///   (`flipped = true`). A `Neutral`-owned sub is always eroding (no ship is `Neutral`).
    ///
    /// Pure and deterministic: draws no randomness and touches no global state.
    #[inline]
    pub fn capture_step(
        owner: Faction,
        resistance: f32,
        max_resistance: f32,
        present_player: u32,
        present_enemy: u32,
    ) -> (Faction, f32, bool) {
        // The single present faction (None => zero present OR both present => frozen).
        let single: Option<(Faction, u32)> = match (present_player > 0, present_enemy > 0) {
            (true, false) => Some((Faction::Player, present_player)),
            (false, true) => Some((Faction::Enemy, present_enemy)),
            _ => None,
        };
        match single {
            None => (owner, resistance, false), // frozen
            Some((f, count)) => {
                if f == owner {
                    // Owner present, uncontested => HEAL toward the cap.
                    let healed = (resistance + count as f32).min(max_resistance);
                    (owner, healed, false)
                } else {
                    // Exactly one foreign faction, uncontested => ERODE.
                    let eroded = resistance - count as f32;
                    if eroded <= 0.0 {
                        (f, max_resistance, true) // FLIP + REFILL
                    } else {
                        (owner, eroded, false)
                    }
                }
            }
        }
    }
}

/// A discrete ship — the unit of Layer-1 combat.
///
/// Ships are never partial: combat removes a *whole* ship via a stochastic one-shot kill
/// (matching `01`'s "destroys an enemy ship when it fires"). A dead ship is marked
/// `alive = false` and keeps its slot (its [`ShipId`] stays stable for the renderer).
#[derive(Debug, Clone, PartialEq)]
pub struct Ship {
    /// Owning seat (always a real seat — ships are never `Neutral`).
    pub faction: Faction,
    /// Current 2D position.
    pub pos: Vec2,
    /// Where the ship is headed:
    /// * `None` — idle, garrisoning at [`Ship::home`].
    /// * `Some(sub)` — moving toward sub-structure `sub` at [`SimParams::ship_speed`],
    ///   aiming at a slightly jittered point inside its radius so ships fan out.
    pub target: Option<SubId>,
    /// The sub-structure this ship currently belongs to (its garrison home while idle, or
    /// the one it last departed while moving). Used for "idle ships at S" queries and to
    /// decide which sub-structures a faction effectively holds.
    pub home: SubId,
    /// Jittered aim point within the target's radius (only meaningful while moving). Stored
    /// so the ship flies a straight line to a stable spread point rather than re-jittering.
    pub aim: Vec2,
    /// `false` once destroyed. Dead ships are skipped everywhere and never fire/are hit.
    pub alive: bool,
}

impl Ship {
    /// True if this ship is alive and currently idle (garrisoning, no move target).
    #[inline]
    pub fn is_idle(&self) -> bool {
        self.alive && self.target.is_none()
    }
}

/// Tunable constants governing the Layer-1 sim. All are documented dials; the defaults are
/// the operating point the headless runner and tests use. See `LAYER1_SIM.md`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SimParams {
    /// **Engagement radius `R`** (metres). A ship is *engaged* when a living enemy ship is
    /// within `R`. Defines the battle bubble. Larger `R` => fights start sooner and across
    /// wider sub-structure gaps.
    pub engagement_radius: f32,

    /// **Fire probability `p`** per engaged ship per combat sub-step. On firing, the ship
    /// one-shots a random living enemy within `R`. Expected kills/tick scale with the
    /// number of engaged shooters — this is what makes combat a stochastic square law.
    pub fire_prob: f64,

    /// **Combat sub-steps per tick.** Combat is resolved in this many equal sub-steps each
    /// tick (interleaving both sides' fire) so kills are smooth and neither side gets a
    /// whole tick of free shooting. Higher => smoother; determinism is unaffected.
    pub combat_substeps: u32,

    /// **Ship speed** (metres per tick) while moving toward a target sub-structure.
    pub ship_speed: f32,

    /// **Arrival tolerance** (metres). A moving ship is considered arrived (becomes idle,
    /// `home = target`) when within this distance of its jittered aim point.
    pub arrival_tolerance: f32,

    /// **Per-ship spread radius** (metres). When ordered to a sub-structure, each ship aims
    /// at a random point within this radius of a chosen spot inside the target, so ships
    /// fan out instead of stacking on one pixel.
    pub spread_radius: f32,

    /// **Production period** (ticks). An owned sub-structure spawns one ship for its owner
    /// every this-many ticks. Smaller => faster snowball. `Neutral` sub-structures do not
    /// produce.
    pub production_period: u32,

    /// **Defender edge** — extra fire probability (additive, before clamping) granted to a
    /// ship firing while it sits inside one of *its own* sub-structures' radius. The
    /// Layer-1 analog of defender advantage (`01` "you may still need an explicit defender
    /// term"). Modest by default; set to `0.0` to disable.
    pub defender_fire_bonus: f64,

    /// Cap on the number of ships a single sub-structure can spawn over the match. Purely a
    /// safety bound so a runaway snowball cannot allocate without limit in a pathological
    /// config; far above normal play. Not a strategic dial.
    pub max_ships_per_sub: u32,

    /// **Soft-cap free allowance** — flat parked-ship headroom per faction per structure,
    /// independent of how many subs it owns. Part of `soft = softcap_free + softcap_per_sub *
    /// owned_subs`. See [`Structure::resolve_softcap`].
    pub softcap_free: u32,

    /// **Soft-cap per-owned-sub allowance** — parked headroom added per owned sub. With the
    /// default `10`, equilibrium surplus settles at ≈ 10× production (10 ships of slack per
    /// owned sub). Part of `soft = softcap_free + softcap_per_sub * owned_subs`.
    pub softcap_per_sub: u32,

    /// **Soft-cap attrition coefficient.** When a faction's parked ships exceed its soft cap by
    /// `over`, `ceil(softcap_attrition * sqrt(over))` of its parked ships are destroyed this
    /// tick (random via the structure RNG). The `sqrt` shape makes the cap a self-limiting
    /// plateau (the count settles just above `soft`) rather than a hard wall.
    pub softcap_attrition: f32,

    /// **Structure hard cap** — a far-above-play safety bound on a faction's parked ships in one
    /// structure. NOT a strategic dial: there is intentionally no hard strategic ceiling. It
    /// only guarantees a pathological configuration cannot grow parked stacks without limit.
    pub structure_hard_cap: u32,

    /// **Per-sub orbit cap** (positional only). When more than this many of a faction's ships
    /// would idle at a single sub, the overflow is conceptually *placed* at a wider structure
    /// orbit so one sub is not an infinitely dense dot. It is a rendering/positioning concern:
    /// it NEVER destroys ships and is **not** enforced inside [`Structure::resolve_softcap`]
    /// (which would draw RNG). Kept here as the documented dial.
    pub sub_orbit_cap: u32,
}

impl Default for SimParams {
    /// The Layer-1 operating point. Tuned so a ~5-7 sub-structure skirmish resolves in a
    /// few hundred ticks with chancy small fights and decisive large ones, and so combat is
    /// not so lethal that a single opening clash ends the match — reinforcement and capture
    /// get time to matter.
    fn default() -> Self {
        SimParams {
            engagement_radius: 7.0,
            fire_prob: 0.035,
            combat_substeps: 4,
            ship_speed: 1.4,
            arrival_tolerance: 0.75,
            spread_radius: 2.2,
            production_period: 18,
            defender_fire_bonus: 0.012,
            max_ships_per_sub: 4000,
            softcap_free: 20,
            softcap_per_sub: 10,
            softcap_attrition: 1.0,
            structure_hard_cap: 1000,
            sub_orbit_cap: 50,
        }
    }
}

/// A single battle bubble: a connected cluster of mutually-in-range *opposing* ships.
///
/// Exposed so the future renderer can draw the bubble (e.g. a glowing hull around the
/// brawl). A bubble exists only where at least two opposing ships are within `R` of a chain
/// of engaged ships; pure-friendly clusters are not bubbles.
#[derive(Debug, Clone, PartialEq)]
pub struct BattleBubble {
    /// Ships (by [`ShipId`]) participating in this engagement, both factions mixed.
    pub ships: Vec<ShipId>,
    /// Axis-aligned centre of the participating ships (a convenient anchor for drawing).
    pub center: Vec2,
    /// Bounding radius from `center` covering all participants (for a quick draw extent).
    pub radius: f32,
    /// Living ship counts within the bubble, per side: `(player, enemy)`.
    pub player_count: usize,
    pub enemy_count: usize,
}

/// Living present counts of each real seat inside one sub-structure plus its owner — the
/// inputs to the capture rule, returned by [`Structure::sub_presence`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SubPresence {
    /// Living `Player` ships inside the sub's radius.
    pub player: u32,
    /// Living `Enemy` ships inside the sub's radius.
    pub enemy: u32,
    /// The sub's current owner.
    pub owner: Faction,
}

/// Who has won, or the lead at the horizon.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Outcome {
    /// `Some(faction)` if that faction won (the other was eliminated, or it led at the
    /// horizon). `None` only for an exact tie at the horizon.
    pub winner: Option<Faction>,
    /// True if the match ended by elimination rather than reaching the horizon.
    pub by_elimination: bool,
    /// Tick at which the outcome was taken.
    pub tick: u64,
    /// Final ship counts `(player, enemy)`.
    pub ships: (usize, usize),
    /// Final owned-sub-structure counts `(player, enemy)`.
    pub subs: (usize, usize),
}

/// The complete, mutable Layer-1 battlefield: one structure (its sub-structures) plus all
/// ships, the seeded RNG, and the elapsed tick count.
///
/// This is the single object the renderer reads and the AI/GUI drive. It is fully
/// deterministic given its seed: `Structure::step` is the only place randomness enters, and
/// it draws solely from the embedded [`Rng`].
#[derive(Debug, Clone)]
pub struct Structure {
    /// The sub-structures making up this single structure.
    pub subs: Vec<SubStructure>,
    /// All ships, alive and (marked) dead. Stable indices = stable [`ShipId`]s.
    pub ships: Vec<Ship>,
    /// Whole ticks elapsed since the start.
    pub tick: u64,
    /// The seeded generator. Cloning the [`Structure`] clones this, so a clone replays
    /// identically — the basis of the determinism guarantee.
    rng: Rng,
}

impl Structure {
    /// Create an empty structure (no ships) seeded with `seed`. Add sub-structures with
    /// [`Structure::add_sub`] and ships with [`Structure::spawn_ship`], or use
    /// [`crate::scenario::sample_structure`] for the ready-made sample.
    pub fn new(seed: u64) -> Structure {
        Structure { subs: Vec::new(), ships: Vec::new(), tick: 0, rng: Rng::new(seed) }
    }

    /// Add a sub-structure, returning its [`SubId`].
    pub fn add_sub(&mut self, sub: SubStructure) -> SubId {
        self.subs.push(sub);
        self.subs.len() - 1
    }

    /// Spawn an idle ship for `faction` garrisoned at `home`, placed at a jittered point
    /// inside the sub-structure's radius. Returns its [`ShipId`]. Used at setup and by
    /// production.
    pub fn spawn_ship(&mut self, faction: Faction, home: SubId) -> ShipId {
        let pos = self.jitter_in_sub(home);
        self.ships.push(Ship { faction, pos, target: None, home, aim: pos, alive: true });
        self.ships.len() - 1
    }

    /// A random point within `spread_radius` of the sub-structure centre, clamped to sit
    /// inside the sub-structure's own radius. Deterministic (draws from the embedded RNG).
    fn jitter_in_sub(&mut self, sub: SubId) -> Vec2 {
        let s = &self.subs[sub];
        let r = (self.rng.next_f32().sqrt()) * s.radius.min(self.params_spread());
        let theta = self.rng.next_f32() * std::f32::consts::TAU;
        Vec2::new(s.pos.x + r * theta.cos(), s.pos.y + r * theta.sin())
    }

    /// Spread radius used for spawn jitter. A free function would need params threaded in;
    /// production/spawn happen before a `step`, so we read the default operating spread.
    /// (Spawn jitter is cosmetic; the only sim-relevant fact is "inside the sub".)
    #[inline]
    fn params_spread(&self) -> f32 {
        SimParams::default().spread_radius
    }

    // ----------------------------------------------------------------------
    // Queries (the renderer + AI read these)
    // ----------------------------------------------------------------------

    /// Number of living ships of `faction` (idle + moving).
    pub fn ship_count(&self, faction: Faction) -> usize {
        self.ships.iter().filter(|s| s.alive && s.faction == faction).count()
    }

    /// Number of sub-structures owned by `faction`.
    pub fn sub_count(&self, faction: Faction) -> usize {
        self.subs.iter().filter(|s| s.owner == faction).count()
    }

    /// Living **idle** ships garrisoned at sub-structure `sub`, regardless of faction.
    pub fn idle_ships_at(&self, sub: SubId) -> impl Iterator<Item = ShipId> + '_ {
        self.ships
            .iter()
            .enumerate()
            .filter(move |(_, s)| s.is_idle() && s.home == sub)
            .map(|(i, _)| i)
    }

    /// Count of living idle ships of `faction` garrisoned at `sub`.
    pub fn idle_count_at(&self, sub: SubId, faction: Faction) -> usize {
        self.ships
            .iter()
            .filter(|s| s.is_idle() && s.home == sub && s.faction == faction)
            .count()
    }

    /// Count of living ships of `faction` physically inside `sub`'s radius (idle or not).
    /// This is what "presence" means for capture.
    pub fn presence_in_sub(&self, sub: SubId, faction: Faction) -> usize {
        let s = &self.subs[sub];
        let r2 = s.radius * s.radius;
        self.ships
            .iter()
            .filter(|sh| sh.alive && sh.faction == faction && sh.pos.dist_sq(s.pos) <= r2)
            .count()
    }

    /// Like [`Structure::presence_in_sub`] but counts only **idle** ships (`target == None`)
    /// of `faction` physically inside `sub`'s radius.
    ///
    /// The forward-projection (in the `world` crate) seeds its initial per-sub presence with
    /// this so it does not double-count a still-inside *moving* ship that is also a scheduled
    /// arrival — it uses the same authoritative radius metric as [`Structure::presence_in_sub`].
    pub fn idle_presence_in_sub(&self, sub: SubId, faction: Faction) -> usize {
        let s = &self.subs[sub];
        let r2 = s.radius * s.radius;
        self.ships
            .iter()
            .filter(|sh| {
                sh.is_idle() && sh.faction == faction && sh.pos.dist_sq(s.pos) <= r2
            })
            .count()
    }

    /// The erosion/heal driver for `sub` as a first-class read: living present counts of each
    /// real seat (by the [`Structure::presence_in_sub`] radius metric) plus the sub's owner.
    /// Out-of-range `sub` yields zeros and `Neutral`.
    pub fn sub_presence(&self, sub: SubId) -> SubPresence {
        if sub >= self.subs.len() {
            return SubPresence { player: 0, enemy: 0, owner: Faction::Neutral };
        }
        SubPresence {
            player: self.presence_in_sub(sub, Faction::Player) as u32,
            enemy: self.presence_in_sub(sub, Faction::Enemy) as u32,
            owner: self.subs[sub].owner,
        }
    }

    /// The **single present faction** at `sub` and its count, or `None` if zero or both real
    /// seats are present (the frozen case). This is exactly the discriminant
    /// [`SubStructure::capture_step`] keys off; surfaced so callers (and strategy helpers like
    /// "is this sub being eroded?") don't re-derive it from two presence calls.
    pub fn single_present_faction(&self, sub: SubId) -> Option<(Faction, u32)> {
        let p = self.sub_presence(sub);
        match (p.player > 0, p.enemy > 0) {
            (true, false) => Some((Faction::Player, p.player)),
            (false, true) => Some((Faction::Enemy, p.enemy)),
            _ => None,
        }
    }

    /// The `(current, max)` capture resistance of `sub`. A thin query over the
    /// [`SubStructure::resistance`] / [`SubStructure::max_resistance`] fields. Out-of-range
    /// `sub` yields `(0.0, 0.0)`.
    pub fn sub_resistance(&self, sub: SubId) -> (f32, f32) {
        match self.subs.get(sub) {
            Some(s) => (s.resistance, s.max_resistance),
            None => (0.0, 0.0),
        }
    }

    /// Sum of `resistance` over every sub **not** owned by `vs_owner` — the total grind a
    /// faction faces to fully own the structure. This is the quantity a resistance-proportional
    /// colonizer sizes its wave on (it includes neutral subs, whose owner is never `vs_owner`).
    pub fn total_foreign_resistance(&self, vs_owner: Faction) -> f32 {
        self.subs
            .iter()
            .filter(|s| s.owner != vs_owner)
            .map(|s| s.resistance)
            .sum()
    }

    /// **Parked** ship count for `faction` in this structure: living ships that are either idle
    /// or in **intra-structure** transit (i.e. all living ships of the faction in this
    /// `Structure`). This mirrors exactly what [`Structure::resolve_softcap`] attrites.
    /// Inter-planet fleets live in the `world` crate, not in a `Structure`, so they are not
    /// counted here (they are cap-exempt by construction).
    pub fn parked_count(&self, faction: Faction) -> u32 {
        self.ships
            .iter()
            .filter(|s| s.alive && s.faction == faction)
            .count() as u32
    }

    /// The soft cap for `faction` in this structure, expressed as the **sum of per-sub
    /// capacities** of the subs it owns plus the flat free allowance:
    /// `softcap_free + Σ_{owned sub} sub.soft_cap_capacity(params)`.
    ///
    /// With today's uniform [`SubStructure::soft_cap_capacity`] (`= softcap_per_sub` for every
    /// owned sub) this is numerically identical to the old `softcap_free + softcap_per_sub *
    /// owned_subs`, so [`Structure::resolve_softcap`] and every prior hash are unchanged. The
    /// reason for the sum form is **modularity**: a future sub type that stores more (a
    /// "warehouse") raises this faction's cap simply by returning a bigger capacity from its own
    /// `soft_cap_capacity`, with no change to the soft-cap math, the projection, or the AI.
    pub fn soft_cap(&self, faction: Faction, params: &SimParams) -> u32 {
        let mut cap = params.softcap_free;
        for s in &self.subs {
            if s.owner == faction {
                cap = cap.saturating_add(s.soft_cap_capacity(params));
            }
        }
        cap
    }

    /// True if `faction` has been eliminated: zero living ships **and** zero owned
    /// sub-structures (so it can neither fight now nor produce later).
    pub fn is_eliminated(&self, faction: Faction) -> bool {
        self.ship_count(faction) == 0 && self.sub_count(faction) == 0
    }

    // ----------------------------------------------------------------------
    // Orders (the AI and the GUI both call this)
    // ----------------------------------------------------------------------

    /// Issue a [`MoveOrder`]: retarget a fraction-bucket of `source`'s **idle** ships to
    /// `target`. Returns how many ships were actually ordered.
    ///
    /// The order is the Layer-1 atomic action. It is robust to junk (the future GUI/AI may
    /// emit anything): it is a silent no-op when `source == target`, when `source` has no
    /// idle ships, or when either id is out of range. Only *idle* ships move — ships already
    /// in transit are not redirected, matching the "commit then it's flying" feel.
    ///
    /// Which specific idle ships are chosen is deterministic (lowest [`ShipId`] first), so
    /// a given order on a given state always produces the same result.
    pub fn issue_order(&mut self, order: MoveOrder) -> usize {
        let MoveOrder { source, target, fraction } = order;
        if source == target || source >= self.subs.len() || target >= self.subs.len() {
            return 0;
        }
        let idle: Vec<ShipId> = self.idle_ships_at(source).collect();
        let n = fraction.count_of(idle.len());
        if n == 0 {
            return 0;
        }
        // Pre-compute jittered aim points inside the target (deterministic via RNG).
        for &sid in idle.iter().take(n) {
            let aim = self.spread_point(target);
            let sh = &mut self.ships[sid];
            sh.target = Some(target);
            sh.aim = aim;
        }
        n
    }

    /// A spread aim point: a random offset within `spread_radius` of a random point inside
    /// the target sub-structure, so a wave of ships fans across the destination rather than
    /// converging on one coordinate.
    fn spread_point(&mut self, sub: SubId) -> Vec2 {
        let s = &self.subs[sub];
        let sp = SimParams::default().spread_radius; // spread is cosmetic; default is fine
        // Random point in a disk of radius min(sub.radius, spread) around the centre.
        let max_r = s.radius.min(sp).max(0.01);
        let r = self.rng.next_f32().sqrt() * max_r;
        let theta = self.rng.next_f32() * std::f32::consts::TAU;
        Vec2::new(s.pos.x + r * theta.cos(), s.pos.y + r * theta.sin())
    }

    // ----------------------------------------------------------------------
    // Idle-ship EXTRACTION (Layer-2 inter-planet export)
    // ----------------------------------------------------------------------
    //
    // These helpers *remove* idle ships from this structure entirely (they are marked
    // dead, so they vanish from this Structure's accounting) and report how many were
    // taken. They exist so a higher layer — the `world` crate — can lift a planet's idle
    // garrison off one Layer-1 `Structure`, carry it across an inter-planet lane as a
    // fleet, and inject it into the destination `Structure` via `spawn_ship`. From this
    // structure's point of view an extracted ship is simply gone (same as if it had been
    // destroyed); from the world's point of view it is conserved (re-spawned on arrival).
    // They draw no randomness, so they never perturb the RNG stream — extracting ships
    // does not change subsequent combat rolls, preserving bit-reproducibility.

    /// Remove up to `n` **idle** ships of `faction` garrisoned at `sub`, marking them dead,
    /// and return how many were actually removed.
    ///
    /// Only living, idle (`target == None`) ships whose `home == sub` and whose faction
    /// matches are eligible — ships in transit are never yanked (consistent with
    /// [`Structure::issue_order`], which also only moves idle ships). Selection is
    /// deterministic (lowest [`ShipId`] first), so a given call on a given state always
    /// removes the same ships. Out-of-range `sub` or `n == 0` removes nothing. This draws
    /// no randomness, so it leaves the RNG stream untouched.
    ///
    /// Intended for the Layer-2 lens: the `world` crate calls this to detach a fleet's
    /// ships from a source planet, then re-spawns the same count at the destination on
    /// arrival (conserving ships across the world even though each Layer-1 `Structure`
    /// only ever marks them dead).
    pub fn take_idle_ships(&mut self, sub: SubId, faction: Faction, n: usize) -> usize {
        if n == 0 || sub >= self.subs.len() {
            return 0;
        }
        let mut taken = 0;
        for sh in self.ships.iter_mut() {
            if taken >= n {
                break;
            }
            if sh.alive && sh.target.is_none() && sh.home == sub && sh.faction == faction {
                sh.alive = false;
                taken += 1;
            }
        }
        taken
    }

    /// Planet-wide export: remove a [`FractionBucket`] of `faction`'s total **idle** ships,
    /// drawn from the sub-structures `faction` owns, while leaving at least `keep_floor`
    /// idle ships at each source sub. Returns how many were actually removed.
    ///
    /// The target count is `fraction.count_of(total_idle_of_faction)` — the bucket applied
    /// to *all* of the faction's idle ships across the whole structure. Ships are then pulled
    /// sub-by-sub in ascending [`SubId`] order, but no sub is ever taken below `keep_floor`
    /// idle ships (a small garrison the planet keeps to defend/seed itself). If the floor
    /// binds on every sub, fewer than the target — possibly zero — are taken; the return value
    /// is always the true count removed. Only subs **owned by `faction`** are drawn from
    /// (idle ships of `faction` sitting on a sub it does not own are left in place — they are
    /// garrisoning captured ground, not surplus to export).
    ///
    /// Deterministic and RNG-free, exactly like [`Structure::take_idle_ships`]. This is the
    /// primitive a [`crate::types::FractionBucket`] inter-planet "launch a fleet" order uses
    /// at the world level.
    pub fn take_idle_ships_planetwide(
        &mut self,
        faction: Faction,
        fraction: FractionBucket,
        keep_floor: usize,
    ) -> usize {
        // Total idle ships of this faction across the whole structure.
        let total_idle = self
            .ships
            .iter()
            .filter(|s| s.is_idle() && s.faction == faction)
            .count();
        let mut want = fraction.count_of(total_idle);
        if want == 0 {
            return 0;
        }
        let mut taken = 0;
        // Draw sub-by-sub in ascending SubId order for determinism, honouring the per-sub
        // keep-floor. Only subs this faction owns are eligible export sources.
        for sub in 0..self.subs.len() {
            if want == 0 {
                break;
            }
            if self.subs[sub].owner != faction {
                continue;
            }
            let idle_here = self.idle_count_at(sub, faction);
            if idle_here <= keep_floor {
                continue;
            }
            let exportable_here = (idle_here - keep_floor).min(want);
            let got = self.take_idle_ships(sub, faction, exportable_here);
            taken += got;
            want -= got;
        }
        taken
    }

    // ----------------------------------------------------------------------
    // The tick loop
    // ----------------------------------------------------------------------

    /// Advance the simulation by exactly one tick, in this **fixed** order (for determinism):
    ///   1. **production** — owned sub-structures spawn ships on their cadence, *gated by denial*
    ///      (a sub being eroded by an uncontested foe does not produce; see [`Structure::produce`]),
    ///   2. **movement** — moving ships advance toward their aim; arrivals become idle,
    ///   3. **combat** — `combat_substeps` rounds of stochastic square-law fire,
    ///   4. **resistance** — capture grind / heal / flip ([`Structure::resolve_resistance`]),
    ///   5. **soft-cap** — anti-hoard attrition ([`Structure::resolve_softcap`]).
    ///
    /// Two ordering facts the design relies on: **combat resolves before resistance** (a
    /// defender must survive the firefight to count as present for the heal; an attacker erodes
    /// with its post-combat count), and **resistance uses post-movement presence** (a ship that
    /// arrives this tick is inside the radius when the grind runs, so it counts on its arrival
    /// tick). All randomness is drawn from the embedded RNG (combat fire + soft-cap destruction),
    /// so two `Structure`s with the same seed and the same orders evolve identically.
    pub fn step(&mut self, params: &SimParams) {
        self.produce(params);
        self.advance_movement(params);
        self.resolve_combat(params);
        self.resolve_resistance();
        self.resolve_softcap(params);
        self.tick += 1;
    }

    /// (1) Production: each owned sub-structure counts down and spawns one idle ship for its
    /// owner when the timer hits zero, then resets. Neutral sub-structures are skipped and
    /// held at the period.
    ///
    /// **Denial gate (Mechanic B).** A sub that is being *actively eroded* — exactly one foreign
    /// faction present and the owner absent (start-of-tick presence, since `produce` runs first)
    /// — does **not** produce, and its `production_timer` is **held steady**. Parking on an
    /// enemy sub starves its output even before capture. A contested-but-defended sub (owner
    /// *and* foe present) keeps producing — defenders keep the line running.
    fn produce(&mut self, params: &SimParams) {
        let n = self.subs.len();
        for sub in 0..n {
            let owner = self.subs[sub].owner;
            if !owner.is_real() {
                self.subs[sub].production_timer = params.production_period;
                continue;
            }
            // Denial: one uncontested foreign faction present and the owner absent => the sub
            // is being eroded; freeze its output and hold the timer (no catch-up on relief).
            let owner_here = self.presence_in_sub(sub, owner) > 0;
            let foe_here = self.presence_in_sub(sub, owner.opponent()) > 0;
            if foe_here && !owner_here {
                continue; // production denied; timer untouched (held steady)
            }
            if self.subs[sub].production_timer == 0 {
                // Respect the per-sub safety cap on lifetime spawns.
                let already = self.ships.iter().filter(|s| s.home == sub).count() as u32;
                if already < params.max_ships_per_sub {
                    self.spawn_ship(owner, sub);
                }
                self.subs[sub].production_timer = params.production_period;
            } else {
                self.subs[sub].production_timer -= 1;
            }
        }
    }

    /// (2) Movement: advance each moving ship straight toward its `aim` at `ship_speed`. On
    /// reaching the aim (within `arrival_tolerance`) the ship becomes idle and adopts the
    /// target as its new `home` (it is now garrisoning there).
    fn advance_movement(&mut self, params: &SimParams) {
        for sh in &mut self.ships {
            if !sh.alive {
                continue;
            }
            let Some(target) = sh.target else { continue };
            let to = sh.aim;
            let d = sh.pos.dist(to);
            if d <= params.arrival_tolerance.max(1e-4) {
                sh.pos = to;
                sh.home = target;
                sh.target = None;
                continue;
            }
            let stepd = params.ship_speed.min(d);
            let ux = (to.x - sh.pos.x) / d;
            let uy = (to.y - sh.pos.y) / d;
            sh.pos.x += ux * stepd;
            sh.pos.y += uy * stepd;
            // Snap-arrive if this step lands us within tolerance, to avoid jitter.
            if sh.pos.dist(to) <= params.arrival_tolerance.max(1e-4) {
                sh.pos = to;
                sh.home = target;
                sh.target = None;
            }
        }
    }

    /// (3) Combat: `combat_substeps` rounds of stochastic square-law fire over the current
    /// proximity graph.
    ///
    /// Each sub-step:
    ///   * recompute, for every living ship, the list of living enemy ships within `R`
    ///     (cheap O(N^2); N is small at Layer 1),
    ///   * every ship with >= 1 enemy in range is *engaged* and fires with probability
    ///     `fire_prob` (+`defender_fire_bonus` if it sits inside one of its own subs),
    ///   * **fire is simultaneous within the sub-step**: we collect all shots against the
    ///     pre-substep liveness, then apply kills, so neither side gets to react first
    ///     inside the sub-step (removing seat bias). A ship already killed earlier in the
    ///     same sub-step cannot be "killed again" — each kill picks a *currently* living
    ///     target at random, and a shot whose chosen target is already dead is wasted,
    ///     which keeps the kill rate honest.
    ///
    /// The square law emerges because each side fields shooters proportional to its engaged
    /// count, so the opponent's expected losses are proportional to your engaged count.
    fn resolve_combat(&mut self, params: &SimParams) {
        let substeps = params.combat_substeps.max(1);
        let r2 = params.engagement_radius * params.engagement_radius;
        for _ in 0..substeps {
            let n = self.ships.len();
            // Snapshot positions/liveness/faction for this sub-step (immutable view).
            // Collect shooters and let each pick one in-range enemy target to kill.
            // We gather (target_id) kill requests, then apply.
            let mut kills: Vec<ShipId> = Vec::new();
            for i in 0..n {
                let sh = &self.ships[i];
                if !sh.alive {
                    continue;
                }
                // Gather living enemies in range.
                // (Recomputed per sub-step against current liveness.)
                let mut in_range: Vec<ShipId> = Vec::new();
                for j in 0..n {
                    if i == j {
                        continue;
                    }
                    let other = &self.ships[j];
                    if other.alive
                        && other.faction != sh.faction
                        && other.faction.is_real()
                        && sh.faction.is_real()
                        && sh.pos.dist_sq(other.pos) <= r2
                    {
                        in_range.push(j);
                    }
                }
                if in_range.is_empty() {
                    continue; // not engaged
                }
                // Engaged: fire with probability p (+defender bonus if inside own sub).
                let mut p = params.fire_prob;
                if params.defender_fire_bonus != 0.0 && self.ship_in_own_sub(i) {
                    p += params.defender_fire_bonus;
                }
                if self.rng.chance(p) {
                    // One-shot a uniformly random in-range enemy.
                    let pick = self.rng.below(in_range.len());
                    kills.push(in_range[pick]);
                }
            }
            // Apply kills. A target already downed this sub-step => the shot is wasted.
            for t in kills {
                self.ships[t].alive = false;
            }
        }
    }

    /// True if ship `i` is alive and currently within the radius of any sub-structure its
    /// own faction owns (the condition for the defender fire bonus).
    fn ship_in_own_sub(&self, i: ShipId) -> bool {
        let sh = &self.ships[i];
        if !sh.alive {
            return false;
        }
        self.subs.iter().any(|s| {
            s.owner == sh.faction && sh.pos.dist_sq(s.pos) <= s.radius * s.radius
        })
    }

    /// (4) Resistance: the capture **grind / heal / flip** (Mechanic A), applied per sub via the
    /// pure [`SubStructure::capture_step`] (the same function the forward-projection calls, so
    /// the two can never drift).
    ///
    /// Using post-combat, post-movement presence: an uncontested foreign faction erodes the
    /// `resistance` bar by its present count; the owner present and uncontested heals it; both
    /// present (or none) freezes it. On the bar hitting `<= 0` the sub **flips** to the eroding
    /// faction and **refills** to `max_resistance`. Ownership is the only thing that changes —
    /// garrisoned ships keep their `home`, so a freshly captured sub starts producing for the
    /// new owner next tick (subject to the denial gate). On a flip we nudge the production timer
    /// to `>= 1` so a just-seized sub does not pop a ship the very next tick.
    fn resolve_resistance(&mut self) {
        let n = self.subs.len();
        for sub in 0..n {
            let present_player = self.presence_in_sub(sub, Faction::Player) as u32;
            let present_enemy = self.presence_in_sub(sub, Faction::Enemy) as u32;
            let s = &self.subs[sub];
            let (new_owner, new_res, flipped) = SubStructure::capture_step(
                s.owner,
                s.resistance,
                s.max_resistance,
                present_player,
                present_enemy,
            );
            let s = &mut self.subs[sub];
            s.owner = new_owner;
            s.resistance = new_res;
            if flipped {
                s.production_timer = s.production_timer.max(1);
            }
        }
    }

    /// (5) Soft cap (Mechanic C): anti-hoard attrition. For each real seat, with
    /// `parked = ` living ships of the seat in this structure (idle or intra-structure transit;
    /// inter-planet fleets are not in a `Structure`, so they are exempt) and
    /// `soft = softcap_free + softcap_per_sub * owned_subs`:
    ///
    /// ```text
    /// over      = parked - soft                              (only if parked > soft)
    /// soft_kill = ceil(softcap_attrition * sqrt(over))
    /// hard_kill = parked.saturating_sub(structure_hard_cap)  (far-above-play safety only)
    /// n         = max(soft_kill, hard_kill).min(parked)
    /// destroy n parked ships at random (idle preferred over in-transit) via the structure RNG
    /// ```
    ///
    /// The `sqrt` shape makes the cap a self-limiting **plateau**, not a wall: the count settles
    /// just above `soft`. There is intentionally **no** hard strategic ceiling — `structure_hard_cap`
    /// is only a pathology guard. Surplus must be spent or kept moving (inter-planet transit is
    /// the cap-exempt escape valve).
    ///
    /// Determinism: the random victims are drawn from the structure's seeded RNG, and the draw
    /// position is folded into [`Structure::state_hash`]. To keep the RNG stream stable when no
    /// attrition happens, **no RNG is drawn unless at least one ship must die.**
    fn resolve_softcap(&mut self, params: &SimParams) {
        for &faction in &[Faction::Player, Faction::Enemy] {
            // Living ships of this faction in this structure, partitioned idle-first so we can
            // prefer destroying idle ships over in-transit ones.
            let mut idle: Vec<ShipId> = Vec::new();
            let mut moving: Vec<ShipId> = Vec::new();
            for (i, sh) in self.ships.iter().enumerate() {
                if sh.alive && sh.faction == faction {
                    if sh.target.is_none() {
                        idle.push(i);
                    } else {
                        moving.push(i);
                    }
                }
            }
            let parked = (idle.len() + moving.len()) as u32;
            let soft = self.soft_cap(faction, params);
            if parked <= soft {
                continue;
            }
            let over = parked - soft;
            let soft_kill = (params.softcap_attrition.max(0.0) * (over as f32).sqrt()).ceil() as u32;
            let hard_kill = parked.saturating_sub(params.structure_hard_cap);
            let n = soft_kill.max(hard_kill).min(parked);
            if n == 0 {
                continue;
            }
            // Build the victim pool idle-first, then in-transit, and destroy the first `n` by a
            // deterministic RNG shuffle within each tier (idle tier consumed before moving tier).
            // Drawing only when n > 0 keeps the RNG stream untouched on the common no-attrition
            // path, preserving prior hashes for unchanged behaviour.
            let mut remaining = n as usize;
            for tier in [idle, moving] {
                if remaining == 0 {
                    break;
                }
                let mut pool = tier;
                // Partial Fisher–Yates: pick `take` distinct victims uniformly from `pool`.
                let take = remaining.min(pool.len());
                for k in 0..take {
                    let j = k + self.rng.below(pool.len() - k);
                    pool.swap(k, j);
                    self.ships[pool[k]].alive = false;
                }
                remaining -= take;
            }
        }
    }

    // ----------------------------------------------------------------------
    // Battle bubbles (for the renderer)
    // ----------------------------------------------------------------------

    /// Compute the current set of [`BattleBubble`]s: connected clusters of mutually-in-range
    /// **opposing** ships. Two engaged ships are in the same bubble if a chain of
    /// within-`R` ship pairs connects them and the cluster contains both factions.
    ///
    /// This is a read-only view for drawing; it does not mutate the sim. Cost is O(N^2)
    /// over living ships (N is small at Layer 1). A cluster with only one faction present
    /// is *not* a bubble (nobody is fighting), so it is omitted.
    pub fn battle_bubbles(&self, params: &SimParams) -> Vec<BattleBubble> {
        let r2 = params.engagement_radius * params.engagement_radius;
        let live: Vec<ShipId> =
            (0..self.ships.len()).filter(|&i| self.ships[i].alive).collect();

        // Union-find over living ship indices, unioning any two *opposing* ships in range
        // (an engagement edge). Same-faction ships are joined transitively only through a
        // shared enemy, which is exactly the "connected cluster of mutually-in-range
        // opposing ships" we want to draw.
        let mut parent: Vec<usize> = (0..self.ships.len()).collect();
        fn find(parent: &mut [usize], mut x: usize) -> usize {
            while parent[x] != x {
                parent[x] = parent[parent[x]];
                x = parent[x];
            }
            x
        }
        for (a_idx, &i) in live.iter().enumerate() {
            for &j in live.iter().skip(a_idx + 1) {
                let si = &self.ships[i];
                let sj = &self.ships[j];
                if si.faction != sj.faction && si.pos.dist_sq(sj.pos) <= r2 {
                    let ri = find(&mut parent, i);
                    let rj = find(&mut parent, j);
                    if ri != rj {
                        parent[ri] = rj;
                    }
                }
            }
        }

        // Group living ships by root, but only those that actually have an engagement edge
        // (i.e. their component contains both factions). We detect that by tracking, per
        // root, whether each faction appeared and the list of members.
        use std::collections::HashMap;
        struct Acc {
            ships: Vec<ShipId>,
            has_player: bool,
            has_enemy: bool,
        }
        let mut groups: HashMap<usize, Acc> = HashMap::new();
        for &i in &live {
            let root = find(&mut parent, i);
            let e = groups.entry(root).or_insert(Acc {
                ships: Vec::new(),
                has_player: false,
                has_enemy: false,
            });
            e.ships.push(i);
            match self.ships[i].faction {
                Faction::Player => e.has_player = true,
                Faction::Enemy => e.has_enemy = true,
                Faction::Neutral => {}
            }
        }

        let mut bubbles: Vec<BattleBubble> = Vec::new();
        for acc in groups.into_values() {
            // A real bubble must contain both factions (a fight is happening).
            if !(acc.has_player && acc.has_enemy) {
                continue;
            }
            let mut cx = 0.0f32;
            let mut cy = 0.0f32;
            let mut player_count = 0usize;
            let mut enemy_count = 0usize;
            for &s in &acc.ships {
                cx += self.ships[s].pos.x;
                cy += self.ships[s].pos.y;
                match self.ships[s].faction {
                    Faction::Player => player_count += 1,
                    Faction::Enemy => enemy_count += 1,
                    Faction::Neutral => {}
                }
            }
            let cnt = acc.ships.len() as f32;
            let center = Vec2::new(cx / cnt, cy / cnt);
            let mut radius = 0.0f32;
            for &s in &acc.ships {
                radius = radius.max(self.ships[s].pos.dist(center));
            }
            let mut ships = acc.ships;
            ships.sort_unstable(); // deterministic order for the renderer/tests
            bubbles.push(BattleBubble { ships, center, radius, player_count, enemy_count });
        }
        // Deterministic ordering of bubbles (by lowest member id).
        bubbles.sort_by_key(|b| *b.ships.first().unwrap_or(&0));
        bubbles
    }

    /// Number of active battle bubbles (convenience for the headless summary).
    pub fn bubble_count(&self, params: &SimParams) -> usize {
        self.battle_bubbles(params).len()
    }

    // ----------------------------------------------------------------------
    // Outcome
    // ----------------------------------------------------------------------

    /// The outcome **as of now**: if exactly one real faction is eliminated, the other
    /// wins by elimination; otherwise the winner is whoever leads on `ships + sub_count`
    /// (an exact tie => `None`). Mirrors `cell-core`'s `MatchOutcome` spirit.
    pub fn outcome(&self) -> Outcome {
        let p_ships = self.ship_count(Faction::Player);
        let e_ships = self.ship_count(Faction::Enemy);
        let p_subs = self.sub_count(Faction::Player);
        let e_subs = self.sub_count(Faction::Enemy);
        let p_dead = self.is_eliminated(Faction::Player);
        let e_dead = self.is_eliminated(Faction::Enemy);

        let (winner, by_elim) = if p_dead && !e_dead {
            (Some(Faction::Enemy), true)
        } else if e_dead && !p_dead {
            (Some(Faction::Player), true)
        } else {
            // Lead at horizon by combined ships + sub-structures.
            let p_score = p_ships + p_subs;
            let e_score = e_ships + e_subs;
            let w = if p_score > e_score {
                Some(Faction::Player)
            } else if e_score > p_score {
                Some(Faction::Enemy)
            } else {
                None
            };
            (w, false)
        };
        Outcome {
            winner,
            by_elimination: by_elim,
            tick: self.tick,
            ships: (p_ships, e_ships),
            subs: (p_subs, e_subs),
        }
    }

    /// A 64-bit fingerprint of the *entire* simulation state (every sub-structure, every
    /// ship, the tick, and the RNG stream position). Two runs with the same seed and orders
    /// produce identical hashes at every tick — the determinism test asserts on this.
    ///
    /// Implemented as an inline FNV-1a over the state's bytes; floats are hashed by their
    /// bit pattern so the comparison is exact.
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
        mix_u64(&mut h, self.subs.len() as u64);
        for s in &self.subs {
            mix_f32(&mut h, s.pos.x);
            mix_f32(&mut h, s.pos.y);
            mix_f32(&mut h, s.radius);
            mix(&mut h, faction_byte(s.owner));
            mix_u64(&mut h, s.production_timer as u64);
            // Capture state is part of the fingerprint so a divergent grind is detected.
            mix_f32(&mut h, s.resistance);
            mix_f32(&mut h, s.max_resistance);
        }
        mix_u64(&mut h, self.ships.len() as u64);
        for sh in &self.ships {
            mix(&mut h, faction_byte(sh.faction));
            mix_f32(&mut h, sh.pos.x);
            mix_f32(&mut h, sh.pos.y);
            mix(&mut h, if sh.alive { 1 } else { 0 });
            mix_u64(&mut h, sh.home as u64);
            mix_u64(&mut h, sh.target.map(|t| t as u64 + 1).unwrap_or(0));
            mix_f32(&mut h, sh.aim.x);
            mix_f32(&mut h, sh.aim.y);
        }
        // Fold in the RNG's current position so divergent random draws are detected even if
        // they have not yet changed any visible field.
        mix_u64(&mut h, self.rng.clone().next_u64());
        h
    }

    /// Drop dead ships, compacting the `ships` Vec. **Invalidates existing [`ShipId`]s**, so
    /// only call between frames if the renderer does not cache ids across the call. The sim
    /// itself never needs this (dead ships are skipped); it is offered for hosts that want
    /// to bound memory over very long runs.
    pub fn compact_dead(&mut self) {
        self.ships.retain(|s| s.alive);
    }
}

#[inline]
fn faction_byte(f: Faction) -> u8 {
    match f {
        Faction::Player => 1,
        Faction::Enemy => 2,
        Faction::Neutral => 0,
    }
}

#[cfg(test)]
mod take_idle_tests {
    //! Unit tests for the Layer-2 inter-planet export helpers
    //! ([`Structure::take_idle_ships`] / [`Structure::take_idle_ships_planetwide`]).
    //!
    //! These live in the library crate (not the `tests/` integration target) so they run as
    //! part of the `layer1` lib test harness.
    use super::*;

    /// Two owned subs for `faction`, far apart so nothing fights, with the requested idle
    /// garrisons. Returns the structure and the two SubIds.
    fn two_sub_struct(seed: u64, faction: Faction, n0: usize, n1: usize) -> (Structure, SubId, SubId) {
        let mut st = Structure::new(seed);
        let a = st.add_sub(SubStructure::new(Vec2::new(0.0, 0.0), 4.0, faction));
        let b = st.add_sub(SubStructure::new(Vec2::new(1000.0, 0.0), 4.0, faction));
        for _ in 0..n0 {
            st.spawn_ship(faction, a);
        }
        for _ in 0..n1 {
            st.spawn_ship(faction, b);
        }
        (st, a, b)
    }

    #[test]
    fn take_idle_removes_exactly_n_of_faction() {
        let (mut st, a, _b) = two_sub_struct(1, Faction::Player, 5, 0);
        let took = st.take_idle_ships(a, Faction::Player, 3);
        assert_eq!(took, 3);
        assert_eq!(st.idle_count_at(a, Faction::Player), 2);
        assert_eq!(st.ship_count(Faction::Player), 2, "taken ships are removed from the count");
    }

    #[test]
    fn take_idle_caps_at_available() {
        let (mut st, a, _b) = two_sub_struct(2, Faction::Player, 2, 0);
        // Asking for more than present removes only what is there.
        let took = st.take_idle_ships(a, Faction::Player, 10);
        assert_eq!(took, 2);
        assert_eq!(st.idle_count_at(a, Faction::Player), 0);
    }

    #[test]
    fn take_idle_ignores_moving_ships() {
        let params = SimParams::default();
        let (mut st, a, b) = two_sub_struct(3, Faction::Player, 4, 0);
        // Send 2 of a's ships toward b (now in transit, not idle).
        let moved = st.issue_order(MoveOrder::new(a, b, FractionBucket::Half));
        assert_eq!(moved, 2);
        // Only the 2 still-idle ships at a are eligible.
        let took = st.take_idle_ships(a, Faction::Player, 10);
        assert_eq!(took, 2, "in-transit ships must not be extracted");
        // The two moving ships still exist (they later arrive at b).
        for _ in 0..60 {
            st.step(&params);
        }
        assert!(st.ship_count(Faction::Player) >= 2);
    }

    #[test]
    fn take_idle_wrong_faction_or_oob_is_noop() {
        let (mut st, a, _b) = two_sub_struct(4, Faction::Player, 3, 0);
        assert_eq!(st.take_idle_ships(a, Faction::Enemy, 2), 0, "no enemy ships to take");
        assert_eq!(st.take_idle_ships(999, Faction::Player, 2), 0, "out-of-range sub is a no-op");
        assert_eq!(st.take_idle_ships(a, Faction::Player, 0), 0, "n=0 is a no-op");
        assert_eq!(st.idle_count_at(a, Faction::Player), 3);
    }

    #[test]
    fn take_idle_does_not_perturb_rng() {
        // Extraction must draw no randomness: the state_hash folds the RNG position, so a
        // structure that had ships extracted and then re-added back must leave the RNG where
        // it started (i.e. extraction itself advanced nothing).
        let (mut st, a, _b) = two_sub_struct(5, Faction::Player, 4, 0);
        let rng_before = st.rng.clone().next_u64();
        let _ = st.take_idle_ships(a, Faction::Player, 2);
        let rng_after = st.rng.clone().next_u64();
        assert_eq!(rng_before, rng_after, "extraction must not advance the RNG");
    }

    #[test]
    fn planetwide_respects_keep_floor() {
        // 10 idle on sub a, 0 on b. Half of 10 = 5 wanted. With keep_floor 3, a can export
        // at most 10-3 = 7, so all 5 are taken and 5 remain.
        let (mut st, a, _b) = two_sub_struct(6, Faction::Player, 10, 0);
        let took = st.take_idle_ships_planetwide(Faction::Player, FractionBucket::Half, 3);
        assert_eq!(took, 5);
        assert_eq!(st.idle_count_at(a, Faction::Player), 5);
    }

    #[test]
    fn planetwide_floor_can_bind_and_reduce_export() {
        // 4 idle on a, 4 on b => total 8, All => want 8. keep_floor 3 => each sub exports at
        // most 1, so only 2 are taken (1 from each), floor binds hard.
        let (mut st, a, b) = two_sub_struct(7, Faction::Player, 4, 4);
        let took = st.take_idle_ships_planetwide(Faction::Player, FractionBucket::All, 3);
        assert_eq!(took, 2, "keep-floor on every sub caps the export");
        assert_eq!(st.idle_count_at(a, Faction::Player), 3);
        assert_eq!(st.idle_count_at(b, Faction::Player), 3);
    }

    #[test]
    fn planetwide_only_pulls_from_owned_subs() {
        // a is Player-owned with 5 idle; b is Neutral but happens to have 5 idle Player ships
        // garrisoned on it (e.g. just arrived, pre-capture). Planet-wide export for Player
        // must only draw from the owned sub a.
        let mut st = Structure::new(8);
        let a = st.add_sub(SubStructure::new(Vec2::new(0.0, 0.0), 4.0, Faction::Player));
        let b = st.add_sub(SubStructure::new(Vec2::new(1000.0, 0.0), 4.0, Faction::Neutral));
        for _ in 0..5 {
            st.spawn_ship(Faction::Player, a);
        }
        for _ in 0..5 {
            st.spawn_ship(Faction::Player, b);
        }
        // total idle player = 10, All => want 10, but only owned sub a (5) is eligible,
        // keep_floor 0 => take all 5 from a, none from neutral b.
        let took = st.take_idle_ships_planetwide(Faction::Player, FractionBucket::All, 0);
        assert_eq!(took, 5);
        assert_eq!(st.idle_count_at(a, Faction::Player), 0);
        assert_eq!(st.idle_count_at(b, Faction::Player), 5, "idle ships on an unowned sub are not exported");
    }
}
