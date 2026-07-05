# LEVELS — the level / campaign system (`crates/levels`)

> **Refreshed 2026-06-10** (the deep-review fix pass — see `CHANGELOG.md`, which stays
> authoritative where this doc lags). Campaign state: **full Simple** — L1 = `Passive`,
> L2–L10 = `Roster::SimpleColonize` (the stateful live `SimpleController`). **L1–L3 are
> hand-authored single-struct missions; L4–L10 are placeholder multi-struct worlds** awaiting the
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
`layer1` (the spatial sim used to author each struct's sub-structures and garrisons). It is wired
into the workspace `members` **and** `default-members` in the root `Cargo.toml`, and carries
**zero external dependencies**, so every level build and match replay is bit-reproducible.

```
crates/levels/src/
  lib.rs          Level + StartView + campaign() (the GUI-facing API) + the lib tests.
  builders.rs     World-authoring helpers (stocked/neutral structs; the diamond; L1-L3 author inline).
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
lvl.start_view;            // StartView::Layer1(StructId) | StartView::Layer2 (where the camera opens)
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

* **`enum StartView { Layer1(StructId), Layer2 }`** — the lens the camera opens in. The
  single-struct missions (L1–L3) open zoomed **into** their only struct's Layer-1 view
  (`Layer1(0)`); the multi-struct levels open in the Layer-2 tactical view.
* **`struct Level`** — plain data + one `fn` pointer (`build`), so it is trivially
  `Clone`/inspectable and carries no hidden state.
* **`fn campaign() -> Vec<Level>`** — the 10 levels in play order. This is the single list the
  GUI reads.

The **player** is always `Faction::Player`; the **enemy seats** are `Faction::Ai(i)`, one per
`enemies[i]` entry — the list *is* the declaration of how many opponents a level has (L3 declares
two and is a three-way free-for-all). A level is **won** when every rival seat is eliminated
(`World::outcome` folds all non-player seats into the combined-enemy slot).

---

## The campaign plan — three arcs (owner's design, 2026-07-03)

**Player-experience objective:** make the player *feel like a heartless automaton* — first
exploring basic mechanics, then relentlessly optimizing their gameplay against progressively
more complex strategies and situations. **Lore:** the player is a self-improving AI tasked with
military strategy, first in a simulation, then deployed in the real world. The developer logs
(the second, human voice beside the in-fiction mission briefings) implicitly track the devs
being proud of their creation → dissociating as it acts too machine-like (e.g. a battle where
progress requires sacrificing hundreds of ships, which they deem inhuman) → unable to steer it
as it soars into a self-sustained paperclip-like empire.

**Arc 1 — Tutorial** (mechanics in order: movement → combat → logistics + fortresses →
multi-agent → teleporters + shipyard-as-target → Layer 2 → wrap-up). The specials rule:
**threat first, possession second** — the player meets each special as an enemy-held problem
before mastering it themselves.

1. **First steps** — movement (as built).
2. **Fire in the sky** — combat + massing. *Needs re-authoring: its middle-square flashpoint
   was tuned to the old engagement radius (7) and died at 3.5.*
3. **The Sinews of War** — logistics + the fortress (**BUILT**, see table; briefing copy
   pending). Sub variety through function: shipyard (production), warehouses (forward storage
   vs the reserve ring), fortresses met threat-first (the enemy's thin middle fort grows into
   a wall as Simple mans it; its fort doctrine — floor = capacity, never evacuates — then
   keeps it standing) and possession-second (the outer wall forts start neutral and empty; a
   player-manned one reaches into the middle fort's own ground — the wall can be turned). The
   back-fort-covered endgame is the first mild taste of pay-the-toll arithmetic. Uses the
   per-level reserve dial (0.6× — one dense battlefield, staging adjacent rather than
   interplanetary).
4. **Deliberation** — multi-agent free-for-all (as built).
5. *(planned)* **The teleporter mission** — the counter to fortresses: an impregnable enemy
   fortress line, frontally hopeless and geometrically unflankable; a neutral gate dissolves it
   (owned-gate departures hop instantly; the walk to the gate crosses no gauntlet). Behind the
   line, the enemy's **active shipyard** — the "head of the serpent" — feeds the wall; the deep
   strike through the gate decapitates the economy and the line starves. Mobility doesn't
   fight defense, it invalidates it. (The *contested-activation* shipyard flavour — the 10 800
   neutral grind as a king-of-the-hill objective — is deliberately saved for Arc 2.)
6. *(planned)* **The Layer-2 revelation** — two structs presented as one: `StartView::Layer1`,
   no lens mention anywhere, diegetic hints only ("where do they keep coming from?"). Simple's
   funnel keeps landing reinforcements in the enemy reserve; the off-screen arrows accumulate
   at the frame edge pointing at ships from *somewhere out there*; wheeling out past minimum
   zoom IS the discovery — the interface is the epiphany. The sandbox was always bigger than
   the box. (Ships on other structs deliberately show nothing.)
7. *(planned)* **Wrap-up** — multi-struct synthesis: wall + gate + yard + real logistics.
   *Open question:* whether the first full sacrifice battle lands here or opens Arc 2 as its
   thesis statement.

**Arc 2 — Mastering the basics:** escalating missions vs Simple, fair → unfair (per-level
`SimpleParams` dials scale the brain; a fortress-naïve Simple is `fort_toll = 0`). Reserved
material: the contested shipyard activation, the `STORAGE_ENEMY_BLOCK` reserve-blockade
mechanic (currently untaught), and the inhuman-sacrifice set-piece battle.

**Arc 3 — Automation + new enemies:** parked pending its own design discussion (player
automation redesign, greedy rework, automata/Counter revival).

None of the L5–L11 placeholder topologies or briefings survive the arc; only M1's briefing is
final copy. Mission count is decided as missions are made. All new layouts are authored at the
3.5 engagement radius and the corrected game scale (the reserve ring is a far outer orbit — a
fort can cover an *approach*, never the ring itself).

## The 11 levels (as built)

**L1–L4 are the hand-authored missions** (single struct, Layer-1 only — with one struct the game
locks to the interior). **L5–L11 are placeholder multi-struct worlds** — leftover geometry from
the parked automata curriculum (the seam, the diamond RPS), kept playable against Simple until
the tutorial arc replaces them. Difficulty is **ad-hoc** (tuned by playtest), not a curve.

| # | Title | View | Topology (ownership) | Enemies |
|---|---|---|---|---|
| 1 | **First steps** | Layer-1 | 1 struct, 5 subs — a wide square (Player corner home, 100 ships, storage 60/prod 2; three neutral corners) around a **Passive centre fortress** (Enemy, 400 ships, storage 100/prod 3). The outer edges stay out of the centre's reach, so the player expands corner-to-corner before striking | `[Passive]` |
| 2 | **Fire in the sky** | Layer-1 | 1 struct, 6 subs — four neutral production posts in a central square (storage 60, **prod 3**) between two opposite homes (Player left / Enemy right, 60 ships, storage 60/prod 1). *The posts were authored ~11 apart to trade fire across the gaps at the old engagement radius (7); at the halved radius (3.5) that flashpoint no longer fires — re-author with the tutorial arc.* | `[SimpleColonize]` |
| 3 | **The Sinews of War** | Layer-1 | 1 struct, 15 subs — **left (player):** an active **shipyard** (starts with 1 ship; output pools at the yard up to the invisible 120 virtual cap) + two neutral **200-cap/1-prod warehouses** (default 12 000 resistance — a deliberate midgame investment); **middle:** a vertical wall of three mutually covering **fortresses** (20 apart, reach ~21.7 at `FORTRESS_RANGE` 18) — only the **middle** starts enemy, manned with just **10** (Simple's manning thickens it toward capacity; its fort doctrine — floor = capacity, never evacuates — then keeps it there); top/bottom are **neutral and empty** (claimable — a manned outer fort reaches into the middle fort's own ground), flanked above/below by two neutral 60/2 posts inside the outer forts' dormant zones; **right:** five asymmetric 60/2 heartland subs (ONE enemy-owned, fully stocked with 60 ships; Simple expands from there) + two enemy **back forts manned 50/90** gating the eastern approach to the reserve — the endgame toll. Reserve ring at the **0.6× level dial** (`add_storage_sub_scaled`) | `[SimpleColonize]` |
| 4 | **Deliberation** | Layer-1 | 1 struct, 13 subs — a horizontal neutral chain (storage 30/prod 1) from the Player start **A** (60 ships, 60/2); a **rich upper branch** (two 60/2 posts) up to **B** (Simple, 60 ships, 120/4) and a **lean lower branch** (two 30/1 posts) down to **C** (Simple, 60 ships, 90/3) — a three-way **free-for-all** | `[SimpleColonize, SimpleColonize]` |
| 5 | **Far far away** | Layer-2 | 2 bigger homes + 1 long lane — Player 4 subs vs Enemy 4 subs *(placeholder)* | `[SimpleColonize]` |
| 6 | **Three Fronts** | Layer-2 | triangle — two 3-sub homes + a 2-sub neutral crossroads, 3 lanes *(placeholder)* | `[SimpleColonize]` |
| 7 | **The Prize** | Layer-2 | 5 structs — two 3-sub homes, a fat 3-sub neutral prize (max_resistance 600 so its grind resolves in horizon), two 1-sub spurs, 6 lanes *(placeholder)* | `[SimpleColonize]` |
| 8 | **The Seam** | Layer-2 | 4 structs — Player 3-sub home, Enemy single-sub rear one short lane away, a 2-step neutral bait corridor *(placeholder; the greedy thin-rear seam lesson is parked)* | `[SimpleColonize]` |
| 9–11 | **Overreach / The Turtle / The Hammer** | Layer-2 | the **diamond** — two 3-sub homes, two 1-sub private flank neutrals, a 2-sub contested centre, 6 lanes *(placeholder; the attack≻colonize≻defend≻attack RPS lessons are parked)* | `[SimpleColonize]` |

> **Roster note.** L8–L11 keep their distinctive blurbs/hints/objectives and their topology, but
> every L2–L11 enemy seat currently fields `Roster::SimpleColonize` — the stateful, ledger-driven
> **`SimpleController`** (synchronized staggered taskforces over the projection-free
> `World::sub_influx_for`). The pure Colonize/Defend/Attack automata those flavour texts describe
> are parked until the automata track is revived.

---

## Headless validation (the real test)

`crates/levels/src/validation.rs` runs **two** checks per level; a level **gates on the
first** — `LevelReport::ok()` is `deterministic`. The second is *measured and reported* (for
information) but **not gated**, because the lessons + difficulty curve are being redesigned
against Simple. The lib test `campaign_is_well_formed` asserts `ok()` for all 11. The seed set
is `{1, 7, 42, 2024, 31337}`. *(A structural-spec gate — a hand-maintained `spec_for` mirror of
each `build` function — existed once and was removed as non-load-bearing: it only ever broke on
legitimate authoring changes. A `build` that panics still fails inside the determinism check.)*

1. **Determinism.** Building the same level with the same seed twice yields the same
   `World::state_hash`, and a short scripted match — player-greedy vs **every declared enemy
   seat**, each driven through the same `ai::SeatController` dispatch the game uses, at the
   game's reference decision cadence (`GAME_DECISION_BASE = 5`) — replays bit-for-bit (identical
   per-tick hashes and outcome).
2. **Winnability *(measured, not gated)*.** `not_auto_lost`: a *competent player proxy* plays the
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

### The reserve / storage node (every struct)

Every campaign struct carries an **ownerless reserve / patrol-zone node** (`add_storage_sub`,
called by the builders' struct helpers and by the L1–L3 inline builds): the universal inter-struct
entry/exit chokepoint — fleets arrive into it and depart reserve-first. It is permanently
**Neutral and never captured**, produces nothing, and is **excluded from territory everywhere**
(`sub_count`, `is_eliminated`, `total_subs`, production). Full design + wiring points: `CHANGELOG.md` (struct-storage section)
and the agent note `memory/struct-storage.md`. (L1 overrides its reserve capacity to 10 000 — a
single-struct staging buffer the over-cap corner production auto-flows into.)

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
