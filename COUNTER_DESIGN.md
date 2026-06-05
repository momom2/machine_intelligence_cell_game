# COUNTER_DESIGN.md — the Counter opponent (opponent-modeling + modular best-response)

Status: **design / spec for implementation.** The Counter is the arc-2 opponent from
`02-ai-opponents.md`, but built richer than the original "decision-tree → fixed repertoire":
it **observes** an opponent, **profiles** their policy approximately in our own legible
vocabulary, and **synthesizes a counter** that blends a robust rock-paper-scissors backbone
with projection-validated exploits of specific weaknesses.

This document is the agreed design (the rundown the project owner signed off). It changes no
code; it is the reference the implementation follows.

Contents: 1) the key insight · 2) scope & the P_max axis · 3) data to collect · 4) inference ·
5) counter synthesis · 6) the playstyle/robustness dial · 7) test plan & interpretation ·
8) pitfalls & caveats · 9) implementation plan · 10) deferred/speculative · 11) references.

---

## 1. The key insight — reuse the substrate, borrow only the dials

The computer-poker robust-counter line (RNR, DBR, safe exploitation) had to build three things
from scratch with tabular CFR. **We already have all three**, which dissolves the "CFR does not
scale to a real-time game" caveat:

| Poker needed… | We already have |
|---|---|
| a representation to model the opponent in | the **DSL / `ai::vocab`** (legible condition→action primitives) |
| a fast evaluator for best-response (CFR) | the **mean-field projection** (`world::project_forward`, ~30 µs/call) + the headless sim |
| a structured response space | the **automaton roster** + the validated **RPS cycle** |

So the Counter is **opponent-modeling + best-response over our own primitives**, importing the
literature's *robustness dials and pitfalls* — not its solvers.

## 2. Scope & the P_max axis

**In scope (v1):** observe an opponent's play → build a confidence-weighted profile → play a
counter = (RPS backbone) + (projection-validated module exploits), with character set by `P_max`.

**`P_max` is a PLAYSTYLE axis, not a difficulty knob.** It runs from *robust generalist* (lean on
the RPS backbone) to *vulnerability hunter* (lean on specific exploits). Its effect is
**non-monotonic** — which end wins depends on how adaptive/volatile the opponent is — so it is a
lever for giving different Counter *instances* distinct personalities (useful for later narrative),
**not** a "harder/easier" slider.

**Deferred (see §10):** continuous within-match re-learning, and a dramatic mid-match
"the Counter just flipped its whole policy based on what it learned" event. v1 infers from
accumulated observation; the targets it is tested on (fixed automata) are stationary within a
match, so within-match adaptation is not needed yet.

## 3. Data to collect

Log every opponent **decision** as a *(situation → choice)* pair, in the game's own legible
vocabulary — so inference is over the same primitives the automatons and the player use.

- **Choice** — abstracted to the *kind* of move, never raw node ids:
  `Colonize(neutral)` · `Reinforce(threatened own node)` · `Strike(enemy node)` · `Hold` —
  plus the fraction bucket committed and the (categorical) source/target roles.
- **Situation** — the legible features at that instant: my/their territory & ship share, frontier
  pressure, whether anything of mine is threatened, and the resistance/denial/soft-cap state.
- **Inaction** — declining to move is itself a tell (a turtle defends by *not* over-extending), so
  log "no-op" decisions too.

Principle: **bucket the situation** into a small discrete key
(`ownership × garrison-band × neighbor-threat × frontier`) so observations accumulate per
*situation-type*. These buckets are the unit of both inference (§4) and confidence (§6) — they are
the game's analog of an "information set". Coarse enough that counts accrue; fine enough that the
profile is exploitable.

## 4. Inference — an approximate, confidence-weighted profile

Do **not** reconstruct the opponent's exact program. Estimate their **tendencies** in our
vocabulary, at two grains:

- **Strategic mix** — the coarse colonize/defend/attack proportions (the simplex point), which
  places them in RPS space. This is the arc-1 capstone's mix-inference, generalized to run from a
  rolling log instead of a one-shot scout.
