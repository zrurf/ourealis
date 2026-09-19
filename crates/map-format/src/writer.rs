//! OMF writer.
//!
//! Physical layout is fixed and written in order:
//!
//! ```text
//! [Header 128 B][Meta Block][Quadtree Skeleton][Chunk Data Region][Directory][Footer 64 B]
//! ```
//!
//! Metadata and the skeleton are serialised into the file the moment the first
//! chunk is written, so they must be set beforehand — attempting to change them
//! later returns an error that says exactly that. The directory and footer are
//! written by [`MapWriter::finish`], which then seeks back to patch the header.
//!
//! **Whole-file hash.** The header is only final after the directory offsets are
//! known, so it cannot take part in a single forward streaming pass. The file
//! hash is therefore composed: `H(header ‖ H(body))` where `body` is every byte
//! from offset 128 up to the hash field. Every byte of the file is covered
//! exactly once, and both writer and reader can compute it without reading the
//! file twice.

use std::fs::File;
use std::io::{BufWriter, Read, Seek, SeekFrom, Write};
use std::path::Path;

use crate::codec::{self, ChunkShape, CodecContext};
use crate::directory::{ChunkDirectory, ChunkRecord};
use crate::error::{MapError, Result};
use crate::footer::{self, Footer};
use crate::geometry::Aabb;
use crate::header::{self, Header, HeaderFlags};
use crate::layer::{LayerDesc, LayerId};
use crate::quadtree::QNode;
use crate::raster::{ChunkGrid, RasterChunk};
use crate::tlv::value::*;
use crate::tlv::{TlvBlock, TlvRecord, TlvValue, tag};

/// Header fields the caller supplies; everything else is derived from content.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MapHeaderSpec {
    /// Reference longitude in radians (origin of the local metre plane).
    pub ref_lon: f64,
    /// Reference latitude in radians.
    pub ref_lat: f64,
    /// EPSG code, or 0 for a pure local metre plane.
    pub epsg: u32,
    /// Map extent in the local metre plane.
    pub bounds: Aabb,
    /// Finest resolution in centimetres per cell.
    pub base_res_cm: u16,
    /// Chunk side length in cells.
    pub chunk_size: u16,
    /// Number of LOD pyramid levels.
    pub lod_count: u8,
}

impl Default for MapHeaderSpec {
    fn default() -> Self {
        Self {
            ref_lon: 0.0,
            ref_lat: 0.0,
            epsg: 0,
            bounds: Aabb::new(0.0, 0.0, 1000.0, 1000.0),
            base_res_cm: 50,
            chunk_size: 256,
            lod_count: 1,
        }
    }
}

impl MapHeaderSpec {
    /// Chunk geometry implied by the specification.
    pub fn grid(&self) -> ChunkGrid {
        ChunkGrid::new(
            self.bounds,
            self.base_res_cm as f64 / 100.0,
            self.chunk_size as u32,
            self.lod_count,
        )
    }
}

/// Streaming sink that supports the writer's seek-back operations.
pub trait MapSink: Write + Seek + Send {}
impl<T: Write + Seek + Send> MapSink for T {}

/// Streams an OMF file into any seekable sink.
pub struct MapWriter<W: Write + Seek + Send> {
    sink: W,
    spec: MapHeaderSpec,
    meta: TlvBlock,
    ext_meta: Vec<TlvRecord>,
    skeleton: Vec<QNode>,
    layers: Vec<LayerDesc>,
    directory: ChunkDirectory,
    body_hasher: twox_hash::XxHash3_128,
    prologue: Option<Prologue>,
    finished: bool,
}

#[derive(Debug, Clone, Copy)]
struct Prologue {
    meta_offset: u64,
    meta_len: u32,
    ext_offset: u64,
    ext_len: u32,
}

impl MapWriter<BufWriter<File>> {
    /// Creates a writer that builds `path`.
    pub fn create(path: impl AsRef<Path>, spec: MapHeaderSpec) -> Result<Self> {
        let path = path.as_ref();
        let file = File::create(path)?;
        Self::new(BufWriter::new(file), spec).map_err(|e| e.with_path(path))
    }
}

