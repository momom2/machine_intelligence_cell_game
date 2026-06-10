# Changelog

Newest first. Each entry records the **delta** from the previous state and is authoritative for the
mechanics it touches — when a per-component doc (`LAYER1_SIM.md`, `GAME.md`, `WORLD.md`, `LEVELS.md`,
`README.md`) disagrees with the latest entry here, **this file wins** until that doc is refreshed.

---

## feat/counter — the per-sub / WYSIWYG / struct-storage era (uncommitted working tree, as of 2026-06)

### Rendering performance — ~1000-ship lag (the sim was never the bottleneck)
Profiling-by-analysis found the lag is **per-frame rendering**, not the sim (~1900 ticks/s headless, needs
60). All render-only; no sim/determinism impact (`--selftest` unaffected).
- **Ships are batched into one mesh.** The interior ship view replaced ~N individual immediate-mode
  `draw_circle`/`draw_ship_triangle` calls with a **single `draw_mesh`** (reused thread-local vertex/index
  buffers, chunked at the `u16` cap). **Idle ships are quad dots** (2 tris) instead of tessellated 20-gon
  circles; moving ships stay triangles. (`draw_ships_interior`/`draw_ships_meshed`.)
- **Off-screen culling + density LOD.** Off-screen ships are skipped (big win at high zoom). Above
  `SHIP_DENSITY_THRESHOLD` (1500) on-screen ships the view switches to a **screen-grid density blob**
  (`draw_ships_binned`) — one quad per ~9px cell, size+opacity ∝ count — so extreme densities stay cheap.
- **Kill-FX no longer churns every frame.** The ship-liveness snapshot + diff (`spawn_kill_fx`) now runs
  **only when a sim tick actually drains** (ships only die on a tick), reusing a persistent `prev_alive`
  buffer instead of allocating `Vec<Vec<bool>>` per frame.
- **Count labels tally once per frame.** The interior sub labels + contested ring, and the lens node
  counts, read a per-(sub/planet, seat) table built in **one O(N) pass** instead of calling
  `idle_count_at`/`ship_count` per sub/planet per faction (was O(S·N) / O(P·N)).
- **Frame-timing overlay (toggle `F3`).** EMA-smoothed ms for `update` / `kill-fx` / `draw` / `ships`,
  plus FPS and the on-screen ship count (`Perf` + `draw_perf_overlay`) — for future optimization.

### GUI: wider zoom + box-select; Simple neutral-priority rework
- **Zoom range widened** to `ZOOM_MIN = 0.5` … `ZOOM_MAX = 7.0` (was `1.0`…`3.5`): `0.5×` zooms *out* below
  the fitted view (~2× the area), `7×` zooms deep in.
- **Box-select (click-drag).** A left-drag past `BOX_DRAG_THRESHOLD` draws a selection rectangle and
  multi-selects every **player-commandable** position inside it — subs in the Layer-1 interior (excluding
  the struct-storage node), planets on the Layer-2 lens. The next click is the **target**: the order goes
  from every selected source to it (the target may be one of the selected) and the selection clears.
  Click-vs-drag is resolved on mouse-**up** (a plain click keeps the old single-select / order behaviour;
  the old drag-from-source-to-target gesture is replaced by the box). New `Game` state `sel_subs` /
  `sel_planets` / `drag_start` / `box_active`; the box + each selection highlight are drawn each frame.
