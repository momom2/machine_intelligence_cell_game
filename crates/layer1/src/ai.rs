//! The Layer-1 **Automaton** — a fixed, handwritten reactive micro-policy.
//!
//! Per `02-ai-opponents.md`, the Automaton is "a fixed, handwritten rule-set, deliberately
//! simple, with **one clear exploitable flaw**." This is the Layer-1 (spatial/micro) analog:
//! it issues [`MoveOrder`]s over the structure each decision tick, concentrating force,
//! assaulting where it has local numerical superiority, and reinforcing threatened
//! sub-structures.
//!
//! # The policy (priority-ordered; first matching rule that finds idle ships wins)
//!
//! Evaluated from the Automaton's own seat each decision tick:
//!
//! 1. **REINFORCE a losing bubble.** If one of my sub-structures is *contested* (an enemy
//!    ship is inside it or within engagement range of it) and I am outnumbered locally,
//!    rush idle ships from my nearest *safe* sub-structure to it. Concentration of force is
//!    a theorem under the square law, so feeding a fight I am losing can flip it.
//! 2. **ASSAULT on local superiority.** Find the enemy/neutral sub-structure where, if I
//!    committed the idle ships of my nearest owned sub-structure, I would *outnumber* the
//!    defenders by a comfortable margin. Send them. (Neutrals have no defenders, so a lone
//!    scout claims them — cheap expansion.)
//! 3. **MASS at the forward sub-structure.** Otherwise, pull idle ships from rear
//!    sub-structures toward my most forward owned sub-structure (the one closest to the
//!    enemy), building a stack for the next assault. This is the "concentrate, then strike"
//!    behaviour.
//!
//! Each rule only ever moves **idle** ships (matching [`Interior::issue_order`]), and only
//! one order per (rule, source) per decision tick, so the Automaton commits gradually rather
//! than teleporting its whole army.
//!
//! # THE DIAGNOSABLE SEAM (documented, single, exploitable)
//!
//! **The Automaton always commits its reserve to the *nearest* live fight (rules 1 then 2
//! pick the closest target), and rule 3 keeps pulling rear ships *forward*. It never keeps a
//! rear guard.** Concretely, the seam is:
//!
//! > *Once the Automaton is engaged at the front, its rear/home sub-structures are bled to
//! > feed the front and left thinly held. A small detachment that flanks to an undefended
//! > rear sub-structure can capture it uncontested while the Automaton is "all-in" forward —
//! > and because a captured sub-structure keeps producing, the flank snowballs.*
//!
//! This is exactly the canonical Automaton-0 flaw from `02` ("always reinforces the frontier
//! nearest the enemy and leaves its rear thin"), realised spatially. It is *diagnosable*
//! (watch it strip its home garrison the moment contact happens) and *exploitable* (hold a
//! detachment back, send it wide to the enemy's home/rear post when the front engages). The
//! [`crate::ai`] tests demonstrate a flanker beating the Automaton via this seam.
//!
//! The Automaton is stateless across ticks (a pure function of the observed [`Interior`]),
//! so it is fully deterministic and the same type can drive either faction.

use crate::sim::{SimParams, Interior};
use crate::types::{Faction, FractionBucket, MoveOrder, SubId};

/// Idle-ship floor a rear sub-structure keeps before MASS (rule 4) feeds surplus forward.
///
/// This only *slows* the rearward drain; it is **not** a dedicated rear guard (rules 1 and 3
/// will draw from the home anyway when a fight is live), which is precisely the documented
/// seam — see the module docs and rule 4. Small by design.
const HOME_FLOOR: usize = 3;

