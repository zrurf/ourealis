//! Motion generation: from a geometric path to a physical trajectory.
//!
//! This module is where the realism of the output is decided. The stages run in
//! a fixed order because each one constrains the next:
//!
//! 1. [`limits`] computes the speed ceiling from physiology, curvature, downhill
//!    braking and look-ahead grade;
//! 2. [`pace`] and [`profile`] turn that ceiling into a timed speed profile with
//!    acceleration feasibility, fatigue and pacing strategy;
//! 3. [`offset`] injects the lateral habit and re-derives the *effective*
//!    curvature the trajectory actually has;
//! 4. [`attitude`] and [`bounce`] build the body frame and the vertical motion
//!    that the inertial sensors measure;
//! 5. [`maneuvers`] adds the standing and turning behaviour that a continuous
//!    speed profile cannot express.
//!
//! The output is a dense sample sequence at a single, fixed rate. Every sensor
//! derives from it, which is what keeps GNSS, IMU, magnetometer and barometer
//! consistent with one another.

pub mod attitude;
pub mod bounce;
pub mod limits;
pub mod maneuvers;
pub mod offset;
pub mod pace;
pub mod profile;

use glam::DVec2;

use crate::error::{CoreError, Result};
use crate::field::HardMask;
use crate::math::angle_of;
use crate::path::Path;
use crate::rng::{Rng, Stream};
use crate::terrain::{DistanceField, Terrain};

use super::person::PersonParams;

pub use attitude::{AttitudeConfig, AttitudeSample};
pub use bounce::BounceConfig;
pub use limits::{CURVATURE_DENOMINATOR_FLOOR, LookAheadMode, SpeedLimitParams};
pub use maneuvers::{Maneuver, ManeuverConfig, TurnProfile};
pub use offset::{OffsetConfig, OffsetSample};
pub use pace::{FatigueModel, IterationControl, PacingModel};
pub use profile::{LimitModifier, ProfileConfig, SpeedProfile, StopHold};

/// Configuration of the motion stage.
#[derive(Debug, Clone, PartialEq)]
pub struct MotionConfig {
    /// Speed limit parameters.
    pub limits: SpeedLimitParams,
    /// Speed profile parameters.
    pub profile: ProfileConfig,
    /// Lateral offset parameters.
    pub offset: OffsetConfig,
    /// Attitude parameters.
    pub attitude: AttitudeConfig,
    /// Maneuver parameters.
    pub maneuver: ManeuverConfig,
    /// Second-harmonic weight of the bounce *displacement* waveform.
    ///
    /// Calibrated to 0.049 from the reference recordings. The waveform's asymmetry
    /// is what the barometer sees, and it is also what the accelerometer's second
    /// harmonic comes from once the `k^2` amplification of a displacement harmonic
    /// is accounted for: `4 x 0.049 = 0.195`, against a measured ratio of 0.195. The
    /// design's placeholder was 0.3, which would put the accelerometer above 1.
    pub bounce_beta2: f64,
    /// Rate of the ground-truth samples, Hz. Sensors resample from this.
    pub sample_rate_hz: f64,
    /// Number of laps of the same path to run back to back.
    ///
    /// The laps share one continuous timeline: the noise processes keep evolving
    /// across the seam instead of restarting, which is what makes consecutive
    /// laps differ and the seam invisible.
    pub loop_laps: usize,
    /// Whether the individual's parameters overwrite the sub-configurations.
    ///
    /// Enabled by default, which is what most callers want. Turning it off gives
    /// full control of the motion configuration, at the cost of having to keep
    /// the individual's parameters consistent with it.
    pub adapt_individual: bool,
}

impl Default for MotionConfig {
    fn default() -> Self {
        Self {
            limits: SpeedLimitParams::default(),
            profile: ProfileConfig::default(),
            offset: OffsetConfig::default(),
            attitude: AttitudeConfig::default(),
            maneuver: ManeuverConfig::default(),
            bounce_beta2: 0.049,
            sample_rate_hz: 100.0,
            loop_laps: 1,
            adapt_individual: true,
        }
    }
}

impl MotionConfig {
    /// Applies individual parameters to the sub-configurations that depend on them.
    pub fn adapt_to(&mut self, person: &PersonParams) {
        if !self.adapt_individual {
            return;
        }
        self.limits.look_ahead_m = person.look_ahead_m;
        self.limits.a_lat_max = person.a_lat_max;
        self.limits.k_down = person.k_down;
        self.offset.mean_m = person.lateral_offset_mean;
        self.offset.std_m = person.lateral_offset_std;
        self.offset.tau_s = person.lateral_offset_tau_s;
        self.offset.a_lat_max = person.a_lat_max;
        self.attitude.lean_max_deg = person.lean_max_deg;
        self.attitude.head_look_ahead_s = person.head_look_ahead_s;
        self.maneuver.turn_omega_max = person.turn_omega_max;
        self.attitude.lean_max_deg = person.lean_max_deg;
    }
}

/// One ground-truth sample of the run.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TrajectorySample {
    /// Elapsed time since the start of the recording, seconds.
    pub time_s: f64,
    /// Arc length along the path, metres.
    pub arc_s: f64,
    /// Offset trajectory position, metres.
    pub position: DVec2,
    /// Path centre line position, metres.
    pub center: DVec2,
    /// Terrain elevation at the position, metres.
    pub terrain_z: f64,
    /// Bounce displacement, metres.
    pub bounce_z: f64,
    /// Ground-truth altitude `terrain_z + bounce_z`, metres.
    pub z: f64,
    /// Speed along the path, m/s.
    pub speed: f64,
    /// Body heading, radians.
    pub heading: f64,
    /// Raw tangent angle of the offset trajectory, radians.
    pub tangent_angle: f64,
    /// Head heading, radians; leads the body through turns.
    pub head_heading: f64,
    /// Pitch, radians.
    pub pitch: f64,
    /// Roll, radians.
    pub roll: f64,
    /// Centre-line curvature, per metre.
    pub kappa: f64,
    /// Effective curvature of the offset trajectory, per metre.
    pub kappa_eff: f64,
    /// Applied lateral offset, metres.
    pub offset_m: f64,
    /// Terrain grade along the direction of travel.
    pub grade: f64,
    /// True while the runner stands still.
    pub standing: bool,
    /// True while performing an on-the-spot turn.
    pub turning: bool,
}

impl TrajectorySample {
    /// True when the sample is a genuine motion sample (not standing or turning).
    pub fn is_moving(&self) -> bool {
        !self.standing && !self.turning
    }

    /// Horizontal speed, m/s.
    pub fn horizontal_speed(&self) -> f64 {
        self.speed
    }
}

