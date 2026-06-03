//! Core value types shared across the engine.
//!
//! Everything here is plain data. There is deliberately **no randomness** anywhere
//! in this crate: this is the *mean-field* layer described in `01-mechanics.md`.
//! Randomness is a future *render* concern only.

/// Which player owns a node or fleet.
///
/// We model a strictly two-player match plus unowned ("neutral") nodes, which is
/// all the R2 cycle-sweep needs. Neutral nodes do not produce until colonized.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Owner {
    /// The first seat.
    A,
    /// The second seat.
    B,
    /// Unowned / empty node. Produces nothing until someone colonizes it.
    Neutral,
}

impl Owner {
    /// The opposing *player* seat. `Neutral.opponent()` is meaningless and returns
    /// `Neutral` (callers never ask for a neutral's opponent in practice).
    #[inline]
    pub fn opponent(self) -> Owner {
        match self {
            Owner::A => Owner::B,
            Owner::B => Owner::A,
            Owner::Neutral => Owner::Neutral,
        }
    }

    /// True for the two real player seats (not neutral).
    #[inline]
    pub fn is_player(self) -> bool {
        matches!(self, Owner::A | Owner::B)
    }
}

/// Index of a node into [`crate::engine::GameState::nodes`].
pub type NodeId = usize;

/// Index of an edge into [`crate::engine::GameState::edges`].
pub type EdgeId = usize;

/// The fraction buckets the design fixes for atomic actions: 25/50/75/100 %.
///
/// `01-mechanics.md`: "Atomic action: `(source, fraction-bucket ∈ {25,50,75,100}%,
/// target)`." Policies emit commands using these buckets so that the action space
/// matches the eventual DSL exactly.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FractionBucket {
    Quarter,
    Half,
    ThreeQuarter,
    All,
}

impl FractionBucket {
    /// The multiplier in [0,1] this bucket represents.
    #[inline]
    pub fn as_f64(self) -> f64 {
        match self {
            FractionBucket::Quarter => 0.25,
            FractionBucket::Half => 0.50,
            FractionBucket::ThreeQuarter => 0.75,
            FractionBucket::All => 1.00,
        }
    }
}

/// A production structure ("planet"/"factory") on the battlefield graph.
#[derive(Debug, Clone, PartialEq)]
pub struct Node {
    /// Who currently controls this node.
    pub owner: Owner,
    /// Current garrison (number of stationed ships). Real-valued because the
    /// mean-field model is continuous; the rendered game would round for display.
    pub garrison: f64,
    /// Per-tick production multiplier for this node. Effective production is
    /// `production_mult * r` (the swept colonization rate) when player-owned.
    /// Lets maps have richer/poorer nodes without changing the global `r`.
    pub production_mult: f64,
}

/// A bidirectional path between two nodes with a travel length (in ticks).
#[derive(Debug, Clone, PartialEq)]
pub struct Edge {
    pub a: NodeId,
    pub b: NodeId,
    /// Travel time in ticks for a fleet to traverse this edge (after undocking).
    pub length: f64,
}

impl Edge {
    /// Given one endpoint, return the other. Panics if `from` is not an endpoint.
    #[inline]
    pub fn other(&self, from: NodeId) -> NodeId {
        if from == self.a {
            self.b
        } else if from == self.b {
            self.a
        } else {
            panic!("node {from} is not an endpoint of edge {:?}", self);
        }
    }

    /// True if `n` is one of this edge's endpoints.
    #[inline]
    pub fn touches(&self, n: NodeId) -> bool {
        self.a == n || self.b == n
    }
}

/// A fleet in transit along an edge.
///
/// Movement has two phases, both deterministic given commit order
/// (see `01-mechanics.md` "Movement"):
///   1. **Undock**: `undock_remaining` ticks during which the ships have left the
///      source garrison but have NOT begun travelling. They deal no damage and are
///      not yet protected by a node — this is the commitment-vs-tempo tension.
///   2. **Transit**: once undocked, `progress` advances toward `edge.length`.
#[derive(Debug, Clone, PartialEq)]
pub struct Fleet {
    pub owner: Owner,
    /// Edge being traversed.
    pub edge: EdgeId,
    /// The node this fleet departed from (so we know which way it is going).
    pub from: NodeId,
    /// The destination node.
    pub to: NodeId,
    /// Number of ships in the fleet (real-valued, mean-field).
    pub count: f64,
    /// Ticks of undock delay still remaining before transit begins.
    pub undock_remaining: f64,
    /// Distance travelled so far along the edge once undocked, in ticks.
    pub progress: f64,
}

impl Fleet {
    /// True while the fleet is still undocking (vulnerable, dealing no damage).
    #[inline]
    pub fn is_undocking(&self) -> bool {
        self.undock_remaining > 0.0
    }
}

/// A single command a policy can issue on a tick: send `fraction` of `source`'s
/// garrison toward `target` along the (assumed direct) connecting edge.
///
/// This is exactly the design's atomic action `(source, fraction-bucket, target)`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Command {
    pub source: NodeId,
    pub target: NodeId,
    pub fraction: FractionBucket,
}
