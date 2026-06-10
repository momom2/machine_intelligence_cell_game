# LEVELS — the level / campaign system (`crates/levels`)

> **STATUS (latest session) — this doc lags; `CHANGELOG.md` is authoritative.** Since it was last
> refreshed: **`Level.enemy` + `enemy2` → `enemies: Vec<Roster>`** (the level declares its seats;
> `enemies[i]` drives `Faction::Ai(i)`), basic player-automation is **quarantined** (off on every
> level), and **L1–L3 are hand-authored single-planet levels** while **L4–L10 are placeholder
> multi-planet worlds** awaiting redesign.

> **Refreshed for the `feat/counter` full-Simple campaign.** The campaign now runs **L1 =
> Passive, L2–L10 = `Roster::SimpleColonize`**; the Colonize/Defend/Attack automata, the L7 seam
> exploit, and the L8–L10 rock-paper-scissors lessons are **parked** while the levels + difficulty
> curve are redesigned against Simple before the automata track is revived. Lesson/difficulty
> validation is parked too: a level now gates on **structure + determinism only**. Level
> titles/topology and the builders' geometry are unchanged. `CHANGELOG.md` (top `feat/counter`
> entry) is authoritative where this doc lags.

> **Phase 3** deliverable. A headless, deterministic, fully-tested library that defines the
> game's **10-level campaign**: for each level, the GUI-facing **metadata** plus a
> `build(seed) -> (World, WorldParams)` **world-builder**, and a **headless validation harness**
> that asserts every level is well-formed and deterministic (lesson-holds checks parked). No
> graphics.