/// Complete motion ground truth of one run.
#[derive(Debug, Clone)]
pub struct Trajectory {
    /// Dense ground-truth samples in time order.
    pub samples: Vec<TrajectorySample>,
    /// Geometric path the run follows.
    pub path: Path,
    /// Timed speed profile.
    pub profile: SpeedProfile,
    /// Lateral offset record.
    pub offsets: Vec<OffsetSample>,
    /// Scheduled maneuvers.
    pub maneuvers: Vec<Maneuver>,
    /// Bounce waveform of this individual.
    pub bounce: BounceConfig,
    /// Target speed of the individual, m/s.
    pub target_speed: f64,
    /// Step frequency of the individual, Hz.
    pub step_frequency: f64,
    /// True when the recording starts with a standing period.
    pub starts_standing: bool,
    /// Number of laps covered; greater than one only for loop sessions.
    pub laps: usize,
    /// Length of one lap, metres.
    pub lap_length_m: f64,
}

impl Trajectory {
    /// Builds the motion ground truth of a run.
    #[allow(clippy::too_many_arguments)]
    pub fn build(
        path: Path,
        terrain: &Terrain,
        hard: &HardMask,
        distance_field: &DistanceField,
        person: &PersonParams,
        config: &MotionConfig,
        seed: u64,
        individual: u32,
    ) -> Result<Self> {
        Self::build_with_backend(
            path,
            terrain,
            hard,
            distance_field,
            person,
            config,
            seed,
            individual,
            None,
        )
    }

    /// Builds the ground truth, querying the constraint mask through `backend`.
    ///
    /// The only stage that asks the environment a large batch of independent
    /// questions is the offset cap: every sample probes the legal limit along its own
    /// normal, which is a few hundred queries with nothing to order them. Handing
    /// them to a compute backend is what [`crate::gpu::ComputeBackend::projection_check_batch`]
    /// exists for. The algorithm is the same either way — the binary search is
    /// batched, not replaced — so the trajectory is identical whichever backend runs.
    #[allow(clippy::too_many_arguments)]
    pub fn build_with_backend(
        path: Path,
        terrain: &Terrain,
        hard: &HardMask,
        distance_field: &DistanceField,
        person: &PersonParams,
        config: &MotionConfig,
        seed: u64,
        individual: u32,
        backend: Option<&dyn crate::gpu::ComputeBackend>,
    ) -> Result<Self> {
        let mut config = config.clone();
        config.adapt_to(person);
        person.validate()?;

        // Pace drift: the intended pace wanders slowly, which is what makes two
        // laps differ and gives the speed residual its Ornstein-Uhlenbeck colour
        // rather than a deterministic trend. The process is indexed by elapsed
        // time, and the clock at each profile sample comes out of the profile, so
        // the profile is built once to read it and once with the drift it defines.
        let pace_drift = if person.pace_drift_sigma > 0.0 {
            let clock =
                SpeedProfile::build(&path, terrain, &config.limits, person, &config.profile, &[])?;
            generate_pace_drift(person, &clock.t, seed, individual)
        } else {
            Vec::new()
        };

        let profile = SpeedProfile::build(
            &path,
            terrain,
            &config.limits,
            person,
            &config.profile,
            &pace_drift,
        )?;

        let mut offset_rng = Rng::stream(seed, Stream::LateralOffset, individual, 0);
        let offsets = offset::generate(
            &path,
            &profile,
            terrain,
            hard,
            distance_field,
            &config.offset,
            &mut offset_rng,
        )?;

        let phase0 = {
            let mut rng = Rng::stream(seed, Stream::StepPhase, individual, 0);
            rng.uniform_range(0.0, std::f64::consts::TAU)
        };
        let bounce = BounceConfig::new(
            person.bounce_amplitude_m,
            person.step_frequency,
            config.bounce_beta2,
            phase0,
        );

        let mut maneuver_rng = Rng::stream(seed, Stream::Maneuver, individual, 0);
        let maneuvers = maneuvers::schedule(&path, &config.maneuver, &mut maneuver_rng);

        let laps = config.loop_laps.max(1);
        let lap_length = if laps > 1 {
            path.total_length() / laps as f64
        } else {
            path.total_length()
        };

        let samples = assemble_timeline(
            &path,
            terrain,
            hard,
            distance_field,
            &profile,
            &offsets,
            &maneuvers,
            &bounce,
            person,
            &config,
            backend,
        )?;

        let starts_standing = samples.first().map(|s| s.standing).unwrap_or(false);

        Ok(Self {
            samples,
            path,
            profile,
            offsets,
            maneuvers,
            bounce,
            target_speed: person.target_speed,
            step_frequency: person.step_frequency,
            starts_standing,
            laps: config.loop_laps.max(1),
            lap_length_m: lap_length,
        })
    }

    /// Duration of the recording, seconds.
    pub fn duration_s(&self) -> f64 {
        self.samples.last().map(|s| s.time_s).unwrap_or(0.0)
    }

    /// Number of samples.
    pub fn len(&self) -> usize {
        self.samples.len()
    }

    /// True when the trajectory has no sample.
    pub fn is_empty(&self) -> bool {
        self.samples.is_empty()
    }

    /// Sample closest to a time.
    pub fn sample_at(&self, time_s: f64) -> Option<&TrajectorySample> {
        if self.samples.is_empty() {
            return None;
        }
        let clamped = time_s.clamp(0.0, self.duration_s());
        let index = match self.samples.binary_search_by(|sample| {
            sample
                .time_s
                .partial_cmp(&clamped)
                .unwrap_or(std::cmp::Ordering::Equal)
        }) {
            Ok(index) => index,
            Err(index) => index.min(self.samples.len() - 1),
        };
        self.samples.get(index)
    }

    /// Highest speed reached, m/s.
    pub fn max_speed(&self) -> f64 {
        self.samples
            .iter()
            .map(|sample| sample.speed)
            .fold(0.0f64, f64::max)
    }

    /// Geometric length of the path the trajectory follows, metres.
    pub fn length_m(&self) -> f64 {
        self.path.total_length()
    }

    /// Ratio of the mean pace to the individual's target pace.
    ///
    /// Reported so that metrics comparing the realised step signature against the
    /// nominal bounce amplitude can scale the prediction to the pace actually run.
    pub fn bounce_amplitude_ratio(&self) -> f64 {
        let speeds: Vec<f64> = self
            .samples
            .iter()
            .filter(|sample| sample.is_moving() && sample.speed > 0.3)
            .map(|sample| sample.speed)
            .collect();
        if speeds.is_empty() {
            return 1.0;
        }
        let mean = speeds.iter().sum::<f64>() / speeds.len() as f64;
        self.bounce.pace_ratio(mean, self.target_speed)
    }