/// The Layer-1 Automaton policy for one seat.
///
/// Construct with [`Automaton::new`], then call [`Automaton::decide`] each decision tick to
/// get the orders to issue. It holds only its seat and a couple of tunable thresholds — no
/// per-tick mutable state — so a clone behaves identically and re-running is deterministic.
#[derive(Debug, Clone)]
pub struct Automaton {
    /// Which seat this instance controls.
    pub seat: Faction,
    /// Local-superiority margin required to launch an ASSAULT (rule 2). E.g. `1.3` means
    /// "attack only if my committed force would be >= 1.3x the defenders". A clean tunable;
    /// higher = more cautious. Neutral (undefended) targets always pass.
    pub assault_margin: f32,
    /// If my contested sub-structure's local force ratio (mine / theirs) is below this, rule
    /// 1 reinforces it. `1.0` = reinforce only when actually outnumbered.
    pub reinforce_below: f32,
}

impl Automaton {
    /// A new Automaton for `seat` with the default thresholds (the operating point used by
    /// the headless runner and tests).
    pub fn new(seat: Faction) -> Automaton {
        Automaton { seat, assault_margin: 1.25, reinforce_below: 1.0 }
    }

    /// Decide this tick's orders from the current structure state. Returns a (possibly
    /// empty) list of [`MoveOrder`]s for the caller to feed to [`Interior::issue_order`].
    ///
    /// Deterministic: a pure function of `(st, params, self)`. Issues at most a handful of
    /// orders per call (it commits gradually).
    pub fn decide(&self, st: &Interior, params: &SimParams) -> Vec<MoveOrder> {
        let me = self.seat;
        let enemy = me.opponent();
        let mut orders: Vec<MoveOrder> = Vec::new();

        let my_subs: Vec<SubId> =
            (0..st.subs.len()).filter(|&s| st.subs[s].owner == me).collect();
        if my_subs.is_empty() {
            return orders; // eliminated from the board; nothing to command
        }

        // --- Rule 1: REINFORCE a losing/contested owned sub-structure. ----------------
        // A sub is "contested" if an enemy ship is within engagement range of its centre.
        // We reinforce the WORST (lowest local force ratio) contested sub from the nearest
        // safe sub that has idle ships. (SEAM: "nearest safe sub" includes the home, so the
        // home gets stripped to feed the front.)
        let mut worst: Option<(SubId, f32)> = None;
        for &s in &my_subs {
            if !self.is_contested(st, s, params) {
                continue;
            }
            let ratio = self.local_ratio(st, s, params);
            if ratio < self.reinforce_below {
                match worst {
                    Some((_, wr)) if wr <= ratio => {}
                    _ => worst = Some((s, ratio)),
                }
            }
        }
        if let Some((target, _)) = worst {
            if let Some(src) = self.nearest_safe_source_with_idle(st, target, params) {
                // Commit hard to a fight we're losing: send everything idle from the source.
                orders.push(MoveOrder::new(src, target, FractionBucket::All));
                // One reinforcement order per tick is enough; gradual commitment.
                return orders;
            }
        }

        // --- Rule 2: EXPAND — claim an UNCONTESTED neutral with a cheap scout wave. -----
        // Before committing to fights, grow the economy (the colonize instinct of the
        // triad). Only *uncontested* neutrals (no enemy near) and only a Quarter of a
        // source's idle ships, so expansion never empties a garrison. Nearest neutral to
        // any owned sub wins. This is what makes the opening a land-grab and gives the
        // square-law snowball something to compound.
        let mut best_expand: Option<(SubId, SubId, f32)> = None; // (src, tgt, dist)
        for tgt in 0..st.subs.len() {
            if st.subs[tgt].owner != Faction::Neutral {
                continue;
            }
            if self.defenders_of(st, tgt, enemy, params) > 0 {
                continue; // contested neutral is an ASSAULT target, not a free grab
            }
            for &src in &my_subs {
                if st.idle_count_at(src, me) < 1 {
                    continue;
                }
                let d = st.subs[src].pos.dist(st.subs[tgt].pos);
                match best_expand {
                    Some((_, _, bd)) if bd <= d => {}
                    _ => best_expand = Some((src, tgt, d)),
                }
            }
        }
        if let Some((src, tgt, _)) = best_expand {
            orders.push(MoveOrder::new(src, tgt, FractionBucket::Quarter));
            return orders;
        }

        // --- Rule 3: ASSAULT an enemy/contested sub-structure on local superiority. -----
        // For each of my subs with idle ships, consider every enemy-held or contested sub;
        // if my idle stack would outnumber its defenders by `assault_margin`, attack it.
        // Pick the globally nearest such (source, target) pair (SEAM: nearest-first — the
        // Automaton can be baited toward a cheap fight and sniped elsewhere).
        let mut best_assault: Option<(SubId, SubId, f32)> = None; // (src, tgt, dist)
        for &src in &my_subs {
            let idle = st.idle_count_at(src, me) as f32;
            if idle < 1.0 {
                continue;
            }
            for tgt in 0..st.subs.len() {
                if st.subs[tgt].owner == me {
                    continue;
                }
                let defenders = self.defenders_of(st, tgt, enemy, params) as f32;
                if defenders <= 0.0 {
                    continue; // undefended neutrals are handled by EXPAND
                }
                if idle < defenders * self.assault_margin {
                    continue;
                }
                let d = st.subs[src].pos.dist(st.subs[tgt].pos);
                match best_assault {
                    Some((_, _, bd)) if bd <= d => {}
                    _ => best_assault = Some((src, tgt, d)),
                }
            }
        }
        if let Some((src, tgt, _)) = best_assault {
            // Commit the bulk of the source stack to the strike.
            orders.push(MoveOrder::new(src, tgt, FractionBucket::ThreeQuarter));
            return orders;
        }

        // --- Rule 4: MASS surplus at the most-forward owned sub-structure. -------------
        // Pull *surplus* idle ships (above a small home floor) from rear subs toward the
        // owned sub closest to the enemy, building the next assault stack.
        //
        // THE SEAM lives here: the floor is small and is itself fed forward whenever a
        // fight is live (rules 1/3 draw from the nearest source, home included), so under
        // sustained pressure the rear is bled to the front and never gets a dedicated
        // guard. The floor only slows the drain — it does not post a real rear defence — so
        // a flanking detachment still finds the rear thinly held.
        if let Some(front) = self.most_forward_sub(st, me, enemy) {
            for &src in &my_subs {
                if src == front {
                    continue;
                }
                let idle = st.idle_count_at(src, me);
                if idle > HOME_FLOOR {
                    // Send a quarter of the surplus stack, so massing is steady.
                    orders.push(MoveOrder::new(src, front, FractionBucket::Quarter));
                }
            }
        }

        orders
    }

