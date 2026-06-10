# LAYER2_GAME — the real-time PLAYABLE Layer-2 (tactical) renderer

`crates/layer2-game` is the macroquad GUI over the headless [`cell-core`](crates/cell-core)
deterministic mean-field **graph** engine, with a HIDDEN-mix [`automaton`](crates/automaton)
Automaton as the opponent. This is **Layer 2** from `03-ui-layers.md`: the Solarmax-like
tactical view — a graph of production nodes connected by edges, where you send fractions of a
node's garrison along edges to **colonize** neutral nodes, **reinforce** your own, or **assault**
enemy nodes. It is the classic cell-game / RTS mind.

It honours the design's signature principle (`00-overview.md`, *decouple computation from
spectacle*): **all** model logic lives in `cell-core` (engine) and `automaton` (the enemy
policy); this binary only **draws** the nodes / edges / fleet streams / combat flashes, and turns
human input into `cell_core::Command`s through the *exact same* engine API the AI uses
(`GameState::launch_with`). It does not modify the model — it supplies node **coordinates** (which
the engine does not carry) and a GUI tick pace, data this crate owns, purely to render and pace a
human match.

The human commands seat **A** (Player, cyan); the hidden-mix Automaton commands seat **B** (Enemy,
red). Neutral nodes are grey until colonized.

## The headline experience: scout → infer → counter (the "glass genome")

Per `02-ai-opponents.md` / `CAPSTONE_RESULTS.md`, the enemy is a **hidden but fixed** mix of
colonize / defend / attack. The arc-1 climax is a four-step skill: **scout** the opponent's early
behaviour, **infer** its hidden lean from the legible HUD features, **select** the counter from the
rock-paper-scissors cycle (attack ≻ colonize ≻ defend ≻ attack), and **execute** the now-static
optimization. The mix stays HIDDEN during play and is **REVEALED on the end screen** (e.g.
`Enemy was: colonize 50 / defend 20 / attack 30`) — reading it correctly *was* the game.

The top-bar **`read enemy:`** readout exposes the three tells the design names (expansion rate,
aggression timing, frontier behaviour) so a human can guess the lean at a glance:

- **expansion** — neutral nodes the enemy has colonized so far (rising fast ⇒ *colonize* lean).
- **aggression** — fleets the enemy has thrown at *your* nodes (high ⇒ *attack* lean).
- **frontier-hold** — the enemy's mean garrison on its front-line nodes (heavy ⇒ *defend* lean;
  thin ⇒ colonize/attack).

(Concretely, verified in screenshots: a pure-**Attack** enemy shows *aggression 22, frontier-hold
0.6*; a pure-**Defend** enemy shows *aggression 0, frontier-hold 54* — the vocabulary genuinely
separates the corners.)

---

## How to run (RELEASE)

> Build and run **release**. Freshly linked Windows *debug* exes can be blocked by Smart App
> Control with `os error 4551`; the release binary runs fine. (`cargo run` without `--release`
> may hit that block on this machine.)

```powershell
# From the repo root (a Cargo workspace; layer2-game is a default member).
cargo run -p layer2-game --release
```

Or run the built binary directly:

```powershell
cargo build -p layer2-game --release
.\target\release\layer2-game.exe
```

Optional flags / env (see **Modes** below): `--auto`, `--shot <path>`, `--seed <hex|dec>`.

---

## Controls (mirrors `layer1-game`)

| Input | Action |
|---|---|
| **Left-click your node** | Select it as the command **source** (valid linked neighbours light up). |
| **Left-click a linked node** | Issue `Command { source, target, fraction }` along that edge (colonize / reinforce / assault). |
| **Left-drag** your node → a linked node | Same as click-source then click-target. |
| **Right-click / Esc** | Clear the current selection. |
| **1 / 2 / 3 / 4** | Set the send fraction to **25 / 50 / 75 / 100 %** (default **50 %**). |
| **Space** | Pause / resume. |
| **`-` / `+`** (or **`[` / `]`**) | Slower / faster (speed multiplier). |
| **R** | **New match** — fresh seed ⇒ new map **and** new hidden mix. |

