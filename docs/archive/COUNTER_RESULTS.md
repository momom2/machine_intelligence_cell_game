# COUNTER_RESULTS — arc-2 Counter opponent (observe → profile → counter), the diagnostic

> **⚠️ UPDATE 2026-06-05 — numbers below are STALE; a refresh is pending one design decision.**
> The pure automatons were made **complete** (no idling; optimal *within* their priority): `Colonize`
> is now a priority ladder — expand while it can, then **defend** its attacked structures and **strike
> vulnerable** enemy ground once it has conquered every neutral it could (`crates/ai/src/automata.rs`).
> The RPS cycle still closes (`attack>colonize` eased `10–0 → 9–1`; `colonize>defend 10–0`;
> `defend>attack 6–4`). Re-running `counter-diag` after that change gives, per target (5 seeds × 2
> seatings × {0.2,0.6,0.95}):
>
> | target | win-rate (was) | inferred read | note |
> |---|---|---|---|
> | **Colonize** | **10/10** (was 7/10) | `Attack` → plays **Defend** | the read is now *honest*: a complete Colonize attacks ~84% of the match once expanded, and Defend beats that — winning **more** than the textbook Colonize→Attack would (~9/10) |
> | **Defend** | 10/10 | `Defend` → Colonize | textbook, unchanged |
> | **Attack** | **6/10** (was 2/10) | `Attack`/`Colonize` | tracks the thin `defend>attack` (6–4) edge |
> | **SimpleColonize** | 7–9/10 | `Colonize` (+ seam) | unchanged; seam fires |
>
> The old "live-contact contamination" framing (§Verdict/§5 below) is **superseded**: the colonizer no
> longer *dribbles* ships through foe ground (the artifact that produced the spurious attack-read); it
> now deliberately colonizes or attacks, so the strategic read reflects real behaviour. **Open
> decision:** should the Counter read the opponent's opening **priority** (so it *identifies* Colonize
> as colonize — better as a *diagnostic*) or its sustained **behaviour** (current — wins more as an
> *opponent*)? The full rewrite of the tables/interpretation below follows that decision.

> Phase 3 deliverable for `COUNTER_DESIGN.md`. The Counter (opponent-modeling + projection-validated
> modular best-response) run as a **diagnostic for the automata**: `Roster::Counter { p_max }` vs each
> of the four targets — `StrategicPolicy::{Colonize, Defend, Attack}` and `Roster::SimpleColonize` —
> sweeping the `p_max` **playstyle** dial over `{0.2, 0.6, 0.95}`, both seatings, 5 seeds (`[1, 7, 42,
> 100, 2024]`), to the standard `3000`-tick horizon. Everything is deterministic (the mean-field
> projection draws no RNG; the profile is a deterministic function of the observation log), so every
> number below is bit-reproducible. Reproduce with `cargo run -p ai --bin counter-diag --release`.

## Verdict

