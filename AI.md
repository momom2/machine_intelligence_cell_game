# AI — the AI layer for v1 (`crates/ai`)

> **Phase 2** deliverable. A headless, deterministic AI layer built on the Layer-2 lens
> (`crates/world`, see `WORLD.md`) and the Layer-1 spatial sim (`crates/layer1`, see
> `LAYER1_SIM.md`). It contains four things: the **layer-agnostic greedy tactical policy**
> (implemented once over an abstract view, adapted to both layers), the **pure strategic
> policies** (colonize / defend / attack + mixes + passive), the **AI controller + roster** the
> GUI/levels call, and a **headless AI-vs-AI harness** with the validation tests. No graphics.

The crate is wired into the workspace `members` **and** `default-members` in the root
`Cargo.toml`. It carries **zero external dependencies** — all randomness lives inside each
planet's `layer1` `Structure` behind its seeded PRNG, so every run is bit-reproducible.

```
crates/ai/src/
  greedy.rs      The abstract PositionView + GreedyParams + decide_greedy (the policy, ONCE).
  adapters.rs    Layer1View (subs -> MoveOrder) and Layer2View (planets -> FleetOrder).
  graph.rs       Dijkstra path-length + first-hop over the lane graph (shared by L2 adapter/strategies).
  strategy.rs    StrategicPolicy (Passive/GreedyLocal/Colonize/Defend/Attack/+mixes) + TacticalPolicy.
  controller.rs  AiController (decide -> {FleetOrders, per-planet MoveOrders}; apply) + Roster.
  harness.rs     Symmetric test worlds + the AI-vs-AI match runner.
  tests.rs       The headless validation suite (lib tests — see the Windows note below).
  main.rs        `ai-harness` binary: prints the seam demo + the pure-strategy cycle matrix.
```

---

## 1. The layer-agnostic GREEDY policy (`greedy.rs`)

The project owner's greedy rule is implemented **once** against an abstract position view, then
adapted to both layers. The abstract view is tiny:

```rust
struct PositionInfo { id, owner: Me|Enemy|Neutral, my_ships: u32, enemy_ships: u32, contested: bool }
trait PositionView {
    fn len(&self) -> usize;
    fn info(&self, id) -> PositionInfo;
    fn distance(&self, from, to) -> Option<f32>;   // None = unreachable; only ORDERING is used
    fn can_export_from(&self, from) -> bool;        // Layer-2 export precondition (default true)
}
```

### The decision (exactly as specified), per owned position with surplus, in ascending id order

Surplus = `my_ships − garrison_floor` (only ships **strictly above** the floor move). For each
owned position with surplus > 0 (and `can_export_from` true):

1. **Retreat from a losing fight.** If the position is **contested AND outnumbered**
   (`enemy_ships > my_ships`) → send the surplus to the **nearest safe owned** position (owned
   by me, not contested). *Kind = `Retreat`.*
2. **Expand to the nearest uncontested position.** Otherwise → send the surplus to the
   **nearest uncontested** position, where *uncontested* = **NOT enemy-owned AND no enemy ships
   present**. *Kind = `Expand`.*
3. **Concentrate to break through.** If **no uncontested position exists anywhere** → send the
   surplus to the **least-defended contested** position (the reachable contested position with
   the fewest enemy ships). *Kind = `Concentrate`.*

Each owned-with-surplus position emits **at most one** action per decision, so the policy
commits gradually and the output is order-stable.

### The documented constants & tie-breaks (`GreedyParams`)

| Constant | Value | Meaning |
|---|---|---|
| `garrison_floor` | **2** | Per-position home guard kept on every owned position; ships above it are surplus. Matches `WorldParams::keep_floor = 2`, so at Layer 2 the floor the policy *reasons with* equals the floor the launch primitive *enforces* (the policy never plans a move the `FleetOrder` would refuse to release). |
| `prefer_neutral_expand` | **true** | Rule-2 tie-break: a capturable **neutral** is preferred over reinforcing a friendly position. The nearest neutral wins; a friendly destination is the fallback only when no neutral is reachable. |

**Other documented tie-breaks (all deterministic):**

- *"Nearest" ties* break to the **lowest destination id** (strictly-less comparison keeps the
  first seen).
- *Least-defended-contested* ranks by `(enemy_ships, distance)` lexicographically, then lowest
  id.
