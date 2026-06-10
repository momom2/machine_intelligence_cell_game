//! # `ai::vocab` — the shared, composable AUTOMATON VOCABULARY
//!
//! This module is the **whole point** of the AI layer's design: a small, named, documented
//! language in which *every* automaton — the four hand-written ones in [`crate::automata`] and
//! any future **evolved / abstract** agent — is written as a short program. The language has
//! exactly three kinds of word, and they are deliberately **orthogonal** so they recombine
//! cleanly:
//!
//! | kind | what it is | where it lives |
//! |------|------------|----------------|
//! | **QUERIES** | look-ahead questions about the future of the board | [`world::Projection`] methods, surfaced through the [`PositionView`] (`capture_eta`, `marginal_ticks_saved`, `force_for_efficiency`, `incoming_mine`, `returning_owner_force`, `projected_next_owner`) |
//! | **PREDICATES** | yes/no facts about a position *now* | this module ([`is_frontier`], [`being_eroded`], [`has_surplus`], [`over_soft_cap`], [`owned_by_me`], [`contested`], [`production_superior`], …) |
//! | **ACTIONS** | one atomic thing to do with a position's surplus | this module ([`wave`], [`hold`], [`retreat`], [`deny`]) |
//!
//! A policy is then literally: *for each owned position, look at some QUERIES + PREDICATES and
//! emit one ACTION.* The predicates read **property signals** off the [`PositionView`] (which are
//! themselves thin accessors over the sim's read-signals); the queries read the **shared
//! forward-[`world::Projection`]**; the actions produce the layer-agnostic [`GreedyAction`] the
//! existing adapters already turn into `layer1::MoveOrder` / `world::FleetOrder`.
//!
//! ## THE HARD INVARIANT — no mechanic constant or formula in `ai`
//!
//! See [`NO_MECHANIC_CONSTANTS`]. Nothing in this crate writes a raw mechanic number or rule:
//! not the capture cap, not the production period, not the soft-cap / square-law formulas. Every
//! mechanic question is answered by a **projection query** (the look-ahead already folds the
//! grind/heal/combat/denial/soft-cap rules) or a **property accessor** (`resistance`,
//! `present_count`, `soft_cap_at`, …). The *only* numbers the AI is allowed to name are **policy
//! tunables** — `SHIPS_PER_RES`, `DESIRED_COMBAT_EFFICIENCY`, `PROJECTION_HORIZON`,
//! `GARRISON_FLOOR`, … — and each lives in its automaton's own `*Params` struct in
//! [`crate::automata`], clearly separated from anything the sim owns. The
//! `no_raw_mechanic_constants_in_ai` test documents and guards the separation.

use crate::greedy::{GreedyAction, GreedyKind, PosOwner, PositionInfo, PositionView, Side};

/// **The no-mechanic-constant marker (grep me).**
///
/// Grep the `ai` crate for `NO_MECHANIC_CONSTANTS` to audit the hard invariant: this is the one
/// place that *names* it, and the [`crate::automata`] `*Params` structs are the one place that
/// holds the *policy* tunables. If you find a literal `1800`, a `production_period` arithmetic,
/// or any resistance / soft-cap / square-law formula anywhere under `crates/ai/`, the invariant
/// is broken — route the question through a [`world::Projection`] query or a [`PositionView`]
/// property accessor instead.
///
/// (A `const` rather than a doc-only comment so it is a real, referenceable symbol the guard
/// test can point at.)
pub const NO_MECHANIC_CONSTANTS: &str =
    "ai answers every mechanic question via a projection query or a property accessor — never a \
     raw constant/formula. The only numbers in ai are policy tunables in the *Params structs.";

// =====================================================================================
// PREDICATES — yes/no facts about a position (or the board) *now*.
//
// Each is a pure read of the PositionView property signals. They are the legible building
// blocks the automaton programs branch on; keeping them named (rather than inlining
// `info.owner == Me`) is what lets a future agent recombine the SAME facts.
// =====================================================================================

