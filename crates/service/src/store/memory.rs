//! In-memory map library.
//!
//! The default: nothing survives the process. `keep_maps` bounds the library so a
//! long-lived service that keeps receiving uploads does not grow without limit;
//! the oldest entry is evicted when the bound is reached.

use std::collections::VecDeque;
use std::sync::{Arc, RwLock};

use crate::api::dto::MapSummary;
use crate::error::{Result, ServiceError};
use crate::store::{MapEntry, MapStore};

/// Session-only map library.
#[derive(Debug)]
pub struct MemoryMapStore {
    entries: RwLock<VecDeque<Arc<MapEntry>>>,
    keep: usize,
}

impl MemoryMapStore {
    /// Creates a store holding at most `keep` maps.
    pub fn new(keep: usize) -> Self {
        Self {
            entries: RwLock::new(VecDeque::new()),
            keep: keep.max(1),
        }
    }

    fn read(&self) -> Result<std::sync::RwLockReadGuard<'_, VecDeque<Arc<MapEntry>>>> {
        self.entries
            .read()
            .map_err(|_| ServiceError::Internal("map store lock is poisoned".to_string()))
    }

    fn write(&self) -> Result<std::sync::RwLockWriteGuard<'_, VecDeque<Arc<MapEntry>>>> {
        self.entries
            .write()
            .map_err(|_| ServiceError::Internal("map store lock is poisoned".to_string()))
    }
}

impl MapStore for MemoryMapStore {
    fn insert(&self, entry: MapEntry) -> Result<()> {
        let mut entries = self.write()?;
        if entries.iter().any(|existing| existing.id == entry.id) {
            return Err(ServiceError::Conflict(format!(
                "map {} is already in the library",
                entry.id
            )));
        }
        entries.push_back(Arc::new(entry));
        while entries.len() > self.keep {
            entries.pop_front();
        }
        Ok(())
    }

    fn list(&self) -> Vec<MapSummary> {
        self.read()
            .map(|entries| entries.iter().map(|entry| entry.summary.clone()).collect())
            .unwrap_or_default()
    }

    fn get(&self, id: &str) -> Result<Arc<MapEntry>> {
        self.read()?
            .iter()
            .find(|entry| entry.id == id)
            .cloned()
            .ok_or_else(|| ServiceError::not_found(format!("map {id}")))
    }

    fn remove(&self, id: &str) -> Result<()> {
        let mut entries = self.write()?;
        let before = entries.len();
        entries.retain(|entry| entry.id != id);
        if entries.len() == before {
            return Err(ServiceError::not_found(format!("map {id}")));
        }
        Ok(())
    }

    fn next_id(&self, name: &str) -> String {
        let existing: Vec<String> = self
            .read()
            .map(|entries| entries.iter().map(|entry| entry.id.clone()).collect())
            .unwrap_or_default();
        let mut candidate = crate::store::identifier(name);
        // A collision needs two uploads inside the same eight hex digits of a v4
        // UUID; regenerate rather than overwrite.
        while existing.contains(&candidate) {
            candidate = crate::store::identifier(name);
        }
        candidate
    }
}
