//! # `counter::profile` — confidence-weighted opponent inference (COUNTER_DESIGN §4)
//!
//! Reduces an [`ObservationLog`] to an [`OpponentProfile`]: an **approximate, confidence-weighted**
//! read of the watched seat's policy *in our own legible vocabulary*. We do **not** reconstruct
//! their program; we estimate their **tendencies** at two grains (COUNTER_DESIGN §4):
//!
//! * the coarse **strategic mix** — colonize/defend/attack proportions (the simplex point that
//!   places them in RPS space), generalized to run from a rolling log instead of a one-shot scout;
//! * the **per-module tendencies** — [`Modules`]: *guards-rear?*, *over-commits-attacks?*,
//!   *hoards-past-cap?*, *expands-nearest-first / rate-greedy?*, *feeds-a-losing-grind?* — each a
//!   simple statistic over the bucketed `(situation → choice)` frequencies.
//!
//! Principles, straight from the spec:
//! * keep only the **mean** behaviour per bucket (Theorem 2.1 — respond to the mean strategy);
//! * **tag every estimate with a confidence** = how often that situation/slice was seen (`n_I`), so
//!   the Counter knows behaviour where it has many observations and stays agnostic where it does
//!   not (the guard against the documented RNR sparse-data overfitting);
//! * **recency weighting** (exponential decay) is included as the hook for future non-stationarity;
//!   in v1, against stationary automata, simple accumulation (decay `0`) suffices, and is the
//!   default.
//!
//! Output is **not a rigid clone** — it is the confidence-weighted profile, e.g.
//! `mix ≈ 60/30/10 colonize/attack/defend; modules: never-guards-rear [high-conf]`.
//!
//! This module is OBSERVE+INFER only — it computes the read. The counter *synthesis* (RPS backbone
//! pick + projection-validated module exploits) is a later phase; [`OpponentProfile::rps_counter`]
//! exposes the backbone *suggestion* the synthesis will start from, but issues no orders.

use std::collections::HashMap;

use crate::counter::observe::{
    FractionBand, FrontierBand, GarrisonBand, MoveKind, ObservationLog, OwnershipBand, Sample,
    SituationBucket, ThreatBand,
};
use crate::strategy::StrategicPolicy;

// =====================================================================================
// Strategic mix — the colonize/defend/attack simplex point (RPS space).
// =====================================================================================

/// The coarse **strategic mix**: the colonize / defend / attack proportions (a simplex point that
/// sums to ~1 when any active move was seen). This is what places the opponent in RPS space.
///
/// The mapping from the abstracted [`MoveKind`]s is the natural one:
/// [`MoveKind::Colonize`] → colonize, [`MoveKind::Reinforce`] → defend, [`MoveKind::Strike`] →
/// attack. [`MoveKind::Hold`] is *not* an active strategy choice but is a strong defensive tell, so
/// it is folded into the **defend** weight at a discounted rate (a turtle's signature is declining
/// to over-extend) — see [`HOLD_AS_DEFEND_WEIGHT`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StrategicMix {
    /// Share of expansion (colonize) behaviour, in `[0, 1]`.
    pub colonize: f32,
    /// Share of defensive (reinforce / hold) behaviour, in `[0, 1]`.
    pub defend: f32,
    /// Share of aggressive (strike) behaviour, in `[0, 1]`.
    pub attack: f32,
}

/// How much a **defensive** (below-cap) [`MoveKind::Hold`] counts toward the **defend** weight in
/// the strategic mix. A hold is a real defensive tell (keeping a reserve / not over-extending) but a
/// *weaker* one than an actual reinforcement, so it is discounted. Tuned (against the four targets)
/// so a turtle's reserve-holding reads defend-dominant while a colonizer's lighter holding never
/// outweighs its active expansion: a colonizer's below-cap holds are comparable in count to its
/// active colony waves, so the weight must sit well below 1. A policy tunable, not a mechanic.
pub const HOLD_AS_DEFEND_WEIGHT: f32 = 0.15;

