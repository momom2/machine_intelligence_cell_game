//! Breed the **Evolved** brain's lineages.
//!
//! ```sh
//! cargo run -p ai --release --example evolve -- --stages 4 --gens 24 --pop 32
//! ```
//!
//! Each *stage* runs a (mu + lambda) evolution strategy over the 15-gene [`Genome`]. Fitness is a
//! genome's mean arena score against a field — the hand-written roster plus every champion of the
//! earlier stages — blended with its worst matchup (`0.5·mean + 0.5·min`) so a lineage cannot win
//! by beating three opponents and folding to the fourth. The stage's champion becomes the next
//! lineage; the next stage starts from it and must beat it too. The lineage table is rewritten to
//! `crates/ai/src/lineages.rs`, and a generation log to `docs/evolution.md`.

use ai::arena::{duel_score, Contender};
use ai::evolved::{GENES, GENE_NAMES};
use ai::{Genome, Roster, SimpleVersion};
use layer1::Rng;
use std::io::Write;

/// The top of `docs/evolution.md`.
const HEADER: &str = "# Evolution

The last arc's opponents are *bred*, not written. `crates/ai/examples/evolve.rs` runs a (mu + lambda)
evolution strategy over the 15 dials of `ai::evolved::Genome` (see `GENE_NAMES`):

- **Fitness** of a genome = `0.5 * mean + 0.5 * min` of its arena score against every opponent in the
  stage's field (hand-written roster + every earlier champion). The `min` term stops a lineage from
  winning by beating most of the field and folding to one opponent.
- **Arena**: five symmetric boards (`ai::arena::BOARDS`), both seatings, the game's combat dials.
  A win scores 1.0 plus up to 0.2 for speed; otherwise up to 0.45 for the ship and sub lead at the horizon.
- **Stages**: each stage's champion becomes the next lineage and joins the field of the following stage.
  Lineage `n` is therefore bred against lineages `0..n` and the roster, and never saw lineages after it.
- The champions are frozen in `crates/ai/src/lineages.rs` (generated); a mission pins one with
  `enemy = evolved N`. A test checks each lineage beats its predecessor.

Regenerate (about three hours on 4 cores at the default 5 stages x 20 generations x 32 genomes):

```sh
cargo run -p ai --release --example evolve -- --stages 5 --gens 20 --pop 32
cargo run -p ai --release --example bench      # round-robin table of the roster and every lineage
```

Regeneration changes every lineage that follows a changed stage; missions balanced against the old
table need re-measuring (`cargo run -p levels --release --example measure`).

";

struct Args {
    stages: usize,
    gens: usize,
    pop: usize,
    horizon: u64,
    seed: u64,
    out: String,
    log: String,
    /// Start from the shipped lineages: they join the field and the new stages append to the table.
    keep: bool,
    /// Weight of the novelty bonus: distance (mean absolute gene difference, capped at 0.25 and
    /// scaled to 0..1) from the nearest existing champion, so a stage is pushed into a new niche.
    novelty: f32,
}

fn parse_args() -> Args {
    let mut a = Args {
        stages: 4,
        gens: 24,
        pop: 32,
        horizon: 2400,
        seed: 1,
        out: "crates/ai/src/lineages.rs".into(),
        log: "docs/evolution.md".into(),
        keep: false,
        novelty: 0.0,
    };
    let v: Vec<String> = std::env::args().skip(1).collect();
    let mut i = 0;
    while i < v.len() {
        if v[i] == "--keep" {
            a.keep = true;
            i += 1;
            continue;
        }
        if i + 1 >= v.len() {
            break;
        }
        match v[i].as_str() {
            "--novelty" => a.novelty = v[i + 1].parse().unwrap(),
            "--stages" => a.stages = v[i + 1].parse().unwrap(),
            "--gens" => a.gens = v[i + 1].parse().unwrap(),
            "--pop" => a.pop = v[i + 1].parse().unwrap(),
            "--horizon" => a.horizon = v[i + 1].parse().unwrap(),
            "--seed" => a.seed = v[i + 1].parse().unwrap(),
            "--out" => a.out = v[i + 1].clone(),
            "--log" => a.log = v[i + 1].clone(),
            _ => {}
        }
        i += 2;
    }
    a
}

fn gauss(rng: &mut Rng) -> f32 {
    (0..4).map(|_| rng.next_f32()).sum::<f32>() - 2.0
}

