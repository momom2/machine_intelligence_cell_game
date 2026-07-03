//! Headless **validation** of the campaign — the real test the GUI's content rests on.
//!
//! For every level this checks three things:
//!
//! 1. **Interior** — the built [`world::World`] matches the level's spec: struct count, each
//!    struct's sub-structure count and per-faction ownership, and lane connectivity (the lane
//!    count + that the intended pairs are connected).
//! 2. **Determinism** — building the same level with the same seed twice yields the **same
//!    [`world::World::state_hash`]**, and a short scripted match replays bit-for-bit (the
//!    substrate's guarantee, re-confirmed at the level layer).
//! 3. **Lesson holds** *(measured, not gated during the full-Simple transition)* — the level is
//!    **not auto-lost** by a competent player proxy against **every** declared enemy seat,
//!    driven through the same [`SeatController`] roster→brain dispatch the game uses (so the
//!    stateful live Simple is what is measured) at the game's reference decision cadence.
//!    The specialised automata-track lessons (the L8-L10 RPS counters, the L7 seam flank) are
//!    kept as documented dormant checks for the automata revival.
//!
//! Each [`LevelReport`] collects the pass/fail of these checks (plus the measured win-loss
//! numbers) so the lib test can assert on them and the report text can quote them. Everything is
//! deterministic — no randomness lives outside each struct's seeded `Interior`.

use ai::harness::{run_match, DEFAULT_DECISION_INTERVAL, GAME_DECISION_BASE};
use ai::{AiController, Roster, SeatController};
use layer1::{Faction, FractionBucket, SimParams};
use world::{FleetOrder, StructOwner, World, WorldParams};

use crate::{Level, StartView};

/// The seeds the curriculum-sanity measurements sweep. A small fixed set (the same spirit as the
/// `ai` suite's seeds) — enough to show a lesson holds robustly, few enough to keep the test
/// fast.
pub const VALIDATION_SEEDS: [u64; 5] = [1, 7, 42, 2024, 31337];

/// The structural expectation for one level: total structs, per-structure
/// `(sub_count, player_subs, enemy_subs, neutral_subs)`, expected lane count, and the structure
/// pairs that must be connected. Authored alongside each level so a drift in a `build` function
/// is caught immediately.
#[derive(Debug, Clone)]
struct Spec {
    structs: usize,
    /// Per structure, in `StructId` order: `(total_subs, player_subs, enemy_subs, neutral_subs)`.
    subs: Vec<(usize, usize, usize, usize)>,
    lanes: usize,
    /// Structure-id pairs that must be lane-connected.
    connected: Vec<(usize, usize)>,
}

/// The pass/fail outcome of validating one level, with the measured numbers behind the
/// curriculum-sanity verdict.
#[derive(Debug, Clone)]
pub struct LevelReport {
    pub id: u32,
    pub title: String,
    /// The built world matched the structural spec.
    pub structure_ok: bool,
    /// First structural mismatch (if any), for a readable failure.
    pub structure_detail: Option<String>,
    /// Building twice (and replaying a scripted match) was deterministic.
    pub deterministic: bool,
    /// The intended lesson held when measured.
    pub lesson_ok: bool,
    /// Human-readable measured result behind `lesson_ok` (e.g. "Attack beats Colonize 10-0").
    pub lesson_detail: String,
}

impl LevelReport {
    /// Hard invariants a level must always satisfy: it builds to the authored **structure** and
    /// replays **deterministically**. The **lesson** (`lesson_ok` / `lesson_detail`) is measured
    /// and reported but **not gated** while the campaign is mid-transition to a full-Simple
    /// roster with its difficulty curve + level redesign still pending.
    pub fn ok(&self) -> bool {
        self.structure_ok && self.deterministic
    }
}

/// Validate the whole campaign, returning a [`LevelReport`] per level (in order).
pub fn validate_campaign() -> Vec<LevelReport> {
    crate::campaign().iter().map(validate_level).collect()
}

/// Validate a single level's **gates only** — structure + determinism, the two checks
/// [`LevelReport::ok`] actually decides on — skipping the minutes-long informational lesson
/// sweep (full matches over the seed set). The gated lib test runs this; the lesson numbers
/// come from the `#[ignore]`d `print_validation_report` tool (which uses the full
/// [`validate_level`]).
pub fn validate_level_gates(level: &Level) -> LevelReport {
    let (structure_ok, structure_detail) = check_structure(level);
    let deterministic = check_determinism(level);
    LevelReport {
        id: level.id,
        title: level.title.clone(),
        structure_ok,
        structure_detail,
        deterministic,
        lesson_ok: true,
        lesson_detail: "(lesson not measured — see the ignored print_validation_report tool)".into(),
    }
}

