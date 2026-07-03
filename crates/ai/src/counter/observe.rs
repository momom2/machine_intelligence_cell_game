//! # `counter::observe` — the observation log (COUNTER_DESIGN §3)
//!
//! Records every decision a **watched** seat makes during a match as a *(situation-bucket →
//! choice)* sample, **in the game's own legible vocabulary** — so inference (`counter::profile`)
//! runs over the exact same primitives the automatons and the player branch on, never raw node
//! ids.
//!
//! The two abstractions this module owns:
//!
//! * **[`SituationBucket`]** — a small discrete key `ownership × garrison-band × neighbor-threat ×
//!   frontier`, computed from the legible [`crate::adapters::Layer2View`] features (the same
//!   property signals the automatons read). Coarse enough that counts accrue per situation-type;
//!   fine enough that the profile is exploitable (COUNTER_DESIGN §3's "information set" analog).
//! * **[`MoveKind`]** — the *kind* of move, abstracted away from ids:
//!   [`MoveKind::Colonize`] a neutral · [`MoveKind::Reinforce`] a threatened own node ·
//!   [`MoveKind::Strike`] an enemy node · [`MoveKind::Hold`] (decline to move — itself a tell, so
//!   we log it too) — plus the committed [`FractionBand`] and the (categorical) source/target
//!   roles.
//!
//! The unit of accumulation is the **per-source decision**: for one decision tick we look at the
//! pre-decision world the watched seat saw, and for **each owned struct that *could* export**
//! (`fully_owned_uncontested`, the world's own export precondition) we record either the
//! [`MoveKind`] of the order issued from it, or [`MoveKind::Hold`] if it issued none. That keeps
//! "a turtle defends by *not* over-extending" a first-class observation rather than an absence.
//!
//! ## The hook
//!
//! [`Observer`] is the harness/controller-facing hook. Drive a match as usual; each planning tick,
//! **before** the watched seat applies its orders, call [`Observer::observe_decision`] with the
//! pre-decision `&World` and the seat's chosen [`world::FleetOrder`]s. The observer rebuilds the
//! *same* [`crate::adapters::Layer2View`] the policy saw (so it reads identical features) and folds
//! the decision into its [`ObservationLog`]. Everything is a deterministic pure read — no RNG, no
//! mutation of the world — so a given `(seed, policies, horizon)` yields the same log bit-for-bit.

use layer1::{Faction, SimParams};
use world::{FleetOrder, StructId, StructOwner, World, WorldParams};

use crate::adapters::Layer2View;
use crate::greedy::{PosOwner, PositionView, Side};

// =====================================================================================
// The CHOICE — an abstracted move KIND + fraction band + roles.
// =====================================================================================

/// The **kind** of move a watched seat made, abstracted away from raw node ids (COUNTER_DESIGN
/// §3). This is the categorical the strategic-mix and module inference are computed over.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MoveKind {
    /// **Colonize** — surplus sent toward a *neutral* (foe-free, capturable) destination. The
    /// colonizer's expand signal.
    Colonize,
    /// **Reinforce** — surplus sent toward a *threatened own* node (an owned destination that is
    /// contested or the projection says is falling). The defender's hold-my-ground signal.
    Reinforce,
    /// **Strike** — surplus sent toward an *enemy-bearing* node (enemy-owned, or contested with
    /// the foe present). The attacker's press signal.
    Strike,
    /// **Hold** — declined to move a source that *could* have exported (inaction). A turtle
    /// defends by not over-extending, so this is recorded, not dropped.
    Hold,
}

impl MoveKind {
    /// A short human-readable name (diagnostics / reports).
    pub fn name(self) -> &'static str {
        match self {
            MoveKind::Colonize => "Colonize",
            MoveKind::Reinforce => "Reinforce",
            MoveKind::Strike => "Strike",
            MoveKind::Hold => "Hold",
        }
    }

    /// The three **active** kinds (everything but [`MoveKind::Hold`]), in a stable order. The
    /// strategic mix is the proportion among these.
    pub const ACTIVE: [MoveKind; 3] = [MoveKind::Colonize, MoveKind::Reinforce, MoveKind::Strike];
}

