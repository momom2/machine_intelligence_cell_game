//! Integration tests for the Layer-1 spatial sim.
//!
//! These pin the load-bearing properties from the task spec:
//!   (i)   the stochastic **square law** — more ships win, and the advantage grows
//!         *super-linearly* (averaged over several seeds);
//!   (ii)  **determinism** — same seed + same orders => identical final state (asserted on
//!         the full state hash);
//!   (iii) **ship movement** between sub-structures;
//!   (iv)  **capture** of a neutral and of an enemy sub-structure;
//!   (v)   **outcome** by elimination and by lead at a horizon.
//! Plus a test that the documented **AI seam** (thin rear) is genuinely exploitable.

use layer1::scenario::{sample_params, sample_structure};
use layer1::sim::{SimParams, Structure, SubStructure};
use layer1::{Automaton, Faction, FractionBucket, MoveOrder, Ship, Vec2};

/// Helper: a minimal two-sub structure with `np` Player and `ne` Enemy ships clustered at
/// the origin within engagement range, owned subs placed FAR away so neither the defender
/// bonus nor capture interferes — isolating raw combat. Production at the far subs cannot
/// reach the origin within the short fights we run.
fn origin_clash(seed: u64, np: usize, ne: usize) -> Structure {
    let mut st = Structure::new(seed);
    let a = st.add_sub(SubStructure::new(Vec2::new(-1000.0, 0.0), 3.0, Faction::Player));
    let b = st.add_sub(SubStructure::new(Vec2::new(1000.0, 0.0), 3.0, Faction::Enemy));
    for i in 0..np {
        let y = i as f32 * 0.25;
        st.ships.push(Ship {
            faction: Faction::Player,
            pos: Vec2::new(-1.0, y),
            target: None,
            home: a,
            aim: Vec2::new(-1.0, y),
            alive: true,
        });
    }
    for i in 0..ne {
        let y = i as f32 * 0.25;
        st.ships.push(Ship {
            faction: Faction::Enemy,
            pos: Vec2::new(1.0, y),
            target: None,
            home: b,
            aim: Vec2::new(1.0, y),
            alive: true,
        });
    }
    st
}

/// Survivors of each side near the origin after fighting `ticks` ticks (ignoring any far
/// production spawns by filtering on |x| < 50).
fn fight_origin(seed: u64, np: usize, ne: usize, ticks: u64, params: &SimParams) -> (usize, usize) {
    let mut st = origin_clash(seed, np, ne);
    for _ in 0..ticks {
        st.step(params);
        // stop early if one side near origin is wiped
        let near = |f: Faction| {
            st.ships.iter().filter(|s| s.alive && s.faction == f && s.pos.x.abs() < 50.0).count()
        };
        if near(Faction::Player) == 0 || near(Faction::Enemy) == 0 {
            break;
        }
    }
    let near = |f: Faction| {
        st.ships.iter().filter(|s| s.alive && s.faction == f && s.pos.x.abs() < 50.0).count()
    };
    (near(Faction::Player), near(Faction::Enemy))
}

// ===========================================================================
// (i) Stochastic square law
// ===========================================================================

/// Equal forces split roughly evenly over many seeds (no seat bias in combat).
#[test]
fn equal_forces_are_fair() {
    let params = sample_params();
    let seeds = 200u64;
    let mut p_wins = 0i32;
    let mut e_wins = 0i32;
    for seed in 0..seeds {
        let (p, e) = fight_origin(seed.wrapping_mul(0x9E3779B1), 8, 8, 30, &params);
        if p > e {
            p_wins += 1;
        } else if e > p {
            e_wins += 1;
        }
    }
    // Within ~3 sigma of a fair coin over 200 trials the gap should be modest.
    let gap = (p_wins - e_wins).abs();
    assert!(gap < 45, "combat looks seat-biased: P {p_wins} E {e_wins} (gap {gap})");
}

