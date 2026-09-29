//! The **arena**: symmetric boards and a scoring rule for AI-vs-AI evolution and benchmarking,
//! played at the shipped game's operating point (reference tick rate, the game's combat dials).
//! Used by `examples/evolve.rs` and the strength tests; nothing in the live game depends on it.

use layer1::{Faction, Interior, Outcome, SimParams, SubStructure, Vec2};

use crate::controller::{Roster, SeatController};
use crate::evolved::{EvolvedController, Genome};
use crate::harness::{run_match, GAME_DECISION_BASE};

/// The game's combat/economy dials at reference tick rate (mirrors the game's `gui_params(1.0)`).
pub fn game_params() -> SimParams {
    let mut p = SimParams::default();
    p.fire_prob = 0.0055;
    p.defender_fire_bonus = 0.0;
    p.transit_fire_gating = true;
    p.spread_damage = true;
    p.per_sub_attrition = true;
    p.ring_jitter_step = 0.03;
    p
}

/// Who can sit in a seat.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Contender {
    Roster(Roster),
    Genome(Genome),
}

impl Contender {
    pub fn controller(&self, seat: Faction) -> SeatController {
        match self {
            Contender::Roster(r) => SeatController::from_roster(seat, *r),
            Contender::Genome(g) => SeatController::Evolved(EvolvedController::new(seat, g)),
        }
    }
}

/// The benchmark boards. Every one is symmetric under swapping the seats.
pub const BOARDS: [&str; 5] = ["corridor", "diamond", "pairs", "wall", "ring"];

fn post(st: &mut Interior, x: f32, y: f32, owner: Faction, cap: u32, prod: u32) -> usize {
    st.add_sub(
        SubStructure::new(Vec2::new(x, y), 0.0, owner).with_storage_capacity(cap).with_production(prod),
    )
}

fn garrison(st: &mut Interior, sub: usize, seat: Faction, n: usize) {
    for _ in 0..n {
        st.spawn_ship(seat, sub);
    }
}

/// Build board `name` (one of [`BOARDS`]) with `seed`.
pub fn board(name: &str, seed: u64) -> Interior {
    let (p, e) = (Faction::Player, Faction::Ai(0));
    let mut st = Interior::new(seed);
    match name {
        "corridor" => {
            let a = post(&mut st, -60.0, 0.0, p, 60, 2);
            let b = post(&mut st, 60.0, 0.0, e, 60, 2);
            for x in [-30.0f32, 0.0, 30.0] {
                post(&mut st, x, 0.0, Faction::Neutral, 60, 2);
            }
            garrison(&mut st, a, p, 60);
            garrison(&mut st, b, e, 60);
        }
        "diamond" => {
            let a = post(&mut st, -60.0, 0.0, p, 60, 2);
            let b = post(&mut st, 60.0, 0.0, e, 60, 2);
            post(&mut st, 0.0, 30.0, Faction::Neutral, 90, 3);
            post(&mut st, 0.0, -30.0, Faction::Neutral, 90, 3);
            garrison(&mut st, a, p, 60);
            garrison(&mut st, b, e, 60);
        }
        "pairs" => {
            for (sx, seat) in [(-1.0f32, p), (1.0, e)] {
                for y in [14.0f32, -14.0] {
                    let s = post(&mut st, sx * 28.0, y, seat, 60, 2);
                    garrison(&mut st, s, seat, 50);
                }
            }
            for (x, y) in [(0.0f32, 26.0f32), (0.0, -26.0), (0.0, 0.0)] {
                post(&mut st, x, y, Faction::Neutral, 60, 2);
            }
        }
        "wall" => {
            for (sx, seat) in [(-1.0f32, p), (1.0, e)] {
                let h = post(&mut st, sx * 75.0, 0.0, seat, 60, 2);
                garrison(&mut st, h, seat, 60);
                let f = st.add_sub(SubStructure::fortress(Vec2::new(sx * 25.0, 0.0), seat));
                garrison(&mut st, f, seat, 40);
                post(&mut st, sx * 50.0, 22.0, Faction::Neutral, 60, 2);
                post(&mut st, sx * 50.0, -22.0, Faction::Neutral, 60, 2);
            }
        }
        _ => {
            // "ring": homes at the poles, six neutral posts round the middle.
            let a = post(&mut st, -70.0, 0.0, p, 60, 2);
            let b = post(&mut st, 70.0, 0.0, e, 60, 2);
            for k in 0..6 {
                let ang = k as f32 * std::f32::consts::TAU / 6.0;
                post(&mut st, 34.0 * ang.cos(), 34.0 * ang.sin(), Faction::Neutral, 60, 2);
            }
            garrison(&mut st, a, p, 60);
            garrison(&mut st, b, e, 60);
        }
    }
    st
}

/// Play `a` (seat `Player` if `a_is_player`) against `b` on `board_name`; returns the outcome.
pub fn play(
    board_name: &str,
    seed: u64,
    a: &Contender,
    b: &Contender,
    a_is_player: bool,
    horizon: u64,
) -> Outcome {
    let params = game_params();
    let mut st = board(board_name, seed);
    let (sa, sb) = if a_is_player { (Faction::Player, Faction::Ai(0)) } else { (Faction::Ai(0), Faction::Player) };
    let mut ca = a.controller(sa);
    let mut cb = b.controller(sb);
    run_match(&mut st, &params, &mut ca, &mut cb, horizon, GAME_DECISION_BASE)
}

/// Score in `[0, 1.2]` for the contender that sat in `seat`: a win is worth `1` plus up to `0.2`
/// for finishing early; otherwise up to `0.45` for the ship and sub lead held at the horizon.
pub fn score(o: &Outcome, seat: Faction, horizon: u64) -> f32 {
    let (mine_ships, foe_ships, mine_subs, foe_subs) = if seat == Faction::Player {
        (o.ships.0, o.ships.1, o.subs.0, o.subs.1)
    } else {
        (o.ships.1, o.ships.0, o.subs.1, o.subs.0)
    };
    if o.winner == Some(seat) {
        return 1.0 + 0.2 * (1.0 - (o.tick as f32 / horizon as f32).min(1.0));
    }
    let share = |a: usize, b: usize| if a + b == 0 { 0.5 } else { a as f32 / (a + b) as f32 };
    0.3 * share(mine_ships, foe_ships) + 0.15 * share(mine_subs, foe_subs)
}

/// The mean [`score`] of `a` against `b` over every board in [`BOARDS`] and both seatings.
pub fn duel_score(a: &Contender, b: &Contender, seed: u64, horizon: u64) -> f32 {
    let mut total = 0.0;
    let mut n = 0.0;
    for name in BOARDS {
        for a_is_player in [true, false] {
            let o = play(name, seed, a, b, a_is_player, horizon);
            let seat = if a_is_player { Faction::Player } else { Faction::Ai(0) };
            total += score(&o, seat, horizon);
            n += 1.0;
        }
    }
    total / n
}
