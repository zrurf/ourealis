//! Default values of the configuration.
//!
//! Kept as named constants rather than literals in the `Default` impls so the
//! documentation and the tests can refer to the same names, and so a change is a
//! single edit in a place that shows what it affects.

use crate::config::StorageMode;

/// Default log level.
pub const LOG_LEVEL: &str = "info";

/// Default gRPC listen address: loopback, so a default configuration is not
/// reachable from the network.
pub const RPC_LISTEN: &str = "127.0.0.1:50051";

/// Default HTTP listen address.
pub const HTTP_LISTEN: &str = "127.0.0.1:8080";

/// Default shutdown grace, seconds.
pub const SHUTDOWN_GRACE_S: f64 = 10.0;

/// Default gRPC message size limit, 16 MiB: enough for a streamed result frame,
/// small enough that a runaway request fails fast.
pub const RPC_MAX_MESSAGE_BYTES: usize = 16 * 1024 * 1024;

/// Default stream limit per gRPC connection.
pub const RPC_MAX_CONCURRENT_STREAMS: u32 = 64;

/// Default request body limit, MiB. Map images are the large bodies.
pub const HTTP_MAX_BODY_MB: usize = 256;

/// Default request timeout, seconds.
pub const HTTP_REQUEST_TIMEOUT_S: f64 = 30.0;

/// Default SSE keep-alive interval, seconds.
pub const HTTP_SSE_KEEP_ALIVE_S: f64 = 15.0;

/// Default page size cap.
pub const HTTP_MAX_PAGE_SIZE: usize = 100_000;

/// Default number of concurrent simulations.
pub const SIM_MAX_CONCURRENT: usize = 2;

/// Default queue capacity.
pub const SIM_QUEUE_CAPACITY: usize = 64;

/// Default samples per streamed frame.
pub const SIM_STREAM_FRAME_SAMPLES: usize = 50_000;

/// Default number of finished results kept.
pub const SIM_KEEP_RESULTS: usize = 32;

/// Default storage mode.
pub const STORAGE_MODE: StorageMode = StorageMode::Memory;

/// Default data directory used in disk mode.
pub const STORAGE_DATA_DIR: &str = "ourealis-data";

/// Default number of maps kept in memory.
pub const STORAGE_KEEP_MAPS: usize = 64;

/// Default cache header for hashed build outputs.
pub const WEB_CACHE_CONTROL: &str = "public, max-age=3600";
