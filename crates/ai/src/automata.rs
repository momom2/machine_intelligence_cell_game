//! # `ai::automata` — the four AI automatons, written as COMPOSITIONS over [`crate::vocab`]
//!
//! Each automaton here is a **short program in the shared language**: its body is nothing but
//! QUERIES ([`world::Projection`] methods, surfaced through the [`PositionView`]), PREDICATES and
//! ACTIONS (from [`crate::vocab`]). They are layer-agnostic (they run over the abstract
//! [`PositionView`], so the *same* program drives Layer-1 sub-structures and Layer-2 planets) and
//! deliberately **legible** — a future evolved/abstract agent is meant to recombine these exact
//! pieces, so the four below read as four *example policies* in one vocabulary, not four bespoke
//! AIs.
//!
//! ## The hard invariant (see [`crate::vocab::NO_MECHANIC_CONSTANTS`])
//!
//! No automaton names a raw mechanic constant or formula. Every mechanic question — "how long to
//! capture?", "will this hold?", "how big a force wins efficiently?", "is the cap destroying my
//! ships?" — is a projection QUERY or a property accessor. The **only** numbers each automaton
//! names are **policy tunables**, and they live in that automaton's own `*Params` struct below,
//! each documented as a dial (never a mechanic). That separation is the whole point.
//!
//! ## The four (and their RPS identities)
//!
//! * [`SimpleColonizerParams`] — the early-campaign everyman: size each wave to the target's
//!   resistance, fill nearest-first, keep the documented thin-rear seam.
//! * [`ColonizeParams`] — resistance-optimized expansion: send one more ship only while it still
//!   *pays* (`marginal_ticks_saved >= transit_cost`); a few fronts in parallel.
//! * [`AttackParams`] — grind-and-hold siege: win the firefight efficiently, **sustain** the hold
//!   so the heal cannot reset the grind, cheap denial gated behind production superiority.
//! * [`DefendParams`] — heal-and-hold turtle: mass defenders to an *efficient* (not infinite)
//!   ratio, reinforce the sub that falls first, colonize the genuine surplus.

use crate::greedy::{GreedyAction, PosOwner, PositionView, Side};
use crate::vocab::{
    being_eroded, deny, foe_present, foe_takes_first, has_surplus, hold, nearest, outnumbered_here,
    over_soft_cap, owned_by_me, production_superior, retreat, settles_mine, surplus_of, wave,
    would_overstack,
};

/// Default look-ahead the automatons project over, in ticks. A **policy** choice (how far to
/// trust an enemy-blind forecast), not a mechanic — it equals the projection's own documented
/// default so the controller and the policies agree.
pub const PROJECTION_HORIZON: u64 = world::DEFAULT_PROJECTION_HORIZON;

/// The garrison floor every automaton keeps on an owned position; ships above it are surplus.
/// A **policy** tunable equal to the world's `keep_floor` so a policy never plans to move ships
/// the launch primitive would refuse to release. (Re-stated in each `*Params` so each automaton
/// owns its dials; centralized here only as the shared default value.)
pub const GARRISON_FLOOR: u32 = 2;

// =====================================================================================
// The automaton handle.
// =====================================================================================

/// One of the four automatons, each bundling its own policy `*Params`. Call [`Automaton::decide`]
/// with any [`PositionView`] (Layer 1 or Layer 2) to get the layer-agnostic [`GreedyAction`]s; the
/// existing adapters fold those into `MoveOrder` / `FleetOrder`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Automaton {
    /// Resistance-sized, nearest-first colonizer (the everyman).
    SimpleColonizer(SimpleColonizerParams),
    /// Marginal-value colonizer (send one more ship only while it pays).
    Colonize(ColonizeParams),
    /// Grind-and-hold siege (concentrate, deny, sustain).
    Attack(AttackParams),
    /// Heal-and-hold turtle (efficient defense, reinforce first-fall, colonize the surplus).
    Defend(DefendParams),
}

impl Automaton {
    /// Run this automaton's program over `view`, returning the abstract actions for the tick.
    pub fn decide<V: PositionView>(&self, view: &V) -> Vec<GreedyAction> {
        match self {
            Automaton::SimpleColonizer(p) => simple_colonize(view, p),
            Automaton::Colonize(p) => colonize(view, p),
            Automaton::Attack(p) => attack(view, p),
            Automaton::Defend(p) => defend(view, p),
        }
    }

    /// A short human-readable name.
    pub fn name(&self) -> &'static str {
        match self {
            Automaton::SimpleColonizer(_) => "SimpleColonizer",
            Automaton::Colonize(_) => "Colonize",
            Automaton::Attack(_) => "Attack",
            Automaton::Defend(_) => "Defend",
        }
    }
}

// =====================================================================================
// 1. SimpleColonizer — resistance-sized, nearest-first; keeps the thin-rear seam.
// =====================================================================================

