//! Can a runner be here?
//!
//! The workspace lets a route be drawn with the pointer, and a point dropped inside a
//! building is not something the client can see: the hard mask lives in the map, and the
//! client only draws the surface. Without this the reader learns about it a plan later,
//! as a failed task, which reads as "the planner is broken" rather than "this point is
//! in a wall".
//!
//! The answer is deliberately the cheap half of the environment — the hard-forbidden
//! bitmap and, around the point, the distance to the nearest blocked cell. No cost
//! field, no graph, no PRM: a handful of chunk reads, so the endpoint is synchronous and
//! the client can call it as the reader drops a point.

use std::collections::HashMap;
use std::sync::Arc;

use axum::Json;
use axum::Router;
use axum::extract::rejection::{JsonRejection, PathRejection};
use axum::extract::{Path, State};
use axum::routing::post;
use ourealis_map_format::layer::LayerId;
use ourealis_map_format::raster::RasterChunk;
use ourealis_map_format::reader::Map;
use serde::{Deserialize, Serialize};

use crate::api::dto::Vec2;
use crate::api::error::json_rejection;
use crate::api::maps::{open_map, path_rejection};
use crate::app::AppState;
use crate::error::{Result, ServiceError};

/// Largest number of points one request may carry.
///
/// A drawn route has a handful of points; the bound keeps a hostile caller from
/// turning "read a few cells" into a whole-map scan.
const MAX_POINTS: usize = 256;

/// Largest safe radius a request may ask for, metres.
const MAX_SAFE_RADIUS_M: f64 = 25.0;

/// Radius used when a request does not name one, metres.
///
/// The same value `OffsetConfig::safe_radius_m` defaults to, so a point this endpoint
/// calls legal is one the offset stage would also accept.
const DEFAULT_SAFE_RADIUS_M: f64 = 0.75;

/// Request of the feasibility query.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FeasibilityRequest {
    /// Points to test, in the map's local metre plane.
    pub points: Vec<Vec2>,
    /// Distance a point must keep from a blocked cell, metres.
    #[serde(default)]
    pub safe_radius_m: Option<f64>,
}

/// Why a point is or is not usable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FeasibilityReason {
    /// Usable.
    Ok,
    /// Outside the map's extent.
    Outside,
    /// Inside a cell the map marks as forbidden.
    Forbidden,
    /// Passable, but closer to a blocked cell than the safe radius.
    TooClose,
    /// The map declares no hard-forbidden layer, so nothing can be said.
    Unknown,
}

/// The answer for one point.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FeasibilityAnswer {
    /// The point that was tested.
    pub point: Vec2,
    /// Whether the planner would accept it.
    pub legal: bool,
    /// Why.
    pub reason: FeasibilityReason,
    /// Distance to the nearest blocked cell centre, metres; absent without a mask.
    pub distance_m: Option<f64>,
    /// Cell the point falls in, in the map's own grid; absent outside the map.
    pub cell: Option<[i64; 2]>,
    /// Elevation at the point, metres, when the map carries an elevation layer.
    pub elevation_m: Option<f64>,
}

/// Answer to a feasibility query.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FeasibilityReply {
    /// One answer per requested point, in the order they were given.
    pub items: Vec<FeasibilityAnswer>,
}

/// Tests a list of points against the map's constraints.
pub async fn check(
    State(state): State<Arc<AppState>>,
    path: Result<Path<String>, PathRejection>,
    body: Result<Json<FeasibilityRequest>, JsonRejection>,
) -> Result<Json<FeasibilityReply>> {
    let id = path.map_err(path_rejection)?.0;
    let request = body.map_err(json_rejection)?.0;
    if request.points.len() > MAX_POINTS {
        return Err(ServiceError::Invalid(format!(
            "{} points were given; at most {MAX_POINTS} are accepted",
            request.points.len()
        )));
    }
    let radius = request.safe_radius_m.unwrap_or(DEFAULT_SAFE_RADIUS_M);
    if !(0.0..=MAX_SAFE_RADIUS_M).contains(&radius) {
        return Err(ServiceError::Invalid(format!(
            "safe_radius_m must lie in [0, {MAX_SAFE_RADIUS_M}]"
        )));
    }
    let (_entry, map) = open_map(&state, &id)?;
    Ok(Json(FeasibilityReply {
        items: check_points(&map, &request.points, radius),
    }))
}

