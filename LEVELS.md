# LEVELS — the level / campaign system (`crates/levels`)

> **Refreshed 2026-06-10** (the deep-review fix pass — see `CHANGELOG.md`, which stays
> authoritative where this doc lags). Campaign state: **full Simple** — L1 = `Passive`,
> L2–L10 = `Roster::SimpleColonize` (the stateful live `SimpleController`). **L1–L3 are
> hand-authored single-planet missions; L4–L10 are placeholder multi-planet worlds** awaiting the
> missions 1–10 redesign. The Colonize/Defend/Attack automata, the L7 seam exploit and the L8–L10
> rock-paper-scissors lessons are **parked**; lesson/difficulty validation is parked with them (a
> level gates on **structure + determinism** only). Basic player-automation is **quarantined**
> (off on every level).

A headless, deterministic, fully-tested library that defines the game's **10-level campaign**:
for each level, the GUI-facing **metadata** plus a `build(seed) -> (World, WorldParams)`
**world-builder**, and a **headless validation harness** that asserts every level is well-formed
and deterministic. No graphics.

The crate sits on top of the substrate — [`world`](WORLD.md) (the Layer-2 lens), `ai` (the
opponent brains + the validation proxies; the archived design history is in `docs/archive/`), and
`layer1` (the spatial sim used to author each planet's sub-structures and garrisons). It is wired
into the workspace `members` **and** `default-members` in the root `Cargo.toml`, and carries
**zero external dependencies**, so every level build and match replay is bit-reproducible.

```
crates/levels/src/
  lib.rs          Level + StartView + campaign() (the GUI-facing API) + the lib tests.
  builders.rs     World-authoring helpers (stocked/neutral planets; the diamond; L1-L3 author inline).
  campaign.rs     The 10 level definitions and their build(seed) world-builders.
  validation.rs   The headless validation harness (gates on structure + determinism; the
                  winnability lesson is measured with the live SeatController dispatch but not gated).
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
lvl.title;                 // "First steps"
lvl.blurb;                 // 1-2 sentence intro / framing
lvl.objective;             // the short on-screen goal
lvl.hints;                 // Vec<String> tutorial pointers (controls / tactics to teach)
lvl.enemies;               // Vec<ai::Roster> — THE seat declaration: enemies[i] drives Faction::Ai(i)
lvl.start_view;            // StartView::Layer1(PlanetId) | StartView::Layer2 (where the camera opens)
lvl.automation_available;  // whether to offer basic automation (currently false everywhere)
lvl.horizon;               // match horizon in ticks (AI-driven matches only; humans play to a sealed result)

// Instantiate the playable world (seeded; safe to call repeatedly):
let (mut world, wp) = (lvl.build)(seed);        // or lvl.world(seed)
let sim = SimParams::default();
// The host builds one ai::SeatController per enemies[i] (the shared roster→brain dispatch — the
// stateful SimpleController for Roster::SimpleColonize, the stateless AiController otherwise),
// steps World::step(&sim, &wp) each tick, applies each seat on the decision cadence, and reports
// a WIN when every Ai(i) seat is eliminated.
```

* **`enum StartView { Layer1(PlanetId), Layer2 }`** — the lens the camera opens in. The
  single-planet missions (L1–L3) open zoomed **into** their only planet's Layer-1 view
  (`Layer1(0)`); the multi-planet levels open in the Layer-2 tactical view.
* **`struct Level`** — plain data + one `fn` pointer (`build`), so it is trivially
  `Clone`/inspectable and carries no hidden state.
* **`fn campaign() -> Vec<Level>`** — the 10 levels in play order. This is the single list the
  GUI reads.

The **player** is always `Faction::Player`; the **enemy seats** are `Faction::Ai(i)`, one per
`enemies[i]` entry — the list *is* the declaration of how many opponents a level has (L3 declares
two and is a three-way free-for-all). A level is **won** when every rival seat is eliminated
(`World::outcome` folds all non-player seats into the combined-enemy slot).

---

## The 10 levels (as built)

**L1–L3 are the hand-authored missions** (single planet, Layer-1 only — with one planet the game
locks to the interior). **L4–L10 are placeholder multi-planet worlds** — leftover geometry from
the parked automata curriculum (the seam, the diamond RPS), kept playable against Simple until the
missions redesign. Difficulty is **ad-hoc** (tuned by playtest), not a curve. The structural spec
each build must satisfy lives in `validation.rs::spec_for`.

| # | Title | View | Topology (ownership) | Enemies |
|---|---|---|---|---|
| 1 | **First steps** | Layer-1 | 1 planet, 5 subs — a wide square (Player corner home, 100 ships, storage 60/prod 2; three neutral corners) around a **Passive centre fortress** (Enemy, 400 ships, storage 100/prod 3). The outer edges stay out of the centre's reach, so the player expands corner-to-corner before striking | `[Passive]` |
| 2 | **Fire in the sky** | Layer-1 | 1 planet, 6 subs — four neutral production posts in a central square (storage 60, **prod 3**) between two opposite homes (Player left / Enemy right, 60 ships, storage 60/prod 1). *The posts were authored ~11 apart to trade fire across the gaps at the old engagement radius (7); at the halved radius (3.5) that flashpoint no longer fires — re-author with the tutorial arc.* | `[SimpleColonize]` |
| 3 | **Deliberation** | Layer-1 | 1 planet, 13 subs — a horizontal neutral chain (storage 30/prod 1) from the Player start **A** (60 ships, 60/2); a **rich upper branch** (two 60/2 posts) up to **B** (Simple, 60 ships, 120/4) and a **lean lower branch** (two 30/1 posts) down to **C** (Simple, 60 ships, 90/3) — a three-way **free-for-all** | `[SimpleColonize, SimpleColonize]` |
| 4 | **Far far away** | Layer-2 | 2 bigger homes + 1 long lane — Player 4 subs vs Enemy 4 subs *(placeholder)* | `[SimpleColonize]` |
| 5 | **Three Fronts** | Layer-2 | triangle — two 3-sub homes + a 2-sub neutral crossroads, 3 lanes *(placeholder)* | `[SimpleColonize]` |
| 6 | **The Prize** | Layer-2 | 5 planets — two 3-sub homes, a fat 3-sub neutral prize (max_resistance 600 so its grind resolves in horizon), two 1-sub spurs, 6 lanes *(placeholder)* | `[SimpleColonize]` |
| 7 | **The Seam** | Layer-2 | 4 planets — Player 3-sub home, Enemy single-sub rear one short lane away, a 2-step neutral bait corridor *(placeholder; the greedy thin-rear seam lesson is parked)* | `[SimpleColonize]` |
| 8–10 | **Overreach / The Turtle / The Hammer** | Layer-2 | the **diamond** — two 3-sub homes, two 1-sub private flank neutrals, a 2-sub contested centre, 6 lanes *(placeholder; the attack≻colonize≻defend≻attack RPS lessons are parked)* | `[SimpleColonize]` |

> **Roster note.** L7–L10 keep their distinctive blurbs/hints/objectives and their topology, but
> every L2–L10 enemy seat currently fields `Roster::SimpleColonize` — the stateful, ledger-driven
> **`SimpleController`** (synchronized staggered taskforces over the projection-free
> `World::sub_influx_for`). The pure Colonize/Defend/Attack automata those flavour texts describe
> are parked until the automata track is revived.

---

## Headless validation (the real test)

`crates/levels/src/validation.rs` runs **three** checks per level; a level **gates on the first
two** — `LevelReport::ok()` is `structure_ok && deterministic`. The third is *measured and
reported* (for information) but **not gated**, because the lessons + difficulty curve are being
redesigned against Simple. The lib test `campaign_is_well_formed_and_lessons_hold` asserts `ok()`
for all 10. The seed set is `{1, 7, 42, 2024, 31337}`.

1. **Structure.** The built `World` matches an independently-authored spec (`spec_for`): planet
   count, each planet's sub-structure count and per-faction ownership `(player, enemy, neutral)`
   (both AI seats of L3 fold into the `enemy` slot), the lane count, and that the intended planet
   pairs are lane-connected. A drift in any `build` function fails here immediately. **The
   per-planet counts exclude the reserve / storage node** (see below).
2. **Determinism.** Building the same level with the same seed twice yields the same
   `World::state_hash`, and a short scripted match — player-greedy vs **every declared enemy
   seat**, each driven through the same `ai::SeatController` dispatch the game uses, at the
   game's reference decision cadence (`GAME_DECISION_BASE = 5`) — replays bit-for-bit (identical
   per-tick hashes and outcome).
