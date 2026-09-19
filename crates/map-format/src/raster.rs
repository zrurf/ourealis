//! Raster chunk geometry, in-memory representation and the quantisation
//! boundary between them.
//!
//! Chunks are always full `chunk_size × chunk_size` cells, including at the map
//! edge, where out-of-bounds cells are stored as zero. Uniform chunk shapes
//! keep GPU uploads and codec arithmetic simple at the cost of a few padded
//! cells per layer.
//!
//! Payload order is channel-continuous — `index = (y * width + x) * channels + c`
//! — so a chunk can be handed to a GPU tensor as `[H][W][D]` without shuffling.

use crate::bytes as le;
use crate::codec::ChunkShape;
use crate::error::{MapError, Result};
use crate::geometry::{Aabb, morton_encode_chunk};
use crate::layer::{DType, LayerDesc, LayerId};

/// Chunk geometry of one map: bounds, base resolution and pyramid depth.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ChunkGrid {
    /// Map extent in the local metre plane.
    pub bounds: Aabb,
    /// Finest resolution in metres per cell (level 0).
    pub base_res_m: f64,
    /// Chunk side length in cells.
    pub chunk_size: u32,
    /// Number of LOD levels.
    pub lod_count: u8,
}

impl ChunkGrid {
    /// Creates a grid description.
    pub fn new(bounds: Aabb, base_res_m: f64, chunk_size: u32, lod_count: u8) -> Self {
        Self {
            bounds,
            base_res_m,
            chunk_size,
            lod_count,
        }
    }

    /// Cell size at `level` in metres.
    pub fn cell_size_m(&self, level: u8) -> f64 {
        self.base_res_m * 2f64.powi(level as i32)
    }

    /// Cell dimensions of the whole map at `level`.
    pub fn cell_dims(&self, level: u8) -> (u32, u32) {
        let cell = self.cell_size_m(level).max(f64::MIN_POSITIVE);
        (
            (self.bounds.width() / cell).ceil().max(1.0) as u32,
            (self.bounds.height() / cell).ceil().max(1.0) as u32,
        )
    }

    /// Chunk dimensions of the whole map at `level`.
    pub fn chunk_dims(&self, level: u8) -> (u32, u32) {
        let (cx, cy) = self.cell_dims(level);
        let size = self.chunk_size.max(1);
        (cx.div_ceil(size), cy.div_ceil(size))
    }

    /// Chunk index containing a position, or `None` when outside the map or
    /// when the grid has more chunks per axis than the 16-bit id can address.
    pub fn chunk_index_at(&self, x: f64, y: f64, level: u8) -> Option<(u16, u16)> {
        if !self.bounds.contains(x, y) {
            return None;
        }
        let cell = self.cell_size_m(level).max(f64::MIN_POSITIVE);
        let cx = ((x - self.bounds.min_x) / cell).floor().max(0.0) as u32;
        let cy = ((y - self.bounds.min_y) / cell).floor().max(0.0) as u32;
        let size = self.chunk_size.max(1);
        let (ix, iy) = (cx / size, cy / size);
        let (dim_x, dim_y) = self.chunk_dims(level);
        if ix >= dim_x
            || iy >= dim_y
            || !Self::axis_addressable(dim_x)
            || !Self::axis_addressable(dim_y)
        {
            return None;
        }
        Some((ix as u16, iy as u16))
    }

    /// Morton chunk id containing a position.
    pub fn chunk_id_at(&self, x: f64, y: f64, level: u8) -> Option<u32> {
        self.chunk_index_at(x, y, level)
            .map(|(ix, iy)| morton_encode_chunk(ix, iy))
    }

