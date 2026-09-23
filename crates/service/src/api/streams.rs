//! Sample streams of a finished run.
//!
//! The paged endpoints and the newline-delimited streams share one mapping from
//! `core`'s sample types to the wire DTOs, so a page item and a streamed line can
//! never disagree about a field. The streams are frames of
//! `simulation.stream_frame_samples` samples carried through a bounded channel,
//! which gives a client that stops reading back-pressure instead of a service
//! that buffers the whole result.

use std::sync::Arc;

use axum::Json;
use axum::Router;
use axum::body::{Body, Bytes};
use axum::extract::rejection::PathRejection;
use axum::extract::{Path, State};
use axum::http::header::{CONTENT_DISPOSITION, CONTENT_TYPE};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use ourealis_core::SimulationOutput;
use tokio_stream::wrappers::ReceiverStream;

use axum::extract::Query;
use axum::extract::rejection::QueryRejection;

use crate::api::dto::result::{SensorSampleDto, TruthSampleDto};
use crate::api::dto::{Page, PageQuery, Vec2};
use crate::api::error::query_rejection;
use crate::api::maps::path_rejection;
use crate::app::AppState;
use crate::error::{Result, ServiceError};

/// Channels the sensor endpoints accept.
pub(crate) const SENSOR_CHANNELS: [&str; 5] = ["gnss", "accel", "gyro", "mag", "baro"];

/// Ground truth as newline-delimited JSON.
pub async fn truth_ndjson(
    State(state): State<Arc<AppState>>,
    path: Result<Path<String>, PathRejection>,
) -> Result<Response> {
    let id = path.map_err(path_rejection)?.0;
    let task = state.tasks.get(&id)?;
    let output = crate::api::simulations::finished(&task)?;
    let frame = state.config.simulation.stream_frame_samples;
    Ok(ndjson_response(
        stream_ndjson(output, None, frame),
        &format!("{id}-truth.ndjson"),
    ))
}

/// One sensor channel: a page, or a newline-delimited stream.
///
/// axum requires a path parameter to fill a whole segment, so the documented
/// `{channel}.ndjson` form cannot be a route of its own; the channel and its
/// optional suffix are taken from the last segment instead, which keeps both URLs
/// exactly as the design specifies them.
pub async fn sensors_any(
    State(state): State<Arc<AppState>>,
    path: Result<Path<(String, String)>, PathRejection>,
    query: Result<Query<PageQuery>, QueryRejection>,
) -> Result<Response> {
    let (id, tail) = path.map_err(path_rejection)?.0;
    let task = state.tasks.get(&id)?;
    let output = crate::api::simulations::finished(&task)?;
    match tail.strip_suffix(".ndjson") {
        Some(channel) => {
            channel_len(&output, channel)?;
            let frame = state.config.simulation.stream_frame_samples;
            Ok(ndjson_response(
                stream_ndjson(output, Some(channel.to_string()), frame),
                &format!("{id}-{}-sensors.ndjson", channel_slug(channel)),
            ))
        }
        None => {
            let query = query.map_err(query_rejection)?.0;
            let (offset, limit) = state.page(query);
            Ok(Json(sensor_page(&output, &tail, offset, limit)?).into_response())
        }
    }
}

/// Every streaming route of this module.
///
/// The sensor subtree is one wildcard route because the same segment carries
/// either a channel or a channel with the `.ndjson` suffix.
pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/simulations/{id}/truth.ndjson", get(truth_ndjson))
        .route("/simulations/{id}/sensors/{*tail}", get(sensors_any))
}

/// A page of ground-truth samples.
pub(crate) fn truth_page(
    output: &SimulationOutput,
    offset: usize,
    limit: usize,
) -> Page<TruthSampleDto> {
    Page::new(
        slice(&output.truth, offset, limit)
            .iter()
            .map(truth_dto)
            .collect(),
        output.truth.len(),
        offset,
    )
}

/// A page of one sensor channel.
pub(crate) fn sensor_page(
    output: &SimulationOutput,
    channel: &str,
    offset: usize,
    limit: usize,
) -> Result<Page<SensorSampleDto>> {
    let total = channel_len(output, channel)?;
    let items = match channel {
        "gnss" => slice(&output.sensors.gnss, offset, limit)
            .iter()
            .map(|sample| SensorSampleDto {
                time_s: sample.time_s,
                channel: "gnss",
                v: None,
                latitude_deg: sample.latitude_deg,
                longitude_deg: sample.longitude_deg,
                altitude_m: Some(sample.altitude_m),
                speed_mps: Some(sample.speed_mps),
                heading_rad: Some(sample.heading_rad),
                valid: Some(sample.valid),
                satellites: Some(sample.satellites),
                pressure_pa: None,
            })
            .collect(),
        "accel" => slice(&output.sensors.imu.accel, offset, limit)
            .iter()
            .map(|sample| vector_sample(sample.time_s, "accel", sample.accel()))
            .collect(),
        "gyro" => slice(&output.sensors.imu.gyro, offset, limit)
            .iter()
            .map(|sample| vector_sample(sample.time_s, "gyro", sample.omega()))
            .collect(),
        "mag" => slice(&output.sensors.mag, offset, limit)
            .iter()
            .map(|sample| vector_sample(sample.time_s, "mag", sample.field()))
            .collect(),
        "baro" => slice(&output.sensors.baro, offset, limit)
            .iter()
            .map(|sample| SensorSampleDto {
                time_s: sample.time_s,
                channel: "baro",
                v: None,
                latitude_deg: None,
                longitude_deg: None,
                altitude_m: Some(sample.altitude_m),
                speed_mps: None,
                heading_rad: None,
                valid: None,
                satellites: None,
                pressure_pa: Some(sample.pressure_pa),
            })
            .collect(),
        other => {
            return Err(ServiceError::Invalid(format!(
                "unknown sensor channel {other:?}; expected one of {}",
                SENSOR_CHANNELS.join(", ")
            )));
        }
    };
    Ok(Page::new(items, total, offset))
}

