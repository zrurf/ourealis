//! High-level map assembly.
//!
//! The builder takes full-resolution layer grids and does the work that must
//! happen before anything reaches the disk:
//!
//! * chunk splitting, quantisation and encoding (with the CRCs the directory
//!   and the fingerprints need);
//! * global per-channel statistics, which the simulator turns into the
//!   heuristic's positive lower bound;
//! * mixed-granularity partitioning: coarse cells for uniform open areas, with
//!   mean **and** maximum aggregates, and no aggregation where a direction
//!   constraint is present;
//! * fingerprints of every derived layer, computed from the source chunk CRCs.
//!
//! Chunks are encoded as layers are added and the input grids are dropped
//! immediately, except for the partition proxy and the direction layer whose
//! grids are needed by the partitioning pass. Peak memory therefore stays at
//! roughly "one input grid plus the compressed payloads".

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::Arc;

use crate::codec::{self, ChunkShape, CodecContext};
use crate::directory::ChunkRecord;
use crate::error::{MapError, Result};
use crate::fingerprint;
use crate::geometry::morton_encode_chunk;
use crate::graph::kpath::KPathLibrary;
use crate::graph::prm::PrmGraph;
use crate::graph::vector::VectorLayer;
use crate::layer::{DType, LayerDesc, LayerId, LayerKind};
use crate::quadtree::QNode;
use crate::raster::RasterChunk;
use crate::region::RegionSet;
use crate::tlv::value::*;
use crate::writer::{MapHeaderSpec, MapWriter};

/// Options of the mixed-granularity partitioning pass.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PartitionOptions {
    /// Largest coarse cell edge length in metres.
    pub max_coarse_m: f64,
    /// Smallest coarse cell edge length in metres. Below this the finest level
    /// describes the area and no skeleton node is emitted.
    pub min_coarse_m: f64,
    /// A region merges into one coarse cell while `max - mean` of the proxy
    /// channel stays below this value.
    pub uniformity_threshold: f32,
    /// Coarse cells whose maximum exceeds this value get a drill-down hint.
    pub drill_threshold: f32,
    /// Hard cap on the quadtree depth.
    pub max_depth: u8,
}

impl Default for PartitionOptions {
    fn default() -> Self {
        Self {
            max_coarse_m: 20.0,
            min_coarse_m: 5.0,
            uniformity_threshold: 0.15,
            drill_threshold: 0.5,
            max_depth: 12,
        }
    }
}

/// An encoded chunk or section waiting to be written.
#[derive(Debug)]
struct PendingChunk {
    layer_id: LayerId,
    level: u8,
    chunk_id: u32,
    codec: u8,
    shape: ChunkShape,
    stored: Vec<u8>,
    raw_len: u32,
    crc32: u32,
}

/// Assembles an OMF file from layer data.
pub struct MapBuilder {
    spec: MapHeaderSpec,
    schema: FeatureSchema,
    weight_prior: WeightPrior,
    slope_model: SlopeModel,
    magnetic_field: Option<MagneticField>,
    connectors: ConnectorTable,
    regions: Option<RegionSet>,
    vectors: Option<VectorLayer>,
    prm_batches: Vec<PrmGraph>,
    kpath: Option<KPathLibrary>,
    map_info: Option<MapInfo>,
    provenance: Option<Provenance>,
    chunk_layout: Option<ChunkLayout>,
    partition: PartitionOptions,
    partition_proxy: (LayerId, u8),
    lod_levels: u8,
    derived_params: Vec<(LayerId, u64, u16, Vec<u64>)>,

    layers: Vec<LayerDesc>,
    chunks: Vec<PendingChunk>,
    /// Partition proxy: grid width, height, channel count and the grid data,
    /// shared with `direction_grid` when one layer plays both roles.
    proxy_grid: Option<(u32, u32, u32, Arc<Vec<f32>>)>,
    direction_grid: Option<(u32, u32, Arc<Vec<f32>>)>,
}

impl MapBuilder {
    /// Creates a builder for a map with a given extent and feature schema.
    pub fn new(spec: MapHeaderSpec, schema: FeatureSchema) -> Self {
        Self {
            spec,
            schema,
            weight_prior: WeightPrior::default(),
            slope_model: SlopeModel::default(),
            magnetic_field: None,
            connectors: ConnectorTable::default(),
            regions: None,
            vectors: None,
            prm_batches: Vec::new(),
            kpath: None,
            map_info: None,
            provenance: None,
            chunk_layout: Some(ChunkLayout {
                layout: ChunkLayoutKind::ChannelContinuous,
                channel_order: Vec::new(),
            }),
            partition: PartitionOptions::default(),
            partition_proxy: (LayerId::HARD_FORBIDDEN, 0),
            lod_levels: 1,
            derived_params: Vec::new(),
            layers: Vec::new(),
            chunks: Vec::new(),
            proxy_grid: None,
            direction_grid: None,
        }
    }

