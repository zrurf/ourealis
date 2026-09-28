//! Rasterising ways and areas onto the import grid.
//!
//! The fixtures use a 23 x 23 grid of 10 m cells anchored at `0 N, 0 E`, so a
//! local metre position maps to a cell index by eye: cell `i` is centred at
//! `(i + 0.5) * 10`.

use ourealis_geo_import::GeoBounds;
use ourealis_geo_import::osm::class::{OPEN_CROWDING, OPEN_LIGHTING};
use ourealis_geo_import::osm::{Area, AreaKind, OsmData, Road, RoadClass};
use ourealis_geo_import::raster::{Grid, Layers, rasterise};
use ourealis_map_format::surface;

fn grid() -> Grid {
    let bounds = GeoBounds::new(0.0, 0.0, 0.002, 0.002).expect("a valid box");
    Grid::new(bounds, 10.0).expect("a valid grid")
}

fn class(surface: u8, traffic: f32, lighting: f32) -> RoadClass {
    RoadClass {
        surface,
        traffic,
        crowding: 0.2,
        lighting,
        half_width_m: 2.0,
    }
}

/// A way through the given local metre positions.
fn way(grid: &Grid, class: RoadClass, local: &[(f64, f64)]) -> Road {
    Road {
        class,
        points: local
            .iter()
            .map(|(x, y)| grid.geo_of_local(*x, *y))
            .collect(),
    }
}

/// An area bounded by the axis-aligned rectangle through two local corners.
fn area(grid: &Grid, kind: AreaKind, min: (f64, f64), max: (f64, f64)) -> Area {
    area_with_height(grid, kind, None, min, max)
}

/// The same rectangle, carrying the height a building raises the terrain by.
fn area_with_height(
    grid: &Grid,
    kind: AreaKind,
    height_m: Option<f32>,
    min: (f64, f64),
    max: (f64, f64),
) -> Area {
    Area {
        kind,
        outline: [
            (min.0, min.1),
            (max.0, min.1),
            (max.0, max.1),
            (min.0, max.1),
        ]
        .iter()
        .map(|(x, y)| grid.geo_of_local(*x, *y))
        .collect(),
        height_m,
    }
}

fn roads_only(roads: Vec<Road>) -> OsmData {
    OsmData {
        roads,
        areas: Vec::new(),
    }
}

fn areas_only(areas: Vec<Area>) -> OsmData {
    OsmData {
        roads: Vec::new(),
        areas,
    }
}

#[test]
fn a_way_paints_the_cells_whose_centres_are_within_its_half_width() {
    let grid = grid();
    // Row 10 is centred at y = 105, exactly on the way; the half-width of 2 m is
    // below the 5 m one raster cell enforces as a floor.
    let road = way(
        &grid,
        class(surface::ASPHALT, 0.5, 0.8),
        &[(0.0, 105.0), (200.0, 105.0)],
    );
    let layers = rasterise(&grid, &roads_only(vec![road]), 0.0);

    let on = grid.index(10, 10);
    assert_eq!(layers.surface[on], surface::ASPHALT as f32);
    assert_eq!(layers.traffic[on], 0.5);
    assert_eq!(layers.lighting[on], 0.8);
    assert_eq!(layers.forbidden[on], 0.0);

    // One row away is 10 m from the centre line, outside the half-width, and
    // keeps the open-ground defaults.
    let off = grid.index(10, 9);
    assert_eq!(layers.surface[off], surface::GRASS as f32);
    assert_eq!(layers.crowding[off], OPEN_CROWDING);
    assert_eq!(layers.lighting[off], OPEN_LIGHTING);
}

#[test]
fn overlapping_ways_resolve_the_same_way_in_either_order() {
    let grid = grid();
    let asphalt = way(
        &grid,
        class(surface::ASPHALT, 0.6, 0.8),
        &[(0.0, 105.0), (200.0, 105.0)],
    );
    let gravel = way(
        &grid,
        class(surface::GRAVEL, 0.1, 0.3),
        &[(0.0, 105.0), (200.0, 105.0)],
    );
    let cell = grid.index(10, 10);

    for order in [[asphalt.clone(), gravel.clone()], [gravel, asphalt]] {
        let layers = rasterise(&grid, &roads_only(Vec::from(order)), 0.0);
        // The busier way takes the surface and the higher scalar of each
        // channel, whichever order Overpass returned the two in.
        assert_eq!(layers.surface[cell], surface::ASPHALT as f32);
        assert_eq!(layers.traffic[cell], 0.6);
        assert_eq!(layers.lighting[cell], 0.8);
    }
}

