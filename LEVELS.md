# LEVELS — the level / campaign system (`crates/levels`)

> **Phase 3** deliverable. A headless, deterministic, fully-tested library that defines the
> game's **10-level campaign**: for each level, the GUI-facing **metadata** plus a
> `build(seed) -> (World, WorldParams)` **world-builder**, and a **headless validation harness**
> that asserts every level is well-formed, deterministic, and that each intended **lesson
> actually holds** when measured against AI proxies. No graphics.

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
  validation.rs   The headless validation harness (structure + determinism + lesson-holds).
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

## The 10 levels (as built) + the MEASURED validation

All "measured" numbers below are produced by the validation harness in
`crates/levels/src/validation.rs` and asserted by the lib tests (see **Validation** below). They
are deterministic. Win-loss is **wins-losses-draws**; the seed set is `{1, 7, 42, 2024, 31337}`.

| # | Title | View | Topology (ownership) | Enemy | Teaching goal | Measured validation |
|---|---|---|---|---|---|---|
| 1 | **First Moves** | Layer-1 | 1 planet, 3 subs in a wide triangle — **Player 1** (12 ships), **Enemy 1** (3), **Neutral 1**; subs spaced far apart so the start is peaceful | `Passive` | select a sub, send a fraction (25/50/75/100%), capture the neutral then the dormant enemy | concentration proxy **5-0-0** vs Passive (trivially winnable) |
| 2 | **Contact** | Layer-1 | 1 planet, 5 subs in groups 2 / 1 / 2 — **Player 1** (18), **Enemy 1** (10), **Neutral 3** (incl. the contested centre); inner posts sit within engagement range so they fight *across* the gaps | `GreedyLocal` | concentration of force; the layout decides who fights whom | concentration proxy **5-0-0** vs GreedyLocal |
| 3 | **Two Worlds** | Layer-2 | 2 planets + 1 lane — Homeworld **9 subs** (Player 1, 8 neutral), Outpost **5 subs** (Enemy 1, 4 neutral) | `GreedyLocal` | send a fleet between planets, zoom into a planet to micro it, enable basic automation | greedy proxy **5-0-0** vs GreedyLocal |
| 4 | **Hold the Line** | Layer-2 | 2 bigger homes + 1 long lane — Player **4 subs** (12/sub), Enemy **4 subs** (9/sub) | `GreedyLocal` | reinforce L3: lean on automation while timing the decisive cross-lane fleet | greedy proxy **5-0-0** vs GreedyLocal |
| 5 | **Three Fronts** | Layer-2 | triangle, 3 planets — Player home **3 subs** (11/sub), Enemy home **3 subs** (9/sub), shared **neutral** crossroads (2 subs); 3 lanes | `GreedyLocal` | multi-front concentration — grab the crossroads, then concentrate | greedy proxy **5-0-0** vs GreedyLocal |
| 6 | **The Prize** | Layer-2 | 5 planets — two **3-sub** homes (Player 11/sub, Enemy 9/sub), a **fat 3-sub NEUTRAL** prize in the centre, two **1-sub** forward neutral spurs; 6 lanes | `GreedyLocal` | expansion-vs-defense timing around a juicy neutral | greedy proxy **5-0-0** vs GreedyLocal |
| 7 | **The Seam** | Layer-2 | 4 planets — Player **3-sub** home (14/sub), Enemy **single-sub rear** (10) one short lane away, a 2-step **neutral bait corridor** off the rear; 3 lanes | `GreedyLocal` | exploit the greedy Automaton's documented thin-rear seam (flank its undefended rear) | scripted **rear-flank captures the rear and leads the match in 5/5 seeds** |
| 8 | **Overreach** | Layer-2 | the validated **diamond** — two **3-sub** homes (10/sub), two **1-sub** private flank neutrals, a **2-sub** contested centre; 6 lanes | `Colonize` | strike undefended production: a timed assault beats a colonizer (**attack > colonize**) | **Attack vs Colonize 10-0-0** (5 seeds × 2 seatings) |
| 9 | **The Turtle** | Layer-2 | the diamond (same as L8) | `Defend` | out-expand a turtle and win on territory (**colonize > defend**) | **Colonize vs Defend 10-0-0** (5 seeds × 2 seatings) |
| 10 | **The Hammer** | Layer-2 | the diamond (same as L8) | `Attack` | survive the assault, then punish the over-committed, emptied rear (**defend > attack**) | **Defend vs Attack 10-0-0** (5 seeds × 2 seatings) |

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
- **L4-L6 (escalating vs `GreedyLocal`).** L4 reinforces L3 on two fatter homes across a long
  lane (lean on automation, time the blow). L5 is a triangle: the neutral crossroads is a
  2-vs-1 production swing, so the lesson is *multi-front concentration*. L6 adds a fat central
  **neutral prize** plus two cheap forward spurs — *expansion-vs-defense timing*: the prize
  compounds, but over-committing to it leaves home thin. In each, the Player starts with a modest
  garrison edge so a competent player wins clearly.
- **L7 "The Seam".** The greedy Automaton holds a **single-sub** rear one short lane from the
  Player home, with a neutral bait corridor dangling off it. Greedy always ships its surplus
  toward the nearest uncontested grab and **never posts a reserve above the flat garrison floor**
  (its documented seam), so it streams its garrison down the bait corridor and leaves the rear
  floor-defended — a concentrated strike across the short lane overruns it and the captured rear
  snowballs. This is the level-scale version of the seam `AI.md` validated; the campaign
  validation re-confirms a rear-flank beats greedy here (5/5 seeds).
