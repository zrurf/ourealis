//! gRPC facade: the same resources as HTTP, one protobuf encoding.
//!
//! Every method resolves its resource through the builders of [`crate::api`] and
//! only the encoding happens here, which is what keeps the two facades from
//! disagreeing about a map or a job. Structures the web client only inspects —
//! the metadata tree, the evaluation report, the optional map sections — travel
//! as JSON strings in the messages that declare them that way, so there is still
//! exactly one definition of each resource, in [`crate::api::dto`].
//!
//! Limits come from the configuration: `rpc.max_message_bytes` bounds one message
//! in each direction and `rpc.max_concurrent_streams` bounds the streams one
//! connection may open.

use std::sync::Arc;

use tokio::sync::mpsc;
use tokio_stream::wrappers::{ReceiverStream, TcpListenerStream};
use tonic::{Request, Response, Status};

use crate::api::dto::result::{EventDto, SensorSampleDto, TruthSampleDto};
use crate::api::dto::simulation::SimulationStateDto;
use crate::api::{maps, omf, routes, simulations, streams, system};
use crate::app::AppState;
use crate::error::{Result, ServiceError};
use crate::facade;
use crate::job::events::EventEnvelope;
use crate::proto::v1 as pb;

/// Frames queued for one server stream before the producer waits.
const STREAM_QUEUE: usize = 8;

/// Implements `ourealis.api.v1.SystemService`.
struct SystemApi {
    state: Arc<AppState>,
}

/// Implements `ourealis.api.v1.MapService`.
struct MapApi {
    state: Arc<AppState>,
}

/// Implements `ourealis.api.v1.SimulationService`.
struct SimulationApi {
    state: Arc<AppState>,
}

/// Serves the gRPC facade until the shutdown channel reports `true`.
pub async fn spawn(
    state: Arc<AppState>,
    shutdown: tokio::sync::watch::Receiver<bool>,
) -> Result<(tokio::task::JoinHandle<Result<()>>, std::net::SocketAddr)> {
    let (listener, address) = facade::bind(&state.config.server.rpc_listen).await?;
    let max_message = state.config.rpc.max_message_bytes.max(1024);
    let max_streams = state.config.rpc.max_concurrent_streams;
    let system = pb::system_service_server::SystemServiceServer::new(SystemApi {
        state: Arc::clone(&state),
    })
    .max_decoding_message_size(max_message)
    .max_encoding_message_size(max_message);
    let maps = pb::map_service_server::MapServiceServer::new(MapApi {
        state: Arc::clone(&state),
    })
    .max_decoding_message_size(max_message)
    .max_encoding_message_size(max_message);
    let jobs = pb::simulation_service_server::SimulationServiceServer::new(SimulationApi {
        state: Arc::clone(&state),
    })
    .max_decoding_message_size(max_message)
    .max_encoding_message_size(max_message);

    let mut shutdown = shutdown;
    let handle = tokio::spawn(async move {
        tonic::transport::Server::builder()
            .max_concurrent_streams(max_streams)
            .add_service(system)
            .add_service(maps)
            .add_service(jobs)
            .serve_with_incoming_shutdown(TcpListenerStream::new(listener), async move {
                facade::wait_for_shutdown(&mut shutdown).await
            })
            .await
            .map_err(|error| {
                ServiceError::Internal(format!("the gRPC facade stopped serving: {error}"))
            })
    });
    Ok((handle, address))
}

#[tonic::async_trait]
impl pb::system_service_server::SystemService for SystemApi {
    async fn info(
        &self,
        _request: Request<pb::Empty>,
    ) -> std::result::Result<Response<pb::SystemInfo>, Status> {
        let info = system::info_dto(&self.state);
        Ok(Response::new(pb::SystemInfo {
            version: info.version,
            api_version: info.api_version,
            core_version: info.core_version,
            map_format_version: info.map_format_version,
            build_time: info.build_time,
            rpc_enabled: info.rpc_enabled,
            http_enabled: info.http_enabled,
            web_enabled: info.web_enabled,
            backend: info.backend,
            worker_threads: info.worker_threads,
            presets: info.presets,
            web_assets_built: info.web_assets_built,
        }))
    }

