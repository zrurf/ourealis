//! Geographic bounds and the local metre plane the OMF header is anchored in.
//!
//! The importer works in a local tangent plane, the same one the simulator uses
//! (`ourealis_core::math::LocalFrame`): with the reference point at
//! `(ref_lon, ref_lat)`,
//!
//! ```text
//! x = R * (lon - ref_lon) * cos(ref_lat)
//! y = R * (lat - ref_lat)
//! ```
//!
//! and `R = 6 371 000 m`. Over a campus-sized box the curvature error is far
//! below the resolution of the datasets, and keeping the formula identical to the
//! simulator's is what makes a downloaded map line up with a recorded trajectory.
//!
//! The reference point is the **south-west corner** of the request, so every
//! local coordinate is non-negative and the OMF bounds start at the origin.

use std::f64::consts::PI;

use crate::error::{GeoError, Result};

/// Mean Earth radius used by the local tangent-plane projection, metres.
pub const EARTH_RADIUS_M: f64 = 6_371_000.0;

/// Geographic bounding box in degrees, north-up.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GeoBounds {
    /// Southern edge, degrees.
    pub south: f64,
    /// Western edge, degrees.
    pub west: f64,
    /// Northern edge, degrees.
    pub north: f64,
    /// Eastern edge, degrees.
    pub east: f64,
}

impl GeoBounds {
    /// Validates and creates a box.
    pub fn new(south: f64, west: f64, north: f64, east: f64) -> Result<Self> {
        for (name, value) in [
            ("south", south),
            ("west", west),
            ("north", north),
            ("east", east),
        ] {
            if !value.is_finite() {
                return Err(GeoError::invalid(format!("{name} is not finite")));
            }
        }
        if !(-90.0..=90.0).contains(&south) || !(-90.0..=90.0).contains(&north) {
            return Err(GeoError::invalid("latitudes must lie in [-90, 90]"));
        }
        if !(-180.0..=180.0).contains(&west) || !(-180.0..=180.0).contains(&east) {
            return Err(GeoError::invalid("longitudes must lie in [-180, 180]"));
        }
        if south >= north {
            return Err(GeoError::invalid(format!(
                "south ({south}) must be below north ({north})"
            )));
        }
        if west >= east {
            return Err(GeoError::invalid(format!(
                "west ({west}) must be below east ({east})"
            )));
        }
        Ok(Self {
            south,
            west,
            north,
            east,
        })
    }

    /// Box centre as `(longitude, latitude)` in degrees.
    pub fn center(&self) -> (f64, f64) {
        (
            (self.west + self.east) * 0.5,
            (self.south + self.north) * 0.5,
        )
    }

    /// East-west extent in metres, at the reference latitude.
    pub fn width_m(&self) -> f64 {
        EARTH_RADIUS_M * (self.east - self.west).to_radians() * self.south.to_radians().cos()
    }

    /// North-south extent in metres.
    pub fn height_m(&self) -> f64 {
        EARTH_RADIUS_M * (self.north - self.south).to_radians()
    }

    /// Whether a geographic position lies inside the box.
    pub fn contains(&self, lon: f64, lat: f64) -> bool {
        (self.west..=self.east).contains(&lon) && (self.south..=self.north).contains(&lat)
    }

    /// Projects a geographic position into the local metre plane.
    pub fn project(&self, lon: f64, lat: f64) -> (f64, f64) {
        (
            EARTH_RADIUS_M * (lon - self.west).to_radians() * self.south.to_radians().cos(),
            EARTH_RADIUS_M * (lat - self.south).to_radians(),
        )
    }

    /// Box in the `south,west,north,east` order the Overpass API expects.
    pub fn overpass_bbox(&self) -> String {
        format!("{},{},{},{}", self.south, self.west, self.north, self.east)
    }
}

/// Semi-major axis of the Krasovsky 1940 ellipsoid, metres.
const GCJ_A: f64 = 6_378_245.0;

/// Squared eccentricity of the Krasovsky 1940 ellipsoid.
const GCJ_EE: f64 = 0.006_693_421_622_965_943;