/// Tests each point against one opened map.
fn check_points(map: &Map, points: &[Vec2], radius: f64) -> Vec<FeasibilityAnswer> {
    let mut cells = CellReader::new(map);
    points
        .iter()
        .map(|point| answer_for(&mut cells, *point, radius))
        .collect()
}

/// The answer for one point.
fn answer_for(cells: &mut CellReader<'_>, point: Vec2, radius: f64) -> FeasibilityAnswer {
    let geometry = cells.geometry;
    if !geometry.holds(point.x, point.y) {
        return FeasibilityAnswer {
            point,
            legal: false,
            reason: FeasibilityReason::Outside,
            distance_m: None,
            cell: None,
            elevation_m: None,
        };
    }
    let cell = geometry.cell_of(point.x, point.y);
    let elevation_m = cells.value(LayerId::ELEVATION, cell);
    let Some(blocked) = cells
        .value(LayerId::HARD_FORBIDDEN, cell)
        .map(|value| value > 0.5)
    else {
        // No mask, or no cell: the map does not say, and certifying the point would be
        // a promise this endpoint cannot keep.
        return FeasibilityAnswer {
            point,
            legal: true,
            reason: FeasibilityReason::Unknown,
            distance_m: None,
            cell: Some(cell),
            elevation_m,
        };
    };
    if blocked {
        return FeasibilityAnswer {
            point,
            legal: false,
            reason: FeasibilityReason::Forbidden,
            distance_m: Some(0.0),
            cell: Some(cell),
            elevation_m,
        };
    }
    // The rule the offset stage applies: a point can be passable and still unusable
    // because it stands against a wall.
    let distance = distance_to_blocked(&geometry, cells, point, radius);
    let legal = distance.is_none_or(|value| value >= radius);
    FeasibilityAnswer {
        point,
        legal,
        reason: if legal {
            FeasibilityReason::Ok
        } else {
            FeasibilityReason::TooClose
        },
        distance_m: distance,
        cell: Some(cell),
        elevation_m,
    }
}

/// Distance from a point to the nearest blocked cell centre, metres.
///
/// Scanned over a window the size of the radius rather than read from the map's own
/// distance transform: a synthetic map carries no derived layers, and the rule is about
/// cells within a metre or two, so a window is both cheaper and available everywhere.
/// `None` when the map declares no mask.
fn distance_to_blocked(
    geometry: &Geometry,
    cells: &mut CellReader<'_>,
    point: Vec2,
    radius: f64,
) -> Option<f64> {
    let reach = (radius / geometry.resolution).ceil().max(1.0) as i64;
    let centre = geometry.cell_of(point.x, point.y);
    let mut best: Option<f64> = None;
    for row in (centre[1] - reach).max(0)..=(centre[1] + reach).min(i64::from(geometry.rows) - 1) {
        for column in
            (centre[0] - reach).max(0)..=(centre[0] + reach).min(i64::from(geometry.columns) - 1)
        {
            let cell = [column, row];
            let value = cells.value(LayerId::HARD_FORBIDDEN, cell)?;
            if value <= 0.5 {
                continue;
            }
            let (x, y) = geometry.centre_of(cell);
            let distance = (x - point.x).hypot(y - point.y);
            best = Some(best.map_or(distance, |current: f64| current.min(distance)));
        }
    }
    // Nothing blocked inside the window: the point is further from an obstacle than the
    // radius asked about, which is all the caller needs to know.
    Some(best.unwrap_or(radius))
}

/// The map's level-0 grid, read once.
#[derive(Debug, Clone, Copy)]
struct Geometry {
    min_x: f64,
    min_y: f64,
    resolution: f64,
    columns: u32,
    rows: u32,
    chunk_size: u32,
}

