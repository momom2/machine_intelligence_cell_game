//! Fixed, **symmetric** battlefields with equal opposing starts.
//!
//! Symmetry matters for R2: if the map favored a seat, a matchup could "win" for
//! reasons unrelated to strategy. Every map here has an involution swapping A's and
//! B's positions while preserving the graph, so playing both seatings (as the sweep
//! does) fully cancels any first-player/positional bias. The `mirror` field records
//! that involution and the tests assert it holds.
//!
//! ## Why these shapes (hard-won from the R2 tuning)
//! The triad's RPS cycle needs three things from a map simultaneously:
//!  1. **Short contact** — homes/fronts close enough (small edge lengths) that a
//!     timing Attack can profitably strike a thin colonizer before it consolidates.
//!     Long bridges let the commitment loss `l` eat an attacker alive regardless of
//!     skill, which collapses the attack≻colonize edge.
//!  2. **A contestable neutral surplus** — several comparable neutral nodes a fast
//!     expander (Colonize) can grab *more* of than a cautious turtle (Defend),
//!     giving Colonize the economy edge that beats Defend. A single dominant prize
//!     node instead collapses every strategy into "grab the center."
//!  3. **No perpetual oscillating chokepoint** — enough separate neutral nodes that
//!     the two sides claim *different* ones rather than dribbling equal fleets into
//!     one node forever.

use crate::engine::GameState;
use crate::types::*;

/// A named, symmetric starting position.
pub struct MapSpec {
    pub name: &'static str,
    pub state: GameState,
    /// Position involution: swapping the seats maps node `i` to `mirror[i]`.
    pub mirror: Vec<NodeId>,
}

fn node(owner: Owner, garrison: f64, mult: f64) -> Node {
    Node { owner, garrison, production_mult: mult }
}

fn edge(a: NodeId, b: NodeId, length: f64) -> Edge {
    Edge { a, b, length }
}

/// **Corridor-7** — homes at the ends; each side has one private node; a contested
/// strip of **three** neutral nodes runs down the middle.
///
/// ```text
///   A(0) -- P(1) -- N(2) -- N(3) -- N(4) -- P(5) -- B(6)
/// ```
///
/// Short hops (length 1) keep contact tight. The three middle neutrals (2,3,4) are
/// the surplus: a colonizer grabs more of them than a turtle (economy edge), the
/// center node 3 is the natural seam, and after the strip is split the two sides are
/// one hop apart for assaults. Symmetric under `i -> 6 - i`.
pub fn corridor7() -> MapSpec {
    let nodes = vec![
        node(Owner::A, 10.0, 1.0),       // 0 home A
        node(Owner::Neutral, 0.0, 1.0),  // 1 A private
        node(Owner::Neutral, 0.0, 1.0),  // 2 mid near A
        node(Owner::Neutral, 0.0, 1.0),  // 3 center
        node(Owner::Neutral, 0.0, 1.0),  // 4 mid near B
        node(Owner::Neutral, 0.0, 1.0),  // 5 B private
        node(Owner::B, 10.0, 1.0),       // 6 home B
    ];
    let edges = vec![
        edge(0, 1, 1.0),
        edge(1, 2, 1.0),
        edge(2, 3, 1.0),
        edge(3, 4, 1.0),
        edge(4, 5, 1.0),
        edge(5, 6, 1.0),
    ];
    MapSpec {
        name: "corridor7",
        state: GameState::new(nodes, edges),
        mirror: vec![6, 5, 4, 3, 2, 1, 0],
    }
}