    /// Area covered by a chunk.
    pub fn chunk_bounds(&self, level: u8, chunk_id: u32) -> Option<Aabb> {
        let (ix, iy) = crate::geometry::morton_decode_chunk(chunk_id);
        let (dim_x, dim_y) = self.chunk_dims(level);
        if ix as u32 >= dim_x || iy as u32 >= dim_y {
            return None;
        }
        let span = self.cell_size_m(level) * self.chunk_size as f64;
        let min_x = self.bounds.min_x + ix as f64 * span;
        let min_y = self.bounds.min_y + iy as f64 * span;
        // Edge chunks extend past the map bounds; their trailing cells are the
        // zero padding described in the module documentation.
        Some(Aabb::new(min_x, min_y, min_x + span, min_y + span))
    }

    /// Chunk ids whose area intersects `bbox` at `level`.
    ///
    /// Returns an empty vector when the bbox or the grid is degenerate: a
    /// crafted header can derive a grid with billions of chunks per axis, and
    /// enumerating it would be indistinguishable from a hang.
    pub fn chunks_in_bbox(&self, bbox: &Aabb, level: u8) -> Vec<u32> {
        let span = self.cell_size_m(level) * self.chunk_size as f64;
        if span <= 0.0 || !span.is_finite() {
            return Vec::new();
        }
        let (dim_x, dim_y) = self.chunk_dims(level);
        if !Self::axis_addressable(dim_x) || !Self::axis_addressable(dim_y) {
            return Vec::new();
        }
        let clipped = match self.bounds.intersection(bbox) {
            Some(value) => value,
            None => return Vec::new(),
        };
        let ix0 = ((clipped.min_x - self.bounds.min_x) / span)
            .floor()
            .max(0.0)
            .min((dim_x - 1) as f64) as u32;
        let iy0 = ((clipped.min_y - self.bounds.min_y) / span)
            .floor()
            .max(0.0)
            .min((dim_y - 1) as f64) as u32;
        let ix1 = (((clipped.max_x - self.bounds.min_x) / span).floor() as u32).min(dim_x - 1);
        let iy1 = (((clipped.max_y - self.bounds.min_y) / span).floor() as u32).min(dim_y - 1);
        if ix0 > ix1 || iy0 > iy1 {
            return Vec::new();
        }
        // The per-axis guard above does not bound the product: 65 536 chunks on
        // each axis is 4.3e9 cells, which a header can ask for while still
        // passing `validate`. Enumerating that is indistinguishable from a hang,
        // so the query is refused rather than answered.
        let count = (ix1 - ix0 + 1) as u64 * (iy1 - iy0 + 1) as u64;
        if count > Self::MAX_BBOX_CHUNKS {
            tracing::warn!(
                "refusing a chunk query for {count} chunks (limit {}); the header describes a grid                  far beyond the map's declared extent",
                Self::MAX_BBOX_CHUNKS
            );
            return Vec::new();
        }
        let mut out = Vec::new();
        for iy in iy0..=iy1 {
            for ix in ix0..=ix1 {
                out.push(morton_encode_chunk(ix as u16, iy as u16));
            }
        }
        out
    }

    /// Largest number of chunk ids one bounding-box query will enumerate.
    ///
    /// Far above any real query — a 2 km map at 0.5 m resolution is about a
    /// hundred chunks per axis — and low enough that the enumeration cannot be
    /// used to exhaust memory from a crafted header.
    pub const MAX_BBOX_CHUNKS: u64 = 1 << 20;

    /// Largest chunk count per axis that a 16-bit chunk index can address.
    #[inline]
    fn axis_addressable(dim: u32) -> bool {
        dim >= 1 && dim <= u16::MAX as u32 + 1
    }

