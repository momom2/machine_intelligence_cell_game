# Machine Intelligence — a cell-game RTS

**▶ Play it (Windows):** grab `game.exe` from the latest release (unzip next to `assets/`,
double-click; Windows SmartScreen may warn on an unsigned exe — "More info" → "Run anyway"),
or build from source: `build.cmd` (drops `game.exe` at the repo root), or
`cargo run -p game --release`. Feedback is very welcome.

A minimalist real-time strategy game in the Solarmax / Auralux / Galcon "cell game" lineage,
where the real product is the **opponents** and the **arc of how you play**: *operator →
programmer → meta-programmer*. You hold structures that produce ships and send fractions of
them across a graph to colonize, defend, and attack — against AI of escalating epistemic
sophistication.

This repo is the **resistance-era v1**: two playable zoom layers, a roster of Automaton
opponents that reason over a forward **projection** of the world, a 10-level campaign, and a
menu-driven GUI. Capture is no longer instant — it is a **siege grind** the UI surfaces in real
time. It is built on a foundation whose load-bearing design risks were **measured, not assumed**
(see *Validated results* below).

> The design docs and the dev changelog are internal (untracked — the shipped repo carries
> the game, not the process); this README is the public face.

> ⚠️ **Current work is on branch `feat/counter`**, which reworked the economy, combat, ship
> movement, the inter-struct plumbing, and the whole GUI on top of the resistance era. **the dev changelog (internal, untracked)
> is authoritative** for current mechanics; where this README still describes the older model it is
> flagged inline below. The AI **automata** track (Colonize/Defend/Attack, the Counter, the diamond
> RPS) is **parked** — the campaign now plays against `SimpleColonize` only.

---

## The resistance era — what changed

Capture used to be instant. It now resolves through three coupled mechanics that turn every
struct into a contestable siege. (The original formulas live in `docs/archive/AUTOMATA_DESIGN.md`
§1; the constants have since moved — the dev changelog (internal, untracked) is authoritative.)

- **Capture is a resistance grind.** Every sub-structure carries `resistance ∈ [0, max]`,
  starting full; the default max is **proportional to the sub's storage capacity**
  (`capacity · 60` — `3600` for a default sub, so bigger subs are harder to take). The **lone present enemy faction** erodes it by its
  present-ship count each tick; the **owner present alone heals** it; it is **frozen** when zero
  or 2+ factions are present (a firefight must be won before a capture advances); at `≤ 0` it
  **flips** to the attacker and refills. So a capture is *clear the defenders, then hold to grind
  it down* — and a returning defender repairs it, so hit-and-run accomplishes nothing.
- **Production denial.** A sub **stops producing** while it is being eroded undefended (one
  foreign faction present, owner absent). Parking on an enemy sub **starves its output before you
  ever capture it** — real economic damage at less than the cost of a full siege. A
  contested-*but-defended* sub keeps producing.
- **Anti-hoard soft cap (no hard ceiling).** _(feat/counter: reworked — see the dev changelog (internal, untracked).)_ Each
  **sub** now has its own `storage_capacity` (default 60); ships above it bleed at
  `surplus / (60 · production_period)`/tick, settling at an effective cap of `storage + 60 ×
  production` ≈ 120 per sub — a self-limiting plateau, **not** a wall. Inter-struct fleets in transit
  are **exempt**, so surplus must be **spent or kept moving**. _(The old per-struct `≈ 20 + 10 ×
  owned-subs` / `ceil(0.5·√over)` cap and the global "garrison X/Y" readout were removed.)_
- **A mean-field forward projection (`world::project_forward`) — PARKED.** The deferred
  automata/Counter track plans against this read-only, enemy-ignoring, RNG-free look-ahead. **The
  live game builds no projection**: the campaign Simple (and every live roster) reads the
  projection-free `World::sub_influx_for` — the current in-flight state attributed to the subs
  ships will actually land at.

---

## Play it

```sh
cargo run -p game --release
```

- **Main menu → Level Select → play.** Progress unlocks sequentially (saved to `mi_progress.json`).
- **One world, two zoom layers.** The **Layer-2 lens** shows structs as nodes on lanes; click a
  struct (or mouse-wheel / Enter) to **zoom into its Layer-1 interior** — the same struct's
  sub-structures, ships, and proximity "battle bubbles."
- **Controls:** _(see the dev changelog (internal, untracked) for the current UI.)_ Click a struct/sub you own, then click a
  linked target to send; **left-drag a box** to multi-select all your subs/structs inside it and the
  next click orders them all. The **top bar** holds a continuous **1–100% troop slider** (default 100%)
  and a discrete **speed slider** (`0×`=paused / 1× / 3× / 10× / 25×); a clock counts up. The
  **mouse-wheel** and a right-side **zoom slider** (0.5×–7×) zoom between the lens and a struct
  interior; `Esc` opens the pause menu; **`F3`** toggles a frame-timing overlay. Starts **paused**;
  closing the briefing unpauses.