fn random_genome(rng: &mut Rng) -> Genome {
    let mut g = [0.0; GENES];
    for x in &mut g {
        *x = rng.next_f32();
    }
    Genome(g)
}

fn mutate(parent: &Genome, sigma: f32, rng: &mut Rng) -> Genome {
    let mut g = parent.0;
    for x in &mut g {
        if rng.next_f32() < 0.4 {
            *x += sigma * gauss(rng);
        }
    }
    Genome(g).clamped()
}

fn crossover(a: &Genome, b: &Genome, rng: &mut Rng) -> Genome {
    let mut g = a.0;
    for i in 0..GENES {
        if rng.next_f32() < 0.5 {
            g[i] = b.0[i];
        }
    }
    Genome(g)
}

/// Distance in `[0, 1]` from `g` to the nearest of `others`: mean absolute gene difference, so 0.25
/// (a quarter of the range on average) already counts as fully novel.
fn novelty_of(g: &Genome, others: &[Genome]) -> f32 {
    others
        .iter()
        .map(|o| g.0.iter().zip(o.0.iter()).map(|(a, b)| (a - b).abs()).sum::<f32>() / GENES as f32)
        .fold(f32::MAX, f32::min)
        .min(0.25)
        / 0.25
}

/// `(fitness, mean, min)` of `g` against `field` on `seed`.
fn evaluate(g: &Genome, field: &[Contender], seed: u64, horizon: u64) -> (f32, f32, f32) {
    let scores: Vec<f32> =
        field.iter().map(|o| duel_score(&Contender::Genome(*g), o, seed, horizon)).collect();
    let mean = scores.iter().sum::<f32>() / scores.len() as f32;
    let min = scores.iter().cloned().fold(f32::MAX, f32::min);
    (0.5 * mean + 0.5 * min, mean, min)
}

fn evaluate_all(pop: &[Genome], field: &[Contender], seed: u64, horizon: u64) -> Vec<(f32, f32, f32)> {
    let threads = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4);
    let chunk = (pop.len() + threads - 1) / threads;
    let mut out = vec![(0.0, 0.0, 0.0); pop.len()];
    std::thread::scope(|s| {
        for (pc, oc) in pop.chunks(chunk).zip(out.chunks_mut(chunk)) {
            s.spawn(move || {
                for (g, o) in pc.iter().zip(oc.iter_mut()) {
                    *o = evaluate(g, field, seed, horizon);
                }
            });
        }
    });
    out
}

fn write_lineages(path: &str, champions: &[(u32, Genome)]) {
    let mut s = String::from(
        "//! The shipped **lineages** of the Evolved brain: frozen champions bred by\n\
         //! `cargo run -p ai --release --example evolve`. A mission pins a lineage by number\n\
         //! (`enemy = evolved 2`) and it never changes — re-running the tool writes new numbers.\n\
         //!\n\
         //! GENERATED by `crates/ai/examples/evolve.rs` — do not edit by hand.\n\n\
         use crate::evolved::{Genome, GENES};\n\n\
         /// `(generations bred, genome)` per lineage, oldest first.\n\
         pub const LINEAGES: &[(u32, [f32; GENES])] = &[\n",
    );
    for (gens, g) in champions {
        s.push_str(&format!("    ({gens}, [{}]),\n", g.0.iter().map(|x| format!("{x:.4}")).collect::<Vec<_>>().join(", ")));
    }
    s.push_str(
        "];\n\n/// The genome of lineage `n`, if it exists.\npub fn genome(n: usize) -> Option<Genome> {\n    LINEAGES.get(n).map(|&(_, g)| Genome(g))\n}\n\n\
         /// The generation count lineage `n` was bred for, if it exists.\npub fn generation(n: usize) -> Option<u32> {\n    LINEAGES.get(n).map(|&(g, _)| g)\n}\n",
    );
    std::fs::write(path, s).unwrap();
}

