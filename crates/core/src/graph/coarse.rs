//! Coarse cells taken from the map's quadtree skeleton.
//!
//! The map partitions space into a quadtree whose leaves are uniform, direction
//! free blocks of 5–20 m. Those leaves are the map's own statement of "this area
//! is described at coarse granularity", and the design puts them in the search
//! graph as nodes: an open lawn, a plaza or a track infield does not need a metre
//! grid, and collapsing it to one node per block is what keeps the mixed graph
//! small.
//!
//! Two rules come from the format and are enforced here:
//!
//! * a leaf is only usable at coarse granularity when its aggregate **maximum** is
//!   below the passable threshold — the mean alone can hide an obstacle inside the
//!   block, which is exactly why the format stores both values;
//! * a leaf carrying the drill-down hint, a direction constraint, or a
//!   suspect-impassable flag is not used at all, so the fine grid continues to
//!   describe it.

use std::collections::HashMap;

use glam::DVec2;

use ourealis_map_format::quadtree::QNode;
use ourealis_map_format::tlv::value::AggregationRules;
use ourealis_map_format::{Aabb, Map};

use crate::error::Result;
use crate::field::HardMask;
use crate::graph::connectors::{ConnectorSet, FOOTPRINT_RADIUS_M};
use crate::terrain::Grid2D;

/// Tunables of the coarse layer.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CoarseOptions {
    /// Whether skeleton leaves join the search graph.
    pub enabled: bool,
    /// Largest admitted leaf side, metres.
    ///
    /// A leaf bigger than this is described at a granularity the finer stages
    /// cannot refine — the lateral offset, the safety radius and the corridor
    /// width all live at metre scale — so it is left to the fine grid.
    pub max_size_m: f64,
    /// Aggregate maximum above which a leaf counts as needing refinement.
    ///
    /// Compared against the dequantised proxy value, which for the default proxy
    /// (the hard-constraint layer) is a fraction of forbidden cells in the block.
    pub max_forbidden_fraction: f64,
    /// Smallest block the drill-down will produce, metres.
    ///
    /// A block flagged for refinement is quartered rather than dropped, and the
    /// quarters that are uniform are kept. The recursion stops here: below it the
    /// fine grid describes the area better, and continuing would fill the layer
    /// with blocks too small to be worth a node.
    pub min_drill_size_m: f64,
}

impl Default for CoarseOptions {
    fn default() -> Self {
        Self {
            enabled: true,
            max_size_m: 20.0,
            max_forbidden_fraction: 0.0,
            min_drill_size_m: 4.0,
        }
    }
}

/// One admissible skeleton leaf.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CoarseCell {
    /// Centre of the block, metres.
    pub center: DVec2,
    /// Extent of the block, metres.
    pub bounds: Aabb,
    /// Depth in the quadtree.
    pub depth: u8,
    /// Dequantised aggregate mean of the proxy channel.
    pub aggr_mean: f64,
    /// Dequantised aggregate maximum of the proxy channel.
    pub aggr_max: f64,
}

impl CoarseCell {
    /// Side length, metres.
    pub fn size_m(&self) -> f64 {
        self.bounds.width()
    }
}

/// The admissible leaves, with a bucket index for position lookup.
#[derive(Debug, Clone, Default)]
pub struct CoarseGrid {
    cells: Vec<CoarseCell>,
    buckets: HashMap<(i32, i32), Vec<u32>>,
    bucket_size_m: f64,
}

impl CoarseGrid {
    /// Builds the layer from a map's skeleton.
    ///
    /// Returns an empty layer when the map carries no skeleton, which is the
    /// documented meaning of `node_count == 0` rather than an error: a map
    /// without a partition is described entirely at fine granularity.
    pub fn from_map(map: &Map, options: &CoarseOptions) -> Result<Self> {
        let connectors = ConnectorSet::new(&map.connectors()?.unwrap_or_default());
        Self::from_map_with_connectors(map, options, &connectors)
    }

    /// Builds the layer with an explicit constraint mask.
    ///
    /// The mask decides which quarters of a flagged block are uniform: the same
    /// predicate the map's own aggregate maximum encodes, asked at the granularity
    /// the refinement needs.
    pub fn from_map_refined(
        map: &Map,
        options: &CoarseOptions,
        hard: &HardMask,
        connectors: &ConnectorSet,
    ) -> Result<Self> {
        Self::build(map, options, hard, connectors)
    }

