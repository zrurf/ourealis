//! The fixed 128-byte OMF header.
//!
//! Field offsets are spelled out in [`field`] and used directly by
//! [`Header::to_bytes`] / [`Header::from_bytes`]; see the implementation
//! document §5.1 for the table these constants encode.

use crate::bytes as le;
use crate::error::{MapError, Result};
use crate::geometry::Aabb;

/// Major version written by this crate.
pub const VERSION_MAJOR: u16 = 1;
/// Minor version written by this crate.
pub const VERSION_MINOR: u16 = 0;
/// Magic bytes of the file header.
pub const MAGIC: [u8; 4] = *b"OMF\0";

/// Byte offsets and sizes inside the header. Kept public so tooling can diff
/// raw files without duplicating the layout.
pub mod field {
    /// Bytes 0..4: magic `"OMF\0"`.
    pub const MAGIC: usize = 0;
    /// Bytes 4..6: semantic major version.
    pub const VERSION_MAJOR: usize = 4;
    /// Bytes 6..8: semantic minor version.
    pub const VERSION_MINOR: usize = 6;
    /// Bytes 8..12: feature flags.
    pub const FLAGS: usize = 8;
    /// Bytes 12..20: reference longitude (radians).
    pub const REF_LON: usize = 12;
    /// Bytes 20..28: reference latitude (radians).
    pub const REF_LAT: usize = 20;
    /// Bytes 28..32: EPSG code, 0 for the local metre plane.
    pub const EPSG: usize = 28;
    /// Bytes 32..36: bounds min x (metres).
    pub const BOUNDS_X0: usize = 32;
    /// Bytes 36..40: bounds min y (metres).
    pub const BOUNDS_Y0: usize = 36;
    /// Bytes 40..44: bounds max x (metres).
    pub const BOUNDS_X1: usize = 40;
    /// Bytes 44..48: bounds max y (metres).
    pub const BOUNDS_Y1: usize = 44;
    /// Bytes 48..50: finest resolution in centimetres per pixel.
    pub const BASE_RES_CM: usize = 48;
    /// Bytes 50..52: chunk side length in pixels.
    pub const CHUNK_SIZE: usize = 50;
    /// Byte 52: LOD pyramid level count.
    pub const LOD_COUNT: usize = 52;
    /// Byte 53: default resistance feature dimension.
    pub const FEATURE_DIM: usize = 53;
    /// Bytes 54..56: registered layer count.
    pub const LAYER_COUNT: usize = 54;
    /// Bytes 56..64: offset of the meta block.
    pub const META_OFFSET: usize = 56;
    /// Bytes 64..68: length of the meta block.
    pub const META_LEN: usize = 64;
    /// Bytes 68..76: offset of the chunk directory.
    pub const DIR_OFFSET: usize = 68;
    /// Bytes 76..80: length of the chunk directory.
    pub const DIR_LEN: usize = 76;
    /// Bytes 80..88: offset of the footer.
    pub const FOOTER_OFFSET: usize = 80;
    /// Bytes 88..96: offset of the optional extension TLV area.
    pub const EXT_META_OFFSET: usize = 88;
    /// Bytes 96..100: length of the optional extension TLV area.
    pub const EXT_META_LEN: usize = 96;
    /// Bytes 100..104: CRC32 over bytes 0..100.
    pub const HEADER_CRC32: usize = 100;
    /// Bytes 104..128: reserved, always zero.
    pub const RESERVED: usize = 104;
    /// Total header size in bytes.
    pub const SIZE: usize = 128;
    /// Number of bytes covered by the header CRC32.
    pub const CRC_RANGE: usize = HEADER_CRC32;
}

/// Header feature flags (header offset 8).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct HeaderFlags(pub u32);

impl HeaderFlags {
    /// Sparse graph layers (PRM) are present.
    pub const HAS_PRM: u32 = 1 << 0;
    /// A pretrained zstd dictionary is present in the meta block.
    pub const HAS_ZSTD_DICT: u32 = 1 << 1;
    /// A K-shortest-path library is present.
    pub const HAS_KPATH_LIB: u32 = 1 << 2;
    /// Region (polygon annotation) layers are present.
    pub const HAS_REGIONS: u32 = 1 << 3;
    /// Vector line layers are present.
    pub const HAS_VECTORS: u32 = 1 << 4;
    /// Debug data is present. Forbidden in distributed files.
    pub const DEBUG_DATA: u32 = 1 << 31;

