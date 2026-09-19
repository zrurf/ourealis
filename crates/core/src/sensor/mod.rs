//! Sensor simulation.
//!
//! Every stream derives from one ground truth — the trajectory's truth states —
//! and each adds its own independent noise. That is what makes the outputs
//! mutually consistent: the accelerometer integrates to the same motion the GNSS
//! reports, the barometer tracks the same altitude the trajectory claims, and the
//! magnetometer rotates with the same attitude.
//!
//! The streams also share one time base. Sensor timestamps are computed as
//! `t0 + k / rate` rather than by accumulating an interval, so they are strictly
//! monotonic and never drift.

pub mod accel;
pub mod baro;
pub mod config;
pub mod gnss;
pub mod gyro;
pub mod mag;
pub mod truth;

use glam::DVec2;

use ourealis_map_format::region::RegionSet;
use ourealis_map_format::tlv::value::MagneticField;

use crate::error::Result;
use crate::math::LocalFrame;
use crate::motion::{BounceConfig, Trajectory};
use crate::noise::EventScheduler;
use crate::noise::harmonics::StepHarmonics;
use crate::person::PersonParams;
use crate::rng::{Rng, Stream};

pub use accel::ImuAccelSample;
pub use baro::BaroSample;
pub use config::{DeviceMount, SensorConfig};
pub use gnss::{GnssGap, GnssSample};
pub use gyro::ImuGyroSample;
pub use mag::MagSample;
pub use truth::{TruthState, state_at};

/// An inertial measurement unit pair, kept together because they share a rate.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ImuStream {
    /// Accelerometer samples.
    pub accel: Vec<ImuAccelSample>,
    /// Gyroscope samples.
    pub gyro: Vec<ImuGyroSample>,
}

/// Every simulated sensor stream of one run.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Sensors {
    /// GNSS fixes.
    pub gnss: Vec<GnssSample>,
    /// Intervals where GNSS produced no fix.
    pub gnss_gaps: Vec<GnssGap>,
    /// Inertial measurements.
    pub imu: ImuStream,
    /// Magnetometer samples.
    pub mag: Vec<MagSample>,
    /// Barometer samples.
    pub baro: Vec<BaroSample>,
    /// Mount the inertial unit was carried on.
    pub mount: DeviceMount,
    /// Number of multipath events that fired.
    pub multipath_events: usize,
    /// Number of magnetometer samples affected by a disturbance.
    pub magnetic_disturbed_samples: usize,
}

impl Sensors {
    /// Total number of samples across all streams.
    pub fn sample_count(&self) -> usize {
        self.gnss.len()
            + self.imu.accel.len()
            + self.imu.gyro.len()
            + self.mag.len()
            + self.baro.len()
    }

    /// Fraction of GNSS epochs that produced a fix.
    pub fn gnss_availability(&self) -> f64 {
        let total = self.gnss.len() + self.gnss_gaps.iter().map(gap_epochs).sum::<usize>();
        if total == 0 {
            1.0
        } else {
            self.gnss.len() as f64 / total as f64
        }
    }
}

/// Index of the truth sample carrying the value for `time_s`.
///
/// The per-sample event values are indexed by truth sample, and both sensor loops
/// need the same mapping, so it is computed once here instead of scanning the
/// whole sequence for every epoch.
fn sample_index_at(states: &[TruthState], time_s: f64) -> usize {
    let index = states.partition_point(|state| state.time_s < time_s);
    index.min(states.len().saturating_sub(1))
}

fn gap_epochs(gap: &GnssGap) -> usize {
    // Gaps are recorded as inclusive time intervals; the epoch count only feeds
    // a diagnostic ratio, so an approximation from the interval is enough.
    let span = (gap.to_s - gap.from_s).max(0.0);
    if span < 1e-9 {
        1
    } else {
        span.round() as usize + 1
    }
}

/// Complete sensor bundle including the truth states they were derived from.
#[derive(Debug, Clone)]
pub struct SensorBundle {
    /// Ground truth at the inertial rate.
    pub truth: Vec<TruthState>,
    /// Simulated streams.
    pub sensors: Sensors,
}

