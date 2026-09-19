//! # Ourealis Map Format (OMF)
//!
//! Reader, writer and builder for the static map container consumed by the
//! Ourealis running simulator. One `.omf` file holds everything the simulator
//! needs to know about the environment:
//!
//! * spatial structure — traversable areas, hard obstacles, multi-floor links;
//! * environmental features — surface type, traffic attributes, direction
//!   constraints, crowding (stored as a D-dimensional resistance vector);
//! * terrain — elevation, and derived slope / distance fields;
//! * precomputed derived data — Euclidean distance transform, PRM waypoint
//!   graphs, K-shortest-path libraries;
//! * annotations used by sensor simulation — multipath regions, GNSS dropout
//!   zones, local magnetic field parameters.
//!
//! The format is **read-only and immutable**: content changes happen by
//! rebuilding the file or by applying a patch. It stores objective environment
//! descriptions only — never a synthesised cost field — so one map can serve
//! every motion mode.
//!
//! ## Layout
//!
//! ```text
//! Header (128 B)      magic, version, reference point, bounds, section offsets
//! Meta Block          zstd-compressed TLV sequence (metadata, connectors, stats)
//! Quadtree Skeleton   Morton-sorted fixed-size nodes, resident in memory
//! Chunk Data Region   independently compressed raster / graph / region chunks
//! Directory           ChunkRecords sorted by (layer_id, level, chunk_id)
//! Footer (64 B)       directory pointer, counts, whole-file hash
//! ```
//!
//! Reading order is Footer → Header → Directory → chunks on demand; the write
//! order differs (data first, then the directory, footer and header backfill).
//!
//! ## Design rules the implementation enforces
//!
//! 1. **Unknown means skip.** Unrecognised TLVs, layers and codecs are skipped
//!    safely (or reported as unreadable) instead of failing the load.
//! 2. **Derived data carries a fingerprint.** Slope, EDT, PRM and K-path layers
//!    record the content hash of their sources; a mismatch rejects the layer so
//!    stale caches are never used silently.
//! 3. **No synthesised quantities.** Weighted cost fields, attention gating and
//!    Logit choice results are runtime products and never appear in the file.

#![warn(missing_docs)]
#![warn(clippy::doc_markdown)]

pub mod builder;
pub mod bytes;
pub mod codec;
pub mod directory;
pub mod error;
pub mod fingerprint;
pub mod footer;
pub mod geometry;
pub mod graph;
pub mod header;
pub mod layer;
pub mod motion;
pub mod patch;
pub mod quadtree;
pub mod raster;
pub mod reader;
pub mod region;
pub mod synthetic;
pub mod tlv;
pub mod writer;

pub use builder::{MapBuilder, PartitionOptions};
pub use directory::{ChunkDirectory, ChunkRecord};
pub use error::{MapError, Result};
pub use fingerprint::{DerivedStatus, algo};
pub use footer::Footer;
pub use geometry::Aabb;
pub use graph::kpath::{KPath, KPathLibrary, KPathParams};
pub use graph::prm::{PrmEdge, PrmGraph, PrmNode};
pub use graph::vector::{VectorKind, VectorLayer, VectorShape};
pub use header::{Header, HeaderFlags};
pub use layer::{DType, LayerDesc, LayerId, LayerKind};
pub use motion::MotionMode;
pub use patch::Patch;
pub use quadtree::{QNode, QuadtreeSkeleton};
pub use raster::{ChunkGrid, RasterChunk, RasterLayerView};
pub use reader::{BlockSource, FileSource, Map, MapStats, MemSource};
pub use region::{RegionFeature, RegionSet, RegionTag, SpatialEvent, TriggerMode};
pub use writer::{MapHeaderSpec, MapWriter};