    // ----------------------------------------------------------------------
    // Helpers (all pure reads of the structure)
    // ----------------------------------------------------------------------

    /// True if an enemy ship is within engagement range of `sub`'s centre (the sub is in or
    /// next to a fight).
    fn is_contested(&self, st: &Interior, sub: SubId, params: &SimParams) -> bool {
        let enemy = self.seat.opponent();
        let c = st.subs[sub].pos;
        let reach = st.subs[sub].radius + params.engagement_radius;
        let reach2 = reach * reach;
        st.ships
            .iter()
            .any(|s| s.alive && s.faction == enemy && s.pos.dist_sq(c) <= reach2)
    }

    /// Local force ratio at `sub`: (my ships near it) / (enemy ships near it). Counts ships
    /// within `radius + engagement_radius` of the centre. Returns a large number if no enemy
    /// is near (not outnumbered).
    fn local_ratio(&self, st: &Interior, sub: SubId, params: &SimParams) -> f32 {
        let me = self.seat;
        let enemy = me.opponent();
        let c = st.subs[sub].pos;
        let reach = st.subs[sub].radius + params.engagement_radius;
        let reach2 = reach * reach;
        let mut mine = 0.0f32;
        let mut theirs = 0.0f32;
        for s in &st.ships {
            if !s.alive || s.pos.dist_sq(c) > reach2 {
                continue;
            }
            if s.faction == me {
                mine += 1.0;
            } else if s.faction == enemy {
                theirs += 1.0;
            }
        }
        if theirs <= 0.0 {
            f32::INFINITY
        } else {
            mine / theirs
        }
    }