    /// Replaces the partitioning options.
    pub fn with_partition_options(mut self, options: PartitionOptions) -> Self {
        self.partition = options;
        self
    }

    /// Chooses the channel whose aggregates populate the skeleton nodes.
    pub fn with_partition_proxy(mut self, layer_id: LayerId, channel: u8) -> Self {
        self.partition_proxy = (layer_id, channel);
        self
    }

    /// Sets how many LOD pyramid levels [`MapBuilder::generate_lod`] produces.
    pub fn with_lod_levels(mut self, levels: u8) -> Self {
        self.lod_levels = levels;
        self
    }

    /// Sets the map identity block.
    pub fn with_map_info(mut self, info: MapInfo) -> Self {
        self.map_info = Some(info);
        self
    }

    /// Sets the provenance block.
    pub fn with_provenance(mut self, provenance: Provenance) -> Self {
        self.provenance = Some(provenance);
        self
    }

    /// Sets the static weight priors.
    pub fn with_weight_prior(mut self, prior: WeightPrior) -> Self {
        self.weight_prior = prior;
        self
    }

    /// Sets the slope model parameters.
    pub fn with_slope_model(mut self, model: SlopeModel) -> Self {
        self.slope_model = model;
        self
    }

    /// Sets the local geomagnetic parameters.
    pub fn with_magnetic_field(mut self, field: MagneticField) -> Self {
        self.magnetic_field = Some(field);
        self
    }

    /// Sets the Z-axis connector table.
    pub fn with_connectors(mut self, connectors: ConnectorTable) -> Self {
        self.connectors = connectors;
        self
    }

    /// Sets the region annotation layer.
    pub fn with_regions(mut self, regions: RegionSet) -> Self {
        self.regions = Some(regions);
        self
    }

    /// Sets the vector layer.
    pub fn with_vectors(mut self, vectors: VectorLayer) -> Self {
        self.vectors = Some(vectors);
        self
    }

    /// Adds a PRM batch.
    pub fn with_prm_batch(mut self, graph: PrmGraph) -> Self {
        self.prm_batches.push(graph);
        self
    }

    /// Sets the K-shortest-path library.
    pub fn with_kpath_library(mut self, library: KPathLibrary) -> Self {
        self.kpath = Some(library);
        self
    }

    /// Declares the build parameters of a derived layer.
    ///
    /// Without this the builder still records a fingerprint with a zero
    /// parameter hash and the default algorithm version — enough to invalidate
    /// on source edits, not enough to distinguish two parameterisations.
    pub fn with_derived_params(
        mut self,
        layer_id: LayerId,
        build_params_hash: u64,
        algo_version: u16,
        seeds: Vec<u64>,
    ) -> Self {
        self.derived_params.retain(|(id, _, _, _)| *id != layer_id);
        self.derived_params
            .push((layer_id, build_params_hash, algo_version, seeds));
        self
    }

    /// Registers a raster layer and encodes its chunks.
    ///
    /// `data` is channel-continuous with `width * height * channels` samples and
    /// is consumed once the chunks are encoded.
    pub fn add_layer(
        &mut self,
        desc: LayerDesc,
        width: u32,
        height: u32,
        data: Vec<f32>,
    ) -> Result<&mut Self> {
        self.validate_grid(&desc, width, height, &data)?;
        self.register_layer(desc);

        let channels = desc.channels;
        let chunk_cells = self.spec.chunk_size as u32;
        if chunk_cells == 0 {
            return Err(MapError::invalid("chunk_size must be non-zero"));
        }
        let chunk_dim_x = width.div_ceil(chunk_cells);
        let chunk_dim_y = height.div_ceil(chunk_cells);
        // A chunk id is a 16-bit Morton code per axis, so a grid with more
        // chunks than that would silently alias a different chunk.
        if chunk_dim_x > u16::MAX as u32 + 1 || chunk_dim_y > u16::MAX as u32 + 1 {
            return Err(MapError::invalid(format!(
                "layer {} spans {chunk_dim_x} x {chunk_dim_y} chunk(s) per axis, more than the {} a 16-bit chunk index addresses",
                desc.layer_id,
                u16::MAX as u32 + 1
            )));
        }

        for cy in 0..chunk_dim_y {
            for cx in 0..chunk_dim_x {
                let mut chunk = RasterChunk::zeros(chunk_cells, chunk_cells, channels);
                for y in 0..chunk_cells {
                    let src_y = cy * chunk_cells + y;
                    if src_y >= height {
                        break;
                    }
                    for x in 0..chunk_cells {
                        let src_x = cx * chunk_cells + x;
                        if src_x >= width {
                            break;
                        }
                        let src = ((src_y as usize * width as usize) + src_x as usize)
                            * channels as usize;
                        let dst = chunk.index(x, y, 0);
                        chunk.data[dst..dst + channels as usize]
                            .copy_from_slice(&data[src..src + channels as usize]);
                    }
                }
                let payload = crate::raster::pack(&desc, &chunk)?;
                let shape = ChunkShape::new(chunk_cells, chunk_cells, channels, desc.dtype);
                self.push_encoded(
                    &desc,
                    0,
                    morton_encode_chunk(cx as u16, cy as u16),
                    shape,
                    &payload,
                )?;
            }
        }

        // Independent tests, not a chain: the same layer can be both the
        // partition proxy and the direction mask, and taking the first branch
        // would silently drop the mask — which is what forbids aggregating a
        // direction-constrained block at all.
        let wanted_proxy = desc.layer_id == self.partition_proxy.0;
        let wanted_direction = desc.layer_id == LayerId::DIRECTION;
        if wanted_proxy || wanted_direction {
            let shared = std::sync::Arc::new(data);
            if wanted_proxy {
                self.proxy_grid = Some((width, height, channels as u32, Arc::clone(&shared)));
            }
            if wanted_direction {
                self.direction_grid = Some((width, height, shared));
            }
        }
        Ok(self)
    }

