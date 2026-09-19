//! Cost queries along straight segments.
//!
//! Both the graph search and the smoothing pass need the same primitive: the
//! cost of travelling in a straight line, and whether that line is legal. Doing
//! it in one place keeps the search's edge costs and the smoother's feasibility
//! checks consistent — a mismatch there would let the two disagree about which
//! paths exist.

use glam::DVec2;

use super::cost::{CostField, INFINITE_COST};

/// Cost and length of a straight segment.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SegmentCost {
    /// Preference-weighted cost in equivalent metres.
    pub cost_equiv_m: f64,
    /// Geometric length in metres.
    pub length_m: f64,
}

/// Query interface over a cost field.
#[derive(Debug, Clone, Copy)]
pub struct CostSampler<'a> {
    field: &'a CostField,
    step_m: f64,
}

impl<'a> CostSampler<'a> {
    /// Creates a sampler with a default sampling step of half a map cell.
    pub fn new(field: &'a CostField) -> Self {
        Self {
            field,
            step_m: (field.grid().resolution * 0.5).max(0.1),
        }
    }

    /// Overrides the sampling step.
    pub fn with_step(mut self, step_m: f64) -> Self {
        self.step_m = step_m.max(1e-3);
        self
    }

    /// Sampling step in metres.
    #[inline]
    pub fn step_m(&self) -> f64 {
        self.step_m
    }

    /// Underlying cost field.
    #[inline]
    pub fn field(&self) -> &CostField {
        self.field
    }

    /// True when a straight segment stays inside passable ground.
    ///
    /// Uses the grid traversal of [`CostField::segment_is_clear`], which visits
    /// the cells the segment actually enters. An earlier version sampled every
    /// `step_m`, and a shortcut clipping the corner of a building could pass
    /// between two samples with both endpoints legal.
    ///
    /// [`CostField::segment_is_clear`]: crate::field::CostField::segment_is_clear
    pub fn is_clear(&self, from: DVec2, to: DVec2) -> bool {
        self.field.segment_is_clear(from, to)
    }

    /// Cost integral and length of a segment, or `None` when it is blocked.
    ///
    /// Clearance is decided exactly, from the cells the segment enters, while the
    /// cost is integrated by the trapezoidal rule over the samples: sampling the
    /// clearance would let a line clip the corner of a building between two
    /// samples and report the segment as usable.
    ///
    /// The heading is taken from the segment, so direction constraints are priced
    /// correctly.
    pub fn segment_cost(&self, from: DVec2, to: DVec2) -> Option<SegmentCost> {
        if !self.field.segment_is_clear(from, to) {
            return None;
        }
        let delta = to - from;
        let length = delta.length();
        if length <= f64::EPSILON {
            return Some(SegmentCost {
                cost_equiv_m: 0.0,
                length_m: 0.0,
            });
        }
        let heading = delta / length;
        let steps = (length / self.step_m).ceil().max(1.0) as usize;
        let step_length = length / steps as f64;

        let mut previous_cost = self.field.cost_at(from, Some(heading));
        if !previous_cost.is_finite() {
            return None;
        }
        let mut total = 0.0f64;
        for index in 1..=steps {
            let t = index as f64 / steps as f64;
            let point = from + delta * t;
            let cost = self.field.cost_at(point, Some(heading));
            if !cost.is_finite() {
                return None;
            }
            total += 0.5 * (previous_cost + cost) * step_length;
            previous_cost = cost;
        }
        Some(SegmentCost {
            cost_equiv_m: total,
            length_m: length,
        })
    }

    /// Cost of a polyline, or `None` when any of its segments is blocked.
    pub fn polyline_cost(&self, points: &[DVec2]) -> Option<SegmentCost> {
        let mut total = SegmentCost {
            cost_equiv_m: 0.0,
            length_m: 0.0,
        };
        for window in points.windows(2) {
            let segment = self.segment_cost(window[0], window[1])?;
            total.cost_equiv_m += segment.cost_equiv_m;
            total.length_m += segment.length_m;
        }
        Some(total)
    }

    /// Cost of a segment in equivalent metres, treating blockage as infinite.
    pub fn cost_or_infinite(&self, from: DVec2, to: DVec2) -> f64 {
        self.segment_cost(from, to)
            .map(|segment| segment.cost_equiv_m)
            .unwrap_or(f64::INFINITY)
    }

    /// Sentinel used to mark forbidden cells inside the field.
    #[inline]
    pub fn sentinel(&self) -> f32 {
        INFINITE_COST
    }
}