The atomic action only travels **one edge** (the engine's `Command` model): a click on a
non-adjacent node is ignored. Selecting a source highlights exactly the legal targets.

---

## Modes

| Invocation | Behaviour |
|---|---|
| *(none)* | **Human** plays seat A; the hidden-mix Automaton plays seat B. |
| `--auto` or `LAYER2_AUTO=1` | Both seats AI (seat A = a DSL colonizer); hands-off demo, never exits. |
| `--shot <path>` or `LAYER2_SHOT=<path>` | **Headless verification**: both seats AI, race deterministically to a target tick, capture the framebuffer to a PNG, then exit. |
| `--seed <hex\|dec>` or `LAYER2_SEED=…` | Pick the starting seed (selects the map + hidden mix; see below). |
| `LAYER2_SHOT_TICK=<n>` | Override the `--shot` capture tick. A very large value (e.g. `100000`) captures the **end-of-match REVEAL banner**. Default `90`. |

Verification command actually run (writes a non-empty PNG, then exits cleanly):

```powershell
cargo run -p layer2-game --release -- --shot target/layer2_shot.png
# -> [layer2-game] shot written: target/layer2_shot.png (1279x799)  map=corridor7 tick=90 hidden_mix=A
```

---

## How the engine is driven (the computation/spectacle split)

Each rendered frame accumulates wall-clock `dt` and steps the deterministic engine a whole number
of fixed ticks, exactly mirroring `cell_core::GameState::run_match`:

1. **Seat A**: launch the human's queued `Command`s (or, in auto/shot, the player Automaton's
   commands) via `GameState::launch_with(Owner::A, cmd, &params)`.