impl<W: Write + Seek + Send> MapWriter<W> {
    /// Creates a writer over an arbitrary seekable sink.
    pub fn new(sink: W, spec: MapHeaderSpec) -> Result<Self> {
        if spec.chunk_size == 0 {
            return Err(MapError::invalid("chunk_size must be non-zero"));
        }
        if spec.base_res_cm == 0 {
            return Err(MapError::invalid("base_res_cm must be non-zero"));
        }
        if spec.bounds.width() <= 0.0 || spec.bounds.height() <= 0.0 {
            return Err(MapError::invalid("map bounds must have positive extent"));
        }
        spec.grid().validate()?;
        Ok(Self {
            sink,
            spec,
            meta: TlvBlock::new(),
            ext_meta: Vec::new(),
            skeleton: Vec::new(),
            layers: Vec::new(),
            directory: ChunkDirectory::default(),
            body_hasher: twox_hash::XxHash3_128::new(),
            prologue: None,
            finished: false,
        })
    }

    /// Header specification this writer was created with.
    pub fn spec(&self) -> &MapHeaderSpec {
        &self.spec
    }

    /// Chunk geometry implied by the specification.
    pub fn grid(&self) -> ChunkGrid {
        self.spec.grid()
    }

    /// Number of chunks written so far.
    pub fn chunk_count(&self) -> usize {
        self.directory.len()
    }

    fn ensure_metadata_mutable(&self, what: &str) -> Result<()> {
        if self.prologue.is_some() {
            return Err(MapError::invalid(format!(
                "cannot set {what} after the first chunk was written; set all metadata first"
            )));
        }
        Ok(())
    }

    /// Stores a typed metadata record, replacing any previous value.
    pub fn set_meta<T: TlvValue>(&mut self, tag_value: u32, name: &str, value: &T) -> Result<()> {
        self.ensure_metadata_mutable(name)?;
        self.meta.set(TlvRecord::encode(tag_value, value));
        Ok(())
    }

    /// Stores a raw metadata record, replacing any previous value.
    pub fn set_meta_raw(&mut self, record: TlvRecord) -> Result<()> {
        self.ensure_metadata_mutable("a metadata record")?;
        self.meta.set(record);
        Ok(())
    }

    /// Adds a record to the optional extension TLV area.
    pub fn push_ext_meta(&mut self, record: TlvRecord) -> Result<()> {
        self.ensure_metadata_mutable("an extension record")?;
        self.ext_meta.push(record);
        Ok(())
    }

    /// Sets `MAP_INFO`.
    pub fn set_map_info(&mut self, info: &MapInfo) -> Result<()> {
        self.set_meta(tag::MAP_INFO, "MAP_INFO", info)
    }

    /// Sets `FEATURE_SCHEMA`.
    pub fn set_feature_schema(&mut self, schema: &FeatureSchema) -> Result<()> {
        self.set_meta(tag::FEATURE_SCHEMA, "FEATURE_SCHEMA", schema)
    }

    /// Sets `WEIGHT_PRIOR`.
    pub fn set_weight_prior(&mut self, prior: &WeightPrior) -> Result<()> {
        self.set_meta(tag::WEIGHT_PRIOR, "WEIGHT_PRIOR", prior)
    }

    /// Sets `SLOPE_MODEL`.
    pub fn set_slope_model(&mut self, model: &SlopeModel) -> Result<()> {
        self.set_meta(tag::SLOPE_MODEL, "SLOPE_MODEL", model)
    }

    /// Sets `CONNECTOR_TABLE`.
    pub fn set_connectors(&mut self, table: &ConnectorTable) -> Result<()> {
        self.set_meta(tag::CONNECTOR_TABLE, "CONNECTOR_TABLE", table)
    }

    /// Sets `MAGNETIC_FIELD`.
    pub fn set_magnetic_field(&mut self, field: &MagneticField) -> Result<()> {
        self.set_meta(tag::MAGNETIC_FIELD, "MAGNETIC_FIELD", field)
    }

    /// Sets `PROVENANCE`.
    pub fn set_provenance(&mut self, provenance: &Provenance) -> Result<()> {
        self.set_meta(tag::PROVENANCE, "PROVENANCE", provenance)
    }

    /// Sets `GLOBAL_STATS`.
    pub fn set_global_stats(&mut self, stats: &GlobalStats) -> Result<()> {
        self.set_meta(tag::GLOBAL_STATS, "GLOBAL_STATS", stats)
    }

    /// Sets `AGGREGATION_RULES`.
    pub fn set_aggregation_rules(&mut self, rules: &AggregationRules) -> Result<()> {
        self.set_meta(tag::AGGREGATION_RULES, "AGGREGATION_RULES", rules)
    }

    /// Sets `CHUNK_LAYOUT`.
    pub fn set_chunk_layout(&mut self, layout: &ChunkLayout) -> Result<()> {
        self.set_meta(tag::CHUNK_LAYOUT, "CHUNK_LAYOUT", layout)
    }

