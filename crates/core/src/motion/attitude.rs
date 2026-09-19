//! Attitude ground truth.
//!
//! The attitude is what makes the inertial sensors physical rather than
//! decorative: the accelerometer measures specific force *in the body frame*,
//! so a missing pitch on a climb or a missing roll in a bend shows up as a
//! missing DC component on a horizontal axis, which is trivially detectable.
//!
//! Decomposition is ZYX (yaw, then pitch, then roll). Body yaw and head yaw are
//! separate: the head leads into a turn, the torso follows, and only the torso
//! defines the mounted device frame. Both are low-passed, because the raw
//! tangent angle steps at every path vertex and would otherwise appear in the
//! gyroscope as a non-physical pulse.

use glam::DMat3;

use crate::math::sampling::{low_pass_angle, unwrap_angle};

/// Configuration of the attitude model.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AttitudeConfig {
    /// Heading low-pass time constant, seconds.
    pub heading_tau_s: f64,
    /// Roll low-pass time constant, seconds.
    pub roll_tau_s: f64,
    /// Head look-ahead time, seconds.
    pub head_look_ahead_s: f64,
    /// Maximum forward lean at target speed, degrees.
    pub lean_max_deg: f64,
    /// Roll clamp, degrees.
    pub roll_limit_deg: f64,
}

impl Default for AttitudeConfig {
    fn default() -> Self {
        Self {
            heading_tau_s: 0.35,
            roll_tau_s: 0.5,
            head_look_ahead_s: 1.0,
            lean_max_deg: 3.0,
            roll_limit_deg: 10.0,
        }
    }
}

/// Attitude at one sample.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AttitudeSample {
    /// Body yaw, radians, unwrapped.
    pub yaw: f64,
    /// Head yaw, radians, unwrapped; leads the body into a turn.
    pub head_yaw: f64,
    /// Pitch, radians; positive means leaning forward on a climb.
    pub pitch: f64,
    /// Roll, radians; positive means leaning right, a right-hand rotation about
    /// the body forward axis. A left turn therefore carries a negative roll.
    pub roll: f64,
}

impl AttitudeSample {
    /// Rotation matrix `Rz(yaw) Ry(pitch) Rx(roll)`.
    ///
    /// This is the matrix that maps body-frame vectors into the world frame, so
    /// sensors convert with its transpose.
    pub fn rotation(&self) -> DMat3 {
        rotation_matrix(self.yaw, self.pitch, self.roll)
    }

    /// Body angular velocity implied by a sequence of attitudes, in rad/s.
    pub fn angular_velocity(
        previous: &AttitudeSample,
        current: &AttitudeSample,
        dt: f64,
    ) -> [f64; 3] {
        if dt <= 0.0 {
            return [0.0; 3];
        }
        [
            (current.roll - previous.roll) / dt,
            (current.pitch - previous.pitch) / dt,
            (current.yaw - previous.yaw) / dt,
        ]
    }
}

/// Builds the ZYX rotation matrix.
pub fn rotation_matrix(yaw: f64, pitch: f64, roll: f64) -> DMat3 {
    let (sy, cy) = yaw.sin_cos();
    let (sp, cp) = pitch.sin_cos();
    let (sr, cr) = roll.sin_cos();
    // Column-major: each column is a body axis expressed in world coordinates.
    DMat3::from_cols_array(&[
        cy * cp,
        sy * cp,
        -sp,
        cy * sp * sr - sy * cr,
        sy * sp * sr + cy * cr,
        cp * sr,
        cy * sp * cr + sy * sr,
        sy * sp * cr - cy * sr,
        cp * cr,
    ])
}

/// Inputs needed to build one attitude sample.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AttitudeInput {
    /// Time, seconds.
    pub time_s: f64,
    /// Raw tangent angle of the offset trajectory, radians.
    pub tangent_angle: f64,
    /// Terrain grade along the direction of travel.
    pub grade: f64,
    /// Effective curvature, per metre.
    pub kappa_eff: f64,
    /// Speed, m/s.
    pub speed: f64,
    /// Whether the sample belongs to an on-the-spot turn.
    pub turning: bool,
}