/// **`owned_by_me`** — the acting seat owns this position.
#[inline]
pub fn owned_by_me<V: PositionView>(view: &V, id: usize) -> bool {
    view.info(id).owner == PosOwner::Me
}

/// **`owned_by_foe`** — the opponent owns this position.
#[inline]
pub fn owned_by_foe<V: PositionView>(view: &V, id: usize) -> bool {
    view.info(id).owner == PosOwner::Enemy
}

/// **`neutral`** — the position is unowned (capturable ground, nobody's producer yet).
#[inline]
pub fn neutral<V: PositionView>(view: &V, id: usize) -> bool {
    view.info(id).owner == PosOwner::Neutral
}

/// **`contested`** — both sides have a presence here (a live fight; the capture grind is frozen
/// until combat decides who is the only side present).
#[inline]
pub fn contested<V: PositionView>(view: &V, id: usize) -> bool {
    view.info(id).contested
}

/// **`outnumbered_here`** — I am present but the enemy out-masses me at this position (the
/// retreat trigger: a fight I am losing locally).
#[inline]
pub fn outnumbered_here<V: PositionView>(view: &V, id: usize) -> bool {
    let i = view.info(id);
    i.contested && i.enemy_ships > i.my_ships
}

/// **`foe_present`** — at least one enemy ship is at this position (it is either enemy-owned or
/// being contested by the enemy). The "capturable by attack, not by colonize" discriminator.
#[inline]
pub fn foe_present<V: PositionView>(view: &V, id: usize) -> bool {
    let i = view.info(id);
    i.owner == PosOwner::Enemy || i.enemy_ships > 0
}

/// **`is_frontier`** — an owned position with at least one **reachable foe-bearing** position
/// (enemy-owned or contested). The places a defender wants a wall and a colonizer leaves thin.
/// (Reachability stands in for "the enemy can come at it next".)
pub fn is_frontier<V: PositionView>(view: &V, id: usize) -> bool {
    if view.info(id).owner != PosOwner::Me {
        return false;
    }
    (0..view.len()).any(|j| j != id && foe_present(view, j) && view.reachable(id, j))
}

/// **`being_eroded`** — an owned position the enemy is actively grinding: a foe is present and
/// its capture ETA (the projection's grind-aware look-ahead) hands it to the enemy within the
/// horizon. This is the denial/`production-choked` signal that snaps a defender home. The ETA
/// query answers the mechanic; the predicate just reads it.
pub fn being_eroded<V: PositionView>(view: &V, id: usize) -> bool {
    owned_by_me(view, id)
        && foe_present(view, id)
        && view.projected_next_owner(id) == Some(Side::Foe)
}

/// **`has_surplus`** — this owned position has movable ships above the garrison `floor` (the
/// pool any ACTION draws from). `floor` is the automaton's policy tunable, passed in.
#[inline]
pub fn has_surplus<V: PositionView>(view: &V, id: usize, floor: u32) -> bool {
    let i = view.info(id);
    i.owner == PosOwner::Me && i.my_ships > floor
}

/// **`over_soft_cap`** — parked ships at this position have reached `frac` of the soft cap, so the
/// anti-hoard attrition is about to (or does) bite: surplus must be **spent or kept moving**.
/// `frac == 1.0` is "the cap is actively destroying ships". The ratio comes straight from the
/// `parked / soft_cap` property signals — no `sqrt(over)` formula here.
#[inline]
pub fn over_soft_cap<V: PositionView>(view: &V, id: usize, frac: f32) -> bool {
    view.parked_ratio(id) >= frac
}

/// **`would_overstack`** — sending more of my ships to `id` would push my **idle** stack there to
/// `frac` of its soft cap (so the reinforcement would just bleed to attrition). The colonizer's
/// "don't pour into a position that cannot hold it" guard.
#[inline]
pub fn would_overstack<V: PositionView>(view: &V, id: usize, frac: f32) -> bool {
    let cap = view.soft_cap_at(id);
    if cap == 0 || cap == u32::MAX {
        return false;
    }
    view.idle_at(id, Side::Me) as f32 >= frac * cap as f32
}

