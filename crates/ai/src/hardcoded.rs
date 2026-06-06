//! # `ai::hardcoded` — the v1 HARDCODED strategic automata via projection-scored LOCAL SEARCH
//!
//! The Layer-2 engines behind **HardcodedColonize / HardcodedDefend / HardcodedAttack**. All three
//! are the **same** shared local-search allocation engine ([`allocate`]); only their **target set**,
//! **ship pool**, and **objective** differ — exactly the per-automaton recipe the project owner
//! specified:
//!
//! * **Colonize** — targets = capturable NEUTRAL ground (→ capturable ENEMY when no neutral remains);
//!   pool = all launchable surplus; objective = [`Objective::Capture`]. Pours surplus onto the ground
//!   it can take *fastest*, spreading across fronts (an extra ship on a capture already in hand scores
//!   ~0, so force never dies idle on a won fight). Never defends its own — its blind spot.
//! * **Attack** — targets = capturable ENEMY ground; pool = all launchable; objective =
//!   [`Objective::Capture`]. The same engine, restricted to enemy ground: it CONCENTRATES (the
//!   bundle-move discovers that one source cannot crack a defended sub alone) and abandons a fight it
//!   projects losing (a bundle that cannot flip a target scores 0 there).
//! * **Defend** — if any owned planet is contested OR enemy ships are inbound: targets = my planets +
//!   reachable enemy ground; pool = ALL ships (may spend the garrison floor); objective =
//!   [`Objective::Defend`] (lexicographic: hold contested ground first, then trade efficiently). Else:
//!   targets = NEUTRAL (→ ENEMY when none), pool = OVER-soft-cap surplus only, objective =
//!   [`Objective::Capture`] (behaves like Colonize with the genuine spare).
//!
//! ## The shared engine (the DESIGN's local search)
//!
//! State is an **assignment** `a[source] ∈ {Hold} ∪ {targets}`; a **bundle** is all sources sharing a
//! target. Local search explores two move classes and keeps the strictly-better neighbour until a
//! local maximum:
//!   * **single reassign** — set one source to Hold or to any reachable target;
//!   * **bundle move** — assign *all* currently-Hold reachable sources to a target at once (the
//!     multi-source move concentration needs: one source alone scores 0 on a sub it cannot flip, so
//!     single moves never discover a concentration — "all-Hold" would otherwise be a local maximum).
//! On a tie a **single reassign dominates a bundle move** (prefer the smaller commitment), and the
//! all-Hold assignment is the fallback when no target has positive contribution.
//!
//! ## Coordinated arrival (v1 stance)
//!
//! A bundle is **scored** as arriving at its target TOGETHER, at `sync = max(transit of its sources)`
//! — and **issued** "launch all now" (the DESIGN's explicit v1 fallback). Because the shared forward
//! [`world::Projection`] already folds in MY in-flight ships from prior ticks, the per-tick re-search
//! re-assembles the strike with no cross-tick state; the grind still accumulates if the arrival is
//! slightly staggered. (A robustness margin for contested / known-inbound targets is a v2 item — see
//! `AUTOMATA_DESIGN.md`.)
//!
//! ## Determinism + the no-raw-mechanic contract
//!
//! Every mechanic question (how much force wins, when a sub flips, who a planet settles to, the
//! combat exchange) is a [`world::Projection`] query — this file names only **policy** dials
//! ([`WIN_RATIO`], [`EASE_WEIGHT`]). The search is **single-pass** over the *one* projection the
//! controller built this tick (it never clones the world or re-projects per candidate), so a decision
//! is ~tens-to-low-hundreds of integer-keyed ordering ops — well within budget. All `f64`/transit
//! sorts carry an explicit `.then(id.cmp())` planet-id tie-break and all membership sets are
//! `BTreeSet`, so the assignment — and thus `World::state_hash` — is bit-identical across runs.

use layer1::{Faction, FractionBucket, SimParams};
use world::{FleetOrder, PlanetId, PlanetOwner, Projection, World, WorldParams};

use crate::adapters::Layer2View;
use crate::greedy::PositionView;

/// Defender:attacker casualty ratio a capture/hold is sized to win at (fed into the projection's
/// `force_for_efficiency`). Modest — enough to win the firefight efficiently, not infinite.
const WIN_RATIO: f32 = 1.5;

/// How strongly Attack tempers *proximity* with *ease of battle* when ranking enemy targets: a
/// target's tie-break cost is `transit + EASE_WEIGHT * force_to_win`. Higher ⇒ prefers the softer
/// fight even if farther. A policy dial; it only orders otherwise-equal-objective candidates.
const EASE_WEIGHT: f64 = 4.0;

// =====================================================================================
// The three public recipes (the wiring in `strategy.rs` calls these; signatures fixed).
// =====================================================================================

/// **HardcodedColonize.** Pour every secure planet's surplus onto the ground it can take fastest:
/// capturable NEUTRAL first, and — when no neutral remains anywhere — capturable ENEMY ground. It
/// never reinforces its own ground (its blind spot: undefended production).
pub fn colonize(world: &World, seat: Faction, sp: &SimParams, wp: &WorldParams, proj: &Projection) -> Vec<FleetOrder> {
    let view = Layer2View::with_projection(world, seat, proj, sp, wp);
    let neutrals = capturable(world, seat, proj, |agg, _| matches!(agg.owner, PlanetOwner::Neutral));
    let targets = if neutrals.is_empty() {
        capturable(world, seat, proj, |agg, foe| is_foe_ground(agg, foe))
    } else {
        neutrals
    };
    let sources = secure_sources(world, seat, wp);
    allocate(world, seat, &view, proj, &sources, &targets, Objective::Capture)
}