    /// Sets `PRM_SEEDS`.
    pub fn set_prm_seeds(&mut self, seeds: &PrmSeeds) -> Result<()> {
        self.set_meta(tag::PRM_SEEDS, "PRM_SEEDS", seeds)
    }

    /// Sets `DERIVED_LAYERS`.
    pub fn set_derived_layers(&mut self, derived: &DerivedLayers) -> Result<()> {
        self.set_meta(tag::DERIVED_LAYERS, "DERIVED_LAYERS", derived)
    }

    /// Sets `ZSTD_DICT`, enabling dictionary compression for later chunks.
    pub fn set_zstd_dict(&mut self, dict: &ZstdDict) -> Result<()> {
        self.set_meta(tag::ZSTD_DICT, "ZSTD_DICT", dict)
    }

    /// Registers a layer descriptor.
    pub fn set_layer(&mut self, desc: LayerDesc) -> Result<()> {
        self.ensure_metadata_mutable("a layer descriptor")?;
        if desc.channels == 0 {
            return Err(MapError::invalid(format!(
                "layer {} declares zero channels",
                desc.layer_id
            )));
        }
        if desc.kind.is_chunked() {
            let shape = ChunkShape::new(
                self.spec.chunk_size as u32,
                self.spec.chunk_size as u32,
                desc.channels,
                desc.dtype,
            );
            let total = shape
                .total_bytes_checked()
                .ok_or_else(|| MapError::invalid("chunk shape overflows"))?;
            if total > codec::MAX_RAW_CHUNK_BYTES {
                return Err(MapError::invalid(format!(
                    "layer {} chunk payload of {total} byte(s) exceeds the {}-byte limit",
                    desc.layer_id,
                    codec::MAX_RAW_CHUNK_BYTES
                )));
            }
        }
        match self.layers.iter_mut().find(|d| d.layer_id == desc.layer_id) {
            Some(slot) => *slot = desc,
            None => self.layers.push(desc),
        }
        Ok(())
    }

    /// Replaces the quadtree skeleton.
    pub fn set_skeleton(&mut self, nodes: Vec<QNode>) -> Result<()> {
        self.ensure_metadata_mutable("the quadtree skeleton")?;
        self.skeleton = nodes;
        Ok(())
    }

    /// Writes a raster chunk: packs, compresses and appends it.
    ///
    /// The chunk must be a full `chunk_size` square. The reader derives a
    /// chunk's shape from the layer's grid geometry rather than from the record,
    /// so a partial chunk would be written without complaint and then fail to
    /// load; edge chunks are padded out to the full size, which is what the
    /// reader's shape expects.
    pub fn write_raster_chunk(
        &mut self,
        desc: &LayerDesc,
        level: u8,
        chunk_id: u32,
        codec_override: Option<u8>,
        chunk: &RasterChunk,
    ) -> Result<u32> {
        if chunk.width != self.spec.chunk_size as u32 || chunk.height != self.spec.chunk_size as u32
        {
            return Err(MapError::invalid(format!(
                "chunk is {}x{} cells but the map declares {}x{}; pad edge chunks to the full size",
                chunk.width, chunk.height, self.spec.chunk_size, self.spec.chunk_size
            )));
        }
        let shape = ChunkShape::new(chunk.width, chunk.height, chunk.channels, desc.dtype);
        let payload = crate::raster::pack(desc, chunk)?;
        self.write_payload(desc, level, chunk_id, codec_override, shape, &payload)
    }

    /// Writes an already-packed chunk payload.
    pub fn write_payload(
        &mut self,
        desc: &LayerDesc,
        level: u8,
        chunk_id: u32,
        codec_override: Option<u8>,
        shape: ChunkShape,
        payload: &[u8],
    ) -> Result<u32> {
        let codec_id = codec_override.unwrap_or(desc.codec);
        let dict = self.zstd_dict();
        let ctx = CodecContext {
            dict: dict.as_deref(),
            parent: None,
        };
        let stored = codec::encode(codec_id, &shape, payload, &ctx)?;
        self.append_chunk(desc, level, chunk_id, codec_id, payload.len(), &stored)
    }

