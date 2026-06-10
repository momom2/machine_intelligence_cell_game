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
//! 3. **Amass and assault.** If there is **no uncontested expand target anywhere** (nothing
//!    left to colonize), greedy must still apply force rather than idle — it **amasses** its
//!    production and breaks the enemy. Two choices, both decided from the abstract view:
//!    * **Where to strike.** A *production-superiority* proxy compares how many positions each
//!      side owns (every position is a producer): `superior = my_positions > enemy_positions`.
//!      If superior, strike the enemy where it is **strongest** (`MAX enemy_ships`) — superior
//!      production wins the war of attrition against the thickest stack. If not superior, strike
//!      where it is **weakest** (`MIN enemy_ships`) — take what can actually be taken. The target
//!      is any reachable position with enemy presence (enemy-owned, or contested with enemy
//!      ships).
//!    * **How to commit (concentration of force).** All surplus is routed through **one
//!      staging position** — the owned position nearest the target. The staging position
//!      **holds** (keeps amassing) until it has reached **local superiority**
//!      (`staging.my_ships >= target.enemy_ships`), then commits its surplus at the target;
//!      every other owned position ships its surplus **toward the staging position**. This
//!      avoids piecemeal feeding into a strong stack (square-law death) — force concentrates
//!      before it strikes. *Fallbacks so nothing freezes:* if the staging position cannot reach
//!      the target, or a feeder cannot reach the staging position, that position sends its
//!      surplus **directly at the target** instead. *Kind = `Assault`.*
//!
//! Each owned-with-surplus position emits **at most one** action per decision (it commits its
//! surplus to a single destination), so the policy commits gradually rather than teleporting
//! its whole army, and the result is order-stable.
//!
//! # THE DIAGNOSABLE SEAM (documented, single, exploitable)
//!
//! **Greedy always sends its surplus toward a *fight* (the nearest uncontested grab, or — when
//! there is nothing to colonize — forward to the assault's staging position) and it *never posts
//! a dedicated rear guard above the flat garrison floor.*** A position that is *uncontested right
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