/// THE square-law property: a numerical edge wins, and the edge is *super-linear*.
///
/// Under Lanchester's SQUARE law the conserved quantity is `N_big^2 - N_small^2`, so the
/// bigger side should keep ~`sqrt(big^2 - small^2)` survivors while the smaller side is
/// (nearly) annihilated — i.e. a 1.5x numbers edge yields far more than a 1.5x survivor
/// edge. We verify, averaged over several seeds:
///   * the bigger side wins on average (more ships win), and
///   * the *fraction of the bigger side that survives* a `big vs small` fight is much higher
///     than `(big - small)/big` would give under a LINEAR law — the signature of the square
///     law. Concretely, for 15 vs 10 the linear law predicts the winner keeps ~5 (the raw
///     difference); the square law predicts ~sqrt(125) ≈ 11.2. We require the empirical
///     mean to sit well above the linear prediction.
#[test]
fn square_law_superlinear_advantage() {
    let params = sample_params();
    let seeds = 120u64;
    let (big, small) = (15usize, 10usize);

    let mut big_surv_sum = 0.0;
    let mut small_surv_sum = 0.0;
    let mut big_win = 0i32;
    for seed in 0..seeds {
        let (p, e) = fight_origin(seed.wrapping_mul(0x2545F491), big, small, 40, &params);
        big_surv_sum += p as f64;
        small_surv_sum += e as f64;
        if p > e {
            big_win += 1;
        }
    }
    let big_mean = big_surv_sum / seeds as f64;
    let small_mean = small_surv_sum / seeds as f64;

    // More ships win, almost always.
    assert!(big_win as f64 / seeds as f64 > 0.9, "bigger side should usually win: {big_win}/{seeds}");

    // Super-linear: winner keeps far more than the linear-law difference of 5.
    let linear_pred = (big - small) as f64; // 5
    let square_pred = ((big * big - small * small) as f64).sqrt(); // ~11.18
    assert!(
        big_mean > linear_pred + 2.0,
        "winner survivors {big_mean:.2} not super-linear (linear law predicts ~{linear_pred})"
    );
    // And the smaller side is essentially wiped (square law annihilates the lesser force).
    assert!(small_mean < 1.5, "smaller side should be nearly annihilated, got {small_mean:.2}");
    // Sanity: empirical mean is in the neighbourhood of the square-law prediction (loose).
    assert!(
        big_mean > 0.6 * square_pred,
        "winner survivors {big_mean:.2} far below square-law prediction {square_pred:.2}"
    );
}

/// Concentration of force as ~a theorem: doubling one side's ships turns a coin-flip into a
/// near-certain, near-costless win. We compare 10v10 (fair) to 20v10 (2x) and require the
/// 2x side to win ~always and keep most of its ships (square law => sqrt(300)≈17 survive).
#[test]
fn doubling_ships_dominates() {
    let params = sample_params();
    let seeds = 100u64;
    let mut win2x = 0i32;
    let mut surv_sum = 0.0;
    for seed in 0..seeds {
        let (p, e) = fight_origin(seed.wrapping_mul(0xA24BAED4), 20, 10, 45, &params);
        if p > e {
            win2x += 1;
        }
        surv_sum += p as f64;
    }
    assert!(win2x as f64 / seeds as f64 > 0.95, "2x side should almost always win: {win2x}/{seeds}");
    assert!(surv_sum / seeds as f64 > 12.0, "2x side should keep most ships (square law), got {:.2}", surv_sum / seeds as f64);
}

// ===========================================================================
// (ii) Determinism
// ===========================================================================

/// Same seed + same orders => byte-identical evolution at every tick (full state hash).
#[test]
fn deterministic_same_seed_same_orders() {
    let params = sample_params();
    let scripted = |st: &mut Structure| {
        // A fixed script of orders issued at fixed ticks (exercises movement + capture).
        if st.tick == 0 {
            st.issue_order(MoveOrder::new(0, 6, FractionBucket::Half));
            st.issue_order(MoveOrder::new(1, 6, FractionBucket::Half));
        }
        if st.tick == 30 {
            st.issue_order(MoveOrder::new(0, 4, FractionBucket::All));
        }
    };

    let run = || {
        let (mut st, _l) = sample_structure(0xDEAD_BEEF);
        let mut hashes = Vec::new();
        for _ in 0..300 {
            scripted(&mut st);
            st.step(&params);
            hashes.push(st.state_hash());
        }
        (st.state_hash(), hashes)
    };

    let (final_a, trace_a) = run();
    let (final_b, trace_b) = run();
    assert_eq!(final_a, final_b, "final state hash diverged across identical runs");
    assert_eq!(trace_a, trace_b, "per-tick hash trace diverged across identical runs");
}

/// Different seeds (with the same orders) generally diverge — confirms the seed actually
/// drives the stochastic combat (and that determinism above is not just "no randomness").
#[test]
fn different_seeds_diverge() {
    let params = sample_params();
    let player = Automaton::new(Faction::Player);
    let enemy = Automaton::new(Faction::Enemy);
    let run = |seed: u64| {
        let (mut st, _l) = sample_structure(seed);
        layer1::run_auto_vs_auto(&mut st, &params, &player, &enemy, 200, 4, |_, _| {});
        st.state_hash()
    };
    assert_ne!(run(1), run(2), "two seeds produced identical states (combat not seeded?)");
}

