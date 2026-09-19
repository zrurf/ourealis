//! Lazy Theta* over the mixed graph.
//!
//! Theta* connects a node directly to its grandparent whenever a line of sight
//! exists, which produces any-angle paths instead of the eight-direction
//! staircase of grid A*. The "lazy" variant defers the line-of-sight test to
//! node expansion, which removes most of the checks while keeping the path
//! quality.
//!
//! Two properties of this system shape the implementation:
//!
//! * the runner is an omnidirectional agent, so any-angle shortcuts are legal
//!   and no kinematic constraints are imposed here — acceleration limits are
//!   enforced later by the speed profile;
//! * the cost field is non-uniform, so "line of sight" is a *cost-integrated*
//!   check that also rejects hard constraints and connector footprints.

use std::cmp::Ordering;
use std::collections::{BinaryHeap, HashMap, HashSet};

use glam::DVec2;
use smallvec::SmallVec;

use crate::error::{CoreError, Result};

use super::super::graph::{EdgeKind, MixedGraph, NodeId};

/// Tunables of the search.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SearchConfig {
    /// Heuristic inflation `epsilon`. Values above one trade optimality for
    /// speed with a bounded suboptimality factor.
    pub epsilon: f64,
    /// Weight of the turn penalty, in equivalent metres.
    pub turn_penalty: f64,
    /// Safety valve on node expansions.
    ///
    /// Hitting it aborts the search with [`crate::error::CoreError::NoPath`]:
    /// a truncated search has no goal to reconstruct towards, and returning a
    /// path that stops short of the goal would leave the caller with an
    /// unchecked straight jump to it.
    pub max_expansions: usize,
    /// Maximum detour of a node from the straight start-goal corridor, in
    /// metres. `None` searches the whole map.
    pub search_window: Option<f64>,
}

impl Default for SearchConfig {
    fn default() -> Self {
        Self {
            epsilon: 1.2,
            // Equivalent metres per unit of (1 - cos) heading change. Set high
            // enough that the search prefers a slightly longer smooth route over a
            // tight reversal, which the motion stage would otherwise have to turn
            // into a full stop.
            turn_penalty: 1.0,
            max_expansions: 400_000,
            search_window: None,
        }
    }
}

/// A planned path with its cost accounting.
#[derive(Debug, Clone, PartialEq)]
pub struct SearchResult {
    /// Positions from start to goal.
    pub points: Vec<DVec2>,
    /// Graph nodes the path passes through, for edge-keyed post-processing such
    /// as penalty accumulation.
    pub nodes: Vec<NodeId>,
    /// Total cost in equivalent metres.
    pub cost_equiv_m: f64,
    /// Geometric length in metres.
    pub length_m: f64,
    /// Nodes expanded before termination.
    pub expanded: usize,
    /// True when the goal was reached.
    pub reached: bool,
}

/// Multiplier applied to individual edges, used by the K-shortest enumeration.
pub trait EdgePenalty {
    /// Multiplier for the edge `from -> to`; `1.0` leaves the edge unchanged.
    fn multiplier(&self, from: NodeId, to: NodeId) -> f64;
}

/// Penalty that leaves every edge alone.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoPenalty;

impl EdgePenalty for NoPenalty {
    fn multiplier(&self, _from: NodeId, _to: NodeId) -> f64 {
        1.0
    }
}

/// Accumulated edge penalties keyed by directed edge.
#[derive(Debug, Clone, Default)]
pub struct PenaltySet {
    multipliers: HashMap<(u32, u32), f64>,
}

impl PenaltySet {
    /// Ceiling on the accumulated penalty of one edge.
    ///
    /// The multiplier exists to make a repeated route unattractive; once it is a
    /// few times the true cost, any distinct alternative that exists has already
    /// been found, and every further escalation only widens the gap between the
    /// heuristic — which still bounds the *unpenalised* cost — and the search's
    /// actual costs, so each retry expands more of the graph than the last. Twenty
    /// rounds of `1.6` reach a factor of ten thousand, which is where a leg with
    /// no room for several distinct routes turns into minutes of searching.
    pub const MAX_MULTIPLIER: f64 = 8.0;

