//! The layer-agnostic **GREEDY** tactical policy — the project owner's exact spec,
//! implemented **once** against an abstract position view and then adapted to *both* layers.
//!
//! # Why an abstract view
//!
//! The greedy rule the project owner specified is the same whether the "positions" are a
//! single planet's **sub-structures** (Layer 1) or the **planets** of the whole `World`
//! (Layer 2). Only two things differ between the layers: what a *position* is and how
//! *distance* between positions is measured. So the decision logic lives here, over a tiny
//! [`PositionView`] trait, and the two adapters in [`crate::adapters`] supply (a) the per-
//! position snapshot and (b) the distance metric, then translate the abstract
//! [`GreedyAction`]s back into concrete `layer1::MoveOrder`s / `world::FleetOrder`s.
//!
//! # The policy (exactly as specified)
//!
//! Keep a small **garrison floor** on every owned position; ships above it are **surplus**.
//! For each owned position **with surplus**, in a deterministic order:
//!
//! 1. **Retreat from a losing fight.** If the position is **contested AND I am outnumbered**
//!    there (`enemy_ships > my_ships`), send the surplus to the **nearest safe owned**
//!    position (owned by me, not contested). *Surplus committed to the nearest safe rear.*
//! 2. **Expand to the nearest uncontested position.** Otherwise send the surplus to the
//!    **nearest uncontested** position, where *uncontested* means **NOT enemy-owned AND no
//!    enemy ships present**. A capturable **neutral** is preferred over a friendly one (the
//!    documented tie-break — neutral expands the base; reinforcing an already-owned position
//!    is only a fallback when no neutral is reachable). See [`GreedyParams`].
//! 3. **Concentrate to break through.** If **no uncontested position exists anywhere**, send
//!    the surplus to the **least-defended contested** position (the reachable contested
//!    position with the fewest enemy ships) — mass on the thinnest part of the line.
//!
//! Each owned-with-surplus position emits **at most one** action per decision (it commits its
//! surplus to a single destination), so the policy commits gradually rather than teleporting
//! its whole army, and the result is order-stable.
//!
//! # THE DIAGNOSABLE SEAM (documented, single, exploitable)
//!
//! **Greedy always sends its surplus toward a *fight* (the nearest uncontested grab, or — when
//! everything is contested — the nearest/weakest contested position) and it *never posts a
//! dedicated rear guard above the flat garrison floor.*** A position that is *uncontested right
//! now* but *exposed* (an enemy can reach it next) keeps only `garrison_floor` ships, because
//! the moment that position is no longer the cheapest expand target its surplus has already
//! been shipped forward. The exploit is identical in spirit to Layer-1's
//! `ai_seam_thin_rear_is_exploitable`:
//!
//! > *Hold a detachment back and send it wide to a thinly-held rear/home position while Greedy
//! > is committing its surplus forward. Because a captured position keeps producing, the flank
//! > snowballs faster than Greedy's forward push.*
//!
//! It is **diagnosable** (watch every owned position sit at exactly the floor while the surplus
//! streams to the front) and **exploitable** (the harness test demonstrates a rear strike
//! beating a pure-greedy seat through it).

/// Abstract per-position snapshot the greedy policy reasons over. A position is identified by
/// an opaque `usize` id (the index the adapter uses); [`PositionView`] turns an id into this.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PositionInfo {
    /// The position's id in the adapter's indexing.
    pub id: usize,
    /// Ownership from the *acting seat's* point of view.
    pub owner: PosOwner,
    /// Living ships of **mine** associated with the position (garrisoned; the adapter decides
    /// whether to fold in incoming).
    pub my_ships: u32,
    /// Living ships of the **enemy** associated with the position.
    pub enemy_ships: u32,
    /// True if both sides have a presence here (the position is being fought over).
    pub contested: bool,
}

/// Ownership of a position **relative to the acting seat** (`Me`/`Enemy`/`Neutral`). The
/// adapter maps the concrete owner onto this so the greedy logic is seat-agnostic.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PosOwner {
    /// Owned by the seat the policy is deciding for.
    Me,
    /// Owned by the opponent.
    Enemy,
    /// Unowned / neutral — capturable.
    Neutral,
}

