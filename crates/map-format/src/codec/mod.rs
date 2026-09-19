//! Chunk codecs.
//!
//! Two responsibilities are deliberately separated:
//!
//! * **Quantisation** is a property of the layer: `LayerDesc::dtype` fixes the
//!   stored element type and `scale`/`bias` fix the `real = raw * scale + bias`
//!   mapping. Packing `f32` samples into stored bytes and back happens in
//!   [`crate::raster`], so every codec sees a plain byte string.
//! * **Codecs** are byte-level transforms applied to that string. Codec `0x40`
//!   ("quantised + zstd") therefore behaves like `0x10`: the quantisation it
//!   refers to is expressed by the layer descriptor, and the codec id documents
//!   the builder's intent.
//!
//! Every chunk is compressed independently — random access is the point of the
//! format, so whole-file compression is not an option.

pub mod delta;
pub mod entropy;
pub mod pyramid;
pub mod sparse;

use crate::error::{MapError, Result};
use crate::layer::DType;

/// Upper bound on the decompressed size of one chunk payload.
///
/// `ChunkRecord::raw_len` is a `u32`, so no chunk of a well-formed file can
/// exceed that; the tighter cap keeps shape-derived buffers bounded when the
/// shape comes from a hostile header. It leaves every realistic chunk
/// representable by a wide margin: `4096 × 4096` cells × 4 channels × `f32` is
/// exactly this size.
pub const MAX_RAW_CHUNK_BYTES: usize = 256 << 20;

/// Codec identifiers.
pub mod id {
    /// Uncompressed.
    pub const RAW: u8 = 0x00;
    /// zstd frame.
    pub const ZSTD: u8 = 0x10;
    /// lz4 block.
    pub const LZ4: u8 = 0x11;
    /// Vertical (row-to-row) delta followed by zstd.
    pub const DELTA_VERTICAL: u8 = 0x20;
    /// Channel-to-channel delta followed by zstd.
    pub const DELTA_CHANNEL: u8 = 0x21;
    /// Sparse: validity bitmask plus packed valid values.
    pub const SPARSE: u8 = 0x30;
    /// Run-length encoding followed by zstd.
    pub const RLE: u8 = 0x31;
    /// Quantised payload followed by zstd.
    pub const QUANTISED_ZSTD: u8 = 0x40;
    /// Pyramid: zstd with the parent chunk as a raw dictionary.
    pub const PYRAMID: u8 = 0x50;
}

/// Shape of a chunk payload, needed by codecs that work in rows or channels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChunkShape {
    /// Chunk width in cells.
    pub width: u32,
    /// Chunk height in cells.
    pub height: u32,
    /// Channels per cell.
    pub channels: u8,
    /// Stored element type.
    pub dtype: DType,
}

impl ChunkShape {
    /// Creates a shape.
    pub fn new(width: u32, height: u32, channels: u8, dtype: DType) -> Self {
        Self {
            width,
            height,
            channels,
            dtype,
        }
    }

    /// Bytes per cell (all channels).
    #[inline]
    pub fn cell_bytes(&self) -> usize {
        self.channels as usize * self.dtype.element_size()
    }

    /// Bytes per row.
    ///
    /// Bit layers are packed, so a row occupies one bit per cell and channel
    /// and is padded to a byte boundary. Saturates on overflow; allocating
    /// callers use [`ChunkShape::row_bytes_checked`].
    #[inline]
    pub fn row_bytes(&self) -> usize {
        self.row_bytes_checked().unwrap_or(usize::MAX)
    }

    /// Total payload bytes of the packed representation.
    ///
    /// Saturates at `usize::MAX` on shapes whose arithmetic would overflow;
    /// callers that allocate use [`ChunkShape::total_bytes_checked`].
    #[inline]
    pub fn total_bytes(&self) -> usize {
        self.total_bytes_checked().unwrap_or(usize::MAX)
    }

    /// Total payload bytes, or `None` when the arithmetic overflows `usize`.
    #[inline]
    pub fn total_bytes_checked(&self) -> Option<usize> {
        let row = self.row_bytes_checked()?;
        row.checked_mul(self.height as usize)
    }

    /// Bytes per row, or `None` when the arithmetic overflows `usize`.
    #[inline]
    pub fn row_bytes_checked(&self) -> Option<usize> {
        let width = self.width as usize;
        if self.dtype == DType::Bit {
            return width
                .checked_mul(self.channels as usize)?
                .checked_add(7)
                .map(|bits| bits / 8);
        }
        width.checked_mul(self.cell_bytes())
    }

