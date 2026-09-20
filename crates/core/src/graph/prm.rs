//! PRM waypoint roadmap for open areas.
//!
//! In a uniform open space a grid search either hugs obstacles or produces a
//! bare straight line; neither resembles how people cross a plaza, where they
//! follow desire lines with a bit of individual variation. The roadmap moves
//! that randomness into preprocessing: waypoints are sampled once with a fixed
//! seed and connected where a line of sight exists, which yields slightly
//! different implicit corridors per seed while staying reproducible.

use std::collections::HashMap;

use glam::DVec2;

use ourealis_map_format::Map;
use ourealis_map_format::graph::prm::{InterfaceLink, PrmEdge, PrmGraph, PrmNode};

use crate::error::Result;
use crate::field::{CostField, CostSampler, HardMask};
use crate::rng::Rng;
use crate::terrain::Grid2D;

use super::connectors::ConnectorSet;
use super::los::line_of_sight;

/// Sampling parameters of the roadmap.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PrmParams {
    /// Mean waypoint spacing in metres.
    pub spacing_m: f64,
    /// Maximum length of a connecting edge in metres.
    pub connect_radius_m: f64,
    /// Spacing of interface points along the open/structured boundary, in metres.
    pub interface_spacing_m: f64,
    /// Local cost variance below which an area counts as open and uniform.
    pub openness_variance: f64,
}

impl Default for PrmParams {
    fn default() -> Self {
        Self {
            spacing_m: 15.0,
            connect_radius_m: 40.0,
            interface_spacing_m: 2.0,
            openness_variance: 0.01,
        }
    }
}

/// A roadmap: waypoints, undirected adjacency and interface links.
#[derive(Debug, Clone, Default)]
pub struct PrmRoadmap {
    /// Waypoint positions in the local plane.
    pub nodes: Vec<DVec2>,
    /// Elevation of each waypoint, metres.
    pub elevations: Vec<f64>,
    /// Adjacency: for each node, `(target, cost in equivalent metres, length)`
    /// for both directions (the list is symmetric).
    pub adjacency: Vec<Vec<(u32, f64, f64)>>,
    /// Interface nodes: waypoints that also connect to a fine-grid cell.
    pub interfaces: Vec<InterfaceLink>,
    /// Seed that produced this roadmap.
    pub seed: u64,
}

impl PrmRoadmap {
    /// Loads one batch from the map, verifying its fingerprint.
    pub fn from_map(map: &Map, batch: u16) -> Result<Option<Self>> {
        let Some(graph) = map.prm_graph(batch)? else {
            return Ok(None);
        };
        Ok(Some(Self::from_graph(&graph)))
    }

    /// Converts a stored graph into a roadmap.
    pub fn from_graph(graph: &PrmGraph) -> Self {
        let nodes: Vec<DVec2> = graph
            .nodes
            .iter()
            .map(|node| DVec2::new(node.position[0] as f64, node.position[1] as f64))
            .collect();
        let elevations = graph
            .nodes
            .iter()
            .map(|node| node.position[2] as f64)
            .collect();
        let mut adjacency = vec![Vec::new(); graph.nodes.len()];
        for (index, node) in graph.nodes.iter().enumerate() {
            let _ = node;
            for edge in graph.neighbours(index) {
                adjacency[index].push((edge.to, edge.cost_equiv_m as f64, edge.len_m as f64));
            }
        }
        Self {
            nodes,
            elevations,
            adjacency,
            interfaces: graph.interfaces.clone(),
            seed: graph.seed,
        }
    }

    /// Exports the roadmap in the stored graph form.
    pub fn to_graph(&self, batch: u16) -> PrmGraph {
        let nodes: Vec<PrmNode> = self
            .nodes
            .iter()
            .zip(self.elevations.iter())
            .enumerate()
            .map(|(index, (position, elevation))| {
                let flags = if self
                    .interfaces
                    .iter()
                    .any(|link| link.prm_node == index as u32)
                {
                    PrmNode::INTERFACE
                } else {
                    0
                };
                PrmNode {
                    position: [position.x as f32, position.y as f32, *elevation as f32],
                    flags,
                }
            })
            .collect();

        let mut edges: Vec<(u32, PrmEdge)> = Vec::new();
        for (from, list) in self.adjacency.iter().enumerate() {
            for (to, cost, length) in list {
                edges.push((
                    from as u32,
                    PrmEdge {
                        to: *to,
                        cost_equiv_m: *cost as f32,
                        len_m: *length as f32,
                        dir: 0,
                        flags: 0,
                    },
                ));
            }
        }
        let (offsets, edges) = PrmGraph::build_offsets(nodes.len(), &edges);
        PrmGraph {
            batch,
            seed: self.seed,
            nodes,
            offsets,
            edges,
            interfaces: self.interfaces.clone(),
        }
    }

