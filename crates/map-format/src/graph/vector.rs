//! Vector geometry: polylines and polygons.
//!
//! Used for map editing anchors, predefined loop circuits referenced by
//! connector `attr_ref`, region outlines and export. The planning algorithms
//! never depend on this layer.

use crate::bytes::{Reader, Writer};
use crate::error::{MapError, Result};

/// Geometry kind of a vector feature.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum VectorKind {
    /// Open polyline.
    Polyline = 0,
    /// Closed polygon; the first point is not repeated at the end.
    Polygon = 1,
}

impl VectorKind {
    /// Parses an on-disk identifier.
    pub const fn from_u8(value: u8) -> Option<Self> {
        match value {
            0 => Some(VectorKind::Polyline),
            1 => Some(VectorKind::Polygon),
            _ => None,
        }
    }
}

/// One vector feature.
#[derive(Debug, Clone, PartialEq)]
pub struct VectorShape {
    /// Feature identifier, referenced by `Connector::attr_ref` and regions.
    pub id: u32,
    /// Geometry kind.
    pub kind: VectorKind,
    /// Vertices in local metres.
    pub points: Vec<[f32; 2]>,
    /// Optional numeric attributes `(key, value)`.
    pub attributes: Vec<(u16, f32)>,
}

impl VectorShape {
    /// Creates an empty shape.
    pub fn new(id: u32, kind: VectorKind) -> Self {
        Self {
            id,
            kind,
            points: Vec::new(),
            attributes: Vec::new(),
        }
    }

    /// Serialises the shape.
    pub fn encode(&self) -> Vec<u8> {
        let mut w = Writer::with_capacity(8 + self.points.len() * 8);
        w.write_u32(self.id);
        w.write_u8(self.kind as u8);
        w.write_u16(self.points.len().min(u16::MAX as usize) as u16);
        for point in self.points.iter().take(u16::MAX as usize) {
            w.write_f32(point[0]);
            w.write_f32(point[1]);
        }
        w.write_u16(self.attributes.len().min(u16::MAX as usize) as u16);
        for (key, value) in self.attributes.iter().take(u16::MAX as usize) {
            w.write_u16(*key);
            w.write_f32(*value);
        }
        w.into_vec()
    }

    /// Parses a shape.
    pub fn decode(bytes: &[u8]) -> Result<Self> {
        let mut r = Reader::new(bytes);
        let id = r.read_u32()?;
        let kind_raw = r.read_u8()?;
        let kind = VectorKind::from_u8(kind_raw)
            .ok_or_else(|| MapError::invalid(format!("unknown vector kind {kind_raw}")))?;
        let point_count = r.read_u16()? as usize;
        let mut points = Vec::with_capacity(point_count);
        for _ in 0..point_count {
            points.push([r.read_f32()?, r.read_f32()?]);
        }
        let attr_count = r.read_u16()? as usize;
        let mut attributes = Vec::with_capacity(attr_count);
        for _ in 0..attr_count {
            attributes.push((r.read_u16()?, r.read_f32()?));
        }
        Ok(Self {
            id,
            kind,
            points,
            attributes,
        })
    }
}

/// Collection of vector features.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct VectorLayer {
    /// All shapes in the layer.
    pub shapes: Vec<VectorShape>,
}

impl VectorLayer {
    /// Looks up a shape by id.
    pub fn get(&self, id: u32) -> Option<&VectorShape> {
        self.shapes.iter().find(|s| s.id == id)
    }

    /// Total vertex count, useful for sizing buffers.
    pub fn vertex_count(&self) -> usize {
        self.shapes.iter().map(|s| s.points.len()).sum()
    }

    /// Serialises the layer.
    pub fn encode(&self) -> Vec<u8> {
        let mut body = Writer::new();
        body.write_u32(self.shapes.len() as u32);
        for shape in &self.shapes {
            let encoded = shape.encode();
            body.write_u32(encoded.len() as u32);
            body.write_bytes(&encoded);
        }
        super::write_section(super::SectionId::Vectors, body.as_slice())
    }

    /// Parses a layer payload.
    pub fn decode(bytes: &[u8]) -> Result<Self> {
        let mut sections = super::SectionReader::new(bytes);
        let Some(payload) = sections.find(super::SectionId::Vectors)? else {
            return Ok(Self::default());
        };
        let mut r = Reader::new(payload);
        let count = r.read_u32()? as usize;
        // Each shape needs its 4-byte length prefix plus the 9-byte fixed
        // header; a count that cannot fit in the remaining bytes is corrupt.
        const MIN_SHAPE_BYTES: usize = 4 + 4 + 1 + 2 + 2;
        let needed = count
            .checked_mul(MIN_SHAPE_BYTES)
            .ok_or_else(|| MapError::invalid("vector shape count overflows"))?;
        if needed > r.remaining() {
            return Err(MapError::Truncated {
                offset: r.position() as u64,
                needed,
                available: r.remaining(),
            });
        }
        let mut shapes = Vec::with_capacity(count);
        for _ in 0..count {
            let len = r.read_u32()? as usize;
            shapes.push(VectorShape::decode(r.read_bytes(len)?)?);
        }
        Ok(Self { shapes })
    }
}
