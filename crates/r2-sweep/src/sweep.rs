//! The sweep proper: grid definition, evaluation, region analysis, operating-point
//! selection. Kept separate from `main.rs` (I/O) so the analysis is unit-testable.

use cell_core::engine::{GameState, Params};
use cell_core::harness::{cycle_on_map, cycle_over_maps, CyclePoint};

/// An inclusive, evenly-spaced axis of grid values.
#[derive(Debug, Clone)]
pub struct Axis {
    pub name: &'static str,
    pub min: f64,
    pub max: f64,
    pub steps: usize,
}

impl Axis {
    pub fn values(&self) -> Vec<f64> {
        if self.steps <= 1 {
            return vec![self.min];
        }
        let n = self.steps;
        (0..n)
            .map(|i| self.min + (self.max - self.min) * (i as f64) / ((n - 1) as f64))
            .collect()
    }
}

/// The full sweep configuration: the three axes (r, k, l), the match horizon, the
/// fixed engine constants, and the win-margin epsilon.
#[derive(Debug, Clone)]
pub struct SweepConfig {
    pub r_axis: Axis,
    pub k_axis: Axis,
    pub l_axis: Axis,
    /// Match length cap in ticks.
    pub horizon: u64,
    /// Minimum margin (in score units, [-1,1]) by which an intended counter must
    /// win for that edge to count as a genuine win. The cycle "holds" at a point
    /// iff all three edges exceed this.
    pub epsilon: f64,
    /// Fixed engine constants (undock delay, combat rate, substeps) reused at every
    /// grid point. Only `r`, `k`, `l` vary.
    pub fixed_undock: f64,
    pub fixed_combat_rate: f64,
    pub fixed_substeps: u32,
}

impl Default for SweepConfig {
    fn default() -> Self {
        SweepConfig {
            // Ranges chosen to bracket the qualitatively distinct regimes:
            //  r small  -> economy slow, board static; r large -> snowbally.
            //  k = 1    -> no defender edge; k up to 3 -> strong turtle.
            //  l = 0    -> free aggression; l up to ~0.45 -> very costly assaults.
            r_axis: Axis { name: "r", min: 0.4, max: 2.0, steps: 9 },
            k_axis: Axis { name: "k", min: 1.0, max: 3.0, steps: 9 },
            l_axis: Axis { name: "l", min: 0.0, max: 0.45, steps: 10 },
            horizon: 600,
            epsilon: 0.05,
            fixed_undock: 3.0,
            fixed_combat_rate: 0.5,
            fixed_substeps: 8,
        }
    }
}

impl SweepConfig {
    /// Build the [`Params`] for a given `(r, k, l)` grid point.
    pub fn params_at(&self, r: f64, k: f64, l: f64) -> Params {
        Params {
            r,
            k,
            l,
            undock_delay: self.fixed_undock,
            combat_substeps: self.fixed_substeps,
            combat_rate: self.fixed_combat_rate,
        }
    }

    /// A short human description of the swept ranges/resolution.
    pub fn describe(&self) -> String {
        format!(
            "{} in [{:.2},{:.2}] x{}, {} in [{:.2},{:.2}] x{}, {} in [{:.2},{:.2}] x{}; \
             horizon={}, epsilon={:.3}; fixed undock={}, combat_rate={}, substeps={}",
            self.r_axis.name, self.r_axis.min, self.r_axis.max, self.r_axis.steps,
            self.k_axis.name, self.k_axis.min, self.k_axis.max, self.k_axis.steps,
            self.l_axis.name, self.l_axis.min, self.l_axis.max, self.l_axis.steps,
            self.horizon, self.epsilon,
            self.fixed_undock, self.fixed_combat_rate, self.fixed_substeps,
        )
    }
}

/// One evaluated grid point: its coordinates, the three cycle-edge margins
/// (averaged over all maps and both seatings), and whether the cycle holds.
#[derive(Debug, Clone, Copy)]
pub struct GridPoint {
    pub ir: usize,
    pub ik: usize,
    pub il: usize,
    pub r: f64,
    pub k: f64,
    pub l: f64,
    pub cycle: CyclePoint,
}

