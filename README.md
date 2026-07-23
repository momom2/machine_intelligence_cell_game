# Machine Intelligence — a cell-game RTS

A minimalist real-time strategy game in the *cell game* lineage (Galcon / Auralux / Solarmax):
you hold sub-structures that produce ships, send fractions of them across a board to colonize,
defend, and attack, and try to eliminate your opponents. The twist is what you play *against* —
the real product of this project is the **AI opponents** and the arc of learning to out-think
them.

Written in Rust. The whole simulation is headless and deterministic, so every match is
reproducible from a seed and can be recorded and replayed exactly.

## Quickstart

You need a [Rust toolchain](https://rustup.rs/). From the repo root:

```sh
cargo run -p game --release
```

That builds and launches the game. Or build a standalone executable next to `assets/`:

```sh
./build.sh        # POSIX  — produces ./game
build.cmd         # Windows — produces game.exe at the repo root
```

Pre-built desktop and browser (WebAssembly) builds are published on the
[releases page](https://github.com/momom2/machine_intelligence_cell_game/releases/latest).
On Windows, SmartScreen may warn on an unsigned executable — choose "More info" → "Run anyway".

From the main menu, **Play** starts the campaign. See **[how to play](docs/gameplay.md)** for the
controls and how to read the board.

## What it is

You command one faction (cyan) on a single board of **sub-structures**. Owned sub-structures
produce ships; capturing more of them grows your economy, which grows your fleet — the snowball
is the game. Capture is not instant: it is a **resistance grind** you can see happening, and a
returning defender heals it, so you have to *concentrate force and hold what you take*.

The design's ambition is a progression from **operator** (moving ships well) to **programmer**
(delegating and automating) to **meta-programmer** (reasoning about an opponent that reasons
about you), carried by a roster of AI opponents of escalating sophistication. The current build
is a 7-mission campaign against hand-written, legible AI brains — each with a documented
strength and a diagnosable weakness.

Under the hood, the guiding principle is **decouple computation from spectacle**: all the game
logic lives in small, dependency-light, fully-tested headless crates, and the graphical binary
only draws the world and turns your input into the same orders the AI issues.

## Documentation

- **[How to play](docs/gameplay.md)** — controls, reading the board, and the campaign.
- **[The simulation model](docs/simulation.md)** — how the economy, capture, and combat actually
  resolve.
- **[Architecture](docs/architecture.md)** — how the code is organised, for contributors.

## Architecture at a glance

A Cargo workspace. The live game is four crates; the rest is a deferred research track that still
compiles but is not on the game path. Full detail in [architecture.md](docs/architecture.md).

| Crate | Role |
|---|---|
| `layer1` | The headless, deterministic spatial simulation (the game's substrate). |
| `ai` | The enemy brains + the roster/seat dispatch that runs one per enemy seat. |
| `levels` | The 7-level campaign as data (`.lvl` files) + validation. |
| `game` | The single macroquad binary: menu, level select, the match, replays. |
| `cell-core`, `automaton`, `architect` | A deferred mean-field / autoconstructive-AI research track. |

## Develop

```sh
cargo build --workspace                    # build everything
cargo test                                 # run the default-member test suites
cargo run -p game --release -- --selftest  # headless: drive every level, assert determinism
```

The game binary also carries screenshot and replay verification flags
(`--shot`, `--level`, `--at-tick`, `--auto`, `--seed`, `--replay`, `--snaptest`) so a built UI is
checkable headlessly. Levels are plain-text `.lvl` files under `assets/levels/` — editing a
mission needs no recompile, just a restart.

## Status

The game is **pure Layer-1**: one board per match, balanced around direct play. An earlier design
had a second, zoomed-out strategic layer of many boards joined by lanes; it was removed and lives
on the `layer2` branch. Active work is on balancing the campaign and the AI. The deferred
research crates (`cell-core` / `automaton` / `architect`) are a longer-horizon track — an
autoconstructive, self-improving opponent — that is not yet wired into the live game.
