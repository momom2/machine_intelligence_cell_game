//! A small **headless AI-vs-AI harness**: symmetric test-world builders plus a deterministic
//! match runner that pits two [`Roster`] entries (or raw [`AiController`]s) against each other
//! to a horizon and reports the [`world::WorldOutcome`].
//!
//! This is the real test of the AI layer: the strategy tests in
//! `crates/ai/tests/ai_tests.rs` and the `ai-harness` binary both drive it. The runner draws
//! no randomness of its own (all randomness lives inside each planet's `layer1` Structure), so
//! a given `(world build seed, policies, horizon, interval)` replays bit-for-bit — the basis of
//! the determinism test.

use layer1::{Faction, SimParams, Structure, SubStructure, Vec2};
use world::{Planet, World, WorldOutcome, WorldParams};

use crate::controller::{AiController, Roster};

/// How often (in ticks) each seat re-plans. Both seats decide on the **same pre-step snapshot**
/// every `decision_interval` ticks (Player issues first — a fixed, documented tie-break,
/// matching `layer1::run_auto_vs_auto`), so forces commit over time rather than re-issuing
/// every tick.
pub const DEFAULT_DECISION_INTERVAL: u64 = 8;

/// Default match horizon (ticks) for the standard test worlds. Raised for the resistance-grind
/// model: a capture now takes ~`max_resistance / present_force` ticks (the default fresh
/// resistance is `1800`), so a full economic lead-to-conversion and an assault landing-and-being-
/// punished play out over **thousands** of ticks, not hundreds. At `3000` the diamond RPS cycle
/// resolves on every edge over both seatings + multiple seeds (the `attack>colonize>defend>attack`
/// measurement); a shorter horizon cuts matches off mid-grind while an aggressor is *transiently*
/// ahead and the cycle reads as not-closed. (Was `1200` under the old instant-capture model.)
pub const DEFAULT_HORIZON: u64 = 3000;

/// Result of one match: the world outcome plus the seat that each roster entry played (so a
/// caller swapping seatings can attribute wins correctly).
#[derive(Debug, Clone, Copy)]
pub struct MatchResult {
    /// The world outcome at the horizon / elimination.
    pub outcome: WorldOutcome,
    /// Which seat the "A" roster entry played this match.
    pub a_seat: Faction,
}

impl MatchResult {
    /// Did roster entry **A** win this match? (`false` on a loss *or* a draw.)
    pub fn a_won(&self) -> bool {
        self.outcome.winner == Some(self.a_seat)
    }
    /// Did roster entry **B** win this match?
    pub fn b_won(&self) -> bool {
        self.outcome.winner == Some(self.a_seat.opponent())
    }
}

/// Run a single match between two [`AiController`]s on `world` to `horizon` ticks (or until one
/// seat is world-eliminated), re-planning every `decision_interval` ticks. Both controllers
/// must already be set to opposing seats. Returns the final [`WorldOutcome`].
///
/// Decision order each planning tick: both decide on the same snapshot, then **Player applies
/// first, Enemy second** (documented tie-break). Deterministic.
pub fn run_match(
    world: &mut World,
    params: &SimParams,
    wp: &WorldParams,
    a: &AiController,
    b: &AiController,
    horizon: u64,
    decision_interval: u64,
) -> WorldOutcome {
    let interval = decision_interval.max(1);
    // Order the two controllers so Player-seat applies first regardless of which is A.
    let (first, second) = if a.seat == Faction::Player { (a, b) } else { (b, a) };
    while world.tick < horizon {
        if world.is_eliminated(Faction::Player) || world.is_eliminated(Faction::Enemy) {
            break;
        }
        if world.tick % interval == 0 {
            // Decide both on the SAME pre-step snapshot (decide is a pure read), then apply
            // Player-first. We compute both decisions before applying either.
            let d_first = first.decide(world, params, wp);
            let d_second = second.decide(world, params, wp);
            first.apply(world, &d_first, wp);
            second.apply(world, &d_second, wp);
        }
        world.step(params, wp);
    }
    world.outcome()
}

/// Run roster entry `a` vs `b` on a freshly built `world` (seat A = Player, B = Enemy) to the
/// default horizon/interval. Convenience over [`run_match`].
pub fn run_roster(
    mut world: World,
    params: &SimParams,
    wp: &WorldParams,
    a: Roster,
    b: Roster,
) -> MatchResult {
    let ca = AiController::from_roster(Faction::Player, a);
    let cb = AiController::from_roster(Faction::Enemy, b);
    let outcome = run_match(
        &mut world,
        params,
        wp,
        &ca,
        &cb,
        DEFAULT_HORIZON,
        DEFAULT_DECISION_INTERVAL,
    );
    MatchResult { outcome, a_seat: Faction::Player }
}

