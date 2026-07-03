# Design Overview — Autoconstructive Strategy Game (working title TBD)

> **Purpose of these documents.** They are a handoff from an extended design conversation
> to a development (Claude Code) instance that has *none* of that conversation's context.
> They capture not just conclusions but the reasoning behind them, because most of the value
> was in the "why." Throughout, decisions are tagged:
> **[DECIDED]** settled, **[OPEN]** genuinely unresolved, **[LEAN: X]** a recommendation that is
> *not* a final decision. Do not treat a LEAN as DECIDED.

## What the game is

A minimalist real-time strategy game in the "cell game" / fleet-conquest lineage
(touchstones: **Solarmax 2**, **Auralux**, **Cell War**, **Galcon**). You hold nodes that
produce ships; you send fractions of garrisons along edges to colonize empty nodes, conquer
enemy nodes, or defend your own.

The hook is not the base game — it is the **opponents**. The player faces AI of escalating
*epistemic* sophistication, culminating in a genuinely self-improving (autoconstructive) AI.
The intended feel is the "understanding and cognitive improvement" loop of **Gladiabots**, and
ultimately a felt sense of the richness of machine intelligence — deliberately evoking
"90s sci-fi AGI."

The campaign arc: **early game = learn the mechanics (act as an operator); late game = transcend
naïve human capability via automation and high-level learning (become a programmer, then a
meta-programmer).**

A crucial enabling freedom: **because we control the engine, we deliberately shape it to be
unusually friendly to autoconstructive algorithms** — cheap to simulate, low-variance, and
legible. This is not a constraint we work around; it is a lever we exploit on almost every
mechanic.

## The design spine (read this twice)

Everything in these documents converges onto a single axis: **shedding human intuition in favor
of machine-grade reasoning.** Three *coupled* ascents run along that axis. They are mechanically
interlocked, not three separate systems — that coupling *is* the design:

1. **The player ascends:** operator → programmer → meta-programmer.
2. **The AI ascends:** autoconstruction (self-improvement over its own strategy).
3. **The map/representation ascends:** spatial & Euclidean → topological & graph-like.

Two recurring principles make the spine work and reappear in every document:

- **Shared vocabulary.** The single most important trick: make three things the *same language* —
  the AI's strategy representation, the player's mental model, and the fitness signal that drives
  the AI's evolution. When these converge, self-improvement becomes simultaneously *tractable*
  (cheap closed-domain fitness) and *legible* (the player can read what the AI is doing because
  it is written in the player's own vocabulary). Concretely: all four AI opponents **and** the
  player's automation share one `condition → action` rule DSL over one legible feature set.
  Reading the AI's policy *is* learning to write your own subroutines, and vice versa.

- **Decouple computation from spectacle.** Separate *what the system computes on* from *what the
  player sees*. Two instances: (a) the AI's fitness is evaluated on cheap **deterministic
  mean-field** combat, while the played match renders a **stochastic** realization; (b) **command**
  migrates *up* the abstraction ladder across the campaign, while **spectacle** stays pinned to the
  *bottom* and is always rendered. The player never stops *seeing* the factory grow; they stop
  *micromanaging* it. This principle is becoming the design's signature — apply it deliberately.

## Why this game can exist when "Eurisko/Gödel-machine for X" usually can't

This design descends from a prior analysis of self-improving systems (Eurisko, the Gödel Machine,
OOPS, autoconstructive evolution). The conclusion of that analysis: **those methods supply a
self-improvement *loop*, not the *base capability* the loop refines; they pay off only when the
domain is closed, the fitness oracle is cheap and trustworthy, and the base capability is already
tractable.** Chess satisfies this; an "adaptive virus" does not (its hard parts — vulnerability
discovery, exploit synthesis, defense modeling — are exactly what the loop does not provide, and
its fitness signal is sparse, deceptive, and dangerous to obtain).

A cell game is engineered to satisfy all three conditions *by construction*:
- **Closed, cheap oracle:** tiny deterministic state → a headless simulator runs thousands of
  matches/second. The simulator *is* ground truth, so there is no unsound-oracle gap to exploit.
- **Base capability is trivial:** an atomic action is just `(source, fraction-bucket, target)`.
  There is no hard sub-skill (no move-generation, no vuln-discovery). *All* intelligence lives in
  which rules get composed — exactly where we want it.
- **Variance is controllable:** deterministic by default; the empirical acceptance gate (the
  "relaxed Gödel Machine" — see below) is therefore not fooled by noise.

The AI is a **relaxed / empirical Gödel Machine**: it proposes self-rewrites to its own strategy
and installs one only if a gate certifies improvement — but the gate is *measured performance*,
never a *proof* (proving "this rewrite increases playing strength" is the intractable wall).
This forfeits global-optimality guarantees and makes variance control load-bearing, both of which
are accounted for in the mechanics.

## Document map

- **00-overview.md** — this file: vision, spine, principles, status.
- **01-mechanics.md** — the engine and game mechanics: combat (Lanchester square law),
  movement, structures, and the colonize/defend/attack strategic triad.
- **02-ai-opponents.md** — the four-opponent architecture sharing one policy substrate, the
  autoconstructive loop, and the arc-1 capstone.
- **03-ui-layers.md** — the three-layer zoom UI and the command/spectacle/ladder design.
- **04-open-questions-and-next-steps.md** — the unresolved decisions, the load-bearing risks,
  and a recommended build order. **Start here for "what do I do first."**

## Current status (snapshot)

**Settled in spirit:** the cell-game engine; the shared-vocabulary principle; the
computation/spectacle decoupling; the combat model and its mean-field/stochastic split; the
four-opponent taxonomy; the three-layer UI concept; the colonize/defend/attack triad as the
strategic core; the abstract-vs-spatial map resolved as a *ramp*.

**The three biggest open items** (detail in `04`):
1. **[OPEN]** Layer interaction model: one-way **ladder** vs. fluid **toggle**. **[LEAN: toggle]**
2. **[OPEN]** How central is player-authored automation: **endgame** vs. **convenience layer**.
   **[LEAN: central]**
3. **[OPEN, and the scariest]** Does the strategy space avoid **dominant-policy collapse**? This is
   unprovable up front and must be *measured*. The recommended day-one task is a headless sweep to
   check the triad's payoff cycle is non-degenerate.

**Not yet specified, and now blocking:** the exact **feature/HUD vocabulary** and the **rule DSL**
(these are the same shared language; several downstream choices wait on them).
