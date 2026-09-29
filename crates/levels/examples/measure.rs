//! Rough difficulty probe: plays a stand-in player (a roster brain, default Simple v1) against a
//! level's real enemy seats over several seeds, at the reference tick rate with the game's
//! combat dials, and prints the outcomes. It is a yardstick, not a gate — a human plays far
//! better than Simple and far worse than a bred lineage.
//!
//! ```sh
//! cargo run -p levels --release --example measure -- [level id ...] [--seeds N] [--as greedy|simple|cycler|opportunist|evolved:N]
//! ```

use ai::arena::game_params;
use ai::harness::GAME_DECISION_BASE;
use ai::{Roster, SeatController, SimpleVersion};
use layer1::Faction;

fn main() {
    let mut ids: Vec<u32> = Vec::new();
    let mut seeds = 6u64;
    let mut stand_in = Roster::SimpleColonize { version: SimpleVersion::V1 };
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--seeds" => {
                seeds = args[i + 1].parse().unwrap();
                i += 1;
            }
            "--as" => {
                stand_in = match args[i + 1].as_str() {
                    "greedy" => Roster::GreedyLocal,
                    "simple" => stand_in,
                    "cycler" => Roster::Cycler,
                    "opportunist" => Roster::Opportunist,
                    other => match other.strip_prefix("evolved:").and_then(|n| n.parse::<u8>().ok()) {
                        Some(lineage) => Roster::Evolved { lineage },
                        None => panic!("unknown stand-in `{other}` (greedy|simple|cycler|opportunist|evolved:N)"),
                    },
                };
                i += 1;
            }
            other => ids.push(other.parse().expect("level id")),
        }
        i += 1;
    }
    let params = game_params();
    println!("stand-in player: {}", stand_in.name());
    for lvl in levels::campaign() {
        if !ids.is_empty() && !ids.contains(&lvl.id) {
            continue;
        }
        let (mut wins, mut losses, mut draws, mut ticks) = (0, 0, 0, 0u64);
        for seed in 1..=seeds {
            let mut st = lvl.interior(seed);
            let mut me = SeatController::from_roster(Faction::Player, stand_in);
            let mut foes: Vec<SeatController> = lvl
                .enemies
                .iter()
                .enumerate()
                .map(|(k, &r)| SeatController::from_roster(Faction::Ai(k as u8), r))
                .collect();
            while st.tick < lvl.horizon {
                if st.is_eliminated(Faction::Player)
                    || (0..foes.len()).all(|k| st.is_eliminated(Faction::Ai(k as u8)))
                {
                    break;
                }
                if st.tick % GAME_DECISION_BASE == 0 {
                    me.decide_and_apply(&mut st, &params);
                    for f in &mut foes {
                        f.decide_and_apply(&mut st, &params);
                    }
                }
                st.step(&params);
            }
            ticks += st.tick;
            let foes_alive = (0..foes.len()).any(|k| !st.is_eliminated(Faction::Ai(k as u8)));
            if st.is_eliminated(Faction::Player) {
                losses += 1;
            } else if !foes_alive {
                wins += 1;
            } else {
                draws += 1;
            }
        }
        println!(
            "L{:>2} {:<22} vs {:<34} wins {wins} losses {losses} unresolved {draws}  (mean {} ticks)",
            lvl.id,
            lvl.title,
            lvl.enemies.iter().map(|r| r.label()).collect::<Vec<_>>().join(" + "),
            ticks / seeds
        );
    }
}
