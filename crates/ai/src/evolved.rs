//! The **Evolved** brain — a parameterised tactical policy whose dials are the **genome** the
//! `evolve` tool breeds (`crates/ai/examples/evolve.rs`). The shipped opponents of the late
//! campaign are frozen champions of that process ([`crate::lineages`]): each *lineage* was bred
//! against the roster and against every earlier lineage, so each is measurably better at the
//! headless game than the last — the arms race the campaign narrates.
//!
//! One decision tick:
//!
//! 1. **Observe** — per sub: my idle ships, my ships already inbound, the foes present or inbound;
//!    plus the opponents' *commitment* (share of their fleet in flight, smoothed).
//! 2. **Defend** — an owned sub whose defenders are outnumbered by what stands on / heads for it
//!    pulls spare ships from the others (gene `defend_pull`).
//! 3. **Attack** — every non-owned sub within reach is scored (production, special kind, distance,
//!    weakness, neutral-ness); best first, each is sized to `ratio · defenders + min_force +
//!    grind · resistance`, minus what is already inbound, and funded from spare ships nearest-first
//!    (from one source, or several if the `concentrate` gene allows). At most `max_ops` new
//!    operations per decision.
//! 4. **Consolidate** — idle ships above a sub's capacity (which would bleed away) move to the
//!    owned sub nearest the frontier.
//!
//! Pure function of the observed interior plus its own small state: no RNG, replays bit-identical.

use layer1::{Faction, Interior, SimParams, SubKind};

/// Number of genes.
pub const GENES: usize = 15;

/// A genome: every gene in `[0, 1]`, mapped to a real dial range by [`Dials::from_genome`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Genome(pub [f32; GENES]);

impl Genome {
    /// The all-0.5 genome: every dial mid-range.
    pub const NEUTRAL: Genome = Genome([0.5; GENES]);

    /// Clamp every gene into `[0, 1]`.
    pub fn clamped(mut self) -> Genome {
        for g in &mut self.0 {
            *g = g.clamp(0.0, 1.0);
        }
        self
    }
}

/// Gene names, in genome order (for reports).
pub const GENE_NAMES: [&str; GENES] = [
    "floor_frac", "ratio", "min_force", "grind", "w_prod", "w_dist", "w_def", "w_neutral",
    "w_special", "reach", "max_ops", "defend_pull", "concentrate", "read", "overflow",
];

/// The real-valued dials a genome decodes to.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Dials {
    /// Fraction of a sub's capacity kept at home.
    pub floor_frac: f32,
    /// Ships demanded per defender.
    pub ratio: f32,
    /// Flat ships added to every operation.
    pub min_force: f32,
    /// Extra ships per point of remaining resistance / 60.
    pub grind: f32,
    /// Score weight of the target's production.
    pub w_prod: f32,
    /// Score penalty per 50 world units of distance.
    pub w_dist: f32,
    /// Score penalty per defender relative to the force needed.
    pub w_def: f32,
    /// Score bonus for unowned (neutral) targets.
    pub w_neutral: f32,
    /// Score bonus for shipyards (and penalty for fortresses).
    pub w_special: f32,
    /// Maximum source-to-target distance.
    pub reach: f32,
    /// New operations per decision.
    pub max_ops: usize,
    /// Willingness to pull ships to a threatened sub (`> 0.35` enables it).
    pub defend_pull: f32,
    /// Whether an operation may draw on several sources.
    pub concentrate: bool,
    /// How much the opponents' commitment lowers `ratio`.
    pub read: f32,
    /// Fraction of capacity that idle stock may exceed before it is consolidated.
    pub overflow: f32,
}

fn lerp(g: f32, lo: f32, hi: f32) -> f32 {
    lo + g.clamp(0.0, 1.0) * (hi - lo)
}