/// Builds an attitude sequence from trajectory inputs.
///
/// `target_speed` normalises the forward lean; `dt` is the sample interval and
/// is taken per sample from the inputs, so a non-uniform trajectory is handled
/// correctly.
pub fn generate(
    inputs: &[AttitudeInput],
    target_speed: f64,
    config: &AttitudeConfig,
) -> Vec<AttitudeSample> {
    let mut out = Vec::with_capacity(inputs.len());
    if inputs.is_empty() {
        return out;
    }
    let lean_max = config.lean_max_deg.to_radians();
    let roll_limit = config.roll_limit_deg.to_radians();

    let mut yaw = inputs[0].tangent_angle;
    let mut roll = 0.0;
    // Head yaw uses the body yaw of a later sample, which is why the sequence is
    // built in two passes.
    let mut body_yaws: Vec<f64> = Vec::with_capacity(inputs.len());

    for (index, input) in inputs.iter().enumerate() {
        let dt = if index == 0 {
            0.0
        } else {
            (input.time_s - inputs[index - 1].time_s).max(1e-6)
        };
        if index == 0 {
            yaw = unwrap_angle(yaw, input.tangent_angle);
        } else if input.turning || inputs[index - 1].turning {
            // A turn maneuver *is* a heading profile, so it is followed exactly
            // rather than filtered; filtering it would blunt the trapezoid the
            // gyroscope reports. What is followed is its *increment*: the
            // profile's absolute values are anchored to the raw path tangent of
            // the arrival sample, while `yaw` carries that sample's low-passed
            // heading, and adopting the absolute value would put the difference
            // into the gyroscope as a spike at the entry and again at the exit.
            yaw +=
                crate::math::angle_difference(input.tangent_angle, inputs[index - 1].tangent_angle);
        } else {
            yaw = low_pass_angle(yaw, input.tangent_angle, dt, config.heading_tau_s);
        }
        body_yaws.push(yaw);

        // Pitch: terrain baseline plus a speed-proportional forward lean.
        let lean = if target_speed > 1e-6 {
            lean_max * (input.speed / target_speed).clamp(0.0, 1.5)
        } else {
            0.0
        };
        let pitch = input.grade.atan() + lean;

        // Roll: centripetal lean, clamped and low-passed. The runner banks into
        // the turn, so a left turn (positive curvature, centre on the left)
        // needs a negative angle — the matrix rolls about the body forward axis
        // by the right-hand rule, which tilts the body to the right for a
        // positive angle.
        let target_roll =
            -(input.speed * input.speed * input.kappa_eff / crate::math::GRAVITY).atan();
        let target_roll = target_roll.clamp(-roll_limit, roll_limit);
        roll = if dt > 0.0 {
            crate::math::sampling::low_pass(roll, target_roll, dt, config.roll_tau_s)
        } else {
            target_roll
        };

        out.push(AttitudeSample {
            yaw,
            head_yaw: yaw,
            pitch,
            roll,
        });
    }

    // Second pass: the head looks ahead by sampling a future body yaw. This is
    // what produces the "look, then turn" pattern in a head-mounted gyroscope.
    let look_ahead_samples = {
        let dt = if inputs.len() > 1 {
            (inputs[1].time_s - inputs[0].time_s).max(1e-6)
        } else {
            1.0
        };
        (config.head_look_ahead_s / dt).round().max(0.0) as usize
    };
    if look_ahead_samples > 0 {
        for index in 0..out.len() {
            let future = body_yaws[(index + look_ahead_samples).min(body_yaws.len() - 1)];
            let dt = if index == 0 {
                (inputs[1.min(inputs.len() - 1)].time_s - inputs[0].time_s).max(1e-6)
            } else {
                (inputs[index].time_s - inputs[index - 1].time_s).max(1e-6)
            };
            out[index].head_yaw =
                low_pass_angle(out[index].head_yaw, future, dt, config.heading_tau_s);
        }
    }

    out
}

/// Convenience: attitude of a stationary runner facing `heading`.
pub fn standing(heading: f64) -> AttitudeSample {
    AttitudeSample {
        yaw: heading,
        head_yaw: heading,
        pitch: 0.0,
        roll: 0.0,
    }
}
