//! Progress reporting for a run.
//!
//! A run is one synchronous call, so a caller that only sees [`crate::sim::Simulator::run`]
//! learns that it started and that it finished, and nothing in between. On a map of a few
//! square kilometres and a route of several kilometres that gap is minutes, and an
//! interface asked to show what a run is doing has nothing to show.
//!
//! [`RunObserver`] closes the gap without changing what a run *is*: it is a set of named
//! points along the pipeline the simulator already walks, reported as it passes them. The
//! stages are deliberately coarse — a percentage would have to be invented, and an
//! invented percentage is worse than an honest name — and the note carries whatever the
//! stage measured, so a client can show "planning the route" rather than only that
//! planning started.
//!
//! Reporting is opt-in and additive: [`crate::sim::Simulator::run`] keeps working exactly
//! as before, and an observer that is slow or panicking is the caller's problem, not the
//! run's.

use std::sync::{Arc, Mutex};

/// A named point along the run's pipeline.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum RunStage {
    /// Opening the map and reading its header.
    Map,
    /// Resolving the cost weights and building the cost, hard-constraint and sampler fields.
    Fields,
    /// Searching for a route.
    Plan,
    /// Running the motion model along the planned path.
    Motion,
    /// Generating truth and sensor samples.
    Sensors,
    /// Computing the metrics report.
    Metrics,
    /// Assembling the output.
    Done,
}

impl RunStage {
    /// Every stage, in the order a run passes through them.
    pub const ORDER: [RunStage; 7] = [
        RunStage::Map,
        RunStage::Fields,
        RunStage::Plan,
        RunStage::Motion,
        RunStage::Sensors,
        RunStage::Metrics,
        RunStage::Done,
    ];

    /// Machine-readable name, as the service reports it.
    pub const fn name(self) -> &'static str {
        match self {
            RunStage::Map => "map",
            RunStage::Fields => "fields",
            RunStage::Plan => "plan",
            RunStage::Motion => "motion",
            RunStage::Sensors => "sensors",
            RunStage::Metrics => "metrics",
            RunStage::Done => "done",
        }
    }

    /// Position in the pipeline, `0.0` at the first stage and `1.0` at the last.
    ///
    /// This is a position in a *sequence of named steps*, not a share of the work: how
    /// long a stage takes varies with the map and the route, and a route search on a
    /// coarse graph can outlast everything else combined. It is offered for the
    /// interfaces that want a bar; a client that shows stage names is reading it the
    /// honest way.
    pub const fn fraction(self) -> f64 {
        let index = match self {
            RunStage::Map => 0,
            RunStage::Fields => 1,
            RunStage::Plan => 2,
            RunStage::Motion => 3,
            RunStage::Sensors => 4,
            RunStage::Metrics => 5,
            RunStage::Done => 6,
        };
        index as f64 / (Self::ORDER.len() - 1) as f64
    }
}

impl std::fmt::Display for RunStage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.name())
    }
}

/// Something that wants to hear about a run's stages.
///
/// Implemented for `&T`, `Box<T>` and `Arc<T>` where `T` implements it, so an observer
/// can be shared with a worker thread without a lifetime parameter.
pub trait RunObserver {
    /// Called once per stage, as the run enters it.
    ///
    /// `note` is what the stage measured, and is empty when it measured nothing worth
    /// saying. It is a display string, not a stable value to parse.
    fn stage(&self, stage: RunStage, note: &str);
}

impl<T: RunObserver + ?Sized> RunObserver for &T {
    fn stage(&self, stage: RunStage, note: &str) {
        (**self).stage(stage, note)
    }
}

impl<T: RunObserver + ?Sized> RunObserver for Box<T> {
    fn stage(&self, stage: RunStage, note: &str) {
        (**self).stage(stage, note)
    }
}

impl<T: RunObserver + ?Sized> RunObserver for Arc<T> {
    fn stage(&self, stage: RunStage, note: &str) {
        (**self).stage(stage, note)
    }
}

/// An observer that discards everything, which is what [`crate::sim::Simulator::run`]
/// passes.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoObserver;

impl RunObserver for NoObserver {
    fn stage(&self, _stage: RunStage, _note: &str) {}
}

/// The stages a run passed through, collected for a caller that wants them afterwards.
///
/// Interior mutability rather than `&mut self`, because an observer is handed to a run as
/// a shared reference and the pipeline itself is sequential, so the lock is uncontended.
#[derive(Debug, Clone, Default)]
pub struct StageLog {
    entries: Arc<Mutex<Vec<(RunStage, String)>>>,
}

impl StageLog {
    /// An empty log.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The stages seen, in order, with their notes.
    #[must_use]
    pub fn entries(&self) -> Vec<(RunStage, String)> {
        self.entries
            .lock()
            .map(|entries| entries.clone())
            .unwrap_or_default()
    }

    /// The stage names seen, in order.
    #[must_use]
    pub fn names(&self) -> Vec<&'static str> {
        self.entries
            .lock()
            .map(|entries| entries.iter().map(|(stage, _)| stage.name()).collect())
            .unwrap_or_default()
    }
}

impl RunObserver for StageLog {
    fn stage(&self, stage: RunStage, note: &str) {
        if let Ok(mut entries) = self.entries.lock() {
            entries.push((stage, note.to_string()));
        }
    }
}

/// An observer that forwards each stage to a closure.
pub struct FnObserver<F>(pub F);

impl<F: Fn(RunStage, &str)> RunObserver for FnObserver<F> {
    fn stage(&self, stage: RunStage, note: &str) {
        (self.0)(stage, note)
    }
}
