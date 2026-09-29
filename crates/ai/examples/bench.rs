//! Round-robin of the roster over the arena: `cargo run -p ai --release --example bench`.
use ai::arena::*;
use ai::{Genome, Roster, SimpleVersion};

fn main() {
    let field: Vec<(&str, Contender)> = vec![
        ("greedy", Contender::Roster(Roster::GreedyLocal)),
        ("simple", Contender::Roster(Roster::SimpleColonize { version: SimpleVersion::V1 })),
        ("cycler", Contender::Roster(Roster::Cycler)),
        ("opportunist", Contender::Roster(Roster::Opportunist)),
        ("evo-neutral", Contender::Genome(Genome::NEUTRAL)),
    ];
    let mut field = field;
    for n in 0..ai::lineages::LINEAGES.len() {
        field.push((Box::leak(format!("lineage{n}").into_boxed_str()), Contender::Genome(ai::lineages::genome(n).unwrap())));
    }
    print!("{:>12}", "");
    for (n, _) in &field { print!("{:>12}", n); }
    println!();
    for (na, a) in &field {
        print!("{:>12}", na);
        for (_, b) in &field {
            let t = std::time::Instant::now();
            let s = duel_score(a, b, 1, 2400);
            print!("{:>12.2}", s);
            eprint!(" [{:.1}s]", t.elapsed().as_secs_f32());
        }
        println!();
    }
}