    /// True when the trajectory ends at a standstill.
    pub fn ends_standing(&self) -> bool {
        self.samples.last().map(|s| s.standing).unwrap_or(false)
    }

    /// Samples of a time window.
    pub fn window(&self, from_s: f64, to_s: f64) -> &[TrajectorySample] {
        let start = self
            .samples
            .partition_point(|sample| sample.time_s < from_s);
        let end = self.samples.partition_point(|sample| sample.time_s <= to_s);
        &self.samples[start..end]
    }
}

/// Upper bound on how long the walk may run, from a nominal speed.
///
/// Only used to bound the sample count; the real duration comes out of the
/// profile.
fn trajectory_length_estimate(path: &Path, total_length: f64) -> f64 {
    let _ = path;
    // Two seconds per metre at 0.5 m/s, which is slower than any configured run.
    total_length * 2.0
}

/// Draws the pace-drift series: one multiplicative factor per profile sample.
///
/// The process steps in *time*, so it needs the clock at each profile sample, and
/// that only exists once a profile has been built. Stepping it by a fixed one
/// second per sample — the profile is spaced in arc length, and a quarter-metre
/// sample is 76 ms at jogging speed — runs the process about thirteen times too
/// fast: a 60 s time constant behaves like 4.5 s, and the drift then correlates
/// over metres of path instead of minutes of running.
fn generate_pace_drift(
    person: &PersonParams,
    times: &[f64],
    seed: u64,
    individual: u32,
) -> Vec<f64> {
    if person.pace_drift_sigma <= 0.0 || times.is_empty() {
        return vec![1.0; times.len().max(1)];
    }
    let mut rng = Rng::stream(seed, Stream::PaceDrift, individual, 0);
    let mut process = crate::noise::OuProcess::new(crate::noise::OuParams::centred(
        person.pace_drift_sigma,
        person.pace_drift_tau_s,
    ));
    let mut out = Vec::with_capacity(times.len());
    let mut previous = 0.0f64;
    for time in times {
        let dt = (time - previous).max(0.0);
        previous = *time;
        out.push(1.0 + process.step(dt, &mut rng));
    }
    out
}