    /// Builds the layer, excluding blocks that a connector's footprint reaches.
    pub fn from_map_with_connectors(
        map: &Map,
        options: &CoarseOptions,
        connectors: &ConnectorSet,
    ) -> Result<Self> {
        let hard = HardMask::from_map(map)?;
        Self::build(map, options, &hard, connectors)
    }

    fn build(
        map: &Map,
        options: &CoarseOptions,
        hard: &HardMask,
        connectors: &ConnectorSet,
    ) -> Result<Self> {
        if !options.enabled || map.skeleton().is_empty() {
            return Ok(Self::default());
        }
        let rules = map.aggregation_rules()?;
        let bounds = map.header().bounds;
        let base_res = map.header().base_res_m();
        let mut cells = Vec::new();
        for node in map.skeleton() {
            let Some(cell) = cell_of(node, &rules, bounds, base_res) else {
                continue;
            };
            let size_m = cell.bounds.width().max(cell.bounds.height());
            if admissible(node, &rules, options, size_m) {
                cells.push(cell);
            } else if refines(node, &rules, options) {
                // The block is not uniform enough to stand for its whole area, but
                // parts of it may be: quarter it and keep the quarters that are,
                // which is the design's "refine on entry" done once, up front.
                drill(&cell, options, hard, &mut cells);
            }
        }
        cells.retain(|cell| !connectors.footprint_touches(&cell.bounds, FOOTPRINT_RADIUS_M));
        Ok(Self::with_cells(cells))
    }

    /// Builds the layer from explicit cells, used by tests.
    pub fn with_cells(cells: Vec<CoarseCell>) -> Self {
        let bucket_size_m = cells
            .iter()
            .map(|cell| cell.size_m())
            .fold(8.0f64, f64::max);
        let mut layer = Self {
            cells,
            buckets: HashMap::new(),
            bucket_size_m,
        };
        layer.rebuild_index();
        layer
    }

    fn rebuild_index(&mut self) {
        self.buckets.clear();
        for (index, cell) in self.cells.iter().enumerate() {
            let (ix0, iy0) = self.bucket_of(cell.bounds.min_x, cell.bounds.min_y);
            let (ix1, iy1) = self.bucket_of(cell.bounds.max_x, cell.bounds.max_y);
            for iy in iy0..=iy1 {
                for ix in ix0..=ix1 {
                    self.buckets.entry((ix, iy)).or_default().push(index as u32);
                }
            }
        }
    }

    fn bucket_of(&self, x: f64, y: f64) -> (i32, i32) {
        (
            (x / self.bucket_size_m).floor() as i32,
            (y / self.bucket_size_m).floor() as i32,
        )
    }

    /// All cells.
    #[inline]
    pub fn cells(&self) -> &[CoarseCell] {
        &self.cells
    }

    /// Cell by index.
    #[inline]
    pub fn cell(&self, index: u32) -> Option<&CoarseCell> {
        self.cells.get(index as usize)
    }

    /// Number of cells.
    #[inline]
    pub fn len(&self) -> usize {
        self.cells.len()
    }

