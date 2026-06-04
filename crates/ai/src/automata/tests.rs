//! Unit tests for the four composable automatons, over a tiny configurable in-memory
//! [`PositionView`] (`ProgView`) that supplies both the property signals and synthetic projection
//! answers. These verify each automaton's *program* (the composition over `crate::vocab`)
//! independently of either real adapter — exactly as `greedy.rs` tests the greedy rule over a
//! `LineView`.

use super::*;
use crate::greedy::{GreedyKind, PosOwner, PositionInfo, PositionView, Side};

/// A line of positions with per-position property signals + synthetic projection answers, so the
/// automaton programs can be exercised deterministically. Distance is `|dx|`.
#[derive(Clone)]
struct ProgView {
    infos: Vec<PositionInfo>,
    xs: Vec<f32>,
    resist: Vec<f32>,
    min_foothold: Vec<f32>,
    present_me: Vec<u32>,
    present_foe: Vec<u32>,
    idle_me: Vec<u32>,
    soft_cap: Vec<u32>,
    parked_ratio: Vec<f32>,
    next_owner: Vec<Option<Side>>,
    capture_eta: Vec<Option<u64>>,
    marginal: Vec<u64>, // marginal_ticks_saved for target id (from is ignored in tests)
    force_eff: Vec<Option<u32>>,
    incoming_me: Vec<u32>,
    returning: Vec<u32>,
    transit: u64, // uniform transit cost in ticks
}

impl ProgView {
    /// All-defaults builder for `n` positions on a unit-spaced line; the caller mutates fields.
    fn new(rows: &[(PosOwner, u32, u32)]) -> ProgView {
        let n = rows.len();
        let infos = rows
            .iter()
            .enumerate()
            .map(|(i, &(owner, my, en))| PositionInfo {
                id: i,
                owner,
                my_ships: my,
                enemy_ships: en,
                contested: (owner == PosOwner::Me || my > 0) && (owner == PosOwner::Enemy || en > 0),
            })
            .collect();
        ProgView {
            infos,
            xs: (0..n).map(|i| i as f32).collect(),
            resist: vec![0.0; n],
            min_foothold: vec![0.0; n],
            present_me: rows.iter().map(|&(_, m, _)| m).collect(),
            present_foe: rows.iter().map(|&(_, _, e)| e).collect(),
            idle_me: rows.iter().map(|&(_, m, _)| m).collect(),
            soft_cap: vec![30; n],
            parked_ratio: vec![0.0; n],
            next_owner: vec![None; n],
            capture_eta: vec![None; n],
            marginal: vec![0; n],
            force_eff: vec![Some(0); n],
            incoming_me: vec![0; n],
            returning: vec![0; n],
            transit: 5,
        }
    }
}

impl PositionView for ProgView {
    fn len(&self) -> usize {
        self.infos.len()
    }
    fn info(&self, id: usize) -> PositionInfo {
        self.infos[id]
    }
    fn distance(&self, from: usize, to: usize) -> Option<f32> {
        Some((self.xs[from] - self.xs[to]).abs())
    }
    fn resistance(&self, id: usize) -> f32 {
        self.resist[id]
    }
    fn min_foothold_resistance(&self, id: usize) -> f32 {
        self.min_foothold[id]
    }
    fn present_count(&self, id: usize, side: Side) -> u32 {
        match side {
            Side::Me => self.present_me[id],
            Side::Foe => self.present_foe[id],
        }
    }
    fn idle_at(&self, id: usize, side: Side) -> u32 {
        match side {
            Side::Me => self.idle_me[id],
            Side::Foe => 0,
        }
    }
    fn soft_cap_at(&self, id: usize) -> u32 {
        self.soft_cap[id]
    }
    fn parked_ratio(&self, id: usize) -> f32 {
        self.parked_ratio[id]
    }
    fn transit_ticks(&self, _from: usize, _to: usize) -> Option<u64> {
        Some(self.transit)
    }
    fn capture_eta(&self, id: usize) -> Option<u64> {
        self.capture_eta[id]
    }
    fn projected_next_owner(&self, id: usize) -> Option<Side> {
        self.next_owner[id]
    }
    fn marginal_ticks_saved(&self, target: usize, _from: usize) -> u64 {
        self.marginal[target]
    }
    fn force_for_efficiency(&self, id: usize, _ratio: f32) -> Option<u32> {
        self.force_eff[id]
    }
    fn incoming_mine(&self, id: usize) -> u32 {
        self.incoming_me[id]
    }
    fn returning_owner_force(&self, id: usize) -> u32 {
        self.returning[id]
    }
}