/// Policy tunables for [`simple_colonize`]. **All dials, no mechanics** (the resistance values
/// they multiply come from the projection/accessors).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SimpleColonizerParams {
    /// Garrison floor (ships kept home on every owned position). Policy dial; default
    /// [`GARRISON_FLOOR`].
    pub garrison_floor: u32,
    /// **Ships committed per unit of capture resistance.** The wave-size dial: the force GOAL
    /// toward a target is `SHIPS_PER_RES * Σ(remaining foreign sub resistance)`. Purely a policy
    /// conversion from "how much grind is left" (a projection/accessor number) to "how many ships
    /// I want committed"; it is NOT the mechanic's grind rate.
    pub ships_per_res: f32,
    /// Minimum wave size — never bother sending fewer than this (a one-ship trickle is wasted
    /// transit). Policy dial.
    pub min_wave: u32,
}

impl Default for SimpleColonizerParams {
    fn default() -> Self {
        // `ships_per_res` converts "grind left on a target" into "ships I want committed toward it".
        // Under the 1800-resistance grind a wave accumulates present force on the sub and erodes it
        // together, so the GOAL only needs to be a healthy standing commitment (~30-40 ships), not a
        // one-shot crack: `0.02 * 1800 ≈ 36`. (The old `0.12` was sized for the retired
        // max_resistance≈100 and at 1800 demanded ~216 ships before sending anything — it never
        // fired, drawing with a passive enemy. See AUTOMATA_DESIGN §6.)
        SimpleColonizerParams { garrison_floor: GARRISON_FLOOR, ships_per_res: 0.02, min_wave: 3 }
    }
}

/// **SimpleColonizer — the program.**
///
/// ```text
/// for each owned position FROM (ascending id):
///     if outnumbered_here(FROM):                 retreat(FROM -> nearest safe)   ; THE SEAM:
///                                                                                  no rear guard
/// targets = { T : foe-free capturable (neutral, or mine-but-thin) AND not settles_mine(T) }
/// for each T: force_goal[T]  = SHIPS_PER_RES * resistance(T)          # size by total grind
///             committed[T]   = incoming_mine(T)                       # ships already inbound
///             threshold[T]   = SHIPS_PER_RES * min_foothold(T)        # crack the cheapest sub
/// fill NEAREST-FIRST:
///   for each owned FROM with surplus (ascending id):
///       T = nearest unfilled target reachable from FROM
///       want = min(surplus, force_goal[T] - committed[T])
///       # only SEND if we can field at least the cheapest-foothold threshold,
///       # and STOP once committed >= goal (no over-send)
///       if want > 0 and (committed[T] + want) >= threshold[T]:
///           wave(FROM -> T, want) ; committed[T] += released
/// ```
///
/// **Identity & seam.** Sizes by resistance only (ignores transit cost), fills nearest-first,
/// keeps only the garrison floor everywhere — so an exposed-but-quiet rear sits at the floor while
/// production streams forward (THE seam: re-expressed under the new mechanics as a *sustained
/// uncontested-presence / denial streak* an exploiter gets on the thin rear — see the seam test).
fn simple_colonize<V: PositionView>(view: &V, p: &SimpleColonizerParams) -> Vec<GreedyAction> {
    let n = view.len();
    let mut actions = Vec::new();
    if n == 0 {
        return actions;
    }
    let floor = p.garrison_floor;

    // (0) Reactive retreat reflex (the greedy rule 1) — no dedicated rear guard (THE seam).
    let mut spent = vec![false; n];
    for from in 0..n {
        if outnumbered_here(view, from) {
            if let Some(a) = retreat(view, from, floor) {
                actions.push(a);
                spent[from] = true;
            }
        }
    }

    // (1) Targets: capturable & foe-free (a neutral, or a thin friendly worth reinforcing), that
    //     the projection does not already settle in my favour. Size the force GOAL by the TOTAL
    //     cumulative foreign resistance; the send THRESHOLD by the cheapest single foothold.
    struct Tgt {
        id: usize,
        goal: u32,
        committed: u32,
        threshold: u32,
    }
    let mut targets: Vec<Tgt> = Vec::new();
    for t in 0..n {
        let info = view.info(t);
        let foe_free = !foe_present(view, t);
        let capturable = matches!(info.owner, PosOwner::Neutral) // neutral ground, or
            || (info.owner == PosOwner::Me && is_thin_friendly(view, t)); // a thin friendly to thicken
        if !foe_free || !capturable || settles_mine(view, t) {
            continue;
        }
        let total_res = view.resistance(t);
        if total_res <= 0.0 {
            continue;
        }
        let goal = (p.ships_per_res * total_res).ceil() as u32;
        let goal = goal.max(p.min_wave);
        // SEND threshold = the minimum wave (avoid pointless 1-2 ship trickles). It is NOT
        // resistance-scaled: under the grind, successive waves ACCUMULATE present force on the sub
        // and erode it together, so the colonizer keeps feeding toward `goal` over many ticks rather
        // than needing to crack the foothold in a single wave (a resistance-scaled threshold at 1800
        // would block the first send forever).
        let threshold = p.min_wave.max(1);
        targets.push(Tgt { id: t, goal, committed: view.incoming_mine(t), threshold });
    }
    if targets.is_empty() {
        return actions;
    }

    // (2) Fill NEAREST-FIRST. Each owned source pours surplus into the nearest unfilled target it
    //     can reach; STOP a target once committed (present+in-transit) >= goal (no over-send), and
    //     only SEND when the commitment can crack the cheapest foothold.
    for from in 0..n {
        if spent[from] || !has_surplus(view, from, floor) {
            continue;
        }
        let Some(mut avail) = surplus_of(view, from, floor) else { continue };
        // Sort candidate targets by (distance asc, id asc) — nearest-first, deterministic.
        let mut order: Vec<usize> = (0..targets.len())
            .filter(|&k| {
                targets[k].committed < targets[k].goal && view.reachable(from, targets[k].id)
            })
            .collect();
        order.sort_by(|&a, &b| {
            let da = view.distance(from, targets[a].id).unwrap_or(f32::INFINITY);
            let db = view.distance(from, targets[b].id).unwrap_or(f32::INFINITY);
            da.partial_cmp(&db).unwrap().then(targets[a].id.cmp(&targets[b].id))
        });
        for k in order {
            if avail == 0 {
                break;
            }
            let remaining = targets[k].goal.saturating_sub(targets[k].committed);
            let want = avail.min(remaining);
            if want == 0 {
                continue;
            }
            // Only send if the commitment toward this target can crack the cheapest foothold.
            if targets[k].committed + want < targets[k].threshold {
                continue;
            }
            if let Some(a) = wave(view, from, targets[k].id, want, floor) {
                let released = a.count;
                actions.push(a);
                targets[k].committed = targets[k].committed.saturating_add(released);
                avail = avail.saturating_sub(released);
            }
        }
    }
    actions
}