    /// Registers a bit-packed constraint layer.
    pub fn add_bitmap_layer(
        &mut self,
        layer_id: LayerId,
        width: u32,
        height: u32,
        data: Vec<f32>,
    ) -> Result<&mut Self> {
        let desc = LayerDesc::new(layer_id, LayerKind::Bitmap, 1, DType::Bit, codec::id::RLE);
        self.add_layer(desc, width, height, data)
    }

    /// Registers a graph / region / vector section.
    pub fn add_section(&mut self, desc: LayerDesc, payload: &[u8]) -> Result<&mut Self> {
        self.register_layer(desc);
        let shape = ChunkShape::new(payload.len().max(1) as u32, 1, 1, DType::U8);
        self.push_encoded(&desc, 0, 0, shape, payload)?;
        Ok(self)
    }

    fn validate_grid(&self, desc: &LayerDesc, width: u32, height: u32, data: &[f32]) -> Result<()> {
        if width == 0 || height == 0 {
            return Err(MapError::invalid(format!(
                "layer {} has an empty grid",
                desc.layer_id
            )));
        }
        if desc.channels == 0 {
            return Err(MapError::invalid(format!(
                "layer {} declares zero channels",
                desc.layer_id
            )));
        }
        let expected = width as usize * height as usize * desc.channels as usize;
        if data.len() != expected {
            return Err(MapError::invalid(format!(
                "layer {} data has {} sample(s), expected {expected}",
                desc.layer_id,
                data.len()
            )));
        }
        if desc.dtype == DType::Bit && desc.channels != 1 {
            return Err(MapError::invalid(
                "bit layers must declare exactly one channel",
            ));
        }
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
        Ok(())
    }

    fn register_layer(&mut self, desc: LayerDesc) {
        match self.layers.iter_mut().find(|d| d.layer_id == desc.layer_id) {
            Some(slot) => *slot = desc,
            None => self.layers.push(desc),
        }
    }

    fn push_encoded(
        &mut self,
        desc: &LayerDesc,
        level: u8,
        chunk_id: u32,
        shape: ChunkShape,
        payload: &[u8],
    ) -> Result<()> {
        let stored = codec::encode(desc.codec, &shape, payload, &CodecContext::none())?;
        self.chunks.push(PendingChunk {
            layer_id: desc.layer_id,
            level,
            chunk_id,
            codec: desc.codec,
            shape,
            crc32: crc32fast::hash(&stored),
            raw_len: payload.len() as u32,
            stored,
        });
        Ok(())
    }

