//! The **Opportunist** — the first campaign brain that *models the player* ([`crate::Roster::Opportunist`]).
//!
//! It plays the frozen Simple v1 colonizer for its economy, and layers one reading of its
//! opponent on top: **it watches where the opponent's ships are, and strikes a sub whose
//! garrison has been sent away.**
//!
//! Per decision tick:
//!
//! 1. **Read the opponent.** For every foe-owned sub, the *defence* is the foe ships idle there
//!    plus foe ships already inbound to it (a snapshot — reinforcements not yet launched are
//!    invisible). The foe's *commitment* — the share of its fleet currently in flight — is
//!    tracked as a moving average: an opponent that keeps throwing its fleet across the board
//!    is judged softer.
//! 2. **Strike** (at most once per [`STRIKE_COOLDOWN`] decisions): pick the foe sub whose defence it
//!    can beat cheaply — needing `k · defence + margin` ships, with `k` falling from
//!    [`K_CAUTIOUS`] toward [`K_BOLD`] as the opponent's commitment rises — from idle ships
//!    within [`STRIKE_REACH`] of it, keeping a [`garrison_floor`] at home. Productive targets
//!    are preferred.
//! 3. Otherwise it **plays Simple** (expansion and funding).
//!
//! Identity: it punishes an opponent who over-commits. Blind spots: it judges by a snapshot (a
//! defender who reinforces *late* — or a bait sub with a cheap garrison next to a fortress —
//! outfights the strike it invited), and the ships it sends leave its own subs thin.
//!
//! The brain draws no randomness: it is a pure function of the observed interior and its own
//! small state, so replays stay bit-identical.

use layer1::{Faction, Interior, SimParams};

use crate::simple::{SimpleController, SimpleVersion};

/// Minimum DECISIONS between strikes (the game re-plans every 5 reference ticks, so 24 decisions
/// is 120 reference ticks at any tick rate): a strike is an event, not a stream.
pub const STRIKE_COOLDOWN: u64 = 24;
/// Force ratio demanded against a fully-garrisoned opponent (square-law: 2x ships ≈ 4x power).
pub const K_CAUTIOUS: f32 = 1.8;
/// Force ratio demanded against an opponent whose whole fleet is in flight.
pub const K_BOLD: f32 = 1.0;
/// Flat ships added on top of `k · defence`.
pub const STRIKE_MARGIN: f32 = 6.0;
/// Only ships within this many world units of the target join a strike.
pub const STRIKE_REACH: f32 = 90.0;
/// Weight of the newest observation in the commitment moving average.
const COMMIT_ALPHA: f32 = 0.15;

/// Ships a sub keeps at home when it contributes to a strike.
pub fn garrison_floor(storage_capacity: usize) -> usize {
    (storage_capacity / 6).max(3)
}

/// The force ratio demanded at a given observed opponent commitment in `[0, 1]`.
pub fn required_ratio(commitment: f32) -> f32 {
    K_CAUTIOUS + (K_BOLD - K_CAUTIOUS) * commitment.clamp(0.0, 1.0)
}

/// The stateful Opportunist driver; one per enemy seat.
#[derive(Clone)]
pub struct OpportunistController {
    pub seat: Faction,
    base: SimpleController,
    /// Decisions taken so far (the clock: the game's tick rate varies, its decision cadence does not).
    decisions: u64,
    last_strike: Option<u64>,
    commitment: f32,
}

impl OpportunistController {
    pub fn new(seat: Faction) -> OpportunistController {
        OpportunistController {
            seat,
            base: SimpleController::new(seat, SimpleVersion::V1),
            decisions: 0,
            last_strike: None,
            commitment: 0.0,
        }
    }

    /// The moving-average share of the opponents' fleet found in flight (for tests / debugging).
    pub fn commitment(&self) -> f32 {
        self.commitment
    }