/// A side **relative to the acting seat**, used by the projection-backed view reads
/// ([`PositionView::present_count`] etc.) so the abstract policies stay seat-agnostic: `Me`
/// is whichever real faction the view was built for, `Foe` is its opponent. The adapter maps
/// these onto the concrete `layer1::Faction` when it talks to the projection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    /// The acting seat.
    Me,
    /// The acting seat's opponent.
    Foe,
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

    /// **Query helper — the first hop** a move from `from` toward `to` routes onto THIS tick.
    /// Because a move primitive is valid only one lane/step at a time, a far objective is routed one
    /// hop at a time; this is that hop. `None` if `from == to`, `to` is unreachable, or the view has
    /// no adjacency model. It lets a policy distinguish *stepping onto* a position it can reach from
    /// *routing a wave THROUGH* a (possibly foe-held) waypoint — the latter is an assault on the
    /// waypoint, not colonisation/expansion. Default `None` (no adjacency); the real adapters
    /// override it (Layer 2 via the lane graph's next hop; Layer 1 returns `to` itself, since a
    /// structure's sub-positions are mutually adjacent).
    fn first_hop(&self, _from: usize, _to: usize) -> Option<usize> {
        None
    }

    // ----------------------------------------------------------------------------------------
    // THE PROPERTY SIGNALS + QUERIES the composable automatons (`crate::vocab`) read.
    //
    // Everything below is a thin **property accessor** or a pass-through to a **projection
    // query** — NO mechanic is re-derived here (see the `NO_MECHANIC_CONSTANTS` marker in
    // `crate::vocab`). Each has a conservative default so a view that does not wire the
    // projection (e.g. the unit-test `LineView`) still type-checks and behaves inertly; the two
    // real adapters ([`crate::adapters::Layer1View`] / [`Layer2View`]) override them to read the
    // sim signals and the shared [`world::Projection`].
    // ----------------------------------------------------------------------------------------

    /// **Property signal — capture resistance remaining** at `id` *from the acting seat's point
    /// of view*: the total foreign resistance an attacker must grind down to take this position
    /// (sum over the not-mine subs). `0.0` means nothing left to capture here. Read through the
    /// sim accessor `total_foreign_resistance` / `planet_total_resistance_vs`; the automaton
    /// never knows the `1800`/heal/refill rule behind it.
    fn resistance(&self, _id: usize) -> f32 {
        0.0
    }

    /// **Property signal — production** at `id`: ships minted per period (the sub's `production`, or a
    /// planet's summed production at Layer 2). Used to rank capture targets by *value* (e.g.
    /// resistance-per-production). Defaults to `1.0` (never `0`, so callers can divide by it safely).
    fn production(&self, _id: usize) -> f32 {
        1.0
    }

    /// **Property signal — the cheapest foothold** at `id`: the *minimum* single foreign sub
    /// resistance among the not-mine subs (the least grind to crack one sub and flip a producer),
    /// or `0.0` if there is no foreign sub. `SimpleColonizer`'s send-threshold reads this.
    fn min_foothold_resistance(&self, _id: usize) -> f32 {
        0.0
    }

    /// **Property signal — present living force** of `side` at `id` (ships physically in the
    /// position now). The defender count an attacker must clear / the heal force a holder keeps.
    fn present_count(&self, _id: usize, _side: Side) -> u32 {
        0
    }

    /// **Property signal — idle (parked-at-this-position) ships** of `side` at `id`. Used by the
    /// over-stack guard (idle vs the soft cap). Distinct from [`PositionView::present_count`],
    /// which also counts moving/co-present ships.
    fn idle_at(&self, _id: usize, _side: Side) -> u32 {
        0
    }

    /// **Property signal — the acting seat's soft cap** at `id` (the parked allowance before
    /// attrition bites). Read through the `soft_cap` accessor, expressed as a sum of per-sub
    /// capacities — the AI never writes `20 + 10*subs`.
    fn soft_cap_at(&self, _id: usize) -> u32 {
        u32::MAX
    }

    /// **Property signal — parked-pressure ratio** for the acting seat at `id`:
    /// `parked / soft_cap` in `[0, ∞)`. `>= 1` means the soft cap is destroying ships here, so
    /// surplus must be spent or kept moving. Composed from the two `parked_count` / `soft_cap`
    /// accessors.
    fn parked_ratio(&self, _id: usize) -> f32 {
        0.0
    }

    /// **Query helper — transit time** (ticks) for surplus leaving `from` to reach `to`, or
    /// `None` if unreachable. Euclidean/`ship_speed` at Layer 1; lane-path/`transit_speed`
    /// (+undock) at Layer 2. Composed only from world geometry + params (no mechanic).
    fn transit_ticks(&self, _from: usize, _to: usize) -> Option<u64> {
        None
    }

    // ---- Pass-throughs to the shared forward-projection QUERIES (per-position roll-ups). ----

    /// **Query — capture ETA.** Absolute tick this position's owner first changes on the current
    /// plan (present + in-transit, enemy passive), or `None` within the horizon. Layer-1 reads
    /// the sub's [`world::Projection::capture_eta`]; Layer-2 rolls up [`world::Projection::planet_capture`].
    fn capture_eta(&self, _id: usize) -> Option<u64> {
        None
    }

    /// **Query — projected next owner** at `id` (who holds it right after its first change), or
    /// `None` if it does not change within the horizon. `Some(Side::Me)` ⇒ already settling mine;
    /// `Some(Side::Foe)` ⇒ the enemy takes it first. Lets a policy skip targets the projection
    /// already settles, and skip subs that fall before a wave could land.
    fn projected_next_owner(&self, _id: usize) -> Option<Side> {
        None
    }

    /// **Query — marginal value of one more ship**, in *ticks saved* on the capture of `target`
    /// if that ship is sent from `from`. The steeply-diminishing `dT ≈ r/w²` quantity Colonize's
    /// "send only while it pays" rule reads. `0` means it does not help. Pass-through to
    /// [`world::Projection::marginal_ticks_saved`].
    fn marginal_ticks_saved(&self, _target: usize, _from: usize) -> u64 {
        0
    }

    /// **Query — value of committing a WAVE of `ships`** from `from` to `target`, in ticks saved
    /// on `target`'s capture vs not sending it (accounting for the from→target transit). This is
    /// the **cumulative form** of [`PositionView::marginal_ticks_saved`] — the integral of the
    /// per-ship marginal over the whole wave — computed from the projection's `capture_eta_if`
    /// what-if. Colonize uses it to size a wave under a deep grind (where a *single* extra ship
    /// cannot flip the target within the horizon, so the per-ship marginal reads 0, but a whole
    /// wave can): "send the wave while the wave still pays its transit". `0` if it does not help.
    fn wave_value_ticks(&self, _target: usize, _from: usize, _ships: u32) -> u64 {
        0
    }

    /// **Query — smallest efficient assault force** at `id`: the least force that beats the
    /// current defenders trading at least `ratio`-to-1 (square-law). `Some(0)` if undefended,
    /// `None` if even an overwhelming force cannot reach the ratio. Pass-through to
    /// [`world::Projection::force_for_efficiency`] — the single place the on-sub defender edge
    /// enters AI reasoning.
    fn force_for_efficiency(&self, _id: usize, _ratio: f32) -> Option<u32> {
        None
    }

    /// **Query — my in-flight force already arriving** at `id` within the horizon (so a policy
    /// does not double-send to a target its own fleets already settle). Pass-through to
    /// [`world::Projection::incoming_present_at`] for the acting seat.
    fn incoming_mine(&self, _id: usize) -> u32 {
        0
    }

    /// **Query — the returning-owner heal force** the projection expects at `id` (in-flight ships
    /// of its current owner). Attack sizes a heal-outlasting hold from this. Pass-through to
    /// [`world::Projection::returning_owner_force`].
    fn returning_owner_force(&self, _id: usize) -> u32 {
        0
    }

    /// **Query — in-flight FOE force arriving** at `id` within the horizon: the aggregate of every
    /// real faction *other than the acting seat* (the mirror of [`PositionView::incoming_mine`]).
    /// The stateful colonizer adds this to the present `enemy_ships` to size a target against the
    /// force that will actually contest it. Pass-through to [`world::Projection::incoming_present_at`]
    /// summed over the non-seat real factions (matching how `enemy_ships` is already aggregated).
    fn enemy_incoming(&self, _id: usize) -> u32 {
        0
    }

    /// **Query — earliest tick my own in-flight ships first reach `id`** (absolute world tick), or
    /// `None` if I have nothing inbound there within the horizon. Floors the synchronized-landing
    /// time so a fresh wave is staggered to arrive *with* (not before) force already on the way.
    /// Pass-through to [`world::Projection::eta_to_present_for`] for the acting seat.
    fn friendly_eta(&self, _id: usize) -> Option<u64> {
        None
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
///
/// The first three are the classic greedy rules; the last two are the extra atomic ACTIONS the
/// composable automatons (`crate::vocab`) emit — they share the same [`GreedyAction`] shape and
/// the same adapters, so a `Deny`/`Wave` action becomes a `MoveOrder`/`FleetOrder` exactly like
/// an `Expand`. (`hold` emits *no* action, so it needs no variant.)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GreedyKind {
    /// Rule 1 — retreat the surplus to the nearest safe owned position (losing a fight here).
    Retreat,
    /// Rule 2 — expand the surplus to the nearest uncontested position.
    Expand,
    /// Rule 3 — **amass and assault**: with nothing left to colonize, route the surplus toward
    /// the assault (forward to the staging position, or — for the staging position once it has
    /// local superiority, or on a routing fallback — directly at the enemy target). Subsumes the
    /// old "concentrate on the least-defended contested position".
    Assault,
    /// Vocabulary action `wave(target, size)` — a sized colonization/capture wave toward a
    /// target (the composable automatons' expand primitive). Distinguished from `Expand` only so
    /// tests/diagnostics can see the automaton sized it deliberately; the adapter treats it the same.
    Wave,
    /// Vocabulary action `deny(target)` — a cheap detachment parked **on a productive foreign
    /// sub** purely to FREEZE its output (Mechanic B) before/without capturing it.
    Deny,
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
    // rule 3 globally, per spec ("if no uncontested position exists anywhere → amass and assault").
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

    // When there is nothing left to colonize, precompute the single assault plan ONCE (so every
    // owned position routes consistently this decision): the enemy target to break, and the
    // staging position to amass behind. Computed deterministically from the abstract view; see
    // [`plan_assault`]. `None` when there is no reachable enemy presence at all (then rule 3 is a
    // no-op and the surplus simply stays put — there is nothing to attack).
    let assault = if any_uncontested { None } else { plan_assault(view) };

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
            // the surplus rather than freeze — expand/assault below).
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
            // fall through to the assault so this position's surplus is still used.
        }

        // --- Rule 3: nothing to colonize -> AMASS behind one staging position and ASSAULT. --
        // The plan (target + staging + production superiority) was computed once above. Each
        // owned position routes by its role:
        //   * the staging position commits at the target ONCE it has local superiority
        //     (my_ships >= target.enemy_ships), else it HOLDS (keeps amassing);
        //   * every other owned position ships its surplus toward the staging position;
        //   * fallback (so nothing freezes): if a position cannot reach its intended hop
        //     (staging unreachable from the spearhead, or staging unreachable from a feeder),
        //     it sends its surplus directly at the target instead.
        if let Some(plan) = assault {
            if from == plan.staging {
                // Spearhead. Commit only with local superiority; otherwise hold and amass.
                let target_enemy = view.info(plan.target).enemy_ships;
                if me.my_ships >= target_enemy && view.reachable(from, plan.target) {
                    actions.push(GreedyAction { from, to: plan.target, count: surplus, kind: GreedyKind::Assault });
                }
                // else: HOLD (emit nothing) — keep building the stack to break the target.
                continue;
            }
            // Feeder: ship surplus toward the staging position, else fall back to the target.
            let to = if view.reachable(from, plan.staging) {
                plan.staging
            } else if view.reachable(from, plan.target) {
                plan.target
            } else {
                continue; // can reach neither — leave the surplus in place rather than freeze.
            };
            actions.push(GreedyAction { from, to, count: surplus, kind: GreedyKind::Assault });
            continue;
        }
        // No assault plan (nothing to colonize AND no reachable enemy presence): leave the
        // surplus in place — there is genuinely nothing to do.
    }

    actions
}