    /// Checks that the grid can be addressed and carries a positive cell size.
    ///
    /// Bounds, resolution and chunk size come from the header on the read path,
    /// so this is what keeps a crafted file from producing an unaddressable
    /// grid whose derived sizes overflow.
    pub fn validate(&self) -> Result<()> {
        if self.chunk_size == 0 {
            return Err(MapError::invalid("chunk_size must be non-zero"));
        }
        if !self.base_res_m.is_finite() || self.base_res_m <= 0.0 {
            return Err(MapError::invalid("base resolution must be positive"));
        }
        if !self.bounds.width().is_finite()
            || self.bounds.width() <= 0.0
            || !self.bounds.height().is_finite()
            || self.bounds.height() <= 0.0
        {
            return Err(MapError::invalid(
                "map bounds must have finite positive extent",
            ));
        }
        let (dim_x, dim_y) = self.chunk_dims(0);
        if !Self::axis_addressable(dim_x) || !Self::axis_addressable(dim_y) {
            return Err(MapError::invalid(format!(
                "map grid spans {dim_x} x {dim_y} chunk(s) per axis, more than the {} a 16-bit chunk index addresses",
                u16::MAX as u32 + 1
            )));
        }
        Ok(())
    }

    /// Global cell coordinates of a chunk's top-left corner.
    pub fn chunk_origin_cell(&self, chunk_id: u32) -> (u32, u32) {
        let (ix, iy) = crate::geometry::morton_decode_chunk(chunk_id);
        (ix as u32 * self.chunk_size, iy as u32 * self.chunk_size)
    }

    /// Shape of a chunk payload at `level`.
    ///
    /// Every level stores `chunk_size` cells per side. A level-`L` cell covers
    /// `base_res * 2^L` metres, so a level-`L` chunk spans `chunk_size * 2^L`
    /// base pixels — the same number of cells at every level, which is what
    /// makes the level-`L` chunk cover exactly four level-`L-1` chunks.
    pub fn chunk_shape(&self, desc: &LayerDesc, level: u8) -> ChunkShape {
        debug_assert!(
            level < self.lod_count.max(1),
            "LOD level outside the declared range"
        );
        let size = self.chunk_size.max(1);
        ChunkShape {
            width: size,
            height: size,
            channels: desc.channels.max(1),
            dtype: desc.dtype,
        }
    }

    /// Position of a global cell centre in metres.
    pub fn cell_center(&self, x: u32, y: u32, level: u8) -> (f64, f64) {
        let cell = self.cell_size_m(level);
        (
            self.bounds.min_x + (x as f64 + 0.5) * cell,
            self.bounds.min_y + (y as f64 + 0.5) * cell,
        )
    }
}

/// Decoded raster chunk in dequantised floating point form.
#[derive(Debug, Clone, PartialEq)]
pub struct RasterChunk {
    /// Chunk width in cells.
    pub width: u32,
    /// Chunk height in cells.
    pub height: u32,
    /// Channels per cell.
    pub channels: u8,
    /// Channel-continuous samples: `(y * width + x) * channels + c`.
    pub data: Vec<f32>,
}

impl RasterChunk {
    /// Allocates a zero-filled chunk.
    pub fn zeros(width: u32, height: u32, channels: u8) -> Self {
        let len = width as usize * height as usize * channels.max(1) as usize;
        Self {
            width,
            height,
            channels: channels.max(1),
            data: vec![0.0; len],
        }
    }

    /// Creates a chunk from existing data, checking the length.
    pub fn from_vec(width: u32, height: u32, channels: u8, data: Vec<f32>) -> Result<Self> {
        let expected = width as usize * height as usize * channels.max(1) as usize;
        if data.len() != expected {
            return Err(MapError::invalid(format!(
                "chunk data length {} does not match {width}x{height}x{channels}",
                data.len()
            )));
        }
        Ok(Self {
            width,
            height,
            channels: channels.max(1),
            data,
        })
    }

    /// Index of a sample in [`RasterChunk::data`].
    #[inline]
    pub fn index(&self, x: u32, y: u32, channel: u8) -> usize {
        ((y as usize * self.width as usize) + x as usize) * self.channels as usize
            + channel as usize
    }

    /// Reads one sample; out-of-range coordinates read as zero.
    #[inline]
    pub fn get(&self, x: u32, y: u32, channel: u8) -> f32 {
        if x >= self.width || y >= self.height || channel >= self.channels {
            return 0.0;
        }
        self.data[self.index(x, y, channel)]
    }

