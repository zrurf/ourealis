//! OMF reader.
//!
//! Opening a map validates the footer, the header CRC and the layer registry;
//! chunk payloads are read lazily and cached. Derived layers are never handed
//! out without a fingerprint check — the only way to obtain one is through a
//! method that verifies it against the current source content, so a stale cache
//! cannot be used silently.

use std::collections::{HashMap, VecDeque};
use std::fs::File;
use std::path::Path;
use std::sync::{Arc, Mutex};

use crate::codec::{self, ChunkShape, CodecContext};
use crate::directory::{ChunkDirectory, ChunkRecord};
use crate::error::{MapError, Result};
use crate::fingerprint::{self, DerivedStatus};
use crate::footer::{self, Footer};
use crate::graph::kpath::KPathLibrary;
use crate::graph::prm::PrmGraph;
use crate::graph::vector::VectorLayer;
use crate::header::Header;
use crate::layer::{LayerDesc, LayerId};
use crate::quadtree::{QNode, QuadtreeSkeleton};
use crate::raster::{ChunkGrid, RasterChunk, RasterLayerView};
use crate::region::RegionSet;
use crate::tlv::value::*;
use crate::tlv::{TlvBlock, tag};

/// Reads a byte range after checking that it lies inside the file.
///
/// Every length involved comes from the file itself, so a corrupt or hostile
/// image could otherwise request a multi-gigabyte allocation before the read
/// discovers that the data does not exist.
fn read_region(source: &dyn BlockSource, offset: u64, len: usize, what: &str) -> Result<Vec<u8>> {
    match offset.checked_add(len as u64) {
        Some(end) if end <= source.len() => source.read_at(offset, len),
        _ => Err(MapError::invalid(format!(
            "{what} at offset {offset} with length {len} extends past the end of the {}-byte file",
            source.len()
        ))),
    }
}

/// Random-access byte source backing a [`Map`].
pub trait BlockSource: Send + Sync {
    /// Reads exactly `len` bytes at `offset`.
    fn read_at(&self, offset: u64, len: usize) -> Result<Vec<u8>>;
    /// Total length of the source in bytes.
    fn len(&self) -> u64;
    /// True when the source is empty.
    fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// Source backed by a file, using positional reads so no seek lock is needed.
#[derive(Debug)]
pub struct FileSource {
    file: File,
    len: u64,
}

impl FileSource {
    /// Opens a file as a block source.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let file = File::open(path.as_ref())?;
        let len = file.metadata()?.len();
        Ok(Self { file, len })
    }
}

impl BlockSource for FileSource {
    fn read_at(&self, offset: u64, len: usize) -> Result<Vec<u8>> {
        let mut buffer = vec![0u8; len];
        let mut filled = 0usize;
        while filled < len {
            let read = read_at_impl(&self.file, &mut buffer[filled..], offset + filled as u64)?;
            if read == 0 {
                return Err(MapError::Truncated {
                    offset,
                    needed: len,
                    available: filled,
                });
            }
            filled += read;
        }
        Ok(buffer)
    }

    fn len(&self) -> u64 {
        self.len
    }
}

#[cfg(windows)]
fn read_at_impl(file: &File, buffer: &mut [u8], offset: u64) -> Result<usize> {
    use std::os::windows::fs::FileExt;
    Ok(file.seek_read(buffer, offset)?)
}

#[cfg(unix)]
fn read_at_impl(file: &File, buffer: &mut [u8], offset: u64) -> Result<usize> {
    use std::os::unix::fs::FileExt;
    Ok(file.read_at(buffer, offset)?)
}

/// Source backed by an in-memory image.
#[derive(Debug, Clone)]
pub struct MemSource {
    bytes: Arc<Vec<u8>>,
}

impl MemSource {
    /// Wraps an owned buffer.
    pub fn new(bytes: Vec<u8>) -> Self {
        Self {
            bytes: Arc::new(bytes),
        }
    }
}

impl BlockSource for MemSource {
    fn read_at(&self, offset: u64, len: usize) -> Result<Vec<u8>> {
        let start = offset as usize;
        let end = start.saturating_add(len);
        self.bytes
            .get(start..end)
            .map(|slice| slice.to_vec())
            .ok_or(MapError::Truncated {
                offset,
                needed: len,
                available: self.bytes.len().saturating_sub(start),
            })
    }