/// Number of samples of a channel.
pub(crate) fn channel_len(output: &SimulationOutput, channel: &str) -> Result<usize> {
    Ok(match channel {
        "gnss" => output.sensors.gnss.len(),
        "accel" => output.sensors.imu.accel.len(),
        "gyro" => output.sensors.imu.gyro.len(),
        "mag" => output.sensors.mag.len(),
        "baro" => output.sensors.baro.len(),
        other => {
            return Err(ServiceError::Invalid(format!(
                "unknown sensor channel {other:?}; expected one of {}",
                SENSOR_CHANNELS.join(", ")
            )));
        }
    })
}

/// One ground-truth sample.
pub(crate) fn truth_dto(sample: &ourealis_core::sensor::TruthState) -> TruthSampleDto {
    TruthSampleDto {
        time_s: sample.time_s,
        position: Vec2::from(sample.position),
        position_low: Vec2::from(sample.position_low),
        z: sample.z,
        terrain_z: sample.terrain_z,
        speed: sample.speed,
        heading_rad: sample.heading,
        head_heading_rad: sample.head_heading,
        pitch_rad: sample.pitch,
        roll_rad: sample.roll,
        kappa_eff: sample.kappa_eff,
        offset_m: sample.offset_m,
        grade: sample.grade,
        standing: sample.standing,
        turning: sample.turning,
        velocity: sample.velocity,
        acceleration: sample.acceleration,
    }
}

/// The paged range of a slice, empty when the range starts past the end.
fn slice<T>(items: &[T], offset: usize, limit: usize) -> &[T] {
    let end = offset.saturating_add(limit).min(items.len());
    if offset >= end {
        return &[];
    }
    items.get(offset..end).unwrap_or(&[])
}

/// A sample whose payload is a three-vector.
fn vector_sample(time_s: f64, channel: &'static str, values: [f64; 3]) -> SensorSampleDto {
    SensorSampleDto {
        time_s,
        channel,
        v: Some(values),
        latitude_deg: None,
        longitude_deg: None,
        altitude_m: None,
        speed_mps: None,
        heading_rad: None,
        valid: None,
        satellites: None,
        pressure_pa: None,
    }
}

/// Streams samples as newline-delimited JSON, one frame at a time.
fn stream_ndjson(output: Arc<SimulationOutput>, channel: Option<String>, frame: usize) -> Body {
    let (sender, receiver) = tokio::sync::mpsc::channel::<Result<Bytes, std::io::Error>>(4);
    let frame = frame.max(1);
    tokio::spawn(async move {
        let total = match &channel {
            None => output.truth.len(),
            Some(name) => match channel_len(&output, name) {
                Ok(total) => total,
                Err(error) => {
                    let _ = sender
                        .send(Err(std::io::Error::other(error.to_string())))
                        .await;
                    return;
                }
            },
        };
        let mut offset = 0usize;
        while offset < total {
            let text = match frame_of(&output, channel.as_deref(), offset, frame) {
                Ok(text) => text,
                Err(error) => {
                    let _ = sender
                        .send(Err(std::io::Error::other(error.to_string())))
                        .await;
                    return;
                }
            };
            if sender.send(Ok(Bytes::from(text))).await.is_err() {
                return;
            }
            offset = offset.saturating_add(frame);
        }
    });
    Body::from_stream(ReceiverStream::new(receiver))
}

/// Renders one frame of NDJSON, one sample per line.
fn frame_of(
    output: &SimulationOutput,
    channel: Option<&str>,
    offset: usize,
    frame: usize,
) -> Result<String> {
    let mut text = String::new();
    match channel {
        None => {
            for sample in slice(&output.truth, offset, frame) {
                text.push_str(&serde_json::to_string(&truth_dto(sample))?);
                text.push('\n');
            }
        }
        Some(name) => {
            for sample in sensor_page(output, name, offset, frame)?.items {
                text.push_str(&serde_json::to_string(&sample)?);
                text.push('\n');
            }
        }
    }
    Ok(text)
}

/// A response carrying a newline-delimited JSON stream.
fn ndjson_response(body: Body, filename: &str) -> Response {
    attachment(body, "application/x-ndjson", filename)
}

/// A response that downloads as a file.
pub(crate) fn attachment(body: impl Into<Body>, content_type: &str, filename: &str) -> Response {
    let mut response = Response::new(body.into());
    if let Ok(value) = axum::http::HeaderValue::from_str(content_type) {
        response.headers_mut().insert(CONTENT_TYPE, value);
    }
    if let Ok(value) =
        axum::http::HeaderValue::from_str(&format!("attachment; filename=\"{filename}\""))
    {
        response.headers_mut().insert(CONTENT_DISPOSITION, value);
    }
    response
}

/// A channel name that is safe inside a filename.
pub(crate) fn channel_slug(channel: &str) -> String {
    channel
        .chars()
        .filter(|character| character.is_ascii_alphanumeric())
        .collect()
}