impl StrategicMix {
    /// The dominant strategic axis (the simplex vertex with the largest share). Ties break
    /// colonize > defend > attack (a stable, documented order). Returns `None` only for an
    /// all-zero (empty) mix.
    pub fn dominant(&self) -> Option<StrategicAxis> {
        let entries = [
            (StrategicAxis::Colonize, self.colonize),
            (StrategicAxis::Defend, self.defend),
            (StrategicAxis::Attack, self.attack),
        ];
        let mut best: Option<(StrategicAxis, f32)> = None;
        for (ax, w) in entries {
            match best {
                Some((_, bw)) if bw >= w => {}
                _ => best = Some((ax, w)),
            }
        }
        best.filter(|&(_, w)| w > 0.0).map(|(ax, _)| ax)
    }
}

/// The three RPS axes a [`StrategicMix`] can be dominated by (the simplex vertices).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StrategicAxis {
    /// Expansion-dominant (colonize).
    Colonize,
    /// Defense-dominant (reinforce / hold).
    Defend,
    /// Aggression-dominant (strike).
    Attack,
}

impl StrategicAxis {
    /// A short human-readable name.
    pub fn name(self) -> &'static str {
        match self {
            StrategicAxis::Colonize => "Colonize",
            StrategicAxis::Defend => "Defend",
            StrategicAxis::Attack => "Attack",
        }
    }

    /// The validated **RPS backbone counter** to a seat dominated by this axis (COUNTER_DESIGN §5):
    /// infer-Colonize → play **Attack**, infer-Attack → play **Defend**, infer-Defend → play
    /// **Colonize**. (Inference output only; the synthesis phase consumes it.)
    pub fn rps_counter(self) -> StrategicPolicy {
        match self {
            StrategicAxis::Colonize => StrategicPolicy::Attack,
            StrategicAxis::Attack => StrategicPolicy::Defend,
            StrategicAxis::Defend => StrategicPolicy::Colonize,
        }
    }
}

// =====================================================================================
// Per-module tendencies — predicates over the bucketed (situation -> choice) frequencies.
// =====================================================================================

/// One inferred module tendency: a frequency `rate` in `[0, 1]`, the sample count `n_i` it was
/// estimated from (its **confidence** — the `n_I` of COUNTER_DESIGN §4/§6), and the `fires`
/// verdict at the module's documented threshold. Where the relevant slice was never observed,
/// `n_i == 0` and the module stays agnostic (`fires == false`, `rate == 0`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ModuleStat {
    /// The estimated frequency the module's behaviour occurs at, in `[0, 1]`.
    pub rate: f32,
    /// How many samples fed this estimate (the confidence `n_I`).
    pub n_i: u32,
    /// Whether the module *fires* — its frequency crosses the documented threshold with enough
    /// observations to trust it (see each field's docs on [`Modules`]).
    pub fires: bool,
}

impl ModuleStat {
    /// An agnostic (never-observed) module: zero rate, zero confidence, does not fire.
    const AGNOSTIC: ModuleStat = ModuleStat { rate: 0.0, n_i: 0, fires: false };

    /// Build from a `(hits, n)` tally with a firing `threshold` and a minimum-confidence `min_n`.
    /// Fires iff `rate >= threshold` AND `n >= min_n` (the DBR guard: never trust a thin slice).
    fn from_tally(hits: u32, n: u32, threshold: f32, min_n: u32) -> ModuleStat {
        if n == 0 {
            return ModuleStat::AGNOSTIC;
        }
        let rate = hits as f32 / n as f32;
        ModuleStat { rate, n_i: n, fires: rate >= threshold && n >= min_n }
    }
}

/// The minimum slice-confidence (`n_I`) a module needs before it is allowed to *fire* (the DBR
/// sparse-data guard — COUNTER_DESIGN §4/§8). Below this we stay agnostic regardless of rate.
pub const MIN_MODULE_CONFIDENCE: u32 = 8;

