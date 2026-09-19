//! The fixed 64-byte OMF footer, placed at the very end of the file.
//!
//! The footer is written last and is the entry point for readers: it locates
//! the directory and carries the whole-file hash. Because it always occupies
//! the final 64 bytes, `file_hash` is defined as
//! `XxHash3-128(file[0 .. file_len - 16])` — "everything but the hash field
//! itself" — with no extra exclusion rules.

use crate::bytes as le;
use crate::error::{MapError, Result};
use crate::header::{HeaderFlags, VERSION_MAJOR};

/// Magic bytes of the footer.
pub const MAGIC: [u8; 4] = *b"OMFF";

/// Byte offsets and sizes inside the footer.
pub mod field {
    /// Bytes 0..4: magic `"OMFF"`.
    pub const MAGIC: usize = 0;
    /// Bytes 4..6: semantic major version.
    pub const VERSION_MAJOR: usize = 4;
    /// Bytes 6..8: semantic minor version.
    pub const VERSION_MINOR: usize = 6;
    /// Bytes 8..12: feature flags (mirror of the header flags).
    pub const FLAGS: usize = 8;
    /// Bytes 12..16: chunk directory length in bytes.
    pub const DIR_LEN: usize = 12;
    /// Bytes 16..24: chunk directory offset.
    pub const DIR_OFFSET: usize = 16;
    /// Bytes 24..28: number of chunk directory records.
    pub const DIR_RECORD_COUNT: usize = 24;
    /// Bytes 28..32: number of stored chunks (records minus tombstones).
    pub const CHUNK_COUNT: usize = 28;
    /// Bytes 32..36: number of quadtree skeleton nodes.
    pub const NODE_COUNT: usize = 32;
    /// Bytes 36..40: number of registered layers.
    pub const LAYER_COUNT: usize = 36;
    /// Bytes 40..48: total file length in bytes.
    pub const FILE_LEN: usize = 40;
    /// Bytes 48..64: XxHash3-128 over the final header and the body.
    pub const FILE_HASH: usize = 48;
    /// Size of the whole-file hash field in bytes.
    pub const FILE_HASH_SIZE: usize = 16;
    /// Total footer size in bytes.
    pub const SIZE: usize = 64;
}

/// Parsed OMF footer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Footer {
    /// Semantic major version.
    pub version_major: u16,
    /// Semantic minor version.
    pub version_minor: u16,
    /// Feature flags.
    pub flags: HeaderFlags,
    /// Chunk directory length in bytes.
    pub dir_len: u32,
    /// Chunk directory offset.
    pub dir_offset: u64,
    /// Number of directory records.
    pub dir_record_count: u32,
    /// Number of stored chunks.
    pub chunk_count: u32,
    /// Number of quadtree skeleton nodes.
    pub node_count: u32,
    /// Number of registered layers.
    pub layer_count: u32,
    /// Total file length.
    pub file_len: u64,
    /// Whole-file hash.
    pub file_hash: [u8; 16],
}

impl Default for Footer {
    fn default() -> Self {
        Self {
            version_major: VERSION_MAJOR,
            version_minor: 0,
            flags: HeaderFlags::default(),
            dir_len: 0,
            dir_offset: 0,
            dir_record_count: 0,
            chunk_count: 0,
            node_count: 0,
            layer_count: 0,
            file_len: 0,
            file_hash: [0u8; 16],
        }
    }
}

impl Footer {
    /// Serializes into the fixed 64-byte layout.
    pub fn to_bytes(&self) -> [u8; field::SIZE] {
        let mut buf = [0u8; field::SIZE];
        let write = |buf: &mut [u8; field::SIZE]| -> Result<()> {
            le::put_slice(&mut buf[..], field::MAGIC, &MAGIC)?;
            le::put_u16(&mut buf[..], field::VERSION_MAJOR, self.version_major)?;
            le::put_u16(&mut buf[..], field::VERSION_MINOR, self.version_minor)?;
            le::put_u32(&mut buf[..], field::FLAGS, self.flags.0)?;
            le::put_u32(&mut buf[..], field::DIR_LEN, self.dir_len)?;
            le::put_u64(&mut buf[..], field::DIR_OFFSET, self.dir_offset)?;
            le::put_u32(&mut buf[..], field::DIR_RECORD_COUNT, self.dir_record_count)?;
            le::put_u32(&mut buf[..], field::CHUNK_COUNT, self.chunk_count)?;
            le::put_u32(&mut buf[..], field::NODE_COUNT, self.node_count)?;
            le::put_u32(&mut buf[..], field::LAYER_COUNT, self.layer_count)?;
            le::put_u64(&mut buf[..], field::FILE_LEN, self.file_len)?;
            le::put_slice(&mut buf[..], field::FILE_HASH, &self.file_hash)?;
            Ok(())
        };
        let _ = write(&mut buf);
        buf
    }

