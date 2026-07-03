//! # `counter::synthesize` — modular best-response, projection-validated (COUNTER_DESIGN §5–6)
//!
//! The third Counter phase: given an [`OpponentProfile`] (the §4 read), the live `&World`, the
//! acting `seat`, and a `p_max` **playstyle** dial, emit this decision tick's [`FleetOrder`]s by
//! blending two layers, **every exploit validated in the projection before use**:
//!
//! * **(a) the ROBUST BACKBONE (RPS).** From the inferred [`StrategicMix`], pick the countering
//!   automaton from the existing roster — infer-Colonize ⇒ play **Attack**, infer-Attack ⇒
//!   **Defend**, infer-Defend ⇒ **Colonize** ([`OpponentProfile::rps_counter`]) — and run it via
//!   the existing [`crate::strategy`] / [`crate::automata`] policies. This is the never-worse-than-a-
//!   pure-strategy fallback, used wherever confidence is low.
//! * **(b) TARGETED EXPLOITS.** For each *high-confidence* weak module the profile fires, synthesize
//!   a candidate response over [`crate::vocab`] (`never_guards_rear` ⇒ flank the undefended rear;
//!   `over_commits_attacks` ⇒ defend-then-counterpunch the emptied rear; `hoards_past_cap` ⇒
//!   out-tempo while the cap bleeds them). Each candidate is **scored in the mean-field projection
//!   against the inferred (passive) model**, and kept **only if the projection confirms it beats the
//!   backbone** — we ship simulated-real counters, not pattern matches.
//! * **(c) BLEND & escalate safely.** `p_max` sets the *character* (backbone ↔ exploits); the
//!   per-bucket DBR confidence `P_conf = p_max · n_I/(s + n_I)` gates *which* exploits are trusted;
//!   and an exploit is escalated only as far as the opponent's projection-scored **gifts** afford
//!   (the value it leaves on the table). A clean opponent draws the safe backbone; a sloppy one is
//!   punished.
//!
//! ## Statefulness — accumulate-then-counter ([`CounterController`])
//!
//! The synthesis function itself is a **pure** function of `(profile, world, seat, p_max)`. The
//! across-match accumulation lives in [`CounterController`]: it owns an [`Observer`] watching the
//! **opposing** seat, folds that seat's chosen orders each decision tick
//! ([`CounterController::observe_opponent`]), re-infers the [`OpponentProfile`] and re-derives the
//! counter on the decision cadence ([`CounterController::decide`]). This is *accumulate-then-
//! counter*: the profile sharpens as the match runs, but there is **no** within-match policy flip
//! and **no** continuous re-learning beyond re-inferring from the growing log (both deferred per
//! COUNTER_DESIGN §2/§10).
//!
//! Determinism: the projection draws no RNG and the profile is a deterministic function of the log,
//! so a given `(seed, opponent policy, horizon, p_max)` re-derives the same counter bit-for-bit.

use layer1::{Faction, SimParams};
use world::{FleetOrder, StructOwner, World, WorldParams, DEFAULT_PROJECTION_HORIZON};

use crate::adapters::Layer2View;
use crate::controller::AiDecision;
use crate::counter::observe::Observer;
use crate::counter::profile::{ModuleStat, OpponentProfile};
use crate::greedy::{GreedyAction, GreedyKind, PosOwner, PositionView};
use crate::strategy::{StrategicPolicy, TacticalPolicy};
use crate::vocab::{foe_present, owned_by_me, surplus_of};

// =====================================================================================
// Blend dials — the §6 DBR knobs (playstyle character, NOT difficulty).
// =====================================================================================

/// The DBR s-curve **shape** `s` in `P_conf(I) = p_max · n_I/(s + n_I)` (COUNTER_DESIGN §6). The
/// per-bucket confidence reaches half its `p_max` ceiling at `n_I == s` observations, so `s` is
/// "how many samples of a situation before we half-trust the model there". A **policy** dial (how
/// eagerly the Counter trusts a slice), tuned to the single-match sample budget — far below poker's
/// ~100k, so we trust a bucket only after a handful-to-dozen sightings. Not a mechanic.
pub const DBR_CURVE_S: f32 = 12.0;

