//! Meta block: a TLV (tag–length–value) sequence carrying all metadata.
//!
//! The whole block is zstd-compressed as one frame; the raw TLV bytes produced
//! and consumed here are what the compressor sees. Parsing never fails on an
//! unknown tag: unrecognised records are preserved verbatim so a reader can
//! round-trip a file it does not fully understand.

pub mod value;

use crate::bytes::{Reader, Writer};
use crate::error::{MapError, Result};

/// Core tag namespace: metadata defined by the base specification.
pub const NS_CORE: u8 = 0x00;
/// Experimental namespace (draft specification stage).
pub const NS_EXPERIMENTAL: u8 = 0x01;
/// Lowest tag of the community extension range.
pub const NS_COMMUNITY_START: u8 = 0x10;
/// Lowest tag of the vendor-private range.
pub const NS_VENDOR_START: u8 = 0x80;
/// Debug namespace. Data using it must not appear in distributed files.
pub const NS_DEBUG: u8 = 0xFF;

/// Returns the namespace byte (high 8 bits) of a tag.
#[inline]
pub const fn namespace_of(tag: u32) -> u8 {
    (tag >> 24) as u8
}

/// Core metadata tags.
pub mod tag {
    /// Map name, build time, author, upstream data snapshot hash.
    pub const MAP_INFO: u32 = 0x0000_0001;
    /// Layer registry: one layer descriptor per raster or graph layer; see
    /// [`super::value::LayerTable`].
    pub const LAYER_TABLE: u32 = 0x0000_0002;
    /// Semantics of the D resistance feature dimensions.
    pub const FEATURE_SCHEMA: u32 = 0x0000_0003;
    /// Static prior weights per motion mode.
    pub const WEIGHT_PRIOR: u32 = 0x0000_0004;
    /// Slope–speed model parameters and stair speeds.
    pub const SLOPE_MODEL: u32 = 0x0000_0005;
    /// Z-axis connector table.
    pub const CONNECTOR_TABLE: u32 = 0x0000_0006;
    /// Optional pretrained zstd dictionary for chunk compression.
    pub const ZSTD_DICT: u32 = 0x0000_0007;
    /// Upstream provenance and algorithm version registry.
    pub const PROVENANCE: u32 = 0x0000_0008;
    /// Local geomagnetic field parameters.
    pub const MAGNETIC_FIELD: u32 = 0x0000_0009;
    /// Random seeds of the PRM batches stored in the graph layer.
    pub const PRM_SEEDS: u32 = 0x0000_000A;
    /// In-memory layout declaration for feature layers (GPU upload).
    pub const CHUNK_LAYOUT: u32 = 0x0000_000B;
    /// Per-channel global statistics and the minimum unit-cost bound.
    pub const GLOBAL_STATS: u32 = 0x0000_000C;
    /// Coarse-level aggregation rules and quantisation parameters.
    pub const AGGREGATION_RULES: u32 = 0x0000_000D;
    /// Fingerprint headers of every derived (cache) layer.
    ///
    /// Not present in the original specification table; without it a derived
    /// layer has nowhere to declare the sources it was built from, which the
    /// specification requires in order to reject stale caches.
    pub const DERIVED_LAYERS: u32 = 0x0000_000E;
}

/// A single TLV record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TlvRecord {
    /// Namespaced tag.
    pub tag: u32,
    /// Raw payload.
    pub value: Vec<u8>,
}

impl TlvRecord {
    /// Creates a record from a tag and an owned payload.
    pub fn new(tag: u32, value: Vec<u8>) -> Self {
        Self { tag, value }
    }

    /// Creates a record by serialising `value`.
    pub fn encode<T: TlvValue>(tag: u32, value: &T) -> Self {
        Self {
            tag,
            value: value.encode(),
        }
    }

    /// Decodes the payload into `T`.
    pub fn decode<T: TlvValue>(&self) -> Result<T> {
        T::decode(&self.value)
    }
}

