# CAPSTONE_RESULTS — arc-1 capstone (scout → infer → counter → execute)

> Build-order **step 4** deliverable. Headless validation of the arc-1 capstone from `02-ai-opponents.md`: a ladder of **hidden-but-fixed** Automatons spanning the colonize/defend/attack simplex, and a player routine that **scouts** an opponent, **infers** its hidden mix from the legible features (`01`/`DSL_SPEC.md`), selects the rock-paper-scissors **counter**, and **executes** it. Everything is built on the deterministic mean-field engine + the shared rule DSL from `cell-core`, so every number here is bit-reproducible (no RNG).

## Verdict

1. **Does infer-then-counter beat a fixed non-inferring baseline?** **YES**
   - Capstone mean win-rate **0.760** vs. baseline **0.656** (Δ = +0.104); mean score margin **+0.518** vs. **+0.400** (Δ = +0.118).
2. **Does difficulty rise with simplex centrality, as the design predicts?** **NO — not supported by measurement**
   - Dense-grid Spearman rank correlation of centrality vs. *read*-difficulty = **-0.445**, vs. *counter*-difficulty = **-0.109**, vs. *combined* capstone difficulty = **-0.048** (28 grid mixes). A positive value would mean central mixes are harder; the measured values are ≈0 or negative.

**Bottom line.** The capstone *mechanism* works: reading the opponent and committing to the matched counter is worth a real, repeatable edge over the same machinery that skips the read. The design's *a-priori difficulty metric*, however, is **not** validated: on this engine and feature set, difficulty is governed by **which axis the mix loads on** — defend is the least legible — not by distance-from-pure. This is exactly the kind of empirically-decided question `04` flags ("cannot be proven up front … must be measured"), and the measurement comes back negative for centrality.

## Setup

