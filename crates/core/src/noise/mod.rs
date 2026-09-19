//! The three noise layers.
//!
//! Randomness in this system is deliberately split by *role*, not by magnitude:
//!
//! * **decision** noise is the Logit temperature of [`crate::search::logit`] and
//!   acts once per path;
//! * **motion** noise lives here — the Ornstein–Uhlenbeck drifts of pace and
//!   lateral position, the step-frequency harmonics and the region events;
//! * **sensor** noise is in [`crate::sensor`], with per-device parameters.
//!
//! Mixing them, for instance by adding one Gaussian term to positions, produces
//! the wrong autocorrelation and is detectable by the evaluation metrics in
//! [`crate::eval`].

pub mod events;
pub mod harmonics;
pub mod ou;

pub use events::{ActiveEvent, EventScheduler};
pub use harmonics::{DEFAULT_HARMONIC_2_RATIO, DEFAULT_HARMONIC_3_RATIO, StepHarmonics};
pub use ou::{OuParams, OuProcess, OuVector2, OuVector3};
