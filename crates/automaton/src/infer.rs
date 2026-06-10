//! **Online policy inference** — scout an opponent and estimate its hidden mix from
//! the legible features.
//!
//! This is the epistemic core of the arc-1 capstone (`02-ai-opponents.md`): the player
//! "scouts, **infers the mix from early behavior**, then commits to the counter under
//! uncertainty. This is *online policy inference* — exactly what the Apprentice does to
//! the player." It is also the first stress-test of decision **D3**: the inference may
//! read **only** the legible feature vocabulary, because the design's whole bet is that
//! a *human* can do this same read off the HUD at a glance.
//!
//! ## What we observe, and which simplex axis it scores (`02`: "expansion rate,
//! aggression timing, frontier behavior")
//!
//! Watching the opponent (seat `opp`) over a short scouting window, we accumulate three
//! nonnegative scores from quantities a player literally watches on screen:
//!
//! * **colonize score** — the opponent's **expansion rate**: how fast its territory and
//!   production climb. A colonizer floods neutrals, so its dot-count and the top-bar
//!   production number rise fast while its army stays thin.
//! * **attack score** — its **aggression timing**: hostile ships it sends at *our*
//!   nodes (fleets we watch streaking in) plus army it masses toward our front. A timed
//!   wave is the unmistakable attack tell.
//! * **defend score** — its **frontier behavior**: ships held in garrison on its
//!   frontier rather than spent expanding or attacking — a heavy, manned wall. Because
//!   "hold" is the *absence* of the other two, this is measured as garrisoned frontier
//!   mass net of expansion/aggression.
//!
//! Normalizing the three scores onto the simplex yields the estimated [`Mix`]. The
//! estimate is **deterministic** (a pure function of the observed deterministic states),
//! so the whole capstone remains bit-reproducible.

use cell_core::features::{self, OwnerFeatures};
use cell_core::{GameState, Owner, Params};

use crate::mix::Mix;

/// Tunables for the scout. Defaults are chosen so a human-plausible early window
/// (`02`: "scout/observe the opponent EARLY behaviour") is enough to read the mix.
#[derive(Debug, Clone, Copy)]
pub struct InferConfig {
    /// How many ticks to observe before the estimate is considered final (the scouting
    /// window). The capstone player switches to its counter after this many ticks.
    pub scout_ticks: u64,
    /// Weight on the **colonize** signal: expansion rate + fleet **mobility** (the share
    /// of the opponent's army kept in motion). A colonizer floods — territory climbs and
    /// ships are constantly in transit toward fresh neutrals.
    pub colonize_gain: f64,
    /// Weight on the **attack** signal: hostile ships aimed at *our* nodes + army staged
    /// forward on the contact frontier. The unmistakable aggression tell.
    pub attack_gain: f64,
    /// Weight on the **defend** signal: **stationary mass** — an army held still (low
    /// mobility) rather than spent expanding or thrown at us. A turtle parks its ships.
    pub defend_gain: f64,
}

impl Default for InferConfig {
    fn default() -> Self {
        InferConfig {
            // A short scouting window: long enough that an attacker reaches contact (so
            // its aggression manifests) and the mobility gap is visible, but short enough
            // that (a) the neutral scout posture is not crippled before it commits and
            // (b) the *committed counter* — not the scout — plays the bulk of the match
            // and so carries the win/loss. Keeping the window short is what makes the
            // counter (hence inference quality) the thing that decides the outcome, which
            // is the whole point of the capstone.
            scout_ticks: 50,
            // Calibrated so the three pure corners each read as themselves on the standard
            // maps (see `infers_pure_corners` + the `diag_inference` table). Mobility is
            // the load-bearing colonize↔defend discriminator, so colonize/defend gains are
            // tuned against it; aggression is a clean, large signal so attack needs less.
            colonize_gain: 1.0,
            attack_gain: 1.2,
            defend_gain: 1.15,
        }
    }
}