- **The friendly-expand refinement (important).** A *friendly* uncontested position is a valid
  rule-2 target **only if it is strictly thinner than the source** (`info.my_ships < source.my_ships`);
  equal-strength friendly positions are **not** targets. This is faithful to "send surplus to
  the nearest uncontested position" while eliminating the degenerate case where a fully-owned
  cluster of equal positions ships ships back and forth between itself forever (which would keep
  a planet's ships perpetually in transit and **starve Layer-2 export**). The global rule-2 vs
  rule-3 gate (`any_uncontested`) reads "uncontested" in this same *expansion* sense — a
  capturable neutral, or a friendly strictly thinner than some other owned position — so it does
  not count a side's own already-secure home as "an uncontested position" (which would make
  rule 3 never fire).

### THE DIAGNOSABLE SEAM (single, documented, exploitable)

> **Greedy always sends its surplus toward a *fight* (the nearest uncontested grab, or — when
> everything is contested — the nearest/weakest contested position) and it *never posts a
> dedicated rear guard above the flat garrison floor*.**

A position that is uncontested *now* but *exposed* keeps only `garrison_floor` ships, because
its surplus has already shipped forward. The exploit (same spirit as Layer-1's
`ai_seam_thin_rear_is_exploitable`): **hold a detachment back and strike a thinly-held rear/home
while greedy commits forward; the captured rear keeps producing and the flank snowballs.** This
is validated headlessly (see §4).

### The two adapters (`adapters.rs`)