/// Validate a single level: structure + determinism + the intended lesson.
pub fn validate_level(level: &Level) -> LevelReport {
    let (structure_ok, structure_detail) = check_structure(level);
    let deterministic = check_determinism(level);
    let (lesson_ok, lesson_detail) = check_lesson(level);
    LevelReport {
        id: level.id,
        title: level.title.clone(),
        structure_ok,
        structure_detail,
        deterministic,
        lesson_ok,
        lesson_detail,
    }
}

// ======================================================================================
// (1) Interior.
// ======================================================================================

/// The authored structural spec for each level id. Kept here (next to the checker) so the spec
/// is an independent statement of intent the `build` function must satisfy.
fn spec_for(id: u32) -> Spec {
    match id {
        // L1: one structure, 5 subs (square + centre) — Player 1, Enemy 1 (centre), Neutral 3. No lanes.
        1 => Spec { structs: 1, subs: vec![(5, 1, 1, 3)], lanes: 0, connected: vec![] },
        // L2: one structure, 6 subs (4-square middle + 2 side homes) — Player 1, Enemy 1, Neutral 4. No lanes.
        2 => Spec { structs: 1, subs: vec![(6, 1, 1, 4)], lanes: 0, connected: vec![] },
        // L3: one struct (Layer-1-only), 13 subs — Player 1 (A), two AI seats 1 each (B = Enemy,
        // C = Enemy2), 10 neutral (6 chain + 2 upper "1" + 2 lower "0"). The binary `(_,_,enemy,_)`
        // tuple predates `Enemy2`, so both AI seats are counted under `enemy` (2). No lanes.
        3 => Spec {
            structs: 1,
            subs: vec![(13, 1, 2, 10)],
            lanes: 0,
            connected: vec![],
        },
        // L4: two 4-sub homes + one lane.
        4 => Spec {
            structs: 2,
            subs: vec![(4, 4, 0, 0), (4, 0, 4, 0)],
            lanes: 1,
            connected: vec![(0, 1)],
        },
        // L5: triangle — two 3-sub homes + a 2-sub neutral; 3 lanes.
        5 => Spec {
            structs: 3,
            subs: vec![(3, 3, 0, 0), (3, 0, 3, 0), (2, 0, 0, 2)],
            lanes: 3,
            connected: vec![(0, 2), (1, 2), (0, 1)],
        },
        // L6: four structs — two 3-sub homes, a fat 3-sub prize, two 1-sub spurs; 6 lanes.
        6 => Spec {
            structs: 5,
            subs: vec![(3, 3, 0, 0), (3, 0, 3, 0), (3, 0, 0, 3), (1, 0, 0, 1), (1, 0, 0, 1)],
            lanes: 6,
            connected: vec![(0, 2), (1, 2), (0, 3), (1, 4), (3, 2), (4, 2)],
        },
        // L7: seam — Player 3-sub home, Enemy 1-sub rear, two 1-sub baits; 3 lanes.
        7 => Spec {
            structs: 4,
            subs: vec![(3, 3, 0, 0), (1, 0, 1, 0), (1, 0, 0, 1), (1, 0, 0, 1)],
            lanes: 3,
            connected: vec![(0, 1), (1, 2), (2, 3)],
        },
        // L8/L9/L10: the diamond — two 3-sub homes, two 1-sub flank neutrals, a 2-sub centre;
        // 6 lanes. Structure order from `builders::diamond`: P=0, E=1, fP=2, fE=3, centre=4.
        8..=10 => Spec {
            structs: 5,
            subs: vec![(3, 3, 0, 0), (3, 0, 3, 0), (1, 0, 0, 1), (1, 0, 0, 1), (2, 0, 0, 2)],
            lanes: 6,
            connected: vec![(0, 2), (1, 3), (0, 4), (1, 4), (2, 4), (3, 4)],
        },
        _ => Spec { structs: 0, subs: vec![], lanes: 0, connected: vec![] },
    }
}

