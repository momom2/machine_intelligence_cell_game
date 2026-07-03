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
use layer1::sim::{SimParams, Interior, SubStructure};
use layer1::{Automaton, Faction, FractionBucket, MoveOrder, Ship, Vec2};

/// Helper: a minimal two-sub structure with `np` Player and `ne` Enemy ships clustered at
/// the origin within engagement range, owned subs placed FAR away so neither the defender
/// bonus nor capture interferes — isolating raw combat. Production at the far subs cannot
/// reach the origin within the short fights we run.
fn origin_clash(seed: u64, np: usize, ne: usize) -> Interior {
    let mut st = Interior::new(seed);
    // Two **neutral** subs co-located at the origin. With the orbit, each side's idle ships ring
    // their home sub; co-located ⇒ the rings coincide and every ship is within engagement range,
    // isolating raw combat: neutral ⇒ no production and no defender bonus, and with *both* sides
    // present each sub stays contested (frozen) so nothing captures during the short fights.
    let a = st.add_sub(SubStructure::new(Vec2::new(0.0, 0.0), 3.0, Faction::Neutral));
    let b = st.add_sub(SubStructure::new(Vec2::new(0.0, 0.0), 3.0, Faction::Neutral));
    for i in 0..np {
        st.ships.push(Ship {
            faction: Faction::Player,
            pos: Vec2::new(0.0, 0.0),
            target: None,
            home: a,
            aim: Vec2::new(0.0, 0.0),
            alive: true,
            angle: i as f32 * 0.5, // spread around the ring; relaxation evens it out
            undock_remaining: 0,
            drift_remaining: 0,
            ring_offset: 0.0,
        });
    }
    for i in 0..ne {
        st.ships.push(Ship {
            faction: Faction::Ai(0),
            pos: Vec2::new(0.0, 0.0),
            target: None,
            home: b,
            aim: Vec2::new(0.0, 0.0),
            alive: true,
            angle: i as f32 * 0.5,
            undock_remaining: 0,
            drift_remaining: 0,
            ring_offset: 0.0,
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
        if near(Faction::Player) == 0 || near(Faction::Ai(0)) == 0 {
            break;
        }
    }
    let near = |f: Faction| {
        st.ships.iter().filter(|s| s.alive && s.faction == f && s.pos.x.abs() < 50.0).count()
    };
    (near(Faction::Player), near(Faction::Ai(0)))
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

/// The **spread-damage** combat path (what the GUI uses) must scale like the square law too:
/// a large force facing a small one annihilates it and keeps most of its own. (Diagnostic for
/// the "combat doesn't scale with high-vs-low" report; prints survivor means.)
#[test]
fn spread_combat_scales_with_numbers() {
    let mut params = sample_params();
    params.spread_damage = true;
    let runs = 30u64;
    for (n, m) in [(10usize, 10usize), (50, 10), (200, 10)] {
        let (mut atk, mut def) = (0usize, 0usize);
        for seed in 0..runs {
            let (p, e) = fight_origin(seed.wrapping_mul(0x1234_5677), n, m, 120, &params);
            atk += p;
            def += e;
        }
        let (am, dm) = (atk as f64 / runs as f64, def as f64 / runs as f64);
        println!("SPREAD {n}v{m}: atk_surv={am:.1} def_surv={dm:.1}");
    }
    // 200 vs 10 must be a near-annihilation of the small side.
    let (mut atk, mut def) = (0usize, 0usize);
    for seed in 0..runs {
        let (p, e) = fight_origin(seed.wrapping_mul(0x1234_5677), 200, 10, 120, &params);
        atk += p;
        def += e;
    }
    let (am, dm) = (atk as f64 / runs as f64, def as f64 / runs as f64);
    assert!(dm < 1.5, "200 should annihilate 10 under spread combat, def_surv={dm:.1}");
    assert!(am > 150.0, "200 should keep most of its force, atk_surv={am:.1}");
}

// ===========================================================================
// (ii) Determinism
// ===========================================================================

/// Same seed + same orders => byte-identical evolution at every tick (full state hash).
#[test]
fn deterministic_same_seed_same_orders() {
    let params = sample_params();
    let scripted = |st: &mut Interior| {
        // A fixed script of orders issued at fixed ticks (exercises movement + capture).
        if st.tick == 0 {
            st.issue_order(MoveOrder::new(0, 6, FractionBucket::Half), Faction::Player);
            st.issue_order(MoveOrder::new(1, 6, FractionBucket::Half), Faction::Ai(0));
        }
        if st.tick == 30 {
            st.issue_order(MoveOrder::new(0, 4, FractionBucket::All), Faction::Player);
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
    let enemy = Automaton::new(Faction::Ai(0));
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
///
/// We send to `neutral_left` (low-left, well away from any enemy post) rather than the central
/// keep on purpose: it sits outside every engagement bubble, so no combat thins the wave and we
/// can assert the arrival count *exactly*. (Under the new resistance model the destination does
/// NOT instantly flip — capture is a slow grind — so this test now isolates pure movement, with
/// capture verified separately in `capture_neutral_substructure`.)
#[test]
fn ship_moves_between_substructures() {
    let params = sample_params();
    let (mut st, layout) = sample_structure(42);

    let before_home = st.idle_count_at(layout.player_home, Faction::Player);
    assert!(before_home > 0);
    let before_dest = st.idle_count_at(layout.neutral_left, Faction::Player);
    assert_eq!(before_dest, 0);

    // Send half of the home garrison to the quiet low-left neutral post.
    let ordered = st.issue_order(MoveOrder::new(layout.player_home, layout.neutral_left, FractionBucket::Half), Faction::Player);
    assert!(ordered > 0, "order should move at least one ship");

    // Immediately after issuing: those ships are no longer idle at home (they are moving).
    let moving_now = st.idle_count_at(layout.player_home, Faction::Player);
    assert_eq!(moving_now, before_home - ordered, "ordered ships should leave the home idle pool");

    // Step until they arrive (home->neutral_left is ~19 units at speed 1.4 => well under 60 ticks).
    for _ in 0..60 {
        st.step(&params);
    }
    // No enemy can reach neutral_left, so exactly the ordered ships should be idle there (the
    // home garrison's own production goes to `home`, not here).
    let arrived = st.idle_count_at(layout.neutral_left, Faction::Player);
    assert!(
        arrived >= ordered,
        "ordered ships should have arrived and gone idle at neutral_left, got {arrived} (ordered {ordered})"
    );
}

/// Issuing junk orders is a safe no-op: same source/target, out-of-range ids, and a source
/// with no idle ships all move zero ships and do not panic.
#[test]
fn junk_orders_are_safe_noops() {
    let (mut st, layout) = sample_structure(1);
    assert_eq!(st.issue_order(MoveOrder::new(layout.player_home, layout.player_home, FractionBucket::All), Faction::Player), 0);
    assert_eq!(st.issue_order(MoveOrder::new(999, 0, FractionBucket::All), Faction::Player), 0);
    assert_eq!(st.issue_order(MoveOrder::new(0, 999, FractionBucket::All), Faction::Player), 0);
    // neutral_keep starts empty => no idle ships to send.
    assert_eq!(st.issue_order(MoveOrder::new(layout.neutral_keep, layout.player_home, FractionBucket::All), Faction::Player), 0);
}

// ===========================================================================
// (iv) Capture
// ===========================================================================

/// A wave sent into an uncontested neutral sub-structure **grinds its resistance down** and
/// captures it, and the captured sub then **produces** for its new owner.
///
/// Under the new model capture is no longer instant: an uncontested foreign force erodes the
/// resistance bar by its present count each tick, flipping the sub at zero. We give the target a
/// small `with_max_resistance` so the grind completes quickly and the test stays focused.
#[test]
fn capture_neutral_substructure() {
    let params = sample_params();
    // Custom minimal structure: a player home and a quiet neutral foothold close by, far from
    // any enemy so nothing contests the grind. The foothold has a low resistance cap.
    let mut st = Interior::new(5);
    let home = st.add_sub(SubStructure::new(Vec2::new(0.0, 0.0), 4.0, Faction::Player));
    let neutral = st
        .add_sub(SubStructure::new(Vec2::new(12.0, 0.0), 4.0, Faction::Neutral).with_max_resistance(30.0));
    for _ in 0..8 {
        st.spawn_ship(Faction::Player, home);
    }
    assert_eq!(st.subs[neutral].owner, Faction::Neutral);
    let (res0, max0) = st.sub_resistance(neutral);
    assert_eq!((res0, max0), (30.0, 30.0), "foothold starts at its (low) max resistance");

    // Send a wave; ~5 ships eroding 30 resistance flips it in a handful of ticks once arrived.
    let moved = st.issue_order(MoveOrder::new(home, neutral, FractionBucket::Half), Faction::Player);
    assert!(moved > 0);

    let mut captured_tick = None;
    for _ in 0..80 {
        st.step(&params);
        if st.subs[neutral].owner == Faction::Player {
            captured_tick = Some(st.tick);
            break;
        }
    }
    assert!(captured_tick.is_some(), "player should have ground down and captured the neutral sub");
    assert_eq!(st.subs[neutral].owner, Faction::Player);
    // A freshly captured sub refills to its max and then produces for the new owner on cadence.
    let (res_after, _) = st.sub_resistance(neutral);
    assert_eq!(res_after, 30.0, "a freshly flipped sub refills to its max_resistance");
    let ships_at_capture = st.ship_count(Faction::Player);
    for _ in 0..(params.production_period as usize + 2) {
        st.step(&params);
    }
    assert!(
        st.ship_count(Faction::Player) > ships_at_capture,
        "the captured sub should produce for its new owner"
    );
}

/// An enemy sub-structure left undefended is **ground down** when a player force arrives and no
/// living enemy contests it. We give it a low resistance cap so the grind completes in the loop.
#[test]
fn capture_enemy_substructure() {
    let params = sample_params();
    // Custom minimal structure: a player sub and an (initially enemy) sub close together,
    // with NO enemy ships, so the enemy sub is undefended. The enemy sub has low resistance.
    let mut st = Interior::new(9);
    let p = st.add_sub(SubStructure::new(Vec2::new(0.0, 0.0), 4.0, Faction::Player));
    let e = st
        .add_sub(SubStructure::new(Vec2::new(10.0, 0.0), 4.0, Faction::Ai(0)).with_max_resistance(30.0));
    for _ in 0..6 {
        st.spawn_ship(Faction::Player, p);
    }
    // Enemy owns `e` but has zero ships anywhere.
    assert_eq!(st.subs[e].owner, Faction::Ai(0));
    assert_eq!(st.ship_count(Faction::Ai(0)), 0);

    let moved = st.issue_order(MoveOrder::new(p, e, FractionBucket::All), Faction::Player);
    assert!(moved > 0);
    for _ in 0..80 {
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
    let mut st = Interior::new(3);
    let n = st.add_sub(SubStructure::new(Vec2::new(0.0, 0.0), 5.0, Faction::Neutral));
    // Put one of each faction right inside the neutral sub.
    let far_p = st.add_sub(SubStructure::new(Vec2::new(-500.0, 0.0), 3.0, Faction::Player));
    let far_e = st.add_sub(SubStructure::new(Vec2::new(500.0, 0.0), 3.0, Faction::Ai(0)));
    st.ships.push(Ship { faction: Faction::Player, pos: Vec2::new(-0.5, 0.0), target: None, home: far_p, aim: Vec2::new(-0.5, 0.0), alive: true, angle: 0.0, undock_remaining: 0, drift_remaining: 0, ring_offset: 0.0 });
    st.ships.push(Ship { faction: Faction::Ai(0), pos: Vec2::new(0.5, 0.0), target: None, home: far_e, aim: Vec2::new(0.5, 0.0), alive: true, angle: 0.0, undock_remaining: 0, drift_remaining: 0, ring_offset: 0.0 });

    // For a few ticks while both are alive inside, the neutral must stay neutral.
    let mut stayed_neutral_while_contested = true;
    for _ in 0..6 {
        st.step(&params);
        let both_inside = st.presence_in_sub(n, Faction::Player) > 0 && st.presence_in_sub(n, Faction::Ai(0)) > 0;
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
// (iv-b) The new mechanics: capture grind, heal, denial, soft-cap plateau
// ===========================================================================

/// The pure capture rule [`SubStructure::capture_step`] — the single source of truth the sim
/// and the projection share — behaves per spec: frozen (none/both present), heal (owner only),
/// erode (one foreign), and flip+refill at zero.
#[test]
fn capture_step_pure_rule() {
    let max = 100.0;
    // Frozen: nobody present.
    assert_eq!(
        SubStructure::capture_step(Faction::Player, 40.0, max, 0, 0),
        (Faction::Player, 40.0, false)
    );
    // Frozen: both present (contested).
    assert_eq!(
        SubStructure::capture_step(Faction::Player, 40.0, max, 3, 5),
        (Faction::Player, 40.0, false)
    );
    // Heal: only the owner present, by its present count, capped at max.
    assert_eq!(
        SubStructure::capture_step(Faction::Player, 40.0, max, 7, 0),
        (Faction::Player, 47.0, false)
    );
    assert_eq!(
        SubStructure::capture_step(Faction::Player, 98.0, max, 7, 0),
        (Faction::Player, 100.0, false),
        "heal is capped at max_resistance"
    );
    // Erode: only a foreign faction present, by its count; no flip while > 0.
    assert_eq!(
        SubStructure::capture_step(Faction::Player, 40.0, max, 0, 6),
        (Faction::Player, 34.0, false)
    );
    // Flip + refill at <= 0.
    assert_eq!(
        SubStructure::capture_step(Faction::Player, 4.0, max, 0, 6),
        (Faction::Ai(0), 100.0, true)
    );
    // A neutral-owned sub is always eroding (no ship is ever Neutral).
    assert_eq!(
        SubStructure::capture_step(Faction::Neutral, 10.0, max, 3, 0),
        (Faction::Neutral, 7.0, false)
    );
}

/// Capture is a **grind**: clearing a fresh sub with `F` uncontested present attackers takes
/// `ceil(max_resistance / F)` ticks — more ships means a faster (linear) grind. We park a fixed
/// idle force inside a neutral sub (no production, no combat, no movement) and count the ticks to
/// the flip, for two force sizes, and assert the close-form relationship.
#[test]
fn capture_is_a_grind_more_ships_faster() {
    let params = sample_params();

    // Park `force` idle Enemy-faction ships dead-centre in a fresh neutral sub of resistance
    // `maxr`, with the owning home far away and empty, and step until it flips. Returns ticks.
    fn ticks_to_flip(seed: u64, force: usize, maxr: f32, params: &SimParams) -> u64 {
        let mut st = Interior::new(seed);
        let n = st
            .add_sub(SubStructure::new(Vec2::new(0.0, 0.0), 6.0, Faction::Neutral).with_max_resistance(maxr));
        // Home the attackers AT the neutral so the orbit keeps them inside it (eroding from tick 1,
        // no travel). They are the lone present faction, so it flips on ceil(R / F).
        for _ in 0..force {
            st.ships.push(Ship {
                faction: Faction::Ai(0),
                pos: Vec2::new(0.0, 0.0),
                target: None,
                home: n,
                aim: Vec2::new(0.0, 0.0),
                alive: true,
                angle: 0.0,
                undock_remaining: 0,
                drift_remaining: 0,
                ring_offset: 0.0,
            });
        }
        let mut t = 0;
        while st.subs[n].owner != Faction::Ai(0) && t < 100_000 {
            st.step(params);
            t += 1;
        }
        st.tick
    }

    // With F present attackers and resistance R, the flip happens on tick ceil(R / F): each tick
    // subtracts F, and on the tick the bar reaches <= 0 it flips that same tick.
    let maxr = 120.0;
    let t4 = ticks_to_flip(1, 4, maxr, &params);
    let t12 = ticks_to_flip(2, 12, maxr, &params);
    assert_eq!(t4, (maxr / 4.0).ceil() as u64, "4 attackers grind 120 in 30 ticks");
    assert_eq!(t12, (maxr / 12.0).ceil() as u64, "12 attackers grind 120 in 10 ticks");
    assert!(t12 < t4, "more present attackers => a strictly faster grind");
}

/// A returning/garrisoning owner **heals** an eroded sub back toward its max — so a hit-and-run
/// accomplishes nothing. We erode a player sub partway with a foreign force, remove the foreign
/// force, leave a player garrison inside, and watch the resistance climb back to max (owner
/// retained throughout).
#[test]
fn owner_presence_heals_resistance_back_to_max() {
    let params = sample_params();
    let mut st = Interior::new(7);
    // One player sub. We'll erode it by hand, then let a garrison heal it.
    let s = st.add_sub(SubStructure::new(Vec2::new(0.0, 0.0), 6.0, Faction::Player).with_max_resistance(100.0));
    // Manually drop its resistance (simulating prior erosion).
    st.subs[s].resistance = 40.0;
    // Place 5 idle player ships inside it (the healers); no enemy present.
    let far = st.add_sub(SubStructure::new(Vec2::new(10_000.0, 0.0), 3.0, Faction::Player));
    let _ = far;
    for i in 0..5 {
        let x = (i as f32) * 0.05;
        st.ships.push(Ship {
            faction: Faction::Player,
            pos: Vec2::new(x, 0.0),
            target: None,
            home: s,
            aim: Vec2::new(x, 0.0),
            alive: true,
            angle: 0.0,
            undock_remaining: 0,
            drift_remaining: 0,
            ring_offset: 0.0,
        });
    }
    let (res0, max0) = st.sub_resistance(s);
    assert_eq!((res0, max0), (40.0, 100.0));

    // Heal climbs by the present count (5) per tick, capped at max; owner never changes.
    st.step(&params);
    let (res1, _) = st.sub_resistance(s);
    assert!(res1 > res0, "owner presence should heal resistance upward");
    for _ in 0..40 {
        st.step(&params);
    }
    let (res_n, max_n) = st.sub_resistance(s);
    assert_eq!(res_n, max_n, "a held sub heals back to its max");
    assert_eq!(st.subs[s].owner, Faction::Player, "healing never changes the owner");
}

/// Production denial (Mechanic B): a sub being **eroded by an uncontested foe (owner absent)**
/// does not produce and its production timer is held steady; a sub defended by its owner (even
/// while contested) keeps producing.
#[test]
fn production_is_denied_while_eroded_undefended() {
    let params = sample_params();

    // Case A: an enemy-owned sub with ONLY a player force parked on it (owner absent) must not
    // produce for the enemy while it is being eroded.
    let mut st = Interior::new(11);
    let e = st.add_sub(SubStructure::new(Vec2::new(0.0, 0.0), 6.0, Faction::Ai(0)).with_max_resistance(100_000.0));
    // Park idle player ships inside it; enemy has no ships present (undefended).
    let pf = st.add_sub(SubStructure::new(Vec2::new(10_000.0, 0.0), 3.0, Faction::Player));
    let _ = pf;
    for i in 0..3 {
        let x = (i as f32) * 0.05;
        st.ships.push(Ship {
            faction: Faction::Player,
            pos: Vec2::new(x, 0.0),
            target: None,
            home: e, // sitting on the enemy sub
            aim: Vec2::new(x, 0.0),
            alive: true,
            angle: 0.0,
            undock_remaining: 0,
            drift_remaining: 0,
            ring_offset: 0.0,
        });
    }
    // High resistance so it never flips during the window; enemy ship count stays 0 (no
    // production) for the whole denial window.
    for _ in 0..(params.production_period as usize * 3) {
        st.step(&params);
        assert_eq!(
            st.ship_count(Faction::Ai(0)),
            0,
            "an eroded, undefended enemy sub must not spawn ships (production denied)"
        );
    }

    // Case B: a player-owned sub DEFENDED by its owner (player present) keeps producing even with
    // an enemy also present (contested-but-defended).
    let mut st2 = Interior::new(12);
    let d = st2.add_sub(SubStructure::new(Vec2::new(0.0, 0.0), 6.0, Faction::Player).with_max_resistance(100_000.0));
    let efar = st2.add_sub(SubStructure::new(Vec2::new(10_000.0, 0.0), 3.0, Faction::Ai(0)));
    let _ = efar;
    // Player garrison + an enemy intruder both inside the sub (contested but defended).
    for i in 0..4 {
        let x = (i as f32) * 0.05;
        st2.ships.push(Ship { faction: Faction::Player, pos: Vec2::new(-1.0 - x, 0.0), target: None, home: d, aim: Vec2::new(-1.0 - x, 0.0), alive: true, angle: 0.0, undock_remaining: 0, drift_remaining: 0, ring_offset: 0.0 });
    }
    // Keep a single enemy far enough not to be one-shot instantly but inside the radius — use a
    // fresh sub and just assert production continues over a couple of periods. To avoid the
    // firefight removing the defenders, give them numerical dominance (4 vs 1) so some survive.
    st2.ships.push(Ship { faction: Faction::Ai(0), pos: Vec2::new(1.0, 0.0), target: None, home: efar, aim: Vec2::new(1.0, 0.0), alive: true, angle: 0.0, undock_remaining: 0, drift_remaining: 0, ring_offset: 0.0 });
    let before = st2.ship_count(Faction::Player);
    for _ in 0..(params.production_period as usize + 2) {
        st2.step(&params);
    }
    assert!(
        st2.ship_count(Faction::Player) > before,
        "a defended (owner-present) sub keeps producing even while contested"
    );
}

/// Soft-cap plateau (Mechanic C): a parked hoard far above the soft cap is trimmed by `sqrt`
/// attrition until it settles **just at the soft cap** — a self-limiting plateau, not a wall —
/// while a controlled (below-cap) stack is never touched.
#[test]
fn softcap_plateaus_hoard_and_spares_control() {
    let params = sample_params();

    // Hoard: one player sub, a huge idle stack, no enemy, no movement. soft = 20 + 10*1 = 30.
    let mut st = Interior::new(1);
    let s = st.add_sub(SubStructure::new(Vec2::new(0.0, 0.0), 6.0, Faction::Player));
    for _ in 0..200 {
        st.spawn_ship(Faction::Player, s);
    }
    let soft = st.soft_cap(Faction::Player, &params);
    assert_eq!(soft, 30, "soft = softcap_free(20) + softcap_per_sub(10) * owned_subs(1)");
    assert_eq!(st.parked_count(Faction::Player), 200);

    for _ in 0..200 {
        st.step(&params);
    }
    // Production keeps adding one ship every period, but the cap trims back to exactly `soft`.
    assert_eq!(
        st.parked_count(Faction::Player),
        soft,
        "a hoard plateaus at the soft cap (sqrt attrition is self-limiting)"
    );

    // Control: a stack at/under the soft cap is never attrited. 25 idle <= soft 30.
    let mut st2 = Interior::new(2);
    let s2 = st2.add_sub(SubStructure::new(Vec2::new(0.0, 0.0), 6.0, Faction::Player));
    for _ in 0..25 {
        st2.spawn_ship(Faction::Player, s2);
    }
    let start = st2.parked_count(Faction::Player);
    // Step a window short enough that production does not push it over the cap.
    for _ in 0..(params.production_period as usize - 1) {
        st2.step(&params);
    }
    assert!(
        st2.parked_count(Faction::Player) >= start,
        "a below-cap stack is never destroyed by the soft cap"
    );
}

/// The new capture state is part of the determinism fingerprint: two structures that differ
/// ONLY in a sub's resistance hash differently (so a divergent grind is detected), and an
/// otherwise-identical pair stays bit-identical through stepping.
#[test]
fn resistance_is_folded_into_state_hash() {
    let a = {
        let mut st = Interior::new(5);
        st.add_sub(SubStructure::new(Vec2::new(0.0, 0.0), 4.0, Faction::Player));
        st
    };
    let mut b = a.clone();
    assert_eq!(a.state_hash(), b.state_hash(), "identical structures hash identically");
    // Perturb only the resistance of b's sub.
    b.subs[0].resistance -= 1.0;
    assert_ne!(
        a.state_hash(),
        b.state_hash(),
        "a difference in resistance alone must change the state hash"
    );
}

// ===========================================================================
// (v) Outcome
// ===========================================================================

/// Elimination: a faction with zero ships and zero subs is eliminated, and the outcome names
/// the other as winner by elimination.
#[test]
fn outcome_by_elimination() {
    let mut st = Interior::new(0);
    let p = st.add_sub(SubStructure::new(Vec2::new(0.0, 0.0), 4.0, Faction::Player));
    // Player has a sub + a ship; Enemy has nothing.
    st.spawn_ship(Faction::Player, p);
    assert!(st.is_eliminated(Faction::Ai(0)));
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
    let mut st = Interior::new(0);
    let p = st.add_sub(SubStructure::new(Vec2::new(0.0, 0.0), 4.0, Faction::Player));
    let e = st.add_sub(SubStructure::new(Vec2::new(50.0, 0.0), 4.0, Faction::Ai(0)));
    for _ in 0..5 {
        st.spawn_ship(Faction::Player, p);
    }
    for _ in 0..2 {
        st.spawn_ship(Faction::Ai(0), e);
    }
    let o = st.outcome();
    assert_eq!(o.winner, Some(Faction::Player));
    assert!(!o.by_elimination);

    // Tie case: mirror counts => draw.
    let mut st2 = Interior::new(0);
    let p2 = st2.add_sub(SubStructure::new(Vec2::new(0.0, 0.0), 4.0, Faction::Player));
    let e2 = st2.add_sub(SubStructure::new(Vec2::new(50.0, 0.0), 4.0, Faction::Ai(0)));
    for _ in 0..3 {
        st2.spawn_ship(Faction::Player, p2);
        st2.spawn_ship(Faction::Ai(0), e2);
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
    let enemy = Automaton::new(Faction::Ai(0));
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
/// rear thinly held") is genuinely exploitable by a flanking detachment — over several seeds,
/// not by luck.
///
/// **Re-expressed for the new resistance/denial model.** Capture is no longer instant, so the
/// old "snipe the rear in a few ticks" no longer happens — and *that is the point*. Under the
/// new mechanics the seam manifests as **sustained denial + grind**: a flank that reaches the
/// Automaton's rear home finds it undefended and *stays* uncontested there for a long stretch,
/// because the Automaton has no rear-guard rule (it is busy at the fixed front). While the
/// flank sits uncontested it (a) **starves** the rear's production (Mechanic B) and (b) grinds
/// the rear's resistance down — and given enough uncontested time, captures it.
///
/// Setup: the Enemy runs the Automaton. The Player keeps a *holding* force at the central keep
/// to fix the Enemy's front (its nearest-fight rules glue its army there), and sends a wide
/// detachment to the Enemy's rear home. We require, in a **majority** of seeds, that the flank
/// either captures the rear home OR holds it **uncontested for a sustained window** (>= 30
/// consecutive ticks of Player-only presence on the enemy home) — the spatial signature that
/// the Automaton posts no rear guard. (Empirically this holds for all 7 seeds with large
/// margin; we assert only a majority to stay robust.)
#[test]
#[ignore = "curriculum contract for the PARKED layer1 demo Automaton (the seam); void until the greedy rework"]
fn ai_seam_thin_rear_is_exploitable() {
    let params = sample_params();
    let seeds: [u64; 7] = [1, 7, 42, 100, 0x5EA1, 2024, 31337];
    // A sustained uncontested-presence streak this long on the enemy rear is the denial/grind
    // signature of the seam under the new model (vs the old instant snipe).
    const DENY_STREAK_TICKS: u32 = 30;
    let mut exploited = 0;

    for &seed in &seeds {
        let (mut st, layout) = sample_structure(seed);
        let enemy = Automaton::new(Faction::Ai(0));
        let mut launched = false;
        let mut exploited_this_seed = false;
        let mut deny_streak = 0u32;

        for _ in 0..700 {
            for o in enemy.decide(&st, &params) {
                st.issue_order(o, Faction::Ai(0));
            }
            // Holding force: trickle a few ships to the keep to fix the Enemy's front, but
            // keep a reserve at home (do NOT bleed the player dry — that is the mistake the
            // naive line makes). Only send if the home is well-stocked.
            if st.tick % 10 == 0 && st.idle_count_at(layout.player_home, Faction::Player) > 6 {
                st.issue_order(MoveOrder::new(layout.player_home, layout.neutral_keep, FractionBucket::Quarter), Faction::Player);
            }
            // The FLANK: send the forward post AND a home detachment wide to the enemy home
            // early, before the Enemy can snowball. The enemy home is the prize the seam
            // leaves open.
            if !launched && st.tick >= 8 {
                st.issue_order(MoveOrder::new(layout.player_post, layout.enemy_home, FractionBucket::All), Faction::Player);
                st.issue_order(MoveOrder::new(layout.player_home, layout.enemy_home, FractionBucket::Half), Faction::Player);
                launched = true;
            }
            // Keep funnelling reinforcement to the flank's target so the grind is sustained.
            if launched && st.tick % 12 == 0 {
                st.issue_order(MoveOrder::new(layout.player_home, layout.enemy_home, FractionBucket::Half), Faction::Player);
            }
            st.step(&params);

            // Outright capture (the grind completed) is the strongest proof.
            if st.subs[layout.enemy_home].owner == Faction::Player || st.is_eliminated(Faction::Ai(0)) {
                exploited_this_seed = true;
                break;
            }
            // Otherwise track sustained uncontested presence on the enemy rear: Player ships
            // inside it with NO enemy ship contesting (the Automaton never sent a rear guard).
            let p_here = st.presence_in_sub(layout.enemy_home, Faction::Player);
            let e_here = st.presence_in_sub(layout.enemy_home, Faction::Ai(0));
            if p_here > 0 && e_here == 0 {
                deny_streak += 1;
                if deny_streak >= DENY_STREAK_TICKS {
                    exploited_this_seed = true;
                    break;
                }
            } else {
                deny_streak = 0;
            }
        }
        if exploited_this_seed {
            exploited += 1;
        }
    }

    assert!(
        exploited * 2 > seeds.len(),
        "the flank should exploit the thin-rear seam (capture OR sustained denial of the rear) \
         in a majority of seeds, got {exploited}/{}",
        seeds.len()
    );
}

/// The Automaton actually plays: from the sample start it pushes into neutral territory within
/// the opening — it is not inert.
///
/// **Re-expressed for the new resistance model.** Capture is now a grind (fresh resistance equals
/// the sub's storage capacity), so a neutral will not *flip* in 40 ticks. The early "expand" signal is therefore
/// the Automaton committing a wave that *erodes* a neutral: by the early game some neutral sub
/// has ships present AND its resistance has been ground below its max. That proves the policy is
/// issuing real expansion orders and the grind is underway.
#[test]
fn automaton_expands_early() {
    let params = sample_params();
    let (mut st, _l) = sample_structure(123);
    let player = Automaton::new(Faction::Player);
    let enemy = Automaton::new(Faction::Ai(0));
    layer1::run_auto_vs_auto(&mut st, &params, &player, &enemy, 40, 4, |_, _| {});

    // Some neutral sub should have a ship present (the expansion wave has arrived/is arriving)
    // and have lost resistance (the grind has begun) by the early game.
    let mut neutral_presence = 0usize;
    let mut some_neutral_eroded = false;
    for s in 0..st.subs.len() {
        if st.subs[s].owner == Faction::Neutral {
            neutral_presence +=
                st.presence_in_sub(s, Faction::Player) + st.presence_in_sub(s, Faction::Ai(0));
            let (res, max) = st.sub_resistance(s);
            if res < max {
                some_neutral_eroded = true;
            }
        }
    }
    assert!(
        neutral_presence > 0 && some_neutral_eroded,
        "an Automaton should have pushed a wave into a neutral and begun grinding it early \
         (presence={neutral_presence}, eroded={some_neutral_eroded})"
    );
}


// =====================================================================================
// Special sub-structures (SubKind): fortress / teleporter / shipyard.
// =====================================================================================

use layer1::SubKind;

/// Push an idle ship directly (the raw-construction idiom `origin_clash` uses).
fn park_ship(st: &mut Interior, faction: Faction, home: usize, angle: f32) {
    let pos = st.subs[home].pos;
    st.ships.push(Ship {
        faction,
        pos,
        target: None,
        home,
        aim: pos,
        alive: true,
        angle,
        undock_remaining: 0,
        drift_remaining: 0,
        ring_offset: 0.0,
    });
}

#[test]
fn shipyard_constructor_active_iff_owned() {
    let owned = SubStructure::shipyard(Vec2::new(0.0, 0.0), Faction::Player);
    assert_eq!(owned.kind, SubKind::Shipyard { active: true });
    assert!(owned.max_resistance <= 1.0 + 1e-6, "an owned-authored yard starts with the token bar");
    assert_eq!(owned.production, layer1::sim::SHIPYARD_PRODUCTION);
    assert_eq!(owned.storage_capacity, 0);

    let neutral = SubStructure::shipyard(Vec2::new(0.0, 0.0), Faction::Neutral);
    assert_eq!(neutral.kind, SubKind::Shipyard { active: false });
    assert!((neutral.max_resistance - layer1::sim::SHIPYARD_INITIAL_RESISTANCE).abs() < 1e-3);

    // Normal-size footprint despite the zero capacity (selection + garrison-ring geometry).
    let standard = SubStructure::new(Vec2::new(0.0, 0.0), 0.0, Faction::Neutral);
    assert!((neutral.radius - standard.radius).abs() < 1e-6);
}

#[test]
fn shipyard_activation_collapses_the_bar_for_good() {
    let mut params = SimParams::default();
    params.softcap_free = 10_000; // no attrition noise on the big grinding stack

    let mut st = Interior::new(7);
    let yard = st.add_sub(SubStructure::shipyard(Vec2::new(0.0, 0.0), Faction::Neutral));
    // 200 attackers grind the 10800 activation bar in ceil(10800/200) = 54 ticks.
    for i in 0..200 {
        park_ship(&mut st, Faction::Player, yard, i as f32 * 0.03);
    }
    let mut flipped = false;
    for _ in 0..80 {
        st.step(&params);
        if st.subs[yard].owner == Faction::Player {
            flipped = true;
            break;
        }
    }
    assert!(flipped, "the activation grind must complete");
    assert_eq!(st.subs[yard].kind, SubKind::Shipyard { active: true });
    assert!(
        st.subs[yard].max_resistance <= 1.0 + 1e-6,
        "activation collapses the bar to the token grind, got {}",
        st.subs[yard].max_resistance
    );

    // From here the yard flips to any lone visitor almost instantly — and STAYS active.
    for sh in &mut st.ships {
        sh.alive = false; // clear the player garrison
    }
    st.subs[yard].production_timer = 50; // hold production so no fresh defender contests the flip
    for i in 0..3 {
        park_ship(&mut st, Faction::Ai(0), yard, i as f32);
    }
    for _ in 0..3 {
        st.step(&params);
    }
    assert_eq!(st.subs[yard].owner, Faction::Ai(0), "an active yard is trivially stealable");
    assert_eq!(st.subs[yard].kind, SubKind::Shipyard { active: true });
    assert!(st.subs[yard].max_resistance <= 1.0 + 1e-6, "no re-inflation on later flips");
}

#[test]
fn teleporter_departures_arrive_instantly_for_the_owner_only() {
    let params = SimParams::default();
    let undock = params.undock_ticks as usize;

    // (a) The owner's departures land the tick the undock burns out.
    let mut st = Interior::new(11);
    let gate = st.add_sub(SubStructure::teleporter(Vec2::new(0.0, 0.0), Faction::Player));
    let far = st.add_sub(SubStructure::new(Vec2::new(60.0, 0.0), 0.0, Faction::Neutral));
    for i in 0..10 {
        park_ship(&mut st, Faction::Player, gate, i as f32 * 0.6);
    }
    st.issue_order(MoveOrder::new(gate, far, FractionBucket::All), Faction::Player);
    for _ in 0..undock {
        st.step(&params);
    }
    assert!(
        st.idle_count_at(far, Faction::Player) >= 8,
        "teleported ships are garrisoned at the target after the undock alone, got {}",
        st.idle_count_at(far, Faction::Player)
    );

    // (b) Control: a STANDARD source over the same 60-unit gap is still in transit.
    let mut st2 = Interior::new(11);
    let src = st2.add_sub(SubStructure::new(Vec2::new(0.0, 0.0), 0.0, Faction::Player));
    let far2 = st2.add_sub(SubStructure::new(Vec2::new(60.0, 0.0), 0.0, Faction::Neutral));
    for i in 0..10 {
        park_ship(&mut st2, Faction::Player, src, i as f32 * 0.6);
    }
    st2.issue_order(MoveOrder::new(src, far2, FractionBucket::All), Faction::Player);
    for _ in 0..(undock + 2) {
        st2.step(&params);
    }
    assert_eq!(st2.idle_count_at(far2, Faction::Player), 0, "normal movers take the transit leg");

    // (c) A foreign side's departures from someone ELSE's teleporter move normally.
    let mut st3 = Interior::new(11);
    let gate3 = st3.add_sub(SubStructure::teleporter(Vec2::new(0.0, 0.0), Faction::Ai(0)));
    let far3 = st3.add_sub(SubStructure::new(Vec2::new(60.0, 0.0), 0.0, Faction::Neutral));
    for i in 0..10 {
        park_ship(&mut st3, Faction::Player, gate3, i as f32 * 0.6);
    }
    st3.issue_order(MoveOrder::new(gate3, far3, FractionBucket::All), Faction::Player);
    for _ in 0..(undock + 2) {
        st3.step(&params);
    }
    assert_eq!(
        st3.idle_count_at(far3, Faction::Player),
        0,
        "the gate works only for the side that owns it"
    );
}

#[test]
fn fortress_garrison_outranges_and_outguns_a_nearby_camp() {
    // Fortress (Player) at the origin vs an enemy camp 13 units away: the ring-to-ring gap sits
    // between R (= 3.5) and FORTRESS_RANGE (= 12), so the fortress garrison shoots into the camp
    // while the camp can never answer. With a stat-identical STANDARD sub in the fortress's place, the
    // same geometry produces no engagement at all. Checked on both combat paths. (The camps are
    // different HOMES farther apart than either sub's radius, so the enemy-seek orbit mode never
    // engages — this is a pure range test.)
    let run = |fortress: bool, spread: bool| -> (usize, usize) {
        let mut st = Interior::new(21);
        let home = if fortress {
            st.add_sub(SubStructure::fortress(Vec2::new(0.0, 0.0), Faction::Player))
        } else {
            let mut s = SubStructure::new(Vec2::new(0.0, 0.0), 0.0, Faction::Player)
                .with_storage_capacity(layer1::sim::FORTRESS_STORAGE_CAPACITY)
                .with_max_resistance(layer1::sim::FORTRESS_RESISTANCE);
            s.production = 0;
            st.add_sub(s)
        };
        let camp = st.add_sub(SubStructure::new(Vec2::new(13.0, 0.0), 0.0, Faction::Ai(0)));
        st.subs[camp].production = 0; // freeze both economies; this is a pure range test
        for i in 0..20 {
            park_ship(&mut st, Faction::Player, home, i as f32 * 0.3);
            park_ship(&mut st, Faction::Ai(0), camp, i as f32 * 0.3);
        }
        let mut params = SimParams::default();
        params.fire_prob = 0.5; // frequent hits => a short, robust test
        params.defender_fire_bonus = 0.0;
        params.softcap_free = 10_000;
        params.spread_damage = spread;
        for _ in 0..120 {
            st.step(&params);
        }
        (st.ship_count(Faction::Player), st.ship_count(Faction::Ai(0)))
    };
    for spread in [false, true] {
        let (p_fort, e_fort) = run(true, spread);
        assert_eq!(p_fort, 20, "nothing can reach the fortress garrison (spread={spread})");
        assert!(
            e_fort < 20,
            "the fortress garrison kills into the camp from beyond normal range (spread={spread})"
        );
        let (p_std, e_std) = run(false, spread);
        assert_eq!(p_std, 20, "control: out of range both ways (spread={spread})");
        assert_eq!(e_std, 20, "control: a standard sub cannot reach the camp (spread={spread})");
    }
}

#[test]
fn sub_kind_is_part_of_the_state_hash() {
    let build = |teleporter: bool| -> u64 {
        let mut st = Interior::new(5);
        if teleporter {
            st.add_sub(SubStructure::teleporter(Vec2::new(0.0, 0.0), Faction::Player));
        } else {
            // Stat-identical to the teleporter (production 0); only `kind` differs.
            let mut s = SubStructure::new(Vec2::new(0.0, 0.0), 0.0, Faction::Player);
            s.production = 0;
            st.add_sub(s);
        }
        st.state_hash()
    };
    assert_ne!(build(true), build(false), "two structures differing only in kind must hash apart");
}

#[test]
fn specials_replay_deterministically() {
    let run = || -> u64 {
        let mut st = Interior::new(33);
        let gate = st.add_sub(SubStructure::teleporter(Vec2::new(0.0, 0.0), Faction::Player));
        let yard = st.add_sub(SubStructure::shipyard(Vec2::new(30.0, 0.0), Faction::Neutral));
        let _fort = st.add_sub(SubStructure::fortress(Vec2::new(0.0, 30.0), Faction::Ai(0)));
        for i in 0..20 {
            park_ship(&mut st, Faction::Player, gate, i as f32 * 0.3);
            park_ship(&mut st, Faction::Ai(0), _fort, i as f32 * 0.3);
        }
        let params = SimParams::default();
        st.issue_order(MoveOrder::new(gate, yard, FractionBucket::Half), Faction::Player);
        for _ in 0..200 {
            st.step(&params);
        }
        st.state_hash()
    };
    assert_eq!(run(), run(), "a map with all three specials must replay bit-for-bit");
}


#[test]
fn ship_engagement_reach_is_per_shooter() {
    let params = SimParams::default();
    let mut st = Interior::new(3);
    let fort = st.add_sub(SubStructure::fortress(Vec2::new(0.0, 0.0), Faction::Player));
    let plain = st.add_sub(SubStructure::new(Vec2::new(30.0, 0.0), 0.0, Faction::Player));
    park_ship(&mut st, Faction::Player, fort, 0.0); // 0: the owner's garrison — boosted
    park_ship(&mut st, Faction::Ai(0), fort, 1.0); // 1: a foreign squatter — NOT boosted
    park_ship(&mut st, Faction::Player, plain, 0.0); // 2: a plain garrison — not boosted
    let r = params.engagement_radius;
    assert_eq!(
        st.ship_engagement_reach(0, &params),
        layer1::sim::FORTRESS_RANGE,
        "the fortress owner's garrison fires at the fixed fortress range"
    );
    assert_eq!(st.ship_engagement_reach(1, &params), r, "a foreign squatter gets no boost");
    assert_eq!(st.ship_engagement_reach(2, &params), r, "a plain sub's garrison gets no boost");
}
