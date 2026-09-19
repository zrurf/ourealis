//! Strongly typed payloads of the core metadata TLVs.
//!
//! Every type here round-trips through [`TlvValue`], so a writer and a reader
//! agree on one encoding. Fields are ordered as they appear on the wire.

use serde::{Deserialize, Serialize};

use crate::bytes::{Reader, Writer};
use crate::error::{MapError, Result};
use crate::layer::{LayerDesc, LayerId};
use crate::motion::MotionMode;
use crate::tlv::TlvValue;

/// `MAP_INFO`: identity and provenance of the map content.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct MapInfo {
    /// Human-readable map name.
    pub name: String,
    /// Author or generating organisation.
    pub author: String,
    /// Build time as a Unix timestamp in seconds.
    pub built_unix: u64,
    /// Hash of the upstream data snapshot (for example an OSM extract hash).
    pub upstream_hash: Vec<u8>,
    /// Free-form description.
    pub description: String,
}

impl TlvValue for MapInfo {
    fn encode(&self) -> Vec<u8> {
        let mut w = Writer::new();
        w.write_string(&self.name);
        w.write_string(&self.author);
        w.write_u64(self.built_unix);
        w.write_blob(&self.upstream_hash);
        w.write_string(&self.description);
        w.into_vec()
    }

    fn decode(bytes: &[u8]) -> Result<Self> {
        let mut r = Reader::new(bytes);
        Ok(Self {
            name: r.read_string()?,
            author: r.read_string()?,
            built_unix: r.read_u64()?,
            upstream_hash: r.read_blob()?.to_vec(),
            description: r.read_string()?,
        })
    }
}

/// `LAYER_TABLE`: registry of every layer stored in the file.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct LayerTable {
    /// Registered descriptors, in registration order.
    pub layers: Vec<LayerDesc>,
}

impl LayerTable {
    /// Looks up a descriptor by id.
    pub fn get(&self, layer_id: LayerId) -> Option<&LayerDesc> {
        self.layers.iter().find(|d| d.layer_id == layer_id)
    }

    /// Inserts or replaces a descriptor.
    pub fn upsert(&mut self, desc: LayerDesc) {
        match self.layers.iter_mut().find(|d| d.layer_id == desc.layer_id) {
            Some(slot) => *slot = desc,
            None => self.layers.push(desc),
        }
    }
}

impl TlvValue for LayerTable {
    fn encode(&self) -> Vec<u8> {
        let mut w = Writer::with_capacity(2 + self.layers.len() * LayerDesc::SIZE);
        w.write_u16(self.layers.len().min(u16::MAX as usize) as u16);
        for desc in self.layers.iter().take(u16::MAX as usize) {
            w.write_bytes(&desc.to_bytes());
        }
        w.into_vec()
    }

    fn decode(bytes: &[u8]) -> Result<Self> {
        let mut r = Reader::new(bytes);
        let count = r.read_u16()? as usize;
        let mut layers = Vec::with_capacity(count);
        for _ in 0..count {
            layers.push(LayerDesc::from_bytes(r.read_bytes(LayerDesc::SIZE)?)?);
        }
        Ok(Self { layers })
    }
}

/// Semantics of one resistance feature dimension.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[repr(u8)]
pub enum FeatureKind {
    /// Continuous scalar, normalised to `[0, 1]`.
    Scalar = 0,
    /// Category id resolved through the palette.
    Category = 1,
    /// Angular constraint, stored packed as `angle | strength`.
    Direction = 2,
    /// Boolean occupancy.
    Boolean = 3,
}

impl FeatureKind {
    /// Parses an on-disk identifier.
    pub const fn from_u8(value: u8) -> Option<Self> {
        match value {
            0 => Some(FeatureKind::Scalar),
            1 => Some(FeatureKind::Category),
            2 => Some(FeatureKind::Direction),
            3 => Some(FeatureKind::Boolean),
            _ => None,
        }
    }
}

/// One resistance feature dimension of the environment.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FeatureDim {
    /// Dimension name, for example `surface_type`.
    pub name: String,
    /// Unit of the raw stored value (empty for categories and booleans).
    pub unit: String,
    /// Dimension semantics.
    pub kind: FeatureKind,
    /// Layer holding this dimension.
    pub layer_id: LayerId,
    /// Channel inside that layer.
    pub channel: u8,
    /// Dequantisation scale of the raw value.
    pub scale: f32,
    /// Dequantisation bias of the raw value.
    pub bias: f32,
    /// Lower end of the normalised range used by the cost model.
    pub norm_min: f32,
    /// Upper end of the normalised range.
    pub norm_max: f32,
    /// Normalised value of each category id (empty unless `kind` is `Category`).
    pub palette: Vec<f32>,
}

