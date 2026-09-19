//! GNSS positions, velocities and dropouts.
//!
//! The error budget is the design's: a slow Ornstein–Uhlenbeck bias, white
//! noise, event-driven multipath and regional dropouts. One subtlety drives the
//! implementation: **velocity must be correlated with position**. A receiver
//! derives Doppler velocity from the same signals that produce the position fix,
//! so a slow position bias appears in the velocity as well. Generating the two
//! independently would leave a downstream filter testing position-velocity
//! innovations against data whose real correlation is missing. When correlation
//! is enabled, velocity is therefore differentiated from the *noisy* positions
//! and only the high-frequency remainder is added as independent white noise.

use glam::DVec2;

use serde::{Deserialize, Serialize};

use crate::math::LocalFrame;
use crate::noise::{OuParams, OuProcess, OuVector2};

/// Ratio of vertical to horizontal GNSS error.
pub const VERTICAL_ERROR_RATIO: f64 = 2.0;
use crate::rng::{Rng, Stream};

/// One GNSS fix.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct GnssSample {
    /// Time since the start of the recording, seconds.
    pub time_s: f64,
    /// Latitude in degrees, or `None` when the map has no geographic reference.
    pub latitude_deg: Option<f64>,
    /// Longitude in degrees, or `None` when the map has no geographic reference.
    pub longitude_deg: Option<f64>,
    /// Local-plane position, metres.
    pub x: f64,
    /// Local-plane position, metres.
    pub y: f64,
    /// Altitude, metres.
    pub altitude_m: f64,
    /// Ground speed, m/s.
    pub speed_mps: f64,
    /// Course over ground, radians.
    pub heading_rad: f64,
    /// True when the fix survived the dropout model.
    pub valid: bool,
    /// Number of satellites, a plausible stand-in for a real fix.
    pub satellites: u8,
}

/// A stretch of missing fixes.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct GnssGap {
    /// Time of the first missing fix, seconds.
    pub from_s: f64,
    /// Time of the last missing fix, seconds.
    pub to_s: f64,
}

/// State of the GNSS error model, carried across samples.
#[derive(Debug, Clone)]
pub struct GnssModel {
    bias: OuVector2,
    /// Vertical bias; a receiver estimates height from a much poorer satellite
    /// geometry than horizontal position, so this runs at roughly twice the
    /// horizontal spread.
    vertical_bias: OuProcess,
    rate_hz: f64,
    white_sigma_m: f64,
    vertical_white_sigma_m: f64,
    speed_sigma: f64,
    correlated_velocity: bool,
    /// Last emitted fix's time and its noise offset from the truth.
    last_position: Option<(f64, DVec2)>,
    /// Time the bias processes were last advanced at, seconds.
    last_step_s: Option<f64>,
    filtered_speed: f64,
}

impl GnssModel {
    /// Creates the model for one individual.
    pub fn new(
        rate_hz: f64,
        white_sigma_m: f64,
        bias_sigma_m: f64,
        bias_tau_s: f64,
        speed_sigma: f64,
        correlated_velocity: bool,
    ) -> Self {
        Self {
            bias: OuVector2::new(OuParams::centred(bias_sigma_m, bias_tau_s)),
            vertical_bias: OuProcess::new(OuParams::centred(
                bias_sigma_m * VERTICAL_ERROR_RATIO,
                bias_tau_s,
            )),
            rate_hz: rate_hz.max(1e-3),
            white_sigma_m,
            vertical_white_sigma_m: white_sigma_m * VERTICAL_ERROR_RATIO,
            speed_sigma,
            correlated_velocity,
            last_position: None,
            last_step_s: None,
            filtered_speed: 0.0,
        }
    }

    /// Builds a model from an individual's sensor parameters.
    pub fn for_person(params: &crate::person::SensorNoiseParams, rate_hz: f64) -> Self {
        Self::new(
            rate_hz,
            params.gnss_white_sigma_m,
            params.gnss_bias_sigma_m,
            params.gnss_bias_tau_s,
            params.gnss_speed_sigma,
            params.gnss_correlated_velocity,
        )
    }

