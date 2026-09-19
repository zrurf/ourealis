//! Z-axis connectors: stairs, elevators, footbridges and underpasses.
//!
//! A connector is not terrain. Its grade is not a continuous slope, so the
//! physiological model does not apply: it carries a fixed equivalent speed and a
//! unit cost per metre, plus a waiting cost for anything with a queue. It also
//! owns a footprint on the plane, and a flat line crossing that footprint is
//! rejected — a path must use the link, not walk through the stairwell.

use glam::DVec2;

use ourealis_map_format::tlv::value::{Connector, ConnectorTable, ConnectorType};

/// Radius of the rejected footprint around a connector's planar projection.
pub const FOOTPRINT_RADIUS_M: f64 = 1.5;

/// Largest distance at which a path vertex counts as standing on a connector
/// endpoint, metres.
///
/// A planned path is resampled to roughly a metre before a traversal is looked
/// for, so an endpoint rarely lands on a vertex; half that spacing keeps both
/// endpoints of a link recognisable while keeping a vertex that merely passes
/// near an endpoint from counting.
pub const ENDPOINT_TOLERANCE_M: f64 = 0.5;

/// Connectors with their planar geometry precomputed.
#[derive(Debug, Clone, Default)]
pub struct ConnectorSet {
    entries: Vec<ConnectorEntry>,
}

/// One connector, planar projection included.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ConnectorEntry {
    /// Raw connector record from the map.
    pub connector: Connector,
    /// Endpoint A projected on the local plane.
    pub a: DVec2,
    /// Endpoint B projected on the local plane.
    pub b: DVec2,
}

impl ConnectorEntry {
    /// Builds an entry from a raw connector.
    pub fn new(connector: Connector) -> Self {
        Self {
            connector,
            a: DVec2::new(connector.a[0] as f64, connector.a[1] as f64),
            b: DVec2::new(connector.b[0] as f64, connector.b[1] as f64),
        }
    }

    /// Elevation of endpoint A.
    pub fn elevation_a(&self) -> f64 {
        self.connector.a[2] as f64
    }

    /// Elevation of endpoint B.
    pub fn elevation_b(&self) -> f64 {
        self.connector.b[2] as f64
    }

    /// Kind of the link.
    pub fn kind(&self) -> Option<ConnectorType> {
        ConnectorType::from_u16(self.connector.type_id)
    }

    /// Equivalent cost of traversing from A to B.
    pub fn cost_a_to_b(&self, reference_speed: f64) -> Option<f64> {
        self.connector
            .dir_flag
            .allows_a_to_b()
            .then(|| self.connector.cost_equiv_m(reference_speed as f32) as f64)
    }

    /// Equivalent cost of traversing from B to A.
    pub fn cost_b_to_a(&self, reference_speed: f64) -> Option<f64> {
        self.connector
            .dir_flag
            .allows_b_to_a()
            .then(|| self.connector.cost_equiv_m(reference_speed as f32) as f64)
    }

    /// Equivalent traversal speed in the given direction, m/s.
    pub fn speed(&self, a_to_b: bool) -> f64 {
        self.connector.speed(a_to_b) as f64
    }

    /// Distance from a point to the planar projection of the link.
    pub fn planar_distance(&self, point: DVec2) -> f64 {
        crate::math::point_segment_distance(point, self.a, self.b)
    }
}

impl ConnectorSet {
    /// Builds a set from a map table.
    pub fn new(table: &ConnectorTable) -> Self {
        Self {
            entries: table
                .connectors
                .iter()
                .map(|c| ConnectorEntry::new(*c))
                .collect(),
        }
    }

    /// All connectors.
    #[inline]
    pub fn entries(&self) -> &[ConnectorEntry] {
        &self.entries
    }

    /// Number of connectors.
    #[inline]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// True when the map has no connector.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Smallest unit cost coefficient, the multi-level heuristic bound.
    pub fn min_unit_cost(&self) -> Option<f64> {
        self.entries
            .iter()
            .map(|entry| entry.connector.unit_cost as f64)
            .fold(None, |acc: Option<f64>, value| {
                Some(acc.map(|a| a.min(value)).unwrap_or(value))
            })
    }

    /// Links a point sequence traverses.
    ///
    /// Each traversal is `(entry index, first vertex, last vertex)`. A vertex
    /// counts as standing on an endpoint within [`ENDPOINT_TOLERANCE_M`], and the
    /// far endpoint must be reached without leaving the link's footprint: a path
    /// that walks past one stairwell entrance and comes back around it has not
    /// used the stair. A link may be traversed more than once — a multi-lap
    /// session repeats its path — so the scan continues past a completed
    /// traversal instead of stopping at the first one.
    ///
    /// Two consumers share this rule: the elevation stamp and the route cost. A
    /// stair's height and a stair's price must be decided by the same geometry,
    /// or a plan can be charged for a link it never used.
    pub fn traversals(&self, points: &[DVec2]) -> Vec<(usize, usize, usize)> {
        let mut out = Vec::new();
        if points.len() < 2 {
            return out;
        }
        for (index, entry) in self.entries.iter().enumerate() {
            let mut start = 0usize;
            while start < points.len() {
                let at_a = (points[start] - entry.a).length() <= ENDPOINT_TOLERANCE_M;
                let at_b = !at_a && (points[start] - entry.b).length() <= ENDPOINT_TOLERANCE_M;
                if !at_a && !at_b {
                    start += 1;
                    continue;
                }
                let far = if at_a { entry.b } else { entry.a };
                let mut end = start + 1;
                let mut found = None;
                while end < points.len() {
                    if (points[end] - far).length() <= ENDPOINT_TOLERANCE_M {
                        found = Some(end);
                        break;
                    }
                    if crate::math::point_segment_distance(points[end], entry.a, entry.b)
                        > FOOTPRINT_RADIUS_M
                    {
                        break;
                    }
                    end += 1;
                }
                match found {
                    Some(end) => {
                        out.push((index, start, end));
                        start = end + 1;
                    }
                    None => start += 1,
                }
            }
        }
        out
    }