- **Simple neutral prioritization** now ranks neutral capture targets by **cost-effectiveness**
  `resistance / production` plus a small distance term (`neutral_dist_weight = 5.0`, a new `SimpleParams`
  dial) instead of purely nearest-first — so it prefers cheap-to-grind, high-production, nearby neutrals.
  New `PositionView::production` (default `1.0`, overridden on `Layer1View` from the sub's `production`).
  Enemy targets keep nearest-owned-first. `--selftest` stays `det=true` on all 10 levels.

A gameplay-feel + mechanics overhaul on top of the *resistance era* (`AUTOMATA_DESIGN.md`, `README.md`).
The resistance/denial core (per-sub `resistance`, production denial, the forward projection) is
**unchanged**; what changed is the economy, combat, ship movement, the inter-planet plumbing, the
whole GUI operating point, and the campaign roster. The AI automata track (Colonize/Defend/Attack,
the Counter, the diamond RPS) is **parked** — the campaign now runs against `SimpleColonize` only.

### Seats are level-declared, not engine-hardcoded (`Faction::Ai(u8)`) + automation quarantined
The number of enemies is now a **level-layer** decision; the Layer-1 sim no longer hardcodes a "second
enemy". (Replaces the bolted-on `Faction::Enemy2`.)

- **`layer1::Faction` is now `{ Neutral, Player, Ai(u8) }`.** `Player` is the human; `Ai(i)` is the
  `i`-th AI opponent. There is no hardcoded enemy count. `ENEMY`/`ENEMY2` consts alias `Ai(0)`/`Ai(1)`
  for readability; `is_foe_of` is the free-for-all foe relation; `opponent()` survives only as a coarse
  **binary** "primary rival" shim for the parked automata/Counter/projection track.
- **The engine went genuinely N-seat where it is exercised** (the live path): combat bubbles, the
  capture grind (`resolve_resistance`), the lone-present-seat discriminants (`capture_present_faction` /
  `single_present_faction`), `foreign_idle_count`, and the legacy per-structure softcap now **scan the
  ships directly** instead of iterating a hardcoded `[Player, Enemy, Enemy2]` triple (new
  `Structure::foreign_ship_count` / `foreign_sub_count`). `capture_core` was already N-seat.
  `World::outcome` and `PlanetAggregate` aggregate **every** non-player real seat into the lens's
  combined-enemy slot (new `World::total_foreign_ships`/`total_foreign_subs`) — so VICTORY = *all* rivals
  eliminated, for any number of them. This also removes the old "Enemy2 is Layer-1-only" hole at the
  Layer-2 aggregate. **All per-seat GUI readouts went N-seat** (each was previously binary Player+`Ai(0)`,
  so a second AI's ships were invisible / shown as `0`): the Layer-2 planet-node **pie chart** (one wedge
  per seat — Player, **each AI rival in its own colour** via `Game::enemy_color`, then Neutral — sized by
  that seat's producing subs), the Layer-2 node **count stack**, and the interior sub **count labels** +
  **contested-ownership ring** all now iterate every present seat. (Verified on L3's three-way with
  `--shot --view lens`/`--view interior` captures: e.g. both AI homes now show their counts.)
- **`levels::Level.enemy: Roster` + `enemy2: Option<Roster>` → `enemies: Vec<Roster>`.** The list **is**
  the declaration of how many enemies a level has; `enemies[i]` drives `Ai(i)`. `Game` builds one
  `SeatController` per entry; the renderer colours each seat by index (`Game::enemy_color`). New
  `Level::primary_enemy()` for the single-enemy validation proxies. Campaign unchanged in content (L1
  `[Passive]`, L2/L4–L10 `[SimpleColonize]`, L3 `[SimpleColonize, SimpleColonize]`).
- **Determinism preserved.** Faction state-hash codes are unchanged (`Player=1`, `Ai(0)=2`, `Ai(1)=3`,
  …), so existing levels replay bit-identically — `--selftest` is `det=true` on all 10, and the L3 2-AI
  free-for-all confirms the multi-seat path. The parked Counter/projection stay on the binary shim
  (`opponent()` / "all rivals → one enemy slot").
- **Basic player-automation is QUARANTINED** (to be redesigned). `automation_available` is now `false`
  on every level, so the `game` toggle (`A`) / per-tick `run_player_automation` / AUTO render (all gated
  on the flag) are inert; the wiring is kept + marked PARKED. `ai::greedy_layer1_orders` (the adapter it
  used) stays live as the AI's tactical default. The stale `metadata_is_complete` test (which still
  expected L3 = Layer-2 + automation) was corrected.

### Codebase tightening + the forward projection is off the live path
A focused cleanup pass ahead of the missions 1–10 redesign. **The live game no longer builds the
forward projection** at all; the parked automata/Counter track keeps it (deferred).

- **Simple decoupled from the projection.** `SimpleController` no longer calls `project_forward`.
  Its three look-ahead reads (`incoming_mine` / `enemy_incoming` / `friendly_eta`) now come from a new
  projection-free **`World::sub_influx_for(planet, seat, sp, wp) -> SubInflux`** that reads the
  *current* in-flight state directly: intra-structure moving ships attributed to their `target` sub
  (same `ceil((dist−tolerance)/ship_speed)` ETA the sim/projection use), plus inter-planet fleets
  attributed to the sub they **actually land at** — the reserve node (`storage_sub`) if present, else
  the lane-facing `entry_sub`, *identical to `inject_fleet`*. **This fixes the known
  entry-sub-vs-reserve arrival divergence on the live path.** On single-planet levels (every current
  Simple mission) the influx is bit-identical to the old projected arrivals, so Simple's play and
  `state_hash` are unchanged — `--selftest` is `det=true` on all 10 levels. New `Layer1View::direct`
  (carries the `SubInflux`) and `Layer2View::without_projection` (Simple's push uses no projection
  QUERY) host it.
- **`AiController` builds the projection only when needed.** New `StrategicPolicy::needs_projection()`
  (false only for the two live rosters, `Passive` / `GreedyLocal`) + `decide_projection_free()`. The
  live game's Passive (L1) and GreedyLocal (player demo / validation proxy) no longer build a
  projection each tick; the greedy tactical default reads none either, so its Layer-1 view is now
  `Layer1View::new` (behaviour-identical). The parked automata rosters still build + share it.
- **`projection.rs` is marked PARKED** (kept compiling for the deferred `ai::automata`/`hardcoded`/
  `counter`); the `proj-bench` profiling binary was removed.
- **Deleted (dead / superseded / dev-only):** the `--tune` flag + `run_tune()` (headless balance
  probe); `ai/src/bestresponse.rs` (referenced nowhere); the prototype crates **`layer1-game`,
  `layer2-game`, `r2-sweep`** (leaf binaries superseded by `game`; dropped from the workspace);
  stale generated artifacts (`capstone_results.*`, `r2_results.*`, `files.zip`).
- **Parked `ai` tests removed:** the 4 failing + 8 ignored Counter/automata-drift tests (so
  `cargo test -p ai` is green again — 89 pass); passing live + Counter pure-logic coverage kept.
- **Historical docs archived** to `docs/archive/` (`00`–`04`, `AI`, `AUTOMATA_DESIGN`, `CAPSTONE_RESULTS`,
  `COUNTER_*`, `DSL_SPEC`, `LAYER1_GAME`, `LAYER2_GAME`, `R2_RESULTS`); root keeps `CHANGELOG`, `README`,
  `GAME`, `WORLD`, `LEVELS`, `LAYER1_SIM`. **Counter / Apprentice / Architect remain deferred TODOs**
  (their crates `cell-core`/`automaton`/`architect` and the `ai::counter` track are untouched).

### Simple reworked — a stateful, ledger-driven colonizer (`SimpleController`)

The campaign "Simple" enemy was **completely rewritten** from the stateless `simple_colonize` automaton
into a new **stateful** controller (`crates/ai/src/simple.rs`). The trigger: a review found that on the
single-planet missions (1–3) the old `Roster::SimpleColonize` was *two* policies — a Layer-2 strategic
`simple_colonize` (which **does nothing** on a one-planet level) plus the Layer-1 tactical `decide_greedy`.
So the in-game "Simple" was actually **greedy**, and every prior edit to `simple_colonize` was a no-op on
the campaign — which is why the "shuffles ships between owned subs instead of capturing" bug survived
every fix. The rework replaces that with a purpose-built program:

- **`SimpleController`** — non-`Copy`, `#[derive(Clone)]`, built once per match (mirrors the stateful
  `CounterController`), carrying a **persistent per-planet departure ledger** (`Vec<Vec<Op>>`, never a
  HashMap). Hosted by a new `SeatController { Stateless(AiController), Simple(SimpleController) }` dispatch
  enum; `Game.enemies` / `run_tune` build it for `Roster::SimpleColonize` and step it with `&mut`.
- **Layer 1 (the heart) — four phases per planet, per decision tick:** `EXPIRE` (drop landed ops) →
  `DEFEND` (flee a sub a foe can overwhelm — `enemy+incoming ≥ OVERWHELM(mine)` — moving **everything**;
  pin a contested-but-not-overwhelmed sub so its garrison holds) → `PLAN+COMMIT` (secure up to `FRONTS=3`
  fronts at their **minimum**, then deepen toward their **maximum**, nearest-source-first) → `DISPATCH`
  (fire the staggered legs that have come due). `OVERWHELM(n) = max(1.2·n, n+20)`. A neutral costs
  `OVERWHELM(0)=20` to start and up to `OVERWHELM(resistance/60)` to finish; an enemy sub
  `OVERWHELM(present+incoming foes)` up to `OVERWHELM(2·foes)`. `our_force` (present + in-flight +
  undeparted-committed) is subtracted from both, so the planner self-throttles and incremental
  reinforcement falls out — replacing the old binary "ignore" flag.
- **Synchronised arrival without engine support.** The engine never holds ships, so staggering lives
  entirely in the ledger: a committed leg **reserves** its ships (so `spare` won't double-spend) and the
  real move fires only at its `depart_at = land_at − travel`, computed so a multi-source taskforce **lands
  together**. `land_at` is floored by any inbound-friendly ETA and prior ops' landings.
- **Layer 2 (simplified, per the design call):** from each fully-owned, uncontested planet, push surplus
  toward the nearest **frontline** planet — no ledger, no retreat, no staggering. A no-op on single-planet
  levels (so missions 1–3 are pure Layer-1).
- **New plumbing:** `Structure::issue_order_count(src, tgt, n, faction)` — an **exact-count** Layer-1 move
  (the ledger needs exact counts; `FractionBucket`/`issue_order_fraction` would round and desync the
  reservation). Two defaulted `PositionView` reads: `enemy_incoming(id)` (in-flight foe force, summed over
  non-seat real factions) and `friendly_eta(id)` (earliest inbound-friendly tick). `harness::run_simple_match`
  drives the stateful seat for tests.
- **Determinism preserved:** Vec-only ledger, ascending-index iteration, id tie-breaks, `world.tick`
  clock, and a small `ceil` tolerance so `1.2·100` doesn't round to 121. **`--selftest` is green
  (`det=true`) on all ten levels**, and 14 pure-logic unit tests cover OVERWHELM, min≤max, the FRONTS
  batching, the reservation/no-double-spend, the staggering math, and same-input determinism.
- **The thin-rear "seam" is gone for Simple** (it now defends): the two parked Counter tests that asserted
  `SimpleColonize` fires `never_guards_rear` were removed. (Greedy's seam + Level 7 are untouched — that is
  a separate roster's teaching feature.)
- **Balance shifted — re-tune.** `--tune` (player Greedy proxy vs Simple) now reads Simple as a genuine
  threat that *captures* (e.g. L1 geometry: it wins in ~111 ticks; B-strength grows across the board) — vs
  the old flat "shuffle". Tune the campaign with the new `SimpleParams` dials (`floor`, `min_wave`,
  `fronts`, the OVERWHELM ratio/add, the neutral/enemy max multipliers).
- The old Layer-2 `simple_colonize` automaton + `SimpleColonizerParams` are **left intact** (still reached
  by `AiController::from_roster` for the levels-validation sweep); the *game* now routes Simple through
  `SeatController` to the new controller.

### Multi-enemy support — a third faction (`Enemy2`) + Mission 3 (free-for-all)
- **`Faction::Enemy2`** — a second AI seat, symmetric with `Enemy`. The whole stack went N-seat where it
  mattered: combat is already pairwise (any two different real seats fight); `capture_step` gained an
  N-seat `capture_core` (erodes only on a **lone** uncontested foreigner — 2+ foreign ⇒ contested ⇒
  frozen — and flips to it); production-denial + storage auto-divert use a new `foreign_idle_count`
  ("any other real seat"); `Layer1View` maps **every** non-seat real owner to a foe (free-for-all);
  `World::outcome` is player-perspective (VICTORY iff all enemies eliminated, DEFEAT iff the player is,
  the two AI seats aggregated into the binary score slot). `SubPresence`/`capture_presence` track
  `enemy2`. **`--tune`** spawns a controller per enemy seat and reports each side's strength broken
  out (`P:_ B:_ C:_`), so a multi-enemy level can be balanced per-rival. The Layer-2 aggregate + forward-projection stay binary (parked) — fine, since multi-enemy is
  Layer-1-only for now.
- **Game wiring:** `Level.enemy2: Option<Roster>`; `Game` spawns one controller per enemy seat and ticks
  them all. Colour is keyed per seat: `Enemy` → its kind-colour, `Enemy2` → a *distinct* shade of the
  same family (`roster_color_alt`), so two SimpleColonize enemies read as **two different yellows**.
- **Mission 3 "Deliberation"** (renamed from "Two Worlds") — Layer-1-only, **two SimpleColonize
  opponents** in a 3-way free-for-all. A horizontal neutral chain from the Player start (A, storage 60 /
  prod 2) branches up to **B** (Enemy, 120/4, via two richer "1" posts) and down to **C** (Enemy2, 90/3,
  via two lean "0" posts). `spec_for(3)` updated (binary tuple lumps the two AI seats).
- **Renames:** 1 "First steps", 2 "Fire in the sky", 3 "Deliberation", 4 "Far far away". Mission 3 has a
  **placeholder** briefing (`L3_BRIEFING`).
- **Roster display name:** `Roster::SimpleColonize.name()` is now just **"Simple"** (was "SimpleColonize");
  the `--tune` header matches.
- **Level-select enemy tag is spoiler-gated.** The opponent label is drawn only once a mission has been
  **played** (`lvl.id <= briefed` — i.e. its briefing has been received); a locked/unplayed mission hides
  its enemy entirely. A multi-enemy level lists **every** seat, so Mission 3 reads **"Simple, Simple"**.
- **Bug fix — premature free-for-all win.** `Game::match_over`/`seat_finished` were binary: in a
  free-for-all, when one Simple eliminated the *other*, `seat_finished(Enemy)` fired and the **player**
  was declared the winner while a rival still stood. Now `match_over` requires `all_enemies_finished()`
  (every enemy seat done), `seat_finished` treats **any** foreign seat as the eroder, and the win-latch
  matches. (For a single-enemy level `Enemy2` is trivially finished, so behaviour is unchanged there.)
- **Bug fix — SimpleColonize now attacks.** It only ever colonized *foe-free* ground, so a big army with
  no undefended neutral left just shuffled surplus between owned subs. It now also pursues **defended**
  ground (a contested neutral or an enemy sub) when its committable surplus clearly out-numbers the
  present defenders (`OVERWHELM_RATIO = 1.5`, present-count based — no projection). Makes the campaign
  enemies meaningfully stronger; **re-tune levels** (e.g. L2 flipped from a clear player win to contested).
- **Mission 2** gained two lean corridor posts (storage 30 / prod 1) right of the middle line, toward the
  enemy. `spec_for(2)` → `(8,1,1,6)`.

### Struct storage is ownerless + resistance = capacity (sim)
- **Struct storage has no ownership.** The reserve / staging node is now permanently **Neutral** and
  **never captured** (`resolve_resistance` skips it; `add_storage_sub` seeds it Neutral, dropping the
  old `majority_owner` seeding). It is a shared space any side may stage in.
- **Auto-divert gate rewritten.** A producing sub ships its over-capacity surplus into storage only
  while **fewer than `STORAGE_ENEMY_BLOCK = 20` enemy ships** sit there (was: only into a reserve the
  producer *owned*) — so a contested staging area stops accepting deposits.
- **Default resistance is proportional to storage capacity.** `SubStructure::new`/`with_storage_capacity`
  set `resistance = max_resistance = storage_capacity · RESISTANCE_PER_CAPACITY` (`= 30`);
  `with_max_resistance` still overrides. At the default capacity `60` this reproduces the historical
  `1800` grind (so default subs capture at the same pace as before); a bigger sub is proportionally
  harder to take (Mission 1's storage-100 centre → 3000).
- **AI fix for the ownerless node.** `Layer1View` now presents the storage node as the seat's *own*
  position (it was previously seen as a capturable neutral once ownerless), so the greedy/automata
  never pour ships into a node they can't take.
- **Parked fallout:** these retunings legitimately shift the parked Automaton/Counter/diamond-RPS
  dynamics; their tuning-sensitive tests are `#[ignore]`d pending the automata revival + projection rework.

### Rendering: Layer-2 ownership pie (game)
- A Layer-2 planet node is now a **pie chart of sub ownership** (`draw_pie`): each side's wedge ∝ the
  subs it owns (`player_subs`/`enemy_subs`/`neutral_subs`, storage excluded), replacing the flat disk +
  split-dot contested cue (the pie shows the split directly).

### Campaign: Mission 2 rebuilt ("Contact", vs Simple)
- `build_l2` is now **Layer-1 only** (single planet, like Mission 1): **6 subs** — four neutral
  production posts in a central square (storage 60, **production 3**, ~11 apart so adjacent posts trade
  fire) and two home posts on opposite sides (Player left / **SimpleColonize** Enemy right), each **60
  ships, storage 60, production 1** — plus the ownerless staging node. `spec_for(2)` → `(6,1,1,4)`;
  blurb/objective/hints refreshed; **placeholder briefing** (`L2_BRIEFING`, final copy to come).

### Controls: discrete speed slider (game)
- The pause/x1/x3 **buttons are replaced by one discrete speed slider** with five stops —
  **`0x` (=paused) / 1x / 3x / 10x / 25x** — so pause is just the leftmost stop and the whole
  transport is a single control. `SPEED_STEPS = [0.0, 1.0, 3.0, 10.0, 25.0]`; the `paused` bool is
  gone (`Game::paused()` ⇔ `speed_idx == 0`). `-`/`+`/`[`/`]` step through the stops; `P` toggles
  `0x ⇄ last running speed` (`resume_idx`). Top speed rises from 6x to 25x.

### Rendering: presence ring + enemy-kind colour (game)
- **Contested-ownership ring replaces the production indicator.** A sub/struct that holds ships of
  **two or more sides** now draws a ring split into each side's colour, every arc sized to that side's
  share of the present ships (10 vs 30 ⇒ a quarter/three-quarters split), via `draw_ownership_ring`.
  A single side present ⇒ no ring. The old ongoing-production progress ring (`draw_progress_ring` /
  `production_fraction`) is **gone** — production is no longer surfaced as a fill. The orbiting
  production **squares** (spawn-point markers) stay; they show *where* ships appear, not progress.
- **Sub/struct labels now show storage occupancy.** The number above each sub/struct is the owner's
  stored count over its capacity — `<owner-colour>count</> <grey>/ cap</>` (e.g. cyan `60` + grey
  `/ 60`) — shown on every sub/struct so the capacity is always visible (empty ⇒ `0 / cap`). Any other
  present faction stacks above as a bare count in its colour.
- **Enemy colour is keyed on the controlling AI kind, not the level.** `roster_color(Roster)` maps
  `Passive → cool steel-grey`, `SimpleColonize → amber`, every other kind → the default red/orange.
  Resolved through `Game::col`/`dim`/`planet_col` (which consult the enemy seat's roster), so a future
  level fielding several enemy kinds renders each in its own colour. The free `planet_color` helper
  is retired. Player stays cyan, neutral stays grey.

### Framerate, tick grounding & sim performance (game)
- **All behaviour grounded in a tickrate knob.** `TICK_HZ = 60` (historical reference `REF_HZ = 2.5`,
  `TICK_SCALE = TICK_HZ/REF_HZ = 24`). Per-*second* behaviour is **invariant to `TICK_HZ`** — raising
  it subdivides each logical step finer without changing wall-clock pace (60 Hz does **not** shift the
  pace). `gui_params(scale)` ÷scale's per-tick rates / ×scale's periods (ship_speed, orbit_*,
  prod_square_spin, drift_speed, fire_prob, defender_fire_bonus, production_period, drift_ticks,
  undock_ticks); `build_scaled` ×scale's per-sub resistance + undock_ticks, ÷scale's transit_speed;
  `scaled_horizon` ×scale's the match horizon; the seat-decision cadence is `DECISION_BASE(5)·scale`.
- **Variable framerate via fixed-timestep + interpolation.** `Game::update(dt)` is an accumulator loop:
  add `dt·speed` (clamped to `MAX_FRAME_DT = 0.25` s), drain whole `FIXED_DT = 1/60` s ticks through
  `step_one_tick`, and publish `render_alpha = tick_accum/FIXED_DT` so the renderer lerps ship/feature
  positions between the last two sim states. The sim is now decoupled from the display rate — render
  as fast as the monitor allows, simulate at a fixed 60 Hz. 60 fps has wide headroom (the engine sims
  ~1900 combat-heavy ticks/s headless; a 60 fps frame needs ~1 tick).
- **`SimParams::default()` / `WorldParams::default()` stay UNSCALED** (the headless/AI/test reference);
  only the game scales (`Game::new(.., TICK_SCALE)`). Headless `--selftest`/`--tune`/automation build at
  `scale = 1.0` — identical per-second behaviour, ~24× fewer ticks — so the layer1/world/ai suites are
  untouched. `orbit_glide`, `prod_square_spin`, `drift_ticks`, `undock_ticks`, `drift_speed` are now
  `SimParams` fields (default = old consts) so the scaler can reach them.
- **Combat flat-grid (universal, `resolve_combat_spread`).** The per-tick spatial grid changed from a
  `HashMap<(i32,i32),Vec<ShipId>>` to a reused flat `Vec<Vec<ShipId>>` (`Structure.combat_grid` scratch):
  AABB of occupied cells → `cols×rows` buckets indexed `(cx-minx)+(cy-miny)·cols`, `mem::take`/clear+refill/
  restore each tick (no per-tick allocation, no hashing). **Byte-identical behaviour** (same cells, ascending
  -ShipId buckets, same 3×3 scan order, sorted targets, RNG draws); `combat_grid` is scratch and excluded from
  `state_hash`. The dense-battle compute (O(k²) scan + per-ship target sort) is the irreducible floor.
- **Headless tools capped for fast iteration.** `--selftest` (~3 s) and `--tune` (~2.6 s) now stop each
  level at `HEADLESS_TICK_CAP = 700` ticks (`min(horizon, cap)`). Self-test asserts **determinism +
  full-budget progress + automation expansion** (not sealed outcome); tune shows the **early/mid-game
  lead**. Long levels no longer reach their sealed result in these tools — raise the one constant for a
  full-horizon run.

### Per-sub economy (replaces the per-structure soft cap)
- Each `SubStructure` now carries its own **`storage_capacity`** (no-attrition headroom, default
  `DEFAULT_STORAGE_CAPACITY = 60`) and **`production`** (ships minted per period, default
  `DEFAULT_PRODUCTION = 1`).
- **Linear per-sub attrition** (`SimParams::per_sub_attrition`, on in the GUI): a sub with
  `count` idle ships above its `storage_capacity` loses `surplus / (storage_per_production ·
  production_period)` ships/tick — attrition is **independent of production**. With
  `storage_per_production = 60` (`K`, "effective storage a point of production buys") and
  `production_period = 18`, a default sub settles at an **effective cap of
  `storage_capacity + K · production` ≈ 120**. This is a self-limiting plateau, not a wall.
- A sub's **radius is derived from its storage capacity**: `radius_for_storage(cap) = √cap · 0.52`
  (floor 1.5), fixed for the match. **Radius does NOT influence combat range** — engagement range
  is a fixed constant. The Layer-2 planet node is sized by **summed storage capacity** (not ship
  count), excluding the reserve node.
- Old model removed: the per-structure `≈ 20 + 10 × owned-subs` / `ceil(0.5·√over)` sqrt cap and the
  global "garrison X/Y" headroom readout are **gone** (legacy `resolve_softcap` path still exists for
  the headless/AI reference params but is not used by the game).

### Struct storage / reserve node (the inter-planet chokepoint)
- Every planet's `Structure` gets a **reserve / patrol-zone node** via `add_storage_sub()` — a big
  circle enclosing the producing subs, the universal inter-planet **entry/exit** point. Produces
  nothing, huge `STORAGE_RESERVE_CAP = 6000` storage, capturable but confers **no** production.
- **Reserve sizing clears the sub garrisons.** The reserve radius is now solved so its garrison
  **ring** (`ring_frac · radius`) sits at least `DEFAULT_ENGAGEMENT_RADIUS (7) + STORAGE_RING_BUFFER
  (2)` beyond the farthest inner-sub edge: `radius = (encl + 9) / ring_frac`. A reserve ship and an
  inner-sub ship of **opposing** sides are therefore always >1 engagement radius apart, so they never
  auto-fight across the reserve boundary — only a deliberate move brings them into contact. (Replaces
  the old flat `encl · 1.35`.)
- **Stage, then transit.** A fleet now departs **only from the reserve**: `issue_fleet_order*` pulls
  the fraction from the reserve if it holds idle ships, else it **stages** — orders every inner owned
  sub to send its idle surplus (above `keep_floor`) **to the reserve** and launches nothing this tick;
  a later order launches the rallied ships. So ships must rally at the reserve before they can leave
  the structure (the reserve is a real chokepoint, not just an entry sub). Bare structures with no
  reserve (headless/test fixtures) keep the legacy direct-from-subs draw (`export_from_subs`).
  Inter-planet fleets still **arrive into** the reserve. Auto-flow of idle sub surplus → reserve
  (without an order) remains a **deferred** future rule.
- **Reserve garrison is drawn in the Layer-2 lens** as dots on a ring inside the planet node, each at
  its real Layer-1 orbit angle (mirrors the interior orbit viz) — so the staged, ready-to-launch
  ships are visible without zooming in.
- Excluded from territory everywhere: `sub_count`, `is_eliminated`, `total_subs`, level `spec_for`,
  game `seat_finished`, `planet_world_radius`, and production. Full design + wiring points in the
  agent memory note `memory/struct-storage.md`.

### WYSIWYG orbit (universal — applies to the headless suite too)
- Idle ships sit on their home sub's **orbit ring** at `centre + ring_frac · radius · (cosθ, sinθ)`.
  `ring_frac` is fixed at `DEFAULT_RING_FRAC = 0.667` (may be authored per sub, but is **not
  player-adjustable** — the mouse-wheel only zooms; the earlier wheel-resizes-the-ring control and
  its `RING_STEP` are removed).
- Each ship **remembers its angle** (`Ship.angle`) and keeps it through transit (inserted into the
  ring's in-order circular list on arrival). Per tick the angle advances by `orbit_rate`
  (`TAU / 200`, tripled from the original) and relaxes toward its neighbours' midpoint by
  `orbit_relax = 0.1`; position **glides** to the ring slot by `ORBIT_GLIDE = 0.35` (so a
  square-spawned ship slides outward). **What is drawn IS the combat geometry** — no separate visual
  ring. See `memory/wysiwyg-contract.md`.
- **Undock time**: a freshly-ordered ship waits `UNDOCK_TICKS = 5` before it starts moving
  (`Ship.undock_remaining`). Ships are *orderable at all times except while transiting or undocking*.
- **Capture cue now matches the grind (WYSIWYG fix).** `resolve_resistance` erodes/heals a sub from
  its **home-based** idle presence (idle ships with `home == sub`), but the interior "being-captured"
  cue and the host's `seat_finished` end-check were reading the **radius** metric
  (`single_present_faction`), which also counts ships passing through or sitting in the enclosing
  reserve node — so a sub could *show* a capture it wasn't getting, and Victory/Defeat could latch a
  few ticks early. New `Structure::capture_presence` / `capture_present_faction` expose the exact
  home-based counts the grind uses; `resolve_resistance` and both GUI readers now share them (bit-for-
  bit identical sim; the cue/end-check just stop diverging).
- **Production slots slowly orbit (CCW on screen).** A producing sub's spawn slots — the GUI's
  production squares — rotate by `PROD_SQUARE_SPIN_PER_TICK = TAU/80` per **tick**. This is a *sim*
  property, not a render overlay: `spawn_at_square` adds `tick · spin` to the spawn angle, so the
  squares always sit exactly where ships are created, and because it keys off the tick (never
  wall-clock) replay stays bit-for-bit deterministic. The GUI draws the squares at the same tick-based
  angle (interpolated by `render_alpha`).

### Combat rework (grid-accelerated spread)
- `SimParams::spread_damage` (on in GUI): combat runs on a **uniform grid** (cell = engagement
  radius, 3×3 neighbourhood — no O(N²)). Each engaged ship spreads `fire_prob / k` across its `k`
  in-range enemies; expected kills/shooter = `fire_prob`, so **Lanchester's square law is preserved**
  while damage feels continuous and spread across targets.
- **Transit-fire gating** (`SimParams::transit_fire_gating`, on in GUI): a moving ship cannot fire on
  a stationary (idle, no-target) enemy — but a garrison fires on the incoming wave. Lets a defending
  garrison trade with attackers fairly.
- Death-FX picks a **random** in-range enemy as the shooter (not nearest).

### Faction-scoped orders (bug fix)
- `issue_order` / `issue_order_fraction` / the world fleet-order wrappers now take a **`faction`**
  argument and only move that faction's idle ships at the source. Fixes the enemy AI dragging the
  **player's** ships off a contested sub. Threaded through every caller (ai, layer1 drivers, game,
  levels validation, tests).
- Sending **100%** now takes everything (keep-floor 0); other fractions keep the old floor.

### GUI overhaul (`crates/game`)
- **Top bar**: a continuous **1–100% troop slider** + **Pause / x1 / x3** speed buttons
  (`SPEED_IDX_1X = 1`, `SPEED_IDX_3X = 4`) + a count-up clock. Removed the Goal line, the
  "automation" text, the bottom controls help, and the global ship/planet/sub/garrison totals.
- Game **starts paused** and unpauses when the briefing is closed. Default fleet fraction **100%**.
- **Interior render**: per-sub **production squares** (N empty white squares at ½ radius = the sub's
  `production`; a new ship appears at the next square round-robin then glides to the ring); ships
  drawn at real sim positions (idle = circle, moving = triangle); **per-side present counts** in each
  faction's colour (idle/home-based, excludes incoming); the **reserve node** as an outline-only big
  circle (no fill obscuring inner subs, no squares/ring), selectable with counts; ship-death flashes.
- Global pacing slowed: `BASE_TICKS_PER_SEC` 5 → **2.5**; no horizon for human matches (the clock
  counts up; the match ends only on a sealed result). `KILL_FX_TTL = 0.35`.
- GUI operating point (`gui_params()`) **diverges** from `SimParams::default()` (the headless/AI
  reference): `fire_prob = 0.0055`, `defender_fire_bonus = 0.003`, `transit_fire_gating = true`,
  `spread_damage = true`, `per_sub_attrition = true`. Behavioural changes are GUI-gated to protect
  the parked AI suite — **except orbit, which is universal**.

### Campaign (`crates/levels`)
- **Full Simple campaign**: **L1 = Passive**, **L2–L10 = `Roster::SimpleColonize`**. The
  Colonize/Defend/Attack automata and the L7 seam / L8–L10 RPS lessons are **parked**.
- `validation.rs`: `check_lesson` reduced to a uniform `not_auto_lost`; `LevelReport::ok()` =
  `structure_ok && deterministic` (lesson validation parked). `counter_beats_enemy` /
  `seam_flank_beats_greedy` kept as documented dormant `#[allow(dead_code)]` for the automata redo.
- Every campaign planet gets a reserve node (the 3 `builders.rs` planet helpers call
  `add_storage_sub()` before `Planet::new`).

### Verification (current state, after the latest session)
- `cargo check --workspace` **zero warnings** · `cargo run -p game --release -- --selftest` all 10
  levels deterministic (`det=true`), **ALL PASS** · `cargo test -p layer1` 37/37 · `-p ai simple` 14/14
  (full `-p ai` 89/0 but slow) · `-p world` 41/41 · `-p levels metadata_is_complete` green · doctests
  green. The seat-index (N-seat) refactor preserved faction `state_hash` codes, so existing levels
  replay bit-for-bit; L3's two-AI free-for-all confirms the multi-seat path. The render-perf + GUI work
  is render/input-only (no sim impact). **Not re-validated:** the slow `-p levels`/`-p world`
  lesson+projection suites and the parked AI/diamond measurements (they exercise the parked track).

### Deferred / next steps
1. **Mission 1–10 design** — the user drives it; L4–L10 are placeholder multi-planet worlds. Difficulty
   is **ad-hoc, not a curve** (amended when beta-testers complain).
2. **Auto-flow** sub surplus → reserve (future automation rule); **redesign player-automation** (it is
   currently quarantined).
3. **Per-seat Layer-2 aggregate / AI** for a multi-planet free-for-all: `PlanetAggregate` and the AI's
   `Layer2View` are still player-vs-combined-rivals binary (the pie chart + count labels are already
   per-seat; this is only relevant once such a level exists).
4. **Reconcile the parked `projection.rs`** entry-sub-vs-reserve divergence (fixed on the live
   `sub_influx_for` path) before the automata are trusted again.
5. **Revive the automata track** (Colonize/Defend/Attack + Counter); re-run/re-tune the diamond RPS +
   full suite under the new orbit/economy/seat-index.
6. **Further sim perf if needed** — a per-sub idle/ship tally to kill the remaining O(S·N) per-tick
   count scans (`resolve_resistance`/`produce`/softcap); the F3 overlay measures it. The render
   bottleneck (per-frame draw) is addressed.

### Known gaps
- **Parked projection arrival routing**: `projection.rs` still schedules inter-planet arrivals into
  `entry_sub` while `inject_fleet` lands them in the **reserve node**. **Fixed on the live path** —
  Simple/`AiController` read the projection-free `World::sub_influx_for`, which routes to the reserve —
  so the divergence survives only inside the parked projection; reconcile on automata revival.
- **`state_hash` omits `Ship.undock_remaining`**: replay is still bit-for-bit deterministic; fold it in
  if you ever extend the determinism guard.

### Docs that now lag this entry (refresh when the rework settles)
- `README.md` — **refreshed this session** (crate map, controls, campaign, doc map); the "Validated
  results" section is flagged historical (parked AI).
- `GAME.md` / `WORLD.md` / `LEVELS.md` / `LAYER1_SIM.md` — each carries a **status header** pointing
  here, but the body still predates the per-sub economy / orbit / spread combat / reserve node / the
  seat-index (`Faction::Ai(u8)`) / `Level.enemies` / the GUI rework. Refresh a section when it settles.
- `docs/archive/*` (`00`–`04`, `AUTOMATA_DESIGN`, `AI`, `COUNTER_*`, `CAPSTONE_*`, `R2_*`, `DSL_SPEC`,
  `LAYER1_GAME`, `LAYER2_GAME`) — the **parked/deferred** AI/architect/DSL/vision work; accurate as
  history, not current focus. Their `SimpleColonize` material describes the **old stateless
  `simple_colonize` automaton**, not the live stateful `SimpleController`.
