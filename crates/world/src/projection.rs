//! # The unified forward-projection — an event-driven mean-field look-ahead
//!
//! **PARKED — off the live game path.** Nothing the live game runs builds this projection any more:
//! the campaign **Simple** seat and the live `AiController` rosters (Passive / GreedyLocal) read the
//! projection-free [`World::sub_influx_for`] instead. `project_forward` and the `Projection` QUERY
//! reads survive only for the **deferred automata/Counter track** (`ai::automata`, `ai::hardcoded`,
//! `ai::counter`), which is revived later — so this module is kept compiling but is not exercised by
//! a normal match. (It also still carries the known entry-sub-vs-reserve arrival divergence the live
//! `sub_influx_for` path fixes.)
//!
//! This is the single read-only, deterministic, **RNG-free** capture forecast every automaton
//! shares (design review **R3**, which *supersedes* `docs/archive/AUTOMATA_DESIGN.md` §2). It answers, for
//! each sub-structure of each planet:
//!
//! > If **no new orders** are issued and the **enemy stays passive**, considering the ships
//! > **present now** plus the ships **already in transit toward this sub** (both intra-structure
//! > moves *and* inter-planet [`InterFleet`]s), and folding in the resistance grind, expected
//! > square-law combat where forces are co-present, and the owner's production over the window —
//! > when (if ever) over the next `horizon` ticks does this sub's owner change, and to whom?
//!
//! ## Why event-driven (R3 vs the doc's tick loop)
//!
//! `AUTOMATA_DESIGN.md` §2C sketched a per-(sub × tick) loop. R3 replaces it with an
//! **event-driven, recursive** integrator over the **segments between successive arrival ticks**.
//! Within one such segment the *arriving* force is constant, so the dynamics are closed-form:
//!
//! * **Uncontested** (one faction present, no production boundary): the grind/heal is linear, so we
//!   **jump** in O(1) — `ticks_to_flip = ceil(remaining_resistance / present_force)` for an
//!   attacker, or a capped linear heal for the owner — straight to the next event.
//! * **Contested** (both present): combat is active and the grind is frozen (exactly as the sim's
//!   [`layer1::SubStructure::capture_step`]). We advance this stretch **tick-by-tick** under the
//!   **mean-field square law** (the deterministic expectation of the sim's stochastic combat)
//!   until one side is cleared — a naturally short stretch — then resume jumping.
//!
//! The result is `O(arrivals + contested_ticks + spawns)` per sub instead of `O(horizon)` per sub,
//! while matching a straight tick-by-tick reference integrator within rounding (see the unit
//! tests). Both the fast integrator and the reference share **one** canonical per-tick kernel
//! ([`step_one_tick`], applied in the exact sim order *production → arrivals → combat → capture*)
//! so the fast path can only differ where its closed-form jump is provably equal to repeating the
//! kernel.
//!
//! ## Shared grind, no drift
//!
//! The capture rule is **not** re-implemented here: every owner/flip/heal decision goes through
//! the same pure [`layer1::SubStructure::capture_step`] the sim itself calls, so the projection
//! can never drift from the simulation when the rule is tuned (R3 / §5 signal 5).
//!
//! ## What it is blind to (callers must respect — §2D)
//!
//! It models exactly two event classes — in-transit **arrivals** and the **resistance rule** they
//! drive — plus the *expectation* of combat and production **within** the present force. It still
//! ignores **new orders** and any **enemy reaction** (the enemy is passive by construction). Its
//! ETAs are therefore bounds, not promises; `became_contested` / `flips_again` flag the loose
//! cases, and the contract is to **re-project every decision tick**.
//!
//! ## The composable query vocabulary (R3)
//!
//! On top of the per-sub [`SubFate`] roll-ups ([`Projection::sub_fate`] / [`Projection::sub_capture`]
//! / [`Projection::planet_capture`] …), the projection exposes a small, orthogonal set of
//! **semantic queries** that hand-written *and* future evolved agents build policies from — the
//! projection is the **sole** place game mechanics live for the AI:
//!
//! * [`Projection::expected_combat`] — the square-law combat *expectation* (the combat model).
//! * [`Projection::capture_eta`] — when does a sub flip on the current plan?
//! * [`Projection::capture_eta_if`] — …and if `extra` ships of a side arrive in N ticks? (marginal)
//! * [`Projection::marginal_ticks_saved`] — the value, in ticks, of one more ship from a position.
//! * [`Projection::force_for_efficiency`] — smallest force that wins at a target casualty exchange.
//!
//! plus per-element property reads ([`Projection::current_owner`], [`Projection::sub_resistance`],
//! [`Projection::present_now`]). Every per-sub quantity is read through an **accessor**, never a
//! hard-coded constant — including the per-structure soft cap, expressed as
//! `softcap_free + Σ_subs sub.soft_cap_capacity(..)` so a future sub *type* changes the cap by
//! returning a different capacity, not by editing projection/AI code.
//!
//! ## Determinism
//!
//! Pure read of `(&World, &SimParams, &WorldParams)`. It mutates nothing, touches no planet's
//! seeded `Rng`, and draws no randomness — so calling it never perturbs [`World::state_hash`].
//! The marginal what-if queries re-integrate a single sub from a captured scalar seed (no `&World`,
//! no mutation), so they are equally pure.

use layer1::{Faction, SimParams, SubId, SubStructure, Vec2};

use crate::{InterFleet, PlanetId, World, WorldParams};

/// Default look-ahead, in ticks (R3). Set to **2000** in tuning (AUTOMATA_DESIGN §0): the marginal-
/// capture queries must span a full `~max_resistance / force` grind (default fresh resistance is
/// `1800`), else they read `0` and the colonizers never commit. A couple of production periods plus
/// a transit and a whole grind — long enough for the capture calculus the automatons run each
/// decision tick, while still re-projected every tick so the enemy-blind forecast is never trusted
/// past one decision window. (Was `~240` under the retired instant-capture model.)
pub const DEFAULT_PROJECTION_HORIZON: u64 = 2000;

/// A small numeric floor so divisions by a (near-)zero rate never explode.
const EPS: f32 = 1e-6;

/// The projected fate of one sub-structure over the horizon.
///
/// `eta_*` ticks are **absolute world ticks** (`>= base_tick`). All optional fields are `None`
/// when the corresponding event does not occur within the horizon.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SubFate {
    /// The sub's owner at `base_tick` (call time).
    pub current_owner: Faction,
    /// Absolute tick of the **first** owner change, or `None` if the owner never changes within
    /// the horizon.
    pub eta_first_change: Option<u64>,
    /// Who the sub belongs to right after `eta_first_change` (`None` iff no change).
    pub owner_after_first_change: Option<Faction>,
    /// Owner at `base_tick + horizon` (equals `current_owner` if it never changed).
    pub owner_at_horizon: Faction,
    /// Resistance at the horizon (in `[0, max_resistance]`).
    pub resistance_at_horizon: f32,
    /// True if at some tick within the horizon **two** factions were co-present (the grind froze
    /// and combat ran). When set, `eta_first_change` is a **lower** bound on the real flip time
    /// (the sim's stochastic combat decides who is "the only side present"). Callers downgrade
    /// confidence accordingly.
    pub became_contested: bool,
    /// True if a **second** owner change happens after the first within the horizon (a
    /// flip→heal→flip-back style sequence). Downgrades confidence in the single summarized ETA.
    pub flips_again: bool,
}