/// The abstract view the greedy policy queries: the set of positions, a snapshot of each, and
/// a distance metric. The two adapters in [`crate::adapters`] implement it.
///
/// `distance` is only ever used to pick a *nearest* position, so its absolute scale does not
/// matter — only the ordering. A `None` distance means "unreachable" (e.g. no lane connects
/// the two planets at Layer 2): such a position is never chosen as a destination.
pub trait PositionView {
    /// Number of positions (ids are `0..len()`).
    fn len(&self) -> usize;

    /// True when there are no positions.
    fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The snapshot for position `id` (the acting seat is fixed when the view is built).
    fn info(&self, id: usize) -> PositionInfo;

    /// A distance between two positions for "nearest" selection, or `None` if `to` is not
    /// reachable from `from`. Only the *ordering* of these values is used.
    fn distance(&self, from: usize, to: usize) -> Option<f32>;

    /// Whether position `to` is a valid **export source → destination** pair for `from`.
    /// Defaults to `distance(from, to).is_some()` (reachable). Layer 2 additionally requires
    /// the *source* to be fully owned & uncontested (only a securely held planet may export),
    /// which it folds into [`PositionView::can_export_from`]; this method is purely about the
    /// destination being a legal target of a move from `from`.
    fn reachable(&self, from: usize, to: usize) -> bool {
        self.distance(from, to).is_some()
    }

    /// Whether `from` is allowed to export surplus at all this decision. Defaults to `true`
    /// (Layer 1: any owned sub may shed surplus). Layer 2 overrides it with the spec rule
    /// "a planet may only be an export SOURCE when `fully_owned_uncontested(me)`".
    fn can_export_from(&self, _from: usize) -> bool {
        true
    }
}

/// Documented constants + tie-breaks for the greedy policy. Bundled so the magic numbers are
/// named and overridable rather than buried in the logic.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GreedyParams {
    /// **Garrison floor.** Every owned position keeps this many ships as a home guard; only
    /// ships **strictly above** it are *surplus* and eligible to move. A position whose
    /// `my_ships <= garrison_floor` emits nothing. Default **2** — matches
    /// [`world::WorldParams::keep_floor`], so at Layer 2 the floor the policy *reasons with*
    /// and the floor the launch primitive *enforces* agree (the policy will not plan to move
    /// ships the `FleetOrder` would refuse to release).
    pub garrison_floor: u32,

    /// **Expand tie-break — prefer a capturable neutral over reinforcing a friendly.** When
    /// rule 2 fires, neutral destinations are considered first and the nearest neutral wins;
    /// a friendly (already-`Me`) uncontested destination is chosen *only if no neutral is
    /// reachable*. `true` (default) = grab ground first (the colonize instinct that compounds
    /// under the square law). `false` = treat neutral and friendly uncontested positions
    /// uniformly (purely nearest-first).
    pub prefer_neutral_expand: bool,
}

impl Default for GreedyParams {
    fn default() -> Self {
        GreedyParams { garrison_floor: 2, prefer_neutral_expand: true }
    }
}

/// One abstract action the greedy policy decided on: move `count` surplus ships from owned
/// position `from` to position `to`, for the reason `kind`. The adapter turns this into the
/// concrete order for its layer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GreedyAction {
    /// Owned source position the surplus is drawn from.
    pub from: usize,
    /// Destination position the surplus is sent to.
    pub to: usize,
    /// How many surplus ships to send (always `> 0`; = `my_ships - garrison_floor` at `from`).
    pub count: u32,
    /// Which rule produced this action (for tests/diagnostics; the adapter ignores it).
    pub kind: GreedyKind,
}

/// Which greedy rule produced a [`GreedyAction`] (diagnostic only).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GreedyKind {
    /// Rule 1 — retreat the surplus to the nearest safe owned position (losing a fight here).
    Retreat,
    /// Rule 2 — expand the surplus to the nearest uncontested position.
    Expand,
    /// Rule 3 — concentrate the surplus on the least-defended contested position.
    Concentrate,
}

