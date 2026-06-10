# DSL_SPEC — the one language for three roles

> Build-order step 3 deliverable (decisions **D3** + **D4** from
> `04-open-questions-and-next-steps.md`). This specifies the shared
> `condition -> action` rule DSL and the legible feature vocabulary it reads. It is
> the *single* language that serves **three roles at once**, and it is designed to
> **grow across the campaign**, one concept at a time.
>
> Implemented in `crates/cell-core/src/features.rs` (D3) and
> `crates/cell-core/src/dsl.rs` (D4). The expressiveness proof is
> `crates/cell-core/tests/dsl_tests.rs::dsl_reproduces_pure_strategies`.

## 0. Why one language (the spine)

The design's central trick (`00-overview.md`) is to make three things the *same
object*:

1. **The AI's strategy representation.** Every opponent in `02-ai-opponents.md` — the
   Automaton (fixed), the Counter (selects), the Apprentice (imitates), the Architect
   (evolves) — *is* a policy in this DSL. "One substrate, four minds."
2. **The player's automation language.** The player authors the *same* rules to shed
   micromanagement (`03-ui-layers.md`, decision D2 = central/endgame).
3. **The Layer-3 command surface.** Issuing a standing order at the abstract zoom
   level *is* writing a rule (`03`, decision D1 = fluid toggle).