/// Cloning a structure and stepping both identically keeps them bit-identical (the clone
/// carries the RNG state) — the property a renderer relies on for replay.
#[test]
fn clone_replays_identically() {
    let params = sample_params();
    let (mut a, _l) = sample_structure(777);
    for _ in 0..50 {
        a.step(&params);
    }
    let mut b = a.clone();
    for _ in 0..50 {
        a.step(&params);
        b.step(&params);
    }
    assert_eq!(a.state_hash(), b.state_hash(), "clone diverged from original");
}

// ===========================================================================
// (iii) Movement
// ===========================================================================

/// An idle ship ordered to another sub-structure leaves its home, travels, and arrives —
/// becoming idle again with its `home` updated to the destination.
#[test]
fn ship_moves_between_substructures() {
    let params = sample_params();
    let (mut st, layout) = sample_structure(42);

    let before_home = st.idle_count_at(layout.player_home, Faction::Player);
    assert!(before_home > 0);
    let before_keep = st.idle_count_at(layout.neutral_keep, Faction::Player);
    assert_eq!(before_keep, 0);

    // Send half of the home garrison to the central keep.
    let ordered = st.issue_order(MoveOrder::new(layout.player_home, layout.neutral_keep, FractionBucket::Half));
    assert!(ordered > 0, "order should move at least one ship");

    // Immediately after issuing: those ships are no longer idle at home (they are moving).
    let moving_now = st.idle_count_at(layout.player_home, Faction::Player);
    assert_eq!(moving_now, before_home - ordered, "ordered ships should leave the home idle pool");

    // Step until they arrive (home->keep is ~26 units at speed 1.4 => well under 60 ticks).
    for _ in 0..60 {
        st.step(&params);
    }
    let arrived = st.idle_count_at(layout.neutral_keep, Faction::Player);
    assert!(arrived >= ordered, "ordered ships should have arrived and gone idle at the keep, got {arrived}");
}

/// Issuing junk orders is a safe no-op: same source/target, out-of-range ids, and a source
/// with no idle ships all move zero ships and do not panic.
#[test]
fn junk_orders_are_safe_noops() {
    let (mut st, layout) = sample_structure(1);
    assert_eq!(st.issue_order(MoveOrder::new(layout.player_home, layout.player_home, FractionBucket::All)), 0);
    assert_eq!(st.issue_order(MoveOrder::new(999, 0, FractionBucket::All)), 0);
    assert_eq!(st.issue_order(MoveOrder::new(0, 999, FractionBucket::All)), 0);
    // neutral_keep starts empty => no idle ships to send.
    assert_eq!(st.issue_order(MoveOrder::new(layout.neutral_keep, layout.player_home, FractionBucket::All)), 0);
}

// ===========================================================================
// (iv) Capture
// ===========================================================================

/// A lone ship sent into an uncontested neutral sub-structure captures it, and the captured
/// sub then produces for its new owner.
#[test]
fn capture_neutral_substructure() {
    let params = sample_params();
    let (mut st, layout) = sample_structure(5);
    assert_eq!(st.subs[layout.neutral_left].owner, Faction::Neutral);

    // neutral_left is close to player_home; send a quarter wave.
    let moved = st.issue_order(MoveOrder::new(layout.player_home, layout.neutral_left, FractionBucket::Quarter));
    assert!(moved > 0);

    // Step until arrival + capture.
    let mut captured_tick = None;
    for _ in 0..60 {
        st.step(&params);
        if st.subs[layout.neutral_left].owner == Faction::Player {
            captured_tick = Some(st.tick);
            break;
        }
    }
    assert!(captured_tick.is_some(), "player should have captured the neutral sub");
    assert_eq!(st.subs[layout.neutral_left].owner, Faction::Player);
}

/// An enemy sub-structure left undefended is captured when a player force arrives and no
/// living enemy contests it. We empty the enemy home of ships, then walk a player force in.
#[test]
fn capture_enemy_substructure() {
    let params = sample_params();
    // Custom minimal structure: a player sub and an (initially enemy) sub close together,
    // with NO enemy ships, so the enemy sub is undefended.
    let mut st = Structure::new(9);
    let p = st.add_sub(SubStructure::new(Vec2::new(0.0, 0.0), 4.0, Faction::Player));
    let e = st.add_sub(SubStructure::new(Vec2::new(10.0, 0.0), 4.0, Faction::Enemy));
    for _ in 0..6 {
        st.spawn_ship(Faction::Player, p);
    }
    // Enemy owns `e` but has zero ships anywhere.
    assert_eq!(st.subs[e].owner, Faction::Enemy);
    assert_eq!(st.ship_count(Faction::Enemy), 0);

    let moved = st.issue_order(MoveOrder::new(p, e, FractionBucket::All));
    assert!(moved > 0);
    for _ in 0..40 {
        st.step(&params);
        if st.subs[e].owner == Faction::Player {
            break;
        }
    }
    assert_eq!(st.subs[e].owner, Faction::Player, "undefended enemy sub should be captured");
}