/// The minimum **trust** `P_conf` an exploit's gating bucket must clear before the exploit is even
/// *considered* (below it we are too unsure to deviate from the backbone). A policy dial; with the
/// default `p_max` and `s` it corresponds to roughly a module's `MIN_MODULE_CONFIDENCE` of sightings.
pub const EXPLOIT_TRUST_FLOOR: f32 = 0.25;

/// The minimum projection **gift** (scored value the opponent leaves on the table — see
/// [`score_projection`]) an exploit must beat the backbone by before it is shipped. A small positive
/// margin so a wash (exploit ≈ backbone in the forecast) keeps the safe backbone — the
/// safe-exploitation principle: deviate only for a *confirmed* edge. A policy dial.
pub const GIFT_MARGIN: f32 = 0.5;

// =====================================================================================
// The synthesis — pure (profile, world, seat, p_max) -> fleet orders.
// =====================================================================================

/// The outcome of one synthesis: the chosen [`FleetOrder`]s plus a legible record of *why*
/// (COUNTER_DESIGN §5's "announce it"). The controller emits the orders; tests/diagnostics read the
/// record to see whether the backbone or an exploit drove the tick.
#[derive(Debug, Clone, PartialEq)]
pub struct CounterPlan {
    /// The fleet orders to issue this tick (the blended backbone-or-exploit result).
    pub fleet_orders: Vec<FleetOrder>,
    /// The robust RPS backbone policy this counter is built on (the never-worse fallback). `None`
    /// for an empty/agnostic profile (no read yet) — then [`CounterPlan::fleet_orders`] is empty.
    pub backbone: Option<StrategicPolicy>,
    /// Which exploit (if any) the projection confirmed and the blend shipped this tick — `None`
    /// means the safe backbone drove the orders. The legible "Read → counter" line.
    pub exploit: Option<Exploit>,
    /// The projection **gift** the shipped exploit beat the backbone by (`0.0` when the backbone
    /// drove the tick). The safe-exploitation budget actually spent.
    pub gift: f32,
}

/// One synthesizable exploit — the legible name of the weak module it punishes (COUNTER_DESIGN §5).
/// Each maps a *fired, high-confidence* [`crate::counter::profile::Modules`] tendency to a concrete
/// candidate response over [`crate::vocab`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Exploit {
    /// **`never_guards_rear`** ⇒ flank the undefended rear: divert the surplus that would feed the
    /// backbone's fight onto the opponent's thinly-held rear/home producers instead, where it
    /// snowballs (the colonizer's thin-rear seam).
    FlankRear,
    /// **`over_commits_attacks`** ⇒ defend-then-counterpunch: let the over-committed wave break on
    /// our wall, then press its emptied rear. (Synthesized as a forward press on the foe's weakest
    /// producer; the backbone Defend already supplies the holding wall.)
    CounterPunch,
    /// **`hoards_past_cap`** ⇒ out-tempo: keep our own surplus moving onto neutral/forward ground
    /// while the opponent's over-cap stack bleeds to the soft cap (we convert tempo they waste).
    OutTempo,
}

impl Exploit {
    /// A short human-readable name (the legible read).
    pub fn name(self) -> &'static str {
        match self {
            Exploit::FlankRear => "flank-undefended-rear",
            Exploit::CounterPunch => "counterpunch-emptied-rear",
            Exploit::OutTempo => "out-tempo-the-hoard",
        }
    }
}

