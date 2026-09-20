//! Layer identifiers, layer descriptors and the quantisation contract.
//!
//! Layers are grouped into id segments so that unrelated data never collides:
//!
//! | Segment  | Content                                                    |
//! |----------|------------------------------------------------------------|
//! | `0x00xx` | terrain: elevation (source), slope and EDT (derived)       |
//! | `0x10xx` | resistance feature channels                                |
//! | `0x20xx` | constraints: hard forbidden bitmap, direction field        |
//! | `0x25xx` | polygon region annotations                                 |
//! | `0x30xx` | graph structures: PRM, K-path library, vectors             |
//! | `0x40xx` | cached cost / visibility layers (fingerprint required)     |
//! | `0xF0xx` | vendor extensions                                          |

use serde::{Deserialize, Serialize};

use crate::bytes as le;
use crate::error::{MapError, Result};

/// Typed layer identifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct LayerId(pub u16);

impl LayerId {
    /// Elevation raster (source), metres.
    pub const ELEVATION: LayerId = LayerId(0x0001);
    /// Slope raster (derived): `i = |grad h|`.
    pub const SLOPE: LayerId = LayerId(0x0002);
    /// Distance transform (derived): channel 0 distance in 0.1 m, channel 1
    /// gradient direction quantised to 16 compass directions.
    pub const EDT: LayerId = LayerId(0x0003);
    /// First resistance feature layer id; feature `k` uses `FEATURE_BASE + k`.
    pub const FEATURE_BASE: u16 = 0x1000;
    /// Hard forbidden bitmap: bit set means the cell is impassable.
    pub const HARD_FORBIDDEN: LayerId = LayerId(0x2001);
    /// Direction constraint field: packed `angle u8 | strength u8`.
    pub const DIRECTION: LayerId = LayerId(0x2002);
    /// Soft multiplier field (rules that are not absolute).
    pub const SOFT_MULTIPLIER: LayerId = LayerId(0x2003);
    /// Polygon region annotations (multipath, dropout, magnetic disturbance).
    pub const REGIONS: LayerId = LayerId(0x2501);
    /// PRM waypoint graph.
    pub const PRM_GRAPH: LayerId = LayerId(0x3001);
    /// K-shortest-path candidate library.
    pub const KPATH_LIBRARY: LayerId = LayerId(0x3002);
    /// Region interface nodes stitching PRM to the fine grid.
    pub const REGION_INTERFACE: LayerId = LayerId(0x3003);
    /// Vector polylines and polygons.
    pub const VECTORS: LayerId = LayerId(0x3004);
    /// Cached cost field (optional, fingerprint required).
    pub const COST_CACHE: LayerId = LayerId(0x4001);

    /// Layer id of resistance feature `index`.
    #[inline]
    pub const fn feature(index: u8) -> LayerId {
        LayerId(Self::FEATURE_BASE + index as u16)
    }

    /// Raw identifier.
    #[inline]
    pub const fn raw(self) -> u16 {
        self.0
    }

    /// True for layers that are rebuildable caches and must carry a fingerprint.
    ///
    /// The builder and the reader share one source list per id
    /// (`MapBuilder::source_layers_of` and `Map::sources_of`); ids covered by
    /// neither list — `REGION_INTERFACE` and cache layers other than
    /// `COST_CACHE` — have no sources, so they always verify against an entry
    /// with an empty source fingerprint list rather than being treated as
    /// permanently stale. Such layers still may not be patched, because the
    /// fingerprint of a layer with no sources cannot detect an edit.
    #[inline]
    pub const fn is_derived(self) -> bool {
        matches!(self.0, 0x0002..=0x0003 | 0x3001..=0x3003 | 0x4000..=0x4FFF)
    }

    /// Human-readable segment name, for diagnostics.
    pub const fn segment_name(self) -> &'static str {
        match self.0 >> 8 {
            0x00 => "terrain",
            0x10 => "feature",
            0x20 => "constraint",
            0x25 => "region",
            0x30 => "graph",
            0x40 => "cache",
            0xF0 => "extension",
            _ => "unknown",
        }
    }
}

impl std::fmt::Display for LayerId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:#06x}", self.0)
    }
}

/// Payload organisation of a layer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[repr(u8)]
pub enum LayerKind {
    /// Regular raster, addressed by `(level, chunk_id)`.
    Raster = 0,
    /// Polyline / polygon geometry.
    Vector = 1,
    /// Graph structure (PRM, K-path library).
    Graph = 2,
    /// Bit-packed raster.
    Bitmap = 3,
    /// Polygon annotation layer.
    RegionPolygons = 4,
}

impl LayerKind {
    /// True for layer kinds addressed by `(level, chunk_id)` in the directory.
    ///
    /// Graph, region and vector layers are stored as single opaque payloads;
    /// they live in the same directory but carry `level = 0, chunk_id = 0` (or
    /// the batch index for multi-batch layers such as PRM).
    pub const fn is_chunked(self) -> bool {
        matches!(self, LayerKind::Raster | LayerKind::Bitmap)
    }

    /// Parses an on-disk identifier.
    pub const fn from_u8(value: u8) -> Option<Self> {
        match value {
            0 => Some(LayerKind::Raster),
            1 => Some(LayerKind::Vector),
            2 => Some(LayerKind::Graph),
            3 => Some(LayerKind::Bitmap),
            4 => Some(LayerKind::RegionPolygons),
            _ => None,
        }
    }

