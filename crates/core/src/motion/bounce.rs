//! Vertical bounce of the centre of mass.
//!
//! Each step produces one vertical oscillation: a slow sink at contact and a
//! faster push-off into the flight phase. The bounce is part of the *ground
//! truth* rather than a sensor decoration, because three consumers derive from
//! it: the barometric altitude, the accelerometer's step harmonics — which share
//! its phase by construction — and the reported vertical coordinate.
//!
//! Keeping one phase reference for all three is what makes data consistent
//! enough for downstream algorithms that recover height by integrating
//! acceleration.

/// Shape parameters of the bounce waveform.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BounceConfig {
    /// Reference amplitude at the target speed, metres.
    pub amplitude_m: f64,
    /// Step frequency, Hz.
    pub step_frequency: f64,
    /// Second-harmonic weight of the waveform; makes the sink slower than the
    /// push-off and so does not average out.
    pub beta2: f64,
    /// Phase reference in radians, shared with the accelerometer harmonics.
    pub phase0: f64,
}

impl BounceConfig {
    /// Builds a configuration.
    ///
    /// The waveform is deliberately **not** peak-normalised: `amplitude_m` is the
    /// amplitude of its fundamental, so the vertical acceleration fundamental is
    /// exactly `A_b (2 pi f)^2`, which the accelerometer model depends on. The
    /// crest therefore sits near `0.72 A_b`; see [`BounceConfig::waveform_peak`].
    pub fn new(amplitude_m: f64, step_frequency: f64, beta2: f64, phase0: f64) -> Self {
        Self {
            amplitude_m,
            step_frequency,
            beta2,
            phase0,
        }
    }

    /// Largest excursion of the unit-amplitude waveform.
    ///
    /// The waveform is asymmetric by design: the sink after ground contact is
    /// deeper than the push-off crest, so the excursion is set by the trough and
    /// exceeds the amplitude.
    pub fn waveform_peak(&self) -> f64 {
        let mut peak = 0.0f64;
        let steps = 4096;
        for index in 0..steps {
            let u = std::f64::consts::TAU * index as f64 / steps as f64;
            peak = peak.max(self.waveform(u).abs());
        }
        peak
    }

    /// Zero-mean waveform whose fundamental has unit amplitude.
    pub fn waveform(&self, u: f64) -> f64 {
        u.sin() + self.beta2 * (2.0 * u + std::f64::consts::FRAC_PI_2).sin()
    }

    /// Second derivative of the waveform with respect to `u`.
    ///
    /// Used for the vertical acceleration, so the accelerometer sees the exact
    /// acceleration of the bounce truth instead of an approximation of it.
    pub fn waveform_second_derivative(&self, u: f64) -> f64 {
        -u.sin() - 4.0 * self.beta2 * (2.0 * u + std::f64::consts::FRAC_PI_2).sin()
    }

    /// Ratio of the pace actually run to the nominal target pace.
    ///
    /// The bounce amplitude scales with speed, so the amplitude reported in the
    /// configuration is an upper reference: the realised fundamental is smaller
    /// whenever the runner is below their target pace.
    pub fn pace_ratio(&self, speed: f64, target_speed: f64) -> f64 {
        if target_speed <= 1e-6 || self.amplitude_m <= 1e-9 {
            return 1.0;
        }
        self.amplitude_at(speed, target_speed) / self.amplitude_m
    }

    /// Angular frequency of the step cycle, rad/s.
    pub fn omega(&self) -> f64 {
        std::f64::consts::TAU * self.step_frequency
    }

    /// Bounce amplitude at a speed: it grows weakly with speed.
    pub fn amplitude_at(&self, speed: f64, target_speed: f64) -> f64 {
        let ratio = if target_speed > 1e-6 {
            (speed / target_speed).clamp(0.0, 1.5)
        } else {
            1.0
        };
        self.amplitude_m * ratio
    }

    /// Bounce height above the terrain at a time, in metres.
    pub fn height_at(&self, t: f64, speed: f64, target_speed: f64) -> f64 {
        let amplitude = self.amplitude_at(speed, target_speed);
        let phase = self.omega() * t + self.phase0;
        amplitude * self.waveform(phase)
    }

    /// Vertical acceleration produced by the bounce, in m/s^2.
    pub fn vertical_acceleration_at(&self, t: f64, speed: f64, target_speed: f64) -> f64 {
        let amplitude = self.amplitude_at(speed, target_speed);
        let omega = self.omega();
        let phase = omega * t + self.phase0;
        amplitude * omega * omega * self.waveform_second_derivative(phase)
    }

    /// Amplitude of the first harmonic of the vertical acceleration.
    ///
    /// The design locks this to `A_b * (2 pi f)^2`; the waveform's normalisation
    /// makes the realised value equal to that within a few percent, and the
    /// phase is exact because both come from the same waveform.
    pub fn first_harmonic_amplitude(&self, speed: f64, target_speed: f64) -> f64 {
        let amplitude = self.amplitude_at(speed, target_speed);
        let omega = self.omega();
        amplitude * omega * omega
    }

    /// Amplitude of harmonic `k >= 2`, scaled by the calibrated ratios.
    pub fn higher_harmonic_amplitude(
        &self,
        order: u32,
        speed: f64,
        target_speed: f64,
        ratio: f64,
    ) -> f64 {
        if order < 2 {
            return 0.0;
        }
        self.first_harmonic_amplitude(speed, target_speed) * ratio
    }
}