impl GridPoint {
    pub fn holds(&self, epsilon: f64) -> bool {
        self.cycle.holds(epsilon)
    }
    pub fn min_margin(&self) -> f64 {
        self.cycle.min_margin()
    }
}

/// The complete result of a sweep.
#[derive(Debug, Clone)]
pub struct SweepResult {
    pub config: SweepConfig,
    pub points: Vec<GridPoint>,
    /// Number of grid points where the cycle holds.
    pub holds_count: usize,
    /// Total grid points.
    pub total: usize,
    /// Index into `points` of the most robust point (max min-margin among holders,
    /// or overall if none hold).
    pub best_idx: Option<usize>,
    /// Size (in points) of the largest 6-connected contiguous region of holders.
    pub largest_region_size: usize,
    /// The members (indices into `points`) of that largest region.
    pub largest_region: Vec<usize>,
}

/// Run the full sweep over `maps` (the symmetric battlefields) using `config`.
pub fn run_sweep(maps: &[GameState], config: &SweepConfig) -> SweepResult {
    let rs = config.r_axis.values();
    let ks = config.k_axis.values();
    let ls = config.l_axis.values();

    let mut points = Vec::with_capacity(rs.len() * ks.len() * ls.len());
    for (ir, &r) in rs.iter().enumerate() {
        for (ik, &k) in ks.iter().enumerate() {
            for (il, &l) in ls.iter().enumerate() {
                let params = config.params_at(r, k, l);
                let cycle = cycle_over_maps(maps, &params, config.horizon);
                points.push(GridPoint { ir, ik, il, r, k, l, cycle });
            }
        }
    }

    let total = points.len();
    let holds_count = points.iter().filter(|p| p.holds(config.epsilon)).count();
    let best_idx = pick_operating_point(&points, &rs, &ks, &ls, config.epsilon);
    let (largest_region, largest_region_size) =
        largest_holding_region(&points, &rs, &ks, &ls, config.epsilon);

    SweepResult {
        config: config.clone(),
        points,
        holds_count,
        total,
        best_idx,
        largest_region_size,
        largest_region,
    }
}

/// Pick a robust operating point. Among holding points we maximize a robustness
/// score that combines two desiderata:
///   * a large **minimum margin** (furthest from any single edge failing), and
///   * sitting in the **interior** of the region (its 6 grid-neighbors also hold),
///     so the choice tolerates parameter drift rather than perching on the rim.
///
/// Score = `min_margin + 0.05 * (holding_neighbors / 6)`. The small interior bonus
/// only breaks ties between similarly-balanced points, nudging the recommendation
/// toward the fat middle of the ridge. If no point holds, fall back to the best
/// min-margin point overall (still informative for a near-miss redesign).
fn pick_operating_point(
    points: &[GridPoint],
    rs: &[f64],
    ks: &[f64],
    ls: &[f64],
    epsilon: f64,
) -> Option<usize> {
    if points.is_empty() {
        return None;
    }
    let (nr, nk, nl) = (rs.len(), ks.len(), ls.len());
    let idx = |ir: usize, ik: usize, il: usize| -> usize { (ir * nk + ik) * nl + il };
    let holds: Vec<bool> = points.iter().map(|p| p.holds(epsilon)).collect();

    let interior_frac = |p: &GridPoint| -> f64 {
        let (ir, ik, il) = (p.ir, p.ik, p.il);
        let mut held = 0u32;
        let mut total = 0u32;
        let mut check = |c: usize| {
            total += 1;
            if holds[c] {
                held += 1;
            }
        };
        if ir + 1 < nr { check(idx(ir + 1, ik, il)); }
        if ir > 0 { check(idx(ir - 1, ik, il)); }
        if ik + 1 < nk { check(idx(ir, ik + 1, il)); }
        if ik > 0 { check(idx(ir, ik - 1, il)); }
        if il + 1 < nl { check(idx(ir, ik, il + 1)); }
        if il > 0 { check(idx(ir, ik, il - 1)); }
        if total == 0 { 0.0 } else { held as f64 / total as f64 }
    };

    let holders: Vec<usize> = (0..points.len()).filter(|&i| holds[i]).collect();
    if holders.is_empty() {
        // No holder: best min-margin overall.
        return (0..points.len()).max_by(|&a, &b| {
            points[a]
                .min_margin()
                .partial_cmp(&points[b].min_margin())
                .unwrap_or(std::cmp::Ordering::Equal)
        });
    }
    holders.into_iter().max_by(|&a, &b| {
        let sa = points[a].min_margin() + 0.05 * interior_frac(&points[a]);
        let sb = points[b].min_margin() + 0.05 * interior_frac(&points[b]);
        sa.partial_cmp(&sb).unwrap_or(std::cmp::Ordering::Equal)
    })
}

