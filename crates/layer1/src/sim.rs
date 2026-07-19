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
//! * A [`Interior`] is **one** structure = several [`SubStructure`]s at 2D positions, plus
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
//! seeded [`crate::rng::Rng`] threaded through [`Interior::step`].

use crate::rng::Rng;
use crate::types::*;

/// Coupling between a sub's **storage capacity** and its **capture resistance** — the master
/// **grind dial**: a fresh sub (no explicit `with_max_resistance`) starts with
/// `resistance = storage_capacity · this`, so a bigger sub is proportionally harder to take.
/// At the default capacity `60` this is `3600`: clearing a fresh default sub with `F` present,
/// uncontested attackers takes `ceil(3600 / F)` ticks (~200 production periods at the default
/// `production_period = 18`). Per-sub overridable via [`SubStructure::with_max_resistance`].
pub const RESISTANCE_PER_CAPACITY: f32 = 60.0;

/// The fixed combat **engagement radius** (metres) — the operating point both [`SimParams::default`]
/// and the game's `gui_params` use. Promoted to a named constant because the game treats it as a
/// fixed constant (it does **not** scale with a sub's storage-derived size), and the reserve-node
/// sizing ([`Interior::add_storage_sub`]) reads it so the reserve garrison always sits clear of the
/// inner sub garrisons (they never auto-fight across the reserve boundary until ships actually move).
/// **3.5** (halved from the original 7.0 — smaller kill zones make attacking less punishing; the
/// orbit model's nearest-foe drive keeps co-garrisoned enemies from orbiting forever out of the
/// shorter range).
pub const DEFAULT_ENGAGEMENT_RADIUS: f32 = 3.5;

/// Default [`SubStructure::storage_capacity`] — idle ships a sub holds with no attrition. With the
/// default `storage_per_production = 60` and one ship / `production_period`, a sub settles at an
/// effective cap of `60 + 60 = 120` under the per-sub attrition model.
pub const DEFAULT_STORAGE_CAPACITY: u32 = 60;

/// Default [`SubStructure::ring_frac`] — idle ships orbit at this fraction of the sub radius.
pub const DEFAULT_RING_FRAC: f32 = 0.75;

/// Max magnitude of a ship's random radial ring offset, as a fraction of the sub radius (so each
/// ship sits at `(ring_frac ± up to RING_OFFSET) · radius`). See [`Ship::ring_offset`].
pub const RING_OFFSET: f32 = 0.1;

/// Speed cap on the ring-band churn's radial drift, as a fraction of `ship_speed` (owner rule:
/// idle radial motion must never outpace real inter-sub flight, however big the ring). Also
/// capped at a tenth of the band per tick so small subs stay smooth.
pub const RADIAL_DRIFT_SPEED_FRAC: f32 = 0.5;

/// **Pressure kernel width `w`** (world units of arc) of the orbit model v4 crowd engine
/// (owner spec, 2026-07-08): every idle ship repels every OTHER idle ship on its ring —
/// **faction-blind** — with a hat-kernel push `(1 − arc/w)` that dies at this spacing. The
/// universal excluded-volume pressure is the whole peacetime disperser AND what spaces a
/// melee: with no cross-faction standoff anywhere, opposing ships are miscible — they
/// interleave at ~this spacing instead of forming fronts (the owner's explicit aesthetic:
/// battles mix; v3's leash-made fronts are gone).
pub const ORBIT_PRESSURE_SPACING: f32 = 1.0;

/// **Crowd stiffness `K`** of the v4 pressure term: how many net uncancelled hat-kernel
/// contacts on one side amount to a full-flight-speed shove (`urge = fs·Σ/K`, clamped by the
/// speed law). Higher = softer crowds that compress more before pushing back.
pub const ORBIT_CROWD_STIFFNESS: f32 = 10.0;

/// **Cohesion window** (world units of arc, per side) of the v4 same-faction cohesion term
/// (owner refinement, 2026-07-08 — "a tournament of elimination, instead of a single place
/// absorbing everything"): a ship WITH staged foes is additionally urged toward whichever
/// side holds more of its own faction within this arc (normalized count imbalance × flight
/// speed). This is the surface tension the pure pressure+drive model lacked: without it the
/// post-melee state is alternating faction stripes that parade around the ring (nearest-foe
/// pursuit's neutrally-stable translation mode); with it the stripe pattern COARSENS like 1D
/// phase separation — small same-side groups evaporate toward their side's bigger masses,
/// the enemy stripe caught between two merging ones is ground from both edges, and domains
/// grow pairwise until two pockets clash. **Wartime only** — gated on the ship having staged
/// foes: an always-on attraction would collapse the peacetime uniform ring into a ball.
pub const ORBIT_COHESION_SPAN: f32 = 12.0;

/// Cohesion strength, as a fraction of flight speed — **strictly below 1** by design: at a
/// pocket's leading edge the whole side sits behind (imbalance ≈ 1), so cohesion at full
/// flight speed exactly cancels the drive and two clouds NEVER close (observed: an arena
/// where nobody had died by t2400). At 0.5 the front advances at ≥ half speed while the
/// rear keeps up — cohesive advance — and in the striped stalemate (where the drive largely
/// self-cancels between the two boundaries) cohesion still dominates and coarsens.
pub const ORBIT_COHESION_STRENGTH: f32 = 0.5;

/// Settled-ring epsilon, as a fraction of flight speed: a kernel pass whose largest urge
/// stays below `eps × ship_speed`, with no staged foes, marks the ring SETTLED — from then
/// on the kernel runs at the 1/[`ORBIT_SETTLED_DUTY`] duty cycle and the other ticks are
/// pure spin (the orbit fast path, owner ask 2026-07-10). The epsilon is the boundary
/// between the motions the duty cycle OWNS (single soft-cap-bleed gaps on a big parade
/// measure ~0.05–0.1 v — they heal at duty rate, so a perpetually-bleeding reserve ring
/// still sleeps) and real disturbances (a staging foe wakes the ring instantly; any urge
/// above eps — mass arrivals, real clumps — restores the every-tick kernel).
pub const ORBIT_SETTLE_EPS: f32 = 0.2;

/// A SETTLED ring runs its relaxation kernel one tick in this many (staggered by sub id) —
/// enough to keep healing slow leaks (bleed gaps, glide-ins) at ~13% of the cost, while a
/// staged foe still wakes the ring the very tick it appears (the foe scan runs every tick).
pub const ORBIT_SETTLED_DUTY: u64 = 8;


// ---- Special sub-structure defaults (see [`SubKind`] and the constructors). -----------------

/// Default storage capacity of a [`SubKind::Fortress`] (high — a fortress houses a garrison).
pub const FORTRESS_STORAGE_CAPACITY: u32 = 90;
/// Default capture resistance of a fortress (high — 1.5× a default sub's 3600; owner retune
/// 2026-07-08, halved from 10 800: a fort is a wall by garrison + range, not by grind).
pub const FORTRESS_RESISTANCE: f32 = 5_400.0;
/// The **fixed** engagement range of the owner's idle ships garrisoned on a fortress —
/// independent of the basic [`SimParams::engagement_radius`] (3.5), not a multiplier on it.
/// (Raised 12 → 18 in the tutorial-arc tuning: a fortress commands a genuinely wide zone.)
pub const FORTRESS_RANGE: f32 = 18.0;
/// Default production of a [`SubKind::Shipyard`] (extreme: 8 ships per period).
pub const SHIPYARD_PRODUCTION: u32 = 8;
/// The fraction of its pre-activation `max_resistance` a shipyard keeps once **activated**.
/// **Every yard defaults to the 1.0 token bar** (owner rule, 2026-07-07: zero capacity ⇒ no
/// resistance; 1.0 is the engine floor), so this fraction only matters for a level that opts
/// a neutral yard into an activation grind via `with_max_resistance` — the first capture then
/// collapses the authored bar to a token (floored at 1.0). Expressed as a *fraction* so the
/// collapse is invariant under a host's resistance scaling (`build_scaled` ×24 in the GUI).
pub const SHIPYARD_ACTIVE_RESISTANCE_FRAC: f32 = 1.0 / 10_800.0;
/// A shipyard's radius multiplier over the default-capacity footprint (owner: yards read as
/// 30% bigger than a standard sub — the industrial heart should look the part). Real sim
/// geometry, not just a sprite: the garrison ring and production squares sit on it.
pub const SHIPYARD_RADIUS_MULT: f32 = 1.3;
/// A shipyard's **virtual** storage capacity — a PLANNING number, not a physical one (owner
/// rule): it is the production **auto-divert threshold** (output accumulates at the yard up to
/// this many owner idle ships; past it the overflow ships to struct storage) and the capacity
/// **machine intelligences** see (`PositionView::capacity`). The physical sim treats a yard as
/// its declared **capacity 0**: the garrison bleeds under per-sub attrition like any over-cap
/// surplus (hoarding at the yard costs), and resistance stays the yard's own
/// activation/token bar — never capacity-derived. Invisible to the player (no label).
pub const SHIPYARD_VIRTUAL_CAP: usize = 120;

/// Sub radius per √(storage capacity): a sub's physical size is **determined by its storage
/// capacity** (area ∝ capacity), fixed for the match. Tuned so the default-60 sub is ≈4 units.
/// Combat **engagement range is a separate fixed constant** ([`SimParams::engagement_radius`]) and
/// is *not* affected by sub size — only the garrison ring and footprint scale.
pub const RADIUS_PER_SQRT_STORAGE: f32 = 0.52;

/// A sub's radius for a given storage capacity (`√cap · RADIUS_PER_SQRT_STORAGE`, ≥ a small floor).
#[inline]
pub fn radius_for_storage(cap: u32) -> f32 {
    ((cap.max(1) as f32).sqrt() * RADIUS_PER_SQRT_STORAGE).max(1.5)
}

/// Default [`SubStructure::production`] — ships a sub mints per [`SimParams::production_period`]
/// (one per production "slot"/square). Higher = faster output and a higher effective storage cap.
pub const DEFAULT_PRODUCTION: u32 = 1;

/// Storage capacity of a structure's **reserve / patrol-zone** node (~100× a normal sub). It is
/// the universal inter-struct entry/exit point: it produces nothing and capturing it grants no
/// production, but it gates everything moving in and out of the structure. See
/// [`Interior::add_storage_sub`].
pub const STORAGE_RESERVE_CAP: u32 = 0; // owner design 2026-07-08: the reserve no longer
                                        // stockpiles — anything staged bleeds under the
                                        // per-seat attrition; struct storage is a transit
                                        // zone, and the defender's edge moved to the new
                                        // Layer-2 struct overwatch. Levels may still
                                        // override via `[struct] storage_capacity`.

/// Auto-divert cutoff: a producing sub only ships its over-capacity surplus into struct storage
/// while **fewer than this many enemy ships** sit in the storage (don't feed a contested staging
/// area). See the auto-flow in [`Interior::produce`].
pub const STORAGE_ENEMY_BLOCK: usize = 20;

/// Extra clearance (metres) added to the engagement radius when sizing the reserve node's ring, so
/// the reserve garrison sits *strictly* outside engagement range of the inner sub garrisons (not
/// merely at the boundary). See [`Interior::add_storage_sub`].
pub const STORAGE_RING_BUFFER: f32 = 2.0;

/// Scale applied to the reserve node's radius **on top of** the minimum-clearance solve: the
/// game-scale dial that puts strategic room between a struct's tactical sub cluster and its
/// inter-struct entry/exit ring (tactics happen deep inside; the reserve is a genuinely *outer*
/// orbit). 1.0 = the bare clearance solve; 2.0 doubles it. See [`Interior::add_storage_sub`].
pub const STORAGE_RADIUS_SCALE: f32 = 2.0;

/// Per-tick lerp toward the ring slot for idle ships (a ship spawned at a production square glides
/// out to the ring; existing ships follow the rotation smoothly). 1.0 = snap; lower = slower glide.
pub const ORBIT_GLIDE: f32 = 0.35;

/// Intra-structure **undock delay** (ticks): a freshly-ordered ship sits at its ring slot this many
/// ticks before it begins transiting (it has to peel out of the orbit). Mirrors the inter-structure
/// `WorldParams::undock_ticks` at the sub scale, so leaving a sub is never instantaneous.
pub const UNDOCK_TICKS: u32 = 5;

/// Angular speed (radians per **tick**) at which a producing sub's production "slots" slowly orbit.
/// These slots are the **spawn positions** ([`Interior::spawn_at_square`] places a new ship at the
/// cursor's slot), and the GUI draws them as the production squares — so this is *not* a cosmetic
/// overlay: rotating the slots rotates where ships are created. It is keyed off the sim **tick**
/// (never wall-clock), so replay stays bit-for-bit deterministic. Reads counter-clockwise on screen
/// (the GUI maps +angle to CCW); ~one revolution per 80 ticks.
pub const PROD_SQUARE_SPIN_PER_TICK: f32 = std::f32::consts::TAU / 80.0;

/// Ticks an attrited ship spends **drifting out** of its sub before deletion (see
/// [`Ship::drift_remaining`]). It is a live, shootable ship for this whole window, so ordinary
/// combat may claim it first.
pub const DRIFT_TICKS: u32 = 18;

/// Speed (metres/tick) at which an attrited ship drifts radially outward from its sub while dying.
/// Gentle on purpose: over [`DRIFT_TICKS`] it coasts only ~7 units, so it clears the sub but stays
/// near it (and within a level's clearances, e.g. Mission 1's safe square edges).
pub const DRIFT_SPEED: f32 = 0.4;

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
/// (see [`Interior::resolve_resistance`] / the pure [`SubStructure::capture_step`]): the owner
/// healing it back while present, an attacker grinding it down, a flip + refill at zero. The old
/// instant "uncontested presence flips it" rule is gone.
/// What **kind** of sub-structure this is. `Standard` is the ordinary producer; the three
/// **special** kinds carry one extra rule each (their stat defaults live in the
/// `FORTRESS_*` / `SHIPYARD_*` constants and the [`SubStructure::fortress`] /
/// [`SubStructure::teleporter`] / [`SubStructure::shipyard`] constructors):
///
/// * **`Fortress`** — produces nothing; while owned, the owner's **idle ships garrisoned on it
///   fire at the fixed [`FORTRESS_RANGE`]** (18 — far beyond the basic engagement radius; a
///   one-sided overwatch ring: enemies between `R` and `FORTRESS_RANGE` are shot but cannot
///   shoot back). High capacity + very high resistance.
/// * **`Teleporter`** — produces nothing; ships **ordered away from it by its owner arrive
///   instantly** once their undock delay burns (no transit leg). Standard capacity/resistance.
/// * **`Shipyard`** — extreme production that **accumulates at the yard** up to the invisible
///   [`SHIPYARD_VIRTUAL_CAP`]; past the cap the overflow auto-diverts to struct storage.
///   **Default resistance = the 1.0 token bar** (owner rule, 2026-07-07: zero capacity ⇒ no
///   resistance; 1.0 is the engine floor) — a yard flips to any lone visitor almost instantly
///   (high value, trivially stealable), whoever authored it. A level may still opt into an
///   activation grind via `with_max_resistance`: a neutral-authored yard starts
///   `active: false`, and its first capture **activates** it — collapsing any authored bar by
///   [`SHIPYARD_ACTIVE_RESISTANCE_FRAC`] — permanently. Authored owned ⇒ starts active.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SubKind {
    /// An ordinary producing sub-structure.
    #[default]
    Standard,
    /// Range-doubling strongpoint (drawn as a diamond).
    Fortress,
    /// Instant-transit gate for its owner's departures (drawn as nested circles).
    Teleporter,
    /// Extreme production, zero storage; `active` flips true on first capture and never back.
    Shipyard {
        /// Whether the initial-resistance grind has been overcome (or it started owned).
        active: bool,
    },
}

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
    /// tick; on reaching 0 it spawns and resets to `production_period / production − 1` (the
    /// reset tick does not decrement, so successive spawns land exactly
    /// `production_period / production` ticks apart). Held at the period while `Neutral`.
    pub production_timer: u32,
    /// **Capture resistance**, in `[0, max_resistance]`. Starts at `max_resistance`. An
    /// uncontested foreign faction with `E` ships present erodes it by `E`/tick; the owner
    /// present and uncontested heals it by its present count/tick (capped at `max_resistance`).
    /// On reaching `<= 0` the sub flips to the eroding faction and refills to `max_resistance`.
    pub resistance: f32,
    /// The cap (and refill value) of [`SubStructure::resistance`]. Defaults to
    /// `storage_capacity · RESISTANCE_PER_CAPACITY` (proportional to size, so a bigger sub is harder
    /// to capture; `60·60 = 3600` at the default); override per sub with
    /// [`SubStructure::with_max_resistance`]. Always `>= 1.0` in practice.
    pub max_resistance: f32,
    /// **Storage capacity** — the number of the owner's idle ships this sub holds with **no
    /// attrition**. Above it, the per-sub soft cap bleeds surplus at an expected
    /// `surplus / (storage_per_production · production_period)` ships/tick (see
    /// [`Interior::resolve_softcap`]); production keeps pouring in, so the count settles at an
    /// *effective* cap of `storage + storage_per_production · production` (≈120 for the defaults).
    /// Only consulted by the per-sub attrition model ([`SimParams::per_sub_attrition`]); the
    /// legacy per-structure cap ignores it. Default 60; level-design may override per sub.
    pub storage_capacity: u32,
    /// **Ring radius fraction**: idle ships orbit at `ring_frac · radius` from the sub centre.
    /// Fixed at [`DEFAULT_RING_FRAC`] (0.75) for the match. Real sim state — it sets where the
    /// garrison physically sits, so it is part of the combat geometry (what you see is the truth);
    /// it is folded into the state hash and may be authored per sub, but is not player-adjustable.
    pub ring_frac: f32,
    /// **Production capacity**: ships minted per [`SimParams::production_period`] — one per
    /// production "square". Default [`DEFAULT_PRODUCTION`] (1); higher = faster output (and a
    /// higher effective storage cap, `storage + storage_per_production * production`).
    pub production: u32,
    /// Round-robin cursor over the production squares: index (`0..production`) of the square the
    /// next spawned ship appears at. Advances each spawn, wrapping. Positional bookkeeping only.
    pub produce_cursor: u32,
    /// What kind of sub this is (default [`SubKind::Standard`]); the special kinds each carry
    /// one extra rule — see [`SubKind`]. Real sim state, folded into `state_hash`.
    pub kind: SubKind,
    /// Authored **orbital motion** (owner mechanic, 2026-07-07): when set, this sub's `pos` is
    /// a pure function of the tick — `centre + radius·dir(phase + omega·tick)` — refreshed at
    /// the top of every [`Interior::step`] (no incremental drift; replays are bit-exact).
    /// Everything positional follows for free: garrison rings glide with the sub, fortress
    /// zones travel, combat reads the moving truth. Ships ORDERED to a moving sub lead it —
    /// see the intercept in `dispatch_move`. `None` (the default) = the classic static sub.
    pub orbit: Option<SubOrbit>,
    /// Whether over-capacity **production auto-diverts** to the struct-storage node (the
    /// default). Authored `false` (see [`SubStructure::keep_surplus`]) the sub's spawns stay
    /// home no matter how full it is — the surplus simply bleeds under the per-sub attrition
    /// (owner QoL, 2026-07-08: First steps' Passive keep must not leak its garrison onto the
    /// player's staging ring). Authored behaviour, folded into `state_hash`.
    pub divert_surplus: bool,
}

/// An authored sub-structure orbit: the sub circles `center` at `radius`, sitting at `phase`
/// radians at tick 0 and advancing `omega` radians/tick (negative = clockwise on screen, the
/// same convention as [`SimParams::orbit_rate`]). Captured from the authored position by
/// [`SubStructure::orbiting`]; folded into `state_hash`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SubOrbit {
    pub center: Vec2,
    pub radius: f32,
    pub phase: f32,
    pub omega: f32,
}

// =============================================================================================
// Earliest-intercept solvers (owner ask, 2026-07-08): exact closed forms where they exist,
// bracketed bisection where they don't — replacing the old 3-iteration fixed point, which
// neither guaranteed convergence nor the EARLIEST meeting.
// =============================================================================================

