//! Core value types for the Layer-1 spatial sim.
//!
//! Layer 1 (`03-ui-layers.md`) is the *embodied/micro* view: a **single structure made of
//! several sub-structures**, with **discrete ships** that garrison at sub-structures, move
//! between them, and fight by *proximity*. These are the plain data types the simulation
//! ([`crate::sim`]) and the renderer operate on.

/// A 2D point/vector in the structure's local plane (units are arbitrary "metres").
///
/// Kept deliberately tiny (`f32`, `Copy`) and dependency-free so the renderer can use it
/// directly. Only the handful of vector ops the sim needs are provided.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Vec2 {
    pub x: f32,
    pub y: f32,
}

impl Vec2 {
    #[inline]
    pub fn new(x: f32, y: f32) -> Vec2 {
        Vec2 { x, y }
    }

    /// Squared Euclidean distance to `other`. Used for radius checks (avoids a `sqrt`).
    #[inline]
    pub fn dist_sq(self, other: Vec2) -> f32 {
        let dx = self.x - other.x;
        let dy = self.y - other.y;
        dx * dx + dy * dy
    }

    /// Euclidean distance to `other`.
    #[inline]
    pub fn dist(self, other: Vec2) -> f32 {
        self.dist_sq(other).sqrt()
    }
}

/// Which seat owns a sub-structure or ship.
///
/// `Neutral` is unclaimed ground (it produces nothing until captured; no ship is ever neutral). The
/// **real** seats are the **`Player`** (the human in the GUI) and an arbitrary number of indexed AI
/// opponents **`Ai(i)`** — the rivals a *level* declares, in order (`Level::enemies[i]` drives `Ai(i)`).
/// The engine is agnostic to how many `Ai` seats exist: combat, capture and presence treat **every
/// other real seat as a foe** (a free-for-all). **How many enemies a match has is a level-layer
/// decision, never a Layer-1 constant** — there is no hardcoded "second enemy" any more.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Faction {
    /// Unowned ground.
    Neutral,
    /// The first seat — the human in the GUI (and seat 0 of the headless runner).
    Player,
    /// An AI opponent, indexed from 0 in the order the level lists its `enemies` (`Ai(0)` is the
    /// first rival, `Ai(1)` the second, …). Any number may be in play.
    Ai(u8),
}

impl Faction {
    /// True for the real seats (the `Player` or any `Ai`) — i.e. not `Neutral`.
    #[inline]
    pub fn is_real(self) -> bool {
        matches!(self, Faction::Player | Faction::Ai(_))
    }

    /// True iff `self` and `other` are **different real seats** — the foe relation the sim uses
    /// (free-for-all: every real seat fights every other). The N-seat replacement for comparing
    /// against a single `opponent()`.
    #[inline]
    pub fn is_foe_of(self, other: Faction) -> bool {
        self.is_real() && other.is_real() && self != other
    }

    /// A coarse **binary** "primary rival" hint: the human's rival is `Ai(0)`; an AI's rival is the
    /// `Player`. This is *not* the full multi-seat foe relation (use [`Faction::is_foe_of`]) — it is
    /// kept only for the **parked** automata / Counter / forward-projection track, which still reasons
    /// in two seats. `Neutral` has no rival.
    #[inline]
    pub fn opponent(self) -> Faction {
        match self {
            Faction::Player => Faction::Ai(0),
            Faction::Ai(_) => Faction::Player,
            Faction::Neutral => Faction::Neutral,
        }
    }
}

/// Index of a sub-structure into [`crate::sim::Interior::subs`].
pub type SubId = usize;

/// Index of a ship into [`crate::sim::Interior::ships`].
///
/// NOTE: ships are stored in a stable `Vec` and only ever *marked* dead (never removed
/// mid-run), so a `ShipId` stays valid for the whole simulation — convenient for a
/// renderer that wants to track a unit across frames. Dead ships are compacted only by the
/// optional [`crate::sim::Interior::compact_dead`] helper.
pub type ShipId = usize;

