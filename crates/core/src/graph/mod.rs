//! The mixed graph the planner searches.
//!
//! Four node kinds share one identifier space:
//!
//! * **grid** nodes are fine cells of the structured areas. They are never
//!   materialised — a grid node *is* its cell index, and its neighbours are
//!   derived on demand by sampling the cost field;
//! * **PRM** nodes come from the roadmap that models desire lines across open
//!   areas;
//! * **connector** nodes are the endpoints of Z-axis links, which are traversed
//!   as special edges rather than as flat ground;
//! * **interface** nodes are PRM waypoints that also attach to a grid cell, and
//!   are what stitches the two substrates into one connected graph.
//!
//! Adjacency is generated lazily and cached: building the whole graph up front
//! would cost memory proportional to the map area, while a search only ever
//! expands a thin band around the start-goal corridor.

pub mod coarse;
pub mod connectors;
pub mod los;
pub mod prm;

use std::collections::HashMap;

use glam::DVec2;

use crate::field::{CostField, CostSampler, HardMask};
use crate::terrain::Grid2D;

pub use coarse::{CoarseCell, CoarseGrid, CoarseOptions};
pub use connectors::{ConnectorEntry, ConnectorSet, ENDPOINT_TOLERANCE_M, FOOTPRINT_RADIUS_M};
pub use los::{SightResult, line_of_sight};
pub use prm::{PrmParams, PrmRoadmap};

/// Node identifier: kind in the top three bits, index below.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct NodeId(pub u32);

/// Kind of a graph node.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum NodeKind {
    /// Fine grid cell of a structured area.
    Grid = 0,
    /// PRM waypoint in an open area.
    Prm = 1,
    /// Endpoint of a Z-axis connector.
    Connector = 2,
    /// Uniform block of an open area, taken from the map's quadtree skeleton.
    Coarse = 3,
}

impl NodeId {
    /// Bits reserved for the node kind.
    pub const KIND_SHIFT: u32 = 29;
    /// Mask of the index part.
    pub const INDEX_MASK: u32 = (1 << Self::KIND_SHIFT) - 1;

    /// Grid node of a cell index.
    #[inline]
    pub const fn grid(cell: u32) -> Self {
        NodeId(cell & Self::INDEX_MASK)
    }

    /// PRM node of a waypoint index.
    #[inline]
    pub const fn prm(index: u32) -> Self {
        NodeId(((NodeKind::Prm as u32) << Self::KIND_SHIFT) | (index & Self::INDEX_MASK))
    }

    /// Connector endpoint node.
    #[inline]
    pub const fn connector(index: u32) -> Self {
        NodeId(((NodeKind::Connector as u32) << Self::KIND_SHIFT) | (index & Self::INDEX_MASK))
    }

    /// Coarse block node.
    #[inline]
    pub const fn coarse(index: u32) -> Self {
        NodeId(((NodeKind::Coarse as u32) << Self::KIND_SHIFT) | (index & Self::INDEX_MASK))
    }

    /// Kind of this node.
    #[inline]
    pub const fn kind(self) -> NodeKind {
        match self.0 >> Self::KIND_SHIFT {
            1 => NodeKind::Prm,
            2 => NodeKind::Connector,
            3 => NodeKind::Coarse,
            _ => NodeKind::Grid,
        }
    }

    /// Index inside the node kind.
    #[inline]
    pub const fn index(self) -> u32 {
        self.0 & Self::INDEX_MASK
    }

    /// True for fine grid nodes.
    #[inline]
    pub const fn is_grid(self) -> bool {
        matches!(self.kind(), NodeKind::Grid)
    }
}

/// How an edge is traversed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EdgeKind {
    /// Ordinary movement over the plane.
    Plane,
    /// A Z-axis link: fixed equivalent speed and cost.
    Connector,
}

/// A directed edge of the mixed graph.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Edge {
    /// Target node.
    pub to: NodeId,
    /// Preference-weighted cost in equivalent metres.
    pub cost_equiv_m: f64,
    /// Geometric length in metres.
    pub length_m: f64,
    /// Traversal kind.
    pub kind: EdgeKind,
}

