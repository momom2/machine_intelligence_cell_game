# The simulation model

This is a reference for how the game *actually behaves* — the rules the `layer1` crate
enforces. It is the "explanation" layer: read it to understand why a fight or a capture plays
out the way it does. The authoritative numbers live in `layer1::SimParams` and the `sim`
module constants; the values quoted here are the defaults at the time of writing and are
labelled as such.

The whole simulation is **headless and deterministic**: no graphics, one seeded PRNG, so a
given `(board, seed, order stream)` replays bit-for-bit. See [architecture.md](architecture.md)
for why that matters.

## The board

A match is **one `Interior`** — a single structure made of several **sub-structures**
(`SubStructure`) placed on a 2D plane, plus the discrete **ships** that garrison and fight over
them. There is no map graph and no second zoom layer; the whole game happens inside this one
interior.

Each sub-structure has:

- an **owner** — `Player`, an AI seat `Ai(i)`, or `Neutral`;
- a **storage capacity** (default 60) — the no-attrition headroom of idle ships it can hold
  before surplus starts to bleed. It also sets the sub's drawn **radius** (`√capacity`-scaled);
- a **production** rate (default 1) — ships minted per production period. Higher production
  means faster output *and* a higher effective ceiling;
- a **kind** (below).

Radius is cosmetic-plus-capture only: it scales the garrison ring and the footprint that
confers the defender edge, but it does **not** set combat range (that is a fixed constant).

### Special sub kinds

Beyond the ordinary `Standard` producer, three kinds each add one rule:

- **Fortress** — produces nothing; high capacity and very high resistance. While owned, its
  idle garrison fires at a **long overwatch range** (≈18 wu, far past the basic engagement
  radius): attackers inside the ring are shot but cannot shoot back until they close.
- **Teleporter** — produces nothing; ships its **owner** sends *away from it* arrive at their
  destination instantly once their undock delay burns out (no transit flight). Everyone else's
  ships leave it as ordinary movers.
- **Shipyard** — extreme production, negligible resistance, so it flips to any lone visitor
  almost instantly: a high-value prize that is trivially stolen. A level may put a neutral yard
  behind a one-time activation grind instead.

## The economy

Production is the reason to hold ground and the fuel of the snowball. An owned, producing sub
spawns `production` ships per `production_period` (default 18 ticks), one at a time at its
production squares, and the new ship glides out to the orbit ring.

Ships above a sub's storage capacity are **surplus** and bleed off gently — a linear per-sub
attrition, so a sub settles at an effective plateau of roughly `capacity + K·production` (≈120
for the defaults) rather than snowballing without limit. It is a soft plateau, not a wall:
ships **in transit are exempt**, so the way to keep surplus is to spend it or keep it moving.

## Capture is a grind, not a flip

Taking a sub is a **resistance grind**, not an instant flip. Every sub carries a `resistance`
bar in `[0, max]`, starting full; the default max is proportional to storage capacity
(`capacity · 60` ≈ 3600 for a default sub, so bigger subs are harder to take). Each tick,
looking at which factions are **garrisoned** at the sub (home-based, so a ship merely passing
through does not count):

- **one lone foreign faction present** → resistance erodes by its present-ship count. At `≤ 0`
  the sub **flips** to that faction and the bar **refills**. More attackers present ⇒ a
  proportionally faster grind.
- **only the owner present** → resistance **heals** back toward max. A returning defender
  repairs the bar, so **hit-and-run accomplishes nothing** — an attacker must hold enough
  presence to out-erode the heal all the way to zero.
- **nobody, or two-plus factions present** → the grind is **frozen**. You must win the
  firefight before a capture can advance.

**Production denial** rides on the same presence check: a sub being eroded *undefended* (one
foreign faction present, owner absent) **stops producing**. So parking on an enemy sub starves
its output *before* you ever capture it — real economic damage at less than the price of a full
siege. A sub the owner still defends keeps its line running.

## Combat