// =====================================================================================
// SimpleColonizer
// =====================================================================================

#[test]
fn simple_colonizer_sizes_wave_to_resistance_and_fills_nearest_first() {
    // Home (id 0, 20 ships) with two neutral targets: near (id 1, resistance 50) and far
    // (id 2, resistance 100). SHIPS_PER_RES default 0.12 -> goals 6 and 12.
    let mut v = ProgView::new(&[
        (PosOwner::Me, 20, 0),
        (PosOwner::Neutral, 0, 0),
        (PosOwner::Neutral, 0, 0),
    ]);
    v.resist = vec![0.0, 50.0, 100.0];
    v.min_foothold = vec![0.0, 50.0, 100.0];
    let p = SimpleColonizerParams::default();
    let acts = simple_colonize(&v, &p);
    // It should fire wave(s) from the home; the FIRST (nearest) target is id 1.
    assert!(!acts.is_empty(), "simple colonizer should send a wave");
    let first = &acts[0];
    assert_eq!(first.from, 0);
    assert_eq!(first.to, 1, "nearest-first: the near target gets served first");
    assert_eq!(first.kind, GreedyKind::Wave);
    // The wave toward the near target is sized to its goal (0.12*50 = 6), capped by surplus.
    assert!(first.count <= 6 + 1, "wave sized ~to resistance goal, got {}", first.count);
}

#[test]
fn simple_colonizer_does_not_oversend_past_goal() {
    // Target already has enough committed in-flight to meet its goal -> no new wave to it.
    let mut v = ProgView::new(&[(PosOwner::Me, 20, 0), (PosOwner::Neutral, 0, 0)]);
    v.resist = vec![0.0, 50.0]; // goal = ceil(0.12*50) = 6
    v.min_foothold = vec![0.0, 50.0];
    v.incoming_me = vec![0, 6]; // already 6 inbound == goal
    let acts = simple_colonize(&v, &SimpleColonizerParams::default());
    assert!(
        acts.iter().all(|a| a.to != 1),
        "no over-send: a target whose committed (in-flight) already meets the goal gets nothing"
    );
}

#[test]
fn simple_colonizer_keeps_the_retreat_reflex_and_no_rear_guard() {
    // A losing fight at id 0 (mine 4 vs enemy 9) retreats to the safe owned rear (id 2); the
    // remaining home keeps only the floor (the seam: no dedicated rear guard above the floor).
    let mut v = ProgView::new(&[
        (PosOwner::Me, 4, 9),
        (PosOwner::Neutral, 0, 0),
        (PosOwner::Me, 3, 0),
    ]);
    v.resist = vec![0.0, 40.0, 0.0];
    v.min_foothold = vec![0.0, 40.0, 0.0];
    let acts = simple_colonize(&v, &SimpleColonizerParams::default());
    let r = acts.iter().find(|a| a.from == 0).expect("the losing position retreats");
    assert_eq!(r.kind, GreedyKind::Retreat);
    assert_eq!(r.to, 2, "retreat to the nearest safe owned rear");
}

// =====================================================================================
// Colonize — marginal-value rule
// =====================================================================================

#[test]
fn colonize_sends_only_while_the_marginal_ship_pays() {
    // Two neutral targets. id 1: marginal_ticks_saved 20 > transit 5 -> PAYS, send.
    //                       id 2: marginal_ticks_saved 2  < transit 5 -> does NOT pay, skip.
    let mut v = ProgView::new(&[
        (PosOwner::Me, 20, 0),
        (PosOwner::Neutral, 0, 0),
        (PosOwner::Neutral, 0, 0),
    ]);
    v.resist = vec![0.0, 60.0, 60.0];
    v.marginal = vec![0, 20, 2];
    v.transit = 5;
    // Make id 2 the nearest so the program evaluates it first, proving the pay-rule (not distance)
    // gates the send.
    v.xs = vec![0.0, 5.0, 1.0];
    let acts = colonize(&v, &ColonizeParams::default());
    // The nearest is id 2 (does not pay); the program should fall through to the front-runner id 1
    // (pays) rather than send to id 2.
    assert!(acts.iter().any(|a| a.to == 1), "sends to the paying front, got {acts:?}");
    assert!(
        acts.iter().all(|a| a.to != 2),
        "never sends to a target where the marginal ship does not pay its transit, got {acts:?}"
    );
}

