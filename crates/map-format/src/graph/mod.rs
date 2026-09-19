//! Graph structure layers: PRM waypoint graphs, K-shortest-path libraries and
//! vector geometry.
//!
//! These layers do not participate in the raster chunking scheme. They are
//! stored as graph-shaped payloads in the same directory (registered under
//! their own layer ids) and are addressed by a section id instead of a Morton
//! chunk id, so a reader loads the section it needs.

pub mod kpath;
pub mod prm;
pub mod vector;

use crate::bytes::{Reader, Writer};
use crate::error::{MapError, Result};

/// Section identifiers inside a graph layer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u16)]
pub enum SectionId {
    /// PRM node table.
    PrmNodes = 0,
    /// PRM CSR offsets (one entry more than the node count).
    PrmOffsets = 1,
    /// PRM edge table.
    PrmEdges = 2,
    /// Region interface links stitching PRM nodes to the fine grid.
    PrmInterfaces = 3,
    /// K-path parameter block.
    KPathParams = 10,
    /// OD key index (sorted).
    KPathOdIndex = 11,
    /// Path sets.
    KPathSets = 12,
    /// Sample point table shared by all path sets.
    KPathNodes = 13,
    /// Vector geometry blobs.
    Vectors = 20,
}

impl SectionId {
    /// Parses an on-disk identifier.
    pub const fn from_u16(value: u16) -> Option<Self> {
        match value {
            0 => Some(SectionId::PrmNodes),
            1 => Some(SectionId::PrmOffsets),
            2 => Some(SectionId::PrmEdges),
            3 => Some(SectionId::PrmInterfaces),
            10 => Some(SectionId::KPathParams),
            11 => Some(SectionId::KPathOdIndex),
            12 => Some(SectionId::KPathSets),
            13 => Some(SectionId::KPathNodes),
            20 => Some(SectionId::Vectors),
            _ => None,
        }
    }
}

/// Envelope written ahead of each graph section payload.
///
/// Keeping the section id and byte length in the payload lets a reader skip
/// sections it does not know without understanding their content.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SectionHeader {
    /// Section identifier.
    pub section: u16,
    /// Payload length in bytes.
    pub len: u32,
}

impl SectionHeader {
    /// Serialised size in bytes.
    pub const SIZE: usize = 8;

    /// Creates a header for `len` payload bytes.
    pub fn new(section: SectionId, len: usize) -> Self {
        Self {
            section: section as u16,
            len: len.min(u32::MAX as usize) as u32,
        }
    }

    /// Serialises the header.
    pub fn to_bytes(&self) -> [u8; Self::SIZE] {
        let mut buf = [0u8; Self::SIZE];
        let _ = crate::bytes::put_u16(&mut buf, 0, self.section);
        let _ = crate::bytes::put_u16(&mut buf, 2, 0);
        let _ = crate::bytes::put_u32(&mut buf, 4, self.len);
        buf
    }

    /// Parses a header.
    pub fn from_bytes(buf: &[u8]) -> Result<Self> {
        if buf.len() < Self::SIZE {
            return Err(MapError::Truncated {
                offset: 0,
                needed: Self::SIZE,
                available: buf.len(),
            });
        }
        Ok(Self {
            section: crate::bytes::get_u16(buf, 0)?,
            len: crate::bytes::get_u32(buf, 4)?,
        })
    }
}

/// Writes a section: header followed by payload.
pub fn write_section(section: SectionId, payload: &[u8]) -> Vec<u8> {
    let mut w = Writer::with_capacity(SectionHeader::SIZE + payload.len());
    w.write_bytes(&SectionHeader::new(section, payload.len()).to_bytes());
    w.write_bytes(payload);
    w.into_vec()
}

/// Reads sections sequentially, skipping unknown ones.
pub struct SectionReader<'a> {
    reader: Reader<'a>,
}

impl<'a> SectionReader<'a> {
    /// Creates a reader over a graph payload.
    pub fn new(bytes: &'a [u8]) -> Self {
        Self {
            reader: Reader::new(bytes),
        }
    }

    /// Reads the next section header and payload, or `None` at the end.
    pub fn next_section(&mut self) -> Result<Option<(u16, &'a [u8])>> {
        if self.reader.remaining() == 0 {
            return Ok(None);
        }
        let header = SectionHeader::from_bytes(self.reader.read_bytes(SectionHeader::SIZE)?)?;
        let payload = self.reader.read_bytes(header.len as usize)?;
        Ok(Some((header.section, payload)))
    }

    /// Returns the payload of the first section with `section`, if present.
    pub fn find(&mut self, section: SectionId) -> Result<Option<&'a [u8]>> {
        while let Some((id, payload)) = self.next_section()? {
            if id == section as u16 {
                return Ok(Some(payload));
            }
        }
        Ok(None)
    }
}