impl FeatureDim {
    /// Normalises a dequantised raw value into the cost model's `[0, 1]` range.
    pub fn normalise(&self, raw: f32) -> f32 {
        let span = self.norm_max - self.norm_min;
        if span.abs() < f32::EPSILON {
            return 0.0;
        }
        ((raw - self.norm_min) / span).clamp(0.0, 1.0)
    }
}

/// `FEATURE_SCHEMA`: the D resistance feature dimensions.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct FeatureSchema {
    /// Dimensions in weight-vector order; index equals the weight index.
    pub dims: Vec<FeatureDim>,
}

impl FeatureSchema {
    /// Feature dimension count `D`.
    pub fn dim(&self) -> u8 {
        self.dims.len().min(u8::MAX as usize) as u8
    }
}

impl TlvValue for FeatureSchema {
    fn encode(&self) -> Vec<u8> {
        let mut w = Writer::new();
        w.write_u8(self.dim());
        for dim in self.dims.iter().take(u8::MAX as usize) {
            w.write_string(&dim.name);
            w.write_string(&dim.unit);
            w.write_u8(dim.kind as u8);
            w.write_u16(dim.layer_id.raw());
            w.write_u8(dim.channel);
            w.write_f32(dim.scale);
            w.write_f32(dim.bias);
            w.write_f32(dim.norm_min);
            w.write_f32(dim.norm_max);
            w.write_u8(dim.palette.len().min(u8::MAX as usize) as u8);
            for value in dim.palette.iter().take(u8::MAX as usize) {
                w.write_f32(*value);
            }
        }
        w.into_vec()
    }

    fn decode(bytes: &[u8]) -> Result<Self> {
        let mut r = Reader::new(bytes);
        let dim = r.read_u8()? as usize;
        let mut dims = Vec::with_capacity(dim);
        for _ in 0..dim {
            let name = r.read_string()?;
            let unit = r.read_string()?;
            let kind_raw = r.read_u8()?;
            let kind = FeatureKind::from_u8(kind_raw)
                .ok_or_else(|| MapError::invalid(format!("unknown feature kind {kind_raw}")))?;
            let layer_id = LayerId(r.read_u16()?);
            let channel = r.read_u8()?;
            let scale = r.read_f32()?;
            let bias = r.read_f32()?;
            let norm_min = r.read_f32()?;
            let norm_max = r.read_f32()?;
            let palette_len = r.read_u8()? as usize;
            let palette = r.read_f32_vec(palette_len)?;
            dims.push(FeatureDim {
                name,
                unit,
                kind,
                layer_id,
                channel,
                scale,
                bias,
                norm_min,
                norm_max,
                palette,
            });
        }
        Ok(Self { dims })
    }
}

/// Static prior weights for one motion mode.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WeightPriorEntry {
    /// Motion mode this prior applies to.
    pub mode: MotionMode,
    /// Prior weight per feature dimension.
    pub weights: Vec<f32>,
    /// Temperature used when converting logits to weights.
    pub tau: f32,
    /// Uniform cost scale factor applied after the weights.
    pub scale: f32,
}

/// `WEIGHT_PRIOR`: one entry per motion mode.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct WeightPrior {
    /// Registered priors.
    pub entries: Vec<WeightPriorEntry>,
}

impl WeightPrior {
    /// Looks up the prior of a mode.
    pub fn get(&self, mode: MotionMode) -> Option<&WeightPriorEntry> {
        self.entries.iter().find(|e| e.mode == mode)
    }

    /// Inserts or replaces the prior of a mode.
    pub fn upsert(&mut self, entry: WeightPriorEntry) {
        match self.entries.iter_mut().find(|e| e.mode == entry.mode) {
            Some(slot) => *slot = entry,
            None => self.entries.push(entry),
        }
    }
}