/// Round-trippable payload of a TLV record.
pub trait TlvValue: Sized {
    /// Serialises the payload (without the tag and length header).
    fn encode(&self) -> Vec<u8>;
    /// Parses the payload.
    fn decode(bytes: &[u8]) -> Result<Self>;
}

/// A parsed meta block: records in file order plus a tag index.
///
/// Duplicate tags are kept in order; [`TlvBlock::get`] returns the first.
#[derive(Debug, Clone, Default)]
pub struct TlvBlock {
    records: Vec<TlvRecord>,
}

impl TlvBlock {
    /// Creates an empty block.
    pub fn new() -> Self {
        Self::default()
    }

    /// Appends a record.
    pub fn push(&mut self, record: TlvRecord) {
        self.records.push(record);
    }

    /// Replaces the first record with `tag`, or appends when absent.
    pub fn set(&mut self, record: TlvRecord) {
        match self.records.iter_mut().find(|r| r.tag == record.tag) {
            Some(slot) => *slot = record,
            None => self.records.push(record),
        }
    }

    /// Removes every record with `tag`, returning the count removed.
    pub fn remove(&mut self, tag: u32) -> usize {
        let before = self.records.len();
        self.records.retain(|r| r.tag != tag);
        before - self.records.len()
    }

    /// First record with `tag`.
    pub fn get(&self, tag: u32) -> Option<&TlvRecord> {
        self.records.iter().find(|r| r.tag == tag)
    }

    /// Decodes the first record with `tag`.
    pub fn get_as<T: TlvValue>(&self, tag: u32) -> Result<Option<T>> {
        self.get(tag).map(|r| r.decode::<T>()).transpose()
    }

    /// Decodes a record that callers declare as required.
    pub fn require<T: TlvValue>(&self, tag: u32, name: &'static str) -> Result<T> {
        self.get(tag)
            .ok_or(MapError::MissingTlv { tag, name })?
            .decode::<T>()
    }

    /// All records with `tag`.
    pub fn all(&self, tag: u32) -> impl Iterator<Item = &TlvRecord> {
        self.records.iter().filter(move |r| r.tag == tag)
    }

    /// Records in file order.
    pub fn records(&self) -> &[TlvRecord] {
        &self.records
    }

    /// Number of records.
    pub fn len(&self) -> usize {
        self.records.len()
    }

    /// True when no record is present.
    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }

    /// Serialises every record back into raw TLV bytes.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut w = Writer::with_capacity(self.records.iter().map(|r| 8 + r.value.len()).sum());
        for record in &self.records {
            w.write_u32(record.tag);
            w.write_u32(record.value.len() as u32);
            w.write_bytes(&record.value);
        }
        w.into_vec()
    }

    /// Parses raw TLV bytes. Unknown tags are preserved, never rejected.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self> {
        let mut reader = Reader::new(bytes);
        let mut records = Vec::new();
        while reader.remaining() > 0 {
            if reader.remaining() < 8 {
                return Err(MapError::invalid(format!(
                    "trailing {} byte(s) in TLV block are too short for a header",
                    reader.remaining()
                )));
            }
            let tag = reader.read_u32()?;
            let len = reader.read_u32()? as usize;
            let value = reader.read_bytes(len)?.to_vec();
            records.push(TlvRecord { tag, value });
        }
        Ok(Self { records })
    }
}

/// Writes a payload as: `u8` count followed by `count` `f32` values.
pub fn encode_f32_array(values: &[f32]) -> Vec<u8> {
    let mut w = Writer::with_capacity(1 + values.len() * 4);
    w.write_u8(values.len().min(u8::MAX as usize) as u8);
    for v in values.iter().take(u8::MAX as usize) {
        w.write_f32(*v);
    }
    w.into_vec()
}

/// Reads a payload written by [`encode_f32_array`].
pub fn decode_f32_array(bytes: &[u8]) -> Result<Vec<f32>> {
    let mut r = Reader::new(bytes);
    let count = r.read_u8()? as usize;
    r.read_f32_vec(count)
}

#[cfg(test)]
#[path = "../../tests/unit/tlv.rs"]
mod tests;
