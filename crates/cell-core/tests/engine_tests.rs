//! Integration tests for the cell-core engine.
//!
//! The quality bar (design-doc build step 1) requires proving:
//!   (i)   the square-law property `2x ships => ~4x power` emerges,
//!   (ii)  determinism: identical inputs => bit-identical outputs,
//!   (iii) economy correctness.
//!
//! Plus sanity tests on movement (undock delay, commitment loss) and the policies.

use cell_core::engine::{lanchester_resolve, GameState, Params};
use cell_core::maps::{all_maps, corridor7};
use cell_core::policy::{Attack, Colonize, Defend, Policy};
use cell_core::types::*;

const EPS: f64 = 1e-9;

// ---------------------------------------------------------------------------
// (i) Square-law: 2x ships => ~4x power
// ---------------------------------------------------------------------------

/// The defining consequence of Lanchester's *square* law: a force's "power" scales
/// with the SQUARE of its size. Operationally, if force X fights an identical
/// reference force, the survivors scale linearly with X for small advantages, but
/// the *conserved quantity* `A^2 - k D^2` is quadratic. The cleanest legible test:
/// a force of size `2N` fighting a force of size `N` (k=1) should END with roughly
/// `sqrt((2N)^2 - N^2) = N*sqrt(3)` survivors — and the "effective power" measured
/// as survivors-squared of the winner equals the difference of squares.
#[test]
fn square_law_difference_of_squares() {
    // k = 1 (symmetric), fight to depletion.
    let n = 100.0;
    let (a, d) = lanchester_resolve(2.0 * n, n, 1.0, 1.0, f64::INFINITY, 64);
    // Defender annihilated.
    assert!(d < 1.0, "defender should be wiped out, got {d}");
    // Survivors ~ sqrt((2N)^2 - N^2) = N*sqrt(3) ≈ 173.2 for N=100.
    let expected = (4.0 * n * n - n * n).sqrt();
    let rel_err = (a - expected).abs() / expected;
    assert!(
        rel_err < 0.02,
        "square-law survivors {a} should be ~{expected} (rel err {rel_err})"
    );
}

/// Directly demonstrate "2x ships => ~4x power". Define a force's *power* against a
/// fixed reference garrison `R` as the survivors-squared it retains after wiping
/// that reference. Doubling the attacking force should ~quadruple that power.
#[test]
fn double_ships_quadruples_power() {
    let reference = 50.0;
    let k = 1.0;

    // Small attacker that still wins.
    let small = 120.0;
    let (a_small, d_small) = lanchester_resolve(small, reference, k, 1.0, f64::INFINITY, 128);
    assert!(d_small < 1.0);

    // Double the attacker.
    let (a_big, d_big) = lanchester_resolve(2.0 * small, reference, k, 1.0, f64::INFINITY, 128);
    assert!(d_big < 1.0);

    // "Power" = survivors^2 (the square-law conserved quantity vs the same R).
    // power_small = small^2 - R^2 ; power_big = (2 small)^2 - R^2.
    // ratio of powers -> ( (2s)^2 - R^2 ) / ( s^2 - R^2 ).
    let power_small = a_small * a_small;
    let power_big = a_big * a_big;
    let ratio = power_big / power_small;

    // Analytic ratio.
    let analytic = (4.0 * small * small - reference * reference)
        / (small * small - reference * reference);
    assert!(
        (ratio - analytic).abs() / analytic < 0.03,
        "power ratio {ratio} should match analytic {analytic}"
    );
    // And it should be close to 4x (it is slightly more than 4 here because the
    // reference is subtracted from both — for reference << small it -> exactly 4).
    let (a_small2, _) = lanchester_resolve(2000.0, reference, k, 1.0, f64::INFINITY, 256);
    let (a_big2, _) = lanchester_resolve(4000.0, reference, k, 1.0, f64::INFINITY, 256);
    let ratio_large = (a_big2 * a_big2) / (a_small2 * a_small2);
    assert!(
        (ratio_large - 4.0).abs() < 0.05,
        "with reference << force, doubling ships should ~4x power; got {ratio_large}"
    );
}