    /// Generates box-averaged LOD levels for a raster layer.
    ///
    /// Intended for smooth fields (elevation, distance transform) whose coarse
    /// levels are meaningful. Categorical, directional and bit-packed channels
    /// must not use it: averaging them is not defined.
    pub fn generate_lod(&mut self, layer_id: LayerId, levels: u8) -> Result<&mut Self> {
        let desc = *self.layers.iter().find(|d| d.layer_id == layer_id).ok_or(
            MapError::LayerNotFound {
                layer_id: layer_id.raw(),
            },
        )?;
        if levels == 0 || desc.dtype == DType::Bit {
            return Ok(self);
        }
        let channels = desc.channels as usize;

        for level in 1..=levels {
            let sources: Vec<(u8, u32, ChunkShape, Vec<u8>)> = self
                .chunks
                .iter()
                .filter(|c| c.layer_id == layer_id && c.level == level - 1)
                .map(|c| (c.codec, c.chunk_id, c.shape, c.stored.clone()))
                .collect();
            if sources.is_empty() {
                break;
            }

            // A level-`L` chunk covers four level-`L-1` chunks and stores the same
            // number of cells, so a child contributes the quadrant its parity
            // selects and its four cells per parent cell are averaged.
            let mut coarser: HashMap<u32, RasterChunk> = HashMap::new();
            for (source_codec, chunk_id, shape, stored) in &sources {
                let payload = codec::decode(*source_codec, shape, stored, &CodecContext::none())?;
                let chunk = crate::raster::unpack(&desc, shape, &payload)?;
                let (ix, iy) = crate::geometry::morton_decode_chunk(*chunk_id);
                let parent_id = morton_encode_chunk(ix / 2, iy / 2);
                let entry = coarser.entry(parent_id).or_insert_with(|| {
                    RasterChunk::zeros(shape.width, shape.height, desc.channels)
                });
                let (half_w, half_h) = ((shape.width / 2).max(1), (shape.height / 2).max(1));
                let base_x = u32::from(ix % 2) * half_w;
                let base_y = u32::from(iy % 2) * half_h;
                for y in 0..half_h {
                    for x in 0..half_w {
                        for channel in 0..channels {
                            let value = (chunk.get(x * 2, y * 2, channel as u8)
                                + chunk.get(x * 2 + 1, y * 2, channel as u8)
                                + chunk.get(x * 2, y * 2 + 1, channel as u8)
                                + chunk.get(x * 2 + 1, y * 2 + 1, channel as u8))
                                * 0.25;
                            entry.set(base_x + x, base_y + y, channel as u8, value);
                        }
                    }
                }
            }

            let mut ids: Vec<u32> = coarser.keys().copied().collect();
            ids.sort_unstable();
            for id in ids {
                let chunk = &coarser[&id];
                let payload = crate::raster::pack(&desc, chunk)?;
                let shape = ChunkShape::new(chunk.width, chunk.height, desc.channels, desc.dtype);
                self.push_encoded(&desc, level, id, shape, &payload)?;
            }
        }
        Ok(self)
    }

    /// Builds the file at `path`.
    pub fn build(mut self, path: impl AsRef<Path>) -> Result<()> {
        let path = path.as_ref();
        if self.lod_levels > 0 {
            let smooth: Vec<LayerId> = self
                .layers
                .iter()
                .filter(|d| matches!(d.layer_id, LayerId::ELEVATION | LayerId::EDT))
                .map(|d| d.layer_id)
                .collect();
            for layer_id in smooth {
                let levels = self.lod_levels;
                self.generate_lod(layer_id, levels)?;
            }
        }
        let file = std::fs::File::create(path)?;
        self.build_into(std::io::BufWriter::new(file))
            .map(|_| ())
            .map_err(|e| e.with_path(path))
    }

    /// Builds the file into memory.
    pub fn build_to_bytes(mut self) -> Result<Vec<u8>> {
        if self.lod_levels > 0 {
            let smooth: Vec<LayerId> = self
                .layers
                .iter()
                .filter(|d| matches!(d.layer_id, LayerId::ELEVATION | LayerId::EDT))
                .map(|d| d.layer_id)
                .collect();
            for layer_id in smooth {
                let levels = self.lod_levels;
                self.generate_lod(layer_id, levels)?;
            }
        }
        let sink = std::io::Cursor::new(Vec::new());
        let cursor = self.build_into(sink)?;
        Ok(cursor.into_inner())
    }

    /// Encodes the graph, region and vector sections into pending chunks.
    ///
    /// Running this before [`MapBuilder::chunk_records`] is what makes the
    /// fingerprints of derived layers cover the sections they depend on: the
    /// reader hashes the CRC of every stored chunk, so the builder must hash the
    /// very same records.
    fn prepare_sections(&mut self) -> Result<()> {
        if let Some(regions) = &self.regions {
            let desc = self
                .layer_desc_for(LayerId::REGIONS)
                .ok_or(MapError::LayerNotFound {
                    layer_id: LayerId::REGIONS.raw(),
                })?;
            let payload = regions.encode();
            let shape = ChunkShape::new(payload.len().max(1) as u32, 1, 1, DType::U8);
            self.push_encoded(&desc, 0, 0, shape, &payload)?;
        }
        if let Some(vectors) = &self.vectors {
            let desc = self
                .layer_desc_for(LayerId::VECTORS)
                .ok_or(MapError::LayerNotFound {
                    layer_id: LayerId::VECTORS.raw(),
                })?;
            let payload = vectors.encode();
            let shape = ChunkShape::new(payload.len().max(1) as u32, 1, 1, DType::U8);
            self.push_encoded(&desc, 0, 0, shape, &payload)?;
        }
        let prm_chunks: Vec<(u32, Vec<u8>)> = self
            .prm_batches
            .iter()
            .map(|graph| (graph.batch as u32, graph.encode()))
            .collect();
        for (batch, payload) in prm_chunks {
            let desc = self
                .layer_desc_for(LayerId::PRM_GRAPH)
                .ok_or(MapError::LayerNotFound {
                    layer_id: LayerId::PRM_GRAPH.raw(),
                })?;
            let shape = ChunkShape::new(payload.len().max(1) as u32, 1, 1, DType::U8);
            self.push_encoded(&desc, 0, batch, shape, &payload)?;
        }
        if let Some(kpath) = &self.kpath {
            let desc =
                self.layer_desc_for(LayerId::KPATH_LIBRARY)
                    .ok_or(MapError::LayerNotFound {
                        layer_id: LayerId::KPATH_LIBRARY.raw(),
                    })?;
            let payload = kpath.encode();
            let shape = ChunkShape::new(payload.len().max(1) as u32, 1, 1, DType::U8);
            self.push_encoded(&desc, 0, 0, shape, &payload)?;
        }
        if !self.prm_batches.is_empty() {
            self.register_layer(LayerDesc::new(
                LayerId::PRM_GRAPH,
                LayerKind::Graph,
                1,
                DType::U8,
                codec::id::ZSTD,
            ));
        }
        if self.kpath.is_some() {
            self.register_layer(LayerDesc::new(
                LayerId::KPATH_LIBRARY,
                LayerKind::Graph,
                1,
                DType::U8,
                codec::id::ZSTD,
            ));
        }
        Ok(())
    }

