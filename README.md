# Machine Intelligence — a cell-game RTS

A minimalist real-time strategy game in the Solarmax / Auralux / Galcon "cell game" lineage,
where the real product is the **opponents** and the **arc of how you play**: *operator →
programmer → meta-programmer*. You hold structures that produce ships and send fractions of
them across a graph to colonize, defend, and attack — against AI of escalating epistemic
sophistication.

This repo is the **v1**: two playable zoom layers, a roster of Automaton opponents, a
10-level campaign, and a menu-driven GUI. It is built on a foundation whose load-bearing
design risks were **measured, not assumed** (see *Validated results* below).

> The original design hand-off lives in `00-overview.md` … `04-open-questions-and-next-steps.md`.
> Those four documents are the "why"; this README is the "what we built."

---

## Play it

```sh
cargo run -p game --release
```

- **Main menu → Level Select → play.** Progress unlocks sequentially (saved to `mi_progress.json`).
- **One world, two zoom layers.** The **Layer-2 lens** shows planets as nodes on lanes; click a
  planet (or mouse-wheel / Enter) to **zoom into its Layer-1 interior** — the same planet's
  sub-structures, ships, and proximity "battle bubbles."
- **Controls:** click a planet/sub you own → click a linked target (or click-drag) to send;
  `1/2/3/4` = send 25/50/75/100%; `A` = toggle **basic automation** on a planet (hands its
  internals to the greedy policy); mouse-wheel / `Enter` / `Esc` to zoom; `P` pause; `-`/`+` speed;
  `Esc` pause menu.

> Windows note: run the **release** binary. Freshly-linked *debug/test* binaries are intermittently
> blocked from launching by Smart App Control (`os error 4551`); release binaries run fine.

### The campaign (10 levels)
| # | Title | Layer / enemy | Teaches |
|---|---|---|---|
| 1 | First Moves | L1 · passive | move ships, capture |
| 2 | Contact | L1 · greedy | concentration; layout decides who fights |
| 3 | Two Worlds | L2 + zoom · greedy | inter-planet fleets, zoom, **automation** |
| 4–6 | Hold the Line / Three Fronts / The Prize | L2 · greedy | multi-front play, expansion-vs-defense |
| 7 | The Seam | L2 · greedy | **exploit** the greedy Automaton's thin-rear flaw |
| 8 | Overreach | L2 · **Colonize** | strike undefended growth (attack ≻ colonize) |
| 9 | The Turtle | L2 · **Defend** | out-expand the turtle (colonize ≻ defend) |
| 10 | The Hammer | L2 · **Attack** | punish the over-committer (defend ≻ attack) |

L8–L10 are the three edges of the validated rock-paper-scissors cycle.

---

## Architecture — "one world, Layer 2 is a lens"

There is a **single simulation substrate** (the spatial Layer-1 sim). Layer 2 is a zoomed-out
*lens* over many Layer-1 planets plus the lanes fleets travel between them — the camera changes,
the sim does not. This is the design's signature principle: *decouple computation from spectacle.*

```
World ── planets[] ─> Planet { layer1::Structure + map position }   ← the ONE sim (embodied, spatial)
      └─ lanes[]    ─> inter-planet fleets travel here
Layer-2 lens = aggregate of each planet (owner / ships / contested) + lanes  ← a camera + the strategic view
```

### Crate map
| Crate | Role |
|---|---|
| **`game`** | **the v1 product**: menu, level select, the zoomable two-layer game, automation, tutorials |
| `levels` | the 10-level campaign as data + headless validation of every lesson |
| `world` | the unified engine: planets (= Layer-1 structures) + lanes + inter-planet fleets + the Layer-2 aggregate |
| `ai` | the layer-agnostic **greedy** policy + pure **colonize/defend/attack** (+ mixes) + the controller/roster |
| `layer1` | the spatial micro-sim: sub-structures, discrete ships, proximity battle bubbles, stochastic square-law combat |
| `cell-core` | the deterministic **mean-field** engine + the shared `condition→action` DSL + the legible feature set |
| `automaton` | the hidden-mix Automaton ladder + the arc-1 *scout → infer → counter* capstone |
| `r2-sweep` | the day-one go/no-go tool that validated the strategic core (risk R2) |
| `layer1-game`, `layer2-game` | standalone single-layer sandboxes (dev playgrounds; superseded by `game`) |
| `architect` | the autoconstructive self-improving AI — **deferred** (≈70% built; excluded from the default build) |

---

## Validated results (measured, not assumed)

- **R2 — the triad is genuine rock-paper-scissors.** A fat region of the `(rate, defender,
  commitment)` space exists where attack ≻ colonize ≻ defend ≻ attack all hold; robust operating
  point `r=0.6, k=2.25, l=0.15`. → `R2_RESULTS.md`
- **Arc-1 capstone works.** *Scout → infer the hidden mix → counter* beats a non-inferring baseline
  **0.76 vs 0.66**. Surprise the docs didn't predict: difficulty is driven by **defend-content**,
  not simplex centrality (centrality–difficulty correlation ≈ 0). → `CAPSTONE_RESULTS.md`
- **The AI roster behaves as designed.** The greedy Automaton's thin-rear seam is exploitable
  **7/7** seeds; the RPS cycle closes **10-0** per edge on the diamond map. → `AI.md`
- **The campaign teaches what it claims.** L8/L9/L10 counters win **10-0-0**, L7 flank wins **5/5**,
  L1–L6 are winnable. → `LEVELS.md`
- **The game loop is verified.** `cargo run -p game --release -- --selftest` drives all 10 levels
  headlessly: every match terminates, latches a deterministic outcome, and the automation path
  expands the player — **ALL PASS**.

---

## Develop

```sh
cargo build --workspace                 # build everything
cargo run -p game --release -- --selftest   # headless game-loop self-test (no display needed)

# Re-run the foundational measurements:
cargo run -p r2-sweep --release                       # -> R2_RESULTS.md
cargo run -p automaton --bin capstone --release       # -> CAPSTONE_RESULTS.md

# Standalone single-layer sandboxes:
cargo run -p layer1-game --release
cargo run -p layer2-game --release
```

Tests: `cargo test` (each crate is green). On Windows, run tests with a target dir outside the
Desktop tree to dodge the `4551` block, e.g. `$env:CARGO_TARGET_DIR="$env:USERPROFILE\.cargo-mi-target"`.

The whole simulation is **deterministic** (mean-field is RNG-free; the spatial layer uses one seeded
PRNG), so every result above is bit-reproducible from its seed.

---

## Status & what's next

**Done (v1):** the unified two-layer game, the Automaton roster, the 10-level campaign, the
menu/GUI, and per-planet basic automation (the first *operator → programmer* step).

**Deferred / future:**
- A visual **rule editor** (the full "programmer" layer — author your own `condition→action`
  subroutines, the same substrate the AI uses). The DSL exists (`cell-core`, `DSL_SPEC.md`); the
  editor UI is not built.
- The **Architect** — the autoconstructive, self-improving opponent (`architect/`, ~70% built; its
  collapse-detection instrument `diversity.rs` is done and unit-tested, but the full evolutionary run
  is not wired up).
- Interactive **playtest & feel-tuning**, juice (animation/sound), and more levels.

### Document map
`00`–`04` = the original design hand-off · `WORLD.md` `AI.md` `LEVELS.md` `GAME.md`
`LAYER1_SIM.md` `LAYER1_GAME.md` `LAYER2_GAME.md` `DSL_SPEC.md` = per-component implementation notes
· `R2_RESULTS.md` `CAPSTONE_RESULTS.md` = the measured results.
