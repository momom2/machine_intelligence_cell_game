# 03 — UI: The Three Layers

The reference image the player conversation worked from is a Solarmax-2-style view — in this
scheme, that is **Layer 2**. (Note: that reference already contains teleporters, the
distance-breaking structure that begins the spatial→topological ramp.)

## **[DECIDED]** The three layers *are* the spine, made physically navigable

The zoom levels are not a UI convenience; they are the design's central axis turned into something
the player climbs by changing zoom. Each layer corresponds to one rung of every ascent in `00`:

- **Layer 1 — embodied / micro.** A single structure rendered as full 3D models: ships, textures,
  sub-structures where ships garrison. Commands here let you win a battle more *effectively* —
  win 3-ships-vs-4. → the **operator** level.
- **Layer 2 — tactical.** The Solarmax-like view: structures and ships in a 2D slice of a 3D
  volume. Commands here are tactical and strategic. → the classic **RTS mind**.
- **Layer 3 — abstract.** Pure graph and numbers, accounting for every parameter of the
  battlefield. Commands here are strategic and logistical. → the **machine's native
  representation**, i.e. *what the Architect sees*. Climbing to Layer 3 is literally adopting the
  AI's eye-view.

The campaign **gradually expands**: it introduces the bigger scales and the more abstract layers in
later levels, so the player must **master a scale before automating it** and moving on — moving on
precisely as the marginal gains from manual play at that scale shrink. This is the strongest
*structural* idea in the design, because it is not additive: it unifies the player-skill ascent,
the map-abstraction ascent, and "what the AI sees" into one thing the player does with a zoom
control.

## **[DECIDED]** Decouple the command layer from the spectacle layer

There is a real tension hiding in "Layer 3 accounts for every parameter and issues strategic
commands": if Layer 3 is strictly more *complete*, then as the player matures, Layers 1–2 risk
becoming **vestigial — beautiful and pointless**, wasting the very spectacle that makes the game
appealing. The resolution:

- **Command migrates *up* the ladder** over the campaign (you increasingly act at Layer 3).
- **Spectacle stays pinned to the *bottom* and is *always rendered*.** You issue a logistical order
  as a graph operation at Layer 3, and the engine **still shows** the fleets undocking and the
  battle resolving in 3D — exactly as an RTS shows you the fight even when you clicked the minimap.

So **you never stop *seeing* the factory grow; you stop *micromanaging* it.** This is the same
move as the combat mean-field/render split in `01` (deterministic computation, stochastic render),
and it is the design's signature principle — apply it consciously wherever computation and display
can be separated.

## **[DECIDED]** "Every parameter" at Layer 3 is a legibility trap — correct it

A human cannot actually *read* every parameter; that inability is the whole reason automation
exists. So **completeness must live in the *control available* at Layer 3, not in what the human
eyeball processes.** The correct shape:

- At Layer 3 the human reads **structure** — the graph plus a handful of derived quantities.
- The **parameter-level optimization is delegated** to the subprocesses the player has written.

Therefore: **Layer 3's interface design and the rule-DSL design are the same problem.** The abstract
command surface simply *is* the player's subprocess-authoring surface. If Layer 3 dumps raw
parameters, it is noise; if it shows *structure + delegation*, it is mastery. (The parsimony
pressure on the Architect's policy from `02` does a second job here — it keeps the abstract view
inside a vocabulary a human can still read and write.) This is another convergence onto the
shared-vocabulary spine.

## Player-authored automation — the keystone that de-wishes the enemy design

**[DECIDED]** Give the player the **same** prioritized `condition → action` DSL the Architect
evolves. This is what makes the whole edifice cohere rather than read as wishful: "the AI's policy"
and "the player's program" become the *same kind of object*. Reading the glass genome *is* learning
to write your own subroutines; the Gladiabots feel stops being aspirational because the mechanism
that produced it there — author behavior, watch it run, iterate — is now literally present.

The arc it delivers: **operator → programmer → meta-programmer.** Early, you move fleets by hand;
mid, you write rules to shed tedium; late, you tune a policy against the enemy's policy, and the
contest becomes *whose authored-or-evolved program is better* — you versus the Architect, in one
substrate. It also closes the abstract-map loop: when topology outruns intuition, the answer is not
to simplify the game but to **raise your level of abstraction and write a subroutine for what your
eyes can't track.** (The hidden-mix capstone creates the *need* for inference; the DSL supplies the
*means*.)

**Practical stance:**
- A **visual editor** over the primitives — text/code would gut the intended audience.
- **Hybrid control**: standing orders plus manual override, so the player is never reduced to a
  spectator.
- **Grow the DSL across the campaign** — each newly unlocked primitive is one new concept, which
  also makes the DSL the game's cognitive-load pacing mechanism.

## The map-abstraction ramp (resolved)

**[DECIDED]** Abstract-vs-spatial is an *axis*, not a binary. Spatiality is a large readability
subsidy and must dominate early (low cognitive load while learning the vocabulary); full topological
opacity (à la *Hunt the Wumpus*) must never arrive before competence. But abstraction is
thematically the entire point — a graph is the machine's native representation, so forcing the
player into topological reasoning is teaching them to think like the AI and deliberately narrowing
the human–AI representational gap. So **ramp it:** early maps planar and Euclidean → teleporters
start breaking distance-intuition → non-planar graphs → maps where you see *connectivity but not
geometry*. The abstraction of the map *becomes* the embodiment of "going beyond naïve human spatial
capability," gated behind mastery.

## **[DECIDED — honest tradeoff]** Why the player ascends (the sharp version)

Not "manual gains become marginal in the abstract," but: **the player's *manual contribution*
becomes marginal *relative to what automation achieves*, because the lower layers are exactly where
machines beat humans.** Layer-1 micro is fast, parallel, optimization-under-known-rules — the
AlphaStar regime, where superhuman micro was considered so unfair it had to be **capped** to keep
the contest about strategy. The player cedes Layer 1 to automation not because it stops mattering
but because the machine is simply *better* there — and **that felt experience of being out-executed
and choosing to delegate is the lesson.**

Consequences to hold honestly:
- Layer 1's job is **transferable intuition** (it makes the Layer-3 numbers *mean* something) plus
  raw **appeal** — *not* a durable strategic edge.
- This makes the **most art-expensive layer the least strategically central.** The always-on
  spectacle decoupling is what redeems that spend: Layer 1's 3D assets get **reused as ambient
  render across the entire game**, not only during a phase the player outgrows.

## **[OPEN]** The decision that sizes everything downstream: ladder vs. toggle

Are the layers a **one-way ladder** (ascend and abandon each scale) or a **fluid toggle** (drop to
Layer 1 for a critical battle, sit at Layer 3 for logistics, *permanently* available, with the
**center of gravity** of attention drifting upward as battlefields scale past human bandwidth)?

**[LEAN: fluid toggle.]** Rationale: it is the only version that keeps the art budget paying off
*and* preserves a durable human role — the player is never a pure spectator but an **orchestrator**
who still parachutes into the moments that matter. It also sets *where you contest the AI*: early,
you out-strategize a machine that is beating you on Layer-1 micro; late, the arms race is at Layer 3
in **policy space** (the dominant-policy-collapse question in `04`). Human and AI both climb the
ladder and meet at the top.

This single call sets the DSL's ambition, the UX budget, the steepness of the map-abstraction ramp,
and how hard the four opponents lean on the shared substrate. It is coupled to the second open
question in `04` (how central player-authored automation is). **[LEAN on that one: central]** — the
convenience-layer version is "just a nicer Solarmax" and does not pay off the "feel the richness of
machine intelligence" promise.