    /// Number of waypoints.
    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    /// True when the roadmap has no waypoint.
    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    /// Adjacency list of a node.
    pub fn neighbours(&self, index: u32) -> &[(u32, f64, f64)] {
        self.adjacency
            .get(index as usize)
            .map(|list| list.as_slice())
            .unwrap_or(&[])
    }

    /// Nearest waypoint within `max_distance`, if any.
    pub fn nearest_node(&self, position: DVec2, max_distance: f64) -> Option<u32> {
        let mut best: Option<(u32, f64)> = None;
        for (index, node) in self.nodes.iter().enumerate() {
            let distance = (*node - position).length();
            if distance <= max_distance && best.map(|(_, d)| distance < d).unwrap_or(true) {
                best = Some((index as u32, distance));
            }
        }
        best.map(|(index, _)| index)
    }

    /// Grid cell an interface node attaches to, if it is one.
    pub fn interface_cell(&self, node: u32) -> Option<[u32; 2]> {
        self.interfaces
            .iter()
            .find(|link| link.prm_node == node)
            .map(|link| link.grid_cell)
    }

    /// Samples a roadmap over the open parts of the map.
    ///
    /// Points are drawn on a jittered grid and kept where the local cost field
    /// is nearly constant, which is the operational definition of "open area"
    /// used here. Candidates that fall next to a kept point become interface
    /// nodes, so the roadmap stays connected to the fine grid.
    #[allow(clippy::too_many_arguments)]
    #[allow(clippy::too_many_arguments, clippy::needless_range_loop)]
    pub fn generate(
        grid: &Grid2D,
        cost: &CostField,
        hard: &HardMask,
        connectors: &ConnectorSet,
        terrain: Option<&crate::terrain::Terrain>,
        seed: u64,
        params: &PrmParams,
    ) -> Self {
        let sampler = CostSampler::new(cost);
        let mut rng = Rng::stream(seed, crate::rng::Stream::Prm, 0, 0);
        let spacing = params.spacing_m.max(1.0);
        let bounds = grid.bounds();
        let cols = (bounds.width() / spacing).ceil().max(1.0) as usize;
        let rows = (bounds.height() / spacing).ceil().max(1.0) as usize;
        let jitter = spacing * 0.35;

        let mut nodes: Vec<DVec2> = Vec::new();
        let mut elevations: Vec<f64> = Vec::new();
        let mut open_mask = vec![vec![false; cols]; rows];

        for row in 0..rows {
            for col in 0..cols {
                let base = DVec2::new(
                    bounds.min_x + (col as f64 + 0.5) * spacing,
                    bounds.min_y + (row as f64 + 0.5) * spacing,
                );
                let point = base
                    + DVec2::new(
                        rng.uniform_range(-jitter, jitter),
                        rng.uniform_range(-jitter, jitter),
                    );
                if !grid.contains(point) || hard.is_forbidden(point) {
                    continue;
                }
                if local_cost_variance(cost, point, spacing * 0.5) > params.openness_variance {
                    continue;
                }
                open_mask[row][col] = true;
                nodes.push(point);
                elevations.push(terrain.map(|t| t.height_at(point)).unwrap_or(0.0));
            }
        }

        let mut adjacency: Vec<Vec<(u32, f64, f64)>> = vec![Vec::new(); nodes.len()];
        let radius = params.connect_radius_m;
        for i in 0..nodes.len() {
            for j in (i + 1)..nodes.len() {
                if (nodes[i] - nodes[j]).length() > radius {
                    continue;
                }
                if let Some(sight) = line_of_sight(&sampler, connectors, nodes[i], nodes[j]) {
                    adjacency[i].push((j as u32, sight.cost_equiv_m, sight.length_m));
                    adjacency[j].push((i as u32, sight.cost_equiv_m, sight.length_m));
                }
            }
        }

        // Interface points stitch the roadmap to the fine grid. The design asks
        // for them every metre or two along the boundary: the fine grid is built
        // at `base_res`, so a coarser stitch would leave the two substrates
        // connected only at a handful of places and the search would have to
        // detour to reach one.
        let interface_spacing = params.interface_spacing_m.clamp(0.25, spacing);
        let mut interfaces = Vec::new();
        for row in 0..rows {
            for col in 0..cols {
                if open_mask[row][col] {
                    continue;
                }
                let Some(towards_open) = adjacent_open(&open_mask, row as i64, col as i64) else {
                    continue;
                };
                // Only the side of the cell that faces open ground can carry an
                // interface; the other three lead into more structured ground.
                let base = DVec2::new(
                    bounds.min_x + col as f64 * spacing,
                    bounds.min_y + row as f64 * spacing,
                );
                let steps = (spacing / interface_spacing).ceil().max(1.0) as usize;
                for step in 0..=steps {
                    let t = step as f64 / steps as f64;
                    // Which edge of the structured cell the interface sits on. The
                    // side names come from `adjacent_open`, where they label the
                    // *open neighbour's* position in the mask: `North` is the row
                    // above (`row - 1`, lower y) and `South` the row below.
                    //
                    // The offsets below are one cell wide, so a `North`/`South`
                    // interface lands on a cell boundary of the map's cell grid and
                    // `Grid2D::cell_of` resolves it to the neighbouring cell. That
                    // is deliberate: mirroring them to sit on the shared edge with
                    // the open neighbour instead puts them in the forbidden edge cell
                    // of the structured band, where `is_forbidden` rejects them and
                    // the roadmap loses its link to the grid on those two
                    // orientations — measured as `NoPath` in the randomized sweep.
                    let point = match towards_open {
                        Grid4::North => base + DVec2::new(t * spacing, spacing),
                        Grid4::South => base + DVec2::new(t * spacing, 0.0),
                        Grid4::East => base + DVec2::new(spacing, t * spacing),
                        Grid4::West => base + DVec2::new(0.0, t * spacing),
                    };
                    if !grid.contains(point) || hard.is_forbidden(point) {
                        continue;
                    }
                    let nearest = nodes
                        .iter()
                        .enumerate()
                        .map(|(index, node)| (index as u32, (*node - point).length()))
                        .filter(|(_, distance)| *distance <= spacing * 1.5)
                        .min_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));
                    let Some((anchor, _)) = nearest else {
                        continue;
                    };

                    let (cell_x, cell_y) = grid.cell_of(point);
                    // The link to the roadmap is only usable when it is actually
                    // walkable: an interface point whose nearest waypoint is
                    // behind a wall must not become an edge, or the search gains a
                    // hard-constraint crossing priced at a straight-line chord.
                    let Some(sight) =
                        line_of_sight(&sampler, connectors, point, nodes[anchor as usize])
                    else {
                        continue;
                    };
                    let index = nodes.len() as u32;
                    nodes.push(point);
                    elevations.push(terrain.map(|t| t.height_at(point)).unwrap_or(0.0));
                    adjacency.push(Vec::new());
                    adjacency[index as usize].push((anchor, sight.cost_equiv_m, sight.length_m));
                    adjacency[anchor as usize].push((index, sight.cost_equiv_m, sight.length_m));
                    interfaces.push(InterfaceLink {
                        prm_node: index,
                        grid_cell: [cell_x as u32, cell_y as u32],
                        cost_equiv_m: sight.cost_equiv_m as f32,
                        len_m: sight.length_m as f32,
                    });
                }
            }
        }

        Self {
            nodes,
            elevations,
            adjacency,
            interfaces,
            seed,
        }
    }

    /// Mean edge length, a rough sanity metric for tuning the sampler.
    pub fn mean_edge_length(&self) -> f64 {
        let mut total = 0.0;
        let mut count = 0usize;
        for list in &self.adjacency {
            for (_, _, length) in list {
                total += *length;
                count += 1;
            }
        }
        if count == 0 {
            0.0
        } else {
            total / count as f64
        }
    }

    /// Number of edges (counting each undirected edge twice).
    pub fn edge_count(&self) -> usize {
        self.adjacency.iter().map(|list| list.len()).sum()
    }

    /// Lookup of waypoints by grid cell, for interface snapping.
    pub fn cell_lookup(&self, grid: &Grid2D) -> HashMap<(usize, usize), u32> {
        let mut out = HashMap::new();
        for (index, node) in self.nodes.iter().enumerate() {
            out.insert(grid.cell_of(*node), index as u32);
        }
        out
    }
}