/// **HardcodedAttack.** Capture ENEMY ground with the shared engine restricted to enemy targets:
/// it concentrates surplus (the bundle-move) onto the target that buys the most, tempered toward the
/// softer fight, and abandons a fight it projects losing (a bundle that cannot flip scores nothing).
pub fn attack(world: &World, seat: Faction, sp: &SimParams, wp: &WorldParams, proj: &Projection) -> Vec<FleetOrder> {
    let view = Layer2View::with_projection(world, seat, proj, sp, wp);
    let targets = capturable(world, seat, proj, |agg, foe| is_foe_ground(agg, foe));
    let sources = secure_sources(world, seat, wp);
    allocate(world, seat, &view, proj, &sources, &targets, Objective::Capture)
}

/// **HardcodedDefend.** If a planet of mine is contested OR enemy ships are inbound, hold: targets =
/// my (threatened) planets + reachable enemy ground, pool = ALL ships (it may spend the garrison
/// floor to save ground), objective = lexicographic [`Objective::Defend`] (keep contested planets
/// mine first, then trade efficiently). Otherwise it is a colonizer of its **genuine over-soft-cap**
/// surplus only (below the cap it keeps its structures topped, healing).
pub fn defend(world: &World, seat: Faction, sp: &SimParams, wp: &WorldParams, proj: &Projection) -> Vec<FleetOrder> {
    let view = Layer2View::with_projection(world, seat, proj, sp, wp);

    if under_threat(world, seat, proj) {
        // Hold: spend ALL ships (garrison floor included) on my threatened planets + reachable enemy
        // ground, scored to keep my contested ground mine and trade well.
        let sources = all_sources(world, seat);
        let mut targets = my_threatened(world, seat, proj);
        for t in capturable(world, seat, proj, |agg, foe| is_foe_ground(agg, foe)) {
            if !targets.iter().any(|d| d.planet == t.planet) {
                targets.push(t);
            }
        }
        allocate(world, seat, &view, proj, &sources, &targets, Objective::Defend)
    } else {
        // Quiet board: colonize with only the genuine saturated spare (the cap would bleed it),
        // grabbing safe neutral ground first and only pressing the enemy when no neutral remains —
        // the turtle keeps a healing wall and takes free territory, but does not over-extend into a
        // fight on a quiet board (that caution is its identity; the opportunity-cost it pays is the
        // documented blind spot a pure colonizer out-expands).
        let neutrals = capturable(world, seat, proj, |agg, _| matches!(agg.owner, PlanetOwner::Neutral));
        let targets = if neutrals.is_empty() {
            capturable(world, seat, proj, |agg, foe| is_foe_ground(agg, foe))
        } else {
            neutrals
        };
        let sources = overcap_sources(world, seat, sp);
        allocate(world, seat, &view, proj, &sources, &targets, Objective::Capture)
    }
}

// =====================================================================================
// The shared LOCAL-SEARCH allocation engine.
// =====================================================================================

/// What an allocation is scored by. The only thing that differs between the three automata (beyond
/// their target set + ship pool).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Objective {
    /// **V_capture**: reward capturing many targets fast — `Σ max(0, horizon − tick(T flips to me))`
    /// under the assignment's bundle force. An extra ship on a flip already in hand adds ~0, so force
    /// spreads instead of dying idle, and a bundle that cannot flip its target contributes nothing
    /// (the self-flip / under-force gate).
    Capture,
    /// **L_defend** (lexicographic): (1) my planets still mine at the horizon UP; (2) kill-efficiency
    /// UP; then, as an intra-tier cost dial, (3) my ships in transit at the horizon DOWN and (4)
    /// total ships moved DOWN. The strategic goal (hold contested ground) dominates; "ships moved"
    /// is only a tie-break, never above holding a planet.
    Defend,
}

/// One candidate target for the engine: the planet plus the **foothold sub** a spearhead actually
/// cracks (least-resistance foreign sub) — the sub all the projection what-ifs are sized against.
#[derive(Clone, Copy, Debug)]
struct Target {
    planet: PlanetId,
    foothold: usize,
}

/// A scored assignment's value — a small lexicographic tuple so both objectives compare with one
/// `PartialOrd`. For [`Objective::Capture`] only `primary` is used; for [`Objective::Defend`] all
/// four fields carry the lexicographic order (`planets_held`, `kill_eff`, `−in_transit`,
/// `−ships_moved`).
#[derive(Clone, Copy, PartialEq, Debug)]
struct Score {
    primary: f64,
    secondary: f64,
    tertiary: f64,
    quaternary: f64,
}

impl Score {
    /// Strict lexicographic "better than", with the same total order the local search argmaxes over.
    fn better_than(&self, other: &Score) -> bool {
        const EPS: f64 = 1e-9;
        for (a, b) in [
            (self.primary, other.primary),
            (self.secondary, other.secondary),
            (self.tertiary, other.tertiary),
            (self.quaternary, other.quaternary),
        ] {
            if a > b + EPS {
                return true;
            }
            if b > a + EPS {
                return false;
            }
        }
        false
    }
}