    /// Creates an empty set.
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds `multiplier` to the penalty of a directed edge, up to
    /// [`PenaltySet::MAX_MULTIPLIER`].
    pub fn add(&mut self, from: NodeId, to: NodeId, multiplier: f64) {
        let entry = self
            .multipliers
            .entry((from.0, to.0))
            .or_insert_with(|| 1.0);
        *entry = (*entry * multiplier).min(Self::MAX_MULTIPLIER);
    }

    /// Penalty of a directed edge.
    pub fn value(&self, from: NodeId, to: NodeId) -> f64 {
        self.multipliers
            .get(&(from.0, to.0))
            .copied()
            .unwrap_or(1.0)
    }

    /// Number of penalised edges.
    pub fn len(&self) -> usize {
        self.multipliers.len()
    }

    /// True when no edge is penalised.
    pub fn is_empty(&self) -> bool {
        self.multipliers.is_empty()
    }
}

impl EdgePenalty for PenaltySet {
    fn multiplier(&self, from: NodeId, to: NodeId) -> f64 {
        self.value(from, to)
    }
}

/// Weighted value with a total order, for the priority queue.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Priority(f64);

impl Eq for Priority {}

impl Ord for Priority {
    fn cmp(&self, other: &Self) -> Ordering {
        // NaN cannot arise from the cost arithmetic; the fallback keeps the
        // ordering total if it ever does.
        other.0.partial_cmp(&self.0).unwrap_or(Ordering::Equal)
    }
}

impl PartialOrd for Priority {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// Lazy Theta* search over a [`MixedGraph`].
pub struct ThetaStar<'a, 'g> {
    graph: &'g mut MixedGraph<'a>,
    config: SearchConfig,
}

impl<'a, 'g> ThetaStar<'a, 'g> {
    /// Creates a search over `graph`.
    pub fn new(graph: &'g mut MixedGraph<'a>, config: SearchConfig) -> Self {
        Self { graph, config }
    }

    /// Plans a path between two positions.
    ///
    /// Both endpoints are snapped to the nearest passable node first, and the
    /// resulting path carries the exact requested endpoints.
    pub fn plan(&mut self, start: DVec2, goal: DVec2) -> Result<SearchResult> {
        self.plan_with_penalty(start, goal, &NoPenalty)
    }