/// **Synthesize this tick's counter** (the §5 pipeline), pure in `(profile, world, seat, p_max)`.
///
/// 1. Pick the RPS **backbone** from the inferred mix and run it (the fallback fleet orders).
/// 2. For each *trusted* (DBR-gated), *fired* weak module, build its candidate fleet orders over
///    [`crate::vocab`] and **score it in the projection against the inferred (passive) model**.
/// 3. Ship the single best exploit **iff** the projection says it beats the backbone by at least
///    [`GIFT_MARGIN`] (escalate only within the opponent's gifts); else ship the safe backbone.
///
/// `p_max` is clamped to `[0, 1]`; `0` is the pure robust generalist (backbone only), `1` leans
/// fully into any projection-confirmed exploit. It is a **playstyle** axis, not a difficulty knob
/// (COUNTER_DESIGN §2/§6).
pub fn synthesize(
    profile: &OpponentProfile,
    world: &World,
    seat: Faction,
    sp: &SimParams,
    wp: &WorldParams,
    p_max: f32,
) -> CounterPlan {
    let p_max = p_max.clamp(0.0, 1.0);

    // ---- (a) The robust RPS backbone. Empty profile (no read yet) -> agnostic, no orders. ----
    let Some(backbone) = profile.rps_counter() else {
        return CounterPlan { fleet_orders: Vec::new(), backbone: None, exploit: None, gift: 0.0 };
    };

    // The single shared forward projection for scoring this tick (one project_forward call).
    let proj = world.project_forward(sp, wp, DEFAULT_PROJECTION_HORIZON);
    let view = Layer2View::with_projection(world, seat, &proj, sp, wp);

    // Backbone fleet orders (the never-worse fallback) + their projected gift baseline.
    let backbone_orders = run_backbone(backbone, world, seat, sp, wp, &proj);
    let backbone_score = score_candidate(world, seat, sp, wp, &backbone_orders);

    // ---- (b)+(c) Candidate exploits, DBR-gated, projection-validated. ----
    // p_max == 0 is the pure robust generalist: never deviate from the backbone.
    // `best` carries (exploit, orders, raw_gift, adjusted) — we RANK by `adjusted` (the trust-scaled
    // gift, the safe-exploitation budget) but REPORT the raw `gift` (the actual value left on the
    // table the projection scored).
    let mut best: Option<(Exploit, Vec<FleetOrder>, f32, f32)> = None;
    if p_max > 0.0 {
        for cand in candidate_exploits(profile, &view, seat, p_max) {
            // Fold the candidate's abstract actions into concrete fleet orders (first-hop routed,
            // fraction-bucketed) via the SAME Layer-2 adapter the automatons use.
            let orders = view.to_fleet_orders(&cand.actions, wp);
            if orders.is_empty() {
                continue;
            }
            let score = score_candidate(world, seat, sp, wp, &orders);
            let gift = score - backbone_score;
            // Keep only an exploit the projection confirms beats the backbone by the safe margin
            // (escalate only within the opponent's gifts).
            if gift < GIFT_MARGIN {
                continue;
            }
            // The trusted exploitation budget: scale the confirmed gift by the per-bucket DBR trust
            // (`p_max · n_I/(s+n_I)`), then pick the largest such adjusted gift across candidates.
            let adjusted = gift * cand.trust;
            match best {
                Some((_, _, _, ba)) if ba >= adjusted => {}
                _ => best = Some((cand.exploit, orders, gift, adjusted)),
            }
        }
    }

    match best {
        Some((exploit, orders, gift, _adjusted)) => {
            CounterPlan { fleet_orders: orders, backbone: Some(backbone), exploit: Some(exploit), gift }
        }
        None => CounterPlan { fleet_orders: backbone_orders, backbone: Some(backbone), exploit: None, gift: 0.0 },
    }
}

// =====================================================================================
// (a) Backbone — reuse the existing strategy/automata policies.
// =====================================================================================

/// Run the chosen RPS backbone [`StrategicPolicy`] over the **shared** projection (the same R3
/// "one projection per tick" path the controller uses), returning its fleet orders. This is exactly
/// the validated pure strategy — never-worse-than-itself by construction.
fn run_backbone(
    backbone: StrategicPolicy,
    world: &World,
    seat: Faction,
    sp: &SimParams,
    wp: &WorldParams,
    proj: &world::Projection,
) -> Vec<FleetOrder> {
    backbone.decide_with(world, seat, sp, wp, world.tick, proj)
}

// =====================================================================================
// (b) Exploits — synthesized candidates over ai::vocab.
// =====================================================================================

/// One candidate exploit ready to score: its [`Exploit`] tag, the abstract [`GreedyAction`]s it
/// would issue (folded to concrete [`FleetOrder`]s at the call site, where the [`Layer2View`] +
/// `wp` live), and the DBR `trust` (`P_conf`) of the bucket that gates it.
struct Candidate {
    exploit: Exploit,
    actions: Vec<GreedyAction>,
    trust: f32,
}

