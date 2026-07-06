//! The **Cycler** — the custom scripted enemy of the "Command and Control" mission
//! ([`crate::Roster::Cycler`]), owner-designed (2026-07-06). A readable, telegraphed opponent
//! the player learns to out-command: it drills, it reacts, and it strikes only with crushing
//! force — every behaviour is visible on the board before it matters.
//!
//! Per decision tick, per struct (it plays each struct's interior independently and never
//! launches inter-struct fleets):
//!
//! 1. **Committed sieges** (owner update): its units standing on ground it does not own fight
//!    their own war, outside the home economy. At each such sub it compares its force there
//!    (present + inbound) against the enemy's (present + inbound): if it **outnumbers** the
//!    enemy it holds — the capture continues, and those units are **excluded** from the
//!    cycling drill, the gather pool, and the defence pull; if the enemy outnumbers it, the
//!    landed units **retreat to the nearest owned sub** and rejoin the loop. (Ties hold.)
//!    The same rule governs the endgame reserve assault (see 5) — landed reserve attackers
//!    are committed, not re-gathered.
//! 2. **Defence** (preempts strikes): if any of its subs is *attacked* — foe ships idle on it
//!    or in transit toward it — it masses its **pool** (everything not committed to a siege)
//!    onto the **most-attacked** sub (ties → lowest sub id) and aborts any strike being
//!    assembled. In-flight ships cannot be redirected (engine rule); they complete their hop
//!    and are re-ordered on a later decision.
//! 3. **Overwhelm strike**: in peace, it compares its **pool** `M` against each foe-owned
//!    sub's defenders `F` = that sub's foe ships present + in transit toward it. If
//!    `M ≥ max(3·F, F + 60)` for some target it **gathers** the pool at its biggest sub — the
//!    visible tell — and, once ≥ [`GATHER_LAUNCH_FRAC`] of the pool sits there (production
//!    keeps minting stragglers, so literal 100 % never arrives), launches the whole stack at
//!    a qualifying target picked pseudo-randomly (a pure hash of tick × seat × count — the
//!    crate draws no RNG; replays are bit-identical). If nothing still qualifies at launch
//!    time (the player reinforced during the tell), it stands down.
//! 4. **Cycling** (the idle drill): otherwise every owned sub's idle ships **above its
//!    storage capacity** are shuttled to the next owned sub (cyclic by sub id; a lone sub
//!    does nothing). In-transit ships dodge the per-sub idle attrition, so the rotating
//!    column grows well past a parked garrison's balance point — the mission's built-in
//!    clock (owner: intended).
//! 5. **The reserve blind spot** (owner: intended): foe ships staged in — or flying to — the
//!    ownerless struct-storage node are invisible to it: they count neither as an attack,
//!    nor as defenders `F`, nor as a reason to hold back. A player who stages in the reserve
//!    can bait the overwhelm strike into an ambush — that is the lesson. Only once **no
//!    foe-owned sub remains** does it turn on the reserve garrison (same overwhelm
//!    arithmetic against the remnant), so a conquered board still resolves instead of
//!    stalemating.

use layer1::{Faction, FractionBucket, MoveOrder, SimParams, Vec2};
use world::{World, WorldParams};

/// Launch threshold of the gather step: the strike departs once this fraction of the seat's
/// **pool** (total minus committed besiegers) sits idle at the gather sub. Production keeps
/// minting stragglers at the other subs, so waiting for literally *all* ships would chase its
/// own tail forever.
pub const GATHER_LAUNCH_FRAC: f32 = 0.9;

/// The Cycler's overwhelm threshold: strike only when `m ≥ max(3·F, F + 60)` (owner formula —
/// it attacks only with crushing force, so its one attack per cycle is a real event).
#[inline]
pub fn overwhelms(m: usize, f: usize) -> bool {
    m >= (3 * f).max(f + 60)
}

/// The stateful Cycler driver (see the module doc). One per enemy seat, stepped with `&mut`
/// like [`crate::SimpleController`]; the only state is the strike being assembled.
#[derive(Debug, Clone)]
pub struct CyclerController {
    /// The seat this controller plays.
    pub seat: Faction,
    /// Per-struct massing state: `Some(gather_sub)` while a strike is being assembled there.
    massing: Vec<Option<usize>>,
}

impl CyclerController {
    pub fn new(seat: Faction) -> CyclerController {
        CyclerController { seat, massing: Vec::new() }
    }

