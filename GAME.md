# GAME — the complete single-binary v1 cell-game RTS

> **STATUS (latest session) — this doc lags; `CHANGELOG.md` is authoritative.** Since it was last
> refreshed the GUI gained a **0.5×–7× zoom**, **box-select** (left-drag → multi-select → order all),
> a discrete **speed slider** (replacing the Pause/x1/x3 buttons), **per-seat** rendering (the pie chart
> + count labels iterate every `Ai(i)` seat), **batched ship rendering** + an **`F3` perf overlay**;
> **player-automation is quarantined** (off on every level); seats are now `Faction::{Neutral, Player,
> Ai(u8)}`.

> **Refreshed for the `feat/counter` GUI overhaul** (top-bar troop slider + Pause/x1/x3 speed
> buttons + count-up clock; per-sub production squares / orbit render; reserve-node interior;
> faction-scoped orders). `CHANGELOG.md` (top `feat/counter` entry) is authoritative where this
> doc and the code disagree.

`crates/game` is the **playable v1 product**: one macroquad binary that assembles the headless
substrate into a real game with a menu, a sequential-unlock campaign, and the zoomable two-layer
match itself. It is **Phase 4**, sitting on top of everything the earlier phases built and tested:

- [`levels`](crates/levels) — the 10-level campaign (`campaign() -> Vec<Level>`), each `Level`
  carrying title / blurb / objective / hints, the enemy [`ai::Roster`], where the camera opens
  (`StartView`), whether automation is offered, the match horizon, and a `build(seed) -> (World,
  WorldParams)` world-builder.
- [`world`](crates/world) — the Layer-2 lens over Layer-1 structs: `World`, `Structure`, `Lane`,
  `InterFleet`, `FleetOrder`, `World::step`, `World::issue_fleet_order`, `StructAggregate`,
  `World::outcome`, and the projection-free **`World::sub_influx_for`** the live AI reads
  (`world::projection` is PARKED with the automata track).
- [`ai`](crates/ai) — the enemy brains: the stateful campaign **`SimpleController`** (the live
  `Roster::SimpleColonize`), the stateless `AiController` + `Roster`, the shared
  **`SeatController`** roster-to-brain dispatch, and the layer-agnostic greedy adapter (the AI's
  tactical default; also what player automation used before it was quarantined). The
  projection-driven `ai::automata` / `ai::counter` are **parked**.
- [`layer1`](crates/layer1) — the per-struct spatial sim (`Structure`, sub-structures carrying
  **resistance** — the capture grind and production denial — plus each sub's own per-sub economy
  (`storage_capacity` / `production`), orbiting ships, and `MoveOrder`).

It honours the design's signature principle (`00-overview.md`, *decouple computation from
spectacle*): **all** model logic lives in those headless crates; this binary only (a) **draws**
the two layers and (b) turns human input into the **same** `FleetOrder` / `MoveOrder` the AI uses.
It owns only spectacle data — the camera, the GUI tick pace, and the menu/zoom state.

The player is always `Faction::Player` (cyan); each enemy seat is `Faction::Ai(i)`, driven by the
level's `enemies[i]` roster entry and coloured by its AI **kind** (Simple = amber, Passive =
steel-grey; a second same-kind seat gets a distinct shade).

---

## How to run

From the workspace root (Windows / PowerShell; `cargo` on PATH, fallback
`& "$env:USERPROFILE/.cargo/bin/cargo.exe"`):

```powershell
cargo run -p game --release
```

> **Windows run note (historical).** The game builds and runs normally. An earlier Smart App
> Control policy on this machine could refuse freshly-linked binaries (`os error 4551`); it has
> since been disabled, so this is no longer a concern. (`--release` remains the recommended way to
> run for smooth pacing.)

The game opens on the **main menu**. `Play` continues at your highest unlocked level; `Level
Select` lists the 10 levels (only unlocked ones are playable); `Quit` exits.

Handy flags for interactive play:

| Flag | Effect |
|---|---|
| `--level <N>` | Jump straight into level `N` (1-based) on launch. |
| `--seed <S>` | Seed the world build (decimal or `0x…` hex). |
| `--auto` | Drive **both** seats by AI — a hands-off demo of the level. |
| `--unlock-all` | Unlock every level in Level Select (debug). Env: `MI_UNLOCK_ALL=1`. |
| `--selftest` | Run the headless game-loop self-test over all 10 levels, print results, and exit (no display). |

