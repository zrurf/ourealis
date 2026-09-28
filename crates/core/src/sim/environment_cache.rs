//! Reuse of loaded environments between planning tasks.
//!
//! A planning task loads an environment and the task after it usually wants the same
//! one: editing a route changes the route, not the map or the cost model. Reloading it
//! per edit is the bulk of what a replan costs, so the loaded environment is kept.
//!
//! The key is the map source together with the whole configuration, which is
//! deliberately over-specified. A missed reuse costs one load; a stale hit would plan
//! against a cost model the caller never asked for, and the preview's whole purpose is
//! to predict the run exactly.
//!
//! What is kept is bounded twice, because either bound alone is not enough. A slot count
//! alone lets four environments of a large map pile up to hundreds of megabytes, and a
//! byte budget alone would keep dozens of small ones, each dragging its map image behind
//! it. Eviction is least-recently-used on both.
//!
//! Loading happens outside the lock. Two tasks that race on a cold key both load and the
//! second one's result is kept, which is cheaper than making every other task wait.

use std::fmt;
use std::sync::Arc;

use parking_lot::Mutex;

use crate::environment::Environment;
use crate::error::Result;
use crate::sim::{MapSource, SimulationConfig};

/// Environments a cache keeps before it starts evicting.
const DEFAULT_SLOTS: usize = 4;

/// Bytes of loaded environments a cache keeps before it starts evicting.
///
/// A large map's environment is on the order of a hundred megabytes, so this holds one or
/// two of them; on a small map the slot count is what binds instead.
const DEFAULT_BUDGET_BYTES: usize = 256 << 20;

/// A bounded, least-recently-used cache of loaded environments.
pub struct EnvironmentCache {
    capacity: usize,
    budget: usize,
    entries: Mutex<Vec<Entry>>,
}

/// One cached environment and the inputs it was loaded from.
struct Entry {
    source: MapSource,
    config: SimulationConfig,
    environment: Arc<Environment>,
    /// Held with the entry, so eviction does not have to size the environment again.
    bytes: usize,
}

impl EnvironmentCache {
    /// Creates a cache holding up to `capacity` environments within the default budget.
    pub fn new(capacity: usize) -> Self {
        Self::with_budget(capacity, DEFAULT_BUDGET_BYTES)
    }

    /// Creates a cache holding up to `capacity` environments and at most `budget_bytes` of
    /// them.
    pub fn with_budget(capacity: usize, budget_bytes: usize) -> Self {
        Self {
            capacity: capacity.max(1),
            budget: budget_bytes.max(1),
            entries: Mutex::new(Vec::new()),
        }
    }

    /// Returns the environment of `source` and `config`, loading it when it is absent.
    ///
    /// `load` runs without the lock held, so a slow load does not stall other tasks.
    pub fn get_or_load(
        &self,
        source: &MapSource,
        config: &SimulationConfig,
        load: impl FnOnce() -> Result<Environment>,
    ) -> Result<Arc<Environment>> {
        if let Some(hit) = self.take(source, config) {
            return Ok(hit);
        }
        let environment = Arc::new(load()?);
        self.store(source, config, Arc::clone(&environment));
        Ok(environment)
    }

    /// Environments held.
    pub fn len(&self) -> usize {
        self.entries.lock().len()
    }

    /// Whether the cache holds nothing.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Bytes the held environments are estimated to occupy.
    pub fn bytes(&self) -> usize {
        held_bytes(&self.entries.lock())
    }

    /// Removes the matching entry and returns it, keeping it at the front of the queue.
    fn take(&self, source: &MapSource, config: &SimulationConfig) -> Option<Arc<Environment>> {
        let mut entries = self.entries.lock();
        let index = entries
            .iter()
            .position(|entry| &entry.source == source && &entry.config == config)?;
        let entry = entries.remove(index);
        let environment = Arc::clone(&entry.environment);
        entries.insert(0, entry);
        Some(environment)
    }

    /// Inserts an entry, evicting the least recently used ones until both bounds hold.
    fn store(&self, source: &MapSource, config: &SimulationConfig, environment: Arc<Environment>) {
        let bytes = environment.estimated_bytes();
        let mut entries = self.entries.lock();
        entries.insert(
            0,
            Entry {
                source: source.clone(),
                config: config.clone(),
                environment,
                bytes,
            },
        );
        entries.truncate(self.capacity);
        self.evict(&mut entries);
    }

    /// Drops from the back until the cache fits its budget.
    ///
    /// The newest entry always stays, whatever it weighs: it is the one the caller just
    /// loaded and is about to plan with, so a budget below a single environment must not
    /// evict it before it is read.
    fn evict(&self, entries: &mut Vec<Entry>) {
        while entries.len() > 1 && held_bytes(entries) > self.budget {
            entries.pop();
        }
    }
}

/// Bytes a set of entries is estimated to occupy.
fn held_bytes(entries: &[Entry]) -> usize {
    entries.iter().map(|entry| entry.bytes).sum()
}

impl Default for EnvironmentCache {
    fn default() -> Self {
        Self::new(DEFAULT_SLOTS)
    }
}

impl fmt::Debug for EnvironmentCache {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("EnvironmentCache")
            .field("capacity", &self.capacity)
            .field("held", &self.len())
            .field("bytes", &self.bytes())
            .field("budget", &self.budget)
            .finish()
    }
}
