//! Synthesize a **single fixed, legible DSL policy** from a [`Mix`].
//!
//! ## The idea (and why it is legible)
//!
//! `02-ai-opponents.md` requires every Automaton to be "a static policy with exactly
//! one diagnosable seam", and `01`/`DSL_SPEC.md` establish that **the triad cycle
//! lives in the priority and presence of the defend/attack rules above a shared expand
//! rule** — "all three archetypes share one `expand` rule and differ only in what sits
//! above it." We honour that exactly: an Automaton's policy is the **union** of the
//! three archetypes' signature rules, where each archetype's block is lifted by a
//! priority proportional to its weight in the mix.
//!
//! Because the DSL's evaluator commits each node to the **first matching rule**
//! (`dsl` module docs), the highest-weighted archetype's signature rules sit on top and
//! claim a node's garrison first — so the Automaton's *posture* tracks the mix
//! continuously: crank `a` up and the strike rule rises to the top and fires on a
//! slimmer majority; crank `d` up and the reinforce/man-the-wall rules pre-empt
//! expansion; crank `c` up and nothing sits above expansion at all. At a pure corner
//! the off-archetype blocks sink to the bottom with thresholds they almost never clear,
//! recovering (a faithful twin of) the `cell_core::dsl::strategies` archetype.
//!
//! ## Where the diagnosable seam comes from
//!
//! The seam is not bolted on — it is the **opportunity cost the mix dictates**, exactly
//! as `01-mechanics.md` frames the triad:
//! * a colonize-heavy mix keeps almost nothing home → **thin rear**, the seam an
//!   attacker exploits (the canonical Automaton-0 flaw: "leaves its rear thin");
//! * a defend-heavy mix holds ships idle → **cedes the land grab**, the seam a
//!   colonizer exploits;
//! * an attack-heavy mix over-commits into the defender advantage `k` and bleeds the
//!   commitment loss `l` → the seam a defender exploits.
//!
//! So the *same* weight that defines the mix also defines its weakness, and the counter
//! the capstone selects is the strategy that punishes that weakness — which is why
//! "infer the mix" and "pick the counter" are the same skill (`02`).

use cell_core::dsl::{Cmp, OwnerFilter, Rank, Scope};
use cell_core::{Action, Condition, Feature, FractionBucket, NodeSelector, Rule, RulePolicy};

use crate::mix::Mix;

/// A named Automaton: a fixed mix plus the legible DSL policy it compiles to.
///
/// The policy is **hidden** from the capstone player at match time (the player only
/// observes legible features and must infer the mix) — `hidden + fixed`, the design's
/// chosen capstone variant. The `mix` is retained for scoring inference accuracy and
/// for ordering the ladder by centrality, *not* given to the player.
#[derive(Clone)]
pub struct AutomatonSpec {
    /// The hidden ground-truth mix.
    pub mix: Mix,
    /// A human label (e.g. `C`, `C2A1`, `bal`).
    pub name: String,
    /// The compiled, fixed DSL policy.
    pub policy: RulePolicy,
}

impl AutomatonSpec {
    /// Build an Automaton from a mix, compiling its fixed DSL policy.
    pub fn from_mix(mix: Mix) -> AutomatonSpec {
        let name = mix.label();
        let policy = compile(mix, &name);
        AutomatonSpec { mix, name, policy }
    }

    /// A fresh boxed engine policy for this Automaton (the match runner consumes a
    /// `&mut dyn Policy`). Each call yields an independent, stateless instance.
    pub fn make(&self) -> Box<dyn cell_core::Policy> {
        Box::new(cell_core::DslPolicy::new(self.policy.clone()))
    }

    /// The difficulty metric for this rung: simplex centrality (`02`).
    pub fn centrality(&self) -> f64 {
        self.mix.centrality()
    }
}

// ---------------------------------------------------------------------------
// Priority skeleton
// ---------------------------------------------------------------------------
//
// The proven `cell_core::dsl::strategies` archetypes fix a *relative* rule ordering
// that must survive at the corners, so we anchor each rule at a fixed **role priority**
// and lift only the two engagement rules (strike, reinforce) by weight. The role
// skeleton (high → low) is:
//
//   ENGAGE  (strike / reinforce)   -- emergency commitments; lifted by weight
//   WALL    (man the wall)         -- proactive consolidation (defend's #2)
//   EXPAND  (claim neutrals)       -- the shared economy every archetype runs
//   ROUTE   (mass-to-enemy /       -- spend whatever's left; direction is the signal,
//            flow-to-neutral)         priority is always lowest
//
// This reproduces strategies.rs at the corners: pure-attack ⇒ strike(ENGAGE) > expand >
// mass(ROUTE); pure-defend ⇒ reinforce(ENGAGE) > wall > expand > flow; pure-colonize ⇒
// expand > flow (no engagement rule clears its threshold). The weight `lift` only
// re-orders the two ENGAGE rules *relative to each other* when both carry weight (the
// central regime), which is exactly where posture should be contested.