- **Read the siege (zoom in).** Each sub shows a **resistance bar** that drains in the
  **attacker's colour** as it is ground down (green outline while the owner heals it); a sub being
  captured wears a **pulsing ring** in the attacker's colour; the **production ring disappears**
  while a sub is denied (being eroded undefended = not producing). Ships orbit their sub's ring as
  **real sim positions** (what you see is the combat geometry); **per-side present counts** show in
  each faction's colour; the big enclosing outline is the **reserve / patrol-zone node** all
  inter-struct fleets pass through. _(The old global "garrison X/Y" soft-cap readout was removed;
  attrition is now per-sub.)_

> Windows note: the game builds and runs normally. (An earlier Smart App Control policy on this
> machine — `os error 4551` — could refuse freshly-linked binaries; that has been disabled and is
> no longer a concern.)

### The campaign (10 levels)
_(Now a **full Simple campaign** — L1 = Passive, **L2–L10 = `SimpleColonize`**. The per-level lessons
and the L8–L10 rock-paper-scissors framing below are **parked**. **L1–L3 are hand-authored
single-struct levels** (L3 is a two-AI free-for-all); **L4–L10 are placeholder multi-struct worlds**
awaiting redesign. Basic player-automation is currently **quarantined** (off on every level). Difficulty
is ad-hoc, not a curve.)_