    /// True when a segment runs through a connector footprint.
    ///
    /// The footprint is the planar link expanded by [`FOOTPRINT_RADIUS_M`]; a
    /// segment passing within that band must be replaced by the connector edge.
    pub fn crosses_footprint(&self, from: DVec2, to: DVec2) -> bool {
        self.entries
            .iter()
            .any(|entry| segments_intersect(from, to, entry.a, entry.b, FOOTPRINT_RADIUS_M))
    }

    /// True when any connector's footprint reaches into a box.
    ///
    /// Used to keep the coarse granularity away from connectors: a block that
    /// contains or touches one cannot stand for its area, because a planar path
    /// across the footprint is refused by [`ConnectorSet::crosses_footprint`] —
    /// the link has to be traversed as a link. Without this a coarse block could
    /// hide a stairwell and the search would cross it as flat ground.
    pub fn footprint_touches(&self, bounds: &ourealis_map_format::Aabb, radius_m: f64) -> bool {
        if self.entries.is_empty() {
            return false;
        }
        let expanded = ourealis_map_format::Aabb::new(
            bounds.min_x - radius_m,
            bounds.min_y - radius_m,
            bounds.max_x + radius_m,
            bounds.max_y + radius_m,
        );
        let corners = [
            DVec2::new(expanded.min_x, expanded.min_y),
            DVec2::new(expanded.max_x, expanded.min_y),
            DVec2::new(expanded.max_x, expanded.max_y),
            DVec2::new(expanded.min_x, expanded.max_y),
        ];
        for entry in &self.entries {
            // An endpoint inside the box, or the segment crossing one of its
            // edges, covers every way a segment can reach into it.
            if expanded.contains(entry.a.x, entry.a.y) || expanded.contains(entry.b.x, entry.b.y) {
                return true;
            }
            for index in 0..4 {
                let from = corners[index];
                let to = corners[(index + 1) % 4];
                if segments_intersect(entry.a, entry.b, from, to, 0.0) {
                    return true;
                }
            }
        }
        false
    }

    /// True when a segment is an *approach* to a connector endpoint.
    ///
    /// An approach ends on an endpoint by construction, so it lies inside the
    /// footprint and the footprint rejection would refuse it — and with it every
    /// way of reaching the link. The exemption is restricted to segments whose
    /// far end is nearer the endpoint it touches than the other one: that is what
    /// separates "walking up to the stair" from "walking along the stairwell to
    /// the far side", which the connector's own edge must handle.
    pub fn approaches_endpoint(&self, from: DVec2, to: DVec2, tolerance_m: f64) -> bool {
        self.entries.iter().any(|entry| {
            [
                ((from - entry.a).length() <= tolerance_m).then_some((entry.a, entry.b, to)),
                ((from - entry.b).length() <= tolerance_m).then_some((entry.b, entry.a, to)),
                ((to - entry.a).length() <= tolerance_m).then_some((entry.a, entry.b, from)),
                ((to - entry.b).length() <= tolerance_m).then_some((entry.b, entry.a, from)),
            ]
            .into_iter()
            .flatten()
            .any(|(near, far, other_end)| (other_end - near).length() < (other_end - far).length())
        })
    }

    /// True when a segment *is* the planar projection of a connector link.
    pub fn matches_link(&self, from: DVec2, to: DVec2, tolerance_m: f64) -> bool {
        self.entries.iter().any(|entry| {
            let forward =
                (entry.a - from).length() <= tolerance_m && (entry.b - to).length() <= tolerance_m;
            let backward =
                (entry.b - from).length() <= tolerance_m && (entry.a - to).length() <= tolerance_m;
            forward || backward
        })
    }

    /// Connector whose footprint contains a point, if any.
    pub fn at_point(&self, point: DVec2, radius_m: f64) -> Option<&ConnectorEntry> {
        self.entries
            .iter()
            .find(|entry| entry.planar_distance(point) <= radius_m)
    }
}

/// Segment intersection test with a radius, used for footprint rejection.
fn segments_intersect(a0: DVec2, a1: DVec2, b0: DVec2, b1: DVec2, radius: f64) -> bool {
    // Cheap rejection first: bounding boxes grown by the radius.
    let a_min = a0.min(a1) - DVec2::splat(radius);
    let a_max = a0.max(a1) + DVec2::splat(radius);
    let b_min = b0.min(b1) - DVec2::splat(radius);
    let b_max = b0.max(b1) + DVec2::splat(radius);
    if a_max.x < b_min.x || b_max.x < a_min.x || a_max.y < b_min.y || b_max.y < a_min.y {
        return false;
    }

    let d1 = a1 - a0;
    let d2 = b1 - b0;
    let denominator = crate::math::cross2(d1, d2);
    if denominator.abs() > 1e-12 {
        let t = crate::math::cross2(b0 - a0, d2) / denominator;
        let u = crate::math::cross2(b0 - a0, d1) / denominator;
        if (0.0..=1.0).contains(&t) && (0.0..=1.0).contains(&u) {
            return true;
        }
    }
    // Parallel or non-crossing: fall back to the distance between the segments.
    crate::math::point_segment_distance(b0, a0, a1) <= radius
        || crate::math::point_segment_distance(b1, a0, a1) <= radius
        || crate::math::point_segment_distance(a0, b0, b1) <= radius
        || crate::math::point_segment_distance(a1, b0, b1) <= radius
}