/// Earliest time `t ≥ 0` at which a pursuer starting at `p` and flying straight at `speed`
/// meets a target moving LINEARLY, `T(t) = t0 + vel·t`. **Exact**: squaring the meeting
/// condition `|T(t) − p| = speed·t` gives a quadratic in `t`; the earliest non-negative root
/// wins (when the pursuer is faster the parabola opens downward and `t = 0` sits between the
/// roots, so the later root is the first admissible meeting). `None` = the target outruns
/// the pursuer forever. Unused by the intra-struct sim today (subs orbit, they don't cruise)
/// — public for the Layer-2 fleet-interception design, where fleets move linearly on lanes.
pub fn intercept_linear(p: Vec2, speed: f32, t0: Vec2, vel: Vec2) -> Option<f32> {
    let (ox, oy) = (t0.x - p.x, t0.y - p.y);
    let a = vel.x * vel.x + vel.y * vel.y - speed * speed;
    let b = 2.0 * (ox * vel.x + oy * vel.y);
    let c = ox * ox + oy * oy;
    if a.abs() < 1e-9 {
        // Equal speeds: the quadratic degenerates to `b·t + c = 0`.
        if b.abs() < 1e-9 {
            return (c < 1e-9).then_some(0.0);
        }
        let t = -c / b;
        return (t >= 0.0).then_some(t);
    }
    let disc = b * b - 4.0 * a * c;
    if disc < 0.0 {
        return None;
    }
    let s = disc.sqrt();
    let (t1, t2) = ((-b - s) / (2.0 * a), (-b + s) / (2.0 * a));
    let (lo, hi) = if t1 <= t2 { (t1, t2) } else { (t2, t1) };
    if lo >= 0.0 {
        Some(lo)
    } else if hi >= 0.0 {
        Some(hi)
    } else {
        None
    }
}

/// Earliest time `t ≥ 0` (ticks of straight flight at `speed` from `p`) to meet a target on
/// the CIRCLE `center + radius·(cos, sin)(phase + omega·t)` — `phase` is the target's angle
/// at the pursuer's departure. Picks the **earliest** solution when several exist (a fast
/// orbit can swing in and out of reach repeatedly).
///
/// **Exact closed forms where they exist**: a static target (`omega ≈ 0`, or a degenerate
/// `radius ≈ 0` circle) is plain distance/speed; a pursuer standing at the orbit centre sees
/// constant range `radius`. The general meeting condition is transcendental — no closed
/// form — so: march `f(t) = |T(t) − p| − speed·t` in fixed steps of a SIXTEENTH of the orbit
/// period (f oscillates at the orbit frequency, so a root pair cannot hide inside one step
/// short of a razor graze — and a skipped graze just resolves to the next, slightly later
/// window: conservative, never divergent), out to the guaranteed bound
/// `(|p − center| + radius)/speed + period` (by then the pursuer could have reached ANY
/// point of the circle whatever the phase), then close the first bracket with a fixed-depth
/// bisection. Deterministic: the same inputs walk the same float path every time.
pub fn intercept_circular(
    p: Vec2,
    speed: f32,
    center: Vec2,
    radius: f32,
    phase: f32,
    omega: f32,
) -> f32 {
    let v = speed.max(1e-6);
    let pos_at = |t: f32| -> Vec2 {
        let a = phase + omega * t;
        let a = a.rem_euclid(std::f32::consts::TAU);
        let (sin, cos) = sincos_tau(a);
        Vec2::new(center.x + radius * cos, center.y + radius * sin)
    };
    // EXACT: a static target is distance / speed.
    if omega.abs() < 1e-9 || radius < 1e-6 {
        return p.dist(pos_at(0.0)) / v;
    }
    let to_centre = p.dist(center);
    // EXACT: from the orbit centre, every point of the circle is `radius` away.
    if to_centre < 1e-6 {
        return radius / v;
    }
    let f = |t: f32| p.dist(pos_at(t)) - v * t;
    if f(0.0) <= 0.0 {
        return 0.0; // departing on top of the target
    }
    let period = std::f32::consts::TAU / omega.abs();
    let step = period / 16.0;
    let t_max = (to_centre + radius) / v + period;
    let (mut lo, mut hi) = (0.0f32, t_max);
    let mut bracketed = false;
    let mut t = step;
    // Bounded march to the first sign change (the iteration cap is a numeric backstop only —
    // the t_max bound is reached long before it on any sane orbit).
    for _ in 0..4096 {
        if t >= t_max {
            break;
        }
        if f(t) <= 0.0 {
            lo = t - step;
            hi = t;
            bracketed = true;
            break;
        }
        t += step;
    }
    if !bracketed {
        // No crossing inside the bound (numeric corner): aim for the bound — the ship
        // arrives at worst a fraction of a period early and settles on the ring.
        return t_max;
    }
    // 26 halvings saturate f32 resolution on any sane interval.
    for _ in 0..26 {
        let mid = 0.5 * (lo + hi);
        if f(mid) <= 0.0 {
            hi = mid;
        } else {
            lo = mid;
        }
    }
    hi
}

impl SubStructure {
    /// Create a sub-structure at `pos`, owned by `owner`. Its **radius is derived from its storage
    /// capacity** ([`radius_for_storage`]), not the legacy `_radius` argument (kept only for call
    /// compatibility and ignored). Resistance starts full and **proportional to the storage capacity**
    /// (`capacity · RESISTANCE_PER_CAPACITY`, coupled by `with_storage_capacity`); storage, ring
    /// fraction and production take their defaults (override via the builders).
    pub fn new(pos: Vec2, _radius: f32, owner: Faction) -> SubStructure {
        SubStructure {
            pos,
            radius: radius_for_storage(DEFAULT_STORAGE_CAPACITY),
            owner,
            production_timer: 0,
            // Default resistance is **proportional** to storage capacity (Mechanic): a fresh sub is
            // as hard to capture as it is large. `with_storage_capacity` keeps the two coupled; an
            // explicit `with_max_resistance` overrides it.
            resistance: DEFAULT_STORAGE_CAPACITY as f32 * RESISTANCE_PER_CAPACITY,
            max_resistance: DEFAULT_STORAGE_CAPACITY as f32 * RESISTANCE_PER_CAPACITY,
            storage_capacity: DEFAULT_STORAGE_CAPACITY,
            ring_frac: DEFAULT_RING_FRAC,
            production: DEFAULT_PRODUCTION,
            produce_cursor: 0,
            kind: SubKind::Standard,
            orbit: None,
            divert_surplus: true,
        }
    }

    /// Builder: this sub's over-capacity production **stays home** instead of auto-diverting
    /// to the struct-storage node (the surplus bleeds under the per-sub attrition instead).
    pub fn keep_surplus(mut self) -> SubStructure {
        self.divert_surplus = false;
        self
    }

    /// Put this sub on an authored **orbit** around `center` at `omega` radians/tick (negative
    /// = clockwise on screen): the radius and tick-0 phase are captured from the current `pos`,
    /// so position the sub where it should stand at tick 0 and chain this last.
    pub fn orbiting(mut self, center: Vec2, omega: f32) -> SubStructure {
        let (dx, dy) = (self.pos.x - center.x, self.pos.y - center.y);
        self.orbit = Some(SubOrbit {
            center,
            radius: (dx * dx + dy * dy).sqrt(),
            phase: libm::atan2f(dy, dx),
            omega,
        });
        self
    }

    /// Build a **fortress** at `pos` for `owner` (see [`SubKind::Fortress`]): produces nothing,
    /// high capacity ([`FORTRESS_STORAGE_CAPACITY`], radius derived from it) and very high
    /// resistance ([`FORTRESS_RESISTANCE`]); while owned, the owner's idle garrison fires at
    /// the fixed [`FORTRESS_RANGE`]. Override stats with the usual builders.
    pub fn fortress(pos: Vec2, owner: Faction) -> SubStructure {
        let mut s = SubStructure::new(pos, 0.0, owner)
            .with_storage_capacity(FORTRESS_STORAGE_CAPACITY)
            .with_max_resistance(FORTRESS_RESISTANCE);
        s.production = 0;
        s.kind = SubKind::Fortress;
        s
    }

    /// Build a **teleporter** at `pos` for `owner` (see [`SubKind::Teleporter`]): produces
    /// nothing; standard capacity and resistance; the owner's departures arrive instantly after
    /// their undock delay. Override stats with the usual builders.
    pub fn teleporter(pos: Vec2, owner: Faction) -> SubStructure {
        let mut s = SubStructure::new(pos, 0.0, owner);
        s.production = 0;
        s.kind = SubKind::Teleporter;
        s
    }

    /// Build a **shipyard** at `pos` for `owner` (see [`SubKind::Shipyard`]): extreme production
    /// ([`SHIPYARD_PRODUCTION`]) that **pools at the yard** up to the invisible
    /// [`SHIPYARD_VIRTUAL_CAP`] (a planning number: the auto-divert threshold + the AI-visible
    /// capacity; attrition holds the yard to its declared 0 — the garrison bleeds), and a footprint
    /// [`SHIPYARD_RADIUS_MULT`] (30%) **bigger** than a default sub's — the GUI draws no disk;
    /// the radius sets selection, the garrison ring, and the production squares.
    ///
    /// **Default resistance = the 1.0 token bar**, whoever authors it (owner rule, 2026-07-07:
    /// a zero-capacity sub carries no resistance; 1.0 is the engine floor) — its garrison and
    /// its ground are its only defence. A level wanting a one-time activation grind opts in
    /// with `with_max_resistance` (the first-capture collapse in `resolve_resistance` then
    /// applies).
    pub fn shipyard(pos: Vec2, owner: Faction) -> SubStructure {
        let mut s = SubStructure::new(pos, 0.0, owner);
        s.storage_capacity = 0;
        // Oversized footprint despite the zero capacity (selection + garrison-ring geometry):
        // 30% bigger than a default sub — the industrial heart looks the part.
        s.radius = radius_for_storage(DEFAULT_STORAGE_CAPACITY) * SHIPYARD_RADIUS_MULT;
        s.production = SHIPYARD_PRODUCTION;
        s.kind = SubKind::Shipyard { active: owner.is_real() };
        s.max_resistance = 1.0;
        s.resistance = 1.0;
        s
    }

    /// The **planning** capacity: `storage_capacity` — except a shipyard, which reports its
    /// [`SHIPYARD_VIRTUAL_CAP`]. Consulted by the production **auto-divert** threshold (when a
    /// yard starts shipping overflow to struct storage) and by the **AI view**
    /// (`PositionView::capacity` — machine intelligences reason with it). The physical sim does
    /// NOT: per-sub attrition holds every sub, yards included, to its DECLARED capacity (a yard
    /// bleeds above 0 — hoarding at the yard costs; the vcap is never an attrition shield).
    #[inline]
    pub fn storage_cap_effective(&self) -> usize {
        match self.kind {
            SubKind::Shipyard { .. } => SHIPYARD_VIRTUAL_CAP,
            _ => self.storage_capacity as usize,
        }
    }

    /// Builder: set this sub's [`SubStructure::storage_capacity`] (the no-attrition headroom) — and,
    /// since size follows storage, its [`SubStructure::radius`] with it. Lets a level make a
    /// "warehouse" sub (large storage ⇒ physically big) or a thin entry station (small).
    pub fn with_storage_capacity(mut self, cap: u32) -> SubStructure {
        self.storage_capacity = cap;
        self.radius = radius_for_storage(cap);
        // Resistance defaults to capacity × RESISTANCE_PER_CAPACITY. A later `with_max_resistance` wins.
        self.max_resistance = cap as f32 * RESISTANCE_PER_CAPACITY;
        self.resistance = cap as f32 * RESISTANCE_PER_CAPACITY;
        self
    }

    /// Builder: set this sub's [`SubStructure::production`] capacity (ships per period / squares).
    /// `0` is legitimate (owner bugfix, 2026-07-08): a level's `prod = 0` must mean ZERO — the
    /// old `max(1)` clamp gave Deliberation's teleporter a phantom production square.
    pub fn with_production(mut self, p: u32) -> SubStructure {
        self.production = p;
        self
    }

    /// Builder: set this sub's `max_resistance` (clamped to `>= 1.0`) and refill its current
    /// resistance to that max. Lets a scenario make a sub a cheap foothold (low max) or a
    /// fortress (high max), decoupled from the capacity-derived default
    /// (`storage_capacity · RESISTANCE_PER_CAPACITY`).
    pub fn with_max_resistance(mut self, max: f32) -> SubStructure {
        let m = max.max(1.0);
        self.max_resistance = m;
        self.resistance = m;
        self
    }