- **Per-module tendencies** — simple statistics over the bucketed data: *guards-rear?*,
  *over-commits-attacks?*, *hoards past the soft cap?*, *expands nearest-first / rate-greedy?*,
  *abandons a losing grind or feeds it?* Each module is a predicate computed from situation→choice
  frequencies.

Principles:
- Keep only the **mean** behaviour per bucket — by Theorem 2.1 (Ganzfried & Sun) you respond to the
  mean strategy, not a full distribution.
- **Tag every estimate with a confidence = how often that situation was seen** (`n_I`). You "know"
  behaviour where you have many observations and stay agnostic where you do not — this is what stops
  the Counter over-trusting a thin sample (the documented RNR overfitting failure).
- **Recency weighting** (exponential decay) is included as the hook for future non-stationarity; in
  v1, against stationary automata, simple accumulation suffices.

Output: not a rigid clone — a confidence-weighted profile, e.g.
`mix ≈ 60/30/10 colonize/attack/defend; modules: never-guards-rear [high-conf], over-commits [low-conf]`.

## 5. Counter synthesis — modular best-response, projection-validated

Two layers, blended by `P_max` and per-bucket confidence, **every candidate validated in the
projection before use**:

- **Robust backbone (RPS).** From the inferred mix, pick the countering automaton from our
  repertoire (infer-Colonize → play Attack; infer-Attack → Defend; infer-Defend → Colonize). It is
  never worse than a validated pure strategy and is the fallback wherever confidence is low.
- **Targeted exploit.** For each *high-confidence* weak module, synthesize a response over
  `ai::vocab` (e.g. *never-guards-rear* → flank the undefended rear; *over-commits* → defend then
  counter-punch the emptied rear; *hoards* → out-tempo while the cap bleeds them). **Keep an exploit
  only if the mean-field projection confirms it actually beats the inferred model** — ship
  simulated-real counters, not wishful pattern-matches.
- **Blend & escalate safely.** `P_max` sets the character (backbone ↔ exploits); per-bucket
  confidence gates *which* exploits are trusted; and an exploit is escalated only as far as the
  opponent's actual mistakes — the "gifts" the projection scores as value left on the table — afford
  (the safe-exploitation principle; credited in expectation, not by who won the last skirmish). A
  clean opponent thus draws the safe backbone; a sloppy one gets punished.
- **Announce it** (legibility, per `02`): surface the current read + counter
  ("Read: rush → countering defensively") — the intended Gladiabots-style "feel".

**Net mental model:** *observe → profile-with-confidence → counter = (RPS backbone) +
(projection-validated module exploits), dialled by the `P_max` playstyle axis.*

## 6. The playstyle / robustness dial (named techniques, adapted)

The literature's exploitation-vs-robustness machinery, mapped onto our substrate:

- **DBR (Data-Biased Response)** — the recommended core. Per-bucket confidence
  `P_conf(I) = P_max · n_I/(s + n_I)` (s-curve): trust the model in proportion to observations,
  fall back to the backbone where sparse. Two dials: the global ceiling `P_max` (the playstyle axis)
  and the curve shape `s`. Strictly preferred over RNR's single global `p`, which **overfits on
  sparse data** (becomes both more exploitable *and* less exploitive than safe play).
- **Safe-exploitation / "gifts"** (Ganzfried & Sandholm) — only exploit as much as the opponent's
  mistakes afford; a territory-grind game has gifts (over-committed attacks, undefended nodes), so
  bounded exploitation is feasible (unlike rock-paper-scissors, which has none).
- **No tabular CFR** — best-response is *evaluated* by the projection / mean-field sim and *searched*
  over the repertoire + `ai::vocab`, not solved over an information-set tree.

## 7. Test plan & interpretation (the diagnostic)