    /// Plans a path with per-edge cost multipliers.
    pub fn plan_with_penalty(
        &mut self,
        start: DVec2,
        goal: DVec2,
        penalty: &dyn EdgePenalty,
    ) -> Result<SearchResult> {
        let start_node = self.snap(start).ok_or_else(|| {
            CoreError::unusable(start.x, start.y, "no passable node near the start")
        })?;
        let goal_node = self
            .snap(goal)
            .ok_or_else(|| CoreError::unusable(goal.x, goal.y, "no passable node near the goal"))?;

        let min_unit_cost = self.min_unit_cost();
        let mut g: HashMap<NodeId, f64> = HashMap::new();
        let mut parent: HashMap<NodeId, NodeId> = HashMap::new();
        let mut closed: HashSet<NodeId> = HashSet::new();
        let mut open: BinaryHeap<(Priority, NodeId)> = BinaryHeap::new();

        let goal_position = self.graph.position(goal_node).unwrap_or(goal);
        g.insert(start_node, 0.0);
        parent.insert(start_node, start_node);
        open.push((
            Priority(heuristic(
                self.graph.position(start_node).unwrap_or(start),
                goal_position,
                min_unit_cost,
                self.config.epsilon,
            )),
            start_node,
        ));

        let mut expanded = 0usize;
        let mut reached = false;

        while let Some((_, current)) = open.pop() {
            if current == goal_node {
                reached = true;
                break;
            }
            if !closed.insert(current) {
                continue;
            }
            expanded += 1;
            if expanded > self.config.max_expansions {
                break;
            }

            // Lazy line-of-sight repair: if the incoming shortcut is blocked,
            // fall back to the best visited neighbour, which is what makes the
            // deferred check correct.
            let current_parent = parent.get(&current).copied().unwrap_or(current);
            if current_parent != current && !self.graph.visible(current_parent, current) {
                let neighbours: SmallVec<[NodeId; 8]> = self
                    .graph
                    .neighbours(current)
                    .iter()
                    .map(|edge| edge.to)
                    .collect();
                let mut best: Option<(NodeId, f64)> = None;
                for neighbour in neighbours {
                    if !closed.contains(&neighbour) {
                        continue;
                    }
                    let Some(neighbour_g) = g.get(&neighbour).copied() else {
                        continue;
                    };
                    let Some(sight) = self.graph.line_of_sight(neighbour, current) else {
                        continue;
                    };
                    let candidate = neighbour_g + sight.cost_equiv_m;
                    if best.map(|(_, cost)| candidate < cost).unwrap_or(true) {
                        best = Some((neighbour, candidate));
                    }
                }
                if let Some((node, cost)) = best {
                    parent.insert(current, node);
                    g.insert(current, cost);
                }
            }

            let current_g = g.get(&current).copied().unwrap_or(f64::INFINITY);
            let current_position = match self.graph.position(current) {
                Some(position) => position,
                None => continue,
            };
            // Incoming heading, used by the turn penalty.
            let incoming = parent
                .get(&current)
                .and_then(|p| self.graph.position(*p))
                .map(|p| current_position - p)
                .filter(|delta| delta.length() > 1e-9);

            let edges: SmallVec<[(NodeId, f64, f64, EdgeKind); 8]> = self
                .graph
                .neighbours(current)
                .iter()
                .map(|edge| (edge.to, edge.cost_equiv_m, edge.length_m, edge.kind))
                .collect();

            for (next, base_cost, _length, kind) in edges {
                let Some(next_position) = self.graph.position(next) else {
                    continue;
                };
                if let Some(window) = self.config.search_window
                    && !within_corridor(next_position, start, goal, window)
                {
                    continue;
                }
                let mut step_cost = base_cost * penalty.multiplier(current, next);
                if kind == EdgeKind::Plane
                    && self.config.turn_penalty > 0.0
                    && let Some(incoming) = incoming
                {
                    let outgoing = next_position - current_position;
                    step_cost += turn_cost(incoming, outgoing, self.config.turn_penalty);
                }
                let tentative = current_g + step_cost;
                if tentative >= crate::field::PRUNE_THRESHOLD {
                    continue;
                }
                // Theta* update rule: prefer connecting `next` straight to
                // `current`'s parent when the line is clear, and price the
                // shortcut with *its* cost rather than the two-edge cost.
                let grandparent = parent.get(&current).copied().unwrap_or(current);
                let (new_parent, new_g) = if grandparent != current {
                    match self.graph.line_of_sight(grandparent, next) {
                        Some(shortcut) => {
                            let base = g.get(&grandparent).copied().unwrap_or(f64::INFINITY)
                                + shortcut.cost_equiv_m * penalty.multiplier(grandparent, next);
                            (grandparent, base)
                        }
                        None => (current, tentative),
                    }
                } else {
                    (current, tentative)
                };
                if !new_g.is_finite() || new_g >= crate::field::PRUNE_THRESHOLD {
                    continue;
                }
                if g.get(&next).map(|value| new_g < *value).unwrap_or(true) {
                    g.insert(next, new_g);
                    parent.insert(next, new_parent);
                    open.push((
                        Priority(
                            new_g
                                + heuristic(
                                    next_position,
                                    goal_position,
                                    min_unit_cost,
                                    self.config.epsilon,
                                ),
                        ),
                        next,
                    ));
                }
            }
        }

        if !reached {
            return Err(CoreError::NoPath {
                from_x: start.x,
                from_y: start.y,
                to_x: goal.x,
                to_y: goal.y,
            });
        }
        let end_node = goal_node;

        let mut nodes = vec![end_node];
        let mut cursor = end_node;
        let mut guard = 0usize;
        while let Some(next) = parent.get(&cursor).copied() {
            if next == cursor || guard > 1_000_000 {
                break;
            }
            nodes.push(next);
            cursor = next;
            guard += 1;
            if cursor == start_node {
                break;
            }
        }
        if nodes.last().copied() != Some(start_node) {
            return Err(CoreError::NoPath {
                from_x: start.x,
                from_y: start.y,
                to_x: goal.x,
                to_y: goal.y,
            });
        }
        nodes.reverse();

        let mut points: Vec<DVec2> = Vec::with_capacity(nodes.len() + 2);
        points.push(start);
        for node in &nodes {
            if let Some(position) = self.graph.position(*node)
                && points
                    .last()
                    .map(|last| (last - position).length() > 1e-6)
                    .unwrap_or(true)
            {
                points.push(position);
            }
        }
        if points
            .last()
            .map(|last| (last - goal).length() > 1e-6)
            .unwrap_or(true)
        {
            points.push(goal);
        }

        let mut length = 0.0;
        for window in points.windows(2) {
            length += (window[1] - window[0]).length();
        }

        // The search's `g[goal]` prices the graph edges it explored, including
        // turn penalties for corners that the shortcut rule later removed and
        // excluding the two end attachments. Reporting it as the path's cost
        // would hand the Logit choice model a number that does not belong to the
        // path it is choosing between, so the cost is recomputed from the
        // geometry that is actually returned, Z-axis links included.
        let cost_equiv_m = self
            .graph
            .polyline_cost(&points)
            .unwrap_or_else(|| g.get(&end_node).copied().unwrap_or(f64::INFINITY));

        Ok(SearchResult {
            points,
            nodes: nodes.clone(),
            cost_equiv_m,
            length_m: length,
            expanded,
            reached: true,
        })
    }