/// Generates every sensor stream of a run.
#[allow(clippy::too_many_arguments)]
pub fn generate(
    trajectory: &Trajectory,
    regions: Option<&RegionSet>,
    magnetic_field: Option<&MagneticField>,
    frame: Option<&LocalFrame>,
    config: &SensorConfig,
    person: &PersonParams,
    seed: u64,
    individual: u32,
) -> Result<SensorBundle> {
    config.validate()?;
    if trajectory.samples.is_empty() {
        return Ok(SensorBundle {
            truth: Vec::new(),
            sensors: Sensors {
                mount: config.mount,
                ..Default::default()
            },
        });
    }

    // 1. Position jitter for the reported truth. It is generated here rather
    //    than inside the trajectory because only the reported positions carry it,
    //    never the acceleration the inertial sensors see.
    let mut jitter_rng = Rng::stream(seed, Stream::PositionJitter, individual, 0);
    let jitter: Vec<[f64; 2]> = (0..trajectory.samples.len())
        .map(|_| {
            if config.jitter_enabled {
                [
                    jitter_rng.gaussian() * config.jitter_sigma_m,
                    jitter_rng.gaussian() * config.jitter_sigma_m,
                ]
            } else {
                [0.0, 0.0]
            }
        })
        .collect();
    let states = truth::build_states(
        trajectory,
        if config.jitter_enabled {
            config.jitter_sigma_m
        } else {
            0.0
        },
        &jitter,
    );

    // 2. Region events, evaluated once per truth sample so every sensor that
    //    depends on them sees the same schedule.
    let mut event_rng = Rng::stream(seed, Stream::RegionEvent, individual, 0);
    let mut scheduler = EventScheduler::new();
    let mut multipath = vec![DVec2::ZERO; states.len()];
    let mut magnetic_disturbance = vec![[0.0f64; 3]; states.len()];
    if let Some(regions) = regions {
        for (index, state) in states.iter().enumerate() {
            scheduler.update(
                state.time_s,
                state.position,
                regions,
                seed,
                individual,
                &mut event_rng,
                config.force_deterministic_events,
            );
            if config.multipath_enabled {
                multipath[index] = scheduler.bias_at(state.time_s);
            }
            if config.magnetic_disturbance_enabled {
                let bias = scheduler.magnetic_bias_at(state.time_s);
                if bias != DVec2::ZERO {
                    // A disturbance is a short dipole pulse; its direction is
                    // taken from the event and scaled to the configured
                    // amplitude, with a vertical component so the total field
                    // magnitude changes as it does in reality.
                    let scale = config.mag_disturbance_ut / 20.0;
                    magnetic_disturbance[index] =
                        [bias.x * scale, bias.y * scale, bias.y * scale * 0.35];
                }
            }
        }
    }
    let multipath_events = scheduler.history().len();

    // 3. GNSS.
    let mut sensors = Sensors {
        mount: config.mount,
        multipath_events,
        ..Default::default()
    };
    let mut gnss_rng = gnss::stream(seed, individual, 0);
    let mut gnss_model = gnss::GnssModel::for_person(&person.sensors, config.gnss_rate_hz);
    let duration = trajectory.duration_s();
    let gnss_count = (duration * config.gnss_rate_hz).floor() as usize + 1;
    let mut gap_start: Option<f64> = None;
    let mut satellites_rng = Rng::stream(seed, Stream::Gnss, individual, 7);
    for index in 0..gnss_count {
        let time_s = index as f64 / config.gnss_rate_hz;
        let Some(state) = state_at(&states, time_s) else {
            continue;
        };
        // A dropout is a property of the place, not of the multipath model: a
        // tunnel removes every fix whether or not multipath bursts are enabled.
        // The draw is only taken when the location can drop a fix, so the random
        // stream is not consumed on open ground.
        let loss_probability = regions
            .map(|regions| regions.loss_probability_at(state.position.x, state.position.y) as f64)
            .unwrap_or(0.0);
        let dropped = loss_probability > 0.0 && gnss_rng.chance(loss_probability);
        if dropped {
            gap_start.get_or_insert(time_s);
            continue;
        }
        if let Some(from) = gap_start.take() {
            sensors.gnss_gaps.push(GnssGap {
                from_s: from,
                to_s: (time_s - 1.0 / config.gnss_rate_hz).max(from),
            });
        }
        let local = multipath
            .get(sample_index_at(&states, state.time_s))
            .copied()
            .unwrap_or(DVec2::ZERO);
        let (position, speed, heading, vertical_error) = gnss_model.observe(
            time_s,
            state.position,
            state.speed,
            state.heading,
            local,
            &mut gnss_rng,
        );
        let (latitude_deg, longitude_deg) = gnss::to_geographic(frame, position);
        sensors.gnss.push(GnssSample {
            time_s,
            latitude_deg,
            longitude_deg,
            x: position.x,
            y: position.y,
            altitude_m: state.z + vertical_error,
            speed_mps: speed,
            heading_rad: heading,
            valid: true,
            satellites: gnss::satellites_for(
                person.sensors.gnss_white_sigma_m,
                &mut satellites_rng,
            ),
        });
    }
    if let Some(from) = gap_start {
        sensors.gnss_gaps.push(GnssGap {
            from_s: from,
            to_s: duration,
        });
    }

    // 4. Inertial unit.
    let mut accel_rng = accel::stream(seed, individual);
    let mut gyro_rng = gyro::stream(seed, individual);
    let mut accel_model = accel::AccelerometerModel::for_person(&person.sensors);
    let mut gyro_model =
        gyro::GyroscopeModel::for_person(&person.sensors, config.mount == DeviceMount::Head);
    let harmonics = StepHarmonics::new(
        trajectory.step_frequency,
        trajectory.bounce.phase0,
        person.harmonic_2_ratio,
        person.harmonic_3_ratio,
    );
    let bounce: &BounceConfig = &trajectory.bounce;
    let dt = config.imu_dt();
    for state in &states {
        let rotation =
            crate::motion::attitude::rotation_matrix(state.heading, state.pitch, state.roll);
        let accel = accel_model.measure(
            state,
            rotation,
            &harmonics,
            bounce,
            person.target_speed,
            if state.time_s <= 0.0 { 0.0 } else { dt },
            &mut accel_rng,
        );
        sensors.imu.accel.push(ImuAccelSample {
            time_s: state.time_s,
            x: accel[0],
            y: accel[1],
            z: accel[2],
        });
        let gyro = gyro_model.measure(
            state,
            rotation,
            bounce,
            person.target_speed,
            dt,
            &mut gyro_rng,
        );
        sensors.imu.gyro.push(ImuGyroSample {
            time_s: state.time_s,
            x: gyro[0],
            y: gyro[1],
            z: gyro[2],
        });
    }

    // 5. Magnetometer.
    if let Some(field) = magnetic_field {
        let mut mag_rng = mag::stream(seed, individual);
        let mut model = mag::MagnetometerModel::for_person(field, &person.sensors);
        let count = (duration * config.mag_rate_hz).floor() as usize + 1;
        for index in 0..count {
            let time_s = index as f64 / config.mag_rate_hz;
            let Some(state) = state_at(&states, time_s) else {
                continue;
            };
            let rotation =
                crate::motion::attitude::rotation_matrix(state.heading, state.pitch, state.roll);
            let disturbance = magnetic_disturbance
                .get(sample_index_at(&states, time_s))
                .copied()
                .unwrap_or([0.0; 3]);
            let dt_mag = if index == 0 {
                0.0
            } else {
                1.0 / config.mag_rate_hz
            };
            let value = model.measure(rotation, disturbance, dt_mag, &mut mag_rng);
            sensors.mag.push(MagSample {
                time_s,
                x: value[0],
                y: value[1],
                z: value[2],
            });
        }
        sensors.magnetic_disturbed_samples = magnetic_disturbance
            .iter()
            .filter(|value| value[0] != 0.0 || value[1] != 0.0)
            .count();
    }

    // 6. Barometer.
    let mut baro_rng = baro::stream(seed, individual);
    let mut model = baro::BarometerModel::for_config(&person.sensors, config);
    let count = (duration * config.baro_rate_hz).floor() as usize + 1;
    for index in 0..count {
        let time_s = index as f64 / config.baro_rate_hz;
        let Some(state) = state_at(&states, time_s) else {
            continue;
        };
        let dt_baro = if index == 0 {
            0.0
        } else {
            1.0 / config.baro_rate_hz
        };
        let (pressure_pa, altitude_m) = model.measure(state.z, dt_baro, &mut baro_rng);
        sensors.baro.push(BaroSample {
            time_s,
            pressure_pa,
            altitude_m,
        });
    }

    Ok(SensorBundle {
        truth: states,
        sensors,
    })
}