/// One scheduled presence delta: at absolute tick `tick`, `count` more ships of `faction` become
/// present inside a sub. Built from intra-structure moving ships and inter-planet fleet arrivals.
#[derive(Debug, Clone, Copy)]
struct Arrival {
    tick: u64,
    faction: Faction,
    count: u32,
}

/// The forward-projection result: one [`SubFate`] per `(planet, sub)`, plus the scheduled-arrival
/// bookkeeping the derived queries need. Build it with [`World::project_forward`].
///
/// Lookups are O(1) via a per-planet base-index offset into a flat `fates` vector.
#[derive(Debug, Clone)]
pub struct Projection {
    /// The horizon this projection was computed for (ticks).
    pub horizon: u64,
    /// The world tick at call time. All `eta_*` values are absolute and `>= base_tick`.
    pub base_tick: u64,
    /// Flat per-sub fates; `fates[base_index[p] + s]` is `(planet p, sub s)`.
    fates: Vec<SubFate>,
    /// `base_index[p]` is the offset of planet `p`'s first sub in `fates`; `base_index[p+1]` is
    /// one past its last (so a planet's sub count is the difference). Length `planets.len() + 1`.
    base_index: Vec<usize>,
    /// All scheduled arrivals, flat, grouped by sub. `arr_index[g] .. arr_index[g+1]` is the
    /// arrival slice for global sub `g` (same flat index space as `fates`). Sorted by tick.
    arrivals: Vec<Arrival>,
    arr_index: Vec<usize>,
    /// Per-sub initial living presence (idle ships) of each real seat, flat in the same index
    /// space as `fates`. Used by `eta_to_present_for` to answer "present now" without a re-read.
    initial_present: Vec<Presence>,
    /// Per-sub integrator seed (owner / resistance / max / spawn timer at `base_tick`), flat in
    /// the same index space as `fates`. Lets the **marginal** queries (`capture_eta_if`,
    /// `marginal_ticks_saved`) re-integrate a single sub *with a hypothetical extra arrival*
    /// without re-reading the `World` — so the `Projection` is a self-contained query object.
    seeds: Vec<SubSeed>,
    /// The sim parameters this projection was built under (combat rates, production period,
    /// soft-cap dials). Stored — it is `Copy` and tiny — so the combat/force queries
    /// (`expected_combat`, `force_for_efficiency`) use the *same* numbers as the integration.
    sp: SimParams,
}

impl Projection {
    /// Flat global index of `(planet, sub)`, or `None` if either is out of range.
    #[inline]
    fn flat(&self, planet: PlanetId, sub: SubId) -> Option<usize> {
        if planet + 1 >= self.base_index.len() {
            return None;
        }
        let start = self.base_index[planet];
        let end = self.base_index[planet + 1];
        let g = start + sub;
        if g < end {
            Some(g)
        } else {
            None
        }
    }

    /// O(1) fate of one sub. An out-of-range `(planet, sub)` yields a borrowed trivial
    /// "unchanged neutral" fate.
    pub fn sub_fate(&self, planet: PlanetId, sub: SubId) -> &SubFate {
        // A 'static fallback so the signature can return a borrow even for OOB ids.
        static UNCHANGED: SubFate = SubFate {
            current_owner: Faction::Neutral,
            eta_first_change: None,
            owner_after_first_change: None,
            owner_at_horizon: Faction::Neutral,
            resistance_at_horizon: 0.0,
            became_contested: false,
            flips_again: false,
        };
        match self.flat(planet, sub) {
            Some(g) => &self.fates[g],
            None => &UNCHANGED,
        }
    }

    /// Who captures/frees this sub **first** and the absolute tick it happens, if within the
    /// horizon. (Just the first owner change of the sub's fate, surfaced as a tuple.)
    pub fn sub_capture(&self, planet: PlanetId, sub: SubId) -> Option<(Faction, u64)> {
        let f = self.sub_fate(planet, sub);
        match (f.owner_after_first_change, f.eta_first_change) {
            (Some(owner), Some(eta)) => Some((owner, eta)),
            _ => None,
        }
    }

    /// Planet-level roll-up: a planet "flips to" a faction when, at the horizon, that faction
    /// owns **every** owned sub on the planet (and the enemy owns none) **and** at least one sub
    /// actually changed hands within the horizon. Returns that faction and the tick the last
    /// such change completes. `None` if the planet does not become cleanly single-owned by a flip
    /// within the horizon (e.g. it stays mixed, or no sub ever changed).
    ///
    /// Neutral subs that never flip block a clean roll-up (the planet is not yet fully one
    /// faction's) — matching the Layer-2 "fully owned" notion the strategies use.
    pub fn planet_capture(&self, planet: PlanetId) -> Option<(Faction, u64)> {
        if planet + 1 >= self.base_index.len() {
            return None;
        }
        let start = self.base_index[planet];
        let end = self.base_index[planet + 1];
        if start == end {
            return None; // no subs
        }
        let mut player_subs = 0u32;
        let mut enemy_subs = 0u32;
        let mut neutral_subs = 0u32;
        let mut any_change = false;
        let mut last_change_tick = 0u64;
        for g in start..end {
            let f = &self.fates[g];
            match f.owner_at_horizon {
                Faction::Player => player_subs += 1,
                Faction::Ai(_) => enemy_subs += 1, // parked binary projection
                Faction::Neutral => neutral_subs += 1,
            }
            if let Some(t) = f.eta_first_change {
                any_change = true;
                last_change_tick = last_change_tick.max(t);
            }
        }
        if !any_change || neutral_subs > 0 {
            return None;
        }
        match (player_subs > 0, enemy_subs > 0) {
            (true, false) => Some((Faction::Player, last_change_tick)),
            (false, true) => Some((Faction::Ai(0), last_change_tick)),
            _ => None, // mixed ownership: not a clean planet flip
        }
    }

    /// Of `faction`'s scheduled in-transit ships toward this sub, how many are projected to have
    /// **arrived** (be present) by the horizon — the sum of `faction`'s scheduled arrivals into
    /// the sub. Lets a caller avoid double-sending to a sub its own in-flight force already
    /// settles.
    pub fn incoming_present_at(&self, planet: PlanetId, sub: SubId, faction: Faction) -> u32 {
        self.arrivals_for(planet, sub)
            .iter()
            .filter(|a| a.faction == faction && a.tick <= self.base_tick + self.horizon)
            .map(|a| a.count)
            .sum()
    }

    /// The returning-defender present-force the projection expects at this sub: the scheduled
    /// in-flight arrivals of the sub's **current owner** within the horizon. Attack uses this to
    /// size a hold that out-erodes the heal.
    pub fn returning_owner_force(&self, planet: PlanetId, sub: SubId) -> u32 {
        let owner = self.sub_fate(planet, sub).current_owner;
        if !owner.is_real() {
            return 0;
        }
        self.incoming_present_at(planet, sub, owner)
    }

    /// Absolute tick by which `faction` is first projected to be **present** at this sub (its
    /// earliest scheduled arrival within the horizon, or now if it is already present). `None` if
    /// `faction` neither is present now nor has any arrival scheduled within the horizon.
    ///
    /// (`eta_to_present_for` in the §3 pseudocode.) Note: "present now" cannot be read off
    /// `SubFate` alone, so this answers from the scheduled arrivals plus the initial-presence the
    /// projection seeded; an already-present faction returns `base_tick`.
    pub fn eta_to_present_for(&self, planet: PlanetId, sub: SubId, faction: Faction) -> Option<u64> {
        let g = self.flat(planet, sub)?;
        if self.initial_present[g].of(faction) > 0 {
            return Some(self.base_tick);
        }
        self.arrivals_for(planet, sub)
            .iter()
            .filter(|a| a.faction == faction && a.tick <= self.base_tick + self.horizon)
            .map(|a| a.tick)
            .min()
    }

