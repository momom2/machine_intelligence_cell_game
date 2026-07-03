# 02 — AI Opponents

The opponents are the product. They are designed so that the player climbs a **cognitive ladder**
in which each enemy is a different *epistemic* task, and so that the headline opponent — a
genuinely self-improving AI — is both tractable to build and legible to play against.

## One substrate, four minds

**[DECIDED]** All four opponents (and the player's own automation — see `03`) share a **single
policy representation**: a **prioritized list of `condition → action` rules** evaluated each tick,
over the legible feature set from `01`. This is Eurisko's idea of *heuristics as first-class
objects*, but with the crucial twist that the conditions and actions are the **same concepts the
player intuitively uses** — so reading the AI's policy is reading the player's own vocabulary back.

The four opponents differ along just two axes:

| Opponent | Source of strategy | When it adapts | Epistemic task it teaches |
|---|---|---|---|
| Automaton | fixed (handwritten) | never | read a fixed system |
| Counter | selects (from a repertoire) | within a match | read *adaptation*; play the meta |
| Apprentice | copies (imitates the player) | across matches | read *yourself* |
| Architect | invents (evolves new rules) | across matches | open-ended escalation vs. invention |

> The shared substrate is what makes the ladder *cumulative*: a player who has learned to read the
> first three opponents already possesses the vocabulary needed to read the fourth.

### The Automaton (basic)
A fixed, handwritten rule-set, deliberately simple, with **one clear exploitable flaw** (e.g.,
always reinforces the frontier nearest the enemy and leaves its rear thin). Its job is to teach the
vocabulary: the player wins by reverse-engineering a static machine. See the capstone section for
how a *ladder* of these becomes the climax of arc 1.

### The Counter (visibly adapts)
**No invention.** It *classifies* the player's current style from observable features (expansion
rate, aggression timing, frontier behavior) using something as small as a decision tree, then
deploys the known counter from a **curated repertoire** — and **announces it** ("Read: rush →
switching to defensive posture"). The challenge is meta: fixed exploits now get punished, so the
player learns the counter-table and learns to **feint and vary**. Adapts *within* a match,
legibly. **[DECIDED]** This is an **arc-2** opponent — explicitly *not* the arc-1 capstone, because
it removes the "there exists a correct read" guarantee (see capstone).

### The Apprentice (imitates the player)
Induce a rule-set from the player's logged decisions — **behavioural cloning fit to the *same* rule
representation**, so the result is readable rather than a black box. Two features make it special:
- Surface the induced rules as *"here is what I learned about how you play"* — an explicit mirror of
  the player's own policy (pointed, and well-suited to the intended audience).
- Run it **one match behind**, so beating it requires the player to keep *evolving* and to notice
  their own ruts. Teaches **self-modelling**.

### The Architect / Gardener (autoconstructive — the headline)
Between matches, it runs an **autoconstructive evolutionary loop** over a *population* of rule-set
"organisms," where each organism carries **its own** mutation/recombination operators (this is
Lee Spector's *Pushpop* twist — the reproduction machinery itself evolves, not just the strategy).
Fitness comes from **fast simulated coevolution** (cheap mean-field matches, per `01`) **plus**
performance against a cached model of the player.

Two mechanics carry the *feel* — they are what turn "a self-improving AI" from an alien blob into
the Gladiabots experience:

- **The glass genome.** The Architect's current rule-set is **visible** — ideally unlocked as a
  reward for beating it — and the player **watches it mutate and grow between matches as diffs**
  (`+ rule: defend rear when frontier is committed`). Autoconstruction becomes a legible,
  *spectatable* process rather than a black box. This is the single most important "feel" mechanic
  in the game.
- **Your wins become its test cases.** Every line the player beats it with is added to a
  **regression archive** that the next genome must still survive. (This repurposes Schmidhuber's
  *success-story algorithm* — accept a self-modification only if it does not regress past
  performance.) So every exploit the player finds is **visibly closed**, and the AI feels like it is
  learning *from the player specifically*. Mechanically this is just a growing test set feeding the
  fitness function — but emotionally, that **exploit → patch arms race is the engine of the whole
  game.**

## The cognitive ladder (the campaign's spine, in opponent form)

Ordered, the four opponents form a curriculum of escalating epistemic difficulty:

1. **Automaton** — read a fixed system.
2. **Counter** — read *adaptation*; learn to play the meta and to feint.
3. **Apprentice** — read *yourself*; learn self-modelling.
4. **Architect** — survive open-ended escalation against something that *invents*.

This ordering is the same ascent as the player-skill and map-abstraction ascents in `00`/`03`;
they are interlocked, not parallel.

## **[DECIDED]** The arc-1 capstone — a ladder of hidden-but-fixed strategy mixes

The climax of the first arc is *solving the triad dynamic* against Automatons that implement various
mixes of colonize/defend/attack. The key design decision is **how observable the opponent's mix
is**, because that decision defines what "solving" even means:

- *Observable + fixed* → a pure **classify-then-counter** puzzle. Clean and fair, but once the
  counter-table is known it is solved forever; it teaches the meta but does not sustain.
- **Hidden + fixed → [CHOSEN for the capstone].** Adds an **information-gathering** layer: the
  player scouts, **infers the mix from early behavior**, then commits to the counter under
  uncertainty. This is *online policy inference* — exactly what the Apprentice does to the player —
  so it is the first time the player *needs* the inference skill the entire back half of the game is
  built on.
- *Hidden + switching* → this is the **Counter** opponent; it removes the "a correct read exists"
  guarantee and turns a solvable puzzle into an open meta-game. **Save it for arc 2.**

So the capstone is a **ladder of hidden-but-fixed Automatons**, each a different point in the
colonize/defend/attack simplex. Winning requires: **(1)** infer the mix from observation, **(2)**
select the counter-strategy, **(3)** execute the now-static optimization (the scheduling problem
from `01`) against it.

**Difficulty metric (lets you order the ladder rigorously, not by feel):** the *distance of the
enemy's mix from a pure strategy*. Pure strategies have clean counters; **balanced mixes near the
center of the simplex are hardest** to read and to counter. Order the ladder by increasing
centrality.

**Constraint this pushes back onto the rest of the design:** hidden-mix inference is only fair if
the player *can* infer a mix from the HUD in real time. This forces the feature/HUD vocabulary
(`01`) to be human-legible at a glance — the same quantities that feed AI policy and the fitness
gate. This is the shared-vocabulary spine again, and it is why the HUD decision is now blocking.

## Cross-cutting risks (full treatment in `04`)

- **Legibility vs. strength.** Strong self-play AI trends *alien*. We are deliberately **not**
  building the strongest opponent — we are building the most *comprehensible escalation curve*. Put
  a **parsimony penalty** in the Architect's fitness (favor small rule-sets — Occam, and a nod to
  Eurisko's "interestingness" pressure) so the glass genome stays readable and the player can keep
  up. Accept the strength ceiling this buys. This penalty also keeps the evolved policy inside a
  vocabulary the player can read *and write* (relevant to `03`).
- **Overfitting to one player / train–deploy gap.** Coevolution can yield strategies strong vs.
  other AIs but strange vs. a human; adapting to a *model* of the player risks chasing stale
  behavior. Blend self-play fitness with player-model fitness, regularize the player model, and
  treat the archive of the player's **actual** wins as the real test set rather than the model's
  predictions.