/// The committed-fraction band of a move (a coarsening of [`layer1::FractionBucket`] kept as part
/// of the choice). `None`-equivalent for a [`MoveKind::Hold`] is [`FractionBand::None`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FractionBand {
    /// Nothing committed (a [`MoveKind::Hold`]).
    None,
    /// A small commitment — a quarter of the source's exportable surplus.
    Light,
    /// Half (a cautious half-commitment — the turtle's signature).
    Half,
    /// Three-quarters or all (a heavy, over-committing wave — the attacker's signature).
    Heavy,
}

impl FractionBand {
    /// Coarsen a [`layer1::FractionBucket`] into a band.
    pub fn from_bucket(b: layer1::FractionBucket) -> FractionBand {
        match b {
            layer1::FractionBucket::Quarter => FractionBand::Light,
            layer1::FractionBucket::Half => FractionBand::Half,
            layer1::FractionBucket::ThreeQuarter | layer1::FractionBucket::All => FractionBand::Heavy,
        }
    }
}

/// One observed decision sample: the abstracted move [`MoveKind`], the committed [`FractionBand`],
/// and the categorical source-role context. (Target roles are folded into the [`MoveKind`] itself —
/// "colonize a neutral" *is* the target role — so a sample stays small and id-free.)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Choice {
    /// The abstracted move kind.
    pub kind: MoveKind,
    /// How much of the source's exportable surplus was committed.
    pub band: FractionBand,
}

// =====================================================================================
// The SITUATION BUCKET — a small discrete key over the legible features.
// =====================================================================================

/// Ownership share band of the acting seat: how much of the producing board it holds relative to
/// the foe. The coarse "am I ahead / behind on territory" axis of the bucket.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum OwnershipBand {
    /// The seat owns strictly fewer producers than the foe (behind on territory).
    Behind,
    /// Roughly even producer counts (within one).
    Even,
    /// The seat owns strictly more producers than the foe (ahead on territory).
    Ahead,
}

/// The acting *source* node's idle-stack band relative to its soft cap (the anti-hoard pressure
/// signal). Discriminates "sitting on a fat hoard" from "lean and spending".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum GarrisonBand {
    /// Idle stack well below the soft cap (lean).
    Low,
    /// Idle stack approaching the soft cap.
    Mid,
    /// Idle stack at/over the soft cap (a hoard the cap is about to bleed / is bleeding).
    Over,
}

/// Whether **any** node the seat owns is currently under threat (contested or projected to fall).
/// The "is something of mine in danger right now" axis a defender keys on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ThreatBand {
    /// Nothing of mine is threatened.
    Calm,
    /// At least one owned node is contested or projected to fall.
    Threatened,
}

/// Whether the acting *source* node is a frontier node (a reachable foe-bearing node exists). The
/// "is this source exposed to the enemy" axis — the seam (thin-rear) module reads it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FrontierBand {
    /// The source has no reachable foe-bearing node (a rear/interior node).
    Rear,
    /// The source can reach a foe-bearing node (a front node).
    Front,
}

/// The discrete **situation key** for one source decision (COUNTER_DESIGN §3): `ownership ×
/// garrison-band × neighbor-threat × frontier`. Small enough that observations accumulate per
/// situation-type (`Hash + Eq` so it keys the per-bucket tallies), coarse enough to stay
/// exploitable. This is the game's analog of an "information set".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SituationBucket {
    /// Board-wide ownership share of the acting seat.
    pub ownership: OwnershipBand,
    /// The acting source node's idle-stack pressure vs its soft cap.
    pub garrison: GarrisonBand,
    /// Whether anything the seat owns is threatened right now.
    pub threat: ThreatBand,
    /// Whether the acting source node is a frontier (exposed) node.
    pub frontier: FrontierBand,
}

// =====================================================================================
// The OBSERVATION LOG — accumulating (bucket -> choice) samples.
// =====================================================================================