    /// First seat-owned sub on `planet` projected to **flip to the enemy** (i.e. a sub currently
    /// owned by `seat` whose first change hands it to `seat.opponent()`), and the earliest such
    /// tick. `None` if no owned sub is projected to fall within the horizon.
    ///
    /// (`planet_first_fall` in the §3 pseudocode — Defend's "reinforce the sub that falls first".)
    pub fn planet_first_fall(&self, planet: PlanetId, seat: Faction) -> Option<(SubId, u64)> {
        if planet + 1 >= self.base_index.len() {
            return None;
        }
        let start = self.base_index[planet];
        let end = self.base_index[planet + 1];
        let enemy = seat.opponent();
        let mut best: Option<(SubId, u64)> = None;
        for g in start..end {
            let f = &self.fates[g];
            if f.current_owner == seat && f.owner_after_first_change == Some(enemy) {
                if let Some(t) = f.eta_first_change {
                    let sub = g - start;
                    match best {
                        Some((_, bt)) if bt <= t => {}
                        _ => best = Some((sub, t)),
                    }
                }
            }
        }
        best
    }

    /// The scheduled-arrival slice for one sub (empty for OOB ids).
    #[inline]
    fn arrivals_for(&self, planet: PlanetId, sub: SubId) -> &[Arrival] {
        match self.flat(planet, sub) {
            Some(g) => &self.arrivals[self.arr_index[g]..self.arr_index[g + 1]],
            None => &[],
        }
    }
}

// =====================================================================================
// R3 — the composable SEMANTIC QUERY vocabulary
// =====================================================================================
//
// These are the small, orthogonal "language" that hand-written *and* future evolved automatons
// build policies from. They are deliberately minimal and compose:
//
//   * `expected_combat`        — the combat model (square-law expectation) as a standalone query.
//   * `capture_eta`            — when (absolute tick) does this sub flip, on the *current* plan?
//   * `capture_eta_if`         — …and if I add `extra` ships of a faction arriving in N ticks?
//   * `marginal_ticks_saved`   — the value, in ticks, of ONE more ship sent from a given position.
//   * `force_for_efficiency`   — the smallest force that wins the firefight at a target exchange.
//
// plus thin per-element property reads (owner / resistance / capacity / present-now). Everything
// above flows from these; an AI never re-derives a mechanic.
impl Projection {
    // ---- Combat model query --------------------------------------------------------------

    /// **The combat-model query.** Deterministic square-law *expectation* of a fight between
    /// `attackers` and `defenders` resolved to the extinction of one side: the mean of the sim's
    /// stochastic per-ship fire (same per-tick kernel the integrator's contested regime uses).
    /// Returns `(attacker_survivors, defender_survivors)` — one of which is `0` once a side is
    /// wiped (or both non-zero only in the degenerate zero-fire-rate case). `defender_in_own_sub`
    /// grants the defender the additive [`SimParams::defender_fire_bonus`] (the on-sub edge), so
    /// this is the single place the defender advantage enters AI reasoning.
    ///
    /// Pure and frame-independent: it does **not** read the projection's state, only the stored
    /// [`SimParams`]; safe to call for any hypothetical pair of counts.
    pub fn expected_combat(&self, attackers: u32, defenders: u32, defender_in_own_sub: bool) -> (u32, u32) {
        expected_combat_impl(&self.sp, attackers as f32, defenders as f32, defender_in_own_sub)
    }

    /// **The combat-TIMELINE query** (L_defend's kill-efficiency primitive). Resolve a fight that
    /// is *not* static: `my` and `foe` start co-present (with `my_in_own_sub` granting my side the
    /// on-sub defender edge), then a time-ordered list of [`CombatEvent`]s reshapes the present
    /// force at known ticks (a reinforcement arriving, a wave of the foe landing, or my rear-guard
    /// **retreating**). Over each constant-force interval between successive events the same
    /// mean-field square law as [`Projection::expected_combat`] attrits both sides (reusing the
    /// shared per-tick kernel — the combat math lives here in `world`, never in `ai`); a
    /// [`CombatEvent::MyRetreat`] additionally books a **rear-guard loss** to my side proportional
    /// to the foe still present (a withdrawal under fire is not free). After the last event the
    /// remaining co-present forces fight to extinction.
    ///
    /// Returns `(my_losses, foe_losses)` — the casualties each side took — from which a caller
    /// reads kill-efficiency as `foe_losses / max(1, my_losses)`. Pure and frame-independent: it
    /// reads only the stored [`SimParams`], so it is safe for any hypothetical force/event list and
    /// draws no RNG (deterministic, like every projection query).
    ///
    /// `events` MUST be sorted by tick ascending (the caller builds them in order); ticks are
    /// **relative** offsets from the start of the engagement (`0` = "already present"). An event at
    /// tick `0` is applied before any combat runs.
    pub fn expected_combat_timeline(
        &self,
        my: u32,
        foe: u32,
        my_in_own_sub: bool,
        events: &[(u64, CombatEvent)],
    ) -> (u32, u32) {
        let mut a = my as f32; // "my" side
        let mut d = foe as f32; // foe
        let mut my_losses = 0.0f32;
        let mut foe_losses = 0.0f32;
        let mut now = 0u64;

        for &(tick, ev) in events {
            // Advance combat over [now, tick) at the current constant force, booking losses.
            if tick > now && a > 0.0 && d > 0.0 {
                let span = tick - now;
                let (na, nd) = combat_interval(&self.sp, a, d, my_in_own_sub, span);
                my_losses += a - na;
                foe_losses += d - nd;
                a = na;
                d = nd;
            }
            now = now.max(tick);
            // Apply the event at `tick`.
            match ev {
                CombatEvent::MyArrival(c) => a += c as f32,
                CombatEvent::FoeArrival(c) => d += c as f32,
                CombatEvent::MyRetreat(c) => {
                    let pulled = (c as f32).min(a);
                    a -= pulled;
                    // Rear-guard loss: a withdrawal under fire is not free — the retreating ships
                    // take one mean-field tick of the foe's fire on the way out, proportional to
                    // the foe still present (capped by the ships that actually pulled out). Booked
                    // as my-side attrition the candidate pays; the ships then leave the board.
                    let rear = (d * foe_fire_per_tick(&self.sp)).min(pulled);
                    my_losses += rear;
                }
            }
        }

        // After the last event, fight the remainder to extinction. Use the same `combat_interval`
        // kernel (my side = on-sub defender) with a large tick bound so the rate assignment matches
        // every interval above; the bound only guards the degenerate near-zero-rate case.
        if a > 0.0 && d > 0.0 {
            let (na, nd) = combat_interval(&self.sp, a, d, my_in_own_sub, 100_000);
            my_losses += a - na;
            foe_losses += d - nd;
        }

        (my_losses.max(0.0).floor() as u32, foe_losses.max(0.0).floor() as u32)
    }

    // ---- Capture-timing queries ----------------------------------------------------------