// =====================================================================================
// 2. Colonize — marginal-value expansion (send one more ship only while it pays).
// =====================================================================================

/// Policy tunables for [`colonize`]. **All dials, no mechanics.**
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ColonizeParams {
    /// Garrison floor. Policy dial.
    pub garrison_floor: u32,
    /// **Parallel fronts.** How many distinct colony targets to feed at once before piling the
    /// rest onto the front-runner. Policy dial.
    pub max_concurrent: usize,
    /// **Per-source wave cap.** Never ship more than this from one position in one tick (so a fat
    /// rear does not dump its whole stack into one lane). Policy dial.
    pub wave_max: u32,
    /// **Over-stack guard fraction.** Do not ship into a position whose idle stack is already this
    /// share of its soft cap (it would just bleed to attrition). Policy dial; the *cap* it
    /// compares against is the accessor's number, not a formula.
    pub overstack_frac: f32,
}

impl Default for ColonizeParams {
    fn default() -> Self {
        ColonizeParams {
            garrison_floor: GARRISON_FLOOR,
            max_concurrent: 3,
            wave_max: 16,
            overstack_frac: 0.8,
        }
    }
}

/// **Colonize — the program.**
///
/// ```text
/// candidates = { T : capturable neutral, foe-free, not settles_mine(T), not foe_takes_first(T) }
/// keep at most MAX_CONCURRENT, preferring the soonest capture_eta (front-runners)
/// for each owned FROM with surplus (ascending id):
///     T = nearest candidate reachable from FROM (else the front-runner)
///     # THE RULE: send one more ship ONLY while it still pays —
///     #   marginal_ticks_saved(T, from=FROM) >= transit_cost_ticks(FROM -> T)
///     if marginal_ticks_saved(T, FROM) >= transit_ticks(FROM, T) and not would_overstack(T):
///         wave(FROM -> T, surplus_capped)
/// ```
///
/// **Identity.** Fastest power-base growth: it spends a ship on a front exactly while that ship
/// buys more capture-time than it costs in transit (the steeply-diminishing `dT ≈ r/w²` sweet
/// spot, read straight off `marginal_ticks_saved`), running a few fronts in parallel.
/// **Blind spot.** Thin defense — everything above the floor goes forward, so a freshly flipped,
/// production-fat colony is held only by the floor (the intended attack-beats-colonize edge).
fn colonize<V: PositionView>(view: &V, p: &ColonizeParams) -> Vec<GreedyAction> {
    let n = view.len();
    let mut actions = Vec::new();
    if n == 0 {
        return actions;
    }
    let floor = p.garrison_floor;

    // Candidate colony targets: capturable NEUTRAL, foe-free, not already mine in-flight, and not
    // one the enemy is projected to take first. (Enemy-owned ground is Attack's job — clean identity.)
    let mut candidates: Vec<usize> = (0..n)
        .filter(|&t| {
            matches!(view.info(t).owner, PosOwner::Neutral)
                && !foe_present(view, t)
                && view.resistance(t) > 0.0
                && !settles_mine(view, t)
                && !foe_takes_first(view, t)
        })
        .collect();
    if candidates.is_empty() {
        return actions;
    }
    // Prefer the front-runners: soonest projected capture (an already-progressing flip), then id.
    candidates.sort_by(|&a, &b| {
        let ea = view.capture_eta(a).unwrap_or(u64::MAX);
        let eb = view.capture_eta(b).unwrap_or(u64::MAX);
        ea.cmp(&eb).then(a.cmp(&b))
    });
    candidates.truncate(p.max_concurrent.max(1));
    let frontrunner = candidates[0];

    // Each owned source sheds surplus toward the nearest chosen target it serves — but only while
    // the marginal ship still pays for its transit (the sweet-spot rule).
    for from in 0..n {
        if !has_surplus(view, from, floor) {
            continue;
        }
        let Some(surplus) = surplus_of(view, from, floor) else { continue };
        // Nearest chosen target reachable from here; fall back to the front-runner.
        let tgt = nearest(view, from, |i| candidates.contains(&i.id) && view.reachable(from, i.id))
            .unwrap_or(frontrunner);
        if !view.reachable(from, tgt) {
            continue;
        }
        // THE marginal rule: one more ship is worth sending iff it saves at least its transit cost.
        let saved = view.marginal_ticks_saved(tgt, nearest_owned_sub_proxy(view, from, tgt));
        let cost = view.transit_ticks(from, tgt).unwrap_or(u64::MAX);
        if saved < cost {
            // Not paying on the nearest; try the front-runner before giving up (it may still pay).
            if tgt == frontrunner {
                continue;
            }
            let saved_fr = view.marginal_ticks_saved(frontrunner, nearest_owned_sub_proxy(view, from, frontrunner));
            let cost_fr = view.transit_ticks(from, frontrunner).unwrap_or(u64::MAX);
            if saved_fr < cost_fr || !view.reachable(from, frontrunner) {
                continue;
            }
            if would_overstack(view, frontrunner, p.overstack_frac) {
                continue;
            }
            if let Some(a) = wave(view, from, frontrunner, surplus.min(p.wave_max), floor) {
                actions.push(a);
            }
            continue;
        }
        if would_overstack(view, tgt, p.overstack_frac) {
            continue;
        }
        if let Some(a) = wave(view, from, tgt, surplus.min(p.wave_max), floor) {
            actions.push(a);
        }
    }
    actions
}