/// The shared engine: local search over a `source → {Hold} ∪ targets` assignment, scored against the
/// **one** shared projection, then issued (launch-all-now, coordinated scoring). Returns the
/// [`FleetOrder`]s for the tick (first-hop routed). Deterministic: candidates are generated in id
/// order, the argmax keeps the first strict improvement, and a single reassign beats a bundle move on
/// a tie (prefer the smaller commitment).
fn allocate(
    world: &World,
    seat: Faction,
    view: &Layer2View,
    proj: &Projection,
    sources: &[(PlanetId, u32)],
    targets: &[Target],
    obj: Objective,
) -> Vec<FleetOrder> {
    if sources.is_empty() || targets.is_empty() {
        return Vec::new();
    }
    let ctx = Ctx { world, seat, view, proj, sources, targets, obj };

    // Assignment: index by source-index; None = Hold, Some(t) = target-index. Start all-Hold.
    let mut assign: Vec<Option<usize>> = vec![None; sources.len()];
    let mut cur = ctx.score(&assign);

    // Local search: pick the single best neighbour; stop when none strictly improves. Each pass is
    // O(sources·targets) candidates; the small maps cap the pass count, but a hard guard keeps it
    // provably terminating regardless.
    let max_passes = (sources.len() * (targets.len() + 1)).max(1) + 1;
    for _ in 0..max_passes {
        let mut best = assign.clone();
        let mut best_score = cur;
        let mut improved = false;

        // (1) SINGLE REASSIGN: each source → Hold or any reachable target.
        for s in 0..sources.len() {
            let prev = assign[s];
            // Hold.
            if prev.is_some() {
                assign[s] = None;
                consider(&ctx, &assign, &mut best, &mut best_score, &mut improved);
            }
            // Each reachable target.
            for (ti, t) in targets.iter().enumerate() {
                if Some(ti) == prev {
                    continue;
                }
                if !ctx.reaches(s, t) {
                    continue;
                }
                assign[s] = Some(ti);
                consider(&ctx, &assign, &mut best, &mut best_score, &mut improved);
            }
            assign[s] = prev; // restore before trying the next source
        }

        // (2) BUNDLE MOVE: assign ALL currently-Hold reachable sources to one target at once. On a
        //     score tie this loses to the single reassigns above (they were considered first with
        //     strict `better_than`), honouring "prefer the smaller commitment".
        for (ti, t) in targets.iter().enumerate() {
            let mut cand = assign.clone();
            let mut any = false;
            for s in 0..sources.len() {
                if cand[s].is_none() && ctx.reaches(s, t) {
                    cand[s] = Some(ti);
                    any = true;
                }
            }
            if any {
                consider_owned(&ctx, cand, &mut best, &mut best_score, &mut improved);
            }
        }

        if improved {
            assign = best;
            cur = best_score;
        } else {
            break; // local maximum — all-Hold is the fallback if nothing scored positive.
        }
    }

    issue(&ctx, &assign)
}

/// Immutable context threaded through the search (so the helpers stay small and borrow-clean).
struct Ctx<'a> {
    world: &'a World,
    seat: Faction,
    view: &'a Layer2View<'a>,
    proj: &'a Projection,
    sources: &'a [(PlanetId, u32)],
    targets: &'a [Target],
    obj: Objective,
}

impl<'a> Ctx<'a> {
    /// Can source-index `s` reach target `t` (a lane path exists and it is not the source's own
    /// planet)? Routing is one-hop-at-a-time, so "reachable" is the multi-lane path existing.
    fn reaches(&self, s: usize, t: &Target) -> bool {
        let from = self.sources[s].0;
        from != t.planet && self.view.reachable(from, t.planet)
    }

    /// Transit ticks from source-index `s` to target `t` (large sentinel if unreachable).
    fn transit(&self, s: usize, t: &Target) -> u64 {
        self.view.transit_ticks(self.sources[s].0, t.planet).unwrap_or(u64::MAX)
    }

    /// Score an assignment under the active objective.
    fn score(&self, assign: &[Option<usize>]) -> Score {
        match self.obj {
            Objective::Capture => self.score_capture(assign),
            Objective::Defend => self.score_defend(assign),
        }
    }

    /// **V_capture**: sum over targets receiving a bundle of `max(0, horizon − flip_eta)`, where the
    /// flip ETA is the projection's marginal what-if for the bundle's pooled force arriving together
    /// (coordinated) at `sync = max source transit`. A bundle that cannot flip its target inside the
    /// horizon contributes 0 (the self-flip / under-force gate, logic-fix #1). `primary` only.
    fn score_capture(&self, assign: &[Option<usize>]) -> Score {
        let horizon = self.proj.base_tick + self.proj.horizon;
        let mut total = 0.0f64;
        // Two tie-break keys under the V_capture primary:
        //  * `committed` (secondary, UP) — total surplus committed to targets the bundle actually
        //    flips. Rewarding commitment makes the engine **never hoard idle surplus** behind a
        //    winnable fight: Colonize keeps pushing every source at takeable ground (maximal
        //    expansion), and Attack — with only enemy targets — pours its whole army onto the single
        //    best one (concentration, *accepting the over-commitment* that is its identity + blind
        //    spot). Gated on a real flip, so a source is never credited for piling onto ground its
        //    force cannot crack.
        //  * `−cost` (tertiary, ease/proximity) — among equal value+commitment, prefer the nearer /
        //    softer fights (`transit + EASE_WEIGHT * force_to_win`): Attack's "proximity tempered with
        //    ease of battle", Colonize's nearest-first. A pure flavour tie-break; deterministic
        //    (integer-keyed reads, lowest id breaks transit ties).
        let mut committed = 0.0f64;
        let mut cost = 0.0f64;
        for (ti, t) in self.targets.iter().enumerate() {
            let (force, sync) = self.bundle_force_and_sync(assign, ti, t);
            if force == 0 {
                continue;
            }
            // The flip tick the foothold would reach if `force` of mine arrived (coordinated) in
            // `sync` ticks — the projection answers the mechanic, sized at the cheapest sub.
            if let Some(eta) = self.proj.capture_eta_if(t.planet, t.foothold, force, sync, self.seat) {
                total += horizon.saturating_sub(eta) as f64;
                committed += self.bundle_surplus(assign, ti, t) as f64;
                let nearest_transit = self.min_transit_to(assign, ti, t);
                let need = self.view.force_for_efficiency(t.planet, WIN_RATIO).unwrap_or(0) as f64;
                cost += nearest_transit + EASE_WEIGHT * need;
            }
            // eta == None ⇒ the bundle does not flip it within the horizon ⇒ contributes nothing
            // (do not credit a self-flip / under-forced target).
        }
        Score { primary: total, secondary: committed, tertiary: -cost, quaternary: 0.0 }
    }