/// The defender-advantage `k` makes a defender ship worth `sqrt(k)` attacker ships.
/// At the balance point `A = sqrt(k) * D`, the fight should be ~a wash (both ~0).
#[test]
fn defender_advantage_balance_point() {
    let d = 100.0;
    let k = 2.25; // sqrt(k) = 1.5
    let a = 1.5 * d; // exactly the balance point
    let (al, dl) = lanchester_resolve(a, d, k, 1.0, f64::INFINITY, 256);
    assert!(al < 2.0 && dl < 2.0, "balance point should mutually annihilate, got a={al} d={dl}");

    // Slightly more attackers => attacker wins.
    let (al2, dl2) = lanchester_resolve(a * 1.1, d, k, 1.0, f64::INFINITY, 256);
    assert!(al2 > dl2 && dl2 < 1.0, "more attackers should win, got a={al2} d={dl2}");

    // Slightly fewer => defender wins.
    let (al3, dl3) = lanchester_resolve(a * 0.9, d, k, 1.0, f64::INFINITY, 256);
    assert!(dl3 > al3 && al3 < 1.0, "fewer attackers should lose, got a={al3} d={dl3}");
}

/// Combat is symmetric when forces are equal and k=1: mutual annihilation.
#[test]
fn equal_forces_mutual_annihilation() {
    let (a, d) = lanchester_resolve(80.0, 80.0, 1.0, 1.0, f64::INFINITY, 128);
    assert!(a < 1.0 && d < 1.0, "equal forces should annihilate, got a={a} d={d}");
}

// ---------------------------------------------------------------------------
// (ii) Determinism
// ---------------------------------------------------------------------------

/// Running the same matchup twice yields a bit-identical outcome. This is the
/// load-bearing property for the empirical R2/Gödel gate.
#[test]
fn determinism_same_inputs_same_outputs() {
    let params = Params::default();
    let base = corridor7().state;

    let run = || {
        let mut a: Box<dyn Policy> = Box::new(Attack);
        let mut b: Box<dyn Policy> = Box::new(Defend);
        base.clone().run_match(a.as_mut(), b.as_mut(), &params, 400)
    };
    let o1 = run();
    let o2 = run();
    assert_eq!(o1.winner, o2.winner);
    assert_eq!(o1.end_tick, o2.end_tick);
    assert_eq!(o1.by_elimination, o2.by_elimination);
    // Exact bit equality of the score.
    assert_eq!(o1.score_a.to_bits(), o2.score_a.to_bits(), "scores must be bit-identical");
}

/// Determinism across all maps and all matchups (broad sweep of the property).
#[test]
fn determinism_all_maps_all_matchups() {
    let params = Params::default();
    let archetypes: [fn() -> Box<dyn Policy>; 3] = [
        || Box::new(Colonize),
        || Box::new(Defend),
        || Box::new(Attack),
    ];
    for m in all_maps() {
        for a_make in archetypes.iter() {
            for b_make in archetypes.iter() {
                let mut a1 = a_make();
                let mut b1 = b_make();
                let mut a2 = a_make();
                let mut b2 = b_make();
                let o1 = m.state.clone().run_match(a1.as_mut(), b1.as_mut(), &params, 300);
                let o2 = m.state.clone().run_match(a2.as_mut(), b2.as_mut(), &params, 300);
                assert_eq!(
                    o1.score_a.to_bits(),
                    o2.score_a.to_bits(),
                    "non-determinism on map {} for a matchup",
                    m.name
                );
                assert_eq!(o1.end_tick, o2.end_tick);
            }
        }
    }
}

/// State evolution itself is deterministic tick-by-tick (no hidden RNG in step()).
#[test]
fn determinism_state_evolution() {
    let params = Params::default();
    let mut s1 = corridor7().state;
    let mut s2 = corridor7().state;
    // Issue identical commands and step identically.
    s1.launch_with(Owner::A, Command { source: 0, target: 1, fraction: FractionBucket::Half }, &params);
    s2.launch_with(Owner::A, Command { source: 0, target: 1, fraction: FractionBucket::Half }, &params);
    for _ in 0..50 {
        s1.step(&params);
        s2.step(&params);
    }
    assert_eq!(s1, s2, "identical command + step sequences must yield identical state");
}

// ---------------------------------------------------------------------------
// (iii) Economy correctness
// ---------------------------------------------------------------------------

