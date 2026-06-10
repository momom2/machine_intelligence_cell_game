# 01 — Mechanics

Every mechanic here was chosen against a two-part test: **(a)** does it deepen *human* play, and
**(b)** does it keep the domain friendly to autoconstruction (cheap to simulate, low-variance for
the empirical fitness gate, legible enough that the evolved policy stays human-readable)? Most of
the Solarmax-derived mechanics pass both nearly for free. Where a mechanic threatens (b), the fix
is the **computation/spectacle decoupling** from the overview.

## Engine basics — the friendly domain

- **Battlefield:** a small graph, ~8–20 nodes. Nodes are production structures ("planets" /
  "factories") with a garrison count and an owner; edges are paths with a length.
- **Resolution:** deterministic at the *decision* level (see Combat for the stochastic render
  layer). Discrete or near-continuous ticks.
- **Atomic action:** `(source, fraction-bucket ∈ {25,50,75,100}%, target)`. Deliberately trivial.
  **[DECIDED]** There must be no hard atomic sub-skill (no move-generation, no pathing puzzle the
  *agent* must solve) — all intelligence is to live in the *composition* of rules, not in any
  single action. This is the property that makes the self-improvement loop sit on solid ground.
- **Derived legible quantities** (these do **triple duty**: player HUD, AI input features, and
  fitness-shaping terms — this triple-use is a direct expression of the shared-vocabulary spine):
  production rate, territory share, unit share, per-node force ratio, "is this a frontier node,"
  distance-to-nearest-enemy, and predictive terms like "incoming hostile fleet in N ticks."
  > **[OPEN / now blocking]** The exact feature set is not finalized. It is constrained from two
  > sides: it must be rich enough to drive AI policies *and* legible enough that a **human can infer
  > an opponent's strategy mix at a glance** (required by the arc-1 capstone in `02`). The same
  > quantities serve all three roles. This decision can no longer be deferred.

## Combat — stochastic Lanchester square law

Adopt Solarmax's model: each engaged ship is a stochastic emitter that destroys an enemy ship when
it "fires," with expected damage-per-tick proportional to the number of *currently engaged* ships
on its side. **This is the stochastic form of Lanchester's square law.**

- **Large-N limit** → the deterministic square-law ODE; the *relative* spread of outcomes shrinks
  roughly like 1/√N. So large battles feel near-deterministic while small skirmishes (<~10 ships)
  feel organic and chancy. That texture is the law of large numbers, not a tuning accident —
  exploit it rather than fighting it.
- **Strategic consequence — concentration of force is ~a theorem:** under the square law, 2× ships
  ≈ 4× effective power. "Mass your fleets" is therefore (i) a deep lesson the player *discovers*
  and (ii) a legible core competency the AI *evolves* — both sides reasoning inside the same
  visible truth. Keep this property; it is load-bearing for legibility.

### **[DECIDED]** The mean-field / stochastic split (critical engineering)

Variance fights the empirical acceptance gate (the relaxed-Gödel gate). Resolve this by separating
computation from spectacle:

- **For the AI's fitness evaluation:** simulate **deterministic mean-field** combat (the square-law
  ODE / its discrete analog). Zero variance, very cheap → ideal for evaluating thousands of
  candidate policies.
- **For the played match:** render the **stochastic** realization.

Randomness thus becomes a *presentation layer*, not a fitness contaminant. Small-skirmish noise is
pure flavor. (If you later want variety in *play*, seed it and evaluate candidates over enough
matchups that the gate is not fooled.)

## Movement

Borrow Solarmax's two movement subtleties; both are deterministic given commit-order, so they add
depth at **zero variance cost** to the AI:

- **Engagement / undock delay:** ships leaving a node are vulnerable and do *not* contribute damage
  during the transition; retreats incur losses. This creates a **commitment-vs-tempo tension**
  (attack *timing* vs. holding position).
- **Staggered arrival:** individual ships arrive spread over a fraction of a second; combined with
  the snowballing nature of square-law combat, this yields an elegant, emergent **defender's
  advantage** without a hardcoded multiplier (though see the triad balance note — you may still need
  an explicit defender term to tune).

Design payoff: the commitment-vs-tempo tension is **non-transitive**, which is one of the forces
that keeps the strategy space from collapsing onto a single dominant line. It also pushes the rule
DSL toward *predictive* conditions ("incoming in N ticks"), which is desirable — predictive
features are exactly what make a policy look like it is "thinking."