    /// Applies the noise model to a truth position.
    ///
    /// `multipath` is the event bias for this instant, already in metres.
    pub fn observe(
        &mut self,
        time_s: f64,
        truth: DVec2,
        truth_speed: f64,
        truth_heading: f64,
        multipath: DVec2,
        rng: &mut Rng,
    ) -> (DVec2, f64, f64, f64) {
        // The step is the time since the last *emitted* fix, not the nominal
        // interval. A dropout does not stop the receiver's bias from wandering,
        // and the gap is elapsed time: stepping by the nominal interval would
        // freeze the bias through a tunnel and then differentiate a single step
        // across the whole gap, damping the correlated part of the velocity in
        // proportion to it.
        let dt = match self.last_step_s {
            Some(previous) => (time_s - previous).max(1e-6),
            None => 1.0 / self.rate_hz,
        };
        self.last_step_s = Some(time_s);
        let bias = self.bias.step(dt, rng);
        let vertical =
            self.vertical_bias.step(dt, rng) + rng.gaussian() * self.vertical_white_sigma_m;
        let white = [
            rng.gaussian() * self.white_sigma_m,
            rng.gaussian() * self.white_sigma_m,
        ];
        // The bias and white noise enter the signal the receiver differentiates;
        // the multipath term is added to the reported position afterwards. A
        // static multipath bias does not appear in Doppler velocity, and
        // differentiating it would report a 12 m bias as a 12 m/s speed spike.
        let observed = truth + DVec2::new(bias[0] + white[0], bias[1] + white[1]);
        let reported = observed + multipath;

        // Velocity: the *noise* is differentiated, not the whole position. A
        // receiver's speed comes from Doppler, so it is unbiased and signed;
        // differencing the full noisy position would instead report the magnitude
        // of the noise, which is positively biased whenever the noise is large
        // compared with the speed. Differentiating the slow bias is what creates
        // the position-velocity correlation downstream filters expect, and the
        // high-frequency part is added as independent white noise.
        let offset = observed - truth;
        // Only the slow bias is differentiated for the velocity: the white
        // component is independent between epochs, so differentiating it would add
        // noise without the correlation the design requires. The high-frequency
        // part is supplied by the receiver's own independent white noise instead.
        let bias_only = DVec2::new(bias[0], bias[1]);
        let _ = offset;
        let (speed, heading) = if self.correlated_velocity {
            let along = match self.last_position {
                Some((last_time, previous)) => {
                    let interval = (time_s - last_time).max(1e-6);
                    let delta = (bias_only - previous) / interval;
                    // Project onto the direction of travel to keep the estimate
                    // signed, then low-pass it.
                    let heading_vector = DVec2::new(truth_heading.cos(), truth_heading.sin());
                    self.filtered_speed + 0.5 * (delta.dot(heading_vector) - self.filtered_speed)
                }
                None => 0.0,
            };
            self.filtered_speed = along;
            (
                (truth_speed + along + rng.gaussian() * self.speed_sigma).max(0.0),
                truth_heading + rng.gaussian() * 0.01,
            )
        } else {
            (
                (truth_speed + rng.gaussian() * self.speed_sigma).max(0.0),
                truth_heading,
            )
        };

        self.last_position = Some((time_s, bias_only));
        (reported, speed, heading, vertical)
    }
}

/// Geographic coordinates of a local-plane position.
pub fn to_geographic(frame: Option<&LocalFrame>, position: DVec2) -> (Option<f64>, Option<f64>) {
    match frame {
        Some(frame) => {
            let (lon, lat) = frame.to_geo_degrees(position);
            (Some(lat), Some(lon))
        }
        None => (None, None),
    }
}

/// Satellite count that produces the configured error level, used for a
/// plausible metadata field.
pub fn satellites_for(noise_sigma_m: f64, rng: &mut Rng) -> u8 {
    let base = (14.0 - noise_sigma_m).clamp(4.0, 14.0);
    let value = base + rng.normal(0.0, 0.8);
    value.round().clamp(4.0, 14.0) as u8
}

/// RNG stream for GNSS noise.
pub fn stream(seed: u64, individual: u32, channel: u32) -> Rng {
    Rng::stream(seed, Stream::Gnss, individual, channel)
}