// =====================================================================================
// 3. Attack — grind-and-hold siege (concentrate, deny, sustain).
// =====================================================================================

/// Policy tunables for [`attack`]. **All dials, no mechanics** (the force/heal numbers come from
/// the projection's `force_for_efficiency` / `returning_owner_force`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AttackParams {
    /// Garrison floor. Policy dial.
    pub garrison_floor: u32,
    /// **Desired combat efficiency** for the spearhead: the attacker:defender casualty ratio the
    /// siege force is sized to win at (via `force_for_efficiency`). A policy dial fed *into* the
    /// projection query — not the square-law itself.
    pub fight_efficiency: f32,
    /// **Heal-outlast margin.** The post-clear hold must exceed the projected returning heal force
    /// by this factor so the heal cannot reset the grind. Policy dial.
    pub heal_outlast_margin: f32,
    /// **Grind-hold floor.** Minimum ships kept INSIDE a grind site so a stray returner cannot
    /// re-contest (freeze) the grind. Policy dial.
    pub grind_hold_floor: u32,
    /// **Denial detachment size** — ships parked on a productive foreign sub purely to freeze its
    /// output. Policy dial.
    pub denial_detach: u32,
    /// **Soft-cap spend trigger** — when parked reaches this share of the soft cap, force a spend
    /// (transit is cap-exempt) rather than let ships die parked. Policy dial.
    pub softcap_spend_trigger: f32,
}

impl Default for AttackParams {
    fn default() -> Self {
        AttackParams {
            garrison_floor: GARRISON_FLOOR,
            // Commit the spearhead at a MODEST efficiency bar. Under the 1800-resistance grind a
            // high bar (e.g. 10:1) makes Attack hoard a mobile stack it never commits — which wins
            // on raw ship count at the horizon and trivially beats the turtle, collapsing the RPS
            // into "attack dominates". A modest bar makes Attack COMMIT its mass into the siege,
            // where a correct Defender's heal + on-sub edge grinds it down and counter-takes the
            // emptied rear — restoring defend>attack while still landing attack>colonize. (Measured:
            // sweeping this is the dominant lever on the defend>attack edge; see AUTOMATA_DESIGN §6.)
            fight_efficiency: 2.0,
            heal_outlast_margin: 1.25,
            grind_hold_floor: 4,
            denial_detach: 6,
            softcap_spend_trigger: 0.8,
        }
    }
}