    /// The pooled *source surplus* (my outbound ships only, not present/incoming) committed to
    /// target-index `ti` — the commitment-reward input.
    fn bundle_surplus(&self, assign: &[Option<usize>], ti: usize, t: &Target) -> u32 {
        let mut pooled = 0u32;
        for (s, a) in assign.iter().enumerate() {
            if *a == Some(ti) && self.reaches(s, t) {
                pooled += self.sources[s].1;
            }
        }
        pooled
    }

    /// The minimum transit among the sources assigned to target-index `ti` (the bundle's leading
    /// edge), as `f64`; `0.0` if the bundle is empty.
    fn min_transit_to(&self, assign: &[Option<usize>], ti: usize, t: &Target) -> f64 {
        let mut best = f64::INFINITY;
        for (s, a) in assign.iter().enumerate() {
            if *a == Some(ti) && self.reaches(s, t) {
                best = best.min(self.transit(s, t) as f64);
            }
        }
        if best.is_finite() {
            best
        } else {
            0.0
        }
    }

    /// **L_defend** (lexicographic). (1) planets still mine at horizon under this assignment; (2)
    /// kill-efficiency of the holds (foe losses / my losses, from `expected_combat_timeline`); (3)
    /// my ships still in transit at horizon, *down* (a cost dial, never above holding ground); (4)
    /// ships moved, *down* (the smallest tie-break).
    fn score_defend(&self, assign: &[Option<usize>]) -> Score {
        let mut planets_held = 0.0f64;
        let mut foe_losses_tot = 0.0f64;
        let mut my_losses_tot = 0.0f64;
        let mut in_transit = 0.0f64;
        let mut ships_moved = 0.0f64;

        for (ti, t) in self.targets.iter().enumerate() {
            let (force, sync) = self.bundle_force_and_sync(assign, ti, t);
            let mine_now = matches!(
                self.world.planet_aggregate(t.planet).owner,
                PlanetOwner::Owned(f) if f == self.seat
            ) || (matches!(self.world.planet_aggregate(t.planet).owner, PlanetOwner::Contested)
                && self.world.planet_aggregate(t.planet).ships_of(self.seat) > 0);

            // (1) Held? A planet I (partly) hold stays mine if, with this bundle's reinforcement, the
            //     projection no longer flips its foothold to the foe within the horizon. Enemy ground
            //     I am assaulting "counts as held" once the bundle flips it to me. A *threatened* own
            //     planet counts as held only when the reinforcement actually arrives (force > 0) and
            //     brings at least local parity — so committing to the hold is strictly better than
            //     sitting all-Hold and being ground down (the defender edge then wins the firefight).
            let held = if mine_now {
                self.holds_after(t, force, sync)
            } else {
                // assaulting enemy ground: held iff the bundle flips it to me
                self.proj.capture_eta_if(t.planet, t.foothold, force, sync, self.seat).is_some() && force > 0
            };
            if held {
                planets_held += 1.0;
            }

            // (2) Kill-efficiency of the engagement at this planet, if contested/assaulted.
            if force > 0 || mine_now {
                let (ml, fl) = self.engagement_losses(t, force, sync);
                my_losses_tot += ml as f64;
                foe_losses_tot += fl as f64;
            }
        }

        // (3)/(4) transit + ships-moved cost dials, summed over the assignment's bundles.
        for (s, a) in assign.iter().enumerate() {
            if let Some(ti) = a {
                if self.reaches(s, &self.targets[*ti]) {
                    let surplus = self.sources[s].1 as f64;
                    ships_moved += surplus;
                    in_transit += surplus; // launched this tick ⇒ in transit at the (near) horizon
                }
            }
        }

        let kill_eff = foe_losses_tot / my_losses_tot.max(1.0);
        Score {
            primary: planets_held,
            secondary: kill_eff,
            tertiary: -in_transit,
            quaternary: -ships_moved,
        }
    }

    /// The pooled force + coordinated sync-tick of the bundle assigned to target-index `ti`: every
    /// source assigned to `ti` that can reach it contributes its surplus; plus my force already
    /// present at the foothold and my in-flight arrivals the projection expects there. `sync` is the
    /// **max** transit among the bundle's sources (coordinated simultaneous arrival), `0` if empty.
    fn bundle_force_and_sync(&self, assign: &[Option<usize>], ti: usize, t: &Target) -> (u32, u64) {
        let mut pooled = 0u32;
        let mut sync = 0u64;
        for (s, a) in assign.iter().enumerate() {
            if *a == Some(ti) && self.reaches(s, t) {
                pooled += self.sources[s].1;
                sync = sync.max(self.transit(s, t));
            }
        }
        if pooled == 0 {
            return (0, 0);
        }
        // Fold in my force already present at the foothold + my in-flight arrivals there (so the
        // engine never double-sends to a sub its own ships already settle).
        let present = self.proj.present_now(t.planet, t.foothold);
        let mine_present = match self.seat {
            Faction::Player => present.0,
            Faction::Enemy => present.1,
            Faction::Neutral => 0,
        };
        let incoming = self.proj.incoming_present_at(t.planet, t.foothold, self.seat);
        (pooled + mine_present + incoming, sync)
    }