#[test]
fn geometry_that_cannot_reach_the_grid_paints_nothing() {
    let grid = grid();
    let road = way(
        &grid,
        class(surface::ASPHALT, 0.5, 0.8),
        &[(-5000.0, -5000.0), (-4900.0, -4900.0)],
    );
    let layers = rasterise(&grid, &roads_only(vec![road]), 0.0);
    assert!(layers.surface.iter().all(|v| *v == surface::GRASS as f32));
    // Nothing was reached, so nothing was opened: the ground stays the blocked
    // lawn the raster starts on.
    assert!(layers.forbidden.iter().all(|v| *v == 1.0));
}

#[test]
fn a_building_footprint_is_impassable_and_grows_with_the_margin() {
    let grid = grid();
    let building = area(&grid, AreaKind::Building, (100.0, 100.0), (140.0, 140.0));
    let osm = areas_only(vec![building]);

    let tight = rasterise(&grid, &osm, 0.0);
    let inside = grid.index(10, 10);
    assert_eq!(tight.forbidden[inside], 1.0);
    assert_eq!(tight.surface[inside], surface::BUILDING as f32);
    assert_eq!(tight.traffic[inside], 0.0);
    assert!(tight.crowding[inside] >= 0.30);
    assert!(tight.lighting[inside] >= 0.80);

    // A cell centred at x = 95 lies outside the wall as surveyed…
    let beside = grid.index(9, 10);
    assert_eq!(tight.surface[beside], surface::GRASS as f32);
    assert_eq!(tight.building_height[beside], 0.0);

    // …and the margin grows the footprint over it.
    let grown = rasterise(&grid, &osm, 12.0);
    assert_eq!(grown.forbidden[beside], 1.0);
    assert_eq!(grown.surface[beside], surface::BUILDING as f32);
    assert!(grown.building_height[beside] > 0.0);
}

#[test]
fn a_building_raises_the_terrain_by_the_height_it_carries() {
    let grid = grid();
    let tall = area_with_height(
        &grid,
        AreaKind::Building,
        Some(48.0),
        (100.0, 100.0),
        (140.0, 140.0),
    );
    let layers = rasterise(&grid, &areas_only(vec![tall]), 0.0);
    // The mask carries the surveyed height, and only the footprint carries it.
    assert_eq!(layers.building_height[grid.index(10, 10)], 48.0);
    assert_eq!(layers.building_height[grid.index(9, 10)], 0.0);

    // A building with no height in its tags still stands: the raster supplies a
    // default so the footprint is never a flat patch.
    let untagged = area(&grid, AreaKind::Building, (100.0, 100.0), (140.0, 140.0));
    let layers = rasterise(&grid, &areas_only(vec![untagged]), 0.0);
    assert!(layers.building_height[grid.index(10, 10)] > 0.0);
}

#[test]
fn a_building_thinner_than_a_cell_is_still_impassable() {
    let grid = grid();
    // 40 m by 4 m on a 10 m grid: no cell centre falls inside the slab, so a fill
    // driven by centres alone would produce no mask and block nothing.
    let wall = area(&grid, AreaKind::Building, (100.0, 100.0), (140.0, 104.0));
    let layers = rasterise(&grid, &areas_only(vec![wall]), 0.0);

    let covered = grid.index(11, 10);
    assert_eq!(layers.forbidden[covered], 1.0);
    assert_eq!(layers.surface[covered], surface::BUILDING as f32);
    // The slab is one cell tall, so the row above it is still open ground.
    assert_eq!(layers.surface[grid.index(11, 11)], surface::GRASS as f32);
    assert_eq!(layers.building_height[grid.index(11, 11)], 0.0);
}

#[test]
fn a_lawn_is_grass_and_impassable() {
    let grid = grid();
    let park = area(&grid, AreaKind::Green, (100.0, 100.0), (140.0, 140.0));
    let layers = rasterise(&grid, &areas_only(vec![park]), 0.0);

    let inside = grid.index(10, 10);
    assert_eq!(layers.surface[inside], surface::GRASS as f32);
    assert_eq!(layers.forbidden[inside], 1.0);
    assert_eq!(layers.traffic[inside], 0.0);
}