/// Run a [`crate::counter::CounterController`] (the **Counter** seat) against a fixed
/// [`AiController`] `opp` on `world` to `horizon`, driving the accumulate-then-counter discipline:
/// each planning tick both seats decide on the **same pre-step snapshot**, the Counter folds the
/// opponent's chosen orders into its profile (the observation hook), then both apply Player-first
/// (the documented tie-break). Returns the final [`WorldOutcome`]. Deterministic.
///
/// This is the match driver the Counter diagnostic (COUNTER_DESIGN §7) and the synthesis tests use;
/// it is the Counter analog of [`run_match`] (which only knows stateless controllers).
pub fn run_counter_match(
    world: &mut World,
    params: &SimParams,
    wp: &WorldParams,
    counter: &mut crate::counter::CounterController,
    opp: &AiController,
    horizon: u64,
    decision_interval: u64,
) -> WorldOutcome {
    let interval = decision_interval.max(1);
    while world.tick < horizon {
        if world.is_eliminated(Faction::Player) || world.is_eliminated(Faction::Enemy) {
            break;
        }
        if world.tick % interval == 0 {
            // Both decide on the same pre-step snapshot (decide is a pure read).
            let d_counter = counter.decide(world, params, wp);
            let d_opp = opp.decide(world, params, wp);
            // THE OBSERVATION HOOK: fold the opponent's chosen orders into the Counter's profile,
            // against the pre-decision world the opponent saw — before either applies.
            counter.observe_opponent(world, &d_opp.fleet_orders);
            // Apply Player-first (the documented tie-break).
            if counter.seat == Faction::Player {
                counter.apply(world, &d_counter, wp);
                opp.apply(world, &d_opp, wp);
            } else {
                opp.apply(world, &d_opp, wp);
                counter.apply(world, &d_counter, wp);
            }
        }
        world.step(params, wp);
    }
    world.outcome()
}

// ======================================================================================
// The Counter DIAGNOSTIC (COUNTER_DESIGN §7) — Counter vs each target, p_max swept.
// ======================================================================================
//
// The Counter doubles as a *diagnostic for the automata* (the project owner's framing): run
// `Roster::Counter { p_max }` against each of the four targets, sweep `p_max` over a robust→
// vulnerability-hunter range, both seatings, several seeds; for each `(target, p_max)` report the
// inferred profile vs ground truth (does the mix point at the right RPS corner? does the seam fire
// for SimpleColonize?), the Counter's win-rate, and which backbone/exploit it converged on. The
// signals: a *brittle* exploit that fires => PATCH the automaton; a mixed line that beats the
// intended RPS counter => REFINE it; and the `p_max` sweep traces the playstyle axis (its effect on
// win-rate is non-monotonic across targets — more `p_max` is not uniformly better).

/// The default `p_max` sweep for the diagnostic: a robust **generalist** end (`0.2`, lean on the
/// RPS backbone), a **balanced** middle (`0.6`), and a **vulnerability-hunter** end (`0.95`, lean
/// into projection-confirmed exploits). Three points are enough to read the (non-monotonic) shape of
/// the playstyle axis without exploding the match count.
pub const COUNTER_DIAG_P_MAX: [f32; 3] = [0.2, 0.6, 0.95];

/// The default seeds the diagnostic sweeps (>= 4, per §7). Each is run on **both seatings**, so a
/// `(target, p_max)` cell is `2 * seeds.len()` matches — enough per-cell games for a stable
/// win-rate while keeping the full sweep tractable.
pub const COUNTER_DIAG_SEEDS: [u64; 5] = [1, 7, 42, 100, 2024];

/// The four diagnostic **targets** (COUNTER_DESIGN §7) paired with the world the Counter meets each
/// on. The three pure automata are run on the **diamond** (where the validated RPS cycle closes —
/// the fair test of "did the backbone converge on the RPS counter and win?"); `SimpleColonize` is
/// run on the **corridor** (a single forward axis, so its thin-rear seam — the documented exploit —
/// is geometrically clean to flank), matching the inference gate's map choice.
pub fn counter_diag_targets() -> Vec<CounterTarget> {
    vec![
        CounterTarget { roster: Roster::Colonize, truth: "Colonize", build: diamond_world, map: "diamond" },
        CounterTarget { roster: Roster::Defend, truth: "Defend", build: diamond_world, map: "diamond" },
        CounterTarget { roster: Roster::Attack, truth: "Attack", build: diamond_world, map: "diamond" },
        CounterTarget { roster: Roster::SimpleColonize, truth: "Colonize", build: corridor_world, map: "corridor" },
    ]
}

/// One diagnostic target: the [`Roster`] entry the Counter faces, its ground-truth dominant axis
/// name, the world builder it is met on, and that map's name (for the report).
#[derive(Clone, Copy)]
pub struct CounterTarget {
    /// The roster entry the Counter observes and counters.
    pub roster: Roster,
    /// The target's ground-truth dominant strategic axis (the corner the inferred mix *should* point
    /// at): `"Colonize"`, `"Defend"`, or `"Attack"`.
    pub truth: &'static str,
    /// The world builder the Counter meets this target on (seeded).
    pub build: fn(u64) -> World,
    /// The map name (for the report tables).
    pub map: &'static str,
}

