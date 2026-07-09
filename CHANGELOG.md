# Changelog

Newest first. Each entry records the **delta** from the previous state and is authoritative for the
mechanics it touches — when a per-component doc (`LAYER1_SIM.md`, `GAME.md`, `WORLD.md`, `LEVELS.md`,
`README.md`) disagrees with the latest entry here, **this file wins** until that doc is refreshed.

---

## tutorial — the narrative TEXT layer: `--text`, per-level asset dirs, `.brf` / `.glg` (2026-07-08)

Owner feature. **All text-narrative features are OFF and hidden by default** — pre-mission
briefings, post-battle logs, the in-mission Notes, the Memory page — and return under the
new **`--text`** flag (without it: missions start directly, no notes icon, no Memory row).

**Per-level asset bundles**: each mission now lives in its own directory —
`assets/levels/01_first_steps/01_first_steps.lvl` + optional `_pre.brf` (briefing) and
`_post.glg` (post-battle log template); the player's notes move to `assets/notes/notes.glg`
(legacy `mi_notes.txt` migrates on first load; `assets/notes/` is gitignored — the game
writes there). The campaign loader scans mission directories (sorted by name; loose `.lvl`
files still load); the in-code briefing consts (L1-L4) moved VERBATIM into `.brf` files.

**The shared template pass** (`crates/game/src/narrative.rs`, unit-tested) runs on both
formats before the briefing markup parser (colors/effects/reveal pacing unchanged):
`#` comments; `{result} {ticks} {lost} {killed} {ships}` metric substitution; and
**logic-based text** — `?if / ?elif / ?else / ?end` with conditions
`<key> <op> <value>` (ops `= != > < ~`; `~` = contains, e.g. `notes ~ teleporter` scans the
player's notes). A **briefing** renders against the PREVIOUS battle's metrics; a **post-battle
log** against the match that just sealed.

**Post-battle logs**: on seal (with `--text`) the level's `.glg` renders against the match —
result, duration, own losses / enemy destroyed (counted off the death-FX liveness diff —
presentation-only, never sim state), final fleet — is appended to
`assets/notes/battle_logs.glg` behind a machine-readable `#metrics` line (the flag source
later logic-based text reads back), and is viewable from the end screen with **`L`**.
`01_first_steps_post.glg` ships as the annotated exemplar; metrics will expand gradually.

---

## tutorial — per-struct zoom bounds; degenerate-fit fix reverted (2026-07-08)

Owner correction: folding the reserve ring into a lone-sub struct's camera fit was the wrong
fix — REVERTED. The right mechanism: **`[struct]` `zoom_min` / `zoom_max`** in the data files
override the out-zoom floor / in-zoom ceiling **while that struct's interior is focused**
(defaults = the `[level]` `zoom_min` / the global `ZOOM_MIN 0.60` / `ZOOM_MAX 7.0`; the wheel,
the slider mapping, pan clamping and the exit-to-lens notch all consult the effective bounds,
and entering a struct clamps a zoom carried from another). Far far away's rear yard-only
struct sets `zoom_min = 0.1`, so the whole reserve ring is visible before the transition to
Layer 2. Code-built worlds (arena, selftest) have no spec — global defaults.

---

## tutorial — ONE pause menu; Ctrl+box additive select; degenerate-struct camera fit (2026-07-08)

Owner control tweaks:

- **The two pauses are one** (`Esc` and `P` both raise it): sim frozen, **camera free
  underneath** (the normal input path runs under the menu; orders stay refused), hover-lit
  **Resume / Restart / Main Menu** — Main Menu goes to the MAIN menu, not level select — and a
  **✕** (top right) that hides the panel and lifts the veil (a bare "Paused" corner tag
  remains) so the player can inspect the frozen level; `Esc`/`P` bring the panel back, and on
  the open panel (or `Space` anytime) resume. `Esc` **never zooms to the lens and never clears
  the selection** any more — right-click clears, wheel-down at the floor exits the interior.
  Internals: `paused_menu` app-state and the `OPEN_PAUSE` sentinel deleted; the menu is
  `Game::{paused, pause_buttons}`; keyboard menu nav dropped (arrows pan the free camera).
- **Ctrl+box-drag is additive** (both layers): the boxed subs/structs JOIN the selection — and
  if every boxed one is already selected, they LEAVE it instead (box-toggle). A lone
  single-select folds into the multi first. An empty Ctrl+box changes nothing (a plain box
  still replaces/clears).
- **Degenerate-struct camera fit** (owner bug report: Far far away's rear struct was a giant
  close-up that exited to the lens before it could ever be seen whole): `interior_camera`
  excludes the reserve from the fit BY DESIGN, but a struct whose tactical cluster is ≤ 1 sub
  now includes the reserve ring — with nothing else to read, the ring IS the structure.

---

## tutorial — v4 cohesion: the striped stalemate coarsens (2026-07-08)

Owner playtest find: after the melee, storage settles into **alternating faction stripes that
parade around the ring** (nearest-foe pursuit's neutrally-stable translation mode) until one
anchor absorbs everything. Wanted instead: "each side progressively agglomerated along their
most dense groupings until the two pockets clash — a tournament of elimination." That is 1D
phase-separation **coarsening**, and it needs the one ingredient v4 deliberately lacked:
**surface tension**. New third term:

- **Cohesion** (same-faction, wartime only): a ship with staged foes is urged toward whichever
  side holds more of its own faction within ±`ORBIT_COHESION_SPAN = 12` wu, at
  `ORBIT_COHESION_STRENGTH = 0.5 × fs ×` the normalized count imbalance. Small same-side groups
  evaporate toward their side's bigger masses; the enemy stripe caught between two merging ones
  is ground from both edges; domains grow pairwise until two pockets clash.
- The strength is **strictly below 1 by design**: the first cut (full fs) froze the arena — at a
  pocket's leading edge the whole side sits behind (imbalance ≈ 1) and cohesion exactly
  cancelled the drive; nobody had died by t2400. At 0.5 the approach survives (slower, cohesive
  advance — an accepted trade).
- Wartime-only gating: an always-on attraction would collapse the peacetime uniform ring.
- Tension named: cohesion tempers pure mixing at the final clash (each side pulled back toward
  its own mass ⇒ a thick grinding band, not full salt-and-pepper) — inherent to coherent
  pockets; micro-mixing at the interface remains (still no standoff).

Verified: suites green untouched; selftest det ×7 (debug binary — the release exe was locked by
a live session); arena timeline: approach ✓ engage ✓ grind ✓ survivors disperse ✓. The stripe
coarsening itself only manifests in live storage brawls — owner playtest is the judge.

---

## tutorial — ORBIT MODEL v4: faction-blind pressure + nearest-foe drive — battles MIX (2026-07-08)

The owner's struct-storage-combat movement rework, grounded in the social-force literature
(overdamped interacting particles on a ring; pairwise kernel forces, never mean positions —
superposition makes balanced crowds exactly calm, responses proportional to imbalance, and the
whole thing an approximate gradient flow: no cycles, deterministic settling). Replaces v3's
five-knob separation/seek/leash engine with two terms and two constants:

- **Pressure** (always on, **faction-blind**): every other idle ship on the ring within the
  hat-kernel width `ORBIT_PRESSURE_SPACING = 1 wu` pushes directly away with weight
  `1 − arc/w`; signed sum × `fs / ORBIT_CROWD_STIFFNESS (10)`. 1D porous-medium flow: a point
  blob rarefies monotonically to the uniform ring. Faction-blind = the owner's explicit
  mixing decision: with no cross-faction standoff anywhere, opposing clouds are MISCIBLE —
  **battles interleave into a salt-and-pepper melee at ~w spacing; fronts are structurally
  impossible** (v3's leash-made firing lines are gone, per the owner: "I'd prefer if they mix").
- **Drive**: a ship with staged foes moves toward its **nearest** foe's bearing (never a
  weighted mean — means point between enemy clusters and strand ships on deterministic
  saddles) at `fs·min(1, arc/w)`: full speed to contact, proportional taper below, no standoff,
  no leash. Kill-gaps refill from the neighbours flowing in.
- DELETED: `SEP_COMFORT`, `SEP_PRESSURE_SPAN`, `ENGAGED_LEASH_FRAC` (consts) and
  `seek_speed_frac`, `orbit_relax` (`SimParams` fields). Radial ring-band churn unchanged.
- Implementation: f64 prefix sums over the sorted bearing list answer each side's windowed
  kernel sum in O(log n) (f32 differences at reserve scale would inject O(1) pressure noise);
  the per-seat nearest-foe binary search is unchanged. Deterministic; speed law intact.
- Also fixed in passing: the id-99 dev ARENA drew at tick 0 — the "Passive seats never block
  victory" rule needed a dev-arena exemption alongside M1's.
- Verified: all suites green untouched (invariant-only tests), selftest det ×7, and the arena
  loop shows the intended lifecycle — two 300-ship clouds converge, interleave into a mixed
  melee, grind by the square law, survivors rarefy around the band. FFA t30000: Simple still
  clears the board.

---

## tutorial — defender_fire_bonus → 0 in the game (2026-07-08)

Owner retune: the game's home-ground fire advantage (`0.003/scale` on top of `0.0055/scale`,
i.e. +55% fire rate ≈ a √1.55 ≈ 1.24× force multiplier for a garrison on owned ground) is
now **0** — defence comes from position, numbers, transit-fire gating and fortress range,
not a hidden dial. The mechanic itself survives (reference `SimParams::default()` keeps
0.012 for the headless/test model; `0.0` is the documented off-switch).

---

## tutorial — Deliberation fixes: prod=0 clamp, storage distance crutch, leash bypass (2026-07-08)

Three owner items from the first post-tutorial playtests:

- **`prod = 0` means zero** (sim bugfix): `with_production` silently clamped to `max(1)`,
  giving Deliberation's teleporter a phantom production square — and a real 1/period output.
  Clamp removed; a level's authored 0 is now honoured (squares, spawns and economy).
- **Storage distance crutch** (Simple's view only): the reserve's centre position is a lie —
  its garrison lives on the huge orbit ring — so it read as everyone's nearest neighbour
  (observed: the most central node treated storage as ~14 wu away). Simple now prices the
  reserve at **its radius' distance from every other sub** (`distance` and `transit_ticks`
  agree, so ranking, adjacency, pull ordering and synchronized landings are consistent).
- **Adjacency leash bypass**: when every sub within `range` of owned ground is owned and
  quiet (uncontested, nothing inbound), the leash has done its job — SimpleAdjacent used to
  go blind at that point (Deliberation's winner conquered the middle, then never reached the
  player's out-of-range corner). The step now runs unleashed — candidates, funding legs and
  stage hops — and re-tightens by itself the moment anything foreign is back in range.

**The observed node↔storage ping-pong, explained**: the reserve at (0,0) was in-leash and,
once the middle was won, the ONLY candidate — so the winner poured spare into a claim op;
meanwhile the shared reserve keeps collecting every seat's auto-diverted overflow (the
player's included), so the staged-majority presentation flips as stacks change, and each
flip re-triggered a wave in (foe-majority = attack) or an evacuation out (over-threat =
DEFEND flees to the nearest safe sub — the central node, by the same lying distance). The
crutch removes both the false adjacency and the false refuge; the bypass gives the winner a
real target. Verified by a hands-off L6 t30000 run: garrisons at floor, the assault wave
driving west at the player, no storage churn. Pinned: leash lift + hold (ai), radius
distances (adapters); suites green, selftest det ×7.

---

## tutorial — `.lvl` format: `+-X` uniform position noise (2026-07-08)

Owner ask (driving the Deliberation re-author): any `pos` component — `[sub]` and `[struct]`
alike — may be written `A+-X`, drawing uniformly in `[A−X, A+X]` **once at world build from
the match seed**: fixed for the whole match, bit-identical on replay (the determinism gate
runs unchanged), and a fresh layout every new match. Implementation: the spec stores
`(base, per-component half-width)`; `LevelSpec::build` applies the jitter via a dedicated
hand-rolled splitmix64 stream (seeded from the match seed, xor-decorrelated from the
interiors' `seed + i` streams), consumed in strict file order and only for non-zero widths.
Derived geometry (garrison rings, reserve clearance solve, orbit radius/phase for a noisy
orbiting sub) follows the jittered position automatically. Ring members and `[orbit]` keys
stay exact. Pinned: parse shape, in-bounds draws, same-seed bit-identity, cross-seed
variation, malformed-width rejection.

---

## tutorial — gate_tuning_constant 1 → 5 (2026-07-08)

Owner retune: the HotS gate (savings .154) now earns `5 × .154 × 5 ≈ 3.9` virtual production
in Simple's neutral ranking — competitive with the 2-prod posts beside it instead of under
half a lean post.

---

## tutorial — fort_coverage: incident edges count (2026-07-08)

Owner bug find (revealed by Sinews' rear forts reading 0.0): `fort_coverage` excluded every
pair **incident to the fort itself**, as if trips to and from a fort were not walked under its
guns. Now ALL `n·(n−1)/2` sub pairs count, and an edge is covered when it is incident to the
fort or its straight segment comes within the overwatch reach (`seg_point_dist ≤ reach` —
which was already catching non-incident edges with an endpoint inside the zone). Every fort
thus floors at `2/n` (its own `n−1` edges). New campaign numbers: HotS wall S→N
.410/.718/.667/.333 (gate savings unchanged, .154 — no analogous bug there); Sinews
.367/.567/.392 + rear pair at the .125 floor; FFA watchers ≈ .58/.58/.59.

---

## tutorial — Simple: prod-equivalent priors + fort-reach ranking; fort resistance 5 400 (2026-07-08)

Owner amendment to the target-choice logic (the two-tier neutral-before-enemy order stands):

- **`FORTRESS_RESISTANCE` 10 800 → 5 400** (sim): a fort is a wall by garrison + range, not by
  grind — now 1.5× a default sub, and its `resistance/60` ship-equivalent (90) matches its
  own capacity.
- **Neutral ranking** (owner formula):
  `(res + 1800) / (prod_raw + fort_pe·coverage·fort_tc + gate_pe·savings·gate_tc) + 20·dist`.
  New dials replace `fort_value`/`gate_value` (travel-tick bonuses, deleted):
  `fort_prod_equiv_value = 5`, `gate_prod_equiv_value = 5`, `fort_tuning_constant = 2`,
  `gate_tuning_constant = 1` — a commanding fort / shortening gate earns its keep as
  *virtual production*. `prod_raw` is the authored number (a fort's true 0; new
  `PositionView::production_raw`); denominator floors at 1e-3, so worthless ground (0-prod,
  no quality — the reserve's virtual-resistance pricing included) stays finite but last.
- **Enemy ranking**: `distance − fort_reach` — nearest first, a rival fort counted as if the
  attacker already stood at its zone edge (new `PositionView::fort_reach`, ≈21.7 for a
  standard fort, any owner; 0 for non-forts).
- **Ranking distance is now raw Euclidean world units** from the nearest owned sub (owner Q:
  it was NOT the same as the old `travel` — that was ticks, undock-inclusive and
  teleporter-aware). Unified on wu for unit-consistency with `fort_reach`;
  `neutral_dist_weight` 5 (per tick) → 20 (per wu). Funding legs, tolls and synchronized
  landings still use travel ticks + gate routing.
- Verified: ai 95 green (new pin: `enemy_fort_ranks_as_if_at_its_zone_edge`; the reordering
  test recomputed for the new formula), selftest det ×7, hands-off FFA t30000 still clears
  the whole board.

---

## tutorial — Simple: STORAGE AS A SUB; fort_toll 1.5 (2026-07-08)

**Storage-specific AI behavior is gone** (owner redesign). Simple's view (`Layer1View::direct`)
now presents the ownerless reserve **like any other position**:

- **Owner = staged-ship majority**: the seat's staged plurality reads *mine* (usable spare, a
  consolidation/muster destination like any sub), a bigger foe stack reads *enemy* — a real
  PLAN target priced `OVERWHELM(present+incoming)`, funded as a synchronized op or, when
  unfundable, massed toward via STAGE FOR SIEGE (the wait-until-overwhelming doctrine falls
  out of the ordinary phases). Ties and an empty reserve read *neutral*.
- **Virtual resistance ∝ capacity** (`storage_capacity × RESISTANCE_PER_CAPACITY` — the same
  coupling every real sub has): with 0 production and no special quality, the reserve is the
  **guaranteed least attractive colonization target around** (default 6000-cap ⇒ 360,000 vs a
  fortress's 10,800). It is "claimed" with a floor wave only when nothing else remains — and
  the sim still never lets storage flip owners, so the claim just stages the stock.
- DELETED: `storage_relief` (the 2026-07-07 reserve→pressured-sub commitment — superseded:
  staged stock is ordinary spare the planner spends) and every `is_staging` special case in
  stage-for-siege/consolidate (a majority-owned reserve is a legitimate rally/muster).
- The greedy/stateless rosters keep the legacy own-ground disguise — only Simple's `direct`
  path gets the new presentation.
- **`fort_toll` 1.0 → 1.5** (owner retune): waves crossing rival fortress zones oversize by
  1.5× the manning. Verified + pinned: the toll **includes the target fort's own zone** —
  assaulting a 30-manned watcher prices `OVERWHELM(30)=50` + toll `ceil(1.5×30)=45` gross.
- Levels: `metadata_is_complete` trimmed to `seat_and_automation_invariants_hold` — the
  blurb/hint/horizon length checks were content gates on designer-owned text (they false-failed
  on live briefing edits; owner rule: tests pin invariants, never content).
- Verified: ai 94 green, selftest det ×7, hands-off FFA t30000 — Simple clears the whole board
  (all six ring subs, all three watchers, 357 pooled at the yard).

---

## tutorial — `.lvl` format: the `[orbit]` ring constructor (2026-07-08)

Rings are now authored as rings, not as lists of hand-computed coordinates (owner ask).
An `[orbit]` section takes `center`, `radius` and `period` (reference ticks per revolution;
negative = clockwise); every `[sub]` that follows — until the next `[orbit]`/`[struct]`/
`[lane]` header — is a **member** of that ring. Members may differ freely (kind, owner, cap,
garrison…) and take an optional `angle = <degrees>` (0° = +x, counter-clockwise) instead of
`pos`; omit **every** angle and they are spaced regularly in file order starting at 0°
(mixing given and omitted angles is a parse error, as is `pos`/`orbit` on a member). The
ring compiles down to per-sub `pos` + `orbit` at parse time — the world interpreter is
untouched. One layout rule: plain positioned subs go **before** a struct's rings (a ring
runs to the next header). Far far away now reads as it was designed — one `[orbit]` for the
six-sub outer ring (regular spacing), one for the three watchers (explicit 90/210/330);
its centre yard moved to sub id 0 (file-order rule; layout unchanged, and the ring math
retired ~4e-4 of hand-rounding in the old coordinates). Format reference: the `spec.rs`
module doc. Verified: spec unit tests pin the constructor's contract; selftest det ×7.

---

## tutorial — DATA-DRIVEN LEVELS: the campaign is 7 `.lvl` files (2026-07-08)

The owner's compile-time ask, delivered: **tweaking a level no longer costs a recompile** —
edit `assets/levels/NN_name.lvl`, restart the game, done (verified end-to-end: a garrison
edit showed up in a fresh `--shot` with zero compilation).

- **The format** (`crates/levels/src/spec.rs` module doc is the reference): plain-text
  `key = value` under `[level] / [struct] / [sub] / [lane]` sections — metadata (title,
  blurb, hints, enemies incl. `simple_adjacent <range>` / `cycler`, start view, horizon,
  zoom_min) plus the full world recipe (positions, kinds, owners, caps, prods, garrisons,
  `max_resistance`, `keep_surplus`, orbits as `center + period`). Hand-rolled parser —
  the zero-external-dependency rule holds; errors panic loudly with file + line.
- **The interpreter** (`LevelSpec::build`) reproduces the old hand-written builders exactly:
  same construction order, same RNG draws (sub added → garrison spawned, reserve last,
  struct `i` seeded `seed + i`). A temporary **migration check** compared every file to its
  old builder sub-by-sub before the builders were deleted — and caught one real
  discrepancy: Far far away's "0.6 reserve dial" existed only in a comment and LEVELS.md;
  the code always shipped the default. The file keeps the SHIPPED behaviour (default), with
  a note showing the one-line opt-in.
- **API**: `Level.build` (bare `fn`) became `Level.source: LevelSource` —
  `Spec(Arc<LevelSpec>)` for the campaign, `Builtin(fn)` for the code-built dev scenarios
  (the arena, the selftest automation world). `Level::world(seed)` dispatches; everything
  downstream (validation, selftest, GUI) is unchanged and green (determinism ×7).
- `campaign()` loads from `assets/levels` next to the exe, else the workspace tree (dev
  runs + tests); filename order = play order.

---

## tutorial - adjacency legs + nearest-first dispatch + STAGE FOR SIEGE; FFA rings (2026-07-08)

Owner fixes on the turning field:

- **SimpleAdjacent no longer crosses the middle**, two mechanisms: (1) the adjacency leash now
  binds **funding legs** too (pull() sources must sit within range of the target - restricting
  only the target let far garrisons fund a front straight across the field); (2) the engine's
  `dispatch_move` now selects idle ships **nearest-to-the-target first** (ties by ShipId; was
  lowest-id) - a partial draw from a huge ring (the reserve above all) peels the side already
  facing the destination instead of yanking far-side ships across the map. Deterministic; full
  sends unaffected; also quietly improves the player's own fractional reserve sends.
- **STAGE FOR SIEGE** (owner: "Simple does not concentrate force"): when capture candidates
  exist but nothing was fundable this decision (a big player garrison; doubly so under the
  adjacency leash), every quiet garrison's surplus above the floor RELAYS toward the
  mustering ground - the owned sub nearest the top target - one adjacency hop at a time
  (directly, unrestricted), accumulating until the front's minimum funds. Supersedes the old
  wandering consolidate in that case (consolidate keeps the no-candidates-at-all case);
  the two Simple tests re-pinned accordingly. Hands-off FFA at t30000: Simple now cracks the
  passive player's 90-garrison sub and clears the board (previously it stalled forever).
- **Far far away rings** (owner-tuned): fortress ring 14 -> 18.2 (x1.3), outer ring
  90.72 -> 72.576 (x0.8); SimpleAdjacent range retuned to 100 (neighbour chord 72.6 <
  100 < skip-one 125.7).

---

## tutorial — Far far away tuning II: the ADJACENT Simple + enemy hub yard (2026-07-08)

Owner batch:

- **`Roster::SimpleAdjacent { range }`** — the full stateful Simple with one leash
  (`SimpleParams::adjacency_range`, a per-mission dial): on any struct where it owns ground,
  its PLAN candidates are filtered to targets within `range` world units of an owned sub —
  expansion crawls neighbour to neighbour and never launches waves across the middle. With no
  owned sub on a struct (fresh invasion) the restriction is moot. Parameterized like
  `Counter`, excluded from `Roster::ALL`; stateless fallback = the plain colonizer.
- **Far far away**: ring radius ×1.2 (75.6 → **90.72**); the hub shipyard starts
  **enemy-owned and empty** (active token bar — its output pools under the watchers' guns);
  fort garrisons **60 → 30**; enemy seat = `SimpleAdjacent { range: 120 }` (the 60° neighbour
  chord = 90.72 plus margin, below the skip-one chord ≈ 157 — neighbours and the hub qualify,
  far arcs never do). Hands-off shot at t30000: Simple crawls the ring sub by sub, takes the
  hub from adjacent ground, and duels the watchers — no cross-middle waves.

---

## tutorial — M2 ⇄ M3 swap: Fire in the sky before Command and Control (2026-07-08)

Owner ordering: **M2 = Fire in the sky** (the first live combat, vs Simple), **M3 = Command
and Control** (fleet command, vs the Cycler). Ids, section headers, tables and plan docs
swapped; the builders and mission content are untouched.

---

## tutorial — surplus can stay home: `keep_surplus` + First steps' keep (2026-07-08)

Owner QoL: in First steps, the Passive centre's over-cap spawns auto-diverted onto the
player's staging ring (the engine auto-flow). New per-sub authoring dial —
**`SubStructure::divert_surplus`** (default `true`; builder **`keep_surplus()`** sets false,
hashed): a `false` sub's production stays home no matter how full it is, the surplus simply
bleeding under the per-sub attrition. L1's keep authors it — its garrison no longer leaks
onto the reserve. Every other sub keeps the auto-flow.

---

## tutorial — per-level zoom floor: ZOOM_MIN 0.60, Far far away 0.8 (2026-07-08)

Owner tuning: the global out-zoom floor rises 0.25 → 0.35 → **0.60** (retuned same day), and
`Level` gains a presentation dial — **`zoom_min: Option<f32>`** (None = the global floor; the
sim never reads it) — resolved by the new `Game::zoom_min()`, which every interactive clamp
(wheel, slider mapping, pan clamp, the interior→lens exit notch) now consults. **Far far away
runs at 0.8** (via 0.5):
the interior frames the turning field; the reveal beyond it belongs to the lens (wheeling out
at the floor still exits to the lens on multi-struct maps). The `--shot --zoom` dev capture
flag is exempt from the play-facing floor (clamped 0.01..MAX) so the aesthetics screenshot
loop keeps its wide frames.

---

## tutorial — L8-L13 deleted; orbit polish; Simple reserve/last-stand fixes; 4x faster rebuilds (2026-07-07)

The post-approval batch (owner):

- **L8-L13 DELETED** — the campaign is the seven hand-authored missions, full stop ("no point
  keeping information explicitly marked as doesn't-matter"). The multi-struct cluster builders
  they used (`stocked_struct` / `neutral_struct` / `neutral_struct_res` / `diamond`) went with
  them; `builders.rs` keeps only `default_world_params`. Counts fixed across lib docs,
  LEVELS.md, GAME.md; every remaining mission opens in Layer 1.
- **Camera pulsation on the turning field FIXED**: the interior fit (and the pan clamp) fed on
  instantaneous sub positions, so the frame breathed as the orbiting ring's bounding box
  turned. Both now use each orbiting sub's **orbit envelope** (centre ± radius + footprint) —
  rotation-invariant, rock-steady.
- **Garrisons ride their platform**: an orbiting sub's per-tick displacement now moves its
  idle ships directly (then the usual glide/urges act on top) — no more trailing behind the
  sub on the glide. Sim-side (`advance_sub_orbits`), deterministic, hashed positions.
- **Production squares scale with zoom** (the old 2..6 px clamp froze them; 1 px floor kept).
- **Max out-zoom reduced to 0.8x** of what it was (`ZOOM_MIN` 0.2 → 0.25) — the reserve still
  fully in view, less dead void beyond.
- **Simple: STORAGE RELIEF** — in a contested struct (foe ships idle on or inbound to a sub it
  owns), its whole reserve stack immediately reinforces the most-pressured owned sub, issued
  before the planner (which then sees them as influx). Reserve hoards fight the home fight.
- **Simple: last-stand targeting reworked** (owner spec): stacks (largest first) each take the
  nearest unclaimed target they can **overwhelm alone** (`max(1.2F, F+20)` vs foes present +
  inbound) — one stack per target where possible; stacks that can solo nothing **merge** onto
  the biggest jointly-overwhelmable target (or reinforce the easiest claimed kill); if
  *nothing* is overwhelmable, **all-in** on one hash-picked target (no RNG drawn).
- **Rebuild times ~4x faster** (owner ask): the release profile's `codegen-units = 1` (a
  legacy of the parked mean-field sweep) serialized a ~105 s optimize of the game crate on
  every one-line touch; now 16 parallel codegen units + thin-LTO → **~26 s**, with the sim's
  10k-ship perf row within ~20 % (2.9-3.1 vs 2.4 ms/tick) and `layer1` pinned to
  `codegen-units = 1`. Measured and rejected: `incremental = true` (ticks ~4x slower),
  `lto = false` (~1.6x slower ticks for only ~14 s less rebuild). Determinism unaffected
  (selftest det ×7).

---

## tutorial — shipyard default = the token bar; the Arc-2 fiction is dead (2026-07-07)

Owner corrections after the FFA hub-yard question:

- **A shipyard's default resistance is 1.0** (the engine floor), whoever authors it — a
  zero-capacity sub carries no resistance. The constructor no longer special-cases neutral
  authorship with the 10 800 activation bar; `SHIPYARD_INITIAL_RESISTANCE` is **deleted**.
  The activation mechanic itself survives as a **per-level opt-in**: author a neutral yard
  with `with_max_resistance(X)` and its first capture still collapses the bar by
  `SHIPYARD_ACTIVE_RESISTANCE_FRAC` (floored at 1.0), permanently. Far far away's hub drops
  its now-redundant override; the two shipyard tests re-pin the new default + the opt-in's
  collapse mechanism. No other shipped yard was neutral-authored, so nothing else moves.
- **There is NO Arc-2 design** (owner decree): every doc claim of Arc-2 shape or "reserved
  material" (the contested-activation flavour, blockade lessons, the sacrifice set-piece) is
  struck from LEVELS.md and the mission docs. **Likewise the L8–L13 placeholders carry no
  design intent** — void leftovers kept only so the campaign list stays playable.

---

## tutorial — Far far away tuning + two endgame QoL rules (2026-07-07)

Owner follow-ups on the rework below:

- **Far far away**: the orbiting ring is **six** 90/3 subs 60° apart (was four cardinals) at
  **R = 75.6** (1.8× — the guarded core now reads as its own place); the hub shipyard drops
  the activation grind to a **token 1.0 resistance** (0 requested; 1.0 is the engine floor,
  and activation keeps the same floor) — reaching it under the watchers' guns is the whole
  cost, and it flips on contact.
- **Simple's LAST STAND**: a Simple with **no producing sub left anywhere** bypasses its
  planner wholesale (ledger cleared) — every idle stack, wherever it sits (reserve
  stockpiles, fort garrisons — the never-evacuate doctrine explicitly overridden — gate
  posts), attacks the nearest foe-owned sub in its struct (else the nearest foe-staged sub);
  stacks already on foe ground or in contact are left to their work. Re-issued each decision
  tick. No more camped remnants forcing a mop-up grind.
- **Passive seats never block the win** (all missions except M1, whose objective IS the
  Passive): `all_enemies_finished` skips `Roster::Passive` seats — they are obstacles, not
  objectives. On the reworked L7 the three watcher forts can simply be avoided; killing
  Simple ends the mission.

---

## tutorial — ORBITING SUBS + "Far far away" reworked as the contested turning field (2026-07-07)

The last tutorial-branch item (owner spec). Two parts: a new **sim mechanic**, and L7 rebuilt
on it.

**Orbiting sub-structures (engine).** `SubStructure` gains an authored orbit
(`SubStructure::orbiting(center, omega)` → `SubOrbit { center, radius, phase, omega }`,
radius/phase captured from the authored tick-0 position; negative omega = clockwise on screen,
the `orbit_rate` convention). The sub's `pos` is a **pure function of the tick** — refreshed
at the top of every `Interior::step`, no incremental drift, replay-exact — so everything
positional follows for free: garrison rings glide with the sub, fortress zones travel, combat
and capture read the moving truth. The orbit spec is folded into `state_hash` (the moving
`pos` already was); `build_scaled` re-grounds omega like the other per-tick rates.
**Intercept targeting** (owner rule: ships never chase): `dispatch_move` aims each ship at the
destination ring **as it will stand on arrival** — undock delay + straight flight at
`ship_speed` (newly cached beside `undock_ticks`, primed by `set_pacing`), a 3-iteration
fixed-point solve of the time-to-arrival against the moving centre; a departure teleporting
through an owned gate leads by the undock alone. AI travel/ETA heuristics treat moving subs
by their current position — an accepted approximation at sane orbital speeds (≤ ~13 % of
flight speed in the shipped content).

**L7 "Far far away", reworked** (placeholder briefing; diegetic — no layer/lens mention
anywhere): **both structs UNNAMED**, and the camera opens **inside** the contested struct
(`StartView::Layer1` on a multi-struct map — the off-screen arrows pile up at the frame edge
and wheeling out past the reserve is the discovery; realizes the "Layer-2 revelation" pattern
from the LEVELS.md plan).
- **The contested struct:** four 90-cap/3-prod subs at the cardinals (R = 42) orbiting the
  centre **clockwise** (τ/1500 per reference tick ≈ 12.5 % of flight speed tangentially) —
  west = Player (90 ships), east = Simple (90 ships), north/south neutral; a **neutral
  shipyard** dead centre (the 10 800 activation grind); three **fortresses** on the inner
  ring (R = 14, 120° apart) orbiting **counter-clockwise, slower** (τ/3000), owned by a
  third **Passive seat hostile to everyone** (`enemies = [SimpleColonize, Passive]`), manned
  60 each — their ~21.7 reach keeps the yard in a kill zone from every bearing while staying
  clear of the cardinal ring (42 − 14 − 21.7 ≈ 6 wu of margin). Victory needs both rival
  seats down; the fort seat produces nothing, so killing its garrisons suffices (the
  production-0 elimination rule).
- **The enemy struct**, far down the one lane: a **single active shipyard** (40 pooled) — the
  source Simple funnels to the front; the stream stops only when it falls. Horizon 4800.
- The levels metadata gate now allows `StartView::Layer1` through L7.

**Enemy AI visibility (owner QoL):** the level-select menu shows every mission's enemy roster
tag unconditionally (it was gated behind "briefing received"; locked rows still hide the
title).

---

## tutorial — Space toggle fix + click-snap to subs (2026-07-06)

- **Space toggles 1x ⇄ the last non-1x stop** (owner bugfix: it used to be one-way — on 1x it
  did nothing). New `Game::speed_alt_idx` remembers the last non-1x stop set by the slider or
  the speed keys (defaults 3x; seeded from the persisted pref when it is faster than 1x).
  While paused, Space just resumes at the current stop.
- **Click-snap** (owner QoL): a click within **1.5× a sub's radius** targets that sub —
  selection and sends alike (`sub_at_screen`). Real subs always beat the reserve node, which
  keeps its exact radius and only claims clicks no real sub wants — a near-miss of a small sub
  no longer dumps the wave into struct storage.
- Follow-up to the ring restyle below, screenshot-verified this pass: rings at 1.36×/1.72×
  (2× the separation), colours owner → lightened owner → white; teleport trail alpha halved
  (0.55 → 0.28).

---

## tutorial — teleporter presence + teleport flash; pause rework (2026-07-06)

Owner changes after approving Head of the Snake as-is:

- **Teleporters read bigger**: the two inner rings moved OUTSIDE the garrison orbit (1.18× /
  1.36× the body radius; the orbit band tops out at ~0.85×), so the innermost drawn ring now
  encloses the orbiting ship circle — the gate swallows its parade. Pure rendering; sim
  footprint/selection unchanged.
- **Teleport flash**: a transient white line from the gate to the arrival point
  (`TELEPORT_FX_TTL = 0.4 s`, wall-clock fade). Fed by a new sim-side per-tick event list
  (`Interior::teleport_events`, `(from, to)` pairs — transient render support like
  `combat_engaged`: cleared each step, never hashed; headless hosts ignore it), drained by the
  game after **every** tick so no jump is missed on multi-tick frames. Capped at 512 like the
  kill flashes. (Code-verified + suites; the visible line still wants a hands-on look — the
  headless proxies never fire an owned gate.)
- **Pause rework** (owner spec): the **0× stop is gone** — `SPEED_STEPS = [1, 3, 10, 25]`.
  Pausing is a separate **overlay pause** on `P` (rebindable action, default unchanged):
  darkened board + "Game paused", **no orders or selection accepted**, camera fully live
  (drag / pan / zoom / sliders), sim frozen (`speed()` reports 0), the slider keeps its stop
  for the resume. The fixed **`Space`** key now snaps to the 1× stop (and lifts the pause)
  instead of hard-pausing. `resume_idx` deleted. The Esc pause-menu is unchanged.
- **Pref migration**: `mi_controls.cfg` now persists the speed **multiplier** (1/3/10/25)
  instead of a slider index — old index-valued files fall back to the 1× default instead of
  landing one stop off. The transient pause is never persisted.
- Docs: GAME.md transport/controls sections refreshed (including the stale "match starts
  paused" note — the briefing lives on the level-select screen; the enemy grace period is the
  reading beat).

---

## tutorial — Cycler update: committed sieges (2026-07-06)

Owner rule: the Cycler's units standing on ground it does not own fight their own war.

- Per decision, at each non-owned sub holding its units: compare its force there (present +
  inbound) against the enemy's (present + inbound). **Outnumbering ⇒ hold** (the capture
  continues) and those units are **excluded** from the cycling drill, the gather pool, the
  overwhelm count M, and the defence pull; **outnumbered ⇒ the landed units retreat to the
  nearest owned sub** (Euclidean, ties → lower id) and rejoin the loop. Ties hold; in-flight
  ships re-evaluate after landing.
- The same rule governs the endgame reserve assault: once the strike lands on the remnant, the
  landed units are committed — this also removes a latent oscillation where the next decision's
  gather would have pulled them straight back out of the fight.
- Defence and the gather now move the **pool** (everything not committed); the launch
  threshold (≥ 90 % gathered) and `max(3F, F+60)` also measure the pool.

---

## tutorial — "Command and Control" re-authored: the Cycler enemy (2026-07-06)

Owner design session replacing the first-draft L2 (Passive keep). The mission is now a duel
against a **custom scripted enemy** — `Roster::Cycler`, the stateful `ai::cycler::
CyclerController` (third `SeatController` variant, same roster→brain dispatch everywhere).

- **Layout:** two Player 60-cap/2-prod subs west (60 ships each) vs two enemy 60-cap/2-prod
  subs east (50 each), a moderate gap (±28 x, ±14 y); the ownerless reserve at the default
  dial. Layer-1 only. Placeholder briefing (owner writes copy).
- **The Cycler, per decision tick** (readable + telegraphed by design):
  1. *Defence preempts everything:* if a sub of its is attacked (foe ships idle on it or in
     transit toward it), it masses ALL its idle ships everywhere onto the **most-attacked**
     sub (ties → lower id) and aborts any strike prep. In-flight ships complete their hop
     (engine: no mid-flight redirect).
  2. *Overwhelm strike:* in peace it compares its **total** M against each foe sub's
     defenders **F = present + inbound at that target** (owner clarification: never the
     could-be-reinforcements), fires when `M ≥ max(3·F, F + 60)` (owner formula): gathers
     everything at its fullest sub — the visible tell — and, at ≥ 90 % gathered
     (`GATHER_LAUNCH_FRAC`; production strays make literal 100 % unreachable), launches
     all-in at a qualifying target picked by a pure state hash (no RNG drawn — the ai crate
     stays a pure function of observed state). If the tell was answered (nothing still
     qualifies), it stands down.
  3. *Cycling drill:* otherwise each sub's idle surplus above storage capacity shuttles to
     the next owned sub. **Intended:** the in-transit column dodges per-sub idle attrition,
     so it outgrows parked garrisons — the mission's built-in clock against turtling.
- **The reserve blind spot (intended):** foe ships staged in or flying to the struct-storage
  node count as nothing — not attackers, not defenders — until **no foe sub remains** (then
  the reserve remnant becomes the last target, so the board resolves). Staging the real army
  in the reserve both hides it from F and baits the overwhelm strike into an ambush — the
  mission's core lesson, with the off-screen arrows pointing at it.
- Suites green (ai, levels — the determinism gate drives the Cycler through the shared
  dispatch); selftest det ×13.

---

## tutorial — balance checks removed from tests (2026-07-06)

Owner rule: **all balancing is done per-level, by hand** — so no test measures balance. The
levels validation is now determinism-only (the one gate a level must pass; the entry below
quoting winnability tallies is the historical record of the sweep's last run):

- Removed from `levels::validation`: `not_auto_lost` + both winnability proxies (the Layer-1
  concentration proxy, the Layer-2 greedy baseline), the dormant automata-track lesson checks
  (`counter_beats_enemy`, `seam_flank_beats_greedy`), the `#[ignore]`d
  `print_validation_report` tool, `validate_campaign`/`validate_level`, the
  `lesson_ok`/`lesson_detail` report fields, and `Level::primary_enemy` (only the proxies used
  it). `VALIDATION_SEEDS` went with them (the determinism check uses its own fixed seeds).
- `LevelReport` is now `{id, title, deterministic}`; `validate_level_gates` is the only
  validation entry point (re-exported in place of the removed pair).
- The AI crate's canaries are untouched — they pin the *brain's* ability to resolve a game on
  synthetic boards (engine/AI capability), not any level's difficulty.
- Docs refreshed: `LEVELS.md` validation section, `levels` module docs (which still carried the
  pre-rework 10-level curriculum list — replaced with a pointer to the campaign table).

---

## tutorial — two new missions: "Command and Control" (L2) + "Head of the Snake" (L5) (2026-07-06)

The campaign grows 11 → **13 levels**; every id after L1 shifts (hand-authored builders renamed
to descriptive names; the L7-L13 placeholders keep their slated-for-retirement status). Briefing
copy on both new missions is **PLACEHOLDER** — the owner writes the final blurb/objective/hints.

- **L2 "Command and Control"** (between First steps and Fire in the sky) — the fleet-command
  tutorial: vs a **Passive** garrison too strong for any single sub's wave (keep 120-cap/3-prod
  with 200 ships + two 60/1 flank posts at 30), behind a brisk mid-field (three neutral 60/2
  posts at reduced 1800 resistance). The lesson is the command vocabulary — ctrl+click
  multi-selection, the send fraction, the reserve ring as mustering ground — massing before
  the combat mission asks for it under pressure. Horizon 2400. `[Passive]`.
- **L5 "Head of the Snake"** (between The Sinews of War and Deliberation) — the mobility
  mission from the LEVELS.md Arc-1 plan: an impregnable four-fort wall (spacing 20, zones
  overlapping, manned 60 each — Simple's doctrine tops them toward 90), frontally hopeless and
  geometrically unflankable within the sub graph; a **neutral teleporter gate** on the player's
  side dissolves it (owned-gate departures arrive at undock-end — the priced crossing never
  happens); behind the line the enemy's **active shipyard** (40 pooled) + heartland feed the
  wall — the gate-strike decapitates the economy (an active yard keeps a token bar), flips its
  8-prod to the player inside enemy ground, and the starving wall is dismantled last. Reserve
  at the 0.6× dial, horizon 4800. `[SimpleColonize]`.
- Validation: all 13 levels pass the determinism gate. Informational winnability sweep
  (`print_validation_report`, 5 seeds): L2 **5-0**; L5 **3-2** — the gate-blind concentration
  proxy wins the majority by brute massing, so a human with the gate has real margin. FLAG for
  the owner: L4 Sinews reads **0-5** under the proxy — most plausibly the 2026-07-05 hand
  rebalance (middle fort now starts at capacity 90, was 10), which post-dates the last recorded
  sweep; the fort-heavier L5 still proxy-wins under v3, so this is not a global v3 attacking
  nerf. Not gated (lessons are parked); owner judges human balance.

---

## tutorial — orbit model v3 BUILT: separation engine + damped seek + engaged leash (2026-07-06)

The session objective green-lit the v3 proposal below; built with **one correction the arena
probe forced**. The proposal claimed capped nearest-neighbour repulsion achieves rarefaction at
the geometric bound — it does not: symmetric pair repulsion is gradient-driven, so a uniform
clump's interior feels zero net force and dispersal stayed O(N²) edge-diffusion (measured: two
staged 300-ship clumps relaxed from 0.2 rad to only ~1 rad of arc, then stalled). The built
engine adds the missing **far-field term**; everything else is the proposal as written.

- **The speed law, structural**: every angular urge is computed in world arc-units at the
  ship's own ring radius, summed, clamped to ±`ship_speed` on top of the shared spin. Idle
  angular motion can never outpace real flight, however big the ring. `enemy_seek_rate`
  (rad/tick — the exact violation class) is **deleted**; seek is `SimParams::seek_speed_frac`
  (dimensionless, 0.4, no GUI rescale).
- **Friendly separation, two terms** (always on): near-field pair repulsion under the comfort
  spacing `SEP_COMFORT = 2.0` (ramps to full flight speed at gap 0 — pair spacing, front
  packing); far-field **density pressure** — same-faction count imbalance across
  ±`SEP_PRESSURE_SPAN = 24` arc windows pushes a ship away from the heavier side (binary
  searches over the per-faction sorted bearing lists). The pressure term is what rarefies a
  dense clump's interior at flight speed: O(min(N·d*, πr)/v), the geometric bound. Interior of
  any uniform ring: balanced, zero; windows overlapping (small rings): zero.
- **Damped seek** (0.4 × flight speed) for not-engaged ships with staged foes, never
  overshooting the bearing; **engaged range-leash** — an engaged ship (`combat_engaged`,
  one-tick lag) drops seek, keeps separating, and clamps any step past
  `ENGAGED_LEASH_FRAC (0.9) ×` engagement radius of arc from its nearest staged foe (the
  sub-unity fraction is the hysteresis against engage/disengage vibration). Adjacent-midpoint
  relaxation survives as the peacetime polisher (now a world-unit urge under the same clamp).
- **Churn kick tuned down** 0.05 → 0.03 (`gui_params`) — the separation engine does the mixing
  now.
- **Validated by the screenshot loop + numbers** (owner rule: no mechanism micro-tests):
  `--shot` gained **`--dump <csv>`** (the focused struct's raw ship table at the captured
  tick). Probe + arena shots: clumps rarefy to half the ring in ~2500 GUI ticks (the
  flight-speed bound), approach stays spread and thickens at the stall, combat opens on TWO
  fronts with dense-middle/frayed-edge profiles, radial band uniform throughout. A false alarm
  worth recording: the "sub-band ship lines" in early shots were the **off-screen presence
  arrows** of the just-off-frame cloud — the dump settled it; sim state was clean.
- **Perf**: worst-case 10.3k-ship row 1.82 → 2.42 ms/tick (the orbit pass carries the new
  binary searches) — still far inside the documented envelope; next perf tier only if a real
  board demands it.
- All suites green; selftest determinism re-verified alongside the missions entry above.

---

## tutorial — slot parade REVERTED; orbit model v3 in design (2026-07-06)

Owner verdict on the slot parade: line-shaped dispersal, and ships visibly violating orbital
mechanics — the parade cap was in RADIANS/tick, so a 91-unit reserve ring dispersed at ~10×
flight speed. Reverted to the original spin + adjacent relaxation (span-8 experiment dropped
too); engaged-hold, radial churn, and the arena/--zoom/--pan tools remain. `parade_rate`
deleted.

Owner's ground-up spec for v3 (design notes; not yet built):
1. **Transition** peace→combat: defenders stay spread at first, group progressively toward
   denser enemy areas.
2. **In combat**: fighting dispersed across ≥ one weapons range of arc (bigger for large
   fights); an engaged ship tries to DISPERSE FROM FRIENDLIES while keeping a foe in range.
3. **Post-combat**: relaxation ~O(log N)…O(N); never O(N²).
4. **Speed law**: ship speed within [orbit speed ± travel speed] — caps in WORLD units.

Proposed v3 mechanism (presented, **awaiting owner approval — do not build yet**). All angular
urges computed in world arc-units at the ship's actual radius, summed, clamped to ±ship_speed
on top of the shared orbit (the speed law, structurally enforced). One engine: **friendly
separation** — repulsion from same-faction angular neighbours when closer than a comfort
spacing d*, ramping to full flight speed as the gap closes. Then: not-engaged ships with
staged foes get a **damped seek** (~0.4× flight speed toward nearest foe bearing; separation
still acts, so approaching defenders stay spread and thicken progressively where fights stall
them — phase 1). **Engaged** ships drop seek, keep separating from friendlies, with a range
**leash**: a separation step that would lose contact with every foe clamps at the range
boundary (phase 2; front width ≥ weapons range emerges from packing at combat spacing while
staying in range; expect to need a small hysteresis band against engage/disengage edge
vibration). Existing gentle adjacent relaxation stays as the peacetime cosmetic polisher;
radial churn stays (kick to be tuned down, 0.05 → ~0.03).

Complexity, corrected after owner challenge ("why would distance grow with N?"): the geometric
lower bound is O(min(N·d*, πr)/v) — the dispersed footprint is N·d* of arc, so escaping a
DENSE clump means edge ships travel ~N·d*/2 (linear in N; why O(log N) is impossible under a
hard speed cap), but the bound **saturates at half the circumference** once N·d* ≥ 2πr
(constant in N; ~80 s at 1× on the reserve). And phase 2 keeps combat spacing near d*, so
post-combat clumps are rarely dense — usual relaxation is far below worst case. The old O(N²)
was never distance-limited: midpoint relaxation is diffusion (interior net force ≈ 0); capped
repulsion fixes the dynamics (gas-into-vacuum rarefaction, edge at full speed) and achieves
the geometric bound.

Validation: arena screenshot loop only (`--arena --shot --zoom --pan --at-tick`) — transition
shot, mid-fight cloud, post-combat time series. No mechanism micro-tests (owner rule).

---

## tutorial — the BALLISTIC SLOT PARADE (2026-07-06)

The owner asked for the asymptotics and the math condemned the model: neighbour-midpoint
relaxation is rank-space DIFFUSION — a post-battle clump of N survivors relaxes in
`T ≈ N²/(2π²·(1+m)·orbit_relax)` ≈ 1.35·N² ticks at the live operating point (300 survivors
= ~34 min at 1×; the interior of a uniform clump feels ZERO imbalance, so only edges peel).
Same lesson as the radial fix: capped local diffusion cannot cross a macroscopic distance.

- **Peaceful rings** (fewer than two staged real seats) now run the **ballistic slot
  parade**: every idle ship steps — capped at the new `SimParams::parade_rate` (τ/40/tick,
  GUI-scaled) — straight toward its equal-spacing slot (circle cut at the largest angular
  gap, slot fan anchored on the median ship, so clumps fan out symmetrically and an
  already-uniform ring computes zero steps). Convergence **O(τ/rate) ≈ seconds, independent
  of N**; equilibrium identical to what relaxation converged to. The span-8 relaxation term
  (a 9× constant on an unfixable exponent) is deleted; `orbit_relax` survives only as the
  contested engaged-hold's local fan-out.
- **Contested rings** keep seek / engaged-hold (a full-ring slot fan would rip engaged ships
  off the front; battles are O(log N) by the owner's own square-law argument).
- **`--arena` dev scenario** (`--arena --shot --at-tick N --zoom/--pan`): two dense 300-ship
  clumps staged on a reserve ring, anchor garrisons so the match never seals, no AI — the
  reproducible screenshot loop for combat aesthetics. Verified by pixels: battle at ~2000,
  and by tick 5000 the 82 survivors are already redispersed around the whole ring (was: a
  line for half an hour).
- All suites green, `--selftest` det=true ×11.

---

## tutorial — aesthetics validate by SCREENSHOT, not by micro-test (2026-07-06)

Owner verdict after the cloud-model report: mechanism micro-tests (single-tick angle deltas,
band-clamp checks) pass while the emergent picture fails — they test the wrong layer. All
three orbit-aesthetic tests RETIRED (`seeker_holds_once_engaged`,
`ring_jitter_churns…`, `reserve_ring_parades…`); the behavioural rules live in code + docs,
and the validation instrument is now the screenshot: **`--shot` gained `--zoom <f>` and
`--pan <x,y>`** so any board region is capturable headlessly at any scale.

First use of the instrument: current-code L1 @tick 30 000, reserve wide + close — the band
disperses as a proper scatter (no lines, no combs). The owner's line/comb screenshots do not
reproduce from this build; likeliest cause is a stale running binary (sessions survive many
rebuilds — the recurring exe-lock), with the supply CONVOY (in-flight diverted production,
untouched by idle churn by design) as the second suspect if it persists after a relaunch.

---

## tutorial — the 3-hour production-death bug (2026-07-06)

Reported: L1's Passive centre stopped producing after ~3 h. Root cause: the per-sub spawn
cap counted **lifetime** spawns — `home == sub` including CORPSES (dead ships are never
removed from the roster) — so any long-lived producer went silent at `max_ships_per_sub`
(4000) spawns. The arithmetic matches the report exactly: prod 3 at the GUI period = one
spawn / 2.4 s ⇒ 4000 spawns ≈ 2.7 h. (The full-reserve correlation was coincidental timing.)

- **The cap now bounds the LIVING homed population** (its documented intent was a runaway-
  snowball guard; corpses were never the point). Pinned by `production_survives_corpse_churn`.
- **Automatic corpse compaction**: `step` now drops dead ships once they dominate a large
  roster (every 256 ticks, >2048 entries, >half dead) — long sessions no longer accumulate
  tens of thousands of dead entries that every O(N) pass walks (a silent long-session perf
  drain). Deterministic (pure-state trigger; replays compact at identical ticks). The old
  never-called opt-in `compact_dead` host API is now the forced-compaction primitive under
  the policy, and clears the per-ship transient caches; the GUI kill-FX diff gained the
  index guard a shrinking roster requires. Pinned by `dead_ships_are_compacted_once_they_dominate`.

---

## tutorial — reserve combat v2: engaged-hold, ballistic churn, bulk dispersal (2026-07-06)

Owner review of the first clouds pass found two failures: attackers held a polite GAP off the
enemy line (the arc-distance hold stopped them between hold range and weapons range), and
post-combat agglomerations dispersed painfully slowly (adjacent-neighbour relaxation is edge-
only diffusion). Design discussion first, then the agreed equilibrium:

- **Seek-until-ENGAGED** (replaces `seek_hold_arc`, deleted days after adding it — the wrong
  abstraction): a ship holds only when a foe sat inside its OWN weapons reach last combat
  phase (new transient `combat_engaged` flag recorded by both combat paths; one-tick lag).
  Ships press to actual firing contact — no gap — then parade + relax tangentially: dense at
  the middle, fraying at the edges, density rising with battle size (the owner's spec: area
  is band-limited, spreading is angular, and battles last O(log N) anyway).
- **Ring churn v2 — ballistic, capped** (owner motion rules): new `Ship::ring_drift` (hashed)
  radial velocity under small random kicks (`ring_jitter_step` = per-tick kick as a fraction
  of the speed cap; GUI 0.05, reference 0 = no RNG), speed-capped at
  `RADIAL_DRIFT_SPEED_FRAC` (0.5) × `ship_speed` (idle drift never outpaces real flight,
  however big the ring; also ≤ band/10 per tick), soft-bouncing at the ±band edges. A capped
  random walk on POSITION could never cross the reserve band; capped ballistic drift crosses
  it in ~15 s at 1x.
- **Bulk clump dispersal**: a second, long-range relaxation term (`ORBIT_RELAX_SPAN` = ±8th
  neighbours' midpoint, per-slot normalised, zero at uniform spacing) lets a dense clump's
  INTERIOR feel clump-scale imbalance — agglomerations spread in bulk, not edge-first.
- Tests re-pinned: `seeker_holds_once_engaged` (on-ring fixture — plain `park_ship` stacks
  ships at the sub centre where everyone reads as engaged), churn band/clamp. All suites
  green, selftest det ×11.
- **Repo hygiene**: an agent-side PowerShell Get/Set-Content round-trip mojibaked the UTF-8
  in `sim_tests.rs`/`world_tests.rs` (committed form of the latter in 23a637c is affected);
  both repaired via a cp1252 round-trip reversal. File edits stay on the harness tools now.

---

## tutorial — reserve-combat aesthetics: clouds, not lines (2026-07-06)

Owner tuning of contested-ring behaviour (both effects emergent from two dials, no scripted
formations):

- **Seek-hold** (`SimParams::seek_hold_arc`, default 2× engagement radius = 7.0, scale-free):
  a seeker whose nearest foe is already within that ring-arc distance STOPS steering and
  resumes parade behaviour — spin + spacing relaxation over the mixed-faction ring. The old
  radial-line pile-up was the seek working as specified with relaxation globally disabled in
  contested mode (the owner's intuition was right that the neighbour-midpoint term would
  spread the front — it was just switched off); now far ships stream toward contact and near
  ships fan out tangentially: fronts meet as clouds.
- **Ring-band churn** (`SimParams::ring_jitter_step`): each idle ship's radial slot takes a
  uniform ±step random walk clamped to the ±`RING_OFFSET` band. Frozen per-ship jitter could
  strand same-bearing opponents at opposite band edges — a gap the engagement radius cannot
  bridge on the huge reserve ring — in a permanent standoff; churn makes radii cross within
  seconds. (The owner's inverse-proportional-step idea was flagged and adjusted: diffusion
  that slows away from the canonical radius makes ships ACCUMULATE at the extremes; the
  bounded uniform walk keeps the stationary distribution uniform over the band — the same
  look as the spawn jitter.) **Reference default 0.0 — no RNG drawn, headless geometry
  bit-identical; the GUI operating point sets 0.006/tick** (mixes the band in ~20 s at 1x;
  the orbit glide low-passes the per-tick wobble below visibility).
- Tests: `seeker_holds_once_its_foe_is_within_the_hold_arc` (full-rate seek far / parade-only
  near, on a damped 20-ship ring) + `ring_jitter_churns_the_band_and_stays_clamped`.
  All suites green; `--selftest` det=true ×11 with the churn live.

---

## tutorial — main-menu cleanup (2026-07-06)

Owner: the "operator -> programmer -> meta-programmer" tagline removed (poor taste), the
bottom help line removed, and the button block re-centred (~55% height, sized to the full
item count) to fill the reclaimed space. Title + subtitle stay.

---

## tutorial — empty specials don't block the win + M3 wall swap (2026-07-05)

- **Zero-production subs no longer prevent elimination** (owner QoL): a seat with no ships
  whose only holdings are fortresses/teleporters can never rebuild — the match ends. New
  `Interior::productive_sub_count` (+ foreign variant) feed `World::is_eliminated` and the
  outcome's enemies-dead check; the game's early-seal (`seat_finished`) skips production-0
  subs like it already skipped the reserve. The horizon-lead territory score still counts ALL
  owned subs. Seat-neutral: a player reduced to empty forts is likewise dead. Pinned by
  `empty_fortresses_do_not_prevent_elimination` (world).
- **M3 wall swap** (balance): the middle fort starts at full **90**, the two backline forts
  drop to **10 each** (Simple's manning still grows them over time) — the wall is the
  obstacle, the backline is the endgame toll, not a second wall.
- **Post-send deselect window 1 s → 2 s** (`DESELECT_AFTER_SEND_S`).

---

## tutorial — the shipyard virtual cap is a PLANNING number (2026-07-05)

Owner clarification of the vcap semantics: **a shipyard physically behaves as capacity 0** —
per-sub attrition bleeds its garrison like any over-cap surplus (hoarding at the yard costs;
the parade hovers just under the vcap, production out-pacing the gentle bleed). The
`SHIPYARD_VIRTUAL_CAP` (120) survives as a **planning** number only: (a) the production
**auto-divert threshold** (output pools at the yard up to 120, overflow ships to struct
storage), and (b) the capacity **machine intelligences** see (`PositionView::capacity`, e.g.
Simple's consolidation). Attrition (`bleed_sub_surplus`) reads the DECLARED capacity again.
Edge case verified: nothing derives a yard's **resistance** from the 120 — `shipyard()` sets
the explicit activation/token bar (never capacity-derived), Simple prices from
`view.resistance`, and the constructor test pins an active yard's bar at ≤ 1.0 (attractive
targets stay attractive). Yard test re-pinned to hover-under-vcap + bleed-exists + overflow.

---

## tutorial — perf pass 2: the 10k worst case (2026-07-05)

Owner target: 60 fps at 25× with **10 000 units** (mostly reserve stockpile). The perf tool
gained that worst-case row (10k ships, 6k in the reserve): **77.6 ms/tick — orbit 72** (!).
Root cause: the enemy-seek scan built ONE flat bearing list of every staged ship and every
ship looped over it checking `is_foe_of` — a 6 000-ship reserve parade was **36 million**
checks per tick, O(n²).

- **Seek is now per-seat sorted bearing lists + binary search** (the circular nearest of a
  sorted list is a neighbour of the insertion point): O(n log n). One documented behavior
  delta: an exact-distance bearing tie now resolves CCW instead of lowest foe ShipId
  (measure-zero in f32; deterministic either way).
- **Combat candidate cache**: "any foe bit in my scan neighbourhood" computed ONCE per tick
  (positions are frozen across substeps; kills only shrink the foe set — exact), so the 4
  substeps skip certified-peaceful ships.
- **Tally passes** replace the remaining per-sub full-ship scans: `produce` (homed counts +
  per-seat idle counts, with the fresh-spawn `+1` preserved for the divert query),
  `resolve_resistance` (first-seen-order seat counts — the lone-foreign rule resolves
  identically), `resolve_softcap_per_sub`/`bleed` (pre-bucketed per-sub idle rosters in
  ascending-id order — identical Fisher–Yates victims and RNG draws).
- **Orbit sort**: key-extracted `(angle_bits, id)` unstable sort — bit-order-isomorphic for
  the `[0, τ)` angles, reproduces the old stable sort exactly.
- **Result: 77.6 → 1.82 ms/tick at 10.3k ships** (42×); the 2.5k board runs 0.34 ms/tick.
  Honest ledger vs the target: 25× × 1.82 ≈ 45 ms/frame of sim at a TRUE 10k worst case
  (~20 fps); 60 fps at 25× holds to ~3 500 ships, and 10k runs 60 fps at ~10×. Closing the
  last ~4× needs the next tier — sleeping fully-settled parade rings, SIMD/data-oriented
  ship storage, or a frame-budget tick governor — decision deferred until a real board hits it.
- All refactors verified behavior-identical: full suites green, `--selftest` det=true ×11.

---

## tutorial — the sim perf pass + Simple's CONSOLIDATE rule (2026-07-05)

**Perf (profiled, then fixed — 3.5× faster).** 10x/25x lagged at ~1000+ ships. New permanent
diagnostics: `Interior::step_timed` (per-phase wall-clock) + the `#[ignore]`d layer1
`perf_report` tool (Sinews-like 2000-ship board under dial toggles). Findings at ~2500 ships:
**7.9 ms/tick total — combat 5.9, orbit 1.4** — on a board with ZERO fighting; the cost was
structural, driven by attrition's drifters, not battles. Fixes (all behavior-identical —
same targets, same RNG draws; selftest det=true ×11 unchanged):
- **Combat grid**: far-coasting drifters balloon the flat AABB grid to tens of thousands of
  cells, and every cell was cleared every tick → reset now walks an **occupied-cells list**
  (O(occupied)), and the grid never shrinks (no realloc churn).
- **Per-cell faction masks**: each grid cell carries a seat bitmask; a shooter skips any cell
  holding no foe bit — on peaceful boards the 4-substep combat pass collapses to mask reads
  (5.9 → 0.96 ms).
- **Orbit single-pass bucketing**: per-sub idle lists + a compact idle-real snapshot built in
  ONE pass over the ships instead of a full scan per sub (1.4 → 0.89 ms).
- Result: **2.27 ms/tick** at ~2500 ships (was 7.93) → 25x speed costs ~57 ms/frame of sim at
  2500 ships, ~25 ms at 1000. If heavy boards still drag, the remaining suspects are the
  per-substep mask scans and the fixed 25-ticks-per-frame budget (no frame-skip guard yet).

**Simple's CONSOLIDATE rule (owner design).** The observed stalemate: both sides fully
capped ⇒ every OVERWHELM bar unfundable from any single sub ⇒ Simple idles while surplus rots
under per-sub attrition. New phase (2c) in the Layer-1 program: when the planner has NOTHING
to do (no fundable front, no mop-up, no ledger ops, no defensive fire), any owned sub more
than `SimpleParams::consolidate_margin` (20) **over its capacity** ships its whole surplus
(≥ 20) to the **nearest friendly sub** (never the reserve). The mass absorbs local
over-production hop by hop, rides out attrition while in transit, and the moment a front
becomes fundable the planner has an objective and consolidation stops. New defaulted
`PositionView::capacity` signal (`storage_cap_effective` — a yard reports its virtual cap;
a fort its 90, consistent with the fort doctrine). Two planner tests pin fires-when-idle /
silent-while-a-front-is-live. ai 90/0; all suites green; det ×11.

---

## tutorial — fortress doctrine + range 18 + the reserve level-dial (2026-07-03)

Owner tuning pass on the fortress game and M3:

- **Simple's fort doctrine — a fortress's floor IS its capacity**: `spare()` uses
  `fort_capacity` as the floor for owned forts (troops leave only as the over-capacity
  surplus), and **a fortress never evacuates** — under threat it is PINNED (holds its
  extended-range ground, fights, cancels its unsent outbound legs) instead of fleeing. Two new
  planner tests pin both. Net effect: walls stay walls; Simple's offense is funded by
  production, never by milking garrisons.
- **`FORTRESS_RANGE` 12 → 18** (owner: the overwatch zone should be commanding). Everything
  derives from the const (sim reach, spread-grid scan, GUI threat ring, adapters
  `fort_overwatch_reach`); one ai fixture (`fort_world`) rescaled ×1.5 to preserve its
  crossing/flanking semantics.
- **Per-level reserve-radius dial**: `Interior::add_storage_sub_scaled(scale)` (clamped ≥ 1.0 —
  below the clearance solve the reserve garrison would auto-fight the inner subs);
  `add_storage_sub()` = the default `STORAGE_RADIUS_SCALE`.
- **M3 rework**: the wall respaced 14 → 20 apart for the new reach (~21.7; flanks ±38, back
  forts (90, ±26) — still clear of the heartland); reserve at **0.6×** the default scale (the
  ring frames the board on-screen — one dense battlefield). *Balance (amended same-session,
  owner):* only the **middle** wall fort starts enemy, manned with just **10** (was briefly
  all three at 90/90 — too hard); top/bottom are neutral and EMPTY; the backline pair keeps
  90/90. Simple's manning grows the middle fort toward capacity live — the wall thickens as
  the match runs.
- **Space = fixed hard-pause** (0x ⇄ last running speed, shared `toggle_pause` with the
  rebindable pause action). Space is RESERVED like Esc/Enter — removed from the bindable key
  table; dismissing the mission intro now consumes the whole keypress frame (a Space
  dismissal cannot instantly re-pause).
- **Drawcall clamping FIXED**: "geometry() exceeded max drawcall size, clamping" — macroquad's
  batch is 10 000 vertices / 5 000 INDICES per draw call, and the ship mesh flushed at 60 000
  vertices, so past ~830 on-screen idle ships the excess geometry was silently DROPPED (ships
  vanished) with the warning. Both mesh paths (meshed + binned LOD) now chunk on the binding
  budget: `SHIP_MESH_ICAP` 4 500 indices (~750 quads) per flush.
- Verified: zero warnings, 176/0 tests, `--selftest` ALL PASS det=true ×11.

*Amended again — yard overflow, enemy grace, persisted prefs, M3 down to one owned sub:*
- **Shipyard at the virtual cap now OVERFLOWS to struct storage** instead of wasting
  production (owner reversal of the same-day "bleeds away production" rule, framed as the
  better behavior on sight: a full yard keeps producing and banks the overflow — which also
  restores "the yard feeds the funnel" for a Simple-owned yard). Implementation: the yard
  takes the ordinary auto-divert path with `storage_cap_effective()` as its threshold; the
  garrison still settles at exactly 120 idle (overflow rides the pipeline); test re-pinned.
- **Enemy grace period**: the enemy seats hold still for `ENEMY_GRACE_TICKS` (2 s at 1x) at
  match start — the player gets a beat to read the board. Game-loop only (harness/validation
  drive seats directly); selftest stays det=true.
- **Speed + send-fraction persist between matches** as play prefs in `mi_controls.cfg`
  (`speed = <idx>`, `send_fraction = <pct>`; saved on change, loaded into every `Game::new`).
  0x is never persisted — a match can't start pre-paused.
- **M3 balance**: only ONE heartland sub starts enemy-owned (fully stocked, 60 ships); the
  other four are neutral. Enemy opening: 1 sub + thin middle fort (10) + the two 90/90
  backline forts.

*Amended (2026-07-05) — specials stat pass:*
- **Fortress production 0 by default**: already true in `SubStructure::fortress` (the owner
  asked for it as a balance change; the constructor zeroed it all along) — now PINNED in the
  constructor test so it can't regress.
- **Shipyard footprint ×1.3** (`SHIPYARD_RADIUS_MULT`): 30% bigger than a default sub. Real
  sim geometry, not just a look — the garrison ring and production squares ride the radius.
- **M3's yard produces 12** (`with_production(12)` — the sanctioned per-mission dial over the
  default `SHIPYARD_PRODUCTION` 8): the player's entire economy is that one yard.
  *Reverted same-day after playtest:* yard back to the default **8**; backline forts start
  **50/90** (was 90/90 — Simple's manning still tops them toward capacity over time).
- **Phantom production square on fortresses FIXED**: the renderer drew `production.max(1)`
  squares — one phantom square inside every production-0 sub (fortress, teleporter) that the
  sim never spawns from. Production-0 subs now draw none.

*Amended again — four fixes from the first hands-on session:*
- **Reserve "radial line" bug**: the orbit's enemy-seek trigger counted idle foes **inside the
  sub's radius** — but the reserve's radius circle encloses the whole battlefield, so any
  enemy garrison anywhere locked the reserve into permanent seek (all ships converging on one
  bearing, spacing relaxation off; arrivals glided to the ring in "waves" then marched to the
  line). Fix (owner-corrected to the principled rule — a first geometric ring-band fix was
  rejected as unprincipled): the STORAGE node counts only foes **garrisoned on it**
  (`home == the reserve`) — ships orbiting some other sub are that sub's fight, however deep
  inside the reserve's circle they sit. A genuinely contested reserve (enemies staged in it)
  still seeks; a peaceful one parades and spreads. Normal subs keep the disk rule (the
  overlapping-rings clash is intended there). Pinned by
  `reserve_ring_parades_when_foes_are_only_deep_inside_the_disk`.
- **Camera refused to zoom near map edges**: `clamp_pan` bounded the pan by the FITTED frame
  (the tactical cluster) — the reserve ring and map edges were unreachable at high zoom. It
  now bounds by the **content extent** (all subs incl. the storage ring / all lens nodes),
  ±half a view.
- **~2000-ship lag**: the off-screen arrows were ~2000 individual `draw_triangle` calls per
  frame; now batched into one chunked mesh (a handful of draw calls).
- **"Big-pixel ships" when zoomed out**: not Layer 2 bleeding in — the density LOD's blob
  mode, tripping whenever the (off-tactical-view) reserve stockpile entered the cull window
  past 1500 on-screen ships (hence the pan-direction dependence). Threshold raised
  1500 → 6000: with the chunked mesh a few thousand quads is cheap, and blobs are back to
  being an extreme-density fallback only.

---

## tutorial — M3 "The Sinews of War" + the shipyard virtual cap (2026-07-03)

The first new tutorial-arc mission (owner-designed layout), inserted between "Fire in the sky"
and "Deliberation" — the campaign is now **11 levels** (old 3-10 renumbered 4-11; briefings
re-keyed; `spec_for` table shifted; the 10-level gates updated). The full three-arc campaign
plan (player-as-heartless-automaton objective, lore, mission-by-mission Arc 1) is now WRITTEN
INTO `LEVELS.md` — no longer memory-only.

- **Shipyard rework — the virtual cap** (owner decision): a yard's output now **pools at the
  yard** up to the invisible `SHIPYARD_VIRTUAL_CAP` (120) instead of auto-diverting to struct
  storage; at the cap further production is **wasted** (output bleeds, never the garrison —
  `storage_cap_effective()` makes per-sub attrition honour the virtual cap). Rationale: the
  player must never have to recall ships from the distant reserve ring. Declared
  `storage_capacity` stays 0 (radius/resistance/hash untouched); the GUI shows a bare count on
  yards (no misleading "/ 0"). Campaign impact zero until M3 (no prior yards); a Simple-owned
  yard now hoards a 120-ship garrison the planner spends as spare (it no longer streams to the
  funnel). New layer1 test pins hoard-to-cap + no-divert + no-bleed-at-cap.
- **M3 layout** (see LEVELS.md for the full spec): player shipyard (1 ship) + two neutral
  200/1 warehouses (default 12 000 resistance — midgame investment, per the owner) | a
  vertical wall of three mutually covering forts (14 apart, reach ~15.7; middle enemy manned
  40, outer two neutral/claimable) with 60/2 flank posts in the outer forts' dormant zones |
  five asymmetric 60/2 heartland subs (two enemy-owned, 60 ships) + two enemy back forts at
  capacity gating the eastern approach (a fort can never cover the reserve ring itself — the
  ring is derived 2× beyond the farthest sub; they price the approach corridor instead).
- **Structural-spec gate REMOVED** (owner: non-load-bearing tests that mirror the code are a
  pointless waste of worry). `Spec`/`spec_for`/`check_structure` deleted; `LevelReport::ok()`
  is now **determinism only** (a panicking `build` still fails inside the determinism check's
  two builds). The gate's only lifetime catch was today's legitimate M3 insertion.
- Verified: zero warnings; all suites green (layer1 31, levels gates at 11 levels);
  `--selftest` ALL PASS det=true ×11. `--shot` at tick 0 and 6000: Simple secures its side,
  thickens the middle fort 40 → 90/90, takes the flank posts, correctly ignores the
  neutral outer forts (bad grind value — they stay claimable for the player) and the
  warehouses; the idle player proxy pools 113 at the yard. Placeholder briefing/blurb/hints
  authored for the owner to replace.

---

## tutorial — the canon rename: planets are STRUCTS (2026-07-03)

Owner decree: "planet" was a legacy artefact — the canonical terms are **structs and subs**
(structures and sub-structures), because the lore and canonical scale of the game may change.
Pure rename, ZERO behavior change (all suites green, `--selftest` det=true ×10 after).

- **`layer1::Structure` → `layer1::Interior`** (a struct's internal sim: subs + ships + combat).
  `SubStructure` unchanged. Renamed FIRST so the world-level type could take the canon name.
- **`world::Planet` → `world::Structure`** (`struct` is a Rust keyword, so the type carries the
  long form), field **`.structure` → `.interior`** (`Structure { interior: Interior, pos, name }`).
  `PlanetId` → `StructId`, `World::planets` → `structs`, `add_planet` → `add_struct`,
  `PlanetAggregate`/`PlanetOwner` → `StructAggregate`/`StructOwner`,
  `planet_aggregate` → `struct_aggregate`.
- **Every planet-compound identifier followed**: `take_idle_ships_planetwide` → `_structwide`,
  builders `stocked_planet`/`neutral_planet(_res)` → `stocked_struct`/`neutral_struct(_res)`,
  harness `home_planet` → `home_struct`, game `sel_planet(s)`/`planet_at_screen`/`planet_col`/
  `draw_planet_interior` → struct forms, ai `planet_is_sink`/`planet_moves`/`Target.planet` →
  struct forms, projection `planet_capture`/`planet_first_fall` → `struct_capture`/
  `struct_first_fall`, plus all test names. Locals that held a `&Structure` are named
  `structure`; locals/params/fields holding a bare `StructId` are named **`sid`** (never
  `structure` — an id is not the object).
- **Prose + player-visible strings**: "planet(s)" → "struct(s)" in comments, doc-comments, UI
  copy, and the five live root docs (README/GAME/WORLD/LEVELS/LAYER1_SIM). The M1 briefing's
  "planetary" became "structural" (its copy is otherwise hand-authored — flagged for the owner;
  scale-baked words like "interstellar" were left for the owner's pass). `CHANGELOG.md` history
  and `docs/archive/` are records of their time — untouched.
- **Scope**: the five live crates (`layer1`, `world`, `ai`, `levels`, `game`). The deferred
  crates (`cell-core`, `automaton`, `architect`) keep their own vocabulary until revived.

---

## tutorial — the game-scale correction (2026-07-03)

The owner's scale pass: the game felt cramped and lacked the room intended for moving from
tactics to grand strategy. Three coordinated changes:

- **Struct storage radius ×2** — new `layer1::sim::STORAGE_RADIUS_SCALE` (2.0) multiplies the
  reserve node's minimum-clearance solve in `add_storage_sub`. The clearance guarantee (ring
  clears every inner garrison by engagement radius + buffer) still holds — the scale inflates
  *beyond* it, so the inter-planet entry/exit ring is a genuinely outer orbit far from the
  tactical cluster. Applies to every structure (authored, builders, harness fixtures).
- **Ship visuals ×0.5** (GUI-only) — interior idle dot 2.4 → 1.2 px, moving-triangle
  nose/back 6.0/3.5 → 3.0/1.75 px (both the meshed interior path and the shared
  `draw_ship_triangle` the world-layer fleet flocks use). Density-LOD blobs unchanged
  (they encode count, not ship size).
- **Authored sub spacing ×2** — L1 corner offset 20 → 40; L2 middle square ±5.5 → ±11 and
  homes ±24 → ±48; L3 all coordinates doubled (chain, branches, B, C); the L4-L10 placeholder
  builders' ring radius 9 → 18 (`stocked_planet` / `neutral_planet`). Net effect on authored
  reserve rings: enclosure ×~2 · scale ×2 ⇒ ~3.5-4× today's radius.
- **Interior camera fits the tactical cluster, not the reserve** — `interior_camera` now skips
  the storage sub (fitting the huge ring opened every planet as an unreadable central blob);
  the ring sits off-screen at the default fit. `ZOOM_MIN` 0.5 → 0.2 so zooming out still brings
  the whole reserve ring on screen (~0.3× needed at this scale).
- Knock-on: matches run longer in wall-clock terms (travel dominates more) — L7's selftest no
  longer seals by tick 465 (capped instead, still PASS det=true). The dead L2 flashpoint
  comment is finally corrected (the ~11-apart posts stopped trading fire when the engagement
  radius halved to 3.5; its combat lesson is re-authored with the tutorial arc).
- Verified: zero warnings; ai 86 + layer1 + world + levels all green; `--selftest` ALL PASS
  det=true ×10; L1-L3 opening frames inspected via `--shot` (cluster readable, ring as a
  depth cue at the frame edge).

*Amended again — selection ergonomics, off-screen presence, automation control removed:*
- **Automation control REMOVED** (owner: an unimplemented mechanic is an empty promise —
  weeks from shipping). No binding, no Settings row (`ACTIONS` 13 → 12; an old
  `toggle_automation` cfg line is ignored harmlessly). The engine plumbing stays behind
  `automation_available = false` (incl. the synthetic selftest); re-add an `Action` when the
  redesigned mechanic ships.
- **Send keeps the selection for 1 s, then auto-deselects** (`DESELECT_AFTER_SEND_S`,
  wall-clock; each send re-arms it). Single AND multi-select: a multi-selection now survives
  its send (it used to clear immediately) — supports "50% default, multi-click to send more" —
  while the expiry stops the classic mis-click (send, then try to select another sub, and ship
  ships at it instead). Any deliberate selection/box/clear cancels the pending window.
- **Ctrl+left-click adds to the selection** (toggles: ctrl-clicking a selected member removes
  it; merges an active single-select into the multi list; ctrl+click on a non-commandable
  target leaves the selection untouched). Works on both layers (subs / planets).
- **Off-screen presence arrows**: the focused planet's off-screen ships (reserve-staged,
  mid-transit beyond the frame) render as border arrows pointing at them — ship-coloured,
  `SHIP_ALPHA` 0.6, ship-sized at near-visibility shrinking linearly to 1/3 at one full
  struct-storage radius past the edge, sliding along the border as the ship moves. Ships on
  other planets show NOTHING (the interior only draws the focused planet). Lens twin: fully
  off-screen planet nodes get owner-coloured edge arrows (0.6 × node size, same distance rule
  with the planet's own storage radius as yardstick). Shared helpers `draw_arrow_px` /
  `edge_anchor` / `arrow_rect`.
- Verified: zero warnings, selftest ALL PASS det ×10; L2 @3000 shot shows reserve-overflow
  arrows on the border. Selection timing + ctrl-click + arrow feel need the interactive pass.

*Amended again — camera navigation + rebindable controls + two visual fixes:*
- **Cursor-anchored zoom**: the wheel now zooms toward the mouse (the world point under the
  cursor stays put; a new per-layer camera **pan** absorbs the correction). New `Camera::to_world`
  inverse; `Game::pan[(lens, interior)]` sits on top of the fitted centre; `clamp_pan` bounds it
  (≤ one fitted frame + one view beyond centre) so the board can never be lost. Interior pan
  resets when the focus planet changes.
- **WASD pan** (held; `PAN_SPEED_PX` 700 px/s ÷ camera scale = same feel at any zoom) and
  **right-drag grab-pan** (the pressed world point follows the cursor; a plain right-click still
  clears the selection — resolved on release past the box-drag threshold, like left click-vs-box).
- **Rebindable controls**: 13 actions (`ACTIONS`) in `mi_controls.cfg` next to the exe
  (hand-editable `action = key` lines; unknown lines ignored), live in a `BINDS` thread-local.
  New **Settings** page on the main menu (row per action, click/Enter → press-a-key capture,
  Esc cancels; a taken key SWAPS bindings so every action stays bound; "Reset to defaults" row;
  `--screen settings` shot target). Mouse buttons, Esc, Enter/Space, and menu navigation stay
  FIXED (rebinding those could lock the player out). **Binding changes**: automation toggle
  moved `A` → `T` (A is now pan-left); the `[`/`]` and numpad speed-key alternates are gone
  (speed = `Minus`/`Equal`, rebindable).
- **Visual fix — the ring-fraction lie**: the interior sub body had an 8-px minimum draw radius,
  so at low zoom the circle was inflated while ships orbited at their true sim positions — the
  garrison ring appeared at a zoom-dependent fraction of the body. The floor is removed (the
  drawn circle is world-true at every zoom); ship POSITIONS were never affected (sim truth,
  WYSIWYG). The invisible 10-px click-target floor in `sub_at_screen` stays.
- **Ship translucency**: interior ships render at `SHIP_ALPHA` 0.6 so overlap stacks toward
  opaque — density reads as a gradient instead of flat glyph soup.
- Verified: zero warnings; all suites green; `--selftest` ALL PASS det ×10; settings screen +
  tick-1500 interior inspected via `--shot`. Wheel/WASD/right-drag feel needs an interactive pass.

*Amended same-session — ships are now **world-sized** and the wheel zooms:*
- **Apparent ship size scales with zoom** — ship visuals converted from fixed screen-px to
  world units (`SHIP_NOSE_WU` 0.40 / `SHIP_BACK_WU` 0.235 / `SHIP_DOT_R_WU` 0.15, sized to
  reproduce the old look at the default interior fit) × the camera's px-per-world-unit scale,
  floored at `SHIP_MIN_NOSE_PX` (1 px; dots at half) so extreme zoom-out stays faintly visible.
  Applies to the interior mesh AND the lens fleet flocks (`draw_ship_triangle` /
  `draw_fleet_cluster` take the camera scale; the flock's formation pitch is world-sized too,
  0.8 wu, so the whole formation scales coherently).
- **Mouse wheel = continuous zoom** on the current layer (the same per-layer value the
  right-side slider drives; `WHEEL_ZOOM_STEP` 1.15/notch, clamped to `ZOOM_MIN..ZOOM_MAX`),
  with the old layer transitions preserved at the ends: lens wheel-up over a planet still
  enters it (with no planet hovered it zooms the lens instead of jumping into the focus);
  interior wheel-down at `ZOOM_MIN` leaves to the lens (multi-planet maps). The wheel now
  works on single-planet missions too (it used to do nothing there). Zoom stays
  centre-anchored like the slider (no cursor-anchored zoom / pan yet).
- Verified: workspace check zero warnings; L2 interior + L4 lens-with-fleets frames via
  `--shot` (garrison rings and flock labels read correctly). Wheel behaviour needs the next
  interactive session (not `--shot`-checkable).

---

## feat/counter — Simple learns the special subs (2026-06-12)

The owner's design for Simple vs fortresses / teleporters / shipyards. All policy lives behind
five new **defaulted `PositionView` signals** (the view does the geometry, the planner only
reads): `overwatch_toll(from, to)`, `fort_coverage(id)`, `gate_savings(id)`,
`fort_capacity(id)`, `via_gate(from, to)`. Inert on special-free maps — campaign behavior is
bit-identical (`--selftest` det=true ×10).

- **Fortress cost — the gauntlet toll.** A leg whose straight source→target segment crosses a
  rival fortress's overwatch zone (point-to-segment distance ≤ ring + `FORTRESS_RANGE`) must be
  oversized by `SimpleParams::fort_toll` (1.0) × that fort's **manning ships** — a fixed price
  per crossing leg, charged in `pull()` (phase A rolls back fronts that can't cover base +
  tolls; no half-priced assaults). A leg routed via an owned teleporter walks nothing and pays
  nothing. Otherwise NO special cost treatment (B1: price it right, let OVERWHELM + the
  synchronized stagger do their job).
- **Fortress value — path coverage** (the owner's metric): a fort's capture priority improves
  by `fort_value` (40 travel-ticks) × the fraction of complete-graph sub pairs whose segment
  crosses its zone — how much of the planet's movement space it commands. Static, map-intrinsic,
  readable by a level designer.
- **Manning** — local, per the design: while the planet's ledger holds an active op (an
  expansion ongoing ⇒ not in dire need), every owned fortress is topped up to **capacity**
  *before* the fronts are funded (a fort manned after the conquest is manned too late), its
  garrison becomes wall-duty (not wave spare), immediate moves, no stagger, no toll. No ops ⇒
  floor only, the wall stands down. **No flee-into-the-fort** — regular fleeing only.
- **Teleporter routing is EXECUTED, not just believed**: `travel()` = min(direct, via an owned
  gate), and the commit step turns a routed leg into **two chained ledger legs** (walk to the
  gate, then the instant hop — `Leg` gained its own `tgt`; `committed_in` counts by leg
  destination so a routed op self-throttles once, not twice). Planned times are realised times —
  no stagger desync — and the route charges **both undocks** (the walk-leg's own, then the
  second at the gate; pinned by test). `pull()` already ranked sources by travel, so gate-staged
  ships were automatically preferred. *Owner-flagged toll interaction:* a routed leg waives only
  the gauntlets the teleport skips — the **walk to the gate still pays the tolls on its own
  stretch** (`overwatch_toll(src, gate)`), and multiple crossed forts price **additively**
  (the signal sums every crossing fort; per-leg tolls accumulate).
- **Teleporter value — complete-graph savings** (static, deliberately decoupled from the live
  routing): a gate's capture priority improves by `gate_value` (40 ticks) × the fraction of
  complete-graph travel it would save (`u→v` shortens to `u→gate`; departures hop out
  instantly). Encodes "take the gate first" without any multi-step planning.
- **Shipyards need no policy at all** (confirmed by analysis): `resistance/production` already
  covets them pre-activation (10800/8 beats 3600/1), an active rival yard prices on its garrison
  alone, output auto-feeds the funnel, and the default floor garrison guards an owned yard (the
  slow capacity-0 bleed is cheap insurance vs instant steals — owner's call).
- 9 new tests: 5 planner (manning-before-fronts, manning-needs-an-ongoing-op, toll
  inflates-or-blocks, two-chained-legs with dispatch order, value reorders candidates) + 4
  geometry (toll crossing, coverage fraction, gate savings + owned-only routes, fort_capacity
  ownership). ai 84/84 active (+17 parked/tool ignores); zero warnings.

---

## feat/counter — test hygiene: tools out of the gating suite (2026-06-11)

Report tools were running as tests on every `cargo test`. Three `#[ignore = "report tool — run
with --ignored --nocapture"]` conversions (the parked `automaton` diag/emit tools were already
ignored):
- `ai::pure_strategy_cycle_corridor_report` (30 full matches, zero assertions) — the ai release
  suite drops 16 s → ~3 s.
- `levels::print_validation_report` (re-ran the full validation just to print it).
- The minutes-long informational **lesson sweep** is out of the gated levels test: new
  `validation::validate_level_gates` checks exactly what `LevelReport::ok()` gates (structure +
  determinism); the lib test is renamed `campaign_is_well_formed` and runs in ~10 s (was
  ~7 min). The full sweep (lessons included) lives on in the ignored `print_validation_report`.
No gating coverage changed — only ungated compute moved behind `--ignored`.

*Amended same-session — **greedy is officially PARKED for rework**, and all its tests are
`#[ignore]`d with reasons (17 ignored in `ai`, 1 in `layer1`): the 12 `greedy::tests` unit
tests, the seam exploit (`greedy_seam_thin_rear_is_exploitable` + layer1's
`ai_seam_thin_rear_is_exploitable` — curriculum contracts for the parked L7 lesson: they pinned
a heuristic's *emergent match-scale personality*, which is a content contract, not an algorithm
test, and is void while that content doesn't ship), `greedy_is_sensible_expands_and_beats_passive`,
and `canary_greedy` (resolution stays guarded by `canary_simple` at the live operating point).
Greedy never acts in the shipped human game — campaign seats are Passive + `SimpleController`,
player automation is quarantined — it survives as the `--auto`/selftest player proxy, where the
selftest still sanity-runs it end-to-end on all 10 levels every build.*

---

## feat/counter — Simple's Layer 2: the funneling DAG + the mop-up rule (2026-06-11)

The owner's design for Simple's missing struct-to-struct logistics (replacing both the removed
Layer-1 "stagnation pressure" and the old fully-owned→frontline surplus push — which, it turns
out, was also **blind to reserve-staged ships**: `to_fleet_orders` sized exports against
owned-sub idle only, so a planet whose surplus had auto-diverted into its ownerless reserve read
as having nothing to send).

- **Layer 1 needs no send-to-Layer-2 policy at all**: a planet with no local work lets its
  production overflow auto-divert into struct storage — pooling there *is* the default fate of
  surplus. Two explicit rules complete the picture:
  - **No local target, no enemy ships ⇒ do nothing** (surplus pools).
  - **MOP-UP — enemy ships remain but nothing is capturable ⇒ hunt them**: the nearest holdout
    (including one massing in the shared reserve, which is never a *capture* candidate but is
    now a combat target) is priced at `OVERWHELM(foes)`; when even the whole planet's spare
    cannot fund that bar, Simple **masses everything available and sends it anyway** — one big
    wave is the least inefficient battle the square law allows. Two guards keep it sane: a
    position with no garrison is never marked "fleeing" (an empty reserve under ≥20 foes would
    otherwise veto the mop-up forever), and the mop-up waits out any evacuation in progress
    (consolidate first — never evacuate a position and feed ships back into the same grinder in
    one breath).
- **Layer 2 is a funneling DAG, not demand/supply** (demand fluctuates on Layer-1 timescales;
  Simple carries no optimality requirement): every world that still *needs* ships — any
  not-mine position, or any foe present/incoming — is a **sink**; every undemanding world
  points one hop downhill toward its nearest sink (multi-source BFS over the lane graph,
  ascending-id tie-breaks ⇒ a DAG) and each decision ships **100% of its staged storage** along
  that edge (`FractionBucket::All` + the reserve-first fleet draw = exactly "everything in
  struct storage, garrisons untouched"). Relays pass it on; ships pool in the sink's reserve
  where its Layer-1 planner spends them; worlds with no presence are sinks too, so invasions
  stage the same way. Stateless, deterministic, junk-safe.
- **The Simple resolution canary is un-ignored and green** — it now runs at the live attrition
  model (`per_sub_attrition = true`): under the parked reference's per-structure sqrt cap, a
  one-sub foothold can never stand more ships than a number BELOW Simple's doctrine bar against
  a dug-in garrison, so no logistics design can close that gap there by construction; Simple is
  the live enemy and its canary tests the loop the shipped game runs. Suite: ai 91/91 + 1
  parked ignore; `--selftest` det=true ×10 (L7 seals at 465 — demo balance shifted again with
  the funnel; human balance is the mission redesign's).

---

## feat/counter — resolution canaries + Simple's stagnation pressure (2026-06-11)

A "16-hour test run" scare unwound into three real findings. (The scare itself was an artifact:
the overnight `cargo test -p ai` wall time included machine **suspension** — re-measured at
147 s dev / 13 s release. Wall-clock is therefore never asserted in tests; tick budgets are the
cost proxy.)

- **The suite had no resolution alarm.** Match tests asserted *outcomes* (wins can be by
  lead-at-horizon) and report tests asserted nothing — a regression that stops matches from
  *ending* would pass silently at full-horizon cost. New
  `canary_predators_eliminate_passive_within_budget`: greedy (corridor) and the stateful Simple
  (diamond) must beat a do-nothing `Passive` **by elimination** within ⅔ of the horizon, two
  seeds each. The "matches stop resolving" class now fails loudly.
- **The canary exposed a Simple endgame gap on day one** (present at the old engagement radius
  too): Simple's per-front bar `OVERWHELM(F) = max(ratio·F, F + add)` can sit permanently above
  what the reference soft cap lets it concentrate on one planet, while its simplified Layer-2
  push cannot mass the rest of its (much larger) economy into a decisive wave — a diamond
  endgame vs `Passive` runs to the horizon without elimination (probe: 88 ships / 8 subs
  globally, yet 30 parked beside the last 40 defenders forever). A Layer-1 **stagnation
  pressure** rule (drop the flat margin + waive the floor after a starved-patience) was built,
  proven to resolve the position (200-0 by ~tick 1700) — and **removed the same day, per the
  owner**: that position is out-of-gameplay (a human wins or restarts long before it), and the
  *general* capability gap is **Layer-2 struct-to-struct reinforcement, which is not yet
  designed** — the proper fix. The Simple half of the canary is `#[ignore]`d with that design as
  its stated reason; it is the acceptance test the L2 reinforcement design must pass. The greedy
  half stays active (greedy eliminates passive on the corridor within budget).
- **The reserve-blockade endgame gap is pinned as a test**
  (`reserve_blockade_remnant_endgame_is_pinned`): a beaten remnant ≥ `STORAGE_ENEMY_BLOCK` (20)
  parked in the ownerless reserve blockades the auto-divert and cannot be captured out — the
  map-controlling winner wins on lead, **never by elimination** (measured: remnant of 20
  survives the horizon). The stalemate is now a versioned suite fact with flip-me instructions,
  not folklore; the agreed future fix is an endgame rule (remnant-hunting nudge /
  end-condition), not orbit steering.

---

## feat/counter — combat rework: halved engagement radius + the enemy-seek orbit (2026-06-10)

Attacking was too punishing under the old kill zones: **`DEFAULT_ENGAGEMENT_RADIUS` is halved,
7.0 → 3.5** (both operating points — the constant is spatial, not tick-scaled). Knock-on facts:
a **fortress**'s reach is now a **fixed `FORTRESS_RANGE = 12`** (decoupled from the basic
radius — initially a 2x multiplier, changed to fixed in the same session); **reserve nodes
shrink** (the sizing clearance is now `3.5 + 2`); the AI's `radius + engagement_radius` threat
reads and the kill-tracer reach all follow automatically.

**The enemy-seek orbit** (new `SimParams::enemy_seek_rate`, default `TAU/40` ≈ 5× the orbit
rate; tick-scaled in `gui_params` like `orbit_rate`): at the halved range, idle enemies sharing
a sub could orbit forever just out of reach, staring at each other (already an occasional comedy
in the reserve node; lethal to fun at R = 3.5 where opposite sides of a *default* ring are out
of range). Now, when idle **foes** sit inside a sub's radius (a contested garrison, rival
stockpiles in the shared reserve, an overlapping rival ring), every idle ship on that ring drops
the parade — spin and spacing-relaxation off — and **rotates toward the bearing of its nearest
foe** (shortest arc, capped at the seek rate). The seek steers **bearings only** — radial
positions (the jittered ring slots) are never touched, so on the huge reserve ring (whose ±10%
jitter alone spans more than the new reach) converged ships may still stare at each other out of
reach: an accepted standoff, per the owner. Both sides seek, so opposing clusters wheel around
the ring, meet, and the firefight resolves the standoff everywhere else. Determinism: all from a start-of-tick snapshot,
simultaneous and RNG-free; the equal-spacing relaxation (the ordered-list machinery) and the
seek term never run on the same ship in the same tick, so the parade's ordering assumptions are
untouched. Known limit: two overlapping rings of *very* different radii can stay radially apart
at the same bearing — an authored-map edge, not an orbit bug.

- Mission fallout for the redesign: **L2's flashpoint no longer fires** (its middle posts, ~11
  apart, were authored to trade fire across the gaps at the old reach; at 3.5 they are silent
  neighbours) — re-author with the tutorial arc. `--selftest` stays `det=true` ×10; the `--auto`
  demo balance shifted again (L7 seals at 417).
- Tests: layer1 30/30 (the square-law / capture suites hold — the seek keeps the origin-clash
  brawls engaged); the fortress range test re-anchored to the new radii (camp at 13: the gap sits
  between 3.5 and 12). world 41/41.
- *Amended same-session:* seek mode **keeps the base orbital spin** (both sides share it, so it
  cancels out of the convergence — the clash precesses around the still-turning ring); only the
  spacing relaxation switches off. The original cut ("no spin, no relaxation") parked converged
  clusters in place. *Also amended:* the original cut glided seekers to the **nominal** ring
  (radial jitter ignored) to guarantee convergence on the reserve; the owner prefers the seek to
  steer bearings only, accepting the occasional reserve-ring stare-down.

---

## feat/counter — special sub-structures: fortress / teleporter / shipyard (2026-06-10)

Engine support for the three **special sub kinds** the tutorial-arc redesign builds on (specs by
the owner; missions come next — no campaign level uses them yet). New `layer1::SubKind`
(`Standard` default | `Fortress` | `Teleporter` | `Shipyard { active }`) on every `SubStructure`,
folded into `state_hash`; constructors `SubStructure::fortress/teleporter/shipyard(pos, owner)`;
stats overridable via the usual builders. Special-free maps are behaviour-identical (`--selftest`
det=true ×10; layer1 29/29 incl. 6 new special tests; world 41/41).

- **Fortress** (`SubKind::Fortress`, drawn as a **diamond**): produces nothing; capacity
  `FORTRESS_STORAGE_CAPACITY = 90`, resistance `FORTRESS_RESISTANCE = 10 800`. While owned, the
  owner's **idle ships garrisoned on it fire at the fixed `FORTRESS_RANGE = 12`** (decoupled
  from the basic engagement radius) — a one-sided overwatch ring (enemies between R and 12 are
  shot and cannot answer; range is per-shooter). Both combat paths honour it: classic uses a per-shooter reach; the
  spread grid widens the boosted shooters' cell scan (`±ceil(FORTRESS_RANGE/R)` cells) while normal shooters keep the
  exact 3×3 order. The GUI draws a faint threat-envelope ring while owned. **Kill attribution is
  per-shooter** (new `Structure::ship_engagement_reach`): a death's tracer candidates are every
  enemy whose *own* reach covers the victim — regular ships within R, fortress garrisons within
  `FORTRESS_RANGE` — picked uniformly, so a kill traces to a fortress ship exactly in proportion to the
  fortress's share of the plausible shooters (and never to a far ship outside one). Under
  transit-fire gating, movers are also excluded as tracer sources for stationary victims (they
  could not have fired).
- **Teleporter** (`SubKind::Teleporter`, drawn as **nested circles**): produces nothing; standard
  capacity/resistance (60 / 3600). A departure ordered by **its owner** arrives the instant the
  undock delay burns out — no transit leg (checked at the undock-completion tick with the owner
  at that tick; everyone else's ships leave it as ordinary movers, so capturing the gate matters).
  `Layer1View::transit_ticks` and `World::sub_influx_for`'s friendly ETA both report
  undock-only for owner departures (Simple's synchronized landings stay in sync on teleporter
  maps). The renderer **snaps** (never lerps) ship jumps larger than any legitimate per-tick
  movement, so teleports don't smear across the screen.
- **Shipyard** (`SubKind::Shipyard { active }`): extreme production
  (`SHIPYARD_PRODUCTION = 8`), **zero** storage (its output streams to the reserve via the
  existing auto-divert; parked surplus bleeds), normal-size *invisible* footprint (radius as if
  capacity were 60 — selection + garrison ring only; no disk is drawn). Authored **neutral** it
  carries a one-time `SHIPYARD_INITIAL_RESISTANCE = 10 800` **activation grind**
  (`active: false`); on its **first capture** it activates for good — the bar collapses to a
  **token fraction** of its pre-activation max (`SHIPYARD_ACTIVE_RESISTANCE_FRAC`, = 1.0 at the
  reference scale; a *fraction* so the collapse survives `build_scaled`'s ×24) and the yard
  thereafter flips to any lone visitor almost instantly — high value, trivially stealable.
  Authored owned ⇒ starts active. GUI: pre-activation it renders as a **tight white grid** of
  its 8 future production squares that **progressively segments** toward their ring slots as the
  bar erodes (the animation parameter is the eroded fraction — groundwork for a richer animation
  later); once active, only the standard free-floating production squares render.
- **Not yet AI-aware beyond travel times** (deliberate): Simple prices a pre-active shipyard by
  its big resistance (the existing reads) and under-counts a fortress kill-zone it walks into —
  acceptable while the specials are player-facing tutorial material; revisit with the missions.
- Render note: the three visuals are code-complete but await the first authored mission for an
  interactive look (no current level fields a special).

---

## feat/counter — deep-review fix pass (2026-06-10)

A multi-agent review of the whole codebase (7 reviewers + adversarial verification of every
finding) ahead of the campaign expansion; 90 findings confirmed, the live-path bugs all fixed.
`--selftest` stays `det=true` on all 10 levels (L2/L7 now *seal* as Player wins in `--auto` —
the proxy-strengthening fixes below shifted the demo balance; human balance is retuned at the
mission redesign). One **parked** Counter gate (`gate_attack_is_attack_dominant`) is `#[ignore]`d:
the dynamics shifts below legitimately moved its tuned diamond matches (re-tune on automata
revival).

### Sim / engine fixes (`layer1`, `world`)
- **Production cadence off-by-one.** `produce()` reset its timer to the full interval but the
  reset tick never decremented, so ships spawned every `interval+1` ticks — and since the GUI
  scales the period ×24, per-second production silently differed between the reference and the
  GUI (up to ~16% on `production: 3` subs). Timer now resets to `interval − 1`: spawns land
  exactly `production_period / production` apart at every tick scale.
- **`total_foreign_resistance` no longer counts the reserve node.** The ownerless storage node
  (never capturable, bar never erodes) inflated every "grind to fully own" read by `3600·scale`
  per planet — Simple's wave ceilings were sized against a phantom fortress. Excluded, matching
  `sub_count`. Same family: `Layer1View::resistance` reads `0` on the storage node,
  `Layer2View::min_foothold_resistance` / `cheapest_foothold_and_source` skip it.
- **Reserve ring clearance accounts for `RING_OFFSET`.** The reserve radius was solved against
  the *nominal* garrison ring, but ships sit at `(ring_frac ± 0.1)·radius`, so on standard
  campaign geometry a reserve ship could auto-engage an inner-sub garrison across the boundary —
  exactly what the sizing promises never happens. Now solved against the innermost jittered reach
  (`(ring_frac − RING_OFFSET)` divisor; reserves grow ~15%).
- **Per-sub attrition now reaches the reserve.** `resolve_softcap_per_sub` skipped every
  non-real-owned sub, so the permanently-Neutral reserve was attrition-exempt (its instant-destroy
  branch was dead code and the hard-cap guard unenforced in GUI mode). The storage node now bleeds
  **each seat's** stockpile above its 6000 capacity independently (ascending seat order —
  determinism preserved; unreachable in normal play, it is the pathology guard).
- **`state_hash` extended**: per-sub `storage_capacity` + the structure's `storage_sub`
  designation are folded in (both shape evolution); `World::state_hash` now also folds the lanes
  (lengths drive fleet dynamics) and its doc states what is deliberately excluded (params).
- **`sub_influx_for`**: a friendly mover's ETA now includes its remaining **undock** delay
  (was up to 120 ticks early at the GUI scale, desynchronising Simple's staggered landings —
  `Layer1View::transit_ticks` charges the undock too, mirroring Layer 2); a foe mover already
  **inside its target's engaging reach** is no longer also counted as influx (Simple was
  double-counting adjacent attackers as present *and* incoming); out-of-range planet ids return
  an empty influx instead of panicking (and `entry_sub` bounds-checks both ids).
- **`fleet_arrival_ticks` moved** from the PARKED `projection.rs` into `world/src/lib.rs` next to
  the `step` loop it mirrors — the live `sub_influx_for` calls it every Simple decision, so it no
  longer lives in (and re-implements the lane clamp of) a file marked "not exercised".
- `inject_fleet` lost its unused `SimParams` param; its doc now leads with the reserve-node rule.
- New `Structure::set_pacing(&SimParams)` primes the cached undock/drift pacing; `Game::new`
  calls it so **tick-0 orders** undock at the scaled pace (first wave peeled out 24× too fast).

### AI fixes (`ai`)
- **No more hardcoded seat lists on the live path.** `Layer1View::build` summed threats over
  `[Player, Ai(0), Ai(1)]` — a third AI seat's ships would have been invisible to every other
  seat's threat reads. Now one generic `is_foe_of` pass (`engaging_foes_count`); the parked
  projection branch got the same via `Projection::incoming_present_foes_at`. The game's win
  condition `all_enemies_finished` likewise iterated exactly `Ai(0) && Ai(1)` — now every seat
  the level declares.
- **Simple: contested neutrals are priced as defended.** `minimum()`/`maximum()` ignored
  present+incoming foes on Neutral targets (a camped neutral cost `OVERWHELM(0) = 20`), so Simple
  fed 20-ship waves into rival armies on the L2/L3 races. `minimum = OVERWHELM(foes)` (undefended
  ⇒ unchanged); a neutral's `maximum` also covers `mult·foes`.
- **Simple: staggered legs no longer evaporate.** EXPIRE ran before DISPATCH, so a leg whose
  `[depart_at, land_at)` window fell between two decision ticks (120 ticks apart in the GUI) was
  retained-out before it ever fired — silently cancelling the nearest, bulk-carrying leg of a
  taskforce. An op with unsent legs now survives `land_at`; the legs fire (late) on the next
  decision tick.
- **Greedy: the friendly-reinforce churn is fixed.** The "send everything to any strictly-thinner
  friendly" rule inverted itself every decision, keeping a fully-owned planet's whole garrison
  perpetually in transit (and draining the reserve's staged export stock back into subs — starving
  Layer-2 export). Reinforcement is now **water-levelling**: only into a friendly holding ≤ half
  the source's ships, moving half the gap (converges); and the staging node (new
  `PositionView::is_staging`) is never a reinforce *source* (still a destination / capture-wave /
  assault source).
- **`greedy_layer2_orders` runs at the real operating point**: it took no `SimParams` and baked
  `SimParams::default()` into its view (`Layer2View::new`, now deleted) — benign today, a silent
  TICK_HZ-contract breach the moment any Layer-2 read consults `sp`. Now threaded through
  `decide_projection_free`.
- Dead code removed: `SubPresence` + `sub_presence`/`capture_presence` (uncalled, hardcoded
  3-seat shape), `Faction::ENEMY`/`ENEMY2` aliases (zero uses), `DEFAULT_MAX_RESISTANCE` (the
  real default is `capacity · RESISTANCE_PER_CAPACITY`), `builders::authored_planet`/`HOME_R`
  (no callers; the tutorials author inline).

### Validation now certifies the shipped enemy (`levels`, `ai`)
- **`SeatController` (the roster→brain dispatch) moved from the game binary into `ai`** and both
  hosts share it: the levels validation previously built every enemy via
  `AiController::from_roster`, whose `SimpleColonize` arm routes to the *parked stateless
  automaton* — so the determinism/winnability gates certified a different enemy than the game
  ships. All validation runs now drive the **live stateful `SimpleController`**.
- **Every declared seat plays.** Validation drove only `Ai(0)`; L3's second Simple never received
  an order through the whole sweep. One `SeatController` per `enemies[i]`, all stepped in seat
  order, elimination early-outs are now all-seats.
- **Cadence matches the game**: new `ai::harness::GAME_DECISION_BASE = 5` (the game's
  `DECISION_BASE` sources it); validation's live runs use it instead of the harness's 8.
- **The concentration proxy targets capturable ground**: nearest **neutral first**, never the
  storage node (it charged L1's 400-ship centre fortress first, and poured captured posts'
  production into the uncapturable centroid node).
- Full `cargo test -p levels` green under the new drivers (structure + determinism on all 10,
  ~2 min).

### GUI fixes (`game`)
- **Click-through is gated.** Mouse-release resolved clicks without requiring the press to have
  been seen by the board handler, so clicking the pause menu's *Resume*, an overlay's *Close*,
  etc. issued a board click (worst case an unintended move order) on release. Releases without a
  tracked press are ignored; opening the pause menu abandons any drag in progress.
- **Death-drift fade is tick-scale-correct**: the fade divided by the unscaled `DRIFT_TICKS`
  const while `drift_remaining` is set from the ×24-scaled param — alpha was pinned >1 for ~95%
  of the drift, then popped. Now divides by the match's `sim.drift_ticks` (clamped).
- **Ship-LOD hysteresis**: the meshed↔binned switch at 1500 on-screen ships now has a ±~7% dead
  band, so a battle hovering at the boundary doesn't flicker between modes each frame.
- **Render snapshots stop copying per tick**: the interactive frame loop snapshots ship
  positions only before the frame's *final* tick (a 25× frame drained up to 30 ticks, each
  copying every ship position for a snapshot only the last of which was rendered) and the
  buffers are reused; a match ending mid-frame renders at `alpha = 1` instead of lerping against
  a stale snapshot. Headless paths (`--selftest`/`--shot`) are unchanged.
- `enemy_color`'s doc no longer overstates per-seat distinctness (seats 1+ share the alt shade).

### Corrections to this changelog (design-record drift, resolved with the owner)
- **`RESISTANCE_PER_CAPACITY` is `60`** (default sub grind `3600`, L1 centre `6000`) — the
  playtested value; the original entry's `30`/"historical 1800 pace" text below is amended.
- **L2 ships as the 6-sub layout** (`spec_for(2) → (6,1,1,4)`); the corridor-post bullet below
  is amended (that re-tune never landed).
- `DEFAULT_RING_FRAC` is `0.75` (the 0.667 mention below is amended).

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
- **Mission 2** corridor posts: *(corrected — this entry previously described two lean corridor posts
  (storage 30 / prod 1) and a `(8,1,1,6)` spec; that re-tune was dropped and never shipped.* L2 ships
  as the 6-sub layout — 4 middle posts + 2 homes, `spec_for(2)` → `(6,1,1,4)`.

### Struct storage is ownerless + resistance = capacity (sim)
- **Struct storage has no ownership.** The reserve / staging node is now permanently **Neutral** and
  **never captured** (`resolve_resistance` skips it; `add_storage_sub` seeds it Neutral, dropping the
  old `majority_owner` seeding). It is a shared space any side may stage in.
- **Auto-divert gate rewritten.** A producing sub ships its over-capacity surplus into storage only
  while **fewer than `STORAGE_ENEMY_BLOCK = 20` enemy ships** sit there (was: only into a reserve the
  producer *owned*) — so a contested staging area stops accepting deposits.
- **Default resistance is proportional to storage capacity.** `SubStructure::new`/`with_storage_capacity`
  set `resistance = max_resistance = storage_capacity · RESISTANCE_PER_CAPACITY` (`= 60` — *corrected:
  this entry originally said 30/“reproduces the historical 1800 grind”; the shipped, playtested value
  is 60*); `with_max_resistance` still overrides. At the default capacity `60` a fresh sub starts at
  `3600` (double the historical pace — sieges are deliberately slower); a bigger sub is proportionally
  harder to take (Mission 1's storage-100 centre → 6000).
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
  `ring_frac` is fixed at `DEFAULT_RING_FRAC = 0.75` (retuned from the original 2/3; may be authored per sub, but is **not
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
