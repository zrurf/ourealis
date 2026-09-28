//! The campus overlay: the geometry it generates and how it replaces the OSM
//! areas it covers.

use ourealis_geo_import::cqupt;
use ourealis_geo_import::facilities::Overlay;
use ourealis_geo_import::osm::{Area, AreaKind, OsmData};
use ourealis_geo_import::raster::Grid;
use ourealis_map_format::surface;

fn grid() -> Grid {
    Grid::new(cqupt::bounds(), 1.0).expect("the campus grid")
}

/// The areas of one surface category.
fn areas_of(data: &OsmData, category: u8) -> Vec<&Area> {
    data.areas
        .iter()
        .filter(|area| area.kind == AreaKind::Surface(category))
        .collect()
}

/// East-west and north-south extent of an outline in the local metre plane.
fn local_span(grid: &Grid, outline: &[(f64, f64)]) -> (f64, f64) {
    let (mut west, mut east) = (f64::INFINITY, f64::NEG_INFINITY);
    let (mut south, mut north) = (f64::INFINITY, f64::NEG_INFINITY);
    for (lon, lat) in outline {
        let (x, y) = grid.local_of_geo(*lon, *lat);
        west = west.min(x);
        east = east.max(x);
        south = south.min(y);
        north = north.max(y);
    }
    (east - west, north - south)
}

/// Centre of an outline in the local metre plane, as the centre of its bounding box.
fn local_centre(grid: &Grid, outline: &[(f64, f64)]) -> (f64, f64) {
    let (mut west, mut east) = (f64::INFINITY, f64::NEG_INFINITY);
    let (mut south, mut north) = (f64::INFINITY, f64::NEG_INFINITY);
    for (lon, lat) in outline {
        let (x, y) = grid.local_of_geo(*lon, *lat);
        west = west.min(x);
        east = east.max(x);
        south = south.min(y);
        north = north.max(y);
    }
    ((west + east) * 0.5, (south + north) * 0.5)
}

/// A rectangular ground area over a local metre box.
fn rectangle(grid: &Grid, min: (f64, f64), max: (f64, f64)) -> Area {
    Area {
        kind: AreaKind::Surface(surface::GROUND),
        outline: [
            (min.0, min.1),
            (max.0, min.1),
            (max.0, max.1),
            (min.0, max.1),
        ]
        .iter()
        .map(|(x, y)| grid.geo_of_local(*x, *y))
        .collect(),
        height_m: None,
    }
}

fn distance(a: (f64, f64), b: (f64, f64)) -> f64 {
    (a.0 - b.0).hypot(a.1 - b.1)
}

#[test]
fn a_standard_track_is_a_four_hundred_metre_capsule() {
    let grid = grid();
    let mut data = OsmData::default();
    Overlay::cqupt().apply(&grid, &mut data);

    let tracks = areas_of(&data, surface::TRACK);
    assert_eq!(tracks.len(), 2);
    let infields = areas_of(&data, surface::GROUND);

    for track in &tracks {
        // Two 46.26 m ends across and an 84.39 m straight between them.
        let (width, height) = local_span(&grid, &track.outline);
        assert!((width - 92.52).abs() < 0.5, "track width {width} m");
        assert!((height - 176.91).abs() < 0.5, "track length {height} m");

        // The infield is the ground area nearest the track's centre; the ring it
        // leaves is the lane width.
        let centre = local_centre(&grid, &track.outline);
        let infield = infields
            .iter()
            .copied()
            .min_by(|a, b| {
                let (a, b) = (
                    local_centre(&grid, &a.outline),
                    local_centre(&grid, &b.outline),
                );
                distance(a, centre).total_cmp(&distance(b, centre))
            })
            .expect("an infield");
        let (infield_width, _) = local_span(&grid, &infield.outline);
        let lane = (width - infield_width) / 2.0;
        assert!((lane - 9.76).abs() < 0.5, "lane width {lane} m");
    }
}

#[test]
fn the_tracks_land_where_the_anchors_say() {
    let grid = grid();
    let mut data = OsmData::default();
    Overlay::cqupt().apply(&grid, &mut data);

    let centres: Vec<(f64, f64)> = areas_of(&data, surface::TRACK)
        .iter()
        .map(|track| local_centre(&grid, &track.outline))
        .collect();

    // The GCJ-02 anchors, converted to WGS84 and projected, put 风华运动场 and
    // 太极运动场 at these local metre positions.
    assert!(
        centres
            .iter()
            .any(|c| distance(*c, (1087.0, 1610.0)) < 15.0),
        "{centres:?}"
    );
    assert!(
        centres
            .iter()
            .any(|c| distance(*c, (1300.0, 1627.0)) < 15.0),
        "{centres:?}"
    );
}

#[test]
fn apply_replaces_the_osm_areas_it_covers() {
    let grid = grid();
    let covered = rectangle(&grid, (1050.0, 1580.0), (1130.0, 1640.0));
    let outside = rectangle(&grid, (300.0, 300.0), (380.0, 360.0));
    let mut data = OsmData {
        roads: Vec::new(),
        areas: vec![covered.clone(), outside.clone()],
    };

    Overlay::cqupt().apply(&grid, &mut data);

    // The area over the track's site is replaced by the capsule, the one far from
    // every facility is left as the extract drew it.
    assert!(
        !data
            .areas
            .iter()
            .any(|area| area.outline == covered.outline)
    );
    assert!(
        data.areas
            .iter()
            .any(|area| area.outline == outside.outline)
    );
    // Two tracks, each a rubber ring and a bare infield, three courts, and the
    // untouched extract area.
    assert_eq!(areas_of(&data, surface::TRACK).len(), 2);
    assert_eq!(areas_of(&data, surface::GROUND).len(), 6);
}

#[test]
fn a_court_is_ground_with_the_surveyed_span() {
    let grid = grid();
    let mut data = OsmData::default();
    Overlay::cqupt().apply(&grid, &mut data);

    // 中心网球场, from its surveyed corners.
    let found = areas_of(&data, surface::GROUND).into_iter().any(|area| {
        let (width, height) = local_span(&grid, &area.outline);
        (width - 71.0).abs() < 1.0 && (height - 27.0).abs() < 1.0
    });
    assert!(found, "中心网球场 is missing its surveyed span");
}