- **L8-L10 (one PURE Automaton each, on the diamond).** All three use the **symmetric diamond**
  — the map on which `AI.md` measured the rock-paper-scissors cycle closing cleanly. Each level
  fields one pure strategy and the validation confirms the intended counter beats it on this very
  map, over both seatings: **L8** Colonize → countered by **Attack** (strike its undefended
  production); **L9** Defend → countered by **Colonize** (out-expand the turtle); **L10** Attack
  → countered by **Defend** (punish the over-committed stack). Each level's blurb/hints point at
  the Automaton's blind spot without fully solving it.

---

## Headless validation (the real test)

`crates/levels/src/validation.rs` checks **three** things for **every** level, and the lib test
`campaign_is_well_formed_and_lessons_hold` asserts all of them pass for all 10:

1. **Structure.** The built `World` matches an independently-authored spec: planet count, each
   planet's sub-structure count and per-faction ownership `(player, enemy, neutral)`, the lane
   count, and that the intended planet pairs are lane-connected. A drift in any `build` function
   fails here immediately.
2. **Determinism.** Building the same level with the same seed twice yields the **same
   `World::state_hash`**, and a short scripted match (player-greedy vs the level's enemy) replays
   bit-for-bit (identical per-tick hashes and outcome). This re-confirms the substrate's
   determinism guarantee at the level layer.
3. **Lesson holds.** The level is sane as a *curriculum* — its intended lesson actually holds
   when measured against AI proxies:
   - **L8 / L9 / L10** — the intended counter (`Attack` / `Colonize` / `Defend` from the `ai`
     roster) **beats** the level's pure Automaton on the level's own map, over **both seatings**
     and all seeds (it must win a strict majority; it wins **all** — 10-0-0 each).
   - **L7** — a scripted **rear-flank** proxy (mass the home, punch the greedy rear across the
     short lane each decision interval) **captures the rear and leads the match** in a majority
     of seeds (it does so in **5/5**).
   - **L1-L6** — the level is **winnable**: a *competent player proxy* wins the strict majority
     of games against the level's enemy. The proxy models competence at the lens the level opens
     in:
     - **Layer-1 micro (L1/L2)** — a scripted **concentration** proxy that masses each owned
       sub's idle ships onto the nearest not-yet-owned sub each decision tick (capture-forward).
       This directly enacts the tutorials' lesson; it is the right yardstick because the generic
       greedy baseline *dribbles* surplus and never concentrates — the very mistake L2 teaches
       against — so it would understate a competent human. (Measured: the greedy baseline only
       squeaks L2 by **3-2**; the concentration proxy wins **5-0**.)
     - **Layer-2 (L3-L6)** — the greedy baseline (`Roster::GreedyLocal`) on the Player seat: the
       natural "competent player" automaton at the tactical layer.

The full measured report (printed by the `print_validation_report` lib test with `--nocapture`):

```
L 1 First Moves    structure:ok deterministic:ok lesson:ok
      lesson: Layer-1 concentration proxy vs Passive: player 5-0-0 (over 5 seeds)
L 2 Contact        structure:ok deterministic:ok lesson:ok
      lesson: Layer-1 concentration proxy vs GreedyLocal: player 5-0-0 (over 5 seeds)
L 3 Two Worlds     structure:ok deterministic:ok lesson:ok
      lesson: Layer-2 greedy proxy vs GreedyLocal: player 5-0-0 (over 5 seeds)
L 4 Hold the Line  structure:ok deterministic:ok lesson:ok
      lesson: Layer-2 greedy proxy vs GreedyLocal: player 5-0-0 (over 5 seeds)
L 5 Three Fronts   structure:ok deterministic:ok lesson:ok
      lesson: Layer-2 greedy proxy vs GreedyLocal: player 5-0-0 (over 5 seeds)
L 6 The Prize      structure:ok deterministic:ok lesson:ok
      lesson: Layer-2 greedy proxy vs GreedyLocal: player 5-0-0 (over 5 seeds)
L 7 The Seam       structure:ok deterministic:ok lesson:ok
      lesson: rear-flank captured greedy's rear and led the match in 5/5 seeds
L 8 Overreach      structure:ok deterministic:ok lesson:ok
      lesson: Attack vs Colonize on this map: 10-0-0 (5 seeds x 2 seatings)
L 9 The Turtle     structure:ok deterministic:ok lesson:ok
      lesson: Colonize vs Defend on this map: 10-0-0 (5 seeds x 2 seatings)
L10 The Hammer     structure:ok deterministic:ok lesson:ok
      lesson: Defend vs Attack on this map: 10-0-0 (5 seeds x 2 seatings)
```

### Maps that were retuned (and why)

- **L2 "Contact"** — *no map retune was needed for the lesson to hold,* but the **proxy** was
  corrected. With the generic greedy baseline as the "competent player," L2 measured only **3-2**
  — because the greedy adapter dribbles its surplus to the nearest target and never concentrates,
  which is precisely the mistake the level teaches against. Bumping the Player home garrison
  (14 → 18) did **not** move that 3-2 (small-skirmish RNG dominated). The honest fix was to use a
  proxy that models the *competence the level is about*: a **concentration** proxy (mass the
  nearest foreign ground), which wins **5-0**. This demonstrates the lesson is learnable rather
  than merely "not auto-lost." (The Player garrison is kept at 18 as a small, fair head-start for
  a tutorial.)
- **L8-L10** — built on the **diamond** topology straight from the validated `ai` harness rather
  than an invented map, precisely because `AI.md` measured the full cycle closing there (10-0 on
  every edge). The campaign validation re-measures it on the level builds and reproduces 10-0-0
  on each level. No retune was required.

Every intended lesson **holds**; none had to be reported as un-satisfiable.

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