impl TlvValue for WeightPrior {
    fn encode(&self) -> Vec<u8> {
        let mut w = Writer::new();
        w.write_u16(self.entries.len().min(u16::MAX as usize) as u16);
        for entry in self.entries.iter().take(u16::MAX as usize) {
            w.write_u8(entry.mode.as_u8());
            w.write_f32(entry.tau);
            w.write_f32(entry.scale);
            w.write_u8(entry.weights.len().min(u8::MAX as usize) as u8);
            for weight in entry.weights.iter().take(u8::MAX as usize) {
                w.write_f32(*weight);
            }
        }
        w.into_vec()
    }

    fn decode(bytes: &[u8]) -> Result<Self> {
        let mut r = Reader::new(bytes);
        let count = r.read_u16()? as usize;
        let mut entries = Vec::with_capacity(count);
        for _ in 0..count {
            let mode_raw = r.read_u8()?;
            let mode = MotionMode::from_u8(mode_raw)
                .ok_or_else(|| MapError::invalid(format!("unknown motion mode {mode_raw}")))?;
            let tau = r.read_f32()?;
            let scale = r.read_f32()?;
            let dim = r.read_u8()? as usize;
            let weights = r.read_f32_vec(dim)?;
            entries.push(WeightPriorEntry {
                mode,
                weights,
                tau,
                scale,
            });
        }
        Ok(Self { entries })
    }
}

/// `SLOPE_MODEL`: physiological model parameters and their validity bounds.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct SlopeModel {
    /// Absolute grade bound beyond which the Minetti polynomial is clamped.
    pub minetti_clamp: f32,
    /// Default downhill speed-cap coefficient (`v_down = k_down * v_target`).
    pub k_down_default: f32,
    /// Equivalent ascent speed on stairs, m/s.
    pub stair_v_up: f32,
    /// Equivalent descent speed on stairs, m/s.
    pub stair_v_down: f32,
}

impl Default for SlopeModel {
    fn default() -> Self {
        Self {
            minetti_clamp: 0.25,
            k_down_default: 1.15,
            stair_v_up: 0.5,
            stair_v_down: 0.7,
        }
    }
}

impl TlvValue for SlopeModel {
    fn encode(&self) -> Vec<u8> {
        let mut w = Writer::new();
        w.write_f32(self.minetti_clamp);
        w.write_f32(self.k_down_default);
        w.write_f32(self.stair_v_up);
        w.write_f32(self.stair_v_down);
        w.into_vec()
    }

    fn decode(bytes: &[u8]) -> Result<Self> {
        let mut r = Reader::new(bytes);
        Ok(Self {
            minetti_clamp: r.read_f32()?,
            k_down_default: r.read_f32()?,
            stair_v_up: r.read_f32()?,
            stair_v_down: r.read_f32()?,
        })
    }
}

/// Kind of Z-axis link.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[repr(u8)]
pub enum ConnectorType {
    /// Stairway.
    Stair = 0,
    /// Elevator.
    Elevator = 1,
    /// Footbridge / overpass.
    Overpass = 2,
    /// Underpass.
    Underpass = 3,
    /// Predefined loop circuit referencing a vector polyline.
    Loop = 4,
}

impl ConnectorType {
    /// Parses an on-disk identifier.
    pub const fn from_u16(value: u16) -> Option<Self> {
        match value {
            0 => Some(ConnectorType::Stair),
            1 => Some(ConnectorType::Elevator),
            2 => Some(ConnectorType::Overpass),
            3 => Some(ConnectorType::Underpass),
            4 => Some(ConnectorType::Loop),
            _ => None,
        }
    }
}

/// Traversal direction of a connector.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[repr(u8)]
pub enum ConnectorDirection {
    /// Passable in both directions.
    Both = 0,
    /// Only from endpoint A to endpoint B.
    AToB = 1,
    /// Only from endpoint B to endpoint A.
    BToA = 2,
}

impl ConnectorDirection {
    /// Parses an on-disk identifier.
    pub const fn from_u8(value: u8) -> Option<Self> {
        match value {
            0 => Some(ConnectorDirection::Both),
            1 => Some(ConnectorDirection::AToB),
            2 => Some(ConnectorDirection::BToA),
            _ => None,
        }
    }

    /// True when travel from A to B is allowed.
    pub const fn allows_a_to_b(self) -> bool {
        matches!(self, ConnectorDirection::Both | ConnectorDirection::AToB)
    }

    /// True when travel from B to A is allowed.
    pub const fn allows_b_to_a(self) -> bool {
        matches!(self, ConnectorDirection::Both | ConnectorDirection::BToA)
    }
}