    async fn health(
        &self,
        _request: Request<pb::Empty>,
    ) -> std::result::Result<Response<pb::HealthReply>, Status> {
        Ok(Response::new(pb::HealthReply {
            status: "ok".to_string(),
        }))
    }
}

#[tonic::async_trait]
impl pb::map_service_server::MapService for MapApi {
    async fn list(
        &self,
        request: Request<pb::ListMapsRequest>,
    ) -> std::result::Result<Response<pb::ListMapsReply>, Status> {
        let request = request.into_inner();
        let (offset, limit) = self.state.page(crate::api::dto::PageQuery {
            offset: request.offset as usize,
            limit: request.limit as usize,
        });
        let page = maps::entry_page(&self.state, offset, limit);
        let total = page.total as u32;
        let items = page
            .items
            .iter()
            .map(|summary| summary_to_pb(summary, self.created_at_ms(summary)))
            .collect();
        Ok(Response::new(pb::ListMapsReply { items, total }))
    }

    async fn get(
        &self,
        request: Request<pb::GetMapRequest>,
    ) -> std::result::Result<Response<pb::MapMetadata>, Status> {
        let id = request.into_inner().id;
        let (entry, map) = maps::open_map(&self.state, &id).map_err(status)?;
        let metadata = maps::metadata_dto(&entry, &map).map_err(status)?;
        let sections = metadata.sections;
        let has_prm = map
            .layer_desc(ourealis_map_format::LayerId::PRM_GRAPH)
            .is_some();
        let has_kpath = map
            .layer_desc(ourealis_map_format::LayerId::KPATH_LIBRARY)
            .is_some();
        Ok(Response::new(pb::MapMetadata {
            summary: Some(summary_to_pb(&metadata.summary, entry.created_at_ms)),
            header_json: json_string(&metadata.header)?,
            map_info_json: optional_json_string(metadata.map_info.as_ref())?,
            feature_schema_json: optional_json_string(metadata.feature_schema.as_ref())?,
            layers_json: json_string(&metadata.layers)?,
            global_stats_json: optional_json_string(metadata.global_stats.as_ref())?,
            connector_count: sections.connectors.unwrap_or(0) as u32,
            region_count: sections.regions.unwrap_or(0) as u32,
            vector_count: sections.vectors.unwrap_or(0) as u32,
            skeleton_node_count: metadata.skeleton_nodes as u32,
            has_prm,
            has_kpath_library: has_kpath,
            has_magnetic_field: metadata.magnetic_field.is_some(),
        }))
    }

    async fn import(
        &self,
        request: Request<tonic::Streaming<pb::ImportChunk>>,
    ) -> std::result::Result<Response<pb::MapSummary>, Status> {
        let mut stream = request.into_inner();
        let mut bytes = Vec::new();
        let mut name = String::new();
        let limit = self.state.max_body_bytes();
        while let Some(chunk) = stream.message().await? {
            if name.is_empty() {
                name = chunk.name;
            }
            if bytes.len().saturating_add(chunk.data.len()) > limit {
                return Err(status(ServiceError::TooLarge(format!(
                    "the uploaded image exceeds the configured {limit}-byte limit"
                ))));
            }
            bytes.extend_from_slice(&chunk.data);
        }
        let given = (!name.trim().is_empty()).then_some(name.as_str());
        let summary = maps::add_image(&self.state, &bytes, "import", given).map_err(status)?;
        Ok(Response::new(summary_to_pb(
            &summary,
            self.created_at_ms(&summary),
        )))
    }