/// Accumulates observations of `opp` over the scouting window and produces a [`Mix`]
/// estimate. Fed one [`GameState`] snapshot per tick by the capstone player.
///
/// Rather than integrate three incommensurable running scores, the scout accumulates a
/// few clean **summary statistics** (total expansion, peak aggression, average held
/// mass) and combines them **once**, in [`Scout::estimate`], onto the simplex. This
/// keeps the three signals on comparable scales — the bug that otherwise makes a
/// colonizer's transient captured-node garrison swamp its (loud, but small-numbered)
/// expansion-rate signal. The scout keeps the previous snapshot's opponent macro
/// features to measure rates, which is what makes "expansion rate" legible.
#[derive(Debug, Clone)]
pub struct Scout {
    /// The seat being scouted.
    opp: Owner,
    cfg: InferConfig,

    // ---- accumulated summary statistics over the window ----
    /// Total territory the opponent gained over the window (expansion → colonize).
    terr_gain: f64,
    /// Peak total hostile ships aimed at *our* nodes (aggression → attack).
    incoming_peak: f64,
    /// Sum over ticks of the opponent's contact-frontier staging excess (attack massing).
    staging_excess_sum: f64,
    /// Sum over ticks of the opponent's **mobility**: the fraction of its army in transit
    /// (fleets / total units). High = a colonizer keeping ships flowing to fresh
    /// neutrals; low = a turtle parking its army. The load-bearing colonize↔defend tell.
    mobility_sum: f64,
    /// Sum over ticks of the opponent's army "presence" (a saturating function of its
    /// total units), so the defend score reflects *parking a real army* rather than
    /// sitting still with nothing. Keeps an idle near-empty board from reading as defend.
    mass_presence_sum: f64,
    /// Sum over ticks of the opponent's **held rear/interior garrison fraction**: the
    /// share of its *garrisoned* (not in-flight) army sitting on non-frontier (interior)
    /// nodes. This is the man-the-wall / held-reserve signature that uniquely marks a
    /// **defender**: a colonizer keeps almost no garrison anywhere (ships flood out), and
    /// an attacker drains its rear forward to the contact line — only a turtle stocks its
    /// interior. The load-bearing colonize↔defend↔attack discriminator the global
    /// mobility signal alone could not provide (a defend+attack mix has high mobility too,
    /// but still holds its rear). Read off the board exactly like the others — "are its
    /// back planets stacked?".
    rear_garrison_sum: f64,

    // Previous-tick opponent macro features, to measure rates of change.
    prev: Option<OwnerFeatures>,
    ticks_seen: u64,
}

impl Scout {
    /// Start scouting seat `opp` with the given config.
    pub fn new(opp: Owner, cfg: InferConfig) -> Scout {
        Scout {
            opp,
            cfg,
            terr_gain: 0.0,
            incoming_peak: 0.0,
            staging_excess_sum: 0.0,
            mobility_sum: 0.0,
            mass_presence_sum: 0.0,
            rear_garrison_sum: 0.0,
            prev: None,
            ticks_seen: 0,
        }
    }

    /// True once the scouting window has elapsed (the estimate is final and the player
    /// should commit to its counter).
    pub fn done(&self) -> bool {
        self.ticks_seen >= self.cfg.scout_ticks
    }

    /// Number of ticks observed so far.
    pub fn ticks_seen(&self) -> u64 {
        self.ticks_seen
    }

    /// Raw accumulated signals
    /// `(terr_gain_per_tick, avg_mobility, incoming_per_tick, staging_per_tick,
    /// avg_rear_garrison)`, exposed for calibration/diagnostics only. `incoming` (ships
    /// aimed at us) and `staging` (forward frontier mass) are kept separate because they
    /// are scored very differently (see [`Scout::estimate`]); `avg_rear_garrison` is the
    /// held-interior-mass signal that was evaluated as a defend tell but left out of the
    /// score (it hurt on these small maps).
    #[doc(hidden)]
    pub fn debug_signals(&self) -> (f64, f64, f64, f64, f64) {
        let ticks = self.ticks_seen.max(1) as f64;
        (
            self.terr_gain / ticks,
            self.mobility_sum / ticks,
            self.incoming_peak / ticks,
            self.staging_excess_sum / ticks,
            self.rear_garrison_sum / ticks,
        )
    }

