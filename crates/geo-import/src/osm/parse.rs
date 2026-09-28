//! Decoding the Overpass JSON response.

use std::collections::HashMap;

use serde::Deserialize;

use crate::error::{GeoError, Result};
use crate::osm::class::{classify, is_green, parse_metres};
use crate::osm::{Area, AreaKind, OsmData, Road};

/// Overpass response envelope.
#[derive(Debug, Deserialize)]
struct Response {
    #[serde(default)]
    elements: Vec<Element>,
}

/// One element of a response.
///
/// A way carries its vertices in `geometry` and a relation in the `geometry` of
/// its members, so both fields are read for every element and the element type
/// decides which one means anything.
#[derive(Debug, Deserialize)]
struct Element {
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    tags: HashMap<String, String>,
    /// Vertices of a way; an entry is `null` when Overpass could not resolve one
    /// of the way's nodes.
    #[serde(default)]
    geometry: Vec<Option<LatLon>>,
    /// Members of a relation.
    #[serde(default)]
    members: Vec<Member>,
}

/// One member of a relation.
#[derive(Debug, Deserialize)]
struct Member {
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    role: String,
    #[serde(default)]
    geometry: Vec<Option<LatLon>>,
}

#[derive(Debug, Deserialize)]
struct LatLon {
    lat: f64,
    lon: f64,
}

/// Parses an Overpass `out geom` response into roads and areas.
///
/// An element is dropped rather than guessed at when its geometry is incomplete,
/// when a way has fewer than two vertices, or when its tags describe nothing the
/// map stores. Buildings and water bodies are closed by construction; a way that
/// closes but does not carry an area tag is still imported as a line.
pub fn parse_response(text: &str) -> Result<OsmData> {
    let response: Response = serde_json::from_str(text).map_err(|source| GeoError::Json {
        what: "overpass response".to_owned(),
        source,
    })?;

    let mut data = OsmData::default();
    let mut skipped = 0usize;
    for element in response.elements {
        let imported = match element.kind.as_str() {
            "way" => way_points(&element.geometry)
                .is_some_and(|points| import_way(&element.tags, points, &mut data)),
            "relation" => import_relation(&element.tags, &element.members, &mut data),
            _ => false,
        };
        if !imported {
            skipped += 1;
        }
    }
    tracing::debug!(
        roads = data.roads.len(),
        areas = data.areas.len(),
        skipped,
        "parsed overpass response"
    );
    if data.roads.is_empty() && data.areas.is_empty() {
        return Err(GeoError::data(
            "the overpass response contains no element the importer can use",
        ));
    }
    Ok(data)
}

fn way_points(geometry: &[Option<LatLon>]) -> Option<Vec<(f64, f64)>> {
    let mut points = Vec::with_capacity(geometry.len());
    for vertex in geometry {
        let vertex = vertex.as_ref()?;
        if !vertex.lat.is_finite() || !vertex.lon.is_finite() {
            return None;
        }
        points.push((vertex.lon, vertex.lat));
    }
    (points.len() >= 2).then_some(points)
}

/// Whether the vertex list returns to its first position.
fn is_closed(points: &[(f64, f64)]) -> bool {
    points.len() >= 4 && points[0] == points[points.len() - 1]
}

/// Classifies one way into `data`; `false` when nothing was imported.
fn import_way(tags: &HashMap<String, String>, points: Vec<(f64, f64)>, data: &mut OsmData) -> bool {
    if is_building(tags) {
        return push_area(data, AreaKind::Building, building_height_m(tags), points);
    }
    if is_water(tags) {
        return push_area(data, AreaKind::Water, None, points);
    }
    // A way that is both a route and a lawn is a route first: a footway through a
    // park carries the park's tags only when the mapper put them on the path, and
    // blocking it would cut the only way across the green.
    if !tags.contains_key("highway") && is_green(tags) {
        return push_area(data, AreaKind::Green, None, points);
    }
    let Some(class) = classify(tags) else {
        return false;
    };
    // A closed way with an explicit area tag, or a sports ground, describes a
    // surface to run on rather than a line to run along.
    let is_surface = is_closed(&points)
        && (tags.contains_key("leisure")
            || tags.contains_key("sport")
            || tags.get("area").is_some_and(|v| v == "yes"));
    if is_surface {
        return push_area(data, AreaKind::Surface(class.surface), None, points);
    }
    data.roads.push(Road { class, points });
    true
}

