//! Motion modes.
//!
//! A motion mode selects which static weight prior (stored in the map's
//! `WEIGHT_PRIOR` TLV) the runtime cost synthesis starts from. It is a property
//! of the *simulation*, but the map must know the identifier to key its priors,
//! so the enum lives here and is re-exported by the simulator crate.

use serde::{Deserialize, Serialize};

/// Preset running styles the map carries weight priors for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[repr(u8)]
pub enum MotionMode {
    /// Easy / recovery running: soft surfaces and gentle grades preferred.
    Jog = 0,
    /// Steady training pace.
    Moderate = 1,
    /// Racing: straight, fast lines preferred over comfort.
    Race = 2,
}

impl MotionMode {
    /// All variants, in declaration order.
    pub const ALL: [MotionMode; 3] = [MotionMode::Jog, MotionMode::Moderate, MotionMode::Race];

    /// Stable on-disk identifier.
    #[inline]
    pub const fn as_u8(self) -> u8 {
        self as u8
    }

    /// Parses an on-disk identifier.
    pub const fn from_u8(value: u8) -> Option<Self> {
        match value {
            0 => Some(MotionMode::Jog),
            1 => Some(MotionMode::Moderate),
            2 => Some(MotionMode::Race),
            _ => None,
        }
    }
}

#[cfg(test)]
#[path = "../tests/unit/motion.rs"]
mod tests;