    /// True when the map contributed no admissible leaf.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.cells.is_empty()
    }

    /// Index of the cell containing a position.
    ///
    /// Leaves tile the open areas but not the whole map, so this returns `None`
    /// wherever the fine grid is the only description — which is also the signal
    /// the graph uses to decide which node kind a position belongs to.
    pub fn cell_at(&self, position: DVec2) -> Option<u32> {
        let bucket = self.bucket_of(position.x, position.y);
        self.buckets.get(&bucket).and_then(|candidates| {
            candidates.iter().copied().find(|index| {
                self.cells[*index as usize]
                    .bounds
                    .contains(position.x, position.y)
            })
        })
    }

    /// Nearest cell centre within `max_distance_m`.
    pub fn nearest(&self, position: DVec2, max_distance_m: f64) -> Option<u32> {
        let reach = max_distance_m + self.bucket_size_m;
        let (ix0, iy0) = self.bucket_of(position.x - reach, position.y - reach);
        let (ix1, iy1) = self.bucket_of(position.x + reach, position.y + reach);
        let mut best: Option<(u32, f64)> = None;
        for iy in iy0..=iy1 {
            for ix in ix0..=ix1 {
                let Some(indices) = self.buckets.get(&(ix, iy)) else {
                    continue;
                };
                for index in indices {
                    let distance = (self.cells[*index as usize].center - position).length();
                    if distance <= max_distance_m
                        && best.map(|(_, best)| distance < best).unwrap_or(true)
                    {
                        best = Some((*index, distance));
                    }
                }
            }
        }
        best.map(|(index, _)| index)
    }

    /// Cells adjacent to `index`, edge or corner contact included.
    ///
    /// Adjacency is by bounding-box contact rather than by Morton neighbours,
    /// because the partition is adaptive: a large leaf may border four small ones
    /// on a single side.
    pub fn neighbours(&self, index: u32) -> Vec<u32> {
        let Some(cell) = self.cell(index) else {
            return Vec::new();
        };
        let (ix0, iy0) = self.bucket_of(cell.bounds.min_x, cell.bounds.min_y);
        let (ix1, iy1) = self.bucket_of(cell.bounds.max_x, cell.bounds.max_y);
        let mut out: Vec<u32> = Vec::new();
        for iy in (iy0 - 1)..=(iy1 + 1) {
            for ix in (ix0 - 1)..=(ix1 + 1) {
                let Some(indices) = self.buckets.get(&(ix, iy)) else {
                    continue;
                };
                for other in indices {
                    if *other == index {
                        continue;
                    }
                    let candidate = self.cells[*other as usize].bounds;
                    if touches(&cell.bounds, &candidate) && !out.contains(other) {
                        out.push(*other);
                    }
                }
            }
        }
        out.sort_unstable();
        out
    }
}

/// True when two boxes touch or overlap, within a tolerance for rounding.
fn touches(a: &Aabb, b: &Aabb) -> bool {
    const TOLERANCE_M: f64 = 1e-6;
    a.max_x + TOLERANCE_M >= b.min_x
        && b.max_x + TOLERANCE_M >= a.min_x
        && a.max_y + TOLERANCE_M >= b.min_y
        && b.max_y + TOLERANCE_M >= a.min_y
}

/// Whether a skeleton node may be used as a coarse node.
fn admissible(
    node: &QNode,
    rules: &AggregationRules,
    options: &CoarseOptions,
    size_m: f64,
) -> bool {
    if !node.is_leaf() {
        return false;
    }
    // A block larger than `max_size_m` is described at a granularity the finer
    // stages cannot refine, so the fine grid keeps it; the cap is enforced here
    // rather than left to the callers of the layer.
    if !size_admitted(size_m, options) {
        return false;
    }
    // A drill hint, a direction constraint or a suspect flag all mean the block
    // is not uniform enough to be one node; the format says to refine it.
    if node.needs_drill_down()
        || node.flags & QNode::DIRECTION_CONSTRAINED != 0
        || node.flags & QNode::LIKELY_FORBIDDEN != 0
    {
        return false;
    }
    // The aggregate maximum is the one that decides: the mean can hide an
    // obstacle inside the block. Dequantisation follows the format's contract,
    // `real = raw * scale + bias`, with the parameters declared by the map.
    let maximum = dequantise(node.aggr_max, rules);
    if maximum > options.max_forbidden_fraction {
        return false;
    }
    true
}

/// Whether a node the admission rule rejected should be refined instead.
///
/// Only the drill-down hint asks for that. A direction-constrained or
/// suspect-impassable block is left to the fine grid entirely: the first is a hard
/// rule of the format, and the second says the map is not confident about the area
/// at all.
fn refines(node: &QNode, rules: &AggregationRules, options: &CoarseOptions) -> bool {
    node.is_leaf()
        && node.needs_drill_down()
        && node.flags & QNode::DIRECTION_CONSTRAINED == 0
        && node.flags & QNode::LIKELY_FORBIDDEN == 0
        && dequantise(node.aggr_max, rules) > options.max_forbidden_fraction
}