/// The per-module tendency read (COUNTER_DESIGN §4). Each is a simple statistic over the bucketed
/// `(situation → choice)` frequencies, tagged with its own confidence.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Modules {
    /// **guards_rear** — among the seat's decisions from a *rear* (lane-interior, not adjacent to a
    /// foe) source — excluding over-cap saturation holds — the fraction that **guard** it: a
    /// Reinforce, or a below-cap Hold that keeps a reserve on the exposed rear. `rate` = that guard
    /// fraction. A turtle guards almost always (high); a colonizer/greedy strips its rear forward to
    /// expand (low) — the latter is the **thin-rear seam** ([`Modules::never_guards_rear`]). Fires
    /// only when the rear is *almost always* guarded (`rate >= 1 - SEAM_THRESHOLD`) — a true turtle.
    pub guards_rear: ModuleStat,

    /// **over_commits_attacks** — among Strike decisions, the fraction committed at a *heavy* band
    /// (three-quarters / all). A funnel-everything attacker scores high; a cautious counter-puncher
    /// low. Fires when `rate >= 0.5`.
    pub over_commits_attacks: ModuleStat,

    /// **hoards_past_cap** — among decisions taken while the source's idle stack is *over* its soft
    /// cap, the fraction that nonetheless **Hold** (let the stack sit and bleed rather than spend
    /// it). A hoarder scores high. Fires when `rate >= 0.5`.
    pub hoards_past_cap: ModuleStat,

    /// **nearest_first_expand** — the seat is a forward expander: among its *active* (non-Hold)
    /// decisions made *from a frontier source*, the fraction that are Colonize (push the surplus
    /// onto the nearest expandable ground rather than strike or hold). A nearest-first colonizer
    /// scores high. Fires when `rate >= 0.5`. (A legible proxy for "fills nearest-first": id-level
    /// ordering is abstracted away, so we read the colonize-from-the-front signature instead.)
    pub nearest_first_expand: ModuleStat,

    /// **feeds_losing_grind** — when *behind on territory* and *something is threatened*, the
    /// fraction of decisions that still **Strike** (feed the offensive) instead of reinforcing or
    /// holding. An over-committer that pours into a losing fight scores high; a turtle that snaps
    /// home scores ~0. Fires when `rate >= 0.5`.
    pub feeds_losing_grind: ModuleStat,
}

impl Modules {
    /// Convenience: **never_guards_rear** — the SimpleColonizer/greedy **thin-rear seam** read (the
    /// documented seam the validation gate checks fires with high confidence on SimpleColonize).
    /// `rate` = the **rear-strip** rate (`1 - guards_rear.rate`): how often the seat ships its
    /// exposed rear's surplus *forward* instead of guarding it. Fires iff it had enough rear
    /// decisions to judge (`n_I >= MIN_MODULE_CONFIDENCE`) and strips the rear at even the modest
    /// `SEAM_THRESHOLD` rate — set low because, under the resistance grind, a colonizer still *holds*
    /// its rear between waves, so its absolute strip rate is well under half yet far above a turtle's.
    pub fn never_guards_rear(&self) -> ModuleStat {
        let g = self.guards_rear;
        let strip = 1.0 - g.rate;
        ModuleStat {
            rate: strip,
            n_i: g.n_i,
            fires: g.n_i >= MIN_MODULE_CONFIDENCE && strip >= SEAM_THRESHOLD,
        }
    }
}

/// The rear-strip rate above which the **thin-rear seam** ([`Modules::never_guards_rear`]) fires:
/// the seat ships its exposed rear's surplus forward (rather than guarding it) at least this often.
/// Low on purpose — under the resistance grind a colonizer holds its rear between waves, so even a
/// genuine no-rear-guard policy strips it only a minority of decisions, but still many times more
/// than a turtle (which strips it ~never). A policy tunable, not a mechanic.
pub const SEAM_THRESHOLD: f32 = 0.12;

// =====================================================================================
// Per-bucket summary — the MEAN behaviour + confidence (n_I) for one situation type.
// =====================================================================================

/// The inferred **mean behaviour** for one [`SituationBucket`], with its confidence. Keeps only the
/// per-kind frequencies (the mean strategy in that bucket) and the count `n_i` they were estimated
/// from — exactly the COUNTER_DESIGN §4 "mean per bucket, tagged with `n_I`" datum.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BucketSummary {
    /// Total samples observed in this bucket (the confidence `n_I`).
    pub n_i: u32,
    /// Mean frequency of Colonize choices in this bucket, in `[0, 1]`.
    pub colonize: f32,
    /// Mean frequency of Reinforce choices, in `[0, 1]`.
    pub reinforce: f32,
    /// Mean frequency of Strike choices, in `[0, 1]`.
    pub strike: f32,
    /// Mean frequency of Hold (inaction) choices, in `[0, 1]`.
    pub hold: f32,
}