    /// **When does this sub flip?** The absolute tick of the first projected owner change on the
    /// *current* plan (present + already-in-transit ships, enemy passive), or `None` if it does
    /// not change within the horizon. Equivalent to `sub_capture(..).map(|(_, t)| t)`, surfaced as
    /// the primary timing primitive the automatons name.
    pub fn capture_eta(&self, planet: PlanetId, sub: SubId) -> Option<u64> {
        self.sub_fate(planet, sub).eta_first_change
    }

    /// **Marginal-reasoning query.** The flip tick (absolute) this sub *would* have if, on top of
    /// the current plan, `extra` more ships of `reinforce` arrived `arriving_in_ticks` ticks from
    /// now. `None` if it still would not flip within the horizon. Re-integrates **only this sub**
    /// from its captured seed with one extra synthetic arrival merged into its schedule — no
    /// `&World` needed and the live projection is untouched.
    ///
    /// (R3 `capture_eta_if`. The faction is explicit because the extra ships must belong to a
    /// side — pass your own seat for "if I reinforce", the opponent for "if they do".)
    pub fn capture_eta_if(
        &self,
        planet: PlanetId,
        sub: SubId,
        extra: u32,
        arriving_in_ticks: u64,
        reinforce: Faction,
    ) -> Option<u64> {
        self.refate_with_extra(planet, sub, extra, arriving_in_ticks, reinforce)
            .and_then(|f| f.eta_first_change)
    }

    /// **The value of one more ship**, in ticks saved on the capture of `target`, if that ship is
    /// sent from `from_position` (a sub on the *same planet* — distance sets its arrival delay via
    /// [`SimParams::ship_speed`]). Defined exactly as R3:
    /// `marginal = capture_eta_if(0) − capture_eta_if(1 more, arriving from `from_position`)`,
    /// measured for the faction that *owns* `from_position` (the side that could send it).
    ///
    /// Always `>= 0` (one more friendly ship never *delays* a capture): a positive value means the
    /// extra ship pulls the flip sooner by that many ticks; `0` means it does not help (already
    /// uncapturable within the horizon, the sender is neutral, or the marginal ship is absorbed by
    /// the contested freeze). This is the steeply-diminishing `dT ≈ r/w²` quantity Colonize uses to
    /// find its wave sweet spot.
    pub fn marginal_ticks_saved(&self, target_planet: PlanetId, target_sub: SubId, from_position: SubId) -> u64 {
        // The sender's faction = whoever owns `from_position` on the target's planet. (Same-planet
        // marginal reasoning; a Layer-1 view's positions are subs of one structure.)
        let Some(g_from) = self.flat(target_planet, from_position) else { return 0 };
        let sender = self.seeds[g_from].owner;
        if !sender.is_real() {
            return 0; // a neutral position sends nothing
        }
        let delay = self.intra_arrival_delay(target_planet, from_position, target_sub);

        let base_eta = self.capture_eta_if(target_planet, target_sub, 0, delay, sender);
        let plus_eta = self.capture_eta_if(target_planet, target_sub, 1, delay, sender);
        match (base_eta, plus_eta) {
            // Both flip: ticks saved by the extra ship (clamped at 0 — never negative).
            (Some(b), Some(p)) => b.saturating_sub(p),
            // The extra ship turns a non-flip into a flip: value = the whole remaining horizon to
            // the new flip (capped at the horizon), a large-but-finite "newly possible" signal.
            (None, Some(p)) => (self.base_tick + self.horizon).saturating_sub(p),
            // Extra ship does not enable a flip within the horizon: no value.
            (_, None) => 0,
        }
    }

    // ---- Force-sizing query --------------------------------------------------------------

    /// **The smallest attacking force** that beats this sub's *current* defenders at a casualty
    /// exchange of at least `desired_ratio` (attacker losses : defender losses), derived purely
    /// from [`Projection::expected_combat`] + the on-sub defender edge. Concretely: the least
    /// `w` such that fighting `w` attackers against the sub's present defenders leaves the
    /// attacker alive and `attacker_losses <= defender_losses / desired_ratio` (i.e. the attacker
    /// trades at least `desired_ratio`-to-1). Returns `0` if the sub has no defenders (nothing to
    /// fight) and `None` if even an overwhelming force cannot reach the ratio (a degenerate config).
    ///
    /// This is the "win the firefight *efficiently*" primitive Attack sizes a spearhead with; it
    /// is monotone — a higher `desired_ratio` never asks for *fewer* ships.
    pub fn force_for_efficiency(&self, planet: PlanetId, sub: SubId, desired_ratio: f32) -> Option<u32> {
        let defenders = self.defenders_now(planet, sub);
        if defenders == 0 {
            return Some(0);
        }
        let in_own_sub = true; // the defender sits on its own sub (the realistic, conservative case)
        let ratio = desired_ratio.max(0.0);
        // Search upward for the least winning, efficient force. Defenders are bounded by the
        // living fleet, so an O(defenders·k) scan with a generous cap is plenty; combat is convex
        // in the attacker count so the first satisfying `w` is the minimum.
        let cap = defenders.saturating_mul(8).max(defenders + 16);
        for w in 1..=cap {
            let (atk_surv, def_surv) = self.expected_combat(w, defenders, in_own_sub);
            if atk_surv == 0 {
                continue; // attacker wiped — too thin
            }
            let atk_losses = (w - atk_surv) as f32;
            let def_losses = (defenders - def_surv) as f32;
            // Efficient when the attacker trades at least `ratio`-to-1: atk_losses*ratio <= def_losses.
            // (ratio == 0 degenerates to "just win the fight", satisfied as soon as atk survives.)
            if atk_losses * ratio <= def_losses + EPS {
                return Some(w);
            }
        }
        None
    }

    // ---- Per-element property reads (clean accessors behind the queries) -----------------

    /// This sub's owner at `base_tick` (call time). OOB ids report `Neutral`.
    pub fn current_owner(&self, planet: PlanetId, sub: SubId) -> Faction {
        self.sub_fate(planet, sub).current_owner
    }

    /// This sub's `(current, max)` capture resistance at `base_tick`. OOB ids report `(0, 0)`.
    pub fn sub_resistance(&self, planet: PlanetId, sub: SubId) -> (f32, f32) {
        match self.flat(planet, sub) {
            Some(g) => (self.seeds[g].resist, self.seeds[g].maxr),
            None => (0.0, 0.0),
        }
    }

    /// Living present `(player, enemy)` ships seeded for this sub at `base_tick` (idle ships, the
    /// integrator's initial presence). OOB ids report `(0, 0)`.
    pub fn present_now(&self, planet: PlanetId, sub: SubId) -> (u32, u32) {
        match self.flat(planet, sub) {
            Some(g) => (self.initial_present[g].player, self.initial_present[g].enemy),
            None => (0, 0),
        }
    }