impl Dials {
    pub fn from_genome(g: &Genome) -> Dials {
        let v = &g.0;
        Dials {
            floor_frac: lerp(v[0], 0.0, 0.6),
            ratio: lerp(v[1], 0.6, 3.0),
            min_force: lerp(v[2], 0.0, 20.0),
            grind: lerp(v[3], 0.0, 0.6),
            w_prod: lerp(v[4], 0.0, 3.0),
            w_dist: lerp(v[5], 0.0, 3.0),
            w_def: lerp(v[6], 0.0, 3.0),
            w_neutral: lerp(v[7], -1.0, 2.0),
            w_special: lerp(v[8], -2.0, 3.0),
            reach: lerp(v[9], 40.0, 220.0),
            max_ops: 1 + (v[10].clamp(0.0, 0.999) * 4.0) as usize,
            defend_pull: v[11],
            concentrate: v[12] > 0.5,
            read: v[13],
            overflow: lerp(v[14], 0.0, 1.0),
        }
    }
}

/// Weight of the newest observation in the commitment average.
const COMMIT_ALPHA: f32 = 0.15;

/// The stateful Evolved driver; one per enemy seat.
#[derive(Debug, Clone)]
pub struct EvolvedController {
    pub seat: Faction,
    dials: Dials,
    commitment: f32,
}

struct View {
    n: usize,
    idle: Vec<usize>,
    inbound: Vec<usize>,
    foe_at: Vec<usize>,
    foe_ships: usize,
    foe_flying: usize,
}

impl View {
    fn of(st: &Interior, me: Faction) -> View {
        let n = st.subs.len();
        let mut v = View {
            n,
            idle: vec![0; n],
            inbound: vec![0; n],
            foe_at: vec![0; n],
            foe_ships: 0,
            foe_flying: 0,
        };
        for sh in st.ships.iter().filter(|s| s.alive && s.drift_remaining == 0) {
            if sh.faction == me {
                match sh.target {
                    None if sh.home < n => v.idle[sh.home] += 1,
                    Some(t) if t < n => v.inbound[t] += 1,
                    _ => {}
                }
            } else if sh.faction.is_foe_of(me) {
                v.foe_ships += 1;
                if sh.target.is_some() {
                    v.foe_flying += 1;
                }
                let at = sh.target.unwrap_or(sh.home);
                if at < n {
                    v.foe_at[at] += 1;
                }
            }
        }
        v
    }
}

impl EvolvedController {
    pub fn new(seat: Faction, genome: &Genome) -> EvolvedController {
        EvolvedController { seat, dials: Dials::from_genome(genome), commitment: 0.0 }
    }