## Structures — the curriculum-authoring kit

All of Solarmax's map structures formalize trivially on a graph, so they cost the AI almost nothing:

- **Walls** → removed/cut edges.
- **Turrets** → a per-transit toll (destroy N ships crossing an edge/arriving at a node).
- **Teleporters** → a zero-transit-time edge.

**[DECIDED]** Their real purpose is not flavor — they are **how you author the difficulty curve and
the lessons.** Each structure isolates one concept, so *map design becomes lesson design*. Note one
nice coherence: a turret chokepoint reinforces the square-law lesson, because a flat per-transit
toll punishes dribbled forces and rewards concentration. (Teleporters additionally serve as the
first tool that breaks Euclidean distance-intuition — the on-ramp to the topological map ramp in
`03`/`04`.)

## The strategic triad — colonize / defend / attack (the strategic core)

Three primary uses of ships, each with a clear economic identity:

- **Colonize** empty nodes — fastest growth of the power base, but leaves production worlds thinly
  garrisoned and vulnerable.
- **Attack / conquer** enemy production — must overcome the defender's advantage; slower and less
  unit-efficient than colonizing.
- **Defend** — pays a pure **opportunity cost** if the enemy declines to attack and colonizes
  instead.

This triad is the design's keystone because it is the *minimal* mechanic that is simultaneously
(a) trivially solvable against a static world, (b) non-transitive against an adapting one, and
(c) expressible in the shared DSL. It is exactly the layer of strategy a human first learns in
StarCraft II multiplayer, and it is precisely what cell games like Solarmax never explore — because
those games only pit you against fixed, exploitable algorithms, so they add *features* for depth
instead. **This depth can only emerge against a learning opponent.** That is the entire reason the
opponents in `02` exist.

### **[DECIDED — but with a hard, measurable caveat] It is a cycle of three *numbers*, not three *labels***

The danger: as stated, the triad reads partly like a single **tempo gradient** (colonize < defend <
attack, ordered by commitment), and pure tempo gradients have a **dominant line** — usually
"colonize until contact, then react." That dominant line is a fixed point that *kills the arms
race* (and with it the whole game). What rescues the triad into genuine rock-paper-scissors is the
**timing structure** that combat + movement already provide:

- **attack beats greedy-colonize** (undefended production falls to a timed strike),
- **colonize beats turtle-defend** (the defender pays opportunity cost while you out-produce),
- **defend beats attack** (the defender's advantage punishes the over-committed aggressor).

That cycle closes **only if the payoffs are tuned so each edge genuinely wins.** It is therefore a
property of three numbers — **colonization rate, defender-advantage multiplier, attack-commitment
loss** — not of the three words. If the defender multiplier is too high, *defend* dominates and you
are back to a fixed point.

> **[OPEN — the load-bearing question, see `04`]** There may be *no* region of the
> (rate, defender, commitment) parameter space where all three counters hold. This **cannot be
> proven up front** (same reason the Gödel gate is empirical) and must be **measured**. The
> recommended day-one task is a headless sweep over these three constants to confirm a
> non-degenerate region exists. If it does not, the strategic core does not exist.

### **[DECIDED]** The greedy optimum is a small scheduling problem (not naïve nearest-first)

Against a *static* world, each pure strategy has a simple near-optimal solution — but "send ships
out as soon as available to the planet that maximizes colonization rate" is *myopically* greedy.
The true static optimum is a short-horizon **scheduling** problem:

- nearest-first is not always rate-maximal once travel time is amortized over a node's *remaining
  production horizon*;
- there are sequencing effects — e.g., grab the cheap near node first to fund the lucrative far
  one, vs. commit to the far one early so it compounds for longer.

It remains solvable (short-horizon optimization, not a hard combinatorial beast), but it is *just*
nontrivial enough to be (i) satisfying for the player to discover and (ii) a clean dial for
**Automaton sophistication**: e.g. Automaton-0 colonizes nearest-first (exploitable — bait it toward
a cheap cluster and snipe its rear), Automaton-1 colonizes rate-optimally but never defends,
Automaton-2 folds in reactive defense, and so on. Each is a static policy with exactly one
diagnosable seam, and "find the seam" is the whole skill of the first arc.
