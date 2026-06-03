//! Diagnostic: print the three cycle-edge margins per map across a coarse grid so
//! we can see which edge is failing and where. Not part of the shipped sweep.

use cell_core::harness::{cycle_on_map, duel, Archetype};
use cell_core::maps::all_maps;
use cell_core::policy::{Attack, Colonize, Defend, Policy};
use cell_core::types::Owner;
use cell_core::{GameState, Params};

/// Trace a single match between two named archetypes on a given map, printing the
/// board every `every` ticks so we can see what the policies actually do.
fn trace(map: &GameState, a: Archetype, b: Archetype, params: &Params, horizon: u64, every: u64) {
    let mk = |x: Archetype| -> Box<dyn Policy> {
        match x {
            Archetype::Colonize => Box::new(Colonize),
            Archetype::Defend => Box::new(Defend),
            Archetype::Attack => Box::new(Attack),
        }
    };
    let mut pa = mk(a);
    let mut pb = mk(b);
    let mut s = map.clone();
    pa.reset();
    pb.reset();
    println!("\n### TRACE {} (A) vs {} (B) ###", a.name(), b.name());
    let mut t = 0u64;
    while t < horizon {
        if s.is_eliminated(Owner::A) || s.is_eliminated(Owner::B) {
            break;
        }
        let ca = pa.decide(&s, Owner::A, params);
        let cb = pb.decide(&s, Owner::B, params);
        for c in ca {
            s.launch_with(Owner::A, c, params);
        }
        for c in cb {
            s.launch_with(Owner::B, c, params);
        }
        s.step(params);
        t += 1;
        if t % every == 0 {
            let owners: Vec<String> = s
                .nodes
                .iter()
                .map(|n| {
                    let o = match n.owner {
                        Owner::A => "A",
                        Owner::B => "B",
                        Owner::Neutral => ".",
                    };
                    format!("{}{:.0}", o, n.garrison)
                })
                .collect();
            println!(
                "t={:>4} nodes=[{}] fleets={} uA={:.0} uB={:.0} sc={:+.3}",
                t,
                owners.join(" "),
                s.fleets.len(),
                s.total_units(Owner::A),
                s.total_units(Owner::B),
                s.score_from(Owner::A),
            );
        }
    }
    println!("final score_a = {:+.3}", s.score_from(Owner::A));
}

fn p(r: f64, k: f64, l: f64) -> Params {
    Params { r, k, l, undock_delay: 3.0, combat_substeps: 8, combat_rate: 0.5 }
}

fn main() {
    let maps = all_maps();
    let horizon = 600u64;

    // If invoked with `mirror`, trace a Colonize self-play on a chosen map.
    if std::env::args().any(|a| a == "mirror") {
        let params = p(1.0, 1.5, 0.15);
        let mi = std::env::args()
            .find_map(|a| a.strip_prefix("m=").and_then(|n| n.parse::<usize>().ok()))
            .unwrap_or(2);
        println!("MIRROR Colonize self-play on {}", maps[mi].name);
        trace(&maps[mi].state, Archetype::Colonize, Archetype::Colonize, &params, 80, 4);
        return;
    }

    // If invoked with `trace`, trace the three matchups on a chosen map and return.
    if std::env::args().any(|a| a == "trace") {
        let params = p(1.0, 1.8, 0.2);
        // pick map index from arg "m=N", default 0
        let mi = std::env::args()
            .find_map(|a| a.strip_prefix("m=").and_then(|n| n.parse::<usize>().ok()))
            .unwrap_or(0);
        println!("params r=1.0 k=1.8 l=0.2 on map {}", maps[mi].name);
        trace(&maps[mi].state, Archetype::Colonize, Archetype::Defend, &params, 250, 20);
        trace(&maps[mi].state, Archetype::Attack, Archetype::Colonize, &params, 250, 20);
        trace(&maps[mi].state, Archetype::Defend, Archetype::Attack, &params, 250, 20);
        return;
    }

    let rs = [0.6, 1.0, 1.5];
    let ks = [1.0, 1.5, 2.0, 2.5];
    let ls = [0.0, 0.15, 0.3];

    for m in &maps {
        println!("\n=== MAP {} ===", m.name);
        println!("{:>5} {:>5} {:>5} | {:>8} {:>8} {:>8} | min", "r", "k", "l", "a>c", "c>d", "d>a");
        for &r in &rs {
            for &k in &ks {
                for &l in &ls {
                    let params = p(r, k, l);
                    let c = cycle_on_map(&m.state, &params, horizon);
                    let mn = c.attack_beats_colonize.min(c.colonize_beats_defend).min(c.defend_beats_attack);
                    println!(
                        "{:>5.2} {:>5.2} {:>5.2} | {:>8.3} {:>8.3} {:>8.3} | {:>6.3}{}",
                        r, k, l,
                        c.attack_beats_colonize, c.colonize_beats_defend, c.defend_beats_attack, mn,
                        if mn > 0.05 { "  <== HOLDS" } else { "" },
                    );
                }
            }
        }
    }

    // Also dump the individual seating scores for one point to sanity-check.
    println!("\n--- detail on line5 at r=1.0 k=1.5 l=0.15 ---");
    let params = p(1.0, 1.5, 0.15);
    for (f, s) in [
        (Archetype::Attack, Archetype::Colonize),
        (Archetype::Colonize, Archetype::Defend),
        (Archetype::Defend, Archetype::Attack),
    ] {
        let d = duel(&maps[0].state, f, s, &params, params_horizon());
        println!(
            "{} vs {}: mean={:+.3}  (as A {:+.3}, as B {:+.3})",
            f.name(), s.name(), d.score, d.score_first_as_a, d.score_first_as_b
        );
    }
}

fn params_horizon() -> u64 { 600 }