fn adjacent_open(mask: &[Vec<bool>], row: i64, col: i64) -> Option<Grid4> {
    for (side, (dr, dc)) in [
        (Grid4::North, (-1i64, 0i64)),
        (Grid4::South, (1, 0)),
        (Grid4::West, (0, -1)),
        (Grid4::East, (0, 1)),
    ] {
        let r = row + dr;
        let c = col + dc;
        if r < 0 || c < 0 || r as usize >= mask.len() || c as usize >= mask[0].len() {
            continue;
        }
        if mask[r as usize][c as usize] {
            return Some(side);
        }
    }
    None
}

/// Which side of a cell faces the open neighbour.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Grid4 {
    North,
    South,
    East,
    West,
}

/// Variance of the cost field in a square window, used as an openness test.
fn local_cost_variance(cost: &CostField, center: DVec2, half_size: f64) -> f64 {
    let grid = cost.grid();
    let mut values = Vec::new();
    let steps = 3i64;
    for iy in -steps..=steps {
        for ix in -steps..=steps {
            let point = center
                + DVec2::new(
                    ix as f64 * half_size / steps as f64,
                    iy as f64 * half_size / steps as f64,
                );
            if !grid.contains(point) {
                continue;
            }
            let value = cost.sampled_cost_at(point);
            if value.is_finite() {
                values.push(value);
            }
        }
    }
    if values.len() < 4 {
        return f64::INFINITY;
    }
    let mean = values.iter().sum::<f64>() / values.len() as f64;
    values
        .iter()
        .map(|value| (value - mean).powi(2))
        .sum::<f64>()
        / values.len() as f64
}