/// Find the largest 6-connected contiguous region of holding points in the 3D
/// grid (neighbors differ by 1 along exactly one axis). Returns `(members,size)`.
///
/// Contiguity is the design's "is the region contiguous? how large?" question:
/// a single isolated holder is fragile (likely noise/edge effect), while a fat
/// contiguous blob means the cycle is a robust property of a whole regime.
fn largest_holding_region(
    points: &[GridPoint],
    rs: &[f64],
    ks: &[f64],
    ls: &[f64],
    epsilon: f64,
) -> (Vec<usize>, usize) {
    let (nr, nk, nl) = (rs.len(), ks.len(), ls.len());
    let idx = |ir: usize, ik: usize, il: usize| -> usize { (ir * nk + ik) * nl + il };

    let holds: Vec<bool> = points.iter().map(|p| p.holds(epsilon)).collect();
    let mut visited = vec![false; points.len()];
    let mut best: Vec<usize> = Vec::new();

    for start in 0..points.len() {
        if visited[start] || !holds[start] {
            continue;
        }
        // Flood fill from this holder.
        let mut stack = vec![start];
        let mut region = Vec::new();
        visited[start] = true;
        while let Some(cur) = stack.pop() {
            region.push(cur);
            let p = &points[cur];
            let (ir, ik, il) = (p.ir, p.ik, p.il);
            // Six axis-neighbors.
            let mut neighbors: Vec<usize> = Vec::with_capacity(6);
            if ir + 1 < nr { neighbors.push(idx(ir + 1, ik, il)); }
            if ir > 0 { neighbors.push(idx(ir - 1, ik, il)); }
            if ik + 1 < nk { neighbors.push(idx(ir, ik + 1, il)); }
            if ik > 0 { neighbors.push(idx(ir, ik - 1, il)); }
            if il + 1 < nl { neighbors.push(idx(ir, ik, il + 1)); }
            if il > 0 { neighbors.push(idx(ir, ik, il - 1)); }
            for nb in neighbors {
                if !visited[nb] && holds[nb] {
                    visited[nb] = true;
                    stack.push(nb);
                }
            }
        }
        if region.len() > best.len() {
            best = region;
        }
    }
    let size = best.len();
    (best, size)
}

/// Per-map detail at a single params point (for the report's operating-point
/// breakdown).
#[derive(Debug, Clone)]
pub struct PerMapDetail {
    pub map_name: String,
    pub cycle: CyclePoint,
}

/// Evaluate each map individually at a params point (used to show the operating
/// point holds on every map, not just on average).
pub fn per_map_detail(
    named_maps: &[(String, GameState)],
    params: &Params,
    horizon: u64,
) -> Vec<PerMapDetail> {
    named_maps
        .iter()
        .map(|(name, state)| PerMapDetail {
            map_name: name.clone(),
            cycle: cycle_on_map(state, params, horizon),
        })
        .collect()
}