    fn len(&self) -> u64 {
        self.bytes.len() as u64
    }
}

/// Summary numbers useful for diagnostics and loading decisions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MapStats {
    /// Total file length.
    pub file_len: u64,
    /// Number of stored chunks.
    pub chunk_count: usize,
    /// Number of registered layers.
    pub layer_count: usize,
    /// Number of quadtree skeleton nodes.
    pub skeleton_nodes: usize,
    /// Sum of stored (compressed) chunk sizes.
    pub stored_bytes: u64,
    /// Sum of decompressed chunk sizes.
    pub raw_bytes: u64,
}

struct ChunkCache {
    entries: HashMap<(u16, u8, u32), Arc<Vec<u8>>>,
    order: VecDeque<(u16, u8, u32)>,
    capacity: usize,
}

impl ChunkCache {
    fn new(capacity: usize) -> Self {
        Self {
            entries: HashMap::new(),
            order: VecDeque::new(),
            capacity,
        }
    }

    fn get(&self, key: &(u16, u8, u32)) -> Option<Arc<Vec<u8>>> {
        self.entries.get(key).cloned()
    }

    fn insert(&mut self, key: (u16, u8, u32), value: Arc<Vec<u8>>) {
        if self.capacity == 0 {
            return;
        }
        if self.entries.insert(key, value).is_none() {
            self.order.push_back(key);
        }
        while self.order.len() > self.capacity {
            if let Some(old) = self.order.pop_front() {
                self.entries.remove(&old);
            }
        }
    }

    fn clear(&mut self) {
        self.entries.clear();
        self.order.clear();
    }
}

/// An opened OMF map.
pub struct Map {
    source: Arc<dyn BlockSource>,
    header: Header,
    footer: Footer,
    meta: TlvBlock,
    layers: Vec<LayerDesc>,
    grid: ChunkGrid,
    directory: ChunkDirectory,
    skeleton: QuadtreeSkeleton,
    zstd_dict: Option<Vec<u8>>,
    cache: Mutex<ChunkCache>,
    stale_derived: Vec<(LayerId, String)>,
}

impl std::fmt::Debug for Map {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Map")
            .field("file_len", &self.source.len())
            .field("layers", &self.layers.len())
            .field("chunks", &self.directory.len())
            .field("skeleton_nodes", &self.skeleton.len())
            .finish()
    }
}