    /// Run one decision tick, applying the orders. Returns the ships moved.
    pub fn decide_and_apply(&mut self, st: &mut Interior, _sp: &SimParams) -> usize {
        let me = self.seat;
        let view = View::of(st, me);
        if view.foe_ships > 0 {
            let seen = view.foe_flying as f32 / view.foe_ships as f32;
            self.commitment += COMMIT_ALPHA * (seen - self.commitment);
        }
        let d = self.dials;
        let owned: Vec<usize> = (0..view.n).filter(|&s| st.subs[s].owner == me).collect();
        if owned.is_empty() {
            return 0;
        }
        let mut spare: Vec<usize> = (0..view.n)
            .map(|s| {
                if st.subs[s].owner != me {
                    return 0;
                }
                if view.foe_at[s] > 0 {
                    return 0; // a threatened sub keeps everything
                }
                let floor = (d.floor_frac * st.subs[s].storage_capacity as f32).ceil() as usize;
                view.idle[s].saturating_sub(floor)
            })
            .collect();
        let pos: Vec<layer1::Vec2> = st.subs.iter().map(|s| s.pos).collect();
        let dist = |a: usize, b: usize| pos[a].dist(pos[b]);
        let mut moved = 0usize;

        // --- Defend. ---
        if d.defend_pull > 0.35 {
            for &s in &owned {
                let have = view.idle[s] + view.inbound[s];
                if view.foe_at[s] > have {
                    let mut deficit = ((view.foe_at[s] - have) as f32 * 1.3).ceil() as usize;
                    let mut donors: Vec<usize> =
                        owned.iter().copied().filter(|&o| o != s && spare[o] > 0).collect();
                    donors.sort_by(|&a, &b| dist(a, s).partial_cmp(&dist(b, s)).unwrap().then(a.cmp(&b)));
                    for o in donors {
                        if deficit == 0 {
                            break;
                        }
                        let n = spare[o].min(deficit);
                        moved += st.issue_order_count(o, s, n, me);
                        spare[o] -= n;
                        deficit -= n;
                    }
                }
            }
        }

        // --- Attack. ---
        let ratio = (d.ratio - d.read * 0.6 * self.commitment).max(0.4);
        let mut targets: Vec<(f32, usize, usize)> = Vec::new(); // (score, target, need)
        for t in 0..view.n {
            let sub = &st.subs[t];
            if sub.owner == me {
                continue;
            }
            let defenders = view.foe_at[t] as f32;
            let (res, _) = st.sub_resistance(t);
            let need = (ratio * defenders + d.min_force + d.grind * res / 60.0).ceil() as usize;
            let need = need.saturating_sub(view.inbound[t]);
            if need == 0 {
                continue;
            }
            let nearest = owned.iter().map(|&o| dist(o, t)).fold(f32::MAX, f32::min);
            if nearest > d.reach {
                continue;
            }
            let special = match sub.kind {
                SubKind::Shipyard { .. } => d.w_special,
                SubKind::Fortress => -d.w_special.abs().max(0.5),
                _ => 0.0,
            };
            let score = d.w_prod * sub.production as f32 + special
                + if sub.owner == Faction::Neutral { d.w_neutral } else { 0.0 }
                - d.w_dist * nearest / 50.0
                - d.w_def * defenders / need.max(1) as f32;
            targets.push((score, t, need));
        }
        targets.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap().then(a.1.cmp(&b.1)));
        let mut ops = 0usize;
        let mut attacked_from = vec![false; view.n];
        for (_, t, need) in targets {
            if ops >= d.max_ops {
                break;
            }
            let mut sources: Vec<usize> = owned
                .iter()
                .copied()
                .filter(|&o| spare[o] > 0 && dist(o, t) <= d.reach)
                .collect();
            sources.sort_by(|&a, &b| dist(a, t).partial_cmp(&dist(b, t)).unwrap().then(a.cmp(&b)));
            let plan: Vec<(usize, usize)> = if d.concentrate {
                let mut left = need;
                let mut p = Vec::new();
                for o in sources {
                    let n = spare[o].min(left);
                    p.push((o, n));
                    left -= n;
                    if left == 0 {
                        break;
                    }
                }
                if left > 0 { Vec::new() } else { p }
            } else {
                sources
                    .into_iter()
                    .find(|&o| spare[o] >= need)
                    .map(|o| vec![(o, need)])
                    .unwrap_or_default()
            };
            if plan.is_empty() {
                continue;
            }
            for (o, n) in plan {
                let sent = st.issue_order_count(o, t, n, me);
                moved += sent;
                spare[o] -= n.min(spare[o]);
                attacked_from[o] = true;
            }
            ops += 1;
        }

