//! Data transfer objects of the API.
//!
//! These types are the single definition of every resource: the HTTP facade
//! serialises them with `serde`, the gRPC facade maps them onto the generated
//! protobuf messages, and the handlers only ever see this layer. Units are in the
//! field names — `*_m`, `*_s`, `*_rad` — and follow `core`: metres, seconds,
//! radians, with the map's local plane as the spatial reference.

pub mod map;
pub mod result;
pub mod simulation;
pub mod task;

pub use map::{
    ChunkData, LayerGridDto, LayerInfo, MapMetadata, MapSummary, Page, SectionJson, SkeletonDto,
};
pub use result::{
    EventDto, MetricSummary, RoutePreview, RoutePreviewCandidate, SampleCountsDto, SensorSampleDto,
    SummaryDto, TruthSampleDto,
};
pub use simulation::{PersonOverrides, PersonSpec, SimulationRequest, SimulationSettings};
pub use task::{
    SubmitReply, TaskKindDto, TaskReply, TaskResultDto, TaskResultRef, TaskState, TaskStateDto,
    TaskSubmit,
};

use serde::{Deserialize, Serialize};

/// A point or vector in the map's local metre plane.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct Vec2 {
    /// Easting, metres.
    pub x: f64,
    /// Northing, metres.
    pub y: f64,
}

impl From<glam::DVec2> for Vec2 {
    fn from(value: glam::DVec2) -> Self {
        Self {
            x: value.x,
            y: value.y,
        }
    }
}

impl From<Vec2> for glam::DVec2 {
    fn from(value: Vec2) -> Self {
        glam::DVec2::new(value.x, value.y)
    }
}

/// An axis-aligned bounding box in the local metre plane.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct AabbDto {
    /// Minimum easting, metres.
    pub min_x: f64,
    /// Minimum northing, metres.
    pub min_y: f64,
    /// Maximum easting, metres.
    pub max_x: f64,
    /// Maximum northing, metres.
    pub max_y: f64,
}

impl From<ourealis_map_format::Aabb> for AabbDto {
    fn from(value: ourealis_map_format::Aabb) -> Self {
        Self {
            min_x: value.min_x,
            min_y: value.min_y,
            max_x: value.max_x,
            max_y: value.max_y,
        }
    }
}

/// Offset and limit of a paged request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PageQuery {
    /// Number of items to skip.
    #[serde(default)]
    pub offset: usize,
    /// Maximum number of items to return.
    #[serde(default = "default_page_limit")]
    pub limit: usize,
}

impl Default for PageQuery {
    fn default() -> Self {
        Self {
            offset: 0,
            limit: default_page_limit(),
        }
    }
}

/// Default page size when a request does not say.
pub fn default_page_limit() -> usize {
    1_000
}