/// Decide the greedy policy's actions for the acting seat over `view`, using `params`.
///
/// Returns one [`GreedyAction`] per owned position that has surplus and a valid destination.
/// Deterministic: a pure function of `(view snapshot, params)`. Positions are processed in
/// ascending id order and ties in "nearest" break to the **lowest destination id** (documented
/// and stable), so the same state always yields the same actions.
pub fn decide_greedy<V: PositionView>(view: &V, params: &GreedyParams) -> Vec<GreedyAction> {
    let n = view.len();
    let mut actions: Vec<GreedyAction> = Vec::new();
    if n == 0 {
        return actions;
    }

    // Is there ANY uncontested position **worth expanding to** anywhere? This gates rule 2 vs
    // rule 3 globally, per spec ("if no uncontested position exists anywhere → concentrate").
    //
    // We read "uncontested position" in the spec's intended *expansion* sense — a position the
    // policy would actually move surplus to — not the literal "any non-enemy position with no
    // enemy ships" (which would include my own already-secure home and so make rule 3 almost
    // never fire). Concretely a position is an expand target if it is a capturable **neutral**,
    // OR a **friendly** position strictly thinner than some owned position that could feed it
    // (so surplus consolidates toward a weak/forward friendly, but equally-stocked friendly
    // positions never trigger pointless ship-swapping — the degenerate churn that would
    // otherwise keep a fully-owned planet's ships perpetually in transit and starve Layer-2
    // export). See [`is_expand_target`].
    let any_uncontested = (0..n).any(|i| {
        let info = view.info(i);
        is_expand_target_global(&info, view)
    });

    for from in 0..n {
        let me = view.info(from);
        if me.owner != PosOwner::Me {
            continue; // only owned positions shed surplus
        }
        if !view.can_export_from(from) {
            continue; // Layer-2 spec: source must be fully owned & uncontested to export
        }
        let surplus = me.my_ships.saturating_sub(params.garrison_floor);
        if surplus == 0 {
            continue; // at or below the garrison floor — nothing to move
        }

        // --- Rule 1: losing a fight HERE -> retreat surplus to nearest safe owned. ---------
        if me.contested && me.enemy_ships > me.my_ships {
            if let Some(to) = nearest(view, from, |info| {
                info.id != from && info.owner == PosOwner::Me && !info.contested
            }) {
                actions.push(GreedyAction { from, to, count: surplus, kind: GreedyKind::Retreat });
                continue;
            }
            // No safe rear to retreat to: fall through (still try to do something useful with
            // the surplus rather than freeze — expand/concentrate below).
        }

        // --- Rule 2: expand surplus to the nearest UNCONTESTED position. -------------------
        // "uncontested" = NOT enemy-owned AND no enemy ships present. Prefer a capturable
        // **neutral** (the documented tie-break) before reinforcing a friendly position; a
        // friendly position is a valid target only if it is strictly thinner than this source
        // (consolidate surplus toward weakness/the front), never an equal-strength swap.
        if any_uncontested {
            let dest = if params.prefer_neutral_expand {
                // First the nearest reachable capturable NEUTRAL...
                nearest(view, from, |info| {
                    info.id != from && info.owner == PosOwner::Neutral && is_uncontested(info)
                })
                // ...else the nearest reachable friendly position strictly thinner than us
                // (reinforce a weak/forward friendly; equal friendlies are not targets).
                .or_else(|| {
                    nearest(view, from, |info| {
                        info.id != from
                            && info.owner == PosOwner::Me
                            && is_uncontested(info)
                            && info.my_ships < me.my_ships
                    })
                })
            } else {
                // No neutral preference: nearest uncontested that is either a neutral or a
                // strictly-thinner friendly (still no equal-strength churn).
                nearest(view, from, |info| {
                    info.id != from
                        && is_uncontested(info)
                        && (info.owner == PosOwner::Neutral || info.my_ships < me.my_ships)
                })
            };
            if let Some(to) = dest {
                actions.push(GreedyAction { from, to, count: surplus, kind: GreedyKind::Expand });
                continue;
            }
            // any_uncontested was true globally but nothing reachable/useful from this source:
            // fall through to concentrate so this position's surplus is still used.
        }

        // --- Rule 3: no uncontested anywhere (or none reachable) -> concentrate on the -----
        // least-defended reachable CONTESTED position (fewest enemy ships) to break through.
        if let Some(to) = least_defended_contested(view, from) {
            actions.push(GreedyAction { from, to, count: surplus, kind: GreedyKind::Concentrate });
            continue;
        }
        // Nothing reachable to act on from this position; leave its surplus in place.
    }

    actions
}