    async fn delete(
        &self,
        request: Request<pb::DeleteMapRequest>,
    ) -> std::result::Result<Response<pb::Empty>, Status> {
        let id = request.into_inner().id;
        self.state.maps.remove(&id).map_err(status)?;
        Ok(Response::new(pb::Empty {}))
    }

    async fn chunk(
        &self,
        request: Request<pb::ChunkRequest>,
    ) -> std::result::Result<Response<pb::ChunkReply>, Status> {
        let request = request.into_inner();
        let (_, map) = maps::open_map(&self.state, &request.map_id).map_err(status)?;
        let chunk = maps::chunk_dto(
            &map,
            &request.map_id,
            layer_id(request.layer_id)?,
            level(request.level)?,
            request.chunk_id,
        )
        .map_err(status)?;
        Ok(Response::new(pb::ChunkReply {
            width: chunk.width,
            height: chunk.height,
            channels: chunk.channels as u32,
            dtype: chunk.dtype,
            scale: chunk.scale,
            bias: chunk.bias,
            data: chunk.data,
        }))
    }

    async fn layer_grid(
        &self,
        request: Request<pb::ChunkRequest>,
    ) -> std::result::Result<Response<pb::LayerGridReply>, Status> {
        let request = request.into_inner();
        let (_, map) = maps::open_map(&self.state, &request.map_id).map_err(status)?;
        let grid =
            maps::grid_dto(&map, &request.map_id, layer_id(request.layer_id)?).map_err(status)?;
        let dims: Vec<u32> = grid
            .level_dims
            .iter()
            .flat_map(|dims| [dims[0], dims[1]])
            .collect();
        // The chunk index is keyed by level, which JSON expresses directly; a
        // repeated field would have to interleave the levels to say the same.
        let mut levels = serde_json::Map::new();
        for (level, chunks) in grid.chunks.iter().enumerate() {
            levels.insert(
                level.to_string(),
                serde_json::to_value(chunks).map_err(|error| status(ServiceError::Json(error)))?,
            );
        }
        Ok(Response::new(pb::LayerGridReply {
            layer_id: grid.layer_id as u32,
            level_res_m: grid.level_res_m,
            level_dims: dims,
            chunk_dim_x: grid.chunk_dim[0],
            chunk_dim_y: grid.chunk_dim[1],
            chunks_json: serde_json::Value::Object(levels).to_string(),
        }))
    }

    async fn skeleton(
        &self,
        request: Request<pb::GetMapRequest>,
    ) -> std::result::Result<Response<pb::SkeletonReply>, Status> {
        let id = request.into_inner().id;
        let (_, map) = maps::open_map(&self.state, &id).map_err(status)?;
        Ok(Response::new(pb::SkeletonReply {
            nodes_json: json_string(&maps::skeleton_dto(&map))?,
        }))
    }

    async fn regions(
        &self,
        request: Request<pb::GetMapRequest>,
    ) -> std::result::Result<Response<pb::SectionReply>, Status> {
        let (_, map) = self.open(&request.into_inner().id)?;
        Ok(Response::new(pb::SectionReply {
            json: json_string(&maps::regions_section(&map).map_err(status)?)?,
        }))
    }

    async fn connectors(
        &self,
        request: Request<pb::GetMapRequest>,
    ) -> std::result::Result<Response<pb::SectionReply>, Status> {
        let (_, map) = self.open(&request.into_inner().id)?;
        Ok(Response::new(pb::SectionReply {
            json: json_string(&maps::connectors_section(&map).map_err(status)?)?,
        }))
    }

    async fn vectors(
        &self,
        request: Request<pb::GetMapRequest>,
    ) -> std::result::Result<Response<pb::SectionReply>, Status> {
        let (_, map) = self.open(&request.into_inner().id)?;
        Ok(Response::new(pb::SectionReply {
            json: json_string(&maps::vectors_section(&map).map_err(status)?)?,
        }))
    }

