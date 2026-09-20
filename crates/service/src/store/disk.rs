//! Disk-backed map library.
//!
//! Map images are written as `<data_dir>/maps/<id>.omf`, run summaries as
//! `<data_dir>/runs/<id>.json`. On start the directory is scanned and every image
//! is parsed, so a corrupt file is reported once at startup instead of on first
//! use; a file that fails to parse is skipped with a warning rather than stopping
//! the service.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

use crate::api::dto::MapSummary;
use crate::error::{Result, ServiceError};
use crate::store::{MapEntry, MapStore};

/// Map library held on disk and cached in memory.
#[derive(Debug)]
pub struct DiskMapStore {
    root: PathBuf,
    entries: RwLock<VecDeque<Arc<MapEntry>>>,
}

impl DiskMapStore {
    /// Opens the store, creating the directory layout and loading what is there.
    pub fn open(root: &Path) -> Result<Self> {
        std::fs::create_dir_all(root.join("maps"))?;
        std::fs::create_dir_all(root.join("runs"))?;
        let store = Self {
            root: root.to_path_buf(),
            entries: RwLock::new(VecDeque::new()),
        };
        store.load()?;
        Ok(store)
    }

    /// Directory holding map images.
    pub fn maps_dir(&self) -> PathBuf {
        self.root.join("maps")
    }

    /// Directory holding run summaries.
    pub fn runs_dir(&self) -> PathBuf {
        self.root.join("runs")
    }

    /// Reads every image in `maps/`.
    fn load(&self) -> Result<()> {
        let mut entries = VecDeque::new();
        let read = std::fs::read_dir(self.maps_dir())?;
        let mut skipped = 0usize;
        for item in read {
            let path = item?.path();
            if path.extension().and_then(|value| value.to_str()) != Some("omf") {
                continue;
            }
            match load_entry(&path) {
                Ok(entry) => entries.push_back(Arc::new(entry)),
                Err(error) => {
                    skipped += 1;
                    tracing::warn!("skipping {}: {error}", path.display());
                }
            }
        }
        entries
            .make_contiguous()
            .sort_by_key(|entry| entry.created_at_ms);
        if let Ok(mut guard) = self.entries.write() {
            *guard = entries;
        }
        if skipped > 0 {
            tracing::warn!("{skipped} map image(s) could not be loaded");
        }
        Ok(())
    }

    /// Writes a run summary next to the map images.
    pub fn write_run_summary(&self, id: &str, summary: &serde_json::Value) -> Result<PathBuf> {
        crate::store::validate_id(id)?;
        let path = self.runs_dir().join(format!("{id}.json"));
        let text = serde_json::to_string_pretty(summary)?;
        std::fs::write(&path, text)?;
        Ok(path)
    }

    fn entries(&self) -> Result<std::sync::RwLockReadGuard<'_, VecDeque<Arc<MapEntry>>>> {
        self.entries
            .read()
            .map_err(|_| ServiceError::Internal("map store lock is poisoned".to_string()))
    }
}

impl MapStore for DiskMapStore {
    fn insert(&self, entry: MapEntry) -> Result<()> {
        crate::store::validate_id(&entry.id)?;
        let path = self.maps_dir().join(format!("{}.omf", entry.id));
        if path.exists() {
            return Err(ServiceError::Conflict(format!(
                "map {} is already in the library",
                entry.id
            )));
        }
        std::fs::write(&path, entry.bytes.as_ref())?;
        let stored = Arc::new(entry);
        if let Ok(mut entries) = self.entries.write() {
            entries.push_back(Arc::clone(&stored));
        }
        Ok(())
    }

    fn list(&self) -> Vec<MapSummary> {
        self.entries()
            .map(|entries| entries.iter().map(|entry| entry.summary.clone()).collect())
            .unwrap_or_default()
    }

    fn get(&self, id: &str) -> Result<Arc<MapEntry>> {
        // The cache holds what was loaded at start; an id that is not cached but
        // has a file on disk was written by another process, which is supported
        // rather than an error.
        if let Some(entry) = self.entries()?.iter().find(|entry| entry.id == id).cloned() {
            return Ok(entry);
        }
        crate::store::validate_id(id)?;
        let path = self.maps_dir().join(format!("{id}.omf"));
        if !path.is_file() {
            return Err(ServiceError::not_found(format!("map {id}")));
        }
        let entry = Arc::new(load_entry(&path)?);
        if let Ok(mut entries) = self.entries.write() {
            // Two requests may reach for the same uncached image at once; the second
            // one finds it cached by then, and pushing again would list it twice.
            if !entries.iter().any(|existing| existing.id == entry.id) {
                entries.push_back(Arc::clone(&entry));
            }
        }
        Ok(entry)
    }

    fn remove(&self, id: &str) -> Result<()> {
        crate::store::validate_id(id)?;
        let path = self.maps_dir().join(format!("{id}.omf"));
        if path.is_file() {
            std::fs::remove_file(&path)?;
        }
        let mut found = false;
        if let Ok(mut entries) = self.entries.write() {
            let before = entries.len();
            entries.retain(|entry| entry.id != id);
            found = entries.len() != before;
        }
        if !found && !path.exists() {
            return Err(ServiceError::not_found(format!("map {id}")));
        }
        Ok(())
    }

    fn save_run_summary(&self, id: &str, summary: &serde_json::Value) -> Result<()> {
        self.write_run_summary(id, summary).map(|_| ())
    }

    fn next_id(&self, name: &str) -> String {
        let mut candidate = crate::store::identifier(name);
        while self.maps_dir().join(format!("{candidate}.omf")).exists() {
            candidate = crate::store::identifier(name);
        }
        candidate
    }
}

/// Reads one image and builds its library entry.
fn load_entry(path: &Path) -> Result<MapEntry> {
    let bytes = std::fs::read(path)?;
    let id = path
        .file_stem()
        .and_then(|value| value.to_str())
        .ok_or_else(|| ServiceError::Invalid(format!("{} has no usable name", path.display())))?
        .to_string();
    let created_at_ms = std::fs::metadata(path)
        .and_then(|meta| meta.modified())
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|delta| delta.as_millis() as i64)
        .unwrap_or_else(crate::store::created_at_ms);
    let summary = crate::api::maps::summarise(&id, &bytes, "import", created_at_ms)?;
    Ok(MapEntry {
        id,
        name: summary.name.clone(),
        source: "import".to_string(),
        created_at_ms,
        summary,
        bytes: Arc::new(bytes),
    })
}
