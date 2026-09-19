//! The quadtree skeleton: a Morton-sorted array of fixed-size nodes that stays
//! resident in memory and answers "at what granularity is this place
//! described?".
//!
//! The skeleton is a *linear* quadtree: a node's depth is encoded by the number
//! of significant bits in its Morton code, so no extra depth field is needed for
//! navigation (the `depth` byte is redundant convenience). Each node carries the
//! aggregated value of one proxy channel, stored as mean **and** maximum — the
//! mean alone would hide impassable cells inside a coarse cell.

use crate::bytes as le;
use crate::error::{MapError, Result};
use crate::geometry::{Aabb, node_key, node_key_depth, node_key_xy};
use crate::tlv::value::AggregationRules;

/// A skeleton node.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QNode {
    /// Linear-quadtree node key; see [`crate::geometry::node_key`].
    ///
    /// The key carries the depth in a reserved bit above the interleaved
    /// coordinates, because a bare Morton code would collide for the origin
    /// node at different depths.
    pub morton: u64,
    /// Status bits, see [`QNode::LEAF`] and friends.
    pub flags: u8,
    /// Depth, redundant with the Morton code but convenient.
    pub depth: u8,
    /// Handle of the aggregate data, resolved through the directory.
    pub tile_ref: u16,
    /// Quantised aggregate mean of the proxy channel.
    pub aggr_mean: u16,
    /// Quantised aggregate maximum of the proxy channel.
    pub aggr_max: u16,
}

/// Largest cell count per axis the quadtree side is derived from.
///
/// Two to the power of this still fits `u64`, so rounding up cannot overflow.
/// The bound is far above the `2^31` cells the 31 depth levels can address and
/// exists only so that hostile bounds/resolution pairs saturate instead of
/// wrapping `next_power_of_two`.
const MAX_SIDE_CELLS: u64 = 1 << 32;

/// Side of the square region the partition builder split, in metres.
///
/// The builder takes `next_power_of_two(max(width, height))` base pixels so that
/// every split halves both axes; this reconstructs that region from the header
/// alone. A rectangular partition cannot express the same thing: its cell count
/// per axis would not be a power of two, so a key's `(ix, iy)` could not address
/// it.
///
/// Bounds and resolution are header fields, so the computation saturates at
/// `MAX_SIDE_CELLS` instead of panicking on values whose pixel count does not
/// fit the arithmetic.
pub fn skeleton_side_m(map_bounds: &Aabb, base_res_m: f64) -> f64 {
    if !base_res_m.is_finite() || base_res_m <= 0.0 {
        return map_bounds.width().max(map_bounds.height());
    }
    let extent = map_bounds.width().max(map_bounds.height()).max(0.0);
    let cells = (extent / base_res_m).ceil().max(1.0);
    if !cells.is_finite() || cells >= MAX_SIDE_CELLS as f64 {
        return MAX_SIDE_CELLS as f64 * base_res_m;
    }
    (cells as u64).next_power_of_two() as f64 * base_res_m
}

impl QNode {
    /// Leaf node: no children with finer data.
    pub const LEAF: u8 = 1 << 0;
    /// The maximum aggregate is meaningful.
    pub const HAS_AGGR_MAX: u8 = 1 << 1;
    /// Drill-down hint: entering this coarse cell must refine it.
    pub const DRILL_HINT: u8 = 1 << 2;
    /// Likely impassable area.
    pub const LIKELY_FORBIDDEN: u8 = 1 << 3;
    /// Contains a direction-constrained area, which must never be aggregated.
    pub const DIRECTION_CONSTRAINED: u8 = 1 << 4;

    /// Serialised size in bytes.
    pub const SIZE: usize = 16;

    /// Creates a node with the given code, deriving depth from it.
    pub fn new(morton: u64, flags: u8, tile_ref: u16, aggr_mean: u16, aggr_max: u16) -> Self {
        Self {
            morton,
            flags,
            depth: node_key_depth(morton),
            tile_ref,
            aggr_mean,
            aggr_max,
        }
    }

    /// Depth implied by the key.
    #[inline]
    pub fn key_depth(&self) -> u8 {
        node_key_depth(self.morton)
    }

