//! Map library storage.
//!
//! Two implementations behind one trait. The in-memory store is the default and
//! drops everything at exit; the disk store keeps map images under
//! `<data_dir>/maps` and reloads them on start. Finished run summaries are
//! written next to them in disk mode, but full results — the sensor streams — stay
//! in memory: they are large, and resubmitting a run reproduces them exactly from
//! the recorded seed.

pub mod disk;
pub mod memory;

use std::sync::Arc;
use std::time::Duration;

use crate::api::dto::MapSummary;
use crate::config::{StorageConfig, StorageMode};
use crate::error::{Result, ServiceError};

pub use disk::DiskMapStore;
pub use memory::MemoryMapStore;

/// One map in the library: the image, the parsed summary and its provenance.
#[derive(Debug)]
pub struct MapEntry {
    /// Server-assigned identifier.
    pub id: String,
    /// Name the map is listed under.
    pub name: String,
    /// How it entered the library: `import`, `synthetic` or `inline`.
    pub source: String,
    /// Creation time, Unix milliseconds.
    pub created_at_ms: i64,
    /// Summary built when the map was added, so listing does not parse images.
    pub summary: MapSummary,
    /// The OMF image.
    pub bytes: Arc<Vec<u8>>,
}

impl MapEntry {
    /// A `Map` opened over this entry's image.
    ///
    /// Opening re-validates the header, the directory and the footer hash, which
    /// costs little next to the work that follows and means a corrupted image can
    /// never be used silently.
    pub fn open(&self) -> Result<ourealis_map_format::Map> {
        Ok(ourealis_map_format::Map::from_bytes(
            self.bytes.as_ref().clone(),
        )?)
    }
}

/// Map library.
pub trait MapStore: Send + Sync + std::fmt::Debug {
    /// Adds a map; the id must not be in use.
    fn insert(&self, entry: MapEntry) -> Result<()>;
    /// Lists summaries, oldest first.
    fn list(&self) -> Vec<MapSummary>;
    /// Fetches one map.
    fn get(&self, id: &str) -> Result<Arc<MapEntry>>;
    /// Removes one map.
    fn remove(&self, id: &str) -> Result<()>;
    /// Identifier to use for the next map of this name.
    fn next_id(&self, name: &str) -> String;
    /// Keeps a finished run's summary, when the store persists anything.
    ///
    /// The default does nothing: a session-only library has nowhere to put it, and a
    /// result can always be reproduced from the seed in its manifest.
    fn save_run_summary(&self, _id: &str, _summary: &serde_json::Value) -> Result<()> {
        Ok(())
    }
    /// Whether the library holds an id.
    fn contains(&self, id: &str) -> bool {
        self.get(id).is_ok()
    }
}

/// Opens the store the configuration asks for.
pub fn open(config: &StorageConfig) -> Result<Arc<dyn MapStore>> {
    match config.mode {
        StorageMode::Memory => Ok(Arc::new(MemoryMapStore::new(config.keep_maps))),
        StorageMode::Disk => Ok(Arc::new(DiskMapStore::open(&config.data_dir)?)),
    }
}

/// Builds a short, filesystem-safe identifier from a name.
///
/// Random suffixes come from the OS, so two uploads of the same name never collide
/// and an id never has to be refreshed.
pub fn identifier(name: &str) -> String {
    let mut slug: String = name
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() {
                character.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect();
    slug = slug.trim_matches('-').to_string();
    if slug.len() > 32 {
        slug.truncate(32);
        slug = slug.trim_end_matches('-').to_string();
    }
    if slug.is_empty() {
        slug = "map".to_string();
    }
    let suffix = uuid::Uuid::new_v4().simple().to_string();
    format!("{slug}-{}", &suffix[..8])
}

/// Rejects an identifier that could name a file outside the data directory.
///
/// Map ids arrive from request paths and request bodies, both of which are decoded
/// by the facade: a percent-encoded `%2F` becomes a separator again before the id
/// reaches the store, and `PathBuf::join` with an absolute or `..`-bearing id
/// discards the data directory entirely. Only the alphabet [`identifier`] produces
/// is accepted, so the two ends of the pipeline agree by construction.
pub fn validate_id(id: &str) -> Result<()> {
    let acceptable = !id.is_empty()
        && id.len() <= 96
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    if acceptable {
        return Ok(());
    }
    Err(ServiceError::Invalid(format!(
        "{id:?} is not a usable map identifier; identifiers are built from letters, digits, '-' and '_'"
    )))
}

/// Timestamp used for library entries and files.
pub fn created_at_ms() -> i64 {
    crate::api::time::now_unix_ms()
}

/// Age of an entry, for the disk store's staleness checks.
pub fn age_of(created_at_ms: i64) -> Duration {
    let now = crate::api::time::now_unix_ms();
    Duration::from_millis(now.saturating_sub(created_at_ms).max(0) as u64)
}