/// A Z-axis link stored as a special graph edge.
///
/// The four bytes of the specification's `reserved` tail are used for
/// `unit_cost`; the last byte stays reserved.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Connector {
    /// Link kind.
    pub type_id: u16,
    /// Endpoint A in local metres plus elevation.
    pub a: [f32; 3],
    /// Endpoint B in local metres plus elevation.
    pub b: [f32; 3],
    /// Direction restriction.
    pub dir_flag: ConnectorDirection,
    /// Equivalent speed going up, m/s (overrides the `SLOPE_MODEL` default).
    pub v_up: f32,
    /// Equivalent speed going down, m/s.
    pub v_down: f32,
    /// Mean waiting time (elevators), seconds.
    pub wait_time: f32,
    /// Reference to extra attributes, for example a vector polyline id.
    pub attr_ref: u32,
    /// Unit cost coefficient in equivalent metres per metre of link length.
    pub unit_cost: f32,
}

impl Connector {
    /// Serialised size in bytes.
    pub const SIZE: usize = 48;

    /// Vector from A to B.
    pub fn delta(&self) -> [f32; 3] {
        [
            self.b[0] - self.a[0],
            self.b[1] - self.a[1],
            self.b[2] - self.a[2],
        ]
    }

    /// Full 3D length.
    pub fn length_3d(&self) -> f32 {
        let d = self.delta();
        (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt()
    }

    /// Length projected on the horizontal plane.
    pub fn length_horizontal(&self) -> f32 {
        let d = self.delta();
        (d[0] * d[0] + d[1] * d[1]).sqrt()
    }

    /// Elevation gain from A to B (signed).
    pub fn delta_h(&self) -> f32 {
        self.b[2] - self.a[2]
    }

    /// Equivalent-distance cost of the link: `length * unit_cost + wait_cost`.
    ///
    /// `wait_cost` is the waiting time converted to equivalent metres with
    /// `speed_ref`.
    pub fn cost_equiv_m(&self, speed_ref: f32) -> f32 {
        self.length_3d() * self.unit_cost + self.wait_time * speed_ref
    }

    /// Equivalent traversal speed in the given direction.
    pub fn speed(&self, a_to_b: bool) -> f32 {
        if self.delta_h() >= 0.0 {
            if a_to_b { self.v_up } else { self.v_down }
        } else if a_to_b {
            self.v_down
        } else {
            self.v_up
        }
    }

    /// Serialises into the fixed layout.
    pub fn to_bytes(&self) -> [u8; Self::SIZE] {
        let mut buf = [0u8; Self::SIZE];
        let _ = crate::bytes::put_u16(&mut buf, 0, self.type_id);
        for (i, v) in self.a.iter().enumerate() {
            let _ = crate::bytes::put_f32(&mut buf, 2 + i * 4, *v);
        }
        for (i, v) in self.b.iter().enumerate() {
            let _ = crate::bytes::put_f32(&mut buf, 14 + i * 4, *v);
        }
        let _ = crate::bytes::put_u8(&mut buf, 26, self.dir_flag as u8);
        let _ = crate::bytes::put_f32(&mut buf, 27, self.v_up);
        let _ = crate::bytes::put_f32(&mut buf, 31, self.v_down);
        let _ = crate::bytes::put_f32(&mut buf, 35, self.wait_time);
        let _ = crate::bytes::put_u32(&mut buf, 39, self.attr_ref);
        let _ = crate::bytes::put_f32(&mut buf, 43, self.unit_cost);
        buf
    }

    /// Parses the fixed layout.
    pub fn from_bytes(buf: &[u8]) -> Result<Self> {
        use crate::bytes as le;
        if buf.len() < Self::SIZE {
            return Err(MapError::Truncated {
                offset: 0,
                needed: Self::SIZE,
                available: buf.len(),
            });
        }
        let type_id = le::get_u16(buf, 0)?;
        let dir_raw = le::get_u8(buf, 26)?;
        let dir_flag = ConnectorDirection::from_u8(dir_raw)
            .ok_or_else(|| MapError::invalid(format!("unknown connector direction {dir_raw}")))?;
        Ok(Self {
            type_id,
            a: [
                le::get_f32(buf, 2)?,
                le::get_f32(buf, 6)?,
                le::get_f32(buf, 10)?,
            ],
            b: [
                le::get_f32(buf, 14)?,
                le::get_f32(buf, 18)?,
                le::get_f32(buf, 22)?,
            ],
            dir_flag,
            v_up: le::get_f32(buf, 27)?,
            v_down: le::get_f32(buf, 31)?,
            wait_time: le::get_f32(buf, 35)?,
            attr_ref: le::get_u32(buf, 39)?,
            unit_cost: le::get_f32(buf, 43)?,
        })
    }

    /// Convenience constructor with both speeds and a unit cost.
    pub fn new(
        kind: ConnectorType,
        a: [f32; 3],
        b: [f32; 3],
        dir_flag: ConnectorDirection,
        v_up: f32,
        v_down: f32,
        unit_cost: f32,
    ) -> Self {
        Self {
            type_id: kind as u16,
            a,
            b,
            dir_flag,
            v_up,
            v_down,
            wait_time: 0.0,
            attr_ref: 0,
            unit_cost,
        }
    }
}

/// `CONNECTOR_TABLE`: every Z-axis link in the map.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ConnectorTable {
    /// Registered links.
    pub connectors: Vec<Connector>,
}