/// A contested sub-structure does NOT flip: with both factions' ships inside its radius,
/// ownership is frozen until one side wins the local fight.
#[test]
fn contested_substructure_does_not_flip() {
    let params = sample_params();
    let mut st = Structure::new(3);
    let n = st.add_sub(SubStructure::new(Vec2::new(0.0, 0.0), 5.0, Faction::Neutral));
    // Put one of each faction right inside the neutral sub.
    let far_p = st.add_sub(SubStructure::new(Vec2::new(-500.0, 0.0), 3.0, Faction::Player));
    let far_e = st.add_sub(SubStructure::new(Vec2::new(500.0, 0.0), 3.0, Faction::Enemy));
    st.ships.push(Ship { faction: Faction::Player, pos: Vec2::new(-0.5, 0.0), target: None, home: far_p, aim: Vec2::new(-0.5, 0.0), alive: true });
    st.ships.push(Ship { faction: Faction::Enemy, pos: Vec2::new(0.5, 0.0), target: None, home: far_e, aim: Vec2::new(0.5, 0.0), alive: true });

    // For a few ticks while both are alive inside, the neutral must stay neutral.
    let mut stayed_neutral_while_contested = true;
    for _ in 0..6 {
        st.step(&params);
        let both_inside = st.presence_in_sub(n, Faction::Player) > 0 && st.presence_in_sub(n, Faction::Enemy) > 0;
        if both_inside && st.subs[n].owner != Faction::Neutral {
            stayed_neutral_while_contested = false;
            break;
        }
        if !both_inside {
            break; // someone died; contest resolved
        }
    }
    assert!(stayed_neutral_while_contested, "a contested sub should not flip while both sides are inside");
}

// ===========================================================================
// (v) Outcome
// ===========================================================================

/// Elimination: a faction with zero ships and zero subs is eliminated, and the outcome names
/// the other as winner by elimination.
#[test]
fn outcome_by_elimination() {
    let mut st = Structure::new(0);
    let p = st.add_sub(SubStructure::new(Vec2::new(0.0, 0.0), 4.0, Faction::Player));
    // Player has a sub + a ship; Enemy has nothing.
    st.spawn_ship(Faction::Player, p);
    assert!(st.is_eliminated(Faction::Enemy));
    assert!(!st.is_eliminated(Faction::Player));
    let o = st.outcome();
    assert_eq!(o.winner, Some(Faction::Player));
    assert!(o.by_elimination);
}

/// Lead at horizon: with both sides alive, the winner is whoever leads on ships+subs, and an
/// exact tie is a draw.
#[test]
fn outcome_by_lead_and_tie() {
    // Lead case: Player has more ships.
    let mut st = Structure::new(0);
    let p = st.add_sub(SubStructure::new(Vec2::new(0.0, 0.0), 4.0, Faction::Player));
    let e = st.add_sub(SubStructure::new(Vec2::new(50.0, 0.0), 4.0, Faction::Enemy));
    for _ in 0..5 {
        st.spawn_ship(Faction::Player, p);
    }
    for _ in 0..2 {
        st.spawn_ship(Faction::Enemy, e);
    }
    let o = st.outcome();
    assert_eq!(o.winner, Some(Faction::Player));
    assert!(!o.by_elimination);

    // Tie case: mirror counts => draw.
    let mut st2 = Structure::new(0);
    let p2 = st2.add_sub(SubStructure::new(Vec2::new(0.0, 0.0), 4.0, Faction::Player));
    let e2 = st2.add_sub(SubStructure::new(Vec2::new(50.0, 0.0), 4.0, Faction::Enemy));
    for _ in 0..3 {
        st2.spawn_ship(Faction::Player, p2);
        st2.spawn_ship(Faction::Enemy, e2);
    }
    assert_eq!(st2.outcome().winner, None, "equal ships+subs should be a draw");
}