| # | Title | Enemy | Notes |
|---|---|---|---|
| 1 | First steps | Passive | single struct; move ships, capture |
| 2 | Fire in the sky | Simple | single struct; concentration — the middle posts decide it |
| 3 | Deliberation | Simple × 2 | single struct; a three-way **free-for-all** |
| 4–6 | Far far away / Three Fronts / The Prize | Simple | placeholder multi-struct worlds (redesign pending) |
| 7 | The Seam | Simple | placeholder (was: exploit the greedy's thin-rear flaw) |
| 8–10 | Overreach / The Turtle / The Hammer | Simple | placeholder (was: the attack≻colonize≻defend≻attack RPS edges) |

---

## Architecture — "one world, Layer 2 is a lens"

There is a **single simulation substrate** (the spatial Layer-1 sim). Layer 2 is a zoomed-out
*lens* over many Layer-1 structs plus the lanes fleets travel between them — the camera changes,
the sim does not. This is the design's signature principle: *decouple computation from spectacle.*

```
World ── structs[] ─> Structure { layer1::Structure + map position }   ← the ONE sim (embodied, spatial)
      └─ lanes[]    ─> inter-struct fleets travel here
Layer-2 lens = aggregate of each struct (owner / ships / contested) + lanes  ← a camera + the strategic view
```

### Crate map
| Crate | Role |
|---|---|
| **`game`** | **the v1 product**: menu, level select, the zoomable two-layer game (incl. the **siege UI**, box-select, F3 perf overlay), tutorials |
| `levels` | the 10-level campaign as data (each `Level` declares its `enemies: Vec<Roster>`) + headless validation |
| `world` | the unified engine: structs (= Layer-1 structures) + lanes + inter-struct fleets + the Layer-2 aggregate. **`world::projection`** (`project_forward` → `Projection`) is **PARKED** — the live game reads the projection-free `World::sub_influx_for` instead; the projection survives only for the deferred automata/Counter track |
| `ai` | the layer-agnostic **greedy** policy + the controller/roster + the live stateful **`Simple`** (`ai::simple`). `ai::vocab` / `ai::automata` / `ai::counter` (the projection-driven automatons) are **parked** |
| `layer1` | the spatial micro-sim: sub-structures with **resistance** (capture grind + denial + per-sub economy), discrete ships, proximity combat, stochastic square-law. Seats are **`Faction::{Neutral, Player, Ai(u8)}`** — any number of AI opponents, declared by the level |
| `cell-core`, `automaton`, `architect` | the **deferred** Apprentice/Architect mean-field track (the older DSL engine + hidden-mix ladder + autoconstructive AI). Present and compiling but not in the live game path |

---

## Validated results (measured, not assumed) — *historical, parked*

> These results validated the **automata/Counter track**, which is currently **parked** behind the
> full-Simple campaign. They are accurate as history (the measurements were real); the source docs are
> now under `docs/archive/`. The current campaign plays only Passive + `SimpleColonize`.

- **R2 — the triad is genuine rock-paper-scissors.** A fat region of the `(rate, defender,
  commitment)` space exists where attack ≻ colonize ≻ defend ≻ attack all hold; robust operating
  point `r=0.6, k=2.25, l=0.15`. → `R2_RESULTS.md`
- **Arc-1 capstone works.** *Scout → infer the hidden mix → counter* beats a non-inferring baseline
  **0.76 vs 0.66**. Surprise the docs didn't predict: difficulty is driven by **defend-content**,
  not simplex centrality (centrality–difficulty correlation ≈ 0). → `CAPSTONE_RESULTS.md`
- **The AI roster behaves as designed.** The four automatons (`ai::automata`, compositions over
  `ai::vocab` + the forward projection) close the diamond rock-paper-scissors over **both seatings
  and every seed** — **attack ≻ colonize 10-0**, **colonize ≻ defend 10-0**, **defend ≻ attack
  6-4** (5 seeds × 2 seatings; the fragile `defend ≻ attack` edge holds 10-6 over 8 seeds). The
  greedy Automaton's thin-rear seam — now expressed as *sustained denial* of the undefended rear
  under the grind — is exploitable **7/7** seeds. The corridor map honestly does **not** fully
  close (a reported, map-dependent negative result). → `AI.md`, `AUTOMATA_DESIGN.md`
- **The campaign teaches what it claims.** L8/L9/L10 counters win **10-0 / 10-0 / 6-4** on the
  diamond, L7 flank wins **5/5**, L1–L6 are winnable (L6's prize re-tuned to `max_resistance = 600`
  so the grind resolves in horizon). → `LEVELS.md`
- **The game loop is verified.** `cargo run -p game --release -- --selftest` drives all 10 levels
  headlessly: every match terminates, latches a deterministic outcome, and the automation path
  expands the player — **ALL PASS**.

---

## Develop

```sh
cargo build --workspace                      # build everything
cargo run -p game --release                  # play
cargo run -p game --release -- --selftest    # headless game-loop self-test (10 levels, deterministic)
```

Tests: `cargo test -p layer1 -p world -p ai` (the live crates). `cargo test -p ai simple` is the fast
slice; the full `-p ai` / `-p levels` suites include the **parked** automata/projection battery and run
for minutes (one parked Counter gate is `#[ignore]`d pending the automata re-tune). The deferred Apprentice/Architect track (`cargo run -p automaton --bin capstone`, etc.)
still builds but is excluded from the default-members build.

The whole simulation is **deterministic** (mean-field is RNG-free; the spatial layer uses one seeded
PRNG, and the AI's forward projection draws **no** randomness, so planning never perturbs the
per-tick `state_hash`), so every result above is bit-reproducible from its seed.

---

## Status & what's next

> **Live status (branch `feat/counter`) lives in the dev changelog (internal, untracked).** The section below describes the
> **resistance-era v1** baseline (on `main`); `feat/counter` builds on it with the per-sub economy,
> the struct-storage reserve node, WYSIWYG orbit, grid-spread combat, and a reworked GUI, and parks
> the automata/Counter track behind a **full Simple campaign** + level/difficulty redesign.

**Done (v1, resistance era — on `main`):** the resistance/denial/soft-cap mechanics, the unified
two-layer game with the on-screen siege UI, the forward **projection** (`world::project_forward`),
the four projection-driven automatons (`ai::automata` over `ai::vocab`) plus the rest of the roster,
the closed diamond rock-paper-scissors, the 10-level campaign, the menu/GUI, and per-struct basic
automation (the first *operator → programmer* step).

**Next opponent — the COUNTER (built, then PARKED).** An **opponent-modeling, adaptive** AI:
observe the play, infer which strategy/mix the opponent runs, and shift to its counter (the arc
that turns the player from *operator* into something a model must reason about). Phases 1–3
(observation log, inference, synthesis + diagnostic battery) are implemented in `ai::counter`, but
the whole automata/Counter track is **parked** behind the full-Simple campaign and the mission
redesign; it revives together with the projection rework.

**Deferred / future:**
- The **Architect** — the autoconstructive, self-improving opponent (`architect/`, ~70% built; its
  collapse-detection instrument `diversity.rs` is done and unit-tested, but the full evolutionary run
  is not wired up). **Deferred** behind the COUNTER.
- A visual **rule editor** (the full "programmer" layer — author your own `condition→action`
  subroutines, the same substrate the AI uses). The DSL exists (`cell-core`, `DSL_SPEC.md`); the
  editor UI is not built.
- Interactive **playtest & feel-tuning**, juice (animation/sound), and more levels.

### Document map
**the dev changelog (internal, untracked) = authoritative for current mechanics.** Live component docs under `docs/` —
`GAME.md` `WORLD.md` `LEVELS.md` `LAYER1_SIM.md` — lag the CHANGELOG and each carries a status header.
Everything else (the `00`–`04` hand-off, `AUTOMATA_DESIGN.md`, `AI.md`, `DSL_SPEC.md`, the `COUNTER_*`
/ `CAPSTONE_*` / `R2_*` design+results, and the deleted sandboxes' docs) is **archived under
`docs/archive/`** — accurate history of the parked/deferred work, not current focus.