    /// Ingest one tick's snapshot. `me` is the scouting seat (so we can read "incoming
    /// hostile at *my* nodes" — the aggression tell — from my perspective).
    pub fn observe(&mut self, state: &GameState, me: Owner, params: &Params) {
        let opp = self.opp;
        // Opponent macro features (its economy/board picture) — from its own seat.
        let of = OwnerFeatures::compute(state, opp, params);

        // ---- expansion (colonize, primary): total territory gained -----------
        if let Some(prev) = self.prev {
            self.terr_gain += (of.territory_count as f64 - prev.territory_count as f64).max(0.0);
        }

        // ---- mobility (colonize ↔ defend discriminator) ----------------------
        // Fraction of the opponent's army in transit. A colonizer keeps ships flowing to
        // fresh neutrals (high); a turtle parks them on its wall (low). Sampled over the
        // whole window — the gap is present throughout, not just late.
        let inflight: f64 = state.fleets.iter().filter(|f| f.owner == opp).map(|f| f.count).sum();
        let total = of.total_units.max(1e-9);
        self.mobility_sum += inflight / total;
        // Saturating presence of a real standing army (so "low mobility" only reads as
        // defend once there *is* an army to park): units mapped through x/(x+ref) ∈ [0,1).
        self.mass_presence_sum += of.total_units / (of.total_units + MASS_REF);

        // ---- aggression (attack): ships aimed at us + forward staging --------
        // (a) Peak total hostile ships aimed at *our* nodes over the window (a single
        // sustained wave counts once, not once per tick).
        let look = features::default_look_ahead(params);
        let incoming_now: f64 = features::my_nodes(state, me)
            .iter()
            .map(|&n| features::incoming_hostile(state, me, n, look))
            .sum();
        self.incoming_peak = self.incoming_peak.max(incoming_now);

        // (b) Opponent army staged on the contact frontier (adjacent to us) beyond a thin
        // holding level — a push being assembled. Summed over the window.
        let split = opponent_frontier_garrison(state, opp, me);
        self.staging_excess_sum += split.staging_excess();

        // ---- held rear mass (defend signature) -------------------------------
        // Garrison sitting on the opponent's *interior* (non-contact) nodes, as a fraction
        // of its whole army. Only a turtle stocks its rear: a colonizer's ships are in
        // transit (counted in mobility, not garrison) and an attacker's are pushed to the
        // front (counted in staging). Scaled by presence so an empty early board does not
        // read as a held wall.
        let presence = of.total_units / (of.total_units + MASS_REF);
        self.rear_garrison_sum += split.rear_fraction() * presence;

        self.prev = Some(of);
        self.ticks_seen += 1;
    }

