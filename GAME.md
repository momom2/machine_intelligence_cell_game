# GAME — the complete single-binary v1 cell-game RTS

`crates/game` is the **playable v1 product**: one macroquad binary that assembles the headless
substrate into a real game with a menu, a sequential-unlock campaign, and the zoomable two-layer
match itself. It is **Phase 4**, sitting on top of everything the earlier phases built and tested:

- [`levels`](crates/levels) — the 10-level campaign (`campaign() -> Vec<Level>`), each `Level`
  carrying title / blurb / objective / hints, the enemy [`ai::Roster`], where the camera opens
  (`StartView`), whether automation is offered, the match horizon, and a `build(seed) -> (World,
  WorldParams)` world-builder.
- [`world`](crates/world) — the Layer-2 lens over Layer-1 planets: `World`, `Planet`, `Lane`,
  `InterFleet`, `FleetOrder`, `World::step`, `World::issue_fleet_order`, `PlanetAggregate`,
  `World::outcome`, and `World::project_forward` (the forward-projection the AI plans over).
- [`ai`](crates/ai) — the enemy brain (`AiController` + `Roster`, the projection-driven
  `ai::automata` over `ai::vocab`) and the layer-agnostic greedy adapter that **is** the player's
  "basic automation" for a planet.
- [`layer1`](crates/layer1) — the per-planet spatial sim (`Structure`, sub-structures carrying
  **resistance** — the capture grind, production denial, and the anti-hoard soft cap — ships,
  battle bubbles, `MoveOrder`).

It honours the design's signature principle (`00-overview.md`, *decouple computation from
spectacle*): **all** model logic lives in those headless crates; this binary only (a) **draws**
the two layers and (b) turns human input into the **same** `FleetOrder` / `MoveOrder` the AI uses.
It owns only spectacle data — the camera, the GUI tick pace, and the menu/zoom state.

The player is always `Faction::Player` (cyan); the enemy seat is `Faction::Enemy` (red), driven by
each level's `Roster`.

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
The planet graph. Each planet is a **node** coloured by its `PlanetAggregate` owner — Player
**cyan**, Enemy **red**, Neutral **grey**, **Contested** warm/amber with a split cyan+red cue and a
second ring. Nodes show the **total ships** present (garrisoned + arriving), the planet **name**, a
production pip on owned planets, and an `AUTO` ring/tag on automated ones. **Lanes** are drawn
between connected planets; **inter-planet fleets** stream along their lane (the renderer
interpolates each `InterFleet`'s `progress` between ticks). Selecting a planet highlights its
lane-connected neighbours (valid fleet targets).

**Issue a fleet:** click a planet you own (selects the source), then click a **lane-connected**
planet — or **drag** from source to target. This calls
`World::issue_fleet_order(FleetOrder { from, to, current_fraction }, Faction::Player, &wp)`. The
source stays selected for rapid repeat orders.

### Layer-1 interior (zoomed into one planet)
That planet's `Structure`: **sub-structures** (disk + owner ring + production progress arc + idle
ship count), **ships** (idle dots / moving triangles, interpolated between ticks), and **battle
bubbles** (pulsing halos with the `P vs E` engaged counts). A faint metre grid gives a frame of
reference. This is also where the **siege** is read in real time (see the next subsection).

**Issue a move:** click one of your sub-structures (source), then click another sub — or drag.
This calls `planet.structure.issue_order(MoveOrder { source, target, fraction })`. All subs on a
planet are mutually reachable, so any sub is a valid target.

### The siege UI (reading capture, denial, and the soft cap)
Capture is a **resistance grind**, not an instant flip, and the interior view surfaces every part
of it (`draw_interior` + `draw_resistance_bar` in `main.rs`):

- **Resistance bar (per sub).** A thin bar below each sub shows `resistance / max_resistance` (the
  capture meter, default `max = 1800`, full = held firmly). It is drawn on **capturable neutrals**,
  any **damaged** sub, and any sub **being ground**. As an attacker erodes it, the depleted slice
  fills in the **attacker's colour** and the bar's outline **pulses in that colour** — you watch
  the bar drain toward 0, at which point the sub **flips** to the attacker and refills.
