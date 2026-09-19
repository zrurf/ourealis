//! General-purpose entropy codecs: zstd (with optional dictionary) and lz4.

use crate::error::{MapError, Result};

/// Default zstd compression level used for chunks.
pub const DEFAULT_ZSTD_LEVEL: i32 = 3;

/// Compresses with zstd, optionally using a pretrained dictionary.
pub fn zstd_compress(payload: &[u8], dict: Option<&[u8]>) -> Result<Vec<u8>> {
    match dict {
        Some(dict) if !dict.is_empty() => {
            let mut compressor = zstd::bulk::Compressor::with_dictionary(DEFAULT_ZSTD_LEVEL, dict)
                .map_err(|e| MapError::invalid(format!("zstd dictionary rejected: {e}")))?;
            compressor
                .compress(payload)
                .map_err(|e| MapError::invalid(format!("zstd compression failed: {e}")))
        }
        _ => zstd::bulk::compress(payload, DEFAULT_ZSTD_LEVEL)
            .map_err(|e| MapError::invalid(format!("zstd compression failed: {e}"))),
    }
}

/// Decompresses a zstd frame, requiring exactly `expected_len` bytes.
pub fn zstd_decompress(stored: &[u8], expected_len: usize, dict: Option<&[u8]>) -> Result<Vec<u8>> {
    let out = match dict {
        Some(dict) if !dict.is_empty() => {
            let mut decompressor = zstd::bulk::Decompressor::with_dictionary(dict)
                .map_err(|e| MapError::invalid(format!("zstd dictionary rejected: {e}")))?;
            decompressor
                .decompress(stored, expected_len)
                .map_err(|e| MapError::invalid(format!("zstd decompression failed: {e}")))?
        }
        _ => zstd::bulk::decompress(stored, expected_len)
            .map_err(|e| MapError::invalid(format!("zstd decompression failed: {e}")))?,
    };
    if out.len() != expected_len {
        return Err(MapError::invalid(format!(
            "zstd frame expanded to {} byte(s), expected {expected_len}",
            out.len()
        )));
    }
    Ok(out)
}

/// Largest meta block the reader will decode, bytes.
///
/// The meta block is a TLV sequence: map names, layer descriptions, feature
/// schemas, connector tables, weight priors. Even a large campus map with
/// hundreds of layers stays far below this; the bound exists so that a crafted
/// file cannot make the reader materialise an unbounded expansion while opening.
pub const MAX_META_BYTES: usize = 64 << 20;

/// Decompresses a zstd frame whose uncompressed size is unknown to the reader.
///
/// Used for the meta block, where the frame carries its own content size and no
/// trustworthy upper bound is available before decoding; the expansion is capped
/// at [`MAX_META_BYTES`] while decoding.
pub fn zstd_decompress_all(stored: &[u8]) -> Result<Vec<u8>> {
    zstd_decompress_bounded(stored, MAX_META_BYTES)
}

/// Decompresses a zstd frame whose exact uncompressed size is unknown, while
/// rejecting payloads larger than `max_len`.
///
/// Codecs that store their own header (run-length, sparse) do not know the
/// decompressed size up front; the bound comes from the chunk shape. The cap is
/// enforced while decoding, not after: a compressed payload can expand by
/// several orders of magnitude, so a post-hoc check would still materialise the
/// whole bomb first.
pub fn zstd_decompress_bounded(stored: &[u8], max_len: usize) -> Result<Vec<u8>> {
    use std::io::Read;

    let mut decoder = zstd::stream::read::Decoder::new(std::io::Cursor::new(stored))
        .map_err(|e| MapError::invalid(format!("zstd decompression failed: {e}")))?;
    let mut out = Vec::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let read = decoder
            .read(&mut buffer)
            .map_err(|e| MapError::invalid(format!("zstd decompression failed: {e}")))?;
        if read == 0 {
            return Ok(out);
        }
        if out.len() + read > max_len {
            return Err(MapError::invalid(format!(
                "zstd frame expanded beyond the {max_len} byte bound"
            )));
        }
        out.extend_from_slice(&buffer[..read]);
    }
}

/// Compresses with lz4 block format (no frame header, fastest to decode).
pub fn lz4_compress(payload: &[u8]) -> Vec<u8> {
    lz4_flex::block::compress_prepend_size(payload)
}

/// Decompresses an lz4 block written by [`lz4_compress`].
///
/// The size prefix is checked against `expected_len` *before* decoding, and the
/// payload is decoded into a buffer of exactly that size. `decompress_size_prepended`
/// allocates the size the payload declares, so a corrupt or hostile block would
/// otherwise reserve up to 4 GiB for a payload of a few bytes, and the length
/// check after the fact would come far too late.
pub fn lz4_decompress(stored: &[u8], expected_len: usize) -> Result<Vec<u8>> {
    let (declared, body) = lz4_flex::block::uncompressed_size(stored)
        .map_err(|e| MapError::invalid(format!("lz4 block header is invalid: {e}")))?;
    if declared != expected_len {
        return Err(MapError::invalid(format!(
            "lz4 block declares {declared} byte(s), expected {expected_len}"
        )));
    }
    lz4_flex::block::decompress(body, expected_len)
        .map_err(|e| MapError::invalid(format!("lz4 decompression failed: {e}")))
}