impl Map {
    /// Opens a map file.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let source = FileSource::open(path)?;
        Self::with_source(Arc::new(source), true).map_err(|e| e.with_path(path))
    }

    /// Opens a map from an in-memory image.
    pub fn from_bytes(bytes: Vec<u8>) -> Result<Self> {
        Self::with_source(Arc::new(MemSource::new(bytes)), true)
    }

    /// Opens a map over an arbitrary block source.
    ///
    /// `verify_hash` recomputes the whole-file hash; disabling it is a
    /// performance option for very large files, not a correctness shortcut for
    /// derived layers, which are always fingerprint-checked on load.
    pub fn with_source(source: Arc<dyn BlockSource>, verify_hash: bool) -> Result<Self> {
        let len = source.len();
        if len < (crate::header::field::SIZE + footer::field::SIZE) as u64 {
            return Err(MapError::invalid(format!(
                "file is {len} byte(s) long, too short to hold an OMF header and footer"
            )));
        }

        let header_bytes = source.read_at(0, crate::header::field::SIZE)?;
        let header = Header::from_bytes(&header_bytes)?;

        let footer_offset = len - footer::field::SIZE as u64;
        if header.footer_offset != 0 && header.footer_offset != footer_offset {
            return Err(MapError::invalid(format!(
                "header points at footer offset {}, file length implies {footer_offset}",
                header.footer_offset
            )));
        }
        let footer_bytes = source.read_at(footer_offset, footer::field::SIZE)?;
        let footer = Footer::from_bytes(&footer_bytes)?;
        if footer.file_len != len {
            return Err(MapError::invalid(format!(
                "footer records file length {}, actual length is {len}",
                footer.file_len
            )));
        }
        if header.version_minor != footer.version_minor {
            return Err(MapError::invalid(
                "header and footer disagree on the minor version",
            ));
        }

        if verify_hash {
            let computed = compose_hash(&header_bytes, &source, len)?;
            if computed != footer.file_hash {
                return Err(MapError::FileHash {
                    stored: footer.file_hash,
                    computed,
                });
            }
        }

        let meta_bytes = read_region(
            source.as_ref(),
            header.meta_offset,
            header.meta_len as usize,
            "meta block",
        )?;
        let meta_raw = codec::entropy::zstd_decompress_all(&meta_bytes)?;
        let meta = TlvBlock::from_bytes(&meta_raw)?;

        let layers = match meta.get_as::<LayerTable>(tag::LAYER_TABLE)? {
            Some(table) => table.layers,
            None => Vec::new(),
        };

        let dir_bytes = read_region(
            source.as_ref(),
            header.dir_offset,
            header.dir_len as usize,
            "chunk directory",
        )?;
        let directory = ChunkDirectory::from_bytes(&dir_bytes)?;

        let skeleton = if footer.node_count > 0 {
            let start = skeleton_offset(&header, &meta)?;
            let bytes = read_region(
                source.as_ref(),
                start,
                footer.node_count as usize * QNode::SIZE,
                "quadtree skeleton",
            )?;
            QuadtreeSkeleton::from_bytes(&bytes)?
        } else {
            QuadtreeSkeleton::empty()
        };

        let zstd_dict = meta
            .get_as::<ZstdDict>(tag::ZSTD_DICT)?
            .map(|d| d.0)
            .filter(|d| !d.is_empty());

        let grid = ChunkGrid::new(
            header.bounds,
            header.base_res_m(),
            header.chunk_size as u32,
            header.lod_count,
        );
        // Bounds and resolution are header fields, so the derived grid is only
        // usable once validated: without this a crafted header produces an
        // unaddressable grid and shape-derived allocations of arbitrary size.
        grid.validate()?;

        let mut map = Self {
            source,
            header,
            footer,
            meta,
            layers,
            grid,
            directory,
            skeleton,
            zstd_dict,
            cache: Mutex::new(ChunkCache::new(256)),
            stale_derived: Vec::new(),
        };
        map.stale_derived = map.find_stale_derived()?;
        Ok(map)
    }

    /// Derived layers whose fingerprints no longer match their sources.
    ///
    /// Evaluated once, when the map is opened: the reader is immutable, so the
    /// answer cannot change while the map is alive. Keeping the list lets the
    /// chunk accessors refuse stale caches without re-hashing the directory on
    /// every read.
    fn find_stale_derived(&self) -> Result<Vec<(LayerId, String)>> {
        let mut stale = Vec::new();
        for desc in &self.layers {
            if !desc.layer_id.is_derived() {
                continue;
            }
            if let DerivedStatus::Stale { reason } = self.verify_derived(desc.layer_id)? {
                tracing::warn!(
                    "derived layer {} is stale and will not be served: {}",
                    desc.layer_id,
                    reason
                );
                stale.push((desc.layer_id, reason));
            }
        }
        Ok(stale)
    }

    /// Reason a derived layer is stale, if it is.
    fn stale_reason(&self, layer_id: LayerId) -> Option<&str> {
        self.stale_derived
            .iter()
            .find(|(id, _)| *id == layer_id)
            .map(|(_, reason)| reason.as_str())
    }

    /// Parsed header.
    pub fn header(&self) -> &Header {
        &self.header
    }

    /// Parsed footer.
    pub fn footer(&self) -> &Footer {
        &self.footer
    }

    /// Raw meta block, including unknown records.
    pub fn meta_block(&self) -> &TlvBlock {
        &self.meta
    }

    /// Chunk geometry.
    pub fn grid(&self) -> &ChunkGrid {
        &self.grid
    }

    /// Registered layer descriptors.
    pub fn layers(&self) -> &[LayerDesc] {
        &self.layers
    }

    /// Registered layer ids.
    pub fn layer_ids(&self) -> Vec<LayerId> {
        self.layers.iter().map(|d| d.layer_id).collect()
    }

    /// Descriptor of a layer.
    pub fn layer_desc(&self, layer_id: LayerId) -> Option<&LayerDesc> {
        self.layers.iter().find(|d| d.layer_id == layer_id)
    }

    /// Raster view of a layer, including the levels that carry data.
    pub fn layer(&self, layer_id: LayerId) -> Option<RasterLayerView> {
        let desc = *self.layer_desc(layer_id)?;
        let levels: Vec<u8> = self
            .directory
            .layer_levels()
            .into_iter()
            .filter(|(id, _)| *id == layer_id)
            .map(|(_, level)| level)
            .collect();
        Some(RasterLayerView::new(desc, self.grid, levels))
    }

    /// Quadtree skeleton nodes.
    pub fn skeleton(&self) -> &[QNode] {
        self.skeleton.nodes()
    }

    /// Finest skeleton node covering a position.
    pub fn locate(&self, x: f64, y: f64, max_depth: u8) -> Option<&QNode> {
        self.skeleton.locate(
            x,
            y,
            &self.header.bounds,
            self.header.base_res_m(),
            max_depth,
        )
    }

    /// Chunk directory.
    pub fn directory(&self) -> &ChunkDirectory {
        &self.directory
    }

    /// Clears the decoded-chunk cache.
    pub fn clear_cache(&self) {
        if let Ok(mut cache) = self.cache.lock() {
            cache.clear();
        }
    }

    /// Directory records of a layer at a level.
    pub fn records(&self, layer_id: LayerId, level: u8) -> &[ChunkRecord] {
        self.directory.for_layer_level(layer_id, level)
    }

    /// Decoded chunk, or `None` when the area is described at a coarser level.
    pub fn chunk(
        &self,
        layer_id: LayerId,
        level: u8,
        chunk_id: u32,
    ) -> Result<Option<RasterChunk>> {
        let desc = self.layer_desc(layer_id).ok_or(MapError::LayerNotFound {
            layer_id: layer_id.raw(),
        })?;
        let payload = match self.chunk_payload(layer_id, level, chunk_id)? {
            Some(bytes) => bytes,
            None => return Ok(None),
        };
        let shape = self.grid.chunk_shape(desc, level);
        let chunk = crate::raster::unpack(desc, &shape, &payload)?;
        Ok(Some(chunk))
    }

    /// Decompressed chunk payload bytes.
    ///
    /// A stale derived layer is refused here rather than at its dedicated
    /// accessor, so that no caller — raster, graph or region — can reach the
    /// payload of a cache whose sources have changed.
    pub fn chunk_payload(
        &self,
        layer_id: LayerId,
        level: u8,
        chunk_id: u32,
    ) -> Result<Option<Vec<u8>>> {
        if let Some(reason) = self.stale_reason(layer_id) {
            return Err(MapError::StaleDerived {
                layer_id: layer_id.raw(),
                reason: reason.to_string(),
            });
        }
        let record = match self.directory.find(layer_id, level, chunk_id) {
            Some(record) => *record,
            None => return Ok(None),
        };
        if record.flags & ChunkRecord::TOMBSTONE != 0 {
            return Ok(None);
        }
        if !codec::is_supported(record.codec) {
            return Err(MapError::UnsupportedCodec {
                codec: record.codec,
                layer_id: layer_id.raw(),
                level,
                chunk_id,
            });
        }
        let key = (layer_id.raw(), level, chunk_id);
        if let Ok(cache) = self.cache.lock()
            && let Some(hit) = cache.get(&key)
        {
            return Ok(Some(hit.as_ref().clone()));
        }

        let stored = read_region(
            self.source.as_ref(),
            record.offset,
            record.comp_len as usize,
            "chunk payload",
        )?;
        let crc = crc32fast::hash(&stored);
        if crc != record.crc32 {
            return Err(MapError::ChunkCrc {
                layer_id: layer_id.raw(),
                level,
                chunk_id,
                stored: record.crc32,
                computed: crc,
            });
        }

        let desc = self.layer_desc(layer_id).ok_or(MapError::LayerNotFound {
            layer_id: layer_id.raw(),
        })?;
        let shape: ChunkShape = if desc.kind.is_chunked() {
            let shape = self.grid.chunk_shape(desc, level);
            // The directory records the decompressed payload length, so it must
            // agree with the shape. Besides rejecting corrupt records, this is
            // what caps every shape-derived allocation below by the on-disk
            // field instead of by the header alone.
            let total = shape
                .total_bytes_checked()
                .ok_or_else(|| MapError::invalid("chunk shape overflows"))?;
            if total > codec::MAX_RAW_CHUNK_BYTES {
                return Err(MapError::invalid(format!(
                    "layer {layer_id} chunk shape demands {total} byte(s), above the {}-byte limit",
                    codec::MAX_RAW_CHUNK_BYTES
                )));
            }
            if total != record.raw_len as usize {
                return Err(MapError::invalid(format!(
                    "layer {layer_id} level {level} chunk {chunk_id} records a {}-byte payload but its shape is {total} byte(s)",
                    record.raw_len
                )));
            }
            shape
        } else {
            // Graph-shaped layers are stored as one opaque payload; their
            // "shape" is the byte count itself, so the codec layer treats the
            // payload as one row of raw bytes. The header alone would let a
            // record ask for an arbitrary allocation here, so the same cap the
            // chunked path uses applies before the codec sees the shape.
            let total = record.raw_len as usize;
            if total > codec::MAX_RAW_CHUNK_BYTES {
                return Err(MapError::invalid(format!(
                    "layer {layer_id} declares a {total}-byte payload, above the {}-byte limit",
                    codec::MAX_RAW_CHUNK_BYTES
                )));
            }
            ChunkShape::new(record.raw_len, 1, 1, crate::layer::DType::U8)
        };
        let ctx = CodecContext {
            dict: self.zstd_dict.as_deref(),
            parent: None,
        };
        let payload = codec::decode(record.codec, &shape, &stored, &ctx)?;

        if let Ok(mut cache) = self.cache.lock() {
            cache.insert(key, Arc::new(payload.clone()));
        }
        Ok(Some(payload))
    }

    /// Opaque payload of a non-chunked (graph, region, vector) layer.
    pub fn section(&self, layer_id: LayerId) -> Result<Option<Vec<u8>>> {
        self.chunk_payload(layer_id, 0, 0)
    }

    /// Number of stored batches of a multi-batch layer such as the PRM graph.
    pub fn batch_count(&self, layer_id: LayerId) -> usize {
        self.directory.for_layer_level(layer_id, 0).len()
    }

    /// `MAP_INFO`, when present.
    pub fn map_info(&self) -> Result<Option<MapInfo>> {
        self.meta.get_as(tag::MAP_INFO)
    }

    /// `FEATURE_SCHEMA`, when present.
    pub fn feature_schema(&self) -> Result<Option<FeatureSchema>> {
        self.meta.get_as(tag::FEATURE_SCHEMA)
    }

    /// `WEIGHT_PRIOR`, when present.
    pub fn weight_prior(&self) -> Result<Option<WeightPrior>> {
        self.meta.get_as(tag::WEIGHT_PRIOR)
    }

    /// `PROVENANCE`, when present.
    pub fn provenance(&self) -> Result<Option<Provenance>> {
        self.meta.get_as(tag::PROVENANCE)
    }

    /// `MAGNETIC_FIELD`, when present.
    pub fn magnetic_field(&self) -> Result<Option<MagneticField>> {
        self.meta.get_as(tag::MAGNETIC_FIELD)
    }

    /// `GLOBAL_STATS`, when present.
    pub fn global_stats(&self) -> Result<Option<GlobalStats>> {
        self.meta.get_as(tag::GLOBAL_STATS)
    }

    /// `PRM_SEEDS`, when present.
    pub fn prm_seeds(&self) -> Result<Option<PrmSeeds>> {
        self.meta.get_as(tag::PRM_SEEDS)
    }

    /// `CHUNK_LAYOUT`, when present.
    pub fn chunk_layout(&self) -> Result<Option<ChunkLayout>> {
        self.meta.get_as(tag::CHUNK_LAYOUT)
    }

    /// `DERIVED_LAYERS`, when present.
    pub fn derived_layers(&self) -> Result<Option<DerivedLayers>> {
        self.meta.get_as(tag::DERIVED_LAYERS)
    }

    /// `CONNECTOR_TABLE`, when present.
    pub fn connectors(&self) -> Result<Option<ConnectorTable>> {
        self.meta.get_as(tag::CONNECTOR_TABLE)
    }

    /// `SLOPE_MODEL`, falling back to the specification defaults.
    pub fn slope_model(&self) -> Result<SlopeModel> {
        Ok(self
            .meta
            .get_as::<SlopeModel>(tag::SLOPE_MODEL)?
            .unwrap_or_default())
    }

    /// `AGGREGATION_RULES`, falling back to the specification defaults.
    pub fn aggregation_rules(&self) -> Result<AggregationRules> {
        Ok(self
            .meta
            .get_as::<AggregationRules>(tag::AGGREGATION_RULES)?
            .unwrap_or_default())
    }

    /// Loads one PRM batch after checking its fingerprint.
    pub fn prm_graph(&self, batch: u16) -> Result<Option<PrmGraph>> {
        if self.layer_desc(LayerId::PRM_GRAPH).is_none() {
            return Ok(None);
        }
        fingerprint::require_valid(self.verify_derived(LayerId::PRM_GRAPH)?, LayerId::PRM_GRAPH)?;
        let Some(payload) = self.chunk_payload(LayerId::PRM_GRAPH, 0, batch as u32)? else {
            return Ok(None);
        };
        Ok(Some(PrmGraph::decode(&payload)?))
    }

    /// Loads every PRM batch in the file.
    pub fn prm_graphs(&self) -> Result<Vec<PrmGraph>> {
        let mut out = Vec::new();
        for batch in 0..self.batch_count(LayerId::PRM_GRAPH) {
            if let Some(graph) = self.prm_graph(batch as u16)? {
                out.push(graph);
            }
        }
        Ok(out)
    }

    /// Loads the K-path library after checking its fingerprint.
    pub fn kpath_library(&self) -> Result<Option<KPathLibrary>> {
        if self.layer_desc(LayerId::KPATH_LIBRARY).is_none() {
            return Ok(None);
        }
        fingerprint::require_valid(
            self.verify_derived(LayerId::KPATH_LIBRARY)?,
            LayerId::KPATH_LIBRARY,
        )?;
        self.section(LayerId::KPATH_LIBRARY)?
            .map(|payload| KPathLibrary::decode(&payload))
            .transpose()
    }

    /// Loads the region set.
    pub fn regions(&self) -> Result<Option<RegionSet>> {
        self.section(LayerId::REGIONS)?
            .map(|payload| RegionSet::decode(&payload))
            .transpose()
    }

    /// Loads the vector layer.
    pub fn vectors(&self) -> Result<Option<VectorLayer>> {
        self.section(LayerId::VECTORS)?
            .map(|payload| VectorLayer::decode(&payload))
            .transpose()
    }

    /// Source layers a derived layer is built from.
    ///
    /// Mirrors the builder's table item for item. Ids matched by no arm have no
    /// source set, so [`Map::verify_derived`] checks them against an empty
    /// fingerprint list, which is what the builder writes for them.
    pub fn sources_of(&self, layer_id: LayerId) -> Vec<LayerId> {
        let features = || {
            self.layers
                .iter()
                .map(|d| d.layer_id)
                .filter(|id| (0x1000..0x1100).contains(&id.raw()))
                .collect::<Vec<_>>()
        };
        match layer_id {
            LayerId::SLOPE => vec![LayerId::ELEVATION],
            LayerId::EDT => vec![LayerId::HARD_FORBIDDEN],
            LayerId::PRM_GRAPH => {
                let mut sources = features();
                sources.push(LayerId::ELEVATION);
                sources.push(LayerId::HARD_FORBIDDEN);
                sources.push(LayerId::DIRECTION);
                sources
            }
            LayerId::KPATH_LIBRARY => {
                vec![LayerId::PRM_GRAPH, LayerId::HARD_FORBIDDEN]
            }
            LayerId::COST_CACHE => {
                let mut sources = features();
                sources.push(LayerId::HARD_FORBIDDEN);
                sources
            }
            _ => Vec::new(),
        }
    }

    /// Current content fingerprint of a source layer.
    ///
    /// Covers every level the layer stores data at, so adding or removing a
    /// coarse level invalidates dependants just like editing fine data.
    pub fn source_fingerprint(&self, layer_id: LayerId) -> Option<u64> {
        let desc = self.layer_desc(layer_id)?;
        let records: Vec<ChunkRecord> = self
            .directory
            .layer_levels()
            .into_iter()
            .filter(|(id, _)| *id == layer_id)
            .flat_map(|(_, level)| self.directory.for_layer_level(layer_id, level))
            .copied()
            .collect();
        Some(fingerprint::layer_fingerprint(desc, &records))
    }

    /// Checks a derived layer against the fingerprints of its sources.
    pub fn verify_derived(&self, layer_id: LayerId) -> Result<DerivedStatus> {
        let Some(desc) = self.layer_desc(layer_id) else {
            return Ok(DerivedStatus::Stale {
                reason: format!("layer {layer_id} is not registered"),
            });
        };
        if !desc.layer_id.is_derived() {
            return Ok(DerivedStatus::Valid);
        }
        let Some(headers) = self.derived_layers()? else {
            return Ok(DerivedStatus::Stale {
                reason: format!("layer {layer_id} has no fingerprint table"),
            });
        };
        let mut fingerprints = Vec::new();
        for source in self.sources_of(layer_id) {
            if self.layer_desc(source).is_none() {
                continue;
            }
            if let Some(value) = self.source_fingerprint(source) {
                fingerprints.push(value);
            }
        }
        let params_hash = headers
            .get(layer_id)
            .map(|entry| entry.build_params_hash)
            .unwrap_or(0);
        Ok(fingerprint::verify(
            &headers,
            layer_id,
            &fingerprints,
            params_hash,
        ))
    }

    /// Recomputes and compares the whole-file hash.
    pub fn verify_file_hash(&self) -> Result<()> {
        let len = self.source.len();
        let header_bytes = self.source.read_at(0, crate::header::field::SIZE)?;
        let computed = compose_hash(&header_bytes, &self.source, len)?;
        if computed != self.footer.file_hash {
            return Err(MapError::FileHash {
                stored: self.footer.file_hash,
                computed,
            });
        }
        Ok(())
    }

    /// Summary statistics.
    pub fn stats(&self) -> MapStats {
        MapStats {
            file_len: self.source.len(),
            chunk_count: self.directory.len(),
            layer_count: self.layers.len(),
            skeleton_nodes: self.skeleton.len(),
            stored_bytes: self.directory.total_compressed_bytes(),
            raw_bytes: self.directory.total_raw_bytes(),
        }
    }

    /// True when the file declares itself as world-coordinate capable.
    pub fn has_geo_reference(&self) -> bool {
        self.header.ref_lon != 0.0 || self.header.ref_lat != 0.0
    }
}

/// Recomputes the whole-file hash of a source without loading the body twice.
///
/// Mirrors [`crate::writer::MapWriter::finish`]: the body digest covers every
/// byte from the end of the header up to the 16-byte hash field, and the final
/// header is composed on top of it.
fn compose_hash(header_bytes: &[u8], source: &Arc<dyn BlockSource>, len: u64) -> Result<[u8; 16]> {
    let header_len = crate::header::field::SIZE as u64;
    let body_len = (len - header_len - footer::field::FILE_HASH_SIZE as u64) as usize;
    let body = source.read_at(header_len, body_len)?;
    let digest = twox_hash::XxHash3_128::oneshot(&body).to_le_bytes();
    let mut header = [0u8; crate::header::field::SIZE];
    let take = header_bytes.len().min(header.len());
    header[..take].copy_from_slice(&header_bytes[..take]);
    Ok(footer::compose_file_hash(&header, &digest))
}

/// Computes where the skeleton starts from the header and meta block.
fn skeleton_offset(header: &Header, _meta: &TlvBlock) -> Result<u64> {
    let end = header.meta_offset + header.meta_len as u64 + header.ext_meta_len as u64;
    Ok(crate::writer::align8(end))
}
