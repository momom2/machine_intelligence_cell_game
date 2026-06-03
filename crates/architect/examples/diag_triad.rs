//! Diagnostic: print the round-robin margins among the seed archetypes (genome form) to
//! see whether the rock-paper-scissors cycle holds, and compare to the reference
//! `cell_core::dsl::strategies`.
//!
//! Run: `cargo run -p architect --release --example diag_triad`

use architect::fitness::duel_over_maps;
use architect::genome::seed_archetypes;
use cell_core::maps::all_maps;
use cell_core::{GameState, Params};

fn params() -> Params {
    Params { r: 0.6, k: 2.25, l: 0.15, ..Params::default() }
}

fn main() {
    let maps: Vec<GameState> = all_maps().into_iter().map(|m| m.state).collect();
    let arches = seed_archetypes();
    let names = ["colonize", "defend", "attack"];

    println!("== genome seed-archetype round robin (row beats col, margin) ==");
    print!("{:>10}", "");
    for n in names {
        print!("{:>10}", n);
    }
    println!();
    for i in 0..3 {
        print!("{:>10}", names[i]);
        for j in 0..3 {
            if i == j {
                print!("{:>10}", "-");
                continue;
            }
            let gj = arches[j].clone();
            let s = duel_over_maps(&maps, &arches[i], &move || gj.make_player(), &params());
            print!("{:>+10.3}", s);
        }
        println!();
    }

    // The three cycle edges we expect: attack≻colonize, colonize≻defend, defend≻attack.
    let ac = {
        let g = arches[0].clone();
        duel_over_maps(&maps, &arches[2], &move || g.make_player(), &params())
    };
    let cd = {
        let g = arches[1].clone();
        duel_over_maps(&maps, &arches[0], &move || g.make_player(), &params())
    };
    let da = {
        let g = arches[2].clone();
        duel_over_maps(&maps, &arches[1], &move || g.make_player(), &params())
    };
    println!("\ncycle edges: attack>colonize {ac:+.3}  colonize>defend {cd:+.3}  defend>attack {da:+.3}");

    // Compare against the reference DSL strategies (genome should be close).
    println!("\n== reference cell_core::dsl::strategies, same edges ==");
    let ref_ac = ref_duel(&maps, dsl_attack, dsl_colonize);
    let ref_cd = ref_duel(&maps, dsl_colonize, dsl_defend);
    let ref_da = ref_duel(&maps, dsl_defend, dsl_attack);
    println!("cycle edges: attack>colonize {ref_ac:+.3}  colonize>defend {ref_cd:+.3}  defend>attack {ref_da:+.3}");

    // Cross: each genome archetype vs its reference twin.
    println!("\n== genome archetype vs its reference twin (should be ~competitive) ==");
    let col = cross(&maps, &arches[0], dsl_colonize);
    let def = cross(&maps, &arches[1], dsl_defend);
    let atk = cross(&maps, &arches[2], dsl_attack);
    println!("colonize vs ref {col:+.3}  defend vs ref {def:+.3}  attack vs ref {atk:+.3}");

    // Localize the broken defend>attack edge: cross genome-defend vs ref-attack, and
    // genome-attack vs ref-defend, and per-map genome defend>attack.
    println!("\n== localize defend>attack ==");
    let gdef_vs_ratk = cross(&maps, &arches[1], dsl_attack);
    let gatk_vs_rdef = cross(&maps, &arches[2], dsl_defend);
    println!("genome-defend vs REF-attack {gdef_vs_ratk:+.3}  (ref-defend vs ref-attack {ref_da:+.3})");
    println!("genome-attack vs REF-defend {gatk_vs_rdef:+.3}");
    println!("per-map genome defend>attack:");
    for m in &all_maps() {
        let ga = arches[2].clone();
        let s = duel_over_maps(std::slice::from_ref(&m.state), &arches[1], &move || ga.make_player(), &params());
        println!("  {:>13}: {:+.3}", m.name, s);
    }
}

fn ref_duel(
    maps: &[GameState],
    first: fn() -> Box<dyn cell_core::Policy>,
    second: fn() -> Box<dyn cell_core::Policy>,
) -> f64 {
    let n = maps.len() as f64;
    maps.iter()
        .map(|m| {
            let s1 = m.clone().run_match(first().as_mut(), second().as_mut(), &params(), 600).score_a;
            let s2 = -m.clone().run_match(second().as_mut(), first().as_mut(), &params(), 600).score_a;
            0.5 * (s1 + s2)
        })
        .sum::<f64>()
        / n
}

fn cross(
    maps: &[GameState],
    g: &architect::genome::Genome,
    twin: fn() -> Box<dyn cell_core::Policy>,
) -> f64 {
    duel_over_maps(maps, g, &move || twin(), &params())
}

fn dsl_colonize() -> Box<dyn cell_core::Policy> {
    Box::new(cell_core::DslPolicy::new(cell_core::dsl::strategies::colonize()))
}
fn dsl_defend() -> Box<dyn cell_core::Policy> {
    Box::new(cell_core::DslPolicy::new(cell_core::dsl::strategies::defend()))
}
fn dsl_attack() -> Box<dyn cell_core::Policy> {
    Box::new(cell_core::DslPolicy::new(cell_core::dsl::strategies::attack()))
}