/// One observed `(SituationBucket → Choice)` pair, the atomic unit the log accumulates and the
/// profile reduces. Kept as a flat record (rather than a pre-aggregated map) so the profile can
/// apply recency weighting / decay over the raw stream if asked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Sample {
    /// The decision tick this sample was observed at (absolute world tick) — the recency key.
    pub tick: u64,
    /// The situation the watched seat faced from this source.
    pub bucket: SituationBucket,
    /// The choice it made (move kind + committed band).
    pub choice: Choice,
}

/// The rolling **observation log** for one watched faction across a match. A flat, append-only
/// list of [`Sample`]s, each a `(situation-bucket → choice)` pair in the legible vocabulary. The
/// profile (`counter::profile`) is a deterministic function of this log.
#[derive(Debug, Clone, Default)]
pub struct ObservationLog {
    /// The watched faction (set on first observation; `None` until then).
    watched: Option<Faction>,
    /// Every per-source decision sample, in observation (tick-ascending) order.
    pub samples: Vec<Sample>,
}

impl ObservationLog {
    /// A fresh, empty log.
    pub fn new() -> ObservationLog {
        ObservationLog { watched: None, samples: Vec::new() }
    }

    /// The faction this log is watching, if any sample has been recorded.
    pub fn watched(&self) -> Option<Faction> {
        self.watched
    }

    /// Total samples recorded (a quick activity gauge).
    pub fn len(&self) -> usize {
        self.samples.len()
    }

    /// True when nothing has been observed yet.
    pub fn is_empty(&self) -> bool {
        self.samples.is_empty()
    }

    /// Append one sample (used by [`Observer`] and tests).
    pub fn push(&mut self, watched: Faction, sample: Sample) {
        self.watched.get_or_insert(watched);
        self.samples.push(sample);
    }
}

// =====================================================================================
// The OBSERVER — the harness/controller hook.
// =====================================================================================

/// The observation **hook** for the AI-vs-AI harness (COUNTER_DESIGN §9 `counter::observe`).
///
/// Watches one `seat`. Each planning tick, the host calls [`Observer::observe_decision`] with the
/// **pre-decision** world the seat saw and the [`FleetOrder`]s it chose; the observer rebuilds the
/// same [`Layer2View`] (identical features), buckets every owned-exportable source, classifies the
/// issued order (or records [`MoveKind::Hold`]), and folds the samples into [`Observer::log`].
///
/// Stateless apart from the growing log and the params; deterministic.
#[derive(Debug, Clone)]
pub struct Observer {
    /// The faction being profiled.
    pub seat: Faction,
    /// The accumulated observation log.
    pub log: ObservationLog,
    /// Sim params for the feature reads (soft cap, geometry) — the same the controller used.
    sp: SimParams,
    /// World params for the feature reads.
    wp: WorldParams,
}

impl Observer {
    /// A fresh observer watching `seat`, reading features under `sp`/`wp` (pass the same params the
    /// match runs under so the buckets match what the policy saw).
    pub fn new(seat: Faction, sp: SimParams, wp: WorldParams) -> Observer {
        Observer { seat, log: ObservationLog::new(), sp, wp }
    }

