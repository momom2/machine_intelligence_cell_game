# LAYER1_SIM — the headless Layer-1 spatial simulation

> **STATUS (latest session) — this doc lags; `CHANGELOG.md` is authoritative.** Since it was written:
> the per-sub economy / WYSIWYG orbit / grid-spread combat / reserve node landed, and seats became
> **`Faction::{Neutral, Player, Ai(u8)}`** — any number of AI opponents, declared by the level (no
> hardcoded `Enemy2`). Combat / capture / presence scan ships generically (no `[Player,Enemy,Enemy2]`
> triples; new `Structure::foreign_ship_count`/`foreign_sub_count`).

This document describes the `layer1` crate (`crates/layer1`): the **headless, deterministic
spatial micro-simulation** for **Layer 1** of the cell game — the *embodied / micro* view from
`03-ui-layers.md` ("a single structure composed of multiple sub-structures, where ships
garrison; commands here let you win 3-ships-vs-4; the operator level").

It is the **simulation only — no graphics**, honouring the design's signature principle from
`00-overview.md`: *decouple computation from spectacle*. This crate is the computation; a future
macroquad renderer is the spectacle, built directly on the types here. The crate has **zero
external dependencies** and carries its own seeded PRNG, so every run is bit-reproducible from a
seed.

> **Refresh banner (`feat/counter` era; branch now `tutorial`).** This doc has been refreshed for
> the **per-sub economy / WYSIWYG orbit / struct-storage** rework. `CHANGELOG.md` (top entries)
> is **authoritative** for these mechanics — the idle-orbit prose below summarizes the **built
> orbit model v3** (2026-07-06: separation engine + damped seek + engaged leash under the
> world-unit speed law, plus ring-band churn, membership-based reserve steering, corpse
> compaction); trust the changelog and `resolve_orbit`'s doc comment over this file if they diverge. Two operating points now coexist: `SimParams::default()`
> is the **headless / AI reference** (legacy combat + legacy per-structure soft cap), while the
> interactive game's `gui_params()` (`crates/game/src/main.rs`) flips on `spread_damage`,
> `transit_fire_gating`, and `per_sub_attrition`. The orbit is **universal** (not gated). The
> resistance / denial mechanics, the projection contract, and the determinism principle are
> largely unchanged.

---

## The model

### One structure, several sub-structures

A **`Structure`** is a *single* structure made of several **`SubStructure`**s placed at 2D
positions (`Vec2`, `f32`). Each sub-structure has:

- a **position** and a **radius** — but the radius is now **derived from storage capacity**, not
  set freely: `radius = radius_for_storage(cap) = √cap · RADIUS_PER_SQRT_STORAGE` (`0.52`), floored
  at `1.5`. (`SubStructure::new(pos, _radius, owner)` ignores its legacy `_radius` arg.) **Radius
  does NOT influence combat range** — engagement range is a fixed constant (see below); radius only
  scales the garrison ring and footprint, and matters for presence/capture.
- an **owner**: `Faction::Player`, an indexed AI seat `Faction::Ai(i)`, or `Faction::Neutral`,
- a **`storage_capacity`** (default `DEFAULT_STORAGE_CAPACITY = 60`): the no-attrition headroom of
  idle ships the sub holds (the per-sub soft cap; see Mechanic C). Its radius follows from it.
- a **`production`** (default `DEFAULT_PRODUCTION = 1`): ships minted per `production_period` (one
  per production "square"). Higher = faster output **and** a higher effective storage cap. A
  `production == 0` sub (the reserve node, below) mints nothing. Neutral subs produce nothing until
  captured. Production is the reason to hold ground and the fuel of the square-law snowball.
- a **`ring_frac`** (default `DEFAULT_RING_FRAC = 0.75`): the fraction of the radius at which idle
  ships orbit (the WYSIWYG ring; see below). Real sim state — folded into `state_hash`. Not
  player-adjustable (the mouse-wheel only zooms).

### Special sub-structure kinds (`SubKind`)

Every sub carries a `kind: SubKind` (default `Standard`; folded into `state_hash`). Three
**special** kinds each add one rule (constructors `SubStructure::fortress / teleporter /
shipyard`; stats overridable with the usual builders; `CHANGELOG.md` has the full spec):

- **`Fortress`** *(diamond)* — produces nothing; capacity 90, resistance 10 800. While owned, the
  owner's idle garrison fires at the **fixed `FORTRESS_RANGE` = 18** (one-sided overwatch:
  enemies between R = 3.5 and 12 are shot and cannot answer).
- **`Teleporter`** *(nested circles)* — produces nothing; standard capacity/resistance. The
  **owner's** departures arrive the instant their undock delay burns out (no transit leg);
  everyone else's ships leave it as ordinary movers.
- **`Shipyard { active }`** *(free-floating production squares; no disk)* — production **8**,
  storage **0** (output streams to the reserve via the auto-divert), normal-size invisible
  footprint. **Default resistance = the 1.0 token bar** whoever authors it (owner rule,
  2026-07-07: zero capacity ⇒ no resistance; 1.0 is the engine floor) — a yard flips to any
  lone visitor almost instantly. A level may opt a neutral yard into a one-time activation
  grind via `with_max_resistance`; its first capture then collapses the bar for good
  (`SHIPYARD_ACTIVE_RESISTANCE_FRAC`). Authored owned => starts active.