- **Being-captured pulse ring.** A sub currently being eroded by a single foreign faction (owner
  absent) wears a **pulsing ring in the attacker's colour** around its disk — an at-a-glance "this
  is falling" cue distinct from a mere firefight.
- **Healing cue.** When the owner sits alone on a damaged sub it **heals** (resistance climbs back
  to max); the bar's outline turns **green** to show the repair. A returning defender undoes an
  attacker's progress, so hit-and-run is worthless.
- **Denial = the production ring disappears.** The production progress ring is drawn **only while a
  sub is owned AND not being eroded**. A sub being eroded undefended **stops producing**, so its
  ring vanishes — visual confirmation that parking on an enemy sub **starves its output** before
  you ever capture it. (A *contested-but-defended* sub keeps both its garrison fighting and its
  ring turning.)
- **Soft-cap garrison readout.** Top-left, a **`garrison X/Y`** line shows your parked ships `X`
  against this planet's soft cap `Y` (≈ `20 + 10 × your owned subs`). It is muted normally, turns
  **amber `near cap`** above 80%, and **red `OVER CAP — ships bleeding`** once you exceed it and the
  anti-hoard attrition starts destroying the surplus. The lesson: spend it or keep it moving
  (inter-planet fleets in transit are exempt from the cap).

### Zoom control
- **Click** a planet in the lens to zoom **into** it; **mouse-wheel up** zooms into the
  hovered/selected planet; **wheel down** / **`Esc`** zooms back **out** to the lens.
- The camera **lerps** (centre + log-scale) between the lens framing (all planets fit) and the
  focused planet's interior framing; a short crossfade swaps the lens scene for the interior scene
  around the midpoint of the zoom so the transition reads as diving into the planet.
- A level whose `StartView` is `Layer1(p)` (the L1/L2 micro tutorials) **opens already zoomed
  into** planet `p`; `StartView::Layer2` levels open in the lens. Single-planet levels simply show
  the one planet as a node when zoomed out (fine).

### Fraction buckets
Keys **1 / 2 / 3 / 4** = **25 / 50 / 75 / 100 %** (default 50%), shared by both layers (the bucket
applies to fleet launches and to intra-planet moves alike). **Right-click** or **`Esc`** clears a
pending selection.

### Basic automation (the "delegate a planet" lesson)
On levels where `automation_available` is true, press **`A`** while a planet you own is focused
(zoomed in) or selected (in the lens) to toggle **AUTO** on it. An automated planet's internal
ships are then driven **every decision tick** by `ai::greedy_layer1_orders(...)` — the *same*
Layer-1 greedy policy the enemy runs on its own planets — so it auto-expands and auto-defends its
sub-structures while you fly fleets elsewhere. Automated planets show a pulsing green ring + `AUTO`
tag in the lens and an `AUTO ENABLED` badge in the interior; the HUD shows how many planets are
automated.

### The enemy
`Faction::Enemy` is driven by an `AiController::from_roster(Faction::Enemy, level.enemy)`. Each
decision tick it runs both layers — strategic `FleetOrder`s and per-planet greedy internals — via
`decide_and_apply`. The strategic policies are now the projection-driven **automatons**
(`ai::automata` over `ai::vocab`): each builds **one** shared `World::project_forward` look-ahead
per decision tick and plans against it (re-projecting every tick rather than trusting a stale ETA).
The roster escalates across the campaign (Passive → GreedyLocal → the pure Colonize / Defend /
Attack automatons), exactly as the levels were validated; on the diamond those three close the
rock-paper-scissors cycle (attack ≻ colonize ≻ defend ≻ attack).

### Pacing
Fixed-tick world stepping with render interpolation: `BASE_TICKS_PER_SEC = 5` at 1x, decisions
every `DECISION_INTERVAL = 5` ticks, capped at `MAX_TICKS_PER_FRAME = 8` ticks per rendered frame.
**Pause** with `P`; **speed** with `-` / `+` (or `[` / `]`) across
`0.5 / 1 / 1.5 / 2 / 3 / 4 / 6×`. A match ends at the level `horizon` or world-wide elimination,
decided by `World::outcome()`.

