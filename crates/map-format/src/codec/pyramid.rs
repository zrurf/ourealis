//! Pyramid codec: zstd with the parent chunk as a raw-content dictionary.
//!
//! Elevation and distance fields are smooth, so a coarse parent chunk is an
//! excellent predictor for its children. Rather than inventing a bespoke
//! predictor, the parent bytes are handed to zstd as a dictionary: no extra
//! format, and decoding only needs the parent, which the caller already has
//! when walking the LOD pyramid.
//!
//! When no parent is supplied the codec degrades to plain zstd.

use crate::codec::entropy;
use crate::codec::{ChunkShape, CodecContext};
use crate::error::Result;

/// Compresses `payload`, using the parent chunk as dictionary when available.
pub fn encode(payload: &[u8], ctx: &CodecContext<'_>) -> Result<Vec<u8>> {
    let dict = ctx.parent.or(ctx.dict);
    entropy::zstd_compress(payload, dict)
}

/// Inverse of [`encode`].
pub fn decode(stored: &[u8], shape: &ChunkShape, ctx: &CodecContext<'_>) -> Result<Vec<u8>> {
    let dict = ctx.parent.or(ctx.dict);
    entropy::zstd_decompress(stored, shape.total_bytes(), dict)
}