- **Operating point** (R2's robust point): `r = 0.600`, `k = 2.250`, `l = 0.150`; `undock_delay = 3`, `combat_rate = 0.50`, `combat_substeps = 8`.
- **Maps**: 3 symmetric maps (corridor7, twin_bridge8, star9), **both seatings** → 6 matches per rung. Horizon 600 ticks. Scout window **50 ticks**, then commit.
- **Ladder**: 16 rungs, ordered by increasing **centrality** (distance-from-pure, normalized so a pure corner = 0 and the balanced centre = 1).
- **Players, all sharing one machinery** (scout-the-same-window, then commit):
   - **Capstone** — commits to the counter of the *inferred* mix.
   - **Baseline** (control) — commits to a **fixed balanced** mix, ignoring the observation. Any gap vs. Capstone is attributable to *reading the opponent*.
   - **Oracle** (perfect *read*) — commits to the counter of the opponent's **true** mix. It isolates reading quality, but note it is **not** an absolute performance ceiling: the counter *rotation* is the theoretical RPS response, which is optimal at the pure corners but only approximate for blends — so on some blended rungs the capstone's (mis-)read lands on a *practically stronger* response than the exact rotation, and beats the oracle. See the note below the headline table.

## Headline numbers

| Metric | Value |
|---|---|
| Capstone mean win-rate | **0.760** |
| Baseline mean win-rate | 0.656 |
| Oracle mean win-rate (ceiling) | 0.750 |
| Capstone mean score margin | **+0.518** |
| Baseline mean score margin | +0.400 |
| Oracle mean score margin | +0.353 |
| Mean inference error (simplex distance) | 0.372 |
| Dominant-lean classification accuracy | 0.625 |

> **On the Oracle scoring *below* the Capstone here:** the Oracle plays the exact RPS *rotation* of the true mix, which is optimal only at the pure corners. For *blended* opponents the rotation is merely approximate, so the capstone's read — which leans toward attack/colonize, the strategies that happen to be strongest on these maps — sometimes lands on a practically better response than the exact rotation. So the Oracle isolates *reading* quality, but is not an absolute ceiling; improving the counter *selection* for blends (beyond a fixed rotation) is the clear next lever, and is what the arc-2 Counter opponent will need anyway.

## Per-rung results (ladder ordered easiest → hardest by centrality)

`cen` = centrality (0 = pure corner, 1 = balanced centre). `inf` = mean inferred mix; `infErr` = its simplex distance to the truth; `corner?` = was the dominant lean read correctly. Win-rates and score margins are over all maps × both seatings.

| # | mix | cen | true (c,d,a) | inferred (c,d,a) | infErr | corner? | cap WR | base WR | orc WR | cap sc | base sc | orc sc |
|---|---|---|---|---|---|---|---|---|---|---|---|---|
| 1 | A | 0.00 | 0.00,0.00,1.00 | 0.37,0.01,0.62 | 0.53 | Y | 0.83 | 0.83 | 0.83 | +0.67 | +0.67 | +0.17 |
| 2 | C | 0.00 | 1.00,0.00,0.00 | 0.61,0.37,0.02 | 0.54 | Y | 0.83 | 0.83 | 0.83 | +0.43 | +0.27 | +0.10 |
| 3 | D | 0.00 | 0.00,1.00,0.00 | 0.42,0.48,0.10 | 0.68 | Y | 0.67 | 0.67 | 0.67 | +0.08 | +0.08 | +0.08 |
| 4 | C2A5 | 0.43 | 0.25,0.00,0.75 | 0.36,0.01,0.63 | 0.17 | Y | 0.67 | 0.83 | 0.50 | +0.33 | +0.67 | +0.00 |
| 5 | C5D2 | 0.43 | 0.75,0.25,0.00 | 0.60,0.40,0.00 | 0.21 | Y | 0.83 | 0.00 | 0.67 | +0.71 | -0.28 | +0.55 |
| 6 | D5A2 | 0.43 | 0.00,0.75,0.25 | 0.47,0.13,0.39 | 0.79 | · | 1.00 | 1.00 | 1.00 | +1.00 | +1.00 | +0.45 |
| 7 | C2A4 | 0.58 | 0.33,0.00,0.67 | 0.34,0.01,0.65 | 0.03 | Y | 1.00 | 1.00 | 1.00 | +1.00 | +1.00 | +1.00 |
| 8 | C4D2 | 0.58 | 0.67,0.33,0.00 | 0.59,0.40,0.00 | 0.10 | Y | 0.33 | 0.00 | 0.50 | -0.10 | -0.39 | +0.07 |
| 9 | D4A2 | 0.58 | 0.00,0.67,0.33 | 0.46,0.13,0.41 | 0.71 | · | 1.00 | 0.67 | 1.00 | +1.00 | +0.33 | +0.48 |
| 10 | C2D2A3 | 0.75 | 0.25,0.25,0.50 | 0.45,0.06,0.50 | 0.28 | Y | 0.50 | 0.33 | 0.17 | +0.00 | -0.33 | -0.67 |
| 11 | C2D3A2 | 0.75 | 0.25,0.50,0.25 | 0.51,0.14,0.35 | 0.45 | · | 0.83 | 1.00 | 1.00 | +0.67 | +1.00 | +1.00 |
| 12 | C3D2A2 | 0.75 | 0.50,0.25,0.25 | 0.55,0.12,0.33 | 0.16 | Y | 0.83 | 0.83 | 0.83 | +0.67 | +0.67 | +0.67 |
| 13 | C3A3 | 0.87 | 0.50,0.00,0.50 | 0.34,0.00,0.66 | 0.22 | · | 0.67 | 0.67 | 0.67 | +0.33 | +0.33 | +0.33 |
| 14 | C3D3 | 0.87 | 0.50,0.50,0.00 | 0.56,0.41,0.03 | 0.11 | Y | 0.50 | 0.00 | 0.50 | +0.17 | -0.29 | +0.21 |
| 15 | D3A3 | 0.87 | 0.00,0.50,0.50 | 0.48,0.09,0.43 | 0.64 | · | 0.83 | 0.83 | 0.83 | +0.67 | +0.67 | +0.20 |
| 16 | bal | 1.00 | 0.33,0.33,0.33 | 0.41,0.07,0.52 | 0.33 | · | 0.83 | 1.00 | 1.00 | +0.67 | +1.00 | +1.00 |

Read the gap between **cap** and **base**: on the rungs where the baseline's fixed balanced mix *loses* (negative `base sc` — e.g. the colonize/defend blends `C5D2`, `C4D2`, `C3D3`), the inferring capstone *wins*. That is precisely the value of reading the opponent, and it is concentrated exactly where a fixed hedge is wrong.

## Does centrality predict difficulty? (the dense-grid study)

The 16-rung ladder is a curated *campaign* path; its small size lets per-matchup idiosyncrasy dominate a correlation. The statistically sound test samples the simplex **uniformly** and densely and correlates centrality against difficulty over all of it. The design's claim is *monotone* ("more central ⇒ harder"), so the headline is the **Spearman rank** correlation; Pearson is shown for reference.

Grid: **28 mixes** at 1/6 resolution (every corner, edge, and interior point), same maps × seatings.

| Difficulty channel | Spearman ρ | Pearson r | Interpretation |
|---|---|---|---|
| **read** (inference error) | -0.445 | -0.456 | **falls** with centrality (contradicts design) |
| **counter** (oracle score-difficulty) | -0.109 | -0.171 | no relationship (does not support design) |
| **combined** (capstone score-difficulty) | -0.048 | -0.093 | no relationship (does not support design) |

Difficulty by centrality band (the concrete shape — if the design held, every column would rise top-to-bottom):

| centrality band | mean read err | mean counter diff | mean combined diff | n |
|---|---|---|---|---|
| ~0.1 | 0.584 | 0.442 | 0.303 | 3 |
| ~0.3 | 0.555 | 0.283 | 0.258 | 6 |
| ~0.5 | 0.389 | 0.203 | 0.244 | 9 |
| ~0.7 | 0.319 | 0.306 | 0.194 | 6 |
| ~0.9 | 0.326 | 0.282 | 0.271 | 4 |

## Why centrality fails — and what *does* drive difficulty

The read-difficulty column actually **falls** toward the centre. The cause is visible in the per-rung `inferred` column: the **defend** weight is systematically under-read (a heavy turtle's `d` is recovered only partially; a defend+attack mix reads as almost pure attack). Defend is intrinsically the hardest axis to see from these observables, because:

- a defender's signature act (reinforce) only fires when *threatened*, so a neutral scout barely provokes it inside the short window;
- every Automaton runs the shared *expand* economy, so a turtle still keeps ships moving — its mobility is not as low as a pure-defend caricature would be;
- the cleanest defend tell (a stocked **rear/interior garrison**) was implemented and tested as a second signal, but on these 7–9-node maps it *washed out* the cleaner mobility gap and mis-read pure defenders as colonizers, so it is excluded from the score (and documented in `infer.rs`). The honest conclusion is that defend is the least legible axis here.

Because the ladder/grid place defend-heavy mixes across the whole centrality range (not concentrated at the centre), the defend-legibility penalty does **not** line up with centrality — so centrality does not predict difficulty. Difficulty is better predicted by *defend content* than by *distance-from-pure*.

This does not weaken the capstone as a game mechanism — inference+counter still pays off — but it means an honest difficulty ramp for arc 1 should be ordered by **measured** win-rate (or by defend content), not by the geometric centrality metric the design assumed up front.

## How to reproduce

```
cargo run -p automaton --bin capstone --release
```

Writes `CAPSTONE_RESULTS.md` (this file), `capstone_results.csv`, and `capstone_results.json` at the repo root. The engine and both players are deterministic (no RNG), so reruns are bit-identical. The asserted regression tests live in `crates/automaton` (`cargo test -p automaton`); the per-rung table and the grid study are also printed by the ignored diagnostics `cargo test -p automaton diag_results -- --ignored --nocapture` and `… diag_inference …`.

---

*Score margin is in [-1, 1] = A's combined (unit + territory) share minus B's, averaged over both seatings; a win is a strictly positive end score (eliminations score ±1). Centrality, inference error, and the correlations are defined in `crates/automaton/src/{mix,infer,capstone}.rs`.*