/// **Attack — the program.**
///
/// ```text
/// plan = plan_siege(view)            # ONE target with foe presence + ONE staging (nearest owned)
/// if plan is None: return colonize(view)            # pre-contact: develop, don't idle
/// for each owned FROM (ascending id):
///     force_spend = over_soft_cap(FROM, SOFTCAP_SPEND_TRIGGER)
///     if outnumbered_here(FROM):              retreat(FROM)               ; preserve the army
///     elif FROM == staging:
///          if ready_to_commit(target):  wave(FROM -> target, surplus)    ; the spearhead wave
///          elif force_spend:            wave(FROM -> target, surplus)     ; cap valve
///          else:                        hold                              ; amass
///     elif is_grind_site(FROM):                                          ; SUSTAIN the hold
///          keep = max(GRIND_HOLD_FLOOR, HEAL_OUTLAST_MARGIN * returning_owner_force(FROM))
///          if surplus_above(keep) and force_spend: wave(excess -> staging)
///          else: hold                                                     ; out-heal the grind
///     elif production_superior and deny_target D: deny(FROM -> D, DENIAL_DETACH)  ; cheap choke
///     else:                                   wave(FROM -> staging)       ; feed the mass
/// ```
///
/// where `ready_to_commit(target)` = the spearhead can field at least
/// `force_for_efficiency(target, FIGHT_EFFICIENCY)` AND its post-clear force out-lasts the
/// projected heal AND the projection does not say the target falls (to me or the foe) first.
///
/// **Identity.** A SIEGE, not a raid: commit a force that wins the firefight efficiently and
/// out-erodes the returning heal, then **sustain** it; deny productive subs cheaply when ahead.
/// **Blind spot.** Over-extension — it strips its rear to the floor to feed the siege (the
/// intended defend-beats-attack edge).
fn attack<V: PositionView>(view: &V, p: &AttackParams) -> Vec<GreedyAction> {
    let n = view.len();
    if n == 0 {
        return Vec::new();
    }
    let floor = p.garrison_floor;
    let Some(plan) = plan_siege(view) else {
        // Pre-contact: no foe presence to besiege — develop with the colonizer's expansion.
        return colonize(view, &ColonizeParams { garrison_floor: floor, ..ColonizeParams::default() });
    };

    let mut actions = Vec::new();
    let superior = production_superior(view);
    for from in 0..n {
        if !owned_by_me(view, from) || !view.can_export_from(from) {
            continue;
        }
        let force_spend = over_soft_cap(view, from, p.softcap_spend_trigger);
        let Some(surplus) = surplus_of(view, from, floor) else {
            // No surplus, but a capped grind holder may still need to bleed overflow — handled below.
            if !force_spend {
                continue;
            }
            // fall through with zero surplus only matters to the grind-site branch; skip otherwise.
            continue;
        };

        // (a) Retreat a losing local fight.
        if outnumbered_here(view, from) {
            if let Some(a) = retreat(view, from, floor) {
                actions.push(a);
            }
            continue;
        }

        // (b) The spearhead (staging position): commit only when READY; else hold and amass.
        if from == plan.staging {
            if ready_to_commit(view, p, plan.target, from) {
                if let Some(a) = wave(view, from, plan.target, surplus, floor) {
                    actions.push(a);
                }
            } else if force_spend {
                if let Some(a) = wave(view, from, plan.target, surplus, floor) {
                    actions.push(a);
                }
            } // else HOLD (amass)
            continue;
        }

        // (c) Siege holder (already grinding the target / a freshly captured sub): SUSTAIN so the
        //     heal cannot reset the grind — keep enough present, release only true overflow.
        if is_grind_site(view, from) {
            let heal = view.returning_owner_force(from);
            let keep = p
                .grind_hold_floor
                .max((p.heal_outlast_margin * heal as f32).ceil() as u32);
            let here = view.info(from).my_ships;
            let releasable = here.saturating_sub(keep);
            if releasable > 0 && force_spend {
                if let Some(a) = wave(view, from, plan.staging, releasable, floor) {
                    actions.push(a);
                }
            }
            continue;
        }

        // (d) Cheap DENIAL detach: park on a productive foreign sub to freeze it (Mechanic B),
        //     gated behind genuine production superiority so it cannot become the whole strategy.
        if superior {
            if let Some(dtarget) = pick_denial_target(view, plan.target) {
                if surplus >= p.denial_detach {
                    if let Some(a) = deny(view, from, dtarget, p.denial_detach, floor) {
                        actions.push(a);
                        continue;
                    }
                }
            }
        }

        // (e) Feeder: funnel surplus toward staging (mass); fall back to the target if staging is
        //     unreachable, so nothing freezes.
        let to = if view.reachable(from, plan.staging) {
            Some(plan.staging)
        } else if view.reachable(from, plan.target) {
            Some(plan.target)
        } else {
            None
        };
        if let Some(to) = to {
            if let Some(a) = wave(view, from, to, surplus, floor) {
                actions.push(a);
            }
        } else {
            let _ = hold();
        }
    }
    actions
}

/// The single siege plan Attack commits to: ONE target with foe presence, ONE staging position.
#[derive(Debug, Clone, Copy)]
struct SiegePlan {
    target: usize,
    staging: usize,
}

/// Choose the siege target (soft & productive & shallow & near = better) and a staging position
/// (owned position nearest the target). `None` if there is no reachable foe presence — pre-contact.
///
/// Target score uses only QUERY/property numbers: the projection's defenders (`present_count` of
/// the foe), the resistance to grind, the distance, and the producer count. Pure and deterministic.
fn plan_siege<V: PositionView>(view: &V) -> Option<SiegePlan> {
    let n = view.len();
    let mut best: Option<(usize, f32)> = None; // (id, cost; lower is better)
    for t in 0..n {
        if !foe_present(view, t) {
            continue;
        }
        // Must be reachable from at least one owned position; skip targets already settling mine.
        let reachable = (0..n).any(|o| owned_by_me(view, o) && o != t && view.reachable(o, t));
        if !reachable || settles_mine(view, t) {
            continue;
        }
        let foe = view.present_count(t, Side::Foe) as f32;
        let res = view.resistance(t);
        let dist = nearest_owned_dist(view, t).unwrap_or(0.0);
        // Soft & productive & shallow & near = lower cost. Weights are pure POLICY scoring (they
        // never enter a mechanic): defenders dominate, then distance, then a little resistance.
        let cost = foe + 0.10 * dist + 0.02 * res;
        match best {
            Some((_, bc)) if bc <= cost => {}
            _ => best = Some((t, cost)),
        }
    }
    let (target, _) = best?;
    let staging = nearest(view, target, |i| i.id != target && i.owner == PosOwner::Me)
        .or_else(|| (0..n).find(|&i| owned_by_me(view, i)))?;
    Some(SiegePlan { target, staging })
}