    /// Does my planet `t` stay mine after a `force`-ship reinforcement arrives in `sync` ticks?
    ///
    /// If the projection does not project any owned sub to fall to the foe within the horizon, it
    /// already holds (no reinforcement needed). Otherwise it counts as held when my present force
    /// **plus** the reinforcement out-sizes the foe present at the contested foothold — local
    /// **parity** (`>=`), which the on-sub defender edge (`defender_in_own_sub`) then tips into a won
    /// firefight. Crucially the threatened planet only counts as held once the reinforcement is
    /// actually committed (`force > 0`): a planet projected to fall does NOT count as held while the
    /// defender sits all-Hold, so committing the garrison to repel the strike is strictly better than
    /// being ground down. `sync` is arrival-time-independent for this parity test (kept uniform).
    fn holds_after(&self, t: &Target, force: u32, _sync: u64) -> bool {
        if self.proj.planet_first_fall(t.planet, self.seat).is_none() {
            return true; // nothing projected to fall — already holding, no reinforcement needed
        }
        if force == 0 {
            return false; // projected to fall and I send nothing ⇒ it falls (must commit to hold it)
        }
        let foe_present = self.foe_present(t);
        // Local parity with the defender's on-sub edge: my present + reinforcement >= the foe present
        // there. (The projection's `force_for_efficiency` sizes the *efficient* 1.5× hold; for the
        // strategic tier-1 "does it stay mine" we accept the cheaper parity hold the on-sub edge wins,
        // so the turtle commits to ground it can actually keep rather than abandoning it.)
        self.mine_present(t) + force >= foe_present
    }

    /// The foe's present force at a target's foothold sub (the attackers my hold must out-last).
    fn foe_present(&self, t: &Target) -> u32 {
        let present = self.proj.present_now(t.planet, t.foothold);
        match self.seat {
            Faction::Player => present.1,
            Faction::Enemy => present.0,
            Faction::Neutral => 0,
        }
    }

    /// My present force at a target's foothold sub (idle defenders the projection seeded).
    fn mine_present(&self, t: &Target) -> u32 {
        let present = self.proj.present_now(t.planet, t.foothold);
        match self.seat {
            Faction::Player => present.0,
            Faction::Enemy => present.1,
            Faction::Neutral => 0,
        }
    }

    /// The `(my_losses, foe_losses)` of the engagement at `t` if my bundle (`force`, arriving in
    /// `sync`) reinforces it, via the world-side combat-timeline (the square law lives in `world`).
    /// My side holds its own ground (the on-sub defender edge). The foe present is read from the
    /// projection; my reinforcement enters as a scheduled `MyArrival` event.
    fn engagement_losses(&self, t: &Target, force: u32, sync: u64) -> (u32, u32) {
        let present = self.proj.present_now(t.planet, t.foothold);
        let (mine, foe) = match self.seat {
            Faction::Player => (present.0, present.1),
            Faction::Enemy => (present.1, present.0),
            Faction::Neutral => (0, 0),
        };
        let mut events: Vec<(u64, world::CombatEvent)> = Vec::new();
        if force > 0 {
            events.push((sync, world::CombatEvent::MyArrival(force)));
        }
        self.proj.expected_combat_timeline(mine, foe, true, &events)
    }
}

/// Consider a *borrowed* candidate assignment: score it and, if it strictly beats `best_score`,
/// record a clone as the new best.
fn consider(
    ctx: &Ctx,
    cand: &[Option<usize>],
    best: &mut Vec<Option<usize>>,
    best_score: &mut Score,
    improved: &mut bool,
) {
    let s = ctx.score(cand);
    if s.better_than(best_score) {
        *best = cand.to_vec();
        *best_score = s;
        *improved = true;
    }
}

/// Consider an *owned* candidate assignment (the bundle move builds a fresh `Vec`), avoiding a clone
/// on the common reject path.
fn consider_owned(
    ctx: &Ctx,
    cand: Vec<Option<usize>>,
    best: &mut Vec<Option<usize>>,
    best_score: &mut Score,
    improved: &mut bool,
) {
    let s = ctx.score(&cand);
    if s.better_than(best_score) {
        *best = cand;
        *best_score = s;
        *improved = true;
    }
}

/// Issue the chosen assignment as [`FleetOrder`]s. **Launch-all-now** (the v1 coordinated-arrival
/// fallback): every assigned source ships its whole surplus toward its target's first hop this tick;
/// the per-tick re-search re-forms the coordinated strike as nearer sources' windows open. Orders are
/// emitted in source-id then target-id order (deterministic).
///
/// **Dead-source handling (REACHABILITY fix).** The local search only ever *assigns* a source to a
/// target it can reach (every move is gated by [`Ctx::reaches`]) and it explores *all* reachable
/// targets, so it already prefers the next-best reachable target before leaving a source on Hold —
/// a source is never committed to ground it cannot route to. A first-hop that fails to resolve here
/// is therefore unreachable: it cannot happen on a static decision tick, and is asserted in debug
/// builds so a routing regression is loud rather than a silent "force committed nowhere"; in release
/// the source simply holds (emits nothing) instead of producing an invalid order.
fn issue(ctx: &Ctx, assign: &[Option<usize>]) -> Vec<FleetOrder> {
    let mut orders = Vec::new();
    for (s, a) in assign.iter().enumerate() {
        let Some(ti) = a else { continue };
        let t = ctx.targets[*ti];
        let from = ctx.sources[s].0;
        match crate::graph::next_hop(ctx.world, from, t.planet) {
            Some(hop) => orders.push(FleetOrder::new(from, hop, FractionBucket::All)),
            None => debug_assert!(
                false,
                "assigned source {from} cannot route to target planet {} — the search should only \
                 assign reachable targets (force committed nowhere)",
                t.planet
            ),
        }
    }
    orders
}

// =====================================================================================
// Target-set + ship-pool builders (Layer-2 reads + projection gating; no mechanic re-derived).
// =====================================================================================