    /// Decide and apply this seat's turn. Returns `(ships moved internally, fleets launched)`
    /// — the Cycler never launches inter-struct fleets, so the second count is always 0.
    pub fn decide_and_apply(
        &mut self,
        world: &mut World,
        _sim: &SimParams,
        _wp: &WorldParams,
    ) -> (usize, usize) {
        if self.massing.len() < world.structs.len() {
            self.massing.resize(world.structs.len(), None);
        }
        let tick = world.tick;
        let mut moved = 0usize;
        for sid in 0..world.structs.len() {
            moved += self.play_struct(world, sid, tick);
        }
        (moved, 0)
    }

    /// One struct's interior turn (the module-doc state machine). Returns ships ordered.
    fn play_struct(&mut self, world: &mut World, sid: usize, tick: u64) -> usize {
        let me = self.seat;

        // --- One pass of tallies, copied out so the orders below can borrow mutably. -------
        // Ships "at" a sub: idle homed there, or in transit toward it (undock included).
        let (n, my_idle_at, my_inbound, my_total, foe_at, owners, caps, positions, storage) = {
            let st = &world.structs[sid].interior;
            let n = st.subs.len();
            let mut my_idle_at = vec![0usize; n];
            let mut my_inbound = vec![0usize; n];
            let mut my_total = 0usize;
            let mut foe_at = vec![0usize; n]; // foes of ME (pressure on mine / defenders of theirs)
            for sh in &st.ships {
                if !sh.alive || sh.drift_remaining > 0 {
                    continue;
                }
                if sh.faction == me {
                    my_total += 1;
                    match sh.target {
                        None if sh.home < n => my_idle_at[sh.home] += 1,
                        Some(t) if t < n => my_inbound[t] += 1,
                        _ => {}
                    }
                } else if sh.faction.is_foe_of(me) {
                    let at = sh.target.unwrap_or(sh.home);
                    if at < n {
                        foe_at[at] += 1;
                    }
                }
            }
            let owners: Vec<Faction> = st.subs.iter().map(|s| s.owner).collect();
            let caps: Vec<usize> = st.subs.iter().map(|s| s.storage_capacity as usize).collect();
            let positions: Vec<Vec2> = st.subs.iter().map(|s| s.pos).collect();
            (n, my_idle_at, my_inbound, my_total, foe_at, owners, caps, positions, st.storage_sub)
        };
        let mine: Vec<usize> = (0..n).filter(|&s| owners[s] == me).collect();
        if mine.is_empty() || my_total == 0 {
            self.massing[sid] = None;
            return 0;
        }
        let is_storage = |s: usize| storage == Some(s);
        let foe_subs: Vec<usize> =
            (0..n).filter(|&t| owners[t].is_foe_of(me) && !is_storage(t)).collect();

        // --- (1) COMMITTED SIEGES: units on ground we don't own fight their own war. -------
        // Outnumber the enemy there (both sides present + inbound) ⇒ hold, excluded from the
        // pool; outnumbered ⇒ the landed units retreat to the nearest owned sub (in-flight
        // ones re-evaluate after landing). The reserve counts only during the endgame remnant
        // assault — otherwise our overflow staged there stays in the pool.
        let mut moved = 0usize;
        let mut committed = vec![false; n];
        let mut committed_ships = 0usize;
        for t in 0..n {
            if owners[t] == me {
                continue;
            }
            if is_storage(t) && !(foe_subs.is_empty() && foe_at[t] > 0) {
                continue;
            }
            let here = my_idle_at[t] + my_inbound[t];
            if here == 0 {
                continue;
            }
            if foe_at[t] > here {
                // Outnumbered: retreat to the nearest owned sub, rejoining the loop.
                if my_idle_at[t] > 0 {
                    let home = mine
                        .iter()
                        .copied()
                        .min_by(|&a, &b| {
                            positions[a]
                                .dist(positions[t])
                                .partial_cmp(&positions[b].dist(positions[t]))
                                .unwrap_or(std::cmp::Ordering::Equal)
                                .then(a.cmp(&b))
                        })
                        .expect("mine is non-empty");
                    moved += world.structs[sid]
                        .interior
                        .issue_order(MoveOrder::new(t, home, FractionBucket::All), me);
                }
            } else {
                committed[t] = true;
                committed_ships += here;
            }
        }
        let pool = my_total.saturating_sub(committed_ships);

        // --- (2) DEFENCE preempts strikes: mass the pool on the most-attacked sub. ---------
        // (The reserve blind spot: `foe_at[storage]` is never an attack on us.)
        let defend = mine
            .iter()
            .copied()
            .filter(|&s| foe_at[s] > 0)
            .max_by_key(|&s| (foe_at[s], std::cmp::Reverse(s)));
        if let Some(target) = defend {
            self.massing[sid] = None;
            return moved + self.send_pool_toward(world, sid, target, &my_idle_at, &committed);
        }

        // --- Target set + overwhelm test (the pool does the striking). ----------------------
        // Foe-owned subs; once none remain, the reserve remnant becomes the last target.
        let targets: Vec<usize> = if !foe_subs.is_empty() {
            foe_subs
        } else {
            match storage {
                Some(stg) if foe_at[stg] > 0 && !committed[stg] => vec![stg],
                _ => Vec::new(),
            }
        };
        let qualifying: Vec<usize> =
            targets.iter().copied().filter(|&t| overwhelms(pool, foe_at[t])).collect();

        // --- (3) The gathered strike. -------------------------------------------------------
        if let Some(gather) = self.massing[sid] {
            if owners[gather] != me {
                // Lost the mustering ground mid-gather: stand down (re-evaluated next tick).
                self.massing[sid] = None;
                return moved;
            }
            if (my_idle_at[gather] as f32) >= GATHER_LAUNCH_FRAC * pool as f32 {
                self.massing[sid] = None;
                if qualifying.is_empty() {
                    return moved; // the tell was answered — stand down
                }
                let pick = qualifying[(mix(tick, me, pool) as usize) % qualifying.len()];
                return moved
                    + world.structs[sid]
                        .interior
                        .issue_order(MoveOrder::new(gather, pick, FractionBucket::All), me);
            }
            // Keep pulling stragglers in.
            return moved + self.send_pool_toward(world, sid, gather, &my_idle_at, &committed);
        }
        if !qualifying.is_empty() {
            // Enter the gather: muster at the sub already holding the most ships (ties →
            // lowest id — max_by_key keeps the max with the smallest id via Reverse).
            let gather = mine
                .iter()
                .copied()
                .max_by_key(|&s| (my_idle_at[s], std::cmp::Reverse(s)))
                .expect("mine is non-empty");
            self.massing[sid] = Some(gather);
            return moved + self.send_pool_toward(world, sid, gather, &my_idle_at, &committed);
        }

        // --- (4) The idle drill: cycle each sub's over-capacity surplus to the next own sub.
        if mine.len() < 2 {
            return moved;
        }
        for (i, &s) in mine.iter().enumerate() {
            let cap = caps[s];
            let idle = my_idle_at[s];
            if idle > cap {
                let next = mine[(i + 1) % mine.len()];
                moved += world.structs[sid].interior.issue_order_count(s, next, idle - cap, me);
            }
        }
        moved
    }

