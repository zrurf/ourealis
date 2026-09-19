//! Sparse and run-length codecs.
//!
//! **Sparsity convention**: a cell is *valid* when any byte of any of its
//! channels is non-zero. Invalid cells are not stored and are reconstructed as
//! all-zero bytes, which makes the transform exactly reversible for the data
//! this format stores (an absent sample is written as zero by the builder).

use crate::bytes::{Reader, Writer};
use crate::codec::ChunkShape;
use crate::codec::entropy;
use crate::error::{MapError, Result};

/// Encodes a payload as `mask + valid values`, both zstd-compressed.
pub fn encode(payload: &[u8], shape: &ChunkShape) -> Result<Vec<u8>> {
    if shape.is_bit_packed() {
        return Err(MapError::invalid(
            "sparse coding is not defined for bit-packed chunks; use run-length coding",
        ));
    }
    let cell_bytes = shape.cell_bytes();
    if cell_bytes == 0 {
        return Err(MapError::invalid("chunk shape has zero-sized cells"));
    }
    if payload.len() != shape.total_bytes() {
        return Err(MapError::invalid(format!(
            "payload length {} does not match chunk shape ({} bytes)",
            payload.len(),
            shape.total_bytes()
        )));
    }
    let cells = shape.width as usize * shape.height as usize;
    let mask_len = cells.div_ceil(8);
    let mut mask = vec![0u8; mask_len];
    let mut values = Vec::new();
    for (index, cell) in payload.chunks_exact(cell_bytes).enumerate() {
        if cell.iter().any(|b| *b != 0) {
            mask[index / 8] |= 1 << (index % 8);
            values.extend_from_slice(cell);
        }
    }

    let mut body = Writer::with_capacity(8 + mask_len + values.len());
    body.write_u32(cells as u32);
    body.write_u32((values.len() / cell_bytes) as u32);
    body.write_bytes(&mask);
    body.write_bytes(&values);
    entropy::zstd_compress(body.as_slice(), None)
}

/// Inverse of [`encode`].
pub fn decode(stored: &[u8], shape: &ChunkShape) -> Result<Vec<u8>> {
    if shape.is_bit_packed() {
        return Err(MapError::invalid(
            "sparse coding is not defined for bit-packed chunks; use run-length coding",
        ));
    }
    let cell_bytes = shape.cell_bytes();
    let cells = (shape.width as usize)
        .checked_mul(shape.height as usize)
        .ok_or_else(|| MapError::invalid("chunk shape overflows"))?;
    let total = shape
        .total_bytes_checked()
        .ok_or_else(|| MapError::invalid("chunk shape overflows"))?;
    // The shape may come from an untrusted header; without a bound the
    // decompression buffer below would be sized by it alone.
    if total > super::MAX_RAW_CHUNK_BYTES {
        return Err(MapError::invalid(format!(
            "sparse chunk shape demands {total} byte(s), above the {}-byte limit",
            super::MAX_RAW_CHUNK_BYTES
        )));
    }
    let mask_len = cells.div_ceil(8);
    let capacity = 8usize
        .checked_add(mask_len)
        .and_then(|value| value.checked_add(total))
        .ok_or_else(|| MapError::invalid("chunk shape overflows"))?;
    let body = entropy::zstd_decompress_bounded(stored, capacity)?;

    let mut reader = Reader::new(&body);
    let stored_cells = reader.read_u32()? as usize;
    let valid_cells = reader.read_u32()? as usize;
    if stored_cells != cells {
        return Err(MapError::invalid(format!(
            "sparse chunk describes {stored_cells} cell(s), chunk shape has {cells}"
        )));
    }
    let mask = reader.read_bytes(mask_len)?;
    // The mask and the value count encode the same fact, so a mismatch means the
    // payload is corrupt. Checking it here is what keeps the value offset below
    // in range on hostile input.
    let set_cells: usize = mask.iter().map(|byte| byte.count_ones() as usize).sum();
    if set_cells != valid_cells {
        return Err(MapError::invalid(format!(
            "sparse chunk mask sets {set_cells} cell(s) but carries {valid_cells} value(s)"
        )));
    }
    let values = reader.read_bytes(valid_cells * cell_bytes)?;

    let mut out = vec![0u8; total];
    let mut value_index = 0usize;
    for cell in 0..cells {
        let present = mask[cell / 8] & (1 << (cell % 8)) != 0;
        if present {
            let start = cell * cell_bytes;
            out[start..start + cell_bytes]
                .copy_from_slice(&values[value_index * cell_bytes..(value_index + 1) * cell_bytes]);
            value_index += 1;
        }
    }
    Ok(out)
}

/// Run-length encodes a byte stream, then compresses it.
///
/// Only meaningful for one-byte element types (category ids, bitmaps), which is
/// what the specification prescribes it for.
pub fn rle_encode(payload: &[u8]) -> Result<Vec<u8>> {
    let mut body: Vec<u8> = Vec::with_capacity(payload.len());
    let mut iter = payload.iter().copied().peekable();
    while let Some(value) = iter.next() {
        let mut run = 1u16;
        while run < u16::MAX && iter.peek() == Some(&value) {
            iter.next();
            run += 1;
        }
        body.push(value);
        body.extend_from_slice(&run.to_le_bytes());
    }
    entropy::zstd_compress(&body, None)
}

/// Inverse of [`rle_encode`].
// `array_chunks` would express this more directly but is not stable yet.
#[allow(clippy::chunks_exact_to_as_chunks)]
pub fn rle_decode(stored: &[u8], expected_len: usize) -> Result<Vec<u8>> {
    // A run costs three bytes for at least one output byte, so the expanded
    // stream never exceeds three times the payload length.
    let body = entropy::zstd_decompress_bounded(stored, expected_len.saturating_mul(3).max(8))?;
    if body.len() % 3 != 0 {
        return Err(MapError::invalid(format!(
            "run-length payload length {} is not a multiple of 3",
            body.len()
        )));
    }
    let mut out = Vec::with_capacity(expected_len);
    for run in body.chunks_exact(3) {
        let value = run[0];
        let len = u16::from_le_bytes([run[1], run[2]]) as usize;
        // A single run may claim up to 65535 bytes, so the declared length has
        // to be checked against the output on every step; deferring it to the
        // caller would let a tiny payload expand to hundreds of megabytes.
        if out.len() + len > expected_len {
            return Err(MapError::invalid(format!(
                "run-length payload expands beyond the expected {expected_len} byte(s)"
            )));
        }
        out.resize(out.len() + len, value);
    }
    Ok(out)
}