/// Planets matching `pred(agg, foe)` that `seat` does not already own **and the projection does not
/// already settle to `seat`** — no point piling onto a capture in hand (PROJECTION WIN-GATING, the
/// v1 behavioural fix). Each kept planet is paired with the foothold sub a spearhead cracks first.
fn capturable(
    world: &World,
    seat: Faction,
    proj: &Projection,
    pred: impl Fn(&world::PlanetAggregate, Faction) -> bool,
) -> Vec<Target> {
    let foe = seat.opponent();
    (0..world.planets.len())
        .filter_map(|p| {
            let agg = world.planet_aggregate(p);
            if matches!(agg.owner, PlanetOwner::Owned(f) if f == seat) {
                return None;
            }
            if !pred(&agg, foe) {
                return None;
            }
            // WIN-GATING: drop any target the passive projection already flips to me — my in-flight
            // ships settle it, so a fresh wave is wasted.
            if matches!(proj.planet_capture(p), Some((f, _)) if f == seat) {
                return None;
            }
            foothold_sub(world, p, seat).map(|foothold| Target { planet: p, foothold })
        })
        .collect()
}

/// My planets that are **threatened** (contested now, or the projection hands a sub to the foe) and
/// still worth holding, paired with the sub that falls first (the one a reinforcement should cover).
/// A threatened sub the projection's own in-flight owner force already covers is skipped (the
/// `returning_owner_force` / `incoming_present_at` gate from the win-gating fix).
fn my_threatened(world: &World, seat: Faction, proj: &Projection) -> Vec<Target> {
    (0..world.planets.len())
        .filter_map(|p| {
            let agg = world.planet_aggregate(p);
            let mine = matches!(agg.owner, PlanetOwner::Owned(f) if f == seat)
                || (matches!(agg.owner, PlanetOwner::Contested) && agg.ships_of(seat) > 0);
            if !mine {
                return None;
            }
            // The sub projected to fall first to the foe (if any); contested-now without a projected
            // fall still counts (we defend the cheapest foreign/own foothold).
            let fall = proj.planet_first_fall(p, seat);
            let contested = matches!(agg.owner, PlanetOwner::Contested);
            if fall.is_none() && !contested {
                return None; // not actually threatened within the horizon
            }
            let foothold = match fall {
                Some((sub, _)) => {
                    // Skip if my own returning/in-flight force already covers this sub (don't
                    // double-send into a hold the projection already settles).
                    let covered = proj.returning_owner_force(p, sub) > 0
                        && matches!(proj.planet_capture(p), Some((f, _)) if f == seat);
                    if covered {
                        return None;
                    }
                    sub
                }
                None => first_owned_or_zero(world, p, seat),
            };
            Some(Target { planet: p, foothold })
        })
        .collect()
}

/// Is `seat` under threat *anywhere* — a contested owned planet, or an enemy fleet inbound to one of
/// its planets? Gates Defend's hold mode vs its quiet-board colonize mode.
fn under_threat(world: &World, seat: Faction, proj: &Projection) -> bool {
    let foe = seat.opponent();
    (0..world.planets.len()).any(|p| {
        let agg = world.planet_aggregate(p);
        let mine_contested = matches!(agg.owner, PlanetOwner::Contested) && agg.ships_of(seat) > 0;
        // An enemy sub of mine projected to fall, or an enemy fleet already inbound to my planet.
        let mine = matches!(agg.owner, PlanetOwner::Owned(f) if f == seat) || mine_contested;
        let projected_fall = mine && proj.planet_first_fall(p, seat).is_some();
        let enemy_inbound = mine && agg.ships_of(foe) > 0;
        mine_contested || projected_fall || enemy_inbound
    })
}

/// Is this aggregate **foe ground** (enemy-owned, or contested with the foe present)?
fn is_foe_ground(agg: &world::PlanetAggregate, foe: Faction) -> bool {
    matches!(agg.owner, PlanetOwner::Owned(f) if f == foe)
        || (matches!(agg.owner, PlanetOwner::Contested) && agg.ships_of(foe) > 0)
}

/// The cheapest **foreign foothold** sub on planet `p` for `seat` (the least-resistance not-mine sub
/// a spearhead cracks first), or `None` if the planet has no foreign sub. Read straight off the
/// structure's resistances (the projection sizes the force against this sub).
fn foothold_sub(world: &World, p: PlanetId, seat: Faction) -> Option<usize> {
    let planet = world.planets.get(p)?;
    let st = &planet.structure;
    let mut best: Option<(usize, f32)> = None;
    for s in 0..st.subs.len() {
        if st.subs[s].owner == seat {
            continue;
        }
        let r = st.sub_resistance(s).0;
        best = Some(match best {
            // Strictly-less keeps the lowest-id sub on a tie (deterministic).
            Some((bs, br)) if br <= r => (bs, br),
            _ => (s, r),
        });
    }
    best.map(|(s, _)| s)
}

/// The lowest-id sub `seat` owns on `p`, or 0 (the foothold for a contested own planet with no
/// projected fall — we cover the seat's own ground).
fn first_owned_or_zero(world: &World, p: PlanetId, seat: Faction) -> usize {
    let Some(planet) = world.planets.get(p) else { return 0 };
    (0..planet.structure.subs.len())
        .find(|&s| planet.structure.subs[s].owner == seat)
        .unwrap_or(0)
}

/// Secure source planets (fully owned & uncontested) with exportable surplus above `keep_floor`, as
/// `(planet, surplus)`. The pool Colonize / Attack / quiet-Defend draw from.
fn secure_sources(world: &World, seat: Faction, wp: &WorldParams) -> Vec<(PlanetId, u32)> {
    (0..world.planets.len())
        .filter_map(|p| {
            let agg = world.planet_aggregate(p);
            if matches!(agg.owner, PlanetOwner::Owned(f) if f == seat) && agg.fully_owned_uncontested(seat) {
                let s = exportable_surplus(world, p, seat, wp.keep_floor);
                if s > 0 {
                    return Some((p, s));
                }
            }
            None
        })
        .collect()
}