/// Combined search substrate.
pub struct MixedGraph<'a> {
    cost: &'a CostField,
    hard: &'a HardMask,
    terrain: Option<&'a crate::terrain::Terrain>,
    connectors: ConnectorSet,
    prm: Option<PrmRoadmap>,
    coarse: CoarseGrid,
    cache: HashMap<NodeId, Vec<Edge>>,
    reference_speed: f64,
    /// Attachment table between connector endpoints and the plane network.
    attachments: ConnectorAttachments,
}

/// Attachment table between connector endpoints and the plane network.
///
/// The graph is directed, so an endpoint that is only reachable *from* the plane
/// can never be entered and a Z-axis link would exist without ever being used.
/// Both directions of the attachment are therefore materialised.
#[derive(Debug, Clone, Default)]
struct ConnectorAttachments {
    /// Plane node to the connector endpoint nodes it can reach.
    from_plane: HashMap<u32, Vec<u32>>,
    /// Connector endpoint node to the plane node it attaches to.
    from_connector: HashMap<u32, NodeId>,
}

/// Radius within which a connector endpoint attaches to the plane network.
///
/// A connector's planar projection is a stairwell or a bridge landing, so its
/// endpoints sit on the network and the nearest node is a few metres away at
/// most; the radius only has to cover a landing whose cell centre is offset from
/// the endpoint.
const CONNECTOR_ATTACH_RADIUS_M: f64 = 12.0;

impl<'a> MixedGraph<'a> {
    /// Builds the graph over a cost field.
    ///
    /// `reference_speed` converts a connector's waiting time into equivalent
    /// metres and should be the individual's target speed.
    pub fn new(
        cost: &'a CostField,
        hard: &'a HardMask,
        terrain: Option<&'a crate::terrain::Terrain>,
        connectors: ConnectorSet,
        prm: Option<PrmRoadmap>,
        reference_speed: f64,
    ) -> Self {
        Self::with_coarse(
            cost,
            hard,
            terrain,
            connectors,
            prm,
            CoarseGrid::default(),
            reference_speed,
        )
    }

    /// Builds the graph with the map's quadtree leaves as coarse nodes.
    ///
    /// A coarse node replaces the fine grid inside its block, which is what makes
    /// the mixed graph smaller than the metre grid; see [`coarse`] for the
    /// admission rules and `MixedGraph::is_covered` for the replacement.
    #[allow(clippy::too_many_arguments)]
    pub fn with_coarse(
        cost: &'a CostField,
        hard: &'a HardMask,
        terrain: Option<&'a crate::terrain::Terrain>,
        connectors: ConnectorSet,
        prm: Option<PrmRoadmap>,
        coarse: CoarseGrid,
        reference_speed: f64,
    ) -> Self {
        let mut graph = Self {
            cost,
            hard,
            terrain,
            connectors,
            prm,
            coarse,
            cache: HashMap::new(),
            reference_speed: reference_speed.max(0.1),
            attachments: ConnectorAttachments::default(),
        };
        graph.attachments = graph.build_attachments();
        graph
    }

    /// Finds the plane node each connector endpoint attaches to.
    fn build_attachments(&self) -> ConnectorAttachments {
        let mut table = ConnectorAttachments::default();
        for index in 0..self.connectors.entries().len() as u32 {
            for node_index in [index * 2, index * 2 + 1] {
                let Some(endpoint) = self.position(NodeId::connector(node_index)) else {
                    continue;
                };
                let Some(plane) = self.nearest_plane_node(endpoint, CONNECTOR_ATTACH_RADIUS_M)
                else {
                    continue;
                };
                table
                    .from_plane
                    .entry(plane.0)
                    .or_default()
                    .push(node_index);
                table.from_connector.insert(node_index, plane);
            }
        }
        table
    }

    /// Cost field the graph searches over.
    #[inline]
    pub fn cost(&self) -> &CostField {
        self.cost
    }

    /// Hard constraint mask.
    #[inline]
    pub fn hard(&self) -> &HardMask {
        self.hard
    }

    /// Grid geometry.
    #[inline]
    pub fn grid(&self) -> &Grid2D {
        self.cost.grid()
    }

    /// Connectors of the map.
    #[inline]
    pub fn connectors(&self) -> &ConnectorSet {
        &self.connectors
    }