**Layer-1 adapter (`Layer1View`)** — positions = a planet's **sub-structures**:
- `owner` relative to the seat; `my_ships` = idle ships of the seat garrisoned at the sub (only
  idle ships are movable, matching `Structure::issue_order`); `enemy_ships` / `contested` =
  enemy ships within `radius + engagement_radius` of the sub (the Automaton's contesting notion).
- `distance` = **Euclidean** over `Vec2`; every sub reaches every other (one structure).
- emits `layer1::MoveOrder`s; the abstract `count` is folded into the **smallest fraction bucket
  that covers it** (`bucket_for`). This is also the **player's optional "basic automation"** for
  a planet (`greedy_layer1_orders`).

**Layer-2 adapter (`Layer2View`)** — positions = the World's **planets**:
- reads `PlanetAggregate`: `owner` relative to seat (`Contested` maps to `Neutral`-for-ownership
  but is flagged `contested`); `my_ships`/`enemy_ships` = garrisoned **plus arriving**
  (`ships_of`).
- `distance` = **shortest-path lane length** (Dijkstra over lane lengths, `graph::path_len`).
- `can_export_from` = **`PlanetAggregate::fully_owned_uncontested(seat)`** — the world spec's
  rule that only a securely-held planet may export surplus.
- emits `world::FleetOrder`s. Because a `FleetOrder` is valid only between **lane-adjacent**
  planets, a multi-lane greedy action is routed to the **first hop** of the shortest path toward
  the chosen destination (`graph::next_hop`). The bucket is sized against the planet's
  *exportable* idle surplus (idle ships on owned subs above `keep_floor`) so the chosen fraction
  actually releases ≈ the intended surplus.

---

## 2. The pure strategic policies (`strategy.rs`, over `PlanetAggregate`)

Each is a stateless, legible hand-written policy. All obey the two world rules (export only from
`fully_owned_uncontested` planets; route to the first lane hop). Commit bucket = `ThreeQuarter`;
reinforce/expand bucket for the cautious defender = `Half`.

| Policy | Identity | Behaviour | **Blind spot** |
|---|---|---|---|
| **Colonize** | fastest expansion | every exportable planet ships surplus toward its **nearest neutral** planet; barely defends | **undefended production** — a timed strike takes a fat, thinly-held planet (loses to Attack) |
| **Defend** | turtle; concentrate on own ground | reinforce the **most threatened owned** planet (contested-and-losing first, else a thin frontier) from the nearest secure rear; expand only onto an **immediately adjacent** neutral when nothing of ours is threatened | **opportunity cost** — sits on a static base while an out-expander out-produces it (loses to Colonize) |
| **Attack** | mass, then strike the soft valuable target | pick the enemy planet minimizing `enemy_ships − 1.5·enemy_subs` (weak defence, high production), **stage** surplus at the owned planet nearest it, then commit the spearhead along the lane toward the target | **over-extension** — strips its own planets to feed the assault (loses to a Defender that survives and counter-punches) |
| **ColonizeThenAttack** (mix) | grab ground, then cash it in | plays Colonize until tick ≥ **280** *or* it owns a planet majority, then flips to Attack | weak at the **transition** (struck too early / out-expanded before the flip) |
| **Balanced** (mix) | hedged generalist | runs GreedyLocal; when no uncontested expansion remains and an enemy exists, presses Attack on the weakest enemy | master of none — a committed pure line out-focuses it |
| **GreedyLocal** | balanced baseline | the Layer-2 greedy export rule (§1) over the planet graph | the **greedy seam** (no rear guard) |
| **Passive** | inert dummy | issues nothing (Level 1's enemy seat) | — |

The **tactical** policy (`TacticalPolicy`) governs each planet's *internal* sub-structure play:
`Greedy` (the default — the Layer-1 greedy adapter, auto-defend/expand) or `None` (leave
internals idle; used by the passive dummy and to isolate the strategic layer in tests).

---

## 3. The controller API the GUI / levels call (`controller.rs`)

```rust
let ctrl = AiController::from_roster(Faction::Enemy, Roster::Attack); // bundles {strategic, tactical}
// or: AiController::new(seat, StrategicPolicy::Attack, TacticalPolicy::Greedy)

// Pure read — does not mutate the world:
let decision: AiDecision = ctrl.decide(&world, &sim_params, &world_params);
//   decision.fleet_orders:  Vec<FleetOrder>                 (the strategic layer)
//   decision.planet_orders: Vec<(PlanetId, Vec<MoveOrder>)> (each owned planet's internal play)

// Apply (planet internals first, then fleets — the documented order):
let (moved, launched) = ctrl.apply(&mut world, &decision, &world_params);
// or in one call:
let (moved, launched) = ctrl.decide_and_apply(&mut world, &sim_params, &world_params);
```

Every owned planet's internal play **defaults to the Layer-1 greedy adapter** (auto-defend /
expand) via `TacticalPolicy::Greedy`. The controller is stateless across ticks (a pure function
of the observed world), so it is deterministic and either seat can run any policy.

**The roster** (`Roster`) is the clean menu the GUI / levels pick from. Each entry bundles a
`{StrategicPolicy, TacticalPolicy}` pair and carries a human-readable `name()` and
`description()` (identity + blind spot). `Roster::ALL` lists every entry in display order:
`Passive, GreedyLocal, Colonize, Defend, Attack, ColonizeThenAttack, Balanced`.

The **harness** (`harness.rs`) provides `run_match` / `run_roster` / `duel_both_seatings` plus
two symmetric test-world builders (`corridor_world`, `diamond_world`). Both seats decide on the
same pre-step snapshot every `DEFAULT_DECISION_INTERVAL` (8) ticks; **Player applies first**
(the documented tie-break, matching `layer1::run_auto_vs_auto`). Default horizon 1200 ticks.

---

## 4. Measured headless validation (the real test)

All numbers below are **observed**, deterministic (no RNG outside each planet's seeded sim), and
asserted by the test suite in `crates/ai/src/tests.rs`.

### The greedy seam IS exploitable — confirmed

On a "bait" world (a Player home one short lane from greedy's **single-sub rear**, with a neutral
bait corridor dangling off the rear), a scripted rear strike beats the pure-greedy (Enemy) seat
in **7/7 seeds**. Trace (seed 1): greedy bleeds its rear toward the bait, and by **tick ~32 the
rear is captured** (`E-rear` flips to Player, its 1 sub lost) while greedy is committed forward;
the Player wins the match (168 vs 91 ships, 4 vs 3 subs at the horizon). Greedy never posted a
reserve above the flat floor — exactly the documented seam.

### The three pure strategies are distinct — confirmed

Their opening **fleet-order** sets differ pairwise on the diamond (colonize expands to flanks,
attack commits toward the enemy, defend holds), asserted in `pure_strategies_are_distinct`.

### The validated cycle (attack > colonize > defend > attack) — MEASURED

Fair symmetric worlds, **both seatings**, default horizon. Win-loss is over **5 seeds × 2
seatings = 10 games per edge** (seeds `1, 7, 42, 2024, 31337`).

**On the `diamond` world — the cycle CLOSES, cleanly and on every game:**

| edge | result (wins-losses) | holds? |
|---|---|---|
| **attack > colonize** | **10-0** | ✅ |
| **colonize > defend** | **10-0** | ✅ |
| **defend > attack** | **10-0** | ✅ |

This directionally reproduces the validated triad cycle from `01-mechanics.md` /
`CAPSTONE_RESULTS.md` (the operating point `r=0.6, k=2.25, l=0.15`): undefended production falls
to a strike, a colonizer out-produces a turtle, and a defender punishes the over-committed
aggressor. The diamond (two private flank neutrals + one shared contested centre) is the world
that gives each strategy a clean expression of its identity, so it is the **showcase map** for
the campaign levels.

**On the `corridor` world (linear `P—n1—n2—n3—E`) — the cycle does NOT fully close (honest
negative result):**

| edge | result (wins-losses-draws) | holds? |
|---|---|---|
| attack > colonize | **1-9-0** | ❌ (colonize dominates) |
| colonize > defend | 10-0-0 | ✅ |
| defend > attack | **3-6-1** | ❌ (attack wins more) |

On a 1-D corridor, grabbing the centre (`n1`/`n2`) is decisive, so the **colonizer out-positions
both the attacker and the defender** — the `colonize > defend` edge holds strongly but the other
two invert. This is reported, not hidden: per `01-mechanics.md` the cycle is "a property of
three numbers, not three words," and whether all three edges close is a **measurement** that
depends on the map. The corridor is still useful for the campaign — it cleanly showcases
colonize's strength and defend's opportunity-cost blind spot — even though one edge is weak
there. The diamond is the map to use when the full rock-paper-scissors is required.

### Determinism — confirmed

Two identical runs (same world seed, same policies) produce **identical per-tick `state_hash`**
and the same final outcome (`determinism_same_seed_same_hashes`), and the `run_match` path
replays identically (`determinism_run_match_is_stable`). The AI layer adds no nondeterminism.

---

## How to build & test

`cargo build` does not need the workaround; **running** freshly-linked test binaries on this
machine does (see the Windows note).

```sh
cargo build -p ai           # the library + the ai-harness bin
cargo build --workspace     # everything (incl. the set-aside `architect`)

# Run the tests (see the Windows note about CARGO_TARGET_DIR):
cargo test -p ai            # the AI validation suite (30 tests)
cargo test                  # the whole default-members suite
```

### Verified results (this machine)

Using `CARGO_TARGET_DIR` outside the Desktop tree (see below), `--release`:

- **`cargo test -p ai`** → **30 passed; 0 failed** (greedy unit tests, both adapters, the
  strategic policies, the controller, the seam exploit, distinctness, the diamond cycle, the
  corridor report, and the two determinism tests).
- Existing default members still green: **`layer1`** 14 lib + 16 integration; **`world`** 14
  integration; **`cell-core`** 11 lib (+1 ignored) + 18 integration; **`automaton`** 18 (+4
  ignored). `cargo build --workspace` succeeds with no warnings.

The `ai-harness` binary (`cargo run -p ai --bin ai-harness --release`) prints the seam demo and
the full pure-strategy win-rate matrix for both worlds; it is for human inspection — the
asserted validation lives in the lib tests.

### Windows note (Smart App Control, `os error 4551`)

As elsewhere in this workspace, freshly-linked **test/bin** binaries under the `Desktop/` tree
can be blocked from *running* by a Windows app-control policy
(`Une stratégie de contrôle d'application a bloqué ce fichier.`). The code is unaffected — it
compiles fine. Build/run the tests with the target dir outside that tree, e.g. PowerShell:

```powershell
$env:CARGO_TARGET_DIR="$env:USERPROFILE\.cargo-mi-target"  # or any path under %TEMP%
cargo test -p ai --release
```

On this machine the block is *reputation-based and per-binary*: a freshly-linked binary may be
blocked on its first invocation and then run on a retry of the **same unchanged** binary. The
validation tests are therefore kept as **lib tests** (which proved reliably runnable) rather than
a separate `tests/` integration target. If a run is blocked, simply re-issue the same
`cargo test` command (no rebuild) and it typically runs.