    /// Snaps a position onto a passable graph node.
    ///
    /// Prefers a node the position can actually see: the attachment segment is
    /// not part of the search, so a node behind a wall would otherwise leave an
    /// unchecked straight jump — up to the snap radius — at each end of every
    /// returned path.
    pub fn snap(&self, position: DVec2) -> Option<NodeId> {
        let direct = self.graph.nearest_plane_node(position, 25.0)?;
        if self.graph.visible_point(position, direct) {
            return Some(direct);
        }
        self.graph
            .plane_nodes_near(position, 25.0)
            .into_iter()
            .filter(|node| self.graph.visible_point(position, *node))
            .min_by(|a, b| {
                let da = self.graph.position(*a).map(|p| (p - position).length());
                let db = self.graph.position(*b).map(|p| (p - position).length());
                da.partial_cmp(&db).unwrap_or(Ordering::Equal)
            })
    }

    fn min_unit_cost(&self) -> f64 {
        let field = self.graph.cost().min_unit_cost();
        match self.graph.connectors().min_unit_cost() {
            Some(connector) => field.min(connector).max(crate::field::MIN_COST_EPSILON),
            None => field,
        }
    }
}

/// Inflated Euclidean heuristic with a positive lower bound.
fn heuristic(position: DVec2, goal: DVec2, min_unit_cost: f64, epsilon: f64) -> f64 {
    epsilon * (goal - position).length() * min_unit_cost
}

/// Turn penalty `lambda * (1 - cos(delta theta))`.
fn turn_cost(incoming: DVec2, outgoing: DVec2, lambda: f64) -> f64 {
    let (a, b) = (incoming.normalize_or_zero(), outgoing.normalize_or_zero());
    if a == DVec2::ZERO || b == DVec2::ZERO {
        return 0.0;
    }
    lambda * (1.0 - a.dot(b)).clamp(0.0, 2.0)
}

/// True when a point stays within `window` metres of the start-goal segment.
fn within_corridor(point: DVec2, start: DVec2, goal: DVec2, window: f64) -> bool {
    crate::math::point_segment_distance(point, start, goal) <= window
}