    /// Speed a connector's waiting time is converted into distance with.
    #[inline]
    pub fn reference_speed(&self) -> f64 {
        self.reference_speed
    }

    /// Cost of a planned polyline, Z-axis links included.
    ///
    /// The cost field prices the plane, and a link is not on it: a stair has a
    /// three-dimensional length, its own unit cost and possibly a waiting time.
    /// Integrating a polyline that traverses one would price it as its horizontal
    /// chord, which is what the search refuses to do — so the traversal is
    /// re-priced here, by the link's own record.
    ///
    /// Returns `None` when any segment of the polyline crosses a hard constraint.
    pub fn polyline_cost(&self, points: &[DVec2]) -> Option<f64> {
        polyline_cost(
            &self.sampler(),
            &self.connectors,
            self.reference_speed,
            points,
        )
    }

    /// The roadmap, when the map carries one.
    #[inline]
    pub fn prm(&self) -> Option<&PrmRoadmap> {
        self.prm.as_ref()
    }

    /// The coarse layer of the map's quadtree leaves.
    #[inline]
    pub fn coarse(&self) -> &CoarseGrid {
        &self.coarse
    }

    /// Coarse block containing a position, when one does.
    ///
    /// Fine nodes inside such a block are not part of the graph: the block is one
    /// node, so nothing links to them.
    #[inline]
    pub fn coarse_at(&self, position: DVec2) -> Option<u32> {
        self.coarse.cell_at(position)
    }

    /// True when a fine cell is covered by a coarse block.
    fn is_covered(&self, cell: u32) -> bool {
        if self.coarse.is_empty() {
            return false;
        }
        let (x, y) = self.grid().coordinates(cell as usize);
        self.coarse.cell_at(self.grid().cell_center(x, y)).is_some()
    }