/// A single owned node with no movement grows by exactly `r * production_mult` per
/// tick, and neutral nodes never grow.
#[test]
fn economy_linear_production() {
    let mut params = Params::default();
    params.r = 1.0;
    // Two nodes: A-owned mult 1.0, and a neutral node mult 1.0.
    let nodes = vec![
        Node { owner: Owner::A, garrison: 10.0, production_mult: 1.0 },
        Node { owner: Owner::Neutral, garrison: 0.0, production_mult: 1.0 },
    ];
    let edges = vec![Edge { a: 0, b: 1, length: 2.0 }];
    let mut s = GameState::new(nodes, edges);

    for t in 1..=10u64 {
        s.step(&params);
        // Owned node grew by r each tick.
        let expected = 10.0 + params.r * t as f64;
        assert!((s.nodes[0].garrison - expected).abs() < EPS, "owned grew wrong at t={t}: {}", s.nodes[0].garrison);
        // Neutral never produces.
        assert!((s.nodes[1].garrison - 0.0).abs() < EPS, "neutral produced at t={t}");
    }
}

/// Production scales with `production_mult` and with `r`.
#[test]
fn economy_production_scaling() {
    let nodes = vec![
        Node { owner: Owner::A, garrison: 0.0, production_mult: 2.0 },
    ];
    let edges = vec![];
    let mut s = GameState::new(nodes, edges);
    let mut params = Params::default();
    params.r = 0.5;
    s.step(&params);
    // 0 + r*mult = 0.5 * 2.0 = 1.0
    assert!((s.nodes[0].garrison - 1.0).abs() < EPS, "got {}", s.nodes[0].garrison);
}

/// Total unit accounting: production conserves the launched fleet (no ships
/// vanish in transit), and reinforcing a friendly node returns exactly the count.
#[test]
fn economy_units_conserved_in_transit_and_reinforce() {
    let mut params = Params::default();
    params.r = 0.0; // freeze production to isolate movement accounting
    params.undock_delay = 0.0;
    // Two A-owned nodes connected; send all from 0 to 1, expect 1 to gain exactly.
    let nodes = vec![
        Node { owner: Owner::A, garrison: 40.0, production_mult: 1.0 },
        Node { owner: Owner::A, garrison: 5.0, production_mult: 1.0 },
    ];
    let edges = vec![Edge { a: 0, b: 1, length: 3.0 }];
    let mut s = GameState::new(nodes, edges);
    let before = s.total_units(Owner::A);
    s.issue(Owner::A, Command { source: 0, target: 1, fraction: FractionBucket::All });
    // Step until the fleet arrives.
    for _ in 0..5 {
        s.step(&params);
    }
    assert!(s.fleets.is_empty(), "fleet should have arrived");
    let after = s.total_units(Owner::A);
    assert!((before - after).abs() < EPS, "units not conserved: before={before} after={after}");
    // Node 1 should now hold 45.
    assert!((s.nodes[1].garrison - 45.0).abs() < EPS, "reinforce wrong: {}", s.nodes[1].garrison);
}

/// Colonizing an empty node transfers ownership and the fleet's ships.
#[test]
fn economy_colonize_empty_node() {
    let mut params = Params::default();
    params.r = 0.0;
    params.undock_delay = 0.0;
    let nodes = vec![
        Node { owner: Owner::A, garrison: 20.0, production_mult: 1.0 },
        Node { owner: Owner::Neutral, garrison: 0.0, production_mult: 1.0 },
    ];
    let edges = vec![Edge { a: 0, b: 1, length: 2.0 }];
    let mut s = GameState::new(nodes, edges);
    s.issue(Owner::A, Command { source: 0, target: 1, fraction: FractionBucket::Half });
    for _ in 0..4 {
        s.step(&params);
    }
    assert_eq!(s.nodes[1].owner, Owner::A, "node should be colonized");
    assert!((s.nodes[1].garrison - 10.0).abs() < EPS, "colonized garrison wrong: {}", s.nodes[1].garrison);
    assert_eq!(s.territory(Owner::A), 2);
}

// ---------------------------------------------------------------------------
// Movement: undock delay & commitment loss
// ---------------------------------------------------------------------------