/// Is the spearhead READY to commit the siege at `target`? It must (1) field at least the
/// efficient winning force the projection sizes, (2) out-last the projected returning heal, and
/// (3) not be racing a flip the projection already calls (mine or the foe's). All QUERIES.
fn ready_to_commit<V: PositionView>(view: &V, p: &AttackParams, target: usize, here: usize) -> bool {
    // (3) Don't commit into a flip the projection already decides.
    if settles_mine(view, target) || foe_takes_first(view, target) {
        return false;
    }
    let have = view.info(here).my_ships;
    // (1) Enough to win the firefight efficiently (the projection sizes the force at our ratio).
    match view.force_for_efficiency(target, p.fight_efficiency) {
        Some(need) if have < need => return false,
        None => return false, // cannot win efficiently even overwhelming — keep amassing
        _ => {}
    }
    // (2) Out-last the heal: our committed force must beat the returning-owner heal by the margin.
    let heal = view.returning_owner_force(target);
    (have as f32) >= p.heal_outlast_margin * (heal.max(1) as f32)
}

/// Is `from` an active grind site: an owned (just-flipped) or contested position where I am
/// present and the projection has the position changing/contested within the horizon — i.e. a
/// place a hold must be SUSTAINED to out-erode the heal. A pure read of present force + projection.
fn is_grind_site<V: PositionView>(view: &V, from: usize) -> bool {
    let info = view.info(from);
    // I have ships here and there is a live fight or a projected change at this position.
    info.my_ships > 0 && (info.contested || view.capture_eta(from).is_some())
}

/// Pick a cheap DENIAL target: a productive foreign sub (foe present) reachable and distinct from
/// the main siege target, that the projection is NOT already settling mine. `None` if none.
fn pick_denial_target<V: PositionView>(view: &V, siege_target: usize) -> Option<usize> {
    (0..view.len()).find(|&t| {
        t != siege_target && foe_present(view, t) && !settles_mine(view, t)
    })
}

// =====================================================================================
// 4. Defend — heal-and-hold turtle (efficient defense, reinforce first-fall, colonize surplus).
// =====================================================================================

/// Policy tunables for [`defend`]. **All dials, no mechanics** (the defensive force it masses
/// comes from `force_for_efficiency`, an enemy-aware projection query).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DefendParams {
    /// Garrison floor. Policy dial.
    pub garrison_floor: u32,
    /// **Desired defensive combat efficiency** (defender:attacker casualty ratio) the threatened
    /// subs are massed to — *enough for an efficient defense, NOT infinite*. A policy dial fed into
    /// `force_for_efficiency`; the on-sub defender edge it exploits lives in the projection.
    pub desired_efficiency: f32,
    /// **Soft-cap spend slack** — treat ships as genuine surplus (free to colonize with) only once
    /// parked is within this many of the soft cap. Keeps the turtle from exporting its healers.
    /// Policy dial.
    pub softcap_spend_slack: u32,
    /// **Over-stack guard fraction** for the productive (colonize) branch. Policy dial.
    pub overstack_frac: f32,
    /// **Counter-punch cap** — the most ships the turtle commits *per source per tick* into an
    /// over-committed attacker's emptied rear (branch 2b). A small harassing trickle, NOT a
    /// conquering wave: it keeps the turtle's surplus MOVING (cap-exempt) so an aggressor's mobile
    /// hoard cannot simply out-mass it at the horizon, without turning the turtle into an
    /// out-expander that would also beat a colonizer. Policy dial.
    pub counter_punch_cap: u32,
}

impl Default for DefendParams {
    fn default() -> Self {
        DefendParams {
            garrison_floor: GARRISON_FLOOR,
            desired_efficiency: 10.0,
            softcap_spend_slack: 2,
            overstack_frac: 0.8,
            counter_punch_cap: 30,
        }
    }
}