3. **Winnability *(measured, not gated)*.** `not_auto_lost`: a *competent player proxy* plays the
   level against its full enemy seating over the seed set, and the report quotes the win-loss
   tally. The proxy models competence at the lens the level opens in:
     * **Layer-1 missions (L1–L3)** — a scripted **concentration** proxy that masses each owned
       sub's idle ships onto the nearest **capturable** sub (neutrals first, never the ownerless
       storage node) each decision tick — it enacts the tutorials' "mass, don't dribble" lesson.
     * **Layer-2 levels (L4–L10)** — the greedy baseline (`Roster::GreedyLocal`) on the Player
       seat: the natural "competent player" automaton at the tactical layer.

   The specialised **automata-track** lessons — the RPS-counter check (`counter_beats_enemy`) and
   the seam-flank check (`seam_flank_beats_greedy`) — are kept as documented dormant
   `#[allow(dead_code)]` for the automata revival. Neither is invoked.

### The reserve / storage node (every planet)

Every campaign planet carries an **ownerless reserve / patrol-zone node** (`add_storage_sub`,
called by the builders' planet helpers and by the L1–L3 inline builds): the universal inter-planet
entry/exit chokepoint — fleets arrive into it and depart reserve-first. It is permanently
**Neutral and never captured**, produces nothing, and is **excluded from territory everywhere**
(`sub_count`, `is_eliminated`, `total_subs`, the `spec_for` counts, production), so the validation
specs are unaffected by it. Full design + wiring points: `CHANGELOG.md` (struct-storage section)
and the agent note `memory/struct-storage.md`. (L1 overrides its reserve capacity to 10 000 — a
single-planet staging buffer the over-cap corner production auto-flows into.)

---

## How to build & test

```sh
cargo build -p levels                       # the library
cargo test  -p levels metadata_is_complete  # fast metadata gate (sub-second)
cargo test  -p levels                       # the full validation sweep (~2 min: every level's
                                            # structure + determinism + the winnability tally,
                                            # printed by print_validation_report --nocapture)
```

Current state (2026-06-10): `cargo test -p levels` → **3 passed** (+1 doctest) under the
all-seats / live-Simple drivers; `cargo check --workspace` zero warnings. The old Windows
Smart-App-Control note (`os error 4551`, blocked freshly-linked binaries) is historical — the
policy has been disabled and binaries run normally.
