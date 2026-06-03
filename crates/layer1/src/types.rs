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
/// Two real seats (`Player`, `Enemy`) plus `Neutral` for as-yet-uncaptured sub-structures.
/// The two real seats are symmetric; the AI ([`crate::ai`]) can drive either one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Faction {
    /// The first seat (the human in the future GUI).
    Player,
    /// The second seat (the Automaton in the headless runner).
    Enemy,
    /// Unowned. A neutral sub-structure produces nothing until captured; no ships are
    /// ever neutral.
    Neutral,
}

impl Faction {
    /// The opposing *player* seat. `Neutral.opponent()` is meaningless and returns
    /// `Neutral` (callers never ask a neutral for its opponent in practice).
    #[inline]
    pub fn opponent(self) -> Faction {
        match self {
            Faction::Player => Faction::Enemy,
            Faction::Enemy => Faction::Player,
            Faction::Neutral => Faction::Neutral,
        }
    }

    /// True for the two real seats (not neutral).
    #[inline]
    pub fn is_real(self) -> bool {
        matches!(self, Faction::Player | Faction::Enemy)
    }
}

/// Index of a sub-structure into [`crate::sim::Structure::subs`].
pub type SubId = usize;

/// Index of a ship into [`crate::sim::Structure::ships`].
///
/// NOTE: ships are stored in a stable `Vec` and only ever *marked* dead (never removed
/// mid-run), so a `ShipId` stays valid for the whole simulation — convenient for a
/// renderer that wants to track a unit across frames. Dead ships are compacted only by the
/// optional [`crate::sim::Structure::compact_dead`] helper.
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
        if available == 0 {
            return 0;
        }
        let want = (available as f32 * self.as_f32()).round() as usize;
        want.clamp(1, available)
    }
}

/// A move order: send a fraction-bucket of `source`'s **idle** ships to `target`.
///
/// This is the Layer-1 atomic action. It is intentionally the same shape as `cell-core`'s
/// `Command` (`source, fraction-bucket, target`) so the shared-vocabulary spine holds
/// across layers. Issued via [`crate::sim::Structure::issue_order`].
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