/// **Defend — the program.**
///
/// ```text
/// threatened = { my position P : contested(P) or being_eroded(P) or capture_eta(P) within horizon }
/// if threatened not empty:
///     # reinforce the ONE that falls FIRST (soonest capture_eta), to the EFFICIENT force only
///     target = argmin capture_eta over threatened
///     need   = force_for_efficiency(target, DESIRED_EFFICIENCY) - present_mine(target)
///     from   = nearest owned-with-surplus that can reach target
///     wave(from -> target, min(surplus, need))      # NOT infinite: just enough to defend well
/// # army BEYOND what the threatened subs need at that efficiency COLONIZES the surplus —
/// for each owned FROM with surplus (ascending id):
///     if FROM is a needed reinforcer this tick: skip
///     # only spend the GENUINE surplus the cap would otherwise destroy (stay a turtle)
///     if not over_soft_cap(FROM, slack): continue
///     T = nearest foe-free neutral not settles-foe ; if T and not would_overstack(T):
///         wave(FROM -> T, surplus)                    # productive while defending
/// ```
///
/// **Identity.** A turtle that **stays productive**: garrison subs heal back to max and keep
/// producing; mass defenders to an *efficient* ratio (not infinite); reinforce the sub the
/// projection says falls first; colonize only the surplus the cap would destroy.
/// **Blind spot.** Opportunity cost — a pure colonizer out-expands it (the intended
/// colonize-beats-defend edge).
fn defend<V: PositionView>(view: &V, p: &DefendParams) -> Vec<GreedyAction> {
    let n = view.len();
    let mut actions = Vec::new();
    if n == 0 {
        return actions;
    }
    let floor = p.garrison_floor;

    // Am I at least at production PARITY (own >= as many positions as the foe)? The gate on the
    // counter-punch below: a turtle that is out-EXPANDED (strictly fewer positions, e.g. against a
    // colonizer) must NOT trade its surplus for the enemy's ground — that is the opponent's game and
    // is how it loses to Colonize. But at parity or ahead (the typical frozen board against an
    // over-committed Attacker) the turtle keeps its over-cap surplus MOVING into the enemy's thin
    // forward positions rather than bleeding it to the soft cap — punishing the over-extension.
    let not_outexpanded = {
        let mut mine = 0usize;
        let mut foe = 0usize;
        for i in 0..n {
            match view.info(i).owner {
                PosOwner::Me => mine += 1,
                PosOwner::Enemy => foe += 1,
                PosOwner::Neutral => {}
            }
        }
        mine >= foe
    };

    // (1) Threatened positions worth defending. Two cases, both "my own ground under threat":
    //     * an OWNED position the foe is eroding or the projection says falls within the horizon
    //       (the heal + on-sub defender edge make reinforcing it a winning tar-pit); OR
    //     * a CONTESTED position where I am the MAJORITY present (a fight on my ground I am holding)
    //       — a contested position reads as `Neutral`-owned, so this must NOT require `owned_by_me`.
    //     The majority gate is load-bearing: reinforcing a contested position where the foe already
    //     out-masses me (e.g. a neutral centre an attacker has seized) just feeds a losing brawl off
    //     my own ground and drains the home wall — so the turtle reinforces only fights it is winning
    //     locally, and otherwise holds/colonizes/counter-punches below.
    let mut threatened: Vec<usize> = (0..n)
        .filter(|&id| {
            let i = view.info(id);
            (owned_by_me(view, id) && (being_eroded(view, id) || falls_within(view, id)))
                || (i.contested && i.my_ships >= i.enemy_ships && i.my_ships > 0)
        })
        .collect();
    // Reinforce the ONE that falls FIRST (soonest capture ETA; contested-now sorts ahead of a
    // future ETA via a 0 key).
    threatened.sort_by_key(|&id| reinforce_urgency_key(view, id));

    let mut reinforcer: Option<usize> = None;
    if let Some(&target) = threatened.first() {
        // Mass to the EFFICIENT defensive force — enough, not infinite. The projection sizes the
        // force that beats the foe present here at the desired casualty ratio; if that ratio is
        // *infeasible* (the on-sub edge cannot deliver it even overwhelming), we still must hold,
        // so we fall back to simply OUT-MASSING the attacker by one (a bounded, non-infinite floor)
        // rather than freezing — the turtle never abandons a threatened sub.
        let present = view.present_count(target, Side::Me);
        let foe = view.present_count(target, Side::Foe);
        let need_total = view
            .force_for_efficiency(target, p.desired_efficiency)
            .unwrap_or_else(|| foe.saturating_add(1));
        let need = need_total.saturating_sub(present).max(if foe > present { 1 } else { 0 });
        if need > 0 {
            if let Some(from) = nearest(view, target, |i| {
                i.id != target && i.owner == PosOwner::Me && surplus_of(view, i.id, floor).is_some()
                    && view.reachable(i.id, target)
            }) {
                if let Some(a) = wave(view, from, target, need, floor) {
                    actions.push(a);
                    reinforcer = Some(from);
                }
            }
        }
        // Even if we could not field the reinforcement, stay defensive: do not also colonize from
        // the threatened set this tick (the turtle concentrates).
    }

    // (2) The army BEYOND the threatened subs' efficient need stays PRODUCTIVE while defending.
    for from in 0..n {
        if Some(from) == reinforcer {
            continue;
        }
        if !owned_by_me(view, from) || !view.can_export_from(from) {
            continue;
        }
        // Only spend the GENUINE over-cap surplus the soft cap would otherwise destroy (else hold
        // the reserve home, healing — the turtle's opportunity-cost blind spot vs a colonizer).
        if !over_soft_cap(view, from, soft_cap_spend_ratio(view, from, p.softcap_spend_slack)) {
            continue;
        }
        let Some(surplus) = surplus_of(view, from, floor) else { continue };

        // (2a) COLONIZE the nearest foe-free neutral the foe is not about to take.
        let tgt = nearest(view, from, |i| {
            matches!(i.owner, PosOwner::Neutral)
                && !foe_present(view, i.id)
                && !foe_takes_first(view, i.id)
                && view.reachable(from, i.id)
        });
        if let Some(tgt) = tgt {
            if !would_overstack(view, tgt, p.overstack_frac) {
                if let Some(a) = wave(view, from, tgt, surplus, floor) {
                    actions.push(a);
                }
            }
            continue;
        }

        // (2b) COUNTER-PUNCH (gated on parity). No neutral left to colonize but we hold genuine
        //      over-cap surplus the soft cap would DESTROY. If NOT out-expanded, keep it MOVING
        //      (cap-exempt transit) onto the WEAKEST reachable foe-bearing position — an
        //      over-committed attacker's emptied rear. Gated on parity so a turtle losing the
        //      land-grab (vs a colonizer, where mine < foe) does not chase enemy ground; below the
        //      cap it keeps its healing reserve home. (`counter_punch_cap` bounds a single source's
        //      commitment so a fat rear does not dump its whole stack down one lane.)
        if !not_outexpanded {
            continue;
        }
        if let Some(press) = weakest_foe(view, from) {
            let want = surplus.min(p.counter_punch_cap);
            if let Some(a) = wave(view, from, press, want, floor) {
                actions.push(a);
            }
        }
    }
    actions
}