    /// True for bit-packed payloads, which only support byte-oriented codecs.
    #[inline]
    pub fn is_bit_packed(&self) -> bool {
        self.dtype == DType::Bit
    }
}

/// Optional compression dictionary shared across chunks.
#[derive(Debug, Clone, Default)]
pub struct CodecContext<'a> {
    /// Pretrained zstd dictionary from the `ZSTD_DICT` TLV.
    pub dict: Option<&'a [u8]>,
    /// Parent chunk used as a raw dictionary by the pyramid codec.
    pub parent: Option<&'a [u8]>,
}

impl<'a> CodecContext<'a> {
    /// Context without dictionaries.
    pub fn none() -> Self {
        Self::default()
    }

    /// Context with a pretrained dictionary.
    pub fn with_dict(dict: &'a [u8]) -> Self {
        Self {
            dict: Some(dict),
            parent: None,
        }
    }
}

/// Human-readable codec name, for diagnostics.
pub fn name(codec: u8) -> &'static str {
    match codec {
        id::RAW => "raw",
        id::ZSTD => "zstd",
        id::LZ4 => "lz4",
        id::DELTA_VERTICAL => "delta-vertical+zstd",
        id::DELTA_CHANNEL => "delta-channel+zstd",
        id::SPARSE => "sparse",
        id::RLE => "rle+zstd",
        id::QUANTISED_ZSTD => "quantised+zstd",
        id::PYRAMID => "pyramid",
        _ => "unknown",
    }
}

/// True when the codec id is implemented by this reader.
pub fn is_supported(codec: u8) -> bool {
    matches!(
        codec,
        id::RAW
            | id::ZSTD
            | id::LZ4
            | id::DELTA_VERTICAL
            | id::DELTA_CHANNEL
            | id::SPARSE
            | id::RLE
            | id::QUANTISED_ZSTD
            | id::PYRAMID
    )
}

/// Compresses `payload` (already packed bytes) with `codec`.
pub fn encode(
    codec: u8,
    shape: &ChunkShape,
    payload: &[u8],
    ctx: &CodecContext<'_>,
) -> Result<Vec<u8>> {
    if payload.len() != shape.total_bytes() {
        return Err(MapError::invalid(format!(
            "payload length {} does not match chunk shape ({} bytes)",
            payload.len(),
            shape.total_bytes()
        )));
    }
    match codec {
        id::RAW => Ok(payload.to_vec()),
        id::ZSTD | id::QUANTISED_ZSTD => entropy::zstd_compress(payload, ctx.dict),
        id::LZ4 => Ok(entropy::lz4_compress(payload)),
        id::DELTA_VERTICAL => delta::vertical_encode(payload, shape, ctx),
        id::DELTA_CHANNEL => delta::channel_encode(payload, shape, ctx),
        id::SPARSE => sparse::encode(payload, shape),
        id::RLE => sparse::rle_encode(payload),
        id::PYRAMID => pyramid::encode(payload, ctx),
        other => Err(MapError::UnsupportedCodec {
            codec: other,
            layer_id: 0,
            level: 0,
            chunk_id: 0,
        }),
    }
}

/// Decompresses `stored` into a payload of exactly `shape.total_bytes()`.
pub fn decode(
    codec: u8,
    shape: &ChunkShape,
    stored: &[u8],
    ctx: &CodecContext<'_>,
) -> Result<Vec<u8>> {
    let out = match codec {
        id::RAW => stored.to_vec(),
        id::ZSTD | id::QUANTISED_ZSTD => {
            entropy::zstd_decompress(stored, shape.total_bytes(), ctx.dict)?
        }
        id::LZ4 => entropy::lz4_decompress(stored, shape.total_bytes())?,
        id::DELTA_VERTICAL => delta::vertical_decode(stored, shape, ctx)?,
        id::DELTA_CHANNEL => delta::channel_decode(stored, shape, ctx)?,
        id::SPARSE => sparse::decode(stored, shape)?,
        id::RLE => sparse::rle_decode(stored, shape.total_bytes())?,
        id::PYRAMID => pyramid::decode(stored, shape, ctx)?,
        other => {
            return Err(MapError::UnsupportedCodec {
                codec: other,
                layer_id: 0,
                level: 0,
                chunk_id: 0,
            });
        }
    };
    if out.len() != shape.total_bytes() {
        return Err(MapError::invalid(format!(
            "codec {} produced {} byte(s), expected {}",
            name(codec),
            out.len(),
            shape.total_bytes()
        )));
    }
    Ok(out)
}