// =====================================================================================
// The OpponentProfile — the assembled, confidence-weighted read.
// =====================================================================================

/// The recency-decay parameter (COUNTER_DESIGN §4): a per-tick exponential **half-life** in ticks,
/// or `None` for plain accumulation (every sample weighted equally — the v1 default against
/// stationary automata). With `Some(h)`, a sample observed `Δ` ticks before the last is weighted
/// `0.5^(Δ / h)`, so recent behaviour dominates (the hook for future non-stationarity).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RecencyDecay(pub Option<f32>);

impl Default for RecencyDecay {
    /// Default: **accumulate** (no decay) — v1 targets are stationary within a match.
    fn default() -> Self {
        RecencyDecay(None)
    }
}

impl RecencyDecay {
    /// The weight for a sample observed at `tick`, given the log's last observed tick `last`.
    /// `1.0` under plain accumulation; `0.5^((last - tick) / half_life)` under decay.
    fn weight(&self, tick: u64, last: u64) -> f32 {
        match self.0 {
            None => 1.0,
            Some(half_life) if half_life > 0.0 => {
                let dt = last.saturating_sub(tick) as f32;
                0.5f32.powf(dt / half_life)
            }
            Some(_) => 1.0,
        }
    }
}

/// The assembled, **confidence-weighted** opponent profile (COUNTER_DESIGN §4) — the output of the
/// observe+infer phase. Carries the strategic [`StrategicMix`] (RPS placement), the per-module
/// tendencies [`Modules`], and the per-[`SituationBucket`] mean behaviour + confidence.
#[derive(Debug, Clone)]
pub struct OpponentProfile {
    /// The coarse strategic mix (the simplex point that places the opponent in RPS space).
    pub mix: StrategicMix,
    /// The per-module tendency reads, each tagged with its own confidence.
    pub modules: Modules,
    /// The mean behaviour + confidence (`n_I`) for every situation bucket actually observed.
    pub buckets: HashMap<SituationBucket, BucketSummary>,
    /// Total active (non-Hold) samples the mix was estimated from (the mix's overall confidence).
    pub active_samples: u32,
    /// Total samples observed (active + holds).
    pub total_samples: u32,
    /// The recency-decay setting this profile was inferred under.
    pub decay: RecencyDecay,
}

impl OpponentProfile {
    /// Infer a profile from `log` with **plain accumulation** (the v1 default — stationary targets).
    pub fn infer(log: &ObservationLog) -> OpponentProfile {
        Self::infer_with(log, RecencyDecay::default())
    }

    /// Infer a profile from `log` under an explicit [`RecencyDecay`]. Deterministic: a pure
    /// function of the log + decay.
    pub fn infer_with(log: &ObservationLog, decay: RecencyDecay) -> OpponentProfile {
        let last = log.samples.last().map(|s| s.tick).unwrap_or(0);

        // ---- Strategic mix: weighted active-kind shares, + DEFENSIVE Hold folded into defend. ----
        //
        // Two deliberate refinements over a naive kind-frequency count (both validated empirically
        // against the four targets — see `counter::tests`):
        //  * **Saturated holds are dropped.** A Hold taken while the source idle stack is already
        //    *over* its soft cap ([`GarrisonBand::Over`]) is post-victory saturation noise (a winner
        //    sitting on a full board with nothing left to grab), NOT a strategic choice — and it
        //    grows without bound as a decided match runs on. Counting it would swamp every other
        //    signal, so it is excluded from the mix entirely.
        //  * **Defensive holds fold into defend** at [`HOLD_AS_DEFEND_WEIGHT`]. A *below-cap* Hold is
        //    a deliberate reserve / not-over-extending — a real (if weaker than a reinforcement)
        //    turtle tell — so it counts toward the defend vertex at a discount.
        let mut w_col = 0.0f32;
        let mut w_def = 0.0f32; // reinforce
        let mut w_atk = 0.0f32; // strike
        let mut w_hold_def = 0.0f32; // below-cap (defensive) holds
        let mut active_n = 0u32;
        let mut total_n = 0u32;
        for s in &log.samples {
            let w = decay.weight(s.tick, last);
            total_n += 1;
            match s.choice.kind {
                MoveKind::Colonize => {
                    w_col += w;
                    active_n += 1;
                }
                MoveKind::Reinforce => {
                    w_def += w;
                    active_n += 1;
                }
                MoveKind::Strike => {
                    w_atk += w;
                    active_n += 1;
                }
                // Saturated (over-cap) holds: dropped. Below-cap holds: a defensive tell.
                MoveKind::Hold => {
                    if s.bucket.garrison != GarrisonBand::Over {
                        w_hold_def += w;
                    }
                }
            }
        }
        let def_total = w_def + HOLD_AS_DEFEND_WEIGHT * w_hold_def;
        let denom = (w_col + def_total + w_atk).max(f32::EPSILON);
        let mix = StrategicMix {
            colonize: w_col / denom,
            defend: def_total / denom,
            attack: w_atk / denom,
        };

        // ---- Per-bucket mean behaviour + confidence (n_I). ----
        let buckets = summarize_buckets(&log.samples);

        // ---- Per-module tendencies (statistics over the bucketed data). ----
        let modules = infer_modules(&log.samples);

        OpponentProfile {
            mix,
            modules,
            buckets,
            active_samples: active_n,
            total_samples: total_n,
            decay,
        }
    }

