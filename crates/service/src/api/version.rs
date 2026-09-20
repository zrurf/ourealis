//! API path version.
//!
//! The version in the path is a policy, not a derivation: the crate version is
//! `0.x`, whose major is zero, so it cannot express an API version. Every HTTP
//! route lives under [`API_PREFIX`]; a breaking change to a resource bumps
//! [`API_VERSION`], and additive changes leave it alone.

/// Path version of the HTTP API.
pub const API_VERSION: &str = "v1";

/// Prefix every HTTP API route is mounted under.
pub const API_PREFIX: &str = "/api/v1";