/// Undocking ships are not counted as garrison and take `undock_delay` extra ticks.
#[test]
fn movement_undock_delay_adds_latency() {
    let mut params = Params::default();
    params.r = 0.0;
    params.undock_delay = 5.0;
    let nodes = vec![
        Node { owner: Owner::A, garrison: 20.0, production_mult: 1.0 },
        Node { owner: Owner::Neutral, garrison: 0.0, production_mult: 1.0 },
    ];
    let edges = vec![Edge { a: 0, b: 1, length: 2.0 }];
    let mut s = GameState::new(nodes, edges);
    s.launch_with(Owner::A, Command { source: 0, target: 1, fraction: FractionBucket::All }, &params);
    // With undock 5 + length 2 = 7 ticks, the node must not be captured before then.
    for _ in 0..6 {
        s.step(&params);
        assert_eq!(s.nodes[1].owner, Owner::Neutral, "captured too early (tick {})", s.tick);
    }
    s.step(&params); // tick 7: should arrive
    assert_eq!(s.nodes[1].owner, Owner::A, "should have arrived by tick 7");
}

/// The attack-commitment loss `l` reduces the attacker's effective force on a
/// hostile arrival: a higher `l` leaves the defender stronger after the assault.
#[test]
fn movement_commitment_loss_weakens_attacker() {
    let make = |l: f64| {
        let mut params = Params::default();
        params.r = 0.0;
        params.undock_delay = 0.0;
        params.k = 1.0;
        params.l = l;
        let nodes = vec![
            Node { owner: Owner::A, garrison: 60.0, production_mult: 1.0 },
            Node { owner: Owner::B, garrison: 30.0, production_mult: 1.0 },
        ];
        let edges = vec![Edge { a: 0, b: 1, length: 1.0 }];
        let mut s = GameState::new(nodes, edges);
        s.issue(Owner::A, Command { source: 0, target: 1, fraction: FractionBucket::All });
        for _ in 0..3 {
            s.step(&params);
        }
        // Return A's surviving garrison on the captured node (or 0 if defender held).
        if s.nodes[1].owner == Owner::A { s.nodes[1].garrison } else { -s.nodes[1].garrison }
    };
    let low_loss = make(0.0);
    let high_loss = make(0.4);
    assert!(
        low_loss > high_loss,
        "higher commitment loss should leave the attacker worse off: low_l={low_loss} high_l={high_loss}"
    );
}

// ---------------------------------------------------------------------------
// Policy sanity
// ---------------------------------------------------------------------------

/// Each policy issues only legal commands (owns source, target adjacent).
#[test]
fn policies_emit_only_legal_commands() {
    let params = Params::default();
    let mut policies: Vec<Box<dyn Policy>> =
        vec![Box::new(Colonize), Box::new(Defend), Box::new(Attack)];
    for m in all_maps() {
        let mut s = m.state.clone();
        // Run a handful of ticks issuing commands from each policy as seat A.
        for _ in 0..30 {
            for p in policies.iter_mut() {
                let cmds = p.decide(&s, Owner::A, &params);
                for c in &cmds {
                    assert_eq!(s.nodes[c.source].owner, Owner::A, "{} issued from non-owned node", p.name());
                    assert!(
                        s.edge_between(c.source, c.target).is_some(),
                        "{} issued to non-adjacent target on map {}",
                        p.name(), m.name
                    );
                }
            }
            // Advance with Colonize driving A so the board changes over time.
            let mut driver: Box<dyn Policy> = Box::new(Colonize);
            let cmds = driver.decide(&s, Owner::A, &params);
            for c in cmds { s.launch_with(Owner::A, c, &params); }
            s.step(&params);
        }
    }
}

/// Colonize actually expands: against an idle opponent it should gain territory.
#[test]
fn colonize_expands_territory() {
    let params = Params::default();
    let base = corridor7().state;
    let mut col: Box<dyn Policy> = Box::new(Colonize);
    let mut idle: Box<dyn Policy> = Box::new(IdlePolicy);
    let out = base.run_match(col.as_mut(), idle.as_mut(), &params, 200);
    // Colonize (seat A) should be ahead.
    assert!(out.score_a > 0.0, "colonize should beat an idle opponent, score_a={}", out.score_a);
}