/// The converged read + win-rate for one `(target, p_max)` cell of the sweep — the diagnostic datum.
#[derive(Debug, Clone)]
pub struct CounterDiagCell {
    /// The `p_max` playstyle dial this cell was run at.
    pub p_max: f32,
    /// Counter wins over the cell's `2 * seeds` matches.
    pub counter_wins: u32,
    /// Target wins.
    pub target_wins: u32,
    /// Draws.
    pub draws: u32,
    /// The Counter's win-rate over the cell (`counter_wins / games`).
    pub win_rate: f64,
    /// The backbone the Counter **converged on** (the RPS counter to the inferred mix), as a
    /// strategy name — read from the final accumulated profile, pooled across the cell's matches.
    /// `"none"` only if the Counter never observed an active move (it stayed agnostic).
    pub converged_backbone: String,
    /// The dominant axis the Counter **inferred** for the target, pooled across the cell (the corner
    /// the mix points at): `"Colonize"`/`"Defend"`/`"Attack"`/`"none"`.
    pub inferred_dominant: String,
    /// Whether the inferred dominant axis matched the target's ground truth (the "right corner?").
    pub inferred_matches_truth: bool,
    /// The pooled inferred mix `(colonize, defend, attack)` percentages, averaged over the cell's
    /// final profiles (so the report shows *where* on the simplex the read landed).
    pub mix_pct: (f32, f32, f32),
    /// Whether the Counter's read **fires the thin-rear seam** (`never_guards_rear`) for this target,
    /// in any of the cell's matches (the SimpleColonize/Colonize seam check).
    pub seam_fired: bool,
    /// How many decision ticks (pooled across the cell) the Counter actually **shipped an exploit**
    /// (a projection-confirmed deviation from the backbone), and the total decision ticks — so the
    /// report can show exploits fire *selectively*, gated by the projection, not constantly.
    pub exploit_ticks: u32,
    /// Total Counter decision ticks across the cell (the denominator for `exploit_ticks`).
    pub total_ticks: u32,
    /// The names of the distinct exploits that actually shipped in the cell (e.g.
    /// `"flank-undefended-rear"`), in first-seen order. Empty when the backbone alone drove every
    /// tick — the honest "what did it converge on" record.
    pub exploits_shipped: Vec<String>,
}

impl CounterDiagCell {
    /// Total games in the cell.
    pub fn games(&self) -> u32 {
        self.counter_wins + self.target_wins + self.draws
    }
    /// The exploit-fire rate over the cell's decision ticks (`exploit_ticks / total_ticks`).
    pub fn exploit_rate(&self) -> f64 {
        if self.total_ticks == 0 { 0.0 } else { self.exploit_ticks as f64 / self.total_ticks as f64 }
    }
    /// A compact "what shipped" tag for the report: the converged backbone, plus the exploits that
    /// fired (with their fire-rate), or `"backbone only"` when the backbone drove every tick.
    pub fn convergence_tag(&self) -> String {
        if self.exploits_shipped.is_empty() {
            format!("{} (backbone only)", self.converged_backbone)
        } else {
            format!(
                "{} + [{}] ({:.0}% of ticks)",
                self.converged_backbone,
                self.exploits_shipped.join(", "),
                self.exploit_rate() * 100.0
            )
        }
    }
}

/// The full diagnostic result for one target: the target's identity + the per-`p_max` cells.
#[derive(Debug, Clone)]
pub struct CounterDiagRow {
    /// The target's roster name.
    pub target: String,
    /// The target's ground-truth dominant axis.
    pub truth: String,
    /// The map the cells were run on.
    pub map: String,
    /// One cell per swept `p_max` (in [`COUNTER_DIAG_P_MAX`] order).
    pub cells: Vec<CounterDiagCell>,
}

/// Run **one Counter match** for the diagnostic — the Counter on `counter_seat` (playstyle `p_max`)
/// vs the fixed `target` roster on the other seat, on a freshly built `world` to `horizon` — and
/// return `(outcome, telemetry)`. Mirrors [`run_counter_match`]'s accumulate-then-counter discipline
/// (decide both on the same pre-step snapshot, fold the opponent's orders into the profile, apply
/// Player-first) but additionally records, each Counter decision tick, the [`crate::counter::CounterPlan`]
/// the Counter shipped (backbone + any confirmed exploit) so the diagnostic can report convergence.
/// Deterministic.
fn run_counter_diag_match(
    world: &mut World,
    params: &SimParams,
    wp: &WorldParams,
    counter_seat: Faction,
    p_max: f32,
    target: Roster,
    horizon: u64,
    decision_interval: u64,
) -> (WorldOutcome, CounterMatchTelemetry) {
    use crate::counter::CounterController;
    let interval = decision_interval.max(1);
    let mut counter = CounterController::new(counter_seat, p_max, *params, *wp);
    let opp = AiController::from_roster(counter_seat.opponent(), target);
    let mut tele = CounterMatchTelemetry::default();

    while world.tick < horizon {
        if world.is_eliminated(Faction::Player) || world.is_eliminated(Faction::Enemy) {
            break;
        }
        if world.tick % interval == 0 {
            // Both decide on the same pre-step snapshot. One synthesis yields BOTH the decision and
            // the plan (the legible "Read → counter") we record — no redundant re-synthesis.
            let (d_counter, plan) = counter.decide_with_plan(world, params, wp);
            let d_opp = opp.decide(world, params, wp);
            // The observation hook (before either applies): fold the opponent's chosen orders in.
            counter.observe_opponent(world, &d_opp.fleet_orders);
            // Record convergence telemetry for this tick.
            tele.record(&plan);
            // Apply Player-first (the documented tie-break).
            if counter.seat == Faction::Player {
                counter.apply(world, &d_counter, wp);
                opp.apply(world, &d_opp, wp);
            } else {
                opp.apply(world, &d_opp, wp);
                counter.apply(world, &d_counter, wp);
            }
        }
        world.step(params, wp);
    }

    // The FINAL accumulated read (the converged profile) is what the diagnostic reports as the
    // inferred-vs-truth for this match — the sharpest read, after a full match of observation.
    tele.final_profile = Some(counter.profile());
    (world.outcome(), tele)
}