/// Builds the dense sample sequence from the profile, offsets and maneuvers.
#[allow(clippy::too_many_arguments)]
fn assemble_timeline(
    path: &Path,
    terrain: &Terrain,
    hard: &HardMask,
    distance_field: &DistanceField,
    profile: &SpeedProfile,
    offsets: &[OffsetSample],
    maneuvers: &[Maneuver],
    bounce: &BounceConfig,
    person: &PersonParams,
    config: &MotionConfig,
    backend: Option<&dyn crate::gpu::ComputeBackend>,
) -> Result<Vec<TrajectorySample>> {
    let dt = 1.0 / config.sample_rate_hz.max(1.0);
    let total_length = path.total_length();
    let mut samples: Vec<TrajectorySample> = Vec::new();
    let mut attitude_inputs: Vec<attitude::AttitudeInput> = Vec::new();
    let mut time = 0.0f64;

    // Standing start: the runner waits, then accelerates from rest.
    if let Some(Maneuver::StandStart {
        duration_s,
        position,
        heading,
    }) = maneuvers.first()
    {
        // The runner waits where the run will begin, lateral offset included;
        // standing on the bare centre line would put a step into the first
        // sample of the trajectory.
        let first_offset = offset_at(offsets, 0.0);
        let stand_position = *position + path.normal_at(0.0) * first_offset.offset_m;
        // A hold on the path start takes the same elevation the runner would
        // have there moving: the override when the path carries one, the terrain
        // otherwise.
        let stand_z = path
            .elevation_at(0.0)
            .unwrap_or_else(|| terrain.height_at(stand_position));
        let steps = (*duration_s / dt).round().max(1.0) as usize;
        for _ in 0..steps {
            samples.push(standing_sample(
                time,
                stand_position,
                *position,
                stand_z,
                *heading,
                first_offset.offset_m,
            ));
            attitude_inputs.push(attitude::AttitudeInput {
                time_s: time,
                tangent_angle: *heading,
                grade: 0.0,
                kappa_eff: 0.0,
                speed: 0.0,
                turning: false,
            });
            time += dt;
        }
    }

    // Motion phase: walk the path in time, inserting turn maneuvers.
    let mut pending_turns: Vec<&Maneuver> = maneuvers
        .iter()
        .filter(|maneuver| matches!(maneuver, Maneuver::Turn { .. }))
        .collect();
    pending_turns.sort_by(|a, b| {
        a.arc_s()
            .unwrap_or(f64::INFINITY)
            .partial_cmp(&b.arc_s().unwrap_or(f64::INFINITY))
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    let mut arc = 0.0f64;
    // The lateral offset is a displacement in the runner's own frame, so it
    // rotates with the body. A turn therefore sweeps it around the pivot instead
    // of leaving it on the old side.
    let mut turns: Vec<TurnRender> = Vec::new();
    // Stops are consumed in order: a zero-duration stop would otherwise be
    // re-detected on the next iteration, leaving the runner on it forever.
    let mut next_stop = 0usize;
    // The walk advances by at most one sample interval of arc length, so the
    // sample count is bounded by the duration; anything beyond a generous
    // multiple of that means the walk is stuck and is reported instead of
    // allocating without limit.
    let sample_budget =
        ((trajectory_length_estimate(path, total_length) / dt) as usize).max(16) * 8;
    while arc < total_length && samples.len() < sample_budget {
        // A turn holds the runner in place while the heading rotates.
        if let Some(Maneuver::Turn {
            arc_s: turn_arc,
            position: _nominal,
            exit_heading,
            profile: scheduled,
        }) = pending_turns.first().copied()
            && arc >= *turn_arc
        {
            // The profile holds a zero-speed point at the reversal, so the
            // runner arrives at a standstill.
            //
            // The turn rotates the *body* and the lateral offset is carried in
            // the body frame, so the offset rotates with it: the runner's centre
            // sweeps the arc it would sweep by pivoting about a point one offset
            // away. Holding the position instead and restoring the offset on the
            // far side of the turn is what produces a metre-scale jump at the
            // exit — twice the offset, in a single sample.
            let held = samples.last().copied();
            let heading_before = held
                .map(|sample| sample.heading)
                .unwrap_or_else(|| angle_of(path.tangent_at(arc)));
            // Rotate by what is left between the heading the runner actually
            // holds and the direction the path resumes in. Using the scheduled
            // window angle instead would repeat the part of the swing that the
            // approach to the reversal already covered.
            let remaining = crate::math::angle_difference(*exit_heading, heading_before);
            let turn = TurnProfile::solve(
                remaining,
                config.maneuver.turn_omega_max,
                config.maneuver.turn_alpha,
            )
            .unwrap_or(*scheduled);
            let turn_duration = turn.duration_s();
            let steps = (turn_duration / dt).ceil().max(1.0) as usize;
            let first = samples.len();
            let mut angles = Vec::with_capacity(steps + 1);
            for step in 0..=steps {
                let local = (step as f64 * dt).min(turn_duration);
                angles.push(turn.angle_at(local));
            }
            for angle in &angles {
                let heading = heading_before + angle;
                let turn_position = position_of(held, path, arc);
                let z = path
                    .elevation_at(arc)
                    .unwrap_or_else(|| terrain.height_at(turn_position));
                samples.push(turning_sample(time, arc, turn_position, z, heading));
                attitude_inputs.push(attitude::AttitudeInput {
                    time_s: time,
                    tangent_angle: heading,
                    grade: 0.0,
                    kappa_eff: 0.0,
                    speed: 0.0,
                    turning: true,
                });
                time += dt;
            }
            turns.push(TurnRender {
                base_index: first.saturating_sub(1),
                first,
                angles,
            });
            pending_turns.remove(0);
            // Resume where the runner stands; the deceleration before the
            // turn already brought the speed to nearly zero, so the loop
            // advances slowly here.
            continue;
        }

        let offset = offset_at(offsets, arc);
        let center = path.position_at(arc);
        let normal = path.normal_at(arc);
        let position = center + normal * offset.offset_m;
        let tangent = path.tangent_at(arc);
        // The path's elevation override, when it carries one, replaces the
        // terrain for both the height and the grade: on a stair the terrain's
        // answer describes the ground under the link, not the link.
        let grade = path
            .grade_at(arc)
            .unwrap_or_else(|| terrain.directional_slope_at(center, tangent));
        let terrain_z = path
            .elevation_at(arc)
            .unwrap_or_else(|| terrain.height_at(position));
        let speed = profile.speed_at(arc);
        let bounce_z = bounce.height_at(time, speed, person.target_speed);
        let heading = angle_of(tangent);

        samples.push(TrajectorySample {
            time_s: time,
            arc_s: arc,
            position,
            center,
            terrain_z,
            bounce_z,
            z: terrain_z + bounce_z,
            speed,
            heading,
            tangent_angle: heading,
            head_heading: heading,
            pitch: 0.0,
            roll: 0.0,
            kappa: offset.kappa,
            kappa_eff: offset.kappa_eff,
            offset_m: offset.offset_m,
            grade,
            standing: false,
            turning: false,
        });
        attitude_inputs.push(attitude::AttitudeInput {
            time_s: time,
            tangent_angle: heading,
            grade,
            kappa_eff: offset.kappa_eff,
            speed,
            turning: false,
        });

        // A stop is reached, not approached: the step below is clamped so the
        // runner lands exactly on the stop's arc, and the hold starts from the
        // sample just emitted there. Holding wherever the runner happened to be
        // within a window of the stop and then moving the arc onto it after the
        // hold is a position step of up to half a profile sample in one sample
        // interval — a metre per second of velocity in the differentiated truth
        // that the runner never had.
        if let Some(stop) = profile.stops.get(next_stop).copied()
            && (arc - stop.s).abs() <= 1e-9
        {
            if speed > 1.0 {
                tracing::debug!(
                    "stop at {:.2} m is being held at {:.2} m/s: the profile did not brake into it",
                    stop.s,
                    speed
                );
            }
            let dwell_steps = (stop.duration_s / dt).round().max(1.0) as usize;
            let last = samples.last().copied();
            if let Some(base) = last {
                for _ in 0..dwell_steps {
                    time += dt;
                    let mut held = base;
                    held.time_s = time;
                    held.standing = true;
                    held.speed = 0.0;
                    held.bounce_z = 0.0;
                    held.z = base.terrain_z;
                    samples.push(held);
                    attitude_inputs.push(attitude::AttitudeInput {
                        time_s: time,
                        tangent_angle: held.tangent_angle,
                        grade: held.grade,
                        kappa_eff: held.kappa_eff,
                        speed: 0.0,
                        turning: false,
                    });
                }
            }
            next_stop += 1;
        }

        // Advance by the distance covered in one sample. The quadratic term keeps
        // the arc and the clock consistent when the runner starts from rest: with
        // `v = 0` a purely linear step would stall and any nudge would move the
        // runner without consuming the matching time, which shows up as a
        // free acceleration no physical budget allows.
        //
        // Its coefficient is the acceleration the profile itself implies, `v dv/ds`,
        // not the individual's budget. A term that always adds distance makes the
        // runner advance `0.5 a dt^2` faster than their own speed says — a fifth of
        // a percent at racing speed, but the dominant term as they come to a stop,
        // where it turns the last few samples into a deceleration several times the
        // budget.
        // Never step past the end: overshooting would leave the last sample a
        // full step short of the finish and then append a standstill, which reads
        // as a braking spike no physical budget allows. The same clamp to the next
        // stop is what makes the arrival exact.
        let remaining = (total_length - arc).max(0.0);
        let probe = 0.5;
        let ahead = profile.speed_at(arc + probe);
        let along_acceleration = (ahead * ahead - speed * speed) / (2.0 * probe);
        let mut step = (speed * dt + 0.5 * along_acceleration * dt * dt)
            .max(0.0)
            .min(remaining);
        if let Some(stop) = profile.stops.get(next_stop)
            && stop.s > arc
        {
            step = step.min(stop.s - arc);
        }
        arc += step;
        time += dt;
    }

    // The last step is clamped to the remaining length, so a runner who stops at
    // the end lands *on* the end — but the sample carrying that position is the
    // one the loop would have emitted next, and the hold that follows then freezes
    // a runner still moving at a few centimetres per second. At 100 Hz that is a
    // deceleration of several metres per second squared in a single sample, well
    // outside the individual's budget. Emitting the arrival sample closes it.
    //
    // Only when the profile stops at the end: a periodic lap arrives at speed and
    // its seam is the path start, so appending a standstill there would break the
    // continuity the loop mode is built on.
    let stops_at_end = profile.speed_at(total_length) <= 1e-6;
    if stops_at_end
        && !config.profile.periodic
        && let Some(last) = samples.last()
        && total_length - last.arc_s > 1e-9
    {
        let center = path.position_at(total_length);
        let tangent = path.tangent_at(total_length);
        let heading = angle_of(tangent);
        let offset = offset_at(offsets, total_length);
        let normal = path.normal_at(total_length);
        let position = center + normal * offset.offset_m;
        let grade = path
            .grade_at(total_length)
            .unwrap_or_else(|| terrain.directional_slope_at(center, tangent));
        let terrain_z = path
            .elevation_at(total_length)
            .unwrap_or_else(|| terrain.height_at(position));
        samples.push(TrajectorySample {
            time_s: time,
            arc_s: total_length,
            position,
            center,
            terrain_z,
            bounce_z: 0.0,
            z: terrain_z,
            speed: 0.0,
            heading,
            tangent_angle: heading,
            head_heading: heading,
            pitch: 0.0,
            roll: 0.0,
            kappa: 0.0,
            kappa_eff: 0.0,
            offset_m: offset.offset_m,
            grade,
            // The runner has stopped: marking it as a hold is what makes the hold
            // that follows continue from the end of the path rather than from a
            // step short of it.
            standing: true,
            turning: false,
        });
        attitude_inputs.push(attitude::AttitudeInput {
            time_s: time,
            tangent_angle: heading,
            grade: 0.0,
            kappa_eff: 0.0,
            speed: 0.0,
            turning: false,
        });
        time += dt;
    }

    // Standing end.
    if let Some(Maneuver::StandEnd {
        duration_s,
        position,
        heading,
    }) = maneuvers
        .iter()
        .find(|maneuver| matches!(maneuver, Maneuver::StandEnd { .. }))
    {
        let last_offset = offset_at(offsets, total_length);
        let stand_position = *position + path.normal_at(total_length) * last_offset.offset_m;
        let steps = (*duration_s / dt).round().max(1.0) as usize;
        let z = path
            .elevation_at(total_length)
            .unwrap_or_else(|| terrain.height_at(stand_position));
        for _ in 0..steps {
            samples.push(standing_sample(
                time,
                stand_position,
                *position,
                z,
                *heading,
                last_offset.offset_m,
            ));
            attitude_inputs.push(attitude::AttitudeInput {
                time_s: time,
                tangent_angle: *heading,
                grade: 0.0,
                kappa_eff: 0.0,
                speed: 0.0,
                turning: false,
            });
            time += dt;
        }
    }

    if samples.is_empty() {
        return Err(CoreError::config("trajectory assembly produced no sample"));
    }

    // The tangent is recomputed from the positions actually visited rather than
    // from the path geometry: the trajectory is what the gyroscope has to agree
    // with, and the centre line is only the line it was derived from.
    //
    // What is differenced is the *centre* line, not the offset-perturbed position.
    // The offset is placed along this tangent's normal, so estimating the tangent
    // from the placed position closes a loop: a lateral drift tilts the tangent,
    // the tilted normal swings the same offset along the track, and that swing
    // tilts the tangent further. One pass does not converge the loop — measured on
    // a straight path, a 0.07 m/s lateral drift became ±16 m/s^2 of longitudinal
    // acceleration the runner never produced, twenty times their acceleration
    // budget, and all of it goes into the accelerometer.
    //
    // The estimate spans a window of a fraction of a stride instead of two
    // adjacent samples. A curve is represented by a polyline, so consecutive
    // positions differ in direction by the chord angle, and a one-sample
    // difference would turn those steps into yaw-rate spikes.
    const TANGENT_WINDOW_M: f64 = 0.5;
    let count = samples.len();
    for index in 0..count {
        if !samples[index].is_moving() {
            continue;
        }
        let arc = samples[index].arc_s;
        let mut back = index;
        while back > 0 {
            back -= 1;
            if samples[back].is_moving() && arc - samples[back].arc_s >= TANGENT_WINDOW_M {
                break;
            }
        }
        let mut forward = index;
        while forward + 1 < count {
            forward += 1;
            if samples[forward].is_moving() && samples[forward].arc_s - arc >= TANGENT_WINDOW_M {
                break;
            }
        }
        // A window that spans a standstill or a turn covers two different
        // directions, and its difference is meaningless — at a reversal the two
        // sides even coincide, leaving a near-zero vector. There, the heading is
        // estimated from one side only, which is the direction the runner is
        // actually facing.
        let spans_maneuver = samples[back..=forward]
            .iter()
            .any(|sample| !sample.is_moving());
        // Which side of the window carries the direction of travel depends on
        // where the reversal is. Behind the runner it means the heading changed
        // on the far side of it, and the estimate has to come from ahead, or the
        // body keeps facing the way it came: the lateral offset rides on the body
        // normal, so the attitude then swings through the turn a second time, a
        // metre of sideways sweep at running speed, one sample at a time. Ahead of
        // the runner the opposite holds, and the estimate comes from behind.
        let behind_turns = samples[back..=index].iter().any(|sample| sample.turning);
        let (from, to) = if behind_turns {
            (index, forward)
        } else if spans_maneuver {
            let previous = (0..index).rev().find(|other| samples[*other].is_moving());
            match previous {
                Some(previous) => (previous, index),
                None => (index, forward),
            }
        } else {
            (back, forward)
        };
        let delta = samples[to].center - samples[from].center;
        // A window whose two ends coincide cannot be differenced: the runner has
        // just resumed from a hold or a turn, so both ends are the same point. The
        // heading there is the one the runner was already facing — the adjacent
        // sample's, whether it was moving or turning — not the raw path tangent the
        // sample was pushed with. At a fold the path's own tangent points back the
        // way the runner came, so following it turns the body through most of a half
        // turn, and since the position carries the lateral offset along the body
        // normal, that swing alone moves the runner 0.15 m sideways in one sample
        // while their speed along the path is a centimetre per second.
        let carried = (delta.length() <= 1e-4)
            .then(|| {
                (0..index)
                    .rev()
                    .find(|other| !samples[*other].standing)
                    .map(|previous| samples[previous].tangent_angle)
            })
            .flatten();
        if let Some(angle) =
            carried.or_else(|| (delta.length() > 1e-4).then(|| delta.y.atan2(delta.x)))
        {
            samples[index].tangent_angle = angle;
            if let Some(input) = attitude_inputs.get_mut(index) {
                input.tangent_angle = angle;
            }
        }
    }

    // The turn rotations are applied now, against the arrival heading the
    // tangent pass finally produced, and with the body-frame offset swept around
    // the pivot so it ends on the far side of the turn without a step.
    apply_turns(&mut samples, &mut attitude_inputs, &turns, hard);

    // Attitude is built from the whole sequence at once, because the head
    // look-ahead needs future body headings.
    let attitudes = attitude::generate(&attitude_inputs, person.target_speed, &config.attitude);
    for (sample, attitude) in samples.iter_mut().zip(attitudes.iter()) {
        sample.heading = attitude.yaw;
        sample.head_heading = attitude.head_yaw;
        sample.pitch = attitude.pitch;
        sample.roll = attitude.roll;
    }

    // The offset is a displacement in the runner's own frame, so the placement
    // uses the body heading. It has to: the path's tangent is a property of a
    // polyline, and where the polyline folds the tangent — and with it the path
    // normal — reverses between two samples a couple of centimetres apart, while
    // the body heading is low-passed and does not. Placing on the path normal
    // would swing the runner across the centre line at every such fold.
    for sample in samples.iter_mut().filter(|sample| sample.is_moving()) {
        let normal = crate::math::left_normal(crate::math::dir_of(sample.heading));
        sample.position = sample.center + normal * sample.offset_m;
    }

    limit_offset_acceleration(
        &mut samples,
        dt,
        config.limits.a_lat_max,
        config.offset.smoothing_s,
    );

    // The offset was cleared at the profile's own spacing, which is roughly a
    // metre, while the trajectory is sampled far more finely and the map's grid
    // is coarser than either. A placement that is legal at one profile sample can
    // therefore be illegal between two of them, so the final positions are
    // checked here and the offset is pulled in where it has to be.
    cap_offsets(&mut samples, hard, distance_field, backend);
    freeze_stands(&mut samples);

    Ok(samples)
}

/// Freezes every standing hold onto the motion sample it continues from.
///
/// The holds are the standing starts, the dwells and the final stop, and the
/// design says the truth position does not move during them. Two things would
/// otherwise move it: the lateral offset is a body-frame displacement, so a hold
/// placed from the *path* normal sits somewhere else on the centre line wherever
/// the heading lags the path; and the attitude's heading low-pass keeps converging
/// for a moment after the runner stops, which drags the offset around by a couple
/// of millimetres. Both are removed by giving the whole hold the placement of the
/// sample it continues from, which also makes the transition into and out of the
/// hold continuous.
///
/// Only contiguous `standing` runs are touched: a turn maneuver also does not
/// translate but has its own swept placement.
fn freeze_stands(samples: &mut [TrajectorySample]) {
    let total = samples.len();
    let mut index = 0usize;
    while index < total {
        if !samples[index].standing {
            index += 1;
            continue;
        }
        let start = index;
        while index < total && samples[index].standing {
            index += 1;
        }
        // The sample before the hold, or — for a hold that opens the recording,
        // where there is none — the sample after it. Any non-hold sample serves:
        // a hold that follows a turn continues from a *turn* sample, and skipping
        // back to the last sample that was merely moving would place the hold with
        // the placement of a body facing the other way.
        let anchor_index = (0..start)
            .rev()
            .find(|candidate| !samples[*candidate].standing)
            .or_else(|| (index..total).find(|candidate| samples[*candidate].is_moving()));
        let Some(anchor_index) = anchor_index else {
            continue;
        };
        let anchor = samples[anchor_index];
        for sample in samples[start..index].iter_mut() {
            continue_from(sample, &anchor);
        }
    }
}

/// Copies the placement of the sample a hold continues from.
fn continue_from(sample: &mut TrajectorySample, anchor: &TrajectorySample) {
    sample.position = anchor.position;
    sample.center = anchor.center;
    sample.offset_m = anchor.offset_m;
    sample.heading = anchor.heading;
    sample.tangent_angle = anchor.tangent_angle;
    sample.kappa = anchor.kappa;
    sample.kappa_eff = anchor.kappa_eff;
}

/// Largest change of the lateral offset per metre of arc, m/m.
///
/// The cap has to be able to shrink quickly enough to respect a wall, but a
/// change of `s` per metre at a running speed of `v` is a lateral velocity of
/// `s v`; a tenth keeps that well inside the few tenths of a metre per second a
/// runner can move sideways.
const OFFSET_CAP_SLOPE: f64 = 0.1;

/// Bounds the lateral acceleration the offset asks for.
///
/// The offset is drawn at the profile's spacing and interpolated onto the
/// trajectory, and an Ornstein-Uhlenbeck process is continuous but not
/// differentiable: its velocity is white. Interpolating that onto samples an order of
/// magnitude closer together turns every knot into a step in the *rate*, which is a
/// lateral acceleration of tens of metres per second squared — several times the
/// individual's whole lateral budget — and the accelerometer reports every one of
/// them. The design's answer is a low-pass before the offset is applied; the filter
/// has to run here, at the rate the trajectory is actually sampled at, or it only
/// smooths the knots' *values* and leaves their steps in the rate.
///
/// Two exponential passes at the configured time constant, then two sweeps bounding
/// the second difference by the individual's lateral budget, in the same spirit as
/// the speed profile's sweeps bounding longitudinal acceleration. Both stages only
/// move the runner further from the centre line at worst, never through a wall — and
/// where they and the environment disagree the environment has the final say, since
/// [`cap_offsets`] runs afterwards.
fn limit_offset_acceleration(
    samples: &mut [TrajectorySample],
    dt: f64,
    a_lat_max: f64,
    smoothing_s: f64,
) {
    let count = samples.len();
    if count < 3 || dt <= 0.0 {
        return;
    }
    if smoothing_s > 0.0 {
        let alpha = (dt / (smoothing_s + dt)).clamp(0.0, 1.0);
        for _ in 0..2 {
            let mut value = samples[0].offset_m;
            for sample in samples
                .iter_mut()
                .skip(1)
                .filter(|sample| sample.is_moving())
            {
                value += (sample.offset_m - value) * alpha;
                sample.offset_m = value;
            }
        }
    }
    if a_lat_max > 0.0 {
        let bound = a_lat_max * dt * dt;
        for index in 2..count {
            let extrapolated = 2.0 * samples[index - 1].offset_m - samples[index - 2].offset_m;
            let value = samples[index].offset_m;
            samples[index].offset_m = value.clamp(extrapolated - bound, extrapolated + bound);
        }
        for index in (0..count - 2).rev() {
            let extrapolated = 2.0 * samples[index + 1].offset_m - samples[index + 2].offset_m;
            let value = samples[index].offset_m;
            samples[index].offset_m = value.clamp(extrapolated - bound, extrapolated + bound);
        }
    }
    for sample in samples.iter_mut().filter(|sample| sample.is_moving()) {
        let normal = crate::math::left_normal(crate::math::dir_of(sample.heading));
        sample.position = sample.center + normal * sample.offset_m;
    }
}

/// Bounds every sample's offset by the largest value that is legal there, with a
/// rate limit along the arc.
///
/// Clamping each sample on its own is enough to keep every point on passable
/// ground, but it steps the offset wherever the legal limit jumps — and it does
/// jump, because the limit is resolved against a grid cell boundary. Two sweeps
/// over the caps, backwards and then forwards, bound how fast the cap may change
/// along the arc while never exceeding the local limit: the correction starts
/// before the obstacle and recovers after it, which is the same forward-backward
/// idea the speed profile uses.
///
/// The arc does not advance during a turn, so the zero-length span between its
/// samples forces them to share one cap; the surrounding motion ramps to and from
/// it, and the turn keeps the continuity it was built for.
fn cap_offsets(
    samples: &mut [TrajectorySample],
    hard: &HardMask,
    distance_field: &DistanceField,
    backend: Option<&dyn crate::gpu::ComputeBackend>,
) {
    let count = samples.len();
    let mut limit: Vec<f64> = Vec::with_capacity(count);
    let mut cap: Vec<f64> = Vec::with_capacity(count);
    let mut probes: Vec<Probe> = Vec::with_capacity(count);
    // The limit is resolved along the normal the sample is actually placed on.
    let sample_normal =
        |sample: &TrajectorySample| crate::math::left_normal(crate::math::dir_of(sample.heading));
    for sample in samples.iter() {
        let magnitude = sample.offset_m.abs();
        probes.push(Probe {
            centre: sample.center,
            // The cap belongs to the side the runner is actually on: the offset is
            // signed, so probing the left normal unconditionally would certify the
            // opposite side of the path and let the placement keep an offset the
            // ground under it does not allow.
            normal: sample_normal(sample) * sample.offset_m.signum(),
            magnitude,
        });
        limit.push(magnitude);
        cap.push(magnitude);
    }

    // Whether each probe's centre is legal at all, and then the halvings. The order of
    // the probes is the order of the samples, so the answers index back one for one.
    let centres: Vec<DVec2> = probes.iter().map(|probe| probe.centre).collect();
    let forbidden_at_centre = forbidden_flags(hard, distance_field, backend, &centres);
    for (index, probe) in probes.iter_mut().enumerate() {
        if forbidden_at_centre[index] {
            // Even the centre line is unusable; the smoothing stage's projection owns
            // that problem, so no offset is permitted here.
            probe.magnitude = 0.0;
        }
    }

    // Twelve halvings, each one batched over every probe that is still narrowing. The
    // scalar version of this loop is what the code did before, and it converges to the
    // same number: the same probes are evaluated in the same order.
    const HALVINGS: usize = 12;
    let mut low: Vec<f64> = vec![0.0; count];
    let mut high: Vec<f64> = probes.iter().map(|probe| probe.magnitude).collect();
    for _ in 0..HALVINGS {
        let mids: Vec<f64> = low
            .iter()
            .zip(high.iter())
            .map(|(low, high)| 0.5 * (low + high))
            .collect();
        let points: Vec<DVec2> = probes
            .iter()
            .zip(mids.iter())
            .map(|(probe, mid)| probe.centre + probe.normal * *mid)
            .collect();
        let forbidden = forbidden_flags(hard, distance_field, backend, &points);
        for index in 0..count {
            if forbidden[index] {
                high[index] = mids[index];
            } else {
                low[index] = mids[index];
            }
        }
    }
    for index in 0..count {
        limit[index] = low[index];
        cap[index] = cap[index].min(limit[index]);
    }

    for _ in 0..2 {
        for index in (0..count.saturating_sub(1)).rev() {
            let span = (samples[index + 1].arc_s - samples[index].arc_s).abs();
            let reachable = cap[index + 1] + OFFSET_CAP_SLOPE * span;
            cap[index] = cap[index].min(reachable).min(limit[index]);
        }
        for index in 1..count {
            let span = (samples[index].arc_s - samples[index - 1].arc_s).abs();
            let reachable = cap[index - 1] + OFFSET_CAP_SLOPE * span;
            cap[index] = cap[index].min(reachable).min(limit[index]);
        }
    }

    for (index, sample) in samples.iter_mut().enumerate() {
        if cap[index] >= sample.offset_m.abs() {
            continue;
        }
        sample.offset_m = cap[index] * sample.offset_m.signum();
        sample.position = sample.center + sample_normal(sample) * sample.offset_m;
    }
}

/// One point's legality query: the centre, the direction to move along, and how far.
struct Probe {
    centre: DVec2,
    normal: DVec2,
    magnitude: f64,
}

/// Whether each point is on passable ground.
///
/// Two paths that must agree: the mask is read directly when there is no backend,
/// which keeps the CPU cost to one grid lookup per point, and through the compute
/// backend otherwise, where the whole batch is answered at once. Both answer the
/// same question — the cell containing the point, and whether its bit is set — so
/// the caller cannot tell which one ran.
fn forbidden_flags(
    hard: &HardMask,
    distance_field: &DistanceField,
    backend: Option<&dyn crate::gpu::ComputeBackend>,
    points: &[DVec2],
) -> Vec<bool> {
    let Some(backend) = backend else {
        return points
            .iter()
            .map(|point| hard.is_forbidden(*point))
            .collect();
    };
    match crate::gpu::projection_batch_from(
        hard.grid(),
        hard.mask(),
        |point| distance_field.distance_at(point),
        points,
    )
    .and_then(|batch| backend.projection_check_batch(&batch))
    {
        Ok(answers) => (0..points.len())
            .map(|index| answers.get(index).forbidden)
            .collect(),
        Err(error) => {
            // A backend that cannot answer is not a reason to produce a trajectory
            // with an unchecked offset; the mask is right here.
            tracing::warn!(
                "projection batch through {} failed ({error}); reading the constraint mask directly",
                backend.name()
            );
            points
                .iter()
                .map(|point| hard.is_forbidden(*point))
                .collect()
        }
    }
}

/// Position of the last sample, or a path position when there is none yet.
fn position_of(held: Option<TrajectorySample>, path: &Path, arc: f64) -> DVec2 {
    held.map(|sample| sample.position)
        .unwrap_or_else(|| path.position_at(arc))
}

/// A turn whose absolute headings can only be fixed after the tangent pass.
///
/// The rotation is *relative* to the heading the runner holds on arrival, and the
/// tangent pass may adjust that heading (it is re-derived from the sampled
/// positions). Applying the rotation afterwards keeps the entry, the rotation and
/// the exit continuous with whatever heading the runner actually has.
struct TurnRender {
    /// Sample the rotation is anchored to.
    base_index: usize,
    /// Index of the first turn sample.
    first: usize,
    /// Rotation applied at each turn sample, relative to the arrival heading.
    angles: Vec<f64>,
}

/// Applies the turn rotations and sweeps the body-frame offset around the pivot.
///
/// The sample following the last turn sample is set to the rotation's end, so it
/// continues the turn rather than snapping back to the path tangent — which at a
/// reversal is the average of the two directions and points nowhere useful.
///
/// The offset magnitude is ramped from the arrival value down to the largest
/// value that is legal in every direction the body sweeps through, and back up to
/// the arrival value. The offset was cleared against the environment along the
/// path normal, and a pivot turns the body through directions that check never
/// saw; ramping keeps the entry and the exit at the value the surrounding samples
/// use, so neither end steps.
fn apply_turns(
    samples: &mut [TrajectorySample],
    attitude_inputs: &mut [attitude::AttitudeInput],
    turns: &[TurnRender],
    hard: &HardMask,
) {
    let legal_limit = |centre: DVec2, normal: DVec2, magnitude: f64| -> f64 {
        let is_legal = |limit: f64| !hard.is_forbidden(centre + normal * limit);
        if !is_legal(0.0) {
            return 0.0;
        }
        let (mut low, mut high) = (0.0f64, magnitude);
        for _ in 0..12 {
            let middle = 0.5 * (low + high);
            if is_legal(middle) {
                low = middle;
            } else {
                high = middle;
            }
        }
        low
    };

    for render in turns {
        if render.base_index >= render.first || render.angles.is_empty() {
            continue;
        }
        let base = samples[render.base_index];
        // The offset is a body-frame displacement, so it rotates with the body.
        let centre = base.center;
        let offset = base.offset_m;
        let magnitude = offset.abs();

        // Largest magnitude legal in every direction the body passes through.
        let mut floor = magnitude;
        for angle in &render.angles {
            let heading = base.tangent_angle + angle;
            let normal = crate::math::left_normal(crate::math::dir_of(heading));
            floor = floor.min(legal_limit(centre, normal, magnitude));
        }

        // A symmetric dip: the full offset at both ends of the turn, the legal
        // floor at its middle, where the body faces furthest from the approach.
        let span = render.angles.len().saturating_sub(1).max(1) as f64;
        let mut heading = base.tangent_angle;
        let mut applied = offset;
        for (step, angle) in render.angles.iter().enumerate() {
            let index = render.first + step;
            if index >= samples.len() {
                break;
            }
            heading = base.tangent_angle + angle;
            let normal = crate::math::left_normal(crate::math::dir_of(heading));
            let shape = 1.0 - (2.0 * step as f64 / span - 1.0).abs();
            let wanted = magnitude + (floor - magnitude) * shape;
            applied = legal_limit(centre, normal, wanted).min(wanted) * offset.signum();
            let sample = &mut samples[index];
            sample.heading = heading;
            sample.tangent_angle = heading;
            sample.center = centre;
            sample.offset_m = applied;
            sample.position = centre + normal * applied;
            sample.terrain_z = sample.z - sample.bounce_z;
            if let Some(input) = attitude_inputs.get_mut(index) {
                input.tangent_angle = heading;
            }
        }
        // The sample that resumes the motion continues the rotation, and carries
        // the offset the turn ended on: the placement pass below would otherwise
        // restore the unclamped value and step sideways at the exit.
        let exit = render.first + render.angles.len();
        if let Some(sample) = samples.get_mut(exit) {
            sample.heading = heading;
            sample.tangent_angle = heading;
            sample.offset_m = applied;
            if let Some(input) = attitude_inputs.get_mut(exit) {
                input.tangent_angle = heading;
            }
        }
    }
}

/// Offset sample at an arc length.
fn offset_at(offsets: &[OffsetSample], arc: f64) -> OffsetSample {
    if offsets.is_empty() {
        return OffsetSample {
            s: arc,
            t: 0.0,
            offset_m: 0.0,
            requested_m: 0.0,
            kappa: 0.0,
            kappa_eff: 0.0,
            clamped_by_curvature: false,
            clamped_by_environment: false,
        };
    }
    let index = match offsets.binary_search_by(|sample| {
        sample
            .s
            .partial_cmp(&arc)
            .unwrap_or(std::cmp::Ordering::Equal)
    }) {
        Ok(index) => index,
        Err(index) => index.saturating_sub(1),
    };
    let index = index.min(offsets.len() - 1);
    let current = offsets[index];
    // Interpolate between the profile samples: holding the offset constant would
    // make the trajectory a staircase, and every step would appear in the
    // gyroscope as a spike.
    let next = offsets.get(index + 1);
    let Some(next) = next else {
        return current;
    };
    let span = next.s - current.s;
    if span <= 1e-9 {
        return current;
    }
    let t = ((arc - current.s) / span).clamp(0.0, 1.0);
    OffsetSample {
        s: arc,
        t: current.t + (next.t - current.t) * t,
        offset_m: current.offset_m + (next.offset_m - current.offset_m) * t,
        requested_m: current.requested_m + (next.requested_m - current.requested_m) * t,
        kappa: current.kappa + (next.kappa - current.kappa) * t,
        kappa_eff: current.kappa_eff + (next.kappa_eff - current.kappa_eff) * t,
        clamped_by_curvature: current.clamped_by_curvature || next.clamped_by_curvature,
        clamped_by_environment: current.clamped_by_environment || next.clamped_by_environment,
    }
}

fn standing_sample(
    time: f64,
    position: DVec2,
    center: DVec2,
    z: f64,
    heading: f64,
    offset_m: f64,
) -> TrajectorySample {
    TrajectorySample {
        time_s: time,
        arc_s: 0.0,
        position,
        center,
        terrain_z: z,
        bounce_z: 0.0,
        z,
        speed: 0.0,
        heading,
        tangent_angle: heading,
        head_heading: heading,
        pitch: 0.0,
        roll: 0.0,
        kappa: 0.0,
        kappa_eff: 0.0,
        offset_m,
        grade: 0.0,
        standing: true,
        turning: false,
    }
}

/// A sample of an on-the-spot turn.
///
/// The arc length is the position of the reversal, not zero: the lap and
/// progress bookkeeping indexes samples by arc length, and attributing a turn to
/// the start of the path would fold a late maneuver into the first lap.
fn turning_sample(
    time: f64,
    arc_s: f64,
    position: DVec2,
    z: f64,
    heading: f64,
) -> TrajectorySample {
    TrajectorySample {
        time_s: time,
        arc_s,
        position,
        center: position,
        terrain_z: z,
        bounce_z: 0.0,
        z,
        speed: 0.0,
        heading,
        tangent_angle: heading,
        head_heading: heading,
        pitch: 0.0,
        roll: 0.0,
        kappa: 0.0,
        kappa_eff: 0.0,
        offset_m: 0.0,
        grade: 0.0,
        standing: false,
        turning: true,
    }
}
