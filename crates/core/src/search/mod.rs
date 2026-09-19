//! Path search and discrete choice.
//!
//! Search answers "which way"; the speed profile answers "how fast". The two are
//! deliberately separate: the search is omnidirectional and imposes no
//! kinematics, and every acceleration, curvature and fatigue limit is enforced
//! later by the profile. Keeping that split means the search stays a plain
//! graph problem with a clean optimality story.

pub mod ksp;
pub mod logit;
pub mod theta;

pub use ksp::{CandidateParams, generate_candidates, overlap_ratio, same_polyline};
pub use logit::{Candidate, CandidateSet};
pub use theta::{EdgePenalty, NoPenalty, PenaltySet, SearchConfig, SearchResult, ThetaStar};
