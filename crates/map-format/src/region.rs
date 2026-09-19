//! Polygon region annotations.
//!
//! Sensor simulation needs *areal* environment events, not per-cell classes:
//! a multipath burst is caused by the area between high-rises, a GNSS dropout
//! by a tunnel. Each region therefore carries event parameters — trigger
//! probability, bias magnitude, dropout probability — instead of a category id.
//!
//! Trigger modes matter for reproducibility: the probabilistic mode suits
//! population studies, while the spatially deterministic mode derives the
//! decision from `(individual seed, region id, entry index)` so repeated runs
//! of the same individual produce byte-identical sensor data. Regression tests
//! and parameter calibration must use the latter.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::bytes::{Reader, Writer};
use crate::error::{MapError, Result};
use crate::geometry::Aabb;

/// Semantic tag of an environment region.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[repr(u16)]
pub enum RegionTag {
    /// Dense high-rise area: strong multipath.
    HighRise = 0,
    /// Elevated road or bridge deck overhead.
    Overpass = 1,
    /// Street canyon.
    Canyon = 2,
    /// Tunnel or covered passage: satellite visibility lost.
    Tunnel = 3,
    /// Indoor area.
    Indoor = 4,
    /// Area with strong transient magnetic disturbance (steel structures).
    MagneticDisturbance = 5,
}

impl RegionTag {
    /// Parses an on-disk identifier.
    pub const fn from_u16(value: u16) -> Option<Self> {
        match value {
            0 => Some(RegionTag::HighRise),
            1 => Some(RegionTag::Overpass),
            2 => Some(RegionTag::Canyon),
            3 => Some(RegionTag::Tunnel),
            4 => Some(RegionTag::Indoor),
            5 => Some(RegionTag::MagneticDisturbance),
            _ => None,
        }
    }
}

/// How region events are decided.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[repr(u8)]
pub enum TriggerMode {
    /// Independent sampling on every entry; suits statistical evaluation.
    Probabilistic = 0,
    /// Deterministic spatial hash; required for regression tests and
    /// calibration because it makes repeated runs reproducible.
    SpatialDeterministic = 1,
}

impl TriggerMode {
    /// Parses an on-disk identifier.
    pub const fn from_u8(value: u8) -> Option<Self> {
        match value {
            0 => Some(TriggerMode::Probabilistic),
            1 => Some(TriggerMode::SpatialDeterministic),
            _ => None,
        }
    }
}

/// Event parameters of one region.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct RegionFeature {
    /// Semantic tag; unknown tags are preserved but not interpreted.
    pub tag_id: u16,
    /// Index of the outline inside the region layer's polygon array.
    pub geom_ref: u16,
    /// Probability of a multipath event per entry.
    pub p_mp: f32,
    /// Multipath bias magnitude in metres (typical 5–20).
    pub mp_bias_m: f32,
    /// Probability of dropping a GNSS sample while inside.
    pub p_loss: f32,
    /// Decision mode for events in this region.
    pub mp_mode: TriggerMode,
}

impl RegionFeature {
    /// Serialised size in bytes.
    ///
    /// Layout: `tag_id u16 | geom_ref u16 | p_mp f32 | mp_bias_m f32 |
    /// p_loss f32 | mp_mode u8 | reserved 3 B`. The reserved tail keeps the
    /// record 4-byte aligned; a 16-byte record would have placed `mp_mode`
    /// inside the last byte of `p_loss`.
    pub const SIZE: usize = 20;

    /// Semantic tag, when recognised.
    pub fn tag(&self) -> Option<RegionTag> {
        RegionTag::from_u16(self.tag_id)
    }

    /// Serialises into the fixed layout.
    pub fn to_bytes(&self) -> [u8; Self::SIZE] {
        let mut buf = [0u8; Self::SIZE];
        let _ = crate::bytes::put_u16(&mut buf, 0, self.tag_id);
        let _ = crate::bytes::put_u16(&mut buf, 2, self.geom_ref);
        let _ = crate::bytes::put_f32(&mut buf, 4, self.p_mp);
        let _ = crate::bytes::put_f32(&mut buf, 8, self.mp_bias_m);
        let _ = crate::bytes::put_f32(&mut buf, 12, self.p_loss);
        let _ = crate::bytes::put_u8(&mut buf, 16, self.mp_mode as u8);
        buf
    }