    /// Order the **pool's** idle ships everywhere (any sub, the reserve included — but never
    /// a committed siege's) onto `target`.
    fn send_pool_toward(
        &self,
        world: &mut World,
        sid: usize,
        target: usize,
        my_idle_at: &[usize],
        committed: &[bool],
    ) -> usize {
        let mut moved = 0;
        for s in 0..my_idle_at.len() {
            if s != target && my_idle_at[s] > 0 && !committed[s] {
                moved += world.structs[sid]
                    .interior
                    .issue_order(MoveOrder::new(s, target, FractionBucket::All), self.seat);
            }
        }
        moved
    }
}

/// A pure splitmix64-style hash of (tick, seat, count) — the strike target's "random" pick
/// without drawing any RNG (the crate's determinism rule: policies are pure functions of the
/// observed state).
fn mix(tick: u64, seat: Faction, count: usize) -> u64 {
    let seat_byte = match seat {
        Faction::Player => 1u64,
        Faction::Ai(i) => 2 + i as u64,
        _ => 0,
    };
    let mut x = tick ^ (seat_byte << 56) ^ ((count as u64) << 24);
    x ^= x >> 33;
    x = x.wrapping_mul(0xff51_afd7_ed55_8ccd);
    x ^= x >> 33;
    x = x.wrapping_mul(0xc4ce_b9fe_1a85_ec53);
    x ^ (x >> 33)
}
