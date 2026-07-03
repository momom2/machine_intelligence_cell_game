# WORLD — the Layer-2 lens over multiple Layer-1 planets

> **STATUS (latest session) — this doc lags; `CHANGELOG.md` is authoritative.** Since it was last
> refreshed: seats are `Faction::{Neutral, Player, Ai(u8)}` (any number of AI); `World::outcome` and
> `PlanetAggregate` fold **all** non-player seats into one combined-enemy slot (`World::total_foreign_*`),
> and the live AI reads the projection-free **`World::sub_influx_for`** — **`world::projection` is PARKED**
> (kept only for the deferred automata/Counter track).

> **Refreshed for `feat/counter`** — the inter-planet plumbing now routes through each planet's
> **reserve / patrol-zone node** (fleets *arrive into* the reserve and *depart reserve-first*) and
> fleet orders are **faction-scoped**. The projection / Layer-2 aggregate / lanes core is
> unchanged. `CHANGELOG.md` (top `feat/counter` entry) is **authoritative** where this doc and it
> disagree.

This document describes the `world` crate (`crates/world`): **Phase 1** of making **Layer 2**
(the tactical, Solarmax-like view from `03-ui-layers.md`) a *lens* over several **Layer-1**
battlefields rather than a second game.

> **"ONE WORLD, Layer 2 is a lens."**

There is exactly **one** simulation substrate — the spatial Layer-1 sim in the `layer1` crate
(`LAYER1_SIM.md`). Every **planet** *is* a real `layer1::Structure` (its own sub-structures and
discrete ships, all sharing one `SimParams`). Layer 2 adds only two things on top:

- **lanes** — edges between planets, and
- **inter-planet fleets** — clumps of ships in transit from one planet to another.

It is **not** a second combat model. `World::step` steps every planet's `Structure` (which
fights, captures, and produces exactly as before) and moves ships between planets along lanes.
When a fleet arrives, it **injects** its ships into the destination planet's `Structure`,
spawned idle, so the ordinary Layer-1 sim then resolves the landing. The crate is **headless**
(no graphics), **deterministic**, and **fully tested**, with **zero external dependencies**
beyond `layer1` (all randomness stays inside each planet's seeded PRNG).

---

## The model

### Planets — one Layer-1 `Structure` each

A **`Planet`** is:

```rust
pub struct Planet {
    pub structure: layer1::Structure, // the real Layer-1 sim (subs + ships + RNG)
    pub pos: layer1::Vec2,            // the planet's position on the LAYER-2 map
    pub name: String,
}
```

The planet *is* its `structure`: all of its sub-structures, ships, production, combat, and
capture happen inside it under the shared `layer1::SimParams`. `pos` is where the planet sits on
the **Layer-2** map — a separate space from the *intra*-planet coordinates inside `structure`
(used to draw the planet and to give lanes a direction). Planets are referenced by
**`PlanetId = usize`** (the index into `World::planets`).

### Lanes — Layer-2 edges

A **`Lane { a: PlanetId, b: PlanetId, length: f32 }`** is an **undirected** edge between two
planets. `length` (in Layer-2 map units) sets how long a fleet takes to cross. `World` keeps a
deterministic **adjacency** index (`World::neighbors(p)`), rebuilt as lanes are added.

### Inter-planet fleets — ships in transit

An **`InterFleet`** is a clump of ships of one faction crossing a lane:

```rust
pub struct InterFleet {
    pub faction: Faction,        // a real seat (never Neutral)
    pub from: PlanetId,          // source planet (ships were pulled from here)
    pub to: PlanetId,            // destination planet (ships injected here on arrival)
    pub count: u32,              // ships carried (conserved: removed from `from`, re-spawned at `to`)
    pub undock_remaining: u32,   // ticks left undocking before crossing begins
    pub progress: f32,           // fraction of the lane crossed, in [0,1]; advances after undock
}
```

It **mirrors Layer-1's undock-then-transit movement** so inter-planet travel feels consistent
with intra-planet movement: the fleet first burns `undock_remaining` ticks leaving the source
(like ships peeling off a sub-structure), then advances `progress` from `0.0` to `1.0` across
the lane. At `progress >= 1.0` it has **arrived** (`InterFleet::arrived()`).

### `WorldParams` — the inter-planet dials

Intra-planet behaviour is governed by `layer1::SimParams`; the **inter-planet** layer has its
own small set of dials:

| Field | Default | Meaning |
|---|---|---|
| `undock_ticks` | **6** | Ticks a freshly launched fleet spends undocking before it starts crossing the lane (the Layer-2 analog of ships peeling off a sub). |
| `transit_speed` | **1.4** | Lane-length units covered per tick once transiting. Per-tick `progress` gain is `transit_speed / lane.length`, so a lane of `length L` takes ~`L / transit_speed` ticks (after undocking). Defaults to Layer-1's `ship_speed`. |
| `keep_floor` | **2** | Per-sub idle **garrison floor** kept on the source planet when a fleet launches: no owned sub is drawn below this many idle ships, so a planet never exports itself empty. |

### `FleetOrder` — the inter-planet atomic action

```rust
pub struct FleetOrder { pub from: PlanetId, pub to: PlanetId, pub fraction: FractionBucket }
```

Deliberately the **same shape** as Layer-1's `MoveOrder` (`source`/`target`/`fraction`, here
`from`/`to`/`fraction`) so the shared-vocabulary spine holds across layers. Like `MoveOrder`, it
carries **no faction** — the *acting seat* is supplied at the call site:

