//! Lifting connector elevations onto a planned path.
//!
//! A plan is geometry on the plane: the search knows a stair is a link with a
//! vertical profile, the plan carries only its footprint. The trajectory, and
//! through it the barometer and the body pitch, takes its height from the
//! terrain — which is exactly right everywhere except on a Z-axis link, where
//! the terrain has nothing to say. This module turns the connector records the
//! plan was routed through into per-vertex elevation overrides, so the motion
//! stage sees the climb without knowing that connectors exist.
//!
//! The same traversal also carries the link's *speed*: the physiological slope
//! model does not describe steps, so a stair declares its own equivalent speed
//! (up and down) and that is the ceiling over the traversal. Without it the
//! runner would take a stairwell at the speed of the ground beside it.
//!
//! Both channels are stamped once, on the finished path, rather than carried
//! through planning: the planner moves vertices (smoothing, resampling,
//! simplification) and any channel parallel to the points would have to be
//! moved with them everywhere. The finished path is also the first and only
//! place where the path is known to pass the connector endpoints.

use crate::error::Result;
use crate::graph::{ConnectorEntry, ConnectorSet, ENDPOINT_TOLERANCE_M};
use crate::path::Path;
use crate::terrain::Terrain;

/// Stamps the elevation and speed of every connector a path traverses.
///
/// For each connector, the vertices coinciding with its two endpoints — within
/// half a metre, in either traversal order — take the endpoint elevations, the
/// vertices strictly between them take a linear ramp, and the two vertices just
/// outside the link take the terrain height. The outside pair is what makes the
/// interpolation leave the link *at* the terrain instead of smearing the ramp
/// over the rest of the path, which would make the barometer read a climb that
/// never ends. The traversed vertices also take the link's equivalent speed for
/// the direction travelled.
///
/// A path that traverses no connector is returned unchanged: both channels stay
/// empty and the path stays byte-for-byte the planner's output.
pub fn lift_connector_elevations(
    path: &Path,
    connectors: &ConnectorSet,
    terrain: &Terrain,
) -> Result<Path> {
    if connectors.is_empty() || path.len() < 2 {
        return Ok(path.clone());
    }
    let mut elevation: Vec<Option<f64>> = vec![None; path.len()];
    let mut link_speed: Vec<Option<f64>> = vec![None; path.len()];
    let mut lifted = false;
    for (entry_index, start, end) in connectors.traversals(path.points()) {
        let entry = &connectors.entries()[entry_index];
        if stamp_connector(
            path,
            entry,
            start,
            end,
            terrain,
            &mut elevation,
            &mut link_speed,
        ) {
            lifted = true;
        }
    }
    if !lifted {
        return Ok(path.clone());
    }
    Path::with_links(path.points().to_vec(), elevation, link_speed)
}

/// Stamps one traversal: the two endpoint vertices and the ramp between them.
///
/// Returns whether the link declares a usable speed for the direction travelled;
/// when it does not, the elevation still stands and the runner keeps the
/// physiological ceiling.
fn stamp_connector(
    path: &Path,
    entry: &ConnectorEntry,
    start: usize,
    end: usize,
    terrain: &Terrain,
    elevation: &mut [Option<f64>],
    link_speed: &mut [Option<f64>],
) -> bool {
    let points = path.points();
    let cumulative = path.cumulative();
    let at_a = (points[start] - entry.a).length() <= ENDPOINT_TOLERANCE_M;
    let (from_z, to_z) = if at_a {
        (entry.elevation_a(), entry.elevation_b())
    } else {
        (entry.elevation_b(), entry.elevation_a())
    };

    // The declared speed is `v_up` or `v_down` according to which end is higher
    // and which way the runner is going; a link that declares neither keeps the
    // physiological ceiling, which is what an unmapped ramp wants.
    let declared = entry.connector.speed(at_a);
    let speed = (declared > 0.0).then_some(declared);

    let span = (cumulative[end] - cumulative[start]).max(f64::MIN_POSITIVE);
    for index in start..=end {
        let t = ((cumulative[index] - cumulative[start]) / span).clamp(0.0, 1.0);
        elevation[index] = Some(from_z + (to_z - from_z) * t);
        link_speed[index] = speed.map(f64::from);
    }
    if start > 0 {
        elevation[start - 1] = Some(terrain.height_at(points[start - 1]));
    }
    if end + 1 < points.len() {
        elevation[end + 1] = Some(terrain.height_at(points[end + 1]));
    }
    speed.is_some()
}
