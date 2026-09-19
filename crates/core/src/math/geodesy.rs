//! Local tangent-plane projection.
//!
//! Every geometric computation happens in a local metre plane anchored at a
//! reference point. Over a campus-sized map the flat-earth approximation is far
//! below sensor noise, and it keeps the whole pipeline free of projection
//! libraries.

use glam::DVec2;

use super::EARTH_RADIUS_M;

/// Metric plane anchored at a reference longitude/latitude.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LocalFrame {
    /// Reference longitude in radians.
    pub ref_lon: f64,
    /// Reference latitude in radians.
    pub ref_lat: f64,
}

impl LocalFrame {
    /// Creates a frame from a reference point in radians.
    pub fn new(ref_lon: f64, ref_lat: f64) -> Self {
        Self { ref_lon, ref_lat }
    }

    /// Creates a frame from a reference point in degrees.
    pub fn from_degrees(lon_deg: f64, lat_deg: f64) -> Self {
        Self::new(lon_deg.to_radians(), lat_deg.to_radians())
    }

    /// Creates a frame from a parsed OMF header.
    pub fn from_header(header: &ourealis_map_format::Header) -> Self {
        Self::new(header.ref_lon, header.ref_lat)
    }

    /// Latitude scaling factor: one radian of longitude is `cos(lat)` radians of
    /// arc shorter than one radian of latitude.
    #[inline]
    pub fn cos_lat(&self) -> f64 {
        self.ref_lat.cos()
    }

    /// Projects a geographic position into the local metre plane.
    pub fn to_local(&self, lon: f64, lat: f64) -> DVec2 {
        DVec2::new(
            EARTH_RADIUS_M * (lon - self.ref_lon) * self.cos_lat(),
            EARTH_RADIUS_M * (lat - self.ref_lat),
        )
    }

    /// Projects a position given in degrees.
    pub fn to_local_degrees(&self, lon_deg: f64, lat_deg: f64) -> DVec2 {
        self.to_local(lon_deg.to_radians(), lat_deg.to_radians())
    }

    /// Inverse of [`LocalFrame::to_local`], returning radians.
    pub fn to_geo(&self, position: DVec2) -> (f64, f64) {
        let cos_lat = self.cos_lat();
        let scale = if cos_lat.abs() < 1e-12 {
            1e-12
        } else {
            cos_lat
        };
        (
            self.ref_lon + position.x / (EARTH_RADIUS_M * scale),
            self.ref_lat + position.y / EARTH_RADIUS_M,
        )
    }

    /// Inverse of [`LocalFrame::to_local`], returning degrees.
    pub fn to_geo_degrees(&self, position: DVec2) -> (f64, f64) {
        let (lon, lat) = self.to_geo(position);
        (lon.to_degrees(), lat.to_degrees())
    }
}