```rust
let launched: u32 = world.issue_fleet_order(order, faction, &world_params);
```

`issue_fleet_order` pulls a `FractionBucket` of **`faction`'s** **idle** ships off the source
planet — it is **faction-scoped**, so it only ever moves the acting seat's ships and never drags
the *other* seat's ships off a contested sub — keeping `keep_floor` idle per sub, removes them
from the source `Structure` immediately, and pushes one `InterFleet` toward `to`. It returns the
number of ships actually launched. Sending **100 %** (`fraction.as_f32() >= 1.0`) takes
*everything* — the keep-floor is dropped to `0`; any smaller fraction keeps the floor.

`issue_fleet_order_fraction(from, to, frac, faction, wp)` is the same action with a **continuous**
send-fraction `frac` in `(0, 1]` (the GUI's free 1–100 % troop slider) instead of a
`FractionBucket`; same lane validation, faction scoping, keep-floor, and 100 %→floor-0 rule (the
four snap positions match the buckets exactly). Both funnel through the shared `launch_fleet` core.

**Validity / junk-safety (returns 0, no fleet created):**
- `from == to`, or either id out of range,
- **no lane connects `from` and `to`** (only *connected* orders are valid),
- `faction` is `Neutral`,
- the source planet has no exportable idle surplus for `faction` (everything is below the
  garrison floor, in transit, or on subs the faction does not own).

Once launched, a fleet is **not redirected** — "commit, then it's flying," matching Layer-1.

### Surplus / garrison-floor on launch — reserve-first

The pull uses the Layer-1 helper `Structure::take_idle_ships_planetwide(faction, fraction,
keep_floor)` (see below; the fraction-slider path uses the sibling
`take_idle_ships_planetwide_fraction`). Both delegate to one core, `export_idle_planetwide`, which
departs **reserve-first**:

- **If the planet's reserve / patrol-zone node (the `storage_sub`) holds idle ships of `faction`**,
  the fleet departs from **there** — the reserve is the staging area, so the fraction applies to the
  reserve's idle count and **no keep-floor is held back on the reserve** (it is not territory, see
  below). Interior subs are reached and left by intra-structure moves, not directly by an
  inter-planet order.
- **Only when the reserve is empty** (or the structure has no reserve) does the pull fall back to
  the producing subs: the bucket applies to the faction's **total idle ships across the planet**,
  ships are drawn sub-by-sub in ascending `SubId` order, **no owned sub is taken below `keep_floor`
  idle ships**, and **only subs the faction owns** are eligible export sources (idle ships on a
  not-yet-captured sub are garrisoning ground, not surplus).

At **game start the reserve is empty**, so the first fleets pull from the subs; once ships flow into
the reserve, departures draw from it first. (Auto-flow of sub surplus → reserve is a **deferred**
future rule, not yet implemented.) If the floor binds everywhere on the fallback path, fewer than
the bucket — possibly zero — are launched; the returned count is always the true number sent.

### Arrival injection — the reserve-node entry rule