### HUD
Top bar: **Player vs Enemy** totals (ships, planets owned, subs owned); the current **layer** +
focused planet; **tick / horizon** + clock; the **send fraction** and **speed/paused** state; the
level **objective**; and the **automation** status. A context-sensitive **controls line** runs
along the bottom. The start/hint **overlay** frames the level at the beginning.

---

## Full control list

**Menus** (main / level-select / pause)
- `Up` / `Down` (or `W` / `S`) or mouse hover — move the highlight
- `Enter` / `Space` or click — select / activate
- `Esc` — back (level-select → main; pause → resume)

**In-level — shared**
- `1` / `2` / `3` / `4` — send fraction 25 / 50 / 75 / 100 %
- mouse-wheel up / `Enter`-on-a-planet / click a planet — zoom **in**
- mouse-wheel down / `Esc` — zoom **out** to the lens
- `A` — toggle **automation** on the focused/selected owned planet (if the level allows it)
- right-click / `Esc` — clear the current selection (`Esc` with nothing selected, in the lens,
  opens the Pause overlay)
- `P` — pause / resume
- `-` / `+` (or `[` / `]`) — slower / faster

**In-level — Layer-2 lens**
- left-click your planet → click a lane-connected planet — send a **fleet** (or drag source→target)
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

| # | Title | Opens | Enemy | Teaches |
|---|---|---|---|---|
| 1 | First Moves | interior | Passive | select a sub, send a fraction, capture |
| 2 | Contact | interior | Greedy (local) | concentration of force; layout decides who fights |
| 3 | Two Worlds | lens | Greedy (local) | inter-planet fleets, zoom-to-micro, automation |
| 4 | Hold the Line | lens | Greedy (local) | reinforce + lean on automation |
| 5 | Three Fronts | lens | Greedy (local) | multi-front concentration on a triangle |
| 6 | The Prize | lens | Greedy (local) | expansion-vs-defense timing around a fat neutral |
| 7 | The Seam | lens | Greedy (local) | exploit greedy's undefended rear |
| 8 | Overreach | lens | Colonize | strike undefended production (attack ≻ colonize) |
| 9 | The Turtle | lens | Defend | out-expand a turtle (colonize ≻ defend) |
| 10 | The Hammer | lens | Attack | punish the over-committed stack (defend ≻ attack) |

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

1. **Every campaign level** (all 10): an `--auto` match (both seats AI) **terminates**, **latches a
   deterministic outcome by the level horizon**, and is **bit-reproducible** on a rerun (same seed →
   identical final `state_hash` and winner). Each line reads e.g.
   `L 1 First Moves … ended=true tick=…/1200 winner=Some(Player) det=true -> PASS`.
2. **Player basic automation issues effective orders**: on an automation level, a player who does
   **nothing** should not gain ground, but the same player with every owned planet set to AUTO
   should **expand** (capture sub-structures) via the greedy adapter — `auto-peak > idle-peak`.

The footer prints `== self-test: ALL PASS ==` when every check passes (the exit status reflects it).
This is the headless verification that all 10 levels run end-to-end.

### Screenshot capture (`--shot`) — render one frame and exit

The binary can also render **one frame and exit**, so a built UI is checkable via PNGs:

| Flag | Meaning |
|---|---|
| `--shot <path>` | Render one frame to `<path>` (PNG) and exit. |
| `--screen <menu\|select>` | Capture a menu instead of a level. |
| `--level <N>` | Load level `N` (1-based) for the shot. |
| `--view <lens\|interior>` | Open the shot in the lens or zoomed into the start planet. |
| `--at-tick <T>` | Advance the sim to tick `T` before capturing (deterministic, frame-rate-independent). |
| `--auto` | Drive **both** seats by AI while advancing (for gameplay frames). |
| `--seed <S>` | Seed the world build (decimal or `0x…` hex). |

The capture races the sim to the target tick, renders a couple of **settle** frames so the
framebuffer is fully drawn, then `get_screen_data().export_png(path)` (the same idiom as
`layer1-game` / `layer2-game`). Note the framebuffer is grabbed **after** the final draw but
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
  the first **delegation** step (toggle a whole planet to the greedy policy); the visual rule
  editor is future work.
- **Demo mode disables pointer orders.** With `--auto`, both seats are AI and human fleet/move
  clicks are ignored (zoom/pause/speed still work) — it is a spectator demo.