    /// Coordinates of the node inside its depth's grid.
    #[inline]
    pub fn key_xy(&self) -> (u32, u32) {
        node_key_xy(self.morton)
    }

    /// True when the node has no finer children.
    #[inline]
    pub fn is_leaf(&self) -> bool {
        self.flags & Self::LEAF != 0
    }

    /// True when entering this node requires drilling down.
    #[inline]
    pub fn needs_drill_down(&self) -> bool {
        self.flags & Self::DRILL_HINT != 0
    }

    /// Area covered by the node, in metres.
    ///
    /// The key's `(ix, iy)` at depth `d` indexes the `2^d` subdivision of a
    /// **square** region of `skeleton_side_cells` base pixels anchored at the map
    /// origin — the region the partition builder actually split. It is not a
    /// subdivision of the map extent: the partition covers
    /// `next_power_of_two(max(w, h))` pixels so that every split halves both axes,
    /// which for a non-square map is larger than the map itself. Nodes whose cell
    /// falls outside the map carry no data and are absent from the skeleton.
    ///
    /// `base_res_m` is the header's finest resolution; the square side is derived
    /// from it and the bounds, so the reader needs no extra field.
    pub fn bounds(&self, map_bounds: &Aabb, base_res_m: f64) -> Aabb {
        let depth = self.key_depth();
        let (ix, iy) = self.key_xy();
        let side = skeleton_side_m(map_bounds, base_res_m);
        let cell = side / 2f64.powi(depth as i32);
        Aabb::new(
            map_bounds.min_x + ix as f64 * cell,
            map_bounds.min_y + iy as f64 * cell,
            map_bounds.min_x + (ix as f64 + 1.0) * cell,
            map_bounds.min_y + (iy as f64 + 1.0) * cell,
        )
    }

    /// Serialises into the fixed layout.
    pub fn to_bytes(&self) -> [u8; Self::SIZE] {
        let mut buf = [0u8; Self::SIZE];
        let _ = le::put_u64(&mut buf, 0, self.morton);
        let _ = le::put_u8(&mut buf, 8, self.flags);
        let _ = le::put_u8(&mut buf, 9, self.depth);
        let _ = le::put_u16(&mut buf, 10, self.tile_ref);
        let _ = le::put_u16(&mut buf, 12, self.aggr_mean);
        let _ = le::put_u16(&mut buf, 14, self.aggr_max);
        buf
    }

    /// Parses the fixed layout.
    pub fn from_bytes(buf: &[u8]) -> Result<Self> {
        if buf.len() < Self::SIZE {
            return Err(MapError::Truncated {
                offset: 0,
                needed: Self::SIZE,
                available: buf.len(),
            });
        }
        Ok(Self {
            morton: le::get_u64(buf, 0)?,
            flags: le::get_u8(buf, 8)?,
            depth: le::get_u8(buf, 9)?,
            tile_ref: le::get_u16(buf, 10)?,
            aggr_mean: le::get_u16(buf, 12)?,
            aggr_max: le::get_u16(buf, 14)?,
        })
    }
}

/// Sorted skeleton node array with binary-search navigation.
#[derive(Debug, Clone, Default)]
pub struct QuadtreeSkeleton {
    nodes: Vec<QNode>,
}

impl QuadtreeSkeleton {
    /// Builds a skeleton from nodes in any order, sorting them by Morton code.
    ///
    /// Duplicate codes are rejected: two nodes for the same cell would make
    /// granularity decisions ambiguous.
    pub fn build(mut nodes: Vec<QNode>) -> Result<Self> {
        nodes.sort_unstable_by_key(|n| n.morton);
        if let Some(pair) = nodes.windows(2).find(|w| w[0].morton == w[1].morton) {
            return Err(MapError::invalid(format!(
                "duplicate quadtree node for morton code {:#x}",
                pair[0].morton
            )));
        }
        Ok(Self { nodes })
    }

    /// Builds an empty skeleton.
    pub fn empty() -> Self {
        Self { nodes: Vec::new() }
    }

    /// All nodes, sorted by Morton code.
    #[inline]
    pub fn nodes(&self) -> &[QNode] {
        &self.nodes
    }