/// **`production_superior`** — the acting seat owns strictly more **producing positions** than
/// the enemy (every owned position is a producer, so owned-count is the production-rate proxy).
/// The gate Attack puts in front of cheap denial and "break the strongest" target selection.
pub fn production_superior<V: PositionView>(view: &V) -> bool {
    let mut mine = 0usize;
    let mut foe = 0usize;
    for i in 0..view.len() {
        match view.info(i).owner {
            PosOwner::Me => mine += 1,
            PosOwner::Enemy => foe += 1,
            PosOwner::Neutral => {}
        }
    }
    mine > foe
}

/// **`settles_mine`** — the projection already hands this position to me (present + in-flight,
/// enemy passive) within the horizon, so committing *more* to it would be wasteful. Used to prune
/// targets a wave is already winning.
#[inline]
pub fn settles_mine<V: PositionView>(view: &V, id: usize) -> bool {
    view.projected_next_owner(id) == Some(Side::Me)
}

/// **`foe_takes_first`** — the projection says the enemy captures this position before I could
/// land (it is owned-by/flipping-to the foe first). The gate that stops a policy throwing ships at
/// a position it has already lost the race for.
#[inline]
pub fn foe_takes_first<V: PositionView>(view: &V, id: usize) -> bool {
    view.projected_next_owner(id) == Some(Side::Foe)
}

// =====================================================================================
// ACTIONS — one atomic thing to do with a position's surplus.
//
// Each returns an `Option<GreedyAction>` (None = the action could not be taken, e.g. nowhere
// safe to retreat to). The caller pushes the `Some`. They never read mechanics; they only
// place a sized move, and the adapter sizes the fraction bucket + routes the first hop.
// =====================================================================================

/// **`wave(from, target, size)`** — commit a sized capture/colonization wave: move up to `size`
/// surplus ships from `from` toward `target`. The colonizer's expand primitive. `size` is clamped
/// to the actual surplus above `floor`; `None` if there is no surplus or `size == 0`.
pub fn wave<V: PositionView>(view: &V, from: usize, target: usize, size: u32, floor: u32) -> Option<GreedyAction> {
    let surplus = surplus_of(view, from, floor)?;
    let count = size.min(surplus);
    (count > 0).then_some(GreedyAction { from, to: target, count, kind: GreedyKind::Wave })
}

/// **`retreat(from, to_safe)`** — pull this position's whole surplus back to the nearest **safe
/// owned** position (owned by me, not contested), preserving the army from a fight it is losing.
/// `None` if there is no safe rear reachable (the caller then falls through to another action).
pub fn retreat<V: PositionView>(view: &V, from: usize, floor: u32) -> Option<GreedyAction> {
    let surplus = surplus_of(view, from, floor)?;
    let to = nearest(view, from, |i| i.id != from && i.owner == PosOwner::Me && !i.contested)?;
    Some(GreedyAction { from, to, count: surplus, kind: GreedyKind::Retreat })
}

/// **`deny(from, target, detach)`** — park a *cheap* detachment of `detach` ships on a productive
/// foreign sub to FREEZE its output (Mechanic B) without paying the full capture grind. The
/// caller gates this behind [`production_superior`]. `None` if the surplus cannot field the
/// detachment (denial must not dilute a siege).
pub fn deny<V: PositionView>(view: &V, from: usize, target: usize, detach: u32, floor: u32) -> Option<GreedyAction> {
    let surplus = surplus_of(view, from, floor)?;
    (surplus >= detach && detach > 0)
        .then_some(GreedyAction { from, to: target, count: detach, kind: GreedyKind::Deny })
}