Example: watch the AI play level 8 hands-off — `cargo run -p game --release -- --level 8 --auto`.

---

## The app state machine

```
MainMenu ──► LevelSelect ──► InLevel ──► Victory ──► (next level | menu)
   ▲             │              │     └─► Defeat  ──► (retry | level select)
   └─────────────┴──────────────┴── Pause overlay (Resume | Restart | Back to Menu)
```

- **Main menu** — title + `Play` / `Level Select` / `Quit`.
- **Level select** — the 10 campaign levels by title with the enemy roster tag. **Sequential
  unlock**: beating level `N` unlocks `N+1`. Progress persists to `mi_progress.json` *next to the
  executable* (a tiny `{"unlocked": N}`); if absent, only level 1 is unlocked. `--unlock-all`
  overrides for testing.
- **In-level** — the zoomable two-layer match (below). On `World::outcome()`:
  - **Player wins** → Victory screen, the next level unlocks, `Enter` advances (or returns to the
    menu after level 10).
  - **Enemy wins** → Defeat screen, `R` retries.
  - **`Esc`** opens the **Pause** overlay (Resume / Restart / Back to Menu) when nothing is
    selected; from the end screen `Esc` returns to Level Select.
- A start overlay shows the level **title / blurb / objective / hints**; dismiss with
  `Enter` / `Space` / click. The objective stays in the HUD; hints appear on the overlay.

---

## The zoomable two-layer game (the core)

There is **one** `World` (the level builds it). The simulation always runs the same — computation
is decoupled from spectacle — while the **camera** has two zoom states it lerps smoothly between:

