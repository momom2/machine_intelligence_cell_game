//! The colonize/defend/attack **simplex** weight and its geometry.
//!
//! Every Automaton (`02-ai-opponents.md`) is a *mix* of the three pure strategies — a
//! point `(c, d, a)` with `c + d + a = 1`, `c,d,a >= 0`. This module is just that
//! point plus the two geometric quantities the capstone needs:
//!
//! * **centrality** — how close the mix is to the balanced centre `(1/3,1/3,1/3)`.
//!   The design fixes this as the **difficulty metric**: "balanced mixes near the
//!   centre of the simplex are hardest to read and to counter. Order the ladder by
//!   increasing centrality." So centrality is *the* number the whole step-4 validation
//!   is built around.
//! * **nearest corner** — which pure strategy the mix is closest to, used both to name
//!   a rung and as the ground-truth label inference is scored against.

/// The three pure strategies of the triad. The corners of the simplex.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Corner {
    Colonize,
    Defend,
    Attack,
}

impl Corner {
    pub fn name(self) -> &'static str {
        match self {
            Corner::Colonize => "Colonize",
            Corner::Defend => "Defend",
            Corner::Attack => "Attack",
        }
    }

    /// Short single-letter tag (C/D/A) for compact ladder labels.
    pub fn tag(self) -> char {
        match self {
            Corner::Colonize => 'C',
            Corner::Defend => 'D',
            Corner::Attack => 'A',
        }
    }

    /// This corner as a pure [`Mix`].
    pub fn as_mix(self) -> Mix {
        match self {
            Corner::Colonize => Mix::new(1.0, 0.0, 0.0),
            Corner::Defend => Mix::new(0.0, 1.0, 0.0),
            Corner::Attack => Mix::new(0.0, 0.0, 1.0),
        }
    }

    pub const ALL: [Corner; 3] = [Corner::Colonize, Corner::Defend, Corner::Attack];
}

/// A point in the colonize/defend/attack simplex: nonnegative weights summing to 1.
///
/// Construction always **normalizes** so callers can pass raw nonnegative weights
/// (e.g. `Mix::new(2.0, 1.0, 1.0)` for "twice as colonial as defensive/aggressive").
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Mix {
    /// Colonize weight.
    pub c: f64,
    /// Defend weight.
    pub d: f64,
    /// Attack weight.
    pub a: f64,
}

impl Mix {
    /// Build a mix from raw nonnegative weights, normalizing to sum 1. A
    /// degenerate all-zero input falls back to the balanced centre (so the type is
    /// total and never yields NaNs downstream).
    pub fn new(c: f64, d: f64, a: f64) -> Mix {
        let c = c.max(0.0);
        let d = d.max(0.0);
        let a = a.max(0.0);
        let s = c + d + a;
        if s <= 0.0 {
            return Mix::centre();
        }
        Mix { c: c / s, d: d / s, a: a / s }
    }

    /// The balanced centre `(1/3, 1/3, 1/3)` — the maximally central (hardest) mix.
    pub fn centre() -> Mix {
        let t = 1.0 / 3.0;
        Mix { c: t, d: t, a: t }
    }

    /// The weights as a 3-array in canonical (c, d, a) order.
    #[inline]
    pub fn weights(self) -> [f64; 3] {
        [self.c, self.d, self.a]
    }

    /// Euclidean distance to another mix in weight space.
    pub fn distance(self, other: Mix) -> f64 {
        let dc = self.c - other.c;
        let dd = self.d - other.d;
        let da = self.a - other.a;
        (dc * dc + dd * dd + da * da).sqrt()
    }

    /// The pure corner this mix is closest to (ties broken in C, D, A order). Used to
    /// name a rung and as the **ground-truth label** inference accuracy scores against.
    pub fn nearest_corner(self) -> Corner {
        let mut best = Corner::Colonize;
        let mut best_d = f64::INFINITY;
        for corner in Corner::ALL {
            let dist = self.distance(corner.as_mix());
            if dist < best_d - 1e-12 {
                best_d = dist;
                best = corner;
            }
        }
        best
    }