        // --- Consolidate the overflow that would bleed away. ---
        let front = owned.iter().copied().min_by(|&a, &b| {
            let fa = self.frontier_distance(st, a);
            let fb = self.frontier_distance(st, b);
            fa.partial_cmp(&fb).unwrap().then(a.cmp(&b))
        });
        if let Some(front) = front {
            for &s in &owned {
                if s == front || attacked_from[s] || view.foe_at[s] > 0 {
                    continue;
                }
                let cap = st.subs[s].storage_capacity as f32;
                let over = view.idle[s] as f32 - cap * (1.0 + d.overflow);
                if over >= 1.0 {
                    moved += st.issue_order_count(s, front, over as usize, me);
                }
            }
        }
        moved
    }

    /// Distance from `sub` to the nearest sub this seat does not own.
    fn frontier_distance(&self, st: &Interior, sub: usize) -> f32 {
        st.subs
            .iter()
            .filter(|o| o.owner != self.seat)
            .map(|o| o.pos.dist(st.subs[sub].pos))
            .fold(f32::MAX, f32::min)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use layer1::{SubStructure, Vec2};

    fn ctl(genome: Genome) -> EvolvedController {
        EvolvedController::new(Faction::Ai(0), &genome)
    }

    fn duo() -> (Interior, usize, usize) {
        let mut st = Interior::new(1);
        let home = st.add_sub(SubStructure::new(Vec2::new(0.0, 0.0), 4.0, Faction::Ai(0)));
        let post = st.add_sub(SubStructure::new(Vec2::new(30.0, 0.0), 4.0, Faction::Neutral));
        for _ in 0..50 {
            st.spawn_ship(Faction::Ai(0), home);
        }
        (st, home, post)
    }

    #[test]
    fn genome_decodes_within_documented_ranges() {
        for g in [Genome([0.0; GENES]), Genome([1.0; GENES]), Genome::NEUTRAL] {
            let d = Dials::from_genome(&g);
            assert!((1..=4).contains(&d.max_ops));
            assert!(d.reach >= 40.0 && d.reach <= 220.0);
            assert!(d.ratio >= 0.6 && d.ratio <= 3.0);
        }
    }

    #[test]
    fn expands_to_a_neutral_post_in_reach() {
        let (mut st, _, post) = duo();
        let moved = ctl(Genome::NEUTRAL).decide_and_apply(&mut st, &SimParams::default());
        assert!(moved > 0);
        assert!(st.ships.iter().any(|s| s.target == Some(post)));
    }

    #[test]
    fn the_reach_gene_limits_where_it_will_go() {
        let (mut st, _, _) = duo();
        let mut g = Genome::NEUTRAL;
        g.0[9] = 0.0; // reach 40; make the post 100 away
        st.subs[1].pos = Vec2::new(100.0, 0.0);
        assert_eq!(ctl(g).decide_and_apply(&mut st, &SimParams::default()), 0);
    }

    #[test]
    fn does_not_double_send_to_a_target_already_covered() {
        let (mut st, _, post) = duo();
        let mut c = ctl(Genome::NEUTRAL);
        c.decide_and_apply(&mut st, &SimParams::default());
        let first = st.ships.iter().filter(|s| s.target == Some(post)).count();
        c.decide_and_apply(&mut st, &SimParams::default());
        let second = st.ships.iter().filter(|s| s.target == Some(post)).count();
        assert_eq!(first, second, "the inbound force already covers the need");
    }

    #[test]
    fn a_threatened_sub_is_reinforced_when_defend_pull_is_on() {
        let mut st = Interior::new(1);
        let a = st.add_sub(SubStructure::new(Vec2::new(0.0, 0.0), 4.0, Faction::Ai(0)));
        let b = st.add_sub(SubStructure::new(Vec2::new(25.0, 0.0), 4.0, Faction::Ai(0)));
        let far = st.add_sub(SubStructure::new(Vec2::new(60.0, 0.0), 4.0, Faction::Player));
        for _ in 0..40 {
            st.spawn_ship(Faction::Ai(0), a);
        }
        for _ in 0..3 {
            st.spawn_ship(Faction::Ai(0), b);
        }
        for _ in 0..25 {
            st.spawn_ship(Faction::Player, far);
        }
        st.issue_order_count(far, b, 25, Faction::Player);
        let mut g = Genome::NEUTRAL;
        g.0[11] = 1.0;
        ctl(g).decide_and_apply(&mut st, &SimParams::default());
        let to_b = st.ships.iter().filter(|s| s.faction == Faction::Ai(0) && s.target == Some(b)).count();
        assert!(to_b >= 20, "expected reinforcement of the threatened sub, got {to_b}");
    }

    #[test]
    fn deterministic() {
        let run = || {
            let (mut st, ..) = duo();
            let mut c = ctl(Genome::NEUTRAL);
            for _ in 0..30 {
                c.decide_and_apply(&mut st, &SimParams::default());
                st.step(&SimParams::default());
            }
            st.state_hash()
        };
        assert_eq!(run(), run());
    }
}
