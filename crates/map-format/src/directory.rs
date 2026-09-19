//! Chunk directory: the addressing layer that maps `(layer, level, chunk)` to a
//! byte range in the file.
//!
//! The directory is a flat array sorted by `(layer_id, level, chunk_id)`, which
//! makes both exact lookup and "all chunks of a layer at a level" binary
//! searches. A missing record is a meaningful state — it means the area is
//! described at a coarser granularity, not that the file is broken.

use crate::bytes as le;
use crate::error::{MapError, Result};
use crate::layer::LayerId;

/// One directory record.
///
/// `offset` is a **file-absolute** byte offset. The specification described it
/// as relative to the chunk data region, but that region's start is not
/// recorded anywhere (it shifts when the extension area is present), so an
/// absolute offset is used instead; the byte layout is unchanged.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChunkRecord {
    /// Morton code of the chunk inside its `(layer, level)` grid.
    pub chunk_id: u32,
    /// Owning layer.
    pub layer_id: LayerId,
    /// LOD level; 0 is the finest.
    pub level: u8,
    /// Codec of this chunk, overriding the layer default.
    pub codec: u8,
    /// File-absolute offset of the compressed payload.
    pub offset: u64,
    /// Compressed payload length in bytes.
    pub comp_len: u32,
    /// Length of the payload after decompression.
    pub raw_len: u32,
    /// CRC32 of the stored (compressed) payload.
    pub crc32: u32,
    /// Status bits, see [`ChunkRecord::MEAN_BLOCK`] and friends.
    pub flags: u32,
}

impl ChunkRecord {
    /// Chunk stores aggregate means rather than fine data.
    pub const MEAN_BLOCK: u32 = 1 << 0;
    /// Chunk stores aggregate maxima.
    pub const MAX_BLOCK: u32 = 1 << 1;
    /// Chunk is sparsely stored (mask plus valid values).
    pub const SPARSE_BLOCK: u32 = 1 << 2;
    /// Chunk is a tombstone: present in a patched directory, data removed.
    pub const TOMBSTONE: u32 = 1 << 31;

    /// Serialised size in bytes.
    pub const SIZE: usize = 32;

    /// Creates a record with no flags.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        layer_id: LayerId,
        level: u8,
        chunk_id: u32,
        codec: u8,
        offset: u64,
        comp_len: u32,
        raw_len: u32,
        crc32: u32,
    ) -> Self {
        Self {
            chunk_id,
            layer_id,
            level,
            codec,
            offset,
            comp_len,
            raw_len,
            crc32,
            flags: 0,
        }
    }

    /// Sort key of the directory.
    #[inline]
    pub fn sort_key(&self) -> (u16, u8, u32) {
        (self.layer_id.raw(), self.level, self.chunk_id)
    }

    /// Serialises into the fixed layout.
    pub fn to_bytes(&self) -> [u8; Self::SIZE] {
        let mut buf = [0u8; Self::SIZE];
        let _ = le::put_u32(&mut buf, 0, self.chunk_id);
        let _ = le::put_u16(&mut buf, 4, self.layer_id.raw());
        let _ = le::put_u8(&mut buf, 6, self.level);
        let _ = le::put_u8(&mut buf, 7, self.codec);
        let _ = le::put_u64(&mut buf, 8, self.offset);
        let _ = le::put_u32(&mut buf, 16, self.comp_len);
        let _ = le::put_u32(&mut buf, 20, self.raw_len);
        let _ = le::put_u32(&mut buf, 24, self.crc32);
        let _ = le::put_u32(&mut buf, 28, self.flags);
        buf
    }

    /// Parses the fixed layout.
    pub fn from_bytes(buf: &[u8]) -> Result<Self> {
        if buf.len() < Self::SIZE {
            return Err(MapError::Truncated {
                offset: 0,
                needed: Self::SIZE,
                available: buf.len(),
            });
        }
        Ok(Self {
            chunk_id: le::get_u32(buf, 0)?,
            layer_id: LayerId(le::get_u16(buf, 4)?),
            level: le::get_u8(buf, 6)?,
            codec: le::get_u8(buf, 7)?,
            offset: le::get_u64(buf, 8)?,
            comp_len: le::get_u32(buf, 16)?,
            raw_len: le::get_u32(buf, 20)?,
            crc32: le::get_u32(buf, 24)?,
            flags: le::get_u32(buf, 28)?,
        })
    }
}

