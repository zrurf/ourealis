//! Deterministic random streams.
//!
//! Every stochastic quantity in the simulator draws from a stream derived from
//! `(global seed, purpose, individual index, channel index)`. Nothing uses a
//! thread-local generator, so a run is reproducible regardless of how the work
//! is spread across threads, and rerunning with the same seed reproduces the
//! output bit for bit.
//!
//! Gaussian samples come from a Box–Muller transform with a cached second
//! value. Implementing it here rather than pulling in a distribution crate keeps
//! the random stream under this crate's control: the mapping from a seed to a
//! sequence never changes with a dependency upgrade.

use rand::{Rng as _, RngExt, SeedableRng};
use rand_chacha::ChaCha8Rng;

/// Purpose tag mixed into a stream seed.
///
/// Distinct tags guarantee that two features using the same individual index
/// never share a stream by accident.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum Stream {
    /// Population sampling of individual parameters.
    Person = 1,
    /// PRM waypoint sampling.
    Prm = 2,
    /// Logit path choice.
    PathChoice = 3,
    /// Ornstein–Uhlenbeck pace drift.
    PaceDrift = 4,
    /// Ornstein–Uhlenbeck lateral offset.
    LateralOffset = 5,
    /// White position jitter.
    PositionJitter = 6,
    /// Step-frequency phase reference.
    StepPhase = 7,
    /// GNSS slow bias and white noise.
    Gnss = 8,
    /// Accelerometer bias and white noise.
    Accelerometer = 9,
    /// Gyroscope bias and white noise.
    Gyroscope = 10,
    /// Magnetometer bias and disturbance.
    Magnetometer = 11,
    /// Barometer noise and weather drift.
    Barometer = 12,
    /// Region event decisions in probabilistic mode.
    RegionEvent = 13,
    /// Multi-path events.
    Multipath = 14,
    /// Turn maneuver timing jitter.
    Maneuver = 15,
}

/// A seeded, reproducible random source.
#[derive(Debug, Clone)]
pub struct Rng {
    inner: ChaCha8Rng,
    spare_gaussian: Option<f64>,
}

impl Rng {
    /// Creates a stream from a fully specified key.
    ///
    /// The four components are folded in one at a time rather than packed into
    /// disjoint bit fields: a packed key needs every component to fit its slot,
    /// and `individual` and `channel` are both wider than the slots a 64-bit
    /// word can spare, so two different keys would otherwise collide.
    pub fn stream(seed: u64, stream: Stream, individual: u32, channel: u32) -> Self {
        let mut mixed = mix64(seed);
        mixed = mix64(mixed ^ (stream as u64).wrapping_mul(KEY_GOLDEN));
        mixed = mix64(mixed ^ (individual as u64).wrapping_mul(KEY_GOLDEN));
        mixed = mix64(mixed ^ (channel as u64).wrapping_mul(KEY_GOLDEN));
        Self {
            inner: ChaCha8Rng::seed_from_u64(mixed),
            spare_gaussian: None,
        }
    }

    /// Creates a stream from an explicit 64-bit seed.
    pub fn from_seed(seed: u64) -> Self {
        Self {
            inner: ChaCha8Rng::seed_from_u64(seed),
            spare_gaussian: None,
        }
    }

    /// Derives an independent stream from this one.
    pub fn derive(&mut self, tag: u64) -> Self {
        Self::from_seed(mix64(self.inner.next_u64() ^ tag))
    }

    /// Uniform sample in `[0, 1)`.
    #[inline]
    pub fn uniform(&mut self) -> f64 {
        self.inner.random::<f64>()
    }

    /// Uniform sample in `[low, high)`.
    #[inline]
    pub fn uniform_range(&mut self, low: f64, high: f64) -> f64 {
        low + (high - low) * self.uniform()
    }

    /// Uniform integer in `[low, high)`.
    #[inline]
    pub fn uniform_int(&mut self, low: i64, high: i64) -> i64 {
        if high <= low {
            return low;
        }
        let span = (high - low) as u64;
        low + (self.inner.random::<u64>() % span) as i64
    }

    /// Standard normal sample.
    #[inline]
    pub fn gaussian(&mut self) -> f64 {
        if let Some(spare) = self.spare_gaussian.take() {
            return spare;
        }
        // Box–Muller: two uniforms give two independent standard normals.
        let u1 = self.uniform().max(f64::MIN_POSITIVE);
        let u2 = self.uniform();
        let radius = (-2.0 * u1.ln()).sqrt();
        let angle = std::f64::consts::TAU * u2;
        self.spare_gaussian = Some(radius * angle.sin());
        radius * angle.cos()
    }

    /// Normal sample with the given mean and standard deviation.
    #[inline]
    pub fn normal(&mut self, mean: f64, std_dev: f64) -> f64 {
        mean + std_dev * self.gaussian()
    }

    /// True with probability `p`.
    #[inline]
    pub fn chance(&mut self, p: f64) -> bool {
        self.uniform() < p.clamp(0.0, 1.0)
    }

    /// Fills `out` with standard normal samples.
    pub fn fill_gaussian(&mut self, out: &mut [f64]) {
        for value in out.iter_mut() {
            *value = self.gaussian();
        }
    }

    /// Two normal values as a 2D vector.
    pub fn gaussian2(&mut self) -> [f64; 2] {
        [self.gaussian(), self.gaussian()]
    }
}

/// Odd multiplier used to separate the components of a stream key before
/// mixing, so that permuting two components cannot cancel out.
const KEY_GOLDEN: u64 = 0x9E37_79B9_7F4A_7C15;

/// SplitMix64 finaliser: mixes a counter into a well-distributed 64-bit value.
pub fn mix64(mut value: u64) -> u64 {
    value = value.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = value;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}
