//! Error types for the OMF container.

use std::path::PathBuf;

/// Result alias used throughout the crate.
pub type Result<T> = std::result::Result<T, MapError>;

/// Every failure mode of reading, writing or patching an OMF file.
///
/// Variant fields carry the identifiers needed to locate the problem in the
/// file and are named after the entities they refer to, so they are not
/// documented individually.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
#[allow(missing_docs)]
pub enum MapError {
    /// Underlying I/O failure.
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    /// File does not start with the OMF magic.
    #[error("bad magic: expected {expected:?}, found {found:?}")]
    BadMagic { expected: [u8; 4], found: [u8; 4] },

    /// Major version mismatch; the file cannot be interpreted by this reader.
    #[error("unsupported major version {found} (this reader supports {supported})")]
    UnsupportedVersion { found: u16, supported: u16 },

    /// Header CRC32 does not match its contents.
    #[error("header crc32 mismatch: stored {stored:#010x}, computed {computed:#010x}")]
    HeaderCrc { stored: u32, computed: u32 },

    /// A chunk's stored CRC32 does not match its bytes.
    #[error(
        "chunk crc32 mismatch (layer {layer_id:#06x}, level {level}, chunk {chunk_id}): stored {stored:#010x}, computed {computed:#010x}"
    )]
    ChunkCrc {
        layer_id: u16,
        level: u8,
        chunk_id: u32,
        stored: u32,
        computed: u32,
    },

    /// Whole-file hash mismatch, reported when verifying an existing file.
    #[error("file hash mismatch: stored {stored:02x?}, computed {computed:02x?}")]
    FileHash {
        stored: [u8; 16],
        computed: [u8; 16],
    },

    /// A read or decode ran past the end of its buffer.
    #[error("truncated data: need {needed} byte(s) at offset {offset}, only {available} available")]
    Truncated {
        offset: u64,
        needed: usize,
        available: usize,
    },

    /// Chunk uses a codec this reader does not implement. Callers may treat the
    /// chunk as "unreadable" and fall back to a coarser level.
    #[error(
        "unsupported codec {codec:#04x} for layer {layer_id:#06x}, level {level}, chunk {chunk_id}"
    )]
    UnsupportedCodec {
        codec: u8,
        layer_id: u16,
        level: u8,
        chunk_id: u32,
    },

    /// A TLV declared as required by the caller is absent.
    #[error("missing required metadata TLV {tag:#06x} ({name})")]
    MissingTlv { tag: u32, name: &'static str },

    /// Content is inconsistent with the format specification.
    #[error("invalid map data: {0}")]
    Invalid(String),

    /// Referenced layer is not registered in the layer table.
    #[error("layer {layer_id:#06x} is not present in the layer table")]
    LayerNotFound { layer_id: u16 },

    /// A derived (cache) layer does not match the fingerprint of its sources.
    #[error("derived layer {layer_id:#06x} is stale and must be rebuilt: {reason}")]
    StaleDerived { layer_id: u16, reason: String },

    /// A cell-shaped read was asked of a layer that stores one opaque payload.
    ///
    /// Regions, vectors and graphs have no grid: asking for their chunk would derive
    /// a shape from the chunk grid and compare it against the section bytes, which
    /// reads as corruption. Naming it keeps the caller's mistake distinguishable from
    /// a damaged file.
    #[error("layer {layer_id:#06x} is not a cell layer but a {kind} one")]
    NotACellLayer { layer_id: u16, kind: &'static str },

    /// Patch was produced for a different base file.
    #[error("patch base hash {patch:#018x} does not match base file {base:#018x}")]
    PatchBaseMismatch { patch: u64, base: u64 },

    /// Patch attempts to modify a derived layer, which is forbidden.
    #[error("patch may only modify source layers, but targets layer {layer_id:#06x}")]
    PatchDerivedLayer { layer_id: u16 },

    /// Path-based convenience error.
    #[error("failed to open map file {path}: {source}")]
    OpenFile {
        path: PathBuf,
        #[source]
        source: Box<MapError>,
    },
}

impl MapError {
    /// Builds an [`MapError::Invalid`] from anything printable.
    pub fn invalid(msg: impl Into<String>) -> Self {
        MapError::Invalid(msg.into())
    }

    /// Attaches a file path to an error, for reporting convenience.
    pub fn with_path(self, path: impl Into<PathBuf>) -> Self {
        MapError::OpenFile {
            path: path.into(),
            source: Box::new(self),
        }
    }

    /// Whether a caller may degrade gracefully (skip the chunk, use a coarser
    /// level) instead of aborting the whole load.
    ///
    /// Returns `true` for unreadable-but-structurally-valid conditions and
    /// `false` for conditions that indicate the file itself is broken.
    pub fn is_recoverable(&self) -> bool {
        matches!(
            self,
            MapError::UnsupportedCodec { .. }
                | MapError::ChunkCrc { .. }
                | MapError::StaleDerived { .. }
                | MapError::LayerNotFound { .. }
        )
    }
}
