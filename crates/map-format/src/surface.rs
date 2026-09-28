//! Surface categories shared by every map producer.
//!
//! A cell's surface is a small integer, and that integer has to mean the same thing to
//! whoever writes the file and whoever draws it: the importer rasters a lawn as one value,
//! the viewer looks the value up in a material table. This module is that agreement — nine
//! categories with fixed ids, never reordered, so a file written by one producer reads
//! correctly in any consumer.
//!
//! It stops at identity on purpose. The resistance a category contributes to the cost model
//! is *not* shared: each file carries its own `palette` of normalised resistances in the
//! layer metadata, so the synthetic generator keeps the tuning its tests were written
//! against while an OSM import uses the table the cost model was tuned for. Category ids
//! cross the boundary; resistance values do not.

/// Bituminous road.
pub const ASPHALT: u8 = 0;
/// Concrete, brick or stone paving.
pub const PAVING: u8 = 1;
/// Rubber running track.
pub const TRACK: u8 = 2;
/// Loose gravel or a compacted surface.
pub const GRAVEL: u8 = 3;
/// Bare earth or sand.
pub const GROUND: u8 = 4;
/// Lawn, meadow or soil.
pub const GRASS: u8 = 5;
/// Stairs.
pub const STEPS: u8 = 6;
/// Open water.
pub const WATER: u8 = 7;
/// Building footprint.
pub const BUILDING: u8 = 8;
/// Number of categories, i.e. the length every surface palette must have.
pub const COUNT: usize = 9;