    async fn roadmap(
        &self,
        request: Request<pb::GetMapRequest>,
    ) -> std::result::Result<Response<pb::SectionReply>, Status> {
        // The message carries no batch selector, so the first batch is the one
        // reported; a client that needs another reads it over HTTP.
        let id = request.into_inner().id;
        let (_, map) = self.open(&id)?;
        Ok(Response::new(pb::SectionReply {
            json: json_string(&maps::prm_section(&map, &id, 0).map_err(status)?)?,
        }))
    }

    async fn inspect(
        &self,
        request: Request<pb::EditRequest>,
    ) -> std::result::Result<Response<pb::InspectReply>, Status> {
        let request = request.into_inner();
        let map = ourealis_map_format::Map::from_bytes(request.image)
            .map_err(|error| status(ServiceError::Map(error)))?;
        Ok(Response::new(pb::InspectReply {
            structure_json: json_string(&omf::structure_json(&map))?,
        }))
    }

    async fn edit(
        &self,
        request: Request<pb::EditRequest>,
    ) -> std::result::Result<Response<pb::ImageReply>, Status> {
        let request = request.into_inner();
        let edits: omf::EditScript = serde_json::from_str(&request.edits_json)
            .map_err(|error| status(ServiceError::Invalid(format!("edits_json: {error}"))))?;
        let (image, _) = omf::apply_edit(&request.image, &edits).map_err(status)?;
        Ok(Response::new(pb::ImageReply { image }))
    }
}

/// A 16-bit layer identifier, as the container numbers them.
fn layer_id(value: u32) -> std::result::Result<u16, Status> {
    u16::try_from(value).map_err(|_| {
        status(ServiceError::Invalid(format!(
            "layer id {value} exceeds 16 bits"
        )))
    })
}

/// An 8-bit LOD level.
fn level(value: u32) -> std::result::Result<u8, Status> {
    u8::try_from(value).map_err(|_| {
        status(ServiceError::Invalid(format!(
            "level {value} exceeds 8 bits"
        )))
    })
}

impl MapApi {
    /// Opens a map by id, so the section methods share one path.
    fn open(
        &self,
        id: &str,
    ) -> std::result::Result<(Arc<crate::store::MapEntry>, ourealis_map_format::Map), Status> {
        maps::open_map(&self.state, id).map_err(status)
    }

    /// Creation time of a map in Unix milliseconds.
    ///
    /// The library entry is the authority — the JSON summary it carries keeps only
    /// whole seconds — so every method reports the same instant for the same map;
    /// the summary's own timestamp answers only when the entry disappeared between
    /// the listing and this read.
    fn created_at_ms(&self, summary: &crate::api::dto::MapSummary) -> i64 {
        match self.state.maps.get(&summary.id) {
            Ok(entry) => entry.created_at_ms,
            Err(_) => rfc3339_to_unix_ms(&summary.created_at),
        }
    }
}

#[tonic::async_trait]
impl pb::simulation_service_server::SimulationService for SimulationApi {
    async fn submit(
        &self,
        request: Request<pb::SubmitRequest>,
    ) -> std::result::Result<Response<pb::SubmitReply>, Status> {
        let request = request.into_inner();
        let parsed: crate::api::dto::SimulationRequest =
            serde_json::from_str(&request.request_json)
                .map_err(|error| status(ServiceError::Invalid(format!("request_json: {error}"))))?;
        simulations::validate_request(&parsed).map_err(status)?;
        let id = crate::store::identifier(parsed.name.as_deref().unwrap_or("run"));
        let job = self.state.runner.submit(id, parsed).await.map_err(status)?;
        Ok(Response::new(pb::SubmitReply {
            id: job.id().to_string(),
            state: job_state(job.state()),
        }))
    }

