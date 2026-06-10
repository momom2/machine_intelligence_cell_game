//! Tiny deterministic graph helpers over a [`world::World`]'s lane graph, shared by the
//! Layer-2 adapter ([`crate::adapters`]) and the strategic policies ([`crate::strategy`]).
//!
//! Both need the same two primitives: the **shortest-path length** between two planets (to
//! pick a "nearest" objective) and the **first hop** along that path (because a
//! `world::FleetOrder` is only valid between lane-adjacent planets, so a multi-lane move must
//! be routed one lane at a time). They are plain Dijkstra over lane lengths on the small
//! planet graph (an O(V^2) scan — fine here and order-stable, lowest id breaking ties).

use world::{PlanetId, World};

/// Per-planet Dijkstra result from a single source: `dist[v]` (sum of lane lengths, `INFINITY`
/// if unreachable) and `prev[v]` (predecessor on the best path, `usize::MAX` if none).
struct Sssp {
    dist: Vec<f32>,
    prev: Vec<usize>,
}

/// Dijkstra from `from` over the lane graph (lane lengths as edge weights, clamped to a tiny
/// positive minimum so a zero/negative length still has finite cost). Deterministic: the
/// frontier picks the lowest-id node among equal distances.
fn sssp(world: &World, from: PlanetId) -> Sssp {
    let n = world.planets.len();
    let mut dist = vec![f32::INFINITY; n];
    let mut prev = vec![usize::MAX; n];
    let mut done = vec![false; n];
    if from < n {
        dist[from] = 0.0;
    }
    for _ in 0..n {
        let mut u = usize::MAX;
        let mut best = f32::INFINITY;
        for v in 0..n {
            if !done[v] && dist[v] < best {
                best = dist[v];
                u = v;
            }
        }
        if u == usize::MAX {
            break;
        }
        done[u] = true;
        for &w in world.neighbors(u) {
            let len = world.lane_length(u, w).unwrap_or(1.0).max(0.0001);
            let nd = dist[u] + len;
            if nd < dist[w] {
                dist[w] = nd;
                prev[w] = u;
            }
        }
    }
    Sssp { dist, prev }
}

/// Shortest-path length (sum of lane lengths) from `from` to `to`, or `None` if unreachable.
/// `from == to` is `Some(0.0)`.
pub fn path_len(world: &World, from: PlanetId, to: PlanetId) -> Option<f32> {
    if from == to {
        return Some(0.0);
    }
    if from >= world.planets.len() || to >= world.planets.len() {
        return None;
    }
    let s = sssp(world, from);
    let d = s.dist[to];
    if d.is_infinite() {
        None
    } else {
        Some(d)
    }
}

/// First planet on a shortest path from `from` to `to` (the lane-adjacent planet a
/// `world::FleetOrder` should target to move toward `to`), or `None` if `to` is unreachable or
/// `from == to`.
pub fn next_hop(world: &World, from: PlanetId, to: PlanetId) -> Option<PlanetId> {
    if from == to || from >= world.planets.len() || to >= world.planets.len() {
        return None;
    }
    let s = sssp(world, from);
    if s.dist[to].is_infinite() {
        return None;
    }
    let mut cur = to;
    while s.prev[cur] != from {
        if s.prev[cur] == usize::MAX {
            return None;
        }
        cur = s.prev[cur];
    }
    Some(cur)
}