    /// Parses the fixed layout. Unknown tags and trigger modes are preserved
    /// where possible; an unknown trigger mode falls back to probabilistic.
    pub fn from_bytes(buf: &[u8]) -> Result<Self> {
        if buf.len() < Self::SIZE {
            return Err(MapError::Truncated {
                offset: 0,
                needed: Self::SIZE,
                available: buf.len(),
            });
        }
        let mode_raw = crate::bytes::get_u8(buf, 16)?;
        Ok(Self {
            tag_id: crate::bytes::get_u16(buf, 0)?,
            geom_ref: crate::bytes::get_u16(buf, 2)?,
            p_mp: crate::bytes::get_f32(buf, 4)?,
            mp_bias_m: crate::bytes::get_f32(buf, 8)?,
            p_loss: crate::bytes::get_f32(buf, 12)?,
            // Unknown modes fall back to the format's declared default (0), which
            // is the probabilistic one; guessing "deterministic" would silently
            // change the statistics of an unrecognised producer's regions.
            mp_mode: TriggerMode::from_u8(mode_raw).unwrap_or(TriggerMode::Probabilistic),
        })
    }
}

/// Outcome of a deterministic event decision.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct SpatialEvent {
    /// Whether the event fires.
    pub triggered: bool,
    /// Bias direction in radians, measured in the local plane.
    pub direction_rad: f32,
    /// Normalised magnitude in `[0, 1]`, scaled by the caller's amplitude.
    pub magnitude: f32,
}

/// Region features plus their outlines and a lookup index.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RegionSet {
    features: Vec<RegionFeature>,
    polygons: Vec<Vec<[f32; 2]>>,
    bounds: Vec<Aabb>,
    buckets: HashMap<(i32, i32), Vec<u32>>,
    bucket_size_m: f32,
}

impl RegionSet {
    /// Builds a region set, indexing the outlines for spatial lookup.
    pub fn new(features: Vec<RegionFeature>, polygons: Vec<Vec<[f32; 2]>>) -> Result<Self> {
        if let Some(feature) = features
            .iter()
            .find(|f| f.geom_ref as usize >= polygons.len())
        {
            return Err(MapError::invalid(format!(
                "region references outline {}, only {} present",
                feature.geom_ref,
                polygons.len()
            )));
        }
        let bounds: Vec<Aabb> = polygons.iter().map(|p| outline_bounds(p)).collect();
        let mut set = Self {
            features,
            polygons,
            bounds,
            buckets: HashMap::new(),
            bucket_size_m: 50.0,
        };
        set.rebuild_index();
        Ok(set)
    }

    /// Region features.
    pub fn features(&self) -> &[RegionFeature] {
        &self.features
    }

    /// Outlines, indexed by `geom_ref`.
    pub fn polygons(&self) -> &[Vec<[f32; 2]>] {
        &self.polygons
    }

    /// True when no region is present.
    pub fn is_empty(&self) -> bool {
        self.features.is_empty()
    }

    /// Outline of a feature.
    pub fn outline(&self, feature: &RegionFeature) -> Option<&[[f32; 2]]> {
        self.polygons
            .get(feature.geom_ref as usize)
            .map(|p| p.as_slice())
    }

    /// Sets the spatial index bucket size.
    pub fn set_bucket_size(&mut self, size_m: f32) {
        self.bucket_size_m = size_m.max(1.0);
        self.rebuild_index();
    }

    /// Fills the spatial index with *feature* indices.
    ///
    /// Outlines are shared: several features may reference the same polygon, and
    /// the polygon order need not match the feature order. Indexing the polygons
    /// directly would therefore hand `regions_at` a foreign index, so the bucket
    /// is filled per feature, from the bounds of the outline it references.
    fn rebuild_index(&mut self) {
        self.buckets.clear();
        for (index, feature) in self.features.iter().enumerate() {
            let Some(bounds) = self.bounds.get(feature.geom_ref as usize) else {
                continue;
            };
            let (ix0, iy0) = self.bucket_of(bounds.min_x, bounds.min_y);
            let (ix1, iy1) = self.bucket_of(bounds.max_x, bounds.max_y);
            for iy in iy0..=iy1 {
                for ix in ix0..=ix1 {
                    self.buckets.entry((ix, iy)).or_default().push(index as u32);
                }
            }
        }
    }

    fn bucket_of(&self, x: f64, y: f64) -> (i32, i32) {
        (
            (x / self.bucket_size_m as f64).floor() as i32,
            (y / self.bucket_size_m as f64).floor() as i32,
        )
    }

    /// Indices of the regions containing `(x, y)`.
    pub fn regions_at(&self, x: f64, y: f64) -> Vec<usize> {
        let bucket = self.bucket_of(x, y);
        let Some(candidates) = self.buckets.get(&bucket) else {
            return Vec::new();
        };
        candidates
            .iter()
            .map(|i| *i as usize)
            .filter(|index| self.bounds[self.features[*index].geom_ref as usize].contains(x, y))
            .filter(|index| {
                point_in_polygon(
                    x,
                    y,
                    &self.polygons[self.features[*index].geom_ref as usize],
                )
            })
            .collect()
    }

    /// Features containing `(x, y)`.
    pub fn features_at(&self, x: f64, y: f64) -> Vec<&RegionFeature> {
        self.regions_at(x, y)
            .into_iter()
            .map(|index| &self.features[index])
            .collect()
    }