    async fn list(
        &self,
        request: Request<pb::ListSimulationsRequest>,
    ) -> std::result::Result<Response<pb::ListSimulationsReply>, Status> {
        let request = request.into_inner();
        let (offset, limit) = self.state.page(crate::api::dto::PageQuery {
            offset: request.offset as usize,
            limit: request.limit as usize,
        });
        let all = self.state.jobs.list();
        let total = all.len() as u32;
        let items = all
            .into_iter()
            .skip(offset)
            .take(limit)
            .map(|job| simulation_state(&job.to_dto()))
            .collect();
        Ok(Response::new(pb::ListSimulationsReply { items, total }))
    }

    async fn get(
        &self,
        request: Request<pb::GetSimulationRequest>,
    ) -> std::result::Result<Response<pb::SimulationState>, Status> {
        let id = request.into_inner().id;
        let job = self.state.jobs.get(&id).map_err(status)?;
        Ok(Response::new(simulation_state(&job.to_dto())))
    }

    async fn cancel(
        &self,
        request: Request<pb::CancelRequest>,
    ) -> std::result::Result<Response<pb::Empty>, Status> {
        let id = request.into_inner().id;
        let job = self.state.jobs.get(&id).map_err(status)?;
        if !job.cancel() {
            return Err(status(ServiceError::Conflict(format!(
                "simulation {id} already finished with state {}",
                job.state().to_string_name()
            ))));
        }
        Ok(Response::new(pb::Empty {}))
    }

    type WatchStream = ReceiverStream<std::result::Result<pb::Event, Status>>;

