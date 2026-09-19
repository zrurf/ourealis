//! # Ourealis core
//!
//! Human running trajectory and sensor stream simulation on a static OMF map.
//!
//! The pipeline has four stages, and each one is a module group:
//!
//! 1. **environment** — [`terrain`] and [`field`] turn the map's objective
//!    descriptions into the cost field the planner searches;
//! 2. **planning** — [`graph`] assembles the search substrate, [`search`] finds
//!    candidate routes and picks one, [`smooth`] makes it feasible;
//! 3. **motion** — [`motion`] turns geometry into a speed profile, a lateral
//!    offset, an attitude and a vertical bounce;
//! 4. **sensing** — [`noise`] and [`sensor`] derive every sensor stream from the
//!    same ground truth, so the outputs stay physically consistent with each
//!    other and with the trajectory.
//!
//! The crate-wide conventions the rest of the documentation assumes:
//!
//! * world geometry is `f64` metres in a local tangent plane ([`math::LocalFrame`]);
//! * cost is measured **per metre** and accumulates as *equivalent metres*, the
//!   unit the Logit temperature and path-size factors are calibrated in;
//! * everything stochastic draws from a stream keyed by
//!   `(seed, purpose, individual, channel)`, so a run is reproducible regardless
//!   of thread scheduling;
//! * all output text, log lines and error messages are English.

#![warn(missing_docs)]

pub mod environment;
pub mod error;
pub mod eval;
pub mod field;
pub mod gpu;
pub mod graph;
pub mod math;
pub mod motion;
pub mod noise;
pub mod path;
pub mod person;
pub mod plan;
pub mod rng;
pub mod search;
pub mod sensor;
pub mod sim;
pub mod smooth;
pub mod terrain;

pub use environment::{Environment, PrmOptions};
pub use error::{CoreError, Result};
pub use person::{PersonParams, Preset};
pub use rng::{Rng, Stream};
pub use sim::{MapSource, SimulationConfig, SimulationOutput, Simulator};