Combat is **purely positional**. Define a fixed **engagement radius** `R` (default 3.5 wu,
independent of any sub's size). Each tick, every ship with a living enemy within `R` is
*engaged* — so ships near the border between two close subs fight *across* them; being in the
same sub is not required. The **layout decides who fights whom**.

Fire is stochastic and follows **Lanchester's square law**: each engaged ship fires with some
probability per sub-step and removes a random in-range enemy, so a side's loss rate scales with
the *enemy's* engaged count — **twice the ships is roughly four times the effective power**.
Large battles trend deterministic; small skirmishes stay chancy.

Two refinements shape the *feel* of the interactive game (they are on in the GUI, off in the
headless reference — see the operating points below):

- **Spread fire** — an engaged ship spreads its shot across *all* in-range enemies rather than
  one-shotting a single target. Expected kills are unchanged (the square law and its mean-field
  survive), but attrition feels continuous instead of lumpy.
- **Transit-fire gating** — a ship *in transit* cannot shoot a stationary garrison: an assault
  wave must **land** before it trades, while the defenders fire on the incoming wave. No
  drive-by kills.

## Ships, orbits, and movement

An idle ship physically **orbits** its home sub on a ring — that ring position *is* its combat
position (what you see is where it fights). Ships steer by a social-force model: a faction-blind
short-range **pressure** that spaces a crowd evenly (so opposing clouds interleave into a
salt-and-pepper melee rather than forming a hard front), a **drive** toward the nearest staged
foe when there is a fight to join, and a wartime same-faction **cohesion** that lets a stalemate
coarsen instead of parading around the ring.

An **order** moves a fraction of a source sub's idle ships to a target sub. Orders are
**faction-scoped** (an order only moves *your* idle ships — an opponent can never drag your
ships off a contested sub) and only affect **idle** ships (a ship already in transit is
committed — "send it, then it's flying"). A freshly ordered ship waits out a short **undock
delay** at its slot before it starts moving, then flies straight to its slot on the target ring
at a fixed speed. Ships ordered to a moving (orbiting) sub *lead* it — they never chase.

## The tick

`Interior::step` advances exactly one tick in a **fixed order**, and the order is load-bearing:

```
1. produce           — spawn ships (skipped on denied subs)
2. advance_movement  — movers step toward their target; arrivals go idle
3. resolve_orbit     — idle ships advance their orbit angle and glide to their ring slot
4. resolve_combat    — the stochastic square-law firefight
5. resolve_resistance— capture grind / heal / flip
6. resolve_softcap   — surplus attrition
```

Two facts follow from the order: **combat resolves before capture** (a defender must survive
the firefight to count for the heal; an attacker erodes with its *post-combat* count), and
**capture uses post-movement presence** (a ship that arrives this tick already counts toward
erosion or heal on its arrival tick).

## Win and elimination

A faction is **eliminated** when it has zero ships *and* zero owned subs — it can neither fight
now nor produce later. If exactly one faction is eliminated, the other **wins by elimination**;
otherwise, at the match horizon, the winner is whoever leads on `ships + subs` (an exact tie is
a draw). A **human** match has no horizon — it ends only on a sealed, unrecoverable result; the
horizon is honoured only for AI-driven runs (`--auto`, `--selftest`, capture).

## Pacing and the two operating points

The sim is grounded at **60 logical ticks per second**, but every per-tick quantity is scaled
from a reference rate, so *per-second* behaviour is independent of the tick rate. The game runs
a fixed-timestep accumulator and interpolates positions between ticks, so it renders as smoothly
as the monitor allows while simulating at a fixed 60 Hz. AI seats re-decide on a coarse cadence,
not every tick, so forces commit over time.

Two `SimParams` operating points coexist:

- **`SimParams::default()`** — the headless / AI reference (classic one-shot combat, the legacy
  per-structure soft cap). This is what the AI test suite measures against.
- **`gui_params()`** — the interactive game. It softens combat for a readable feel (much lower
  fire probability, so economy and territory outweigh a single clash) and turns on
  `spread_damage`, `transit_fire_gating`, and `per_sub_attrition`.

The split keeps the spectacle tuning from disturbing the headless reference. The orbit model is
the one exception — it is universal.

## Determinism

The only randomness comes from one embedded seeded PRNG (an inline xorshift64\* — no `rand`
crate). Every determinism-relevant transcendental routes through the pure-Rust `libm` on all
targets, so a browser-recorded run replays bit-identically on native. `Interior::state_hash()`
gives a 64-bit fingerprint of the full state for exact comparison, and cloning an `Interior`
clones its RNG, so a clone replays the timeline tick-for-tick. This is the property the whole
replay system rests on.
