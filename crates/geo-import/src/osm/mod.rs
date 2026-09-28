//! OpenStreetMap access.
//!
//! One Overpass query returns everything the importer consumes — the highway
//! network, running venues, building footprints and water bodies — as ways and
//! multipolygon relations with inline geometry, which avoids a second request
//! per element.
//!
//! The public Overpass instances shed load under demand, so an extract is
//! retried against the same endpoint and then against the fallbacks before the
//! import gives up.

pub mod class;
pub mod parse;
pub mod query;

pub use class::RoadClass;
pub use parse::parse_response;

use std::time::Duration;

use reqwest::blocking::Client;

use crate::error::{GeoError, Result};
use crate::geo::GeoBounds;

/// Endpoint the importer queries by default.
pub const DEFAULT_OVERPASS_ENDPOINT: &str = "https://overpass-api.de/api/interpreter";

/// Endpoints tried after the preferred one.
///
/// During one import run the same query was answered with `200` only on the
/// fifth attempt, and a run that has already paid for the DEM download should
/// not fail because the first instance was busy.
pub const FALLBACK_ENDPOINTS: [&str; 2] = [
    "https://overpass.private.coffee/api/interpreter",
    "https://overpass.kumi.systems/api/interpreter",
];

/// Attempts made against one endpoint before moving on.
const ATTEMPTS_PER_ENDPOINT: usize = 3;

/// Pause between two attempts against the same endpoint.
const RETRY_PAUSE: Duration = Duration::from_secs(5);

/// Deadline for one extract, seconds.
///
/// Deliberately above the query's own server-side `timeout:` so a slow answer still lands, but
/// finite all the same: an overloaded instance can accept the connection and then hold it
/// open without answering, and the fallbacks are only reached once the attempt in hand
/// returns. Without a deadline that stall is the end of the import rather than a retry.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(240);

/// User agent sent to Overpass, which requires a descriptive one.
pub const USER_AGENT: &str = concat!("ourealis-geo-import/", env!("CARGO_PKG_VERSION"));

/// One way that paints a surface, in `(longitude, latitude)` vertices.
///
/// A way below ground never becomes one: [`class::classify`] drops a tunnel or a
/// negative `layer`, because the cell it would paint is the hill it passes
/// through rather than a surface to run on. A bridge is kept instead of lifted
/// — the importer has no road-network topology to decide which spans to raise,
/// and a deck raised on its own would be unreachable rather than useful — so a
/// bridge rasterises at the terrain elevation like any other way.
#[derive(Debug, Clone, PartialEq)]
pub struct Road {
    /// Surface and channel values the way paints.
    pub class: RoadClass,
    /// Vertices in `(longitude, latitude)` degrees.
    pub points: Vec<(f64, f64)>,
}

/// A closed way that covers an area.
#[derive(Debug, Clone, PartialEq)]
pub struct Area {
    /// What the area means.
    pub kind: AreaKind,
    /// Outline in `(longitude, latitude)` degrees, not repeated at the end.
    pub outline: Vec<(f64, f64)>,
    /// Height of a building above the terrain, metres.
    ///
    /// `None` for every area that is not a building, and for a building whose tags
    /// carry no height; the raster supplies its own default then.
    pub height_m: Option<f32>,
}

/// What an area means for traversability.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AreaKind {
    /// An impassable building footprint.
    Building,
    /// An impassable water body.
    Water,
    /// An impassable lawn, wood or other planted ground.
    ///
    /// Painted as grass and blocked: a park, a wood or a grass verge is mapped
    /// ground that is not a route, and a planner that walked across it would cut
    /// every corner of the campus.
    Green,
    /// A paved surface of the given surface category, which is walkable.
    Surface(u8),
}

/// Everything the importer takes from OpenStreetMap.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct OsmData {
    /// Ways that paint a surface.
    pub roads: Vec<Road>,
    /// Closed ways that cover an area.
    pub areas: Vec<Area>,
}

/// Downloads the ways and relations covering a box.
///
/// `preferred` is queried first; the built-in fallbacks are used only when it
/// does not answer at all, so a caller that points at a private instance is not
/// silently redirected.
pub fn fetch(bounds: &GeoBounds, preferred: &str) -> Result<OsmData> {
    let text = query::overpass_query(bounds);
    let client = Client::builder()
        .timeout(REQUEST_TIMEOUT)
        .build()
        .map_err(|source| GeoError::Http {
            url: preferred.to_owned(),
            source,
        })?;
    let mut failure = GeoError::data("no overpass endpoint was configured");
    for endpoint in endpoints(preferred) {
        match extract(&client, endpoint, &text) {
            Ok(body) => return parse_response(&body),
            Err(error) => {
                tracing::warn!(endpoint, %error, "overpass endpoint unavailable");
                failure = error;
            }
        }
    }
    Err(failure)
}

/// The preferred endpoint followed by the fallbacks, without repeats.
fn endpoints(preferred: &str) -> impl Iterator<Item = &str> {
    std::iter::once(preferred).chain(
        FALLBACK_ENDPOINTS
            .iter()
            .copied()
            .filter(move |endpoint| *endpoint != preferred),
    )
}

/// One extract, with the attempts an overloaded instance needs.
fn extract(client: &Client, endpoint: &str, query: &str) -> Result<String> {
    let mut failure = None;
    for attempt in 0..ATTEMPTS_PER_ENDPOINT {
        if attempt > 0 {
            std::thread::sleep(RETRY_PAUSE);
        }
        match request(client, endpoint, query) {
            Ok(body) => return Ok(body),
            Err(error) => failure = Some(error),
        }
    }
    Err(failure.expect("at least one attempt is always made"))
}

/// One request, reporting a non-success status with the answer it came with.
fn request(client: &Client, endpoint: &str, query: &str) -> Result<String> {
    let url = endpoint.to_string();
    let response = client
        .post(endpoint)
        .header(reqwest::header::USER_AGENT, USER_AGENT)
        .form(&[("data", query)])
        .send()
        .map_err(|source| GeoError::Http {
            url: url.clone(),
            source,
        })?;
    let status = response.status();
    let body = response.text().map_err(|source| GeoError::Http {
        url: url.clone(),
        source,
    })?;
    if status.is_success() {
        return Ok(body);
    }
    Err(GeoError::response(endpoint, status.as_u16(), &body))
}
