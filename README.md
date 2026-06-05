# Machine Intelligence — a cell-game RTS

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

> The original design hand-off lives in `00-overview.md` … `04-open-questions-and-next-steps.md`.
> Those four documents are the "why"; this README is the "what we built."

---

## The resistance era — what changed

Capture used to be instant. It now resolves through three coupled mechanics that turn every
planet into a contestable siege; the AI reasons over a forward projection of where they lead.
Exact formulas and constants live in `AUTOMATA_DESIGN.md` §1.

- **Capture is a resistance grind.** Every sub-structure carries `resistance ∈ [0, max]`
  (default `max = 1800`), starting full. The **lone present enemy faction** erodes it by its
  present-ship count each tick; the **owner present alone heals** it; it is **frozen** when zero
  or 2+ factions are present (a firefight must be won before a capture advances); at `≤ 0` it
  **flips** to the attacker and refills. So a capture is *clear the defenders, then hold to grind
  it down* — and a returning defender repairs it, so hit-and-run accomplishes nothing.
- **Production denial.** A sub **stops producing** while it is being eroded undefended (one
  foreign faction present, owner absent). Parking on an enemy sub **starves its output before you
  ever capture it** — real economic damage at less than the cost of a full siege. A
  contested-*but-defended* sub keeps producing.
- **Anti-hoard soft cap (no hard ceiling).** Per planet/faction, parked ships above
  ≈ `20 + 10 × owned-subs` (≈ 10× production) are destroyed at random — `ceil(0.5·√over)`/tick,
  a self-limiting plateau, **not** a wall. Inter-planet fleets in transit are **exempt**, so
  surplus must be **spent or kept moving**; you cannot turtle on one mountain of ships.
- **A mean-field forward projection (`world::project_forward`).** The AI plans against one
  read-only, enemy-ignoring, RNG-free look-ahead: *if no new orders are issued and the enemy does
  nothing, considering only ships already in transit and the grind their arrivals drive, when (if
  ever) does each sub change owner, and to whom?* Every automaton calls it once per decision tick
  and re-projects rather than trusting a stale plan.

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
- **Read the siege (zoom in).** Each sub shows a **resistance bar** that drains in the
  **attacker's colour** as it is ground down (green outline while the owner heals it); a sub being
  captured wears a **pulsing ring** in the attacker's colour; the **production ring disappears**
  while a sub is denied (being eroded undefended = not producing); and a **`garrison X/Y`**
  readout shows your soft-cap headroom on the focused planet, turning **amber `near cap`** then
  **red `OVER CAP — ships bleeding`** when the anti-hoard attrition kicks in.

> Windows note: the game builds and runs normally. (An earlier Smart App Control policy on this
> machine — `os error 4551` — could refuse freshly-linked binaries; that has been disabled and is
> no longer a concern.)

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
| **`game`** | **the v1 product**: menu, level select, the zoomable two-layer game (incl. the **siege UI**), automation, tutorials |
| `levels` | the 10-level campaign as data + headless validation of every lesson |
| `world` | the unified engine: planets (= Layer-1 structures) + lanes + inter-planet fleets + the Layer-2 aggregate; **`world::projection`** — the read-only, RNG-free forward look-ahead (`project_forward` → `Projection`) every automaton plans over |
| `ai` | the layer-agnostic **greedy** policy + the controller/roster, plus **`ai::vocab`** (predicates / actions / projection queries) and **`ai::automata`** — SimpleColonizer + colonize/defend/attack rebuilt as **compositions** over that vocabulary |
| `layer1` | the spatial micro-sim: sub-structures with **resistance** (capture grind + denial + anti-hoard soft cap), discrete ships, proximity battle bubbles, stochastic square-law combat |
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
cargo build --workspace                 # build everything
cargo run -p game --release -- --selftest   # headless game-loop self-test (no display needed)

# Re-run the foundational measurements:
cargo run -p r2-sweep --release                       # -> R2_RESULTS.md
cargo run -p automaton --bin capstone --release       # -> CAPSTONE_RESULTS.md

# Standalone single-layer sandboxes:
cargo run -p layer1-game --release
cargo run -p layer2-game --release
```

Tests: `cargo test` (each crate is green).

The whole simulation is **deterministic** (mean-field is RNG-free; the spatial layer uses one seeded
PRNG, and the AI's forward projection draws **no** randomness, so planning never perturbs the
per-tick `state_hash`), so every result above is bit-reproducible from its seed.

---

## Status & what's next

**Done (v1, resistance era — on `main`):** the resistance/denial/soft-cap mechanics, the unified
two-layer game with the on-screen siege UI, the forward **projection** (`world::project_forward`),
the four projection-driven automatons (`ai::automata` over `ai::vocab`) plus the rest of the roster,
the closed diamond rock-paper-scissors, the 10-level campaign, the menu/GUI, and per-planet basic
automation (the first *operator → programmer* step).

**Next opponent — the COUNTER (in progress).** An **opponent-modeling, adaptive** AI: observe the
human's play, infer which strategy/mix they are running, and shift to its counter (the arc that
turns the player from *operator* into something a model must reason about). The literature has been
researched; the implementation is underway. The arc-1 `automaton` capstone (*scout → infer → counter*)
is the proof-of-concept this builds on.

**Deferred / future:**
- The **Architect** — the autoconstructive, self-improving opponent (`architect/`, ~70% built; its
  collapse-detection instrument `diversity.rs` is done and unit-tested, but the full evolutionary run
  is not wired up). **Deferred** behind the COUNTER.
- A visual **rule editor** (the full "programmer" layer — author your own `condition→action`
  subroutines, the same substrate the AI uses). The DSL exists (`cell-core`, `DSL_SPEC.md`); the
  editor UI is not built.
- Interactive **playtest & feel-tuning**, juice (animation/sound), and more levels.

### Document map
`00`–`04` = the original design hand-off · `WORLD.md` `AI.md` `LEVELS.md` `GAME.md`
`LAYER1_SIM.md` `LAYER1_GAME.md` `LAYER2_GAME.md` `DSL_SPEC.md` = per-component implementation notes
· `AUTOMATA_DESIGN.md` = the resistance/denial/soft-cap mechanics, the forward projection, and the
four automatons (authoritative for the resistance era) · `R2_RESULTS.md` `CAPSTONE_RESULTS.md` =
the measured results.