/// A position is **uncontested** iff it is NOT enemy-owned AND no enemy ships are present.
/// Neutral-with-no-enemy and friendly-with-no-enemy both qualify; anything the enemy owns or
/// has ships at does not. (This is the literal predicate; whether such a position is a useful
/// *expand target* additionally requires it be neutral or a thinner friendly — see rule 2 and
/// [`is_expand_target_global`].)
#[inline]
fn is_uncontested(info: &PositionInfo) -> bool {
    info.owner != PosOwner::Enemy && info.enemy_ships == 0
}

/// Does `info` count as a globally-meaningful **expand target** (the gate for rule 2 vs rule
/// 3)? A capturable **neutral** always does; a **friendly** uncontested position does only if
/// *some other owned position* is strictly stronger than it (so surplus could flow toward it).
/// Equally-stocked friendly positions are **not** expand targets, which is what prevents a
/// fully-owned cluster from churning ships between its own positions forever. Enemy positions
/// never qualify.
fn is_expand_target_global<V: PositionView>(info: &PositionInfo, view: &V) -> bool {
    if info.owner == PosOwner::Enemy || info.enemy_ships > 0 {
        return false;
    }
    match info.owner {
        PosOwner::Neutral => true,
        PosOwner::Me => {
            // A friendly position is a target only if a strictly stronger owned position exists
            // to feed it (otherwise reinforcing it is either impossible or a pointless swap).
            (0..view.len()).any(|j| {
                let o = view.info(j);
                o.id != info.id && o.owner == PosOwner::Me && o.my_ships > info.my_ships
            })
        }
        PosOwner::Enemy => false,
    }
}

/// The reachable position from `from` (lowest id on ties) minimizing distance, among those
/// matching `pred`. `None` if none match or none are reachable.
fn nearest<V: PositionView>(
    view: &V,
    from: usize,
    pred: impl Fn(&PositionInfo) -> bool,
) -> Option<usize> {
    let n = view.len();
    let mut best: Option<(usize, f32)> = None;
    for to in 0..n {
        let info = view.info(to);
        if !pred(&info) {
            continue;
        }
        let Some(d) = view.distance(from, to) else { continue };
        // Strictly-less keeps the FIRST seen (lowest id) on a tie -> deterministic.
        match best {
            Some((_, bd)) if bd <= d => {}
            _ => best = Some((to, d)),
        }
    }
    best.map(|(id, _)| id)
}

/// The reachable **contested** position from `from` with the fewest enemy ships (the
/// "least-defended"). Ties on enemy count break to the nearer position, then to the lowest id.
fn least_defended_contested<V: PositionView>(view: &V, from: usize) -> Option<usize> {
    let n = view.len();
    let mut best: Option<(usize, u32, f32)> = None; // (id, enemy_ships, distance)
    for to in 0..n {
        if to == from {
            continue;
        }
        let info = view.info(to);
        if !info.contested {
            continue;
        }
        let Some(d) = view.distance(from, to) else { continue };
        let key = (info.enemy_ships, d);
        match best {
            Some((_, be, bd)) if (be, bd) <= key => {}
            _ => best = Some((to, info.enemy_ships, d)),
        }
    }
    best.map(|(id, _, _)| id)
}

#[cfg(test)]
mod tests {
    //! Unit tests over a tiny in-memory [`PositionView`] so the abstract policy is verified
    //! independently of either layer's adapter.
    use super::*;

    /// A hand-built view: positions on a line at integer x-coords; distance is |dx|. Each
    /// position carries its owner/ship counts directly. `export_ok` lets a test gate exporting
    /// (to exercise the Layer-2 `can_export_from` override).
    struct LineView {
        infos: Vec<PositionInfo>,
        xs: Vec<f32>,
        export_ok: Vec<bool>,
    }

