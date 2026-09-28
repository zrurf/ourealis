//! DEM tile naming, GeoTIFF sampling and void filling.
//!
//! The georeferencing convention under test: a raster's `(origin_lon, origin_lat)`
//! is the **centre** of pixel `(0, 0)`, pixels step by `step_lon` to the east and
//! by the negative `step_lat` to the south, and `covers` admits exactly the pixel
//! centres plus the interior between them.

use approx::assert_relative_eq;

use ourealis_geo_import::GeoBounds;
use ourealis_geo_import::cqupt;
use ourealis_geo_import::dem::{DemMosaic, DemRaster, DemSource};
use ourealis_geo_import::raster::{Grid, elevation};

/// A 3 x 3 tile of 0.001 degrees per pixel whose samples equal their linear
/// index, so a bilinear result is easy to predict.
fn ramp() -> DemRaster {
    DemRaster::from_parts(
        3,
        3,
        0.0,
        0.0,
        0.001,
        -0.001,
        None,
        vec![0.0, 1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0],
    )
    .expect("a valid raster")
}

#[test]
fn tiles_are_named_after_their_south_west_degree_cell() {
    assert_eq!(
        DemSource::tile_name(29.53, 106.60),
        "Copernicus_DSM_COG_10_N29_00_E106_00_DEM"
    );
    // The product floors, so 29.99 and 29.01 belong to the same tile.
    assert_eq!(
        DemSource::tile_name(29.99, 106.99),
        "Copernicus_DSM_COG_10_N29_00_E106_00_DEM"
    );
    assert_eq!(
        DemSource::tile_name(-0.5, -0.5),
        "Copernicus_DSM_COG_10_S01_00_W001_00_DEM"
    );
}

#[test]
fn the_campus_box_falls_inside_one_dem_tile() {
    let source = DemSource::copernicus_glo30("data/dem");
    assert_eq!(
        source.tiles_for(&cqupt::bounds()),
        vec!["Copernicus_DSM_COG_10_N29_00_E106_00_DEM".to_owned()]
    );
    assert!(source.cache_dir().ends_with("dem"));
    assert!(source.tile_path("t").ends_with("t.tif"));
}

#[test]
fn a_box_that_straddles_a_degree_boundary_pulls_in_every_tile_it_touches() {
    let source = DemSource::copernicus_glo30("data/dem");
    let bounds = GeoBounds::new(29.9, 106.9, 30.1, 107.1).expect("a valid box");
    assert_eq!(
        source.tiles_for(&bounds),
        vec![
            "Copernicus_DSM_COG_10_N29_00_E106_00_DEM".to_owned(),
            "Copernicus_DSM_COG_10_N29_00_E107_00_DEM".to_owned(),
            "Copernicus_DSM_COG_10_N30_00_E106_00_DEM".to_owned(),
            "Copernicus_DSM_COG_10_N30_00_E107_00_DEM".to_owned(),
        ]
    );
}

#[test]
fn a_box_that_ends_on_a_degree_boundary_does_not_reach_past_it() {
    let source = DemSource::copernicus_glo30("data/dem");
    let bounds = GeoBounds::new(29.5, 106.5, 30.0, 107.0).expect("a valid box");
    assert_eq!(
        source.tiles_for(&bounds),
        vec!["Copernicus_DSM_COG_10_N29_00_E106_00_DEM".to_owned()]
    );
}