    /// True when `bit` is set.
    #[inline]
    pub fn contains(self, bit: u32) -> bool {
        self.0 & bit != 0
    }

    /// Sets `bit`.
    #[inline]
    pub fn insert(&mut self, bit: u32) {
        self.0 |= bit;
    }

    /// Clears `bit`.
    #[inline]
    pub fn remove(&mut self, bit: u32) {
        self.0 &= !bit;
    }
}

/// Parsed OMF header.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Header {
    /// Semantic major version.
    pub version_major: u16,
    /// Semantic minor version.
    pub version_minor: u16,
    /// Feature flags.
    pub flags: HeaderFlags,
    /// Reference longitude in radians; the origin of the local metre plane.
    pub ref_lon: f64,
    /// Reference latitude in radians.
    pub ref_lat: f64,
    /// EPSG code, or 0 when the file is a pure local metre plane.
    pub epsg: u32,
    /// Map extent in the local metre plane.
    pub bounds: Aabb,
    /// Finest raster resolution in centimetres per pixel.
    pub base_res_cm: u16,
    /// Chunk side length in pixels.
    pub chunk_size: u16,
    /// Number of LOD pyramid levels.
    pub lod_count: u8,
    /// Default resistance feature dimension (authoritative value lives in the
    /// `FEATURE_SCHEMA` TLV).
    pub feature_dim: u8,
    /// Number of registered layers.
    pub layer_count: u16,
    /// Meta block offset.
    pub meta_offset: u64,
    /// Meta block length.
    pub meta_len: u32,
    /// Chunk directory offset.
    pub dir_offset: u64,
    /// Chunk directory length in bytes.
    pub dir_len: u32,
    /// Footer offset.
    pub footer_offset: u64,
    /// Extension TLV area offset, 0 when absent.
    pub ext_meta_offset: u64,
    /// Extension TLV area length.
    pub ext_meta_len: u32,
}

impl Default for Header {
    fn default() -> Self {
        Self {
            version_major: VERSION_MAJOR,
            version_minor: VERSION_MINOR,
            flags: HeaderFlags::default(),
            ref_lon: 0.0,
            ref_lat: 0.0,
            epsg: 0,
            bounds: Aabb::new(0.0, 0.0, 0.0, 0.0),
            base_res_cm: 50,
            chunk_size: 256,
            lod_count: 1,
            feature_dim: 0,
            layer_count: 0,
            meta_offset: 0,
            meta_len: 0,
            dir_offset: 0,
            dir_len: 0,
            footer_offset: 0,
            ext_meta_offset: 0,
            ext_meta_len: 0,
        }
    }
}

impl Header {
    /// Finest raster resolution in metres per pixel.
    #[inline]
    pub fn base_res_m(&self) -> f64 {
        self.base_res_cm as f64 / 100.0
    }