/// The single, deterministic **assault plan** rule 3 commits to when there is nothing left to
/// colonize: which enemy `target` to break and which owned `staging` position to amass behind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct AssaultPlan {
    /// The enemy-presence position to break (enemy-owned, or contested with enemy ships).
    target: usize,
    /// The owned position to mass behind — the owned position nearest the target.
    staging: usize,
}

/// Build the [`AssaultPlan`] from the abstract view, or `None` if there is no reachable enemy
/// presence to attack (then rule 3 does nothing). Pure and deterministic.
///
/// * **Production-superiority proxy:** count owned positions on each side (every position is a
///   producer, so this is the available production-rate proxy). `superior = my_positions >
///   enemy_positions`.
/// * **Target:** among positions with enemy presence (enemy-owned, or contested with
///   `enemy_ships > 0`) that are reachable from at least one owned position, pick **MAX
///   `enemy_ships`** when superior (break the strongest — superior production wins attrition) or
///   **MIN `enemy_ships`** otherwise (hit the weakest). Ties break to the lower id (deterministic).
/// * **Staging:** the owned position nearest the chosen target (lower id on distance ties); if no
///   owned position can reach the target, fall back to the lowest-id owned position so feeders
///   still have a rally point and the fallback-to-target path can fire.
fn plan_assault<V: PositionView>(view: &V) -> Option<AssaultPlan> {
    let n = view.len();
    let my_positions = (0..n).filter(|&i| view.info(i).owner == PosOwner::Me).count();
    let enemy_positions = (0..n).filter(|&i| view.info(i).owner == PosOwner::Enemy).count();
    let superior = my_positions > enemy_positions;

    // Choose the target by enemy strength, among reachable enemy-presence positions.
    let mut best: Option<(usize, u32)> = None; // (id, enemy_ships)
    for t in 0..n {
        let info = view.info(t);
        let enemy_presence =
            info.owner == PosOwner::Enemy || (info.contested && info.enemy_ships > 0);
        if !enemy_presence {
            continue;
        }
        // Must be reachable from at least one owned position (else we could never strike it).
        let reachable_from_owned = (0..n).any(|o| {
            o != t && view.info(o).owner == PosOwner::Me && view.reachable(o, t)
        });
        if !reachable_from_owned {
            continue;
        }
        let key = info.enemy_ships;
        best = Some(match best {
            // Superior -> maximize enemy_ships; not superior -> minimize. Lower id breaks ties.
            Some((bid, bk)) => {
                let take = if superior { key > bk } else { key < bk };
                if take { (t, key) } else { (bid, bk) }
            }
            None => (t, key),
        });
    }
    let (target, _) = best?;

    // Staging = owned position nearest the target; fall back to the lowest-id owned position.
    let staging = nearest(view, target, |info| {
        info.id != target && info.owner == PosOwner::Me
    })
    .or_else(|| (0..n).find(|&i| view.info(i).owner == PosOwner::Me))?;

    Some(AssaultPlan { target, staging })
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
        // uncontested position anywhere, the assault rule would fire — but the only enemy
        // presence IS this lone position itself, and a position cannot stage an assault on
        // itself (no *other* owned position can reach it), so there is nothing to do.
        let v = LineView::new(&[(PosOwner::Me, 9, 4, 0.0)]);
        let acts = decide_greedy(&v, &GreedyParams::default());
        assert!(acts.is_empty(), "winning a fight: no retreat, and nowhere else to go");
    }

    #[test]
    fn assaults_weakest_enemy_when_not_production_superior() {
        // (Was `concentrates_on_least_defended_contested_when_no_uncontested`.) One owned
        // position (id 0) vs TWO enemy-owned positions (ids 1,2) -> I am NOT production-superior
        // (1 owned < 2 owned), so the assault hits the WEAKEST enemy: id 2 has 2 enemy ships,
        // id 1 has 8, so target = id 2 even though it is farther. The lone owned position is the
        // staging position and already has local superiority (8 >= 2), so it commits.
        let v = LineView::new(&[
            (PosOwner::Me, 8, 0, 0.0),
            (PosOwner::Enemy, 3, 8, 2.0), // enemy-owned, heavily defended (8)
            (PosOwner::Enemy, 1, 2, 5.0), // enemy-owned, thinly defended (2) -> the weak target
        ]);
        let acts = decide_greedy(&v, &GreedyParams::default());
        assert_eq!(acts.len(), 1);
        assert_eq!(acts[0].from, 0);
        assert_eq!(acts[0].to, 2, "not superior -> hit the WEAKEST enemy (2) over the heavy one (8)");
        assert_eq!(acts[0].kind, GreedyKind::Assault);
    }

    #[test]
    fn assaults_strongest_enemy_when_production_superior() {
        // TWO owned positions (ids 0,1) vs ONE enemy (id 2) -> I AM production-superior
        // (2 > 1), so the assault breaks the enemy where it is STRONGEST. With a single enemy
        // that is trivially the target; the point is the *kind* and that superior production
        // routes the assault. Staging = the owned position nearest the enemy (id 1 at x=8 is
        // nearer than id 0 at x=0), which commits (10 >= 6); the rear (id 0) feeds the staging.
        let v = LineView::new(&[
            (PosOwner::Me, 10, 0, 0.0),  // rear
            (PosOwner::Me, 10, 0, 8.0),  // forward owned -> staging (nearest the enemy)
            (PosOwner::Enemy, 1, 6, 12.0),
        ]);
        let acts = decide_greedy(&v, &GreedyParams::default());
        assert_eq!(acts.len(), 2, "both owned positions act: spearhead strikes, rear feeds it");
        // Staging (id 1) commits at the enemy (id 2).
        let spear = acts.iter().find(|a| a.from == 1).expect("staging acts");
        assert_eq!(spear.to, 2, "the staging spearhead commits at the enemy target");
        assert_eq!(spear.kind, GreedyKind::Assault);
        // Rear (id 0) ships its surplus toward the staging position (id 1), not the target.
        let feeder = acts.iter().find(|a| a.from == 0).expect("rear feeds");
        assert_eq!(feeder.to, 1, "the rear amasses behind the staging position");
        assert_eq!(feeder.kind, GreedyKind::Assault);
    }

    #[test]
    fn assault_staging_holds_until_local_superiority() {
        // Production-superior (2 owned vs 1 enemy) so the assault targets the strongest enemy,
        // but the spearhead (the owned position nearest the enemy) is too THIN to break it yet:
        // staging id 1 has 20 ships vs the target's 30, so it HOLDS (emits nothing) and keeps
        // amassing, while the rear (id 0) ships its surplus forward to build the stack. This is
        // the "don't feed piecemeal into a strong stack" concentration rule.
        //
        // The two owned positions are EQUAL strength (20 each) on purpose: an equal friendly is
        // not a rule-2 "expand to a thinner friendly" target, so this exercises the rule-3
        // assault path (not Expand) even though the staging position is thin relative to the
        // *enemy*.
        let v = LineView::new(&[
            (PosOwner::Me, 20, 0, 0.0),  // rear, feeds the staging position
            (PosOwner::Me, 20, 0, 8.0),  // spearhead -> staging (nearest the enemy), but 20 < 30
            (PosOwner::Enemy, 0, 30, 12.0),
        ]);
        let acts = decide_greedy(&v, &GreedyParams::default());
        // The thin staging position holds; only the rear acts (feeding the staging position).
        assert_eq!(acts.len(), 1, "the thin spearhead holds; only the rear feeds it");
        assert_eq!(acts[0].from, 0);
        assert_eq!(acts[0].to, 1, "rear amasses behind the (still-thin) staging position");
        assert_eq!(acts[0].kind, GreedyKind::Assault);
    }

    #[test]
    fn assaults_a_quiet_passive_enemy_with_nothing_to_colonize() {
        // The headline fix: a quiet (uncontested, never-moving) enemy and NOTHING uncontested to
        // colonize. Greedy must NOT idle — it assaults. Here one owned position with surplus and
        // one quiet enemy-owned position (no fight in progress). Production-superior (1 vs 1 is
        // NOT superior, so this exercises the weakest branch too — single enemy is the target
        // either way). The owned position stages and, with local superiority, strikes.
        let v = LineView::new(&[
            (PosOwner::Me, 12, 0, 0.0),
            (PosOwner::Enemy, 0, 3, 5.0), // enemy-owned & quiet (no fight here) — a passive foe
        ]);
        let acts = decide_greedy(&v, &GreedyParams::default());
        assert_eq!(acts.len(), 1, "greedy assaults the quiet enemy rather than idling");
        assert_eq!(acts[0].from, 0);
        assert_eq!(acts[0].to, 1, "it strikes the enemy position");
        assert_eq!(acts[0].kind, GreedyKind::Assault);
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