// =====================================================================================
// Small shared helpers (pure reads of the view; NO mechanic re-derived).
// =====================================================================================

/// Is this owned position projected to flip (its owner changes within the horizon)? A thin read of
/// `capture_eta`; used by Defend's threat scan.
#[inline]
fn falls_within<V: PositionView>(view: &V, id: usize) -> bool {
    view.capture_eta(id).is_some()
}

/// Reinforce-urgency sort key for Defend: contested-now is most urgent (key 0), else the projected
/// capture ETA (sooner = smaller), else a large sentinel. Ties keep ascending id via the stable sort.
fn reinforce_urgency_key<V: PositionView>(view: &V, id: usize) -> u64 {
    if view.info(id).contested {
        0
    } else {
        view.capture_eta(id).unwrap_or(u64::MAX)
    }
}

/// A friendly position is "thin" (worth thickening as a SimpleColonizer fallback target) iff some
/// other owned position is strictly stronger than it — so surplus could flow toward it without a
/// pointless equal-strength swap. (Mirrors the greedy expand-target gate.)
fn is_thin_friendly<V: PositionView>(view: &V, id: usize) -> bool {
    let me = view.info(id).my_ships;
    (0..view.len()).any(|j| {
        let o = view.info(j);
        o.id != id && o.owner == PosOwner::Me && o.my_ships > me
    })
}

/// The WEAKEST reachable foe-bearing position from `from` (fewest enemy ships present; nearest on
/// ties), skipping any the projection already settles mine. The Defender's counter-punch target —
/// an over-committed attacker's emptied rear. `None` if no reachable foe position qualifies.
fn weakest_foe<V: PositionView>(view: &V, from: usize) -> Option<usize> {
    let mut best: Option<(usize, u32, f32)> = None; // (id, enemy_ships, dist)
    for to in 0..view.len() {
        if to == from || !foe_present(view, to) || settles_mine(view, to) || !view.reachable(from, to) {
            continue;
        }
        let e = view.info(to).enemy_ships;
        let d = view.distance(from, to).unwrap_or(f32::INFINITY);
        match best {
            Some((_, be, bd)) if (be, bd) <= (e, d) => {}
            _ => best = Some((to, e, d)),
        }
    }
    best.map(|(id, _, _)| id)
}

/// The nearest-owned distance to a target (for siege scoring); `None` if no owned position reaches it.
fn nearest_owned_dist<V: PositionView>(view: &V, target: usize) -> Option<f32> {
    let mut best: Option<f32> = None;
    for o in 0..view.len() {
        if !owned_by_me(view, o) || o == target {
            continue;
        }
        if let Some(d) = view.distance(o, target) {
            best = Some(best.map_or(d, |b| b.min(d)));
        }
    }
    best
}

/// The "from position" passed to the per-position `marginal_ticks_saved` query. At Layer 1 the
/// marginal query wants a *sub on the target's planet*; the colonize program reasons at view-id
/// granularity, so we hand it `from` directly (Layer-1 ids ARE subs of the one planet). At Layer 2
/// the Layer2View ignores this argument and resolves the per-sub from-position itself. Centralized
/// here so the program reads cleanly.
#[inline]
fn nearest_owned_sub_proxy<V: PositionView>(_view: &V, from: usize, _target: usize) -> usize {
    from
}

/// The parked-ratio threshold above which Defend treats ships as genuine surplus: `(soft_cap -
/// slack) / soft_cap`. Expressed as a ratio so it composes with [`over_soft_cap`]; the cap is the
/// accessor's number (no formula). Falls back to `1.0` (never surplus) when the cap is unknown.
fn soft_cap_spend_ratio<V: PositionView>(view: &V, id: usize, slack: u32) -> f32 {
    let cap = view.soft_cap_at(id);
    if cap == 0 || cap == u32::MAX {
        return 1.0;
    }
    (cap.saturating_sub(slack)) as f32 / cap as f32
}

#[cfg(test)]
mod tests;