/// A do-nothing policy, for sanity baselines.
struct IdlePolicy;
impl Policy for IdlePolicy {
    fn name(&self) -> &'static str { "Idle" }
    fn decide(&mut self, _s: &GameState, _me: Owner, _p: &Params) -> Vec<Command> { Vec::new() }
}

// ---------------------------------------------------------------------------
// Map symmetry (so both-seatings truly cancels bias)
// ---------------------------------------------------------------------------

/// Both-seatings fairness — the canonical fairness mechanism the design mandates.
///
/// A single seating of self-play can carry a tiny seat bias because the map's
/// adjacency *iteration order* is not necessarily the mirror image of itself (so two
/// identical policies may break a tie toward different nodes at the self-mirror
/// center). The design's answer is to **play both seatings and average**, which
/// cancels that bias exactly. Here we assert that the averaged self-play score for
/// every archetype on every map is exactly zero — the property the sweep relies on.
#[test]
fn both_seatings_self_play_is_exactly_even() {
    use cell_core::harness::{duel, Archetype};
    let params = Params::default();
    for m in all_maps() {
        for arch in [Archetype::Colonize, Archetype::Defend, Archetype::Attack] {
            let d = duel(&m.state, arch, arch, &params, 400);
            assert!(
                d.score.abs() < 1e-9,
                "both-seatings self-play of {} on {} must be even, got {}",
                arch.name(), m.name, d.score
            );
        }
    }
}

/// Sanity: a *single* seating of self-play is at least close to even (the residual
/// adjacency-order bias is small). This guards against a gross seat asymmetry while
/// acknowledging the exact-zero guarantee belongs to the averaged result above.
#[test]
fn single_seating_self_play_is_near_even() {
    let params = Params::default();
    for m in all_maps() {
        let mut a: Box<dyn Policy> = Box::new(Colonize);
        let mut b: Box<dyn Policy> = Box::new(Colonize);
        let out = m.state.clone().run_match(a.as_mut(), b.as_mut(), &params, 300);
        assert!(
            out.score_a.abs() < 0.2,
            "single-seating self-play on {} unexpectedly lopsided: {}",
            m.name, out.score_a
        );
    }
}

/// Structural symmetry: the declared `mirror` involution must (a) be an involution,
/// (b) swap the two seats' starting ownership/garrison/production, and (c) map the
/// edge set onto itself. This proves the maps are genuinely symmetric, which is what
/// makes "play both seatings and average" fully cancel positional bias.
#[test]
fn maps_mirror_involution_is_valid() {
    for m in all_maps() {
        let s = &m.state;
        let mir = &m.mirror;
        assert_eq!(mir.len(), s.nodes.len(), "mirror length mismatch on {}", m.name);

        // (a) involution: mirror[mirror[i]] == i.
        for i in 0..mir.len() {
            assert_eq!(mir[mir[i]], i, "mirror not an involution on {} at {i}", m.name);
        }

        // (b) seat swap: node i and its mirror have swapped owner, equal garrison
        // and production.
        for i in 0..s.nodes.len() {
            let j = mir[i];
            let ni = &s.nodes[i];
            let nj = &s.nodes[j];
            let swapped_owner = match ni.owner {
                Owner::A => Owner::B,
                Owner::B => Owner::A,
                Owner::Neutral => Owner::Neutral,
            };
            assert_eq!(nj.owner, swapped_owner, "owner not seat-swapped on {} node {i}", m.name);
            assert!((nj.garrison - ni.garrison).abs() < EPS, "garrison asym on {} node {i}", m.name);
            assert!((nj.production_mult - ni.production_mult).abs() < EPS, "mult asym on {} node {i}", m.name);
        }

        // (c) edge set maps onto itself: for every edge (a,b,len) there is an edge
        // (mir[a],mir[b]) with the same length.
        let has_edge = |x: NodeId, y: NodeId, len: f64| -> bool {
            s.edges
                .iter()
                .any(|e| e.touches(x) && e.touches(y) && (e.length - len).abs() < EPS)
        };
        for e in &s.edges {
            assert!(
                has_edge(mir[e.a], mir[e.b], e.length),
                "edge ({},{}) has no mirror on {}",
                e.a, e.b, m.name
            );
        }
    }
}
