//! Delta codecs: row-to-row and channel-to-channel differencing.
//!
//! Differencing runs on the stored element type with wrapping arithmetic, so a
//! chunk can be reconstructed byte-exactly regardless of dtype. The differenced
//! bytes then go through zstd, which is where the gain comes from: terrain and
//! derived fields are spatially smooth, so their deltas are small and highly
//! repetitive.

use crate::bytes as le;
use crate::codec::entropy;
use crate::codec::{ChunkShape, CodecContext};
use crate::error::{MapError, Result};
use crate::layer::DType;

/// Subtracts the previous row from each row, then compresses.
pub fn vertical_encode(
    payload: &[u8],
    shape: &ChunkShape,
    ctx: &CodecContext<'_>,
) -> Result<Vec<u8>> {
    let stride = shape.row_bytes();
    let mut out = payload.to_vec();
    for row in (1..shape.height as usize).rev() {
        // `split_at_mut` already moved `cur` past the previous rows, so the
        // current row is simply its first `stride` bytes.
        let (prev, cur) = out.split_at_mut(row * stride);
        let prev_row = &prev[(row - 1) * stride..row * stride];
        let cur_row = &mut cur[..stride];
        subtract_in_place(cur_row, prev_row, shape.dtype);
    }
    entropy::zstd_compress(&out, ctx.dict)
}

/// Inverse of [`vertical_encode`].
pub fn vertical_decode(
    stored: &[u8],
    shape: &ChunkShape,
    ctx: &CodecContext<'_>,
) -> Result<Vec<u8>> {
    let stride = shape.row_bytes();
    let mut out = entropy::zstd_decompress(stored, shape.total_bytes(), ctx.dict)?;
    for row in 1..shape.height as usize {
        let (prev, cur) = out.split_at_mut(row * stride);
        let prev_row = &prev[(row - 1) * stride..row * stride];
        let cur_row = &mut cur[..stride];
        add_in_place(cur_row, prev_row, shape.dtype);
    }
    Ok(out)
}

/// Subtracts channel 0 from every other channel, then compresses.
pub fn channel_encode(
    payload: &[u8],
    shape: &ChunkShape,
    ctx: &CodecContext<'_>,
) -> Result<Vec<u8>> {
    if shape.channels < 2 {
        return Err(MapError::invalid(
            "channel delta needs at least two channels; use another codec",
        ));
    }
    // Bit-packed chunks pad every row to a byte boundary, so one cell's packed
    // bytes are not `channels * element_size` apart; the stride below would walk
    // past the payload.
    if shape.is_bit_packed() {
        return Err(MapError::invalid(
            "channel delta is not defined for bit-packed chunks; use run-length coding",
        ));
    }
    let elem = shape.dtype.element_size();
    let mut out = payload.to_vec();
    let cells = shape.width as usize * shape.height as usize;
    for cell in 0..cells {
        let base = cell * shape.channels as usize * elem;
        let reference = out[base..base + elem].to_vec();
        for channel in 1..shape.channels as usize {
            let at = base + channel * elem;
            let mut current = out[at..at + elem].to_vec();
            subtract_bytes(&mut current, &reference, shape.dtype);
            out[at..at + elem].copy_from_slice(&current);
        }
    }
    entropy::zstd_compress(&out, ctx.dict)
}