When a fleet's `progress` reaches `1.0`, `World::step` injects its `count` ships into the
destination planet's `Structure` as `faction`, spawned **idle** (via `Structure::spawn_ship`),
so the ordinary Layer-1 sim resolves the landing (fight / capture) on subsequent ticks. The
**entry sub** is chosen by `inject_fleet` as `structure.storage_sub.or_else(|| entry_sub(...))`:

1. **If the destination planet has a reserve / patrol-zone node** (the `storage_sub`), the fleet
   lands **into the reserve** — the universal inter-planet entry/exit point. Every campaign planet
   has one, so this is the normal path: arriving ships pool in the reserve, then move into the
   interior subs by ordinary intra-structure moves. (The reserve is capturable but confers no
   production and is not counted as territory — see the aggregate section below.)
2. **Otherwise (a bare structure with no reserve)**, fall back to the lane-facing `entry_sub`,
   which garrisons at **one destination sub-structure** chosen so the landing comes in *along the
   lane* from the source:
   - Compute `dir` = the unit vector **from the destination planet toward the source planet on the
     Layer-2 map**, and a **perimeter point** = `dir * destination.local_radius` in the
     destination structure's local space (its subs are laid out around the local origin).
   - **If `faction` already owns a sub on the destination**, land at the **owned** sub nearest that
     perimeter point — reinforcements rally at the friendly position closest to where the lane
     enters. **Otherwise (a beachhead with no foothold yet)**, land at the destination sub nearest
     that perimeter point — the edge facing the source.

Ties break to the lowest `SubId` (deterministic). If the destination has no sub-structures at all,
nothing is injected (a degenerate map the constructors never build). Injection happens **after**
this tick's planet steps, so freshly landed ships first fight on the **next** tick — the same
"no retroactive action this tick" discipline Layer-1 uses for production/capture.

### `World::step` — the fixed iteration order

`World::step(&params, &world_params)` advances the whole world by exactly one tick in this
**fixed, documented** order (this is what makes the world deterministic):

1. **Planets** — step every planet's `Structure` in ascending `PlanetId` order (each does its
   own production → movement → combat → capture internally).
2. **Fleets** — advance every in-transit fleet in `fleets`-vector order: burn one undock tick,
   else add this tick's lane progress (`transit_speed / lane.length`, clamped to `1.0`).
3. **Arrivals** — any fleet that has now fully crossed injects its ships (entry rule above) and
   is removed; surviving fleets keep their relative order.