### The reserve / patrol-zone node (struct storage)

A `Structure` may carry one special **reserve / patrol-zone node**: `Structure::storage_sub:
Option<SubId>`, added by **`add_storage_sub()`** (called *after* the real subs) and tested with
**`is_storage(sub)`**. It is a big circle that **encloses** the producing subs — its radius is
solved so its garrison ring (including the per-ship `RING_OFFSET` jitter) clears the farthest
inner-sub edge by more than an engagement radius (`radius = (encl + 5.5) / (ring_frac −
RING_OFFSET)`), so reserve and inner-sub garrisons of opposing sides never auto-fight across the
boundary. It is the **universal inter-struct entry/exit point**: fleets arrive into it and depart
from it reserve-first (falling back to subs only while it is empty). It mints **nothing**
(`production == 0`) and has a huge `storage_capacity = STORAGE_RESERVE_CAP = 6000`. It is
**ownerless** — permanently `Neutral`, never captured (`resolve_resistance` skips it; a shared
staging space any side may sit in) — and is **excluded** from `sub_count`, `is_eliminated`,
`total_subs`, level `spec_for`, and territory tallies everywhere. (Full design + wiring points:
`memory/struct-storage.md`.)

### Ships (discrete units)

A **`Ship`** is a discrete unit with: `faction`, 2D `pos`, a `home` sub-structure (its garrison
while idle), a movement `target` (`None` = idle/garrisoning, `Some(sub)` = moving there), an `aim`
(its destination point — under WYSIWYG this is its *ring slot* on the target, not a jittered point),
an `alive` flag, an **`angle`** (its persistent orbit angle, kept through transit), and an
**`undock_remaining`** counter (ticks of undock delay left before a freshly-ordered ship starts
moving; see *Undock*). Combat removes **whole** ships via stochastic one-shot kills (matching
`01-mechanics.md`: "destroys an enemy ship when it fires"). Dead ships keep their slot so a `ShipId`
stays valid for a renderer across frames.

### Movement between sub-structures (WYSIWYG orbit + undock)

Ships move from one sub-structure to another at a fixed `ship_speed`. Under the **WYSIWYG orbit**
(universal — it applies to the headless suite too), each ship aims at **its own slot on the target's
ring** (`ring_pos(target, ship.angle)`) rather than a jittered point, so it flies straight to where
it will garrison. On arrival (within `arrival_tolerance`) a ship becomes idle and adopts the target
as its new `home`, slotting back into that sub's orbit at the same angle.

**Orbiting sub-structures (2026-07-07).** A sub may carry an authored orbit
(`SubStructure::orbiting(center, omega)`): its `pos` becomes a pure function of the tick
(`centre + radius·dir(phase + omega·tick)`, refreshed at the top of every `step` — replay-exact,
folded into the hash). Garrison rings, fortress zones, production squares and capture all read
the moving position. Ships **ordered to a moving sub lead it**: the dispatch intercept solves
time-to-arrival (undock + straight flight) against the orbit and aims at the ring as it will
stand on arrival — ships never chase. (Owner-teleporter departures lead by the undock alone.)

**Idle orbit — orbit model v3 (2026-07-06).** An idle ship physically sits at
`centre + (ring_frac + ring_offset) · radius · (cos θ, sin θ)` — its *real* combat position, not a
separate visual ring. `resolve_orbit` (a tick phase) advances `θ` by the shared spin
(`orbit_rate = TAU/200`) plus the ship's **angular urge**: every urge is computed in **world
arc-units at the ship's own radius**, summed, and clamped to ±`ship_speed` on top of the spin —
the *speed law* (idle angular motion stays within `[orbit − travel, orbit + travel]`; big rings
can never disperse faster than real flight). The urge terms (full detail in `resolve_orbit`'s doc
comment — the authoritative prose):

* **Friendly separation** (always on, two terms): *near-field* — repulsion from each same-faction
  angular neighbour, ramping to full flight speed as the arc gap closes under the comfort spacing
  `SEP_COMFORT = 2.0`; *far-field* — density pressure away from the heavier side by same-faction
  count imbalance over ±`SEP_PRESSURE_SPAN = 24` arc windows (a dense post-battle clump rarefies
  at flight speed — O(min(N·d*, πr)/v), never O(N²) edge diffusion).
* **Damped seek** (`seek_speed_frac = 0.4` × flight speed): a not-engaged ship with staged foes
  advances toward its nearest foe's bearing; separation still acts, so an approaching side stays
  spread and thickens where the front stalls it. Both sides seek — and share the spin, which
  cancels out of the convergence.
* **Engaged range-leash**: a ship engaged last combat phase (`combat_engaged`, one-tick lag)
  drops the seek, keeps separating, and clamps any step that would carry it past
  `ENGAGED_LEASH_FRAC (0.9) ×` engagement radius of arc from its nearest staged foe — fronts
  pack near comfort spacing while staying in range.
* **Peacetime polish**: with no staged foes, the gentle adjacent-midpoint relaxation
  (`orbit_relax = 0.1`, mixed ring) evens the spacing.