/// Per-match convergence telemetry the diagnostic accumulates (which backbone/exploit the Counter
/// shipped over the match, and its final read).
#[derive(Debug, Clone, Default)]
struct CounterMatchTelemetry {
    /// Total Counter decision ticks observed.
    total_ticks: u32,
    /// Ticks on which a projection-confirmed exploit was shipped (a deviation from the backbone).
    exploit_ticks: u32,
    /// The distinct exploit names shipped, in first-seen order.
    exploits_shipped: Vec<String>,
    /// The final accumulated [`crate::counter::OpponentProfile`] (set at match end).
    final_profile: Option<crate::counter::OpponentProfile>,
}

impl CounterMatchTelemetry {
    /// Fold one tick's shipped [`crate::counter::CounterPlan`] into the telemetry.
    fn record(&mut self, plan: &crate::counter::CounterPlan) {
        self.total_ticks += 1;
        if let Some(e) = plan.exploit {
            self.exploit_ticks += 1;
            let name = e.name().to_string();
            if !self.exploits_shipped.iter().any(|n| n == &name) {
                self.exploits_shipped.push(name);
            }
        }
    }
}

/// Run the **Counter diagnostic** (COUNTER_DESIGN §7): for each target in `targets`, for each
/// `p_max` in `p_maxes`, run `Roster::Counter { p_max }` vs the target on **both seatings** of each
/// seed in `seeds`, to `horizon` ticks. Returns one [`CounterDiagRow`] per target (with a
/// [`CounterDiagCell`] per `p_max`): the inferred-vs-truth read, the Counter win-rate, and the
/// converged backbone/exploit. Deterministic (the projection draws no RNG; the profile is a
/// deterministic function of the log).
///
/// Pooling: within a cell, win/draw counts sum over all matches; the inferred dominant axis and the
/// seam verdict are taken from the matches' **final** profiles (a profile is "right" iff it points at
/// the truth corner — reported as the fraction of matches that agreed, collapsed to a single verdict
/// = the majority of the cell's matches); exploit ticks/names pool across the cell.
pub fn counter_diagnostic(
    targets: &[CounterTarget],
    p_maxes: &[f32],
    seeds: &[u64],
    horizon: u64,
) -> Vec<CounterDiagRow> {
    let params = SimParams::default();
    let wp = WorldParams::default();
    let mut rows = Vec::new();

    for t in targets {
        let mut cells = Vec::new();
        for &p_max in p_maxes {
            let mut counter_wins = 0u32;
            let mut target_wins = 0u32;
            let mut draws = 0u32;
            let mut matches_right = 0u32;
            let mut matches_total = 0u32;
            let mut seam_fired = false;
            let mut exploit_ticks = 0u32;
            let mut total_ticks = 0u32;
            let mut exploits_shipped: Vec<String> = Vec::new();
            // Pool the inferred mix + the dominant-axis vote across the cell's final profiles. The
            // vote is tallied over a FIXED label order (not a HashMap) so the reported majority — and
            // thus the converged backbone derived from it — is bit-deterministic, ties included.
            let mut sum_col = 0.0f32;
            let mut sum_def = 0.0f32;
            let mut sum_atk = 0.0f32;
            // Votes for [Colonize, Defend, Attack, none], in this stable tie-break order.
            let mut dom_votes = [0u32; 4];

            for &seed in seeds {
                for &counter_seat in &[Faction::Player, Faction::Enemy] {
                    let mut w = (t.build)(seed);
                    let (outcome, tele) = run_counter_diag_match(
                        &mut w, &params, &wp, counter_seat, p_max, t.roster, horizon, DEFAULT_DECISION_INTERVAL,
                    );
                    match outcome.winner {
                        Some(f) if f == counter_seat => counter_wins += 1,
                        Some(_) => target_wins += 1,
                        None => draws += 1,
                    }
                    exploit_ticks += tele.exploit_ticks;
                    total_ticks += tele.total_ticks;
                    for e in &tele.exploits_shipped {
                        if !exploits_shipped.contains(e) {
                            exploits_shipped.push(e.clone());
                        }
                    }
                    if let Some(prof) = &tele.final_profile {
                        matches_total += 1;
                        let dom = prof.mix.dominant().map(|a| a.name()).unwrap_or("none");
                        if dom == t.truth {
                            matches_right += 1;
                        }
                        let idx = match dom {
                            "Colonize" => 0,
                            "Defend" => 1,
                            "Attack" => 2,
                            _ => 3,
                        };
                        dom_votes[idx] += 1;
                        sum_col += prof.mix.colonize;
                        sum_def += prof.mix.defend;
                        sum_atk += prof.mix.attack;
                        if prof.modules.never_guards_rear().fires {
                            seam_fired = true;
                        }
                    }
                }
            }

            let games = counter_wins + target_wins + draws;
            let win_rate = if games == 0 { 0.0 } else { counter_wins as f64 / games as f64 };
            // The cell's converged dominant = the majority vote across its matches' final reads, with
            // a FIXED tie-break: Colonize > Defend > Attack > none (the FIRST index achieving the max
            // wins — same documented order as `StrategicMix::dominant`). Iterating with strict `>`
            // keeps the first maximum, so the reported string is deterministic even on a tie.
            let labels = ["Colonize", "Defend", "Attack", "none"];
            let mut best_idx = 0usize;
            for i in 1..4 {
                if dom_votes[i] > dom_votes[best_idx] {
                    best_idx = i;
                }
            }
            let inferred_dominant = labels[best_idx].to_string();
            // The converged backbone is the RPS counter to that dominant axis (the never-worse pick).
            let converged_backbone = rps_counter_name(&inferred_dominant);
            let denom = matches_total.max(1) as f32;
            let cell = CounterDiagCell {
                p_max,
                counter_wins,
                target_wins,
                draws,
                win_rate,
                converged_backbone,
                inferred_dominant,
                inferred_matches_truth: matches_right * 2 >= matches_total, // majority right
                mix_pct: (sum_col / denom * 100.0, sum_def / denom * 100.0, sum_atk / denom * 100.0),
                seam_fired,
                exploit_ticks,
                total_ticks,
                exploits_shipped,
            };
            cells.push(cell);
        }
        rows.push(CounterDiagRow {
            target: t.roster.name().to_string(),
            truth: t.truth.to_string(),
            map: t.map.to_string(),
            cells,
        });
    }
    rows
}