    /// Name used in diagnostics.
    pub const fn name(self) -> &'static str {
        match self {
            LayerKind::Raster => "raster",
            LayerKind::Vector => "vector",
            LayerKind::Graph => "graph",
            LayerKind::Bitmap => "bitmap",
            LayerKind::RegionPolygons => "region",
        }
    }
}

/// Element type of a raster channel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[repr(u8)]
pub enum DType {
    /// 32-bit float.
    F32 = 0,
    /// 32-bit signed integer.
    I32 = 1,
    /// 16-bit signed integer.
    I16 = 2,
    /// 8-bit unsigned integer.
    U8 = 3,
    /// Single bit.
    Bit = 4,
    /// 16-bit float.
    F16 = 5,
}

impl DType {
    /// Parses an on-disk identifier.
    pub const fn from_u8(value: u8) -> Option<Self> {
        match value {
            0 => Some(DType::F32),
            1 => Some(DType::I32),
            2 => Some(DType::I16),
            3 => Some(DType::U8),
            4 => Some(DType::Bit),
            5 => Some(DType::F16),
            _ => None,
        }
    }

    /// Storage size of one element in bytes. Bits report 1 byte per element in
    /// the *unpacked* sense; packed bitmaps are handled by the codec.
    pub const fn element_size(self) -> usize {
        match self {
            DType::F32 | DType::I32 => 4,
            DType::I16 | DType::F16 => 2,
            DType::U8 | DType::Bit => 1,
        }
    }

    /// True for floating-point element types.
    pub const fn is_float(self) -> bool {
        matches!(self, DType::F32 | DType::F16)
    }
}

/// Fixed 24-byte layer descriptor (`LAYER_TABLE` entry).
///
/// Quantisation contract: `real = raw * scale + bias`, applied to every stored
/// scalar of this layer regardless of codec.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct LayerDesc {
    /// Layer identifier.
    pub layer_id: LayerId,
    /// Payload organisation.
    pub kind: LayerKind,
    /// Number of channels per cell.
    pub channels: u8,
    /// Element type.
    pub dtype: DType,
    /// Default codec id; individual chunks may override it.
    pub codec: u8,
    /// Dequantisation scale.
    pub scale: f32,
    /// Dequantisation bias.
    pub bias: f32,
    /// Whether storage is sparse (absent cells are invalid).
    pub sparse: bool,
}

impl LayerDesc {
    /// Serialised size in bytes.
    pub const SIZE: usize = 24;

    /// Creates a descriptor with identity quantisation.
    pub fn new(
        layer_id: LayerId,
        kernel: LayerKind,
        channels: u8,
        dtype: DType,
        codec: u8,
    ) -> Self {
        Self {
            layer_id,
            kind: kernel,
            channels,
            dtype,
            codec,
            scale: 1.0,
            bias: 0.0,
            sparse: false,
        }
    }

    /// Sets the quantisation parameters.
    pub fn with_quantisation(mut self, scale: f32, bias: f32) -> Self {
        self.scale = scale;
        self.bias = bias;
        self
    }

    /// Converts a stored raw value into its real value.
    #[inline]
    pub fn dequantise(&self, raw: f32) -> f32 {
        raw * self.scale + self.bias
    }

    /// Converts a real value into the stored raw value.
    #[inline]
    pub fn quantise(&self, real: f32) -> f32 {
        if self.scale == 0.0 {
            0.0
        } else {
            (real - self.bias) / self.scale
        }
    }

    /// Serialises into the fixed layout.
    pub fn to_bytes(&self) -> [u8; Self::SIZE] {
        let mut buf = [0u8; Self::SIZE];
        let _ = le::put_u16(&mut buf, 0, self.layer_id.0);
        let _ = le::put_u8(&mut buf, 2, self.kind as u8);
        let _ = le::put_u8(&mut buf, 3, self.channels);
        let _ = le::put_u8(&mut buf, 4, self.dtype as u8);
        let _ = le::put_u8(&mut buf, 5, self.codec);
        let _ = le::put_f32(&mut buf, 6, self.scale);
        let _ = le::put_f32(&mut buf, 10, self.bias);
        let _ = le::put_u8(&mut buf, 14, self.sparse as u8);
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
        let layer_id = LayerId(le::get_u16(buf, 0)?);
        let kind_raw = le::get_u8(buf, 2)?;
        let kind = LayerKind::from_u8(kind_raw)
            .ok_or_else(|| MapError::invalid(format!("unknown layer kind {kind_raw}")))?;
        let dtype_raw = le::get_u8(buf, 4)?;
        let dtype = DType::from_u8(dtype_raw)
            .ok_or_else(|| MapError::invalid(format!("unknown dtype {dtype_raw}")))?;
        Ok(Self {
            layer_id,
            kind,
            channels: le::get_u8(buf, 3)?,
            dtype,
            codec: le::get_u8(buf, 5)?,
            scale: le::get_f32(buf, 6)?,
            bias: le::get_f32(buf, 10)?,
            sparse: le::get_u8(buf, 14)? != 0,
        })
    }
}
