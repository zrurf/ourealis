//! Rasterising the downloaded data onto the import grid.
//!
//! Every layer the map stores is a dense grid of `f32`, in the order the OMF
//! builder consumes: channel-continuous, row by row from the south-west corner.
//! Painting happens in a fixed order so overlaps resolve predictably —
//! `roads` first, then walkable `areas`, then obstacles — and the result is one
//! [`Layers`] value that the elevation pass adds heights to.

pub mod areas;
pub mod elevation;
pub mod grid;
pub mod roads;

pub use grid::Grid;

use ourealis_map_format::surface;

use crate::osm::OsmData;
use crate::osm::class::{OPEN_CROWDING, OPEN_LIGHTING};

/// The rasterised map, one `f32` per cell per layer.
#[derive(Debug, Clone)]
pub struct Layers {
    /// Cell geometry.
    pub grid: Grid,
    /// Terrain elevation in metres.
    pub elevation: Vec<f32>,
    /// Surface category id, see [`ourealis_map_format::surface`].
    pub surface: Vec<f32>,
    /// Hard obstacle mask, `1.0` where the cell is impassable.
    pub forbidden: Vec<f32>,
    /// Motor traffic exposure in `[0, 1]`.
    pub traffic: Vec<f32>,
    /// Pedestrian crowding in `[0, 1]`.
    pub crowding: Vec<f32>,
    /// Street lighting in `[0, 1]`.
    pub lighting: Vec<f32>,
    /// Packed direction constraint, `angle_index * 256 + strength`; zero where
    /// the cell carries no preference.
    pub direction: Vec<f32>,
    /// Height a building raises the terrain by, metres; zero on open ground.
    ///
    /// Kept beside the layers rather than in one of them: it is not a
    /// traversability channel, it is the shape of the city, and the builder adds
    /// it to the elevation so a footprint reads as a block rather than as paint.
    pub building_height: Vec<f32>,
}

impl Layers {
    /// A grid covered in open lawn, which nothing may cross.
    ///
    /// The default is *blocked* rather than open: unmapped ground is a lawn, a
    /// verge or a courtyard, and a planner that treats it as walkable cuts
    /// straight across the campus. Only the cells a way or a paved area paints
    /// become passable, so the network the raster holds is exactly the network
    /// that was surveyed.
    pub fn new(grid: Grid) -> Self {
        let cells = grid.len();
        Self {
            elevation: vec![0.0; cells],
            surface: vec![surface::GRASS as f32; cells],
            forbidden: vec![1.0; cells],
            traffic: vec![0.0; cells],
            crowding: vec![OPEN_CROWDING; cells],
            lighting: vec![OPEN_LIGHTING; cells],
            direction: vec![0.0; cells],
            building_height: vec![0.0; cells],
            grid,
        }
    }
}

/// Paints the OSM ways and areas onto a fresh set of layers.
///
/// `building_margin_m` grows every building footprint by that much. The margin
/// exists because an OSM footprint is a surveyed outline of the walls, and a
/// route that hugs one is not runnable; it also keeps the planner from
/// threading a path through a gap narrower than a person.
///
/// The order is what resolves overlaps: greens first, so a park reads as a lawn
/// under everything built in it, then walkable areas — a pitch inside the oval
/// that encloses it, since areas are painted largest first — then the ways, so a
/// path across a park is a path rather than a hole in the lawn, and finally the
/// obstacles, which override whatever surface a crossing way put down.
pub fn rasterise(grid: &Grid, osm: &OsmData, building_margin_m: f64) -> Layers {
    let mut layers = Layers::new(grid.clone());
    areas::paint_green(&mut layers, &osm.areas);
    areas::paint_surfaces(&mut layers, &osm.areas);
    roads::paint(&mut layers, &osm.roads);
    areas::apply_obstacles(&mut layers, &osm.areas, building_margin_m);
    layers
}