/// ALL of `seat`'s deployable ships, floor included — the Defend hold pool (it may spend the garrison
/// floor to save ground). Drawn only from securely-held planets (the export gate still forbids
/// launching out of a contested planet), but counting *every* idle ship, not just the above-floor
/// surplus.
fn all_sources(world: &World, seat: Faction) -> Vec<(PlanetId, u32)> {
    (0..world.planets.len())
        .filter_map(|p| {
            let agg = world.planet_aggregate(p);
            if matches!(agg.owner, PlanetOwner::Owned(f) if f == seat) && agg.fully_owned_uncontested(seat) {
                let s = exportable_surplus(world, p, seat, 0); // floor 0 ⇒ all idle ships
                if s > 0 {
                    return Some((p, s));
                }
            }
            None
        })
        .collect()
}

/// Source planets that are **saturated** — parked at or above their soft cap, where the anti-hoard
/// attrition is actively bleeding their production — paired with their above-`keep_floor` exportable
/// surplus. The quiet-board Defend colonize pool.
///
/// This is the recipe's "spend only the surplus the soft cap would otherwise destroy", read at the
/// **cap edge** rather than strictly above it: a home pinned exactly at the cap is bleeding every
/// tick (production pushes it over, the `sqrt` attrition knocks it back), so the genuine spare it
/// should colonize with is its above-floor surplus *once saturated*. Below the cap it returns
/// nothing — the turtle keeps its healing reserve home (it does NOT over-expand early; that is what
/// preserves the `colonize > defend` opportunity-cost edge, since a pure colonizer expands from tick
/// 0 while the turtle only grabs ground once its wall is full).
fn overcap_sources(world: &World, seat: Faction, sp: &SimParams) -> Vec<(PlanetId, u32)> {
    (0..world.planets.len())
        .filter_map(|p| {
            let agg = world.planet_aggregate(p);
            if !(matches!(agg.owner, PlanetOwner::Owned(f) if f == seat) && agg.fully_owned_uncontested(seat)) {
                return None;
            }
            // Saturated? (parked has reached the cap the attrition bleeds against.)
            if world.parked_count(p, seat) < world.soft_cap(p, seat, sp) {
                return None;
            }
            // Spend the above-floor surplus (keep a home guard), not just the instantaneous overflow
            // (which the bleed already shaved to ~0 by decision time).
            let surplus = exportable_surplus(world, p, seat, WorldParams::default().keep_floor);
            if surplus > 0 {
                Some((p, surplus))
            } else {
                None
            }
        })
        .collect()
}

/// Exportable surplus of `seat` on `p`: idle ships above `keep_floor` on the subs it owns.
fn exportable_surplus(world: &World, p: PlanetId, seat: Faction, keep_floor: usize) -> u32 {
    let Some(planet) = world.planets.get(p) else { return 0 };
    let st = &planet.structure;
    let mut total = 0u32;
    for s in 0..st.subs.len() {
        if st.subs[s].owner != seat {
            continue;
        }
        let idle = st.idle_count_at(s, seat);
        if idle > keep_floor {
            total += (idle - keep_floor) as u32;
        }
    }
    total
}

#[cfg(test)]
mod tests {
    use super::*;
    use layer1::{Structure, SubStructure, Vec2};
    use world::Planet;

    fn sim() -> SimParams {
        SimParams::default()
    }

    /// A fully-owned Player planet with `ships` idle ships on a single Player sub.
    fn home(seed: u64, ships: usize, pos: Vec2, name: &str) -> Planet {
        let mut st = Structure::new(seed);
        let s = st.add_sub(SubStructure::new(Vec2::new(0.0, 0.0), 4.0, Faction::Player));
        for _ in 0..ships {
            st.spawn_ship(Faction::Player, s);
        }
        Planet::new(st, pos, name)
    }

    fn neutral(seed: u64, pos: Vec2, name: &str) -> Planet {
        let mut st = Structure::new(seed);
        st.add_sub(SubStructure::new(Vec2::new(0.0, 0.0), 4.0, Faction::Neutral));
        Planet::new(st, pos, name)
    }

    fn enemy(seed: u64, ships: usize, pos: Vec2, name: &str) -> Planet {
        let mut st = Structure::new(seed);
        let s = st.add_sub(SubStructure::new(Vec2::new(0.0, 0.0), 4.0, Faction::Enemy));
        for _ in 0..ships {
            st.spawn_ship(Faction::Enemy, s);
        }
        Planet::new(st, pos, name)
    }

    fn project(w: &World) -> (SimParams, WorldParams, Projection) {
        let sp = sim();
        let wp = WorldParams::default();
        let proj = w.project_forward(&sp, &wp, world::DEFAULT_PROJECTION_HORIZON);
        (sp, wp, proj)
    }

    /// Colonize sends a fully-owned home's surplus toward a reachable neutral (the basic recipe).
    #[test]
    fn colonize_sends_to_a_reachable_neutral() {
        let mut w = World::new();
        let p = w.add_planet(home(1, 14, Vec2::new(0.0, 0.0), "P"));
        let n = w.add_planet(neutral(2, Vec2::new(30.0, 0.0), "N"));
        w.add_lane(p, n, 30.0);
        let (sp, wp, proj) = project(&w);
        let orders = colonize(&w, Faction::Player, &sp, &wp, &proj);
        assert!(
            orders.iter().any(|o| o.from == p && o.to == n),
            "colonize routes the home's surplus toward the neutral, got {orders:?}"
        );
    }