    /// Writes an already-encoded chunk verbatim.
    ///
    /// Used by [`crate::builder::MapBuilder`], which encodes chunks before the
    /// prologue so it can compute CRCs and fingerprints over their contents.
    /// The CRC is recomputed here from the same bytes, so the directory and the
    /// fingerprint always agree.
    pub fn write_encoded_chunk(
        &mut self,
        layer_id: LayerId,
        level: u8,
        chunk_id: u32,
        codec_id: u8,
        stored: &[u8],
        raw_len: u32,
    ) -> Result<u32> {
        self.ensure_prologue()?;
        let offset = self.sink.stream_position()?;
        self.write_hashed(stored)?;
        let crc = crc32fast::hash(stored);
        let comp_len = u32::try_from(stored.len())
            .map_err(|_| MapError::invalid("stored chunk length exceeds the u32 field"))?;
        self.directory.upsert(ChunkRecord::new(
            layer_id, level, chunk_id, codec_id, offset, comp_len, raw_len, crc,
        ));
        Ok(crc)
    }

    /// Writes a graph / region / vector section as a single unsplit chunk.
    ///
    /// These layers are not spatially chunked; the whole section is stored with
    /// `level = 0` and `chunk_id = 0` so the directory stays uniform.
    pub fn write_section(&mut self, desc: &LayerDesc, payload: &[u8]) -> Result<u32> {
        self.append_chunk(desc, 0, 0, codec::id::RAW, payload.len(), payload)
    }

    fn append_chunk(
        &mut self,
        desc: &LayerDesc,
        level: u8,
        chunk_id: u32,
        codec_id: u8,
        raw_len: usize,
        stored: &[u8],
    ) -> Result<u32> {
        self.ensure_prologue()?;
        let offset = self.sink.stream_position()?;
        self.write_hashed(stored)?;
        let crc = crc32fast::hash(stored);
        let raw_len = u32::try_from(raw_len)
            .map_err(|_| MapError::invalid("chunk payload length exceeds the u32 field"))?;
        let comp_len = u32::try_from(stored.len())
            .map_err(|_| MapError::invalid("stored chunk length exceeds the u32 field"))?;
        self.directory.upsert(ChunkRecord::new(
            desc.layer_id,
            level,
            chunk_id,
            codec_id,
            offset,
            comp_len,
            raw_len,
            crc,
        ));
        Ok(crc)
    }

    fn zstd_dict(&self) -> Option<Vec<u8>> {
        self.meta
            .get(tag::ZSTD_DICT)
            .and_then(|record| record.decode::<ZstdDict>().ok())
            .map(|dict| dict.0)
            .filter(|bytes| !bytes.is_empty())
    }

    /// Writes bytes and feeds them into the body hash.
    fn write_hashed(&mut self, bytes: &[u8]) -> Result<()> {
        self.sink.write_all(bytes)?;
        self.body_hasher.write(bytes);
        Ok(())
    }

    /// Writes header placeholder, meta block, extension area and skeleton.
    #[allow(clippy::field_reassign_with_default)]
    fn ensure_prologue(&mut self) -> Result<()> {
        if self.prologue.is_some() {
            return Ok(());
        }
        let mut table = LayerTable::default();
        table.layers = self.layers.clone();
        self.meta.set(TlvRecord::encode(tag::LAYER_TABLE, &table));

        let meta_bytes = self.meta.to_bytes();
        // The dictionary is stored inside this very block, so it cannot be used
        // to compress it: the reader has to decompress the meta block before it
        // can obtain the dictionary. Chunk payloads still use it.
        let compressed = codec::entropy::zstd_compress(&meta_bytes, None)?;

        let ext_bytes = if self.ext_meta.is_empty() {
            Vec::new()
        } else {
            let mut block = TlvBlock::new();
            for record in &self.ext_meta {
                block.push(record.clone());
            }
            block.to_bytes()
        };

        let skeleton_bytes: Vec<u8> = self
            .skeleton
            .iter()
            .flat_map(|node| node.to_bytes())
            .collect();

        let meta_offset = header::field::SIZE as u64;
        let ext_offset = if ext_bytes.is_empty() {
            0
        } else {
            meta_offset + compressed.len() as u64
        };
        let skeleton_offset =
            align8(meta_offset + compressed.len() as u64 + ext_bytes.len() as u64);

        self.sink.seek(SeekFrom::Start(0))?;
        self.sink.write_all(&[0u8; header::field::SIZE])?;
        self.write_hashed(&compressed)?;
        if !ext_bytes.is_empty() {
            self.write_hashed(&ext_bytes)?;
        }
        let written = meta_offset + compressed.len() as u64 + ext_bytes.len() as u64;
        if skeleton_offset > written {
            let padding = vec![0u8; (skeleton_offset - written) as usize];
            self.write_hashed(&padding)?;
        }
        self.write_hashed(&skeleton_bytes)?;

        self.prologue = Some(Prologue {
            meta_offset,
            meta_len: compressed.len() as u32,
            ext_offset,
            ext_len: ext_bytes.len() as u32,
        });
        Ok(())
    }