/// Map a dominant-axis name to the RPS backbone strategy name the Counter plays against it
/// (infer-Colonize ⇒ play Attack, infer-Attack ⇒ Defend, infer-Defend ⇒ Colonize). `"none"` for an
/// agnostic read. Pure helper for the diagnostic report.
fn rps_counter_name(dominant: &str) -> String {
    match dominant {
        "Colonize" => "Attack",
        "Attack" => "Defend",
        "Defend" => "Colonize",
        _ => "none",
    }
    .to_string()
}

/// Pretty-print a [`counter_diagnostic`] result as the COUNTER_DESIGN §7 report tables (win-rate +
/// inferred-vs-truth + convergence, the `p_max` sweep per target). Returns the report as a `String`
/// (so a bin can print it and a test can assert on it). Deterministic.
pub fn format_counter_diagnostic(rows: &[CounterDiagRow], seeds: usize) -> String {
    use std::fmt::Write;
    let mut s = String::new();
    let games_per_cell = seeds * 2;
    writeln!(
        s,
        "== Counter diagnostic (COUNTER_DESIGN §7) — deterministic; {} matches/cell (both seatings × {} seeds) ==",
        games_per_cell, seeds
    )
    .ok();
    writeln!(s).ok();

    for row in rows {
        writeln!(s, "--- TARGET: {} (truth: {}-dominant) on the {} ---", row.target, row.truth, row.map).ok();
        writeln!(
            s,
            "  {:>6} | {:>10} | {:>9} | {:>22} | {:>5} | {}",
            "p_max", "win-rate", "inferred", "mix col/def/atk", "seam", "converged on (backbone + exploits)"
        )
        .ok();
        writeln!(s, "  {}", "-".repeat(108)).ok();
        for c in &row.cells {
            let inferred = format!(
                "{}{}",
                c.inferred_dominant,
                if c.inferred_matches_truth { " OK" } else { " X" }
            );
            writeln!(
                s,
                "  {:>6.2} | {:>4}/{:<4}{:>1} | {:>9} | {:>6.0}/{:>4.0}/{:<4.0}    | {:>5} | {}",
                c.p_max,
                c.counter_wins,
                c.games(),
                "",
                inferred,
                c.mix_pct.0,
                c.mix_pct.1,
                c.mix_pct.2,
                if c.seam_fired { "fires" } else { "-" },
                c.convergence_tag(),
            )
            .ok();
        }
        writeln!(s).ok();
    }
    s
}

/// Play `a` vs `b` on **both seatings** of a freshly built world (built by `build`, called
/// twice so each match starts fresh) and return `(a_wins, b_wins, draws)` over the two games.
/// This is the fair, seat-symmetric comparison the strategy tests use.
pub fn duel_both_seatings(
    build: impl Fn() -> World,
    params: &SimParams,
    wp: &WorldParams,
    a: Roster,
    b: Roster,
) -> (u32, u32, u32) {
    let mut a_wins = 0;
    let mut b_wins = 0;
    let mut draws = 0;

    // Seating 1: A=Player, B=Enemy.
    {
        let mut w = build();
        let ca = AiController::from_roster(Faction::Player, a);
        let cb = AiController::from_roster(Faction::Enemy, b);
        let o = run_match(&mut w, params, wp, &ca, &cb, DEFAULT_HORIZON, DEFAULT_DECISION_INTERVAL);
        match o.winner {
            Some(Faction::Player) => a_wins += 1,
            Some(Faction::Enemy) => b_wins += 1,
            _ => draws += 1,
        }
    }
    // Seating 2: A=Enemy, B=Player (swap seats; same map).
    {
        let mut w = build();
        let ca = AiController::from_roster(Faction::Enemy, a);
        let cb = AiController::from_roster(Faction::Player, b);
        let o = run_match(&mut w, params, wp, &cb, &ca, DEFAULT_HORIZON, DEFAULT_DECISION_INTERVAL);
        match o.winner {
            Some(Faction::Enemy) => a_wins += 1,
            Some(Faction::Player) => b_wins += 1,
            _ => draws += 1,
        }
    }
    (a_wins, b_wins, draws)
}

