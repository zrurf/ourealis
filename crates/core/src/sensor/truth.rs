//! Truth state used by every sensor model.
//!
//! Two distinct motions are carried side by side, and the distinction is load
//! bearing:
//!
//! * **low-frequency centre of mass** — the position, velocity and acceleration
//!   of the smoothed centre-of-mass path. The accelerometer measures *this*,
//!   which is what the design's accelerometer model means by "p is the
//!   low-frequency centre-of-mass trajectory, excluding bounce";
//! * **reported truth position** — the same motion plus the high-frequency
//!   position jitter and the vertical bounce. This is what the output trajectory
//!   and the GNSS input use.
//!
//! Differentiating the jitter at 100 Hz would produce metres per second squared
//! of pure noise that swamps the real acceleration signature, so it never enters
//! the inertial path.

use glam::DVec2;

use crate::motion::Trajectory;

/// Truth quantities at one inertial sample.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TruthState {
    /// Time, seconds.
    pub time_s: f64,
    /// Reported truth position including jitter and bounce, metres.
    pub position: DVec2,
    /// Low-frequency centre-of-mass position, metres.
    pub position_low: DVec2,
    /// Reported altitude including bounce, metres.
    pub z: f64,
    /// Terrain elevation, metres.
    pub terrain_z: f64,
    /// Low-frequency velocity in the world frame, m/s.
    pub velocity: [f64; 3],
    /// Low-frequency acceleration in the world frame, m/s^2.
    pub acceleration: [f64; 3],
    /// Speed along the path, m/s.
    pub speed: f64,
    /// Body heading, radians.
    pub heading: f64,
    /// Head heading, radians.
    pub head_heading: f64,
    /// Pitch, radians.
    pub pitch: f64,
    /// Roll, radians.
    pub roll: f64,
    /// Effective curvature, per metre.
    pub kappa_eff: f64,
    /// Lateral offset from the path centre line, metres.
    pub offset_m: f64,
    /// Terrain grade along the direction of travel, rise over run.
    pub grade: f64,
    /// True while standing.
    pub standing: bool,
    /// True while turning on the spot.
    pub turning: bool,
    /// True when the sample comes from a stop; the jitter may be present but the
    /// motion is otherwise frozen.
    pub bounce_phase_source: f64,
}

/// Builds the truth sequence of a trajectory.
///
/// Velocities and accelerations are central differences of the *low-frequency*
/// position, which is smooth by construction, so the finite difference is
/// accurate rather than noisy.
pub fn build_states(
    trajectory: &Trajectory,
    jitter_sigma_m: f64,
    jitter: &[[f64; 2]],
) -> Vec<TruthState> {
    let samples = &trajectory.samples;
    let n = samples.len();
    let mut out = Vec::with_capacity(n);
    if n == 0 {
        return out;
    }

    let low_position = |index: usize| -> [f64; 3] {
        let sample = &samples[index];
        [sample.position.x, sample.position.y, sample.terrain_z]
    };
    let times: Vec<f64> = samples.iter().map(|sample| sample.time_s).collect();
    let accelerations = second_differences(&times, low_position);

    for index in 0..n {
        let sample = &samples[index];
        let previous = index.saturating_sub(1);
        let next = (index + 1).min(n - 1);
        let dt = (samples[next].time_s - samples[previous].time_s).max(1e-6);

        let (velocity, acceleration) = if next == previous {
            ([0.0; 3], [0.0; 3])
        } else {
            let a = low_position(previous);
            let b = low_position(next);
            let velocity = [(b[0] - a[0]) / dt, (b[1] - a[1]) / dt, (b[2] - a[2]) / dt];
            (velocity, accelerations[index])
        };

        let jitter_offset = jitter.get(index).copied().unwrap_or([0.0, 0.0]);
        out.push(TruthState {
            time_s: sample.time_s,
            position: sample.position + DVec2::new(jitter_offset[0], jitter_offset[1]),
            position_low: sample.position,
            z: sample.z,
            terrain_z: sample.terrain_z,
            velocity,
            acceleration,
            speed: sample.speed,
            heading: sample.heading,
            head_heading: sample.head_heading,
            pitch: sample.pitch,
            roll: sample.roll,
            kappa_eff: sample.kappa_eff,
            offset_m: sample.offset_m,
            grade: sample.grade,
            standing: sample.standing,
            turning: sample.turning,
            bounce_phase_source: sample.bounce_z,
        });
    }
    let _ = jitter_sigma_m;
    out
}