    impl LineView {
        fn new(rows: &[(PosOwner, u32, u32, f32)]) -> LineView {
            // (owner, my, enemy, x). `contested` derived from both-present.
            let infos = rows
                .iter()
                .enumerate()
                .map(|(i, &(owner, my, en, _))| PositionInfo {
                    id: i,
                    owner,
                    my_ships: my,
                    enemy_ships: en,
                    contested: presence(owner, my, en),
                })
                .collect();
            let xs = rows.iter().map(|&(_, _, _, x)| x).collect();
            let export_ok = rows.iter().map(|_| true).collect();
            LineView { infos, xs, export_ok }
        }
    }

    /// Presence-based "contested": both sides present (a sub or a ship each). Mirrors the
    /// world aggregate's rule closely enough for the policy tests.
    fn presence(owner: PosOwner, my: u32, en: u32) -> bool {
        let mine = owner == PosOwner::Me || my > 0;
        let theirs = owner == PosOwner::Enemy || en > 0;
        mine && theirs
    }

    impl PositionView for LineView {
        fn len(&self) -> usize {
            self.infos.len()
        }
        fn info(&self, id: usize) -> PositionInfo {
            self.infos[id]
        }
        fn distance(&self, from: usize, to: usize) -> Option<f32> {
            Some((self.xs[from] - self.xs[to]).abs())
        }
        fn can_export_from(&self, from: usize) -> bool {
            self.export_ok[from]
        }
    }

    #[test]
    fn floor_holds_no_surplus_no_action() {
        // One owned position at exactly the floor + one neutral next door: nothing moves.
        let v = LineView::new(&[
            (PosOwner::Me, 2, 0, 0.0),
            (PosOwner::Neutral, 0, 0, 1.0),
        ]);
        let acts = decide_greedy(&v, &GreedyParams::default());
        assert!(acts.is_empty(), "at the garrison floor there is no surplus to move");
    }

    #[test]
    fn expands_surplus_to_nearest_neutral() {
        // Owned with 6 (surplus 4) at x=0; a far neutral at x=10 and a near neutral at x=2.
        let v = LineView::new(&[
            (PosOwner::Me, 6, 0, 0.0),
            (PosOwner::Neutral, 0, 0, 10.0),
            (PosOwner::Neutral, 0, 0, 2.0),
        ]);
        let acts = decide_greedy(&v, &GreedyParams::default());
        assert_eq!(acts.len(), 1);
        assert_eq!(acts[0].from, 0);
        assert_eq!(acts[0].to, 2, "nearest neutral wins");
        assert_eq!(acts[0].count, 4, "surplus = my_ships - floor");
        assert_eq!(acts[0].kind, GreedyKind::Expand);
    }

    #[test]
    fn prefers_neutral_over_friendly_even_when_friendly_is_closer() {
        // Source x=0; a friendly uncontested at x=1 (closer) and a neutral at x=3 (farther).
        // The documented tie-break prefers the capturable neutral despite the friendly being
        // nearer.
        let v = LineView::new(&[
            (PosOwner::Me, 7, 0, 0.0),
            (PosOwner::Me, 0, 0, 1.0),
            (PosOwner::Neutral, 0, 0, 3.0),
        ]);
        let acts = decide_greedy(&v, &GreedyParams::default());
        assert_eq!(acts.len(), 1);
        assert_eq!(acts[0].to, 2, "neutral preferred over a nearer friendly");
        assert_eq!(acts[0].kind, GreedyKind::Expand);
    }

    #[test]
    fn falls_back_to_friendly_when_no_neutral() {
        // No neutral anywhere -> expand reinforces the nearest friendly uncontested position.
        let v = LineView::new(&[
            (PosOwner::Me, 7, 0, 0.0),
            (PosOwner::Me, 0, 0, 5.0),
            (PosOwner::Me, 0, 0, 2.0),
        ]);
        let acts = decide_greedy(&v, &GreedyParams::default());
        assert_eq!(acts.len(), 1);
        assert_eq!(acts[0].to, 2, "nearest friendly uncontested when no neutral exists");
        assert_eq!(acts[0].kind, GreedyKind::Expand);
    }