The crate sits on top of the substrate — [`world`](WORLD.md) (the Layer-2 lens), [`ai`](AI.md)
(the opponent roster + the validation proxies), and `layer1` (the spatial sim used to author each
planet's sub-structures and garrisons). It is wired into the workspace `members` **and**
`default-members` in the root `Cargo.toml`, and carries **zero external dependencies**, so every
level build and match replay is bit-reproducible.

```
crates/levels/src/
  lib.rs          Level + StartView + campaign() (the GUI-facing API) + the lib tests.
  builders.rs     World-authoring helpers (single-planet layouts; the validated diamond; etc.).
  campaign.rs     The 10 level definitions and their build(seed) world-builders.
  validation.rs   The headless validation harness (gates on structure + determinism; lesson-holds measured but parked).
```

---

## The API the GUI consumes

```rust
use levels::{campaign, Level, StartView, Roster};
use layer1::{Faction, SimParams};

let levels: Vec<Level> = campaign();            // the 10 levels, in order
let lvl = &levels[0];

// Metadata drives the UI:
lvl.id;                    // 1..=10
lvl.title;                 // "First Moves"
lvl.blurb;                 // 1-2 sentence intro / framing
lvl.objective;             // the short on-screen goal
lvl.hints;                 // Vec<String> tutorial pointers (controls / tactics to teach)
lvl.enemy;                 // ai::Roster — the Faction::Enemy Automaton this level fields
lvl.start_view;            // StartView::Layer1(PlanetId) | StartView::Layer2 (where the camera opens)
lvl.automation_available;  // whether to offer the player's "basic automation" toggle
lvl.horizon;               // match horizon in ticks (decides the winner if neither is eliminated)

// Instantiate the playable world (seeded; safe to call repeatedly):
let (mut world, wp) = (lvl.build)(seed);        // or lvl.world(seed)
let sim = SimParams::default();
// The host then runs the world: player orders + an ai::AiController::from_roster(Faction::Enemy,
// lvl.enemy) for the opponent, World::step(&sim, &wp) each tick, and reports a WIN when
// World::outcome().winner == Some(Faction::Player).
```

* **`enum StartView { Layer1(PlanetId), Layer2 }`** — the lens the camera opens in. The two
  single-structure tutorials open zoomed **into** their (only) planet's Layer-1 view
  (`Layer1(0)`); every other level opens in the Layer-2 tactical view. (A Layer-2 level still
  lets the player zoom *into* a planet — that is the Layer-1 view of that planet.)
* **`struct Level`** — plain data + one `fn` pointer (`build`), so it is trivially
  `Clone`/inspectable and carries no hidden state.
* **`fn campaign() -> Vec<Level>`** — the 10 levels in play order (the difficulty / teaching
  progression). This is the single list the GUI reads.

The **player** is always `Faction::Player`; the **enemy** seat is `Faction::Enemy`, driven by the
level's `enemy` roster entry. A level is **won** when `World::outcome()` favours `Faction::Player`
(by world-wide elimination, or by leading on `total ships + total owned subs` at the horizon).

---

## The 10 levels (as built) + the validation

The structure/topology below is the authored geometry the builders produce (asserted by the
validation harness in `crates/levels/src/validation.rs`). **Enemy** is the level's current roster
entry; **Lesson (parked)** is the curriculum lesson each level was *originally designed around* —
those lessons are now informational only, not gated, while the campaign runs against Simple (see
the banner + **Validation** below). The seed set is `{1, 7, 42, 2024, 31337}`.

| # | Title | View | Topology (ownership) | Enemy | Lesson (parked) |
|---|---|---|---|---|---|
| 1 | **First Moves** | Layer-1 | 1 planet, 3 subs in a wide triangle — **Player 1** (12 ships), **Enemy 1** (3), **Neutral 1**; subs spaced far apart so the start is peaceful | `Passive` | select a sub, send a fraction (25/50/75/100%), capture the neutral then the dormant enemy |
| 2 | **Contact** | Layer-1 | 1 planet, 5 subs in groups 2 / 1 / 2 — **Player 1** (18), **Enemy 1** (10), **Neutral 3** (incl. the contested centre); inner posts sit within engagement range so they fight *across* the gaps | `SimpleColonize` | concentration of force; the layout decides who fights whom |
| 3 | **Two Worlds** | Layer-2 | 2 planets + 1 lane — Homeworld **9 subs** (Player 1, 8 neutral), Outpost **5 subs** (Enemy 1, 4 neutral) | `SimpleColonize` | send a fleet between planets, zoom into a planet to micro it, enable basic automation |
| 4 | **Hold the Line** | Layer-2 | 2 bigger homes + 1 long lane — Player **4 subs** (12/sub), Enemy **4 subs** (9/sub) | `SimpleColonize` | reinforce L3: lean on automation while timing the decisive cross-lane fleet |
| 5 | **Three Fronts** | Layer-2 | triangle, 3 planets — Player home **3 subs** (11/sub), Enemy home **3 subs** (9/sub), shared **neutral** crossroads (2 subs); 3 lanes | `SimpleColonize` | multi-front concentration — grab the crossroads, then concentrate |
| 6 | **The Prize** | Layer-2 | 5 planets — Player home **3 subs** (14/sub), Enemy home **3 subs** (9/sub), a **fat 3-sub NEUTRAL** prize in the centre (reduced capture resistance 600), two **1-sub** forward neutral spurs; 6 lanes | `SimpleColonize` | expansion-vs-defense timing around a juicy neutral |
| 7 | **The Seam** | Layer-2 | 4 planets — Player **3-sub** home (14/sub), Enemy **single-sub rear** (10) one short lane away, a 2-step **neutral bait corridor** off the rear; 3 lanes | `SimpleColonize` | *(automata track, parked)* exploit the greedy Automaton's documented thin-rear seam (flank its undefended rear) |
| 8 | **Overreach** | Layer-2 | the **diamond** — two **3-sub** homes (10/sub), two **1-sub** private flank neutrals, a **2-sub** contested centre; 6 lanes | `SimpleColonize` | *(automata track, parked)* strike undefended production: a timed assault beats a colonizer (**attack > colonize**) |
| 9 | **The Turtle** | Layer-2 | the diamond (same as L8) | `SimpleColonize` | *(automata track, parked)* out-expand a turtle and win on territory (**colonize > defend**) |
| 10 | **The Hammer** | Layer-2 | the diamond (same as L8) | `SimpleColonize` | *(automata track, parked)* survive the assault, then punish the over-committed, emptied rear (**defend > attack**) |

> **Roster note.** L7–L10 keep their distinctive *blurbs/hints/objectives* (the seam, the
> colonizer, the turtle, the hammer) and their *topology*, but every L2–L10 enemy seat currently
> fields `Roster::SimpleColonize`. The pure Colonize/Defend/Attack Automata that those flavour
> texts describe are parked until the automata track is revived and the difficulty curve is
> retuned against Simple.

### Per-level notes

- **L1 "First Moves" (movement).** The three sites form a wide, shallow triangle ~26-30 units
  apart, far beyond the engagement radius (7), so nothing fights at the start — the lesson is
  *movement and capture*. With a `Passive` enemy (issues nothing, internals idle) the player
  simply ships a wave to the neutral apex, then to the inert enemy site. Trivially winnable.
- **L2 "Contact" (combat).** The five sites are a left pair (home + inner-left neutral), a centre
  keep, and a right pair (inner-right neutral + enemy home). The three inner posts (`x = -8, 0,
  +8`) are 8 apart, so with radius 4 and engagement radius 7 they fire **across** the gaps once
  ships garrison them — the centre keep is the flashpoint. The two homes (`x = ±22`) are out of
  range of each other, so the fight is decided in the middle: *concentrate or stall.*
- **L3 "Two Worlds" (Layer-2 + automation intro).** The Homeworld is a deliberately wide
  internal frontier (1 owned sub, 8 neutral) so there is plenty to expand into with
  micro/automation while a fleet ships down the single lane to the greedy Outpost (1 owned sub, 4
  neutral). `automation_available = true` first appears here.
- **L4-L6 (escalating).** L4 reinforces L3 on two fatter homes across a long lane (lean on
  automation, time the blow). L5 is a triangle: the neutral crossroads is a 2-vs-1 production
  swing, so the lesson is *multi-front concentration*. L6 adds a fat central **neutral prize**
  (its subs carry a reduced capture resistance of 600 vs the 1800 default, so the contest resolves
  within the horizon) plus two cheap forward spurs — *expansion-vs-defense timing*: the prize
  compounds, but over-committing to it leaves home thin. In each, the Player starts with a modest
  garrison edge. All three currently field `Roster::SimpleColonize`.
- **L7 "The Seam" (automata lesson parked).** The topology is unchanged — the enemy holds a
  **single-sub** rear one short lane from the Player home, with a neutral bait corridor dangling
  off it — but the seat currently fields `Roster::SimpleColonize`, not the greedy Automaton the
  seam exploit was designed against. The thin-rear seam lesson (greedy never posts a reserve, so a
  concentrated flank across the short lane overruns the rear) is **parked** with the automata
  track; its scripted check (`seam_flank_beats_greedy`) is kept dormant for the redesign.
- **L8-L10 (PURE Automaton lessons parked).** All three keep the **symmetric diamond** topology —
  the map on which `AI.md` measured the rock-paper-scissors cycle closing cleanly — and their
  distinctive blurbs (Overreach the colonizer, the Turtle, the Hammer). But each seat currently
  fields `Roster::SimpleColonize`; the pure Colonize/Defend/Attack strategies and the RPS counter
  lessons (Attack > Colonize, Colonize > Defend, Defend > Attack) are **parked** until the
  automata track is revived. The RPS check (`counter_beats_enemy`) is kept dormant for that
  redesign.

---

## Headless validation (the real test)

`crates/levels/src/validation.rs` runs **three** checks per level, but during the full-Simple
transition a level **gates on only the first two** — `LevelReport::ok()` is
`structure_ok && deterministic`. The lesson check is still *measured and reported* (for
information) but **not gated**, because the lessons + difficulty curve are being redesigned against
Simple. The lib test `campaign_is_well_formed_and_lessons_hold` asserts `ok()` for all 10:

1. **Structure.** The built `World` matches an independently-authored spec (`spec_for`): planet
   count, each planet's sub-structure count and per-faction ownership `(player, enemy, neutral)`,
   the lane count, and that the intended planet pairs are lane-connected. A drift in any `build`
   function fails here immediately. **The expected counts are unchanged from the pre-reserve era:
   `planet_aggregate`'s `sub_count` excludes the reserve / storage node, so the per-planet
   `(total_subs, …)` specs do not count it** (see the reserve-node note below).
2. **Determinism.** Building the same level with the same seed twice yields the **same
   `World::state_hash`**, and a short scripted match (player-greedy vs the level's enemy) replays
   bit-for-bit (identical per-tick hashes and outcome). This re-confirms the substrate's
   determinism guarantee at the level layer.
3. **Lesson holds *(measured, not gated)*.** `check_lesson` is now a uniform `not_auto_lost`: for
   **every** level it runs a *competent player proxy* against the level's enemy over the seed set
   and reports the win-loss tally. The proxy models competence at the lens the level opens in:
     - **Layer-1 micro (L1/L2)** — a scripted **concentration** proxy that masses each owned
       sub's idle ships onto the nearest not-yet-owned sub each decision tick (capture-forward).
       This directly enacts the tutorials' lesson; it is a better yardstick than the generic
       greedy baseline, which *dribbles* surplus and never concentrates — the very mistake L2
       teaches against.
     - **Layer-2 (L3-L10)** — the greedy baseline (`Roster::GreedyLocal`) on the Player seat: the
       natural "competent player" automaton at the tactical layer.

   The specialised **automata-track** lessons are **parked**: the RPS-counter check
   (`counter_beats_enemy`, for the old L8-L10 pure Colonize/Defend/Attack) and the seam-flank
   check (`seam_flank_beats_greedy`, for the old L7) are kept as documented dormant
   `#[allow(dead_code)]` for when the automata track is revived. Neither is currently invoked.

The validation report (printed by the `print_validation_report` lib test with `--nocapture`) now
shows `structure:ok deterministic:ok` for all 10 levels, with the informational `not_auto_lost`
lesson line under each (the Layer-1 concentration proxy for L1/L2, the Layer-2 greedy proxy for
L3-L10, each vs the level's `enemy` — `Passive` for L1, `SimpleColonize` for L2-L10).

### Maps / proxies of note

- **L2 "Contact"** — the **concentration** proxy (mass the nearest foreign ground) is the right
  competence model for the Layer-1 tutorials, where the generic greedy baseline dribbles surplus
  and never concentrates (the mistake L2 teaches against). The Player home garrison is kept at 18
  as a small, fair head-start for a tutorial.
- **L8-L10** — built on the **diamond** topology straight from the validated `ai` harness (the
  map on which `AI.md` measured the full RPS cycle closing, 10-0 on every edge). The topology is
  retained for the parked automata track; the levels currently field `SimpleColonize`.

The reserve-node addition (below) **did not change** any structural spec count, because the
reserve sub is excluded from `sub_count` and from territory aggregation everywhere.

### The reserve / storage node (every planet)

Per the `feat/counter` struct-storage work, **every campaign planet now carries a reserve /
patrol-zone node**: the three planet helpers in `builders.rs` — `authored_planet`,
`stocked_planet`, and `neutral_planet_res` (and so `neutral_planet`, which delegates to it) — call
`st.add_storage_sub()` immediately before `Planet::new(...)`. This is the inter-planet entry/exit
chokepoint (fleets arrive into and depart from it). It produces nothing and is **excluded from
territory everywhere** (`sub_count`, `is_eliminated`, `total_subs`, level `spec_for`, production),
so the validation specs above are unaffected. Full design + wiring points are in `CHANGELOG.md`
(the struct-storage section) and the agent note `memory/struct-storage.md`.

---

## How to build & test

`cargo build` does not need the Windows workaround; **running** freshly-linked test binaries on
this machine does (see below).

```sh
cargo build -p levels          # the library
cargo build --workspace        # everything (incl. the set-aside `architect`)
cargo test  -p levels          # the campaign validation (3 lib tests + 1 doctest)
cargo test                     # the whole default-members suite
```

### Verified results (this machine)

Using a `CARGO_TARGET_DIR` outside the `Desktop/` tree (see the note below), `--release`:

- **`cargo test -p levels`** → **3 passed; 0 failed** (`campaign_is_well_formed_and_lessons_hold`,
  `metadata_is_complete`, `print_validation_report`) + **1 doctest passed**. The validation sweep
  takes ~90 s (it runs every level's lesson check across the seed set, including the L8-10
  both-seatings matches to the full horizon).
- Existing **default members still green** (unchanged by this phase): `ai` 30, `automaton` 18
  (+4 ignored), `cell-core` 11 (+1 ignored) lib + 18 integration, `layer1` 14 lib + 16
  integration, `world` 14 integration. `cargo build --workspace` succeeds with **no warnings**.
  > The `r2-sweep` **binary** (a 0-test `main.rs`) could not be *run* on this machine because the
  > Windows app-control policy persistently blocked that particular freshly-linked binary
  > (`os error 4551`); it contains no tests and is a pre-existing crate untouched by this phase,
  > so this does not affect any result.

### Windows note (Smart App Control, `os error 4551`)

As elsewhere in this workspace, freshly-linked **test/bin** binaries under the `Desktop/` tree can
be blocked from *running* by a Windows app-control policy
(`Une stratégie de contrôle d'application a bloqué ce fichier.`). The code is unaffected — it
compiles fine. Build/run the tests with the target dir outside that tree, e.g. PowerShell:

```powershell
$env:CARGO_TARGET_DIR="$env:TEMP\mi-levels-target"   # any path off the Desktop tree
cargo test -p levels --release
```

The block is reputation-based and per-binary: a freshly-linked binary may be blocked on its first
invocation(s) and then run on a retry, or run from a different target path. The validation is kept
as **lib** tests (which proved reliably runnable) rather than a separate `tests/` integration
target.
