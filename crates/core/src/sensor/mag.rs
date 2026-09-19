//! Magnetometer: the geomagnetic field in the body frame.
//!
//! The earth field comes from the map's `MAGNETIC_FIELD` metadata — strength,
//! declination and inclination — and is rotated into the body frame by the same
//! attitude matrix the other sensors use. On top of it sit two error sources that
//! matter for heading estimation: a slowly varying hard-iron bias, and transient
//! disturbances from nearby steel or passing vehicles, which the region event
//! scheduler triggers. Modelling only the slow term produces data that is
//! recognisably too smooth.

use glam::DMat3;

use ourealis_map_format::tlv::value::MagneticField;
use serde::{Deserialize, Serialize};

use crate::noise::{OuParams, OuVector3};
use crate::rng::{Rng, Stream};

/// One magnetometer sample in the body frame.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct MagSample {
    /// Time since the start of the recording, seconds.
    pub time_s: f64,
    /// Body-frame field, microtesla.
    pub x: f64,
    /// Body-frame field, microtesla.
    pub y: f64,
    /// Body-frame field, microtesla.
    pub z: f64,
}

impl MagSample {
    /// Field vector as an array.
    pub fn field(&self) -> [f64; 3] {
        [self.x, self.y, self.z]
    }

    /// Field magnitude, microtesla.
    pub fn magnitude(&self) -> f64 {
        (self.x * self.x + self.y * self.y + self.z * self.z).sqrt()
    }
}

/// Magnetometer error state.
#[derive(Debug, Clone)]
pub struct MagnetometerModel {
    earth: [f64; 3],
    bias: OuVector3,
    white_sigma: f64,
}

impl MagnetometerModel {
    /// Creates a model from the map's geomagnetic parameters.
    ///
    /// The vector is expressed in the local frame: `x` points east, `y` north and
    /// `z` up. Declination is measured from north towards east, inclination
    /// downwards, which is why the vertical component is negative.
    pub fn new(field: &MagneticField, bias_sigma_ut: f64, white_sigma_ut: f64) -> Self {
        let strength = field.strength_ut as f64;
        let declination = (field.declination_deg as f64).to_radians();
        let inclination = (field.inclination_deg as f64).to_radians();
        let horizontal = strength * inclination.cos();
        Self {
            earth: [
                horizontal * declination.sin(),
                horizontal * declination.cos(),
                -strength * inclination.sin(),
            ],
            bias: OuVector3::new(OuParams::centred(bias_sigma_ut, 900.0)),
            white_sigma: white_sigma_ut,
        }
    }

    /// Builds the model from individual sensor parameters.
    pub fn for_person(field: &MagneticField, params: &crate::person::SensorNoiseParams) -> Self {
        Self::new(field, params.mag_bias_sigma_ut, params.mag_white_sigma_ut)
    }

    /// Earth field expressed in the local frame.
    pub fn earth_field(&self) -> [f64; 3] {
        self.earth
    }

    /// Measures the field at a body attitude.
    ///
    /// `disturbance` is the transient dipole disturbance in the world frame,
    /// already scaled by its envelope and amplitude.
    pub fn measure(
        &mut self,
        rotation: DMat3,
        disturbance: [f64; 3],
        dt: f64,
        rng: &mut Rng,
    ) -> [f64; 3] {
        let world = [
            self.earth[0] + disturbance[0],
            self.earth[1] + disturbance[1],
            self.earth[2] + disturbance[2],
        ];
        let body = rotation.transpose() * glam::DVec3::new(world[0], world[1], world[2]);
        let bias = self.bias.step(dt, rng);
        [
            body.x + bias[0] + rng.gaussian() * self.white_sigma,
            body.y + bias[1] + rng.gaussian() * self.white_sigma,
            body.z + bias[2] + rng.gaussian() * self.white_sigma,
        ]
    }
}

/// RNG stream for magnetometer noise.
pub fn stream(seed: u64, individual: u32) -> Rng {
    Rng::stream(seed, Stream::Magnetometer, individual, 0)
}