/// Role base priorities (see the skeleton comment above).
const P_ENGAGE: i32 = 200;
const P_WALL: i32 = 150;
const P_EXPAND: i32 = 100;
const P_ROUTE: i32 = 50;

/// Engagement rules are lifted by their weight so that, when both an attack and a defend
/// impulse are live, the heavier one commits the node first. Small enough to keep ENGAGE
/// above WALL for any weight (`P_ENGAGE - P_WALL = 50 > LIFT`).
const LIFT: f64 = 40.0;

/// **Presence gate.** An archetype's *posture* rules (strike for attack; reinforce +
/// man-the-wall for defend; mass / neutral-flow routing) are emitted **only** when that
/// archetype's weight clears this threshold. This is what keeps a pure (or near-pure)
/// corner faithful to the proven `strategies.rs` archetype: a colonizer with `d≈0` must
/// have *no* reinforce rule at all, otherwise it turtles up under attack and stops being
/// the thin economist the cycle needs (which would silently break attack≻colonize). Just
/// above the gate the posture is mild (high strike threshold / low reinforce margin) and
/// ramps to full strength at the corner, so the simplex is still covered continuously.
const GATE: f64 = 0.12;

#[inline]
fn lift(weight: f64) -> i32 {
    (weight * LIFT).round() as i32
}

// ---------------------------------------------------------------------------
// The compiler
// ---------------------------------------------------------------------------