/// Quarters a block, keeping the sub-blocks that are uniform.
///
/// The recursion stops at `min_drill_size_m`: below that the fine grid describes
/// the area better than a node per quarter would.
fn drill(cell: &CoarseCell, options: &CoarseOptions, hard: &HardMask, out: &mut Vec<CoarseCell>) {
    let size = cell.size_m() * 0.5;
    if size < options.min_drill_size_m.max(1e-3) {
        return;
    }
    for (dx, dy) in [(0.0, 0.0), (1.0, 0.0), (0.0, 1.0), (1.0, 1.0)] {
        let bounds = Aabb::new(
            cell.bounds.min_x + dx * size,
            cell.bounds.min_y + dy * size,
            cell.bounds.min_x + (dx + 1.0) * size,
            cell.bounds.min_y + (dy + 1.0) * size,
        );
        let quarter = CoarseCell {
            center: DVec2::new(bounds.center().0, bounds.center().1),
            bounds,
            depth: cell.depth.saturating_add(1),
            aggr_mean: 0.0,
            aggr_max: 0.0,
        };
        let uniform = uniform_ground(&bounds, hard);
        if uniform && size_admitted(size, options) {
            out.push(quarter);
        } else if !uniform {
            drill(&quarter, options, hard, out);
        }
    }
}

/// True when a block of this side may stand for its area as one node.
///
/// The upper bound is [`CoarseOptions::max_size_m`]: past it the lateral offset
/// and safety radius live at a scale the block cannot resolve, so the area is
/// left to the fine grid. A uniform quarter above the bound is dropped for the
/// same reason rather than split further.
fn size_admitted(size_m: f64, options: &CoarseOptions) -> bool {
    size_m <= options.max_size_m.max(1e-3)
}

/// True when no cell centre inside a box is forbidden.
fn uniform_ground(bounds: &Aabb, hard: &HardMask) -> bool {
    let grid = hard.grid();
    let resolution = grid.resolution.max(1e-6);
    let origin = grid.bounds();
    let first_x = ((bounds.min_x - origin.min_x) / resolution).round();
    let first_y = ((bounds.min_y - origin.min_y) / resolution).round();
    let count = (bounds.width() / resolution).round().max(1.0) as i64;
    for iy in 0..count {
        for ix in 0..count {
            let point = DVec2::new(
                origin.min_x + (first_x + ix as f64 + 0.5) * resolution,
                origin.min_y + (first_y + iy as f64 + 0.5) * resolution,
            );
            if !grid.contains(point) {
                continue;
            }
            if !hard.is_passable(point) {
                return false;
            }
        }
    }
    true
}

/// Turns a stored aggregate into real units.
fn dequantise(raw: u16, rules: &AggregationRules) -> f64 {
    f64::from(raw) * f64::from(rules.aggr_scale) + f64::from(rules.aggr_bias)
}

/// World-space description of a skeleton node, clipped to the map.
///
/// The partition splits a square region of `next_power_of_two(max(w, h))` pixels,
/// which for a non-square map reaches past the map's edge; the cells beyond it
/// carry no proxy value and are not part of the map. Such a block is clipped to
/// the map rather than dropped, because what was certified — no forbidden cell
/// among the cells that *are* in the map — still holds for the clipped part. A
/// clip that leaves less than one grid cell is dropped: there is nothing there to
/// stand for.
fn cell_of(
    node: &QNode,
    rules: &AggregationRules,
    map_bounds: Aabb,
    base_res_m: f64,
) -> Option<CoarseCell> {
    let bounds = node
        .bounds(&map_bounds, base_res_m)
        .intersection(&map_bounds)?;
    if bounds.width() < base_res_m || bounds.height() < base_res_m {
        return None;
    }
    Some(CoarseCell {
        center: DVec2::new(bounds.center().0, bounds.center().1),
        bounds,
        depth: node.depth,
        aggr_mean: dequantise(node.aggr_mean, rules),
        aggr_max: dequantise(node.aggr_max, rules),
    })
}

/// Cells whose size exceeds the grid resolution but does not exceed `max_size_m`.
///
/// Used by callers that want to know whether the coarse layer is worth building
/// for a given map; the graph itself only needs [`CoarseGrid::cell_at`].
pub fn usable_cells(layer: &CoarseGrid, grid: &Grid2D, max_size_m: f64) -> usize {
    layer
        .cells()
        .iter()
        .filter(|cell| cell.size_m() > grid.resolution && cell.size_m() <= max_size_m)
        .count()
}