    /// Number of nodes.
    #[inline]
    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    /// True when the skeleton has no nodes.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    /// Deepest node depth present.
    pub fn max_depth(&self) -> u8 {
        self.nodes.iter().map(|n| n.depth).max().unwrap_or(0)
    }

    /// Exact lookup of a node by Morton code.
    pub fn get(&self, morton: u64) -> Option<&QNode> {
        self.nodes
            .binary_search_by_key(&morton, |n| n.morton)
            .ok()
            .map(|i| &self.nodes[i])
    }

    /// Finds the finest node covering `(x, y)`.
    ///
    /// Walks from `max_depth` upwards and returns the first (deepest) node that
    /// exists, which is the granularity at which this location is described.
    pub fn locate(
        &self,
        x: f64,
        y: f64,
        map_bounds: &Aabb,
        base_res_m: f64,
        max_depth: u8,
    ) -> Option<&QNode> {
        if self.nodes.is_empty() || map_bounds.width() <= 0.0 || map_bounds.height() <= 0.0 {
            return None;
        }
        let side = skeleton_side_m(map_bounds, base_res_m);
        let u = (x - map_bounds.min_x) / side;
        let v = (y - map_bounds.min_y) / side;
        // The maximum edge is inside the map (`Aabb::contains` and
        // `chunk_index_at` both treat it that way), so the half-open range that
        // the index arithmetic prefers is widened by the last-cell clamp below.
        if !(0.0..=1.0).contains(&u) || !(0.0..=1.0).contains(&v) {
            return None;
        }
        for depth in (0..=max_depth.min(31)).rev() {
            let scale = 1u32 << depth;
            let ix = (u * scale as f64) as u32;
            let iy = (v * scale as f64) as u32;
            let code = node_key(ix.min(scale - 1), iy.min(scale - 1), depth);
            if let Some(node) = self.get(code) {
                return Some(node);
            }
        }
        None
    }

    /// Nodes whose area intersects `bbox`.
    pub fn query(&self, bbox: &Aabb, map_bounds: &Aabb, base_res_m: f64) -> Vec<&QNode> {
        self.nodes
            .iter()
            .filter(|n| n.bounds(map_bounds, base_res_m).intersects(bbox))
            .collect()
    }

    /// Serialises every node in Morton order.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.nodes.len() * QNode::SIZE);
        for node in &self.nodes {
            out.extend_from_slice(&node.to_bytes());
        }
        out
    }

    /// Parses a node array.
    // `array_chunks` would express this more directly but is not stable yet.
    #[allow(clippy::chunks_exact_to_as_chunks)]
    pub fn from_bytes(bytes: &[u8]) -> Result<Self> {
        if !bytes.len().is_multiple_of(QNode::SIZE) {
            return Err(MapError::invalid(format!(
                "skeleton length {} is not a multiple of {}",
                bytes.len(),
                QNode::SIZE
            )));
        }
        let mut nodes = Vec::with_capacity(bytes.len() / QNode::SIZE);
        for chunk in bytes.chunks_exact(QNode::SIZE) {
            nodes.push(QNode::from_bytes(chunk)?);
        }
        Self::build(nodes)
    }

    /// Aggregate mean of a node in real units.
    pub fn dequantise_mean(&self, node: &QNode, rules: &AggregationRules) -> f32 {
        node.aggr_mean as f32 * rules.aggr_scale + rules.aggr_bias
    }

    /// Aggregate maximum of a node in real units.
    pub fn dequantise_max(&self, node: &QNode, rules: &AggregationRules) -> f32 {
        node.aggr_max as f32 * rules.aggr_scale + rules.aggr_bias
    }

    /// Quantisation helper used by builders: maps a real aggregate value into
    /// the `u16` range of the quantisation contract declared in
    /// [`AggregationRules`].
    pub fn quantise(value: f32, rules: &AggregationRules) -> u16 {
        if rules.aggr_scale.abs() < f32::EPSILON {
            return 0;
        }
        let raw = (value - rules.aggr_bias) / rules.aggr_scale;
        raw.round().clamp(0.0, u16::MAX as f32) as u16
    }
}