    /// **Query — expected future production of `seat`**, the policy OBJECTIVE, in *owned-sub-ticks*:
    /// `Σ over all subs of E[ticks `seat` owns the sub within [base, base+horizon]]`. A sub produces
    /// only while owned, at one fixed rate, so owned-sub-ticks is proportional to ships produced —
    /// and the constant rate cancels when comparing candidate plans, so it is omitted here.
    ///
    /// This is the principled "value-to-go": a producer captured **early** and held **long** scores
    /// high; a **contested** sub (short expected tenure) is discounted automatically; one the foe
    /// takes scores zero. Computed from each sub's projected fate via a two-segment approximation of
    /// its ownership trajectory (`[base, eta)` under the current owner, `[eta, end)` under the owner
    /// after the first change) — exact for the common ≤1-flip case, a close estimate otherwise.
    pub fn expected_production(&self, seat: Faction) -> f64 {
        let base = self.base_tick;
        let end = self.base_tick.saturating_add(self.horizon);
        let mut owned_subticks = 0.0f64;
        for f in &self.fates {
            owned_subticks += match f.eta_first_change {
                // Never changes within the horizon: owned the whole window iff currently mine.
                None => {
                    if f.current_owner == seat {
                        (end - base) as f64
                    } else {
                        0.0
                    }
                }
                // One (modelled) change at `eta`: sum the two ownership segments that are mine.
                Some(eta) => {
                    let eta = eta.clamp(base, end);
                    let seg1 = if f.current_owner == seat { eta - base } else { 0 };
                    let seg2 = if f.owner_after_first_change == Some(seat) { end - eta } else { 0 };
                    (seg1 + seg2) as f64
                }
            };
        }
        owned_subticks
    }

    // ---- Private helpers for the marginal queries ---------------------------------------

    /// Living present ships of the sub's **current foreign side** — the defenders an attacker must
    /// clear. If the sub is owned by a real seat, that owner's present ships; if neutral, `0`.
    fn defenders_now(&self, planet: PlanetId, sub: SubId) -> u32 {
        let (pp, pe) = self.present_now(planet, sub);
        match self.current_owner(planet, sub) {
            Faction::Player => pp,
            Faction::Ai(_) => pe,
            Faction::Neutral => 0,
        }
    }

    /// Intra-structure arrival delay (ticks) for a ship leaving `from` and aiming at `to` on the
    /// same planet, from the captured sub centres + radii with the same straight-line
    /// `ship_speed` rule the real scheduler uses: a ship aims *inside* the target radius and is
    /// "arrived" within [`SimParams::arrival_tolerance`], so the travelled gap is
    /// `dist(centres) − target_radius − arrival_tolerance`. Floored at 1 tick (a marginal ship
    /// cannot land the same tick it departs).
    fn intra_arrival_delay(&self, planet: PlanetId, from: SubId, to: SubId) -> u64 {
        let (Some(gf), Some(gt)) = (self.flat(planet, from), self.flat(planet, to)) else {
            return 1;
        };
        let sf = &self.seeds[gf];
        let st = &self.seeds[gt];
        let gap = (sf.pos.dist(st.pos) - st.radius - self.sp.arrival_tolerance).max(0.0);
        let speed = self.sp.ship_speed.max(EPS);
        ((gap / speed).ceil() as u64).max(1)
    }

    /// Re-integrate a single sub from its captured seed with one extra synthetic arrival of
    /// `extra` ships of `reinforce` at `base + arriving_in_ticks`, returning its new fate. `None`
    /// for OOB ids. The live projection is untouched.
    fn refate_with_extra(
        &self,
        planet: PlanetId,
        sub: SubId,
        extra: u32,
        arriving_in_ticks: u64,
        reinforce: Faction,
    ) -> Option<SubFate> {
        let g = self.flat(planet, sub)?;
        let base_slice = &self.arrivals[self.arr_index[g]..self.arr_index[g + 1]];

        let inject = extra > 0 && reinforce.is_real();
        // `arriving_in_ticks == 0` means "already present" — the integrator only consumes arrivals
        // at ticks `>= base + 1` (an arrival lands *on* its arrival tick, never the base tick), so
        // a delay-0 reinforcement must seed the INITIAL presence instead of scheduling at `base`
        // (which would be silently skipped). Any positive delay is a normal scheduled arrival.
        let mut initial = self.initial_present[g];
        let merged: Vec<Arrival> = if !inject || arriving_in_ticks == 0 {
            if inject {
                match reinforce {
                    Faction::Player => initial.player += extra,
                    Faction::Ai(_) => initial.enemy += extra,
                    Faction::Neutral => {}
                }
            }
            base_slice.to_vec()
        } else {
            let mut v = Vec::with_capacity(base_slice.len() + 1);
            v.extend_from_slice(base_slice);
            v.push(Arrival { tick: self.base_tick.saturating_add(arriving_in_ticks), faction: reinforce, count: extra });
            v.sort_by_key(|a| a.tick);
            v
        };

        Some(integrate_sub(
            self.seeds[g],
            initial,
            &merged,
            self.base_tick,
            self.horizon,
            &self.sp,
            false,
        ))
    }
}

/// A single timed reshaping of the present force in a [`Projection::expected_combat_timeline`]
/// walk. The events are how a *moving* fight differs from the static [`Projection::expected_combat`]:
/// reinforcements land, the foe's wave lands, or my rear guard withdraws (paying a rear-guard loss).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CombatEvent {
    /// `count` ships of my side arrive (a reinforcement landing).
    MyArrival(u32),
    /// `count` ships of the foe arrive (the enemy's staggered wave landing).
    FoeArrival(u32),
    /// `count` ships of my side **retreat** off the board; the withdrawal books a rear-guard loss
    /// proportional to the foe still present (a pull-back under fire is not free).
    MyRetreat(u32),
}

/// The mean-field per-tick fire rates `(attacker_rate, defender_rate)`: each side's expected kills
/// per tick is `own_count * rate`. The defender carries the additive on-sub edge when
/// `defender_in_own_sub`. The **single** place the square-law rate (and the defender advantage) is
/// expressed for both the extinction solver and the timeline walk, so they can never diverge.
#[inline]
fn combat_rates(sp: &SimParams, defender_in_own_sub: bool) -> (f32, f32) {
    let substeps = sp.combat_substeps.max(1) as f32;
    let atk_rate = sp.fire_prob as f32 * substeps;
    let def_rate =
        (sp.fire_prob as f32 + if defender_in_own_sub { sp.defender_fire_bonus as f32 } else { 0.0 }) * substeps;
    (atk_rate, def_rate)
}

/// The foe's expected kills per ship per tick (the defender-side rate when *I* hold the sub — i.e.
/// the foe is the attacker with no on-sub edge). Used to size a [`CombatEvent::MyRetreat`]'s
/// rear-guard loss. Always the plain attacker rate (the foe never holds my sub in this framing).
#[inline]
fn foe_fire_per_tick(sp: &SimParams) -> f32 {
    combat_rates(sp, false).0
}

/// Standalone square-law combat expectation, shared by [`Projection::expected_combat`] and the
/// integrator's [`combat_tick`]: run mean-field fire to the extinction of one side (or until a
/// fixed safety bound for the degenerate zero-rate case), returning integer survivors (floored).
fn expected_combat_impl(sp: &SimParams, attackers: f32, defenders: f32, defender_in_own_sub: bool) -> (u32, u32) {
    let (atk_rate, def_rate) = combat_rates(sp, defender_in_own_sub);
    let mut a = attackers.max(0.0);
    let mut d = defenders.max(0.0);
    // Both rates zero => no one ever dies; return the inputs (degenerate, but well-defined).
    if atk_rate <= 0.0 && def_rate <= 0.0 {
        return (a.floor() as u32, d.floor() as u32);
    }
    // March one mean-field tick at a time until a side is (essentially) gone. The loop is bounded:
    // each tick removes a strictly positive amount while both sides live, and we cap iterations as
    // a hard safety net against pathological tiny rates.
    let mut guard = 0u32;
    while a > 0.5 && d > 0.5 && guard < 100_000 {
        let a_kills = a * atk_rate; // defenders the attacker removes
        let d_kills = d * def_rate; // attackers the defender removes
        a = (a - d_kills).max(0.0);
        d = (d - a_kills).max(0.0);
        guard += 1;
    }
    (a.floor() as u32, d.floor() as u32)
}