/// Build the candidate exploit set for the profile's *fired, trusted* weak modules. Each candidate
/// is a short [`crate::vocab`] program (built below); the projection decides which (if any) ships.
fn candidate_exploits<V: PositionView>(
    profile: &OpponentProfile,
    view: &V,
    seat: Faction,
    p_max: f32,
) -> Vec<Candidate> {
    let mut out = Vec::new();

    // never_guards_rear (the thin-rear seam) -> flank the undefended rear. Gated on the rear-strip
    // confidence: trust scales with how many rear decisions we have seen (the seam's n_I).
    let seam = profile.modules.never_guards_rear();
    if seam.fires {
        if let Some(trust) = trusted(seam, p_max) {
            let actions = flank_rear(view, seat);
            if !actions.is_empty() {
                out.push(Candidate { exploit: Exploit::FlankRear, actions, trust });
            }
        }
    }

    // over_commits_attacks -> defend-then-counterpunch the emptied rear (a forward press on the
    // weakest foe producer; the backbone Defend supplies the wall).
    let oc = profile.modules.over_commits_attacks;
    if oc.fires {
        if let Some(trust) = trusted(oc, p_max) {
            let actions = counterpunch(view, seat);
            if !actions.is_empty() {
                out.push(Candidate { exploit: Exploit::CounterPunch, actions, trust });
            }
        }
    }

    // hoards_past_cap -> out-tempo: keep our surplus moving onto expandable ground while their
    // over-cap stack bleeds. (A colonize-flavoured spend from every exportable source.)
    let hp = profile.modules.hoards_past_cap;
    if hp.fires {
        if let Some(trust) = trusted(hp, p_max) {
            let actions = out_tempo(view, seat);
            if !actions.is_empty() {
                out.push(Candidate { exploit: Exploit::OutTempo, actions, trust });
            }
        }
    }

    out
}

/// The DBR per-bucket **trust** `P_conf = p_max · n_I/(s + n_I)` for a module's confidence `n_I`,
/// or `None` if it does not clear [`EXPLOIT_TRUST_FLOOR`] (too sparse to deviate). The `n_I` of a
/// module IS the count of the slice it was estimated from (COUNTER_DESIGN §4/§6), so this is the
/// §6 confidence the spec specifies, applied per gating bucket.
fn trusted(m: ModuleStat, p_max: f32) -> Option<f32> {
    let n = m.n_i as f32;
    let p_conf = p_max * n / (DBR_CURVE_S + n);
    (p_conf >= EXPLOIT_TRUST_FLOOR).then_some(p_conf)
}

/// **flank-undefended-rear.** The seam exploit: send each owned source's surplus toward the
/// opponent's **rear** producer — a foe-bearing position the foe leaves thin (fewest enemy ships,
/// nearest on ties) — rather than the nearest contact. Against a colonizer that never guards its
/// rear, a captured rear producer keeps producing and the flank snowballs (the documented exploit).
fn flank_rear<V: PositionView>(view: &V, _seat: Faction) -> Vec<GreedyAction> {
    let floor = crate::automata::GARRISON_FLOOR;
    let mut acts = Vec::new();
    for from in 0..view.len() {
        if !owned_by_me(view, from) || !view.can_export_from(from) {
            continue;
        }
        let Some(surplus) = surplus_of(view, from, floor) else { continue };
        if let Some(target) = thinnest_reachable_foe(view, from) {
            acts.push(GreedyAction { from, to: target, count: surplus, kind: GreedyKind::Wave });
        }
    }
    acts
}

/// **counterpunch-emptied-rear.** Against an over-committer, the backbone is already Defend (the
/// wall); this candidate adds the *press*: every owned source ships surplus onto the weakest
/// reachable foe-bearing position — the rear the attacker stripped to feed its wave.
fn counterpunch<V: PositionView>(view: &V, seat: Faction) -> Vec<GreedyAction> {
    // Same shape as the flank (hit the thinnest foe), but it composes with a Defend backbone rather
    // than an Attack one — kept distinct so the legible read names the right opponent weakness.
    flank_rear(view, seat)
}