// ======================================================================================
// Symmetric test-world builders.
// ======================================================================================

/// Build a sub-structure planet for a home base: `subs` Player/Enemy-owned subs in a small
/// cluster (so the Layer-1 greedy internals have several positions to play), each seeded with
/// `per_sub` idle ships. `seed` keys the planet's RNG.
fn home_planet(seed: u64, owner: Faction, subs: usize, per_sub: usize, pos: Vec2, name: &str) -> Planet {
    let mut st = Structure::new(seed);
    // Lay the subs out in a tight ring so they are within fighting/engagement proximity and the
    // internal greedy can shuffle between them, but capture stays clean at setup.
    let ids: Vec<_> = (0..subs)
        .map(|i| {
            let ang = (i as f32) / (subs.max(1) as f32) * std::f32::consts::TAU;
            let r = if i == 0 { 0.0 } else { 9.0 };
            st.add_sub(SubStructure::new(
                Vec2::new(r * ang.cos(), r * ang.sin()),
                4.0,
                owner,
            ))
        })
        .collect();
    for &s in &ids {
        for _ in 0..per_sub {
            st.spawn_ship(owner, s);
        }
    }
    Planet::new(st, pos, name)
}

/// Build a neutral planet with `subs` empty neutral sub-structures (capturable production).
fn neutral_planet(seed: u64, subs: usize, pos: Vec2, name: &str) -> Planet {
    let mut st = Structure::new(seed);
    for i in 0..subs.max(1) {
        let ang = (i as f32) / (subs.max(1) as f32) * std::f32::consts::TAU;
        let r = if i == 0 { 0.0 } else { 9.0 };
        st.add_sub(SubStructure::new(Vec2::new(r * ang.cos(), r * ang.sin()), 4.0, Faction::Neutral));
    }
    Planet::new(st, pos, name)
}

/// The standard **symmetric corridor** test world: two homes at the ends of a chain of neutral
/// planets, mirror-symmetric so swapping seats is perfectly fair.
///
/// ```text
///   P(home) — n1 — n2(centre) — n3 — E(home)
/// ```
/// Both homes start with 3 owned subs at ~10 idle ships each (a real army to spend), and there
/// are three single-sub neutral planets between them: two flanking colonization targets and a
/// contested centre. Lane lengths are symmetric. This is the world the cycle measurement and
/// the seam test use; `seed` keys both planets' RNG (offset so the two homes differ).
pub fn corridor_world(seed: u64) -> World {
    let mut w = World::new();
    let p = w.add_planet(home_planet(seed, Faction::Player, 3, 10, Vec2::new(0.0, 0.0), "P-home"));
    let n1 = w.add_planet(neutral_planet(seed + 11, 1, Vec2::new(35.0, 0.0), "n1"));
    let n2 = w.add_planet(neutral_planet(seed + 12, 1, Vec2::new(70.0, 0.0), "n2-centre"));
    let n3 = w.add_planet(neutral_planet(seed + 13, 1, Vec2::new(105.0, 0.0), "n3"));
    let e = w.add_planet(home_planet(seed + 1, Faction::Enemy, 3, 10, Vec2::new(140.0, 0.0), "E-home"));
    // Symmetric chain of equal-length lanes.
    let l = 35.0;
    w.add_lane(p, n1, l);
    w.add_lane(n1, n2, l);
    w.add_lane(n2, n3, l);
    w.add_lane(n3, e, l);
    w
}

/// A slightly richer symmetric world: two homes, each with a **private** flank neutral (close,
/// safe to colonize), plus a single shared **contested centre** neutral. A diamond:
///
/// ```text
///        fP          fE          (flank neutrals, one per side)
///       /  \        /  \
///   P-home   centre   E-home
/// ```
/// `fP`/`fE` are each adjacent only to their own home and the centre; the centre bridges both
/// sides. Mirror-symmetric. Good for separating colonize (grab the easy flank) from attack
/// (push through the centre) and defend (hold the flank+home).
pub fn diamond_world(seed: u64) -> World {
    let mut w = World::new();
    let p = w.add_planet(home_planet(seed, Faction::Player, 3, 10, Vec2::new(0.0, 0.0), "P-home"));
    let e = w.add_planet(home_planet(seed + 1, Faction::Enemy, 3, 10, Vec2::new(120.0, 0.0), "E-home"));
    let fp = w.add_planet(neutral_planet(seed + 11, 1, Vec2::new(30.0, 40.0), "fP"));
    let fe = w.add_planet(neutral_planet(seed + 12, 1, Vec2::new(90.0, 40.0), "fE"));
    let centre = w.add_planet(neutral_planet(seed + 13, 2, Vec2::new(60.0, 0.0), "centre"));
    w.add_lane(p, fp, 35.0);
    w.add_lane(e, fe, 35.0);
    w.add_lane(p, centre, 45.0);
    w.add_lane(e, centre, 45.0);
    w.add_lane(fp, centre, 40.0);
    w.add_lane(fe, centre, 40.0);
    w
}