    /// Writes one sample.
    #[inline]
    pub fn set(&mut self, x: u32, y: u32, channel: u8, value: f32) {
        if x < self.width && y < self.height && channel < self.channels {
            let index = self.index(x, y, channel);
            self.data[index] = value;
        }
    }

    /// All channels of one cell.
    #[inline]
    pub fn cell(&self, x: u32, y: u32) -> &[f32] {
        let start = self.index(x, y, 0);
        &self.data[start..start + self.channels as usize]
    }

    /// Number of stored samples.
    pub fn len(&self) -> usize {
        self.data.len()
    }

    /// True when the chunk holds no sample (zero-sized shape).
    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }
}

/// Packs dequantised samples into the layer's stored element type.
///
/// Bit rows are padded to a byte boundary, matching
/// [`ChunkShape::row_bytes`]: a bitmap chunk occupies
/// `ceil(width / 8) * height` bytes, never one continuous run of bits.
pub fn pack(desc: &LayerDesc, chunk: &RasterChunk) -> Result<Vec<u8>> {
    let channels = chunk.channels.max(1);
    if desc.dtype == DType::Bit {
        if channels != 1 {
            return Err(MapError::invalid(
                "bit layers must have exactly one channel",
            ));
        }
        let row_bytes = (chunk.width as usize).div_ceil(8);
        let mut out = vec![0u8; row_bytes * chunk.height as usize];
        for y in 0..chunk.height {
            for x in 0..chunk.width {
                if chunk.get(x, y, 0) != 0.0 {
                    out[y as usize * row_bytes + x as usize / 8] |= 1 << (x % 8);
                }
            }
        }
        return Ok(out);
    }
    let elem = desc.dtype.element_size();
    let cells = chunk.width as usize * chunk.height as usize;
    let channels = channels as usize;
    let samples = cells
        .checked_mul(channels)
        .ok_or_else(|| MapError::invalid("chunk shape overflows"))?;
    let mut out = vec![
        0u8;
        samples
            .checked_mul(elem)
            .ok_or_else(|| MapError::invalid("chunk shape overflows"))?
    ];
    for y in 0..chunk.height {
        for x in 0..chunk.width {
            for channel in 0..channels {
                let index = ((y as usize * chunk.width as usize) + x as usize) * channels + channel;
                let raw = desc.quantise(chunk.get(x, y, channel as u8));
                write_element(&mut out[index * elem..(index + 1) * elem], raw, desc.dtype);
            }
        }
    }
    Ok(out)
}

/// Unpacks stored bytes into dequantised samples.
pub fn unpack(desc: &LayerDesc, shape: &ChunkShape, bytes: &[u8]) -> Result<RasterChunk> {
    let width = shape.width as usize;
    let height = shape.height as usize;
    let channels = shape.channels.max(1) as usize;
    let samples = width
        .checked_mul(height)
        .and_then(|cells| cells.checked_mul(channels))
        .ok_or_else(|| MapError::invalid("chunk shape overflows"))?;
    if desc.dtype == DType::Bit {
        // Rows are byte-padded, so the bound must match the row-wise indexing
        // below rather than a continuous bit run.
        let row_bits = width
            .checked_mul(channels)
            .ok_or_else(|| MapError::invalid("chunk shape overflows"))?;
        let row_bytes = row_bits.div_ceil(8);
        let expected = row_bytes
            .checked_mul(height)
            .ok_or_else(|| MapError::invalid("chunk shape overflows"))?;
        if bytes.len() < expected {
            return Err(MapError::Truncated {
                offset: 0,
                needed: expected,
                available: bytes.len(),
            });
        }
        let mut data = vec![0.0f32; samples];
        for y in 0..height {
            for x in 0..width {
                for channel in 0..channels {
                    let index = (y * width + x) * channels + channel;
                    let bit = x * channels + channel;
                    data[index] = if bytes[y * row_bytes + bit / 8] & (1 << (bit % 8)) != 0 {
                        1.0
                    } else {
                        0.0
                    };
                }
            }
        }
        return RasterChunk::from_vec(shape.width, shape.height, shape.channels, data);
    }
    let elem = desc.dtype.element_size();
    let expected = shape
        .total_bytes_checked()
        .ok_or_else(|| MapError::invalid("chunk shape overflows"))?;
    if bytes.len() != expected {
        return Err(MapError::invalid(format!(
            "stored payload has {} byte(s), expected {expected}",
            bytes.len()
        )));
    }
    // Allocate only after the length checks: the shape may come from an
    // untrusted header and must not drive the allocation on its own.
    let mut data = vec![0.0f32; samples];
    for y in 0..height {
        for x in 0..width {
            for channel in 0..channels {
                let index = (y * width + x) * channels + channel;
                let raw = read_element(&bytes[index * elem..(index + 1) * elem], desc.dtype);
                data[index] = desc.dequantise(raw);
            }
        }
    }
    RasterChunk::from_vec(shape.width, shape.height, shape.channels, data)
}