When these coincide, two hard problems are solved together: self-improvement becomes
**tractable** (a cheap, closed, deterministic fitness oracle — the mean-field engine)
and **legible** (the player reads the AI's policy because it is written in the
player's own words). Reading the Architect's *glass genome* and writing your own
subroutine become the same skill. Everything below is in service of that coincidence,
so every primitive is chosen to be **legible-at-a-glance first, expressive second.**

## 1. The feature vocabulary (D3)

Features are *derived legible quantities* computed from `GameState`. They do **triple
duty**: the player HUD, the AI's input features, and the Architect's fitness-shaping
terms are **the same numbers**. The binding constraint (the arc-1 capstone in `02`) is
that **a human must be able to infer an opponent's colonize/defend/attack mix at a
glance** from these — so anything that cannot be read off the board by eye does not
belong here.

There are two grains. Both are needed because the capstone names exactly these
observables ("expansion rate, aggression timing, frontier behavior").

### Global / per-owner features (`OwnerFeatures`) — the top bar

| Feature | Meaning | Reads as (at a glance) | Mix signal |
|---|---|---|---|
| **production rate** | ships/tick across owned nodes (`Σ r·mult`) | "how fast my numbers climb" | high + thin army → **colonize** |
| **territory share** | `(mine − enemy)/(mine + enemy)` nodes, in [−1,1] | count the dots on the map | rising fast → **colonize** |
| **unit share** | same shape over ships (garrisons + fleets) | the army-strength bar | high vs. territory → **attack** (massing) |
| territory count | raw node count | — | — |
| total units | raw ship count | — | — |

### Per-node features (`NodeFeatures`) — one structure at a time

| Feature | Meaning | Reads as (at a glance) | Mix signal |
|---|---|---|---|
| **force ratio** | `eff. defense / incoming hostile` (credits `√k` + inbound friendlies) | "am I winning *this* fight?" | — (the defend trigger) |
| **is-frontier** | touches any non-mine node | the colored front line | garrisoned frontier → **defend**; thin → **colonize** |
| **distance-to-enemy** | hops to nearest enemy node | "how far is the fight" | shrinking + local stack growing → **attack** |
| **incoming hostile in N ticks** *(predictive)* | enemy ships with ETA ≤ N | fleets visibly streaking in | a timed wave → **attack** |
| incoming friendly | friendly ships inbound | "help is on the way" | — |
| **attack force ratio** | `my·(1−l) / weakest adjacent enemy` | "can I take the planet next door?" | massing past 1 → **attack** |
| **weakest-neighbor force ratio** | min force ratio over adjacent friendly | "is a neighbor of mine in trouble?" | kept high → **defend** |

Why the predictive term matters: the movement model's **undock delay** (`01`) is what
makes "incoming in N ticks" readable *in time to react*. Predictive conditions are
"exactly what make a policy look like it is thinking" (`01`), and they are what let a
human pre-empt an attack — the skill the whole back half of the game is built on.

The default look-ahead `N = undock_delay + 14` is centralized
(`features::default_look_ahead`) so the HUD and every policy mean the *same thing* by
"incoming" — otherwise the shared-vocabulary guarantee breaks.

## 2. The rule language (D4)

```
Policy   = prioritized Vec<Rule>          // evaluated top-to-bottom each tick
Rule     = { priority, condition, action, note }
Condition = Always
          | Compare { feature, cmp, feature }   // cmp ∈ { <, ≤, >, ≥ }
          | And | Or | Not                       // combinators
Action   = { source: NodeSelector, fraction: FractionBucket, target: NodeSelector }
NodeSelector = { scope, owner-filter, rank }
  scope        ∈ { Here, Adjacent, Anywhere }
  owner-filter ∈ { Mine, Enemy, Empty, NotMine, Any }
  rank         ∈ { WeakestGarrison, StrongestGarrison, Nearest,
                   BestColonizeValue, LowestForceRatio,
                   ClosestToEnemy, ClosestToNeutral }
FractionBucket ∈ { 25%, 50%, 75%, 100% }   // the design's fixed buckets
```

The **atomic action** is exactly the design's `(source, fraction-bucket, target)`.
There is deliberately *no* sub-action skill (no pathing, no move-generation): **all**
intelligence lives in which rules get composed (`00`, `01`). That property is what
puts the self-improvement loop on solid ground.

### Evaluation semantics (and why)

Evaluation is **source-driven**: a rule means *"for each of my nodes that matches the
source selector and satisfies the condition (tested at that node), send `fraction` of
its garrison to the node its target selector resolves to (relative to that source)."*
This mirrors how a player reasons ("for each planet with spare ships, send them
somewhere") and how the hardcoded strategies loop, which is why the DSL can reproduce
them.

**Priority = first-matching-rule-per-node wins.** Within a tick each source node is
committed by at most one rule: the highest-priority rule that both matches *and* finds
a target. So a defend rule placed above an expand rule pre-empts expansion exactly
when it should. You read a policy top-down like a sentence:

> *Attack* = "If I can take the planet next door (attack force ratio ≥ 1.1), commit
> everything at the weakest enemy; **else** expand a little to fund the army; **else**
> march the army one hop toward the enemy."

**Determinism.** Every selector resolves with a total tie-break (lowest `NodeId`
wins), nodes are visited in id order, and there is no RNG. A policy is therefore a
pure function of `GameState` — the precondition for the empirical fitness gate
(`00`, `04`) and for the mean-field/stochastic decoupling (the *same* rule-set
evaluated for fitness also drives the rendered match).

### The three roles, concretely

- **AI policy:** `dsl::Policy` wrapped in `dsl::DslPolicy` implements the engine's
  `policy::Policy` trait, so it plugs straight into the match runner, the harness, and
  (later) the Architect's evaluator.
- **Player automation / Layer-3 commands:** the *same* `Rule` values, authored through
  a visual editor (`03` "practical stance"). A standing order is a high-priority rule;
  a manual override is a one-shot top-priority rule for one tick (hybrid control).
- **Glass genome:** because a policy is just a sorted `Vec<Rule>` with human `note`s, a
  between-match mutation is a readable **diff** (`+ rule: reinforce the most threatened
  friendly neighbor`) — the single most important "feel" mechanic in `02`.

## 3. The expressiveness proof

The three pure strategies from build-order step 1 (`policy.rs`) are re-expressed using
**only** the DSL (`dsl::strategies`), and they reproduce the rock-paper-scissors cycle
at R2's recommended operating point (`r=0.6, k=2.25, l=0.15`):

| Edge | DSL margin (mean over the 3 maps + both seatings) |
|---|---|
| attack ≻ colonize | **+0.26** |
| colonize ≻ defend | **+0.17** |
| defend ≻ attack | **+0.42** |

All three clear the R2 acceptance margin (epsilon = 0.05) comfortably, and the cycle
holds on *every* individual map as well as the mean (corridor7, twin_bridge8, star9).
(See `dsl::tests::dsl_reproduces_pure_strategies` for the asserted thresholds; the
`#[ignore]`d `dsl::tests::diag_margins` prints the per-map breakdown.) The cycle living
in the *priority and presence of defend/attack rules* — all three archetypes share one
`expand` rule and differ only in what sits above it — is the legible heart of the
result: the difference between three strategies is three short, readable rule lists.

This is the load-bearing evidence that the DSL is rich enough to carry the strategic
core. If three competent strategies fit in the language, the policies the Architect
evolves and the player writes can too — and R1 (dominant-policy collapse) can be
tested *in this very language* on the stripped prototype (build-order step 5).

## 4. How the language grows across the campaign

> `03`/`04`: "Grow the DSL across the campaign — each newly unlocked primitive is one
> new concept, which also makes the DSL the game's cognitive-load pacing mechanism."

The DSL is intentionally a *superset-by-accretion*: a campaign stage exposes a subset
of features, selectors, ranks, and combinators, and each unlock is **one concept**.
This is the pacing dial. A suggested ramp (each row = one new idea the player must
absorb, interlocked with the map-abstraction ramp and the opponent ladder):

| Stage | New primitive unlocked | New concept taught | Aligns with |
|---|---|---|---|
| 1 | action `(source, 100%, nearest empty)` | "send ships to expand" | operator; Automaton-0 |
| 2 | fraction buckets `{25,50,75}%` | "commit *partially*" | tempo vs. commitment |
| 3 | `BestColonizeValue` rank | "rate-aware, not nearest-first" | the scheduling problem (`01`) |
| 4 | feature `force ratio` + `Compare` | "read whether I'm winning a fight" | reading combat |
| 5 | `priority` (multi-rule policy) | "heuristics have an order" | first-matching-wins |
| 6 | predictive `incoming hostile in N` | "defend *before* the blow lands" | the capstone's inference need |
| 7 | `attack force ratio` + strike rule | "time a push on a majority" | attack ≻ colonize |
| 8 | `weakest-neighbor force ratio` | "help a neighbor in trouble" | defend ≻ attack |
| 9 | `And`/`Or`/`Not` | "compose conditions" | richer policies |
| 10 | global features (shares, production) | "read the macro game" | strategic/logistical (Layer 3) |
| 11 | `ClosestToEnemy` / `ClosestToNeutral` routing | "flow forces across the graph" | topological map ramp |
| 12 | full selector cross-product | "author arbitrary subroutines" | meta-programmer; vs. Architect |

Two rules of thumb keep the growth healthy:

- **One unlock = one concept.** Never introduce two new ideas in the same stage; the
  DSL's growth *is* the difficulty curve, so pace it like lessons (`01`: "map design
  becomes lesson design" — here, *primitive* design becomes lesson design).
- **Parsimony pressure.** The Architect's fitness favors small rule-sets (`02`/`04`
  R3). This keeps the evolved policy inside the vocabulary the player has actually
  unlocked, so the glass genome stays readable *and writable* at every stage. Accept
  the strength ceiling this buys; the goal is the most *comprehensible* escalation, not
  the strongest opponent.

## 5. What is intentionally NOT in the language (yet)

To protect legibility and keep the action atomic, the current DSL omits — and should
only add later, each as a deliberate one-concept unlock — things like: multi-hop
targets (the action moves along a single edge only; routing is expressed as repeated
one-hop rules), arithmetic on features beyond comparison, per-rule cooldowns/state
(policies are memoryless functions of state), and quantified conditions ("there exists
a node such that…"). Each of these is a real expressiveness axis; each is also a real
cognitive-load cost, so each waits until the campaign needs it. The structures from
`01` (walls = cut edges, turrets = per-transit toll, teleporters = zero-length edges)
need **no** new DSL — they are map data the existing distance/edge features already
see, which is the point of grounding the language in derived quantities rather than raw
geometry.