/// A **shared-flank field**: a central cluster of neutrals that **both** homes reach directly, so
/// there is *no private flank* to hide expansion behind (the diamond's `fP`/`fE` is what let
/// Colonize out-develop Attack untouched). All neutral ground is contested ground. Mirror-symmetric.
///
/// ```text
///        nU
///       /  \
///   P-home--nC--E-home     (every neutral connects to BOTH homes; nU/nD give the cluster depth)
///       \  /
///        nD
/// ```
pub fn open_field(seed: u64) -> World {
    let mut w = World::new();
    let p = w.add_planet(home_planet(seed, Faction::Player, 3, 10, Vec2::new(0.0, 0.0), "P-home"));
    let e = w.add_planet(home_planet(seed + 1, Faction::Enemy, 3, 10, Vec2::new(140.0, 0.0), "E-home"));
    let nc = w.add_planet(neutral_planet(seed + 11, 2, Vec2::new(70.0, 0.0), "nC"));
    let nu = w.add_planet(neutral_planet(seed + 12, 1, Vec2::new(70.0, 45.0), "nU"));
    let nd = w.add_planet(neutral_planet(seed + 13, 1, Vec2::new(70.0, -45.0), "nD"));
    // Both homes reach every neutral directly (shared / contested — no private flank).
    for &(home, near) in &[(p, 70.0f32), (e, 70.0)] {
        w.add_lane(home, nc, near);
        w.add_lane(home, nu, 83.0);
        w.add_lane(home, nd, 83.0);
    }
    // Cluster interconnect (a little depth inside the contested zone).
    w.add_lane(nc, nu, 45.0);
    w.add_lane(nc, nd, 45.0);
    w
}