    fn build_into<W: std::io::Write + std::io::Seek + Send>(mut self, sink: W) -> Result<W> {
        self.prepare_sections()?;
        let records = self.chunk_records();
        let stats = self.global_stats();
        let skeleton = self.partition_skeleton()?;
        let derived = self.derived_layers(&records);

        let mut writer = MapWriter::new(sink, self.spec)?;
        if let Some(info) = &self.map_info {
            writer.set_map_info(info)?;
        }
        writer.set_feature_schema(&self.schema)?;
        writer.set_weight_prior(&self.weight_prior)?;
        writer.set_slope_model(&self.slope_model)?;
        if let Some(field) = &self.magnetic_field {
            writer.set_magnetic_field(field)?;
        }
        if !self.connectors.connectors.is_empty() {
            writer.set_connectors(&self.connectors)?;
        }
        if let Some(provenance) = &self.provenance {
            writer.set_provenance(provenance)?;
        }
        if let Some(layout) = &self.chunk_layout {
            writer.set_chunk_layout(layout)?;
        }
        writer.set_global_stats(&stats)?;
        // The skeleton aggregates the channel selected by `with_partition_proxy`,
        // so the file must describe that channel rather than the default one.
        writer.set_aggregation_rules(&AggregationRules {
            proxy_layer: self.partition_proxy.0,
            proxy_channel: self.partition_proxy.1,
            ..AggregationRules::default()
        })?;
        if !derived.entries.is_empty() {
            writer.set_derived_layers(&derived)?;
        }
        if !self.prm_batches.is_empty() {
            writer.set_prm_seeds(&PrmSeeds {
                seeds: self.prm_batches.iter().map(|b| b.seed).collect(),
            })?;
        }
        for desc in &self.layers {
            writer.set_layer(*desc)?;
        }
        for layer_id in self.present_layers() {
            if self.layers.iter().all(|d| d.layer_id != layer_id)
                && let Some(desc) = self.layer_desc_for(layer_id)
            {
                writer.set_layer(desc)?;
            }
        }
        writer.set_skeleton(skeleton)?;

        for chunk in &self.chunks {
            writer.write_encoded_chunk(
                chunk.layer_id,
                chunk.level,
                chunk.chunk_id,
                chunk.codec,
                &chunk.stored,
                chunk.raw_len,
            )?;
        }

        writer.finish()
    }

    /// Chunk records exactly as they will appear in the directory.
    fn chunk_records(&self) -> Vec<ChunkRecord> {
        self.chunks
            .iter()
            .map(|chunk| {
                ChunkRecord::new(
                    chunk.layer_id,
                    chunk.level,
                    chunk.chunk_id,
                    chunk.codec,
                    0,
                    chunk.stored.len() as u32,
                    chunk.raw_len,
                    chunk.crc32,
                )
            })
            .collect()
    }

    /// Descriptor of a layer, synthesising the graph-layer descriptors the
    /// writer registers so fingerprints cover exactly the bytes on disk.
    fn layer_desc_for(&self, layer_id: LayerId) -> Option<LayerDesc> {
        if let Some(desc) = self.layers.iter().find(|d| d.layer_id == layer_id) {
            return Some(*desc);
        }
        let kind = match layer_id {
            LayerId::PRM_GRAPH | LayerId::KPATH_LIBRARY | LayerId::REGION_INTERFACE => {
                LayerKind::Graph
            }
            LayerId::REGIONS => LayerKind::RegionPolygons,
            LayerId::VECTORS => LayerKind::Vector,
            _ => return None,
        };
        Some(LayerDesc::new(
            layer_id,
            kind,
            1,
            DType::U8,
            codec::id::ZSTD,
        ))
    }

