//! Unit tests for the motion mode enum (included from `src/motion.rs`).

use super::*;

#[test]
fn identifiers_roundtrip() {
    for mode in MotionMode::ALL {
        assert_eq!(MotionMode::from_u8(mode.as_u8()), Some(mode));
    }
}

#[test]
fn unknown_identifier_is_rejected() {
    assert_eq!(MotionMode::from_u8(200), None);
}

#[test]
fn identifiers_are_stable() {
    assert_eq!(MotionMode::Jog.as_u8(), 0);
    assert_eq!(MotionMode::Moderate.as_u8(), 1);
    assert_eq!(MotionMode::Race.as_u8(), 2);
}