fn main() {
    let a = parse_args();
    let mut rng = Rng::new(a.seed);
    let base_field: Vec<Contender> = vec![
        Contender::Roster(Roster::GreedyLocal),
        Contender::Roster(Roster::SimpleColonize { version: SimpleVersion::V1 }),
        Contender::Roster(Roster::Cycler),
        Contender::Roster(Roster::Opportunist),
    ];
    let mut champions: Vec<(u32, Genome)> = Vec::new();
    let mut log = String::from(HEADER);
    let mut prev_champion: Option<Genome> = None;
    let mut total_gens = 0u32;
    if a.keep {
        champions = ai::lineages::LINEAGES.iter().map(|&(g, v)| (g, Genome(v))).collect();
        total_gens = champions.last().map_or(0, |c| c.0);
        if let Ok(old) = std::fs::read_to_string(&a.log) {
            log = old;
        }
    }

    for _ in 0..a.stages {
        let stage = champions.len();
        let known: Vec<Genome> = champions.iter().map(|c| c.1).collect();
        let mut field = base_field.clone();
        field.extend(champions.iter().map(|&(_, g)| Contender::Genome(g)));
        let mut pop: Vec<Genome> = (0..a.pop).map(|_| random_genome(&mut rng)).collect();
        pop[0] = Genome::NEUTRAL;
        if let (Some(c), false) = (prev_champion, a.keep) {
            pop[1] = c;
            for i in 2..a.pop / 2 {
                pop[i] = mutate(&c, 0.15, &mut rng);
            }
        }
        log.push_str(&format!(
            "\n## Lineage {stage}\n\nField: {} opponents.{}\n\n| gen | best | mean | worst matchup |\n|---|---|---|---|\n",
            field.len(),
            if a.novelty > 0.0 { format!(" Novelty bonus weight {}.", a.novelty) } else { String::new() }
        ));
        // The top genome of every generation: fitness is measured on a per-generation seed, so
        // the last generation's winner is not necessarily the best one - the champion is picked
        // from the recent tops on fresh seeds below.
        let mut tops: Vec<Genome> = Vec::new();
        for gen in 0..a.gens {
            let seed = 1000 * (stage as u64 + 1) + gen as u64;
            let mut fit = evaluate_all(&pop, &field, seed, a.horizon);
            for (f, g) in fit.iter_mut().zip(pop.iter()) {
                f.0 += a.novelty * novelty_of(g, &known);
            }
            let mut idx: Vec<usize> = (0..pop.len()).collect();
            idx.sort_by(|&x, &y| fit[y].0.partial_cmp(&fit[x].0).unwrap());
            let top = idx[0];
            let mean_pop = fit.iter().map(|f| f.0).sum::<f32>() / fit.len() as f32;
            println!("stage {stage} gen {gen:>3}: best {:.3} (mean {:.3}, min {:.3}) pop-mean {:.3}", fit[top].0, fit[top].1, fit[top].2, mean_pop);
            log.push_str(&format!("| {gen} | {:.3} | {:.3} | {:.3} |\n", fit[top].0, fit[top].1, fit[top].2));
            tops.push(pop[top]);
            // (mu + lambda): keep the top quarter, refill by crossover + mutation of the elite.
            let mu = (a.pop / 4).max(2);
            let elite: Vec<Genome> = idx[..mu].iter().map(|&i| pop[i]).collect();
            let sigma = 0.18 - 0.12 * (gen as f32 / a.gens.max(1) as f32);
            let mut next = elite.clone();
            while next.len() < a.pop {
                let p1 = elite[rng.below(mu)];
                let p2 = elite[rng.below(mu)];
                let child = mutate(&crossover(&p1, &p2, &mut rng), sigma, &mut rng);
                next.push(child);
            }
            pop = next;
            let _ = std::io::stdout().flush();
        }
        // Re-score the champion on fresh seeds so the logged number is not the selection's own.
        let held = |g: &Genome| -> f32 {
            (0..3).map(|k| evaluate(g, &field, 90_000 + k, a.horizon).0).sum::<f32>() / 3.0
                + a.novelty * novelty_of(g, &known)
        };
        let recent = &tops[tops.len().saturating_sub(5)..];
        let (champion, held_out) = recent
            .iter()
            .map(|g| (*g, held(g)))
            .fold((recent[0], f32::MIN), |b, c| if c.1 > b.1 { c } else { b });
        let best = (champion, held_out);
        log.push_str(&format!("\nChampion held-out fitness (3 fresh seeds): **{held_out:.3}**\n\nGenome:\n\n```\n"));
        for (n, v) in GENE_NAMES.iter().zip(best.0 .0.iter()) {
            log.push_str(&format!("{n:>12} = {v:.3}\n"));
        }
        log.push_str("```\n");
        println!("stage {stage}: champion held-out fitness {held_out:.3}");
        total_gens += a.gens as u32;
        champions.push((total_gens, best.0));
        prev_champion = Some(best.0);
        write_lineages(&a.out, &champions);
        std::fs::write(&a.log, &log).unwrap();
    }
}