    /// Distance to the nearest pure corner. Zero at a corner, maximal-ish toward the
    /// centre. This is the raw "distance-from-pure" the design names; [`centrality`]
    /// turns it into a 0..1 difficulty score.
    ///
    /// [`centrality`]: Mix::centrality
    pub fn distance_from_pure(self) -> f64 {
        Corner::ALL
            .iter()
            .map(|c| self.distance(c.as_mix()))
            .fold(f64::INFINITY, f64::min)
    }

    /// **Centrality in [0, 1] — the difficulty metric** (`02`).
    ///
    /// `0.0` at a pure corner (cleanest counter, easiest rung) and `1.0` at the
    /// balanced centre `(1/3,1/3,1/3)` (hardest to read and to counter). Defined as
    /// `distance_from_pure / max_distance_from_pure`, where the denominator is the
    /// centre's own distance-from-pure (the largest any mix can have), so the scale is
    /// exactly pegged to "corner = 0, centre = 1". Monotone in distance-from-pure, which
    /// is all the ladder ordering needs.
    pub fn centrality(self) -> f64 {
        // The centre is the single most-central point; its distance-from-pure is the
        // maximum any mix can have, so it normalizes the scale. A point *at* a corner
        // has distance-from-pure 0 → centrality 0; the centre → 1.
        let max = Mix::centre().distance_from_pure();
        if max <= 0.0 {
            return 0.0;
        }
        (self.distance_from_pure() / max).clamp(0.0, 1.0)
    }

    /// A compact human label like `C` (pure), `C2D1` (2:1 colonize:defend) or `bal`
    /// (the centre), good for ladder rows and report tables.
    pub fn label(self) -> String {
        // Pure corner?
        for corner in Corner::ALL {
            if self.distance(corner.as_mix()) < 1e-9 {
                return corner.tag().to_string();
            }
        }
        // Balanced centre?
        if self.distance(Mix::centre()) < 1e-9 {
            return "bal".to_string();
        }
        // Otherwise list the nonzero corners with small integer-ish ratios. We render
        // each weight as a count out of 6 (the smallest denominator that distinguishes
        // the ladder's points) and drop zeros.
        let parts: [(char, f64); 3] = [
            (Corner::Colonize.tag(), self.c),
            (Corner::Defend.tag(), self.d),
            (Corner::Attack.tag(), self.a),
        ];
        let mut s = String::new();
        for (tag, w) in parts {
            let n = (w * 6.0).round() as i32;
            if n > 0 {
                s.push(tag);
                s.push_str(&n.to_string());
            }
        }
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_to_sum_one() {
        let m = Mix::new(2.0, 1.0, 1.0);
        assert!((m.c + m.d + m.a - 1.0).abs() < 1e-12);
        assert!((m.c - 0.5).abs() < 1e-12);
    }

    #[test]
    fn all_zero_falls_back_to_centre() {
        let m = Mix::new(0.0, 0.0, 0.0);
        assert!((m.distance(Mix::centre())).abs() < 1e-12);
    }

    #[test]
    fn corners_have_zero_centrality() {
        for c in Corner::ALL {
            assert!(c.as_mix().centrality() < 1e-9, "{} corner centrality", c.name());
        }
    }

    #[test]
    fn centre_has_max_centrality() {
        assert!((Mix::centre().centrality() - 1.0).abs() < 1e-9);
    }

    #[test]
    fn centrality_is_monotone_from_corner_to_centre() {
        // Walk from the pure-colonize corner toward the centre; centrality must rise.
        let corner = Corner::Colonize.as_mix();
        let centre = Mix::centre();
        let mut prev = -1.0;
        for i in 0..=10 {
            let t = i as f64 / 10.0;
            let m = Mix::new(
                corner.c * (1.0 - t) + centre.c * t,
                corner.d * (1.0 - t) + centre.d * t,
                corner.a * (1.0 - t) + centre.a * t,
            );
            let cen = m.centrality();
            assert!(cen >= prev - 1e-12, "centrality must be monotone, {cen} < {prev}");
            prev = cen;
        }
    }

    #[test]
    fn nearest_corner_is_the_dominant_weight() {
        assert_eq!(Mix::new(0.6, 0.2, 0.2).nearest_corner(), Corner::Colonize);
        assert_eq!(Mix::new(0.2, 0.6, 0.2).nearest_corner(), Corner::Defend);
        assert_eq!(Mix::new(0.2, 0.2, 0.6).nearest_corner(), Corner::Attack);
    }
}