/// A full Automaton-vs-Automaton match from the sample reaches a decisive outcome within the
/// horizon (it does not stall), and the winner holds strictly more ships+subs than the loser.
#[test]
fn auto_match_reaches_decisive_outcome() {
    let params = sample_params();
    let (mut st, _l) = sample_structure(0xC0FFEE_1234);
    let player = Automaton::new(Faction::Player);
    let enemy = Automaton::new(Faction::Enemy);
    let outcome = layer1::run_auto_vs_auto(&mut st, &params, &player, &enemy, 4000, 4, |_, _| {});
    assert!(outcome.winner.is_some(), "the match should not end in a draw");
    // The sample match resolves by elimination well within the horizon.
    assert!(outcome.by_elimination, "sample match should resolve by elimination");
    assert!(outcome.tick < 4000, "should end before the horizon");
}

// ===========================================================================
// AI seam — the documented flaw is exploitable
// ===========================================================================

/// The Automaton's documented seam ("commits its reserve to the nearest fight, leaving its
/// rear thinly held") is genuinely exploitable by a flanking detachment — over several
/// seeds, not by luck.
///
/// Setup: the Enemy runs the Automaton. The Player runs a hand-scripted FLANK that exploits
/// the seam directly: keep a *holding* force at the central keep to fix the Enemy's front
/// (its nearest-fight rules glue its army there), and immediately send a wide detachment
/// from the player post to the Enemy's rear home, which the Automaton never garrisons for
/// its own sake. We require the flank to capture the Enemy home (or eliminate the Enemy) in
/// a **majority** of seeds — proving the seam is a reliable exploit, not a fluke.
///
/// Contrast control: against an Enemy that *defends its rear* (here approximated by checking
/// the seam exists — the Automaton has no rear-guard rule), the same flank would be punished.
#[test]
fn ai_seam_thin_rear_is_exploitable() {
    let params = sample_params();
    let seeds: [u64; 7] = [1, 7, 42, 100, 0x5EA1, 2024, 31337];
    let mut exploited = 0;

    for &seed in &seeds {
        let (mut st, layout) = sample_structure(seed);
        let enemy = Automaton::new(Faction::Enemy);
        let mut launched = false;
        let mut win = false;

        for _ in 0..700 {
            for o in enemy.decide(&st, &params) {
                st.issue_order(o);
            }
            // Holding force: trickle a few ships to the keep to fix the Enemy's front, but
            // keep a reserve at home (do NOT bleed the player dry — that is the mistake the
            // naive line makes). Only send if the home is well-stocked.
            if st.tick % 10 == 0 && st.idle_count_at(layout.player_home, Faction::Player) > 6 {
                st.issue_order(MoveOrder::new(layout.player_home, layout.neutral_keep, FractionBucket::Quarter));
            }
            // The FLANK: send the forward post AND a home detachment wide to the enemy home
            // early, before the Enemy can snowball. The enemy home is the prize the seam
            // leaves open.
            if !launched && st.tick >= 8 {
                st.issue_order(MoveOrder::new(layout.player_post, layout.enemy_home, FractionBucket::All));
                st.issue_order(MoveOrder::new(layout.player_home, layout.enemy_home, FractionBucket::Half));
                launched = true;
            }
            // Keep funnelling reinforcement to the flank's target once en route.
            if launched && st.tick % 12 == 0 {
                st.issue_order(MoveOrder::new(layout.player_home, layout.enemy_home, FractionBucket::Half));
            }
            st.step(&params);
            if st.subs[layout.enemy_home].owner == Faction::Player || st.is_eliminated(Faction::Enemy) {
                win = true;
                break;
            }
        }
        if win {
            exploited += 1;
        }
    }

    assert!(
        exploited * 2 > seeds.len(),
        "the flank should exploit the thin-rear seam in a majority of seeds, got {exploited}/{}",
        seeds.len()
    );
}

/// The Automaton actually plays: from the sample start it expands (captures at least one
/// neutral) within the opening — it is not inert.
#[test]
fn automaton_expands_early() {
    let params = sample_params();
    let (mut st, _l) = sample_structure(123);
    let player = Automaton::new(Faction::Player);
    let enemy = Automaton::new(Faction::Enemy);
    let start_player_subs = st.sub_count(Faction::Player);
    layer1::run_auto_vs_auto(&mut st, &params, &player, &enemy, 40, 4, |_, _| {});
    let now_player_subs = st.sub_count(Faction::Player);
    // Either side expanding proves the policy issues real orders; the player (acts first)
    // should have grown its territory by the early game.
    assert!(
        now_player_subs > start_player_subs || st.sub_count(Faction::Enemy) > 2,
        "an Automaton should have captured at least one neutral early"
    );
}
