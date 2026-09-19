//! Planning modes.
//!
//! Three request shapes cover the intended uses: a standard start–waypoints–goal
//! route, a closed loop for track sessions, and a standard route that may be
//! redirected while it runs. All three produce the same artefacts — a smoothed
//! and arc-length parameterised path plus the waypoint behaviour the motion
//! stage needs — so the motion and sensor stages never learn which mode produced
//! the geometry.

pub mod clearance;
pub mod dynamic;
pub mod library;
pub mod loop_mode;
pub mod request;
pub mod standard;

pub use dynamic::{DynamicConfig, ReplanRecord, blend_trajectories, replan};
pub use loop_mode::{LoopConfig, laps_for_duration, plan_loop};
pub use request::{Checkpoint, LoopRequest, PlanMode, StandardRequest, ViaSemantics, Waypoint};
pub use standard::{Leg, PlannedRoute, RouteConfig, distance_to_path, plan, project_onto_path};