#[test]
fn colonize_skips_targets_the_enemy_takes_first() {
    // A neutral the projection says the enemy captures first is not a colony candidate.
    let mut v = ProgView::new(&[(PosOwner::Me, 20, 0), (PosOwner::Neutral, 0, 0)]);
    v.resist = vec![0.0, 60.0];
    v.marginal = vec![0, 50];
    v.next_owner = vec![None, Some(Side::Foe)]; // enemy takes id 1 first
    let acts = colonize(&v, &ColonizeParams::default());
    assert!(acts.is_empty(), "colonize abandons a neutral the enemy will take first");
}

// =====================================================================================
// Attack — siege math, sustain, denial
// =====================================================================================

#[test]
fn attack_pre_contact_develops_like_colonize() {
    // No foe presence anywhere -> plan_siege is None -> Attack develops (colonizes) instead of idling.
    let mut v = ProgView::new(&[(PosOwner::Me, 20, 0), (PosOwner::Neutral, 0, 0)]);
    v.resist = vec![0.0, 60.0];
    v.marginal = vec![0, 50];
    v.transit = 5;
    let acts = attack(&v, &AttackParams::default());
    assert!(!acts.is_empty(), "pre-contact attack develops rather than idling");
    assert!(acts.iter().any(|a| a.to == 1));
}

#[test]
fn attack_spearhead_holds_until_it_can_win_efficiently_then_commits() {
    // Staging (id 0) vs an enemy target (id 1). force_for_efficiency(target) = 15.
    // First: staging has 10 (<15) and no cap pressure -> HOLD (no commit to the target).
    let mut v = ProgView::new(&[(PosOwner::Me, 10, 0), (PosOwner::Enemy, 0, 5)]);
    v.force_eff = vec![Some(0), Some(15)];
    v.returning = vec![0, 0];
    let acts = attack(&v, &AttackParams::default());
    assert!(
        acts.iter().all(|a| a.to != 1),
        "thin spearhead HOLDS (does not feed the target piecemeal), got {acts:?}"
    );

    // Now give the staging enough to win efficiently (20 >= 15) and out-last heal -> COMMIT.
    v.infos[0].my_ships = 20;
    v.present_me[0] = 20;
    v.idle_me[0] = 20;
    let acts2 = attack(&v, &AttackParams::default());
    let spear = acts2.iter().find(|a| a.from == 0).expect("staging acts");
    assert_eq!(spear.to, 1, "ready spearhead commits at the target");
    assert_eq!(spear.kind, GreedyKind::Wave);
}

#[test]
fn attack_denies_a_productive_foe_sub_when_production_superior() {
    // I own {0,2} (2) vs enemy {1,3} (2) on the line 0-1-2-3, but I have 4 owned positions... no:
    // give myself a 3rd owned position so I am production-superior (3 owned vs 2 foe). The cheapest
    // siege target is id 1 (2 defenders) staged from the adjacent id 0; id 3 (8 defenders, far) is
    // a productive foe sub the surplus from id 2 parks a cheap DENIAL detachment on.
    let mut v = ProgView::new(&[
        (PosOwner::Me, 20, 0),   // 0: staging (adjacent to the cheap target id 1)
        (PosOwner::Enemy, 0, 2), // 1: the CHEAP siege target (fewest defenders, nearest)
        (PosOwner::Me, 20, 0),   // 2: another owned source -> does the denial
        (PosOwner::Enemy, 0, 8), // 3: a productive but well-defended foe sub -> deny, don't siege
        (PosOwner::Me, 5, 0),    // 4: a third owned position (makes me production-superior 3 vs 2)
    ]);
    v.force_eff = vec![Some(0), Some(4), Some(0), Some(40), Some(0)];
    // Keep the staging from being "ready" so the test focuses on the denial branch from id 2.
    v.returning = vec![0, 99, 0, 0, 0]; // huge heal at the target -> staging not ready, holds
    let acts = attack(&v, &AttackParams::default());
    assert!(
        acts.iter().any(|a| a.kind == GreedyKind::Deny && a.to == 3),
        "production-superior attack parks a denial detachment on a productive foe sub, got {acts:?}"
    );
}