2. **Seat B**: the hidden-mix enemy `DslPolicy::decide(&state, Owner::B, &params)` → launch each
   command via `launch_with(Owner::B, …)`. (A is applied before B, the engine's documented order.)
3. `GameState::step(&params)` advances exactly one tick (produce → move fleets / resolve arrivals →
   combat).

Fleet motion is **interpolated** between ticks by `progress / edge.length` (undocking fleets sit at
the source until `undock_remaining` hits 0, then stream toward the target), so a slow tick rate
still looks fluid — the engine's determinism explicitly supports render-side interpolation. A
short **flash** is drawn over a node when a fleet arrives and combat/colonization resolves there
(detected by an owner change or a meaningful garrison change at a node that had inbound fleets).

### cell-core / automaton APIs used

- **State & stepping**: `GameState` (`nodes`, `edges`, `fleets`, `tick`), `GameState::step`,
  `GameState::launch_with`, `GameState::edge_between`, `GameState::neighbors`,
  `GameState::territory`, `GameState::is_eliminated`, `GameState::score_from`.
- **Types**: `Owner` (A / B / Neutral), `Command { source, target, fraction }`, `FractionBucket`,
  `types::Fleet` (`from/to/edge/count/undock_remaining/progress`), `Node`, `Edge`.
- **Maps**: `maps::all_maps()` → `corridor7`, `twin_bridge8`, `star9` (each a `MapSpec { name,
  state, mirror }`). The engine carries **no coordinates**, so this crate supplies a per-map
  `layout` matching each map's ASCII diagram and fits it to the window.
- **Features (HUD inference)**: `OwnerFeatures::compute(&state, who, &params)` for the top-bar
  totals (`territory_count`, `total_units`, `production_rate`). The `read enemy:` tells are derived
  locally (cumulative neutral captures, fleets aimed at seat A, mean enemy frontier garrison).
- **Enemy policy**: `automaton::Mix` + `automaton::compile(mix, label)` compiles a colonize/defend/
  attack mix into one legible prioritized DSL rule-set, wrapped in `cell_core::DslPolicy::new(…)` so
  it plays exactly as it does headlessly in the capstone. `Mix::nearest_corner()` / `Mix::centrality()`
  feed the end-screen reveal. The player Automaton (auto/shot only) is `dsl::strategies::colonize()`.
- **Operating point** (R2-validated, so the triad is a genuine RPS cycle):
  `Params { r: 0.6, k: 2.25, l: 0.15, ..Params::default() }`.

## How the hidden enemy mix is chosen + revealed

- A **roster** of 10 mixes spans the simplex rim-to-centre — the three pure corners (C, D, A),
  three lopsided 2:1 blends, three even edge-midpoint blends, and the balanced centre `(⅓,⅓,⅓)` —
  the same sampling the capstone ladder uses, so easy pure corners up to the hardest balanced mix
  all appear.
- `pick_match(seed)` deterministically selects **`map = all_maps()[seed % 3]`** and
  **`mix = roster[(seed / 3) % 10]`**, on independent strides, so each restart (`seed += 1`) varies
  *both* the map and the mix. The mix is kept **hidden** during play.
- On the end screen the true mix is **revealed** as integer percentages summing to 100
  (largest-remainder rounding): e.g. `Enemy was: colonize 100 / defend 0 / attack 0`, plus its
  nearest-corner lean and centrality (difficulty). **R** starts a new match with a new hidden mix.

---

## Pacing constants (the spectacle layer's operating point)

| Constant | Value | Why |
|---|---|---|
| `BASE_TICKS_PER_SEC` | **5.0** | At 1× a full `HORIZON`-tick game lasts ~120 s (2 min); elimination games are shorter — lands the design's **1–3 minute** target. |
| `HORIZON` | **600** | Match horizon (matches the headless capstone/R2). If neither side is eliminated, the engine's combined unit+territory lead decides the winner. |
| `SPEED_STEPS` | `[0.5, 1, 1.5, 2, 3, 4, 6]` | Speed multipliers (0.5× to study a brawl; up to 6× for impatient play). Starts at **1×**. |
| `MAX_TICKS_PER_FRAME` | 8 | Caps engine ticks per rendered frame so a stall / huge speed can't fast-forward the game. |
| `SHOT_TARGET_TICK` | 90 | `--shot` capture tick (default): far enough that both sides have expanded and fleets are in transit. |

> **Why ~5 ticks/s and not faster:** `cell-core` matches are mean-field and resolve smoothly over up
> to 600 ticks; at 5 t/s that's a watchable ~2-minute arc with render interpolation keeping fleet
> motion fluid between ticks. Slower would drag; much faster would blow past the 1–3 minute window.

---

## Maps & layouts

The three symmetric `cell-core` maps, laid out to match their `maps.rs` ASCII diagrams:

- **corridor7** — a straight line `A(0)–P(1)–N(2)–N(3)–N(4)–P(5)–B(6)`.
- **twin_bridge8** — two homes joined by two parallel two-node neutral bridges (top & bottom).
- **star9** — each home owns two private spokes; the fronts meet at a 3-node contested core.

An unknown map falls back to a circular layout, so adding a new `cell-core` map never panics. The
camera fits the layout to the window every frame, so **window resizes are handled for free**.

---

## Caveats / honesty

- **Observed, not assumed:** `cargo build -p layer2-game --release`, `cargo build --workspace`, and
  the default `cargo test` suite all succeed; `--shot` opened a real macroquad window and wrote a
  **non-empty PNG** (`target/layer2_shot.png`, ~106 KB, 1279×799) at tick 90 on `corridor7`, exiting
  cleanly. The end-screen reveal banner and the per-seed map/mix variation were verified visually
  via additional `--shot` captures (seed 4 → `twin_bridge8` + pure-Defend; seed 2 → `star9` +
  pure-Colonize, end banner showing the reveal).
- The captured PNG is the macroquad framebuffer at high-DPI (1279×799), one pixel shy of the 1280×800
  window — this is macroquad's reported drawable size, not an error.
- The **inference tells** (`expansion / aggression / frontier-hold`) are simple cumulative/derived
  read-outs this crate computes from the public state; they are an *aid*, not the engine's own
  feature structs (the top-bar totals do come from `OwnerFeatures`). They are deliberately legible,
  matching the design's "infer the mix at a glance" requirement.
- In `--auto` / `--shot`, seat A is driven by a fixed DSL **colonizer** (so the hands-off demo plays
  a real game); a colonizer loses to a colonizer-mix enemy by design (the reveal shot shows a DEFEAT
  vs colonize-100), which is expected and not a bug — it's a demo seat, not a counter-picker.
- This crate intentionally does **not** touch any other crate's logic, and leaves the `architect`
  crate alone. It only adds `crates/layer2-game` to the workspace `members` + `default-members`.