    /// Serializes into the fixed 128-byte layout, CRC32 included.
    pub fn to_bytes(&self) -> [u8; field::SIZE] {
        let mut buf = [0u8; field::SIZE];
        let put = |buf: &mut [u8; field::SIZE]| -> Result<()> {
            le::put_slice(&mut buf[..], field::MAGIC, &MAGIC)?;
            le::put_u16(&mut buf[..], field::VERSION_MAJOR, self.version_major)?;
            le::put_u16(&mut buf[..], field::VERSION_MINOR, self.version_minor)?;
            le::put_u32(&mut buf[..], field::FLAGS, self.flags.0)?;
            le::put_f64(&mut buf[..], field::REF_LON, self.ref_lon)?;
            le::put_f64(&mut buf[..], field::REF_LAT, self.ref_lat)?;
            le::put_u32(&mut buf[..], field::EPSG, self.epsg)?;
            le::put_f32(&mut buf[..], field::BOUNDS_X0, self.bounds.min_x as f32)?;
            le::put_f32(&mut buf[..], field::BOUNDS_Y0, self.bounds.min_y as f32)?;
            le::put_f32(&mut buf[..], field::BOUNDS_X1, self.bounds.max_x as f32)?;
            le::put_f32(&mut buf[..], field::BOUNDS_Y1, self.bounds.max_y as f32)?;
            le::put_u16(&mut buf[..], field::BASE_RES_CM, self.base_res_cm)?;
            le::put_u16(&mut buf[..], field::CHUNK_SIZE, self.chunk_size)?;
            le::put_u8(&mut buf[..], field::LOD_COUNT, self.lod_count)?;
            le::put_u8(&mut buf[..], field::FEATURE_DIM, self.feature_dim)?;
            le::put_u16(&mut buf[..], field::LAYER_COUNT, self.layer_count)?;
            le::put_u64(&mut buf[..], field::META_OFFSET, self.meta_offset)?;
            le::put_u32(&mut buf[..], field::META_LEN, self.meta_len)?;
            le::put_u64(&mut buf[..], field::DIR_OFFSET, self.dir_offset)?;
            le::put_u32(&mut buf[..], field::DIR_LEN, self.dir_len)?;
            le::put_u64(&mut buf[..], field::FOOTER_OFFSET, self.footer_offset)?;
            le::put_u64(&mut buf[..], field::EXT_META_OFFSET, self.ext_meta_offset)?;
            le::put_u32(&mut buf[..], field::EXT_META_LEN, self.ext_meta_len)?;
            Ok(())
        };
        // The buffer is exactly `field::SIZE` bytes and every write above stays
        // inside the CRC range, so these writes cannot fail.
        let _ = put(&mut buf);
        let crc = crc32fast::hash(&buf[..field::CRC_RANGE]);
        let _ = le::put_u32(&mut buf[..], field::HEADER_CRC32, crc);
        buf
    }

    /// Parses the fixed layout, validating magic, major version and CRC32.
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
        let stored_crc = le::get_u32(buf, field::HEADER_CRC32)?;
        let computed_crc = crc32fast::hash(&buf[..field::CRC_RANGE]);
        if stored_crc != computed_crc {
            return Err(MapError::HeaderCrc {
                stored: stored_crc,
                computed: computed_crc,
            });
        }
        Ok(Self {
            version_major,
            version_minor: le::get_u16(buf, field::VERSION_MINOR)?,
            flags: HeaderFlags(le::get_u32(buf, field::FLAGS)?),
            ref_lon: le::get_f64(buf, field::REF_LON)?,
            ref_lat: le::get_f64(buf, field::REF_LAT)?,
            epsg: le::get_u32(buf, field::EPSG)?,
            bounds: Aabb::new(
                le::get_f32(buf, field::BOUNDS_X0)? as f64,
                le::get_f32(buf, field::BOUNDS_Y0)? as f64,
                le::get_f32(buf, field::BOUNDS_X1)? as f64,
                le::get_f32(buf, field::BOUNDS_Y1)? as f64,
            ),
            base_res_cm: le::get_u16(buf, field::BASE_RES_CM)?,
            chunk_size: le::get_u16(buf, field::CHUNK_SIZE)?,
            lod_count: le::get_u8(buf, field::LOD_COUNT)?,
            feature_dim: le::get_u8(buf, field::FEATURE_DIM)?,
            layer_count: le::get_u16(buf, field::LAYER_COUNT)?,
            meta_offset: le::get_u64(buf, field::META_OFFSET)?,
            meta_len: le::get_u32(buf, field::META_LEN)?,
            dir_offset: le::get_u64(buf, field::DIR_OFFSET)?,
            dir_len: le::get_u32(buf, field::DIR_LEN)?,
            footer_offset: le::get_u64(buf, field::FOOTER_OFFSET)?,
            ext_meta_offset: le::get_u64(buf, field::EXT_META_OFFSET)?,
            ext_meta_len: le::get_u32(buf, field::EXT_META_LEN)?,
        })
    }
}
