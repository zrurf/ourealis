//! Arc-length parameterised path.
//!
//! Everything downstream of smoothing indexes the path by arc length: the speed
//! profile, the lateral offset injection, the attitude model and the sensors all
//! ask "where am I at metre 1234.5, and which way am I pointing". This type
//! answers exactly that, and caches the curvature array that the speed limits
//! and the turn-rate metrics consume.

use glam::DVec2;

use crate::error::{CoreError, Result};
use crate::math::left_normal;

use super::resample;

/// A polyline with arc-length lookup tables.
#[derive(Debug, Clone, PartialEq)]
pub struct Path {
    points: Vec<DVec2>,
    cumulative: Vec<f64>,
    curvatures: Vec<f64>,
    /// Per-vertex elevation overrides, one entry per sample.
    ///
    /// Empty when the terrain supplies every elevation, which is the common
    /// case. A `None` entry means the vertex takes its elevation from the
    /// terrain; the channel only carries what the terrain cannot know —
    /// Z-axis links — so a path that uses none stays byte-for-byte the path the
    /// planner produced.
    elevation: Vec<Option<f64>>,
    /// Per-vertex speed ceiling, one entry per sample.
    ///
    /// The companion of the elevation channel: a Z-axis link is not terrain, so
    /// neither its height nor its traversal speed can come from the fields. A
    /// stair is climbed at its own equivalent speed — the physiological slope
    /// model does not apply to steps — and a `None` entry means the sample is
    /// ordinary ground. Empty when the path uses no link.
    link_speed: Vec<Option<f64>>,
}

impl Path {
    /// Builds a path from raw points, removing duplicates.
    ///
    /// The result has no elevation override: every vertex takes its elevation
    /// from the terrain.
    pub fn new(points: Vec<DVec2>) -> Result<Self> {
        Self::with_links(points, Vec::new(), Vec::new())
    }

    /// Builds a path from raw points and per-vertex elevation overrides.
    ///
    /// `elevation` is either empty — equivalent to [`Path::new`] — or carries
    /// exactly one entry per input point. Points closer than the deduplication
    /// tolerance are removed together with their override, so the channel keeps
    /// its one-to-one mapping onto the samples.
    pub fn with_elevation(points: Vec<DVec2>, elevation: Vec<Option<f64>>) -> Result<Self> {
        Self::with_links(points, elevation, Vec::new())
    }

    /// Builds a path with both per-vertex link channels.
    ///
    /// Each channel is either empty — every vertex takes the value the fields
    /// give it — or carries exactly one entry per input point. Points closer
    /// than the deduplication tolerance are removed together with their entries,
    /// so both channels keep their one-to-one mapping onto the samples.
    pub fn with_links(
        points: Vec<DVec2>,
        elevation: Vec<Option<f64>>,
        link_speed: Vec<Option<f64>>,
    ) -> Result<Self> {
        for (channel, name) in [(&elevation, "elevation"), (&link_speed, "link speed")] {
            if !channel.is_empty() && channel.len() != points.len() {
                return Err(CoreError::config(format!(
                    "{name} overrides must have one entry per path point"
                )));
            }
        }
        let (points, channels) = deduplicate_with_channels(&points, [elevation, link_speed]);
        if points.len() < 2 {
            return Err(CoreError::config(
                "a path needs at least two distinct points",
            ));
        }
        let cumulative = resample::cumulative_lengths(&points);
        let curvatures = resample::curvatures(&points);
        let [elevation, link_speed] = channels;
        Ok(Self {
            points,
            cumulative,
            curvatures,
            elevation,
            link_speed,
        })
    }

    /// Builds a path, resampling it to a fixed spacing first.
    pub fn resampled(points: Vec<DVec2>, spacing: f64) -> Result<Self> {
        Self::new(resample::resample(&points, spacing))
    }

    /// Sample points.
    #[inline]
    pub fn points(&self) -> &[DVec2] {
        &self.points
    }

    /// Arc length at each sample.
    #[inline]
    pub fn cumulative(&self) -> &[f64] {
        &self.cumulative
    }

    /// Signed curvature at each sample, positive for a left turn.
    #[inline]
    pub fn curvature(&self) -> &[f64] {
        &self.curvatures
    }

    /// Total length in metres.
    #[inline]
    pub fn total_length(&self) -> f64 {
        *self.cumulative.last().unwrap_or(&0.0)
    }

    /// Number of samples.
    #[inline]
    pub fn len(&self) -> usize {
        self.points.len()
    }