/// Sorted chunk directory.
#[derive(Debug, Clone, Default)]
pub struct ChunkDirectory {
    records: Vec<ChunkRecord>,
}

impl ChunkDirectory {
    /// Builds a directory from records in any order, sorting them.
    pub fn new(mut records: Vec<ChunkRecord>) -> Self {
        records.sort_unstable_by_key(ChunkRecord::sort_key);
        Self { records }
    }

    /// All records in sorted order.
    #[inline]
    pub fn records(&self) -> &[ChunkRecord] {
        &self.records
    }

    /// Number of records.
    #[inline]
    pub fn len(&self) -> usize {
        self.records.len()
    }

    /// True when the directory holds no record.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }

    /// Exact lookup.
    pub fn find(&self, layer_id: LayerId, level: u8, chunk_id: u32) -> Option<&ChunkRecord> {
        let key = (layer_id.raw(), level, chunk_id);
        self.records
            .binary_search_by_key(&key, ChunkRecord::sort_key)
            .ok()
            .map(|i| &self.records[i])
    }

    /// All records of one `(layer, level)` pair, contiguous in the sorted array.
    pub fn for_layer_level(&self, layer_id: LayerId, level: u8) -> &[ChunkRecord] {
        let prefix = (layer_id.raw(), level);
        let start = self
            .records
            .partition_point(|r| (r.layer_id.raw(), r.level) < prefix);
        let end = self
            .records
            .partition_point(|r| (r.layer_id.raw(), r.level) <= prefix);
        &self.records[start..end]
    }

    /// Every distinct `(layer, level)` pair present, in sorted order.
    pub fn layer_levels(&self) -> Vec<(LayerId, u8)> {
        let mut out: Vec<(LayerId, u8)> = Vec::new();
        for record in &self.records {
            let key = (record.layer_id, record.level);
            if out.last() != Some(&key) {
                out.push(key);
            }
        }
        out
    }

    /// Layers present in the directory.
    pub fn layers(&self) -> Vec<LayerId> {
        let mut out: Vec<LayerId> = Vec::new();
        for record in &self.records {
            if out.last() != Some(&record.layer_id) {
                out.push(record.layer_id);
            }
        }
        out
    }

    /// Adds or replaces a record, keeping the sort order.
    pub fn upsert(&mut self, record: ChunkRecord) {
        let key = record.sort_key();
        match self
            .records
            .binary_search_by_key(&key, ChunkRecord::sort_key)
        {
            Ok(index) => self.records[index] = record,
            Err(index) => self.records.insert(index, record),
        }
    }

    /// Removes a record, returning it when present.
    pub fn remove(&mut self, layer_id: LayerId, level: u8, chunk_id: u32) -> Option<ChunkRecord> {
        let key = (layer_id.raw(), level, chunk_id);
        self.records
            .binary_search_by_key(&key, ChunkRecord::sort_key)
            .ok()
            .map(|index| self.records.remove(index))
    }

    /// Serialises the directory.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.records.len() * ChunkRecord::SIZE);
        for record in &self.records {
            out.extend_from_slice(&record.to_bytes());
        }
        out
    }

    /// Parses a directory image.
    // `array_chunks` would express this more directly but is not stable yet.
    #[allow(clippy::chunks_exact_to_as_chunks)]
    pub fn from_bytes(bytes: &[u8]) -> Result<Self> {
        if !bytes.len().is_multiple_of(ChunkRecord::SIZE) {
            return Err(MapError::invalid(format!(
                "directory length {} is not a multiple of {}",
                bytes.len(),
                ChunkRecord::SIZE
            )));
        }
        let mut records = Vec::with_capacity(bytes.len() / ChunkRecord::SIZE);
        for chunk in bytes.chunks_exact(ChunkRecord::SIZE) {
            records.push(ChunkRecord::from_bytes(chunk)?);
        }
        Ok(Self::new(records))
    }

    /// Total compressed bytes referenced by this directory.
    pub fn total_compressed_bytes(&self) -> u64 {
        self.records.iter().map(|r| r.comp_len as u64).sum()
    }

    /// Total uncompressed bytes referenced by this directory.
    pub fn total_raw_bytes(&self) -> u64 {
        self.records.iter().map(|r| r.raw_len as u64).sum()
    }
}