impl ConnectorTable {
    /// Smallest unit-cost coefficient across all links, used for the heuristic's
    /// positive lower bound in multi-level maps.
    pub fn min_unit_cost(&self) -> Option<f32> {
        self.connectors
            .iter()
            .map(|c| c.unit_cost)
            .fold(None, |acc, v| {
                Some(match acc {
                    Some(a) if a <= v => a,
                    _ => v,
                })
            })
    }
}

impl TlvValue for ConnectorTable {
    fn encode(&self) -> Vec<u8> {
        let mut w = Writer::with_capacity(2 + self.connectors.len() * Connector::SIZE);
        w.write_u16(self.connectors.len().min(u16::MAX as usize) as u16);
        for connector in self.connectors.iter().take(u16::MAX as usize) {
            w.write_bytes(&connector.to_bytes());
        }
        w.into_vec()
    }

    fn decode(bytes: &[u8]) -> Result<Self> {
        let mut r = Reader::new(bytes);
        let count = r.read_u16()? as usize;
        let mut connectors = Vec::with_capacity(count);
        for _ in 0..count {
            connectors.push(Connector::from_bytes(r.read_bytes(Connector::SIZE)?)?);
        }
        Ok(Self { connectors })
    }
}

/// `PROVENANCE`: how the file was produced.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Provenance {
    /// Tool identifier.
    pub tool: String,
    /// Tool version string.
    pub tool_version: String,
    /// Upstream data sources and licences.
    pub sources: Vec<String>,
    /// Derivation algorithm versions: `(algorithm id, version)`.
    pub algo_versions: Vec<(u16, u16)>,
}

impl TlvValue for Provenance {
    fn encode(&self) -> Vec<u8> {
        let mut w = Writer::new();
        w.write_string(&self.tool);
        w.write_string(&self.tool_version);
        w.write_u16(self.sources.len().min(u16::MAX as usize) as u16);
        for source in self.sources.iter().take(u16::MAX as usize) {
            w.write_string(source);
        }
        w.write_u16(self.algo_versions.len().min(u16::MAX as usize) as u16);
        for (id, version) in self.algo_versions.iter().take(u16::MAX as usize) {
            w.write_u16(*id);
            w.write_u16(*version);
        }
        w.into_vec()
    }

    fn decode(bytes: &[u8]) -> Result<Self> {
        let mut r = Reader::new(bytes);
        let tool = r.read_string()?;
        let tool_version = r.read_string()?;
        let source_count = r.read_u16()? as usize;
        let mut sources = Vec::with_capacity(source_count);
        for _ in 0..source_count {
            sources.push(r.read_string()?);
        }
        let algo_count = r.read_u16()? as usize;
        let mut algo_versions = Vec::with_capacity(algo_count);
        for _ in 0..algo_count {
            algo_versions.push((r.read_u16()?, r.read_u16()?));
        }
        Ok(Self {
            tool,
            tool_version,
            sources,
            algo_versions,
        })
    }
}

/// `MAGNETIC_FIELD`: local geomagnetic parameters for magnetometer simulation.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct MagneticField {
    /// Total field strength in microtesla.
    pub strength_ut: f32,
    /// Declination in degrees (east positive).
    pub declination_deg: f32,
    /// Inclination in degrees (down positive).
    pub inclination_deg: f32,
}