    /// True when the path has no sample.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.points.is_empty()
    }

    /// Start point.
    pub fn start(&self) -> DVec2 {
        self.points[0]
    }

    /// End point.
    pub fn end(&self) -> DVec2 {
        self.points[self.points.len() - 1]
    }

    /// Index of the sample at or before `s`.
    pub fn index_at(&self, s: f64) -> usize {
        let clamped = s.clamp(0.0, self.total_length());
        match self.cumulative.binary_search_by(|value| {
            value
                .partial_cmp(&clamped)
                .unwrap_or(std::cmp::Ordering::Equal)
        }) {
            Ok(index) => index.min(self.points.len() - 2),
            Err(index) => index.saturating_sub(1).min(self.points.len() - 2),
        }
    }

    /// Position at arc length `s`.
    pub fn position_at(&self, s: f64) -> DVec2 {
        let clamped = s.clamp(0.0, self.total_length());
        let index = self.index_at(clamped);
        let start = self.cumulative[index];
        let span = (self.cumulative[index + 1] - start).max(1e-9);
        let t = ((clamped - start) / span).clamp(0.0, 1.0);
        self.points[index] + (self.points[index + 1] - self.points[index]) * t
    }

    /// Forward unit tangent at arc length `s`.
    pub fn tangent_at(&self, s: f64) -> DVec2 {
        let index = self.index_at(s.clamp(0.0, self.total_length()));
        let delta = self.points[index + 1] - self.points[index];
        if delta.length() <= 1e-12 {
            DVec2::X
        } else {
            delta.normalize()
        }
    }

    /// Left unit normal at arc length `s`.
    pub fn normal_at(&self, s: f64) -> DVec2 {
        left_normal(self.tangent_at(s))
    }

    /// Curvature at arc length `s`, linearly interpolated.
    pub fn curvature_at(&self, s: f64) -> f64 {
        let clamped = s.clamp(0.0, self.total_length());
        let index = self.index_at(clamped);
        let start = self.cumulative[index];
        let span = (self.cumulative[index + 1] - start).max(1e-9);
        let t = ((clamped - start) / span).clamp(0.0, 1.0);
        crate::math::sampling::lerp(self.curvatures[index], self.curvatures[index + 1], t)
    }

    /// Elevation override at arc length `s`, linearly interpolated.
    ///
    /// `None` when the terrain supplies the elevation: the channel is empty, or
    /// one of the two vertices bracketing `s` carries no override. Refusing to
    /// extrapolate from a lone override is what makes the override blend back
    /// into the terrain exactly at the sampled vertex that pins it.
    pub fn elevation_at(&self, s: f64) -> Option<f64> {
        if self.elevation.is_empty() {
            return None;
        }
        let clamped = s.clamp(0.0, self.total_length());
        let index = self.index_at(clamped);
        let start = self.elevation[index]?;
        let end = self.elevation[index + 1]?;
        let from = self.cumulative[index];
        let span = (self.cumulative[index + 1] - from).max(1e-9);
        let t = ((clamped - from) / span).clamp(0.0, 1.0);
        Some(start + (end - start) * t)
    }

    /// Speed ceiling of a Z-axis link at arc length `s`, when the sample is on
    /// one.
    ///
    /// Not interpolated, unlike the elevation channel: a link has one equivalent
    /// speed, and blending it with the free speed of the ground beside it would
    /// let the runner accelerate inside the stairwell. The ceiling covers the
    /// whole segment as soon as one of its ends is on the link, so a sample
    /// cannot land on a stair's landing at free speed; a fraction of a metre of
    /// extra caution at each end is the honest side to err on.
    pub fn link_speed_at(&self, s: f64) -> Option<f64> {
        if self.link_speed.is_empty() {
            return None;
        }
        let clamped = s.clamp(0.0, self.total_length());
        let index = self.index_at(clamped);
        let start = self.link_speed[index];
        let end = self.link_speed[index + 1];
        match (start, end) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (Some(value), None) | (None, Some(value)) => Some(value),
            (None, None) => None,
        }
    }

    /// Elevation grade at arc length `s`, rise over run along the path.
    ///
    /// Interpolated between the same two vertices as [`Path::elevation_at`], so
    /// it is `None` exactly where that is; where it is present, the value is
    /// `dz/ds`, the quantity the body pitch and the physiological slope limit
    /// read.
    pub fn grade_at(&self, s: f64) -> Option<f64> {
        if self.elevation.is_empty() {
            return None;
        }
        let clamped = s.clamp(0.0, self.total_length());
        let index = self.index_at(clamped);
        let start = self.elevation[index]?;
        let end = self.elevation[index + 1]?;
        let span = self.cumulative[index + 1] - self.cumulative[index];
        (span > resample::MIN_SEGMENT_M).then(|| (end - start) / span)
    }

    /// Sub-path between two arc lengths.
    pub fn sub_path(&self, from: f64, to: f64) -> Result<Self> {
        let (from, to) = if from <= to { (from, to) } else { (to, from) };
        let from = from.max(0.0);
        let to = to.min(self.total_length());
        if to - from < 1e-6 {
            return Err(CoreError::config("sub-path has zero length"));
        }
        let mut points = vec![self.position_at(from)];
        let mut index = self.index_at(from) + 1;
        while index < self.points.len() && self.cumulative[index] < to {
            points.push(self.points[index]);
            index += 1;
        }
        points.push(self.position_at(to));
        Self::new(points)
    }

    /// Concatenates two paths, dropping the duplicated junction point.
    pub fn concatenate(first: &Path, second: &Path) -> Result<Self> {
        let mut points = first.points.clone();
        let junction = first.end();
        let mut rest = second.points.clone();
        if let Some(first_point) = rest.first()
            && (*first_point - junction).length() <= 1e-6
        {
            rest.remove(0);
        }
        points.extend(rest);
        Self::new(points)
    }

    /// Samples every `spacing` metres, always including the end point.
    ///
    /// A non-positive spacing cannot describe a sampling interval, so the
    /// endpoints are returned instead of dividing by it.
    pub fn sample_positions(&self, spacing: f64) -> Vec<DVec2> {
        let total = self.total_length();
        if spacing.is_nan() || spacing <= 0.0 || total <= spacing {
            return vec![self.start(), self.end()];
        }
        let steps = (total / spacing).ceil().max(1.0) as usize;
        let step = total / steps as f64;
        let mut out: Vec<DVec2> = (0..=steps)
            .map(|i| self.position_at(i as f64 * step))
            .collect();
        if let Some(last) = out.last_mut() {
            *last = self.end();
        }
        out
    }

    /// Recomputes the curvature table after external modification of the points.
    pub fn refresh_curvatures(&mut self) {
        self.curvatures = resample::curvatures(&self.points);
    }

    /// Smooths the curvature table with a moving average.
    pub fn with_smoothed_curvature(mut self, window: usize) -> Self {
        self.curvatures = resample::smooth_scalar(&self.curvatures, window);
        self
    }

    /// Largest absolute curvature along the path.
    pub fn max_curvature(&self) -> f64 {
        self.curvatures
            .iter()
            .map(|value| value.abs())
            .fold(0.0f64, f64::max)
    }

    /// Largest turn angle between consecutive samples, in radians.
    pub fn max_turn_angle(&self) -> f64 {
        resample::max_turn_angle(&self.points)
    }
}