    /// Observe one decision: the watched seat faced `world` (its **pre-decision** snapshot) and
    /// chose `orders`. Buckets each owned-exportable source and records its [`MoveKind`] (or
    /// [`MoveKind::Hold`]). A pure read of `world` (it builds a fresh projection internally, exactly
    /// as [`crate::strategy::StrategicPolicy::decide`] does), so it never perturbs the sim.
    ///
    /// Decision semantics, matching the controller: a source struct may only export when
    /// [`world::StructAggregate::fully_owned_uncontested`] holds (the same gate
    /// [`crate::adapters::Layer2View::can_export_from`] applies), so those are exactly the sources a
    /// decision is recorded for. A source with no exportable surplus issues no order and is **not**
    /// a meaningful "hold" (it had nothing to commit), so it is skipped.
    pub fn observe_decision(&mut self, world: &World, orders: &[FleetOrder]) {
        let proj = world.project_forward(&self.sp, &self.wp, world::DEFAULT_PROJECTION_HORIZON);
        let view = Layer2View::with_projection(world, self.seat, &proj, &self.sp, &self.wp);
        let tick = world.tick;

        // Board-wide ownership band + threat band are the same for every source this tick.
        let ownership = ownership_band(&view);
        let threat = threat_band(&view);

        // First hop -> kind map: an order's destination may be several lanes away (the policy routed
        // it to the first hop), so we classify by the FIRST-HOP destination's role relative to the
        // seat, which is exactly the node the surplus actually moves onto this tick.
        for from in 0..view.len() {
            if view.info(from).owner != PosOwner::Me {
                continue;
            }
            // Only sources the world would let export are meaningful decisions (mirror the policy's
            // own export gate); a source that *cannot* export had no choice to make.
            if !view.can_export_from(from) {
                continue;
            }
            // Did the seat have surplus to commit at all? If not, "Hold" is not a real tell here.
            if !has_exportable_surplus(world, from, self.seat, &self.wp) {
                continue;
            }

            let garrison = garrison_band(&view, from);
            let frontier = frontier_band(world, &view, from, self.seat);
            let bucket = SituationBucket { ownership, garrison, threat, frontier };

            // The order (if any) this source issued. A source issues at most one fleet order per
            // decision in all the v1 policies; if several were issued we take the first (lowest in
            // the order list) deterministically — the dominant commitment from this source.
            let order = orders.iter().find(|o| o.from == from);
            let choice = match order {
                Some(o) => Choice {
                    kind: classify_destination(&view, world, self.seat, o.to),
                    band: FractionBand::from_bucket(o.fraction),
                },
                None => Choice { kind: MoveKind::Hold, band: FractionBand::None },
            };
            self.log.push(self.seat, Sample { tick, bucket, choice });
        }
    }

    /// Consume the observer, returning the accumulated [`ObservationLog`].
    pub fn into_log(self) -> ObservationLog {
        self.log
    }
}

// =====================================================================================
// Feature -> bucket helpers (pure reads of the Layer2View — the SAME signals the policy saw).
// =====================================================================================

/// Classify a destination node (by first-hop id) into the [`MoveKind`] the move represents, from
/// the acting seat's point of view: an enemy-bearing node ⇒ [`MoveKind::Strike`]; a threatened own
/// node (mine, contested or projected to fall) ⇒ [`MoveKind::Reinforce`]; anything else (a neutral,
/// or a safe own node being thickened) ⇒ [`MoveKind::Colonize`]. A pure read of the view.
fn classify_destination<V: PositionView>(view: &V, world: &World, seat: Faction, to: usize) -> MoveKind {
    if to >= world.structs.len() {
        return MoveKind::Colonize; // out-of-range: treat as a (degenerate) expand
    }
    // Classify by the destination's PRE-MOVE *ownership* (who OWNS producing subs there), NOT by
    // instantaneous foe ship-presence. The old test (`enemy_ships > 0 => Strike`) misread a
    // capturable neutral the foe merely *contests* with ships as an attack — so when the Counter
    // fought back, a pure colonizer's neutral-grabs read as strikes (the live-contact contamination,
    // COUNTER_RESULTS.md §5). Intent lives in ownership: enemy-OWNED ground (the foe owns subs there)
    // is a strike; capturable neutral ground is colonize even while the foe contests it with ships.
    let agg = world.struct_aggregate(to);
    let foe = seat.opponent();
    let subs_of = |f: Faction| match f {
        Faction::Player => agg.player_subs,
        Faction::Ai(_) => agg.enemy_subs, // parked Counter; binary Layer-2 aggregate (all rivals combined)
        Faction::Neutral => 0,
    };
    // STRIKE — the foe OWNS producing ground here AND no capturable neutral sub remains, so the move
    // can only be an attack on enemy ground. (A struct where the foe has a foothold but neutral subs
    // are still grabbable stays Colonize: a colonizer taking the neutral part of a contested structure
    // is expanding, not striking — that residual was the rest of the live-contact contamination.)
    if subs_of(foe) > 0 && agg.neutral_subs == 0 {
        return MoveKind::Strike;
    }
    // REINFORCE — my own producing node under threat (contested now, or projected to fall to the foe).
    if subs_of(seat) > 0 {
        let falling = view.projected_next_owner(to) == Some(Side::Foe);
        if matches!(agg.owner, StructOwner::Contested) || falling {
            return MoveKind::Reinforce;
        }
    }
    // COLONIZE — expansion onto capturable/neutral ground (even ground the foe's SHIPS contest), or
    // thickening a safe own node.
    MoveKind::Colonize
}

