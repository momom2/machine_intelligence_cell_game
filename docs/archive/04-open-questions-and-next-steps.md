# 04 — Open Questions, Risks & Next Steps

**Start here for "what do I build first."** This document collects what is *not* settled and orders
the work so the riskiest, cheapest-to-test assumptions are validated before anything expensive is
built.

## The load-bearing risks (unproven, and must be *measured* not assumed)

Two of these share a root cause with the prior self-improvement analysis: you cannot *prove* up
front that the strategic/policy space is non-degenerate, for the same reason the AI's acceptance
gate had to be empirical rather than a proof. So they are **empirical go/no-go tests**, not
design debates.

### R1 — **Dominant-policy collapse** (the scariest; the whole edifice depends on it)
Once *both* sides program in a rich `condition → action` DSL, the design requires that **no single
meta-policy dominates** — every policy must have a counter-policy. Otherwise both the player's
iteration loop *and* the Architect's evolution converge to a fixed point and die: no arms race, and
nothing left to understand. This is far harder to guarantee than action-level (rock-paper-scissors)
balance, and it is unprovable in advance. **If the DSL admits a dominant program, the game is
hollow no matter how elegant the parts.** Must be playtested in **policy space**, early, on a
stripped prototype.

### R2 — **Cycle non-degeneracy** (the smallest concrete instance of R1)
The colonize/defend/attack triad (`01`) is true rock-paper-scissors **only** in some region of the
three-payoff-constant space `(colonization rate, defender-advantage multiplier,
attack-commitment loss)`. There may be no region where all three counters
(attack≻colonize, colonize≻defend, defend≻attack) simultaneously hold. **This is the day-one
go/no-go test for the strategic core.**

> ### Clarification: the headless sim is a *developer instrument*, not a shipped feature
> The recommended first build is a **headless** combat/economy simulator. This is a throwaway
> *developer validation tool* — the player never sees it. **The shipped game is fully visual**
> ("see the factory grow," per `03`); there is no tension between the two. The headless sim exists
> only to answer R2 (and later to be the cheap mean-field evaluator for the Architect's fitness,
> per `01`) before any art, DSL, or UI is built.

### R3 — **Legibility vs. strength** (a genuine tradeoff, tuned by feel)
Strong self-play AI trends *alien*. The game deliberately targets the most *comprehensible
escalation curve*, not the strongest opponent. Lever: a **parsimony penalty** in the Architect's
fitness (favor small rule-sets). This is the one item here that is a real tradeoff to tune rather
than a binary to test — it is the difference between "I can see it thinking and I'm improving" and
"it does something effective I can't parse." Accept a strength ceiling in exchange for legibility.

### R4 — **Overfitting / train–deploy gap**
Coevolution can produce strategies strong vs. AIs but strange vs. humans; adapting to a *model* of
the player risks chasing stale behavior. Mitigations (from `02`): blend self-play and player-model
fitness; regularize the player model; treat the archive of the player's **actual** wins as the real
test set.

### R5 — **Compute / latency budget**
Between-match evolution needs background compute; in-match adaptation must be cheap. The entire
approach is feasible *only because* the simulator is tiny and fast — that simulability is the lever.
Size the budget deliberately; do not let the feature set or map size grow until mean-field
evaluation is no longer cheap.

## The open decisions (with leans — **leans are not decisions**)

### D1 — Layer interaction: **one-way ladder vs. fluid toggle** *(detail in `03`)*
**[LEAN: fluid toggle]** — preserves a durable human role (orchestrator, not spectator), keeps the
art budget paying off, and locates the late-game contest at Layer 3 in policy space. This call sizes
the DSL, the UX budget, the map-abstraction ramp, and how hard the opponents lean on the shared
substrate.

### D2 — How central is player-authored automation: **endgame vs. convenience layer**
**[LEAN: central / endgame]** — only the central version delivers "feel the richness of machine
intelligence"; the convenience version is "just a nicer Solarmax." The player conversation trended
strongly toward central. D1 and D2 are coupled; decide them together.

### D3 — Feature/HUD vocabulary *(now blocking; detail in `01`/`02`)*
**[OPEN, no lean yet.]** The exact set of derived quantities. Constrained from both sides: rich
enough to drive AI policies, *and* legible enough that a human can infer an opponent's strategy mix
at a glance (required by the arc-1 capstone). Same quantities serve HUD + AI features + fitness
shaping. Cannot be deferred further — most DSL and UI work waits on it.

### D4 — The rule DSL specification *(blocking; = D3's sibling)*
**[OPEN.]** This is **one** language serving three roles: the AI policy representation, the player's
automation language, and the Layer-3 command surface. Design it once, for all three. It should
**grow across the campaign** (each unlocked primitive = one new concept = cognitive-load pacing).

## Recommended build order

Ordered to kill the biggest risk most cheaply first; do **not** build art or UI before step 3 clears.

1. **Headless cycle-sweep (answers R2; day-one go/no-go).** Implement the three-number
   combat/economy sim (mean-field combat from `01`). Sweep `(rate, defender, commitment)`. Define
   "the cycle holds" measurably (e.g., for each ordered pair of pure strategies, the intended winner
   wins across the region; report the size/shape of the non-degenerate region). **If no such region
   exists, stop and redesign the triad before anything else.**
2. **Decide D1 + D2** (coupled). These size everything downstream. Leans: toggle; central.
3. **Specify D3 (feature/HUD vocabulary), then D4 (the rule DSL).** D4 depends on D3. Remember D4 is
   one language for three roles.
4. **Prototype the core loop on Layer 2 only**: the Automaton ladder + the arc-1 capstone
   (hidden-but-fixed strategy mixes; difficulty ordered by distance-from-pure-strategy). Validate
   that *inferring a mix and countering it* is fun **before** adding layers or other opponents. This
   also stress-tests D3 (can a human actually read a mix from the HUD?).
5. **Build the Architect's autoconstructive loop** (population of rule-set organisms with their own
   operators; mean-field coevolution fitness + cached player-model fitness + parsimony penalty;
   glass-genome diffs; wins-as-test-cases regression archive). This is where R1, R3, R4 get their
   real test — keep the prototype strippable so policy-space collapse can be observed directly.
6. **Then** layer in the embodied Layer 1 / abstract Layer 3 UI, the map-abstraction ramp, the
   Counter and Apprentice opponents, and the visual DSL editor.

## Intellectual lineage (context for design choices)

- **Genre / feel:** Solarmax 2, Auralux, Cell War, Galcon (cell games); **Gladiabots** (the
  understanding-and-improvement feel); 90s sci-fi AGI (the thematic target).
- **Combat:** Lanchester's square law (the stochastic-emitter model is its stochastic form).
- **AI architecture:** Eurisko (heuristics as first-class, self-modifying objects); Lee Spector's
  *autoconstructive evolution / Pushpop* (organisms carry their own reproduction operators);
  Schmidhuber's *success-story algorithm* (install a self-modification only if it does not regress —
  repurposed as wins-as-test-cases); Co-Reyes et al., *Evolving Reinforcement Learning Algorithms*
  (searching program space for the *learning rule*, not just hyperparameters); the **relaxed /
  empirical Gödel Machine** framing (acceptance by *measured* performance, never proof — which is
  why variance control and the mean-field evaluator are load-bearing).
- **Why micro is ceded to automation:** StarCraft II / AlphaStar, where superhuman micro was capped
  to keep the contest about strategy — evidence that lower-layer execution is the machine-dominant
  regime.