The urges steer **bearings only**; radial slots ride the optional `ring_jitter_step` ring-band
churn (GUI 0.03, reference 0 — no RNG headless). The ship's position then **glides** toward the
ring slot by `ORBIT_GLIDE = 0.35` (a ship spawned at a production square slides outward rather
than snapping). `ring_pos` and `insert_angle` (largest-gap insertion) place a joining ship.
Deterministic (start-of-tick snapshot; RNG only in the GUI churn).

**Undock.** A freshly-ordered ship does not leave instantly: `dispatch_move` sets
`undock_remaining = UNDOCK_TICKS = 5`, and `advance_movement` counts it down (the ship sits at its
ring slot, committed but not moving) before transit begins. Ships are orderable at all times
**except while transiting or undocking**.

### Orders — the atomic action

A faction issues a **`MoveOrder { source, target, fraction }`**: send a fraction-bucket
(`{25, 50, 75, 100}%`, the design's fixed buckets) of the **idle** ships garrisoned at `source`
to `target`. **Orders are faction-scoped**: `issue_order(order, faction)` and
`issue_order_fraction(source, target, frac, faction)` both take a `faction` argument and move **only
that faction's** idle ships at the source — an order from one seat can never drag an opponent's ships
off a contested sub (the bug this fixed). Only idle ships move (ships already in transit are not
redirected — "commit, then it's flying"). Junk orders (same source/target, out-of-range ids, empty
garrison) are safe no-ops. **Both the AI and the GUI issue orders through the same API**, as do the
`world` crate's inter-struct fleet wrappers. `idle_count_at(sub, faction)` reports a faction's idle
(home-based) presence at a sub. Note: for the struct-wide export wrappers, sending **100%** uses
keep-floor `0` (takes everything); the smaller fractions keep the old per-sub floor.

### Proximity battle bubbles (combat is positional)

Combat is **purely proximity-based on individual ship positions**, exactly per the Layer-1 spec:

> "When within a close enough distance, ships are engaged in a battle bubble. Depending on the
> layout of the structure, ships may not need to be in the same sub-structure to battle."

Define an **engagement radius `R`** — a **fixed constant** (`SimParams::engagement_radius`),
**independent of any sub's radius**. Each tick (split into `combat_substeps` sub-steps for
smoothness), every ship that has at least one **living enemy ship within `R`** is *engaged*.
So ships near the border between two close sub-structures fight **across** them — being in the
same sub-structure is **not** required. The **layout** (positions) decides who can fight whom.

A **`BattleBubble`** is a *connected cluster of mutually-in-range opposing ships* (computed by
union-find over engagement edges; a cluster with only one faction present is not a bubble).
`Structure::battle_bubbles()` exposes them (members, centre, bounding radius, per-side counts)
so the renderer can draw each brawl.

### Stochastic Lanchester square-law combat (the Layer-1 / spectacle model)

Per `01-mechanics.md`, this stochastic per-ship combat is the Layer-1 spectacle model. Each
combat sub-step:

