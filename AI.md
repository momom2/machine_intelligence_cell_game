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
3. **Amass and assault.** If **no uncontested expand target exists anywhere** (nothing left to
   colonize) → greedy must still apply force rather than idle (it must beat a passive enemy that
   never moves). It **amasses** its production and breaks the enemy. *Kind = `Assault`.*
   - **Where to strike** comes from a **production-superiority proxy**: count the positions each
     side owns (every position is a producer). `superior = my_positions > enemy_positions`. If
     superior → break the enemy where it is **strongest** (`MAX enemy_ships`) — superior
     production wins attrition against the thickest stack. Else → hit it where it is **weakest**
     (`MIN enemy_ships`). The target is any reachable position with enemy presence (enemy-owned,
     or contested with enemy ships). Ties → lowest id.
   - **How to commit** is **concentration of force** through **one staging position** (the owned
     position nearest the target). The staging position **holds** (keeps amassing) until it has
     **local superiority** (`staging.my_ships >= target.enemy_ships`), then commits its surplus
     at the target; every **other** owned position ships its surplus **toward the staging
     position** — so force massess before it strikes rather than feeding piecemeal into a strong
     stack (square-law death). *Fallback so nothing freezes:* if the staging position cannot
     reach the target, or a feeder cannot reach the staging position, that position sends its
     surplus **directly at the target** instead.

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
> there is nothing to colonize — forward to the assault's staging position) and it *never posts a
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
the cautious defender **reinforces** a threatened planet with `Half` and, in its productive
(quiet-board) branch, commits only a `Quarter` so it always keeps a large home reserve.

| Policy | Identity | Behaviour | **Blind spot** |
|---|---|---|---|
| **Colonize** | fastest expansion | every exportable planet ships surplus toward its **nearest neutral** planet; barely defends | **undefended production** — a timed strike takes a fat, thinly-held planet (loses to Attack) |
| **Defend** | turtle; defense-first, productive when quiet | **(1)** reinforce the **most threatened owned** planet (contested-and-losing first, else a thin **frontier**) from the nearest secure rear (Half, one per tick) — concentrating force on its own ground is what beats Attack; **(2)** only when **nothing is contested and nothing is even on the frontier** (a safe interior — the old "idle" case) does it expand a **portion** (Quarter) toward the nearest neutral, or, if no neutral remains, press the enemy with that same small commitment. It snaps back to defending the instant a planet is contested/frontier | **opportunity cost** — its small-portion expansion still grows slower than a pure colonizer, so an out-expander out-produces it (loses to Colonize) |
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

### Colonizers beat a passive enemy — confirmed

`GreedyLocal`, `Colonize`, **and** `SimpleColonize` each beat `Passive` **2-0** over both seatings
on both the diamond and the corridor (`diag`-measured; the standing assertion lives in
`greedy_is_sensible_expands_and_beats_passive`). Note `SimpleColonize` only beats Passive **after
re-tuning `ships_per_res` from `0.12 → 0.02`**: at the old value (sized for the retired
`max_resistance≈100`) its wave goal was `0.12 × 1800 ≈ 216` ships, so it never sent and *drew* with
the do-nothing dummy. In the game the colonizer-beats-passive case is **Level 1 "First Moves"**
(the greedy player captures the neutral apex then assaults the dormant enemy).

### The greedy seam IS exploitable — confirmed (re-expressed for the grind)

On a "bait" world (a Player home one short lane from greedy's **single-sub rear**, with a neutral
bait corridor dangling off it), greedy bleeds its rear down the corridor and posts no rear guard —
the documented seam. **Re-expressed for the new resistance/denial model** (mirroring `layer1`'s
`ai_seam_thin_rear_is_exploitable`): capture is no longer instant, so a rear strike no longer
"snipes" the rear in a few ticks. The seam now shows up as **sustained denial** — the flank reaches
the undefended rear and *sits there uncontested* (starving its production via Mechanic B and
grinding its resistance), captured given enough time. We assert, in a majority of seeds, that the
flank either **captures** the rear OR holds a **sustained uncontested-presence streak** on it (≥ 20
decision windows of Player-present / Enemy-absent). Holds **7/7 seeds**
(`greedy_seam_thin_rear_is_exploitable`); the campaign's L7 "The Seam" re-confirms it 5/5.

### The three pure strategies are distinct — confirmed

Their opening **fleet-order** sets differ pairwise on the diamond (colonize expands to flanks,
attack commits toward the enemy, defend holds), asserted in `pure_strategies_are_distinct`.

### The validated cycle (attack > colonize > defend > attack) — MEASURED

> **Under the resistance / denial / soft-cap overhaul** (see `AUTOMATA_DESIGN.md`). The three pure
> strategies are now the four **composable automatons** (`ai::automata`, built over `ai::vocab`
> predicates + actions + the event-driven `world::Projection` queries), run at Layer 2 via the
> controller's one-projection-per-tick path. Capture is a **grind** (fresh `max_resistance = 1800`),
> so matches resolve over **thousands** of ticks: the harness `DEFAULT_HORIZON` is now **3000** and
> the AI look-ahead `DEFAULT_PROJECTION_HORIZON` is **2000** (a forecast must span a grind, else the
> marginal-capture queries read 0 and the colonizers never commit). One mechanic dial moved —
> `softcap_attrition 1.0 → 0.5` — plus policy dials (`AttackParams::fight_efficiency = 2`,
> `SimpleColonizerParams::ships_per_res = 0.02`, a parity-gated Defend counter-punch). Details +
> rationale in `AUTOMATA_DESIGN.md` §4/§6.