/// Removes near-duplicate points and their overrides in lockstep.
///
/// [`resample::deduplicate`] keeps the first point of every run, so the paired
/// walk keeps the override that belongs to it. An empty channel passes through
/// the unchecked implementation, which keeps [`Path::new`] exactly as it was.
/// Removes duplicate points, keeping every parallel channel aligned with them.
fn deduplicate_with_channels<const N: usize>(
    points: &[DVec2],
    channels: [Vec<Option<f64>>; N],
) -> (Vec<DVec2>, [Vec<Option<f64>>; N]) {
    if channels.iter().all(|channel| channel.is_empty()) {
        return (
            resample::deduplicate(points, resample::MIN_SEGMENT_M),
            std::array::from_fn(|_| Vec::new()),
        );
    }
    let mut kept_points: Vec<DVec2> = Vec::with_capacity(points.len());
    let mut kept: [Vec<Option<f64>>; N] = std::array::from_fn(|_| Vec::with_capacity(points.len()));
    for (index, point) in points.iter().enumerate() {
        let keep = kept_points
            .last()
            .map(|last: &DVec2| (*last - *point).length() > resample::MIN_SEGMENT_M)
            .unwrap_or(true);
        if !keep {
            continue;
        }
        kept_points.push(*point);
        for (channel, kept_channel) in channels.iter().zip(kept.iter_mut()) {
            if !channel.is_empty() {
                kept_channel.push(channel[index]);
            }
        }
    }
    (kept_points, kept)
}
