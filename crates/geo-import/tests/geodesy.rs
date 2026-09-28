//! Projection and grid geometry.

use approx::assert_relative_eq;

use ourealis_geo_import::GeoBounds;
use ourealis_geo_import::raster::Grid;

fn bounds() -> GeoBounds {
    GeoBounds::new(29.5, 106.5, 29.6, 106.7).expect("valid box")
}

#[test]
fn projection_origin_is_the_south_west_corner() {
    let bounds = bounds();
    let (x, y) = bounds.project(bounds.west, bounds.south);
    assert_relative_eq!(x, 0.0, epsilon = 1e-9);
    assert_relative_eq!(y, 0.0, epsilon = 1e-9);

    let (x, y) = bounds.project(bounds.east, bounds.north);
    assert_relative_eq!(x, bounds.width_m(), epsilon = 1e-6);
    assert_relative_eq!(y, bounds.height_m(), epsilon = 1e-6);
}

#[test]
fn geo_of_local_inverts_local_of_geo() {
    let grid = Grid::new(bounds(), 20.0).expect("valid grid");
    let (x, y) = grid.local_of_geo(106.61, 29.55);
    let (lon, lat) = grid.geo_of_local(x, y);
    assert_relative_eq!(lon, 106.61, epsilon = 1e-9);
    assert_relative_eq!(lat, 29.55, epsilon = 1e-9);
}

#[test]
fn grid_rounds_the_extent_up_to_whole_cells() {
    let grid = Grid::new(bounds(), 20.0).expect("valid grid");
    assert!(grid.width_m() >= grid.geo().width_m());
    assert!(grid.height_m() >= grid.geo().height_m());
    assert!(grid.width_m() - grid.geo().width_m() < grid.res_m());
    assert!(grid.height_m() - grid.geo().height_m() < grid.res_m());

    let extent = grid.extent();
    assert_relative_eq!(extent.max_x, grid.width_m(), epsilon = 1e-9);
    assert_relative_eq!(extent.max_y, grid.height_m(), epsilon = 1e-9);
}

#[test]
fn axis_cells_selects_the_cells_whose_centres_fall_inside() {
    let bounds = GeoBounds::new(0.0, 0.0, 0.002, 0.002).expect("valid box");
    let grid = Grid::new(bounds, 10.0).expect("valid grid");
    let cells = grid.width();

    // Cell `i` is centred at `(i + 0.5) * 10`.
    assert_eq!(grid.axis_cells(25.0, 25.0, cells), Some((2, 2)));
    assert_eq!(grid.axis_cells(25.0, 34.0, cells), Some((2, 2)));
    assert_eq!(grid.axis_cells(25.0, 35.0, cells), Some((2, 3)));
    assert_eq!(grid.axis_cells(-100.0, 0.0, cells), None);
    // A span between two centres contains neither of them.
    assert_eq!(grid.axis_cells(30.0, 30.0, cells), None);

    let (first, last) = grid
        .axis_cells(-1000.0, 1000.0, cells)
        .expect("the whole grid");
    assert_eq!((first, last), (0, cells - 1));
}

#[test]
fn a_box_outside_the_grid_limits_is_refused() {
    let bounds = GeoBounds::new(0.0, 0.0, 0.002, 0.002).expect("valid box");
    // Ten metres per cell over a 222 m box is a 23 x 23 grid; a centimetre per
    // cell would be 22 300 cells on a side and far past the cap.
    assert!(Grid::new(bounds, 0.01).is_err());
    assert!(Grid::new(bounds, 0.0).is_err());
}