/// Check the built world against [`spec_for`]. Returns `(ok, first_mismatch)`.
fn check_structure(level: &Level) -> (bool, Option<String>) {
    let spec = spec_for(level.id);
    let (w, _wp) = level.world(1);

    if w.structs.len() != spec.structs {
        return (false, Some(format!("struct count {} != {}", w.structs.len(), spec.structs)));
    }
    if w.lanes.len() != spec.lanes {
        return (false, Some(format!("lane count {} != {}", w.lanes.len(), spec.lanes)));
    }
    for (pid, &(tot, ps, es, ns)) in spec.subs.iter().enumerate() {
        let agg = w.struct_aggregate(pid);
        let got_total = agg.player_subs + agg.enemy_subs + agg.neutral_subs;
        if got_total != tot {
            return (
                false,
                Some(format!("struct {pid} total subs {got_total} != {tot}")),
            );
        }
        if (agg.player_subs, agg.enemy_subs, agg.neutral_subs) != (ps, es, ns) {
            return (
                false,
                Some(format!(
                    "struct {pid} ownership (P{},E{},N{}) != (P{ps},E{es},N{ns})",
                    agg.player_subs, agg.enemy_subs, agg.neutral_subs
                )),
            );
        }
    }
    for &(a, b) in &spec.connected {
        if !w.are_connected(a, b) {
            return (false, Some(format!("structs {a}-{b} should be lane-connected")));
        }
    }
    (true, None)
}

// ======================================================================================
// (2) Determinism.
// ======================================================================================

/// Build the level twice with the same seed and compare `state_hash`; then run a short scripted
/// match twice and compare per-tick hashes + outcome. Returns `true` if everything matched.
fn check_determinism(level: &Level) -> bool {
    let params = SimParams::default();

    // (a) Two fresh builds are bit-identical.
    let (w1, _) = level.world(42);
    let (w2, _) = level.world(42);
    if w1.state_hash() != w2.state_hash() {
        return false;
    }

    // (b) A scripted player-greedy vs enemy-seats match replays identically (per-tick hashes).
    // Every declared seat is driven (`enemies[i]` → `Ai(i)`) through the same [`SeatController`]
    // dispatch the game uses — so the stateful live Simple, not the parked automaton, is what
    // replays — at the game's reference decision cadence.
    let replay = |level: &Level| -> (Vec<u64>, world::WorldOutcome) {
        let (mut w, wp) = level.world(7);
        let player = AiController::from_roster(Faction::Player, Roster::GreedyLocal);
        let mut enemies = enemy_seats(level);
        let mut hashes = Vec::new();
        for t in 0..300u64 {
            if w.is_eliminated(Faction::Player) || all_enemies_eliminated(&w, level) {
                break;
            }
            if t % GAME_DECISION_BASE == 0 {
                // Player decides+applies first (the documented tie-break), then each enemy seat
                // in ascending index order — matching the game's per-tick seat order.
                let dp = player.decide(&w, &params, &wp);
                player.apply(&mut w, &dp, &wp);
                for e in &mut enemies {
                    e.decide_and_apply(&mut w, &params, &wp);
                }
            }
            w.step(&params, &wp);
            hashes.push(w.state_hash());
        }
        (hashes, w.outcome())
    };
    let (h1, o1) = replay(level);
    let (h2, o2) = replay(level);
    h1 == h2 && o1 == o2
}

/// One [`SeatController`] per declared enemy seat (`level.enemies[i]` drives `Faction::Ai(i)`),
/// through the same roster→brain dispatch the game uses.
fn enemy_seats(level: &Level) -> Vec<SeatController> {
    level
        .enemies
        .iter()
        .enumerate()
        .map(|(i, &r)| SeatController::from_roster(Faction::Ai(i as u8), r))
        .collect()
}

/// True when **every** declared enemy seat is world-wide eliminated (the multi-seat analogue of
/// the old binary `is_eliminated(Ai(0))` early-out).
fn all_enemies_eliminated(w: &World, level: &Level) -> bool {
    (0..level.enemies.len()).all(|i| w.is_eliminated(Faction::Ai(i as u8)))
}

// ======================================================================================
// (3) Lesson holds.
// ======================================================================================

/// Measure the level's lesson. The campaign is mid-transition to a **full Simple campaign** with a
/// difficulty curve + level redesign still to come, so the only lesson we measure for *every* level
/// now is the generic "a competent player proxy is not auto-lost vs the level's enemy" — reported
/// for information but **not gating** (see [`LevelReport::ok`]). The specialised seam / RPS lessons
/// ([`seam_flank_beats_greedy`], [`counter_beats_enemy`]) are dormant, kept for the automata-track
/// redesign.
fn check_lesson(level: &Level) -> (bool, String) {
    not_auto_lost(level)
}