    /// The current best estimate of the opponent's mix, normalized onto the simplex.
    ///
    /// The three legible signals, made commensurate (all are per-tick intensities):
    /// * **attack** — aggression, read primarily from **ships actually aimed at our
    ///   nodes** (`incoming`), which *only* an attacker produces. Forward frontier
    ///   **staging** is a weak, secondary corroborator: a *defender* also masses on its
    ///   frontier (reinforce / man-the-wall), so staging alone cannot mean "attack" — it
    ///   counts only lightly and only above a generous baseline.
    /// * **colonize** — expansion rate + **mobility** (army kept in motion toward fresh
    ///   neutrals). A flooding economist scores high on both.
    /// * **defend** — **stationary mass**: a real army (mass-presence) held *still* (low
    ///   mobility) and *not thrown at us*. This is what separates a turtle (parks its
    ///   army) from a colonizer (keeps it moving) and from an attacker (sends it at us).
    ///
    /// The load-bearing fix vs. a naive scorer: the *incoming* part of aggression (a real
    /// push) is what discounts the colonize and defend reads, **not** the leaky staging
    /// part. Otherwise a turtle — whose reinforce rule answers our scouting pokes by
    /// piling mass on its frontier — would have that defensive reaction misread as a
    /// push and then *subtracted from its own defend score*, which is exactly what made
    /// defend collapse to ~0 for every mix containing any attack weight.
    ///
    /// If nothing was observed (a totally passive opponent) it falls back to the balanced
    /// centre — the maximally uncertain guess.
    pub fn estimate(&self) -> Mix {
        let ticks = self.ticks_seen.max(1) as f64;
        let avg_mobility = (self.mobility_sum / ticks).clamp(0.0, 1.0);
        let avg_presence = (self.mass_presence_sum / ticks).clamp(0.0, 1.0);

        // The two aggression components, kept *separate* because they mean different
        // things. `incoming` (ships streaking at our nodes) is the clean attack tell only
        // an attacker produces. `staging` (forward frontier mass) is shared with a
        // defender's wall, so it is a weak corroborator, not a primary signal.
        let incoming = self.incoming_peak / ticks;
        let staging = self.staging_excess_sum / ticks;

        // Attack: dominated by ships aimed at us; staging adds only a light nudge.
        let attack = self.cfg.attack_gain * (incoming + STAGING_WEIGHT * staging);

        // Only a *genuine push* (ships inbound to us) discounts the other two reads — a
        // defender's reactive frontier mass (staging) must NOT, or it cancels its own
        // defend score.
        let push = incoming;

        // Colonize: expansion rate + the share of mobility *not* explained by an inbound
        // attack. (An attacker also keeps ships in motion, but toward us — that mobility
        // is attributed to attack via the subtraction, so only a peaceful flooder scores
        // here.) Expansion is a weak tie-break (similar across archetypes on small maps),
        // so it is modestly weighted.
        let peaceful_mobility = (avg_mobility - AGGRO_MOBILITY * push).max(0.0);
        let colonize = self.cfg.colonize_gain
            * (EXPANSION_WEIGHT * (self.terr_gain / ticks) + peaceful_mobility);

        // Defend: a real army held *still*. `parked = presence·(1−mobility)` is "a real
        // army that is not in transit" — the tell that cleanly separates a pure turtle (low
        // mobility) from a colonizer (high mobility) on these small maps. Discount only by a
        // real inbound push (not staging) so an attacker's between-commit lull does not read
        // as a wall, while a genuine turtle keeps its full defend score.
        //
        // (An explicit "held rear-garrison fraction" signal was tried as a second defend
        // tell — see [`Scout::observe`] — to catch a defend+attack mix whose mobility is
        // inflated by attacking. On these 7–9-node maps it *hurt*: the Automatons all run a
        // shared expand rule, so even a turtle's rear garrison fraction is modest and the
        // signal washed out the cleaner mobility gap, mis-reading pure defenders as
        // colonizers. It is therefore accumulated for diagnostics but kept out of the score.
        // This is itself a finding: defend is the least legible axis from these observables.)
        let parked = avg_presence * (1.0 - avg_mobility);
        let defend = self.cfg.defend_gain * (parked - DEFEND_AGGRO_DISCOUNT * push).max(0.0);

        Mix::new(colonize, defend, attack)
    }
}

/// Units at which an opponent's army counts as "half a real presence" (the x/(x+ref)
/// saturation point). Below this, low mobility does not yet read as defending — there is
/// no army to park. Chosen on the scale of an early-game garrison.
const MASS_REF: f64 = 20.0;
/// How much each unit of territory-gain-per-tick contributes to the colonize score
/// relative to a unit of mobility. Territory gain is small and weakly differentiated on
/// these maps, so it is only a modest tie-break toward colonize.
const EXPANSION_WEIGHT: f64 = 2.0;
/// Weight of forward frontier **staging** in the attack score, relative to ships actually
/// inbound to us. Small: a defender also stages mass on its wall, so staging is only a
/// weak corroborator of an attack already evidenced by inbound ships — not a tell on its
/// own. (Keeping this low is what stops a pure turtle's reinforced frontier from reading
/// as an attacker.)
const STAGING_WEIGHT: f64 = 0.5;
/// How strongly a real inbound **push** "claims" mobility away from the colonize read. An
/// attacker has high mobility *and* sends ships at us; this factor (>1) attributes that
/// mobility to attack so the attacker does not read as a colonizer. Applied to the push
/// (inbound ships) only, so a peaceful flooder keeps its mobility-as-colonize credit.
const AGGRO_MOBILITY: f64 = 1.2;
/// How much an observed inbound **push** discounts the defend read (an army being thrown
/// at us is not a wall). Small and applied to the push only — NOT to frontier staging —
/// so a turtle's own defensive frontier mass does not cancel its defend score (the bug
/// that previously zeroed defend for every attack-containing mix).
const DEFEND_AGGRO_DISCOUNT: f64 = 0.5;

