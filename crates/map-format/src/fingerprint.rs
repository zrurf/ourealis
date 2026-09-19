//! Fingerprints of derived (cache) layers.
//!
//! Slope, distance transform, PRM graphs and K-path libraries are all
//! rebuildable from source layers. The failure mode this guards against is
//! silent: a source is edited, the cache is not rebuilt, and the runtime
//! happily uses stale data. Every derived layer therefore records the *content*
//! hash of the sources it was built from, and loading compares hashes rather
//! than timestamps — copying, partial transfer or chunk reordering cannot
//! produce a false mismatch.

use crate::bytes::Writer;
use crate::directory::ChunkRecord;
use crate::error::{MapError, Result};
use crate::layer::{LayerDesc, LayerId};
use crate::tlv::value::{DerivedLayerEntry, DerivedLayers};
use twox_hash::XxHash64;

/// Derivation algorithm ids, mirroring [`crate::tlv::value::Provenance`].
pub mod algo {
    /// Slope field from the elevation layer.
    pub const SLOPE: u16 = 1;
    /// Euclidean distance transform and gradient from the hard-constraint mask.
    pub const EDT: u16 = 2;
    /// PRM roadmap from features, elevation and constraints.
    pub const PRM: u16 = 3;
    /// K-shortest-path candidate library.
    pub const KPATH: u16 = 4;
    /// Cached cost field.
    pub const COST_CACHE: u16 = 5;
}

/// Status of a derived layer after comparison with its sources.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DerivedStatus {
    /// Fingerprints match; the cache may be used.
    Valid,
    /// Fingerprints differ or no header exists; the cache must be rebuilt.
    Stale {
        /// Human-readable reason, safe to log.
        reason: String,
    },
}

impl DerivedStatus {
    /// True when the cache is usable.
    pub fn is_valid(&self) -> bool {
        matches!(self, DerivedStatus::Valid)
    }
}

/// Hashes an arbitrary byte string with the crate-wide fingerprint function.
pub fn hash_bytes(bytes: &[u8]) -> u64 {
    XxHash64::oneshot(0, bytes)
}

/// Content fingerprint of one source layer.
///
/// Covers the layer descriptor and the `(level, chunk_id, crc32)` triples of
/// every stored chunk in sorted order. Chunk CRCs already cover the payload
/// bytes, so the fingerprint tracks content rather than layout.
pub fn layer_fingerprint(desc: &LayerDesc, records: &[ChunkRecord]) -> u64 {
    let mut sorted: Vec<(u8, u32, u32)> = records
        .iter()
        .filter(|r| r.flags & ChunkRecord::TOMBSTONE == 0)
        .map(|r| (r.level, r.chunk_id, r.crc32))
        .collect();
    sorted.sort_unstable();

    let mut w = Writer::with_capacity(LayerDesc::SIZE + sorted.len() * 12);
    w.write_bytes(&desc.to_bytes());
    for (level, chunk_id, crc) in &sorted {
        w.write_u8(*level);
        w.write_u32(*chunk_id);
        w.write_u32(*crc);
    }
    hash_bytes(w.as_slice())
}

/// Combined fingerprint of several source layers.
pub fn combine_source_fingerprints(fingerprints: &[u64]) -> u64 {
    let mut sorted = fingerprints.to_vec();
    sorted.sort_unstable();
    let mut w = Writer::with_capacity(sorted.len() * 8);
    for value in &sorted {
        w.write_u64(*value);
    }
    hash_bytes(w.as_slice())
}

/// Fingerprint stored in a derived layer's header.
///
/// Mixes the source content, the derivation parameters and the algorithm
/// version, so changing either the inputs, the parameters or the algorithm
/// invalidates the cache.
pub fn derived_fingerprint(
    source_fingerprints: &[u64],
    build_params_hash: u64,
    algo_version: u16,
) -> u64 {
    let sources = combine_source_fingerprints(source_fingerprints);
    let mut w = Writer::with_capacity(24);
    w.write_u64(sources);
    w.write_u64(build_params_hash);
    w.write_u16(algo_version);
    hash_bytes(w.as_slice())
}

/// Builds the header entry of a derived layer.
pub fn make_entry(
    layer_id: LayerId,
    source_fingerprints: &[u64],
    build_params_hash: u64,
    algo_version: u16,
    seeds: Vec<u64>,
) -> DerivedLayerEntry {
    DerivedLayerEntry {
        layer_id,
        source_fingerprint: derived_fingerprint(
            source_fingerprints,
            build_params_hash,
            algo_version,
        ),
        build_params_hash,
        algo_version,
        seeds,
    }
}

/// Checks a derived layer against the fingerprints of its current sources.
pub fn verify(
    headers: &DerivedLayers,
    layer_id: LayerId,
    source_fingerprints: &[u64],
    build_params_hash: u64,
) -> DerivedStatus {
    let Some(entry) = headers.get(layer_id) else {
        return DerivedStatus::Stale {
            reason: format!("no fingerprint header for layer {layer_id}"),
        };
    };
    let expected = derived_fingerprint(
        source_fingerprints,
        entry.build_params_hash,
        entry.algo_version,
    );
    if entry.source_fingerprint != expected {
        return DerivedStatus::Stale {
            reason: format!(
                "layer {layer_id} records fingerprint {:#018x} but its sources hash to {expected:#018x}",
                entry.source_fingerprint
            ),
        };
    }
    if entry.build_params_hash != build_params_hash {
        return DerivedStatus::Stale {
            reason: format!(
                "layer {layer_id} was built with parameter set {:#018x}, current set is {build_params_hash:#018x}",
                entry.build_params_hash
            ),
        };
    }
    DerivedStatus::Valid
}

/// Convenience: returns `Ok(())` when valid, otherwise a stale-layer error.
pub fn require_valid(status: DerivedStatus, layer_id: LayerId) -> Result<()> {
    match status {
        DerivedStatus::Valid => Ok(()),
        DerivedStatus::Stale { reason } => Err(MapError::StaleDerived {
            layer_id: layer_id.raw(),
            reason,
        }),
    }
}