/// Run `counter` (a Roster strategy) against the level's enemy on the level's map over **both
/// seatings** and `VALIDATION_SEEDS`, and require the counter to win a strict majority (the
/// rock-paper-scissors edge the lesson teaches). Returns the win-loss tally in the detail.
#[allow(dead_code)] // dormant: kept for the automata-track redesign (RPS lessons)
fn counter_beats_enemy(
    level: &Level,
    counter: Roster,
    counter_name: &str,
    enemy_name: &str,
) -> (bool, String) {
    let params = SimParams::default();
    let mut wins = 0u32;
    let mut losses = 0u32;
    let mut draws = 0u32;

    for &seed in &VALIDATION_SEEDS {
        // Seating 1: counter = Player, enemy = Enemy.
        let (mut w, wp) = level.world(seed);
        let o = match_on(&mut w, &params, &wp, counter, level.primary_enemy(), level.horizon);
        tally(o.winner, Faction::Player, &mut wins, &mut losses, &mut draws);

        // Seating 2: counter = Enemy, enemy = Player (swap seats, same map).
        let (mut w, wp) = level.world(seed);
        let o = match_on(&mut w, &params, &wp, level.primary_enemy(), counter, level.horizon);
        tally(o.winner, Faction::Ai(0), &mut wins, &mut losses, &mut draws);
    }

    let ok = wins > losses;
    let detail = format!(
        "{counter_name} vs {enemy_name} on this map: {wins}-{losses}-{draws} (wins-losses-draws over {} seeds x 2 seatings)",
        VALIDATION_SEEDS.len()
    );
    (ok, detail)
}

/// L7: the scripted rear-flank. Greedy (the enemy seat) decides+acts on its own; the player
/// proxy does the one thing the seam invites — mass its whole home and punch the greedy rear
/// across the short strike lane every decision interval.
///
/// **Re-expressed for the new resistance/denial model** (mirrors the `ai` suite's
/// `greedy_seam_thin_rear_is_exploitable`). Capture is no longer instant — taking the fresh
/// `max_resistance ≈ 1800` rear is a long grind — so the seam shows up as **sustained denial**:
/// the flank reaches greedy's undefended rear and *sits there uncontested* (greedy posts no rear
/// guard, it is busy chasing the lure corridor), starving its production and grinding it down. We
/// require, in a majority of seeds, that the flank either **captures** the rear OR holds a
/// **sustained uncontested-presence streak** on it (the spatial signature of the seam).
#[allow(dead_code)] // dormant: kept for the automata-track redesign (seam tactics lesson)
fn seam_flank_beats_greedy(level: &Level) -> (bool, String) {
    let params = SimParams::default();
    let wp = WorldParams::default();
    // Player home is struct 0, the greedy rear is struct 1 (see L7's build / spec).
    let (p_home, e_rear) = (0usize, 1usize);
    // Consecutive decision windows of Player-present / Enemy-absent on the rear = the denial streak.
    const DENY_STREAK_WINDOWS: u32 = 20;
    let mut exploited = 0u32;

    for &seed in &VALIDATION_SEEDS {
        let (mut w, _wp) = level.world(seed);
        let greedy = AiController::from_roster(Faction::Ai(0), Roster::GreedyLocal);
        let mut exploited_this_seed = false;
        let mut deny_streak = 0u32;
        for t in 0..level.horizon {
            if w.is_eliminated(Faction::Player) || w.is_eliminated(Faction::Ai(0)) {
                exploited_this_seed = true;
                break;
            }
            if t % DEFAULT_DECISION_INTERVAL == 0 {
                greedy.decide_and_apply(&mut w, &params, &wp);
                // Flank: mass the home straight at the thin rear, keep feeding the grind.
                w.issue_fleet_order(
                    FleetOrder::new(p_home, e_rear, FractionBucket::All),
                    Faction::Player,
                    &wp,
                );
                let agg = w.struct_aggregate(e_rear);
                if matches!(agg.owner, StructOwner::Owned(Faction::Player)) {
                    exploited_this_seed = true;
                    break;
                }
                if agg.ships_of(Faction::Player) > 0 && agg.ships_of(Faction::Ai(0)) == 0 {
                    deny_streak += 1;
                    if deny_streak >= DENY_STREAK_WINDOWS {
                        exploited_this_seed = true;
                        break;
                    }
                } else {
                    deny_streak = 0;
                }
            }
            w.step(&params, &wp);
        }
        if exploited_this_seed {
            exploited += 1;
        }
    }

    let ok = exploited * 2 > VALIDATION_SEEDS.len() as u32;
    let detail = format!(
        "rear-flank captured OR sustained-denied greedy's rear in {exploited}/{} seeds",
        VALIDATION_SEEDS.len()
    );
    (ok, detail)
}