/// Opponent garrison split into "staging" (garrison on the contact frontier, i.e. a stack
/// poised to strike us) vs. "rear" (garrison on interior nodes, sitting still). Fractions
/// are taken over the opponent's total **units** so the scores are scale-free (a big and a
/// small game read the same posture the same way).
struct FrontierGarrison {
    /// Garrison on the opponent's nodes that touch one of *my* nodes (the contact line).
    staging: f64,
    /// Garrison on the opponent's interior nodes (touching no node of mine).
    rear: f64,
    /// Opponent total units (garrisons + fleets), the scale-free denominator.
    total: f64,
}

impl FrontierGarrison {
    /// The fraction of the opponent's army staged on the contact frontier *beyond* a
    /// thin holding level — the part that looks like a push being assembled.
    fn staging_excess(&self) -> f64 {
        if self.total <= 0.0 {
            return 0.0;
        }
        // A defender also keeps mass on the frontier, so only the staging *fraction*
        // above a modest baseline counts as attack-massing.
        (self.staging / self.total - 0.25).max(0.0)
    }

    /// Fraction of the opponent's army held as garrison on its **interior** (non-contact)
    /// nodes — the man-the-wall / held-reserve signature of a defender.
    fn rear_fraction(&self) -> f64 {
        if self.total <= 0.0 {
            return 0.0;
        }
        (self.rear / self.total).clamp(0.0, 1.0)
    }
}

/// Compute the opponent's garrison split into contact-frontier ("staging") vs. interior
/// ("rear"), and its total units. The "contact frontier" for `opp` is its nodes adjacent
/// to a node owned by `me` (the scout) — where an attacker stages a push; everything else
/// it owns is rear, where a defender stocks its wall.
fn opponent_frontier_garrison(state: &GameState, opp: Owner, me: Owner) -> FrontierGarrison {
    let mut staging = 0.0;
    let mut rear = 0.0;
    for n in 0..state.nodes.len() {
        if state.nodes[n].owner != opp {
            continue;
        }
        let touches_me = state.neighbors(n).iter().any(|&(_, nb)| state.nodes[nb].owner == me);
        if touches_me {
            staging += state.nodes[n].garrison;
        } else {
            rear += state.nodes[n].garrison;
        }
    }
    let total = state.total_units(opp).max(staging + rear);
    FrontierGarrison { staging, rear, total }
}

/// Convenience: run a full scouting pass over a *fixed* opponent for `cfg.scout_ticks`
/// ticks on `base`, with the scout itself playing a neutral economic posture, and return
/// the inferred mix. Used by tests and the validation harness to score inference
/// accuracy in isolation (the live capstone player does the same accumulation inline).
///
/// `me` scouts `opp`. The opponent policy is supplied by the caller; the scout plays a
/// light expander so it neither falls hopelessly behind nor perturbs the read.
pub fn scout_opponent(
    base: &GameState,
    me: Owner,
    opp: Owner,
    opp_policy: &mut dyn cell_core::Policy,
    cfg: InferConfig,
    params: &Params,
) -> Scout {
    let mut scout = Scout::new(opp, cfg);
    let mut state = base.clone();
    let mut scout_brain = crate::capstone::scout_posture();
    opp_policy.reset();
    while !scout.done() && !state.is_eliminated(me) && !state.is_eliminated(opp) {
        scout.observe(&state, me, params);
        // Both sides act; the scout uses a neutral economic posture so it observes the
        // opponent's *uncoerced* early behaviour.
        let (pa, pb) = if me == Owner::A { (me, opp) } else { (opp, me) };
        let cmds_a;
        let cmds_b;
        if pa == me {
            cmds_a = scout_brain.decide(&state, pa, params);
            cmds_b = opp_policy.decide(&state, pb, params);
        } else {
            cmds_a = opp_policy.decide(&state, pa, params);
            cmds_b = scout_brain.decide(&state, pb, params);
        }
        for c in cmds_a {
            state.launch_with(Owner::A, c, params);
        }
        for c in cmds_b {
            state.launch_with(Owner::B, c, params);
        }
        state.step(params);
    }
    scout
}