/// **Twin-bridge-8** — two homes, one private node each, joined by **two** parallel
/// two-node neutral bridges. Gives Defend two doors to cover and Attack a lane to
/// concentrate on, while the four bridge neutrals form the contestable surplus.
///
/// ```text
///         N(2) -- N(3)
///        /          \
///   A(0)-P(1)        P(6)-B(7)
///        \          /
///         N(4) -- N(5)
/// ```
///
/// Node 1 (A private) -> bridge-top 2 and bridge-bottom 4; 2-3 and 4-5 are the
/// bridges; 3 and 5 -> node 6 (B private). Short hops. Symmetric under
/// `0<->7, 1<->6, 2<->5, 3<->4`.
pub fn twin_bridge8() -> MapSpec {
    let nodes = vec![
        node(Owner::A, 10.0, 1.0),      // 0 home A
        node(Owner::Neutral, 0.0, 1.0), // 1 A private
        node(Owner::Neutral, 0.0, 1.0), // 2 top near A
        node(Owner::Neutral, 0.0, 1.0), // 3 top near B
        node(Owner::Neutral, 0.0, 1.0), // 4 bottom near A
        node(Owner::Neutral, 0.0, 1.0), // 5 bottom near B
        node(Owner::Neutral, 0.0, 1.0), // 6 B private
        node(Owner::B, 10.0, 1.0),      // 7 home B
    ];
    let edges = vec![
        edge(0, 1, 1.0),
        edge(1, 2, 1.0),
        edge(1, 4, 1.0),
        edge(2, 3, 1.0),
        edge(4, 5, 1.0),
        edge(3, 6, 1.0),
        edge(5, 6, 1.0),
        edge(6, 7, 1.0),
    ];
    MapSpec {
        name: "twin_bridge8",
        state: GameState::new(nodes, edges),
        mirror: vec![7, 6, 5, 4, 3, 2, 1, 0],
    }
}

/// **Star-9** — each home owns two private spokes; the two fronts meet at a small
/// contested **core of three** neutral nodes in a line. Combines a private economy
/// with a multi-node contested middle, and keeps every hop short.
///
/// ```text
///   P(1)            P(7)
///      \            /
///   A(0)-- C(3)-C(4)-C(5) --B(8)
///      /            \
///   P(2)            P(6)
/// ```
///
/// A(0) -> private spokes 1,2 and core-left 3; core 3-4-5 (all equal, so no single
/// prize favors a pure colonizer); core-right 5 -> B(8); B(8) -> private spokes 7,6.
/// Symmetric under `0<->8, 1<->7, 2<->6, 3<->5, 4<->4`.
pub fn star9() -> MapSpec {
    let nodes = vec![
        node(Owner::A, 10.0, 1.0),      // 0 home A
        node(Owner::Neutral, 0.0, 1.0), // 1 A spoke
        node(Owner::Neutral, 0.0, 1.0), // 2 A spoke
        node(Owner::Neutral, 0.0, 1.0), // 3 core left
        node(Owner::Neutral, 0.0, 1.0), // 4 core center
        node(Owner::Neutral, 0.0, 1.0), // 5 core right
        node(Owner::Neutral, 0.0, 1.0), // 6 B spoke
        node(Owner::Neutral, 0.0, 1.0), // 7 B spoke
        node(Owner::B, 10.0, 1.0),      // 8 home B
    ];
    // Edge order is chosen to be MIRROR-CONSISTENT: each node's adjacency list comes
    // out as the mirror image of its counterpart's, so even a *single* seating of
    // self-play is perfectly even (no tie-break drift at the self-mirror center).
    // A-side edges and their mirrors (under 0<->8,1<->7,2<->6,3<->5,4<->4) are
    // interleaved so both homes iterate neighbors in mirror order.
    let edges = vec![
        edge(0, 1, 1.0),
        edge(8, 7, 1.0), // mirror of 0-1
        edge(0, 2, 1.0),
        edge(8, 6, 1.0), // mirror of 0-2
        edge(0, 3, 1.0),
        edge(8, 5, 1.0), // mirror of 0-3
        edge(3, 4, 1.0),
        edge(5, 4, 1.0), // mirror of 3-4
    ];
    MapSpec {
        name: "star9",
        state: GameState::new(nodes, edges),
        mirror: vec![8, 7, 6, 5, 4, 3, 2, 1, 0],
    }
}

/// All maps used by the sweep, in order.
pub fn all_maps() -> Vec<MapSpec> {
    vec![corridor7(), twin_bridge8(), star9()]
}