#[test]
fn a_raster_samples_bilinearly_between_pixel_centres() {
    let raster = ramp();

    assert_relative_eq!(
        raster.sample(0.0, 0.0).expect("a centre"),
        0.0,
        epsilon = 1e-9
    );
    assert_relative_eq!(
        raster.sample(0.001, -0.001).expect("a centre"),
        4.0,
        epsilon = 1e-9
    );
    // Halfway between the four centres of the south-west window.
    assert_relative_eq!(
        raster.sample(0.0005, -0.0005).expect("inside"),
        2.0,
        epsilon = 1e-9
    );
    // The far corner clamps to the edge pixel rather than reading past it.
    assert_relative_eq!(
        raster.sample(0.002, -0.002).expect("a corner"),
        8.0,
        epsilon = 1e-9
    );

    assert!(raster.covers(0.002, -0.002));
    assert!(!raster.covers(0.0021, -0.002));
    assert!(raster.sample(0.003, -0.001).is_none());
    assert!(raster.sample(0.0, 0.001).is_none());
}

#[test]
fn a_void_anywhere_in_the_interpolation_window_makes_the_sample_a_hole() {
    // A 4 x 4 raster whose 6th sample — pixel (1, 1) — is marked as no data.
    let raster = DemRaster::from_parts(
        4,
        4,
        0.0,
        0.0,
        0.001,
        -0.001,
        Some(5.0),
        (0..16).map(|value| value as f32).collect(),
    )
    .expect("a valid raster");

    // The window of the south-west corner spans pixels (0, 0) to (1, 1).
    assert!(raster.sample(0.0005, -0.0005).is_none());
    // Pixel (2, 2) interpolates inside its own window, which carries no void.
    assert_relative_eq!(
        raster.sample(0.002, -0.002).expect("a valid window"),
        10.0,
        epsilon = 1e-9
    );
}

#[test]
fn a_raster_that_cannot_be_sampled_is_refused() {
    let samples = vec![0.0; 9];
    // A south-up latitude axis is not a north-up geographic grid.
    assert!(DemRaster::from_parts(3, 3, 0.0, 0.0, 0.001, 0.001, None, samples.clone()).is_err());
    // A sample count that does not fill the declared shape.
    assert!(DemRaster::from_parts(3, 3, 0.0, 0.0, 0.001, -0.001, None, vec![0.0; 4]).is_err());
    // Too small to interpolate.
    assert!(DemRaster::from_parts(1, 1, 0.0, 0.0, 0.001, -0.001, None, vec![0.0; 1]).is_err());
    // A mosaic of nothing covers nothing.
    assert!(DemMosaic::new(Vec::new()).is_err());
}

#[test]
fn a_mosaic_answers_from_the_tile_that_covers_the_position() {
    let west = ramp();
    let east = DemRaster::from_parts(3, 3, 0.002, 0.0, 0.001, -0.001, None, vec![100.0; 9])
        .expect("a valid raster");
    let mosaic = DemMosaic::new(vec![west, east]).expect("a mosaic of two tiles");
    assert_eq!(mosaic.tiles().len(), 2);

    assert_relative_eq!(
        mosaic.sample(0.001, 0.0).expect("in the west tile"),
        1.0,
        epsilon = 1e-9
    );
    assert_relative_eq!(
        mosaic.sample(0.003, -0.001).expect("in the east tile"),
        100.0,
        epsilon = 1e-9
    );
    assert!(mosaic.sample(10.0, 0.0).is_none());
}

#[test]
fn elevation_voids_are_filled_from_their_neighbours() {
    let bounds = GeoBounds::new(0.0, 0.0, 0.002, 0.002).expect("a valid box");
    let grid = Grid::new(bounds, 10.0).expect("a valid grid");
    // A tile covering only the southern half of the grid; the northern cell
    // centres fall outside every tile and must be filled in.
    let tile = DemRaster::from_parts(3, 3, 0.0, 0.001, 0.001, -0.001, None, vec![500.0; 9])
        .expect("a valid raster");
    let mosaic = DemMosaic::new(vec![tile]).expect("a mosaic");

    let values = elevation::sample(&grid, &mosaic);
    assert_eq!(values.len(), grid.len());
    // The tile carries a single height, so filling propagates it over the hole.
    for value in &values {
        assert_relative_eq!(*value, 500.0, epsilon = 1e-3);
    }
}