1. for every living ship, gather its living enemies within `R`;
2. every engaged ship **fires with probability `fire_prob`** (+`defender_fire_bonus` if it sits
   inside one of *its own* sub-structures' radius);
3. on firing, it **one-shots a uniformly random in-range enemy**;
4. fire is **simultaneous within a sub-step** (shots are collected against pre-sub-step liveness,
   then applied), so neither side reacts first — no seat bias. A shot whose target was already
   downed this sub-step is wasted, keeping the kill rate honest.

Because each side fields shooters **proportional to its engaged-ship count**, the opponent's loss
rate is proportional to *your* engaged count — the **stochastic square law**. Large battles trend
deterministic (spread ~`1/sqrt(N)`); small skirmishes feel chancy. The property
**"2× ships ⇒ ~4× effective power"** emerges and is verified by tests (see below).

**Two combat paths.** `resolve_combat` dispatches on `SimParams::spread_damage`:

- **Classic** (`spread_damage = false`, the `SimParams::default()` / headless reference):
  `resolve_combat_classic` — O(N²) per sub-step; an engaged ship that fires **one-shots a single
  uniformly-random in-range enemy** (the model the test suite has always measured).
- **Spread** (`spread_damage = true`, the GUI): `resolve_combat_spread` — a **uniform grid**
  (cell = engagement radius, only the **3×3** neighbourhood inspected, no O(N²) all-pairs scan). An
  engaged ship **spreads** its fire across *all* `k` in-range enemies, hitting each with probability
  `fire_prob / k`. Expected kills per shooter stay `fire_prob`, so the **square law and the
  mean-field projection are unchanged** — only variance drops and damage feels continuous. Fully
  deterministic (buckets in ascending `ShipId`, each shooter's targets sorted before RNG draws).

**Transit-fire gating** (`SimParams::transit_fire_gating`; on in the GUI, off in the reference): a
ship **in transit** (`target.is_some()`) cannot fire on a **stationary** (idle, no-target) enemy —
an in-flight wave cannot "drive-by" shoot a garrison; it must *land* (arrive, go idle) before it can
trade with defenders. Stationary defenders still fire on the passing movers, and two movers still
fire on each other. Applies in both combat paths.

### Defender edge (sub-structure advantage)

A ship firing while inside one of its own sub-structures' radius gets `defender_fire_bonus` extra
fire probability — the Layer-1 analog of defender advantage (`01`: "you may still need an explicit
defender term to tune"). Modest by default; set to `0.0` to disable.

### Capture is a grind, not an instant flip (the resistance model)

Capture is the **resistance / denial / soft-cap** model (authoritative spec: `AUTOMATA_DESIGN.md`
§1; implemented in `Structure::resolve_resistance` / `::produce` / `::resolve_softcap`). The old
"uncontested presence flips it instantly" rule is gone. Three mechanics, all folded into
`Structure::step`:

**Resistance-grind capture (Mechanic A).** Each `SubStructure` carries a `resistance: f32` bar in
`[0, max_resistance]`, starting **full**. `max_resistance` defaults to **`storage_capacity ·
RESISTANCE_PER_CAPACITY` (`= 60`)** — `3600` for a default capacity-60 sub (~200 production
periods at the default `production_period = 18`; a bigger sub is proportionally harder to take) —
and is **per-sub overridable** via `SubStructure::with_max_resistance(max)` (clamped `>= 1.0`,
refilling the bar to that max). Each tick, with the living **present** counts
`P`/`E` of each real seat **garrisoned at** the sub (`resolve_resistance` uses the home-based
`idle_count_at`, so a ship merely passing through the radius — or sitting in the big reserve node that
encloses the inner subs — does not spuriously contest; the `presence_in_sub` radius metric is still
what the read-signal queries below report), exactly **one** of the following happens to the bar (the
**single present faction** is the only mover; if zero *or* both seats are present the grind is
**FROZEN** and nothing changes):

- **Erode** — only a *foreign* seat present (count `F`): `resistance -= F`. On reaching `<= 0` the
  sub **flips** to that seat and **refills** to `max_resistance`. (A `Neutral`-owned sub always
  erodes — no ship is `Neutral`.) Clearing a fresh sub thus takes `ceil(max_resistance / F)` ticks
  — **more present ships ⇒ faster** (a *linear* speedup on the grind itself; the square law lives
  only in combat).
- **Heal** — only the *owner* present (count `O`): `resistance = min(resistance + O, max_resistance)`.
  A returning defender repairs the bar, so a **hit-and-run accomplishes nothing**: an attacker must
  keep enough present force to out-erode the heal until the bar hits 0.

Garrisoned ships are untouched by a flip — a captured sub keeps whatever ships sit on it; only its
owner changes (and on a flip the production timer is nudged to `>= 1` so a just-seized sub does not
pop a ship the very next tick). The rule is a **pure function**, `SubStructure::capture_step(owner,
resistance, max_resistance, present_player, present_enemy) -> (new_owner, new_resistance, flipped)`
— and the **same** function is called by both the sim (`resolve_resistance`) and the world crate's
forward-projection (see `WORLD.md`), so the projection's grind can never drift from the sim.

**Production denial (Mechanic B).** `produce()` is **gated**: a sub that is being *actively eroded*
— exactly one foreign faction present and the owner **absent** (start-of-tick presence, since
`produce` runs first) — does **not** produce, and its `production_timer` is **held steady** (no
catch-up when pressure lifts). So parking on an enemy sub **starves its output before you even
capture it**. A contested-*but-defended* sub (owner *and* foe present) keeps producing — defenders
keep the line running. Neutral subs never produce.

**Anti-hoard soft cap (Mechanic C).** A self-limiting plateau on **parked** ships (living ships in
this structure — idle or intra-structure transit; inter-struct fleets live in the `world` crate and
are **cap-exempt**). `resolve_softcap` dispatches on `SimParams::per_sub_attrition`:

- **Per-sub linear attrition** (`per_sub_attrition = true`, the GUI default): `resolve_softcap_per_sub`.
  The cap is now **per sub**, not per structure. For each owned sub, the owner's idle ships above its
  `storage_capacity` are the *surplus*, and this tick destroys an expected
  `surplus / (storage_per_production · production_period)` of them (stochastic rounding via the
  structure RNG; **independent of `production`**). With `storage_per_production = 60` (`K`, "effective
  storage a point of production buys") and `production_period = 18` (denominator `1080`), production
  keeps refilling so a sub settles at an **effective cap ≈ `storage_capacity + K · production` ≈ 120**
  for the defaults. A gentle plateau, not a wall.
- **Legacy per-structure `sqrt` cap** (`per_sub_attrition = false`, the `SimParams::default()` /
  headless / AI reference): `resolve_softcap_struct`. Per real seat each tick, with
  `soft = softcap_free + Σ_{owned sub} sub.soft_cap_capacity(params)` (today every owned sub returns
  the uniform `softcap_per_sub`, so this equals `softcap_free + softcap_per_sub · owned_subs`):

  ```text
  over      = parked - soft                              (only if parked > soft)
  soft_kill = ceil(softcap_attrition * sqrt(over))       (softcap_attrition = 0.5)
  hard_kill = parked.saturating_sub(structure_hard_cap)  (structure_hard_cap = 1000; safety only)
  n         = max(soft_kill, hard_kill).min(parked)
  ```

  …then `n` parked ships are destroyed at random (idle preferred over in-transit) via the structure
  RNG. The `sqrt(over)` shape makes the count settle just *above* `soft`. There is intentionally
  **NO hard strategic ceiling** — `structure_hard_cap` is a far-above-play pathology guard, not a
  dial. `softcap_attrition` is **`0.5`** (tuned down from `1.0` so a turtle can hold a standing wall
  and out-last an over-committed aggressor — the `defend > attack` lever; see `AUTOMATA_DESIGN.md`
  §4/§6). Expressing the cap as a **sum of per-sub capacities** is a modularity hinge.

In both paths surplus must be **spent or kept moving** — inter-struct transit is the cap-exempt
escape valve — and no RNG is drawn unless at least one ship must die (so the no-attrition path leaves
the stream untouched).

> **Other bounds.** The separate **`max_ships_per_sub = 4000`** is only a lifetime-spawn safety
> bound, and `sub_orbit_cap = 50` is **positional only** — it conceptually places overflow idle ships
> at a wider orbit; it **never** destroys ships and is **not** enforced inside `resolve_softcap`.

### Win & elimination

- **Elimination**: a faction is eliminated when it has **zero ships AND zero owned
  sub-structures** (it can neither fight now nor produce later). The **reserve / patrol-zone node**
  is excluded from `sub_count` (and so from elimination, `total_subs`, and level specs): it produces
  nothing and owning it confers no territory.
- **Outcome** (`Structure::outcome()`, mirroring `cell-core`'s `MatchOutcome` spirit): if exactly
  one faction is eliminated, the other wins **by elimination**; otherwise the winner is whoever
  **leads on `ships + sub-structures`** at the horizon (exact tie ⇒ draw).

### The fixed step order

`Structure::step` advances exactly one tick in this **fixed** order (this is what makes the sim
deterministic, and the ordering is load-bearing for the mechanics):

```text
1. produce()            // gated by denial (Mechanic B); spawns via spawn_at_square, skips production==0 subs
2. advance_movement()   // moving ships step toward their aim (after undock); arrivals become idle (home = target)
3. resolve_orbit()      // idle ships advance/relax their angle and glide to the ring slot (deterministic)
4. resolve_combat()     // classic or spread square-law, combat_substeps rounds
5. resolve_resistance() // capture grind / heal / flip (Mechanic A)
6. resolve_softcap()    // anti-hoard attrition (Mechanic C: per-sub or legacy struct)
tick += 1
```

Two ordering facts matter: **combat resolves before resistance** (a defender must *survive* the
firefight to count as present for the heal, and an attacker erodes with its *post-combat* count —
so clearing the fight is a precondition for capture progress), and **resistance uses post-movement
presence** (a ship that arrives this tick is inside the radius when `resolve_resistance` runs, so it
counts toward erosion/heal on its arrival tick).

### Determinism

The only randomness is drawn from one embedded seeded PRNG (`rng::Rng`, an inline **xorshift64\***
— no `rand` crate). Randomness enters in: combat fire, soft-cap destruction (drawn **only when at
least one ship must die**, so the no-attrition path leaves the RNG stream untouched), and the
per-ship **ring-offset** draw wherever a ship is spawned or re-aimed at a ring (`spawn_ship`,
production spawns, the auto-divert, order dispatch). Two structures with the same seed and the
same orders evolve **bit-identically**; `Structure::state_hash()` gives a 64-bit FNV-1a
fingerprint of the state for exact comparison. Per sub it folds in `pos`, `radius`, `owner`,
`production_timer`, **`resistance` and `max_resistance`** (so a divergent grind is detected),
**`ring_frac`**, **`production`**, **`produce_cursor`**, and **`storage_capacity`**; the
structure's **`storage_sub`** designation; per ship `faction`, `pos`, `alive`, `home`, `target`,
`aim`, **`angle`** (the orbit phase is game state), **`ring_offset`**, **`undock_remaining`** and
**`drift_remaining`**; then the tick and the RNG position. Cloning a `Structure` clones its RNG,
so a clone replays identically (useful for renderer replay/prediction).

---

## The Layer-1 Automaton AI (the enemy mind)

`ai::Automaton` is a fixed, handwritten **reactive micro-policy** (per `02-ai-opponents.md`: the
Automaton is handwritten with **one clear exploitable flaw**). It is stateless across ticks (a
pure function of the observed `Structure`), so it is deterministic and the **same type drives
either faction**. Each decision tick it evaluates priority-ordered rules and returns `MoveOrder`s:

1. **REINFORCE a losing fight.** If an owned sub-structure is contested and locally outnumbered,
   rush all idle ships from the nearest *safe* source to it (concentration of force is a theorem).
2. **EXPAND.** Claim the nearest **uncontested** neutral sub-structure with a cheap quarter-wave —
   grow the economy before committing to fights (the colonize instinct).
3. **ASSAULT on local superiority.** Attack the nearest enemy/contested sub-structure where the
   committed stack would outnumber the defenders by `assault_margin`.
4. **MASS surplus forward.** Pull idle ships above a small home floor (`HOME_FLOOR = 3`) toward the
   most-forward owned sub-structure, building the next assault stack.

### The documented, diagnosable SEAM

> **The Automaton always commits its reserve to the *nearest* live fight (rules 1 & 3 pick the
> closest target) and rule 4 keeps pulling rear ships *forward*. It never posts a dedicated rear
> guard.** Under sustained front-line pressure its rear/home sub-structures are bled to feed the
> front and left thinly held.

This is the canonical Automaton-0 flaw from `02` ("always reinforces the frontier nearest the
enemy and leaves its rear thin"), realised spatially. It is **diagnosable** (watch it strip its
home garrison once contact happens) and **exploitable**: a small detachment that flanks wide to
the Automaton's thin rear while it is "all-in" forward. Under the resistance grind the flank no
longer *snipes* the rear in a few ticks — it parks there, which (Mechanic B) **denies the rear's
production** immediately and, given enough uncontested ticks, **grinds it to a flip**; either way
the rear is neutralised while the front is starved. The test `ai_seam_thin_rear_is_exploitable`
asserts exactly this — a scripted flank achieving **capture *or* sustained denial** of the
Automaton's rear in a majority of its 7 seeds. The `HOME_FLOOR` only *slows* the rearward drain; it
is not a real rear defence, so the seam remains open.

---

## Tunable constants and their current values

All live in `sim::SimParams` (defaults = the **headless / AI reference** operating point), except
the module constants and the AI thresholds in `ai::Automaton` / `ai`. The GUI's `gui_params()`
diverges (see the *GUI operating point* note after the table).

| Constant | Field | Value | Meaning |
|---|---|---|---|
| Engagement radius `R` | `engagement_radius` | **3.5** | A ship is engaged when a living enemy is within this **fixed** distance (independent of sub radius). Halved from the original 7.0 (smaller kill zones — attacking is less punishing); a fortress garrison fires at the separate fixed `FORTRESS_RANGE` = 18. |
| Fire probability `p` | `fire_prob` | **0.035** | Per engaged ship per combat sub-step. Expected kills/shooter = `p` in **both** combat paths (classic one-shot, or spread `p/k` across `k` enemies). Drives lethality / fight length. |
| Combat sub-steps | `combat_substeps` | **4** | Combat rounds per tick (smoothness; determinism unaffected). |
| Ship speed | `ship_speed` | **1.4** | Metres per tick while moving. |
| Arrival tolerance | `arrival_tolerance` | **0.75** | Distance at which a moving ship is "arrived" and goes idle. |
| Per-ship spread | `spread_radius` | **2.2** | Legacy fan radius. **No longer used for aim** under the WYSIWYG orbit (ships aim at their ring slot); kept as a field. |
| Production period | `production_period` | **18** | Ticks per production *period*; a sub spawns `production` ships per period (one every `period/production` ticks). |
| Defender fire bonus | `defender_fire_bonus` | **0.012** | Extra fire prob for a ship inside its own sub-structure (defender edge). `0.0` disables. |
| Per-sub spawn cap | `max_ships_per_sub` | **4000** | Safety bound on lifetime spawns per sub-structure (not a strategic dial). |
| Resistance per capacity | `RESISTANCE_PER_CAPACITY` (module const) | **60.0** | A fresh sub's `resistance = storage_capacity · this` (3600 at the default capacity 60); the master grind dial. Clearing it with `F` present attackers takes `ceil(3600/F)` ticks. Per-sub overridable via `SubStructure::with_max_resistance`. |
| Default storage capacity | `DEFAULT_STORAGE_CAPACITY` (module const) | **60** | A sub's no-attrition idle headroom (per-sub soft cap). Also sets its radius. Per-sub via `with_storage_capacity`. |
| Default production | `DEFAULT_PRODUCTION` (module const) | **1** | A sub's ships/period (production squares). Per-sub via `with_production`. |
| Radius per √storage | `RADIUS_PER_SQRT_STORAGE` (module const) | **0.52** | `radius = max(1.5, √cap · 0.52)`. Sub size follows storage; does **not** affect combat range. |
| Default ring fraction | `DEFAULT_RING_FRAC` (module const) | **0.75** | Idle-ship orbit radius as a fraction of sub radius. Not player-adjustable (the wheel only zooms). |
| Reserve node storage | `STORAGE_RESERVE_CAP` (module const) | **6000** | Storage of the reserve / patrol-zone node; produces nothing, gates inter-struct flow. |
| Orbit glide | `ORBIT_GLIDE` (module const) | **0.35** | Per-tick lerp of an idle ship's position toward its ring slot. |
| Undock ticks | `UNDOCK_TICKS` (module const) | **5** | Ticks a freshly-ordered ship waits (at its slot) before transiting. |
| Orbit rate | `orbit_rate` | **TAU/200** | Radians/tick the idle orbit angle advances (game state, hashed). |
| Damped-seek speed | `seek_speed_frac` | **0.4** | Fraction of `ship_speed` a not-engaged idle ship advances toward its nearest foe's bearing when foes share the sub's space (world arc-units at the ship's radius — the v3 speed law; bearings only, ring jitter never steered). Keeps co-garrisoned enemies from orbiting forever out of the halved reach. |
| Orbit relax | `orbit_relax` | **0.1** | How strongly an idle ship's angle is nudged toward its neighbours' midpoint (spacing relaxation). |
| Soft-cap free allowance | `softcap_free` | **20** | (Legacy struct cap) flat parked-ship headroom per faction per structure. |
| Soft-cap per-owned-sub | `softcap_per_sub` | **10** | (Legacy struct cap) parked headroom each owned sub contributes (uniform `soft_cap_capacity` today). |
| Soft-cap attrition | `softcap_attrition` | **0.5** | (Legacy struct cap) `ceil(0.5·sqrt(over))` parked ships destroyed/tick — the `defend > attack` lever. |
| Structure hard cap | `structure_hard_cap` | **1000** | (Legacy struct cap) far-above-play pathology guard, **NOT** a strategic ceiling. |
| Storage per production | `storage_per_production` | **60** | (Per-sub cap, `K`) per-sub surplus bled at `surplus/(K · production_period)`/tick ⇒ effective cap ≈ `storage + K·production`. |
| Per-sub attrition | `per_sub_attrition` | **false** (GUI: **true**) | Dispatch: per-sub linear bleed vs the legacy per-structure `sqrt` cap. |
| Transit fire gating | `transit_fire_gating` | **false** (GUI: **true**) | A mover can't fire on a stationary (idle) enemy. |
| Spread damage | `spread_damage` | **false** (GUI: **true**) | Grid-accelerated combat; fire spread `p/k` across in-range enemies. |
| Per-sub orbit cap | `sub_orbit_cap` | **50** | Positional only — never destroys, not enforced in `resolve_softcap`. |
| AI assault margin | `Automaton.assault_margin` | **1.25** | Local force ratio required to launch an assault. |
| AI reinforce threshold | `Automaton.reinforce_below` | **1.0** | Reinforce a contested sub only when locally outnumbered. |
| AI home floor | `ai::HOME_FLOOR` | **3** | Idle ships a rear sub keeps before massing surplus forward (slows, does not stop, the seam's rear drain). |

The **sample structure** (`scenario::sample_structure`) is 7 sub-structures: two opposing homes
(+12 ships each), two forward posts (+4 ships each), two neutral flank posts, and a central neutral
keep — laid out so the forward posts and keep form one proximity neighbourhood (they fight across
each other) while the homes are a reachable rear for a flank. (Sub *radii* are now derived from
storage capacity, so the legacy "radius 5 / radius 4" figures no longer apply.)

> **GUI operating point.** `SimParams::default()` above is the headless / AI reference. The
> interactive game (`crates/game/src/main.rs::gui_params()`) clones the default and overrides:
> `fire_prob = 0.0055`, `defender_fire_bonus = 0.003`, `transit_fire_gating = true`,
> `spread_damage = true`, `per_sub_attrition = true`. These behavioural changes are GUI-gated to
> protect the parked AI/test suite — **except the orbit**, which is universal (`orbit_rate` /
> `orbit_relax` / `ORBIT_GLIDE` apply to the headless suite too).

---

## How to run

From the repo root (Cargo workspace; `layer1` is a default member):

```sh
# Build everything (includes the set-aside `architect` crate, which still compiles).
cargo build --workspace

# Run the headless Automaton-vs-Automaton demo from a fixed seed.
cargo run -p layer1 --bin layer1-headless
#   (If a Windows app-control / Smart-App-Control policy blocks the freshly linked debug
#    binary with "os error 4551", build & run the release binary instead — identical output:
#    cargo run -p layer1 --release --bin layer1-headless )

# Run the Layer-1 tests (square-law property, determinism, movement, capture, outcome, AI seam).
cargo test -p layer1

# Run the whole default-members test suite.
cargo test
```

### Sample headless output (seed `0xC0FFEE1234`)

> **Stale numbers.** The transcript below predates the universal WYSIWYG orbit (which changes ship
> positions, hence combat outcomes, and folds `angle` into `state_hash`). The headless harness still
> runs `SimParams::default()` (legacy `sqrt` struct cap, classic combat), so the *shape* — opening
> build-up, then a grind that erodes subs one at a time and plateaus at the soft cap — still holds,
> but the exact tick counts and `final state hash` will differ. Re-run to capture current figures.

```
== Layer-1 headless: Automaton vs Automaton ==
seed 0xC0FFEE1234 | horizon 4000 | decision interval 4 ticks | summary every 50 ticks
params: R=7.0 fire_p=0.035 substeps=4 speed=1.40 prod_period=18 defender_bonus=0.012 spread=2.2
structure: 7 sub-structures (player_home=0, enemy_home=1, player_post=2, enemy_post=3, neutral_left=4, neutral_right=5, neutral_keep=6)

  tick | ships P/E |  subs P/E | bubbles
-------+-----------+-----------+--------
     0 |   16/16   |    2/2    |       0
    50 |   22/22   |    2/2    |       0
   100 |   28/28   |    2/2    |       0
   150 |    4/19   |    3/3    |       0
   200 |    4/21   |    3/3    |       0
   250 |    2/29   |    3/4    |       1
   300 |    0/38   |    3/4    |       0
   350 |    0/48   |    2/5    |       0
   400 |    0/63   |    2/5    |       0
   450 |    0/70   |    2/5    |       0
   ...     0/70       2/5            0      (Enemy plateaus at the soft cap: 20 + 10·5 = 70)
   700 |    0/70   |    2/5    |       0
   735 |    0/74   |    0/7    |       0

FINAL @ tick 735: winner = ENEMY (by elimination) | ships P=0 E=74 | subs P=0 E=7
final state hash: 0xC1804F4D75761C1A (identical on every rerun with this seed)
```

The run is now a **grind**, not a 70-tick blitz: an opening build-up (both homes accumulate to
~28 ships before contact), then a firefight that the Enemy wins, after which it slowly **erodes**
and flips the Player's subs one at a time (`subs P/E` drifts 3/3 → 2/5 → 0/7 over hundreds of
ticks). Note the Enemy's ship count **plateauing at exactly 70** from ~tick 450: with 5 owned subs
its soft cap is `softcap_free + softcap_per_sub·5 = 20 + 50 = 70`, so the `sqrt`-attrition holds the
parked stack there rather than letting it snowball — Mechanic C in action. The elimination lands at
tick 735, and the `state_hash` is identical on every rerun.

---

## Read-signals the resistance-era AI consumes

The resistance / denial / soft-cap mechanics added a family of **pure, deterministic query methods**
on `Structure` (and one associated function on `SubStructure`). They mutate nothing and draw no
randomness, so a strategy or the world crate's forward-projection (`WORLD.md`) can call them
mid-decision without perturbing `state_hash`. None of the AI re-derives a mechanic by hand — it asks
through these:

| Signal | Returns | What it answers |
|---|---|---|
| `idle_count_at(sub, faction)` | `usize` | Living **idle** ships of `faction` whose `home == sub` (home-based, not radius-based). This is what `resolve_resistance` (erosion/heal) and the order helpers actually use, so a passer-through or a ship in the enclosing reserve node does not spuriously contest. |
| `idle_presence_in_sub(sub, faction)` | `usize` | Living **idle** ships of `faction` inside the sub's radius (the projection seeds initial presence from this so a still-inside *moving* ship that is also a scheduled arrival is not double-counted; same radius metric as `presence_in_sub`). |
| `single_present_faction(sub)` | `Option<(Faction, u32)>` | The lone **radius**-present seat and its count, or `None` for the frozen case (zero or 2+ present). Spatial (counts movers / passers-through); the home-based grind discriminant is `capture_present_faction`. N-seat generic (one ship scan, no seat list). |
| `capture_present_faction(sub)` | `Option<(Faction, u32)>` | The lone **home-based** present seat (idle ships with `home == sub` — the grind discriminant), or `None` when zero or 2+ are present — exactly what `resolve_resistance` keys off, so "being captured by whom?" matches the sim. |
| `sub_resistance(sub)` | `(f32, f32)` | The sub's `(current, max)` resistance. |
| `total_foreign_resistance(vs_owner)` | `f32` | Sum of `resistance` over every sub **not** owned by `vs_owner` (neutral + enemy) — the total grind to fully own the structure; what a resistance-proportional colonizer sizes its wave on. |
| `parked_count(faction)` | `u32` | Living ships of `faction` in this structure (idle + intra-structure transit) — exactly what `resolve_softcap` attrites (inter-struct fleets are not here, so cap-exempt). |
| `soft_cap(faction, params)` | `u32` | `softcap_free + Σ_{owned sub} soft_cap_capacity(params)` — the parked-ship plateau for `faction`. |
| `SubStructure::soft_cap_capacity(params)` | `u32` | One owned sub's contribution to its owner's soft cap (uniform `softcap_per_sub` today; the modularity hinge for future sub types). |

`SubStructure::capture_step(owner, resistance, max_resistance, present_player, present_enemy)
-> (new_owner, new_resistance, flipped)` is the pure capture rule itself, shared verbatim by
`resolve_resistance` and the projection.

## The public API the renderer will use

```rust
use layer1::{
    Structure, SubStructure, Ship, BattleBubble, SimParams, Outcome,   // sim
    Faction, FractionBucket, MoveOrder, Vec2, SubId, ShipId,           // types
    Automaton, drive, run_auto_vs_auto,                                // ai + driver
    sample_structure, sample_params, SampleLayout,                     // scenario
};

// Build the sample world.
let (mut st, layout): (Structure, SampleLayout) = sample_structure(seed);
let params: SimParams = sample_params();

// Issue a fraction-bucket move order for a *faction's* idle ships (orders are faction-scoped).
st.issue_order(MoveOrder::new(layout.player_home, layout.neutral_keep, FractionBucket::Half), Faction::Player);

// Step the sim by one tick/frame (dt = one tick; call N times for N ticks).
st.step(&params);

// Query for drawing.
let subs:    &[SubStructure]   = &st.subs;
let ships:   &[Ship]           = &st.ships;            // skip `!alive`
let bubbles: Vec<BattleBubble> = st.battle_bubbles(&params);
let (p, e)                     = (st.ship_count(Faction::Player), st.ship_count(Faction::Enemy));

// Outcome.
let outcome: Outcome = st.outcome();

// The enemy mind (drive either seat).
let enemy = Automaton::new(Faction::Enemy);
drive(&enemy, &mut st, &params);     // decide + issue this tick

// Or run a whole AI-vs-AI match with an optional per-tick callback.
let outcome = run_auto_vs_auto(&mut st, &params, &player, &enemy, horizon, decision_interval, |tick, st| { /* draw/log */ });
```

Determinism note for the renderer: `st.clone()` clones the RNG, so a clone can be stepped ahead
for prediction and will match the real timeline tick-for-tick; `st.state_hash()` is an exact
fingerprint for replay verification.
