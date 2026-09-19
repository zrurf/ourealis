//! Region-triggered sensor events.
//!
//! Multipath bursts, GNSS dropouts and magnetic disturbances are caused by where
//! the runner is, not by how far they have run. They are therefore scheduled by
//! region entry, with one of two decision modes:
//!
//! * **probabilistic** — the entry independently triggers with the region's
//!   probability, which is what a statistical evaluation wants;
//! * **spatially deterministic** — a hash of `(seed, region, entry index)`
//!   decides, so the same individual reproduces the same event sequence on every
//!   run. Regression tests and parameter calibration require this mode, since
//!   otherwise two runs with identical inputs produce different data.
//!
//! Events have a smooth envelope: a step-shaped bias would show up in the
//! spectrum as a discontinuity that no physical multipath produces.

use std::collections::HashMap;

use glam::DVec2;

use ourealis_map_format::region::{RegionFeature, RegionSet, TriggerMode};

use crate::rng::Rng;

/// An event currently in progress.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ActiveEvent {
    /// Identifier of the region that produced it.
    pub region_index: usize,
    /// Start time, seconds.
    pub started_s: f64,
    /// Duration of the multipath burst, seconds.
    pub duration_s: f64,
    /// Duration of the magnetic disturbance, seconds.
    ///
    /// Shorter than a multipath burst and drawn separately: the design gives 1-10 s
    /// for a dipole pulse from a passing vehicle or a steel-framed building and
    /// 10-60 s for multipath, and they are different phenomena even when one
    /// region triggers both.
    pub magnetic_duration_s: f64,
    /// Bias direction in the local plane, radians.
    pub direction_rad: f64,
    /// Bias magnitude, in the unit of the sensor being disturbed.
    pub magnitude: f64,
    /// Entry counter of that region, for reproducibility bookkeeping.
    pub entry_index: u32,
}

impl ActiveEvent {
    /// Envelope value in `[0, 1]` at a time: a raised cosine, so the bias ramps
    /// in and out instead of appearing as a step.
    pub fn envelope_at(&self, t: f64) -> f64 {
        if t <= self.started_s || t >= self.started_s + self.duration_s {
            return 0.0;
        }
        let tau = (t - self.started_s) / self.duration_s;
        0.5 - 0.5 * (std::f64::consts::TAU * tau).cos()
    }

    /// True while the event is active.
    pub fn is_active(&self, t: f64) -> bool {
        t >= self.started_s && t < self.started_s + self.duration_s
    }

    /// Envelope of the magnetic disturbance in `[0, 1]` at a time.
    pub fn magnetic_envelope_at(&self, t: f64) -> f64 {
        if t <= self.started_s || t >= self.started_s + self.magnetic_duration_s {
            return 0.0;
        }
        let tau = (t - self.started_s) / self.magnetic_duration_s;
        0.5 - 0.5 * (std::f64::consts::TAU * tau).cos()
    }

    /// Bias vector at a time.
    pub fn bias_at(&self, t: f64) -> DVec2 {
        let envelope = self.envelope_at(t);
        DVec2::new(self.direction_rad.cos(), self.direction_rad.sin()) * (self.magnitude * envelope)
    }

    /// Magnetic bias vector at a time, over the disturbance's own envelope.
    pub fn magnetic_bias_at(&self, t: f64) -> DVec2 {
        let envelope = self.magnetic_envelope_at(t);
        DVec2::new(self.direction_rad.cos(), self.direction_rad.sin()) * (self.magnitude * envelope)
    }
}

/// Per-region bookkeeping of entries.
#[derive(Debug, Clone, Copy, Default)]
struct RegionState {
    inside: bool,
    entry_index: u32,
}

/// Schedules region events along a run.
#[derive(Debug, Clone, Default)]
pub struct EventScheduler {
    states: HashMap<usize, RegionState>,
    active: Vec<ActiveEvent>,
    /// Every event that fired, in order, kept for the output manifest.
    history: Vec<ActiveEvent>,
}

impl EventScheduler {
    /// Creates an empty scheduler.
    pub fn new() -> Self {
        Self::default()
    }

