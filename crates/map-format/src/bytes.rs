//! Little-endian primitive encoding helpers.
//!
//! OMF stores every multi-byte scalar little-endian, floats as IEEE 754. All
//! fixed-layout structures (header, footer, directory records, skeleton nodes)
//! are read and written field by field through these helpers instead of by
//! casting packed structs: several fields sit at offsets that are not naturally
//! aligned (`f64` at offset 12, `f32` at offset 6), so a `#[repr(C, packed)]`
//! transmute would produce unaligned accesses.

use crate::error::{MapError, Result};

const _: () = assert!(std::mem::size_of::<f32>() == 4);
const _: () = assert!(std::mem::size_of::<f64>() == 8);

/// Reads a `u8` at `offset`.
#[inline]
pub fn get_u8(buf: &[u8], offset: usize) -> Result<u8> {
    buf.get(offset)
        .copied()
        .ok_or_else(|| truncation(buf, offset, 1))
}

/// Reads an `i8` at `offset`.
#[inline]
pub fn get_i8(buf: &[u8], offset: usize) -> Result<i8> {
    Ok(get_u8(buf, offset)? as i8)
}

/// Reads a `u16` at `offset`.
#[inline]
pub fn get_u16(buf: &[u8], offset: usize) -> Result<u16> {
    let b = slice::<2>(buf, offset)?;
    Ok(u16::from_le_bytes(b))
}

/// Reads an `i16` at `offset`.
#[inline]
pub fn get_i16(buf: &[u8], offset: usize) -> Result<i16> {
    let b = slice::<2>(buf, offset)?;
    Ok(i16::from_le_bytes(b))
}

/// Reads a `u32` at `offset`.
#[inline]
pub fn get_u32(buf: &[u8], offset: usize) -> Result<u32> {
    let b = slice::<4>(buf, offset)?;
    Ok(u32::from_le_bytes(b))
}

/// Reads an `i32` at `offset`.
#[inline]
pub fn get_i32(buf: &[u8], offset: usize) -> Result<i32> {
    let b = slice::<4>(buf, offset)?;
    Ok(i32::from_le_bytes(b))
}

/// Reads a `u64` at `offset`.
#[inline]
pub fn get_u64(buf: &[u8], offset: usize) -> Result<u64> {
    let b = slice::<8>(buf, offset)?;
    Ok(u64::from_le_bytes(b))
}

/// Reads an `f32` at `offset`.
#[inline]
pub fn get_f32(buf: &[u8], offset: usize) -> Result<f32> {
    let b = slice::<4>(buf, offset)?;
    Ok(f32::from_le_bytes(b))
}

/// Reads an `f64` at `offset`.
#[inline]
pub fn get_f64(buf: &[u8], offset: usize) -> Result<f64> {
    let b = slice::<8>(buf, offset)?;
    Ok(f64::from_le_bytes(b))
}

/// Reads `N` bytes at `offset`.
#[inline]
pub fn get_bytes<const N: usize>(buf: &[u8], offset: usize) -> Result<[u8; N]> {
    slice::<N>(buf, offset)
}

/// Writes a `u8` at `offset`.
#[inline]
pub fn put_u8(buf: &mut [u8], offset: usize, value: u8) -> Result<()> {
    if offset >= buf.len() {
        return Err(truncation(buf, offset, 1));
    }
    buf[offset] = value;
    Ok(())
}

/// Writes an `i8` at `offset`.
#[inline]
pub fn put_i8(buf: &mut [u8], offset: usize, value: i8) -> Result<()> {
    put_u8(buf, offset, value as u8)
}

/// Writes a `u16` at `offset`.
#[inline]
pub fn put_u16(buf: &mut [u8], offset: usize, value: u16) -> Result<()> {
    put_slice(buf, offset, &value.to_le_bytes())
}

/// Writes an `i16` at `offset`.
#[inline]
pub fn put_i16(buf: &mut [u8], offset: usize, value: i16) -> Result<()> {
    put_slice(buf, offset, &value.to_le_bytes())
}

/// Writes a `u32` at `offset`.
#[inline]
pub fn put_u32(buf: &mut [u8], offset: usize, value: u32) -> Result<()> {
    put_slice(buf, offset, &value.to_le_bytes())
}

/// Writes an `i32` at `offset`.
#[inline]
pub fn put_i32(buf: &mut [u8], offset: usize, value: i32) -> Result<()> {
    put_slice(buf, offset, &value.to_le_bytes())
}

/// Writes a `u64` at `offset`.
#[inline]
pub fn put_u64(buf: &mut [u8], offset: usize, value: u64) -> Result<()> {
    put_slice(buf, offset, &value.to_le_bytes())
}

/// Writes an `f32` at `offset`.
#[inline]
pub fn put_f32(buf: &mut [u8], offset: usize, value: f32) -> Result<()> {
    put_slice(buf, offset, &value.to_le_bytes())
}

/// Writes an `f64` at `offset`.
#[inline]
pub fn put_f64(buf: &mut [u8], offset: usize, value: f64) -> Result<()> {
    put_slice(buf, offset, &value.to_le_bytes())
}