impl Geometry {
    /// Reads the geometry the queries need from one map.
    fn of(map: &Map) -> Self {
        let bounds = map.header().bounds;
        let (columns, rows) = map.grid().cell_dims(0);
        Self {
            min_x: bounds.min_x,
            min_y: bounds.min_y,
            resolution: map.grid().cell_size_m(0).max(f64::MIN_POSITIVE),
            columns,
            rows,
            chunk_size: map.grid().chunk_size,
        }
    }

    /// Whether a point lies inside the mapped extent.
    fn holds(&self, x: f64, y: f64) -> bool {
        let inside_columns =
            x >= self.min_x && x < self.min_x + f64::from(self.columns) * self.resolution;
        let inside_rows =
            y >= self.min_y && y < self.min_y + f64::from(self.rows) * self.resolution;
        inside_columns && inside_rows
    }

    /// Cell a point falls in.
    fn cell_of(&self, x: f64, y: f64) -> [i64; 2] {
        [
            ((x - self.min_x) / self.resolution).floor() as i64,
            ((y - self.min_y) / self.resolution).floor() as i64,
        ]
    }

    /// Centre of a cell in the local plane.
    fn centre_of(&self, cell: [i64; 2]) -> (f64, f64) {
        (
            self.min_x + (cell[0] as f64 + 0.5) * self.resolution,
            self.min_y + (cell[1] as f64 + 0.5) * self.resolution,
        )
    }

    /// Chunk id a cell belongs to.
    fn chunk_of(&self, cell: [i64; 2]) -> u32 {
        let side = self.chunk_size.max(1) as i64;
        let ix = (cell[0] / side).max(0) as u16;
        let iy = (cell[1] / side).max(0) as u16;
        ourealis_map_format::geometry::morton_encode_chunk(ix, iy)
    }

    /// Cell index inside its chunk.
    fn local_of(&self, cell: [i64; 2]) -> (u32, u32) {
        let side = self.chunk_size.max(1) as i64;
        (
            (cell[0] - (cell[0] / side).max(0) * side) as u32,
            (cell[1] - (cell[1] / side).max(0) * side) as u32,
        )
    }
}

/// Reads single cells of a map's level-0 layers, caching the chunks it opens.
///
/// A distance scan touches the cells around a point, which can straddle a chunk
/// boundary; without the cache that would be a chunk read per cell.
struct CellReader<'a> {
    map: &'a Map,
    geometry: Geometry,
    chunks: HashMap<(LayerId, u32), Option<RasterChunk>>,
}

impl<'a> CellReader<'a> {
    /// Creates a reader over one opened map.
    fn new(map: &'a Map) -> Self {
        Self {
            map,
            geometry: Geometry::of(map),
            chunks: HashMap::new(),
        }
    }

    /// One cell's first channel, or `None` when the layer or the cell is absent.
    fn value(&mut self, layer: LayerId, cell: [i64; 2]) -> Option<f64> {
        if cell[0] < 0 || cell[1] < 0 {
            return None;
        }
        let chunk_id = self.geometry.chunk_of(cell);
        let (column, row) = self.geometry.local_of(cell);
        let chunk = self.chunk(layer, chunk_id)?;
        if column >= chunk.width || row >= chunk.height {
            return None;
        }
        Some(f64::from(chunk.get(column, row, 0)))
    }

    /// The chunk with an id, read once per layer.
    fn chunk(&mut self, layer: LayerId, chunk_id: u32) -> Option<&RasterChunk> {
        if !self.chunks.contains_key(&(layer, chunk_id)) {
            let chunk = self.map.chunk(layer, 0, chunk_id).ok().flatten();
            self.chunks.insert((layer, chunk_id), chunk);
        }
        self.chunks.get(&(layer, chunk_id))?.as_ref()
    }
}

/// Every route of this module.
pub fn router() -> Router<Arc<AppState>> {
    Router::new().route("/maps/{id}/feasibility", post(check))
}