    /// The RPS backbone counter to this profile's **dominant** axis (the synthesis phase's starting
    /// point — inference output only). `None` only for an empty profile.
    pub fn rps_counter(&self) -> Option<StrategicPolicy> {
        self.mix.dominant().map(StrategicAxis::rps_counter)
    }

    /// A compact one-line summary for diagnostics / the inferred-vs-truth report.
    pub fn summary(&self) -> String {
        let dom = self.mix.dominant().map(|a| a.name()).unwrap_or("none");
        format!(
            "mix col/def/atk = {:.0}/{:.0}/{:.0}  dominant={dom}  \
             [guards_rear {:.2} n{} | over_commits {:.2} n{} | hoards {:.2} n{} | \
             nearest_first {:.2} n{} | feeds_grind {:.2} n{} | never_guards_rear fires={}]  \
             (active {}, total {})",
            self.mix.colonize * 100.0,
            self.mix.defend * 100.0,
            self.mix.attack * 100.0,
            self.modules.guards_rear.rate,
            self.modules.guards_rear.n_i,
            self.modules.over_commits_attacks.rate,
            self.modules.over_commits_attacks.n_i,
            self.modules.hoards_past_cap.rate,
            self.modules.hoards_past_cap.n_i,
            self.modules.nearest_first_expand.rate,
            self.modules.nearest_first_expand.n_i,
            self.modules.feeds_losing_grind.rate,
            self.modules.feeds_losing_grind.n_i,
            self.modules.never_guards_rear().fires,
            self.active_samples,
            self.total_samples,
        )
    }
}

// =====================================================================================
// Inference internals (pure reductions over the sample stream).
// =====================================================================================

/// Reduce the sample stream into per-bucket mean behaviour + confidence. Plain counts (the mean is
/// what §4 asks for; recency weighting is applied to the *mix/modules* where it matters, and the
/// per-bucket mean is reported as the unweighted observed frequency so `n_I` reads as a true count).
fn summarize_buckets(samples: &[Sample]) -> HashMap<SituationBucket, BucketSummary> {
    // (n, col, reinf, strike, hold) running counts per bucket.
    let mut acc: HashMap<SituationBucket, (u32, u32, u32, u32, u32)> = HashMap::new();
    for s in samples {
        let e = acc.entry(s.bucket).or_insert((0, 0, 0, 0, 0));
        e.0 += 1;
        match s.choice.kind {
            MoveKind::Colonize => e.1 += 1,
            MoveKind::Reinforce => e.2 += 1,
            MoveKind::Strike => e.3 += 1,
            MoveKind::Hold => e.4 += 1,
        }
    }
    acc.into_iter()
        .map(|(bucket, (n, c, r, s, h))| {
            let nf = n as f32;
            (
                bucket,
                BucketSummary {
                    n_i: n,
                    colonize: c as f32 / nf,
                    reinforce: r as f32 / nf,
                    strike: s as f32 / nf,
                    hold: h as f32 / nf,
                },
            )
        })
        .collect()
}