    /// This sub's contribution to its **owner's** per-structure soft-cap headroom, in ships —
    /// the per-element capacity that [`Interior::soft_cap`] sums over a faction's owned subs
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
    /// ([`Interior::resolve_resistance`]) and the forward-projection (in the `world` crate)
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
        // Binary façade (Player ↔ Enemy) over the N-seat [`capture_core`] — kept for the
        // forward-projection and the unit tests, which reason in two seats.
        let (owner_present, foreign) = match owner {
            Faction::Player => (present_player, (present_enemy > 0).then_some((Faction::Ai(0), present_enemy))),
            Faction::Ai(0) => (present_enemy, (present_player > 0).then_some((Faction::Player, present_player))),
            _ => (
                0,
                match (present_player > 0, present_enemy > 0) {
                    (true, false) => Some((Faction::Player, present_player)),
                    (false, true) => Some((Faction::Ai(0), present_enemy)),
                    _ => None,
                },
            ),
        };
        Self::capture_core(owner, resistance, max_resistance, owner_present, foreign)
    }

    /// The N-seat capture rule **core** (the single source of truth shared by the sim's
    /// [`Interior::resolve_resistance`] and the world forward-projection). `owner_present` is the
    /// owner's present count; `foreign` is `Some((seat, count))` **iff exactly one foreign real seat
    /// is present** (the lone contester), else `None` (zero foreign, or ≥2 foreign ⇒ contested).
    ///
    /// * **Erode** — a lone foreigner with the owner absent: `resistance` falls by its count; on
    ///   reaching `<= 0` the sub flips to it and refills (`flipped = true`).
    /// * **Heal** — only the owner present: `resistance` rises by the owner's count, capped at max.
    /// * **Frozen** — anything else (empty, contested, or owner + a foreigner): no change.
    #[inline]
    pub fn capture_core(
        owner: Faction,
        resistance: f32,
        max_resistance: f32,
        owner_present: u32,
        foreign: Option<(Faction, u32)>,
    ) -> (Faction, f32, bool) {
        match foreign {
            Some((f, count)) if owner_present == 0 => {
                let eroded = resistance - count as f32;
                if eroded <= 0.0 {
                    (f, max_resistance, true) // FLIP + REFILL
                } else {
                    (owner, eroded, false)
                }
            }
            None if owner_present > 0 => {
                let healed = (resistance + owner_present as f32).min(max_resistance);
                (owner, healed, false)
            }
            _ => (owner, resistance, false), // frozen
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
    /// **Orbit angle** (radians, kept in `[0, 2π)`): the ship's angular position on its home
    /// sub's ring. **Persistent** — kept through transit, so a ship flies to and rejoins an orbit
    /// at the same angle. While idle, the ship physically sits at
    /// `home.centre + home.ring_frac · home.radius · (cos θ, sin θ)` (its real combat position),
    /// and the orbit phase advances `θ` slowly while evening out neighbour spacing.
    pub angle: f32,
    /// Ticks of **undock delay** left before a freshly-ordered ship begins its flight. Set to
    /// [`UNDOCK_TICKS`] when an order is issued; while it counts down the ship sits at its ring
    /// slot (committed but not yet moving), then transits. `0` for idle/garrisoned ships.
    pub undock_remaining: u32,
    /// Ticks of **attrition drift** left. When a ship is bled by the per-sub soft cap (and it is not
    /// sitting in the reserve / patrol-zone node), it is not destroyed at once: it is set to
    /// [`DRIFT_TICKS`] and **drifts outward** from its sub while ordinary combat still applies, then
    /// is deleted when this hits 0. `0` for a normal (non-attriting) ship.
    pub drift_remaining: u32,
    /// Small **random radial offset** from the ring, as a fraction of the sub radius in
    /// `[-RING_OFFSET, RING_OFFSET]`. The ship sits at `(ring_frac + ring_offset) · radius`, so a
    /// sub's idle ships form a slightly fuzzy ring rather than a perfect circle. Re-rolled from the
    /// structure RNG each time the ship is dispatched into transit (and at spawn).
    pub ring_offset: f32,
    /// Radial **drift velocity** of the ring-band churn (offset-fraction per tick): a slowly
    /// wandering, speed- and acceleration-capped velocity that carries the ship back and forth
    /// across the band (ballistic mixing — a capped random walk on POSITION could never cross
    /// the huge reserve band). `0.0` while the churn dial is off. Real sim state (it drives
    /// positions), folded into the state hash.
    pub ring_drift: f32,
}

impl Ship {
    /// True if this ship is alive and currently idle (garrisoning, no move target, not drifting out).
    #[inline]
    pub fn is_idle(&self) -> bool {
        self.alive && self.target.is_none() && self.drift_remaining == 0
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

    /// Cap on the number of **living** ships homed at a single sub-structure. Purely a safety
    /// bound so a runaway snowball cannot grow without limit in a pathological config; far
    /// above normal play. Not a strategic dial. (It once counted LIFETIME spawns — corpses
    /// included — which silenced any long-lived producer after ~4000 spawns ≈ 3 h of play:
    /// the "Passive centre stopped producing" bug.)
    pub max_ships_per_sub: u32,

    /// **Soft-cap free allowance** — flat parked-ship headroom per faction per structure,
    /// independent of how many subs it owns. Part of `soft = softcap_free + softcap_per_sub *
    /// owned_subs`. See [`Interior::resolve_softcap`].
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

    /// **Interior hard cap** — a far-above-play safety bound on a faction's parked ships in one
    /// structure. NOT a strategic dial: there is intentionally no hard strategic ceiling. It
    /// only guarantees a pathological configuration cannot grow parked stacks without limit.
    pub structure_hard_cap: u32,

    /// **Per-sub orbit cap** (positional only). When more than this many of a faction's ships
    /// would idle at a single sub, the overflow is conceptually *placed* at a wider structure
    /// orbit so one sub is not an infinitely dense dot. It is a rendering/positioning concern:
    /// it NEVER destroys ships and is **not** enforced inside [`Interior::resolve_softcap`]
    /// (which would draw RNG). Kept here as the documented dial.
    pub sub_orbit_cap: u32,

    /// **Transit fire gating.** When `true`, a ship that is *in transit* (moving toward a sub —
    /// `Ship::target.is_some()`) may **not** fire on a *stationary* (idle, garrisoned) enemy: an
    /// in-flight wave cannot "drive-by" shoot a garrison. Stationary defenders still fire on the
    /// passing movers, and two movers still fire on each other — so an assault must *land*
    /// (arrive, become idle) before it can trade with a garrison. When `false`, every engaged
    /// ship fires regardless of motion (the original symmetric bubble). The headless validation
    /// default keeps this `false`; the interactive game (`gui_params`) turns it on for feel.
    pub transit_fire_gating: bool,

    /// **Spread-damage combat.** When `true`, combat uses a uniform-grid neighbour search (no
    /// O(N²) all-pairs scan, no visible "bubbles") and every engaged ship **spreads** its fire
    /// evenly across *all* in-range enemies — each in-range enemy is hit with probability
    /// `fire_prob / (in-range count)` — instead of one-shotting a single random target. Expected
    /// kills per shooter are identical (`fire_prob`), so the stochastic square law and the
    /// mean-field projection are unchanged; only the variance drops and damage feels continuous.
    /// When `false`, combat uses the classic one-random-victim path. The headless validation
    /// default keeps this `false`; the interactive game (`gui_params`) turns it on.
    pub spread_damage: bool,

    /// **Effective storage a point of production buys** (`K`), in ships — the only tunable in the
    /// per-sub soft cap. A sub's surplus (idle ships above its [`SubStructure::storage_capacity`])
    /// is bled at an expected `surplus / (K · production_period)` ships/tick, so a sub producing
    /// `P` ships/period balances attrition at a surplus of `K · P` — i.e. the effective cap sits
    /// at `storage_capacity + K · P`. Attrition itself never depends on `P`; faster production
    /// just raises the balance point. Default 60. Only used when `per_sub_attrition` is on.
    pub storage_per_production: u32,

    /// **Per-sub attrition model.** When `true`, the soft cap is applied **per sub** as a gentle
    /// linear bleed of surplus above each sub's storage capacity (see `storage_per_production`),
    /// replacing the legacy per-structure `sqrt` cap. The headless validation default keeps the
    /// legacy cap (`false`) so the AI / level suite is undisturbed; the interactive game turns it on.
    pub per_sub_attrition: bool,

    /// **Orbit rate** (radians/tick): the slow, deterministic spin idle ships' angles advance by
    /// each tick. This is *game movement* (not a cosmetic animation), so it is part of the sim and
    /// the state hash. Default ≈ one revolution per ~600 ticks.
    pub orbit_rate: f32,

    /// **Ring-band churn kick** (per-tick acceleration of [`Ship::ring_drift`], as a fraction
    /// of the drift **speed cap** — [`RADIAL_DRIFT_SPEED_FRAC`] × `ship_speed`, band-limited):
    /// each idle ship's radial velocity wanders under uniform ±kicks, is speed-clamped, and
    /// soft-bounces at the ±[`RING_OFFSET`] band edges. The resulting slow ballistic drift
    /// carries every ship back and forth across the band, so same-bearing opponents stranded
    /// at opposite edges (a gap the engagement radius can't bridge on the huge reserve ring)
    /// cross paths and the standoff dissolves — with radial speed never outpacing real flight
    /// and no snap reversals (owner motion rules). **`0.0` = off (the reference default): no
    /// RNG is drawn, keeping the headless test geometry exact**; the GUI operating point
    /// enables it.
    pub ring_jitter_step: f32,

    /// **Orbit glide** (per-tick lerp, `(0,1]`): how fast an idle ship slides to its ring slot.
    /// Default [`ORBIT_GLIDE`]. A param (not the bare const) so the game can run at a finer tick
    /// rate without snapping the glide.
    pub orbit_glide: f32,

    /// **Production-square spin** (radians/tick): the slow rotation of the production slots / spawn
    /// angle. Default [`PROD_SQUARE_SPIN_PER_TICK`]. A param so the game's finer tick rate keeps the
    /// same on-screen spin speed.
    pub prod_square_spin: f32,

    /// **Attrition drift ticks**: how long an attrited ship drifts before deletion. Default
    /// [`DRIFT_TICKS`]. A param so the game's finer tick rate keeps the same drift *duration*.
    pub drift_ticks: u32,

    /// **Undock delay** (ticks) a freshly-ordered ship waits before transiting. Default
    /// [`UNDOCK_TICKS`]. A param so the game's finer tick rate keeps the same wall-clock peel-out.
    pub undock_ticks: u32,

    /// **Drift speed** (units/tick) an attrited ship coasts outward at. Default [`DRIFT_SPEED`]. A
    /// per-tick rate, so the game's finer tick rate divides it to keep the same coast distance.
    pub drift_speed: f32,
}

impl Default for SimParams {
    /// The Layer-1 operating point. Tuned so a ~5-7 sub-structure skirmish resolves in a
    /// few hundred ticks with chancy small fights and decisive large ones, and so combat is
    /// not so lethal that a single opening clash ends the match — reinforcement and capture
    /// get time to matter.
    fn default() -> Self {
        SimParams {
            engagement_radius: DEFAULT_ENGAGEMENT_RADIUS,
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
            // Gentle anti-hoard bleed: `ceil(0.5 * sqrt(over))` ships/tick above the soft cap. A
            // self-limiting plateau that still settles at the cap, but soft enough that a turtle's
            // standing reserve is not bled faster than it can be rebuilt — so a patient defender can
            // hold a real wall and out-last an over-committed aggressor's cap-exempt mobile hoard
            // (the defend>attack edge). Tuned down from 1.0; see AUTOMATA_DESIGN §6 / the cycle
            // measurement (raising it punishes hoards harder and collapses defend>attack to a tie).
            softcap_attrition: 0.5,
            structure_hard_cap: 1000,
            sub_orbit_cap: 50,
            // Reference operating point keeps the symmetric bubble so the headless harness /
            // unit tests measure the same combat model they always have. The GUI turns it on.
            transit_fire_gating: false,
            spread_damage: false,
            storage_per_production: 60,
            per_sub_attrition: false,
            // ~one revolution per 200 ticks, **clockwise** on screen (negative angle rate). Gentle
            // spacing relaxation. Universal (not gated): the orbit is the single combat-geometry
            // model, so what's drawn is the truth.
            orbit_rate: -std::f32::consts::TAU / 200.0,
            ring_jitter_step: 0.0,
            orbit_glide: ORBIT_GLIDE,
            prod_square_spin: PROD_SQUARE_SPIN_PER_TICK,
            drift_ticks: DRIFT_TICKS,
            undock_ticks: UNDOCK_TICKS,
            drift_speed: DRIFT_SPEED,
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
/// deterministic given its seed: `Interior::step` is the only place randomness enters, and
/// it draws solely from the embedded [`Rng`].
#[derive(Debug, Clone)]
pub struct Interior {
    /// The sub-structures making up this single structure.
    pub subs: Vec<SubStructure>,
    /// All ships, alive and (marked) dead. Stable indices = stable [`ShipId`]s.
    pub ships: Vec<Ship>,
    /// Whole ticks elapsed since the start.
    pub tick: u64,
    /// The seeded generator. Cloning the [`Interior`] clones this, so a clone replays
    /// identically — the basis of the determinism guarantee.
    rng: Rng,
    /// The structure's **reserve / patrol-zone** node, if any: the [`SubId`] of the special sub
    /// (added via [`Interior::add_storage_sub`]) that is the universal inter-struct entry/exit
    /// point. `None` for bare structures (headless tests/scenarios); the game's levels add one.
    pub storage_sub: Option<SubId>,
    /// Cached `params.undock_ticks` / `params.drift_speed` / `params.ship_speed`, refreshed at
    /// the top of each [`step`]. They let the no-params order path ([`dispatch_move`] — undock,
    /// and the moving-sub intercept's flight time) and the drift coast read the *current*
    /// pacing without threading `&SimParams` through `issue_order`'s many callers. Default to
    /// the const reference until the first `step` (hosts prime them via
    /// [`Interior::set_pacing`] for tick-0 orders).
    undock_ticks: u32,
    drift_speed: f32,
    ship_speed: f32,
    /// Reusable flat uniform-grid buckets for `resolve_combat_spread` (cell = engagement radius),
    /// rebuilt each tick by `clear`+refill (no per-tick allocation, no hashing). Scratch only — its
    /// contents are never meaningful between ticks; excluded from `state_hash`.
    combat_grid: Vec<Vec<ShipId>>,
    /// Per-cell **faction bitmask** paired with `combat_grid` (bit per seat): a shooter skips
    /// any cell whose mask holds no foe bit — on a peaceful board the combat pass collapses to
    /// mask reads. Transient cache, never hashed.
    combat_grid_mask: Vec<u32>,
    /// Indices of the `combat_grid` cells occupied LAST tick — so the per-tick reset clears
    /// `O(occupied)` buckets, not every cell of an AABB that far-coasting drifters can balloon
    /// into the tens of thousands. Transient cache, never hashed.
    combat_grid_occupied: Vec<usize>,
    /// Reused per-sub idle-ship buckets for `resolve_orbit` (one filtered pass over the ships
    /// instead of a full scan per sub). Transient cache, never hashed.
    orbit_buckets: Vec<Vec<ShipId>>,
    /// Reused per-ship "any foe in scan range" flags for `resolve_combat_spread` (computed once
    /// per tick; the substeps skip certified-peaceful ships). Transient cache, never hashed.
    combat_candidate: Vec<bool>,
    /// Per-ship **ENGAGED last tick** (had ≥1 foe inside its own engagement reach during the
    /// previous combat phase): the seek's hold signal — an engaged ship stops advancing and
    /// parades (fronts fan out instead of stacking). One-tick lag by construction (orbit runs
    /// before combat); rebuilt every combat phase. Derived state (recomputable from
    /// positions), never hashed.
    combat_engaged: Vec<bool>,
    /// **Teleport jumps this tick**, as `(departure, arrival)` positions — render support for
    /// the GUI's teleport-flash line (a headless host just ignores it). Cleared at the top of
    /// every [`Interior::step`], filled by the movement phase when an owner departure from a
    /// teleporter arrives at undock-end. Transient presentation state: deterministic but
    /// never hashed (like the caches above).
    pub teleport_events: Vec<(Vec2, Vec2)>,
    /// Per-sub **settled-ring flag** (the orbit fast path, owner ask 2026-07-10): true when
    /// the last kernel pass measured every urge on this ring below [`ORBIT_SETTLE_EPS`]
    /// with no staged foes — the ring is at its uniform-parade equilibrium, so the kernel
    /// drops to the 1/[`ORBIT_SETTLED_DUTY`] duty cycle (pure spin + jitter/glide between)
    /// until a staged foe or a real disturbance wakes it. Derived bookkeeping, never hashed
    /// (like `combat_engaged`).
    ring_settled: Vec<bool>,
    /// **World-set fire split** for one faction this tick: `Some((faction, scale))` multiplies
    /// that faction's fire probability in the combat phase. The Layer-2 host sets it before
    /// every step — a struct's sole owner fighting BOTH interior foes and inbound fleets
    /// spreads its budget across the two pools by head-count (this is the interior share;
    /// the `world` crate's overwatch volley fires the complement). Standalone interiors leave
    /// it `None` (= full rate). Transient per-tick input, deterministic but never hashed
    /// (recomputed from hashed state each tick, like the caches above).
    pub fire_scale: Option<(Faction, f64)>,
    /// The shared ORDER JOURNAL, when the host is recording (see [`crate::types::OrderJournal`]):
    /// [`Interior::issue_order_count`] -- the count-canonical order primitive every public order
    /// form resolves to -- appends one entry per call. `None` (the default) records nothing and
    /// costs nothing. Not part of `state_hash` (it's the recording OF inputs, not state). NOTE:
    /// `Clone` shares the journal handle -- replay/projection copies should carry `None`.
    pub journal: Option<crate::types::OrderJournal>,
    /// This interior's struct index in the world, stamped into journal entries (see
    /// `world`'s `set_journaling`); 0 for a standalone interior.
    pub journal_sid: usize,
}

/// Deterministic PAIRED sin/cos for pre-reduced angles (the sim wraps every angle write
/// with `rem_euclid(τ)`, so the caller's `a` sits in `[0, τ)` — a slight spill past τ from
/// float rounding folds correctly too). One exact f64 fold to the nearest quadrant, then
/// f32 minimax polynomials on `[−π/4, π/4]` (Cephes single-precision coefficients,
/// ≤ ~2 × 10⁻⁷ error — the same class as libm): pure IEEE mul/add, bit-identical on wasm
/// and native (the replay contract), and ~3–5× cheaper than a `libm` sinf+cosf pair
/// because the generic huge-argument reduction is skipped. The per-ship ring trig is the
/// orbit tail loop's dominant cost at 10k ships — this is where the batching pays.
#[inline]
pub fn sincos_tau(a: f32) -> (f32, f32) {
    let k = (a * std::f32::consts::FRAC_2_PI).round();
    // Exact-enough residual via one f64 multiply-subtract (f64 arithmetic is IEEE-exact on
    // every target; k ≤ 4 keeps the error ~1e-16, far below an f32 ulp).
    let r = (a as f64 - k as f64 * std::f64::consts::FRAC_PI_2) as f32;
    let x2 = r * r;
    let sin_r = r + r * x2 * (-1.666_665_5e-1 + x2 * (8.332_161e-3 + x2 * (-1.951_529_6e-4)));
    let cos_r = 1.0 - 0.5 * x2 + x2 * x2 * (4.166_664_6e-2 + x2 * (-1.388_731_6e-3 + x2 * 2.443_315_7e-5));
    match (k as i32) & 3 {
        0 => (sin_r, cos_r),
        1 => (cos_r, -sin_r),
        2 => (-sin_r, -cos_r),
        _ => (-cos_r, sin_r),
    }
}

impl Interior {
    /// Create an empty structure (no ships) seeded with `seed`. Add sub-structures with
    /// [`Interior::add_sub`] and ships with [`Interior::spawn_ship`], or use
    /// [`crate::scenario::sample_structure`] for the ready-made sample.
    pub fn new(seed: u64) -> Interior {
        Interior {
            subs: Vec::new(),
            ships: Vec::new(),
            tick: 0,
            rng: Rng::new(seed),
            storage_sub: None,
            undock_ticks: UNDOCK_TICKS,
            drift_speed: DRIFT_SPEED,
            ship_speed: 1.4, // the reference SimParams::default().ship_speed, until set_pacing
            combat_grid: Vec::new(),
            combat_grid_mask: Vec::new(),
            combat_grid_occupied: Vec::new(),
            orbit_buckets: Vec::new(),
            combat_candidate: Vec::new(),
            combat_engaged: Vec::new(),
            teleport_events: Vec::new(),
            ring_settled: Vec::new(),
            fire_scale: None,
            journal: None,
            journal_sid: 0,
        }
    }

    /// Add a sub-structure, returning its [`SubId`].
    pub fn add_sub(&mut self, sub: SubStructure) -> SubId {
        self.subs.push(sub);
        self.subs.len() - 1
    }

    /// True if `sub` is this structure's reserve / patrol-zone node (see [`Interior::add_storage_sub`]).
    #[inline]
    pub fn is_storage(&self, sub: SubId) -> bool {
        self.storage_sub == Some(sub)
    }

    /// Diagnostic: is `sub`'s ring currently SETTLED (the orbit fast path — pure spin until
    /// its population changes or a foe stages)? See [`ORBIT_SETTLE_EPS`] / `resolve_orbit`.
    #[inline]
    pub fn ring_is_settled(&self, sub: SubId) -> bool {
        self.ring_settled.get(sub).copied().unwrap_or(false)
    }

    /// The engagement reach of ship `id` **as a shooter**: the plain engagement radius, or the
    /// fixed [`FORTRESS_RANGE`] for an idle ship garrisoned on a fortress its own side owns
    /// (see [`SubKind::Fortress`]). The per-ship truth combat uses; a renderer attributing a
    /// kill to a plausible shooter should test candidates against *their own* reach — never a
    /// blanket radius (a far ship outside a fortress could not have fired).
    pub fn ship_engagement_reach(&self, id: ShipId, params: &SimParams) -> f32 {
        let boosted = self.ships.get(id).map_or(false, |sh| self.is_fortress_boosted(sh));
        if boosted {
            FORTRESS_RANGE.max(params.engagement_radius)
        } else {
            params.engagement_radius
        }
    }

    /// True for a living, **idle** ship garrisoned on (`home ==`) a fortress its **own side
    /// owns** — the per-ship predicate behind the fortress range boost.
    #[inline]
    fn is_fortress_boosted(&self, sh: &Ship) -> bool {
        sh.is_idle()
            && self
                .subs
                .get(sh.home)
                .map_or(false, |s| s.kind == SubKind::Fortress && s.owner == sh.faction)
    }

    /// Append the structure's **reserve / patrol-zone** node — a giant circle enclosing the existing
    /// subs that is the universal inter-struct entry/exit point. It produces nothing
    /// ([`production`](SubStructure::production) = 0), has a large [`STORAGE_RESERVE_CAP`] storage,
    /// and is **ownerless**: permanently `Neutral`, never captured (`resolve_resistance` skips it) —
    /// a shared staging space. Call **after** the structure's real subs are added. Returns its [`SubId`].
    pub fn add_storage_sub(&mut self) -> SubId {
        self.add_storage_sub_scaled(STORAGE_RADIUS_SCALE)
    }

    /// [`Interior::add_storage_sub`] with a per-level radius `scale` (the level-design dial —
    /// e.g. a mission that wants its reserve ring closer in than the default game scale).
    /// `scale` multiplies the minimum-clearance solve and is clamped to ≥ 1.0: below 1.0 the
    /// reserve garrison would sit inside engagement range of the inner subs and auto-fight
    /// across the boundary.
    pub fn add_storage_sub_scaled(&mut self, scale: f32) -> SubId {
        let scale = scale.max(1.0);
        let n = self.subs.len();
        let (mut cx, mut cy) = (0.0f32, 0.0f32);
        for s in &self.subs {
            cx += s.pos.x;
            cy += s.pos.y;
        }
        let center = if n > 0 { Vec2::new(cx / n as f32, cy / n as f32) } else { Vec2::new(0.0, 0.0) };
        // Enclosing radius: centroid → farthest sub *edge* (an over-estimate of how far a sub's
        // garrison ring reaches, since the ring sits at `ring_frac · radius < radius`).
        let mut encl = 6.0f32;
        for s in &self.subs {
            encl = encl.max(center.dist(s.pos) + s.radius);
        }
        // Struct storage has **no ownership**: it is a shared, never-captured staging space (its
        // owner stays Neutral; `resolve_resistance` skips it). Ships of any side may sit in it.
        let mut storage = SubStructure::new(center, 0.0, Faction::Neutral);
        // Size the reserve so its garrison **ring** clears every inner sub's garrison by the
        // engagement radius (plus a small buffer): a reserve ship and a sub ship of opposing sides
        // are then always >1 engagement radius apart, so they never auto-fight across the reserve
        // boundary — only a deliberate move brings them into contact. A reserve ship actually sits
        // as close as `(ring_frac − RING_OFFSET) · radius` (the per-ship radial jitter), so solve
        // against that innermost reach, not the nominal ring.
        // The clearance solve is the MINIMUM; `scale` (default STORAGE_RADIUS_SCALE) then
        // inflates the ring for strategic room (the scale correction: the entry/exit orbit
        // sits far outside the tactical cluster, not hugging it).
        let clearance = DEFAULT_ENGAGEMENT_RADIUS + STORAGE_RING_BUFFER;
        storage.radius = scale * (encl + clearance) / (storage.ring_frac - RING_OFFSET).max(0.1);
        storage.storage_capacity = STORAGE_RESERVE_CAP;
        storage.production = 0;
        let id = self.add_sub(storage);
        self.storage_sub = Some(id);
        id
    }

    /// Spawn an idle ship for `faction` garrisoned at `home`, placed at a jittered point
    /// inside the sub-structure's radius. Returns its [`ShipId`]. Used at setup and by
    /// production.
    pub fn spawn_ship(&mut self, faction: Faction, home: SubId) -> ShipId {
        let angle = self.insert_angle(home);
        let ring_offset = self.rng.range_f32(-RING_OFFSET, RING_OFFSET);
        let pos = self.ring_pos(home, angle, ring_offset);
        self.ships.push(Ship { faction, pos, target: None, home, aim: pos, alive: true, angle, undock_remaining: 0, drift_remaining: 0, ring_offset, ring_drift: 0.0 });
        self.ships.len() - 1
    }

    /// World position of the point at `angle` on sub `sub`'s ring, with a per-ship radial `offset`
    /// (fraction of the radius): `centre + (ring_frac + offset)·radius·dir`.
    #[inline]
    pub fn ring_pos(&self, sub: SubId, angle: f32, offset: f32) -> Vec2 {
        let s = &self.subs[sub];
        let r = (s.ring_frac + offset) * s.radius;
        let (sin, cos) = sincos_tau(angle);
        Vec2::new(s.pos.x + r * cos, s.pos.y + r * sin)
    }

    /// A good insertion angle for a ship joining sub `sub`'s orbit: the midpoint of the **largest
    /// angular gap** among the ships currently idle there (fills the emptiest arc). `0.0` if the
    /// ring is empty. Deterministic, RNG-free.
    fn insert_angle(&self, sub: SubId) -> f32 {
        let tau = std::f32::consts::TAU;
        let mut angs: Vec<f32> = self
            .ships
            .iter()
            .filter(|s| s.alive && s.target.is_none() && s.drift_remaining == 0 && s.home == sub)
            .map(|s| s.angle.rem_euclid(tau))
            .collect();
        if angs.is_empty() {
            return 0.0;
        }
        angs.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let mut best_mid = angs[0] + tau * 0.5;
        let mut best_gap = -1.0f32;
        for k in 0..angs.len() {
            let a = angs[k];
            let next = if k + 1 < angs.len() { angs[k + 1] } else { angs[0] + tau };
            let gap = next - a;
            if gap > best_gap {
                best_gap = gap;
                best_mid = a + gap * 0.5;
            }
        }
        best_mid.rem_euclid(tau)
    }

    // ----------------------------------------------------------------------
    // Queries (the renderer + AI read these)
    // ----------------------------------------------------------------------

    /// Number of living ships of `faction` (idle + moving).
    pub fn ship_count(&self, faction: Faction) -> usize {
        self.ships.iter().filter(|s| s.alive && s.faction == faction).count()
    }

    /// Number of *producing* sub-structures owned by `faction`. The reserve / patrol-zone node
    /// ([`Interior::add_storage_sub`]) is excluded: it produces nothing and capturing it confers
    /// no territory, so it never counts toward ownership tallies, elimination, or level specs.
    pub fn sub_count(&self, faction: Faction) -> usize {
        self.subs
            .iter()
            .enumerate()
            .filter(|(i, s)| s.owner == faction && self.storage_sub != Some(*i))
            .count()
    }

    /// Owned non-storage subs of `faction` that can still **produce** (`production > 0`).
    /// The ELIMINATION checks count only these (owner QoL): a seat with no ships holding
    /// nothing but fortresses/teleporters can never rebuild — it no longer blocks the win.
    pub fn productive_sub_count(&self, faction: Faction) -> usize {
        self.subs
            .iter()
            .enumerate()
            .filter(|(i, s)| s.owner == faction && s.production > 0 && self.storage_sub != Some(*i))
            .count()
    }

    /// [`Interior::productive_sub_count`] summed over every real seat **other than** `seat`.
    pub fn productive_foreign_sub_count(&self, seat: Faction) -> usize {
        self.subs
            .iter()
            .enumerate()
            .filter(|(i, s)| {
                s.owner.is_real() && s.owner != seat && s.production > 0 && self.storage_sub != Some(*i)
            })
            .count()
    }

    /// Living ships of every real seat **other than** `seat` — the free-for-all "all my rivals"
    /// ship count, summed over any number of AI opponents (no hardcoded seat list).
    pub fn foreign_ship_count(&self, seat: Faction) -> usize {
        self.ships
            .iter()
            .filter(|s| s.alive && s.faction.is_real() && s.faction != seat)
            .count()
    }

    /// Producing sub-structures owned by any real seat **other than** `seat` (reserve node excluded,
    /// like [`Interior::sub_count`]). The free-for-all "all my rivals" territory count.
    pub fn foreign_sub_count(&self, seat: Faction) -> usize {
        self.subs
            .iter()
            .enumerate()
            .filter(|(i, s)| s.owner.is_real() && s.owner != seat && self.storage_sub != Some(*i))
            .count()
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

    /// Idle ships at `sub` belonging to real seats **other than** `owner` — the free-for-all "foes
    /// present" count (every real seat is a foe of every other). Used for production denial and the
    /// storage auto-divert gate so they behave correctly with two-plus AI opponents.
    pub fn foreign_idle_count(&self, sub: SubId, owner: Faction) -> usize {
        self.ships
            .iter()
            .filter(|s| s.is_idle() && s.home == sub && s.faction.is_real() && s.faction != owner)
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

    /// Like [`Interior::presence_in_sub`] but counts only **idle** ships (`target == None`)
    /// of `faction` physically inside `sub`'s radius.
    ///
    /// The forward-projection (in the `world` crate) seeds its initial per-sub presence with
    /// this so it does not double-count a still-inside *moving* ship that is also a scheduled
    /// arrival — it uses the same authoritative radius metric as [`Interior::presence_in_sub`].
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

    /// The **single present faction** at `sub` and its count, or `None` if zero or both real
    /// seats are present (the frozen case). This is exactly the discriminant
    /// [`SubStructure::capture_step`] keys off; surfaced so callers (and strategy helpers like
    /// "is this sub being eroded?") don't re-derive it from two presence calls.
    pub fn single_present_faction(&self, sub: SubId) -> Option<(Faction, u32)> {
        if sub >= self.subs.len() {
            return None;
        }
        let s = &self.subs[sub];
        let r2 = s.radius * s.radius;
        // The lone real seat with a ship inside the radius (idle or moving), or `None` if zero or
        // two-plus distinct real seats are present. Radius-metric analogue of
        // `capture_present_faction`; scans ships once (N-seat correct).
        let mut found: Option<(Faction, u32)> = None;
        for sh in &self.ships {
            if !(sh.alive && sh.faction.is_real() && sh.pos.dist_sq(s.pos) <= r2) {
                continue;
            }
            match found {
                None => found = Some((sh.faction, 1)),
                Some((f, c)) if f == sh.faction => found = Some((f, c + 1)),
                Some(_) => return None,
            }
        }
        found
    }

    /// The **single home-based present faction** at `sub` and its count, or `None` when zero or
    /// both real seats are present (the frozen case) — the home-based analogue of
    /// [`Interior::single_present_faction`] and exactly the discriminant the grind keys off
    /// (living **idle** ships with `home == sub`, the inputs [`Interior::resolve_resistance`]
    /// feeds to [`SubStructure::capture_step`]; a ship merely passing through the radius, or
    /// sitting in the big reserve node that encloses the inner subs, does **not** erode/heal —
    /// the radius metric counts those, this does not). A renderer asking "is this sub being eroded, and by
    /// whom?" should read this so the on-screen cue matches the sim, not the radius metric.
    pub fn capture_present_faction(&self, sub: SubId) -> Option<(Faction, u32)> {
        if sub >= self.subs.len() {
            return None;
        }
        // The lone home-based present real seat (idle, `home == sub`) and its count, or `None` when
        // zero or two-plus distinct real seats are present (contested). Scans ships once — no hardcoded
        // seat list, so it is correct for any number of AI opponents.
        let mut found: Option<(Faction, u32)> = None;
        for sh in &self.ships {
            if !(sh.is_idle() && sh.home == sub && sh.faction.is_real()) {
                continue;
            }
            match found {
                None => found = Some((sh.faction, 1)),
                Some((f, c)) if f == sh.faction => found = Some((f, c + 1)),
                Some(_) => return None, // a second distinct real seat ⇒ contested
            }
        }
        found
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
    /// The ownerless reserve / patrol-zone node is **excluded** — it can never be captured, so
    /// its bar is not part of any faction's grind (same exclusion as [`Interior::sub_count`]).
    pub fn total_foreign_resistance(&self, vs_owner: Faction) -> f32 {
        self.subs
            .iter()
            .enumerate()
            .filter(|(i, s)| s.owner != vs_owner && self.storage_sub != Some(*i))
            .map(|(_, s)| s.resistance)
            .sum()
    }

    /// **Parked** ship count for `faction` in this structure: living ships that are either idle
    /// or in **intra-structure** transit (i.e. all living ships of the faction in this
    /// `Interior`). This mirrors exactly what [`Interior::resolve_softcap`] attrites.
    /// Inter-struct fleets live in the `world` crate, not in a `Interior`, so they are not
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
    /// owned_subs`, so [`Interior::resolve_softcap`] and every prior hash are unchanged. The
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
    /// Which specific idle ships are chosen is deterministic (nearest to the target first, ties by [`ShipId`]), so
    /// a given order on a given state always produces the same result.
    /// COUNT-CANONICAL orders (owner design, 2026-07-10): this bucket form -- like the
    /// percent-slider form below -- resolves to an exact ship count HERE, in-game, and
    /// delegates to [`Interior::issue_order_count`], the one journaled primitive. The
    /// resolution base is [`Interior::idle_count_at`] -- exactly the eligibility set
    /// `dispatch_move` draws from, so the resolved count equals what the closure form
    /// computed (bit-identical behavior, pinned by the replay round-trip).
    pub fn issue_order(&mut self, order: MoveOrder, faction: Faction) -> usize {
        let MoveOrder { source, target, fraction } = order;
        let n = fraction.count_of(self.idle_count_at(source, faction));
        self.issue_order_count(source, target, n, faction)
    }

    /// Like [`Interior::issue_order`] but with a **continuous** send-fraction `frac` in `(0,1]`
    /// — the GUI's free 1–100 % troop slider — instead of a [`FractionBucket`]. Same per-faction
    /// idle-ship selection (nearest-to-target first) and the same determinism; see
    /// [`crate::types::frac_count`]. The four snap positions match the matching bucket exactly.
    pub fn issue_order_fraction(&mut self, source: SubId, target: SubId, frac: f32, faction: Faction) -> usize {
        let n = crate::types::frac_count(self.idle_count_at(source, faction), frac);
        self.issue_order_count(source, target, n, faction)
    }

    /// Like [`Interior::issue_order`] but with an **exact ship count** instead of a fraction: launch
    /// `min(n, idle)` of `faction`'s own idle ships at `source` toward `target`. Returns the number
    /// **actually** dispatched (clamped to what was idle), so a caller keeping its own ledger can
    /// reconcile against the realised count. Same lowest-[`ShipId`]-first selection and determinism as
    /// the bucket/fraction variants — this is the precise primitive a count-based AI (the stateful
    /// colonizer's departure ledger) needs, since [`FractionBucket`] would round the requested count.
    pub fn issue_order_count(&mut self, source: SubId, target: SubId, n: usize, faction: Faction) -> usize {
        // The replay atom: journal the call verbatim (no-ops included -- they replay as
        // no-ops), then dispatch. See [`crate::types::OrderRecord`].
        if let Some(j) = &self.journal {
            j.borrow_mut().push(crate::types::JournalEntry {
                tick: self.tick,
                record: crate::types::OrderRecord::Move {
                    sid: self.journal_sid,
                    source,
                    target,
                    count: n,
                    faction,
                },
            });
        }
        self.dispatch_move(source, target, faction, |idle| n.min(idle))
    }

    /// Shared core of the move orders: take `count(idle_len)` of **`faction`'s own** idle ships at
    /// `source` (**nearest to the target** first, ties by [`ShipId`]) and launch them at `target`,
    /// computing a jittered aim per ship. Returns the number actually dispatched; a silent no-op
    /// (0) on a degenerate/empty order. The faction filter is the safety invariant: an order
    /// issued by one seat can **never** command an opponent's ships that happen to be garrisoned
    /// or capturing on the same sub.
    fn dispatch_move(
        &mut self,
        source: SubId,
        target: SubId,
        faction: Faction,
        count: impl Fn(usize) -> usize,
    ) -> usize {
        if source == target || source >= self.subs.len() || target >= self.subs.len() {
            return 0;
        }
        // Only this faction's idle ships at `source` (matches `idle_count_at`) — taken
        // **nearest-to-the-target first** (ties by ShipId; was lowest-ShipId only). A partial
        // send now peels the side of the ring already facing the destination, so a draw from
        // a huge ring (the reserve above all) no longer yanks far-side ships straight across
        // the middle of the map (owner fix, 2026-07-08). Deterministic — a pure function of
        // positions; full sends are unaffected (everyone goes regardless).
        let tgt_pos = self.subs[target].pos;
        let mut idle: Vec<ShipId> = self
            .ships
            .iter()
            .enumerate()
            .filter(|(_, s)| s.is_idle() && s.home == source && s.faction == faction)
            .map(|(i, _)| i)
            .collect();
        idle.sort_by(|&a, &b| {
            self.ships[a]
                .pos
                .dist_sq(tgt_pos)
                .partial_cmp(&self.ships[b].pos.dist_sq(tgt_pos))
                .unwrap_or(std::cmp::Ordering::Equal)
                .then(a.cmp(&b))
        });
        let n = count(idle.len());
        if n == 0 {
            return 0;
        }
        // Each moved ship keeps its orbit angle, re-rolls its ring offset for the new sub, and aims
        // at that slot on the destination ring, so it flies straight to where it will garrison once
        // arrived (WYSIWYG). A fresh offset per transit gives the ring its fuzzy spread.
        //
        // ORBITING destination (owner rule: ships never chase): the aim slot is computed on the
        // ring as it will stand on ARRIVAL — the EARLIEST intercept of the moving centre
        // (undock delay + straight flight at ship_speed), solved exactly where a closed form
        // exists and by bracketed bisection otherwise (see [`intercept_circular`]; owner ask,
        // 2026-07-08 — replaces the old 3-iteration fixed point, which neither guaranteed
        // convergence nor the earliest meeting). A departure teleporting through an owned
        // gate arrives at undock-end (zero flight), so it leads by the undock alone.
        let teleport_out = self.subs[source].kind == SubKind::Teleporter
            && self.subs[source].owner == faction
            && faction.is_real();
        for &sid in idle.iter().take(n) {
            let off = self.rng.range_f32(-RING_OFFSET, RING_OFFSET);
            let aim = if let Some(o) = self.subs[target].orbit {
                let from = self.ships[sid].pos;
                let speed = self.ship_speed.max(1e-6);
                let base = self.tick + self.undock_ticks as u64;
                let fly = if teleport_out {
                    0.0
                } else {
                    let phase_at_base = o.phase + o.omega * base as f32;
                    intercept_circular(from, speed, o.center, o.radius, phase_at_base, o.omega)
                };
                let pred = self.sub_pos_at(target, base + fly.ceil() as u64);
                let cur = self.subs[target].pos;
                let slot = self.ring_pos(target, self.ships[sid].angle, off);
                Vec2::new(slot.x - cur.x + pred.x, slot.y - cur.y + pred.y)
            } else {
                self.ring_pos(target, self.ships[sid].angle, off)
            };
            let sh = &mut self.ships[sid];
            sh.target = Some(target);
            sh.aim = aim;
            sh.ring_offset = off;
            sh.undock_remaining = self.undock_ticks; // peel out of the orbit before transiting
        }
        n
    }

    // ----------------------------------------------------------------------
    // Idle-ship EXTRACTION (Layer-2 inter-struct export)
    // ----------------------------------------------------------------------
    //
    // These helpers *remove* idle ships from this structure entirely (they are marked
    // dead, so they vanish from this Interior's accounting) and report how many were
    // taken. They exist so a higher layer — the `world` crate — can lift a struct's idle
    // garrison off one Layer-1 `Interior`, carry it across an inter-struct lane as a
    // fleet, and inject it into the destination `Interior` via `spawn_ship`. From this
    // structure's point of view an extracted ship is simply gone (same as if it had been
    // destroyed); from the world's point of view it is conserved (re-spawned on arrival).
    // They draw no randomness, so they never perturb the RNG stream — extracting ships
    // does not change subsequent combat rolls, preserving bit-reproducibility.

    /// Destroy up to `n` living ships of `faction` anywhere on this structure (lowest
    /// [`ShipId`] first — deterministic), returning how many actually died. The Layer-2
    /// combat resolution calls this when a fleet's return fire lands on a struct's
    /// defenders; the deaths surface through the normal liveness diff (kill FX and the
    /// battle-log metrics see them like any other loss). Draws no randomness.
    pub fn kill_ships(&mut self, faction: Faction, n: usize) -> usize {
        let mut killed = 0;
        for sh in self.ships.iter_mut() {
            if killed >= n {
                break;
            }
            if sh.alive && sh.faction == faction {
                sh.alive = false;
                killed += 1;
            }
        }
        killed
    }

    /// Remove up to `n` **idle** ships of `faction` garrisoned at `sub`, marking them dead,
    /// and return how many were actually removed.
    ///
    /// Only living, idle (`target == None`) ships whose `home == sub` and whose faction
    /// matches are eligible — ships in transit are never yanked (consistent with
    /// [`Interior::issue_order`], which also only moves idle ships). Selection is
    /// deterministic (lowest [`ShipId`] first), so a given call on a given state always
    /// removes the same ships. Out-of-range `sub` or `n == 0` removes nothing. This draws
    /// no randomness, so it leaves the RNG stream untouched.
    ///
    /// Intended for the Layer-2 lens: the `world` crate calls this to detach a fleet's
    /// ships from a source structure, then re-spawns the same count at the destination on
    /// arrival (conserving ships across the world even though each Layer-1 `Interior`
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
            if sh.alive && sh.target.is_none() && sh.drift_remaining == 0 && sh.home == sub && sh.faction == faction {
                sh.alive = false;
                taken += 1;
            }
        }
        taken
    }

    /// Structure-wide export: remove a [`FractionBucket`] of `faction`'s total **idle** ships,
    /// drawn from the sub-structures `faction` owns, while leaving at least `keep_floor`
    /// idle ships at each source sub. Returns how many were actually removed.
    ///
    /// The target count is `fraction.count_of(total_idle_of_faction)` — the bucket applied
    /// to *all* of the faction's idle ships across the whole structure. Ships are then pulled
    /// sub-by-sub in ascending [`SubId`] order, but no sub is ever taken below `keep_floor`
    /// idle ships (a small garrison the struct keeps to defend/seed itself). If the floor
    /// binds on every sub, fewer than the target — possibly zero — are taken; the return value
    /// is always the true count removed. Only subs **owned by `faction`** are drawn from
    /// (idle ships of `faction` sitting on a sub it does not own are left in place — they are
    /// garrisoning captured ground, not surplus to export).
    ///
    /// Deterministic and RNG-free, exactly like [`Interior::take_idle_ships`]. This is the
    /// primitive a [`crate::types::FractionBucket`] inter-struct "launch a fleet" order uses
    /// at the world level.
    pub fn take_idle_ships_structwide(
        &mut self,
        faction: Faction,
        fraction: FractionBucket,
        keep_floor: usize,
    ) -> usize {
        self.export_idle_structwide(faction, keep_floor, |total| fraction.count_of(total))
    }

    /// Like [`Interior::take_idle_ships_structwide`] but with a **continuous** send-fraction
    /// `frac` in `(0,1]` — the GUI's free 1–100 % troop slider — instead of a [`FractionBucket`].
    /// Identical sub-by-sub draw order, keep-floor handling and determinism; see
    /// [`crate::types::frac_count`].
    pub fn take_idle_ships_structwide_fraction(
        &mut self,
        faction: Faction,
        frac: f32,
        keep_floor: usize,
    ) -> usize {
        self.export_idle_structwide(faction, keep_floor, |total| crate::types::frac_count(total, frac))
    }

    /// PURE struct-wide export of an EXACT count (the engine half of a fleet order, owner
    /// rule 2026-07-10: no floors, no rally -- only ships moving): with a reserve node,
    /// take `min(n, reserve idle)` straight from the reserve (staging is pure); on a bare
    /// reserveless structure, draw up to `n` sub-by-sub in ascending id order with no
    /// floor. The reserve rally and any home-guard policy live in the callers.
    pub fn take_idle_ships_structwide_count(&mut self, faction: Faction, n: usize) -> usize {
        match self.storage_sub {
            Some(st) => {
                let reserve = self.idle_count_at(st, faction);
                self.take_idle_ships(st, faction, n.min(reserve))
            }
            None => self.export_from_subs(faction, 0, |_| n),
        }
    }

    /// The pool a struct-wide export FRACTION resolves against when deriving its exact
    /// count (the count-canonical wrappers in `world`): the reserve's idle staging -- ships
    /// must rally there before leaving -- or, on a bare reserveless structure, the
    /// faction's total idle (the same base [`Interior::export_idle_structwide`] hands its
    /// `want` closure).
    pub fn export_base(&self, faction: Faction) -> usize {
        match self.storage_sub {
            Some(st) => self.idle_count_at(st, faction),
            None => self.ships.iter().filter(|s| s.is_idle() && s.faction == faction).count(),
        }
    }

    /// Shared core of the struct-wide export. **Ships must rally at the reserve node before they
    /// can leave the structure**, so behaviour depends on whether this structure has a reserve:
    ///
    /// * **With a reserve node** (every campaign structure): a fleet departs **only** from the reserve.
    ///   If the reserve holds this faction's idle ships, `want(reserve)` of them are pulled and
    ///   launched (no keep-floor on the reserve — it is pure staging). If the reserve is **empty**,
    ///   nothing leaves yet: every inner owned sub is ordered to send its idle surplus (above
    ///   `keep_floor`) **to the reserve** (an intra-structure move), and `0` is returned — a later
    ///   export launches them once they have rallied. This is the "stage, then transit" rule.
    /// * **Without a reserve node** (bare structures — headless/test fixtures): the legacy path —
    ///   pull `want(total_idle)` drawn sub-by-sub in ascending [`SubId`] order from owned subs,
    ///   never below `keep_floor`.
    ///
    /// Deterministic and RNG-free; returns the count actually removed for a fleet (`0` when it only
    /// staged).
    fn export_idle_structwide(
        &mut self,
        faction: Faction,
        keep_floor: usize,
        want: impl Fn(usize) -> usize,
    ) -> usize {
        if let Some(st) = self.storage_sub {
            let reserve = self.idle_count_at(st, faction);
            if reserve > 0 {
                // Launch the requested fraction straight from the staging node.
                let n = want(reserve).min(reserve);
                return self.take_idle_ships(st, faction, n);
            }
            // Reserve empty: nothing departs this tick — rally the inner subs' surplus to it first.
            self.stage_to_reserve(faction, st, keep_floor);
            return 0;
        }
        self.export_from_subs(faction, keep_floor, want)
    }

    /// Order every **inner** owned sub to send its idle surplus (everything above `keep_floor`) to
    /// the reserve node `storage` via an ordinary intra-structure move. The reserve itself and subs
    /// not owned by `faction` are skipped. Deterministic / RNG-free (uses [`Interior::dispatch_move`]).
    /// This is the mechanism behind "ships must rally at the reserve before an inter-struct fleet can
    /// depart": an export with an empty reserve stages here instead of launching.
    fn stage_to_reserve(&mut self, faction: Faction, storage: SubId, keep_floor: usize) {
        for sub in 0..self.subs.len() {
            if sub == storage || self.subs[sub].owner != faction {
                continue;
            }
            let idle = self.idle_count_at(sub, faction);
            if idle <= keep_floor {
                continue;
            }
            // Move all idle above the floor toward the reserve, through the JOURNALED
            // count primitive: the rally is caller policy realized as plain movements, so
            // a replay sees it as ordinary Move records (owner rule -- engine primitives
            // and the journal know only which ships move where).
            self.issue_order_count(sub, storage, idle - keep_floor, faction);
        }
    }

    /// PUBLIC rally: order every inner owned sub to send its idle surplus above
    /// `keep_floor` to the reserve node (a no-op without one). The wrapper layer calls
    /// this when a fleet order finds the reserve empty -- "ships must rally at the reserve
    /// before they can leave" is POLICY, expressed as journaled interior moves; the fleet
    /// primitive itself is pure movement.
    pub fn rally_to_reserve(&mut self, faction: Faction, keep_floor: usize) {
        if let Some(storage) = self.storage_sub {
            self.stage_to_reserve(faction, storage, keep_floor);
        }
    }

    /// Legacy direct export for **bare** structures (no reserve node): pull `want(total_idle)` of
    /// `faction`'s idle ships, drawn sub-by-sub in ascending [`SubId`] order from owned subs, never
    /// taking any sub below `keep_floor`. Deterministic and RNG-free; returns the true count removed.
    fn export_from_subs(
        &mut self,
        faction: Faction,
        keep_floor: usize,
        want: impl Fn(usize) -> usize,
    ) -> usize {
        let total_idle = self
            .ships
            .iter()
            .filter(|s| s.is_idle() && s.faction == faction)
            .count();
        let mut want = want(total_idle);
        if want == 0 {
            return 0;
        }
        let mut taken = 0;
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
    ///      (a sub being eroded by an uncontested foe does not produce; see [`Interior::produce`]),
    ///   2. **movement** — moving ships advance toward their aim; arrivals become idle,
    ///   3. **combat** — `combat_substeps` rounds of stochastic square-law fire,
    ///   4. **resistance** — capture grind / heal / flip ([`Interior::resolve_resistance`]),
    ///   5. **soft-cap** — anti-hoard attrition ([`Interior::resolve_softcap`]).
    ///
    /// Two ordering facts the design relies on: **combat resolves before resistance** (a
    /// defender must survive the firefight to count as present for the heal; an attacker erodes
    /// with its post-combat count), and **resistance uses post-movement presence** (a ship that
    /// arrives this tick is inside the radius when the grind runs, so it counts on its arrival
    /// tick). All randomness is drawn from the embedded RNG (combat fire + soft-cap destruction),
    /// so two `Interior`s with the same seed and the same orders evolve identically.
    /// [`Interior::step`] with per-phase wall-clock accumulation into `acc` (seconds; indices:
    /// 0 produce, 1 movement, 2 orbit, 3 combat, 4 resistance, 5 softcap). Diagnostic entry
    /// point for the perf tooling — identical simulation to `step`.
    pub fn step_timed(&mut self, params: &SimParams, acc: &mut [f64; 6]) {
        let mut mark = std::time::Instant::now();
        let mut lap = |acc: &mut f64| {
            let now = std::time::Instant::now();
            *acc += (now - mark).as_secs_f64();
            mark = now;
        };
        self.maybe_compact_dead();
        self.set_pacing(params);
        self.produce(params);
        lap(&mut acc[0]);
        self.advance_movement(params);
        lap(&mut acc[1]);
        self.resolve_orbit(params);
        lap(&mut acc[2]);
        self.resolve_combat(params);
        lap(&mut acc[3]);
        self.resolve_resistance();
        lap(&mut acc[4]);
        self.resolve_softcap(params);
        lap(&mut acc[5]);
        self.tick += 1;
    }

    pub fn step(&mut self, params: &SimParams) {
        // Last tick's teleport flashes have been consumed (or ignored) by now.
        self.teleport_events.clear();
        self.maybe_compact_dead();
        // Cache the pacing params the no-params order path / drift coast need (see the fields).
        self.set_pacing(params);
        // Orbiting subs take this tick's position FIRST — everything downstream (production
        // squares, rings, combat, capture) reads the moving truth.
        self.advance_sub_orbits();
        self.produce(params);
        self.advance_movement(params);
        self.resolve_orbit(params);
        self.resolve_combat(params);
        self.resolve_resistance();
        self.resolve_softcap(params);
        self.tick += 1;
    }

    /// Drop dead ships from the roster once corpses dominate it (checked every 256 ticks, only
    /// past 2048 entries, only when >half are dead) — called automatically by [`Interior::step`].
    /// Corpses are pure overhead — every O(N) pass walks them, and a 3-hour session
    /// accumulates tens of thousands. Deterministic: the trigger is a pure function of the
    /// state, so replays compact at identical ticks (the state hash, which folds corpses,
    /// changes AT the compaction tick — identically in both replays). The GUI's
    /// interpolation/kill-FX snapshots are index-guarded and at worst snap for one frame.
    fn maybe_compact_dead(&mut self) {
        if self.tick % 256 != 0 || self.ships.len() < 2048 {
            return;
        }
        let dead = self.ships.iter().filter(|s| !s.alive).count();
        if dead * 2 > self.ships.len() {
            self.compact_dead();
        }
    }

    /// Prime the cached pacing params ([`SimParams::undock_ticks`] / [`SimParams::drift_speed`])
    /// that the no-params order path ([`Interior::issue_order`] → `dispatch_move`) reads. `step`
    /// refreshes the cache every tick, but an order issued **before a structure's first step**
    /// (e.g. the AI's tick-0 wave) would otherwise undock at the unscaled reference pace — a host
    /// running a scaled operating point should call this once right after building the world.
    pub fn set_pacing(&mut self, params: &SimParams) {
        self.undock_ticks = params.undock_ticks;
        self.drift_speed = params.drift_speed;
        self.ship_speed = params.ship_speed;
    }

    /// Sub `sub`'s centre position at absolute tick `t`: its authored orbit evaluated at `t`
    /// (a static sub just returns its `pos`). Pure — the per-tick update and the movement
    /// intercept share it, so what ships lead is exactly where the sub will stand.
    pub fn sub_pos_at(&self, sub: SubId, t: u64) -> Vec2 {
        match self.subs[sub].orbit {
            Some(o) => {
                let a = (o.phase + o.omega * t as f32).rem_euclid(std::f32::consts::TAU);
                let (sin, cos) = sincos_tau(a);
                Vec2::new(o.center.x + o.radius * cos, o.center.y + o.radius * sin)
            }
            None => self.subs[sub].pos,
        }
    }

    /// Advance every orbiting sub's `pos` to the tick being processed (a pure function of the
    /// authored orbit and the tick — no incremental drift, replay-exact), and **carry its idle
    /// garrison with it** (owner rule, 2026-07-07): the platform's motion moves the ships
    /// directly, and the usual corrective forces (glide, orbit urges) then act on top — a
    /// garrison rides its sub instead of trailing behind on the glide.
    fn advance_sub_orbits(&mut self) {
        let n = self.subs.len();
        let mut deltas: Vec<(f32, f32)> = Vec::new();
        let mut any = false;
        for i in 0..n {
            let d = if self.subs[i].orbit.is_some() {
                let old = self.subs[i].pos;
                let new = self.sub_pos_at(i, self.tick);
                self.subs[i].pos = new;
                any = true;
                (new.x - old.x, new.y - old.y)
            } else {
                (0.0, 0.0)
            };
            deltas.push(d);
        }
        if !any {
            return;
        }
        for sh in &mut self.ships {
            if sh.alive && sh.target.is_none() && sh.drift_remaining == 0 && sh.home < n {
                let (dx, dy) = deltas[sh.home];
                sh.pos.x += dx;
                sh.pos.y += dy;
            }
        }
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
        // ONE tally pass over the ships replaces the old per-sub scans (idle counts, homed
        // counts): `homed_all[sub]` counts the LIVING ships homed there (the population the
        // `max_ships_per_sub` safety cap bounds — counting corpses silenced any long-lived
        // producer after ~4000 lifetime spawns ≈ 3 h of play) and `idle_by[sub]` holds
        // per-seat idle counts in first-seen order. Intra-loop exactness: a spawn at sub `i`
        // is homed at `i` and idle, so it can only affect sub `i`'s own post-spawn divert
        // query (handled with an explicit `+ 1`); no other sub's counts move mid-loop, and
        // nothing spawns homed at storage.
        let mut homed_all: Vec<u32> = vec![0; n];
        let mut idle_by: Vec<Vec<(Faction, usize)>> = vec![Vec::new(); n];
        for sh in &self.ships {
            if sh.home < n && sh.alive {
                homed_all[sh.home] += 1;
                if sh.is_idle() && sh.faction.is_real() {
                    match idle_by[sh.home].iter_mut().find(|(f, _)| *f == sh.faction) {
                        Some((_, c)) => *c += 1,
                        None => idle_by[sh.home].push((sh.faction, 1)),
                    }
                }
            }
        }
        let idle_of = |v: &[(Faction, usize)], f: Faction| -> usize {
            v.iter().find(|(sf, _)| *sf == f).map_or(0, |&(_, c)| c)
        };
        let foreign_of = |v: &[(Faction, usize)], f: Faction| -> usize {
            v.iter().filter(|(sf, _)| *sf != f).map(|&(_, c)| c).sum()
        };
        for sub in 0..n {
            let owner = self.subs[sub].owner;
            if !owner.is_real() {
                self.subs[sub].production_timer = params.production_period;
                continue;
            }
            // Non-producing node (the reserve / patrol-zone storage): mints nothing, ever.
            if self.subs[sub].production == 0 {
                self.subs[sub].production_timer = params.production_period;
                continue;
            }
            // Denial: one uncontested foreign faction present and the owner absent => the sub
            // is being eroded; freeze its output and hold the timer (no catch-up on relief).
            let owner_here = idle_of(&idle_by[sub], owner) > 0;
            let foe_here = foreign_of(&idle_by[sub], owner) > 0; // any other real seat (free-for-all)
            if foe_here && !owner_here {
                continue; // production denied; timer untouched (held steady)
            }
            if self.subs[sub].production_timer == 0 {
                // Respect the per-sub safety cap on the LIVING homed population.
                let already = homed_all[sub];
                if already < params.max_ships_per_sub {
                    let new_id = self.spawn_at_square(owner, sub, params);
                    // Auto-flow surplus: if this sub is **over its effective storage capacity**
                    // (a shipyard's invisible virtual cap stands in for its declared 0), the
                    // freshly-minted ship is shipped to the (ownerless) struct-storage node
                    // rather than piling onto the surplus — so a full yard keeps producing and
                    // its overflow feeds the reserve. Only new production is diverted — idle
                    // surplus ships are never auto-ordered (they bleed off via attrition
                    // instead). Gate: only divert while there are **fewer than
                    // `STORAGE_ENEMY_BLOCK` enemy ships** in storage (don't pour output into a
                    // staging area the foe is contesting). (`+ 1` = the fresh spawn the old
                    // live query saw.)
                    if let Some(storage) = self.storage_sub {
                        if storage != sub
                            && self.subs[sub].divert_surplus
                            && foreign_of(&idle_by[storage], owner) < STORAGE_ENEMY_BLOCK
                            && idle_of(&idle_by[sub], owner) + 1 > self.subs[sub].storage_cap_effective()
                        {
                            let off = self.rng.range_f32(-RING_OFFSET, RING_OFFSET);
                            let aim = self.ring_pos(storage, self.ships[new_id].angle, off);
                            let sh = &mut self.ships[new_id];
                            sh.target = Some(storage);
                            sh.aim = aim;
                            sh.ring_offset = off;
                            sh.undock_remaining = self.undock_ticks;
                        }
                    }
                }
                // `production` ships per period ⇒ one spawn every period/production ticks (≥1).
                // The reset tick itself does not decrement, so reset to interval − 1: the next
                // spawn lands exactly `interval` ticks after this one (interval 1 ⇒ timer 0 ⇒
                // spawn every tick, the intended limit).
                let p = self.subs[sub].production.max(1);
                self.subs[sub].production_timer = (params.production_period / p).max(1) - 1;
            } else {
                self.subs[sub].production_timer -= 1;
            }
        }
    }

    /// Spawn a ship at the sub's next production **square** (round-robin): at half the sub radius
    /// and the square's angle, with that angle as its orbit angle — so the orbit phase then glides
    /// it out to the garrison ring. Advances `produce_cursor`. Returns the new [`ShipId`].
    fn spawn_at_square(&mut self, faction: Faction, sub: SubId, params: &SimParams) -> ShipId {
        let p = self.subs[sub].production.max(1);
        let cursor = self.subs[sub].produce_cursor % p;
        // The slots slowly orbit with the sim tick (deterministic — never wall-clock), so the spawn
        // position cycles round a turning ring. The GUI draws the same tick-based angle as the
        // production squares, keeping square == spawn point. CCW on screen (+angle reads CCW there).
        let base = (cursor as f32) * std::f32::consts::TAU / (p as f32);
        let angle = (base + self.tick as f32 * params.prod_square_spin).rem_euclid(std::f32::consts::TAU);
        let center = self.subs[sub].pos;
        let sq_r = 0.4 * self.subs[sub].radius; // squares sit at 0.4 of the sub radius
        let (sin, cos) = sincos_tau(angle);
        let pos = Vec2::new(center.x + sq_r * cos, center.y + sq_r * sin);
        // A fresh random ring offset for when the orbit glides it out to the garrison ring.
        let ring_offset = self.rng.range_f32(-RING_OFFSET, RING_OFFSET);
        self.ships.push(Ship {
            faction,
            pos,
            target: None,
            home: sub,
            aim: pos,
            alive: true,
            angle,
            undock_remaining: 0,
            drift_remaining: 0,
            ring_offset,
            ring_drift: 0.0,
        });
        self.subs[sub].produce_cursor = (cursor + 1) % p;
        self.ships.len() - 1
    }

    /// (2) Movement: advance each moving ship straight toward its `aim` at `ship_speed`. On
    /// reaching the aim (within `arrival_tolerance`) the ship becomes idle and adopts the
    /// target as its new `home` (it is now garrisoning there). Ships marked for **attrition drift**
    /// instead coast radially outward from their sub and are deleted when their timer runs out.
    fn advance_movement(&mut self, params: &SimParams) {
        // Sub centres (read-only) so the drift step can find a ship's outward direction without a
        // borrow clash against the `&mut self.ships` loop.
        let sub_centers: Vec<Vec2> = self.subs.iter().map(|s| s.pos).collect();
        // Per-sub: the owner whose departures teleport from here (a TELEPORTER works only for the
        // side that owns it; everyone else's ships leave it as ordinary movers). Checked at the
        // moment the undock delay burns out, with the owner *at that tick* — a mid-undock flip
        // changes whether the gate fires (deterministic either way).
        let teleport_owner: Vec<Option<Faction>> = self
            .subs
            .iter()
            .map(|s| (s.kind == SubKind::Teleporter && s.owner.is_real()).then_some(s.owner))
            .collect();
        let drift_speed = self.drift_speed; // hoist out of the &mut self.ships loop
        let mut teleports: Vec<(Vec2, Vec2)> = Vec::new(); // collected outside the &mut borrow
        for sh in &mut self.ships {
            if !sh.alive {
                continue;
            }
            // Attrition drift: coast outward and delete when the timer expires (combat may have
            // claimed it earlier). A drifting ship is not idle and takes no move orders.
            if sh.drift_remaining > 0 {
                sh.drift_remaining -= 1;
                if sh.drift_remaining == 0 {
                    sh.alive = false;
                    continue;
                }
                let c = sub_centers.get(sh.home).copied().unwrap_or(Vec2::new(0.0, 0.0));
                let (mut dx, mut dy) = (sh.pos.x - c.x, sh.pos.y - c.y);
                let mag = (dx * dx + dy * dy).sqrt();
                if mag > 1e-3 {
                    dx /= mag;
                    dy /= mag;
                } else {
                    let (sin, cos) = sincos_tau(sh.angle);
                    dx = cos;
                    dy = sin;
                }
                sh.pos.x += dx * drift_speed;
                sh.pos.y += dy * drift_speed;
                continue;
            }
            let Some(target) = sh.target else { continue };
            // Undock: a freshly-ordered ship peels out of its orbit slot over a few ticks before
            // it starts transiting (leaving a sub is never instantaneous).
            if sh.undock_remaining > 0 {
                sh.undock_remaining -= 1;
                // TELEPORTER: a departure from a teleporter its own side owns arrives the
                // instant the undock burns out — no transit leg (the gate is the undock).
                if sh.undock_remaining == 0
                    && teleport_owner.get(sh.home).copied().flatten() == Some(sh.faction)
                {
                    teleports.push((sh.pos, sh.aim));
                    sh.pos = sh.aim;
                    sh.home = target;
                    sh.target = None;
                }
                continue;
            }
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
        self.teleport_events.extend(teleports);
    }

    /// (2b) Orbit: idle ships sit on their home sub's ring and slowly rotate as **game movement**
    /// (deterministic, no RNG). Each tick, for every sub, its idle ships are taken in angular
    /// order (the in-order circular list); each advances by the shared `orbit_rate` spin plus
    /// its **v3 angular urge** (below); then its real position is recomputed on the ring. These
    /// are the positions combat reads — so what the player sees *is* the combat geometry.
    ///
    /// **ORBIT MODEL v4** (owner spec, 2026-07-08 — literature-grounded rework; supersedes
    /// v3's five-knob separation/seek/leash). The frame: **overdamped interacting particles
    /// on a ring** (the social-force family) — each idle ship's step is a *velocity* summed
    /// from pairwise kernel contributions, clamped to ±`ship_speed` on top of the shared
    /// spin (the structural speed law: idle angular motion never outpaces real flight,
    /// however big the ring). Pairwise superposition — never a mean position — is what makes
    /// balanced crowds exactly calm, responses proportional to imbalance, and contact
    /// dominant; and force-from-potential structure makes the dynamics an (approximate)
    /// gradient flow: no limit cycles, deterministic settling. Two terms:
    ///
    /// * **Pressure** (always on, **faction-blind**): every other ship on the ring within
    ///   the hat-kernel width [`ORBIT_PRESSURE_SPACING`] pushes this ship directly away with
    ///   weight `1 − arc/w`; the signed sum, scaled by `fs/`[`ORBIT_CROWD_STIFFNESS`], is
    ///   the urge. In 1D this is discrete porous-medium flow: a point blob rarefies
    ///   monotonically to the exactly-uniform ring (calm, zero-velocity equilibrium — bulk
    ///   spreading at up to flight speed, a diffusive long-wavelength tail). Faction-BLIND
    ///   is the owner's mixing decision: the only short-range interaction is universal
    ///   excluded volume, so opposing clouds are **miscible** — battles interleave at ~`w`
    ///   spacing (salt-and-pepper melee) and fronts are structurally impossible.
    /// * **Drive**: a ship with staged foes moves toward the bearing of its **nearest** foe
    ///   (shortest circular arc — nearest, never a weighted mean: means point at empty space
    ///   between enemy clusters and strand ships on deterministic saddles) at
    ///   `fs·min(1, arc/w)` — full speed until contact range, a proportional taper below so
    ///   a discrete tick cannot overshoot, and **no standoff and no leash**: nothing stops
    ///   the flow INTO the enemy cloud; pressure spaces the melee and, when a ship dies, the
    ///   gap's neighbours flow in. Foes "staged" = idle real-faction ships inside the sub's
    ///   radius — or, for the STORAGE node, **garrisoned on it** (`home ==`; its giant
    ///   circle would otherwise see the whole map).
    ///
    /// The urges steer **bearings only** — radial positions ride the optional
    /// [`SimParams::ring_jitter_step`] **ring-band churn** (speed- and accel-capped ballistic
    /// drift — the only RNG in this phase, off at the reference point), which dissolves
    /// frozen-jitter standoffs and keeps same-bearing ships visually distinct (same-side
    /// pass-through is deliberate: a hard no-crossing rule would force single-file queues
    /// into the melee). All angular work reads a start-of-tick snapshot — simultaneous,
    /// order-independent. Known (accepted) limit: two overlapping rings of very different
    /// radii can stay radially apart at the same bearing.
    fn resolve_orbit(&mut self, params: &SimParams) {
        let tau = std::f32::consts::TAU;
        // ONE filtered pass over the ships: per-sub idle buckets (ascending ShipId — push order)
        // and a compact idle-real snapshot for the foe scans, instead of a full ships scan per
        // sub (the old O(subs × ships) walk was a top sim cost at 2000+ ships).
        let n_subs = self.subs.len();
        let mut buckets = std::mem::take(&mut self.orbit_buckets);
        buckets.resize_with(n_subs, Vec::new);
        for b in buckets.iter_mut() {
            b.clear();
        }
        // Settled-ring bookkeeping (the fast path below): the flags survive across ticks;
        // a fresh ring starts unsettled.
        let mut settled = std::mem::take(&mut self.ring_settled);
        settled.resize(n_subs, false);
        // (pos, faction, home) of every idle real-faction ship, in ascending ShipId order.
        let mut idle_real: Vec<(Vec2, Faction, SubId)> = Vec::new();
        for i in 0..self.ships.len() {
            let s = &self.ships[i];
            if !(s.alive && s.target.is_none() && s.drift_remaining == 0) {
                continue;
            }
            if s.home < n_subs {
                buckets[s.home].push(i);
            }
            if s.faction.is_real() {
                idle_real.push((s.pos, s.faction, s.home));
            }
        }
        for sub in 0..n_subs {
            let mut ids = std::mem::take(&mut buckets[sub]);
            let n = ids.len();
            if n == 0 {
                settled[sub] = false;
                buckets[sub] = ids;
                continue;
            }
            // SMALL-RING FORCE SKIP (owner rule, 2026-07-10): when the ring's whole diameter
            // fits inside the engagement radius, every ship here is permanently in range of
            // everything else on the ring — pressure, drive, and cohesion are guaranteed
            // tactically irrelevant, so small rings never pay for them. The ring still
            // spins, churns, and glides below; combat reads positions, not forces.
            let ring_out = (self.subs[sub].ring_frac + RING_OFFSET) * self.subs[sub].radius;
            let small = 2.0 * ring_out <= params.engagement_radius;

            // Idle foes this ring should steer toward, as bearings from this sub's centre:
            // * a NORMAL sub counts any idle foe inside its radius (a contested garrison, an
            //   overlapping rival ring — the clash should close and precess);
            // * the STORAGE node counts only foes **garrisoned on it** (`home == sub`). Its
            //   radius circle encloses the whole battlefield, so a geometric test would see
            //   every enemy garrison on the map and lock the reserve into permanent seek (all
            //   ships converging on one bearing, relaxation off — the "radial line" bug).
            //   Ships orbiting some other sub are that sub's fight, even though they sit
            //   inside the reserve's circle; the reserve steers only toward intruders actually
            //   staged in the reserve. Scanned in ascending ShipId order (deterministic
            //   tie-breaks).
            let centre = self.subs[sub].pos;
            let radius2 = self.subs[sub].radius * self.subs[sub].radius;
            let at_storage = self.is_storage(sub);
            // Staged bearings grouped PER SEAT, each list sorted — the per-ship nearest-foe
            // lookup below is a binary search over the foe seats' lists instead of a scan over
            // every staged ship (a 6000-ship reserve parade was 36M `is_foe_of` checks/tick).
            // Tie-break note: among exactly-equidistant bearings the CCW candidate wins (was:
            // lowest foe ShipId) — deterministic; differs only on exact f32 distance ties.
            // Skipped outright for SMALL rings (nothing reads it there).
            let mut seat_bearings: Vec<(Faction, Vec<f32>)> = Vec::new();
            let mut skip = small;
            if !small {
                // Phase 1 — staged-seat DETECTION only (the wake signal): which factions
                // have idle ships staged here. Cheap membership tests, early exit on the
                // second faction; the bearings themselves (atan2 per staged ship) are built
                // only if the kernel actually runs this tick.
                let mut seat_a: Option<Faction> = None;
                let mut foes_present = false;
                for &(pos, f, home) in &idle_real {
                    let staged_here = if at_storage { home == sub } else { pos.dist_sq(centre) <= radius2 };
                    if staged_here {
                        match seat_a {
                            None => seat_a = Some(f),
                            Some(a) if a != f => {
                                foes_present = true;
                                break;
                            }
                            _ => {}
                        }
                    }
                }
                // SETTLED-RING SLEEP (owner ask, 2026-07-10): a ring whose last kernel pass
                // measured every urge below [`ORBIT_SETTLE_EPS`] — with NO staged foes — is
                // at its parade equilibrium: the kernel drops to the 1/[`ORBIT_SETTLED_DUTY`]
                // duty cycle (staggered by sub id), pure spin between. The duty passes keep
                // healing slow leaks (soft-cap bleed gaps, glide-ins — the perpetually
                // bleeding reserve ring sleeps THROUGH its bleed) and re-measure honestly:
                // any real disturbance restores the every-tick kernel, and a staged foe
                // wakes the ring the very tick it appears (phase 1 runs every tick).
                // Deterministic as ever — the decision derives from state.
                // (A fortress-specific smallness rule — a bigger skip threshold from the
                // boosted garrison reach — was tried and REMOVED the same day, owner call:
                // an uncontested fort ring already settles into the duty sleep below, so
                // the special case was redundant.)
                if foes_present {
                    settled[sub] = false;
                } else if settled[sub]
                    && self.tick.wrapping_add(sub as u64) % ORBIT_SETTLED_DUTY != 0
                {
                    skip = true;
                }
                // Phase 2 — the kernel runs: build the per-seat sorted bearings it reads.
                if !skip {
                    for &(pos, f, home) in &idle_real {
                        let staged_here = if at_storage { home == sub } else { pos.dist_sq(centre) <= radius2 };
                        if staged_here {
                            let b = libm::atan2f(pos.y - centre.y, pos.x - centre.x).rem_euclid(tau);
                            match seat_bearings.iter_mut().find(|(sf, _)| *sf == f) {
                                Some((_, v)) => v.push(b),
                                None => seat_bearings.push((f, vec![b])),
                            }
                        }
                    }
                    for (_, v) in seat_bearings.iter_mut() {
                        v.sort_unstable_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
                    }
                }
            }
            if skip {
                // Fast path (small or settled): the shared spin; churn + glide run below.
                for &i in &ids {
                    let a = self.ships[i].angle;
                    self.ships[i].angle = (a + params.orbit_rate).rem_euclid(tau);
                }
            } else {
            // Key-extracted unstable sort: angles live in [0, τ) (rem_euclid on every write),
            // and non-negative f32 bit patterns are order-isomorphic to the floats — so
            // (bits, id) reproduces the old stable by-angle sort (ids arrive ascending) with
            // none of the per-comparison ship indirection.
            let mut keyed: Vec<(u32, ShipId)> = ids.iter().map(|&i| (self.ships[i].angle.to_bits(), i)).collect();
            keyed.sort_unstable();
            ids.clear();
            ids.extend(keyed.iter().map(|&(_, i)| i));
            // Snapshot start-of-tick angles so the relaxation is simultaneous (order-independent).
            let angs: Vec<f32> = ids.iter().map(|&i| self.ships[i].angle).collect();

            // Prefix sums over the full sorted bearing list (ALL factions — the v4 pressure
            // is faction-blind), for O(log n) windowed (count, Σ bearing) queries. f64: the
            // windowed DIFFERENCE of a many-thousand-bearing prefix sum cancels catastrophic
            // f32 digits, and the weight algebra multiplies the residue by r/w (~200 on the
            // reserve ring) — f32 here would inject O(1) pressure noise. Deterministic
            // either way; f64 keeps it *correct*.
            let mut pre: Vec<f64> = Vec::with_capacity(n + 1);
            pre.push(0.0);
            for k in 0..n {
                pre.push(pre[k] + angs[k] as f64);
            }
            // (count, Σ shifted bearings) of ships with bearing strictly inside `(lo, hi)`,
            // each bearing counted as `θ + shift` — the shift unwraps a circular window
            // segment into the caller's linear frame. `lo`/`hi` are plain (unwrapped) reals.
            let seg = |lo: f32, hi: f32, shift: f64| -> (f64, f64) {
                let i1 = angs.partition_point(|&b| b <= lo);
                let i2 = angs.partition_point(|&b| b < hi);
                if i2 <= i1 {
                    (0.0, 0.0)
                } else {
                    ((i2 - i1) as f64, pre[i2] - pre[i1] + (i2 - i1) as f64 * shift)
                }
            };
            let mut max_urge = 0.0f32;
            for k in 0..n {
                let a = angs[k];
                let me = self.ships[ids[k]].faction;
                // FRAME CONSISTENCY (owner bug report, 2026-07-08 — prograde/retrograde
                // asymmetry in storage): `seat_bearings` are POSITION bearings, and an idle
                // position trails its slot angle by the steady glide lag
                // (≈ orbit_rate / orbit_glide rad — spin-directional, worth 10–20 wu of arc
                // on the reserve ring). Measuring foe deltas and cohesion windows against
                // the SLOT angle `a` therefore biased every decision retrograde; the ship's
                // own POSITION bearing `ap` is the like-for-like frame — the lag cancels
                // for same-ring foes and stays honest for cross-ring ones. PRESSURE keeps
                // slot angles: all ring-mates lag equally, so that frame is self-consistent.
                let ap = {
                    let sp = self.ships[ids[k]].pos;
                    libm::atan2f(sp.y - centre.y, sp.x - centre.x).rem_euclid(tau)
                };
                // Nearest foe bearing by shortest circular arc: the circular nearest of a
                // sorted list is one of the two neighbours of the insertion point, so each
                // foe seat costs one binary search. (CCW wins exact-distance ties.)
                let mut best: Option<(f32, f32)> = None; // (signed delta, |delta|)
                for (f, list) in &seat_bearings {
                    if !f.is_foe_of(me) || list.is_empty() {
                        continue;
                    }
                    let idx = list.partition_point(|&b| b < ap);
                    let c1 = list[idx % list.len()];
                    let c2 = list[(idx + list.len() - 1) % list.len()];
                    for &b in &[c1, c2] {
                        let mut d = (b - ap).rem_euclid(tau);
                        if d > tau * 0.5 {
                            d -= tau;
                        }
                        let abs = d.abs();
                        let better = match best {
                            None => true,
                            Some((bd, ba)) => abs < ba || (abs == ba && d > bd),
                        };
                        if better {
                            best = Some((d, abs));
                        }
                    }
                }
                // The ship's slot radius in world units — the arc-unit / angle exchange rate.
                let r = ((self.subs[sub].ring_frac + self.ships[ids[k]].ring_offset)
                    * self.subs[sub].radius)
                    .max(0.25);
                let v = params.ship_speed;
                // PRESSURE (faction-blind, always on): every ship with bearing strictly
                // inside the ±w window pushes this one directly away with hat-kernel weight
                // `1 − arc/w`. For a one-sided window the weighted sum decomposes linearly:
                //   Σ (1 − |a − θ'| · r/w)  =  m − (r/w)·|m·a − Σθ'|-style prefix arithmetic,
                // so each side is one or two O(log n) segment queries. Same-bearing ties
                // (Δ = 0) are excluded by the strict bounds — their push is sign(0) = 0; the
                // radial band churn is what separates them visually.
                let w_arc = ORBIT_PRESSURE_SPACING;
                let w_rad = (w_arc / r).min(tau * 0.5 - 1e-4);
                let tau64 = tau as f64;
                let (nb, sb) = if a - w_rad >= 0.0 {
                    seg(a - w_rad, a, 0.0)
                } else {
                    let (n1, s1) = seg(a - w_rad + tau, tau, -tau64);
                    let (n2, s2) = seg(-1.0, a, 0.0);
                    (n1 + n2, s1 + s2)
                };
                let (nf, sf) = if a + w_rad <= tau {
                    seg(a, a + w_rad, 0.0)
                } else {
                    let (n1, s1) = seg(a, tau, 0.0);
                    let (n2, s2) = seg(-1.0, a + w_rad - tau, tau64);
                    (n1 + n2, s1 + s2)
                };
                let (a64, rw) = (a as f64, (r / w_arc) as f64);
                let s_behind = nb - rw * (nb * a64 - sb); // Σ weights, ships behind (CCW push)
                let s_ahead = nf - rw * (sf - nf * a64); // Σ weights, ships ahead (CW push)
                let mut urge = v * ((s_behind - s_ahead) as f32) / ORBIT_CROWD_STIFFNESS;
                // DRIVE: toward the nearest staged foe's bearing at full flight speed, with a
                // proportional taper inside the pressure spacing (no overshoot in a discrete
                // tick; no standoff, no leash — the melee mixes, pressure spaces it).
                if let Some((d, abs)) = best {
                    urge += v * ((abs * r) / w_arc).min(1.0) * d.signum();
                    // COHESION (same-faction, wartime only — see [`ORBIT_COHESION_SPAN`]):
                    // urged toward whichever side holds more of MY faction within ±the
                    // window, at the normalized count imbalance × flight speed. The surface
                    // tension that coarsens the striped stalemate into a tournament of
                    // pairwise-merging pockets. Uses the same staged per-seat sorted bearings
                    // as the foe lookup (self and exact ties excluded by the strict bounds).
                    if let Some((_, mine)) = seat_bearings.iter().find(|(f, _)| *f == me) {
                        let wc = (ORBIT_COHESION_SPAN / r).min(tau * 0.5 - 1e-4);
                        // Bearings strictly inside the circular arc (from, to), to − from < τ.
                        let count_open = |from: f32, to: f32| -> usize {
                            let gt = |x: f32| mine.partition_point(|&b| b <= x);
                            let ge = |x: f32| mine.partition_point(|&b| b < x);
                            let f = from.rem_euclid(tau);
                            let t = to.rem_euclid(tau);
                            if f <= t {
                                ge(t).saturating_sub(gt(f))
                            } else {
                                (mine.len() - gt(f)) + ge(t)
                            }
                        };
                        let n_b = count_open(ap - wc, ap);
                        let n_f = count_open(ap, ap + wc);
                        urge += v * ORBIT_COHESION_STRENGTH * (n_f as f32 - n_b as f32)
                            / (n_f + n_b).max(4) as f32;
                    }
                }
                // The speed law: urges on top of the shared spin never exceed flight speed.
                urge = urge.clamp(-v, v);
                max_urge = max_urge.max(urge.abs());
                self.ships[ids[k]].angle = (a + params.orbit_rate + urge / r).rem_euclid(tau);
            }
            // Everything quiet and nobody hostile staged ⇒ the ring is settled: next tick
            // takes the fast path until its population changes or a foe stages.
            settled[sub] = seat_bearings.len() < 2 && max_urge < ORBIT_SETTLE_EPS * params.ship_speed;
            } // end full kernel path
            // Ring-band CHURN (GUI dial; 0 = off at the reference point, no RNG drawn): each
            // idle ship carries a radial drift VELOCITY that wanders under small random kicks
            // (`ring_jitter_step` × the speed cap per tick = the acceleration cap), is clamped
            // to the speed cap, and soft-bounces at the band edges. Ballistic, speed- and
            // accel-capped drift: it crosses even the huge reserve band in bounded time
            // (dissolving same-bearing standoffs) while never outpacing real flight or
            // snapping direction — a capped random walk on POSITION could do neither.
            let v_cap = (RADIAL_DRIFT_SPEED_FRAC * params.ship_speed
                / self.subs[sub].radius.max(1e-6))
            .min(RING_OFFSET * 0.1);
            for &i in &ids {
                if params.ring_jitter_step > 0.0 {
                    let a_cap = params.ring_jitter_step * v_cap;
                    let v = (self.ships[i].ring_drift + self.rng.range_f32(-a_cap, a_cap))
                        .clamp(-v_cap, v_cap);
                    let off = self.ships[i].ring_offset + v;
                    if off.abs() > RING_OFFSET {
                        // Soft bounce off the band edge (half the speed, turned inward).
                        self.ships[i].ring_offset = off.clamp(-RING_OFFSET, RING_OFFSET);
                        self.ships[i].ring_drift = -v * 0.5;
                    } else {
                        self.ships[i].ring_offset = off;
                        self.ships[i].ring_drift = v;
                    }
                }
                let ang = self.ships[i].angle;
                // Seekers and paraders alike ride their (now slowly churning) ring slot — the
                // seek steers bearings only.
                let off = self.ships[i].ring_offset;
                let target = self.ring_pos(sub, ang, off);
                let cur = self.ships[i].pos;
                // Glide toward the ring slot rather than snapping: a ship just spawned at a
                // production square (half radius) slides out to the ring, and existing ships
                // follow the rotation smoothly. pos is the real position (WYSIWYG).
                self.ships[i].pos = Vec2::new(
                    cur.x + (target.x - cur.x) * params.orbit_glide,
                    cur.y + (target.y - cur.y) * params.orbit_glide,
                );
            }
            buckets[sub] = ids; // hand the bucket back for reuse next tick
        }
        self.orbit_buckets = buckets;
        self.ring_settled = settled;
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
        // COMBAT-IMPOSSIBILITY GATE (owner approval, 2026-07-10): one cheap pass builds each
        // real faction's bounding box over its living ships; if every hostile pair of boxes
        // is separated on some axis by more than the longest reach in the game (the fortress
        // range or the engagement radius, whichever is bigger), no shot can land this tick —
        // skip the whole pass, grid upkeep included. Fires on single-faction interiors and
        // on boards whose factions haven't met yet (most rear structs, most of the early
        // game). Deterministic: the decision derives from state, and a skipped tick draws no
        // combat RNG (like a tick with no engaged ships).
        {
            // (faction, min_x, max_x, min_y, max_y)
            let mut boxes: Vec<(Faction, f32, f32, f32, f32)> = Vec::new();
            for s in &self.ships {
                if !s.alive || !s.faction.is_real() {
                    continue;
                }
                match boxes.iter_mut().find(|(f, ..)| *f == s.faction) {
                    Some((_, x0, x1, y0, y1)) => {
                        *x0 = x0.min(s.pos.x);
                        *x1 = x1.max(s.pos.x);
                        *y0 = y0.min(s.pos.y);
                        *y1 = y1.max(s.pos.y);
                    }
                    None => boxes.push((s.faction, s.pos.x, s.pos.x, s.pos.y, s.pos.y)),
                }
            }
            let reach = FORTRESS_RANGE.max(params.engagement_radius);
            let mut possible = false;
            'pairs: for i in 0..boxes.len() {
                for j in (i + 1)..boxes.len() {
                    if !boxes[i].0.is_foe_of(boxes[j].0) {
                        continue;
                    }
                    let gap_x = (boxes[j].1 - boxes[i].2).max(boxes[i].1 - boxes[j].2);
                    let gap_y = (boxes[j].3 - boxes[i].4).max(boxes[i].3 - boxes[j].4);
                    if gap_x <= reach && gap_y <= reach {
                        possible = true;
                        break 'pairs;
                    }
                }
            }
            if !possible {
                // Nobody can be engaged: clear the hold flags and skip the pass outright.
                self.combat_engaged.clear();
                self.combat_engaged.resize(self.ships.len(), false);
                return;
            }
        }
        if params.spread_damage {
            self.resolve_combat_spread(params);
        } else {
            self.resolve_combat_classic(params);
        }
    }

    /// Per-ship FORTRESS range boost flags for one combat resolution: `true` for a living,
    /// **idle** ship garrisoned on (`home ==`) a fortress its **own side owns** — those ships
    /// fire at the fixed [`FORTRESS_RANGE`]. Range is per-**shooter**, so the
    /// overwatch is one-sided: an enemy between `R` and `2R` is shot but cannot shoot back.
    /// Idle/home/owner are all fixed during the combat phase, so one snapshot serves every
    /// sub-step.
    fn fortress_boost(&self) -> Vec<bool> {
        self.ships.iter().map(|sh| self.is_fortress_boosted(sh)).collect()
    }

    /// Classic combat: each engaged ship fires with probability `fire_prob` and one-shots a single
    /// random in-range enemy. O(N²) per sub-step; the headless / validation reference model.
    fn resolve_combat_classic(&mut self, params: &SimParams) {
        let substeps = params.combat_substeps.max(1);
        let r2 = params.engagement_radius * params.engagement_radius;
        let boost = self.fortress_boost();
        let boosted_r2 = {
            let br = FORTRESS_RANGE.max(params.engagement_radius);
            br * br
        };
        let mut engaged = std::mem::take(&mut self.combat_engaged);
        engaged.clear();
        engaged.resize(self.ships.len(), false);
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
                // Transit fire gating: a ship in transit (has a move target) may not fire on a
                // *stationary* (idle) enemy. It can still fire on other movers; stationary ships
                // fire on it normally. See [`SimParams::transit_fire_gating`].
                let shooter_moving = sh.target.is_some() || sh.drift_remaining > 0;
                // This shooter's reach (fortress garrisons out-range everyone else).
                let reach2 = if boost[i] { boosted_r2 } else { r2 };
                // Gather living enemies in range (recomputed per sub-step against current
                // liveness), excluding ones this shooter is not permitted to fire on.
                let mut in_range: Vec<ShipId> = Vec::new();
                for j in 0..n {
                    if i == j {
                        continue;
                    }
                    let other = &self.ships[j];
                    if params.transit_fire_gating && shooter_moving && other.target.is_none() {
                        continue; // mover cannot shoot a garrisoned (stationary) ship
                    }
                    if other.alive
                        && other.faction != sh.faction
                        && other.faction.is_real()
                        && sh.faction.is_real()
                        && sh.pos.dist_sq(other.pos) <= reach2
                    {
                        in_range.push(j);
                    }
                }
                if in_range.is_empty() {
                    continue; // not engaged
                }
                engaged[i] = true; // the seek's hold signal (read by next tick's orbit)
                // Engaged: fire with probability p (+defender bonus if inside own sub).
                let mut p = params.fire_prob;
                if params.defender_fire_bonus != 0.0 && self.ship_in_own_sub(i) {
                    p += params.defender_fire_bonus;
                }
                // The world-set fire split: this faction is also firing on inbound fleets
                // in the Layer-2 pass, so only its interior share of the budget lands here.
                if let Some((f, s)) = self.fire_scale {
                    if sh.faction == f {
                        p *= s;
                    }
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
        self.combat_engaged = engaged;
    }

    /// Spread-damage combat (see [`SimParams::spread_damage`]). A **uniform grid** (cell =
    /// engagement radius, rebuilt once per tick — positions are fixed during the combat phase)
    /// replaces the O(N²) all-pairs scan: each ship only inspects the 3×3 block of cells around
    /// it. Every engaged ship then **spreads** its fire across *all* its in-range enemies — each
    /// is hit with probability `fire_prob / k` (`k` = in-range count) — rather than one-shotting
    /// one random target. Expected kills per shooter stay `fire_prob`, so aggregate attrition (and
    /// the mean-field projection) is unchanged; only the variance drops. Fully deterministic: the
    /// grid is only ever queried by key, bucket contents are in ascending [`ShipId`] order, and
    /// each shooter's targets are sorted before the RNG draws.
    fn resolve_combat_spread(&mut self, params: &SimParams) {
        let n = self.ships.len();
        if n == 0 {
            return;
        }
        let r = params.engagement_radius.max(1e-3);
        let r2 = r * r;
        let inv = 1.0 / r;
        let cell_of = |p: crate::types::Vec2| -> (i32, i32) {
            ((p.x * inv).floor() as i32, (p.y * inv).floor() as i32)
        };
        // Flat uniform grid over live, real-faction ships: take the AABB of occupied cells, lay out
        // `cols × rows` flat buckets indexed `(cx-min_cx) + (cy-min_cy)·cols`, reused across ticks
        // (clear + refill — no per-tick allocation, no hashing). Behaviour-identical to the old
        // HashMap: same cells, ascending-ShipId buckets, same 3×3 scan order, same sorted targets,
        // same RNG draws.
        let (mut min_cx, mut min_cy, mut max_cx, mut max_cy) = (i32::MAX, i32::MAX, i32::MIN, i32::MIN);
        for i in 0..n {
            let sh = &self.ships[i];
            if sh.alive && sh.faction.is_real() {
                let (cx, cy) = cell_of(sh.pos);
                min_cx = min_cx.min(cx);
                min_cy = min_cy.min(cy);
                max_cx = max_cx.max(cx);
                max_cy = max_cy.max(cy);
            }
        }
        if min_cx > max_cx {
            return; // no live real-faction ships
        }
        let cols = (max_cx - min_cx + 1) as usize;
        let rows = (max_cy - min_cy + 1) as usize;
        let cell_idx = |cx: i32, cy: i32| -> usize { (cx - min_cx) as usize + (cy - min_cy) as usize * cols };
        // One combat bit per seat (Neutral never fights): a cell's mask says who stands in it,
        // so a shooter can skip foe-free cells without touching the bucket.
        let seat_bit = |f: Faction| -> u32 {
            match f {
                Faction::Player => 1,
                Faction::Ai(i) => 1u32 << (1 + (i as u32).min(30)),
                Faction::Neutral => 0,
            }
        };
        let mut grid = std::mem::take(&mut self.combat_grid);
        let mut mask = std::mem::take(&mut self.combat_grid_mask);
        let mut occupied = std::mem::take(&mut self.combat_grid_occupied);
        // Reset ONLY the cells used last tick (their indices refer to last tick's layout, so
        // clear BEFORE any resize), then grow-to-fit without ever shrinking: far-coasting
        // drifters can balloon the AABB into tens of thousands of cells, and a full-grid sweep
        // per tick was the dominant sim cost on peaceful boards.
        for &c in &occupied {
            grid[c].clear();
            mask[c] = 0;
        }
        occupied.clear();
        let cells = cols * rows;
        if grid.len() < cells {
            grid.resize_with(cells, Vec::new);
            mask.resize(cells, 0);
        }
        for i in 0..n {
            let sh = &self.ships[i];
            if sh.alive && sh.faction.is_real() {
                let (cx, cy) = cell_of(sh.pos);
                let c = cell_idx(cx, cy);
                if grid[c].is_empty() {
                    occupied.push(c);
                }
                grid[c].push(i);
                mask[c] |= seat_bit(sh.faction);
            }
        }
        let substeps = params.combat_substeps.max(1);
        let boost = self.fortress_boost();
        let boosted_reach = FORTRESS_RANGE.max(r);
        let boosted_r2 = boosted_reach * boosted_reach;
        // Cells a boosted shooter must scan: its fixed reach over cells of size R.
        let boost_span = (boosted_reach / r).ceil() as i32;
        // Once per tick: does ship `i`'s scan neighbourhood hold ANY foe bit? Positions are
        // frozen across the substeps and kills only REMOVE candidates, so this is an exact
        // (conservative-only-shrinking) filter — the substeps skip the certified-peaceful
        // ships, which on a quiet board is nearly all of them.
        let mut has_foe = std::mem::take(&mut self.combat_candidate);
        has_foe.clear();
        has_foe.resize(n, false);
        for i in 0..n {
            let sh = &self.ships[i];
            if !sh.alive || !sh.faction.is_real() {
                continue;
            }
            let my_bit = seat_bit(sh.faction);
            let (cx, cy) = cell_of(sh.pos);
            let span = if boost[i] { boost_span } else { 1 };
            'scan: for gx in (cx - span)..=(cx + span) {
                if gx < min_cx || gx > max_cx {
                    continue;
                }
                for gy in (cy - span)..=(cy + span) {
                    if gy < min_cy || gy > max_cy {
                        continue;
                    }
                    if mask[cell_idx(gx, gy)] & !my_bit != 0 {
                        has_foe[i] = true;
                        break 'scan;
                    }
                }
            }
        }
        let mut engaged = std::mem::take(&mut self.combat_engaged);
        engaged.clear();
        engaged.resize(n, false);
        let mut targets: Vec<ShipId> = Vec::new();
        for _ in 0..substeps {
            let mut kills: Vec<ShipId> = Vec::new();
            for i in 0..n {
                if !has_foe[i] {
                    continue;
                }
                let sh = &self.ships[i];
                if !sh.alive || !sh.faction.is_real() {
                    continue;
                }
                let shooter_moving = sh.target.is_some() || sh.drift_remaining > 0;
                let (cx, cy) = cell_of(sh.pos);
                targets.clear();
                // This shooter's reach and cell span (fortress garrisons out-range everyone
                // else, so they scan a wider neighbourhood; everyone else keeps the 3×3).
                let (reach2, span) = if boost[i] { (boosted_r2, boost_span) } else { (r2, 1) };
                // Inspect the neighbourhood of cells (a ship's in-range enemies can only sit
                // within its reach, i.e. within `span` cells in each direction).
                let my_bit = seat_bit(sh.faction);
                for gx in (cx - span)..=(cx + span) {
                    if gx < min_cx || gx > max_cx {
                        continue;
                    }
                    for gy in (cy - span)..=(cy + span) {
                        if gy < min_cy || gy > max_cy {
                            continue;
                        }
                        // Foe-free cell: nothing in it could enter `targets` — skip the bucket.
                        // (Pure filter: identical targets, identical RNG draws.)
                        if mask[cell_idx(gx, gy)] & !my_bit == 0 {
                            continue;
                        }
                        for &j in &grid[cell_idx(gx, gy)] {
                            if j == i {
                                continue;
                            }
                            let other = &self.ships[j];
                            if !other.alive || other.faction == sh.faction {
                                continue;
                            }
                            // Transit gating: a mover cannot fire on a stationary garrison.
                            if params.transit_fire_gating && shooter_moving && other.target.is_none() {
                                continue;
                            }
                            if sh.pos.dist_sq(other.pos) <= reach2 {
                                targets.push(j);
                            }
                        }
                    }
                }
                if targets.is_empty() {
                    continue;
                }
                engaged[i] = true; // the seek's hold signal (read by next tick's orbit)
                targets.sort_unstable(); // deterministic RNG-draw order
                let k = targets.len() as f64;
                let mut d = params.fire_prob;
                if params.defender_fire_bonus != 0.0 && self.ship_in_own_sub(i) {
                    d += params.defender_fire_bonus;
                }
                // The world-set fire split: this faction is also firing on inbound fleets
                // in the Layer-2 pass, so only its interior share of the budget lands here.
                if let Some((f, s)) = self.fire_scale {
                    if sh.faction == f {
                        d *= s;
                    }
                }
                // Spread this ship's fire evenly: each in-range enemy is hit with prob d/k, so the
                // expected number killed by this shooter is k·(d/k) = d (same as the classic path).
                let per = (d / k).min(1.0);
                for &j in &targets {
                    if self.rng.chance(per) {
                        kills.push(j);
                    }
                }
            }
            for t in kills {
                self.ships[t].alive = false;
            }
        }
        self.combat_grid = grid; // hand the buckets back for reuse next tick
        self.combat_grid_mask = mask;
        self.combat_grid_occupied = occupied;
        self.combat_candidate = has_foe;
        self.combat_engaged = engaged;
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
        // Only ships **garrisoned at a sub** (idle, home == sub) contest/erode it — home-based
        // so a ship merely passing through the radius, or sitting in the big reserve node that
        // encloses the inner subs, does not spuriously count. The renderer + the host's
        // end-of-match check read the **same** counts via [`Interior::capture_present_faction`],
        // so what the player sees is exactly what the grind acts on (WYSIWYG).
        // ONE tally pass over the ships (was a full scan per sub): per-sub per-seat idle counts
        // in **first-seen ShipId order** — the same order the old scan discovered factions in,
        // so "the lone foreign seat" resolves identically.
        let mut pres: Vec<Vec<(Faction, u32)>> = vec![Vec::new(); n];
        for sh in &self.ships {
            if sh.is_idle() && sh.faction.is_real() && sh.home < n {
                match pres[sh.home].iter_mut().find(|(f, _)| *f == sh.faction) {
                    Some((_, c)) => *c += 1,
                    None => pres[sh.home].push((sh.faction, 1)),
                }
            }
        }
        for sub in 0..n {
            if self.is_storage(sub) {
                continue; // struct storage has no ownership — it is never captured
            }
            // Free-for-all: the owner's home-based present count, plus the lone contesting
            // foreign real seat (exactly one foreign present; zero or two-plus foreign ⇒
            // frozen). No hardcoded seat list — correct for any seat count.
            let owner = self.subs[sub].owner;
            let mut owner_present = 0u32;
            let mut foreign: Option<(Faction, u32)> = None;
            let mut foreign_contested = false;
            for &(f, c) in &pres[sub] {
                if f == owner {
                    owner_present = c;
                } else if foreign.is_none() {
                    foreign = Some((f, c));
                } else {
                    foreign_contested = true;
                }
            }
            let foreign = if foreign_contested { None } else { foreign };
            let (new_owner, new_res, flipped) = SubStructure::capture_core(
                owner,
                self.subs[sub].resistance,
                self.subs[sub].max_resistance,
                owner_present,
                foreign,
            );
            let s = &mut self.subs[sub];
            s.owner = new_owner;
            s.resistance = new_res;
            if flipped {
                s.production_timer = s.production_timer.max(1);
                // Shipyard ACTIVATION: the first capture overcomes the initial-resistance grind
                // for good — the bar collapses to a token fraction of its pre-activation max
                // (scale-invariant, see [`SHIPYARD_ACTIVE_RESISTANCE_FRAC`]), so from here on the
                // yard flips to any lone visitor almost instantly. One-way: never deactivates.
                if let SubKind::Shipyard { active } = s.kind {
                    if !active {
                        s.kind = SubKind::Shipyard { active: true };
                        s.max_resistance = (s.max_resistance * SHIPYARD_ACTIVE_RESISTANCE_FRAC).max(1.0);
                        s.resistance = s.max_resistance;
                    }
                }
            }
        }
    }

    /// (5) Soft cap (Mechanic C): anti-hoard attrition. For each real seat, with
    /// `parked = ` living ships of the seat in this structure (idle or intra-structure transit;
    /// inter-struct fleets are not in a `Interior`, so they are exempt) and
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
    /// is only a pathology guard. Surplus must be spent or kept moving (inter-struct transit is
    /// the cap-exempt escape valve).
    ///
    /// Determinism: the random victims are drawn from the structure's seeded RNG, and the draw
    /// position is folded into [`Interior::state_hash`]. To keep the RNG stream stable when no
    /// attrition happens, **no RNG is drawn unless at least one ship must die.**
    fn resolve_softcap(&mut self, params: &SimParams) {
        if params.per_sub_attrition {
            self.resolve_softcap_per_sub(params);
        } else {
            self.resolve_softcap_struct(params);
        }
    }

    /// Per-sub linear soft cap (see [`SimParams::per_sub_attrition`]). For each owned sub, the
    /// owner's idle ships above the sub's [`SubStructure::storage_capacity`] are the *surplus*;
    /// this tick destroys an expected `surplus / (storage_per_production · production_period)` of
    /// them (stochastic rounding via the structure RNG). Production keeps refilling, so the count
    /// settles at the effective cap `storage + storage_per_production · P`. Gentle: at the default
    /// denominator (60·18 = 1080) a sub one storage-worth over loses ≈1 ship / 18 ticks — exactly
    /// the production rate, the balance point.
    fn resolve_softcap_per_sub(&mut self, params: &SimParams) {
        let denom = (params.storage_per_production.max(1) * params.production_period.max(1)) as f64;
        // ONE bucketing pass over the ships (was a full scan per (sub, seat)): each sub's idle
        // real ships as (ShipId, Faction) in ascending-id order — exactly the order the old
        // per-seat scans collected, so the Fisher–Yates victims and RNG draws are identical.
        let n = self.subs.len();
        let mut staged: Vec<Vec<(ShipId, Faction)>> = vec![Vec::new(); n];
        for (i, sh) in self.ships.iter().enumerate() {
            if sh.is_idle() && sh.faction.is_real() && sh.home < n {
                staged[sh.home].push((i, sh.faction));
            }
        }
        for sub in 0..n {
            let owner = self.subs[sub].owner;
            let at_storage = self.is_storage(sub);
            if at_storage {
                // The ownerless reserve / patrol-zone node: it is permanently Neutral, so the
                // owner-keyed path below never reaches it — instead bleed **each real seat's**
                // stockpile above the node's (huge) capacity independently, in ascending seat
                // order so the RNG draws replay identically. This is the pathology guard that
                // bounds an otherwise attrition-exempt reserve stockpile.
                let mut seats: Vec<Faction> = Vec::new();
                for &(_, f) in &staged[sub] {
                    if !seats.contains(&f) {
                        seats.push(f);
                    }
                }
                seats.sort_by_key(|f| faction_byte(*f));
                for seat in seats {
                    self.bleed_sub_surplus(sub, seat, &staged[sub], denom, params);
                }
                continue;
            }
            if !owner.is_real() {
                continue; // neutral subs garrison/produce nothing
            }
            self.bleed_sub_surplus(sub, owner, &staged[sub], denom, params);
        }
    }

    /// One seat's per-sub attrition at `sub`: bleed an expected `surplus / denom` of its idle
    /// (stored) ships above the sub's [`SubStructure::storage_capacity`] (stochastic rounding via
    /// the structure RNG; **no RNG is drawn when there is no surplus**). In the reserve /
    /// patrol-zone node a bled ship is destroyed at once; anywhere else it is set adrift (it
    /// coasts out of the sub for `drift_ticks`, shootable, then is deleted). `staged` is the
    /// sub's pre-bucketed idle roster (ascending ShipId).
    fn bleed_sub_surplus(
        &mut self,
        sub: SubId,
        seat: Faction,
        staged: &[(ShipId, Faction)],
        denom: f64,
        params: &SimParams,
    ) {
        // The DECLARED capacity (owner rule): a shipyard physically behaves as capacity 0 —
        // its garrison bleeds like any over-cap surplus. The virtual cap is a PLANNING number
        // (the auto-divert threshold + what machine intelligences see), never an attrition
        // shield.
        let storage = self.subs[sub].storage_capacity as usize;
        // The seat's idle (stored) ships at this sub — its stockpile (already-drifting ships
        // are excluded: they are not idle and are on their way out).
        let mut idle: Vec<ShipId> =
            staged.iter().filter(|&&(_, f)| f == seat).map(|&(i, _)| i).collect();
        let surplus = idle.len().saturating_sub(storage);
        if surplus == 0 {
            return; // at/below storage: no attrition (and no RNG draw)
        }
        // Expected bled = surplus / denom; stochastic-round to a whole count.
        let expected = surplus as f64 / denom;
        let mut n = expected.floor() as usize;
        if self.rng.chance(expected.fract()) {
            n += 1;
        }
        let n = n.min(idle.len());
        // Bleed `n` of the seat's idle ships at this sub (partial Fisher–Yates, RNG-driven).
        let at_storage = self.is_storage(sub);
        for k in 0..n {
            let j = k + self.rng.below(idle.len() - k);
            idle.swap(k, j);
            if at_storage {
                self.ships[idle[k]].alive = false;
            } else {
                self.ships[idle[k]].drift_remaining = params.drift_ticks;
            }
        }
    }

    /// Legacy per-structure `sqrt` soft cap — the headless / validation reference operating point
    /// (see [`SimParams::per_sub_attrition`]).
    fn resolve_softcap_struct(&mut self, params: &SimParams) {
        // Free-for-all: attrit every real seat with ships here, independently, in a deterministic
        // seat order (ascending faction code) so the per-faction RNG draws replay identically.
        let mut seats: Vec<Faction> = Vec::new();
        for sh in &self.ships {
            if sh.alive && sh.faction.is_real() && !seats.contains(&sh.faction) {
                seats.push(sh.faction);
            }
        }
        seats.sort_by_key(|f| faction_byte(*f));
        for faction in seats {
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
                Faction::Ai(_) => e.has_enemy = true,
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
                    Faction::Ai(_) => enemy_count += 1,
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
        let e_ships = self.ship_count(Faction::Ai(0));
        let p_subs = self.sub_count(Faction::Player);
        let e_subs = self.sub_count(Faction::Ai(0));
        let p_dead = self.is_eliminated(Faction::Player);
        let e_dead = self.is_eliminated(Faction::Ai(0));

        let (winner, by_elim) = if p_dead && !e_dead {
            (Some(Faction::Ai(0)), true)
        } else if e_dead && !p_dead {
            (Some(Faction::Player), true)
        } else {
            // Lead at horizon by combined ships + sub-structures.
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
            mix_f32(&mut h, s.ring_frac);
            mix_u64(&mut h, s.production as u64);
            mix_u64(&mut h, s.produce_cursor as u64);
            mix_u64(&mut h, s.storage_capacity as u64);
            mix(&mut h, kind_byte(s.kind));
            mix(&mut h, s.divert_surplus as u8);
            // The authored orbit shapes every future position, so it is part of the fingerprint
            // (the moving `pos` above already reflects the current tick).
            if let Some(o) = s.orbit {
                mix_f32(&mut h, o.center.x);
                mix_f32(&mut h, o.center.y);
                mix_f32(&mut h, o.radius);
                mix_f32(&mut h, o.phase);
                mix_f32(&mut h, o.omega);
            }
        }
        // The reserve / patrol-zone designation shapes evolution (routing, attrition, capture
        // skip), so two differently-configured structures hash differently.
        mix_u64(&mut h, self.storage_sub.map(|i| i as u64 + 1).unwrap_or(0));
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
            mix_f32(&mut h, sh.angle);
            mix_f32(&mut h, sh.ring_offset);
            mix_f32(&mut h, sh.ring_drift);
            mix_u64(&mut h, sh.undock_remaining as u64);
            mix_u64(&mut h, sh.drift_remaining as u64);
        }
        // Fold in the RNG's current position so divergent random draws are detected even if
        // they have not yet changed any visible field.
        mix_u64(&mut h, self.rng.clone().next_u64());
        h
    }

    /// Drop dead ships, compacting the `ships` Vec. **Invalidates existing [`ShipId`]s**, so
    /// only call between frames if the renderer does not cache ids across the call. `step`
    /// invokes this automatically once corpses dominate a large roster (see
    /// `maybe_compact_dead`); it stays public for hosts that want to force it. Ship indices
    /// shift, so the per-ship transient caches from last tick are cleared (one replay-identical
    /// tick of everyone-seeks is the only observable).
    pub fn compact_dead(&mut self) {
        self.ships.retain(|s| s.alive);
        self.combat_engaged.clear();
        self.combat_candidate.clear();
    }
}

#[inline]
fn kind_byte(k: SubKind) -> u8 {
    match k {
        SubKind::Standard => 0,
        SubKind::Fortress => 1,
        SubKind::Teleporter => 2,
        SubKind::Shipyard { active: false } => 3,
        SubKind::Shipyard { active: true } => 4,
    }
}

#[inline]
fn faction_byte(f: Faction) -> u8 {
    match f {
        Faction::Neutral => 0,
        Faction::Player => 1,
        // Ai(0)=2, Ai(1)=3, … — preserves the old Enemy=2 / Enemy2=3 codes, so existing levels'
        // `state_hash` is unchanged; any number of AI seats encodes distinctly (saturates at 255).
        Faction::Ai(i) => 2u8.saturating_add(i),
    }
}

// =============================================================================================
// Snapshot serialization (replay persistence — see `crate::snap` for the primitives)
// =============================================================================================
//
// Everything a restored Interior needs to EVOLVE bit-identically to the clone it was taken
// from. That is a superset of `state_hash`'s coverage: the RNG stream, the cached pacing
// (tick-0 orders read it before the first `step` refresh), and the two derived flags that
// carry cross-tick information (`combat_engaged` — the orbit hold signal has a one-tick lag
// by construction; `ring_settled` — the duty-cycle sleep decides whether the next kernel
// pass runs at all). The per-tick scratch caches (grids, buckets, candidates) and per-tick
// inputs (`fire_scale`, `teleport_events`) rebuild from hashed state and are skipped.

use crate::snap::{r_faction as snap_r_faction, r_vec2 as snap_r_vec2, w_faction as snap_w_faction, w_vec2 as snap_w_vec2};

fn snap_w_sub(w: &mut crate::snap::SnapWriter, s: &SubStructure) {
    snap_w_vec2(w, s.pos);
    w.f32(s.radius);
    snap_w_faction(w, s.owner);
    w.u32(s.production_timer);
    w.f32(s.resistance);
    w.f32(s.max_resistance);
    w.u32(s.storage_capacity);
    w.f32(s.ring_frac);
    w.u32(s.production);
    w.u32(s.produce_cursor);
    match s.kind {
        SubKind::Standard => w.u8(0),
        SubKind::Fortress => w.u8(1),
        SubKind::Teleporter => w.u8(2),
        SubKind::Shipyard { active } => {
            w.u8(3);
            w.bool(active);
        }
    }
    match s.orbit {
        None => w.bool(false),
        Some(o) => {
            w.bool(true);
            snap_w_vec2(w, o.center);
            w.f32(o.radius);
            w.f32(o.phase);
            w.f32(o.omega);
        }
    }
    w.bool(s.divert_surplus);
}

fn snap_r_sub(r: &mut crate::snap::SnapReader) -> Option<SubStructure> {
    let pos = snap_r_vec2(r)?;
    let radius = r.f32()?;
    let owner = snap_r_faction(r)?;
    let production_timer = r.u32()?;
    let resistance = r.f32()?;
    let max_resistance = r.f32()?;
    let storage_capacity = r.u32()?;
    let ring_frac = r.f32()?;
    let production = r.u32()?;
    let produce_cursor = r.u32()?;
    let kind = match r.u8()? {
        0 => SubKind::Standard,
        1 => SubKind::Fortress,
        2 => SubKind::Teleporter,
        3 => SubKind::Shipyard { active: r.bool()? },
        _ => return None,
    };
    let orbit = if r.bool()? {
        Some(SubOrbit { center: snap_r_vec2(r)?, radius: r.f32()?, phase: r.f32()?, omega: r.f32()? })
    } else {
        None
    };
    let divert_surplus = r.bool()?;
    Some(SubStructure {
        pos,
        radius,
        owner,
        production_timer,
        resistance,
        max_resistance,
        storage_capacity,
        ring_frac,
        production,
        produce_cursor,
        kind,
        orbit,
        divert_surplus,
    })
}

fn snap_w_ship(w: &mut crate::snap::SnapWriter, s: &Ship) {
    snap_w_faction(w, s.faction);
    snap_w_vec2(w, s.pos);
    match s.target {
        None => w.bool(false),
        Some(t) => {
            w.bool(true);
            w.uz(t);
        }
    }
    w.uz(s.home);
    snap_w_vec2(w, s.aim);
    w.bool(s.alive);
    w.f32(s.angle);
    w.u32(s.undock_remaining);
    w.u32(s.drift_remaining);
    w.f32(s.ring_offset);
    w.f32(s.ring_drift);
}

fn snap_r_ship(r: &mut crate::snap::SnapReader) -> Option<Ship> {
    let faction = snap_r_faction(r)?;
    let pos = snap_r_vec2(r)?;
    let target = if r.bool()? { Some(r.uz()?) } else { None };
    let home = r.uz()?;
    let aim = snap_r_vec2(r)?;
    let alive = r.bool()?;
    let angle = r.f32()?;
    let undock_remaining = r.u32()?;
    let drift_remaining = r.u32()?;
    let ring_offset = r.f32()?;
    let ring_drift = r.f32()?;
    Some(Ship {
        faction,
        pos,
        target,
        home,
        aim,
        alive,
        angle,
        undock_remaining,
        drift_remaining,
        ring_offset,
        ring_drift,
    })
}

impl Interior {
    /// Serialize this interior's full sim state (see the module-tail comment above for what
    /// is and is not included). Composes into `world`'s snapshot blob.
    pub fn snap_write(&self, w: &mut crate::snap::SnapWriter) {
        w.uz(self.subs.len());
        for s in &self.subs {
            snap_w_sub(w, s);
        }
        w.uz(self.ships.len());
        for s in &self.ships {
            snap_w_ship(w, s);
        }
        w.u64(self.tick);
        w.u64(self.rng.state_bits());
        match self.storage_sub {
            None => w.bool(false),
            Some(s) => {
                w.bool(true);
                w.uz(s);
            }
        }
        w.u32(self.undock_ticks);
        w.f32(self.drift_speed);
        w.f32(self.ship_speed);
        w.uz(self.journal_sid);
        w.uz(self.combat_engaged.len());
        for &b in &self.combat_engaged {
            w.bool(b);
        }
        w.uz(self.ring_settled.len());
        for &b in &self.ring_settled {
            w.bool(b);
        }
    }

    /// Rebuild an interior from [`Interior::snap_write`] bytes. The scratch caches start
    /// empty (rebuilt per tick), `fire_scale`/`teleport_events` start clear (per-tick
    /// inputs), and the journal is `None` (snapshot copies never record). `None` = the blob
    /// is malformed or from a drifted format — the caller drops the snapshot.
    pub fn snap_read(r: &mut crate::snap::SnapReader) -> Option<Interior> {
        let mut it = Interior::new(0);
        for _ in 0..r.uz()? {
            it.subs.push(snap_r_sub(r)?);
        }
        for _ in 0..r.uz()? {
            it.ships.push(snap_r_ship(r)?);
        }
        it.tick = r.u64()?;
        it.rng = Rng::from_state_bits(r.u64()?);
        it.storage_sub = if r.bool()? { Some(r.uz()?) } else { None };
        it.undock_ticks = r.u32()?;
        it.drift_speed = r.f32()?;
        it.ship_speed = r.f32()?;
        it.journal_sid = r.uz()?;
        for _ in 0..r.uz()? {
            it.combat_engaged.push(r.bool()?);
        }
        for _ in 0..r.uz()? {
            it.ring_settled.push(r.bool()?);
        }
        Some(it)
    }
}

#[cfg(test)]
mod take_idle_tests {
    //! Unit tests for the Layer-2 inter-struct export helpers
    //! ([`Interior::take_idle_ships`] / [`Interior::take_idle_ships_structwide`]).
    //!
    //! These live in the library crate (not the `tests/` integration target) so they run as
    //! part of the `layer1` lib test harness.
    use super::*;

    /// Two owned subs for `faction`, far apart so nothing fights, with the requested idle
    /// garrisons. Returns the structure and the two SubIds.
    fn two_sub_struct(seed: u64, faction: Faction, n0: usize, n1: usize) -> (Interior, SubId, SubId) {
        let mut st = Interior::new(seed);
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
        let moved = st.issue_order(MoveOrder::new(a, b, FractionBucket::Half), Faction::Player);
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
        assert_eq!(st.take_idle_ships(a, Faction::Ai(0), 2), 0, "no enemy ships to take");
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
    fn structwide_respects_keep_floor() {
        // 10 idle on sub a, 0 on b. Half of 10 = 5 wanted. With keep_floor 3, a can export
        // at most 10-3 = 7, so all 5 are taken and 5 remain.
        let (mut st, a, _b) = two_sub_struct(6, Faction::Player, 10, 0);
        let took = st.take_idle_ships_structwide(Faction::Player, FractionBucket::Half, 3);
        assert_eq!(took, 5);
        assert_eq!(st.idle_count_at(a, Faction::Player), 5);
    }

    #[test]
    fn structwide_floor_can_bind_and_reduce_export() {
        // 4 idle on a, 4 on b => total 8, All => want 8. keep_floor 3 => each sub exports at
        // most 1, so only 2 are taken (1 from each), floor binds hard.
        let (mut st, a, b) = two_sub_struct(7, Faction::Player, 4, 4);
        let took = st.take_idle_ships_structwide(Faction::Player, FractionBucket::All, 3);
        assert_eq!(took, 2, "keep-floor on every sub caps the export");
        assert_eq!(st.idle_count_at(a, Faction::Player), 3);
        assert_eq!(st.idle_count_at(b, Faction::Player), 3);
    }

    #[test]
    fn structwide_only_pulls_from_owned_subs() {
        // a is Player-owned with 5 idle; b is Neutral but happens to have 5 idle Player ships
        // garrisoned on it (e.g. just arrived, pre-capture). Structure-wide export for Player
        // must only draw from the owned sub a.
        let mut st = Interior::new(8);
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
        let took = st.take_idle_ships_structwide(Faction::Player, FractionBucket::All, 0);
        assert_eq!(took, 5);
        assert_eq!(st.idle_count_at(a, Faction::Player), 0);
        assert_eq!(st.idle_count_at(b, Faction::Player), 5, "idle ships on an unowned sub are not exported");
    }
}

#[cfg(test)]
mod intercept_tests {
    use super::*;
    use std::f32::consts::TAU;

    /// Brute-force earliest intercept of a circular target (fine time scan) — the oracle.
    fn brute_circular(p: Vec2, v: f32, c: Vec2, r: f32, phase: f32, omega: f32) -> f32 {
        let mut t = 0.0f32;
        while t < 10_000.0 {
            let a = phase + omega * t;
            let a = a.rem_euclid(std::f32::consts::TAU);
            let (sin, cos) = sincos_tau(a);
            let tp = Vec2::new(c.x + r * cos, c.y + r * sin);
            if p.dist(tp) <= v * t + 1e-3 {
                return t;
            }
            t += 0.005;
        }
        f32::INFINITY
    }

    #[test]
    fn linear_intercept_is_exact() {
        // Crossing target: |(10, t)| = 2t  =>  t = sqrt(100/3).
        let t = intercept_linear(Vec2::new(0.0, 0.0), 2.0, Vec2::new(10.0, 0.0), Vec2::new(0.0, 1.0))
            .expect("catchable");
        assert!((t - (100.0f32 / 3.0).sqrt()).abs() < 1e-3, "got {t}");
        // A faster target running straight away is never caught.
        assert!(intercept_linear(Vec2::new(0.0, 0.0), 2.0, Vec2::new(10.0, 0.0), Vec2::new(3.0, 0.0))
            .is_none());
    }

    #[test]
    fn linear_intercept_picks_the_earliest_root() {
        // Head-on faster target: meets at t = 2.5 (closing) and t = 5 (overrun) — take 2.5.
        let t = intercept_linear(Vec2::new(0.0, 0.0), 1.0, Vec2::new(10.0, 0.0), Vec2::new(-3.0, 0.0))
            .expect("catchable");
        assert!((t - 2.5).abs() < 1e-4, "got {t}");
    }

    #[test]
    fn circular_exact_cases() {
        // Static (omega = 0): plain distance / speed.
        let t = intercept_circular(Vec2::new(0.0, 0.0), 2.0, Vec2::new(30.0, 0.0), 10.0, 0.0, 0.0);
        assert!((t - 20.0).abs() < 1e-4, "static: got {t}");
        // From the orbit centre: constant range radius.
        let t = intercept_circular(Vec2::new(5.0, 5.0), 2.0, Vec2::new(5.0, 5.0), 12.0, 1.0, 0.02);
        assert!((t - 6.0).abs() < 1e-4, "centre: got {t}");
    }

    #[test]
    fn circular_matches_the_brute_oracle() {
        // The FFA shape: R = 72 ring, slow orbit, pursuer outside.
        let (p, v, c, r, om) = (Vec2::new(100.0, 40.0), 1.4, Vec2::new(0.0, 0.0), 72.0, TAU / 1500.0);
        for k in 0..8 {
            let phase = k as f32 * TAU / 8.0;
            let got = intercept_circular(p, v, c, r, phase, om);
            let want = brute_circular(p, v, c, r, phase, om);
            assert!((got - want).abs() < 0.5, "phase {phase}: got {got}, oracle {want}");
        }
    }

    #[test]
    fn circular_picks_the_earliest_window_of_many() {
        // A target much faster than the pursuer (omega*r = 10 >> v): range sweeps in and out,
        // so the meeting condition has many roots — the solver must take the first window.
        let (p, v, c, r, om) = (Vec2::new(30.0, 0.0), 0.5, Vec2::new(0.0, 0.0), 10.0, 1.0);
        let period = TAU / om;
        for k in 0..6 {
            let phase = k as f32 * 1.1;
            let got = intercept_circular(p, v, c, r, phase, om);
            let want = brute_circular(p, v, c, r, phase, om);
            assert!(
                (got - want).abs() < period / 8.0,
                "phase {phase}: got {got}, oracle {want}"
            );
        }
    }
}

#[cfg(test)]
mod orbit_frame_tests {
    use super::*;

    /// Ticks until a player POCKET (a mover with a tight ally clump — cohesion live) staged
    /// on the STORAGE ring first reaches an enemy placed `gap` radians away (sign = spin
    /// direction). Pins the v4 frame-consistency fix (owner bug, 2026-07-08): foe deltas and
    /// cohesion windows were measured against the SLOT angle while the bearings themselves
    /// come from POSITIONS, which trail their slots by the steady glide lag under the shared
    /// spin (≈ orbit_rate/orbit_glide rad). The lag exceeded the clump's spread, so the
    /// whole clump read as "behind" — a constant retrograde cohesion shove that made
    /// prograde and retrograde chases close at very different speeds. Frames now agree
    /// (position bearings on both sides), so the lag cancels and the chase is symmetric.
    fn staged_pocket_meet_ticks(gap: f32) -> u64 {
        let params = SimParams::default(); // jitter 0: deterministic, no RNG in the orbit
        let mut st = Interior::new(11);
        st.add_sub(SubStructure::new(Vec2::new(-60.0, 0.0), 0.0, Faction::Player));
        st.add_sub(SubStructure::new(Vec2::new(60.0, 0.0), 0.0, Faction::Ai(0)));
        let stg = st.add_storage_sub();
        let base = 1.0f32;
        // The pocket: 9 player ships in a mirror-symmetric spread; one enemy at ±gap.
        let mut pocket = Vec::new();
        for i in 0..9i32 {
            let id = st.spawn_ship(Faction::Player, stg);
            pocket.push((id, base + (i - 4) as f32 * 0.0125));
        }
        let e = st.spawn_ship(Faction::Ai(0), stg);
        pocket.push((e, base + gap));
        for (id, ang) in pocket.iter().copied() {
            st.ships[id].angle = ang.rem_euclid(std::f32::consts::TAU);
            st.ships[id].ring_offset = 0.0;
            st.ships[id].ring_drift = 0.0;
            st.ships[id].pos = st.ring_pos(stg, st.ships[id].angle, 0.0);
        }
        for t in 0..40_000u64 {
            if !st.ships[e].alive {
                return t; // combat resolved it — contact happened already
            }
            let met = pocket[..9].iter().any(|&(id, _)| {
                st.ships[id].alive
                    && st.ships[id].pos.dist(st.ships[e].pos) <= params.engagement_radius
            });
            if met {
                return t;
            }
            st.step(&params);
        }
        panic!("the pocket never reached the foe (gap {gap})");
    }

    #[test]
    fn storage_drive_is_spin_symmetric() {
        let with_spin = staged_pocket_meet_ticks(0.8);
        let against_spin = staged_pocket_meet_ticks(-0.8);
        let diff = with_spin.abs_diff(against_spin);
        let base = with_spin.max(against_spin).max(1);
        assert!(
            (diff as f64) / (base as f64) < 0.15,
            "prograde {with_spin} vs retrograde {against_spin} ticks — spin-asymmetric drive"
        );
    }
}

#[cfg(test)]
mod sincos_tests {
    use super::sincos_tau;

    /// The paired fast sincos must agree with libm across the whole pre-reduced domain —
    /// the cross-platform replay contract rides on both being deterministic, and this pins
    /// their AGREEMENT so a coefficient typo or fold bug cannot silently bend the sim's
    /// geometry. Sweeps [0, τ] inclusive of the τ-spill rounding edge.
    #[test]
    fn sincos_tau_matches_libm_across_the_ring() {
        let tau = std::f32::consts::TAU;
        let mut worst = 0.0f32;
        let mut a = 0.0f32;
        while a < tau + 1e-4 {
            let (s, c) = sincos_tau(a);
            worst = worst
                .max((s - libm::sinf(a)).abs())
                .max((c - libm::cosf(a)).abs());
            a += 1e-4;
        }
        assert!(worst < 3e-7, "sincos_tau drifted from libm: worst delta = {worst}");
    }
}