1. **Does the Counter converge on the RPS counter to each pure automaton and win?** **PARTLY — one of
   three pure corners is a clean win; the other two expose real problems (one in inference, one in the
   automata themselves).**
   - **Defend → clean RPS win.** The Counter reads Defend's corner correctly (mix 13/**87**/0), plays
     the **Colonize** backbone, and **sweeps 10/10 at every `p_max`** (colonize > defend). Textbook.
   - **Colonize → wins, but for the WRONG reason.** The Counter **mis**reads the live colonizer as
     *attack*-dominant (mix 21/13/**66**) and so plays the **Defend** backbone — the *anti*-counter —
     yet still wins **7/10**. It is leaving the **1.00** pure-`attack > colonize` edge on the table.
   - **Attack → loses (2–3/10).** The intended counter is Defend, but `defend > attack` is the
     **weakest** edge in the cycle even for pure automata (**0.56**), and the Counter's live-read of
     Attack is unstable — so it under-performs even that thin pure edge.
2. **Does it exploit SimpleColonizer's thin-rear seam?** **YES.** The seam (`never_guards_rear`)
   **fires**, the projection **confirms** the `flank-undefended-rear` exploit, it **ships**, and the
   win-rate climbs **7/10 → 9/10** as `p_max` rises from 0.2 to 0.6/0.95. The documented seam is real
   and exploitable exactly as designed.
3. **Is `p_max` a difficulty knob or a playstyle axis?** **A playstyle axis with a non-monotonic
   win-rate effect** — see §"The p_max sweep". More `p_max` is *not* uniformly better: it helps vs
   SimpleColonize (7→9), is flat vs Colonize/Defend, and is *worse* vs Attack at the extremes (the
   greedy-exploit read 0.95 < the balanced 0.6).

**Bottom line.** The Counter *mechanism* works where the substrate is clean: it reads a turtle and
plays the textbook colonize counter for a 10/10 sweep, and it confirms-and-ships the SimpleColonize
seam exploit for a measurable win-rate bump. But used as a diagnostic it surfaces **two honest
problems**: (a) a **Counter-side inference defect** — *live contact contaminates the strategic read*,
so an actively-contested colonizer reads as an attacker and the Counter forfeits a 1.00 edge by
playing the wrong backbone; and (b) an **automaton-balance signal** — the `defend > attack` edge is
**marginal (0.56)**, the soft link in the RPS cycle, and the Counter (which leans on it) loses to
Attack. Neither is hidden; both are reported below with the specific patch/refine they point to.

## Setup

- **Counter pipeline:** `observe` (per-source `(situation-bucket → choice)` log of the opposing seat)
  → `profile` (`OpponentProfile::infer`: strategic mix + per-module tendencies + per-bucket
  confidence `n_I`) → `synthesize` (RPS backbone + projection-validated `ai::vocab` exploits, blended
  by `p_max` + DBR confidence + a gift margin). Driven by `CounterController` (accumulate-then-counter:
  it re-infers and re-derives the counter every decision tick from the growing log; no mid-match flip).
- **Final params** (the dials the sweep ran under): `DBR_CURVE_S = 12.0`, `EXPLOIT_TRUST_FLOOR =
  0.25`, `GIFT_MARGIN = 0.5`; module thresholds `MIN_MODULE_CONFIDENCE = 8`, `SEAM_THRESHOLD = 0.12`,
  `HOLD_AS_DEFEND_WEIGHT = 0.15`; decision interval `8`, horizon `3000`.
- **Maps:** the three pure automata on the **diamond** (where the validated `attack > colonize >
  defend > attack` cycle closes); `SimpleColonize` on the **corridor** (a single forward axis, so its
  thin-rear seam is geometrically clean to flank). Both seatings × 5 seeds = **10 matches per cell**.
- **Inference window:** unlike the Phase-2 inference *gate* (which scouted each target against a
  **Passive** sparring partner over a 500-tick opening), the diagnostic infers from the **live,
  contested** match — the Counter is fighting back the whole time. That difference is the source of
  finding (a); see §"Why the live read differs from the gate".

## Headline numbers

The Counter's win-rate vs each target (over 10 matches/cell), next to the **pure** RPS counter's
win-rate on the same diamond (the never-worse fallback the Counter is *supposed* to realize). Pure
baseline: `cargo run -p ai --bin ai-harness --release` (8 seeds × both seatings = 16 games/edge).

| Target | RPS counter (intended) | pure-counter WR (baseline) | Counter WR @0.2 / 0.6 / 0.95 | Counter realizes the edge? |
|---|---|---|---|---|
| **Defend** | Colonize | **1.00** (colonize>defend) | **1.00 / 1.00 / 1.00** | **Yes — fully** |
| **SimpleColonize** | Attack (+ seam) | 1.00 (attack>colonize)\* | **0.70 / 0.90 / 0.90** | Yes — + seam exploit bump |
| **Colonize** | Attack | **1.00** (attack>colonize) | 0.70 / 0.70 / 0.70 | **No — misreads, plays Defend** |
| **Attack** | Defend | **0.56** (defend>attack) | 0.20 / 0.30 / 0.20 | **No — below even the thin edge** |

\* SimpleColonize is run on the corridor (where the pure cycle is *looser*), so its baseline is the
diamond `attack>colonize` for orientation only; the SimpleColonize result stands on its own seam.

## Per-cell diagnostic (the real `counter-diag` output, pasted verbatim)

```
== Counter diagnostic (COUNTER_DESIGN §7) — deterministic; 10 matches/cell (both seatings × 5 seeds) ==

--- TARGET: Colonize (truth: Colonize-dominant) on the diamond ---
   p_max |   win-rate |  inferred |        mix col/def/atk |  seam | converged on (backbone + exploits)
  ------------------------------------------------------------------------------------------------------------
    0.20 |    7/10    |  Attack X |     21/  13/66      | fires | Defend (backbone only)
    0.60 |    7/10    |  Attack X |     17/   5/78      | fires | Defend + [counterpunch-emptied-rear, flank-undefended-rear] (14% of ticks)
    0.95 |    7/10    |  Attack X |     17/   5/78      | fires | Defend + [counterpunch-emptied-rear, flank-undefended-rear] (14% of ticks)

--- TARGET: Defend (truth: Defend-dominant) on the diamond ---
   p_max |   win-rate |  inferred |        mix col/def/atk |  seam | converged on (backbone + exploits)
  ------------------------------------------------------------------------------------------------------------
    0.20 |   10/10    | Defend OK |     13/  87/0       |     - | Colonize (backbone only)
    0.60 |   10/10    | Defend OK |     13/  87/0       |     - | Colonize (backbone only)
    0.95 |   10/10    | Defend OK |     13/  87/0       |     - | Colonize (backbone only)

--- TARGET: Attack (truth: Attack-dominant) on the diamond ---
   p_max |   win-rate |  inferred |        mix col/def/atk |  seam | converged on (backbone + exploits)
  ------------------------------------------------------------------------------------------------------------
    0.20 |    2/10    | Colonize X |     55/   3/42      | fires | Attack (backbone only)
    0.60 |    3/10    | Attack OK |     41/   3/56      | fires | Defend + [counterpunch-emptied-rear, flank-undefended-rear] (14% of ticks)
    0.95 |    2/10    | Colonize X |     48/   3/49      | fires | Attack + [counterpunch-emptied-rear, flank-undefended-rear] (11% of ticks)

--- TARGET: SimpleColonize (truth: Colonize-dominant) on the corridor ---
   p_max |   win-rate |  inferred |        mix col/def/atk |  seam | converged on (backbone + exploits)
  ------------------------------------------------------------------------------------------------------------
    0.20 |    7/10    | Colonize OK |     53/  47/0       | fires | Attack (backbone only)
    0.60 |    9/10    | Colonize OK |     50/  50/0       | fires | Attack + [flank-undefended-rear] (4% of ticks)
    0.95 |    9/10    | Colonize OK |     50/  50/0       | fires | Attack + [flank-undefended-rear] (4% of ticks)

--- summary (target,p_max -> counter_wins/games, inferred(ok?), seam, exploit_rate) ---
  Colonize       p=0.2   7/10  inferred=Attack(X)  seam=Y  exploit_rate=0.00
  Colonize       p=0.6   7/10  inferred=Attack(X)  seam=Y  exploit_rate=0.14
  Colonize       p=0.95  7/10  inferred=Attack(X)  seam=Y  exploit_rate=0.14
  Defend         p=0.2  10/10  inferred=Defend(ok)  seam=n  exploit_rate=0.00
  Defend         p=0.6  10/10  inferred=Defend(ok)  seam=n  exploit_rate=0.00
  Defend         p=0.95 10/10  inferred=Defend(ok)  seam=n  exploit_rate=0.00
  Attack         p=0.2   2/10  inferred=Colonize(X)  seam=Y  exploit_rate=0.00
  Attack         p=0.6   3/10  inferred=Attack(ok)  seam=Y  exploit_rate=0.14
  Attack         p=0.95  2/10  inferred=Colonize(X)  seam=Y  exploit_rate=0.11
  SimpleColonize p=0.2   7/10  inferred=Colonize(ok)  seam=Y  exploit_rate=0.00
  SimpleColonize p=0.6   9/10  inferred=Colonize(ok)  seam=Y  exploit_rate=0.04
  SimpleColonize p=0.95  9/10  inferred=Colonize(ok)  seam=Y  exploit_rate=0.04
```

(`inferred` is the majority dominant-axis vote across the cell's 10 final reads; `OK`/`X` = matches
ground truth; `mix` is the pooled mean simplex point; `seam` = `never_guards_rear` fired in any match;
the convergence column names the backbone the cell converged on plus any projection-confirmed exploit
that shipped, with its share of the Counter's decision ticks.)

### Pure-automaton RPS baseline (the diamond, for comparison)

```
--- Diamond cycle over many seeds (both seatings each) ---
  attack > colonize: A=16 B=0 draws=0 over 16 games  (A win-rate 1.00) => edge holds
  colonize> defend : A=16 B=0 draws=0 over 16 games  (A win-rate 1.00) => edge holds
  defend > attack  : A=9 B=7 draws=0 over 16 games   (A win-rate 0.56) => edge holds
```

## Interpretation (the project owner's lens)

### 1. Convergence — did the Counter play the RPS counter and win, per corner?

- **Defend: YES, the textbook result.** Read = defend-dominant (right corner), backbone = **Colonize**
  (`infer-Defend ⇒ Colonize`), win-rate **10/10 at every `p_max`**. The Counter realizes the full 1.00
  `colonize > defend` edge. Nothing to fix here — this is the showcase "it read you and countered" beat.
- **SimpleColonize: YES, and it also punishes the seam.** Read = colonize-family (right), backbone =
  **Attack** (`infer-Colonize ⇒ Attack`), **plus** the confirmed `flank-undefended-rear` exploit.
  Win-rate **7/10 → 9/10** as `p_max` opens up the exploit. The seam works as the design promised.
- **Colonize: WINS, but does NOT converge on the intended counter.** The read is **wrong** — it lands
  *attack*-dominant — so the backbone is **Defend** instead of the intended **Attack**. The Counter
  still squeaks 7/10 (its greedy internals + early expansion carry it), but it is *not* the RPS path,
  and it forfeits the 1.00 `attack > colonize` edge a correct read would have taken. This is an
  **inference** problem, not a backbone problem (the backbone machinery is correct — it is being fed a
  wrong mix).
- **Attack: LOSES.** Read is unstable (Colonize at the extremes, Attack only at the middle), and even
  the one cell that reads right and plays **Defend** (p=0.6, 3/10) does worse than the thin pure
  `defend > attack` baseline (0.56). Two compounding causes (see §3).

### 2. The thin-rear SEAM — confirmed exploitable

The `never_guards_rear` seam **fires on SimpleColonize** in every cell, the projection **confirms**
the flank, and `flank-undefended-rear` **ships** (4% of decision ticks at `p_max ≥ 0.6` — selective,
exactly as designed: it fires only when the projection scores a strict edge, not constantly). The
win-rate moves **7/10 → 9/10** when the exploit is allowed. **The documented seam is real.**

> **Honest caveat — the seam also fires on Colonize *and* Attack.** `never_guards_rear` is a
> *rear-strip* statistic; *any* aggressive forward-shipping automaton (a pure colonizer pouring its
> rear forward, an attacker stripping its rear to mass) trips it. So "seam fires" is **not** a
> SimpleColonize fingerprint — it is a colonizer/aggressor fingerprint. The exploit only *ships* where
> the projection confirms a gift (SimpleColonize on the corridor: yes, +2 wins; Colonize/Attack on the
> diamond: it ships some ticks but the win-rate does not improve — the gift is marginal there). The
> gate's confidence + projection-validation is doing its job: a fired module is a *candidate*, not an
> automatic exploit.

### 3. Signals for the automata

**REFINE the Defend↔Attack matchup (the soft link in the RPS cycle).** The pure `defend > attack`
edge is only **0.56** (9–7 over 16 games) — far weaker than the other two edges (both 1.00). It is the
cycle's weakest link, and the Counter, which must lean on it to beat Attack, **fails (2–3/10)**. The
specific refine, *without breaking either playstyle*:
- Either **strengthen Defend's counter-punch** — after it survives the over-committed Attack wave, it
  should press the attacker's *emptied rear* harder/sooner (today its half-commitment reinforcement is
  cautious enough that a fresh Attack wave re-arrives before Defend converts the counter-punch), or
- **soften Attack's over-commitment** a touch so a surviving defender reliably rolls up the thinned
  rear (today Attack re-masses fast enough to stay ahead on tempo even after a repelled wave).

  Diagnostic hook: the `counterpunch-emptied-rear` exploit *does* ship vs Attack (14% of ticks at
  p=0.6) but moves the win-rate from 2/10 only to 3/10 — i.e. even an actively-counter-punching policy
  barely dents Attack. That is the measurement that says the matchup, not just the Counter, is thin.

**SimpleColonize seam — "PATCH only if you no longer want a teaching dummy."** The seam is confirmed
exploitable, but SimpleColonize is *by design* the early-campaign everyman whose documented lesson is
the thin rear. So this is a **deliberate, working** flaw, not a bug. **If** you want SimpleColonize to
stop being so flankable (e.g. promote it past the tutorial), the patch is concrete: have it **post a
small rear guard** (retain a garrison on a lane-interior source when a reachable foe exists) instead of
shipping its whole rear surplus forward — that single change is what flips `never_guards_rear` from
*fires* to *quiet* and removes the `flank-undefended-rear` gift. As long as it is the teaching opponent,
**leave it** — the Counter exploiting it on cue is the feature.

**No *brittle* (must-patch) exploit was found.** Every exploit the Counter shipped was either
(i) the *intended* SimpleColonize seam, or (ii) a marginal flank/counterpunch on the diamond that the
projection let through but that did **not** translate into stolen wins (the win-rate was unchanged or
the cell still lost). There is no case of "the Counter found a cheap trick that beats an automaton a
human never would" — the guardrails (DBR confidence floor + `GIFT_MARGIN` + RPS backbone) held.

### 4. A mixed/Counter line that beats the INTENDED RPS counter?

**Not found — the opposite, in fact.** On every target the Counter is **≤** the pure RPS counter's
win-rate (Defend: equal at 1.00; Colonize/Attack: strictly worse because of the misread; SimpleColonize:
the seam exploit *adds* to the pure edge rather than beating it). So there is no "REFINE this automaton
because a clever mixed line out-performs its designed counter" signal — the automata are not being
out-foxed by a sneaky composition. The gap is the Counter under-realizing edges (inference), plus the
one genuinely thin pure edge (`defend>attack`).

### 5. Why the live read differs from the gate (the inference defect, in detail)

The Phase-2 inference *gate* reads each target correctly (Colonize⇒colonize, Attack⇒attack, …) because
it scouts against a **Passive** partner: a colonizer's surplus then flows onto **empty neutral** ground,
which `classify_destination` tags **Colonize**. In the diagnostic the Counter is **fighting back**, so
the colonizer's surplus increasingly lands on **foe-bearing** nodes (the Counter's forces are *there*),
which the same classifier tags **Strike**. Result: *an actively-contested colonizer reads as an
attacker* (mix 21/13/66), and an attacker's read gets noisy. This is a real, documented limitation of
**within-match** inference vs. a clean opening scout — and it is the root cause of both the Colonize
misread and the Attack instability. It is a **Counter-side** issue (the classifier conflates "ships
toward a node the foe happens to occupy" with "intends to attack"), not an automaton flaw. Candidate
fixes (future work, not done here): classify a move by the *target node's pre-move ownership* (was it a
neutral the colonizer was always going to grab?) rather than by instantaneous foe-presence; or weight
the opening window more heavily (the recency-decay hook in
`profile` exists for exactly this) so the *uncontested* early read dominates the contaminated late one.

### The p_max sweep — a PLAYSTYLE axis, and its NON-MONOTONIC effect

`p_max` runs from **robust generalist** (0.2, lean on the RPS backbone) to **vulnerability hunter**
(0.95, lean into projection-confirmed exploits). Its win-rate effect is **not** a difficulty slider —
it is **non-monotonic and target-dependent**:

| Target | WR @0.2 | WR @0.6 | WR @0.95 | shape |
|---|---|---|---|---|
| Defend | 1.00 | 1.00 | 1.00 | **flat** — backbone already wins; no exploit needed or shipped |
| Colonize | 0.70 | 0.70 | 0.70 | **flat** — exploits ship (14% of ticks) but change nothing (marginal gift) |
| SimpleColonize | 0.70 | **0.90** | 0.90 | **rising then flat** — opening the seam exploit buys +2 wins |
| Attack | 0.20 | **0.30** | 0.20 | **peaks in the MIDDLE** — 0.6 reads right & plays Defend; 0.95's greedier read flips back to a losing Colonize |

The Attack column is the cleanest demonstration that **more `p_max` is not better**: the
vulnerability-hunter end (0.95) is *worse* than the balanced middle (0.6), because at high `p_max` the
Counter over-trusts the contaminated live read and lands on the wrong (Colonize) backbone again. So
`p_max` is a genuine **personality** lever — a robust-generalist Counter (low `p_max`) and a
vulnerability-hunter Counter (high `p_max`) are *different opponents with different failure modes*, not
an easy/hard pair — exactly the COUNTER_DESIGN §2/§6 framing, confirmed empirically.

## Regression coverage

Headline outcomes that are **stable** are locked in `crates/ai/src/harness.rs`
(`harness::tests::diag_*`), so a future change to the inference/synthesis that breaks them is caught:

- `diag_counter_reads_defend_and_beats_it_with_colonize` — the clean RPS win (read Defend ⇒ Colonize
  backbone ⇒ sweep the pure Defender).
- `diag_counter_fires_simplecolonize_seam_and_beats_it` — the seam fires, `flank-undefended-rear`
  ships, and the Counter wins the majority vs SimpleColonize.
- `diag_is_deterministic` — identical inputs ⇒ identical cell (the §9 determinism guarantee).
- `diag_shape_is_well_formed` — the sweep plumbing (rows/cells/games per cell).

The deliberately-**unstable** findings (the Colonize misread; the loss to Attack) are **not**
regression-frozen — they are the honest diagnostic signals documented above, to be revisited if/when
the inference defect and the `defend>attack` edge are addressed.

## What this says for the automata (one-line each)

- **Defend** — healthy as the colonizer-bait corner; **but** its win over Attack is thin (0.56) →
  *refine the counter-punch* (see §3).
- **Attack** — the strongest target here (beats the Counter 7–8/10); the cycle's `defend>attack` edge
  needs propping (refine Defend, or soften Attack's over-commit).
- **Colonize** — fine as an automaton; its only "problem" is that the **Counter** can't read it under
  fire (an inference fix, not an automaton fix).
- **SimpleColonize** — its documented thin-rear seam is confirmed exploitable on cue; keep it as the
  teaching dummy, or post a rear guard to graduate it (see §3).