/// The level must be **winnable** — a competent player proxy is not auto-lost. We require
/// the player to **win the strict majority** of games (a winnable level should reward competent
/// play, not merely avoid an auto-loss). **Every** declared enemy seat plays, through the same
/// [`SeatController`] dispatch the game uses (so the stateful live Simple is what is measured,
/// and L3's second seat actually fights), at the game's reference decision cadence.
///
/// The proxy is chosen to model *competence at the lens the level opens in*:
/// * **Layer-1 micro tutorials (the `StartView::Layer1` levels, L1-L3)** — a scripted
///   **concentration** proxy that masses each owned sub's idle ships onto the nearest
///   capturable sub-structure each decision tick. This directly enacts the tutorials' lesson
///   (concentration of force, capture-forward) and is a fair model of a human who pushes rather
///   than dribbles. (The generic greedy baseline is a *poor* competence model here precisely
///   because it dribbles surplus and never concentrates — the very mistake L2 teaches against —
///   so it is not the right yardstick for these levels.)
/// * **Layer-2 levels (L4-L10)** — the greedy baseline ([`Roster::GreedyLocal`]) playing the
///   Player seat: a sensible all-round automaton, the natural "competent player" at the tactical
///   layer.
fn not_auto_lost(level: &Level) -> (bool, String) {
    let params = SimParams::default();
    let mut player_wins = 0u32;
    let mut player_losses = 0u32;
    let mut draws = 0u32;

    let layer1_micro = matches!(level.start_view, StartView::Layer1(_));
    for &seed in &VALIDATION_SEEDS {
        let (mut w, wp) = level.world(seed);
        let o = if layer1_micro {
            run_layer1_concentration_proxy(&mut w, &params, &wp, level)
        } else {
            run_level_match(&mut w, &params, &wp, level)
        };
        tally(o.winner, Faction::Player, &mut player_wins, &mut player_losses, &mut draws);
    }

    // A winnable level: the competent player proxy wins the strict majority of games.
    let ok = player_wins * 2 > VALIDATION_SEEDS.len() as u32;
    let proxy = if layer1_micro { "Layer-1 concentration proxy" } else { "Layer-2 greedy proxy" };
    let detail = format!(
        "{proxy} vs {:?}: player {player_wins}-{player_losses}-{draws} (wins-losses-draws over {} seeds)",
        level.enemies,
        VALIDATION_SEEDS.len()
    );
    (ok, detail)
}

/// Run one match on `w` with the Player driven by [`Roster::GreedyLocal`] and **every** declared
/// enemy seat driven by the level's roster entries (the game's [`SeatController`] dispatch), at
/// the game's reference cadence. Player decides+applies first (the documented tie-break), then
/// each enemy seat in ascending index order — the game's per-tick seat order.
fn run_level_match(
    w: &mut World,
    params: &SimParams,
    wp: &WorldParams,
    level: &Level,
) -> world::WorldOutcome {
    let player = AiController::from_roster(Faction::Player, Roster::GreedyLocal);
    let mut enemies = enemy_seats(level);
    while w.tick < level.horizon {
        if w.is_eliminated(Faction::Player) || all_enemies_eliminated(w, level) {
            break;
        }
        if w.tick % GAME_DECISION_BASE == 0 {
            let dp = player.decide(w, params, wp);
            player.apply(w, &dp, wp);
            for e in &mut enemies {
                e.decide_and_apply(w, params, wp);
            }
        }
        w.step(params, wp);
    }
    w.outcome()
}

