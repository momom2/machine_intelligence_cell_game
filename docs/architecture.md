# Architecture

How the code is organised and why. Read this before contributing. For how the game *plays*
see [gameplay.md](gameplay.md); for the rules the sim enforces see
[simulation.md](simulation.md).

## The signature principle: decouple computation from spectacle

**All model logic lives in headless, dependency-light crates; the graphical binary only (a)
draws the world and (b) turns human input into the same orders the AI issues.** The game owns
only spectacle — the camera, the frame pacing, menus. Anything that decides the outcome of a
match lives in a crate that has no idea a screen exists.

This is what makes the game testable and reproducible: every mechanic can be exercised
headlessly, and a match is fully determined by `(board, seed, order stream)`.

## Pure Layer-1

The game is a single **`layer1::Interior`** — one structure of sub-structures, ships, capture
grinds, and orbits, under one zoomable camera. There is no world graph and no second zoom
layer. (An earlier design had a Layer-2 "lens" of many structures joined by lanes; it was
removed on 2026-07-20 and is preserved on the `layer2` branch, along with the deleted `world`
crate and the v1 replay format.)

## Crate map

It is a Cargo workspace. The live game is the top four crates; the rest is a deferred research
track that still compiles but is not on the game path.

| Crate | Role |
|---|---|
| **`layer1`** | The headless, deterministic **spatial simulation**: `Interior`, sub-structures, ships, proximity square-law combat, resistance capture, the per-sub economy, and the orbit model. Zero external deps + its own seeded PRNG. This is the game's substrate — see [simulation.md](simulation.md). |
| **`ai`** | The **enemy brains**. A layer-agnostic greedy tactical policy; the stateful campaign controllers (the ledger-driven **Simple** colonizer, its adjacency-leashed variant, and the scripted **Cycler** drillmaster); a `Roster` + `SeatController` dispatch that steps one brain per enemy seat; and a headless AI-vs-AI harness. Draws no randomness of its own. |
| **`levels`** | The **7-level campaign as data**. Each level's metadata (title, blurb, objective, hints, enemy `Roster` seats, horizon) plus a `build(seed) -> Interior` board-builder, sourced from a plain-text `.lvl` file (see below) or a built-in `fn` for dev scenarios. A validation harness asserts every level builds and is deterministic. |
| **`game`** | The **single macroquad binary**: main menu, sequential-unlock level select, the single-board match with its siege UI, the replay system, and the end-of-mission stats screen. The only crate that draws. |
| `cell-core`, `automaton`, `architect` | A **deferred** research track — a mean-field engine, a hidden-strategy "Automaton ladder," and an autoconstructive evolutionary opponent ("the Architect"). They compile but are not wired into the live game; `architect` is excluded from the default build. |

Dependency direction runs strictly upward: `layer1` → `ai` → `levels` → `game`. Nothing below
the level layer knows how many enemy seats a match has — the level's `enemies` list decides it,
and the engine is otherwise agnostic to seat count.

## Seats and control

The player is always `Faction::Player`; each enemy is a `Faction::Ai(i)` seat driven by
`level.enemies[i]`. Every brain — and the player's own input — issues the *same* `MoveOrder`
through the *same* faction-scoped API, so the human and the AI are genuinely playing the same
game. A `SeatController::decide_and_apply(&mut Interior, &SimParams)` steps one enemy seat per
decision tick.

## Levels are data

A campaign level is a plain-text `.lvl` file under `assets/levels/<NN_name>/`, parsed at startup
(hand-rolled — the workspace has a zero-external-dependency rule, so no serde). Editing a
mission is *edit the file and restart* — no recompile. The format is line-based `key = value`
under section headers:

- `[level]` — id, title, blurb, objective, `hint` lines, the `enemy` seats, horizon, and
  optional presentation dials;
- `[sub]` — one sub-structure (owner, capacity, production, starting ships, position, kind…);
- `[orbit]` — a ring constructor that places its following `[sub]`s evenly around a circle.

Positions may carry `A+-X` uniform noise, drawn once per match from the seed so layouts vary
between matches while staying replay-deterministic. The seven `.lvl` files under `assets/levels/`
are the campaign; the parser module doc (`crates/levels/src/spec.rs`) is the authoritative format
reference.

## Determinism and replays

Because a match is `(board, seed, order stream)`, the game records every seat's orders with tick
stamps into a `.mir` replay and can play it back exactly, verifying periodic state-hash
checkpoints and flagging any divergence. An extended `.mirx` format additionally logs one line
per rendered frame (camera, cursor, inputs) so a playtester's *experience* — including failed
clicks — can be reconstructed. Cross-platform bit-exactness (browser recording ↔ native
playback) is what the `libm` routing in `layer1` buys.

## Verifying a build

The game carries headless flags so a built binary is checkable without a human at the keyboard:

- `--selftest` drives every campaign level headlessly and asserts each is deterministic;
- `--shot <path>` renders one frame to a PNG (with `--level`, `--at-tick`, `--auto`, `--seed`
  to compose a specific situation), so the UI is screenshot-checkable;
- `--replay <file>` / `--snaptest <file>` exercise the replay and snapshot paths end to end.

See the [README](../README.md#develop) for the exact build, test, and run commands.