fn write_element(out: &mut [u8], raw: f32, dtype: DType) {
    match dtype {
        DType::F32 => {
            let _ = le::put_f32(out, 0, raw);
        }
        DType::F16 => {
            let bytes = half::f16::from_f32(raw).to_le_bytes();
            out[..2].copy_from_slice(&bytes);
        }
        DType::I32 => {
            let _ = le::put_i32(
                out,
                0,
                raw.round().clamp(i32::MIN as f32, i32::MAX as f32) as i32,
            );
        }
        DType::I16 => {
            let _ = le::put_i16(out, 0, raw.round().clamp(-32768.0, 32767.0) as i16);
        }
        DType::U8 | DType::Bit => {
            out[0] = raw.round().clamp(0.0, 255.0) as u8;
        }
    }
}

fn read_element(bytes: &[u8], dtype: DType) -> f32 {
    match dtype {
        DType::F32 => le::get_f32(bytes, 0).unwrap_or(0.0),
        DType::F16 => half::f16::from_le_bytes([bytes[0], bytes[1]]).to_f32(),
        DType::I32 => le::get_i32(bytes, 0).unwrap_or(0) as f32,
        DType::I16 => le::get_i16(bytes, 0).unwrap_or(0) as f32,
        DType::U8 | DType::Bit => bytes[0] as f32,
    }
}

/// Descriptor plus geometry of one raster layer, as seen by callers.
#[derive(Debug, Clone, PartialEq)]
pub struct RasterLayerView {
    desc: LayerDesc,
    grid: ChunkGrid,
    levels: Vec<u8>,
}

impl RasterLayerView {
    /// Creates a view over a registered raster layer.
    pub fn new(desc: LayerDesc, grid: ChunkGrid, mut levels: Vec<u8>) -> Self {
        levels.sort_unstable();
        levels.dedup();
        Self { desc, grid, levels }
    }

    /// Layer descriptor.
    #[inline]
    pub fn desc(&self) -> &LayerDesc {
        &self.desc
    }

    /// Layer id.
    #[inline]
    pub fn layer_id(&self) -> LayerId {
        self.desc.layer_id
    }

    /// Chunk geometry shared with the owning map.
    #[inline]
    pub fn grid(&self) -> &ChunkGrid {
        &self.grid
    }

    /// Levels that actually contain stored chunks, ascending.
    #[inline]
    pub fn levels(&self) -> &[u8] {
        &self.levels
    }

    /// True when the layer stores data at this level.
    pub fn has_level(&self, level: u8) -> bool {
        self.levels.binary_search(&level).is_ok()
    }

    /// Shape of a chunk of this layer at `level`.
    pub fn chunk_shape(&self, level: u8) -> ChunkShape {
        self.grid.chunk_shape(&self.desc, level)
    }

    /// Samples per cell.
    pub fn channels(&self) -> u8 {
        self.desc.channels.max(1)
    }
}

#[cfg(test)]
#[path = "../tests/unit/raster.rs"]
mod tests;