/// Writes `values` at `offset`.
#[inline]
pub fn put_slice(buf: &mut [u8], offset: usize, values: &[u8]) -> Result<()> {
    let end = offset
        .checked_add(values.len())
        .ok_or_else(|| MapError::invalid("offset overflow"))?;
    if end > buf.len() {
        return Err(truncation(buf, offset, values.len()));
    }
    buf[offset..end].copy_from_slice(values);
    Ok(())
}

fn slice<const N: usize>(buf: &[u8], offset: usize) -> Result<[u8; N]> {
    let end = offset
        .checked_add(N)
        .ok_or_else(|| MapError::invalid("offset overflow"))?;
    let s = buf
        .get(offset..end)
        .ok_or_else(|| truncation(buf, offset, N))?;
    let mut out = [0u8; N];
    out.copy_from_slice(s);
    Ok(out)
}

fn truncation(buf: &[u8], offset: usize, needed: usize) -> MapError {
    MapError::Truncated {
        offset: offset as u64,
        needed,
        available: buf.len().saturating_sub(offset),
    }
}

/// Sequential little-endian reader over a byte slice.
///
/// Used for variable-length payloads (TLV values, graph tables) where a
/// fixed-offset layout would be unreadable.
#[derive(Debug, Clone)]
pub struct Reader<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    /// Creates a reader positioned at the start of `buf`.
    pub fn new(buf: &'a [u8]) -> Self {
        Self { buf, pos: 0 }
    }

    /// Current byte position.
    #[inline]
    pub fn position(&self) -> usize {
        self.pos
    }

    /// Remaining byte count.
    #[inline]
    pub fn remaining(&self) -> usize {
        self.buf.len().saturating_sub(self.pos)
    }

    /// True when every byte has been consumed.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.remaining() == 0
    }

    /// Reads a `u8`.
    #[inline]
    pub fn read_u8(&mut self) -> Result<u8> {
        let v = get_u8(self.buf, self.pos)?;
        self.pos += 1;
        Ok(v)
    }

    /// Reads an `i8`.
    #[inline]
    pub fn read_i8(&mut self) -> Result<i8> {
        Ok(self.read_u8()? as i8)
    }

    /// Reads a `u16`.
    #[inline]
    pub fn read_u16(&mut self) -> Result<u16> {
        let v = get_u16(self.buf, self.pos)?;
        self.pos += 2;
        Ok(v)
    }

    /// Reads an `i16`.
    #[inline]
    pub fn read_i16(&mut self) -> Result<i16> {
        let v = get_i16(self.buf, self.pos)?;
        self.pos += 2;
        Ok(v)
    }

    /// Reads a `u32`.
    #[inline]
    pub fn read_u32(&mut self) -> Result<u32> {
        let v = get_u32(self.buf, self.pos)?;
        self.pos += 4;
        Ok(v)
    }

    /// Reads an `i32`.
    #[inline]
    pub fn read_i32(&mut self) -> Result<i32> {
        let v = get_i32(self.buf, self.pos)?;
        self.pos += 4;
        Ok(v)
    }

    /// Reads a `u64`.
    #[inline]
    pub fn read_u64(&mut self) -> Result<u64> {
        let v = get_u64(self.buf, self.pos)?;
        self.pos += 8;
        Ok(v)
    }

    /// Reads an `f32`.
    #[inline]
    pub fn read_f32(&mut self) -> Result<f32> {
        let v = get_f32(self.buf, self.pos)?;
        self.pos += 4;
        Ok(v)
    }

    /// Reads an `f64`.
    #[inline]
    pub fn read_f64(&mut self) -> Result<f64> {
        let v = get_f64(self.buf, self.pos)?;
        self.pos += 8;
        Ok(v)
    }

    /// Reads `n` raw bytes.
    pub fn read_bytes(&mut self, n: usize) -> Result<&'a [u8]> {
        let end = self
            .pos
            .checked_add(n)
            .ok_or_else(|| MapError::invalid("offset overflow"))?;
        let s = self
            .buf
            .get(self.pos..end)
            .ok_or_else(|| truncation(self.buf, self.pos, n))?;
        self.pos = end;
        Ok(s)
    }

    /// Reads a `u16`-length-prefixed UTF-8 string.
    pub fn read_string(&mut self) -> Result<String> {
        let len = self.read_u16()? as usize;
        let raw = self.read_bytes(len)?;
        String::from_utf8(raw.to_vec())
            .map_err(|e| MapError::invalid(format!("invalid utf-8: {e}")))
    }

    /// Reads a `u16`-length-prefixed raw byte string (no UTF-8 validation).
    pub fn read_blob(&mut self) -> Result<&'a [u8]> {
        let len = self.read_u16()? as usize;
        self.read_bytes(len)
    }

    /// Reads `count` `f32` values.
    pub fn read_f32_vec(&mut self, count: usize) -> Result<Vec<f32>> {
        let mut out = Vec::with_capacity(count);
        for _ in 0..count {
            out.push(self.read_f32()?);
        }
        Ok(out)
    }

    /// Reads `count` `u32` values.
    pub fn read_u32_vec(&mut self, count: usize) -> Result<Vec<u32>> {
        let mut out = Vec::with_capacity(count);
        for _ in 0..count {
            out.push(self.read_u32()?);
        }
        Ok(out)
    }
}