/// The fraction buckets the design fixes for atomic actions: 25/50/75/100 %.
///
/// `01-mechanics.md`: "Atomic action: `(source, fraction-bucket ∈ {25,50,75,100}%,
/// target)`." The Layer-1 analog sends a fraction-bucket of the *idle ships garrisoned at
/// a sub-structure*. Both the AI and the GUI issue orders using these buckets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FractionBucket {
    Quarter,
    Half,
    ThreeQuarter,
    All,
}

impl FractionBucket {
    /// The multiplier in `[0,1]` this bucket represents.
    #[inline]
    pub fn as_f32(self) -> f32 {
        match self {
            FractionBucket::Quarter => 0.25,
            FractionBucket::Half => 0.50,
            FractionBucket::ThreeQuarter => 0.75,
            FractionBucket::All => 1.00,
        }
    }

    /// How many ships out of `available` this bucket selects, rounded to the nearest whole
    /// ship but always **at least 1** when `available >= 1` (so a 25% order on 1 idle ship
    /// still sends that ship — the atomic action is never a silent no-op on a non-empty
    /// garrison).
    #[inline]
    pub fn count_of(self, available: usize) -> usize {
        frac_count(available, self.as_f32())
    }
}

/// Ships selected by a raw send-fraction `f` (clamped to `(0,1]`) out of `available`, rounded to
/// the nearest whole ship but always **at least 1** when `available >= 1`. The continuous analog
/// of [`FractionBucket::count_of`] — the GUI's free 1–100% troop slider uses this so an arbitrary
/// slider position sends a sensible whole count, and the 25/50/75/100 % snap positions send
/// *exactly* what the matching bucket would (identical rounding). Deterministic.
#[inline]
pub fn frac_count(available: usize, f: f32) -> usize {
    if available == 0 {
        return 0;
    }
    let f = f.clamp(f32::MIN_POSITIVE, 1.0);
    let want = (available as f32 * f).round() as usize;
    want.clamp(1, available)
}

/// A move order: send a fraction-bucket of `source`'s **idle** ships to `target`.
///
/// This is the Layer-1 atomic action. It is intentionally the same shape as `cell-core`'s
/// `Command` (`source, fraction-bucket, target`) so the shared-vocabulary spine holds
/// across layers. Issued via [`crate::sim::Interior::issue_order`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MoveOrder {
    /// Sub-structure to draw idle ships from.
    pub source: SubId,
    /// Sub-structure to send them to.
    pub target: SubId,
    /// How many of the source's idle ships to send.
    pub fraction: FractionBucket,
}

impl MoveOrder {
    /// Convenience constructor.
    #[inline]
    pub fn new(source: SubId, target: SubId, fraction: FractionBucket) -> MoveOrder {
        MoveOrder { source, target, fraction }
    }
}
// =============================================================================================
// The order journal (replay branch, owner design 2026-07-10)
// =============================================================================================

/// One recorded order -- the replay journal's atom. Orders carry EXACT SHIP COUNTS (owner
/// rule): buckets and the GUI's percent slider resolve to a count in-game at issue time,
/// so a replay is immune to any future change in bucket/slider semantics, and the file
/// format needs no floats. `sid` is the world's struct index (0 for a standalone interior).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OrderRecord {
    /// Interior move: `count` idle ships of `faction` at struct `sid`'s sub `source`,
    /// nearest-to-target first, sent to `target`.
    Move { sid: usize, source: SubId, target: SubId, count: usize, faction: Faction },
    /// Inter-struct fleet: up to `count` ships of `faction` leave struct `from`'s reserve
    /// toward `to` -- PURE MOVEMENT (owner rule: engine primitives carry no floors or
    /// policy; the reserve rally and its home-guard floor live in the wrapper layer and
    /// appear in the journal as ordinary `Move` records).
    Fleet { from: usize, to: usize, count: usize, faction: Faction },
}

/// A journaled order with the tick it was issued at (it applies BEFORE that tick steps).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct JournalEntry {
    pub tick: u64,
    pub record: OrderRecord,
}

/// The shared order journal: ONE `Vec` handed to the world and every interior, so the
/// global within-tick issuance sequence is preserved by construction (a fleet launch
/// consumes idle ships a later interior order would otherwise draw -- split journals would
/// lose that interleaving). Single-threaded by design, like the rest of the sim.
pub type OrderJournal = std::rc::Rc<std::cell::RefCell<Vec<JournalEntry>>>;

