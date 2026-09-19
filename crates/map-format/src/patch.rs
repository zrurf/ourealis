//! Incremental updates through patch files.
//!
//! An OMF file is immutable; content changes arrive as a patch carrying
//! replacement chunks and optional metadata changes. Applying a patch never
//! rewrites the base data: new chunks, a new meta block (if needed) and a new
//! directory are appended, and the header is patched to point at them. Because
//! directory offsets are absolute, base chunks keep their positions and the
//! superseded bytes simply become unreachable. Compaction, if ever needed, is a
//! plain re-write.
//!
//! Patches may only touch **source** layers. Editing a derived layer directly
//! would leave it inconsistent with its fingerprint, so the format refuses it
//! and lets the normal rebuild path handle it: a source edit changes the source
//! fingerprint, every dependent derived layer fails its check on the next load,
//! and the preprocessing tool rebuilds exactly those layers.

use crate::bytes::{Reader, Writer};
use crate::directory::ChunkRecord;
use crate::error::{MapError, Result};
use crate::footer::{self, Footer};
use crate::header;
use crate::layer::LayerId;
use crate::reader::Map;
use crate::tlv::{TlvBlock, TlvRecord, TlvValue};

/// Magic of a patch file.
pub const MAGIC: [u8; 4] = *b"OMFP";

/// Replacement of one chunk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChunkReplacement {
    /// Target layer.
    pub layer_id: LayerId,
    /// Target level.
    pub level: u8,
    /// Target chunk id.
    pub chunk_id: u32,
    /// Codec of the replacement payload.
    pub codec: u8,
    /// Replacement payload, already encoded with `codec`.
    pub payload: Vec<u8>,
    /// Length of the payload after decompression.
    pub raw_len: u32,
}

impl ChunkReplacement {
    /// Builds a replacement from stored bytes.
    pub fn new(
        layer_id: LayerId,
        level: u8,
        chunk_id: u32,
        codec: u8,
        payload: Vec<u8>,
        raw_len: u32,
    ) -> Self {
        Self {
            layer_id,
            level,
            chunk_id,
            codec,
            payload,
            raw_len,
        }
    }

    /// CRC32 of the stored payload.
    pub fn crc32(&self) -> u32 {
        crc32fast::hash(&self.payload)
    }
}

/// A patch: chunk replacements plus optional metadata changes.
#[derive(Debug, Clone, Default)]
pub struct Patch {
    /// Low 64 bits of the whole-file hash of the base file.
    pub base_hash: u64,
    /// Chunk replacements.
    pub replacements: Vec<ChunkReplacement>,
    /// Metadata records merged into the base's meta block on application.
    pub meta_patch: Vec<TlvRecord>,
}

impl Patch {
    /// Creates a patch targeting `base`, rejecting derived-layer replacements.
    pub fn for_map(base: &Map, replacements: Vec<ChunkReplacement>) -> Result<Self> {
        for replacement in &replacements {
            check_patchable(base, replacement.layer_id)?;
        }
        Ok(Self {
            base_hash: footer::hash64(base.footer().file_hash),
            replacements,
            meta_patch: Vec::new(),
        })
    }

    /// Adds a metadata record to the patch.
    pub fn with_meta<T: TlvValue>(mut self, tag: u32, value: &T) -> Self {
        self.meta_patch.push(TlvRecord::encode(tag, value));
        self
    }

    /// Serialises the patch.
    ///
    /// ```text
    /// magic | version | base_hash | counts | replacement records | meta TLV block
    /// ```
    pub fn encode(&self) -> Vec<u8> {
        let meta_bytes = TlvBlock::from_records(&self.meta_patch).to_bytes();

        let mut out = Writer::new();
        out.write_bytes(&MAGIC);
        out.write_u16(header::VERSION_MAJOR);
        out.write_u16(header::VERSION_MINOR);
        out.write_u64(self.base_hash);
        out.write_u32(self.replacements.len().min(u32::MAX as usize) as u32);
        out.write_u32(meta_bytes.len() as u32);
        for replacement in &self.replacements {
            out.write_u16(replacement.layer_id.raw());
            out.write_u8(replacement.level);
            out.write_u8(0);
            out.write_u32(replacement.chunk_id);
            out.write_u8(replacement.codec);
            out.write_u8(0);
            out.write_u16(0);
            out.write_u32(replacement.raw_len);
            out.write_u32(replacement.payload.len() as u32);
            out.write_bytes(&replacement.payload);
        }
        out.write_bytes(&meta_bytes);
        out.into_vec()
    }