    #[test]
    fn retreats_when_contested_and_outnumbered() {
        // Position 0 is contested and outnumbered (mine 5, enemy 9) -> retreat surplus to the
        // nearest SAFE owned (position 2 at x=2, uncontested), not the farther safe one.
        let v = LineView::new(&[
            (PosOwner::Me, 5, 9, 0.0),
            (PosOwner::Me, 0, 0, 8.0),
            (PosOwner::Me, 0, 0, 2.0),
        ]);
        let acts = decide_greedy(&v, &GreedyParams::default());
        // Position 0 retreats; positions 1 and 2 have no surplus.
        assert_eq!(acts.len(), 1);
        assert_eq!(acts[0].from, 0);
        assert_eq!(acts[0].to, 2, "nearest safe owned rear");
        assert_eq!(acts[0].kind, GreedyKind::Retreat);
        assert_eq!(acts[0].count, 3, "surplus = 5 - floor(2)");
    }

    #[test]
    fn does_not_retreat_when_contested_but_winning() {
        // Contested but NOT outnumbered (mine 9, enemy 4): rule 1 does not fire. With no
        // uncontested position anywhere, it concentrates on the (only) contested position...
        // but that IS itself, so from this lone position there is nothing else to do.
        let v = LineView::new(&[(PosOwner::Me, 9, 4, 0.0)]);
        let acts = decide_greedy(&v, &GreedyParams::default());
        assert!(acts.is_empty(), "winning a fight: no retreat, and nowhere else to go");
    }

    #[test]
    fn concentrates_on_least_defended_contested_when_no_uncontested() {
        // Source owned with surplus; two contested positions, one with 2 enemies (x=5), one
        // with 8 (x=2). No uncontested anywhere -> concentrate on the LEAST-defended (2
        // enemies) even though it is farther.
        let v = LineView::new(&[
            (PosOwner::Me, 8, 0, 0.0),
            (PosOwner::Enemy, 3, 8, 2.0), // contested-ish: enemy-owned with my ships present
            (PosOwner::Enemy, 1, 2, 5.0),
        ]);
        let acts = decide_greedy(&v, &GreedyParams::default());
        assert_eq!(acts.len(), 1);
        assert_eq!(acts[0].from, 0);
        assert_eq!(acts[0].to, 2, "least-defended contested (2 enemies) over the heavier one");
        assert_eq!(acts[0].kind, GreedyKind::Concentrate);
    }

    #[test]
    fn export_gate_blocks_a_source() {
        // Even with surplus and a neutral to grab, a source whose can_export_from is false
        // emits nothing (this is how Layer 2 enforces "only fully-owned-uncontested exports").
        let mut v = LineView::new(&[
            (PosOwner::Me, 9, 0, 0.0),
            (PosOwner::Neutral, 0, 0, 1.0),
        ]);
        v.export_ok[0] = false;
        let acts = decide_greedy(&v, &GreedyParams::default());
        assert!(acts.is_empty(), "a non-exportable source is skipped");
    }

    #[test]
    fn the_seam_no_rear_guard_above_the_floor() {
        // THE SEAM, abstractly: a "rear" owned position (id 0) with a big stack sheds ALL its
        // surplus toward the front and is left at exactly the floor — it never keeps a reserve
        // above the flat garrison floor.
        let v = LineView::new(&[
            (PosOwner::Me, 20, 0, 0.0),   // rear/home with a fat stack
            (PosOwner::Neutral, 0, 0, 3.0), // a forward grab
        ]);
        let acts = decide_greedy(&v, &GreedyParams::default());
        assert_eq!(acts.len(), 1);
        assert_eq!(acts[0].from, 0);
        assert_eq!(acts[0].count, 18, "the whole surplus ships forward; only the floor stays");
        // After the move the rear would hold exactly garrison_floor — the seam.
        assert_eq!(v.info(0).my_ships - acts[0].count, GreedyParams::default().garrison_floor);
    }
}