    /// A cost sampler sharing this graph's field.
    pub fn sampler(&self) -> CostSampler<'_> {
        CostSampler::new(self.cost)
    }

    /// Position of a node.
    pub fn position(&self, node: NodeId) -> Option<DVec2> {
        match node.kind() {
            NodeKind::Grid => {
                let (x, y) = self.grid().coordinates(node.index() as usize);
                Some(self.grid().cell_center(x, y))
            }
            NodeKind::Prm => self
                .prm
                .as_ref()
                .and_then(|prm| prm.nodes.get(node.index() as usize).copied()),
            NodeKind::Connector => {
                let (entry, a_side) = self.connector_endpoint(node.index())?;
                Some(if a_side { entry.a } else { entry.b })
            }
            NodeKind::Coarse => self.coarse.cell(node.index()).map(|cell| cell.center),
        }
    }

    /// Elevation of a node, when terrain is available.
    pub fn elevation(&self, node: NodeId) -> f64 {
        let Some(position) = self.position(node) else {
            return 0.0;
        };
        // The connector lookup indexes by `index / 2`, so it must only run for
        // connector nodes; on a grid node it would return whatever connector
        // happens to share the number.
        if node.kind() == NodeKind::Connector
            && let Some((entry, a_side)) = self.connector_endpoint(node.index())
        {
            return if a_side {
                entry.elevation_a()
            } else {
                entry.elevation_b()
            };
        }
        self.terrain
            .map(|terrain| terrain.height_at(position))
            .unwrap_or(0.0)
    }

    /// Connector entry and which endpoint a connector node refers to.
    fn connector_endpoint(&self, index: u32) -> Option<(ConnectorEntry, bool)> {
        let entry_index = (index / 2) as usize;
        let entry = *self.connectors.entries().get(entry_index)?;
        Some((entry, index.is_multiple_of(2)))
    }

    /// Neighbouring edges of a node, computed once and cached.
    pub fn neighbours(&mut self, node: NodeId) -> &[Edge] {
        if !self.cache.contains_key(&node) {
            let edges = self.compute_neighbours(node);
            self.cache.insert(node, edges);
        }
        self.cache
            .get(&node)
            .map(|edges| edges.as_slice())
            .unwrap_or(&[])
    }

    fn compute_neighbours(&self, node: NodeId) -> Vec<Edge> {
        match node.kind() {
            NodeKind::Grid => self.grid_neighbours(node),
            NodeKind::Prm => self.prm_neighbours(node),
            NodeKind::Connector => self.connector_neighbours(node),
            NodeKind::Coarse => self.coarse_neighbours(node),
        }
    }

    /// Edges from a plane node onto the connector endpoints attached to it.
    ///
    /// Priced and checked like any other plane edge except for the footprint,
    /// which the approach to a link cannot satisfy: it ends inside it.
    fn connector_edges(&self, node: NodeId, from: DVec2) -> Vec<Edge> {
        let Some(targets) = self.attachments.from_plane.get(&node.0) else {
            return Vec::new();
        };
        let sampler = self.sampler();
        targets
            .iter()
            .filter_map(|index| {
                let target = NodeId::connector(*index);
                let position = self.position(target)?;
                let sight = los::connector_approach(&sampler, from, position)?;
                Some(Edge {
                    to: target,
                    cost_equiv_m: sight.cost_equiv_m,
                    length_m: sight.length_m,
                    kind: EdgeKind::Plane,
                })
            })
            .collect()
    }

    /// Neighbours of a coarse block: the blocks touching it and the fine cells
    /// along its boundary.
    ///
    /// The block is one node, so a path enters it from outside: the boundary ring
    /// of fine cells is the stitch between the two granularities, and the blocks
    /// touching it carry the path across the open area. Every edge goes through
    /// the same line-of-sight test and cost integral as a grid edge, so the two
    /// granularities are priced on one scale.
    fn coarse_neighbours(&self, node: NodeId) -> Vec<Edge> {
        let index = node.index();
        let Some(cell) = self.coarse.cell(index) else {
            return Vec::new();
        };
        let sampler = self.sampler();
        let mut out = Vec::new();

        for other in self.coarse.neighbours(index) {
            let Some(target) = self.coarse.cell(other) else {
                continue;
            };
            // Blocks that only touch at a corner are joined through that corner,
            // which the traversal prices correctly: it walks the cells the line
            // between the two centres actually enters.
            let Some(sight) =
                los::line_of_sight(&sampler, &self.connectors, cell.center, target.center)
            else {
                continue;
            };
            out.push(Edge {
                to: NodeId::coarse(other),
                cost_equiv_m: sight.cost_equiv_m,
                length_m: sight.length_m,
                kind: EdgeKind::Plane,
            });
        }

        // The boundary ring: fine cells whose centre is outside the block but
        // whose box reaches it.
        let grid = self.grid();
        let (cx, cy) = grid.cell_of(cell.center);
        let rings = (cell.size_m() * 0.5 / grid.resolution).ceil() as isize + 1;
        for dy in -rings..=rings {
            for dx in -rings..=rings {
                let x = cx as isize + dx;
                let y = cy as isize + dy;
                if x < 0 || y < 0 || x >= grid.width as isize || y >= grid.height as isize {
                    continue;
                }
                let (x, y) = (x as usize, y as usize);
                let target = grid.index(x, y);
                if self.hard.mask()[target] {
                    continue;
                }
                let position = grid.cell_center(x, y);
                // Inside the block: the block itself stands for it.
                if cell.bounds.contains(position.x, position.y) {
                    continue;
                }
                if !touches_bounds(&cell.bounds, position, grid.resolution) {
                    continue;
                }
                let Some(sight) =
                    los::line_of_sight(&sampler, &self.connectors, cell.center, position)
                else {
                    continue;
                };
                out.push(Edge {
                    to: NodeId::grid(target as u32),
                    cost_equiv_m: sight.cost_equiv_m,
                    length_m: sight.length_m,
                    kind: EdgeKind::Plane,
                });
            }
        }
        out.extend(self.connector_edges(node, cell.center));
        out
    }

    fn grid_neighbours(&self, node: NodeId) -> Vec<Edge> {
        let grid = self.grid();
        let (x, y) = grid.coordinates(node.index() as usize);
        let from = grid.cell_center(x, y);
        let sampler = self.sampler();
        let mut out = Vec::with_capacity(8);

        if !self.coarse.is_empty() {
            let home = self.coarse.cell_at(from);
            if home.is_some() {
                // The block is the node here; the fine cells it covers are not
                // part of the graph, so this one has no edges of its own. Entering
                // and leaving happens through the block's own neighbourhood.
                return out;
            }
            // A cell outside every block links to the blocks whose box it touches,
            // which is the fine side of the same stitch.
            for (other, block) in self.coarse.cells().iter().enumerate() {
                if !touches_bounds(&block.bounds, from, grid.resolution) {
                    continue;
                }
                let Some(sight) =
                    los::line_of_sight(&sampler, &self.connectors, from, block.center)
                else {
                    continue;
                };
                out.push(Edge {
                    to: NodeId::coarse(other as u32),
                    cost_equiv_m: sight.cost_equiv_m,
                    length_m: sight.length_m,
                    kind: EdgeKind::Plane,
                });
            }
        }

        for (dx, dy) in [
            (1i64, 0i64),
            (-1, 0),
            (0, 1),
            (0, -1),
            (1, 1),
            (1, -1),
            (-1, 1),
            (-1, -1),
        ] {
            let nx = x as i64 + dx;
            let ny = y as i64 + dy;
            if nx < 0 || ny < 0 || nx >= grid.width as i64 || ny >= grid.height as i64 {
                continue;
            }
            let (nx, ny) = (nx as usize, ny as usize);
            let target = grid.index(nx, ny);
            if self.hard.mask()[target] {
                continue;
            }
            let to = grid.cell_center(nx, ny);
            let Some(sight) = los::line_of_sight(&sampler, &self.connectors, from, to) else {
                continue;
            };
            out.push(Edge {
                to: NodeId::grid(target as u32),
                cost_equiv_m: sight.cost_equiv_m,
                length_m: sight.length_m,
                kind: EdgeKind::Plane,
            });
        }

        // Grid cells that host an interface node link into the roadmap. The node
        // sits near the cell centre but not exactly on it, so the link is priced
        // by the same line-of-sight test as every other plane edge; charging the
        // stored anchor cost here would bill the waypoint link twice and skip
        // the hard-constraint check.
        if let Some(prm) = &self.prm {
            for link in &prm.interfaces {
                let [cx, cy] = link.grid_cell;
                if cx as usize != x || cy as usize != y {
                    continue;
                }
                let Some(position) = prm.nodes.get(link.prm_node as usize).copied() else {
                    continue;
                };
                let Some(sight) = los::line_of_sight(&sampler, &self.connectors, from, position)
                else {
                    continue;
                };
                out.push(Edge {
                    to: NodeId::prm(link.prm_node),
                    cost_equiv_m: sight.cost_equiv_m,
                    length_m: sight.length_m,
                    kind: EdgeKind::Plane,
                });
            }
        }

        out.extend(self.connector_edges(node, from));
        out
    }

    fn prm_neighbours(&self, node: NodeId) -> Vec<Edge> {
        let Some(prm) = &self.prm else {
            return Vec::new();
        };
        let index = node.index();
        let mut out: Vec<Edge> = prm
            .neighbours(index)
            .iter()
            .map(|(to, cost, length)| Edge {
                to: NodeId::prm(*to),
                cost_equiv_m: *cost,
                length_m: *length,
                kind: EdgeKind::Plane,
            })
            .collect();

        // An interface waypoint also attaches to its grid cell, priced by the
        // same line-of-sight test as the grid's own edges so both directions of
        // the link cost the same and neither can cross a hard constraint.
        if let Some(cell) = prm.interface_cell(index) {
            let cell_index = cell[1] as usize * self.grid().width + cell[0] as usize;
            if cell_index < self.grid().len() && !self.hard.mask()[cell_index] {
                let target = NodeId::grid(cell_index as u32);
                if let (Some(from), Some(to)) = (
                    prm.nodes.get(index as usize).copied(),
                    self.position(target),
                ) {
                    let sampler = self.sampler();
                    if let Some(sight) = los::line_of_sight(&sampler, &self.connectors, from, to) {
                        out.push(Edge {
                            to: target,
                            cost_equiv_m: sight.cost_equiv_m,
                            length_m: sight.length_m,
                            kind: EdgeKind::Plane,
                        });
                    }
                }
            }
        }
        if let Some(from) = prm.nodes.get(index as usize).copied() {
            out.extend(self.connector_edges(node, from));
        }
        out
    }

    fn connector_neighbours(&self, node: NodeId) -> Vec<Edge> {
        let Some((entry, a_side)) = self.connector_endpoint(node.index()) else {
            return Vec::new();
        };
        let mut out = Vec::new();

        // The link itself, in the direction the connector allows.
        let cost = if a_side {
            entry.cost_a_to_b(self.reference_speed)
        } else {
            entry.cost_b_to_a(self.reference_speed)
        };
        if let Some(cost) = cost {
            let other = if a_side {
                node.index() + 1
            } else {
                node.index() - 1
            };
            out.push(Edge {
                to: NodeId::connector(other),
                cost_equiv_m: cost,
                length_m: entry.connector.length_3d() as f64,
                kind: EdgeKind::Connector,
            });
        }

        // The endpoint attaches to the plane at the node the attachment table
        // picked, so the link is symmetric: whatever can reach the endpoint can
        // also be reached from it. The segment is checked and priced like a plane
        // edge, minus the footprint rejection its own approach cannot satisfy.
        let position = if a_side { entry.a } else { entry.b };
        if let Some(plane) = self.attachments.from_connector.get(&node.index()).copied()
            && let Some(target_position) = self.position(plane)
        {
            let sampler = self.sampler();
            if let Some(sight) = los::connector_approach(&sampler, position, target_position) {
                out.push(Edge {
                    to: plane,
                    cost_equiv_m: sight.cost_equiv_m,
                    length_m: sight.length_m,
                    kind: EdgeKind::Plane,
                });
            }
        }
        out
    }

    /// Nearest passable grid or PRM node to a position.
    pub fn nearest_plane_node(&self, position: DVec2, max_radius_m: f64) -> Option<NodeId> {
        // A position inside a coarse block belongs to that block: the fine cells
        // it covers are not part of the graph, so snapping to one would return a
        // node with no edges.
        if let Some(index) = self.coarse.cell_at(position) {
            return Some(NodeId::coarse(index));
        }
        if let Some(prm) = &self.prm
            && let Some(index) = prm.nearest_node(position, max_radius_m)
        {
            return Some(NodeId::prm(index));
        }
        let cell = self.nearest_passable_cell(position, max_radius_m)?;
        Some(NodeId::grid(cell))
    }

    /// Nearest passable cell index, searching outwards from the containing cell.
    ///
    /// The nearest one, not the first one the ring order reaches: the snap is the
    /// position every path is attached to, so an arbitrary pick among equally
    /// distant cells would put a visible kink in the start of the run.
    pub fn nearest_passable_cell(&self, position: DVec2, max_radius_m: f64) -> Option<u32> {
        let grid = self.grid();
        let (cx, cy) = grid.cell_of(position);
        let max_rings = (max_radius_m / grid.resolution).ceil().max(1.0) as isize;
        let mut best: Option<(f64, u32)> = None;
        for ring in 0..=max_rings {
            // Everything from this ring outwards is at least `ring - 1` cells
            // away, so a hit closer than that cannot be beaten.
            if let Some((best_distance, _)) = best
                && ((ring as f64 - 1.0) * grid.resolution) > best_distance
            {
                break;
            }
            for dy in -ring..=ring {
                for dx in -ring..=ring {
                    if dx.abs() != ring && dy.abs() != ring {
                        continue;
                    }
                    let x = cx as isize + dx;
                    let y = cy as isize + dy;
                    if x < 0 || y < 0 || x >= grid.width as isize || y >= grid.height as isize {
                        continue;
                    }
                    let (x, y) = (x as usize, y as usize);
                    let index = grid.index(x, y);
                    if self.hard.mask()[index] || self.is_covered(index as u32) {
                        continue;
                    }
                    let distance = (grid.cell_center(x, y) - position).length();
                    if best
                        .as_ref()
                        .map(|(best_distance, _)| distance < *best_distance)
                        .unwrap_or(true)
                    {
                        best = Some((distance, index as u32));
                    }
                }
            }
        }
        best.map(|(_, index)| index)
    }

    /// Line of sight between two nodes.
    pub fn line_of_sight(&self, from: NodeId, to: NodeId) -> Option<SightResult> {
        let from_position = self.position(from)?;
        let to_position = self.position(to)?;
        los::line_of_sight(
            &self.sampler(),
            &self.connectors,
            from_position,
            to_position,
        )
    }

    /// True when a straight move between the two nodes is legal.
    pub fn visible(&self, from: NodeId, to: NodeId) -> bool {
        self.line_of_sight(from, to).is_some()
    }

    /// True when a straight move from a free position to a node is legal.
    pub fn visible_point(&self, from: DVec2, to: NodeId) -> bool {
        match self.position(to) {
            Some(to_position) => {
                los::line_of_sight(&self.sampler(), &self.connectors, from, to_position).is_some()
            }
            None => false,
        }
    }

    /// Plane nodes (fine grid and roadmap waypoints) within `radius_m`.
    ///
    /// Used when the nearest node is not reachable: the snap then has to pick
    /// among the candidates it can actually see.
    pub fn plane_nodes_near(&self, position: DVec2, radius_m: f64) -> Vec<NodeId> {
        let mut out = Vec::new();
        for (index, cell) in self.coarse.cells().iter().enumerate() {
            if (cell.center - position).length() <= radius_m + cell.size_m() * 0.5 {
                out.push(NodeId::coarse(index as u32));
            }
        }
        if let Some(prm) = &self.prm {
            for (index, node) in prm.nodes.iter().enumerate() {
                if (*node - position).length() <= radius_m {
                    out.push(NodeId::prm(index as u32));
                }
            }
        }
        let grid = self.grid();
        let resolution = grid.resolution.max(1e-9);
        let rings = (radius_m / resolution).ceil() as isize;
        let (cx, cy) = grid.cell_of(position);
        for dy in -rings..=rings {
            for dx in -rings..=rings {
                let x = cx as isize + dx;
                let y = cy as isize + dy;
                if x < 0 || y < 0 || x >= grid.width as isize || y >= grid.height as isize {
                    continue;
                }
                let (x, y) = (x as usize, y as usize);
                let index = grid.index(x, y);
                if self.hard.mask()[index] || self.is_covered(index as u32) {
                    continue;
                }
                if (grid.cell_center(x, y) - position).length() <= radius_m {
                    out.push(NodeId::grid(index as u32));
                }
            }
        }
        out
    }

    /// Number of cached adjacency lists, for memory diagnostics.
    pub fn cached_nodes(&self) -> usize {
        self.cache.len()
    }
}