/// Mean-field square-law combat for a **bounded** number of ticks (vs the extinction solver
/// [`expected_combat_impl`]): advance `ticks` mean-field rounds (or until a side is essentially
/// gone, whichever comes first) and return the surviving `(my, foe)` as fractional counts (so the
/// timeline walk can keep integrating across intervals without premature rounding). My side ("a")
/// holds the sub when `my_in_own_sub`, so it carries the defender edge.
#[inline]
fn combat_interval(sp: &SimParams, my: f32, foe: f32, my_in_own_sub: bool, ticks: u64) -> (f32, f32) {
    let (foe_rate, my_rate) = combat_rates(sp, my_in_own_sub); // my side is the on-sub defender
    let mut a = my.max(0.0);
    let mut d = foe.max(0.0);
    if (my_rate <= 0.0 && foe_rate <= 0.0) || ticks == 0 {
        return (a, d);
    }
    let mut k = 0u64;
    while a > 0.5 && d > 0.5 && k < ticks {
        let a_kills = a * my_rate; // foe ships my side removes
        let d_kills = d * foe_rate; // my ships the foe removes
        a = (a - d_kills).max(0.0);
        d = (d - a_kills).max(0.0);
        k += 1;
    }
    (a, d)
}

/// Initial per-sub living present counts of each real seat (idle ships seed this; moving ships
/// arrive as scheduled events so they are not double-counted). Small POD for `eta_to_present_for`.
#[derive(Debug, Clone, Copy, Default)]
struct Presence {
    player: u32,
    enemy: u32,
}

impl Presence {
    #[inline]
    fn of(&self, f: Faction) -> u32 {
        match f {
            Faction::Player => self.player,
            Faction::Ai(_) => self.enemy,
            Faction::Neutral => 0,
        }
    }
}

/// The scalar integrator seed for one sub at `base_tick`: everything [`integrate_sub`] needs
/// besides the arrival schedule and the initial presence. Captured into the [`Projection`] so the
/// marginal "what-if" queries can replay a single sub's grind without a `&World`.
#[derive(Debug, Clone, Copy)]
struct SubSeed {
    owner: Faction,
    resist: f32,
    maxr: f32,
    /// `SubStructure::production_timer` at call time (the sim's spawn countdown).
    production_timer: u32,
    /// Sub centre in the structure's local plane and its radius — kept so the marginal queries can
    /// compute an intra-structure arrival delay (`marginal_ticks_saved`'s `from_position` distance)
    /// with the same straight-line `ship_speed` rule the real scheduler uses.
    pos: Vec2,
    radius: f32,
}

/// Fleet arrival timing (R3 / §5): ticks until an in-transit `fleet`'s ships are **injected**
/// into its destination. This reproduces [`World::step`] exactly: it burns the remaining undock
/// delay, then crosses the lane at `transit_speed / lane_len` progress per tick, and `World::step`
/// injects the ships at the **end** of the arriving tick (so they are first present the *next*
/// tick — the `+1` the scheduler adds). A degenerate (non-positive) lane length arrives the first
/// transiting tick. Pure, deterministic, RNG-free.
pub fn fleet_arrival_ticks(world: &World, wp: &WorldParams, fleet: &InterFleet) -> u64 {
    let undock = fleet.undock_remaining as u64;
    // Lane length with the same clamp `World::step` uses (`f_lane_len`): missing/degenerate => 1.
    let len = world
        .lane_length(fleet.from, fleet.to)
        .map(|l| if l > 0.0 { l } else { 1.0 })
        .unwrap_or(1.0);
    let dprog = if len > 0.0 { wp.transit_speed / len } else { 1.0 };
    let remaining = (1.0 - fleet.progress).max(0.0);
    let cross = if dprog > 0.0 {
        (remaining / dprog).ceil() as u64
    } else {
        // No progress possible (zero transit speed): never arrives within any finite horizon.
        u64::MAX
    };
    // While undocking, progress does not advance; the two phases are sequential.
    undock.saturating_add(cross)
}

impl World {
    /// Build the event-driven forward [`Projection`] over `horizon` ticks (R3). Pure read; see the
    /// module docs. `horizon == 0` yields all-unchanged fates.
    pub fn project_forward(&self, sp: &SimParams, wp: &WorldParams, horizon: u64) -> Projection {
        self.project_with(sp, wp, horizon, false)
    }

    /// Build the projection with the **plain tick-by-tick** integrator (`reference = true`) instead
    /// of the fast event-driven one. Test-only oracle: the two must agree (within rounding) on the
    /// same world, which is the central correctness check for the closed-form jumps.
    #[cfg(test)]
    pub(crate) fn project_reference(
        &self,
        sp: &SimParams,
        wp: &WorldParams,
        horizon: u64,
    ) -> Projection {
        self.project_with(sp, wp, horizon, true)
    }