    /// Run one decision tick, applying the orders. Returns the ships moved.
    pub fn decide_and_apply(&mut self, st: &mut Interior, sp: &SimParams) -> usize {
        self.decisions += 1;
        let view = Observation::of(st, self.seat);
        if view.foe_ships > 0 {
            let seen = view.foe_in_flight as f32 / view.foe_ships as f32;
            self.commitment += COMMIT_ALPHA * (seen - self.commitment);
        }
        let ready = self.last_strike.map_or(true, |t| self.decisions >= t + STRIKE_COOLDOWN);
        if ready {
            if let Some(plan) = self.plan_strike(st, &view) {
                let moved: usize = plan
                    .sources
                    .iter()
                    .map(|&(src, n)| st.issue_order_count(src, plan.target, n, self.seat))
                    .sum();
                if moved > 0 {
                    self.last_strike = Some(self.decisions);
                    return moved;
                }
            }
        }
        self.base.decide_and_apply(st, sp)
    }

    /// The cheapest worthwhile strike, if any foe sub is beatable right now.
    fn plan_strike(&self, st: &Interior, view: &Observation) -> Option<StrikePlan> {
        let ratio = required_ratio(self.commitment);
        let mut best: Option<(f32, StrikePlan)> = None;
        for (target, sub) in st.subs.iter().enumerate() {
            if !sub.owner.is_foe_of(self.seat) {
                continue;
            }
            let need = (ratio * view.foe_at[target] as f32 + STRIKE_MARGIN).ceil() as usize;
            let Some(sources) = self.gather(st, view, target, need) else { continue };
            let score = need as f32 - 10.0 * sub.production as f32;
            if best.as_ref().map_or(true, |(s, _)| score < *s) {
                best = Some((score, StrikePlan { target, sources }));
            }
        }
        best.map(|(_, plan)| plan)
    }

    /// Pick `(source, count)` pairs, nearest first, totalling `need` from idle ships in reach;
    /// `None` if the reachable surplus is too small.
    fn gather(
        &self,
        st: &Interior,
        view: &Observation,
        target: usize,
        need: usize,
    ) -> Option<Vec<(usize, usize)>> {
        let mut candidates: Vec<(f32, usize, usize)> = st
            .subs
            .iter()
            .enumerate()
            .filter(|(_, s)| s.owner == self.seat)
            .filter_map(|(i, s)| {
                let spare = view.my_idle[i].saturating_sub(garrison_floor(s.storage_capacity as usize));
                let d = s.pos.dist(st.subs[target].pos);
                (spare > 0 && d <= STRIKE_REACH).then_some((d, i, spare))
            })
            .collect();
        candidates.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap().then(a.1.cmp(&b.1)));
        let mut left = need;
        let mut picked = Vec::new();
        for (_, src, spare) in candidates {
            let n = spare.min(left);
            picked.push((src, n));
            left -= n;
            if left == 0 {
                return Some(picked);
            }
        }
        None
    }
}

struct StrikePlan {
    target: usize,
    sources: Vec<(usize, usize)>,
}

/// One pass of tallies over the interior.
struct Observation {
    /// My idle ships per sub.
    my_idle: Vec<usize>,
    /// Opponent ships defending each sub: idle there + inbound to it.
    foe_at: Vec<usize>,
    foe_ships: usize,
    foe_in_flight: usize,
}