impl Default for MagneticField {
    fn default() -> Self {
        Self {
            strength_ut: 50.0,
            declination_deg: -6.0,
            inclination_deg: 55.0,
        }
    }
}

impl TlvValue for MagneticField {
    fn encode(&self) -> Vec<u8> {
        let mut w = Writer::new();
        w.write_f32(self.strength_ut);
        w.write_f32(self.declination_deg);
        w.write_f32(self.inclination_deg);
        w.into_vec()
    }

    fn decode(bytes: &[u8]) -> Result<Self> {
        let mut r = Reader::new(bytes);
        Ok(Self {
            strength_ut: r.read_f32()?,
            declination_deg: r.read_f32()?,
            inclination_deg: r.read_f32()?,
        })
    }
}

/// `PRM_SEEDS`: one random seed per PRM batch stored in the graph layer.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PrmSeeds {
    /// Seeds in batch order; the batch index is the position in this list.
    pub seeds: Vec<u64>,
}

impl TlvValue for PrmSeeds {
    fn encode(&self) -> Vec<u8> {
        let mut w = Writer::with_capacity(2 + self.seeds.len() * 8);
        w.write_u16(self.seeds.len().min(u16::MAX as usize) as u16);
        for seed in self.seeds.iter().take(u16::MAX as usize) {
            w.write_u64(*seed);
        }
        w.into_vec()
    }

    fn decode(bytes: &[u8]) -> Result<Self> {
        let mut r = Reader::new(bytes);
        let count = r.read_u16()? as usize;
        let mut seeds = Vec::with_capacity(count);
        for _ in 0..count {
            seeds.push(r.read_u64()?);
        }
        Ok(Self { seeds })
    }
}

/// In-memory layout of raster chunks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[repr(u8)]
pub enum ChunkLayoutKind {
    /// `[height][width][channel]`, channels innermost: ready for a GPU upload
    /// without re-shuffling.
    #[default]
    ChannelContinuous = 0,
    /// `[channel][height][width]`, one plane per channel.
    PlanePerChannel = 1,
}

impl ChunkLayoutKind {
    /// Parses an on-disk identifier.
    pub const fn from_u8(value: u8) -> Option<Self> {
        match value {
            0 => Some(ChunkLayoutKind::ChannelContinuous),
            1 => Some(ChunkLayoutKind::PlanePerChannel),
            _ => None,
        }
    }
}

/// `CHUNK_LAYOUT`: declares how raster chunks are laid out in memory.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChunkLayout {
    /// Layout of the chunk payloads.
    pub layout: ChunkLayoutKind,
    /// Channel ids in storage order.
    pub channel_order: Vec<u16>,
}

impl TlvValue for ChunkLayout {
    fn encode(&self) -> Vec<u8> {
        let mut w = Writer::new();
        w.write_u8(self.layout as u8);
        w.write_u16(self.channel_order.len().min(u16::MAX as usize) as u16);
        for channel in self.channel_order.iter().take(u16::MAX as usize) {
            w.write_u16(*channel);
        }
        w.into_vec()
    }

    fn decode(bytes: &[u8]) -> Result<Self> {
        let mut r = Reader::new(bytes);
        let layout_raw = r.read_u8()?;
        let layout = ChunkLayoutKind::from_u8(layout_raw)
            .ok_or_else(|| MapError::invalid(format!("unknown chunk layout {layout_raw}")))?;
        let count = r.read_u16()? as usize;
        let mut channel_order = Vec::with_capacity(count);
        for _ in 0..count {
            channel_order.push(r.read_u16()?);
        }
        Ok(Self {
            layout,
            channel_order,
        })
    }
}

/// Statistics of one raster channel over the whole map.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ChannelStats {
    /// Layer the channel belongs to.
    pub layer_id: LayerId,
    /// Channel index.
    pub channel: u8,
    /// Minimum real value.
    pub min: f32,
    /// Maximum real value.
    pub max: f32,
    /// Mean real value.
    pub mean: f32,
    /// Fraction of cells that carry a valid sample.
    pub coverage: f32,
}