    /// Layers that carry data in this build, including graph sections that are
    /// not part of the raster registration list.
    fn present_layers(&self) -> Vec<LayerId> {
        let mut out: Vec<LayerId> = self.layers.iter().map(|d| d.layer_id).collect();
        let add = |id: LayerId, present: bool, out: &mut Vec<LayerId>| {
            if present && !out.contains(&id) {
                out.push(id);
            }
        };
        add(LayerId::PRM_GRAPH, !self.prm_batches.is_empty(), &mut out);
        add(LayerId::KPATH_LIBRARY, self.kpath.is_some(), &mut out);
        add(LayerId::REGIONS, self.regions.is_some(), &mut out);
        add(LayerId::VECTORS, self.vectors.is_some(), &mut out);
        out
    }

    fn layer_fingerprint(&self, layer_id: LayerId, records: &[ChunkRecord]) -> Option<u64> {
        let desc = self.layer_desc_for(layer_id)?;
        // The writer's directory keeps only the last record for a repeated
        // `(layer, level, chunk_id)` key, so the fingerprint must hash the same
        // subset: a chunk key written twice is represented by its last record.
        let mut seen: HashSet<(u8, u32)> = HashSet::new();
        let subset: Vec<ChunkRecord> = records
            .iter()
            .rev()
            .filter(|r| r.layer_id == layer_id)
            .filter(|r| seen.insert((r.level, r.chunk_id)))
            .copied()
            .collect();
        Some(fingerprint::layer_fingerprint(&desc, &subset))
    }

    fn derived_layers(&self, records: &[ChunkRecord]) -> DerivedLayers {
        let mut out = DerivedLayers::default();
        for layer_id in self.present_layers() {
            let Some(desc) = self.layer_desc_for(layer_id) else {
                continue;
            };
            if !desc.layer_id.is_derived() {
                continue;
            }
            // Only layers that are actually in the file contribute, which is what
            // the reader does — it skips a source with no descriptor. Taking a
            // fingerprint of an absent layer here would fold a synthetic empty-layer
            // hash into the entry, and the layer would read back as stale forever.
            let present = self.present_layers();
            let sources = self.source_layers_of(desc.layer_id);
            let fingerprints: Vec<u64> = sources
                .iter()
                .filter(|id| present.contains(id))
                .filter_map(|id| self.layer_fingerprint(*id, records))
                .collect();
            // A derived layer with no present source (including the id ranges
            // that have no defined source set, such as `REGION_INTERFACE` and
            // cache layers other than `COST_CACHE`) still gets an entry: the
            // reader verifies an empty source list against it, whereas skipping
            // the entry would leave the layer permanently stale.
            let (params_hash, algo_version, seeds) = self
                .derived_params
                .iter()
                .find(|(id, _, _, _)| *id == desc.layer_id)
                .map(|(_, hash, version, seeds)| (*hash, *version, seeds.clone()))
                .unwrap_or((0, default_algo_version(desc.layer_id), Vec::new()));
            out.upsert(fingerprint::make_entry(
                desc.layer_id,
                &fingerprints,
                params_hash,
                algo_version,
                seeds,
            ));
        }
        out
    }

    /// Source layers of a derived layer; mirrors [`crate::reader::Map`].
    ///
    /// The two lists must agree exactly: a layer recorded here but not derived
    /// from the same set by the reader (or the reverse) makes every rebuild look
    /// stale on load. Ids matched by neither list have no source set on either
    /// side, so their entry is written with an empty fingerprint list and the
    /// reader verifies it as valid.
    fn source_layers_of(&self, layer_id: LayerId) -> Vec<LayerId> {
        let features = || {
            self.present_layers()
                .into_iter()
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
            LayerId::KPATH_LIBRARY => vec![LayerId::PRM_GRAPH, LayerId::HARD_FORBIDDEN],
            LayerId::COST_CACHE => {
                let mut sources = features();
                sources.push(LayerId::HARD_FORBIDDEN);
                sources
            }
            _ => Vec::new(),
        }
    }