    /// Shared body of [`World::project_forward`]: `reference` selects the tick-by-tick oracle vs
    /// the fast event-driven integrator (both call the same per-tick kernel).
    fn project_with(&self, sp: &SimParams, wp: &WorldParams, horizon: u64, reference: bool) -> Projection {
        let n_planets = self.planets.len();

        // ---- base_index: flatten (planet, sub) -> global index. ----
        let mut base_index = Vec::with_capacity(n_planets + 1);
        let mut total_subs = 0usize;
        for p in &self.planets {
            base_index.push(total_subs);
            total_subs += p.structure.subs.len();
        }
        base_index.push(total_subs);

        // ---- Seed initial presence (IDLE ships only — moving ships arrive as events). ----
        let mut initial_present = vec![Presence::default(); total_subs];
        for (p, planet) in self.planets.iter().enumerate() {
            let g0 = base_index[p];
            for s in 0..planet.structure.subs.len() {
                initial_present[g0 + s] = Presence {
                    player: planet.structure.idle_presence_in_sub(s, Faction::Player) as u32,
                    enemy: planet.structure.idle_presence_in_sub(s, Faction::Ai(0)) as u32,
                };
            }
        }

        // ---- Schedule arrivals (presence deltas) per global sub. ----
        // Bucket lists first, then flatten into the CSR-style (arrivals, arr_index).
        let mut buckets: Vec<Vec<Arrival>> = vec![Vec::new(); total_subs];
        let base = self.tick;
        let deadline = base.saturating_add(horizon);

        // (a) Intra-structure moving ships: each schedules +1 into its target sub on its eta.
        for (p, planet) in self.planets.iter().enumerate() {
            let g0 = base_index[p];
            let st = &planet.structure;
            for sh in &st.ships {
                if !sh.alive {
                    continue;
                }
                let Some(tgt) = sh.target else { continue };
                if tgt >= st.subs.len() {
                    continue;
                }
                // Ticks to arrive: the closed form of `advance_movement` (straight line to `aim`
                // at `ship_speed`, arrived within `arrival_tolerance`). The sim snaps to arrival
                // on the step that lands within tolerance, so the ceiling of the remaining gap
                // over the speed is the arrival-tick offset (>= 1 while still en route).
                let dist = sh.pos.dist(sh.aim);
                let eff = (dist - sp.arrival_tolerance).max(0.0);
                let speed = sp.ship_speed.max(EPS);
                let eta = base + (eff / speed).ceil() as u64;
                if eta <= deadline {
                    buckets[g0 + tgt].push(Arrival { tick: eta, faction: sh.faction, count: 1 });
                }
            }
        }

        // (b) Inter-planet fleets: schedule +count into the destination's entry sub. The entry
        //     sub uses the *identical* landing rule as `inject_fleet` (the now-public
        //     `World::entry_sub`), and the +1 reflects that `World::step` injects at end-of-tick.
        for f in &self.fleets {
            if !f.faction.is_real() {
                continue;
            }
            let ticks = fleet_arrival_ticks(self, wp, f);
            if ticks == u64::MAX {
                continue; // never arrives
            }
            let arrive = base.saturating_add(ticks).saturating_add(1);
            if arrive > deadline {
                continue;
            }
            let Some(entry) = self.entry_sub(f.to, f.from, f.faction) else { continue };
            let g0 = base_index[f.to];
            buckets[g0 + entry].push(Arrival { tick: arrive, faction: f.faction, count: f.count });
        }

        // Flatten buckets -> CSR, sorting each sub's arrivals by tick (stable, faction order kept).
        let mut arr_index = Vec::with_capacity(total_subs + 1);
        let mut arrivals: Vec<Arrival> = Vec::new();
        for b in buckets.iter_mut() {
            arr_index.push(arrivals.len());
            b.sort_by_key(|a| a.tick);
            arrivals.extend_from_slice(b);
        }
        arr_index.push(arrivals.len());

        // ---- Integrate each sub independently (the fast, event-driven integrator). ----
        // Capture a scalar `SubSeed` per sub alongside the fate so the marginal what-if queries
        // can replay one sub's grind later without a `&World`.
        let mut fates = Vec::with_capacity(total_subs);
        let mut seeds = Vec::with_capacity(total_subs);
        for (p, planet) in self.planets.iter().enumerate() {
            let g0 = base_index[p];
            let st = &planet.structure;
            for s in 0..st.subs.len() {
                let g = g0 + s;
                let sub = &st.subs[s];
                let seed = SubSeed {
                    owner: sub.owner,
                    resist: sub.resistance,
                    maxr: sub.max_resistance,
                    production_timer: sub.production_timer,
                    pos: sub.pos,
                    radius: sub.radius,
                };
                let fate = integrate_sub(
                    seed,
                    initial_present[g],
                    &arrivals[arr_index[g]..arr_index[g + 1]],
                    base,
                    horizon,
                    sp,
                    reference,
                );
                fates.push(fate);
                seeds.push(seed);
            }
        }

        Projection {
            horizon,
            base_tick: base,
            fates,
            base_index,
            arrivals,
            arr_index,
            initial_present,
            seeds,
            sp: *sp,
        }
    }
}

/// The evolving per-sub mean-field state during integration. `pp`/`pe` are *fractional* present
/// counts (combat is modelled in expectation); `ticks_to_spawn` mirrors the sim's countdown.
#[derive(Debug, Clone, Copy)]
struct SubState {
    owner: Faction,
    resist: f32,
    maxr: f32,
    /// Mean-field present Player force.
    pp: f32,
    /// Mean-field present Enemy force.
    pe: f32,
    /// Ticks until this sub's next production spawn (sim's `production_timer` semantics).
    ticks_to_spawn: i64,
    /// The production period this sub resets its spawn countdown to (`SimParams::production_period`,
    /// clamped `>= 1`). Stored so `produce_tick` can reset without re-reading params.
    period: i64,
}

/// The **single canonical per-tick kernel** — the exact sim order, in expectation, for one sub:
///
/// 1. **production** (gated by denial: a sub being solely-eroded does not produce),
/// 2. **arrivals** (movement: scheduled in-transit ships become present this tick),
/// 3. **combat** (mean-field square law where both seats are co-present),
/// 4. **capture** (the shared [`SubStructure::capture_step`] grind/heal/flip).
///
/// Returns `Some(new_owner)` iff the sub flipped this tick (so the caller can record the ETA).
/// `now` is the absolute tick being resolved (used only to consume arrivals scheduled at it).
/// Both the fast integrator's contested regime and the tick-by-tick reference call this, so they
/// cannot diverge.
fn step_one_tick(
    st: &mut SubState,
    arrivals: &[Arrival],
    cursor: &mut usize,
    now: u64,
    sp: &SimParams,
) -> Option<Faction> {
    // (1) Production. Denial gate: owner present AND not being solely-eroded. In this mean-field,
    //     "being eroded" == foe present & owner absent; a contested-but-defended sub still
    //     produces (matches `Structure::produce`). Neutral never produces.
    produce_tick(st);

    // (2) Arrivals (movement). Scheduled ships at exactly `now` become present.
    apply_arrivals(arrivals, cursor, now, &mut st.pp, &mut st.pe);

    // (3) Combat (mean square law), only where both seats are co-present.
    if st.pp > 0.0 && st.pe > 0.0 {
        combat_tick(st, sp);
    }

    // (4) Capture grind/heal/flip via the single shared rule.
    let (no, nr, flipped) = SubStructure::capture_step(
        st.owner,
        st.resist,
        st.maxr,
        st.pp.floor().max(0.0) as u32,
        st.pe.floor().max(0.0) as u32,
    );
    st.owner = no;
    st.resist = nr;
    if flipped {
        // The sim nudges a flipped sub's production timer to >= 1 so it does not pop instantly;
        // resetting the cadence to the full period matches "just seized, no immediate spawn".
        st.ticks_to_spawn = sp.production_period.max(1) as i64;
        Some(no)
    } else {
        None
    }
}

/// One production tick: decrement the spawn countdown and, on reaching 0, add one owner ship
/// **present here** and reset the countdown — but only when the owner is a real seat present at the
/// sub and the sub is **not being solely eroded** (denial gate). Mirrors [`Structure::produce`].
#[inline]
fn produce_tick(st: &mut SubState) {
    let owner = st.owner;
    if !owner.is_real() {
        return;
    }
    let (owner_force, foe_force) = match owner {
        Faction::Player => (st.pp, st.pe),
        Faction::Ai(_) => (st.pe, st.pp),
        Faction::Neutral => (0.0, 0.0),
    };
    // Denial: a foe present with the owner absent freezes production (timer held steady).
    let being_eroded = foe_force > 0.0 && owner_force <= 0.0;
    if being_eroded {
        return;
    }
    st.ticks_to_spawn -= 1;
    if st.ticks_to_spawn <= 0 {
        match owner {
            Faction::Player => st.pp += 1.0,
            Faction::Ai(_) => st.pe += 1.0,
            Faction::Neutral => {}
        }
        st.ticks_to_spawn = st.period;
    }
}

/// One tick of mean-field square-law combat (the deterministic expectation of the sim's
/// stochastic per-ship fire). Each engaged ship fires with probability `fire_prob` per substep
/// over `combat_substeps` substeps, one-shotting a random enemy on a hit, so each side's expected
/// kills/tick ≈ `own_count * fire_prob * substeps`. The **owner** side gets the additive
/// `defender_fire_bonus` (its ships sit inside their own sub's radius). Forces clamp at 0.
///
/// Fire is taken **simultaneously** (both sides use pre-combat counts), matching the sim's
/// "collect all shots against pre-substep liveness, then apply" within a substep, aggregated to a
/// tick in expectation.
#[inline]
fn combat_tick(st: &mut SubState, sp: &SimParams) {
    let substeps = sp.combat_substeps.max(1) as f32;
    let p_rate =
        sp.fire_prob as f32 + if st.owner == Faction::Player { sp.defender_fire_bonus as f32 } else { 0.0 };
    let e_rate =
        sp.fire_prob as f32 + if st.owner == Faction::Ai(0) { sp.defender_fire_bonus as f32 } else { 0.0 };
    let p_kills = st.pp * p_rate * substeps; // enemies the player removes
    let e_kills = st.pe * e_rate * substeps; // players the enemy removes
    let new_pp = (st.pp - e_kills).max(0.0);
    let new_pe = (st.pe - p_kills).max(0.0);
    st.pp = new_pp;
    st.pe = new_pe;
}