/// Classifies one multipolygon relation into `data`; `false` when nothing was
/// imported.
///
/// Only the outer rings become areas: the format stores one outline per area and
/// cannot express a hole, so an inner ring — a courtyard, an island in a lake —
/// is dropped and the area is painted solid.
fn import_relation(tags: &HashMap<String, String>, members: &[Member], data: &mut OsmData) -> bool {
    let (kind, height_m) = if is_building(tags) {
        (AreaKind::Building, building_height_m(tags))
    } else if is_water(tags) {
        (AreaKind::Water, None)
    } else if is_green(tags) {
        (AreaKind::Green, None)
    } else if let Some(class) = classify(tags) {
        (AreaKind::Surface(class.surface), None)
    } else {
        return false;
    };

    let fragments: Vec<Vec<(f64, f64)>> = members
        .iter()
        .filter(|member| member.kind == "way" && (member.role.is_empty() || member.role == "outer"))
        .filter_map(|member| way_points(&member.geometry))
        .collect();

    let mut imported = false;
    for ring in rings(fragments) {
        imported |= push_ring(data, kind, height_m, ring);
    }
    imported
}

/// Joins a relation's outer ways into rings.
///
/// Multipolygon members are open fragments — only their union is a ring — so
/// they are chained end to end. A chain that never returns to its start is a
/// mapping error and is dropped, as an unclosed way is.
fn rings(mut fragments: Vec<Vec<(f64, f64)>>) -> Vec<Vec<(f64, f64)>> {
    let mut rings = Vec::new();
    while let Some(mut chain) = fragments.pop() {
        loop {
            if chain.len() >= 4 && chain[0] == chain[chain.len() - 1] {
                chain.pop();
                rings.push(chain);
                break;
            }
            let end = chain[chain.len() - 1];
            let Some(index) = fragments.iter().position(|fragment| {
                fragment.first() == Some(&end) || fragment.last() == Some(&end)
            }) else {
                break;
            };
            let mut next = fragments.swap_remove(index);
            if next.last() == Some(&end) {
                next.reverse();
            }
            chain.extend_from_slice(&next[1..]);
        }
    }
    rings
}

/// Stores a closed way's outline; `false` when it cannot be an area.
///
/// An outline that does not return to its first vertex is a mapping error.
/// Filling it as if it closed would paint a sliver across the map, so it is
/// dropped and counted as skipped instead.
fn push_area(
    data: &mut OsmData,
    kind: AreaKind,
    height_m: Option<f32>,
    points: Vec<(f64, f64)>,
) -> bool {
    if !is_closed(&points) {
        return false;
    }
    push_ring(data, kind, height_m, points[..points.len() - 1].to_vec())
}

/// Stores an outline that is already a ring, its first vertex not repeated.
fn push_ring(
    data: &mut OsmData,
    kind: AreaKind,
    height_m: Option<f32>,
    outline: Vec<(f64, f64)>,
) -> bool {
    if outline.len() < 3 {
        return false;
    }
    data.areas.push(Area {
        kind,
        outline,
        height_m,
    });
    true
}

/// Height of a building above the terrain, metres, from its own tags.
///
/// `height` is the surveyed value and wins. `building:levels` is a storey count
/// and the storey height varies, so it is only the fallback; `None` leaves the
/// raster's default in place.
fn building_height_m(tags: &HashMap<String, String>) -> Option<f32> {
    if let Some(height) = tags.get("height").and_then(|value| parse_metres(value)) {
        return Some(height as f32);
    }
    tags.get("building:levels")
        .and_then(|value| value.trim().parse::<f32>().ok())
        .filter(|levels| *levels > 0.0)
        .map(|levels| levels * STOREY_HEIGHT_M)
}

/// Storey height used when a building carries only `building:levels`, metres.
const STOREY_HEIGHT_M: f32 = 3.2;

fn is_building(tags: &HashMap<String, String>) -> bool {
    tags.get("building").is_some_and(|value| value != "no")
}

fn is_water(tags: &HashMap<String, String>) -> bool {
    if tags.get("natural").is_some_and(|v| v == "water") {
        return true;
    }
    if tags.get("landuse").is_some_and(|v| v == "reservoir") {
        return true;
    }
    tags.get("waterway")
        .is_some_and(|v| matches!(v.as_str(), "riverbank" | "canal" | "dock"))
}