    /// Parses a patch.
    pub fn decode(bytes: &[u8]) -> Result<Self> {
        let mut reader = Reader::new(bytes);
        let magic = reader.read_bytes(4)?;
        if magic != MAGIC {
            return Err(MapError::BadMagic {
                expected: MAGIC,
                found: [magic[0], magic[1], magic[2], magic[3]],
            });
        }
        let _major = reader.read_u16()?;
        let _minor = reader.read_u16()?;
        let base_hash = reader.read_u64()?;
        let count = reader.read_u32()? as usize;
        let meta_len = reader.read_u32()? as usize;

        // Each replacement record carries 22 fixed bytes plus its payload, so a
        // count that cannot fit in the remaining bytes is corrupt; reserving for
        // it first would let a hostile header demand gigabytes.
        const REPLACEMENT_HEADER: usize = 22;
        let records_len = count
            .checked_mul(REPLACEMENT_HEADER)
            .ok_or_else(|| MapError::invalid("patch replacement count overflows"))?;
        let available = reader.remaining().saturating_sub(meta_len);
        if records_len > available {
            return Err(MapError::Truncated {
                offset: reader.position() as u64,
                needed: records_len,
                available,
            });
        }

        let mut replacements = Vec::with_capacity(count);
        for _ in 0..count {
            let layer_id = LayerId(reader.read_u16()?);
            let level = reader.read_u8()?;
            let _reserved0 = reader.read_u8()?;
            let chunk_id = reader.read_u32()?;
            let codec = reader.read_u8()?;
            let _reserved1 = reader.read_u8()?;
            let _reserved2 = reader.read_u16()?;
            let raw_len = reader.read_u32()?;
            let payload_len = reader.read_u32()? as usize;
            let payload = reader.read_bytes(payload_len)?.to_vec();
            replacements.push(ChunkReplacement {
                layer_id,
                level,
                chunk_id,
                codec,
                payload,
                raw_len,
            });
        }
        let meta_patch = TlvBlock::from_bytes(reader.read_bytes(meta_len)?)?
            .records()
            .to_vec();
        Ok(Self {
            base_hash,
            replacements,
            meta_patch,
        })
    }

