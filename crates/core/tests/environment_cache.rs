//! The environment cache: reuse on a hit, and eviction on either bound.
//!
//! A cache that keeps loaded environments has to pay for itself twice over — a missed
//! reuse costs a map decode and a field synthesis, and a cache that keeps too much costs
//! the memory the machine needed for the search. What has to hold is that a repeated key
//! is served without loading again, and that neither the slot count nor the byte budget
//! is exceeded, while the entry just loaded always survives.

mod fixtures;

use std::sync::atomic::{AtomicUsize, Ordering};

use ourealis_core::environment::Environment;
use ourealis_core::sim::{EnvironmentCache, MapSource, SimulationConfig};

/// A loader that counts how often it runs, so a hit can be told from a miss.
fn counting_loader(loads: &AtomicUsize) -> impl Fn() -> ourealis_core::Result<Environment> + Copy {
    move || {
        loads.fetch_add(1, Ordering::Relaxed);
        Ok(fixtures::flat_environment(&[]))
    }
}

#[test]
fn a_repeated_key_is_served_without_loading_again() {
    let cache = EnvironmentCache::default();
    let source = MapSource::omf("campus.omf");
    let config = SimulationConfig::default();
    let loads = AtomicUsize::new(0);

    cache
        .get_or_load(&source, &config, counting_loader(&loads))
        .expect("first load");
    cache
        .get_or_load(&source, &config, counting_loader(&loads))
        .expect("second load");

    assert_eq!(loads.load(Ordering::Relaxed), 1);
    assert_eq!(cache.len(), 1);
}

#[test]
fn the_slot_count_bounds_how_many_environments_are_kept() {
    let cache = EnvironmentCache::new(2);
    let config = SimulationConfig::default();

    for name in ["a.omf", "b.omf", "c.omf"] {
        cache
            .get_or_load(&MapSource::omf(name), &config, || {
                Ok(fixtures::flat_environment(&[]))
            })
            .expect("load");
    }

    assert_eq!(cache.len(), 2);
}

#[test]
fn the_budget_drops_the_least_recently_used_environment() {
    let one = fixtures::flat_environment(&[]).estimated_bytes();
    // Room for two environments and no more: the third insert has to evict the oldest.
    let cache = EnvironmentCache::with_budget(4, 2 * one);
    let config = SimulationConfig::default();

    for name in ["a.omf", "b.omf", "c.omf"] {
        cache
            .get_or_load(&MapSource::omf(name), &config, || {
                Ok(fixtures::flat_environment(&[]))
            })
            .expect("load");
    }

    assert_eq!(cache.len(), 2);
    assert!(cache.bytes() <= 2 * one);
}

#[test]
fn an_environment_larger_than_the_whole_budget_is_still_kept() {
    let one = fixtures::flat_environment(&[]).estimated_bytes();
    // A budget below a single environment: evicting the only entry would leave the caller
    // holding an environment the cache can never reuse.
    let cache = EnvironmentCache::with_budget(4, one / 2);
    let config = SimulationConfig::default();

    cache
        .get_or_load(&MapSource::omf("campus.omf"), &config, || {
            Ok(fixtures::flat_environment(&[]))
        })
        .expect("load");

    assert_eq!(cache.len(), 1);
}

#[test]
fn a_reused_key_moves_to_the_front_of_the_queue() {
    let one = fixtures::flat_environment(&[]).estimated_bytes();
    let cache = EnvironmentCache::with_budget(4, 2 * one);
    let config = SimulationConfig::default();
    let load = || Ok(fixtures::flat_environment(&[]));

    for name in ["a.omf", "b.omf"] {
        cache
            .get_or_load(&MapSource::omf(name), &config, load)
            .expect("load");
    }
    // Reading `a` again makes `b` the least recently used, so `c` must evict `b`.
    cache
        .get_or_load(&MapSource::omf("a.omf"), &config, load)
        .expect("hit");
    cache
        .get_or_load(&MapSource::omf("c.omf"), &config, load)
        .expect("load");

    assert_eq!(cache.len(), 2);
    let loads = AtomicUsize::new(0);
    cache
        .get_or_load(&MapSource::omf("b.omf"), &config, counting_loader(&loads))
        .expect("load");
    assert_eq!(
        loads.load(Ordering::Relaxed),
        1,
        "`b` was evicted, so reading it has to load again"
    );
}
