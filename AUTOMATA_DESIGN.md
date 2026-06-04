# AUTOMATA_DESIGN.md — automatons under the new resistance / denial / soft-cap mechanics

Status: **IMPLEMENTED + TUNED.** The capture / denial / soft-cap mechanics (§1), the event-driven
forward-projection (§2, now `crates/world/src/projection.rs` — *event-driven*, superseding the
§2C tick loop) and its composable query vocabulary, the new sim read-signals (§5), and the four
automatons (§3) — all built as compositions over `ai::vocab` (predicates + actions + projection
queries) — are **done and validated**. The rock-paper-scissors cycle **CLOSES on the diamond over
both seatings and multiple seeds** (measured numbers in §4). The final tuned constants (mechanic +
policy) are recorded in §1D and §6; see also `AI.md` for the measured win-loss matrix and the
AI-in-loop profile.

> **Tuning headline (this pass).** Keeping `DEFAULT_MAX_RESISTANCE = 1800` (the owner's Solarmax
> pace), the cycle was closed by tuning **policy** dials + the projection/match **horizons** and a
> single **soft-cap** dial:
> - `softcap_attrition`: `1.0 → 0.5` (the one mechanic dial changed — a gentler anti-hoard bleed so
>   a turtle can hold a real standing wall and out-last an over-committed aggressor's mobile hoard;
>   `DEFAULT_MAX_RESISTANCE`, `softcap_free=20`, `softcap_per_sub=10` are **unchanged**).
> - `DEFAULT_PROJECTION_HORIZON`: `240 → 2000` (the AI look-ahead must span a full ~`1800/force`
>   grind, else the marginal-capture queries read 0 and the colonizers never commit).
> - harness `DEFAULT_HORIZON`: `1200 → 3000` (captures take ~`max_resistance/force` ticks, so a
>   match resolves over thousands of ticks; a shorter horizon cuts it off mid-grind).
> - `AttackParams::fight_efficiency`: `10 → 2` (a high bar made Attack hoard a mobile stack it never
>   committed — which trivially out-massed the turtle and collapsed the RPS into "attack dominates";
>   a modest bar makes Attack COMMIT its siege, where a correct Defender punishes it).
> - `SimpleColonizerParams::ships_per_res`: `0.12 → 0.02` + send-threshold `= min_wave` (the old
>   value was sized for the retired `max_resistance≈100` and at 1800 demanded ~216 ships before
>   sending anything — SimpleColonize drew with a passive enemy until fixed).
> - `DefendParams`: added a parity-gated, weakest-foe **counter-punch** (keep the over-cap surplus
>   moving into an over-committed attacker's emptied rear instead of bleeding it to the cap) and a
>   present-majority gate on contested-position reinforcement.
>
> This document remains authoritative for the *intended* behaviour; the per-line code references
> (`Structure::resolve_resistance`, `Structure::produce`, `Structure::resolve_softcap`,
> `World::project_forward`, `ai::automata`, `ai::vocab`) are accurate as of this pass.

This file changes no code. It only references the real code under `crates/layer1`, `crates/world`,
and `crates/ai` so the API names below are accurate as of writing.

Contents:
1. [The new mechanics + exact formulas/constants](#1-the-new-mechanics--exact-formulasconstants)
2. [The unified forward-projection API](#2-the-unified-forward-projection-api)
3. [The four automatons](#3-the-four-automatons) — SimpleColonizer, Colonize, Attack, Defend
4. [Expected RPS-cycle dynamics under the new mechanics](#4-expected-rps-cycle-dynamics)
5. [New sim signals / APIs the implementation must add](#5-new-sim-signals--apis-to-add)
6. [Balance risks + constants to tune first](#6-balance-risks--constants-to-tune-first)

---

## 1. The new mechanics + exact formulas/constants

These three mechanics **replace** the old instant-capture + uncapped-stack model. All three are
folded into `Structure::step`, whose tick order is **fixed for determinism**
(`crates/layer1/src/sim.rs`, `Structure::step`):

```
1. produce()            // gated by denial (Mechanic B)
2. advance_movement()   // moving ships step toward aim; arrivals become idle (home = target)
3. resolve_combat()     // stochastic square-law, combat_substeps rounds (UNCHANGED)
4. resolve_resistance() // capture grind / heal / flip (Mechanic A)
5. resolve_softcap()    // anti-hoard attrition (Mechanic C)
tick += 1
```

Two consequences of this order matter to every automaton:
- **Combat resolves BEFORE resistance.** A defender's ships must survive combat to count as
  "present" for the heal; an attacker's erosion uses its *post-combat* present count. So clearing
  the firefight is a precondition for any capture progress.
- **Resistance uses post-movement presence.** A ship that arrives this tick *is* inside the radius
  when `resolve_resistance` runs, so it contributes to erosion/heal on its arrival tick.

Determinism is preserved end-to-end: resistance (`resistance`, `max_resistance`) is folded into
`Structure::state_hash`, and soft-cap random destruction draws from the structure's seeded
`Rng` (so the draw position is part of the fingerprint via the RNG-position fold). The projection
in §2 reads state and draws **no** randomness, so calling it never perturbs the hash.

### 1A. Resistance capture (the grind)

Every sub-structure (`SubStructure`, `crates/layer1/src/sim.rs`) carries:
- `resistance: f32` in `[0, max_resistance]`, **starts at `max_resistance`**;
- `max_resistance: f32`, default `DEFAULT_MAX_RESISTANCE = 100.0`, **tunable per sub** via
  `SubStructure::with_max_resistance(max)` (clamped to `>= 1.0`).

Each tick, with `P = presence_in_sub(sub, Player)` and `E = presence_in_sub(sub, Enemy)` (living
ships physically inside the sub radius, `presence_in_sub` already exists), let the **single present
faction** be:

```
single = match (P > 0, E > 0) {
    (true,  false) => Some((Player, P)),
    (false, true ) => Some((Enemy,  E)),
    _              => None,        // zero present OR both present  => FROZEN (no change)
}
```

Then exactly as `Structure::resolve_resistance` does:

```
if single is None:                      // frozen
    (no change to owner or resistance)
else let (f, count) = single:
    if f == owner:                      // (b) owner present, uncontested -> HEAL
        resistance = min(resistance + count, max_resistance)
    else:                               // (a) one foreign faction, uncontested -> ERODE
        resistance -= count             // (neutral-owned subs always erode: no ship is Neutral)
        if resistance <= 0:             // FLIP + REFILL
            owner = f
            resistance = max_resistance
```

Consequences (the design intent):
- Capture is a **grind**: clearing a fresh sub (resistance 100) with `F` present attackers takes
  `ceil(100 / F)` ticks — **more ships ⇒ faster** (a linear, not square, speedup *on the grind
  itself*; the square law lives only in combat).
- Progress requires you to have **cleared the enemy ships there** — erosion only advances while you
  are the *only* faction present. While both are present it is frozen; combat (step 3) decides who
  becomes "the only side present".
- A **returning defender HEALS** it, so a hit-and-run accomplishes nothing — an attacker must keep
  enough present force to out-erode the heal until the bar hits 0.
- Garrisoned ships are untouched by capture — a flipped sub keeps whatever ships sit on it; they
  just have a new landlord.

### 1B. Production denial

`Structure::produce` is **gated**: an owned sub produces on its cadence
(`SimParams::production_period`, default `18`) **unless it is being actively eroded** —
defined (start-of-tick presence) as:

```
owner_here   = presence_in_sub(sub, owner) > 0
foe_here     = presence_in_sub(sub, owner.opponent()) > 0
being_eroded = foe_here && !owner_here          // exactly one foreign faction, owner absent
```

If `being_eroded`, the sub **does not produce** and its `production_timer` is **held steady** (the
choke is a pure denial, not a timer that "catches up" the instant pressure lifts). A
**contested-but-defended** sub (`owner_here && foe_here`) **keeps producing** — defenders keep the
line running. A neutral sub never produces.

Design intent: **parking on an enemy sub STARVES its output even before you capture it.** Denial
value is real economic damage available at lower cost than full capture.

### 1C. Soft caps (anti-hoard)

Two distinct caps. Per-sub orbit cap is positional only; the per-structure cap is the load-bearing
attrition.

**Per-sub orbit cap — `SimParams::sub_orbit_cap`, default `50` — POSITIONAL ONLY, NEVER destroys.**
When more than this many of a faction's ships would idle at one sub, the overflow is *placed* at a
wider "structure orbit" radius so a single sub is not an infinitely dense dot. It never destroys
ships and never affects combat/capture/determinism beyond spawn positions. (In the current code
this is documented as a renderer concern and is **not enforced** inside `resolve_softcap`; if/when
enforced it must remain a pure relocation that does not draw RNG. The §3 automatons treat
"idle beyond the orbit cap relocates to outer orbit" as positional and ignore it for capture math,
but Colonize/Attack use it as an over-stack signal — see §3.)

**Per-structure soft cap — SOFT, NO hard ceiling for play purposes.** Per real faction, with
`prod = sub_count(faction)` (owned subs in this structure):

```
soft = softcap_free + softcap_per_sub * prod
     = 20 + 10 * prod                          // defaults: softcap_free=20, softcap_per_sub=10
parked = count of living ships of faction IN THIS STRUCTURE (idle OR intra-structure transit)
         // inter-planet fleets live in `world`, NOT in a Structure, so they are EXEMPT.
if parked > soft:
    over      = parked - soft
    soft_kill = ceil(softcap_attrition * sqrt(over))     // softcap_attrition default 1.0
    hard_kill = parked.saturating_sub(structure_hard_cap) // structure_hard_cap default 1000
    n = max(soft_kill, hard_kill).min(parked)
    destroy n parked ships at random (idle preferred over in-transit), via the structure RNG
```

Notes:
- The **structure production** is the cumulative production of *all* that faction's owned subs in
  it = owned-sub count (1 ship/period each). The soft cap target `softcap_per_sub * prod` with the
  default `10` means **equilibrium surplus ≈ 10× production** (10 ships of headroom per owned sub),
  exactly the "surplus ≈ 10× production" target.
- The `sqrt(over)` shape makes the cap a **self-limiting plateau**, not a wall: small overshoots
  bleed slowly, large ones faster, and the count settles just above `soft` (the unit test
  `softcap_plateaus_hoard_and_spares_control` confirms plateau ≈ `soft ± 1`).
- `structure_hard_cap = 1000` is a far-above-play safety bound, **not** a strategic ceiling. For
  design purposes there is **NO hard cap**; do not build automatons that rely on one.
- **Surplus must be SPENT or kept MOVING.** Inter-planet transit (the `world` crate's
  `InterFleet`) is exempt, so a mobile force bypasses the cap; a parked mega-stack does not. This is
  the lever that prevents turtling on a single mountain of ships.

### 1D. Constant reference (defaults; all tunable)

| Constant | Where | Default | Meaning |
|---|---|---|---|
| `DEFAULT_MAX_RESISTANCE` | `layer1::sim` const | **`1800.0`** | fresh-sub resistance (= start = refill); the owner's Solarmax pace |
| `max_resistance` | per `SubStructure` | `1800.0` | per-sub cap, `with_max_resistance` (per-level override; e.g. L6's prize uses `600`) |
| `production_period` | `SimParams` | `18` | ticks between an owned sub's spawns |
| `ship_speed` | `SimParams` | `1.4` | intra-structure ship speed (units/tick) |
| `arrival_tolerance` | `SimParams` | `0.75` | "arrived" radius |
| `engagement_radius` | `SimParams` | `7.0` | battle-bubble radius `R` |
| `defender_fire_bonus` | `SimParams` | `0.012` | additive fire-prob inside own sub |
| `softcap_free` | `SimParams` | `20` | flat parked allowance per faction/structure |
| `softcap_per_sub` | `SimParams` | `10` | parked allowance added per owned sub (≈ 10× prod) |
| `softcap_attrition` | `SimParams` | **`0.5`** | `ceil(this * sqrt(over))` ships killed/tick (tuned down from `1.0`: see §6 / §4 — the lever that lets a turtle hold a wall and so beat an over-committed attacker) |
| `structure_hard_cap` | `SimParams` | `1000` | safety ceiling, not a play dial |
| `sub_orbit_cap` | `SimParams` | `50` | positional orbit overflow (never destroys) |
| `undock_ticks` | `WorldParams` | `6` | fleet undock delay before transit |
| `transit_speed` | `WorldParams` | `1.4` | lane units/tick once transiting |
| `keep_floor` | `WorldParams` | `2` | per-sub idle garrison kept on export |
| `DEFAULT_PROJECTION_HORIZON` | `world::projection` | **`2000`** | AI forecast depth (policy/forecast dial, not a mechanic); spans a full grind so the marginal-capture queries are non-zero |
| `DEFAULT_HORIZON` | `ai::harness` | **`3000`** | match horizon for the validation worlds; long enough for the grind to resolve |

---

## 2. The unified forward-projection API

All four automatons share **one** read-only, deterministic, **enemy-ignoring** look-ahead. It must
live in the `world` crate (or a `world::projection` module) because it has to reason over **both**
intra-structure moving ships *and* inter-planet `InterFleet`s. It is a **pure read** of
`(&World, &SimParams, &WorldParams)` — it never mutates and never touches a `Structure`'s RNG, so
it is safe to call mid-decision and does not perturb `state_hash`.

### 2A. The question it answers

> For each sub-structure (planet `p`, sub `s`): if **no new orders are issued** and **the enemy
> does nothing**, considering **only ships already in transit** (intra-structure moves AND
> inter-planet fleets) plus the resistance grind/heal/flip those arrivals drive, when (if ever)
> over the next `horizon` ticks does the sub's owner change, and to whom?

It **ignores by construction**: combat (nobody shoots — present ships never die), production
spawns, soft-cap attrition, and any not-yet-issued order. It models exactly the two events already
locked in: (1) in-transit arrivals, and (2) the per-tick resistance rule those arrivals drive,
reproduced **bit-for-bit** from `Structure::resolve_resistance` (ideally by both calling one shared
pure `capture_step` — see §5 signal 5 — so the projection can never drift from the sim).

### 2B. The one signature every automaton uses

```rust
// In world (e.g. crates/world/src/projection.rs), plus a thin World::project_forward wrapper.

pub const DEFAULT_PROJECTION_HORIZON: u64 = 240; // ~ a couple production periods + a transit+grind

pub struct Projection {
    pub horizon: u64,
    pub base_tick: u64,          // == world.tick at call time; all etas are ABSOLUTE world ticks
    // internally: one SubFate per (planet, sub), O(1) lookup via a per-planet base-index offset.
}

/// Outcome for one sub over the horizon.
pub struct SubFate {
    pub current_owner: Faction,
    pub eta_first_change: Option<u64>,   // ABSOLUTE tick of the FIRST owner change; None = none
    pub owner_after_first_change: Option<Faction>,
    pub owner_at_horizon: Faction,       // owner at base_tick + horizon (== current if no change)
    pub resistance_at_horizon: f32,
    pub became_contested: bool,          // at some tick >=2 factions present (grind froze): eta is a LOWER bound
    pub flips_again: bool,               // a 2nd owner change happens after the first (downgrade confidence)
}

impl Projection {
    /// O(1) fate of a sub. Out-of-range -> a trivial "unchanged" fate.
    pub fn sub_fate(&self, planet: PlanetId, sub: SubId) -> &SubFate;

    /// Convenience derived views (computed from the per-sub fates; see notes):
    /// Who captures/frees this sub first and at what ABSOLUTE tick, if within the horizon.
    pub fn sub_capture(&self, planet: PlanetId, sub: SubId) -> Option<(Faction, u64)>;
    /// Planet-level roll-up: the planet "flips" to a faction when its LAST foreign sub flips.
    /// Returns the faction the planet ends the horizon owned-by and the tick that completes, if any.
    pub fn planet_capture(&self, planet: PlanetId) -> Option<(Faction, u64)>;
    /// Of `faction`'s in-transit ships, how many are PROJECTED to be present inside this sub by
    /// the horizon (sum of scheduled arrivals of `faction` into `sub`). Lets a caller avoid
    /// double-sending to a sub its own in-flight force already settles.
    pub fn incoming_present_at(&self, planet: PlanetId, sub: SubId, faction: Faction) -> u32;
    /// Returning-defender present-force the projection expects at this sub (in-flight ships of the
    /// sub's current owner). Used by Attack to size a heal-outlasting hold.
    pub fn returning_owner_force(&self, planet: PlanetId, sub: SubId) -> u32;
}

impl World {
    pub fn project_forward(&self, sp: &SimParams, wp: &WorldParams, horizon: u64) -> Projection;
}
```

Callers that reason at **planet** granularity (Layer-2 strategies) use `planet_capture`; callers at
**sub** granularity (Layer-1 / interior) use `sub_fate` / `sub_capture`. Both are the *same*
projection object, just different roll-ups — this is the reconciliation across the five original
designs (which variously called it `project_capture_eta`, `project_forward`, `proj.fate_of`,
`proj.sub_or_planet_capture`, `Projection::sub_fall_eta`, etc.). **There is one API; those are now
all methods on `Projection`.**

### 2C. Algorithm (cheap, deterministic)

```
project_forward(world, sp, wp, horizon):
  base = world.tick
  for each (planet p, sub s):
      initial_presence[p][s][f] = idle_presence_in_sub(p.structure, s, f)   // IDLE only (signal 1)
      // idle-only avoids double-counting a still-inside moving ship that is also a scheduled arrival.
  // Schedule arrivals (presence deltas), bucketed by (planet, sub, tick, faction):
  for each intra-structure moving ship sh (target = Some(tgt)) in each planet p:
      eta = ceil( max(0, dist(sh.pos, sh.aim) - sp.arrival_tolerance) / max(sp.ship_speed, EPS) )
      schedule +1 at (p, tgt, base + eta, sh.faction)        // arrival counts on its arrival tick
  for each InterFleet f (real faction) in world.fleets:
      ticks_left = fleet_arrival_ticks(world, wp, f)         // signal 3 (undock + ceil((1-progress)/dprog))
      entry = world.entry_sub(f.to, f.from, f.faction)       // signal 2 (promote to pub); None -> drop
      schedule +f.count at (f.to, entry, base + ticks_left + 1, f.faction)
      // +1: World::step injects at END of tick, so injected ships are first present the NEXT tick.
  // Integrate each (p,s) independently over (base+1 ..= base+horizon):
  for each (p, s):
      owner, resist, maxr = current sub state ; pres[Player], pres[Enemy] = initial_presence
      for t in base+1 ..= base+horizon:
          apply all arrivals scheduled at tick t (presence only grows; enemy "does nothing")
          (owner, resist, flipped) = capture_step(owner, resist, maxr, pres[Player], pres[Enemy])
          record first flip tick/owner, contested-ever, second-flip, etc.
      emit SubFate
```

Cost is `O(ships)` scheduling + `O(total_subs * horizon)` integration — a few thousand cheap
integer-ish iterations per call at current map sizes. (An event-driven integrator that jumps
between scheduled arrivals — closed-form `ticks_to_flip = ceil(resist / present_count)` while a
single faction is present — can drop it to `O(arrivals)` if a profile ever demands it; left out for
reviewability. Flagged in §6.)

### 2D. What the projection is blind to (every caller must respect)

It is deliberately wrong wherever fights happen, so its numbers are **optimistic/loose bounds**:
- **No enemy and no new orders:** any enemy reinforcement/retreat/counter-capture is invisible.
- **No combat:** present ships never die, so an attacker's present count (which drives erosion) is
  an **upper** bound — real grind is slower while the defender's ships still live; and it never
  models the attacker being cleared.
- **Freezes on contest** exactly like the sim, so for any sub where both sides are projected
  present the ETA is only a **lower** bound (flagged by `became_contested`).
- **No production / no soft-cap attrition:** long-horizon "still neutral / still mine" can be wrong
  once production reinforces, and a hoard it over-counts would be trimmed by the sim.
- **First flip only** (with `flips_again`): a flip→heal→flip-back is summarized, not fully traced.

**Mitigation contract:** callers treat `eta` as a bound (not a promise) when `became_contested` or
`flips_again` is set, and **re-project every decision tick** rather than trusting a stale plan.

---

## 3. The four automatons

All four are reconciled to:
- emit the existing concrete orders (`world::FleetOrder` at Layer 2 via `next_hop` first-hop
  routing; `layer1::MoveOrder` at Layer 1), converting a ship count to a `FractionBucket` via the
  existing `ai::bucket_for(want, available)`;
- keep a garrison floor `GARRISON_FLOOR = wp.keep_floor = 2` (matches `GreedyParams::garrison_floor`
  and `WorldParams::keep_floor`, so the policy never plans to move ships the launch primitive would
  refuse to release);
- call the **one** `World::project_forward(...)` from §2 once per decision tick and share the
  result;
- read the **same** new signals from §5 (no automaton hand-rolls a capture/landing rule).

Two layer conventions, both already in the codebase:
- **Layer 2** ("structure" = a planet): a sibling of the existing pure policies in
  `crates/ai/src/strategy.rs` (`colonize`/`defend`/`attack`); export only from
  `PlanetAggregate::fully_owned_uncontested(seat)` planets; route multi-lane moves first-hop via
  `graph::next_hop`.
- **Layer 1** (one structure, "positions" = subs): the abstract `PositionView` seam in
  `crates/ai/src/greedy.rs` with the `Layer1View` adapter; distance is Euclidean; every sub is
  reachable.

### 3.1 SimpleColonizer — the early-campaign everyman

**Identity.** A reactive colonizer that sizes each capture wave **proportional to the target's
total cumulative sub resistance** and fills colonization orders **nearest-first**. Deliberately
ignores transit cost (sizes by resistance only). Keeps the greedy retreat reflex and a flat
garrison floor, and keeps the documented seam: **no dedicated rear guard.**

**Blind spot (THE seam).** Every non-contested planet keeps only `GARRISON_FLOOR`; all surplus
ships toward the nearest objective the tick it appears, with nothing reserved for next tick. An
exposed-but-quiet rear sits at the floor while production streams forward — exploitable exactly like
the existing `the_seam_no_rear_guard_above_the_floor` test: hold a detachment back, strike the thin
rear, and (because a captured rear keeps producing, and a *parked* attacker already denies the
target's output under Mechanic B) the flank snowballs faster than the colonizer's push. Secondary
accepted weaknesses: it never trades distance against value; it always tries to fully capture rather
than sometimes just *denying*; and it has no plan to keep surplus moving, so on a quiet board it can
bleed to soft-cap attrition.

**Constants.**
```
GARRISON_FLOOR        = wp.keep_floor      // 2
SHIPS_PER_RESISTANCE  = 0.12               // wave size ~= this * total foreign resistance
MIN_WAVE              = 3
HORIZON               = DEFAULT_PROJECTION_HORIZON  // 240
```

**Per-decision pseudocode (Layer 2 primary form).**
```
fn simple_colonize(world, seat, wp, sp) -> Vec<FleetOrder>:
    enemy = seat.opponent()
    proj  = world.project_forward(sp, wp, HORIZON)         // shared
    orders = []
    spent = {}                                             // planets that retreated this tick

    // (0) REACTIVE RETREAT (greedy rule 1): a contested, OUTNUMBERED owned planet ships its
    //     surplus to the nearest SAFE owned planet (owned by me, not contested). No rear guard.
    for from in exportable_planets(world, seat, wp):       // ascending id (existing helper)
        a = world.planet_aggregate(from)
        if a.owner == Contested and a.ships_of(enemy) > a.ships_of(seat):
            safe = nearest_planet(world, from, |t| owned_by(t,seat) and t.owner != Contested)
            if safe and next_hop(world, from, safe):
                orders.push(FleetOrder::new(from, hop, ThreeQuarter)); spent.insert(from)

    // (1) BUILD TARGETS: neutral, OR enemy-held, OR contested-with-enemy-presence; reachable.
    //     PROJECTION GATE: skip a target the projection already settles in my favor; skip a target
    //     the projection says the enemy captures before I could land AND I have nothing committed.
    targets = []
    for t in 0..world.planets.len():
        at = world.planet_aggregate(t)
        is_obj = at.owner == Neutral or owned_by(t, enemy)
                 or (at.owner == Contested and at.ships_of(enemy) > 0)
        if not is_obj or not any_owned_can_reach(world, seat, t): continue
        if proj.planet_capture(t) == Some((seat, _)): continue        // already settling mine
        // WAVE SIZE = proportional to TOTAL CUMULATIVE FOREIGN sub resistance (signal 5):
        total_res = world.planet_total_resistance_vs(t, seat)         // sum of s.resistance, s.owner != seat
        want = max(MIN_WAVE, ceil(SHIPS_PER_RESISTANCE * total_res))  // NEVER depends on distance
        targets.push({ id: t, want })

    // (2) FILL NEAREST-FIRST. Sweep sources ascending id; each pours surplus into the nearest
    //     unfilled target it can reach. Per-tick best-effort (no multi-tick reservation).
    need = { t.id: t.want for t in targets }
    for from in exportable_planets(world, seat, wp):
        if from in spent: continue
        avail = exportable_surplus(world, from, seat, wp.keep_floor)  // idle - floor (existing)
        if avail == 0: continue
        cands = [t in targets where need[t.id] > 0 and path_len(world, from, t.id) is Some]
        sort cands by (path_len asc, id asc)
        for t in cands:
            if avail == 0: break
            send = min(avail, need[t.id]); hop = next_hop(world, from, t.id)
            if hop is None: continue
            frac = bucket_for(send, avail)
            if frac:
                orders.push(FleetOrder::new(from, hop, frac))
                released = frac.count_of(avail)
                need[t.id] = sat_sub(need[t.id], released); avail = sat_sub(avail, released)
    return orders
```

**Layer-1 variant.** Identical onto `Layer1View` (positions = subs, distance = Euclidean): the only
change vs the existing greedy expand rule is the wave sizing — per target sub
`want = max(MIN_WAVE, ceil(SHIPS_PER_RESISTANCE * sub.resistance))`, filled nearest-first from each
owning sub's surplus. Retreat + floor + seam are the greedy defaults already present.

### 3.2 Colonize — resistance-optimized, wave-sized expansion

**Identity.** **Fastest power-base growth.** Under the new model "growth speed" = *how soon captures
flip* (a sub only produces once owned, and erosion rewards present count), so Colonize maximizes the
rate at which neutral resistance becomes owned production. It commits a **sweet-spot wave** per
target and runs a few targets in parallel, using the projection to avoid wasted waves.

The concentrate-vs-parallelize **sweet spot**, derived:
- Flip time on resistance `r` with `w` present attackers, defender heal `d`:
  `T(w) ≈ r / max(w - d, 1)`.
- Marginal time saved by one more ship: `dT ≈ r / ((w-d)(w+1-d)) ~ r / w²` (steeply diminishing).
- Marginal cost of one more ship ≈ a roughly constant number of ticks `C` (one more ship-period to
  accumulate, or one hop farther of travel).
- Optimum where marginal gain == marginal cost: `r/w² ≈ C ⇒ w* ≈ sqrt(r / C)`.
- With `r ≤ 100` and `C ≈ 6`, `w* ≈ 4..13` — a **handful per wave**, not a mega-stack (wasted
  transit) and not a 1-ship trickle (leaves huge time on the table, and a healing defender
  out-repairs it).

**Blind spot.** **Thin defense — loses to a timed Attack.** It ships everything above
`GARRISON_FLOOR` toward fronts and keeps no rear guard, so a freshly flipped, production-fat colony
is held only by the floor. Three exposures: (1) under denial an attacker need only PARK on the new
colony to choke the very output Colonize exists to compound; (2) the projection ignores the enemy,
so a strike landing after the window hits an undefended fat planet; (3) concentrating waves forward
makes the rear thinner than a trickle-everywhere colonizer, sharpening the seam. This is the
intended **attack-beats-colonize** edge.

**Constants (bundle as `ColonizeParams`).**
```
GARRISON_FLOOR           = 2
HEAL_MARGIN              = 1        // net erosion must beat heal: need w >= d + HEAL_MARGIN
MARGINAL_SHIP_COST_TICKS = 6.0     // C in w* = sqrt(r/C); ~ production_period(18)/3 producers; tune 3..10
WAVE_MIN                 = 4
WAVE_MAX                 = 16
MAX_CONCURRENT_GRABS     = 3       // parallel fronts; beyond this, reinforce the front-runner
OVERSTACK_GUARD_FRACTION = 0.8     // don't ship into a position already >= 0.8 * its soft cap
HORIZON                  = 240
```

**Per-decision pseudocode (runs over the `PositionView` seam — both layers).**
```
fn colonize_decide(view, P) -> Vec<GreedyAction>:
    if view.len() == 0: return []
    proj = view.project_forward(HORIZON)               // shared projection, via the view (signal 4)
    actions = []

    // (1) Candidate colony targets = capturable NEUTRAL with no enemy present. (Enemy-owned is
    //     Attack's job; that is the clean identity / blind spot.) Prune with the projection.
    candidates = [ t for t in 0..view.len()
                     if view.info(t).owner == Neutral and view.present_count(t, Enemy) == 0 ]
    candidates = [ t in candidates
                     if proj.sub_fate(t).owner_after_first_change != Some(ME)        // not already mine in-flight
                     and not (proj.sub_capture(t) is Some((ENEMY, _))) ]             // enemy beats me there

    // (2) Score each by EXPECTED FLIP TIME if I commit a sweet-spot wave (sooner is better;
    //     tie-break nearest then id). Subtract help already in transit so we don't double-send.
    scored = []
    for t in candidates:
        r   = view.resistance(t)                       // signal 5
        d   = view.present_count(t, owner_of(t))       // defender heal rate (0 for neutral)
        wst = wave_size_for(r, d, P)                   // see helper
        inflight = proj.incoming_present_at(t, ME)
        need = max(0, wst - inflight)
        eta  = transit_ticks(view, nearest_owned_to(view, t), t) + ceil(r / max(inflight + wst - d, 1))
        scored.push((t, need, eta))
    sort scored by (eta asc, dist-from-nearest-owned asc, id asc)
    chosen = first MAX_CONCURRENT_GRABS of scored with need > 0
    frontrunner = chosen[0].t if any else None
    demand = { t: need for (t, need, _) in chosen }

    // (3) Each owned position sheds surplus toward the NEAREST chosen target it can serve.
    for from in 0..view.len():
        me = view.info(from)
        if me.owner != ME or not view.can_export_from(from): continue
        surplus = sat_sub(me.my_ships, GARRISON_FLOOR)
        if surplus == 0: continue
        tgt = nearest(view, from, |t| demand.get(t,0) > 0 and view.reachable(from, t))
        if tgt is None:
            // concentration fallback: reinforce the front-runner to flip it sooner (if not overstacked)
            if frontrunner and view.reachable(from, frontrunner) and not would_overstack(view, frontrunner, ME):
                tgt = frontrunner
            else: continue
        want = min(surplus, demand.get(tgt, surplus), WAVE_MAX)
        if want < WAVE_MIN and want < surplus and tgt != frontrunner: continue   // thin source: only help a near-flip
        if would_overstack(view, tgt, ME):                                       // signal 3 (idle vs soft cap)
            tgt2 = nearest(view, from, |t| t != tgt and demand.get(t,0) > 0
                                          and view.reachable(from,t) and not would_overstack(view,t,ME))
            if tgt2 is None: continue
            tgt = tgt2; want = min(surplus, demand.get(tgt, surplus), WAVE_MAX)
        actions.push(GreedyAction{ from, to: tgt, count: want, kind: Expand })
        demand[tgt] = sat_sub(demand[tgt], want)
    return actions
    // adapter (unchanged) folds count -> FractionBucket via bucket_for and emits MoveOrder / first-hop FleetOrder.

fn wave_size_for(r, d, P):
    w_star = ceil(sqrt(r / P.MARGINAL_SHIP_COST_TICKS))
    return clamp(max(w_star, d + HEAL_MARGIN), WAVE_MIN, WAVE_MAX)

fn would_overstack(view, t, faction):
    return view.idle_at(t, faction) >= OVERSTACK_GUARD_FRACTION * view.soft_cap_at(t, faction)   // signal 3
```

### 3.3 Attack — grind-and-hold siege (concentrate, deny, then sustain)

**Identity.** Concentrate force, strike the **soft, productive** target, and **HOLD it through the
grind** — an attack is a SIEGE, not a raid (the defender HEALS, so hit-and-run is worthless). Three
ideas the old greedy assault lacked, all forced by the new mechanics: (1) **siege math** — commit a
force that wins the firefight *and* out-erodes the returning defender's heal, and sustain it; (2)
**denial value** — park a cheap detachment on a productive enemy sub to freeze its output even
before capture; (3) **projection gating** — don't send ships to a sub that will already be captured
(by me or the enemy) before they arrive, and time the commit so the spearhead lands as a wave.

**Blind spot.** **Over-extension** — to field a heal-outlasting siege it strips its other positions
to `GARRISON_FLOOR` and posts no rear guard. Against a Defender that **survives** the strike (its
garrison + heal-on-hold make the target a tar-pit) and counter-punches, the emptied rear is captured
behind it, and a captured rear keeps producing, so the flank rolls up faster than the stalled siege.
The new mechanics *sharpen* this: an under-committed or hesitant siege achieves nothing (resistance
refills) yet still drained the rear; and soft-cap SPEND pressure can leak ships forward before they
are a decisive wave. This is the intended **defend-beats-attack** edge.

**Constants.**
```
GARRISON_FLOOR        = 2
SIEGE_FIGHT_MARGIN    = 1.30   // need >= 1.3x enemy combat strength to win the firefight (square law)
HEAL_OUTLAST_MARGIN   = 1.25   // post-clear present force must exceed projected returning heal by this
GRIND_HOLD_FLOOR      = 4      // min ships kept INSIDE the radius so a stray returner can't freeze the grind
DENIAL_DETACH         = 6      // detachment parked purely to freeze a productive enemy sub
PROJECTION_HORIZON    = 240    // (use the shared default; covers transit + grind start)
TRANSIT_SLACK         = 6      // safety ticks in arrival-race comparisons
SOFTCAP_SPEND_TRIGGER = 0.80   // if parked >= 0.8 * soft cap, force a spend this tick
MAX_SIEGE_TARGETS     = 1      // concentrate: ONE active capture target (denial parks are extra/cheap)
// target scoring weights (soft & productive & shallow & near = better):
W_PRODUCTION=3.0  W_DEFENCE=1.0  W_RESISTANCE=0.02  W_DISTANCE=0.10
```

**Per-decision pseudocode (over the extended `PositionView`).**
```
fn attack_decide(view, sp, wp) -> Vec<Action>:
    if view.len() == 0: return []
    proj = view.project_forward(PROJECTION_HORIZON)
    plan = plan_siege(view, proj)                       // ONE target + ONE staging; None if no enemy presence
    if plan is None: return colonize_decide(view, ...)  // pre-contact: develop, don't idle

    actions = []
    for from in 0..view.len():                          // ascending id
        me = view.info(from)
        if me.owner != ME or not view.can_export_from(from): continue
        surplus = sat_sub(me.my_ships, GARRISON_FLOOR)
        force_spend = view.parked_ratio(from) >= SOFTCAP_SPEND_TRIGGER   // signal 3

        // (1a) RETREAT a losing local fight (preserve the army).
        if me.contested and me.enemy_ships > me.my_ships:
            to = nearest(view, from, |t| t.owner == ME and not t.contested)
            if to: emit(from -> to, surplus, Retreat); continue
        if surplus == 0 and not force_spend: continue

        // (1c) SPEARHEAD (the staging position): commit ONLY when ready (wins fight + outlasts heal
        //      + arrives in time). Otherwise HOLD and amass. Cap-pressured-but-not-ready leaks toward
        //      the target (transit is cap-exempt) rather than dying parked.
        if from == plan.staging:
            if ready_to_commit(view, proj, plan, from): emit(from -> plan.target, surplus, AssaultCommit)
            elif force_spend:                            emit(from -> plan.target, surplus, AssaultCommit)
            // else HOLD
            continue

        // (1d) SIEGE HOLDER (already grinding the target / a freshly captured sub): sustain. Keep
        //      enough present to outlast heal; release only true overflow.
        if is_active_grind_site(view, proj, from):
            keep = max(GARRISON_FLOOR, GRIND_HOLD_FLOOR,
                       ceil(HEAL_OUTLAST_MARGIN * proj.returning_owner_force(from)))
            releasable = sat_sub(me.my_ships, keep)
            if releasable > 0 and force_spend: emit(from -> plan.staging, releasable, Feed)
            continue

        // (1e) DENIAL DETACH (optional, cheap): park DENIAL_DETACH on a productive enemy sub to
        //      freeze its output (Mechanic B), only if production-superior and it won't dilute the siege.
        deny = pick_denial_target(view, proj, plan)
        if deny and surplus >= DENIAL_DETACH and production_superior(view):
            emit(from -> deny, min(DENIAL_DETACH, surplus), Deny); surplus -= DENIAL_DETACH
            if surplus == 0 and not force_spend: continue

        // (1f) FEEDER: funnel surplus toward staging (mass), fallback to target if staging unreachable.
        to = plan.staging if view.reachable(from, plan.staging)
             else plan.target if view.reachable(from, plan.target) else None
        if to: emit(from -> to, surplus, Feed)
    return actions

fn ready_to_commit(view, proj, plan, here):
    tgt = view.info(plan.target)
    eta_arrive = proj.eta_to_present_for(plan.target, ME)
    cap = proj.sub_capture(plan.target)
    if cap is Some((ENEMY, eta)) and eta + TRANSIT_SLACK < eta_arrive: return false  // they finish first
    if cap is Some((ME, _)):                                            return false  // already mine in-flight
    if here.my_ships < SIEGE_FIGHT_MARGIN * tgt.enemy_combat_ships:     return false  // too thin -> amass
    our_after  = expected_survivors(here.my_ships, tgt.enemy_combat_ships)
    their_heal = proj.returning_owner_force(plan.target)
    if our_after < HEAL_OUTLAST_MARGIN * max(1, their_heal):            return false  // would heal-stall
    return true

fn plan_siege(view, proj):
    // candidate = enemy presence, reachable from an owned position, NOT already projected mine in-flight.
    // cost = W_DEFENCE*enemy_combat + W_RESISTANCE*resistance + W_DISTANCE*nearest_owned_dist
    //        - W_PRODUCTION*producer_subs ; pick argmin (lowest id ties).
    // staging = owned position nearest the target, else lowest-id owned (rally point).
```

### 3.4 Defend — resistance-aware turtle (heal-and-hold, spend the cap surplus, withdraw on first-to-fall)

**Identity.** A defense-first turtle that **stays productive**. It garrisons its own subs so they
HEAL back to max and keep producing; it keeps a defensive reserve up to (not over) the soft cap; and
it spends only the **genuine surplus** (the ships the cap would otherwise destroy) on colonizing — or
attacking when no neutral is left. It snaps loose ships home the instant any owned sub is being
eroded or is **projected to fall**, concentrating the withdrawal on the **single sub that the
projection says falls FIRST** so it reinforces in time rather than spreading thin. A healing,
producing wall outlasts an over-committed stack and counter-punches its emptied rear.

**Blind spot.** **Opportunity cost.** It exports only the cap surplus and keeps `RESERVE_FRACTION`
home healing, so a pure Colonizer (which pours `ThreeQuarter` toward neutrals from tick 0) plants
subs sooner and — since production scales with owned-sub count — compounds past the turtle by the
horizon. Two new-mechanic edges Defend deliberately gives up: (1) **denial** — it never sits a
detachment on an enemy sub purely to choke production; (2) in a stalemate where it finds no winning
local concentration it caps out and plateaus, while a mobile attacker keeps ships in cap-exempt
inter-planet transit and stages a larger effective stack than the turtle will hold. This is the
intended **colonize-beats-defend** edge.

**Constants.**
```
RESERVE_FRACTION      = 0.75          // keep this share of the seat's parked army home, healing
REINFORCE_BUCKET      = Half          // cautious withdrawal trickle (existing const)
DEFEND_PRODUCE_BUCKET = Quarter       // small productive commitment (existing const)
HEAL_FLOOR_PER_SUB    = wp.keep_floor // never draw an owned sub below this present-ship heal floor
SOFTCAP_SPEND_SLACK   = 2             // treat ships as surplus only once parked > soft_cap - this
HORIZON / FALL_SOON   = ceil(longest_lane / wp.transit_speed) + undock + margin
// score tiers: contested/eroded (1000) > projected-fall-soon (500) > frontier (100)
```

**Per-decision pseudocode (Layer 2, sibling of the existing `defend`).**
```
fn defend(world, seat, wp, sp) -> Vec<FleetOrder>:
    enemy = seat.opponent()
    proj  = world.project_forward(sp, wp, HORIZON)
    threats = [ per-planet snapshot ] for p in 0..n:
        agg          = world.planet_aggregate(p)
        being_eroded = sub_being_eroded_for(world, p, seat)          // signal 2 (one foe present, owner absent on an owned sub)
        min_res_frac = min over seat-owned subs of resistance/max    // signal 5
        weakest_eta  = proj.planet_first_fall(p, seat)               // first seat-owned sub to flip to enemy
        my_parked    = world.parked_count(p, seat)                   // signal 3
        soft_cap     = world.soft_cap(p, seat, sp)                   // signal 3

    // TIER 1+2: reinforce the single most-urgent owned/contested planet, ONE per tick.
    //   urgent = contested OR being_eroded OR (weakest_eta <= FALL_SOON) OR frontier.
    //   score = enemy_here - mine + tier_bonus + round((1 - min_res_frac)*100) + (sooner-falls-first).
    best = argmax score over urgent planets (lowest id ties)
    if best is Some(target):
        from = nearest_exportable_to(world, seat, wp, target)        // existing helper
        if from and next_hop(world, from, target):
            eta_reinf = transit_eta(world, from, target, wp)         // signal 5 helper (undock + ceil(path_len/transit_speed))
            // PROJECTION GUARD: don't send to a sub already lost before arrival unless we can out-stack it.
            if target.weakest_eta is None or target.weakest_eta >= eta_reinf or can_cover(from, target):
                return [ FleetOrder(from, hop, REINFORCE_BUCKET) ]    // one reinforcement, never over-extend
        // else fall through to be productive

    // TIER 3: nothing contested/eroded/falling/frontier -> be productive, but spend ONLY the cap surplus.
    orders = []
    for from in exportable_planets(world, seat, wp):
        t = threats[from]
        if t.my_parked + SOFTCAP_SPEND_SLACK <= t.soft_cap: continue        // below cap: hold the reserve (turtle)
        if not reserve_ok_after_export(seat, from): continue                 // respect RESERVE_FRACTION of total army
        target = nearest_planet(world, from, |a| a.owner == Neutral and a.ships_of(enemy) == 0
                                                and proj.planet_capture(a.id) != Some((enemy, eta < arrival)))
        if target and next_hop(world, from, target):
            orders.push(FleetOrder(from, hop, DEFEND_PRODUCE_BUCKET))        // small portion; keep reserve
    if orders: return orders
    return attack_with(world, seat, wp, DEFEND_PRODUCE_BUCKET)               // no neutral: press with the SAME small commitment
```

**Sub-layer note.** Moving ships *between* planets is the above. Keeping ships *on* owned subs to
heal + produce is the per-planet tactical layer's job (`TacticalPolicy::Greedy` over `Layer1View`).
For the turtle to heal correctly, that adapter's floor must be read as a **present-ship heal floor**:
never relocate an owned sub below `HEAL_FLOOR_PER_SUB` present ships (so every owned sub keeps a
healer inside the radius; resistance climbs and production runs). No greedy *decision* rule changes —
only the floor semantics and honouring the new signals.

---

## 4. Expected RPS-cycle dynamics

The validated directional cycle (documented in `crates/ai/src/strategy.rs` and the harness) is:
**attack > colonize**, **colonize > defend**, **defend > attack**. The new mechanics should preserve
it — they were chosen to *deepen* the same forces — but each edge has a new risk surface.

### MEASURED (this pass) — the cycle CLOSES on the diamond

With the final tuned constants (§1D/§6), on the symmetric `diamond_world`, both seatings,
`DEFAULT_HORIZON = 3000`, the four automatons (`ai::automata`) over the event-driven projection:

| edge | 5 seeds × 2 seatings | 8 seeds × 2 seatings |
|---|---|---|
| **attack > colonize** | **10–0** ✅ | **16–0** ✅ |
| **colonize > defend** | **10–0** ✅ | **16–0** ✅ |
| **defend > attack** | **6–4** ✅ | **10–6** ✅ |

(Seeds `1,7,42,2024,31337` and `+100,0x5EA1,9001`.) `defend > attack` is the closest edge — as the
analysis below predicts it is the most fragile under a long grind — but it closes robustly on both
sweeps. The `colonize > defend` and `attack > colonize` edges are clean shut-outs. Asserted by
`crates/ai/src/tests.rs::pure_strategy_cycle_closes_on_diamond`; the campaign showcases L8/L9/L10
re-confirm each edge on the level map (Attack>Colonize 10-0, Colonize>Defend 10-0, Defend>Attack
6-4 — `crates/levels` validation). The corridor world still does **not** fully close (an honest
negative result; see `AI.md`).

**How each edge was made to hold under the grind** (the diagnosis the tuning rests on):
- *attack > colonize*: at `fight_efficiency = 10` Attack just **hoarded** a cap-exempt mobile stack
  and never committed, which out-massed everyone at the horizon (a degenerate "attack dominates").
  Dropping it to `2` makes Attack **commit** its siege into the colonizer's thin, fat colony — where
  it lands. 10-0.
- *colonize > defend*: needed the projection horizon raised to `2000` first — at `240` the marginal
  query `marginal_ticks_saved` reads `0` (one ship can't flip 1800 within 240 ticks) so Colonize
  emitted *nothing*. With a grind-spanning horizon it commits sweet-spot waves and out-territories
  the turtle. 10-0.
- *defend > attack*: the turtle was being out-massed because it **capped out** (parked reserve bled
  by the cap) while Attack kept its hoard mobile. Lowering `softcap_attrition` to `0.5` (gentler
  bleed) lets the turtle hold a real wall, and a parity-gated counter-punch keeps its over-cap
  surplus *moving* into the attacker's emptied rear — so it out-lasts and punishes the
  over-extension. 6-4.

### attack > colonize — should still hold, in fact STRONGER
Colonize ships everything above the floor to fronts and leaves fat, freshly flipped colonies held by
`GARRISON_FLOOR`. The new mechanics add a second, cheaper way to win this matchup on top of capture:
**production denial** — Attack need only PARK on the colony (Mechanic B) to choke the exact
compounding output Colonize exists for, *before* paying the full grind. And **heal-on-hold** means a
returning/garrisoning attacker repairs ground it took, so a colonizer cannot cheaply chip it back.
**At risk if:** the grind is *too slow* (high `max_resistance`, low `SIEGE_FIGHT_MARGIN`) so a
strike stalls long enough for the colonizer's territory lead to dominate the horizon score; or
Colonize's `OVERSTACK_GUARD`/projection makes it incidentally leave a thicker-than-floor garrison on
the colony nearest the enemy. **Flag:** verify Attack's siege actually *flips or denies* a colony
within the match horizon for the chosen resistance/period; if not, attack→colonize regresses.

### colonize > defend — should still hold, but TIGHTER than before
Production scales with owned-sub count, and Colonize's sweet-spot waves flip neutrals fast, so it
out-territories a turtle that only spends cap surplus. The new soft cap *helps* Colonize relative to
Defend: a turtle that "keeps its reserve home" runs that reserve into **attrition** once parked >
soft cap, whereas Colonize keeps ships moving (cap-exempt in inter-planet transit) and converts them
to new producers. **At risk if:** (a) Defend's productive Tier-3 is too aggressive (e.g.
`DEFEND_PRODUCE_BUCKET` raised toward `Half`) and it effectively colonizes nearly as fast while
keeping its defensive edge — the original tuning note in `strategy.rs` already records that `Half`
flipped defend→attack and over-dispersed; raising it risks also flipping defend→colonize toward
parity; (b) `max_resistance` is high enough that Colonize's flips are slow, blunting its tempo
advantage and letting the turtle's heal/uptime catch up. **Flag:** this is the **most fragile edge**
under the new model — sweep `SHIPS_PER_RESISTANCE` / `MARGINAL_SHIP_COST_TICKS` (Colonize tempo)
against `RESERVE_FRACTION` / `DEFEND_PRODUCE_BUCKET` (turtle leakage) and confirm Colonize still
leads on territory at the horizon.

### defend > attack — should still hold, and the new heal mechanic REINFORCES it
A patient Defender turns the target into a tar-pit: its frontier garrison + **heal-on-hold** means
an under-committed or hesitant siege never drives resistance to 0 (it refills), while Attack has
already stripped its rear to feed the assault — the Defender then counter-punches the floor-only rear,
and the captured rear keeps producing. The new `ready_to_commit` gate (don't commit without
fight-margin AND heal-outlast-margin AND timely arrival) is what *keeps Attack honest*; the edge
holds precisely because a correct Defender forces Attack to over-commit to break the heal. **At risk
if:** (a) Attack's `HEAL_OUTLAST_MARGIN` / `GRIND_HOLD_FLOOR` are tuned high enough that it only ever
commits truly overwhelming, sustained sieges that the Defender's one-reinforcement-per-tick trickle
cannot out-heal — then attack→defend can tip; (b) **denial** lets Attack starve the Defender's
production from outside the heal loop (park, don't capture), bleeding the turtle's reserve-rebuild
without ever entering the tar-pit — this is the **most likely way the new mechanics could break this
edge**. **Flag:** gate Attack's denial behind genuine production-superiority (as designed) and verify
defend→attack still wins; if denial alone flips it, weaken denial (raise `DENIAL_DETACH` cost, or
restrict to when already winning).

### Cross-cutting risk to ALL three edges
The shared projection ignores the enemy and combat. If any automaton *over-trusts* it (commits as if
an ETA were a promise rather than re-projecting each tick and discounting `became_contested` /
`flips_again`), its real behaviour diverges from the intended identity and the cycle can wobble in
ways that look like a balance bug but are actually a "trusted a loose bound" bug. The
re-project-every-tick contract (§2D) is load-bearing for the whole triad.

---

## 5. New sim signals / APIs to add

None of these mutate state; all are deterministic pure reads (the projection draws no RNG).
Grouped by crate. Items marked **(exists)** are already in the code and are listed only to pin the
reference.

**`layer1::Structure` (per-sub / per-structure reads):**
1. `idle_presence_in_sub(sub: SubId, faction: Faction) -> usize` — like the existing
   `presence_in_sub` **(exists)** but counts only **idle** ships (`target == None`) inside the
   radius. Keeps the projection's initial presence from double-counting a still-inside moving ship
   that is also a scheduled arrival, using the authoritative radius metric.
2. `sub_presence(sub: SubId) -> { player: u32, enemy: u32, owner: Faction }` and
   `single_present_faction(sub: SubId) -> Option<(Faction, u32)>` — the erosion/heal driver as a
   first-class read, so callers don't re-derive it from two `presence_in_sub` calls.
   (`sub_being_eroded_for(world, p, seat)` is a thin strategy-side helper built on this:
   any seat-owned sub with exactly one foreign faction present and the owner absent.)
3. Soft-cap reads (mirror `resolve_softcap`'s "parked"):
   `parked_count(faction: Faction) -> u32` (living idle + intra-structure-transit; inter-planet
   fleets excluded), and `soft_cap(faction: Faction, params: &SimParams) -> u32` returning
   `softcap_free + softcap_per_sub * sub_count(faction)`. Plus an idle accessor at sub/planet scope
   (`idle_count_at` **(exists)** at sub scope) for the `would_overstack` guard.
4. Resistance reads: `sub_resistance(sub) -> (f32 current, f32 max)` (the fields
   `SubStructure.resistance` / `.max_resistance` **(exist)**, this is just a query) and
   `total_foreign_resistance(vs_owner: Faction) -> f32` = sum of `s.resistance` over subs whose
   `owner != vs_owner` (the quantity SimpleColonizer sizes its wave on).
5. `SubStructure::capture_step(owner, resistance, max_resistance, pres_player, pres_enemy)
   -> (new_owner, new_resistance, flipped)` — extract the body of `resolve_resistance` into a
   **pure** function that both the sim and the projection call. This **guarantees** the projection's
   grind matches the sim bit-for-bit and can never drift when the rule is tuned. Without it the
   projection inlines the rule (a documented duplication risk). **Strongly recommended.**

**`world::World` (Layer-2 reads + the projection):**
- `entry_sub(dest, from, faction) -> Option<SubId>` — **promote the existing private `entry_sub`**
  (`crates/world/src/lib.rs`) to `pub`. The projection must use the *identical* fleet-landing rule
  as `inject_fleet`; re-implementing it in the AI would risk drift.
- `fleet_arrival_ticks(&self, wp: &WorldParams, f: &InterFleet) -> u64` — ticks until an in-transit
  fleet's ships are injected: `undock_remaining + ceil((1 - progress)/dprog)` with the same
  lane-length clamp as the private `f_lane_len` **(exists, private)**. Exposing it keeps the
  transit-time formula identical to `World::step`. (Could be a free fn in `world`.)
- `planet_total_resistance_vs(p, seat) -> f32` — Layer-2 wrapper summing
  `total_foreign_resistance(seat)` over planet `p`'s subs (SimpleColonizer wave size).
- `parked_count(p, seat) -> u32` and `soft_cap(p, seat, &SimParams) -> u32` — planet-scope wrappers
  of the Structure reads (Defend's reserve/spend logic). May instead be folded into
  `PlanetAggregate`.
- **`project_forward(&self, &SimParams, &WorldParams, horizon) -> Projection`** plus the
  `Projection` methods in §2B (`sub_fate`, `sub_capture`, `planet_capture`, `incoming_present_at`,
  `returning_owner_force`, and the small derived `planet_first_fall` / `eta_to_present_for` helpers
  the §3 pseudocode references). The centerpiece. Pure, RNG-free, deterministic.

**`ai` crate (view-level surfacing — so both layers share one policy body):**
- Extend `PositionView` (`crates/ai/src/greedy.rs`) with the new per-position reads the §3 policies
  use, each implemented by `Layer1View` (direct sub reads) and `Layer2View` (planet-aggregate /
  world reads):
  `resistance(id) -> f32`, `max_resistance(id) -> f32`,
  `present_count(id, faction) -> u32`, `present_factions(id) -> {Player,Enemy}`,
  `idle_at(id, faction) -> u32`, `soft_cap_at(id, faction) -> u32`,
  `parked_ratio(id) -> f32`, `project_forward(horizon) -> Projection` (borrows/caches the world
  projection; at Layer 1 it projects that single structure), and `transit_ticks(from, to) -> ticks`
  (`dist/ship_speed` at L1; `path_len/transit_speed` at L2).
  For Layer 2, `resistance(planet)` should expose `capture_resistance_remaining(seat)` = sum of
  `s.resistance` over subs not owned by `seat` (see open question in §6 re sum vs min).
- **(exist, reused verbatim):** `bucket_for(want, available)`; `graph::path_len`,
  `graph::next_hop`; `PlanetAggregate::ships_of`, `::fully_owned_uncontested`,
  `::player_subs/enemy_subs/neutral_subs`; `world::World::lane_length`; the strategy helpers
  `exportable_planets`, `exportable_surplus`, `nearest_planet`, `nearest_exportable_to`,
  `is_frontier`, `any_owned_can_reach`.

**Determinism note for the implementer.** The projection must NOT read or advance any `Structure`
RNG and writes nothing, so `state_hash` is untouched by calling it. The only RNG-bearing new
behaviour is soft-cap destruction, which already draws from the structure's seeded `Rng` and is
already folded into the hash via the RNG-position mix.

---

## 6. Balance risks + constants to tune first

### FINAL TUNED VALUES (implemented — what actually closed the cycle)

The sweep below was carried out; the **resolved** operating point that closes the diamond cycle
(§4) while keeping `DEFAULT_MAX_RESISTANCE = 1800`:

| dial | where | from → **to** | why |
|---|---|---|---|
| `softcap_attrition` | `SimParams` | `1.0 → ` **`0.5`** | gentler hoard bleed ⇒ a turtle holds a real wall and out-lasts an over-committed attacker (the `defend > attack` lever; #2 below) |
| `DEFAULT_PROJECTION_HORIZON` | `world` | `240 → ` **`2000`** | the AI look-ahead must span a ~`1800/force` grind, else `marginal_ticks_saved` / `capture_eta` read 0 and the colonizers never commit (#1/#3) |
| `DEFAULT_HORIZON` | `ai::harness` | `1200 → ` **`3000`** | the match must run long enough for the grind to resolve, not cut off mid-siege |
| `AttackParams::fight_efficiency` | `ai::automata` | `10 → ` **`2`** | a high bar made Attack hoard-and-never-commit (degenerate dominance); a modest bar makes it commit the siege Defend punishes (#4) |
| `SimpleColonizerParams::ships_per_res` | `ai::automata` | `0.12 → ` **`0.02`** + threshold `= min_wave` | the old value was sized for `max_resistance≈100`; at 1800 it demanded ~216 ships/target and never sent (SimpleColonize drew with Passive) (#3) |
| `DefendParams::counter_punch_cap` | `ai::automata` | *new* **`30`** | per-source bound on the parity-gated counter-punch that keeps the turtle's over-cap surplus mobile against an attacker's emptied rear (#5/#6) |
| L6 prize `max_resistance` | `levels` | *new* **`600`** | a contested 3-sub prize at 1800 froze into a coin-flip the greedy player lost; a lower-resistance "rich but not impregnable" mine + an asymmetric (shorter Player) approach restores L6 winnability |

Everything in `ai` still routes mechanic questions through projection queries / property accessors
(the `no_raw_mechanic_constants_in_ai` guard still passes); the only numbers named in `ai` are the
policy tunables in each `*Params`. Determinism (`state_hash`) is preserved — the projection draws no
RNG and the AI adds none.

### Tune-first list (highest leverage, roughly in order) — *as swept; outcomes noted*
1. **`max_resistance` / `DEFAULT_MAX_RESISTANCE` (default 100)** — the master grind dial. It sets
   how long every capture takes and therefore the *whole* tempo of attack vs colonize vs defend.
   Sweep this **first**; everything else is relative to it. Too high ⇒ sieges never finish (defend
   and the status quo win everything, attack→colonize regresses); too low ⇒ capture is nearly
   instant and the old "thin rear gets snowballed" dominates, flattening the cycle.
2. **`softcap_per_sub` (10) and `softcap_attrition` (1.0)** — the equilibrium-surplus level
   (≈ 10× production) and how hard hoards are trimmed. These decide how much standing force is
   "free" and how punishing it is to sit still — directly the colonize-vs-defend tightness. If
   Defend plateaus and loses too easily, lower `softcap_attrition` or raise `softcap_per_sub`; if
   mega-stacks still happen, the reverse.
3. **Colonize tempo: `SHIPS_PER_RESISTANCE` (0.12, SimpleColonizer) and
   `MARGINAL_SHIP_COST_TICKS` (6.0) + `WAVE_MIN/WAVE_MAX` (4/16, Colonize)** — set wave sizes and
   thus how fast neutrals flip. Co-tune with `max_resistance` so a typical neutral pulls a wave near
   the sweet spot `sqrt(r/C)` and the colonizer's flips outpace a turtle but don't trivialize the
   board.
4. **Attack siege gates: `SIEGE_FIGHT_MARGIN` (1.30), `HEAL_OUTLAST_MARGIN` (1.25),
   `GRIND_HOLD_FLOOR` (4)** — the line between "honest over-commit that defend punishes" and
   "only-ever-overwhelming sieges that beat defend." Tune to keep **defend > attack** while still
   letting **attack > colonize** land.
5. **Defend leakage: `RESERVE_FRACTION` (0.75) and `DEFEND_PRODUCE_BUCKET` (Quarter)** — how much
   the turtle spends when quiet. The `strategy.rs` note already records `Half` over-disperses; treat
   `Quarter` as the safe default and only raise if Defend is too passive — watching that it does not
   reach colonize/attack parity.
6. **Denial: `DENIAL_DETACH` (6) + the production-superiority gate** — cheap economic damage that
   could, if too strong, flip **defend > attack**. Keep it gated; if it breaks the edge, raise the
   detachment cost or restrict to "already winning."

### Balance risks to watch in the harness
- **Defend's reserve fights the soft cap.** `RESERVE_FRACTION` is a fraction of the *current army*,
  but the cap deletes parked ships above `soft = 20 + 10·prod`. On a 1-sub planet `soft = 30`, so a
  "75% reserve" can simply be attrited. The design sidesteps this by spending only the **over-cap**
  surplus, but the global reserve guard can still bind awkwardly on tiny planets — consider
  expressing the reserve as a fraction of `soft_cap` rather than of the army. (Open.)
- **FALL_SOON pre-emption thrash.** When the projection says a sub falls in N ticks and Defend
  launches a reinforcement, next tick the now-in-transit fleet changes the projection and urgency
  drops — risking oscillation between two threatened subs. The one-reinforcement-per-tick rule + the
  projection already counting in-transit ships should damp it; **add a harness check that the turtle
  does not oscillate.**
- **Projection over-trust** (cross-cutting, see §4): any automaton that treats an ETA as a promise
  instead of re-projecting and discounting `became_contested`/`flips_again` will diverge from its
  identity. Highest-impact correctness risk for the whole triad.
- **Bucket granularity vs resistance-sized waves.** `FractionBucket` (25/50/75/100%) means
  `bucket_for` can over/under-shoot a resistance-sized `want`. Per-tick best-effort filling
  (subtract the bucket's actual release; re-size from fresh surplus next tick) is the intended
  "simple" behaviour — confirm it does not chronically under-deliver against tough targets.
- **Soft-cap SPEND leak feeding piecemeal.** Attack's cap-pressure valve leaks surplus toward the
  target before it is a decisive wave, which can worsen over-extension and feed the square-law death
  concentration is meant to avoid. Acceptable as a self-limiting valve; watch that it doesn't turn
  Attack into a trickle against a Defender.

### Open design questions (carry into review)
- **Layer choice for the everyman:** SimpleColonizer is specified primarily at Layer 2 with a Layer-1
  variant; if the early campaign is the single-structure form, swap which is primary.
- **L2 resistance aggregation:** `capture_resistance_remaining(seat)` as **SUM** over unowned subs
  (total grind to fully own a planet) vs **MIN** (cheapest foothold). Foothold-first (MIN) flips
  ownership/production fastest under the new model; SUM avoids leaving a contested planet. Designs
  here assume foothold-first for flip-speed — confirm.
- **`MARGINAL_SHIP_COST_TICKS` static vs dynamic:** a constant is legible; the truer value is
  `production_period / current #producers` (recomputed per tick). Decide per readability vs accuracy.
- **Should Colonize ever lock a friendly below-max sub** (cheap insurance against a grind-back) or
  stay strictly new-ground to keep the clean identity/blind spot? Designs keep it neutral-only.
- **Event-driven projection** (§2C): worth the extra code only if a profile shows the per-tick loop
  is hot; left simple for now.
- **Entry-sub under projected ownership:** the projection picks a fleet's landing sub from *present*
  ownership; over a long horizon the destination's owner can change before landing. Present-state
  approximation is cheap and matches "what `inject_fleet` would do now" — confirm acceptable.