### Layer-2 lens (zoomed out)
The struct graph. Each struct node is a **pie chart of sub ownership** (`draw_pie`): one wedge
per present seat — Player cyan, **each AI rival in its own colour**, then Neutral grey — sized by
that seat's producing subs. A node's **size is its summed sub storage capacity** (what the struct
can hold, fixed across the match — *not* its momentary ship count; the reserve/storage node is
excluded). Nodes show **per-seat present ship counts** (every present seat, stacked, each in its
colour; incoming inter-struct fleets are not counted until they land) and the struct **name**.
(The `AUTO` ring/tag renders only on automation levels — currently quarantined off.) The struct's **reserve-node garrison is drawn as dots** on a ring inside the node,
each at its real Layer-1 orbit angle — the staged ships rallied and ready to launch (visible without
zooming in). **Lanes** are drawn between connected structs; **inter-struct fleets** stream along
their lane (the renderer interpolates each `InterFleet`'s `progress` between ticks). Selecting a
struct highlights its lane-connected neighbours (valid fleet targets).

**Issue a fleet:** click a struct you own (selects the source), then click a **lane-connected**
struct — or **drag** from source to target. This calls
`World::issue_fleet_order_fraction(src, tgt, send_fraction, Faction::Player, &wp)` — the order is
**faction-scoped** (it only moves the *player's* idle ships at the source; the enemy can no longer
drag the player's ships off a contested struct). The source stays selected for rapid repeat orders.
A fleet departs **only from the reserve node**: the fraction is pulled from the reserve's rallied
ships, and if the reserve is empty the order instead **stages** the struct's inner subs toward it
(ships must reach the reserve before they can cross a lane) — so the first order on a fresh struct
rallies, and a follow-up order launches. The dots on the node show how many are staged and ready.

### Layer-1 interior (zoomed into one struct)
That struct's `Structure`, drawn **WYSIWYG** — what you see *is* the combat geometry (`draw_interior`):

- **Sub-structures**: disk + owner ring + **per-seat present counts** (home-based idle counts for
  every present seat, each in its colour, excluding incoming; the owner's count reads as
  `count / capacity`) + a **contested-ownership ring** split by each present side's share when two
  or more sides hold ships there. (The old production *progress* ring is gone — production is no
  longer surfaced as a fill.)
- **Production squares**: each producing sub shows N empty white squares at ½ its radius, where N =
  the sub's `production`. A freshly minted ship appears at the next square (round-robin, matching the
  sim's `spawn_at_square` angles) then **glides out** to the orbit ring. The slot ring slowly orbits
  **counter-clockwise** — the spawn angle is a function of the sim **tick** (`PROD_SQUARE_SPIN_PER_TICK`,
  so it stays deterministic), and the squares are drawn at that same tick-based angle, so a square is
  always exactly where a ship is created (not a cosmetic overlay).
- **Ships** drawn at their **real, interpolated sim positions** — idle ships ride their sub's actual
  orbit ring (a circle dot); ships in transit fly (a triangle). There is **no separate visual ring**.
- **Reserve / storage node**: rendered as an **outline-only big enclosing circle** (a whisper of
  fill so the inner subs read through it, no production squares, no progress ring). It is the
  universal inter-struct entry/exit point — produces nothing, but is attackable, **selectable**, and
  shows its garrison like any sub. It is sized so its garrison ring clears the inner sub garrisons by
  more than an engagement radius, so a reserve garrison and a sub garrison of opposing sides do **not**
  auto-fight across the boundary until ships are deliberately moved.
- **Ship-death flashes** (white cross, plus a thin line from a **random** in-range enemy as the
  firing source) — battle "bubbles" are gone; combat reads through these flashes.

A faint metre grid gives a frame of reference. This is also where the **siege** is read in real time
(see the next subsection).

**Issue a move:** click one of your sub-structures (source) — selection **prefers the inner sub
over the enclosing reserve node** — then click another sub, or drag. This calls
`structure.issue_order_fraction(src, tgt, fraction, Faction::Player)` (faction-scoped, like fleet
orders). All subs on a struct are mutually reachable, so any sub is a valid target. The
**mouse-wheel only zooms** (up = into the hovered/focused struct, down = out to the lens); each
sub's orbit ring (`SubStructure::ring_frac`) is fixed at its default and is not player-adjustable.

### The siege UI (reading capture, denial, and the per-sub economy)
Capture is a **resistance grind**, not an instant flip, and the interior view surfaces every part
of it (`draw_interior` + `draw_resistance_bar` in `main.rs`):

- **Resistance bar (per sub).** A thin bar below each sub shows `resistance / max_resistance` (the
  capture meter; the default max is capacity-derived — `3600` for a default sub at the reference
  scale — full = held firmly). It is drawn on **capturable neutrals**,
  any **damaged** sub, and any sub **being ground**. As an attacker erodes it, the depleted slice
  fills in the **attacker's colour** and the bar's outline **pulses in that colour** — you watch
  the bar drain toward 0, at which point the sub **flips** to the attacker and refills.
- **Being-captured pulse ring.** A sub currently being eroded by a single foreign faction (owner
  absent) wears a **pulsing ring in the attacker's colour** around its disk — an at-a-glance "this
  is falling" cue distinct from a mere firefight.
- **Healing cue.** When the owner sits alone on a damaged sub it **heals** (resistance climbs back
  to max); the bar's outline turns **green** to show the repair. A returning defender undoes an
  attacker's progress, so hit-and-run is worthless.
- **Denial = production stops.** A sub being eroded undefended **stops producing** — no new ships
  appear at its production squares while it is being ground (parking on an enemy sub **starves its
  output** before you ever capture it). A *contested-but-defended* sub keeps producing. (The old
  production-progress-ring cue was removed with the ring itself; the squares only show *where*
  ships appear.)

> **Per-sub economy, no garrison readout.** The old per-structure soft cap and the top-left
> **`garrison X/Y`** line are **gone**. The cap is now **per sub**: each sub carries its own
> `storage_capacity` (no-attrition headroom, default 60) and `production` (default 1), and a sub
> above its capacity bleeds its surplus via a gentle **linear** `per_sub_attrition` (effective
> plateau ≈ `storage_capacity + 60 × production` ≈ 120 for a default sub). A sub's radius is derived
> from its storage capacity. Spend it or keep it moving — inter-struct fleets in transit are still
> exempt.

### Zoom control
- **Click** a struct in the lens to zoom **into** it; **mouse-wheel up** zooms into the
  hovered/selected struct; **wheel down** / **`Esc`** zooms back **out** to the lens.
- The camera **lerps** (centre + log-scale) between the lens framing (all structs fit) and the
  focused struct's interior framing; a short crossfade swaps the lens scene for the interior scene
  around the midpoint of the zoom so the transition reads as diving into the struct.
- A level whose `StartView` is `Layer1(p)` (the L1/L2 micro tutorials) **opens already zoomed
  into** struct `p`; `StartView::Layer2` levels open in the lens. Single-struct levels simply show
  the one struct as a node when zoomed out (fine).

### Send fraction (the top-bar troop slider)
A continuous **1–100% troop slider** in the top bar sets the fraction of the source sent on every
order (`frac_pct`, default **100%**), shared by both layers (it applies to fleet launches and to
intra-struct moves alike). Drag it directly, or snap it with keys **1 / 2 / 3 / 4** =
**25 / 50 / 75 / 100 %**. Sending **100%** takes everything (keep-floor 0); other fractions keep the
old floor. **Right-click** or **`Esc`** clears a pending selection.

### Basic automation (the "delegate a struct" lesson)
On levels where `automation_available` is true, press **`A`** while a struct you own is focused
(zoomed in) or selected (in the lens) to toggle **AUTO** on it. An automated struct's internal
ships are then driven **every decision tick** by `ai::greedy_layer1_orders(...)` — the *same*
Layer-1 greedy policy the enemy runs on its own structs — so it auto-expands and auto-defends its
sub-structures while you fly fleets elsewhere. Automated structs show a pulsing green ring + `AUTO`
tag in the lens and an `AUTO ENABLED` badge in the interior.

### The enemy
Each `Faction::Ai(i)` seat is driven by an `ai::SeatController::from_roster(Ai(i), level.enemies[i])`
— one per declared seat, stepped in order each decision tick. The campaign fields **L1 = Passive,
L2–L10 = Simple** (`Roster::SimpleColonize` → the stateful **`SimpleController`**, a ledger-driven
colonizer that plans synchronized, staggered taskforces and reads the projection-free
`World::sub_influx_for`). **No projection is built on the live path**; the projection-driven
automatons (Colonize / Defend / Attack, the Counter, the RPS escalation) are **parked** pending the
mission redesign.

### Pacing
The sim is grounded at **`TICK_HZ = 60`** logical ticks/second (reference rate 2.5; every per-tick
quantity is scaled by `TICK_SCALE = 24`, so per-*second* behaviour is independent of the tickrate).
`Game::update(dt)` is a **fixed-timestep accumulator**: it drains whole `1/60 s` ticks and exposes
`render_alpha` so the renderer interpolates positions between the last two sim states — render at
whatever the monitor allows, simulate at a fixed 60 Hz. Seat decisions run every
`DECISION_BASE(5) × 24` ticks. The transport is one discrete **speed slider** with five stops —
**0× (= paused) / 1× / 3× / 10× / 25×**; **`P`** toggles 0× ⇄ the last running speed, **`-` / `+`**
(or `[` / `]`) step the stops. Death flashes live for `KILL_FX_TTL = 0.35 s`.

**The match starts paused** and unpauses when the briefing overlay is closed; a top-right
**count-up clock** then ticks. A **human match has no horizon** — it ends *only* on a sealed result
(`Game::seat_finished`: a seat with no ships anywhere and every still-owned producing sub being
eroded by the other seat, so it can never recover; the reserve/storage node is excluded since
holding only it keeps nobody alive). The level `horizon` is honoured **only for AI-driven matches**
(`--auto` / `--selftest` / capture, where a Player-seat AI runs) via `Game::match_over`; that path
falls back to `World::outcome()` at the horizon.

### The GUI sim operating point (`gui_params`)
The game does **not** run `SimParams::default()`. `gui_params()` softens combat for a deliberate,
readable feel and turns on the new mechanics, **diverging** from the headless/AI reference so the
parked AI suite is protected from the spectacle tuning (the one exception is **orbit**, which is
universal — it applies to the headless suite too):

- `fire_prob = 0.0055`, `defender_fire_bonus = 0.003` — combat resolves far slower than production,
  so economy and territory outweigh a single clash.
- `transit_fire_gating = true` — a wave in transit cannot "drive-by" shoot a garrison; an assault
  must *land* before it trades, while the garrison fires on the incoming wave.
- `spread_damage = true` — grid-accelerated, no bubbles: each ship spreads its fire across all
  in-range enemies (continuous-feeling attrition; Lanchester's square law preserved).
- `per_sub_attrition = true` — the gentle linear per-sub surplus bleed described above.

### HUD
The top bar is now minimal: **Pause / x1 / x3** speed buttons (left, with a live `Nx` / `paused`
multiplier readout next to them), the **`Send` troop slider** with its `N%` readout (middle), and a
**count-up clock** (top right). **Removed** in the overhaul: the Goal/objective line, the
Player-vs-Enemy ship/struct/sub totals, the `garrison X/Y` soft-cap readout, the "automation" status
text, and the context-sensitive bottom controls-help line. An `AUTO ENABLED` badge still appears in
the interior of an automated struct, and the start/hint **overlay** frames the level at the
beginning (title / blurb / objective / tips). The end **banner** is just the title (`VICTORY` /
`DEFEAT` / `DRAW`).

---

## Full control list

**Menus** (main / level-select / pause)
- `Up` / `Down` (or `W` / `S`) or mouse hover — move the highlight
- `Enter` / `Space` or click — select / activate
- `Esc` — back (level-select → main; pause → resume)

**In-level — shared**
- top-bar **`Send` slider** (drag) — send fraction 1–100 % (default 100 %)
- `1` / `2` / `3` / `4` — snap the slider to 25 / 50 / 75 / 100 %
- mouse-wheel up / `Enter`-on-a-struct / click a struct — zoom **in**
- mouse-wheel down / `Esc` — zoom **out** to the lens
- **left-drag a box** — multi-select every player-commandable sub (interior) / struct (lens)
  inside it; the next click orders them all at the clicked target
- `A` — toggle **automation** on the focused/selected owned struct *(quarantined: no current
  level allows it)*
- `F3` — toggle the frame-timing perf overlay
- right-click / `Esc` — clear the current selection (`Esc` with nothing selected, in the lens,
  opens the Pause overlay)
- top-bar **speed slider** (0× / 1× / 3× / 10× / 25×), or `P` — pause ⇄ resume at the last speed
- `-` / `+` (or `[` / `]`) — step the speed stops

**In-level — Layer-2 lens**
- left-click your struct → click a lane-connected struct — send a **fleet** (or drag source→target)
- click an already-selected source again — zoom into it

**In-level — Layer-1 interior**
- left-click your sub → click another sub — **move** ships (or drag source→target)

**End screens**
- Victory: `Enter` — next level (or menu after L10); `Esc` — level select
- Defeat: `R` — retry; `Esc` — level select

---

## The level flow (the campaign)

The 10 levels (`levels::campaign()`) are an authored curriculum — see `LEVELS.md` for the full
table and the measured validation. In short:

| # | Title | Opens | Enemy | Notes |
|---|---|---|---|---|
| 1 | First steps | interior | Passive | select a sub, send a fraction, capture |
| 2 | Fire in the sky | interior | Simple | concentration of force; the middle posts decide it |
| 3 | Deliberation | interior | Simple × 2 | a three-way free-for-all on one struct |
| 4 | Far far away | lens | Simple | placeholder multi-struct world (redesign pending) |
| 5 | Three Fronts | lens | Simple | placeholder (multi-front concentration triangle) |
| 6 | The Prize | lens | Simple | placeholder (expansion-vs-defense around a fat neutral) |
| 7 | The Seam | lens | Simple | placeholder (was: exploit greedy's undefended rear) |
| 8 | Overreach | lens | Simple | placeholder (was: attack ≻ colonize) |
| 9 | The Turtle | lens | Simple | placeholder (was: colonize ≻ defend) |
| 10 | The Hammer | lens | Simple | placeholder (was: defend ≻ attack) |

Beating a level unlocks the next and persists to `mi_progress.json`.

---

## Verification modes (headless self-test + screenshot capture)

### `--selftest` — the headless game-loop self-test (no display)

```powershell
cargo run -p game --release -- --selftest
```

Drives the **same** `Game` + `step_one_tick` loop the interactive app runs (decision cadence,
enemy + player-automation application, outcome latching) entirely headless — it touches no
macroquad rendering. It checks two things and prints a per-level line, then exits with an overall
verdict:

1. **Every campaign level** (all 10): an `--auto` match (all seats AI) runs to
   `min(horizon, HEADLESS_TICK_CAP = 700)` ticks at the coarse reference scale, makes progress,
   and is **bit-reproducible** on a rerun (same seed → identical final `state_hash`). Each line
   reads e.g. `L 1 First steps tick=700/700 outcome=(capped) det=true -> PASS` (a level that
   seals early reports `outcome=sealed:…`).
2. **Player basic automation issues effective orders** on a **synthetic** single-struct world
   (automation is quarantined off on every campaign level): an idle player gains nothing, the
   same player with AUTO on expands via the greedy adapter — `auto-peak > idle-peak`.

The footer prints `== self-test: ALL PASS ==` when every check passes (the exit status reflects it).
This is the headless verification that all 10 levels run end-to-end.

### Screenshot capture (`--shot`) — render one frame and exit

The binary can also render **one frame and exit**, so a built UI is checkable via PNGs:

| Flag | Meaning |
|---|---|
| `--shot <path>` | Render one frame to `<path>` (PNG) and exit. |
| `--screen <menu\|select>` | Capture a menu instead of a level. |
| `--level <N>` | Load level `N` (1-based) for the shot. |
| `--view <lens\|interior>` | Open the shot in the lens or zoomed into the start struct. |
| `--at-tick <T>` | Advance the sim to tick `T` before capturing (deterministic, frame-rate-independent). |
| `--auto` | Drive **both** seats by AI while advancing (for gameplay frames). |
| `--seed <S>` | Seed the world build (decimal or `0x…` hex). |

The capture races the sim to the target tick, renders a couple of **settle** frames so the
framebuffer is fully drawn, then `get_screen_data().export_png(path)` (the idiom inherited from
the deleted `layer1-game` / `layer2-game` prototypes). Note the framebuffer is grabbed **after** the final draw but
**before** the next buffer swap — grabbing after `next_frame().await` yields a black PNG.

Examples used to verify this build:

```powershell
cargo run -p game --release -- --shot target/shots/menu.png        --screen menu
cargo run -p game --release -- --shot target/shots/select.png      --screen select
cargo run -p game --release -- --shot target/shots/l1_interior.png --level 1 --view interior --at-tick 40 --auto
cargo run -p game --release -- --shot target/shots/l3_lens.png     --level 3 --view lens     --at-tick 120 --auto
cargo run -p game --release -- --shot target/shots/l3_interior.png --level 3 --view interior --at-tick 120 --auto
```

---

## Known caveats

- **Windows app-control block (resolved/historical).** An earlier Smart App Control policy on this
  machine could refuse freshly-linked binaries with `os error 4551` (which is why some `cargo test`
  *binary* targets occasionally failed to launch — the library test suites always passed). The
  policy has since been disabled; the game builds, runs, and self-tests normally and this is no
  longer a concern.
- **High-DPI coordinates.** `screen_width()/screen_height()` return *logical* pixels; under a
  high-DPI display they are smaller than the physical framebuffer. Menu/level-select layout is
  therefore computed *proportionally* to `screen_height()` (not hardcoded pixels) so nothing
  collides at any DPI or window size.
- **Lens/interior crossfade, not a morph.** The zoom lerps one continuous camera, but the lens
  scene (nodes) and the interior scene (sub-structures) are different representations; the
  transition **crossfades** between them around the zoom midpoint rather than geometrically
  morphing one into the other. This is intentional and reads cleanly as "diving in".
- **No DSL editor yet.** The design's end-game (`03-ui-layers.md`) is a player-authored
  `condition → action` DSL (operator → programmer → meta-programmer). v1 ships the **operator** and
  the first **delegation** step (toggle a whole struct to the greedy policy); the visual rule
  editor is future work.
- **Demo mode disables pointer orders.** With `--auto`, both seats are AI and human fleet/move
  clicks are ignored (zoom/pause/speed still work) — it is a spectator demo.