Run the Counter vs each of {Colonize, Defend, Attack, SimpleColonizer}, sweeping `P_max`. The
Counter doubles as a **diagnostic for the automata** (the project owner's framing):

- **Ideal:** at moderate `P_max`, the Counter converges on the RPS counter to each pure automaton
  and exploits SimpleColonizer's thin-rear seam.
- **Brittle exploit found** → signal to **patch that automaton** (the exploit is a real, patchable
  flaw a human would not fall for).
- **A mixed counter beats the *intended* RPS counter** → signal to **refine that automaton** (without
  breaking its core playstyle) — it is weaker than designed on some axis.
- The `P_max` sweep should trace the concave exploitation↔robustness frontier on our game; report
  win-rates (both seatings, multiple seeds) and the inferred profile vs ground truth per target.

## 8. Pitfalls & caveats (from the research)

- **Do not ship a raw best-response** to logged actions — empirically brittle, collapses on
  deviation. The DBR confidence + RPS backbone are the guard.
- **Sparse single-match data** — far fewer observations than poker's ~100k threshold, so gate
  confidence per-bucket (DBR), never crank a global `p`.
- **Domain-transfer gap** — every cited guarantee is for 2-player zero-sum, often finitely-repeated,
  imperfect-info games (poker). Our game is deterministic, real-time, single-match. Treat the knobs
  and pitfalls as **well-founded design principles, re-validated empirically in our harness** — the
  theorems do not transfer verbatim.
- **State abstraction risk** — buckets too fine → `n_I` never accrues (no trust); too coarse → the
  profile is unexploitable. This is the main thing to tune.

## 9. Implementation plan

In `crates/ai` (alongside the automatons it reuses), a new `counter` module:
- `counter::observe` — the observation log: records `(situation-bucket → choice)` for a watched
  faction during a match (a hook in the AI-vs-AI harness / controller records the opponent's issued
  orders + the pre-decision features).
- `counter::profile` — inference: rolling per-bucket frequencies → strategic mix + per-module
  tendencies + per-bucket confidence (`n_I`). Output a `OpponentProfile`.
- `counter::synthesize` — counter synthesis: RPS backbone pick + projection-validated `ai::vocab`
  module exploits, blended by `P_max` + confidence + gift budget → the per-tick orders.
- `Roster::Counter { p_max }` — a new roster entry wiring the pipeline as an `AiController` policy.
- A diagnostic in `ai::harness` (or a small bin): Counter vs each of the four targets, `P_max`
  sweep, both seatings, multiple seeds; prints inferred-profile-vs-truth + win-rates + the
  brittle/refine signals.

Determinism preserved (the projection draws no RNG; the profile is a deterministic function of the
log). Reuses `world::project_forward`, `ai::vocab`, the automaton roster, and the existing harness.

## 10. Deferred / speculative (not built now)

- **Within-match continuous re-learning** + a dramatic **mid-match policy flip** ("the Counter just
  re-read you and switched") — a cool future beat; needs change-point detection (BPR+ style, knobs
  ρ/n) on a slow RTS reward signal. Marked speculative.
- **Cross-match learning** (a Counter that remembers a specific human across games) — future.
- **RWYWE expected-payoff budget accounting** as a formal safety floor — adopted here only as the
  informal "escalate within gifts" principle; the formal repeated-zero-sum guarantee is out of scope.

## 11. References (primary sources from the literature pass)

- Johanson, Zinkevich, Bowling — *Computing Robust Counter-Strategies* (NIPS 2007): **RNR**, the
  `p` knob, ε-safe best response, the concave frontier.
- Johanson & Bowling — *Data Biased Robust Counter Strategies* (AISTATS 2009): **DBR**,
  per-information-set `P_conf`, the sparse-data overfitting result.
- Ganzfried & Sandholm — *Safe Opponent Exploitation* (TEAC 2015): **gifts** theorem; RWYW vs RWYWE.
- Ganzfried & Sun — *Bayesian Opponent Exploitation* (2016): **best-respond-to-the-mean**
  (Theorem 2.1); Lipschitz payoff continuity.
- Wang et al. — *Balancing Safety and Exploitability in Opponent Modeling* (AAAI 2011):
  confidence-set / interval models with a data-gated safety floor.
- Hernandez-Leal et al. — **BPR+** (AAMAS 2016) & the non-stationarity survey (JAAMAS 2017):
  change-point detection of strategy switches (for the deferred within-match work).