    /// Colonize is **deterministic**: the same world yields identical orders across runs.
    #[test]
    fn colonize_is_deterministic() {
        let mut w = World::new();
        let p = w.add_planet(home(1, 20, Vec2::new(0.0, 0.0), "P"));
        let _n1 = w.add_planet(neutral(2, Vec2::new(30.0, 0.0), "N1"));
        let _n2 = w.add_planet(neutral(3, Vec2::new(30.0, 40.0), "N2"));
        w.add_lane(p, 1, 30.0);
        w.add_lane(p, 2, 50.0);
        let (sp, wp, proj) = project(&w);
        let a = colonize(&w, Faction::Player, &sp, &wp, &proj);
        let b = colonize(&w, Faction::Player, &sp, &wp, &proj);
        assert_eq!(a, b, "the engine must be deterministic");
        assert!(a.iter().all(|o| o.from == p), "only the lone home exports");
    }

    /// WIN-GATING: a neutral the projection already settles to me draws NO fresh wave. We arrange a
    /// neutral that my big in-flight fleet will flip, and assert colonize does not pile a second home
    /// onto it (it would only send to a *different* target, or hold).
    #[test]
    fn win_gating_skips_a_capture_already_in_hand() {
        let mut w = World::new();
        // Home A already shipping a large fleet at the neutral (in flight), home B nearby.
        let a = w.add_planet(home(1, 6, Vec2::new(0.0, 0.0), "A"));
        let n = w.add_planet(neutral(2, Vec2::new(20.0, 0.0), "N"));
        let b = w.add_planet(home(3, 30, Vec2::new(0.0, 40.0), "B"));
        w.add_lane(a, n, 20.0);
        w.add_lane(b, n, 40.0);
        let wp = WorldParams::default();
        let sp = sim();
        // Launch a heavy wave A->N so the projection settles N to the Player.
        w.issue_fleet_order(FleetOrder::new(a, n, FractionBucket::All), Faction::Player, &wp);
        let proj = w.project_forward(&sp, &wp, world::DEFAULT_PROJECTION_HORIZON);
        // Only proceed if the projection indeed settles N to the Player (the gate's precondition).
        if matches!(proj.planet_capture(n), Some((Faction::Player, _))) {
            let orders = colonize(&w, Faction::Player, &sp, &wp, &proj);
            assert!(
                !orders.iter().any(|o| o.to == n),
                "a capture already in hand must draw no fresh wave, got {orders:?}"
            );
        }
    }

    /// Attack concentrates onto enemy ground and routes the first hop toward it (when winnable). A
    /// heavily-stocked home vs a thinly-defended adjacent enemy: the engine commits.
    #[test]
    fn attack_commits_toward_winnable_enemy() {
        let mut w = World::new();
        let p = w.add_planet(home(1, 200, Vec2::new(0.0, 0.0), "P"));
        let e = w.add_planet(enemy(2, 2, Vec2::new(30.0, 0.0), "E"));
        w.add_lane(p, e, 30.0);
        let (sp, wp, proj) = project(&w);
        let orders = attack(&w, Faction::Player, &sp, &wp, &proj);
        assert!(
            orders.iter().any(|o| o.from == p && o.to == e),
            "a strong attacker commits toward the winnable enemy, got {orders:?}"
        );
    }

    /// Quiet-board Defend below the soft cap HOLDS its reserve (it does not colonize). A 14-ship home
    /// is below the single-sub soft cap, so no order is issued.
    #[test]
    fn defend_holds_reserve_below_cap() {
        let mut w = World::new();
        let p = w.add_planet(home(1, 14, Vec2::new(0.0, 0.0), "P"));
        let _n = w.add_planet(neutral(2, Vec2::new(30.0, 0.0), "N"));
        w.add_lane(p, 1, 30.0);
        let (sp, wp, proj) = project(&w);
        assert!(
            w.parked_count(p, Faction::Player) <= w.soft_cap(p, Faction::Player, &sp),
            "precondition: the home is at/below its soft cap"
        );
        let orders = defend(&w, Faction::Player, &sp, &wp, &proj);
        assert!(orders.is_empty(), "below the cap the turtle holds, got {orders:?}");
    }

    /// Quiet-board Defend OVER the soft cap spends the genuine surplus colonizing the nearest neutral.
    #[test]
    fn defend_spends_overcap_surplus() {
        let mut w = World::new();
        let p = w.add_planet(home(1, 60, Vec2::new(0.0, 0.0), "P"));
        let n = w.add_planet(neutral(2, Vec2::new(30.0, 0.0), "N"));
        w.add_lane(p, n, 30.0);
        let (sp, wp, proj) = project(&w);
        assert!(
            w.parked_count(0, Faction::Player) > w.soft_cap(0, Faction::Player, &sp),
            "precondition: the home is over its soft cap"
        );
        let orders = defend(&w, Faction::Player, &sp, &wp, &proj);
        assert!(
            orders.iter().any(|o| o.from == p && o.to == n),
            "over the cap, the turtle colonizes with the genuine surplus, got {orders:?}"
        );
    }

    /// An all-Hold fallback: with sources but no positive-contribution target (an unreachable enemy),
    /// the engine issues nothing rather than feeding a fight it cannot reach.
    #[test]
    fn all_hold_when_no_positive_target() {
        let mut w = World::new();
        let _p = w.add_planet(home(1, 30, Vec2::new(0.0, 0.0), "P"));
        // An enemy with NO lane to the home: unreachable, so attack has no reachable target.
        let _e = w.add_planet(enemy(2, 4, Vec2::new(999.0, 0.0), "E"));
        let (sp, wp, proj) = project(&w);
        let orders = attack(&w, Faction::Player, &sp, &wp, &proj);
        assert!(orders.is_empty(), "no reachable enemy ⇒ hold, got {orders:?}");
    }
}