    async fn watch(
        &self,
        request: Request<pb::WatchRequest>,
    ) -> std::result::Result<Response<Self::WatchStream>, Status> {
        let request = request.into_inner();
        let job = self.state.jobs.get(&request.id).map_err(status)?;
        let topics = request.topics;
        let (sender, receiver) = mpsc::channel(STREAM_QUEUE);
        tokio::spawn(async move {
            let mut events = job.subscribe();
            if send_event(&sender, &state_event(&job)).await.is_err() {
                return;
            }
            if job.state().is_terminal() {
                let _ = send_event(&sender, &terminal_event(&job)).await;
                return;
            }
            loop {
                match events.recv().await {
                    Ok(envelope) => {
                        let terminal = envelope.is_terminal();
                        if (terminal || crate::job::events::accepts(&topics, &envelope.event))
                            && send_envelope(&sender, &envelope).await.is_err()
                        {
                            return;
                        }
                        if terminal {
                            return;
                        }
                    }
                    // Progress events are idempotent snapshots, so a subscriber
                    // that fell behind is brought up to date rather than replayed.
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                        if send_event(&sender, &state_event(&job)).await.is_err() {
                            return;
                        }
                        // The run may have ended while this stream was behind;
                        // without this the stream would wait for an event the job
                        // never publishes, because the job holds its broadcast
                        // sender for its whole life in the registry.
                        if job.state().is_terminal() {
                            let _ = send_event(&sender, &terminal_event(&job)).await;
                            return;
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => return,
                }
            }
        });
        Ok(Response::new(ReceiverStream::new(receiver)))
    }

    async fn summary(
        &self,
        request: Request<pb::SummaryRequest>,
    ) -> std::result::Result<Response<pb::SummaryReply>, Status> {
        let id = request.into_inner().id;
        let job = self.state.jobs.get(&id).map_err(status)?;
        let output = simulations::finished(&job).map_err(status)?;
        let summary = simulations::build_summary(&id, &output).map_err(status)?;
        Ok(Response::new(pb::SummaryReply {
            summary_json: json_string(&summary)?,
        }))
    }

    type TruthStream = ReceiverStream<std::result::Result<pb::TruthSample, Status>>;

    async fn truth(
        &self,
        request: Request<pb::StreamRequest>,
    ) -> std::result::Result<Response<Self::TruthStream>, Status> {
        let request = request.into_inner();
        let job = self.state.jobs.get(&request.id).map_err(status)?;
        let output = simulations::finished(&job).map_err(status)?;
        let frame = self.frame_size(request.limit);
        let (sender, receiver) = mpsc::channel(STREAM_QUEUE);
        tokio::spawn(async move {
            let total = output.truth.len();
            // The requested offset is where the stream starts; past the end it
            // yields nothing, which lets a client shard a channel without a
            // separate bounds query.
            let mut offset = (request.offset as usize).min(total);
            while offset < total {
                let page = streams::truth_page(&output, offset, frame);
                for sample in &page.items {
                    if sender.send(Ok(truth_sample(sample))).await.is_err() {
                        return;
                    }
                }
                offset = offset.saturating_add(frame);
            }
        });
        Ok(Response::new(ReceiverStream::new(receiver)))
    }

    type SensorsStream = ReceiverStream<std::result::Result<pb::SensorSample, Status>>;

    async fn sensors(
        &self,
        request: Request<pb::StreamRequest>,
    ) -> std::result::Result<Response<Self::SensorsStream>, Status> {
        let request = request.into_inner();
        let job = self.state.jobs.get(&request.id).map_err(status)?;
        let output = simulations::finished(&job).map_err(status)?;
        let channel = request.channel;
        let total = streams::channel_len(&output, &channel).map_err(status)?;
        let frame = self.frame_size(request.limit);
        let (sender, receiver) = mpsc::channel(STREAM_QUEUE);
        tokio::spawn(async move {
            // As for `truth`: the stream starts at the requested offset, and one
            // past the end is an empty stream rather than an error.
            let mut offset = (request.offset as usize).min(total);
            while offset < total {
                let page = match streams::sensor_page(&output, &channel, offset, frame) {
                    Ok(page) => page,
                    Err(error) => {
                        let _ = sender.send(Err(status(error))).await;
                        return;
                    }
                };
                for sample in &page.items {
                    if sender.send(Ok(sensor_sample(sample))).await.is_err() {
                        return;
                    }
                }
                offset = offset.saturating_add(frame);
            }
        });
        Ok(Response::new(ReceiverStream::new(receiver)))
    }

    async fn preview(
        &self,
        request: Request<pb::RoutePreviewRequest>,
    ) -> std::result::Result<Response<pb::RoutePreviewReply>, Status> {
        let request = request.into_inner();
        let parsed: crate::api::dto::SimulationRequest =
            serde_json::from_str(&request.request_json)
                .map_err(|error| status(ServiceError::Invalid(format!("request_json: {error}"))))?;
        let preview = routes::plan_in_background(Arc::clone(&self.state), parsed, true)
            .await
            .map_err(status)?;
        Ok(Response::new(pb::RoutePreviewReply {
            preview_json: json_string(&preview)?,
        }))
    }
}

impl SimulationApi {
    /// Samples per stream frame, bounded by the configuration.
    fn frame_size(&self, limit: u32) -> usize {
        let configured = self.state.config.simulation.stream_frame_samples.max(1);
        if limit == 0 {
            return configured;
        }
        (limit as usize).min(self.state.config.http.max_page_size)
    }
}

/// Maps a service error onto a gRPC status.
fn status(error: ServiceError) -> Status {
    crate::api::error::to_status(&error)
}

/// Serialises a value, classifying a failure as an internal error.
fn json_string<T: serde::Serialize>(value: &T) -> std::result::Result<String, Status> {
    serde_json::to_string(value).map_err(|error| status(ServiceError::Json(error)))
}

/// Serialises an optional value, yielding an empty string when absent.
fn optional_json_string<T: serde::Serialize>(
    value: Option<&T>,
) -> std::result::Result<String, Status> {
    match value {
        Some(value) => json_string(value),
        None => Ok(String::new()),
    }
}

/// One library summary as a protobuf message.
fn summary_to_pb(summary: &crate::api::dto::MapSummary, created_at_unix_ms: i64) -> pb::MapSummary {
    pb::MapSummary {
        id: summary.id.clone(),
        name: summary.name.clone(),
        bounds: Some(pb::Aabb {
            min_x: summary.bounds.min_x,
            min_y: summary.bounds.min_y,
            max_x: summary.bounds.max_x,
            max_y: summary.bounds.max_y,
        }),
        base_res_m: summary.base_res_m,
        chunk_size: summary.chunk_size,
        lod_count: summary.lod_count,
        feature_dim: summary.feature_dim,
        layer_count: summary.layer_count,
        source: summary.source.clone(),
        created_at_unix_ms,
        size_bytes: summary.size_bytes,
    }
}

/// Lifecycle state as the protobuf enum.
fn job_state(state: crate::api::dto::JobStateDto) -> i32 {
    match state {
        crate::api::dto::JobStateDto::Queued => pb::JobState::Queued as i32,
        crate::api::dto::JobStateDto::Running => pb::JobState::Running as i32,
        crate::api::dto::JobStateDto::Succeeded => pb::JobState::Succeeded as i32,
        crate::api::dto::JobStateDto::Failed => pb::JobState::Failed as i32,
        crate::api::dto::JobStateDto::Cancelled => pb::JobState::Cancelled as i32,
    }
}

/// One job's state as a protobuf message.
fn simulation_state(dto: &SimulationStateDto) -> pb::SimulationState {
    pb::SimulationState {
        id: dto.id.clone(),
        state: job_state(dto.state),
        stage: dto.stage.clone(),
        progress: dto.progress,
        elapsed_s: dto.elapsed_s,
        name: dto.name.clone().unwrap_or_default(),
        error: dto.error.clone().unwrap_or_default(),
        error_kind: dto.error_kind.clone().unwrap_or_default(),
        created_at_unix_ms: rfc3339_to_unix_ms(&dto.created_at),
        started_at_unix_ms: dto
            .started_at
            .as_deref()
            .map(rfc3339_to_unix_ms)
            .unwrap_or(0),
        finished_at_unix_ms: dto
            .finished_at
            .as_deref()
            .map(rfc3339_to_unix_ms)
            .unwrap_or(0),
        map_id: dto.map_id.clone(),
        mode: dto.mode.clone(),
    }
}

/// One ground-truth sample as a protobuf message.
fn truth_sample(sample: &TruthSampleDto) -> pb::TruthSample {
    pb::TruthSample {
        time_s: sample.time_s,
        position: Some(vec2(sample.position)),
        position_low: Some(vec2(sample.position_low)),
        z: sample.z,
        terrain_z: sample.terrain_z,
        speed: sample.speed,
        heading: sample.heading_rad,
        head_heading: sample.head_heading_rad,
        pitch: sample.pitch_rad,
        roll: sample.roll_rad,
        kappa_eff: sample.kappa_eff,
        offset_m: sample.offset_m,
        grade: sample.grade,
        standing: sample.standing,
        turning: sample.turning,
        velocity: sample.velocity.to_vec(),
        acceleration: sample.acceleration.to_vec(),
    }
}

/// One sensor sample as a protobuf message.
fn sensor_sample(sample: &SensorSampleDto) -> pb::SensorSample {
    pb::SensorSample {
        time_s: sample.time_s,
        channel: sample.channel.to_string(),
        v: sample.v.map(|v| v.to_vec()).unwrap_or_default(),
        latitude_deg: sample.latitude_deg.unwrap_or(0.0),
        longitude_deg: sample.longitude_deg.unwrap_or(0.0),
        altitude_m: sample.altitude_m.unwrap_or(0.0),
        speed_mps: sample.speed_mps.unwrap_or(0.0),
        heading_rad: sample.heading_rad.unwrap_or(0.0),
        valid: sample.valid.unwrap_or(false),
        satellites: sample.satellites.unwrap_or(0) as u32,
        pressure_pa: sample.pressure_pa.unwrap_or(0.0),
    }
}

/// A point as a protobuf message.
fn vec2(point: crate::api::dto::Vec2) -> pb::Vec2 {
    pb::Vec2 {
        x: point.x,
        y: point.y,
    }
}

/// One event as a protobuf message.
fn event_message(event: &EventDto, at_unix_ms: i64) -> pb::Event {
    pb::Event {
        r#type: event.name().to_string(),
        data_json: serde_json::to_string(event).unwrap_or_else(|_| "{}".to_string()),
        at_unix_ms,
    }
}

/// Sends one event, reporting whether the client is still there.
async fn send_event(
    sender: &mpsc::Sender<std::result::Result<pb::Event, Status>>,
    event: &EventDto,
) -> std::result::Result<(), ()> {
    let at = crate::api::time::now_unix_ms();
    sender
        .send(Ok(event_message(event, at)))
        .await
        .map_err(|_| ())
}

/// Sends one event with the time it was published.
async fn send_envelope(
    sender: &mpsc::Sender<std::result::Result<pb::Event, Status>>,
    envelope: &EventEnvelope,
) -> std::result::Result<(), ()> {
    sender
        .send(Ok(event_message(&envelope.event, envelope.at_ms)))
        .await
        .map_err(|_| ())
}

/// The current state as an event.
fn state_event(job: &crate::job::Job) -> EventDto {
    let dto = job.to_dto();
    EventDto::State {
        state: dto.state.to_string_name().to_string(),
        stage: dto.stage,
        progress: dto.progress,
        elapsed_s: dto.elapsed_s.unwrap_or(0.0),
    }
}

/// The outcome of a job that already finished.
fn terminal_event(job: &crate::job::Job) -> EventDto {
    let dto = job.to_dto();
    match dto.state {
        crate::api::dto::JobStateDto::Failed => EventDto::Error {
            kind: dto.error_kind.unwrap_or_else(|| "internal".to_string()),
            message: dto
                .error
                .unwrap_or_else(|| "the run failed without a message".to_string()),
        },
        _ => EventDto::Done {
            state: dto.state.to_string_name().to_string(),
            summary_url: format!(
                "{}/simulations/{}/summary",
                crate::api::API_PREFIX,
                job.id()
            ),
        },
    }
}

/// Parses the RFC 3339 timestamps this service produces into Unix milliseconds.
///
/// The JSON side carries RFC 3339 and the protobuf side Unix milliseconds, and
/// the only producer of the JSON form is [`crate::api::time`], which always emits
/// `YYYY-MM-DDTHH:MM:SSZ`; the conversion therefore ends in the `* 1000` the
/// `_unix_ms` fields are named for. Anything else yields 0, which a client reads
/// as "unknown" rather than as 1970.
fn rfc3339_to_unix_ms(text: &str) -> i64 {
    let bytes = text.as_bytes();
    if bytes.len() != 20 || bytes[4] != b'-' || bytes[10] != b'T' || bytes[19] != b'Z' {
        return 0;
    }
    let number = |from: usize, to: usize| -> Option<i64> { text.get(from..to)?.parse().ok() };
    let (Some(year), Some(month), Some(day), Some(hour), Some(minute), Some(second)) = (
        number(0, 4),
        number(5, 7),
        number(8, 10),
        number(11, 13),
        number(14, 16),
        number(17, 19),
    ) else {
        return 0;
    };
    if !(1..=12).contains(&month)
        || !(1..=31).contains(&day)
        || hour > 23
        || minute > 59
        || second > 60
    {
        return 0;
    }
    let days = days_from_civil(year, month, day);
    (days * 86_400 + hour * 3_600 + minute * 60 + second) * 1_000
}

/// Days since 1970-01-01 for a civil date.
///
/// Howard Hinnant's `days_from_civil`, the inverse of the conversion in
/// [`crate::api::time`].
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let year_of_era = year - era * 400;
    let month_index = if month > 2 { month - 3 } else { month + 9 };
    let day_of_year = (153 * month_index + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}