    /// Finalises directory, footer and header, returning the sink.
    pub fn finish(mut self) -> Result<W> {
        if self.finished {
            return Ok(self.sink);
        }
        self.ensure_prologue()?;
        let prologue = self
            .prologue
            .ok_or_else(|| MapError::invalid("writer prologue was not written"))?;

        let dir_bytes = self.directory.to_bytes();
        let dir_offset = self.sink.stream_position()?;
        self.write_hashed(&dir_bytes)?;

        let flags = self.derive_flags();
        let footer_offset = dir_offset + dir_bytes.len() as u64;
        let file_len = footer_offset + footer::field::SIZE as u64;

        let header_bytes = Header {
            version_major: header::VERSION_MAJOR,
            version_minor: header::VERSION_MINOR,
            flags,
            ref_lon: self.spec.ref_lon,
            ref_lat: self.spec.ref_lat,
            epsg: self.spec.epsg,
            bounds: self.spec.bounds,
            base_res_cm: self.spec.base_res_cm,
            chunk_size: self.spec.chunk_size,
            lod_count: self.spec.lod_count,
            feature_dim: self.feature_dim(),
            layer_count: self.layers.len().min(u16::MAX as usize) as u16,
            meta_offset: prologue.meta_offset,
            meta_len: prologue.meta_len,
            dir_offset,
            dir_len: dir_bytes.len() as u32,
            footer_offset,
            ext_meta_offset: prologue.ext_offset,
            ext_meta_len: prologue.ext_len,
        }
        .to_bytes();

        let mut footer = Footer {
            version_major: header::VERSION_MAJOR,
            version_minor: header::VERSION_MINOR,
            flags,
            dir_len: dir_bytes.len() as u32,
            dir_offset,
            dir_record_count: self.directory.len() as u32,
            chunk_count: self.directory.len() as u32,
            node_count: self.skeleton.len() as u32,
            layer_count: self.layers.len() as u32,
            file_len,
            file_hash: [0u8; 16],
        };
        let mut footer_bytes = footer.to_bytes();

        // The hash field sits in the last 16 bytes of the footer, so the first
        // 48 bytes are hashed first and only then is the digest final.
        self.write_hashed(&footer_bytes[..footer::field::FILE_HASH])?;
        let body_digest = self.body_hasher.clone().finish_128().to_le_bytes();
        let file_hash = footer::compose_file_hash(&header_bytes, &body_digest);
        footer.file_hash = file_hash;
        footer_bytes = footer.to_bytes();
        self.sink
            .write_all(&footer_bytes[footer::field::FILE_HASH..])?;

        self.sink.seek(SeekFrom::Start(0))?;
        self.sink.write_all(&header_bytes)?;
        self.sink.flush()?;

        self.finished = true;
        Ok(self.sink)
    }

    // The footer is built incrementally: the hash can only be filled in once
    // the body has been hashed, so the initializer cannot be complete.
    #[allow(clippy::field_reassign_with_default)]
    fn derive_flags(&self) -> HeaderFlags {
        let mut flags = HeaderFlags::default();
        let layers = self.directory.layers();
        if layers.contains(&LayerId::PRM_GRAPH) {
            flags.insert(HeaderFlags::HAS_PRM);
        }
        if layers.contains(&LayerId::KPATH_LIBRARY) {
            flags.insert(HeaderFlags::HAS_KPATH_LIB);
        }
        if layers.contains(&LayerId::REGIONS) {
            flags.insert(HeaderFlags::HAS_REGIONS);
        }
        if layers.contains(&LayerId::VECTORS) {
            flags.insert(HeaderFlags::HAS_VECTORS);
        }
        if self.meta.get(tag::ZSTD_DICT).is_some() {
            flags.insert(HeaderFlags::HAS_ZSTD_DICT);
        }
        flags
    }

    fn feature_dim(&self) -> u8 {
        self.meta
            .get(tag::FEATURE_SCHEMA)
            .and_then(|record| record.decode::<FeatureSchema>().ok())
            .map(|schema| schema.dim())
            .unwrap_or(0)
    }
}

/// Aligns `value` up to the next multiple of eight.
pub fn align8(value: u64) -> u64 {
    (value + 7) & !7
}

/// Reads a complete file image into memory.
pub fn read_file_image(path: impl AsRef<Path>) -> Result<Vec<u8>> {
    let mut file = File::open(path.as_ref())?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;
    Ok(bytes)
}