/// Compile a [`Mix`] into a single prioritized [`RulePolicy`].
///
/// Weights modulate two things, leaving the legible role skeleton intact:
/// * **firing thresholds** — how eagerly each posture engages (a slim-`a` mix strikes
///   only on a big majority; a heavy-`d` mix reinforces on a generous margin; a colonial
///   mix keeps almost nothing home). This is what makes the posture vary *continuously*
///   across the simplex and gives each mix its diagnosable seam.
/// * **the relative priority of the two engagement rules** — so near the centre the
///   heavier impulse wins a contested node, while at a corner the off-archetype rules
///   sit dormant below thresholds they almost never clear.
pub fn compile(mix: Mix, name: &str) -> RulePolicy {
    let Mix { c, d, a } = mix;
    let mut rules: Vec<Rule> = Vec::new();

    // ----- Attack: STRIKE (engagement, lifted by a; gated by a) ------------
    // Strike threshold tightens as `a` grows: a barely-aggressive Automaton only strikes
    // on a big majority (so it almost never does), a pure attacker strikes on a thin one.
    // Maps a in [0,1] -> threshold in [1.85 .. 1.10]; the 1.10 floor matches
    // `strategies::attack`'s commit majority. Omitted entirely below the gate so a pure
    // colonizer/defender never strikes (faithful corner).
    if a > GATE {
        let strike_threshold = 1.85 - 0.75 * a;
        rules.push(
            Rule::new(
                P_ENGAGE + lift(a),
                Condition::cmp(Feature::AttackForceRatio, Cmp::Ge, strike_threshold),
                Action {
                    source: NodeSelector::new(Scope::Anywhere, OwnerFilter::Mine, Rank::WeakestGarrison),
                    fraction: FractionBucket::All,
                    target: NodeSelector::new(Scope::Adjacent, OwnerFilter::Enemy, Rank::WeakestGarrison),
                },
            )
            .noted("strike: commit all at the weakest adjacent enemy when we have the majority"),
        );
    }

    // ----- Defend: REINFORCE + MAN THE WALL (gated by d) -------------------
    // Both defend rules are omitted below the gate: a pure colonizer must NOT reinforce
    // under attack (that would turn it into a turtle and break attack≻colonize) and must
    // NOT consolidate a wall. Above the gate the reinforce margin widens with `d` (a
    // heavy defender reinforces on a generous safety margin, a barely-defensive one only
    // when a neighbour is already losing): d in [0,1] -> margin in [1.05 .. 1.85].
    if d > GATE {
        let reinforce_margin = 1.05 + 0.8 * d;
        rules.push(
            Rule::new(
                P_ENGAGE + lift(d),
                Condition::and(
                    Condition::cmp(Feature::WeakestNeighborForceRatio, Cmp::Lt, reinforce_margin),
                    Condition::cmp(Feature::Garrison, Cmp::Ge, 1.0),
                ),
                Action {
                    source: NodeSelector::new(Scope::Anywhere, OwnerFilter::Mine, Rank::WeakestGarrison),
                    fraction: FractionBucket::All,
                    target: NodeSelector::new(Scope::Adjacent, OwnerFilter::Mine, Rank::LowestForceRatio),
                },
            )
            .noted("reinforce: rush the most threatened friendly neighbor"),
        );
        // Man the wall: interior nodes feed the weakest adjacent friendly node when calm.
        // Floor falls as `d` grows (a heavy defender consolidates eagerly; a light one
        // only huge stacks): d in [0,1] -> floor in [12 .. 4].
        let wall_floor = 12.0 - 8.0 * d;
        rules.push(
            Rule::new(
                P_WALL,
                Condition::and(
                    Condition::cmp(Feature::IsFrontier, Cmp::Lt, 0.5),
                    Condition::and(
                        Condition::cmp(Feature::Garrison, Cmp::Ge, wall_floor),
                        Condition::cmp(Feature::IncomingHostile, Cmp::Le, 0.0),
                    ),
                ),
                Action {
                    source: NodeSelector::new(Scope::Anywhere, OwnerFilter::Mine, Rank::WeakestGarrison),
                    fraction: FractionBucket::Half,
                    target: NodeSelector::new(Scope::Adjacent, OwnerFilter::Mine, Rank::WeakestGarrison),
                },
            )
            .noted("man the wall: feed surplus to the weakest adjacent friendly node"),
        );
    }

    // ----- Shared economy: EXPAND (always present) -------------------------
    // Every archetype expands; the cycle is in the posture *above* this baseline. The
    // expansion is more aggressive (lower keep-floor, bigger fraction) the more colonial
    // the mix. A heavy colonizer keeps ~nothing and floods; a heavy defender keeps a big
    // home reserve. keep-floor: c=1 -> 2 (flood), c=0 -> 6 (heavy home).
    let keep = 6.0 - 4.0 * c;
    // Colonial mixes commit three-quarters; cautious mixes a half.
    let expand_frac = if c >= 0.5 { FractionBucket::ThreeQuarter } else { FractionBucket::Half };
    rules.push(
        Rule::new(
            P_EXPAND,
            Condition::cmp(Feature::Garrison, Cmp::Ge, keep),
            Action {
                source: NodeSelector::new(Scope::Anywhere, OwnerFilter::Mine, Rank::WeakestGarrison),
                fraction: expand_frac,
                target: NodeSelector::new(Scope::Adjacent, OwnerFilter::Empty, Rank::BestColonizeValue),
            },
        )
        .noted("expand: send surplus to the best adjacent empty node"),
    );

    // ----- Routing (lowest): MASS toward enemy + FLOW toward neutral -------
    // These spend leftover ships; their *direction* is the signal, not their priority.
    // Crucially they are **weight-gated at compile time**: a pure colonizer must never
    // mass on the enemy (that would blur it into a defender), and a pure attacker should
    // march to contact rather than dribble toward neutrals it won't defend. So we emit
    // the mass rule only when the mix wants contact (`a` meaningful) and always emit the
    // neutral-flow rule (every archetype benefits from reaching the contested middle),
    // but a heavy attacker's mass rule sits just above flow so its army concentrates
    // forward. This mirrors strategies.rs: colonize has flow-to-neutral only; attack has
    // mass-to-enemy as its lowest rule.
    if a > GATE {
        rules.push(
            Rule::new(
                P_ROUTE + 5,
                Condition::cmp(Feature::Garrison, Cmp::Ge, 2.0),
                Action {
                    source: NodeSelector::new(Scope::Anywhere, OwnerFilter::Mine, Rank::WeakestGarrison),
                    fraction: FractionBucket::All,
                    target: NodeSelector::new(Scope::Adjacent, OwnerFilter::Mine, Rank::ClosestToEnemy),
                },
            )
            .noted("mass: push the army one hop toward the enemy front"),
        );
    }
    // Forward-flow toward unclaimed space so growth keeps reaching the contested middle.
    // Routed toward NEUTRAL (the colonize signature). Lowest priority of all. A pure
    // attacker (c ~ 0) drops this so it does not fritter ships into neutral space instead
    // of massing; everyone else keeps it.
    if c > GATE || (a <= GATE && d <= GATE) {
        let flow_floor = 8.0 - 4.0 * c;
        rules.push(
            Rule::new(
                P_ROUTE,
                Condition::cmp(Feature::Garrison, Cmp::Ge, flow_floor),
                Action {
                    source: NodeSelector::new(Scope::Anywhere, OwnerFilter::Mine, Rank::WeakestGarrison),
                    fraction: FractionBucket::Half,
                    target: NodeSelector::new(Scope::Adjacent, OwnerFilter::Mine, Rank::ClosestToNeutral),
                },
            )
            .noted("flow forward: push surplus toward unclaimed space"),
        );
    }

    // `RulePolicy::new` sorts by descending priority (stable). The name is leaked so the
    // engine's `&'static str` name slot is satisfied; ladder labels are few and live for
    // the whole process, so this is a bounded, intentional leak (not a per-tick leak).
    let static_name: &'static str = Box::leak(format!("Automaton[{name}]").into_boxed_str());
    RulePolicy::new(static_name, rules)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mix::Corner;
    use cell_core::maps::all_maps;
    use cell_core::Params;

    fn operating_params() -> Params {
        Params { r: 0.6, k: 2.25, l: 0.15, ..Params::default() }
    }

    const HORIZON: u64 = 600;

    /// Mean score for `first` over both seatings on one base state (cancels the
    /// "A acts before B" tie-break) — the same fair-duel primitive the DSL tests use.
    fn duel_score(
        base: &cell_core::GameState,
        first: &dyn Fn() -> Box<dyn cell_core::Policy>,
        second: &dyn Fn() -> Box<dyn cell_core::Policy>,
        params: &Params,
    ) -> f64 {
        let s1 = base
            .clone()
            .run_match(first().as_mut(), second().as_mut(), params, HORIZON)
            .score_a;
        let s2 = -base
            .clone()
            .run_match(second().as_mut(), first().as_mut(), params, HORIZON)
            .score_a;
        0.5 * (s1 + s2)
    }

    fn duel_over_maps(
        first: &dyn Fn() -> Box<dyn cell_core::Policy>,
        second: &dyn Fn() -> Box<dyn cell_core::Policy>,
        params: &Params,
    ) -> f64 {
        let maps = all_maps();
        let n = maps.len() as f64;
        maps.iter().map(|m| duel_score(&m.state, first, second, params)).sum::<f64>() / n
    }

    /// A pure-corner Automaton plays the corresponding hand-written DSL archetype
    /// closely (it *is* that archetype, by construction). Loose floor: we only assert it
    /// is not a *different* strategy that loses badly head-to-head — the same bar the
    /// `dsl_archetypes_match_their_hardcoded_twins` test uses.
    #[test]
    fn pure_corners_match_dsl_archetypes() {
        let params = operating_params();
        let auto_colonize = || AutomatonSpec::from_mix(Corner::Colonize.as_mix()).make();
        let auto_defend = || AutomatonSpec::from_mix(Corner::Defend.as_mix()).make();
        let auto_attack = || AutomatonSpec::from_mix(Corner::Attack.as_mix()).make();
        let dsl_colonize = || Box::new(cell_core::DslPolicy::new(cell_core::dsl::strategies::colonize())) as Box<dyn cell_core::Policy>;
        let dsl_defend = || Box::new(cell_core::DslPolicy::new(cell_core::dsl::strategies::defend())) as Box<dyn cell_core::Policy>;
        let dsl_attack = || Box::new(cell_core::DslPolicy::new(cell_core::dsl::strategies::attack())) as Box<dyn cell_core::Policy>;

        let floor = -0.45;
        let col = duel_over_maps(&auto_colonize, &dsl_colonize, &params);
        let def = duel_over_maps(&auto_defend, &dsl_defend, &params);
        let atk = duel_over_maps(&auto_attack, &dsl_attack, &params);
        assert!(col > floor, "pure-colonize Automaton too far from DSL colonize: {col}");
        assert!(def > floor, "pure-defend Automaton too far from DSL defend: {def}");
        assert!(atk > floor, "pure-attack Automaton too far from DSL attack: {atk}");
    }

    /// The pure-corner Automatons reproduce the rock-paper-scissors cycle at the
    /// operating point (attack≻colonize, colonize≻defend, defend≻attack). This is the
    /// step-1/3 result, re-confirmed through the synthesized policies the ladder uses,
    /// so the ladder's corners are genuinely the triad.
    #[test]
    fn pure_corners_reproduce_cycle() {
        let params = operating_params();
        let col = || AutomatonSpec::from_mix(Corner::Colonize.as_mix()).make();
        let def = || AutomatonSpec::from_mix(Corner::Defend.as_mix()).make();
        let atk = || AutomatonSpec::from_mix(Corner::Attack.as_mix()).make();
        let ac = duel_over_maps(&atk, &col, &params);
        let cd = duel_over_maps(&col, &def, &params);
        let da = duel_over_maps(&def, &atk, &params);
        let eps = 0.05;
        assert!(ac > eps, "attack should beat colonize, margin {ac}");
        assert!(cd > eps, "colonize should beat defend, margin {cd}");
        assert!(da > eps, "defend should beat attack, margin {da}");
    }

    /// Diagnostic (ignored): per-map corner margins for the Automaton vs the reference
    /// DSL strategies, to localize a broken edge. Run with
    /// `cargo test -p automaton diag_corner_margins -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn diag_corner_margins() {
        let params = operating_params();
        let acol = || AutomatonSpec::from_mix(Corner::Colonize.as_mix()).make();
        let adef = || AutomatonSpec::from_mix(Corner::Defend.as_mix()).make();
        let aatk = || AutomatonSpec::from_mix(Corner::Attack.as_mix()).make();
        let scol = || Box::new(cell_core::DslPolicy::new(cell_core::dsl::strategies::colonize())) as Box<dyn cell_core::Policy>;
        let sdef = || Box::new(cell_core::DslPolicy::new(cell_core::dsl::strategies::defend())) as Box<dyn cell_core::Policy>;
        let satk = || Box::new(cell_core::DslPolicy::new(cell_core::dsl::strategies::attack())) as Box<dyn cell_core::Policy>;
        println!("\n== Automaton corners, per map ==");
        for m in all_maps() {
            let ac = duel_score(&m.state, &aatk, &acol, &params);
            let cd = duel_score(&m.state, &acol, &adef, &params);
            let da = duel_score(&m.state, &adef, &aatk, &params);
            println!("{:>13}: A>C {:+.3}  C>D {:+.3}  D>A {:+.3}", m.name, ac, cd, da);
        }
        println!("== Cross: Automaton-attack vs DSL-colonize, and DSL-attack vs Automaton-colonize ==");
        for m in all_maps() {
            let a_auto_vs_dsl = duel_score(&m.state, &aatk, &scol, &params);
            let dsl_vs_a_auto = duel_score(&m.state, &satk, &acol, &params);
            let dsl_vs_dsl = duel_score(&m.state, &satk, &scol, &params);
            println!(
                "{:>13}: autoA>dslC {:+.3}  dslA>autoC {:+.3}  dslA>dslC {:+.3}",
                m.name, a_auto_vs_dsl, dsl_vs_a_auto, dsl_vs_dsl
            );
        }
        let _ = (sdef,);
    }

    /// A synthesized Automaton is a legal, deterministic policy that beats an idle
    /// opponent (it actually plays the game).
    #[test]
    fn automaton_plays_and_is_deterministic() {
        let params = operating_params();
        let base = all_maps()[0].state.clone();
        let spec = AutomatonSpec::from_mix(Mix::centre());
        let o1 = base.clone().run_match(spec.make().as_mut(), idle().as_mut(), &params, 200);
        let o2 = base.clone().run_match(spec.make().as_mut(), idle().as_mut(), &params, 200);
        assert_eq!(o1.score_a, o2.score_a, "deterministic");
        assert!(o1.score_a > 0.0, "a balanced Automaton should beat idle, got {}", o1.score_a);
    }

    fn idle() -> Box<dyn cell_core::Policy> {
        struct Idle;
        impl cell_core::Policy for Idle {
            fn name(&self) -> &'static str {
                "Idle"
            }
            fn decide(
                &mut self,
                _s: &cell_core::GameState,
                _me: cell_core::Owner,
                _p: &Params,
            ) -> Vec<cell_core::Command> {
                Vec::new()
            }
        }
        Box::new(Idle)
    }
}
