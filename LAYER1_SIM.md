# LAYER1_SIM — the headless Layer-1 spatial simulation

This document describes the `layer1` crate (`crates/layer1`): the **headless, deterministic
spatial micro-simulation** for **Layer 1** of the cell game — the *embodied / micro* view from
`03-ui-layers.md` ("a single structure composed of multiple sub-structures, where ships
garrison; commands here let you win 3-ships-vs-4; the operator level").

It is the **simulation only — no graphics**, honouring the design's signature principle from
`00-overview.md`: *decouple computation from spectacle*. This crate is the computation; a future
macroquad renderer is the spectacle, built directly on the types here. The crate has **zero
external dependencies** and carries its own seeded PRNG, so every run is bit-reproducible from a
seed.

---

## The model

### One structure, several sub-structures

A **`Structure`** is a *single* structure made of several **`SubStructure`**s placed at 2D
positions (`Vec2`, `f32`). Each sub-structure has:

- a **position** and a **radius** (physical extent),
- an **owner**: `Faction::Player`, `Faction::Enemy`, or `Faction::Neutral`,
- a slow **production**: every `production_period` ticks it spawns one new ship for its owner.
  Neutral sub-structures produce nothing until captured. Production is the reason to hold
  ground and the fuel of the square-law snowball.

### Ships (discrete units)

A **`Ship`** is a discrete unit with: `faction`, 2D `pos`, a `home` sub-structure (its garrison
while idle), a movement `target` (`None` = idle/garrisoning, `Some(sub)` = moving there), an
`aim` (its jittered destination point), and an `alive` flag. Combat removes **whole** ships via
stochastic one-shot kills (matching `01-mechanics.md`: "destroys an enemy ship when it fires").
Dead ships keep their slot so a `ShipId` stays valid for a renderer across frames.

### Movement between sub-structures

Ships move from one sub-structure to another at a fixed `ship_speed`, each aiming at a slightly
jittered point inside the target's radius (`spread_radius`) so a wave **fans out** rather than
stacking on one pixel. On arrival (within `arrival_tolerance`) a ship becomes idle and adopts the
target as its new `home`.

### Orders — the atomic action

A faction issues a **`MoveOrder { source, target, fraction }`**: send a fraction-bucket
(`{25, 50, 75, 100}%`, the design's fixed buckets) of the **idle** ships garrisoned at `source`
to `target`. Only idle ships move (ships already in transit are not redirected — "commit, then
it's flying"). Junk orders (same source/target, out-of-range ids, empty garrison) are safe
no-ops. **Both the AI and the future GUI issue orders through the same API**
(`Structure::issue_order`).

### Proximity battle bubbles (combat is positional)

Combat is **purely proximity-based on individual ship positions**, exactly per the Layer-1 spec:

> "When within a close enough distance, ships are engaged in a battle bubble. Depending on the
> layout of the structure, ships may not need to be in the same sub-structure to battle."

Define an **engagement radius `R`**. Each tick (split into `combat_substeps` sub-steps for
smoothness), every ship that has at least one **living enemy ship within `R`** is *engaged*.
So ships near the border between two close sub-structures fight **across** them — being in the
same sub-structure is **not** required. The **layout** (positions + radii) decides who can fight
whom.

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
`[0, max_resistance]`, starting **full**. `max_resistance` defaults to the module constant
`DEFAULT_MAX_RESISTANCE = 1800.0` (~100 production periods at the default `production_period = 18`,
a Solarmax-paced grind) and is **per-sub overridable** via `SubStructure::with_max_resistance(max)`
(clamped `>= 1.0`, refilling the bar to that max). Each tick, with the living **present** counts
`P`/`E` of each real seat inside the sub's radius (`presence_in_sub`), exactly **one** of the
following happens to the bar (the **single present faction** is the only mover; if zero *or* both
seats are present the grind is **FROZEN** and nothing changes):

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

**Anti-hoard soft cap (Mechanic C).** A self-limiting plateau on **parked** ships (living ships of
a seat in this structure — idle or intra-structure transit; inter-planet fleets live in the `world`
crate and are **cap-exempt**). Per real seat each tick, with
`soft = softcap_free + Σ_{owned sub} sub.soft_cap_capacity(params)` (today every owned sub returns
the uniform `softcap_per_sub`, so this equals `softcap_free + softcap_per_sub · owned_subs`):

```text
over      = parked - soft                              (only if parked > soft)
soft_kill = ceil(softcap_attrition * sqrt(over))       (softcap_attrition = 0.5)
hard_kill = parked.saturating_sub(structure_hard_cap)  (structure_hard_cap = 1000; safety only)
n         = max(soft_kill, hard_kill).min(parked)
```

…then `n` parked ships are destroyed at random (idle preferred over in-transit) via the structure
RNG. The `sqrt(over)` shape makes the count settle just *above* `soft` rather than slamming into a
wall. There is intentionally **NO hard strategic ceiling** — `structure_hard_cap` is a
far-above-play pathology guard, not a dial. Surplus must be **spent or kept moving**; inter-planet
transit is the cap-exempt escape valve. `softcap_attrition` is **`0.5`** (tuned down from `1.0` so
a turtle can hold a standing wall and out-last an over-committed aggressor — the `defend > attack`
lever; see `AUTOMATA_DESIGN.md` §4/§6). Expressing the cap as a **sum of per-sub capacities** is a
modularity hinge: a future "warehouse" sub type would raise the cap simply by returning a larger
`soft_cap_capacity`, with no change to the cap math, the projection, or the AI.

> **Two distinct caps.** The per-structure soft cap above is the load-bearing attrition. The
> separate **`max_ships_per_sub = 4000`** is only a lifetime-spawn safety bound, and
> `sub_orbit_cap = 50` is **positional only** — it conceptually places overflow idle ships at a
> wider orbit so one sub is not an infinitely dense dot; it **never** destroys ships and is **not**
> enforced inside `resolve_softcap` (which would draw RNG).

### Win & elimination

- **Elimination**: a faction is eliminated when it has **zero ships AND zero owned
  sub-structures** (it can neither fight now nor produce later).
- **Outcome** (`Structure::outcome()`, mirroring `cell-core`'s `MatchOutcome` spirit): if exactly
  one faction is eliminated, the other wins **by elimination**; otherwise the winner is whoever
  **leads on `ships + sub-structures`** at the horizon (exact tie ⇒ draw).

### The fixed step order

`Structure::step` advances exactly one tick in this **fixed** order (this is what makes the sim
deterministic, and the ordering is load-bearing for the mechanics):

```text
1. produce()            // gated by denial (Mechanic B)
2. advance_movement()   // moving ships step toward their aim; arrivals become idle (home = target)
3. resolve_combat()     // stochastic square-law, combat_substeps rounds
4. resolve_resistance() // capture grind / heal / flip (Mechanic A)
5. resolve_softcap()    // anti-hoard attrition (Mechanic C)
tick += 1
```

Two ordering facts matter: **combat resolves before resistance** (a defender must *survive* the
firefight to count as present for the heal, and an attacker erodes with its *post-combat* count —
so clearing the fight is a precondition for capture progress), and **resistance uses post-movement
presence** (a ship that arrives this tick is inside the radius when `resolve_resistance` runs, so it
counts toward erosion/heal on its arrival tick).

### Determinism

The only randomness is drawn from one embedded seeded PRNG (`rng::Rng`, an inline **xorshift64\***
— no `rand` crate). `Structure::step` is the sole place randomness enters — now in **two** spots:
combat fire and soft-cap destruction (the latter draws **only when at least one ship must die**, so
the no-attrition path leaves the RNG stream untouched and prior hashes unchanged). Two structures
with the same seed and the same orders evolve **bit-identically**; `Structure::state_hash()` gives a
64-bit FNV-1a fingerprint of the entire state (every sub-structure — **including its `resistance`
and `max_resistance`** so a divergent grind is detected — every ship, the tick, and the RNG
position) for exact comparison. Cloning a `Structure` clones its RNG, so a clone replays identically
(useful for renderer replay/prediction).

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

All live in `sim::SimParams` (defaults = the operating point the runner and tests use), except the
AI thresholds in `ai::Automaton` / `ai`:

| Constant | Field | Value | Meaning |
|---|---|---|---|
| Engagement radius `R` | `engagement_radius` | **7.0** | A ship is engaged when a living enemy is within this distance. Defines the battle bubble; larger ⇒ fights start sooner and across wider gaps. |
| Fire probability `p` | `fire_prob` | **0.035** | Per engaged ship per combat sub-step; on firing it one-shots a random in-range enemy. Drives lethality / fight length. |
| Combat sub-steps | `combat_substeps` | **4** | Combat rounds per tick (smoothness; determinism unaffected). |
| Ship speed | `ship_speed` | **1.4** | Metres per tick while moving. |
| Arrival tolerance | `arrival_tolerance` | **0.75** | Distance at which a moving ship is "arrived" and goes idle. |
| Per-ship spread | `spread_radius` | **2.2** | Ships fan across a disk this size at the destination (cosmetic; keeps units from overlapping). |
| Production period | `production_period` | **18** | Ticks between ship spawns at an owned sub-structure. Smaller ⇒ faster snowball. |
| Defender fire bonus | `defender_fire_bonus` | **0.012** | Extra fire prob for a ship inside its own sub-structure (defender edge). `0.0` disables. |
| Per-sub spawn cap | `max_ships_per_sub` | **4000** | Safety bound on lifetime spawns per sub-structure (not a strategic dial). |
| Fresh-sub resistance | `DEFAULT_MAX_RESISTANCE` (module const) | **1800.0** | A sub's starting / refill `resistance`; the master grind dial. Clearing it with `F` present attackers takes `ceil(1800/F)` ticks. Per-sub overridable via `SubStructure::with_max_resistance`. |
| Soft-cap free allowance | `softcap_free` | **20** | Flat parked-ship headroom per faction per structure (the `soft = softcap_free + Σ owned-sub capacity` floor). |
| Soft-cap per-owned-sub | `softcap_per_sub` | **10** | Parked headroom each owned sub contributes (uniform `soft_cap_capacity` today); equilibrium surplus ≈ 10× production. |
| Soft-cap attrition | `softcap_attrition` | **0.5** | `ceil(0.5·sqrt(over))` parked ships destroyed/tick above the soft cap (a plateau, not a wall). Tuned down from `1.0` — the `defend > attack` lever. |
| Structure hard cap | `structure_hard_cap` | **1000** | Far-above-play pathology guard, **NOT** a strategic ceiling (there is intentionally none). |
| Per-sub orbit cap | `sub_orbit_cap` | **50** | Positional only — places overflow idle ships at a wider orbit; never destroys, not enforced in `resolve_softcap`. |
| AI assault margin | `Automaton.assault_margin` | **1.25** | Local force ratio required to launch an assault. |
| AI reinforce threshold | `Automaton.reinforce_below` | **1.0** | Reinforce a contested sub only when locally outnumbered. |
| AI home floor | `ai::HOME_FLOOR` | **3** | Idle ships a rear sub keeps before massing surplus forward (slows, does not stop, the seam's rear drain). |

The **sample structure** (`scenario::sample_structure`) is 7 sub-structures: two opposing homes
(radius 5, +12 ships each), two forward posts (radius 4, +4 ships each), two neutral flank posts,
and a central neutral keep — laid out so the forward posts and keep form one proximity
neighbourhood (they fight across each other) while the homes are a reachable rear for a flank.

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
| `idle_presence_in_sub(sub, faction)` | `usize` | Living **idle** ships of `faction` inside the sub's radius (the projection seeds initial presence from this so a still-inside *moving* ship that is also a scheduled arrival is not double-counted; same radius metric as `presence_in_sub`). |
| `sub_presence(sub)` | `SubPresence { player, enemy, owner }` | The erosion/heal driver as one read — both seats' present counts plus the owner. |
| `single_present_faction(sub)` | `Option<(Faction, u32)>` | The lone present seat and its count, or `None` for the frozen case (zero or both present) — exactly the discriminant `capture_step` keys off. |
| `sub_resistance(sub)` | `(f32, f32)` | The sub's `(current, max)` resistance. |
| `total_foreign_resistance(vs_owner)` | `f32` | Sum of `resistance` over every sub **not** owned by `vs_owner` (neutral + enemy) — the total grind to fully own the structure; what a resistance-proportional colonizer sizes its wave on. |
| `parked_count(faction)` | `u32` | Living ships of `faction` in this structure (idle + intra-structure transit) — exactly what `resolve_softcap` attrites (inter-planet fleets are not here, so cap-exempt). |
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

// Issue a fraction-bucket move order for a sub-structure's idle ships.
st.issue_order(MoveOrder::new(layout.player_home, layout.neutral_keep, FractionBucket::Half));

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