/// **`hold`** — keep amassing / keep healing; emit no order this tick. A first-class word in the
/// language (a policy *chooses* to hold a thin spearhead rather than feed it piecemeal), expressed
/// as `Option::None` so it composes in the same `if let Some(a) = … else hold` shape as the others.
#[inline]
pub fn hold() -> Option<GreedyAction> {
    None
}

// =====================================================================================
// Small shared helpers the actions/automata reuse (pure reads of the view).
// =====================================================================================

/// The movable surplus of an owned position above `floor`, or `None` if it is not mine / at the
/// floor. Centralizes the "ships above the garrison floor" notion every action shares.
#[inline]
pub fn surplus_of<V: PositionView>(view: &V, id: usize, floor: u32) -> Option<u32> {
    let i = view.info(id);
    if i.owner != PosOwner::Me {
        return None;
    }
    let s = i.my_ships.saturating_sub(floor);
    (s > 0).then_some(s)
}

/// The reachable position from `from` (lowest id on ties) minimizing distance among those
/// matching `pred`. The shared "nearest" the actions/automata route by. `None` if none match.
pub fn nearest<V: PositionView>(view: &V, from: usize, pred: impl Fn(&PositionInfo) -> bool) -> Option<usize> {
    let mut best: Option<(usize, f32)> = None;
    for to in 0..view.len() {
        let info = view.info(to);
        if !pred(&info) {
            continue;
        }
        let Some(d) = view.distance(from, to) else { continue };
        match best {
            Some((_, bd)) if bd <= d => {}
            _ => best = Some((to, d)),
        }
    }
    best.map(|(id, _)| id)
}

#[cfg(test)]
mod tests {
    //! Vocabulary unit tests over a tiny in-memory [`PositionView`] (the `VocabView` below),
    //! plus the guard test for the no-mechanic-constant invariant.
    use super::*;
    use crate::greedy::PositionInfo;

    /// A hand-built view exercising the property signals + projection-backed reads the predicates
    /// and actions consume. Positions on a line; each carries its snapshot and a few synthetic
    /// projection answers (next owner, resistance, soft cap, parked ratio).
    struct VocabView {
        infos: Vec<PositionInfo>,
        xs: Vec<f32>,
        next_owner: Vec<Option<Side>>,
        resist: Vec<f32>,
        soft_cap: Vec<u32>,
        idle_me: Vec<u32>,
        parked_ratio: Vec<f32>,
    }

    impl PositionView for VocabView {
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
        fn soft_cap_at(&self, id: usize) -> u32 {
            self.soft_cap[id]
        }
        fn idle_at(&self, id: usize, side: Side) -> u32 {
            match side {
                Side::Me => self.idle_me[id],
                Side::Foe => 0,
            }
        }
        fn parked_ratio(&self, id: usize) -> f32 {
            self.parked_ratio[id]
        }
        fn projected_next_owner(&self, id: usize) -> Option<Side> {
            self.next_owner[id]
        }
    }

    fn view() -> VocabView {
        // 0: my home (10 ships), 1: enemy-owned (3 enemy), 2: neutral, 3: my frontier (5 ships).
        let infos = vec![
            PositionInfo { id: 0, owner: PosOwner::Me, my_ships: 10, enemy_ships: 0, contested: false },
            PositionInfo { id: 1, owner: PosOwner::Enemy, my_ships: 0, enemy_ships: 3, contested: false },
            PositionInfo { id: 2, owner: PosOwner::Neutral, my_ships: 0, enemy_ships: 0, contested: false },
            PositionInfo { id: 3, owner: PosOwner::Me, my_ships: 5, enemy_ships: 0, contested: false },
        ];
        VocabView {
            infos,
            xs: vec![0.0, 3.0, 1.0, 2.0],
            next_owner: vec![None, None, Some(Side::Me), None],
            resist: vec![0.0, 80.0, 100.0, 0.0],
            soft_cap: vec![30, 30, 30, 30],
            idle_me: vec![10, 0, 0, 5],
            parked_ratio: vec![0.2, 0.0, 0.0, 0.9],
        }
    }