impl Observation {
    fn of(st: &Interior, me: Faction) -> Observation {
        let n = st.subs.len();
        let mut o = Observation {
            my_idle: vec![0; n],
            foe_at: vec![0; n],
            foe_ships: 0,
            foe_in_flight: 0,
        };
        for sh in st.ships.iter().filter(|s| s.alive && s.drift_remaining == 0) {
            if sh.faction == me {
                if sh.target.is_none() && sh.home < n {
                    o.my_idle[sh.home] += 1;
                }
            } else if sh.faction.is_foe_of(me) {
                o.foe_ships += 1;
                if sh.target.is_some() {
                    o.foe_in_flight += 1;
                }
                let at = sh.target.unwrap_or(sh.home);
                if at < n {
                    o.foe_at[at] += 1;
                }
            }
        }
        o
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use layer1::{SubStructure, Vec2};

    /// Two of mine and one of the player's, all within reach of each other.
    fn board(player_garrison: usize, my_stack: usize) -> (Interior, usize, usize) {
        let mut st = Interior::new(1);
        let mine = st.add_sub(SubStructure::new(Vec2::new(0.0, 0.0), 4.0, Faction::Ai(0)));
        let theirs = st.add_sub(SubStructure::new(Vec2::new(40.0, 0.0), 4.0, Faction::Player));
        for _ in 0..my_stack {
            st.spawn_ship(Faction::Ai(0), mine);
        }
        for _ in 0..player_garrison {
            st.spawn_ship(Faction::Player, theirs);
        }
        (st, mine, theirs)
    }

    fn opp() -> OpportunistController {
        OpportunistController::new(Faction::Ai(0))
    }

    #[test]
    fn strikes_a_thinly_garrisoned_foe_sub() {
        let (mut st, _, theirs) = board(4, 40);
        let moved = opp().decide_and_apply(&mut st, &SimParams::default());
        assert!(moved > 0);
        let inbound = st
            .ships
            .iter()
            .filter(|s| s.faction == Faction::Ai(0) && s.target == Some(theirs))
            .count();
        assert!(inbound >= 4, "expected a strike on the exposed sub, got {inbound} inbound");
    }

    #[test]
    fn holds_off_a_strongly_garrisoned_foe_sub() {
        let (mut st, _, theirs) = board(40, 40);
        opp().decide_and_apply(&mut st, &SimParams::default());
        let inbound = st.ships.iter().filter(|s| s.target == Some(theirs) && s.faction == Faction::Ai(0)).count();
        assert_eq!(inbound, 0, "40 defenders vs 40 attackers must not draw a strike");
    }

    #[test]
    fn inbound_reinforcements_count_as_defence() {
        // Ten idle + fifteen in flight toward the sub: 25 defenders, more than 40 can cheaply beat.
        let (mut st, _, theirs) = board(10, 40);
        let elsewhere = st.add_sub(SubStructure::new(Vec2::new(40.0, 30.0), 4.0, Faction::Player));
        for _ in 0..15 {
            st.spawn_ship(Faction::Player, elsewhere);
        }
        st.issue_order_count(elsewhere, theirs, 15, Faction::Player);
        opp().decide_and_apply(&mut st, &SimParams::default());
        let inbound = st.ships.iter().filter(|s| s.target == Some(theirs) && s.faction == Faction::Ai(0)).count();
        assert_eq!(inbound, 0);
    }

    #[test]
    fn keeps_a_home_garrison_and_rests_between_strikes() {
        let (mut st, mine, theirs) = board(2, 30);
        let mut c = opp();
        c.decide_and_apply(&mut st, &SimParams::default());
        assert!(st.idle_count_at(mine, Faction::Ai(0)) >= garrison_floor(st.subs[mine].storage_capacity as usize));
        // A second strike request inside the cooldown is not honoured.
        let before = st.ships.iter().filter(|s| s.faction == Faction::Ai(0) && s.target == Some(theirs)).count();
        c.decide_and_apply(&mut st, &SimParams::default());
        let after = st.ships.iter().filter(|s| s.faction == Faction::Ai(0) && s.target == Some(theirs)).count();
        assert_eq!(before, after);
    }

    #[test]
    fn commitment_rises_when_the_foe_fleet_is_in_flight_and_lowers_the_bar() {
        let (mut st, _, theirs) = board(30, 40);
        let far = st.add_sub(SubStructure::new(Vec2::new(200.0, 0.0), 4.0, Faction::Neutral));
        st.issue_order_count(theirs, far, 30, Faction::Player);
        let mut c = opp();
        for _ in 0..30 {
            c.decide_and_apply(&mut st, &SimParams::default());
        }
        assert!(c.commitment() > 0.5, "commitment {}", c.commitment());
        assert!(required_ratio(c.commitment()) < required_ratio(0.0));
    }

    #[test]
    fn plays_simple_when_there_is_nothing_to_strike() {
        let mut st = Interior::new(1);
        let h = st.add_sub(SubStructure::new(Vec2::new(0.0, 0.0), 4.0, Faction::Ai(0)));
        let _n = st.add_sub(SubStructure::new(Vec2::new(30.0, 0.0), 4.0, Faction::Neutral));
        for _ in 0..40 {
            st.spawn_ship(Faction::Ai(0), h);
        }
        let mut c = opp();
        let moved: usize = (0..20).map(|_| c.decide_and_apply(&mut st, &SimParams::default())).sum();
        assert!(moved > 0, "with no foe to read it should colonize like Simple");
    }
}