/// **out-tempo-the-hoard.** Against a hoarder whose stack bleeds to the soft cap, keep *our* surplus
/// in motion: each owned source ships surplus toward the nearest expandable ground (a foe-free
/// capturable neutral, else any foe-free non-self position) so our tempo converts while theirs is
/// wasted.
fn out_tempo<V: PositionView>(view: &V, _seat: Faction) -> Vec<GreedyAction> {
    let floor = crate::automata::GARRISON_FLOOR;
    let mut acts = Vec::new();
    for from in 0..view.len() {
        if !owned_by_me(view, from) || !view.can_export_from(from) {
            continue;
        }
        let Some(surplus) = surplus_of(view, from, floor) else { continue };
        // Nearest foe-free capturable ground (a neutral to grab, or a thinner friendly to thicken).
        let target = nearest_expandable(view, from);
        if let Some(target) = target {
            acts.push(GreedyAction { from, to: target, count: surplus, kind: GreedyKind::Wave });
        }
    }
    acts
}

// =====================================================================================
// (c) Projection scoring — the mean-field eval against the inferred (passive) model.
// =====================================================================================

/// Score a candidate order set by **applying it to a world clone and reading the mean-field
/// projection** (COUNTER_DESIGN §5: validate in `project_forward` against the inferred model).
///
/// The inferred model is *passive at the mean* — the projection's own enemy-passive assumption is
/// exactly "respond to the mean strategy" (Theorem 2.1): we credit a candidate in expectation by
/// the territory it is projected to hold at the horizon, not by who won a skirmish. Higher is better
/// for `seat`. Pure: clones the world, issues only `seat`'s orders, never mutates the live world,
/// and the projection draws no RNG — so this is deterministic.
fn score_candidate(
    world: &World,
    seat: Faction,
    sp: &SimParams,
    wp: &WorldParams,
    orders: &[FleetOrder],
) -> f32 {
    let mut w = world.clone();
    for o in orders {
        w.issue_fleet_order(*o, seat, wp);
    }
    let proj = w.project_forward(sp, wp, DEFAULT_PROJECTION_HORIZON);
    score_projection(&w, &proj, seat)
}

/// The scalar mean-field value of a projected world for `seat`: projected **net producers** at the
/// horizon (mine − foe), counting each sub by its projected `owner_at_horizon` (+1 mine, −1 foe).
/// Because `owner_at_horizon` already folds the candidate's launched fleets through the grind / heal
/// / combat over the window, a candidate that flips the foe's undefended rear scores those producers
/// as *mine* — so this is the "gifts" yardstick: value the opponent leaves on the table shows up as
/// a higher net-producer forecast. A pure read of the projection. A small tempo tie-breaker (below)
/// rewards keeping force in motion vs hoarding it.
fn score_projection(world: &World, proj: &world::Projection, seat: Faction) -> f32 {
    let enemy = seat.opponent();
    let mut score = 0.0f32;
    for p in 0..world.structs.len() {
        let st = &world.structs[p].interior;
        for s in 0..st.subs.len() {
            let fate = proj.sub_fate(p, s);
            // Project ownership at the horizon (folds the grind/heal/combat the candidate set off).
            if fate.owner_at_horizon == seat {
                score += 1.0;
            } else if fate.owner_at_horizon == enemy {
                score -= 1.0;
            }
        }
    }
    // A light tempo term: our in-flight + present ships vs the foe's, so a candidate that keeps more
    // force productively in motion (rather than hoarding it home) reads marginally better — the
    // out-tempo lever. Scaled small so territory dominates (territory is what compounds).
    let my_ships = world.total_ships(seat) as f32;
    let foe_ships = world.total_ships(enemy) as f32;
    score + 0.001 * (my_ships - foe_ships)
}

// =====================================================================================
// Small shared vocab-flavoured helpers (pure reads of the view).
// =====================================================================================