/// `GLOBAL_STATS`: whole-map statistics used for the heuristic's lower bound.
///
/// The simulator derives `c_min >= sum_i w_i * f_i,min + c0` from the per-channel
/// minima. When connectors exist their smallest unit cost participates too,
/// otherwise the bound would overestimate on multi-level maps.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct GlobalStats {
    /// Per-channel statistics.
    pub channels: Vec<ChannelStats>,
    /// Fraction of forbidden cells per constraint layer: `(layer_id, ratio)`.
    pub forbidden_ratio: Vec<(LayerId, f32)>,
    /// Smallest connector unit cost, when the map has connectors.
    pub connector_unit_cost_min: Option<f32>,
}

impl GlobalStats {
    /// Minimum real value of `layer`'s `channel`, if recorded.
    pub fn channel_min(&self, layer: LayerId, channel: u8) -> Option<f32> {
        self.channels
            .iter()
            .find(|c| c.layer_id == layer && c.channel == channel)
            .map(|c| c.min)
    }
}

impl TlvValue for GlobalStats {
    fn encode(&self) -> Vec<u8> {
        let mut w = Writer::new();
        w.write_u16(self.channels.len().min(u16::MAX as usize) as u16);
        for stats in self.channels.iter().take(u16::MAX as usize) {
            w.write_u16(stats.layer_id.raw());
            w.write_u8(stats.channel);
            w.write_f32(stats.min);
            w.write_f32(stats.max);
            w.write_f32(stats.mean);
            w.write_f32(stats.coverage);
        }
        w.write_u16(self.forbidden_ratio.len().min(u16::MAX as usize) as u16);
        for (layer, ratio) in self.forbidden_ratio.iter().take(u16::MAX as usize) {
            w.write_u16(layer.raw());
            w.write_f32(*ratio);
        }
        match self.connector_unit_cost_min {
            Some(value) => {
                w.write_u8(1);
                w.write_f32(value);
            }
            None => {
                w.write_u8(0);
            }
        }
        w.into_vec()
    }

    fn decode(bytes: &[u8]) -> Result<Self> {
        let mut r = Reader::new(bytes);
        let count = r.read_u16()? as usize;
        let mut channels = Vec::with_capacity(count);
        for _ in 0..count {
            channels.push(ChannelStats {
                layer_id: LayerId(r.read_u16()?),
                channel: r.read_u8()?,
                min: r.read_f32()?,
                max: r.read_f32()?,
                mean: r.read_f32()?,
                coverage: r.read_f32()?,
            });
        }
        let forbidden_count = r.read_u16()? as usize;
        let mut forbidden_ratio = Vec::with_capacity(forbidden_count);
        for _ in 0..forbidden_count {
            forbidden_ratio.push((LayerId(r.read_u16()?), r.read_f32()?));
        }
        let connector_unit_cost_min = if r.read_u8()? != 0 {
            Some(r.read_f32()?)
        } else {
            None
        };
        Ok(Self {
            channels,
            forbidden_ratio,
            connector_unit_cost_min,
        })
    }
}

/// Permitted coarse-level aggregation operator of a channel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[repr(u8)]
pub enum AggregationOp {
    /// Store mean and maximum together (scalar features).
    MeanMax = 0,
    /// Boolean OR (hard constraints: any forbidden cell forbids the coarse cell).
    Or = 1,
    /// Dominant category plus a mixing flag.
    DominantWithMix = 2,
    /// Aggregation is forbidden; the channel must stay at level 0.
    Forbidden = 3,
}

impl AggregationOp {
    /// Parses an on-disk identifier.
    pub const fn from_u8(value: u8) -> Option<Self> {
        match value {
            0 => Some(AggregationOp::MeanMax),
            1 => Some(AggregationOp::Or),
            2 => Some(AggregationOp::DominantWithMix),
            3 => Some(AggregationOp::Forbidden),
            _ => None,
        }
    }
}

/// Aggregation rule of one channel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChannelAggregation {
    /// Layer the channel belongs to.
    pub layer_id: LayerId,
    /// Channel index.
    pub channel: u8,
    /// Permitted operator.
    pub op: AggregationOp,
}

/// `AGGREGATION_RULES`: coarse-level aggregation rules and the quantisation of
/// the skeleton's embedded aggregate values.
///
/// Skeleton nodes carry `u16` aggregates of one *proxy* channel; the scale and
/// bias that make those values interpretable live here, so any reader can
/// recover their meaning without extra per-node data.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct AggregationRules {
    /// Layer of the proxy channel the skeleton aggregates.
    pub proxy_layer: LayerId,
    /// Channel of the proxy channel.
    pub proxy_channel: u8,
    /// Dequantisation scale of `QNode::aggr_mean` / `aggr_max`.
    pub aggr_scale: f32,
    /// Dequantisation bias of the aggregate values.
    pub aggr_bias: f32,
}