4. **tick** += 1 (every planet's own `tick` advances in lock-step with the world tick).

---

## The Layer-2 aggregate (the lens datum)

`World::planet_aggregate(p) -> PlanetAggregate` is the **lens data** per planet — what the
renderer draws, and what the strategic AI and the greedy export rule read instead of peering at
every sub-structure. It is computed from the planet's `Structure` plus the in-transit fleets
headed to it; it adds no state.

```rust
pub struct PlanetAggregate {
    pub owner: PlanetOwner,        // Owned(faction) | Contested | Neutral
    pub player_ships: usize,       // living Player ships GARRISONED on the planet
    pub enemy_ships: usize,        // living Enemy ships garrisoned on the planet
    pub player_incoming: u32,      // Player ships in fleets whose `to` is this planet
    pub enemy_incoming: u32,       // Enemy ships arriving
    pub player_subs: usize,        // Player-owned sub-structures
    pub enemy_subs: usize,         // Enemy-owned sub-structures
    pub neutral_subs: usize,       // unowned sub-structures
}
```

**Owner rule** (from *garrisoned* presence = owned subs and/or garrisoned ships):
- **`Owned(faction)`** — `faction` is present and the enemy is **not** (the enemy owns no sub and
  has no garrisoned ship). Neutral subs may remain. (Equivalently: `faction` owns all owned subs
  and no enemy ship is present.)
- **`Contested`** — **both** real factions have a presence (a sub or a garrisoned ship each).
- **`Neutral`** — neither real faction owns a sub and neither has a garrisoned ship (all-neutral
  or empty).

**Sub tallies exclude the reserve node.** `player_subs` / `enemy_subs` / `neutral_subs` come from
`layer1::Structure::sub_count(faction)`, which **excludes the `storage_sub`** — a no-production
patrol zone never counts as territory. So the reserve node, whoever holds it, does **not**
contribute to ownership, to `fully_owned_uncontested`, or to the owner rule above; a planet is
"fully owned" when its *producing* subs are all yours, regardless of the reserve. (Garrisoned
*ships* in the reserve still count toward the `*_ships` tallies and the present/contested check.)

**Ship counts** include both **garrisoned** ships (in the planet's `Structure`) and ships
**currently arriving** (`*_incoming`). `PlanetAggregate::ships_of(faction)` returns the sum.

**Incoming fleets are counted in the tallies but do NOT by themselves set `Contested` or change
the owner** — a fleet that has not landed is not yet "present" for ownership. It flips the
aggregate the tick *after* it lands and its ships become real garrisoned ships. (So an enemy
fleet inbound to a planet you fully own does not, on its own, make that planet contested or
block you from exporting from it — until it lands.)

**Exportable flag:** `PlanetAggregate::fully_owned_uncontested(faction)` is `true` iff `faction`
owns **every** sub on the planet (no enemy *and* no neutral sub remains) **and no enemy ship is
present** (garrisoned). This is the precondition a later phase uses to decide a planet may
**export surplus**: it is securely held, so shipping idle ships elsewhere will not immediately
lose it. (Friendly incoming fleets do not affect it; an inbound enemy fleet flips it to `false`
only once it lands.)

---

## Resistance / soft-cap read signals (planet-scope wrappers)

The resistance / denial / soft-cap overhaul (`LAYER1_SIM.md`, `AUTOMATA_DESIGN.md` §1) added
per-sub / per-structure reads to `layer1::Structure`. `World` re-exports three at **planet scope**
so the strategic AI never reaches into a planet's `Structure`. All are pure, deterministic reads
that add no state and draw no randomness; out-of-range `p` yields the zero value.

- **`planet_total_resistance_vs(p, seat) -> f32`** — total foreign capture resistance on planet `p`
  from `seat`'s point of view: the sum of `resistance` over every sub on `p` **not** owned by `seat`
  (neutral *and* enemy subs). The quantity a resistance-proportional colonizer sizes a capture wave
  on. Wraps `Structure::total_foreign_resistance`.
- **`parked_count(p, seat) -> u32`** — parked ships of `seat` on planet `p` (living ships in the
  planet's `Structure` — idle or intra-structure transit). Inter-planet fleets to/from `p` live in
  `World::fleets` and are **not** counted (they are soft-cap-exempt). Wraps `Structure::parked_count`.
- **`soft_cap(p, seat, sp) -> u32`** — the soft cap for `seat` on planet `p`: `softcap_free` plus the
  **sum of per-sub capacities** of the subs `seat` owns there (numerically `softcap_free +
  softcap_per_sub · owned_subs` today). When `parked_count` exceeds it, the planet's `Structure`
  bleeds the overflow with `sqrt` attrition. Wraps `Structure::soft_cap`.

These are the planet-level signals the **parked** automata (Defend's reserve/spend logic, the
old SimpleColonizer's wave sizing) read; the projection below is the heavier read they share for
capture timing. The **live** Simple reads none of them — it sizes waves off `sub_influx_for` and
the per-sub resistance reads.

---

## The forward-projection — an event-driven mean-field look-ahead

`World::project_forward(&SimParams, &WorldParams, horizon) -> Projection`
(`crates/world/src/projection.rs`) is the **single read-only look-ahead every automaton shares**.
It answers, for each sub of each planet:

> If **no new orders** are issued and the **enemy stays passive**, considering the ships
> **present now** plus the ships **already in transit toward this sub** (both intra-structure moves
> *and* inter-planet `InterFleet`s, each counted from its **arrival tick**), and folding in the
> resistance grind, the expected square-law combat where forces are co-present, and the owner's
> production over the window — when (if ever) over the next `horizon` ticks does this sub's owner
> change, and to whom?

It must live in `world` (not `layer1`) precisely because it has to reason over **both** intra-
structure moving ships *and* inter-planet fleets at once.

### Event-driven, recursive over segments

The integrator is **per-sub and event-driven**: it walks the **segments between successive arrival
ticks**, where the arriving force is constant and the dynamics are closed-form.

- **Uncontested** (one faction present, no spawn/flip boundary inside the span): the grind/heal is
  linear, so it **jumps** in O(1) — `ticks_to_flip = ceil(remaining_resistance / present_force)`
  for an attacker, or a capped linear heal for the owner — straight to the next event.
- **Contested** (both seats present): combat is active and the grind is **frozen** (exactly as the
  sim). This stretch advances **tick-by-tick** under the **mean-field square law** (the
  deterministic *expectation* of the sim's stochastic per-ship fire) until one side is cleared — a
  naturally short stretch — then jumping resumes.

The cost is therefore `O(arrivals + contested_ticks + spawns)` per sub, not `O(horizon)` per sub.
Both the fast path and a tick-by-tick **reference** oracle (test-only, `project_reference`) call the
**one** canonical per-tick kernel `step_one_tick`, applied in the exact sim order **production →
arrivals → combat → capture**, so the fast path can only differ where its closed-form jump is
provably equal to repeating the kernel — which the unit tests assert.

### Shared grind — cannot drift from the sim

Every owner / flip / heal decision goes through the **same pure `layer1::SubStructure::capture_step`
the sim itself calls**, so the projection's capture rule can never drift from the simulation when
the rule is tuned. Likewise, fleet arrival timing reuses `fleet_arrival_ticks` (the exact
`World::step` undock+transit formula), so the projection schedules a fleet at the *identical* tick
(with the `+1` for `World::step`'s end-of-tick injection) the sim would.

> **Known gap (`feat/counter`).** For the **landing sub**, `project_forward` still uses
> `World::entry_sub` (the lane-facing rule) directly, whereas the live `inject_fleet` now lands into
> the planet's reserve node when one exists (`storage_sub.or_else(entry_sub)`). On every campaign
> planet (all of which have a reserve) the projection therefore schedules the arrival into a
> *different* sub than the sim uses. The AI track that consumes this is **parked**, so the gap is not
> yet exercised; reconcile the projection to the reserve-routed entry when the automata are revived.

### What it is blind to (callers must respect)

It models exactly two event classes — in-transit **arrivals** and the **resistance rule** they drive
— plus the *expectation* of combat and production **within** the present force. It still ignores
**new orders** and any **enemy reaction** (the enemy is passive by construction). Its ETAs are
therefore **bounds, not promises**: `became_contested` flags subs where the grind froze under
co-presence (the real flip time is a lower bound, since stochastic combat decides who is "the only
side present"), and `flips_again` flags a second owner change after the first. The contract is to
**re-project every decision tick** rather than trust a stale plan.

### Pure / deterministic / cheap

`project_forward` is a pure read of `(&World, &SimParams, &WorldParams)`: it mutates nothing,
touches no planet's seeded `Rng`, and draws **no randomness**, so calling it never perturbs
`World::state_hash`. The marginal what-if queries (below) re-integrate a single sub from a captured
scalar seed — no `&World`, no mutation — so they are equally pure.

`DEFAULT_PROJECTION_HORIZON = 2000` ticks (raised from `240` so the look-ahead spans a full
`~max_resistance/force` grind; below that the marginal-capture queries read 0 and the colonizers
never commit — see `docs/archive/AUTOMATA_DESIGN.md` §6). Historical profiling (via the
since-deleted `proj-bench` binary, on a representative six-planet world — 21 subs, ~100 garrisoned
ships, 8 fleets in flight) put the event-driven integrator at **tens of microseconds per call at
horizon 2000** (~28 µs best / ~36 µs median; roughly 3 µs @ 60 → 66 µs @ 5000), three to four
orders of magnitude under the ~1 ms/decision budget — the cost was never why the projection was
parked.

### The composable query API

The result is a `Projection` holding one `SubFate` per `(planet, sub)` (O(1) lookup), the scheduled
arrivals, and a captured per-sub seed for the marginal queries. On top of the per-sub roll-ups it
exposes a small, orthogonal **semantic-query vocabulary** that hand-written *and* future evolved
agents build policies from — the projection is the **sole** place game mechanics live for the AI, so
an automaton never re-derives a mechanic:

**Capture-timing queries.**
- `capture_eta(planet, sub) -> Option<u64>` — absolute tick of the first projected owner change on
  the *current* plan, or `None` if it does not flip within the horizon.
- `capture_eta_if(planet, sub, extra, arriving_in_ticks, reinforce) -> Option<u64>` — the flip tick
  this sub *would* have if, on top of the current plan, `extra` more ships of `reinforce` arrived
  `arriving_in_ticks` from now. Re-integrates **only this sub** from its seed with one synthetic
  arrival merged in — no `&World`, the live projection untouched. (Pass your own seat for "if I
  reinforce," the opponent for "if they do.")
- `marginal_ticks_saved(target_planet, target_sub, from_position) -> u64` — the value, **in ticks
  saved on the capture**, of one more ship sent from `from_position` (a sub on the same planet, whose
  distance sets the arrival delay). Defined as `capture_eta_if(0) − capture_eta_if(1 more)` for the
  side that *owns* `from_position`; always `>= 0`. The steeply-diminishing `dT ≈ r/w²` quantity
  Colonize uses to find its wave sweet spot.

**Combat-model query.**
- `expected_combat(attackers, defenders, defender_in_own_sub) -> (atk_survivors, def_survivors)` —
  the deterministic square-law *expectation* of a fight resolved to one side's extinction (the same
  per-tick kernel the contested regime uses). `defender_in_own_sub` grants the additive
  `defender_fire_bonus`, so this is the single place the defender edge enters AI reasoning. Pure /
  frame-independent (reads only the stored `SimParams`).

**Force-sizing query.**
- `force_for_efficiency(planet, sub, desired_ratio) -> Option<u32>` — the **smallest** attacking
  force that beats this sub's *current* defenders while trading at least `desired_ratio`-to-1
  (attacker losses : defender losses), derived purely from `expected_combat` + the on-sub edge.
  `Some(0)` if the sub has no defenders; `None` if even an overwhelming force cannot reach the ratio.
  Monotone in `desired_ratio`. The "win the firefight *efficiently*" primitive Attack sizes a
  spearhead with.

**Per-element property reads.**
- `current_owner(planet, sub) -> Faction` — owner at call time.
- `sub_resistance(planet, sub) -> (f32, f32)` — `(current, max)` resistance at call time.
- `present_now(planet, sub) -> (u32, u32)` — the `(player, enemy)` idle ships seeded as initial
  presence (these cannot be read off `SubFate` alone).

**Per-sub & planet roll-ups.**
- `sub_fate(planet, sub) -> &SubFate` — the full per-sub forecast: `current_owner`,
  `eta_first_change` / `owner_after_first_change`, `owner_at_horizon`, `resistance_at_horizon`, and
  the `became_contested` / `flips_again` confidence flags. All `eta_*` are **absolute world ticks**
  (`>= base_tick`).
- `sub_capture(planet, sub) -> Option<(Faction, u64)>` — who captures/frees the sub first, and when.
- `planet_capture(planet) -> Option<(Faction, u64)>` — planet-level roll-up: a planet "flips to" a
  faction when, at the horizon, that faction owns **every** owned sub on it (enemy owns none) **and**
  at least one sub changed hands within the horizon; a never-flipping neutral sub blocks a clean
  roll-up. Returns that faction and the tick the last such change completes.
- `incoming_present_at(planet, sub, faction) -> u32` — how many of `faction`'s scheduled in-transit
  ships are projected to have arrived by the horizon (so a caller does not double-send to a sub its
  own in-flight force already settles).
- `returning_owner_force(planet, sub) -> u32` — the in-flight arrivals of the sub's **current
  owner** within the horizon (Attack sizes a heal-outlasting hold from this).
- `eta_to_present_for(planet, sub, faction) -> Option<u64>` — the tick by which `faction` is first
  present at the sub (now if already present, else its earliest scheduled arrival).
- `planet_first_fall(planet, seat) -> Option<(SubId, u64)>` — the first `seat`-owned sub projected to
  flip to the enemy and when (Defend's "reinforce the sub that falls first").

Layer-2 strategies read `planet_capture` / `planet_first_fall`; Layer-1 / interior strategies read
`sub_fate` / `sub_capture` — the *same* projection object, different roll-ups.

---

## World-level outcome

`World::outcome() -> WorldOutcome` is the Layer-2 mirror of `layer1::Outcome`, aggregated over
**all** planets and **all** in-transit fleets:

- A faction is **eliminated** when it owns **no sub on any planet** *and* has **no ships
  anywhere** — garrisoned **or in transit** (`World::is_eliminated`).
- If exactly one real faction is eliminated, the other wins **by elimination**.
- Otherwise the winner **leads on `total ships + total owned subs`** at the horizon; an exact
  tie ⇒ `None` (a draw).

`World::total_ships(faction)` counts garrisoned ships across every planet **plus** every ship in
a fleet of that faction; `World::total_subs(faction)` sums owned subs across planets — and since it
is built on `Structure::sub_count`, the per-planet **reserve nodes are excluded** from the
territory total. (`is_eliminated` therefore ignores the reserve too: holding only a reserve node,
with no producing sub and no ship anywhere, still counts as eliminated.)

```rust
pub struct WorldOutcome {
    pub winner: Option<Faction>, // None only for an exact tie
    pub by_elimination: bool,
    pub tick: u64,
    pub ships: (usize, usize),   // total (player, enemy), garrisoned + in transit
    pub subs:  (usize, usize),   // total owned subs (player, enemy)
}
```

---

## Determinism — the guarantees the renderer and AI rely on

The world preserves Layer-1's bit-reproducibility:

- **All randomness stays inside each planet's `Structure`**, behind its own seeded PRNG. The
  inter-planet layer draws **none** — launching, transiting, and resolving arrivals are all
  RNG-free. (Injection's per-ship spawn jitter is drawn from the *destination planet's* own RNG,
  exactly as normal production would be.)
- The **new Layer-1 export helpers draw no randomness** (see below), so extracting a fleet from
  a planet does not perturb that planet's combat rolls.
- **`World::step` has a fixed iteration order** (planets by `PlanetId`, then fleets in vector
  order, then arrivals in vector order).
- **`World::state_hash()`** folds, with the same inline FNV-1a construction `layer1` uses (floats
  by bit pattern, exact): the world `tick`, then **every planet's `Structure::state_hash()`**
  (which already includes that planet's full sim state *and* RNG position) plus its map `pos`,
  then **every in-transit fleet** (`faction`, `from`, `to`, `count`, `undock_remaining`,
  `progress`).

**Guarantee:** same construction + same orders ⇒ **identical `state_hash` at every tick across
reruns**, and an extra order **diverges** the hash. Cloning a `World` clones every planet's RNG,
so a clone replays bit-for-bit (useful for renderer replay/prediction).

---

## The Layer-1 helper added for this phase (and why)

To launch a fleet, the world must **remove idle ships from a source planet** and re-spawn them
on arrival (ships are conserved across the world even though each `Structure` only ever *marks*
ships dead). Two minimal, documented helpers were added to `layer1::Structure` for this, both
**deterministic and RNG-free** (lowest `ShipId`/`SubId` first; they never advance the PRNG, so
they cannot change later combat):

- **`take_idle_ships(&mut self, sub: SubId, faction: Faction, n: usize) -> usize`** — remove up
  to `n` idle ships of `faction` garrisoned at `sub` (marks them dead), returning how many were
  removed. Only living, **idle** ships whose `home == sub` and faction matches are eligible
  (ships in transit are never yanked, consistent with `issue_order`). Out-of-range `sub` or
  `n == 0` removes nothing.
- **`take_idle_ships_planetwide(&mut self, faction, fraction: FractionBucket, keep_floor: usize)
  -> usize`** (and its continuous sibling `take_idle_ships_planetwide_fraction(faction, frac,
  keep_floor)`) — remove a fraction of `faction`'s idle ships, **reserve-first**: if the structure
  has a `storage_sub` holding `faction`'s idle ships, draw from the reserve (no keep-floor on the
  reserve); only when the reserve is empty fall back to the producing subs `faction` **owns**, drawn
  sub-by-sub in ascending `SubId` order, never taking any sub below `keep_floor` idle ships. Both
  delegate to the private `export_idle_planetwide` core. Returns the true count removed (≤ the
  requested amount; possibly 0 if the floor binds everywhere). This is exactly the primitive
  `FleetOrder` uses.

From a single `Structure`'s point of view an extracted ship is simply *gone* (the same as if it
had been destroyed); from the world's point of view it is conserved — re-spawned at the
destination on arrival. All existing `layer1` tests remain green, and the helpers have their own
unit tests (in `crates/layer1/src/sim.rs`).

---

## The public API at a glance

```rust
use world::{
    World, Planet, Lane, InterFleet, WorldParams,        // model
    FleetOrder, PlanetId,                                 // orders / ids
    PlanetAggregate, PlanetOwner, WorldOutcome,           // lens / outcome
    Projection, SubFate, DEFAULT_PROJECTION_HORIZON,      // forward-projection
};
use layer1::{Structure, SimParams, Faction, FractionBucket, Vec2};

// Build a world: each planet wraps a layer1::Structure placed on the Layer-2 map.
let mut w = World::new();
let a = w.add_planet(Planet::new(structure_a, Vec2::new(0.0, 0.0), "A"));
let b = w.add_planet(Planet::new(structure_b, Vec2::new(40.0, 0.0), "B"));
w.add_lane(a, b, 40.0);                       // undirected edge, length 40

let params = SimParams::default();            // intra-planet dials (shared by all planets)
let wp = WorldParams::default();              // inter-planet dials

// Launch a fleet: pull half of Player's idle surplus off A toward B.
let launched: u32 = w.issue_fleet_order(FleetOrder::new(a, b, FractionBucket::Half), Faction::Player, &wp);

// Advance the whole world one tick (steps every planet, moves fleets, resolves arrivals).
w.step(&params, &wp);

// Lens / queries for the renderer and the strategic AI:
let agg: PlanetAggregate = w.planet_aggregate(b);
let exportable = agg.fully_owned_uncontested(Faction::Player);
let neighbors: &[PlanetId] = w.neighbors(a);
let connected = w.are_connected(a, b);
let outcome: WorldOutcome = w.outcome();

// Resistance / soft-cap planet-scope signals the strategic AI reads:
let foreign_res = w.planet_total_resistance_vs(b, Faction::Player); // grind to fully own b
let parked = w.parked_count(b, Faction::Player);                    // vs the soft cap…
let cap = w.soft_cap(b, Faction::Player, &params);                  // …= softcap_free + Σ owned-sub capacity

// The shared forward-projection (pure read; never perturbs the hash). Build once per decision tick.
let proj: Projection = w.project_forward(&params, &wp, DEFAULT_PROJECTION_HORIZON);
let when: Option<u64> = proj.capture_eta(b, 0);                     // when does (b, sub 0) flip, if ever?
let saved: u64 = proj.marginal_ticks_saved(b, 0, /*from*/ 1);      // value of one more ship from sub 1
let (atk, def) = proj.expected_combat(12, 8, /*defender_in_own_sub*/ true);
let plan = proj.planet_capture(b);                                 // planet-level roll-up

// Determinism: identical construction + orders ⇒ identical hashes; clone replays bit-for-bit.
let h = w.state_hash();
```

---

## How to build & test

`world` is a workspace **default member** (in both `members` and `default-members` of the root
`Cargo.toml`):

```sh
cargo build -p world          # build the library
cargo build --workspace       # build everything (incl. the set-aside `architect`)
cargo test -p world           # the world tests (multi-planet step, arrival+capture,
                              # aggregate, determinism, AI-free smoke, projection vs reference)
cargo test                    # the whole default-members suite (layer1 stays green)
```

> **Windows note (Smart App Control, `os error 4551`).** As with the rest of the workspace,
> freshly-linked test binaries under the `Desktop/` tree can be blocked from *running* by a
> Windows app-control policy (`Une stratégie de contrôle d'application a bloqué ce fichier.`).
> The code is unaffected — it compiles and runs fine. If you hit the block, build/run the tests
> with the target directory outside that tree, e.g.:
> ```sh
> CARGO_TARGET_DIR="$HOME/.cargo-world-target" cargo test
> ```

### Tests included (`crates/world/tests/world_tests.rs`)

- **multi-planet step** — every planet's Layer-1 sim advances under one `World::step`.
- **inter-planet fleet** — a fleet launched from A arrives at B and the injected ships **capture
  a neutral sub** there; plus a **beachhead** variant (no foothold ⇒ lands at the sub facing the
  source lane).
- **junk-order safety** — unconnected/same-planet/out-of-range/neutral/no-surplus orders are
  no-ops.
- **`PlanetAggregate`** — neutral, owned (+ exportable), contested, and "incoming counts but does
  not flip the owner."
- **determinism** — two identical runs match per-tick and final `state_hash`; an extra order
  diverges; a clone replays identically.
- **AI-free 2-planet smoke** — both sides push periodic fleets and the world runs to a horizon
  without panicking, with a self-consistent outcome.