/// The **thinnest reachable foe-bearing** position from `from` (fewest enemy ships present;
/// nearest on ties): the rear an exploiter flanks. Mirrors the Defender's `weakest_foe` notion but
/// does not skip settling targets (the flank *wants* the rear the projection will hand us). `None`
/// if no reachable foe position exists.
fn thinnest_reachable_foe<V: PositionView>(view: &V, from: usize) -> Option<usize> {
    let mut best: Option<(usize, u32, f32)> = None; // (id, enemy_ships, dist)
    for to in 0..view.len() {
        if to == from || !foe_present(view, to) || !view.reachable(from, to) {
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

/// The nearest reachable **expandable** position from `from`: a foe-free capturable neutral, else a
/// foe-free friendly strictly thinner than the source (consolidate toward weakness). `None` if none.
fn nearest_expandable<V: PositionView>(view: &V, from: usize) -> Option<usize> {
    let me_ships = view.info(from).my_ships;
    let mut best: Option<(usize, f32)> = None;
    for to in 0..view.len() {
        if to == from || foe_present(view, to) || !view.reachable(from, to) {
            continue;
        }
        let info = view.info(to);
        let expandable = info.owner == PosOwner::Neutral
            || (info.owner == PosOwner::Me && info.my_ships < me_ships);
        if !expandable {
            continue;
        }
        let d = view.distance(from, to).unwrap_or(f32::INFINITY);
        match best {
            Some((_, bd)) if bd <= d => {}
            _ => best = Some((to, d)),
        }
    }
    best.map(|(id, _)| id)
}

// =====================================================================================
// The stateful CounterController — accumulate-then-counter (the wiring).
// =====================================================================================

/// The **stateful Counter driver** wired by `Roster::Counter { p_max }`. It owns the accumulating
/// [`Observer`] (watching the *opposing* seat) and re-derives the counter on the decision cadence.
///
/// Usage mirrors an [`crate::controller::AiController`], with one extra hook per decision tick:
/// before applying, call [`CounterController::observe_opponent`] with the pre-decision world and the
/// **opponent's** chosen orders, then [`CounterController::decide`] for this seat's [`AiDecision`].
/// (A host that does not feed the hook still gets the safe RPS backbone from whatever has been
/// observed so far — agnostic ⇒ no orders early, sharpening as the log grows.)
#[derive(Debug, Clone)]
pub struct CounterController {
    /// The seat the Counter plays.
    pub seat: Faction,
    /// The playstyle dial in `[0, 1]` (backbone ↔ exploits). Not difficulty (COUNTER_DESIGN §2).
    pub p_max: f32,
    /// The accumulating observation log of the **opposing** seat (the profile is re-inferred from it
    /// each decision tick — accumulate-then-counter, no continuous re-learning).
    observer: Observer,
    /// The greedy struct-internals policy for our own structs (the backbone's tactical layer).
    tactical: TacticalPolicy,
    /// Greedy tunables for the per-struct internals.
    greedy: crate::greedy::GreedyParams,
}

impl CounterController {
    /// A fresh Counter for `seat` with playstyle `p_max`, reading features under `sp`/`wp` (pass the
    /// same params the match runs under so the observed buckets match what the opponent saw). It
    /// watches `seat.opponent()`.
    pub fn new(seat: Faction, p_max: f32, sp: SimParams, wp: WorldParams) -> CounterController {
        CounterController {
            seat,
            p_max: p_max.clamp(0.0, 1.0),
            observer: Observer::new(seat.opponent(), sp, wp),
            tactical: TacticalPolicy::Greedy,
            greedy: crate::greedy::GreedyParams::default(),
        }
    }

    /// Build the Counter driver from a [`crate::controller::Roster::Counter`] entry for `seat`, or
    /// `None` if `entry` is not a Counter. The one-call path a host uses after detecting a Counter
    /// seat via [`crate::controller::Roster::counter_p_max`].
    pub fn from_roster(
        seat: Faction,
        entry: crate::controller::Roster,
        sp: SimParams,
        wp: WorldParams,
    ) -> Option<CounterController> {
        entry.counter_p_max().map(|p_max| CounterController::new(seat, p_max, sp, wp))
    }

    /// Fold the **opponent's** decision into the profile: call once per decision tick with the
    /// pre-decision `world` snapshot the opponent saw and the [`FleetOrder`]s it chose, **before**
    /// either seat applies. A pure read (the [`Observer`] builds its own projection); deterministic.
    pub fn observe_opponent(&mut self, world: &World, opponent_orders: &[FleetOrder]) {
        self.observer.observe_decision(world, opponent_orders);
    }

    /// Re-infer the opponent [`OpponentProfile`] from everything observed so far. Cheap enough to
    /// call each decision tick (a linear reduction of the log); exposed so a host/diagnostic can
    /// read the current legible "Read".
    pub fn profile(&self) -> OpponentProfile {
        OpponentProfile::infer(&self.observer.log)
    }

    /// The current [`CounterPlan`] (the legible "Read → counter") this tick, without producing the
    /// full [`AiDecision`]. Re-derives from the accumulated profile.
    pub fn plan(&self, world: &World, sp: &SimParams, wp: &WorldParams) -> CounterPlan {
        let profile = self.profile();
        synthesize(&profile, world, self.seat, sp, wp, self.p_max)
    }

    /// Decide this seat's full [`AiDecision`] for the tick: the synthesized counter
    /// [`FleetOrder`]s (backbone blended with any projection-confirmed exploit) plus the greedy
    /// per-struct internals (the same tactical layer every roster entry uses). Deterministic; a
    /// pure read of `(world, params, wp, self)` — it does **not** mutate the accumulated log (that
    /// is [`CounterController::observe_opponent`]'s job).
    pub fn decide(&self, world: &World, sp: &SimParams, wp: &WorldParams) -> AiDecision {
        self.decide_with_plan(world, sp, wp).0
    }

    /// Like [`CounterController::decide`], but also returns the [`CounterPlan`] (the legible
    /// "Read → counter") the decision was built from — so a host/diagnostic can record which backbone
    /// or exploit drove the tick **without re-synthesizing** (one synthesis, not two). The returned
    /// `AiDecision`'s `fleet_orders` are exactly the plan's. Deterministic.
    pub fn decide_with_plan(
        &self,
        world: &World,
        sp: &SimParams,
        wp: &WorldParams,
    ) -> (AiDecision, CounterPlan) {
        let plan = self.plan(world, sp, wp);

        // Per-struct internals: reuse the controller's greedy tactical default over the SAME shared
        // projection so the Counter's own structs auto-defend/expand exactly like every roster entry.
        let mut struct_orders = Vec::new();
        if self.tactical == TacticalPolicy::Greedy {
            let proj = world.project_forward(sp, wp, DEFAULT_PROJECTION_HORIZON);
            for p in 0..world.structs.len() {
                if !self.has_presence(world, p) {
                    continue;
                }
                let v = crate::adapters::Layer1View::with_projection(
                    &world.structs[p].interior,
                    sp,
                    self.seat,
                    &proj,
                    p,
                );
                let actions = crate::greedy::decide_greedy(&v, &self.greedy);
                let orders = v.to_move_orders(&actions);
                if !orders.is_empty() {
                    struct_orders.push((p, orders));
                }
            }
        }

        let decision = AiDecision { fleet_orders: plan.fleet_orders.clone(), struct_orders };
        (decision, plan)
    }

    /// Apply a decided [`AiDecision`] for this seat (internals first, then fleets) — same discipline
    /// as [`crate::controller::AiController::apply`]. Returns `(moved, launched)`.
    pub fn apply(&self, world: &mut World, decision: &AiDecision, wp: &WorldParams) -> (usize, usize) {
        let mut moved = 0usize;
        for (p, orders) in &decision.struct_orders {
            if *p < world.structs.len() {
                for o in orders {
                    moved += world.structs[*p].interior.issue_order(*o, self.seat);
                }
            }
        }
        let mut launched = 0usize;
        for o in &decision.fleet_orders {
            launched += world.issue_fleet_order(*o, self.seat, wp) as usize;
        }
        (moved, launched)
    }

    /// True if `seat` has any sub or ship on struct `p` (mirrors `AiController::has_presence`).
    fn has_presence(&self, world: &World, p: world::StructId) -> bool {
        let agg = world.struct_aggregate(p);
        let subs = match self.seat {
            Faction::Player => agg.player_subs,
            Faction::Ai(_) => agg.enemy_subs, // parked Counter; binary Layer-2 aggregate (all rivals combined)
            Faction::Neutral => 0,
        };
        subs > 0 || agg.ships_of(self.seat) > 0 || matches!(agg.owner, StructOwner::Owned(f) if f == self.seat)
    }
}

#[cfg(test)]
mod tests;