/// Run a single-struct Layer-1 match where the **Player** is driven by a scripted *concentration*
/// proxy and **every** declared enemy seat is driven by its roster entry (through the game's
/// [`SeatController`] dispatch — on a single struct only the tactical/Layer-1 play matters; there
/// are no lanes, so the strategic layer issues nothing). The player proxy enacts "concentration
/// of force": every decision interval it sends **all** idle ships from each Player-owned sub
/// toward the **nearest capturable** sub-structure (neutrals first, then the enemy).
/// Deterministic; returns the world outcome.
fn run_layer1_concentration_proxy(
    w: &mut World,
    params: &SimParams,
    wp: &WorldParams,
    level: &Level,
) -> world::WorldOutcome {
    let mut enemies = enemy_seats(level);
    while w.tick < level.horizon {
        if w.is_eliminated(Faction::Player) || all_enemies_eliminated(w, level) {
            break;
        }
        if w.tick % GAME_DECISION_BASE == 0 {
            // Player proxy (applies first — the documented tie-break): concentrate forward.
            let orders = concentration_orders(&w.structs[0].interior);
            for o in orders {
                w.structs[0].interior.issue_order(o, Faction::Player);
            }
            // Each enemy seat: its own play on the single structure, ascending index order.
            for e in &mut enemies {
                e.decide_and_apply(w, params, wp);
            }
        }
        w.step(params, wp);
    }
    w.outcome()
}

/// The concentration proxy's per-decision orders for the Player on a single structure: from each
/// **Player-owned** sub with idle ships, send **all** of them to the **nearest capturable** sub
/// (Euclidean nearest; lowest [`layer1::SubId`] breaks ties) — the nearest **neutral** while any
/// remains, else the nearest enemy sub. Capturing neutrals first then pressing the enemy is
/// "advance toward the nearest foreign ground", and sending the full idle stack is the "mass,
/// don't dribble" the tutorial teaches. The ownerless struct-storage node is **never** a target —
/// it cannot be captured (`resolve_resistance` skips it), so pouring the proxy's army into it
/// would only strand ships (the same trap the AI's `Layer1View` avoids by presenting the node as
/// the seat's own position).
fn concentration_orders(st: &layer1::Interior) -> Vec<layer1::MoveOrder> {
    use layer1::{FractionBucket, MoveOrder};
    let mut orders = Vec::new();
    for s in 0..st.subs.len() {
        if st.subs[s].owner != Faction::Player {
            continue;
        }
        if st.idle_count_at(s, Faction::Player) == 0 {
            continue;
        }
        // Nearest capturable sub: neutrals first (cheap, productive ground), else enemy subs.
        // Never the storage node and never a Player-owned sub.
        let from = st.subs[s].pos;
        let mut best_neutral: Option<(usize, f32)> = None;
        let mut best_enemy: Option<(usize, f32)> = None;
        for t in 0..st.subs.len() {
            if t == s || st.is_storage(t) || st.subs[t].owner == Faction::Player {
                continue;
            }
            let d = from.dist(st.subs[t].pos);
            let slot = if st.subs[t].owner == Faction::Neutral {
                &mut best_neutral
            } else {
                &mut best_enemy
            };
            match slot {
                Some((_, bd)) if *bd <= d => {}
                _ => *slot = Some((t, d)),
            }
        }
        if let Some((target, _)) = best_neutral.or(best_enemy) {
            orders.push(MoveOrder::new(s, target, FractionBucket::All));
        }
    }
    orders
}

// ======================================================================================
// Shared match runner + tally.
// ======================================================================================

/// Run one match on `w` to `horizon`: roster `a` plays the **Player** seat, roster `b` plays the
/// **Enemy** seat. Delegates to the `ai` harness's [`run_match`] (Player applies first — the
/// documented tie-break — and both decide on the same pre-step snapshot every
/// [`DEFAULT_DECISION_INTERVAL`] ticks). Returns the world outcome.
fn match_on(
    w: &mut World,
    params: &SimParams,
    wp: &WorldParams,
    a: Roster,
    b: Roster,
    horizon: u64,
) -> world::WorldOutcome {
    let ca = AiController::from_roster(Faction::Player, a);
    let cb = AiController::from_roster(Faction::Ai(0), b);
    run_match(w, params, wp, &ca, &cb, horizon, DEFAULT_DECISION_INTERVAL)
}

/// Tally a match's winner from the perspective of `me`: increment wins if `me` won, losses if
/// the opponent won, draws otherwise.
fn tally(winner: Option<Faction>, me: Faction, wins: &mut u32, losses: &mut u32, draws: &mut u32) {
    match winner {
        Some(f) if f == me => *wins += 1,
        Some(_) => *losses += 1,
        None => *draws += 1,
    }
}