/// Apply every arrival whose tick equals `now`, advancing the cursor past all arrivals at or
/// before `now` (older ones are defensively skipped to keep the cursor monotone).
#[inline]
fn apply_arrivals(arrivals: &[Arrival], cursor: &mut usize, now: u64, pp: &mut f32, pe: &mut f32) {
    while *cursor < arrivals.len() && arrivals[*cursor].tick <= now {
        let a = arrivals[*cursor];
        if a.tick == now {
            match a.faction {
                Faction::Player => *pp += a.count as f32,
                Faction::Ai(_) => *pe += a.count as f32,
                Faction::Neutral => {}
            }
        }
        *cursor += 1;
    }
}

/// Integrate one sub's fate over the ticks `(base, base+horizon]`, event-driven over `arrivals`
/// (sorted by tick, pruned to the horizon). When `reference` is true it runs the **plain
/// tick-by-tick** kernel for every tick (the unit-test oracle); otherwise it takes closed-form
/// **jumps** across uncontested, spawn-free stretches and the gaps between arrivals, stepping
/// tick-by-tick only while contested. Both modes call the same [`step_one_tick`] kernel, so the
/// fast path can only differ where its jump equals repeating the kernel — which the tests check.
fn integrate_sub(
    seed: SubSeed,
    initial: Presence,
    arrivals: &[Arrival],
    base: u64,
    horizon: u64,
    sp: &SimParams,
    reference: bool,
) -> SubFate {
    let period = sp.production_period.max(1) as i64;
    let mut st = SubState {
        owner: seed.owner,
        resist: seed.resist,
        maxr: seed.maxr,
        pp: initial.player as f32,
        pe: initial.enemy as f32,
        ticks_to_spawn: seed.production_timer as i64,
        period,
    };

    let deadline = base + horizon;
    let mut now = base;
    let mut cursor = 0usize;
    let mut became_contested = false;
    let mut flips = FlipLog::default();

    while now < deadline {
        if st.pp > 0.0 && st.pe > 0.0 {
            became_contested = true;
        }

        if reference || (st.pp > 0.0 && st.pe > 0.0) {
            // --- Tick-by-tick (reference mode, or the contested combat regime). ---
            now += 1;
            if let Some(who) = step_one_tick(&mut st, arrivals, &mut cursor, now, sp) {
                flips.note(now, who);
            }
            continue;
        }

        // --- Fast path: uncontested (or empty). Jump to the next boundary. ---
        // The next boundary is the soonest of: an arrival tick, the next spawn tick, the next
        // flip tick (for an attacker), or the deadline. Within that span the per-tick kernel is
        // closed-form, so we advance in one shot and only run the kernel at the boundary tick.
        let next_arrival = peek_next_arrival_tick(arrivals, cursor, now);
        let seg_end = next_arrival.map(|t| t.min(deadline)).unwrap_or(deadline);
        let seg_ticks = seg_end - now; // >= 1 unless seg_end == now (handled below)
        if seg_ticks == 0 {
            // An arrival is due at `now` exactly — resolve that tick with the kernel.
            now += 1;
            if let Some(who) = step_one_tick(&mut st, arrivals, &mut cursor, now, sp) {
                flips.note(now, who);
            }
            continue;
        }

        let pcount = st.pp.floor().max(0.0) as u32;
        let ecount = st.pe.floor().max(0.0) as u32;
        let single: Option<(Faction, u32)> = match (pcount > 0, ecount > 0) {
            (true, false) => Some((Faction::Player, pcount)),
            (false, true) => Some((Faction::Ai(0), ecount)),
            _ => None,
        };

        // In every uncontested branch we close-form across `pre` interior ticks (where the kernel
        // is provably linear) and then resolve exactly ONE boundary tick via the shared kernel —
        // so the spawn-at-0, heal cap, flip+refill, and any arrival landing at the boundary all go
        // through the identical sim rule. `pre` is chosen so no spawn/flip/arrival occurs inside it.
        let pre: u64 = match single {
            None => {
                // Empty sub: nothing changes across the interior; just carry to the boundary tick.
                seg_ticks - 1
            }
            Some((f, _)) if f == st.owner => {
                // Owner present, uncontested: HEAL toward max + PRODUCE on cadence. The interior is
                // the run up to (but not including) the next spawn tick or seg_end, whichever is
                // sooner; heal it closed-form. The boundary tick (spawn / arrival / seg_end) runs
                // through the kernel.
                let to_spawn = st.ticks_to_spawn.max(1) as u64;
                let jump = to_spawn.min(seg_ticks);
                let interior = jump - 1;
                if interior > 0 {
                    let count = if st.owner == Faction::Player { st.pp } else { st.pe };
                    st.resist = (st.resist + count * interior as f32).min(st.maxr);
                    st.ticks_to_spawn -= interior as i64;
                }
                interior
            }
            Some((_f, count)) => {
                // One foreign faction, uncontested: ERODE at `count`/tick (the attacker owns
                // nothing here, so it does not produce). Find the flip tick within the segment.
                let ticks_to_flip = (st.resist / count as f32).ceil().max(1.0) as u64;
                let boundary = ticks_to_flip.min(seg_ticks);
                let interior = boundary - 1;
                if interior > 0 {
                    st.resist -= count as f32 * interior as f32;
                }
                interior
            }
        };
        now += pre;
        // Resolve the single boundary tick through the shared kernel.
        now += 1;
        if let Some(who) = step_one_tick(&mut st, arrivals, &mut cursor, now, sp) {
            flips.note(now, who);
        }
    }

    SubFate {
        current_owner: seed.owner,
        eta_first_change: flips.eta_first,
        owner_after_first_change: flips.owner_after_first,
        owner_at_horizon: st.owner,
        resistance_at_horizon: st.resist.clamp(0.0, st.maxr),
        became_contested,
        flips_again: flips.again,
    }
}

/// Records owner changes during integration: the first sets the ETA + new owner; any later flip
/// flags `again`. (A small struct so `integrate_sub` avoids a closure that would borrow these
/// fields for the whole loop and conflict with the final read.)
#[derive(Default)]
struct FlipLog {
    count: u32,
    eta_first: Option<u64>,
    owner_after_first: Option<Faction>,
    again: bool,
}

impl FlipLog {
    #[inline]
    fn note(&mut self, tick: u64, who: Faction) {
        self.count += 1;
        if self.count == 1 {
            self.eta_first = Some(tick);
            self.owner_after_first = Some(who);
        } else {
            self.again = true;
        }
    }
}

/// Peek the next arrival tick strictly after `now`, from `cursor` onward, or `None`.
#[inline]
fn peek_next_arrival_tick(arrivals: &[Arrival], cursor: usize, now: u64) -> Option<u64> {
    let mut k = cursor;
    while k < arrivals.len() {
        if arrivals[k].tick > now {
            return Some(arrivals[k].tick);
        }
        k += 1;
    }
    None
}

#[cfg(test)]
mod tests;