    /// Combined dropout probability at a position.
    pub fn loss_probability_at(&self, x: f64, y: f64) -> f32 {
        self.features_at(x, y)
            .iter()
            .map(|f| f.p_loss)
            .fold(0.0f32, |acc, p| {
                1.0 - (1.0 - acc) * (1.0 - p.clamp(0.0, 1.0))
            })
    }

    /// Serialises the region layer payload.
    pub fn encode(&self) -> Vec<u8> {
        let mut body = Writer::new();
        body.write_u16(self.features.len().min(u16::MAX as usize) as u16);
        for feature in self.features.iter().take(u16::MAX as usize) {
            body.write_bytes(&feature.to_bytes());
        }
        body.write_u16(self.polygons.len().min(u16::MAX as usize) as u16);
        for polygon in self.polygons.iter().take(u16::MAX as usize) {
            body.write_u16(polygon.len().min(u16::MAX as usize) as u16);
            for point in polygon.iter().take(u16::MAX as usize) {
                body.write_f32(point[0]);
                body.write_f32(point[1]);
            }
        }
        crate::graph::write_section(crate::graph::SectionId::Vectors, body.as_slice())
    }

    /// Parses a region layer payload.
    pub fn decode(bytes: &[u8]) -> Result<Self> {
        let mut sections = crate::graph::SectionReader::new(bytes);
        let Some(payload) = sections.find(crate::graph::SectionId::Vectors)? else {
            return Ok(Self::default());
        };
        let mut r = Reader::new(payload);
        let feature_count = r.read_u16()? as usize;
        let mut features = Vec::with_capacity(feature_count);
        for _ in 0..feature_count {
            features.push(RegionFeature::from_bytes(
                r.read_bytes(RegionFeature::SIZE)?,
            )?);
        }
        let polygon_count = r.read_u16()? as usize;
        let mut polygons = Vec::with_capacity(polygon_count);
        for _ in 0..polygon_count {
            let point_count = r.read_u16()? as usize;
            let mut points = Vec::with_capacity(point_count);
            for _ in 0..point_count {
                points.push([r.read_f32()?, r.read_f32()?]);
            }
            polygons.push(points);
        }
        Self::new(features, polygons)
    }
}

/// Bounding box of an outline.
pub fn outline_bounds(points: &[[f32; 2]]) -> Aabb {
    if points.is_empty() {
        return Aabb::new(0.0, 0.0, 0.0, 0.0);
    }
    let mut bounds = Aabb::new(
        points[0][0] as f64,
        points[0][1] as f64,
        points[0][0] as f64,
        points[0][1] as f64,
    );
    for point in &points[1..] {
        bounds = bounds.union(&Aabb::new(
            point[0] as f64,
            point[1] as f64,
            point[0] as f64,
            point[1] as f64,
        ));
    }
    bounds
}

/// Even–odd rule point-in-polygon test.
///
/// Points exactly on an edge may fall on either side; region outlines are
/// environment annotations, so a boundary cell belongs to neither outcome in a
/// way that matters for the event statistics.
pub fn point_in_polygon(x: f64, y: f64, polygon: &[[f32; 2]]) -> bool {
    if polygon.len() < 3 {
        return false;
    }
    let mut inside = false;
    let mut j = polygon.len() - 1;
    for i in 0..polygon.len() {
        let (xi, yi) = (polygon[i][0] as f64, polygon[i][1] as f64);
        let (xj, yj) = (polygon[j][0] as f64, polygon[j][1] as f64);
        let straddles = (yi > y) != (yj > y);
        if straddles {
            let t = (y - yi) / (yj - yi);
            if x < xi + t * (xj - xi) {
                inside = !inside;
            }
        }
        j = i;
    }
    inside
}

/// Deterministic spatial hash used by [`TriggerMode::SpatialDeterministic`].
///
/// The decision depends only on the individual seed, the region and the entry
/// counter, so an individual entering the same region repeatedly reproduces the
/// same event sequence across runs.
pub fn spatial_event(
    seed: u64,
    region_id: u16,
    entry_index: u32,
    probability: f32,
) -> SpatialEvent {
    use std::hash::Hasher;
    let mut hasher = twox_hash::XxHash64::with_seed(seed);
    hasher.write(&region_id.to_le_bytes());
    hasher.write(&entry_index.to_le_bytes());
    let hash = hasher.finish();

    let unit = (hash >> 11) as f64 / (1u64 << 53) as f64;
    let triggered = unit < probability.clamp(0.0, 1.0) as f64;
    let direction_rad = ((hash >> 32) as u32 as f64 / u32::MAX as f64) * std::f64::consts::TAU;
    let magnitude = ((hash & 0xFFFF) as f32) / 65535.0;
    SpatialEvent {
        triggered,
        direction_rad: direction_rad as f32,
        magnitude,
    }
}