/// Iterations of the forward transform used to invert it.
///
/// The offsets vary far more slowly than the positions they displace, so the
/// fixed-point iteration `wgs = gcj - delta(wgs)` contracts by three or four
/// orders of magnitude per step; five steps resolve well below the centimetre
/// scale of the survey that supplies the coordinates.
const GCJ_ITERATIONS: usize = 5;

/// Converts a GCJ-02 position into WGS84 `(longitude, latitude)` degrees.
///
/// Chinese base maps publish GCJ-02, a deliberately offset datum, while
/// OpenStreetMap and this map's reference frame are WGS84. The offset is a fixed
/// function of the position, so the forward transform is `gcj = wgs + delta(wgs)`
/// and inverting it means solving that equation: each step refines a WGS84
/// estimate by the offset it is missing. Outside China's bounding box the two
/// datums coincide and the input is returned unchanged.
pub fn gcj02_to_wgs84(lon: f64, lat: f64) -> (f64, f64) {
    if out_of_china(lon, lat) {
        return (lon, lat);
    }
    let mut wgs = (lon, lat);
    for _ in 0..GCJ_ITERATIONS {
        let (d_lon, d_lat) = gcj_offset(wgs.0, wgs.1);
        wgs = (lon - d_lon, lat - d_lat);
    }
    wgs
}

/// True when a position lies outside the box the GCJ-02 offset applies over.
fn out_of_china(lon: f64, lat: f64) -> bool {
    !(72.004..=137.8347).contains(&lon) || !(0.8293..=55.8271).contains(&lat)
}

/// Offset the forward transform adds at a WGS84 position, in degrees.
fn gcj_offset(lon: f64, lat: f64) -> (f64, f64) {
    let d_lon = transform_lng(lon - 105.0, lat - 35.0);
    let d_lat = transform_lat(lon - 105.0, lat - 35.0);
    let rad_lat = lat.to_radians();
    let magic = 1.0 - GCJ_EE * rad_lat.sin() * rad_lat.sin();
    let sqrt_magic = magic.sqrt();
    let d_lat = d_lat.to_degrees() / ((GCJ_A * (1.0 - GCJ_EE)) / (magic * sqrt_magic));
    let d_lon = d_lon.to_degrees() / (GCJ_A / sqrt_magic * rad_lat.cos());
    (d_lon, d_lat)
}

/// Longitude component of the published GCJ-02 offset polynomial.
///
/// The arguments carry a bare `PI` rather than a degree-to-radian conversion:
/// the polynomial is a fitted obfuscation, not a geometric series, and is used
/// here exactly as published.
fn transform_lng(x: f64, y: f64) -> f64 {
    let mut ret = 300.0 + x + 2.0 * y + 0.1 * x * x + 0.1 * x * y + 0.1 * x.abs().sqrt();
    ret += (20.0 * (6.0 * x * PI).sin() + 20.0 * (2.0 * x * PI).sin()) * 2.0 / 3.0;
    ret += (20.0 * (x * PI).sin() + 40.0 * (x / 3.0 * PI).sin()) * 2.0 / 3.0;
    ret += (150.0 * (x / 12.0 * PI).sin() + 300.0 * (x / 30.0 * PI).sin()) * 2.0 / 3.0;
    ret
}

/// Latitude component of the published GCJ-02 offset polynomial.
fn transform_lat(x: f64, y: f64) -> f64 {
    let mut ret = -100.0 + 2.0 * x + 3.0 * y + 0.2 * y * y + 0.1 * x * y + 0.2 * x.abs().sqrt();
    ret += (20.0 * (6.0 * x * PI).sin() + 20.0 * (2.0 * x * PI).sin()) * 2.0 / 3.0;
    ret += (20.0 * (y * PI).sin() + 40.0 * (y / 3.0 * PI).sin()) * 2.0 / 3.0;
    ret += (160.0 * (y / 12.0 * PI).sin() + 320.0 * (y / 30.0 * PI).sin()) * 2.0 / 3.0;
    ret
}