    /// Advances the scheduler to a time at a position.
    ///
    /// `probability` and `duration_s` come from the region; `duration_s` is
    /// clamped into the range the design gives for the event type.
    #[allow(clippy::too_many_arguments)]
    pub fn update(
        &mut self,
        time_s: f64,
        position: DVec2,
        regions: &RegionSet,
        seed: u64,
        individual: u32,
        rng: &mut Rng,
        force_deterministic: bool,
    ) {
        let inside_now: Vec<usize> = regions.regions_at(position.x, position.y);

        // Entries: a region that was not entered before.
        for index in &inside_now {
            let state = self.states.entry(*index).or_default();
            if !state.inside {
                state.inside = true;
                let entry_index = state.entry_index;
                state.entry_index = state.entry_index.wrapping_add(1);
                let feature = regions.features()[*index];
                if should_trigger(
                    feature,
                    seed,
                    individual,
                    entry_index,
                    rng,
                    force_deterministic,
                ) {
                    let event = self.make_event(
                        *index,
                        feature,
                        time_s,
                        entry_index,
                        seed,
                        individual,
                        rng,
                    );
                    self.history.push(event);
                    self.active.push(event);
                }
            }
        }
        // Exits.
        let inside_set: std::collections::HashSet<usize> = inside_now.into_iter().collect();
        for (index, state) in self.states.iter_mut() {
            if !inside_set.contains(index) {
                state.inside = false;
            }
        }

        // Drop finished events.
        self.active.retain(|event| event.is_active(time_s));
    }

    /// Combined bias of every active event at a time.
    pub fn bias_at(&self, time_s: f64) -> DVec2 {
        self.active
            .iter()
            .map(|event| event.bias_at(time_s))
            .fold(DVec2::ZERO, |acc, value| acc + value)
    }

    /// Magnetic bias at a time: the same events, over their disturbance envelope.
    ///
    /// A magnetic pulse is far shorter than the multipath burst the same region
    /// triggers, so it needs its own envelope; summing the two would make every
    /// disturbance as long as a multipath event.
    pub fn magnetic_bias_at(&self, time_s: f64) -> DVec2 {
        self.active
            .iter()
            .map(|event| event.magnetic_bias_at(time_s))
            .fold(DVec2::ZERO, |acc, value| acc + value)
    }

    /// Sum of the envelopes of active events, in `[0, n]`.
    pub fn activity_at(&self, time_s: f64) -> f64 {
        self.active
            .iter()
            .map(|event| event.envelope_at(time_s))
            .sum()
    }

    /// Events currently in progress.
    pub fn active_events(&self) -> &[ActiveEvent] {
        &self.active
    }

    /// Every event that has fired so far.
    pub fn history(&self) -> &[ActiveEvent] {
        &self.history
    }

    /// True when an event is in progress.
    pub fn is_active(&self) -> bool {
        !self.active.is_empty()
    }

    #[allow(clippy::too_many_arguments)]
    fn make_event(
        &self,
        region_index: usize,
        feature: RegionFeature,
        time_s: f64,
        entry_index: u32,
        seed: u64,
        individual: u32,
        rng: &mut Rng,
    ) -> ActiveEvent {
        // The deterministic mode must not consume the shared random stream in a
        // way that depends on how many events fired before, so it draws its own
        // numbers from a hash of the entry.
        let (direction_rad, magnitude_scale) = match feature.mp_mode {
            TriggerMode::SpatialDeterministic => {
                let event = ourealis_map_format::region::spatial_event(
                    seed ^ (individual as u64) << 17,
                    feature.tag_id,
                    entry_index,
                    feature.p_mp,
                );
                (event.direction_rad as f64, event.magnitude as f64)
            }
            TriggerMode::Probabilistic => (
                rng.uniform_range(0.0, std::f64::consts::TAU),
                rng.uniform_range(0.5, 1.0),
            ),
        };
        ActiveEvent {
            region_index,
            started_s: time_s,
            duration_s: rng.uniform_range(10.0, 60.0),
            magnetic_duration_s: rng.uniform_range(1.0, 10.0),
            direction_rad,
            magnitude: feature.mp_bias_m as f64 * magnitude_scale,
            entry_index,
        }
    }
}

/// Trigger decision for one region entry.
///
/// `force_deterministic` overrides the mode the map declares. Calibration and
/// regression runs need it: the design requires the same parameters to produce the
/// same sensor stream, and a map whose regions declare the probabilistic default
/// would otherwise inject variation that the optimiser would chase as though it
/// were part of its objective.
fn should_trigger(
    feature: RegionFeature,
    seed: u64,
    individual: u32,
    entry_index: u32,
    rng: &mut Rng,
    force_deterministic: bool,
) -> bool {
    match feature.mp_mode {
        TriggerMode::SpatialDeterministic => {
            ourealis_map_format::region::spatial_event(
                seed ^ (individual as u64) << 17,
                feature.tag_id,
                entry_index,
                feature.p_mp,
            )
            .triggered
        }
        TriggerMode::Probabilistic if !force_deterministic => rng.chance(feature.p_mp as f64),
        // A forced run still has to consume the draw, or the deterministic and
        // forced modes would diverge in every stream that shares this generator.
        TriggerMode::Probabilistic => {
            let _ = rng.chance(feature.p_mp as f64);
            ourealis_map_format::region::spatial_event(
                seed ^ (individual as u64) << 17,
                feature.tag_id,
                entry_index,
                feature.p_mp,
            )
            .triggered
        }
    }
}