// =====================================================================================
// Defend — efficient defense, reinforce-first-fall, colonize the surplus
// =====================================================================================

#[test]
fn defend_masses_to_the_efficient_force_not_infinite() {
    // One threatened owned sub (id 0, contested) needs force_for_efficiency = 12 but only has 4
    // present; a safe rear (id 1) reinforces with exactly the deficit (8), NOT its whole stack.
    let mut v = ProgView::new(&[(PosOwner::Me, 4, 3), (PosOwner::Me, 30, 0)]);
    v.present_me = vec![4, 30];
    v.force_eff = vec![Some(12), Some(0)];
    let acts = defend(&v, &DefendParams::default());
    let reinf = acts.iter().find(|a| a.to == 0).expect("defend reinforces the threatened sub");
    assert_eq!(reinf.from, 1);
    assert_eq!(reinf.count, 8, "masses to the EFFICIENT deficit (12-4), not the whole rear stack");
}

#[test]
fn defend_reinforces_the_sub_that_falls_first() {
    // Two threatened owned subs: id 0 falls at tick 40, id 2 falls at tick 20 (sooner). Defend
    // must reinforce id 2 (first to fall), from the rear id 1.
    let mut v = ProgView::new(&[
        (PosOwner::Me, 5, 0),
        (PosOwner::Me, 30, 0),
        (PosOwner::Me, 5, 0),
    ]);
    v.present_me = vec![5, 30, 5];
    v.capture_eta = vec![Some(40), None, Some(20)];
    v.next_owner = vec![Some(Side::Foe), None, Some(Side::Foe)];
    v.force_eff = vec![Some(10), Some(0), Some(10)];
    let acts = defend(&v, &DefendParams::default());
    let reinf = acts.iter().find(|a| a.kind == GreedyKind::Wave).expect("a reinforcement fires");
    assert_eq!(reinf.to, 2, "reinforce the sub the projection says falls FIRST");
}

#[test]
fn defend_colonizes_only_the_genuine_cap_surplus() {
    // Nothing threatened. A safe owned planet BELOW the soft cap holds its reserve (no colonize);
    // one OVER the cap spends the surplus on a neutral.
    let mut v = ProgView::new(&[
        (PosOwner::Me, 10, 0),    // 0: below cap -> hold (turtle)
        (PosOwner::Neutral, 0, 0), // 1: a neutral to grab
        (PosOwner::Me, 40, 0),    // 2: over cap -> spend surplus
    ]);
    v.soft_cap = vec![30, 30, 30];
    v.parked_ratio = vec![0.3, 0.0, 1.2]; // id 2 is over the cap
    v.resist = vec![0.0, 50.0, 0.0];
    let acts = defend(&v, &DefendParams::default());
    assert!(
        acts.iter().any(|a| a.from == 2 && a.to == 1),
        "the over-cap planet colonizes its genuine surplus, got {acts:?}"
    );
    assert!(
        acts.iter().all(|a| a.from != 0),
        "the below-cap planet holds its healing reserve (does not colonize), got {acts:?}"
    );
}

// =====================================================================================
// Invariant + handle smoke test.
// =====================================================================================

#[test]
fn automaton_handle_dispatches_each_program() {
    let mut v = ProgView::new(&[(PosOwner::Me, 20, 0), (PosOwner::Neutral, 0, 0)]);
    v.resist = vec![0.0, 60.0];
    v.min_foothold = vec![0.0, 60.0];
    v.marginal = vec![0, 50];
    for auto in [
        Automaton::SimpleColonizer(SimpleColonizerParams::default()),
        Automaton::Colonize(ColonizeParams::default()),
        Automaton::Attack(AttackParams::default()),
        Automaton::Defend(DefendParams::default()),
    ] {
        let _ = auto.decide(&v); // must not panic; name must be non-empty.
        assert!(!auto.name().is_empty());
    }
}