/// Sequential little-endian writer producing an in-memory buffer.
#[derive(Debug, Default, Clone)]
pub struct Writer {
    buf: Vec<u8>,
}

impl Writer {
    /// Creates an empty writer.
    pub fn new() -> Self {
        Self { buf: Vec::new() }
    }

    /// Creates an empty writer with `capacity` bytes pre-allocated.
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            buf: Vec::with_capacity(capacity),
        }
    }

    /// Number of bytes written so far.
    #[inline]
    pub fn len(&self) -> usize {
        self.buf.len()
    }

    /// True when nothing has been written.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.buf.is_empty()
    }

    /// Consumes the writer, returning the buffer.
    #[inline]
    pub fn into_vec(self) -> Vec<u8> {
        self.buf
    }

    /// Borrows the written bytes.
    #[inline]
    pub fn as_slice(&self) -> &[u8] {
        &self.buf
    }

    /// Appends a `u8`.
    #[inline]
    pub fn write_u8(&mut self, v: u8) -> &mut Self {
        self.buf.push(v);
        self
    }

    /// Appends an `i8`.
    #[inline]
    pub fn write_i8(&mut self, v: i8) -> &mut Self {
        self.buf.push(v as u8);
        self
    }

    /// Appends a `u16`.
    #[inline]
    pub fn write_u16(&mut self, v: u16) -> &mut Self {
        self.buf.extend_from_slice(&v.to_le_bytes());
        self
    }

    /// Appends an `i16`.
    #[inline]
    pub fn write_i16(&mut self, v: i16) -> &mut Self {
        self.buf.extend_from_slice(&v.to_le_bytes());
        self
    }

    /// Appends a `u32`.
    #[inline]
    pub fn write_u32(&mut self, v: u32) -> &mut Self {
        self.buf.extend_from_slice(&v.to_le_bytes());
        self
    }

    /// Appends an `i32`.
    #[inline]
    pub fn write_i32(&mut self, v: i32) -> &mut Self {
        self.buf.extend_from_slice(&v.to_le_bytes());
        self
    }

    /// Appends a `u64`.
    #[inline]
    pub fn write_u64(&mut self, v: u64) -> &mut Self {
        self.buf.extend_from_slice(&v.to_le_bytes());
        self
    }

    /// Appends an `f32`.
    #[inline]
    pub fn write_f32(&mut self, v: f32) -> &mut Self {
        self.buf.extend_from_slice(&v.to_le_bytes());
        self
    }

    /// Appends an `f64`.
    #[inline]
    pub fn write_f64(&mut self, v: f64) -> &mut Self {
        self.buf.extend_from_slice(&v.to_le_bytes());
        self
    }

    /// Appends raw bytes.
    #[inline]
    pub fn write_bytes(&mut self, v: &[u8]) -> &mut Self {
        self.buf.extend_from_slice(v);
        self
    }

    /// Appends a `u16`-length-prefixed UTF-8 string.
    ///
    /// A string longer than the prefix can describe is truncated *and reported*:
    /// the alternative is a silent loss of data, and cutting at an arbitrary byte
    /// would also split a character and leave a payload that cannot be decoded.
    pub fn write_string(&mut self, v: &str) -> &mut Self {
        let mut end = v.len().min(u16::MAX as usize);
        while end > 0 && !v.is_char_boundary(end) {
            end -= 1;
        }
        if end != v.len() {
            tracing::warn!(
                "a {}-byte string exceeds the 16-bit length prefix; writing the first {end} byte(s)",
                v.len()
            );
        }
        self.write_u16(end as u16);
        self.buf.extend_from_slice(&v.as_bytes()[..end]);
        self
    }

    /// Appends a `u16`-length-prefixed raw blob.
    ///
    /// Truncation is reported rather than silent; see [`Writer::write_string`].
    pub fn write_blob(&mut self, v: &[u8]) -> &mut Self {
        let len = v.len().min(u16::MAX as usize);
        if len != v.len() {
            tracing::warn!(
                "a {}-byte blob exceeds the 16-bit length prefix; writing the first {len} byte(s)",
                v.len()
            );
        }
        self.write_u16(len as u16);
        self.buf.extend_from_slice(&v[..len]);
        self
    }

    /// Appends `count` zero bytes (padding aid).
    pub fn write_zeros(&mut self, count: usize) -> &mut Self {
        self.buf.resize(self.buf.len() + count, 0);
        self
    }
}

/// Pads `buf` with zeros until its length is a multiple of `align`.
pub fn pad_to_alignment(buf: &mut Vec<u8>, align: usize) {
    if align == 0 {
        return;
    }
    let rem = buf.len() % align;
    if rem != 0 {
        buf.resize(buf.len() + (align - rem), 0);
    }
}

#[cfg(test)]
#[path = "../tests/unit/bytes.rs"]
mod tests;