impl Default for AggregationRules {
    fn default() -> Self {
        Self {
            proxy_layer: LayerId::HARD_FORBIDDEN,
            proxy_channel: 0,
            aggr_scale: 1.0,
            aggr_bias: 0.0,
        }
    }
}

impl TlvValue for AggregationRules {
    fn encode(&self) -> Vec<u8> {
        let mut w = Writer::new();
        w.write_u16(self.proxy_layer.raw());
        w.write_u8(self.proxy_channel);
        w.write_f32(self.aggr_scale);
        w.write_f32(self.aggr_bias);
        w.into_vec()
    }

    fn decode(bytes: &[u8]) -> Result<Self> {
        let mut r = Reader::new(bytes);
        Ok(Self {
            proxy_layer: LayerId(r.read_u16()?),
            proxy_channel: r.read_u8()?,
            aggr_scale: r.read_f32()?,
            aggr_bias: r.read_f32()?,
        })
    }
}

/// Fingerprint header of one derived layer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DerivedLayerEntry {
    /// Derived layer id.
    pub layer_id: LayerId,
    /// Combined content hash of the source layers it was built from.
    pub source_fingerprint: u64,
    /// Hash of the derivation parameters.
    pub build_params_hash: u64,
    /// Derivation algorithm version.
    pub algo_version: u16,
    /// Random seeds used (PRM sampling and similar).
    pub seeds: Vec<u64>,
}

/// `DERIVED_LAYERS`: fingerprints of every derived layer in the file.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DerivedLayers {
    /// One entry per derived layer.
    pub entries: Vec<DerivedLayerEntry>,
}

impl DerivedLayers {
    /// Looks up the fingerprint header of a layer.
    pub fn get(&self, layer_id: LayerId) -> Option<&DerivedLayerEntry> {
        self.entries.iter().find(|e| e.layer_id == layer_id)
    }

    /// Inserts or replaces an entry.
    pub fn upsert(&mut self, entry: DerivedLayerEntry) {
        match self
            .entries
            .iter_mut()
            .find(|e| e.layer_id == entry.layer_id)
        {
            Some(slot) => *slot = entry,
            None => self.entries.push(entry),
        }
    }
}

impl TlvValue for DerivedLayers {
    fn encode(&self) -> Vec<u8> {
        let mut w = Writer::new();
        w.write_u16(self.entries.len().min(u16::MAX as usize) as u16);
        for entry in self.entries.iter().take(u16::MAX as usize) {
            w.write_u16(entry.layer_id.raw());
            w.write_u64(entry.source_fingerprint);
            w.write_u64(entry.build_params_hash);
            w.write_u16(entry.algo_version);
            w.write_u16(entry.seeds.len().min(u16::MAX as usize) as u16);
            for seed in entry.seeds.iter().take(u16::MAX as usize) {
                w.write_u64(*seed);
            }
        }
        w.into_vec()
    }

    fn decode(bytes: &[u8]) -> Result<Self> {
        let mut r = Reader::new(bytes);
        let count = r.read_u16()? as usize;
        let mut entries = Vec::with_capacity(count);
        for _ in 0..count {
            let layer_id = LayerId(r.read_u16()?);
            let source_fingerprint = r.read_u64()?;
            let build_params_hash = r.read_u64()?;
            let algo_version = r.read_u16()?;
            let seed_count = r.read_u16()? as usize;
            let mut seeds = Vec::with_capacity(seed_count);
            for _ in 0..seed_count {
                seeds.push(r.read_u64()?);
            }
            entries.push(DerivedLayerEntry {
                layer_id,
                source_fingerprint,
                build_params_hash,
                algo_version,
                seeds,
            });
        }
        Ok(Self { entries })
    }
}

/// `ZSTD_DICT`: raw pretrained dictionary bytes.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ZstdDict(pub Vec<u8>);

impl TlvValue for ZstdDict {
    fn encode(&self) -> Vec<u8> {
        self.0.clone()
    }

    fn decode(bytes: &[u8]) -> Result<Self> {
        Ok(ZstdDict(bytes.to_vec()))
    }
}