/// Infer a fixed opponent's mix by scouting it on `base`: `me` observes `opp`'s early
/// behaviour, then we return the simplex estimate. The thin entry point most callers
/// want (the validation harness and the live [`crate::capstone::Capstone`] use the
/// streaming [`Scout`] directly).
pub fn infer_mix(
    base: &GameState,
    me: Owner,
    opp: Owner,
    opp_policy: &mut dyn cell_core::Policy,
    cfg: InferConfig,
    params: &Params,
) -> Mix {
    scout_opponent(base, me, opp, opp_policy, cfg, params).estimate()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::automaton::AutomatonSpec;
    use crate::mix::Corner;
    use cell_core::maps::corridor7;

    fn params() -> Params {
        Params { r: 0.6, k: 2.25, l: 0.15, ..Params::default() }
    }

    /// Scouting a pure-corner opponent yields an estimate whose nearest corner is that
    /// corner — the basic "can we read the obvious cases" check. This is the headless
    /// proxy for "a human can read the mix off the HUD".
    #[test]
    fn infers_pure_corners() {
        let base = corridor7().state;
        let cfg = InferConfig::default();
        for corner in Corner::ALL {
            let spec = AutomatonSpec::from_mix(corner.as_mix());
            // Scout from seat B observing the opponent on seat A.
            let mut opp = spec.make();
            let est = scout_opponent(&base, Owner::B, Owner::A, opp.as_mut(), cfg, &params()).estimate();
            assert_eq!(
                est.nearest_corner(),
                corner,
                "scouting {} read as {:?} (mix {:?})",
                corner.name(),
                est.nearest_corner(),
                est
            );
        }
    }

    /// Diagnostic (ignored): print the inferred mix for every corner/edge/centre on each
    /// map, to calibrate the gains. Run with
    /// `cargo test -p automaton diag_inference -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn diag_inference() {
        use cell_core::maps::all_maps;
        let cfg = InferConfig::default();
        let probe = [
            ("C", Mix::new(1.0, 0.0, 0.0)),
            ("D", Mix::new(0.0, 1.0, 0.0)),
            ("A", Mix::new(0.0, 0.0, 1.0)),
            ("CD", Mix::new(1.0, 1.0, 0.0)),
            ("DA", Mix::new(0.0, 1.0, 1.0)),
            ("CA", Mix::new(1.0, 0.0, 1.0)),
            ("bal", Mix::centre()),
        ];
        for m in all_maps() {
            println!("\n== map {} ==", m.name);
            for (lbl, mix) in probe {
                let spec = AutomatonSpec::from_mix(mix);
                let mut opp = spec.make();
                let scout = scout_opponent(&m.state, Owner::B, Owner::A, opp.as_mut(), cfg, &params());
                let (terr_rate, mobility, incoming, staging, rear) = scout.debug_signals();
                let est = scout.estimate();
                println!(
                    "  {:>3}: terr/t {:.3} mob {:.2} in {:.3} stag {:.3} rear {:.3} | est c={:.2} d={:.2} a={:.2} -> {:?} (err {:.3})",
                    lbl, terr_rate, mobility, incoming, staging, rear, est.c, est.d, est.a, est.nearest_corner(), est.distance(mix)
                );
            }
        }
    }

    /// Inference is deterministic: same opponent, same observation, same estimate.
    #[test]
    fn inference_is_deterministic() {
        let base = corridor7().state;
        let cfg = InferConfig::default();
        let spec = AutomatonSpec::from_mix(Mix::new(2.0, 1.0, 1.0));
        let mut o1 = spec.make();
        let mut o2 = spec.make();
        let e1 = scout_opponent(&base, Owner::B, Owner::A, o1.as_mut(), cfg, &params()).estimate();
        let e2 = scout_opponent(&base, Owner::B, Owner::A, o2.as_mut(), cfg, &params()).estimate();
        assert_eq!(e1, e2);
    }
}