/// Cost of a planned polyline, Z-axis links included.
///
/// Every segment is integrated against the cost field, except the ones a
/// connector traversal spans: those are charged the link's own cost — its
/// three-dimensional length, its unit cost and its waiting time — instead of the
/// horizontal chord the field would price. `reference_speed` is the speed the
/// waiting time is converted into distance with, the same one the search uses.
pub fn polyline_cost(
    sampler: &CostSampler<'_>,
    connectors: &ConnectorSet,
    reference_speed: f64,
    points: &[DVec2],
) -> Option<f64> {
    let mut total = sampler.polyline_cost(points)?.cost_equiv_m;
    for (entry_index, start, end) in connectors.traversals(points) {
        let entry = &connectors.entries()[entry_index];
        let at_a = (points[start] - entry.a).length() <= ENDPOINT_TOLERANCE_M;
        let link = if at_a {
            entry.cost_a_to_b(reference_speed)
        } else {
            entry.cost_b_to_a(reference_speed)
        };
        let (Some(link), Some(chord)) = (
            link,
            sampler
                .polyline_cost(&points[start..=end])
                .map(|cost| cost.cost_equiv_m),
        ) else {
            continue;
        };
        total += link - chord;
    }
    Some(total)
}

/// True when a position's cell touches a coarse block's box.
///
/// The test is on the cell, not the point: the stitch is between the block and
/// the fine cells along it, so a cell that reaches the box counts even if its
/// centre lies just outside.
fn touches_bounds(bounds: &ourealis_map_format::Aabb, position: DVec2, resolution: f64) -> bool {
    let half = resolution * 0.5;
    const TOLERANCE_M: f64 = 1e-6;
    bounds.max_x + half + TOLERANCE_M >= position.x - half
        && position.x + half + TOLERANCE_M >= bounds.min_x
        && bounds.max_y + half + TOLERANCE_M >= position.y - half
        && position.y + half + TOLERANCE_M >= bounds.min_y
}