    /// Count of enemy `defenders` defending `tgt`: living `def_faction` ships within
    /// `radius + engagement_radius` of the sub centre (so a stack one hop away that can fire
    /// across counts as defending).
    fn defenders_of(&self, st: &Interior, tgt: SubId, def_faction: Faction, params: &SimParams) -> usize {
        let c = st.subs[tgt].pos;
        let reach = st.subs[tgt].radius + params.engagement_radius;
        let reach2 = reach * reach;
        st.ships
            .iter()
            .filter(|s| s.alive && s.faction == def_faction && s.pos.dist_sq(c) <= reach2)
            .count()
    }

    /// The owned sub-structure with idle ships that is nearest to `target` and is itself NOT
    /// contested (a "safe" source to pull a reserve from). Falls back to the nearest owned
    /// sub with idle ships if every owned sub is contested.
    fn nearest_safe_source_with_idle(&self, st: &Interior, target: SubId, params: &SimParams) -> Option<SubId> {
        let me = self.seat;
        let tc = st.subs[target].pos;
        let mut best: Option<(SubId, f32)> = None;
        let mut fallback: Option<(SubId, f32)> = None;
        for s in 0..st.subs.len() {
            if s == target || st.subs[s].owner != me {
                continue;
            }
            if st.idle_count_at(s, me) == 0 {
                continue;
            }
            let d = st.subs[s].pos.dist(tc);
            // fallback ignores safety
            match fallback {
                Some((_, fd)) if fd <= d => {}
                _ => fallback = Some((s, d)),
            }
            if self.is_contested(st, s, params) {
                continue; // not "safe"
            }
            match best {
                Some((_, bd)) if bd <= d => {}
                _ => best = Some((s, d)),
            }
        }
        best.map(|(s, _)| s).or(fallback.map(|(s, _)| s))
    }

    /// My owned sub-structure closest to *any* enemy-owned sub-structure (my "front"). If the
    /// enemy owns nothing, falls back to my sub closest to the structure's centroid so I
    /// still mass toward the contested middle.
    fn most_forward_sub(&self, st: &Interior, me: Faction, enemy: Faction) -> Option<SubId> {
        let enemy_subs: Vec<SubId> =
            (0..st.subs.len()).filter(|&s| st.subs[s].owner == enemy).collect();
        let mut best: Option<(SubId, f32)> = None;
        if enemy_subs.is_empty() {
            // Centroid of all subs as a neutral rallying direction.
            let n = st.subs.len().max(1) as f32;
            let cx = st.subs.iter().map(|s| s.pos.x).sum::<f32>() / n;
            let cy = st.subs.iter().map(|s| s.pos.y).sum::<f32>() / n;
            let c = crate::types::Vec2::new(cx, cy);
            for s in 0..st.subs.len() {
                if st.subs[s].owner != me {
                    continue;
                }
                let d = st.subs[s].pos.dist(c);
                match best {
                    Some((_, bd)) if bd <= d => {}
                    _ => best = Some((s, d)),
                }
            }
            return best.map(|(s, _)| s);
        }
        for s in 0..st.subs.len() {
            if st.subs[s].owner != me {
                continue;
            }
            let nearest_enemy = enemy_subs
                .iter()
                .map(|&e| st.subs[s].pos.dist(st.subs[e].pos))
                .fold(f32::INFINITY, f32::min);
            match best {
                Some((_, bd)) if bd <= nearest_enemy => {}
                _ => best = Some((s, nearest_enemy)),
            }
        }
        best.map(|(s, _)| s)
    }
}

/// Apply an Automaton's decisions for one decision tick directly to the structure (a
/// convenience that issues every order the policy returns). Returns the number of ships
/// ordered this tick. The headless runner uses this for both seats.
pub fn drive(auto: &Automaton, st: &mut Interior, params: &SimParams) -> usize {
    let orders = auto.decide(st, params);
    let mut moved = 0;
    for o in orders {
        moved += st.issue_order(o, auto.seat);
    }
    moved
}