/// The board-wide ownership band of the acting seat: producer-count of mine vs the foe.
fn ownership_band<V: PositionView>(view: &V) -> OwnershipBand {
    let mut mine = 0i32;
    let mut foe = 0i32;
    for i in 0..view.len() {
        match view.info(i).owner {
            PosOwner::Me => mine += 1,
            PosOwner::Enemy => foe += 1,
            PosOwner::Neutral => {}
        }
    }
    match mine - foe {
        d if d <= -1 => OwnershipBand::Behind,
        0 => OwnershipBand::Even,
        _ => OwnershipBand::Ahead,
    }
}

/// The acting source node's idle-stack band relative to its soft cap (the hoard pressure signal).
/// Reads the same `parked_ratio` the anti-hoard predicate ([`crate::vocab::over_soft_cap`]) uses.
fn garrison_band<V: PositionView>(view: &V, from: usize) -> GarrisonBand {
    let r = view.parked_ratio(from);
    if r >= 0.8 {
        GarrisonBand::Over
    } else if r >= 0.4 {
        GarrisonBand::Mid
    } else {
        GarrisonBand::Low
    }
}

/// Whether anything the seat owns is threatened right now: any owned node contested or projected to
/// fall to the foe within the horizon.
fn threat_band<V: PositionView>(view: &V) -> ThreatBand {
    for i in 0..view.len() {
        let info = view.info(i);
        if info.owner == PosOwner::Me
            && (info.contested || view.projected_next_owner(i) == Some(Side::Foe))
        {
            return ThreatBand::Threatened;
        }
    }
    ThreatBand::Calm
}

/// Whether the acting source is a **front** node — *lane-adjacent* to a foe-bearing structure
/// (enemy-owned or contested-with-the-foe) — or a **rear** (interior) node otherwise.
///
/// This is the spatial "is this node on the front line" notion the thin-rear seam reads, and it is
/// **adjacency**-based on purpose: the vocab [`crate::vocab::is_frontier`] uses path-*reachability*,
/// which at Layer 2 is true for every node on a connected map (a path to the enemy always exists),
/// so it cannot distinguish an exposed front node from a protected rear one. Lane adjacency does:
/// a colonizer's stocked home one hop *behind* the contact line reads `Rear`, the contact node
/// `Front` — exactly the rear an exploiter flanks.
fn frontier_band<V: PositionView>(world: &World, _view: &V, from: usize, seat: Faction) -> FrontierBand {
    let enemy = seat.opponent();
    let adj_to_foe = world.neighbors(from).iter().any(|&j| {
        let agg = world.struct_aggregate(j);
        matches!(agg.owner, StructOwner::Owned(f) if f == enemy)
            || (matches!(agg.owner, StructOwner::Contested) && agg.ships_of(enemy) > 0)
    });
    if adj_to_foe {
        FrontierBand::Front
    } else {
        FrontierBand::Rear
    }
}

/// Does `seat` have idle ships above the world's `keep_floor` to export from struct `p`? Mirrors
/// the pool [`crate::adapters`]'s `exportable_idle` / `Interior::take_idle_ships_structwide` draw
/// from, so "Hold" is only recorded when there was genuinely something to commit.
fn has_exportable_surplus(world: &World, p: StructId, seat: Faction, wp: &WorldParams) -> bool {
    let Some(structure) = world.structs.get(p) else { return false };
    // Only a securely held struct exports (the world precondition); but even then it needs surplus.
    if !matches!(world.struct_aggregate(p).owner, StructOwner::Owned(f) if f == seat) {
        return false;
    }
    let st = &structure.interior;
    let floor = wp.keep_floor;
    let mut total = 0usize;
    for s in 0..st.subs.len() {
        if st.subs[s].owner != seat {
            continue;
        }
        let idle = st.idle_count_at(s, seat);
        if idle > floor {
            total += idle - floor;
        }
    }
    total > 0
}