/// Inverse of [`channel_encode`].
pub fn channel_decode(
    stored: &[u8],
    shape: &ChunkShape,
    ctx: &CodecContext<'_>,
) -> Result<Vec<u8>> {
    if shape.channels < 2 {
        return Err(MapError::invalid(
            "channel delta needs at least two channels; use another codec",
        ));
    }
    // See `channel_encode`: the cell stride below is only valid for a dense
    // layout, and a crafted header must not be able to drive it out of bounds.
    if shape.is_bit_packed() {
        return Err(MapError::invalid(
            "channel delta is not defined for bit-packed chunks; use run-length coding",
        ));
    }
    let elem = shape.dtype.element_size();
    let mut out = entropy::zstd_decompress(stored, shape.total_bytes(), ctx.dict)?;
    let cells = shape.width as usize * shape.height as usize;
    for cell in 0..cells {
        let base = cell * shape.channels as usize * elem;
        let reference = out[base..base + elem].to_vec();
        for channel in 1..shape.channels as usize {
            let at = base + channel * elem;
            let mut current = out[at..at + elem].to_vec();
            add_bytes(&mut current, &reference, shape.dtype);
            out[at..at + elem].copy_from_slice(&current);
        }
    }
    Ok(out)
}

fn subtract_in_place(target: &mut [u8], reference: &[u8], dtype: DType) {
    let elem = dtype.element_size();
    debug_assert_eq!(target.len(), reference.len());
    for (t, r) in target
        .chunks_exact_mut(elem)
        .zip(reference.chunks_exact(elem))
    {
        subtract_bytes(t, r, dtype);
    }
}

fn add_in_place(target: &mut [u8], reference: &[u8], dtype: DType) {
    let elem = dtype.element_size();
    debug_assert_eq!(target.len(), reference.len());
    for (t, r) in target
        .chunks_exact_mut(elem)
        .zip(reference.chunks_exact(elem))
    {
        add_bytes(t, r, dtype);
    }
}

fn subtract_bytes(target: &mut [u8], reference: &[u8], dtype: DType) {
    match dtype {
        // Float deltas work on the bit pattern with wrapping arithmetic: a
        // real-valued subtraction is not exactly reversible, and the format
        // requires every codec to be a lossless transform of the stored bytes.
        DType::F32 => {
            let a = le::get_u32(target, 0).unwrap_or(0);
            let b = le::get_u32(reference, 0).unwrap_or(0);
            let _ = le::put_u32(target, 0, a.wrapping_sub(b));
        }
        DType::F16 => {
            let a = le::get_u16(target, 0).unwrap_or(0);
            let b = le::get_u16(reference, 0).unwrap_or(0);
            let _ = le::put_u16(target, 0, a.wrapping_sub(b));
        }
        DType::I32 => {
            let a = le::get_i32(target, 0).unwrap_or(0);
            let b = le::get_i32(reference, 0).unwrap_or(0);
            let _ = le::put_i32(target, 0, a.wrapping_sub(b));
        }
        DType::I16 => {
            let a = le::get_i16(target, 0).unwrap_or(0);
            let b = le::get_i16(reference, 0).unwrap_or(0);
            let _ = le::put_i16(target, 0, a.wrapping_sub(b));
        }
        DType::U8 | DType::Bit => {
            target[0] = target[0].wrapping_sub(reference[0]);
        }
    }
}

fn add_bytes(target: &mut [u8], reference: &[u8], dtype: DType) {
    match dtype {
        DType::I32 => {
            let a = le::get_i32(target, 0).unwrap_or(0);
            let b = le::get_i32(reference, 0).unwrap_or(0);
            let _ = le::put_i32(target, 0, a.wrapping_add(b));
        }
        DType::I16 => {
            let a = le::get_i16(target, 0).unwrap_or(0);
            let b = le::get_i16(reference, 0).unwrap_or(0);
            let _ = le::put_i16(target, 0, a.wrapping_add(b));
        }
        DType::U8 | DType::Bit => {
            target[0] = target[0].wrapping_add(reference[0]);
        }
        DType::F32 => {
            let a = le::get_u32(target, 0).unwrap_or(0);
            let b = le::get_u32(reference, 0).unwrap_or(0);
            let _ = le::put_u32(target, 0, a.wrapping_add(b));
        }
        DType::F16 => {
            let a = le::get_u16(target, 0).unwrap_or(0);
            let b = le::get_u16(reference, 0).unwrap_or(0);
            let _ = le::put_u16(target, 0, a.wrapping_add(b));
        }
    }
}