    fn global_stats(&self) -> GlobalStats {
        let mut channels: Vec<ChannelStats> = Vec::new();
        for desc in &self.layers {
            if !desc.kind.is_chunked() {
                continue;
            }
            let channels_count = desc.channels as usize;
            let mut min = vec![f32::INFINITY; channels_count];
            let mut max = vec![f32::NEG_INFINITY; channels_count];
            let mut sum = vec![0.0f64; channels_count];
            let mut present = vec![0u64; channels_count];
            for chunk in self.chunks.iter().filter(|c| c.layer_id == desc.layer_id) {
                let Ok(payload) = codec::decode(
                    chunk.codec,
                    &chunk.shape,
                    &chunk.stored,
                    &CodecContext::none(),
                ) else {
                    continue;
                };
                let Ok(decoded) = crate::raster::unpack(desc, &chunk.shape, &payload) else {
                    continue;
                };
                for (index, value) in decoded.data.iter().enumerate() {
                    let channel = index % channels_count;
                    min[channel] = min[channel].min(*value);
                    max[channel] = max[channel].max(*value);
                    sum[channel] += *value as f64;
                    present[channel] += 1;
                }
            }
            for channel in 0..channels_count {
                if present[channel] == 0 {
                    continue;
                }
                channels.push(ChannelStats {
                    layer_id: desc.layer_id,
                    channel: channel as u8,
                    min: min[channel],
                    max: max[channel],
                    mean: (sum[channel] / present[channel] as f64) as f32,
                    coverage: 1.0,
                });
            }
        }

        // Fraction of forbidden cells over every stored chunk of the layer, not
        // the first one: the statistic describes the map, and a multi-chunk map
        // would otherwise ship a number describing one block. The margin cells a
        // chunk carries beyond the map edge are excluded — they are padding, and
        // counting them would understate the ratio on a map whose width is not a
        // multiple of the chunk size.
        let forbidden_ratio = match self
            .layers
            .iter()
            .find(|d| d.layer_id == LayerId::HARD_FORBIDDEN)
        {
            Some(desc) => {
                let mut blocked = 0u64;
                let mut total = 0u64;
                for chunk in self
                    .chunks
                    .iter()
                    .filter(|c| c.layer_id == LayerId::HARD_FORBIDDEN && c.level == 0)
                {
                    let chunk_id = chunk.chunk_id;
                    let Ok(payload) = codec::decode(
                        chunk.codec,
                        &chunk.shape,
                        &chunk.stored,
                        &CodecContext::none(),
                    ) else {
                        continue;
                    };
                    let Ok(decoded) = crate::raster::unpack(desc, &chunk.shape, &payload) else {
                        continue;
                    };
                    // Edge chunks are padded out to the chunk size, and the
                    // padding is always zero. Counting it would understate the
                    // ratio on a map whose extent is not a whole number of
                    // chunks, so the in-map part of each row is what counts.
                    let (in_map_w, in_map_h) = self.in_map_cells(chunk_id);
                    let row = chunk.shape.width as usize;
                    let rows = (chunk.shape.height as usize).min(in_map_h);
                    for y in 0..rows {
                        let from = y * row;
                        let to = (from + in_map_w.min(row)).min(decoded.data.len());
                        if from >= to {
                            break;
                        }
                        blocked +=
                            decoded.data[from..to].iter().filter(|v| **v != 0.0).count() as u64;
                        total += (to - from) as u64;
                    }
                }
                if total == 0 {
                    Vec::new()
                } else {
                    vec![(LayerId::HARD_FORBIDDEN, blocked as f32 / total as f32)]
                }
            }
            None => Vec::new(),
        };

        GlobalStats {
            channels,
            forbidden_ratio,
            connector_unit_cost_min: self.connectors.min_unit_cost(),
        }
    }

    /// Width and height of the in-map part of a level-0 chunk, in cells.
    ///
    /// The grid is `ceil(extent / base resolution)` cells, so only the chunks on
    /// the far edges of the map carry padding.
    fn in_map_cells(&self, chunk_id: u32) -> (usize, usize) {
        let res = (self.spec.base_res_cm as f64 / 100.0).max(1e-6);
        let chunk = self.spec.chunk_size.max(1) as usize;
        let map_w = ((self.spec.bounds.max_x - self.spec.bounds.min_x) / res).ceil() as i64;
        let map_h = ((self.spec.bounds.max_y - self.spec.bounds.min_y) / res).ceil() as i64;
        let (ix, iy) = crate::geometry::morton_decode_chunk(chunk_id);
        let width = (map_w - (ix as i64) * chunk as i64).clamp(0, chunk as i64) as usize;
        let height = (map_h - (iy as i64) * chunk as i64).clamp(0, chunk as i64) as usize;
        (width, height)
    }

    /// Builds the quadtree skeleton from the partition proxy channel.
    fn partition_skeleton(&self) -> Result<Vec<QNode>> {
        let Some((width, height, channels, data)) = &self.proxy_grid else {
            return Ok(Vec::new());
        };
        if *width == 0 || *height == 0 {
            return Ok(Vec::new());
        }
        // A multi-channel proxy stores its channels interleaved, so the selected
        // channel has to be part of the addressing. Reading it as a single-channel
        // grid would aggregate a mix of every channel while the file declares the
        // one that was asked for.
        if usize::from(self.partition_proxy.1) >= *channels as usize {
            return Err(MapError::invalid(format!(
                "partition proxy channel {} does not exist in a {channels}-channel layer",
                self.partition_proxy.1
            )));
        }
        let side = (*width).max(*height).next_power_of_two();
        let options = self.partition;
        let base_res = self.spec.base_res_cm as f64 / 100.0;

        let proxy = ProxyChannel {
            data,
            width: *width,
            height: *height,
            channels: *channels,
            channel: usize::from(self.partition_proxy.1),
        };
        let direction = self
            .direction_grid
            .as_ref()
            .map(|(w, h, values)| DirectionMask {
                width: *w,
                height: *h,
                values,
            });

        let mut nodes = Vec::new();
        partition_recursive(
            &proxy,
            direction.as_ref(),
            0,
            0,
            side,
            0,
            &options,
            base_res,
            &mut nodes,
        );
        nodes.sort_unstable_by_key(|n| n.morton);
        nodes.dedup_by_key(|n| n.morton);
        Ok(nodes)
    }
}

