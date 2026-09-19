//! Planning requests: what the caller asks for.
//!
//! Waypoints carry a *behaviour*, not just a position. Without it, "how does the
//! runner leave a waypoint" is ambiguous and the generated trajectory shows an
//! unnatural speed feature at the point. The three semantics cover the cases the
//! design names: a direction anchor, a junction the runner slows for, and a
//! checkpoint where they stop.

use glam::DVec2;

use serde::{Deserialize, Serialize};

/// Behaviour at a waypoint.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum ViaSemantics {
    /// Pass through at speed.
    Pass,
    /// Slow to a fraction of the local limit while inside the waypoint radius.
    Slow,
    /// Stop for a duration.
    Dwell {
        /// Time spent at the waypoint, seconds.
        duration_s: f64,
    },
}

impl ViaSemantics {
    /// Speed factor applied inside the waypoint radius.
    pub fn speed_factor(&self) -> f64 {
        match self {
            ViaSemantics::Pass => 1.0,
            ViaSemantics::Slow => 0.65,
            ViaSemantics::Dwell { .. } => 0.0,
        }
    }

    /// Dwell duration, zero for the other semantics.
    pub fn dwell_s(&self) -> f64 {
        match self {
            ViaSemantics::Dwell { duration_s } => duration_s.max(0.0),
            _ => 0.0,
        }
    }
}

/// A point the route must visit.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Waypoint {
    /// Position in the local plane.
    pub position: DVec2,
    /// Behaviour on arrival.
    pub semantics: ViaSemantics,
    /// Radius of the affected window, metres.
    pub radius_m: f64,
}

impl Waypoint {
    /// Creates a waypoint with the default radius.
    pub fn new(position: DVec2) -> Self {
        Self {
            position,
            semantics: ViaSemantics::Pass,
            radius_m: 5.0,
        }
    }

    /// Sets the behaviour.
    pub fn with_semantics(mut self, semantics: ViaSemantics) -> Self {
        self.semantics = semantics;
        self
    }

    /// Sets the affected radius.
    pub fn with_radius(mut self, radius_m: f64) -> Self {
        self.radius_m = radius_m.max(0.5);
        self
    }
}

/// Start, waypoints and goal of a standard run.
#[derive(Debug, Clone, PartialEq)]
pub struct StandardRequest {
    /// Where the run starts.
    pub start: DVec2,
    /// Ordered points the route visits.
    pub waypoints: Vec<Waypoint>,
    /// Where the run ends.
    pub goal: DVec2,
}

impl StandardRequest {
    /// Creates a request from start to goal.
    pub fn new(start: DVec2, goal: DVec2) -> Self {
        Self {
            start,
            waypoints: Vec::new(),
            goal,
        }
    }

    /// Adds a waypoint.
    pub fn via(mut self, waypoint: Waypoint) -> Self {
        self.waypoints.push(waypoint);
        self
    }

    /// All mandatory points in order, including start and goal.
    pub fn legs(&self) -> Vec<DVec2> {
        let mut points = Vec::with_capacity(self.waypoints.len() + 2);
        points.push(self.start);
        points.extend(self.waypoints.iter().map(|waypoint| waypoint.position));
        points.push(self.goal);
        points
    }
}

/// Closed-loop request, used for tracks and circuits.
#[derive(Debug, Clone, PartialEq)]
pub struct LoopRequest {
    /// Where the loop starts and ends.
    pub start: DVec2,
    /// Optional far-side reference point; when absent one is chosen from the map.
    pub reference: Option<DVec2>,
    /// Number of laps to simulate.
    pub laps: usize,
}

impl LoopRequest {
    /// Creates a loop request.
    pub fn new(start: DVec2, laps: usize) -> Self {
        Self {
            start,
            reference: None,
            laps: laps.max(1),
        }
    }

    /// Sets the far-side reference point.
    pub fn with_reference(mut self, reference: DVec2) -> Self {
        self.reference = Some(reference);
        self
    }
}

/// Checkpoint inserted while a run is already in progress.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Checkpoint {
    /// Time at which the checkpoint is issued, seconds.
    pub issued_at_s: f64,
    /// Where the runner must go.
    pub position: DVec2,
}

/// Mode of a simulation request.
#[derive(Debug, Clone, PartialEq)]
pub enum PlanMode {
    /// Start, ordered waypoints, goal.
    Standard(StandardRequest),
    /// Repeated closed loop.
    Loop(LoopRequest),
    /// Standard route that may be redirected by checkpoints at runtime.
    Dynamic {
        /// Initial route.
        request: StandardRequest,
        /// Checkpoints issued in time order.
        checkpoints: Vec<Checkpoint>,
    },
}

impl PlanMode {
    /// Start position of the request.
    pub fn start(&self) -> DVec2 {
        match self {
            PlanMode::Standard(request) => request.start,
            PlanMode::Loop(request) => request.start,
            PlanMode::Dynamic { request, .. } => request.start,
        }
    }

    /// True when the route is periodic.
    pub fn is_loop(&self) -> bool {
        matches!(self, PlanMode::Loop(_))
    }

    /// Number of laps the session covers; one outside loop mode.
    pub fn laps(&self) -> usize {
        match self {
            PlanMode::Loop(request) => request.laps,
            _ => 1,
        }
    }

    /// Name of the mode, used in manifests.
    pub fn name(&self) -> &'static str {
        match self {
            PlanMode::Standard(_) => "standard",
            PlanMode::Loop(_) => "loop",
            PlanMode::Dynamic { .. } => "dynamic",
        }
    }
}
