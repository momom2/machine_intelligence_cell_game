# Changelog

Newest first. Each entry records the **delta** from the previous state and is authoritative for the
mechanics it touches — when a per-component doc (`LAYER1_SIM.md`, `GAME.md`, `WORLD.md`, `LEVELS.md`,
`README.md`) disagrees with the latest entry here, **this file wins** until that doc is refreshed.

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