/// A **long corridor**: two homes with a deeper chain of FIVE neutrals between them (vs the
/// 3-neutral [`corridor_world`]). More neutral ground (so an aggressive expander can out-grab a
/// cautious one) and a longer supply line (so an attacker over-extends). Single axis — no flanks.
///
/// ```text
///   P-home — n1 — n2 — n3 — n4 — n5 — E-home
/// ```
pub fn long_corridor(seed: u64) -> World {
    let mut w = World::new();
    let l = 32.0;
    let p = w.add_planet(home_planet(seed, Faction::Player, 3, 10, Vec2::new(0.0, 0.0), "P-home"));
    let mut prev = p;
    let mut x = l;
    for i in 0..5u64 {
        let nn = w.add_planet(neutral_planet(seed + 11 + i, 1, Vec2::new(x, 0.0), "n"));
        w.add_lane(prev, nn, l);
        prev = nn;
        x += l;
    }
    let e = w.add_planet(home_planet(seed + 1, Faction::Enemy, 3, 10, Vec2::new(x, 0.0), "E-home"));
    w.add_lane(prev, e, l);
    w
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn corridor_is_symmetric_in_subs_and_ships() {
        let w = corridor_world(7);
        assert_eq!(w.total_subs(Faction::Player), w.total_subs(Faction::Enemy));
        assert_eq!(w.total_ships(Faction::Player), w.total_ships(Faction::Enemy));
        assert_eq!(w.planets.len(), 5);
    }

    #[test]
    fn diamond_is_symmetric() {
        let w = diamond_world(3);
        assert_eq!(w.total_subs(Faction::Player), w.total_subs(Faction::Enemy));
        assert_eq!(w.total_ships(Faction::Player), w.total_ships(Faction::Enemy));
    }

    #[test]
    fn a_match_runs_to_a_self_consistent_outcome() {
        let params = SimParams::default();
        let wp = WorldParams::default();
        let r = run_roster(corridor_world(1), &params, &wp, Roster::Colonize, Roster::Defend);
        // The outcome's ship/sub tallies must be internally consistent (non-negative; winner
        // matches the lead unless by elimination).
        let o = r.outcome;
        assert!(o.tick <= DEFAULT_HORIZON);
        let _ = (r.a_won(), r.b_won());
        // A well-formed outcome: someone leads or it's a draw.
        let p_score = o.ships.0 + o.subs.0;
        let e_score = o.ships.1 + o.subs.1;
        if !o.by_elimination {
            match o.winner {
                Some(Faction::Player) => assert!(p_score >= e_score),
                Some(Faction::Enemy) => assert!(e_score >= p_score),
                _ => {}
            }
        }
    }

    // ==================================================================================
    // Counter DIAGNOSTIC (COUNTER_DESIGN §7) — regression-protect the STABLE headlines.
    // ==================================================================================
    //
    // The full sweep (the `counter-diag` bin) is pasted into COUNTER_RESULTS.md; here we lock in only
    // the outcomes that are *stable* and headline (so a future change to the inference/synthesis that
    // breaks them is caught). We DELIBERATELY do not assert the Colonize/Attack cells — those are the
    // honest *unstable* findings (live-contact contamination flips the read; the Counter loses to
    // Attack), documented in the results, not regression-frozen.

    /// A two-seed slice of the diagnostic for a single `(target, p_max)` cell — fast, decisive, and
    /// deterministic. Returns the single [`CounterDiagCell`].
    fn diag_cell(target: CounterTarget, p_max: f32, seeds: &[u64], horizon: u64) -> CounterDiagCell {
        let rows = counter_diagnostic(&[target], &[p_max], seeds, horizon);
        rows.into_iter().next().unwrap().cells.into_iter().next().unwrap()
    }

    /// HEADLINE 1 — the **clean RPS win**: watching the pure Defend automaton, the Counter infers a
    /// **defend-dominant** mix (the right corner) and plays the **Colonize** backbone, and *wins*
    /// (colonize > defend). This is the textbook converge-on-the-RPS-counter-and-win result, and it
    /// is rock-solid (10/10 across the full sweep, every p_max), so we assert a clean sweep on a
    /// two-seed slice. The Counter beats the relevant pure target at this p_max (the brief's "beats
    /// or matches" headline).
    #[test]
    fn diag_counter_reads_defend_and_beats_it_with_colonize() {
        let target = CounterTarget {
            roster: Roster::Defend,
            truth: "Defend",
            build: diamond_world,
            map: "diamond",
        };
        let cell = diag_cell(target, 0.6, &[1, 7], DEFAULT_HORIZON);
        assert_eq!(cell.inferred_dominant, "Defend", "must read Defend's corner");
        assert!(cell.inferred_matches_truth, "the inferred mix points at the right (defend) corner");
        assert_eq!(cell.converged_backbone, "Colonize", "infer-Defend => play the Colonize backbone");
        // Clean RPS win: the Counter beats the pure Defender (colonize > defend).
        assert!(
            cell.counter_wins == cell.games(),
            "the Counter must sweep the pure Defender here, got {}/{}",
            cell.counter_wins,
            cell.games()
        );
    }

    /// HEADLINE 2 — the **seam exploit**: watching SimpleColonize (the documented thin-rear seam),
    /// the Counter infers a colonize-dominant identity, **fires** `never_guards_rear`, ships the
    /// `flank-undefended-rear` exploit (a projection-confirmed deviation), and **beats** it. We assert
    /// at the mid playstyle point (`p_max = 0.6`), where the exploit actually ships and the win-rate
    /// is high (9/10 over the full sweep). The Counter beats the relevant target here.
    #[test]
    fn diag_counter_fires_simplecolonize_seam_and_beats_it() {
        let target = CounterTarget {
            roster: Roster::SimpleColonize,
            truth: "Colonize",
            build: corridor_world,
            map: "corridor",
        };
        let cell = diag_cell(target, 0.6, &[1, 7], DEFAULT_HORIZON);
        assert!(cell.inferred_matches_truth, "SimpleColonize reads as the colonize family");
        assert!(cell.seam_fired, "the documented thin-rear seam (never_guards_rear) must fire");
        assert!(
            cell.exploits_shipped.iter().any(|e| e == "flank-undefended-rear"),
            "the projection-confirmed flank-rear exploit must ship at p_max=0.6, got {:?}",
            cell.exploits_shipped
        );
        // The Counter beats SimpleColonize (it both reads it AND punishes the seam).
        assert!(
            cell.counter_wins * 2 > cell.games(),
            "the Counter must win the majority vs SimpleColonize, got {}/{}",
            cell.counter_wins,
            cell.games()
        );
    }

    /// The diagnostic is **deterministic** (COUNTER_DESIGN §9): the same `(targets, p_max, seeds,
    /// horizon)` yields a bit-identical cell (win counts, inferred read, exploit telemetry). The
    /// projection draws no RNG and the profile is a deterministic function of the log, so any
    /// divergence would be a real nondeterminism bug.
    #[test]
    fn diag_is_deterministic() {
        let target =
            CounterTarget { roster: Roster::Defend, truth: "Defend", build: diamond_world, map: "diamond" };
        let a = diag_cell(target, 0.6, &[1, 7], 1000);
        let b = diag_cell(target, 0.6, &[1, 7], 1000);
        assert_eq!(a.counter_wins, b.counter_wins);
        assert_eq!(a.target_wins, b.target_wins);
        assert_eq!(a.draws, b.draws);
        assert_eq!(a.inferred_dominant, b.inferred_dominant);
        assert_eq!(a.converged_backbone, b.converged_backbone);
        assert_eq!(a.exploits_shipped, b.exploits_shipped);
        assert_eq!(a.exploit_ticks, b.exploit_ticks);
    }

    /// The diagnostic plumbing is well-formed: the full target set × the full p_max sweep yields one
    /// row per target and one cell per p_max, each cell a complete set of games (both seatings × the
    /// seeds). A cheap structural guard (short horizon) so a wiring regression is caught fast.
    #[test]
    fn diag_shape_is_well_formed() {
        let targets = counter_diag_targets();
        let seeds = [1u64, 7];
        let rows = counter_diagnostic(&targets, &COUNTER_DIAG_P_MAX, &seeds, 200);
        assert_eq!(rows.len(), targets.len(), "one row per target");
        for row in &rows {
            assert_eq!(row.cells.len(), COUNTER_DIAG_P_MAX.len(), "one cell per swept p_max");
            for c in &row.cells {
                assert_eq!(c.games(), (seeds.len() * 2) as u32, "both seatings × seeds games per cell");
                assert!(c.total_ticks > 0, "the Counter took at least one decision");
                assert!(c.exploit_ticks <= c.total_ticks, "exploit ticks are a subset of all ticks");
            }
        }
    }
}