/// Second derivative of a sampled vector quantity, per component.
///
/// The samples are not equally spaced in time: the profile is uniform in arc
/// length, so a runner accelerating from rest passes a sample every few
/// milliseconds. Two consequences have to be handled explicitly:
///
/// * the two one-sided velocity estimates around a sample are separated by
///   `(dt_prev + dt_next) / 2`, not by the full span, so dividing their
///   difference by the span halves every acceleration;
/// * the ends have no neighbour on one side, and a one-sided difference over a
///   zero-length interval reads the runner's own speed as an acceleration of
///   `v / dt` — hundreds of m/s^2 on the last sample of a run that ends while
///   still moving, which then enters the accelerometer.
///
/// Interior samples use the non-uniform central difference and the two ends a
/// one-sided three-point estimate, so a constant velocity gives zero
/// acceleration everywhere. Fewer than three samples support neither and report
/// zero.
fn second_differences(times: &[f64], value_at: impl Fn(usize) -> [f64; 3]) -> Vec<[f64; 3]> {
    let n = times.len();
    let mut out = vec![[0.0; 3]; n];
    if n < 3 {
        return out;
    }
    let one_sided = |i0: usize, i1: usize, i2: usize| -> [f64; 3] {
        let h1 = (times[i1] - times[i0]).max(1e-6);
        let h2 = (times[i2] - times[i1]).max(1e-6);
        let span = h1 + h2;
        let (a, b, c) = (value_at(i0), value_at(i1), value_at(i2));
        std::array::from_fn(|axis| {
            2.0 * (a[axis] / (h1 * span) - b[axis] / (h1 * h2) + c[axis] / (span * h2))
        })
    };
    out[0] = one_sided(0, 1, 2);
    out[n - 1] = one_sided(n - 3, n - 2, n - 1);
    for index in 1..n - 1 {
        let dt_prev = (times[index] - times[index - 1]).max(1e-6);
        let dt_next = (times[index + 1] - times[index]).max(1e-6);
        let (previous, current, next) = (value_at(index - 1), value_at(index), value_at(index + 1));
        out[index] = std::array::from_fn(|axis| {
            let v_prev = (current[axis] - previous[axis]) / dt_prev;
            let v_next = (next[axis] - current[axis]) / dt_next;
            2.0 * (v_next - v_prev) / (dt_prev + dt_next)
        });
    }
    out
}

/// Interpolates the truth state closest to a time.
pub fn state_at(states: &[TruthState], time_s: f64) -> Option<&TruthState> {
    if states.is_empty() {
        return None;
    }
    let clamped = time_s.clamp(states[0].time_s, states[states.len() - 1].time_s);
    let index = match states.binary_search_by(|state| {
        state
            .time_s
            .partial_cmp(&clamped)
            .unwrap_or(std::cmp::Ordering::Equal)
    }) {
        Ok(index) => index,
        Err(index) => index.min(states.len() - 1),
    };
    states.get(index)
}

/// Specific force in the world frame: `f = a - g` with `g = (0, 0, -9.81)`.
///
/// This is the definition the accelerometer model uses. Writing it once, with
/// the sign convention attached, is what keeps the IMU from coming out with a
/// globally inverted sign.
pub fn specific_force(acceleration: [f64; 3]) -> [f64; 3] {
    [
        acceleration[0],
        acceleration[1],
        acceleration[2] + crate::math::GRAVITY,
    ]
}