#[test]
fn a_path_across_a_lawn_opens_the_cells_it_runs_through() {
    let grid = grid();
    let park = area(&grid, AreaKind::Green, (0.0, 0.0), (200.0, 200.0));
    let path = way(
        &grid,
        class(surface::PAVING, 0.0, 0.4),
        &[(0.0, 105.0), (200.0, 105.0)],
    );
    let osm = OsmData {
        roads: vec![path],
        areas: vec![park],
    };
    let layers = rasterise(&grid, &osm, 0.0);

    // The lawn is blocked on either side…
    assert_eq!(layers.forbidden[grid.index(10, 9)], 1.0);
    assert_eq!(layers.surface[grid.index(10, 9)], surface::GRASS as f32);
    // …and the path across it is the way through.
    let on = grid.index(10, 10);
    assert_eq!(layers.forbidden[on], 0.0);
    assert_eq!(layers.surface[on], surface::PAVING as f32);
}

#[test]
fn a_pitch_inside_the_oval_that_encloses_it_is_bare_ground() {
    let grid = grid();
    // The sports centre is one polygon for the whole venue; the field inside it
    // is a second, smaller one. The field has to win, or the map shows a running
    // surface where the grass is and the ring the runner actually uses is lost.
    let oval = area(
        &grid,
        AreaKind::Surface(surface::TRACK),
        (100.0, 100.0),
        (200.0, 200.0),
    );
    let field = area(
        &grid,
        AreaKind::Surface(surface::GROUND),
        (130.0, 130.0),
        (170.0, 170.0),
    );
    let layers = rasterise(&grid, &areas_only(vec![field, oval]), 0.0);

    // The ring the oval paints is still a track…
    let ring = grid.index(10, 15);
    assert_eq!(layers.surface[ring], surface::TRACK as f32);
    assert!(layers.direction[ring] > 0.0);
    // …and the field in the middle is ground with no direction to run.
    let centre = grid.index(15, 15);
    assert_eq!(layers.surface[centre], surface::GROUND as f32);
    assert_eq!(layers.direction[centre], 0.0);
}

#[test]
fn water_is_impassable_and_dark() {
    let grid = grid();
    let pond = area(&grid, AreaKind::Water, (100.0, 100.0), (140.0, 140.0));
    let layers = rasterise(&grid, &areas_only(vec![pond]), 0.0);

    let inside = grid.index(10, 10);
    assert_eq!(layers.forbidden[inside], 1.0);
    assert_eq!(layers.surface[inside], surface::WATER as f32);
    assert_eq!(layers.crowding[inside], 0.0);
    assert_eq!(layers.lighting[inside], 0.10);
}

#[test]
fn a_running_track_carries_a_direction_preference() {
    let grid = grid();
    let track = area(
        &grid,
        AreaKind::Surface(surface::TRACK),
        (100.0, 100.0),
        (200.0, 200.0),
    );
    let layers = rasterise(&grid, &areas_only(vec![track]), 0.0);

    let corner = grid.index(10, 10);
    assert_eq!(layers.surface[corner], surface::TRACK as f32);
    assert_eq!(layers.traffic[corner], 0.0);
    assert_eq!(layers.lighting[corner], 0.60);

    // The packed value is `angle_index * 256 + strength`, so the low byte is the
    // strength the importer fixed.
    let packed = layers.direction[corner];
    assert_eq!(packed % 256.0, 220.0);
    assert!(packed / 256.0 >= 1.0);

    // Opposite sides of the oval prefer opposite tangents.
    assert_ne!(
        layers.direction[grid.index(10, 15)],
        layers.direction[grid.index(19, 15)]
    );

    // The tangent is undefined at the centre of mass, so the cells there carry
    // no preference.
    for (x, y) in [(14, 14), (14, 15), (15, 14), (15, 15)] {
        assert_eq!(layers.direction[grid.index(x, y)], 0.0);
    }
}

#[test]
fn an_empty_extract_leaves_the_ground_blocked_lawn() {
    let grid = grid();
    let layers: Layers = rasterise(&grid, &OsmData::default(), 1.0);
    for index in 0..grid.len() {
        assert_eq!(layers.surface[index], surface::GRASS as f32);
        assert_eq!(layers.crowding[index], OPEN_CROWDING);
        assert_eq!(layers.lighting[index], OPEN_LIGHTING);
        // Unmapped ground is a lawn rather than open space: nothing may cross it
        // until a way or a paved area says the ground is walkable.
        assert_eq!(layers.forbidden[index], 1.0);
        assert_eq!(layers.direction[index], 0.0);
        assert_eq!(layers.building_height[index], 0.0);
    }
}