    /// Parses the fixed layout, validating magic and major version.
    pub fn from_bytes(buf: &[u8]) -> Result<Self> {
        if buf.len() < field::SIZE {
            return Err(MapError::Truncated {
                offset: 0,
                needed: field::SIZE,
                available: buf.len(),
            });
        }
        let magic = le::get_bytes::<4>(buf, field::MAGIC)?;
        if magic != MAGIC {
            return Err(MapError::BadMagic {
                expected: MAGIC,
                found: magic,
            });
        }
        let version_major = le::get_u16(buf, field::VERSION_MAJOR)?;
        if version_major != VERSION_MAJOR {
            return Err(MapError::UnsupportedVersion {
                found: version_major,
                supported: VERSION_MAJOR,
            });
        }
        Ok(Self {
            version_major,
            version_minor: le::get_u16(buf, field::VERSION_MINOR)?,
            flags: HeaderFlags(le::get_u32(buf, field::FLAGS)?),
            dir_len: le::get_u32(buf, field::DIR_LEN)?,
            dir_offset: le::get_u64(buf, field::DIR_OFFSET)?,
            dir_record_count: le::get_u32(buf, field::DIR_RECORD_COUNT)?,
            chunk_count: le::get_u32(buf, field::CHUNK_COUNT)?,
            node_count: le::get_u32(buf, field::NODE_COUNT)?,
            layer_count: le::get_u32(buf, field::LAYER_COUNT)?,
            file_len: le::get_u64(buf, field::FILE_LEN)?,
            file_hash: le::get_bytes::<16>(buf, field::FILE_HASH)?,
        })
    }

    /// Updates the stored hash from a complete file image.
    pub fn seal(&mut self, file: &[u8]) -> Result<()> {
        if file.len() < field::SIZE {
            return Err(MapError::invalid("file too small to contain a footer"));
        }
        self.file_len = file.len() as u64;
        self.file_hash = compute_file_hash(file);
        Ok(())
    }

    /// Recomputes and compares the stored hash against `file`.
    pub fn verify(&self, file: &[u8]) -> Result<()> {
        if file.len() as u64 != self.file_len {
            return Err(MapError::invalid(format!(
                "file length {} does not match footer record {}",
                file.len(),
                self.file_len
            )));
        }
        let computed = compute_file_hash(file);
        if computed != self.file_hash {
            return Err(MapError::FileHash {
                stored: self.file_hash,
                computed,
            });
        }
        Ok(())
    }
}

/// Digest over `bytes[start..]`.
pub fn body_digest_from(bytes: &[u8], start: usize) -> [u8; 16] {
    let start = start.min(bytes.len());
    twox_hash::XxHash3_128::oneshot(&bytes[start..]).to_le_bytes()
}

/// Digest of the file body: every byte from the end of the header up to the
/// footer's hash field.
pub fn body_digest(file: &[u8]) -> [u8; 16] {
    let start = crate::header::field::SIZE.min(file.len());
    let end = file.len().saturating_sub(field::FILE_HASH_SIZE).max(start);
    body_digest_from(&file[..end], start)
}

/// Low 64 bits of a 128-bit hash, used for patch base matching.
pub fn hash64(hash: [u8; 16]) -> u64 {
    u64::from_le_bytes([
        hash[0], hash[1], hash[2], hash[3], hash[4], hash[5], hash[6], hash[7],
    ])
}

/// Composes the whole-file hash from the final header and the body digest.
///
/// The header is written last (its directory offsets are only known then), so
/// it cannot participate in the forward streaming pass that produces the body
/// digest. Composing the two covers every byte of the file exactly once.
pub fn compose_file_hash(header: &[u8; crate::header::field::SIZE], body: &[u8; 16]) -> [u8; 16] {
    let mut hasher = twox_hash::XxHash3_128::new();
    hasher.write(header);
    hasher.write(body);
    hasher.finish_128().to_le_bytes()
}

/// Computes the OMF whole-file hash of a complete in-memory file image.
pub fn compute_file_hash(file: &[u8]) -> [u8; 16] {
    let mut header = [0u8; crate::header::field::SIZE];
    let take = file.len().min(header.len());
    header[..take].copy_from_slice(&file[..take]);
    compose_file_hash(&header, &body_digest(file))
}
