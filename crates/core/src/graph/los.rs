//! Line-of-sight checks in a non-uniform cost field.
//!
//! In a plain grid world, "visible" means "no obstacle between". Here a straight
//! line can also cross a high-cost meadow, a hard-prohibited strip or a stair
//! well that must be traversed as a link rather than as flat ground. The check
//! therefore does three things at once: it samples the hard constraints, it
//! integrates the cost along the line, and it refuses lines that cut through a
//! connector footprint.

use glam::DVec2;

use crate::field::sampler::{CostSampler, SegmentCost};

use super::connectors::ConnectorSet;

/// Result of a line-of-sight query.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SightResult {
    /// Cost of the line in equivalent metres.
    pub cost_equiv_m: f64,
    /// Geometric length in metres.
    pub length_m: f64,
}

impl From<SegmentCost> for SightResult {
    fn from(segment: SegmentCost) -> Self {
        Self {
            cost_equiv_m: segment.cost_equiv_m,
            length_m: segment.length_m,
        }
    }
}

/// Endpoints of a line-of-sight query, independent of either endpoint being a
/// graph node.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SightQuery {
    /// Start of the segment.
    pub from: DVec2,
    /// End of the segment.
    pub to: DVec2,
}

impl SightQuery {
    /// Creates a query.
    pub fn new(from: DVec2, to: DVec2) -> Self {
        Self { from, to }
    }

    /// True when the segment is the connector link itself.
    ///
    /// Not used by [`line_of_sight`]: the link is traversed as a connector edge,
    /// and exempting a plane segment that matches it would let the search cross
    /// the stairwell at the price of the straight line. Kept for callers that
    /// need to recognise the link in a finished polyline.
    pub fn is_connector_link(&self, connectors: &ConnectorSet, tolerance_m: f64) -> bool {
        connectors.matches_link(self.from, self.to, tolerance_m)
    }
}

/// Checks a sight query against cost, hard constraints and connector footprints.
///
/// Returns `None` when the line is not traversable: it enters forbidden ground
/// or crosses a connector footprint that it is not itself part of.
pub fn line_of_sight(
    sampler: &CostSampler<'_>,
    connectors: &ConnectorSet,
    from: DVec2,
    to: DVec2,
) -> Option<SightResult> {
    let query = SightQuery::new(from, to);
    if !connections_allowed(connectors, &query) {
        return None;
    }
    sampler.segment_cost(from, to).map(SightResult::from)
}

/// Tolerance for "this segment is the connector link itself".
///
/// The exemption exists only so that a connector's own attachment segments —
/// which start *at* its endpoints — are not rejected by their own footprint.
/// It must stay far below the grid resolution: at one metre it also excused any
/// ordinary cell-to-cell edge that happened to run between the two endpoints of
/// a short connector, which let the search cross a stairwell as flat ground,
/// ignoring the connector's cost and its direction constraint.
const CONNECTOR_LINK_TOLERANCE_M: f64 = 0.05;

/// Line of sight for a segment that belongs to a connector itself.
///
/// The link and its attachment to the plane both start or end *inside* the
/// footprint, which is exactly what the footprint rejection refuses. Everything
/// else is checked: the hard constraints and the cost integral both run, so an
/// attachment cannot be drawn through a wall. Only the graph's own connector
/// edges use this, which is what keeps the exemption from becoming a way for an
/// ordinary plane edge to cross the stairwell.
pub fn connector_approach(
    sampler: &CostSampler<'_>,
    from: DVec2,
    to: DVec2,
) -> Option<SightResult> {
    sampler.segment_cost(from, to).map(SightResult::from)
}

/// Checks whether the segment may be used with respect to connector footprints.
///
/// One exemption: a segment that *approaches* an endpoint. Without it the cell
/// holding an endpoint could not be entered from any direction, and the link
/// would exist with nothing able to reach it. It is deliberately not "any segment
/// ending on an endpoint", and not "the segment from one endpoint to the other":
/// either of those lets a plane edge run along the link and cross the stairwell at
/// the price of the straight line, ignoring the link's cost and its direction
/// constraint.
///
/// The tolerance is deliberately far below the grid resolution, for the same
/// reason: at one metre an ordinary cell-to-cell edge between two cells that
/// happen to sit near the endpoints was exempted as well.
fn connections_allowed(connectors: &ConnectorSet, query: &SightQuery) -> bool {
    if connectors.is_empty() {
        return true;
    }
    if connectors.approaches_endpoint(query.from, query.to, CONNECTOR_LINK_TOLERANCE_M) {
        return true;
    }
    !connectors.crosses_footprint(query.from, query.to)
}