/// Compute the five per-module tendencies as `(hits, n)` tallies over the right slice of samples,
/// each with a documented firing threshold + the shared minimum-confidence guard.
fn infer_modules(samples: &[Sample]) -> Modules {
    // guards_rear: slice = REAR sources, excluding over-cap saturation holds; hit = a GUARD =
    // Reinforce or a below-cap Hold (a reserve kept on the exposed rear). The complement — shipping
    // the rear's surplus FORWARD (Colonize/Strike) — is the thin-rear seam (`never_guards_rear`).
    let mut gr_hits = 0u32; // guards
    let mut gr_n = 0u32; // rear decisions (excl. saturation)
    // over_commits_attacks: slice = Strike decisions; hit = Heavy band.
    let mut oc_hits = 0u32;
    let mut oc_n = 0u32;
    // hoards_past_cap: slice = source over its soft cap; hit = Hold (let it bleed).
    let mut hp_hits = 0u32;
    let mut hp_n = 0u32;
    // nearest_first_expand: slice = active decisions from a frontier source; hit = Colonize.
    let mut nf_hits = 0u32;
    let mut nf_n = 0u32;
    // feeds_losing_grind: slice = behind-on-territory + threatened; hit = Strike.
    let mut fg_hits = 0u32;
    let mut fg_n = 0u32;

    for s in samples {
        let b = s.bucket;
        let kind = s.choice.kind;

        // guards_rear — rear sources (any threat), excluding over-cap saturation holds. A guard is a
        // reinforce or a below-cap hold (a deliberate reserve on the exposed rear); shipping forward
        // (Colonize/Strike) is NOT a guard (it strips the rear — the seam).
        if b.frontier == FrontierBand::Rear
            && !(kind == MoveKind::Hold && b.garrison == GarrisonBand::Over)
        {
            gr_n += 1;
            match kind {
                MoveKind::Reinforce | MoveKind::Hold => gr_hits += 1, // below-cap hold reaches here
                _ => {}
            }
        }

        // over_commits_attacks — among strikes.
        if kind == MoveKind::Strike {
            oc_n += 1;
            if s.choice.band == FractionBand::Heavy {
                oc_hits += 1;
            }
        }

        // hoards_past_cap — among over-cap sources.
        if b.garrison == GarrisonBand::Over {
            hp_n += 1;
            if kind == MoveKind::Hold {
                hp_hits += 1;
            }
        }

        // nearest_first_expand — active decisions from a frontier source.
        if b.frontier == FrontierBand::Front && kind != MoveKind::Hold {
            nf_n += 1;
            if kind == MoveKind::Colonize {
                nf_hits += 1;
            }
        }

        // feeds_losing_grind — behind + threatened.
        if b.ownership == OwnershipBand::Behind && b.threat == ThreatBand::Threatened {
            fg_n += 1;
            if kind == MoveKind::Strike {
                fg_hits += 1;
            }
        }
    }

    Modules {
        // guards_rear fires only when the rear is *almost always* guarded (a true turtle); the
        // complementary seam (`never_guards_rear`) fires as soon as the rear is stripped forward at
        // even the modest `SEAM_THRESHOLD` rate — a colonizer strips its rear far more than a turtle
        // but, under the grind, still holds it between waves, so the seam bar is intentionally low.
        guards_rear: ModuleStat::from_tally(gr_hits, gr_n, 1.0 - SEAM_THRESHOLD, MIN_MODULE_CONFIDENCE),
        over_commits_attacks: ModuleStat::from_tally(oc_hits, oc_n, 0.5, MIN_MODULE_CONFIDENCE),
        hoards_past_cap: ModuleStat::from_tally(hp_hits, hp_n, 0.5, MIN_MODULE_CONFIDENCE),
        nearest_first_expand: ModuleStat::from_tally(nf_hits, nf_n, 0.5, MIN_MODULE_CONFIDENCE),
        feeds_losing_grind: ModuleStat::from_tally(fg_hits, fg_n, 0.5, MIN_MODULE_CONFIDENCE),
    }
}

/// Re-export of [`Choice`] kept handy for downstream `use crate::counter::profile::*`. (The mix and
/// modules are computed from [`Choice`]/[`MoveKind`], so callers building synthetic logs in tests
/// want both in scope.)
pub use crate::counter::observe::Choice as ObservedChoice;