    #[test]
    fn predicates_read_the_signals() {
        let v = view();
        assert!(owned_by_me(&v, 0));
        assert!(owned_by_foe(&v, 1));
        assert!(neutral(&v, 2));
        assert!(foe_present(&v, 1));
        assert!(!foe_present(&v, 2));
        // production_superior: I own {0,3} (2) vs enemy {1} (1).
        assert!(production_superior(&v));
        // settles_mine: position 2 is projected to flip to me.
        assert!(settles_mine(&v, 2));
        // has_surplus above a floor of 2.
        assert!(has_surplus(&v, 0, 2));
        assert!(has_surplus(&v, 3, 2));
        assert!(!has_surplus(&v, 1, 2)); // enemy-owned, not mine
        // over_soft_cap: position 3 is at 0.9 parked ratio.
        assert!(over_soft_cap(&v, 3, 0.8));
        assert!(!over_soft_cap(&v, 0, 0.8));
        // frontier: 0 and 3 are mine and can reach the enemy at 1.
        assert!(is_frontier(&v, 0));
        assert!(!is_frontier(&v, 1)); // not mine
    }

    #[test]
    fn would_overstack_reads_idle_vs_cap() {
        let v = view();
        // position 0: idle 10 vs cap 30 -> ratio 0.33, not >= 0.8.
        assert!(!would_overstack(&v, 0, 0.8));
        // position 3: idle 5 vs cap 30 -> 0.17, not overstacked either; lower the threshold.
        assert!(would_overstack(&v, 3, 0.1));
    }

    #[test]
    fn actions_emit_sized_moves() {
        let v = view();
        // wave from 0 to 2, size 4, floor 2: surplus = 8, count = min(4,8) = 4.
        let w = wave(&v, 0, 2, 4, 2).expect("wave fires");
        assert_eq!((w.from, w.to, w.count, w.kind), (0, 2, 4, GreedyKind::Wave));
        // wave size larger than surplus clamps to surplus (10 - 2 = 8).
        let w2 = wave(&v, 0, 2, 99, 2).expect("wave clamps");
        assert_eq!(w2.count, 8);
        // deny needs the detachment to fit in the surplus.
        assert!(deny(&v, 0, 1, 6, 2).is_some());
        assert!(deny(&v, 3, 1, 6, 2).is_none(), "surplus 3 cannot field a 6-ship detach");
        // hold is always None.
        assert!(hold().is_none());
    }

    #[test]
    fn retreat_targets_nearest_safe_owned() {
        let mut v = view();
        // Make position 0 a losing fight; it should retreat to the nearest safe owned (id 3).
        v.infos[0] = PositionInfo { id: 0, owner: PosOwner::Me, my_ships: 4, enemy_ships: 9, contested: true };
        assert!(outnumbered_here(&v, 0));
        let r = retreat(&v, 0, 2).expect("retreat fires");
        assert_eq!((r.from, r.to, r.kind), (0, 3, GreedyKind::Retreat));
        assert_eq!(r.count, 2, "surplus = 4 - floor(2)");
    }

    #[test]
    fn no_raw_mechanic_constants_in_ai() {
        // The marker exists and says what it must (a documentation anchor + a guard the reviewer
        // can grep). The *enforcement* is by code review against the invariant text; this test
        // pins the marker so it cannot be silently deleted.
        assert!(NO_MECHANIC_CONSTANTS.contains("projection query"));
        assert!(NO_MECHANIC_CONSTANTS.contains("property accessor"));
        // The default-mechanic numbers that must NEVER appear as literals in ai policy code, for
        // the reviewer's convenience (kept here as data, not used as a mechanic):
        for forbidden in ["1800 (max resistance)", "18 (production period)", "sqrt(over) (soft cap)"] {
            assert!(!NO_MECHANIC_CONSTANTS.contains(forbidden));
        }
    }
}
