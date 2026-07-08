# LEVELS — the level / campaign system (`crates/levels`)

> **Refreshed 2026-07-06** (the tutorial-arc missions pass — see `CHANGELOG.md`, which stays
> authoritative where this doc lags). Campaign state: L1 = `Passive`, L2 = the scripted
> `Roster::Cycler`, L3–L13 = `Roster::SimpleColonize` (the stateful live `SimpleController`). **L1–L6 are
> hand-authored single-struct missions; L7 is the hand-authored orbiting contested field
> (multi-struct, opens in Layer 1)** — the whole campaign; the old L8–L13 placeholders are
> deleted. The Colonize/Defend/Attack automata, the seam exploit and the
> rock-paper-scissors lessons are **parked**; lesson/difficulty validation is parked with them (a
> level gates on **structure + determinism** only). Basic player-automation is **quarantined**
> (off on every level).

A headless, deterministic, fully-tested library that defines the game's **7-level campaign**:
for each level, the GUI-facing **metadata** plus a `build(seed) -> (World, WorldParams)`
**world-builder**, and a **headless validation harness** that asserts every level is well-formed
and deterministic. No graphics.

The crate sits on top of the substrate — [`world`](WORLD.md) (the Layer-2 lens), `ai` (the
opponent brains + the validation proxies; the archived design history is in `docs/archive/`), and
`layer1` (the spatial sim used to author each struct's sub-structures and garrisons). It is wired
into the workspace `members` **and** `default-members` in the root `Cargo.toml`, and carries
**zero external dependencies**, so every level build and match replay is bit-reproducible.

```
assets/levels/*.lvl   THE 7 MISSIONS — plain-text data files (owner, 2026-07-08: tweaking a
                      level costs no recompile — edit the file, restart the game). Format
                      reference: the module doc of crates/levels/src/spec.rs.
crates/levels/src/
  lib.rs          Level + LevelSource + StartView + campaign() (the GUI-facing API) + tests.
  spec.rs         The .lvl format: hand-rolled parser (zero deps) + the deterministic
                  world interpreter (LevelSpec::build).
  campaign.rs     The loader: reads assets/levels (next to the exe, else the workspace
                  tree), sorted by filename = play order; panics loudly on a malformed file.
  builders.rs     default_world_params (all authoring now lives in the data files).
  validation.rs   The headless validation harness (the determinism gate — the only gate;
                  balance is never tested: owner rule, all balancing is per-level by hand).
```

---

## The API the GUI consumes

```rust
use levels::{campaign, Level, StartView, Roster};
use layer1::{Faction, SimParams};

let levels: Vec<Level> = campaign();            // the 7 levels, in order
let lvl = &levels[0];

// Metadata drives the UI:
lvl.id;                    // 1..=7
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
// stateful SimpleController / CyclerController for their rosters, the stateless AiController otherwise),
// steps World::step(&sim, &wp) each tick, applies each seat on the decision cadence, and reports
// a WIN when every Ai(i) seat is eliminated.
```

* **`enum StartView { Layer1(StructId), Layer2 }`** — the lens the camera opens in. The
  single-struct missions (L1–L6) open zoomed **into** their only struct's Layer-1 view
  (`Layer1(0)`); the multi-struct levels open in the Layer-2 tactical view.
* **`struct Level`** — plain data + one `fn` pointer (`build`), so it is trivially
  `Clone`/inspectable and carries no hidden state.
* **`fn campaign() -> Vec<Level>`** — the 7 levels in play order. This is the single list the
  GUI reads.

The **player** is always `Faction::Player`; the **enemy seats** are `Faction::Ai(i)`, one per
`enemies[i]` entry — the list *is* the declaration of how many opponents a level has (L6 declares
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
2. **Fire in the sky** — combat + massing (moved to M2, 2026-07-08 — combat before fleet
   command). *Needs re-authoring: its middle-square flashpoint was tuned to the old
   engagement radius (7) and died at 3.5.*
2b. **Command and Control** — fleet command (**BUILT**, owner-designed 2026-07-06; now M3;
   placeholder briefing): two pairs of matched subs and the scripted **Cycler** enemy — a
   readable drillmaster the player out-commands. Its rotating surplus dodges idle attrition
   (the clock), an attacked sub pulls its whole force (feint counterplay), its all-in strike
   is telegraphed by a visible muster, and it is blind to the reserve (the hidden-staging /
   ambush lesson). See `ai::cycler`.
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
5. **The teleporter mission — "Head of the Snake"** (**BUILT** as L5, 2026-07-06 — the owner
   moved it *before* Deliberation; placeholder briefing) — the counter to fortresses: an
   impregnable enemy fortress line, frontally hopeless and geometrically unflankable; a
   neutral gate dissolves it (owned-gate departures hop instantly; the walk to the gate
   crosses no gauntlet). Behind the line, the enemy's **active shipyard** — the "head of the
   serpent" — feeds the wall; the deep strike through the gate decapitates the economy and
   the line starves. Mobility doesn't fight defense, it invalidates it.
6. **The Layer-2 revelation** — *(realized by the "Far far away" rework, L7, 2026-07-07 —
   see the table; whether a separate dedicated revelation mission is still wanted is the
   owner's call)* — two structs presented as one: `StartView::Layer1`,
   no lens mention anywhere, diegetic hints only ("where do they keep coming from?"). Simple's
   funnel keeps landing reinforcements in the enemy reserve; the off-screen arrows accumulate
   at the frame edge pointing at ships from *somewhere out there*; wheeling out past minimum
   zoom IS the discovery — the interface is the epiphany. The sandbox was always bigger than
   the box. (Ships on other structs deliberately show nothing.)
7. *(planned)* **Wrap-up** — multi-struct synthesis: wall + gate + yard + real logistics.
   *Open question:* where the first full sacrifice battle lands.

**Arc 2 — NOT DESIGNED** (owner decree, 2026-07-07: no Arc-2 design exists — any earlier
notes claiming reserved material or a shape for it are void; it gets its own design
discussion when Arc 1 is done).

**Arc 3 — Automation + new enemies:** parked pending its own design discussion (player
automation redesign, greedy rework, automata/Counter revival).

The old L8–L13 placeholders are **deleted** (owner, 2026-07-07). Only M1's briefing is
final copy. Mission count is decided as missions are made. All new layouts are authored at the
3.5 engagement radius and the corrected game scale (the reserve ring is a far outer orbit — a
fort can cover an *approach*, never the ring itself).

## The 7 levels (as built)

**All seven missions are hand-authored** (L1–L6 single struct, Layer-1 only; L7 is the
orbiting contested field). The old L8–L13 placeholders were **DELETED** (owner, 2026-07-07 —
no point keeping content explicitly marked as carrying no design). Difficulty is **ad-hoc**
(tuned by playtest), not a curve.

| # | Title | View | Topology (ownership) | Enemies |
|---|---|---|---|---|
| 1 | **First steps** | Layer-1 | 1 struct, 5 subs — a wide square (Player corner home, 100 ships, storage 60/prod 2; three neutral corners) around a **Passive centre fortress** (Enemy, 400 ships, storage 100/prod 3). The outer edges stay out of the centre's reach, so the player expands corner-to-corner before striking | `[Passive]` |
| 2 | **Fire in the sky** | Layer-1 | 1 struct, 6 subs — four neutral production posts in a central square (storage 60, **prod 3**) between two opposite homes (Player left / Enemy right, 60 ships, storage 60/prod 1). *The posts were authored ~11 apart to trade fire across the gaps at the old engagement radius (7); at the halved radius (3.5) that flashpoint no longer fires — re-author with the tutorial arc.* | `[SimpleColonize]` |
| 3 | **Command and Control** | Layer-1 | 1 struct, 5 subs — two Player 60/2 subs west (**60 ships each**) vs two enemy 60/2 subs east (**50 each**), a moderate gap between the pairs. The enemy is the scripted **Cycler** (owner-designed): rotates its surplus between its subs (the in-transit column dodges idle attrition — the mission clock), masses its pool on an attacked sub (feint one, strike the other), launches one telegraphed all-in once the pool overwhelms a target's defenders (`max(3F, F+60)`), fights **committed sieges** on foreign ground (hold while outnumbering — present + inbound, both sides — else retreat to the nearest owned sub; committed units never cycle or gather), and is **blind to reserve-staged ships** — the hidden muster and the ambush bait — until no foe sub remains. *Placeholder briefing.* | `[Cycler]` |
| 4 | **The Sinews of War** | Layer-1 | 1 struct, 15 subs — **left (player):** an active **shipyard** (starts with 1 ship; output pools at the yard up to the invisible 120 virtual cap) + two neutral **200-cap/1-prod warehouses** (default 12 000 resistance — a deliberate midgame investment); **middle:** a vertical wall of three mutually covering **fortresses** (20 apart, reach ~21.7 at `FORTRESS_RANGE` 18) — only the **middle** starts enemy, manned with just **10** (Simple's manning thickens it toward capacity; its fort doctrine — floor = capacity, never evacuates — then keeps it there); top/bottom are **neutral and empty** (claimable — a manned outer fort reaches into the middle fort's own ground), flanked above/below by two neutral 60/2 posts inside the outer forts' dormant zones; **right:** five asymmetric 60/2 heartland subs (ONE enemy-owned, fully stocked with 60 ships; Simple expands from there) + two enemy **back forts manned 50/90** gating the eastern approach to the reserve — the endgame toll. Reserve ring at the **0.6× level dial** (`add_storage_sub_scaled`) | `[SimpleColonize]` |
| 5 | **Head of the Snake** | Layer-1 | 1 struct, 12 subs — **west (player):** home (60/2, 60 ships) + two neutral 60/2 posts; **south-west:** a neutral **teleporter gate** (default 60-cap resistance — a midgame investment); **middle:** an impregnable wall of **four** mutually covering enemy **fortresses** (spacing 20, zones overlapping — no seam, no flank in the sub graph), manned **60 each** (Simple tops them toward 90); **east:** the enemy's **active shipyard** (40 pooled at the yard) + one owned 60/2 heartland sub (40 ships) + two neutral 60/2 subs. The gate-strike lands at the yard with no transit, decapitates the 8-prod economy (an active yard keeps a token bar), and the starving wall is dismantled last. Reserve at the **0.6× dial**. *Placeholder briefing.* | `[SimpleColonize]` |
| 6 | **Deliberation** | Layer-1 | 1 struct, 13 subs — a horizontal neutral chain (storage 30/prod 1) from the Player start **A** (60 ships, 60/2); a **rich upper branch** (two 60/2 posts) up to **B** (Simple, 60 ships, 120/4) and a **lean lower branch** (two 30/1 posts) down to **C** (Simple, 60 ships, 90/3) — a three-way **free-for-all** | `[SimpleColonize, SimpleColonize]` |
| 7 | **Far far away** | **Layer-1(!)** | 2 **unnamed** structs + 1 long lane, camera opens INSIDE the contested struct (the lens is the discovery — off-screen arrows, zoom out). **Contested struct:** SIX 90/3 subs 60° apart (R 90.72) **orbiting clockwise** (τ/1500/ref-tick; ships LEAD moving targets — the dispatch intercept) — W Player 90 ships, E Simple 90, four neutral; the **enemy-owned shipyard** at the hub, starting **empty** (active token bar — its output pools under the watchers' guns); three **fortresses** (R 14, 120° apart, **counter-orbiting slower**, τ/3000) owned by a Passive third seat hostile to all, manned **30 each** — the yard never leaves their kill zone. The enemy is the **ADJACENT Simple** (`SimpleAdjacent`, range 120): where it owns ground it only attacks neighbouring positions — it crawls around the ring, never across the middle. **Enemy struct:** a single **active shipyard** (40 pooled) — the source. *Placeholder briefing (diegetic — no lens mention).* | `[SimpleAdjacent 120, Passive]` |

> **Roster note.** L8–L11 keep their distinctive blurbs/hints/objectives and their topology, but
> every L2–L11 enemy seat currently fields `Roster::SimpleColonize` — the stateful, ledger-driven
> **`SimpleController`** (synchronized staggered taskforces over the projection-free
> `World::sub_influx_for`). The pure Colonize/Defend/Attack automata those flavour texts describe
> are parked until the automata track is revived.

---

## Headless validation (the real test)

`crates/levels/src/validation.rs` runs **one** check per level, and it is the only gate —
`LevelReport::ok()` is `deterministic`. The lib test `campaign_is_well_formed` asserts `ok()`
for all 7.

**Determinism.** Building the same level with the same seed twice yields the same
`World::state_hash`, and a short scripted match — player-greedy vs **every declared enemy
seat**, each driven through the same `ai::SeatController` dispatch the game uses, at the
game's reference decision cadence (`GAME_DECISION_BASE = 5`) — replays bit-for-bit (identical
per-tick hashes and outcome). A `build` that panics fails here too.

**Balance is deliberately NOT tested** (owner rule, 2026-07-06: all balancing is per-level, by
hand). The winnability proxies (the Layer-1 concentration proxy, the Layer-2 greedy baseline)
and the dormant automata-track lesson checks (`counter_beats_enemy`, `seam_flank_beats_greedy`)
were removed with the rest of the lesson machinery — the same fate as the structural-spec gate
before them (a hand-maintained `spec_for` mirror of each `build`, removed as non-load-bearing:
it only ever broke on legitimate authoring changes).

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
cargo test  -p levels                       # the validation sweep (~20 s: every level's
                                            # determinism gate, all seats live-Simple driven)
```

Current state (2026-07-06): `cargo test -p levels` → **2 passed** (+1 doctest) under the
all-seats / live-Simple drivers; `cargo check --workspace` zero warnings. The old Windows
Smart-App-Control note (`os error 4551`, blocked freshly-linked binaries) is historical — the
policy has been disabled and binaries run normally.