    /// Applies the patch to a base file image, returning the updated image.
    pub fn apply(&self, base: &[u8]) -> Result<Vec<u8>> {
        let map = Map::from_bytes(base.to_vec())?;
        let base_hash = footer::hash64(map.footer().file_hash);
        if self.base_hash != base_hash {
            return Err(MapError::PatchBaseMismatch {
                patch: self.base_hash,
                base: base_hash,
            });
        }
        for replacement in &self.replacements {
            check_patchable(&map, replacement.layer_id)?;
        }

        let mut out = base.to_vec();
        // Drop the old footer; the new one goes at the end and the old bytes
        // are unreachable either way.
        out.truncate(out.len() - footer::field::SIZE);

        let mut directory = map.directory().clone();
        for replacement in &self.replacements {
            let offset = out.len() as u64;
            out.extend_from_slice(&replacement.payload);
            directory.upsert(ChunkRecord::new(
                replacement.layer_id,
                replacement.level,
                replacement.chunk_id,
                replacement.codec,
                offset,
                replacement.payload.len() as u32,
                replacement.raw_len,
                replacement.crc32(),
            ));
        }

        // A modified meta block is appended rather than rewritten in place, so
        // no existing offset has to shift. The skeleton is not addressed by any
        // header field: the reader locates it at `align8(meta_offset + meta_len
        // + ext_meta_len)`, the layout the writer always produces. Moving the
        // meta block therefore moves that computed address, so the extension
        // area and the skeleton are re-emitted at the new one.
        let (meta_offset, meta_len, ext_meta_offset) = if self.meta_patch.is_empty() {
            (
                map.header().meta_offset,
                map.header().meta_len,
                map.header().ext_meta_offset,
            )
        } else {
            let mut block = map.meta_block().clone();
            for record in &self.meta_patch {
                block.set(record.clone());
            }
            let compressed = crate::codec::entropy::zstd_compress(&block.to_bytes(), None)?;
            let offset = out.len() as u64;
            out.extend_from_slice(&compressed);

            let ext_start = map.header().ext_meta_offset as usize;
            let ext_len = map.header().ext_meta_len as usize;
            let ext_bytes: &[u8] = map
                .header()
                .ext_meta_offset
                .checked_add(map.header().ext_meta_len as u64)
                .filter(|end| *end <= base.len() as u64)
                .map(|_| &base[ext_start..ext_start + ext_len])
                .unwrap_or(&[]);
            let skeleton_offset =
                crate::writer::align8(out.len() as u64 + ext_bytes.len() as u64) as usize;
            let new_ext_offset = out.len() as u64;
            out.extend_from_slice(ext_bytes);
            out.resize(skeleton_offset, 0);
            for node in map.skeleton() {
                out.extend_from_slice(&node.to_bytes());
            }
            (offset, compressed.len() as u32, new_ext_offset)
        };

        let dir_offset = out.len() as u64;
        let dir_bytes = directory.to_bytes();
        out.extend_from_slice(&dir_bytes);

        let footer_offset = out.len() as u64;
        let file_len = footer_offset + footer::field::SIZE as u64;

        let mut header = *map.header();
        header.dir_offset = dir_offset;
        header.dir_len = dir_bytes.len() as u32;
        header.footer_offset = footer_offset;
        header.meta_offset = meta_offset;
        header.meta_len = meta_len;
        header.ext_meta_offset = ext_meta_offset;
        header.layer_count = map.layers().len().min(u16::MAX as usize) as u16;
        let header_bytes = header.to_bytes();

        let mut footer = Footer {
            version_major: header::VERSION_MAJOR,
            version_minor: header::VERSION_MINOR,
            flags: map.header().flags,
            dir_len: dir_bytes.len() as u32,
            dir_offset,
            dir_record_count: directory.len() as u32,
            chunk_count: directory.len() as u32,
            node_count: map.footer().node_count,
            layer_count: map.layers().len() as u32,
            file_len,
            file_hash: [0u8; 16],
        };
        let mut footer_bytes = footer.to_bytes();
        // The body hash covers everything up to (and including) the footer's
        // first 48 bytes, mirroring the writer and the reader.
        out.extend_from_slice(&footer_bytes[..footer::field::FILE_HASH]);
        let body_digest = footer::body_digest_from(&out, header::field::SIZE);
        footer.file_hash = footer::compose_file_hash(&header_bytes, &body_digest);
        footer_bytes = footer.to_bytes();
        out.extend_from_slice(&footer_bytes[footer::field::FILE_HASH..]);
        out[..header::field::SIZE].copy_from_slice(&header_bytes);
        Ok(out)
    }
}

fn check_patchable(map: &Map, layer_id: LayerId) -> Result<()> {
    // Derived-layer ids are refused before the registry lookup: a patch that
    // targets a cache layer is wrong regardless of whether it is present.
    if layer_id.is_derived() {
        return Err(MapError::PatchDerivedLayer {
            layer_id: layer_id.raw(),
        });
    }
    map.layer_desc(layer_id)
        .ok_or(MapError::LayerNotFound {
            layer_id: layer_id.raw(),
        })
        .map(|_| ())
}

/// Low 64 bits of the whole-file hash of an in-memory image.
pub fn file_hash64(file: &[u8]) -> u64 {
    footer::hash64(footer::compute_file_hash(file))
}

impl TlvBlock {
    /// Builds a block from a record list.
    pub fn from_records(records: &[TlvRecord]) -> Self {
        let mut block = Self::new();
        for record in records {
            block.push(record.clone());
        }
        block
    }
}