fn default_algo_version(layer_id: LayerId) -> u16 {
    match layer_id {
        LayerId::SLOPE => fingerprint::algo::SLOPE,
        LayerId::EDT => fingerprint::algo::EDT,
        LayerId::PRM_GRAPH => fingerprint::algo::PRM,
        LayerId::KPATH_LIBRARY => fingerprint::algo::KPATH,
        LayerId::COST_CACHE => fingerprint::algo::COST_CACHE,
        _ => 0,
    }
}

/// Dense proxy channel used for partitioning.
struct ProxyChannel<'a> {
    data: &'a [f32],
    width: u32,
    height: u32,
    /// Channels per cell of the stored grid.
    channels: u32,
    /// Channel the caller selected as the proxy.
    channel: usize,
}

impl ProxyChannel<'_> {
    fn value(&self, x: u32, y: u32) -> Option<f32> {
        if x >= self.width || y >= self.height {
            return None;
        }
        let cell = (y as usize * self.width as usize + x as usize) * self.channels as usize;
        self.data.get(cell + self.channel).copied()
    }
}

/// Direction-constraint mask: any non-zero strength forbids aggregation.
struct DirectionMask<'a> {
    width: u32,
    height: u32,
    values: &'a [f32],
}

impl DirectionMask<'_> {
    fn blocked(&self, x: u32, y: u32) -> bool {
        if x >= self.width || y >= self.height {
            return false;
        }
        self.values
            .get(y as usize * self.width as usize + x as usize)
            .map(|v| *v != 0.0)
            .unwrap_or(false)
    }
}

/// Recursively subdivides the square until regions are uniform enough to merge.
///
/// A node at depth `d` covers `side / 2^d` cells, so its linear-quadtree key is
/// the interleaving of `(x0, y0)` divided by that cell size.
#[allow(clippy::too_many_arguments)]
fn partition_recursive(
    proxy: &ProxyChannel<'_>,
    direction: Option<&DirectionMask<'_>>,
    x0: u32,
    y0: u32,
    size: u32,
    depth: u8,
    options: &PartitionOptions,
    base_res_m: f64,
    out: &mut Vec<QNode>,
) {
    if size == 0 || options.max_coarse_m <= 0.0 {
        return;
    }
    let span_m = size as f64 * base_res_m;
    let mut min = f32::INFINITY;
    let mut max = f32::NEG_INFINITY;
    let mut sum = 0.0f64;
    let mut count = 0u32;
    let mut blocked = false;
    for y in y0..(y0 + size).min(proxy.height) {
        for x in x0..(x0 + size).min(proxy.width) {
            if let Some(value) = proxy.value(x, y) {
                min = min.min(value);
                max = max.max(value);
                sum += value as f64;
                count += 1;
            }
            if direction.map(|d| d.blocked(x, y)).unwrap_or(false) {
                blocked = true;
            }
        }
    }
    if count == 0 {
        return;
    }
    let mean = (sum / count as f64) as f32;

    if span_m < options.min_coarse_m {
        // Small enough that the finest level describes this area.
        return;
    }

    let uniform = (max - mean) <= options.uniformity_threshold;
    if uniform && span_m <= options.max_coarse_m && !blocked {
        let cells = 1u32 << depth;
        let ix = x0 / size.max(1);
        let iy = y0 / size.max(1);
        if ix < cells && iy < cells {
            let morton = crate::geometry::node_key(ix, iy, depth);
            let mut flags = QNode::LEAF | QNode::HAS_AGGR_MAX;
            if max > options.drill_threshold {
                flags |= QNode::DRILL_HINT;
            }
            let rules = AggregationRules::default();
            out.push(QNode::new(
                morton,
                flags,
                0,
                crate::quadtree::QuadtreeSkeleton::quantise(mean, &rules),
                crate::quadtree::QuadtreeSkeleton::quantise(max, &rules),
            ));
        }
        return;
    }

    if depth >= options.max_depth || size < 2 {
        return;
    }
    let half = size / 2;
    if half == 0 {
        return;
    }
    for (dx, dy) in [(0, 0), (half, 0), (0, half), (half, half)] {
        partition_recursive(
            proxy,
            direction,
            x0 + dx,
            y0 + dy,
            half,
            depth + 1,
            options,
            base_res_m,
            out,
        );
    }
}