Fair symmetric worlds, **both seatings**, `DEFAULT_HORIZON = 3000`. Win-loss is over **5 seeds × 2
seatings = 10 games per edge** (seeds `1, 7, 42, 2024, 31337`); also reported over **8 seeds × 2 =
16 games** (`+100, 0x5EA1, 9001`).

**On the `diamond` world — the cycle CLOSES on both seatings + every seed:**

| edge | 5 seeds (wins-losses) | 8 seeds (wins-losses) | holds? |
|---|---|---|---|
| **attack > colonize** | **10-0** | **16-0** | ✅ |
| **colonize > defend** | **10-0** | **16-0** | ✅ |
| **defend > attack** | **6-4** | **10-6** | ✅ |

`defend > attack` is the closest edge (as the analysis predicts it is the most fragile under a long
grind) but closes robustly on both sweeps; the other two are clean shut-outs. This directionally
reproduces the validated triad from `01-mechanics.md` / `CAPSTONE_RESULTS.md`: undefended production
falls to a *committed* strike, a colonizer out-produces a turtle, and a defender that holds a real
wall (the gentler `softcap_attrition`) punishes the over-committed aggressor and counter-punches its
emptied rear. The diamond (two private flank neutrals + one shared contested centre) is the showcase
map for the campaign levels — L8/L9/L10 re-confirm each edge on the level map (10-0 / 10-0 / 6-4).

The diamond cycle had to be **re-closed** for the grind: before tuning it read `attack>colonize
10-0, colonize>defend 0-10, defend>attack 1-9` (a near-total-order — captures were so slow the
turtle held everything and the colonizers, gated on a 240-tick marginal query that read 0, never
expanded). Raising the horizons woke the colonizers (`colonize>defend` flipped to 10-0); committing
Attack (`fight_efficiency 10→2`) fixed `attack>colonize`; the soft-cap dial + Defend counter-punch
tipped `defend>attack` from a tie to 6-4 (10-6 over 8 seeds).

**On the `corridor` world (linear `P—n1—n2—n3—E`) — the cycle still does NOT fully close (an
honest, map-dependent negative result).** On a 1-D corridor, grabbing the centre (`n1`/`n2`) is
decisive, so the **colonizer out-positions both the attacker and the defender** — `colonize >
defend` holds strongly but the other two edges do not. This is reported, not hidden (printed by the
non-asserting `pure_strategy_cycle_corridor_report`): per `01-mechanics.md` the cycle is "a property
of three numbers, not three words," and whether all three edges close is a **measurement** that
depends on the map. The corridor still cleanly showcases colonize's strength and defend's
opportunity-cost blind spot; the **diamond** is the map for the full rock-paper-scissors.

### AI-in-loop profile — the live loop is not too slow

A full headless `diamond` match with **both seats projection-driven** (`Attack` vs `Defend` — the
heaviest pair, each building the shared `world::Projection` at `DEFAULT_PROJECTION_HORIZON = 2000`
once per decision tick), at `DEFAULT_HORIZON = 3000`, decision interval 8 (release build):

| metric | value |
|---|---|
| wall-time / match | **~316 ms** (3000 ticks) |
| **ticks / sec** | **~9,500** |
| decision-ticks / match | 375 |
| **projection calls / match** | **750** (2 seats × 375) |
| time / decision-tick (both seats decide+apply + 8 world steps) | **~844 µs** |

At ~9.5k ticks/sec headless, the projection-every-decision overhead is negligible for interactive
play (a real game advances tens of ticks/sec). The event-driven projection costs ~150–200 µs/call
at horizon 2000 (`world` `proj-bench`), well under the ~1 ms/decision budget even with both seats
projecting. The live loop is comfortably fast.

### Determinism — confirmed

Two identical runs (same world seed, same policies) produce **identical per-tick `state_hash`**
and the same final outcome (`determinism_same_seed_same_hashes`), and the `run_match` path
replays identically (`determinism_run_match_is_stable`). The forward projection draws **no** RNG
and the four automatons add none, so calling `project_forward` every decision tick does not perturb
the hash; the AI layer adds no nondeterminism.

---

## How to build & test

`cargo build` does not need the workaround; **running** freshly-linked test binaries on this
machine does (see the Windows note).

```sh
cargo build -p ai           # the library + the ai-harness bin
cargo build --workspace     # everything (incl. the set-aside `architect`)

# Run the tests (see the Windows note about CARGO_TARGET_DIR):
cargo test -p ai            # the AI validation suite (34 tests)
cargo test                  # the whole default-members suite
```

### Verified results (this machine)

Using `CARGO_TARGET_DIR` outside the Desktop tree (see below), `--release`:

- **`cargo test -p ai`** → **34 passed; 0 failed** (greedy unit tests incl. the four assault
  cases, both adapters, the strategic policies incl. the contested-reinforce and
  productive-colonize defend cases, the controller, the seam exploit, distinctness, the diamond
  cycle, the corridor report, and the two determinism tests).
- Existing default members still green: **`layer1`** 14 lib + 16 integration; **`world`** 14
  integration; **`cell-core`** 11 lib (+1 ignored) + 18 integration; **`automaton`** 18 (+4
  ignored); **`levels`** 3 lib (incl. the lesson-holds validation). `cargo build --workspace`
  succeeds with no warnings.
- The headless game self-test (`cargo run -p game --release -- --selftest`) is **ALL PASS**, with
  **`L 1 First Moves … winner=Some(Player)`** (the headline fix) and the AUTOMATION check passing.
- Out of scope: the work-in-progress **`architect`** crate (no dependency on `ai`) has one
  pre-existing failing rendering test (`glass::render_is_priority_ordered_and_legible`),
  unaffected by — and unrelated to — the AI changes here.

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
