//! Hand-surveyed campus facilities the OSM extract draws wrongly.
//!
//! An OSM extract carries whatever a mapper traced, which at a university
//! athletics ground is routinely a field tagged as a pitch and the ground's
//! outline tagged as a rectangle. A running track is a capsule, not the rectangle
//! the tags describe, and a ground nobody traced is simply absent. This module
//! holds the facilities a campus survey fixed by hand, in the GCJ-02 datum a
//! Chinese base map publishes, and overlays them on the OSM extract.
//!
//! Where the overlay draws it is authoritative: an OSM area whose representative
//! point falls inside one of the generated outlines is replaced rather than kept
//! alongside it, so the rectangle the extract drew at a track's site cannot
//! survive next to the capsule that supersedes it.

use std::f64::consts::PI;

use ourealis_map_format::surface;

use crate::geo::gcj02_to_wgs84;
use crate::osm::{Area, AreaKind, OsmData};
use crate::raster::Grid;

/// A facility the OSM extract draws wrongly or does not carry.
///
/// Coordinates are `(longitude, latitude)` in GCJ-02, the datum the survey was
/// taken in.
#[derive(Debug, Clone, PartialEq)]
enum Facility {
    /// A standard 400 m track, its straights running north-south.
    StandardTrack {
        /// Centre of the infield, in GCJ-02 degrees.
        centre: (f64, f64),
    },
    /// A court or pitch strip.
    Court {
        /// The surveyed corners, in GCJ-02 degrees.
        corners: [(f64, f64); 4],
    },
}

/// A set of surveyed facilities to overlay on an OSM extract.
#[derive(Debug, Clone, PartialEq)]
pub struct Overlay {
    facilities: Vec<Facility>,
}

/// Segments used to tessellate each semicircular end of a track.
///
/// A chord of a `32`-segment semicircle lies `r * (1 - cos(pi / 64))` from the
/// arc it spans, under 6 cm at the outermost radius: well inside the one-metre
/// cell the campus is rasterised at, so the bend reads as smooth.
const END_SEGMENTS: usize = 32;

/// Straight length of a standard 400 m track between its two bends, metres.
const STRAIGHT_M: f64 = 84.39;

/// Radius of the inner kerb of a standard 400 m track, metres.
///
/// The lane-1 measurement line runs 0.30 m outside this kerb, so one lap is
/// `2 * 84.39 + 2 * pi * (36.5 + 0.30) = 400.0 m`, the IAAF figure the campus
/// tracks are built to.
const INNER_RADIUS_M: f64 = 36.5;

/// Radius of a track's outer edge: eight 1.22 m lanes beyond the inner kerb.
const OUTER_RADIUS_M: f64 = INNER_RADIUS_M + 8.0 * 1.22;

impl Overlay {
    /// The facilities surveyed on the CQUPT campus.
    ///
    /// Two 400 m tracks — 风华运动场, which OSM draws as a rectangle, and
    /// 太极运动场, which OSM does not carry — and the strips of 桂花篮球场,
    /// 灯光篮球场 and 中心网球场.
    pub fn cqupt() -> Self {
        Self {
            facilities: vec![
                Facility::StandardTrack {
                    centre: (106.607575, 29.532848),
                },
                Facility::StandardTrack {
                    centre: (106.609763, 29.532927),
                },
                Facility::Court {
                    corners: [
                        (106.607161, 29.530223),
                        (106.607927, 29.530224),
                        (106.607161, 29.529967),
                        (106.607932, 29.529977),
                    ],
                },
                Facility::Court {
                    corners: [
                        (106.608185, 29.532846),
                        (106.608801, 29.532849),
                        (106.608186, 29.531984),
                        (106.608797, 29.531984),
                    ],
                },
                Facility::Court {
                    corners: [
                        (106.605996, 29.535792),
                        (106.60673, 29.535793),
                        (106.605996, 29.535546),
                        (106.606731, 29.535546),
                    ],
                },
            ],
        }
    }

    /// Replaces the OSM areas the overlay covers with its own.
    ///
    /// An OSM area is dropped when its representative point falls inside one of
    /// the generated outlines, and kept otherwise. Whole-outline containment is
    /// deliberately not required: the OSM area at one of these sites is often the
    /// whole venue, far larger than the track, and a partial overlap still has to
    /// be replaced rather than left beside the facility drawn on top of it.
    pub fn apply(&self, grid: &Grid, data: &mut OsmData) {
        let generated: Vec<Area> = self
            .facilities
            .iter()
            .flat_map(|facility| facility.areas(grid))
            .collect();
        data.areas.retain(|area| {
            representative_point(&area.outline).is_none_or(|point| {
                !generated
                    .iter()
                    .any(|own| point_in_outline(point, &own.outline))
            })
        });
        data.areas.extend(generated);
    }
}

impl Facility {
    /// The areas this facility paints, in the form the raster projects.
    fn areas(&self, grid: &Grid) -> Vec<Area> {
        match self {
            Facility::StandardTrack { centre } => track_areas(grid, *centre),
            Facility::Court { corners } => vec![court_area(*corners)],
        }
    }
}

/// The two areas of a standard track: the rubber ring and the infield it encloses.
///
/// The raster paints larger areas first, so emitting the outer capsule as a track
/// and the inner one as ground leaves the bare field on top and the rubber as the
/// ring between them. That respects the format's one-outline-per-area rule, which
/// cannot express the ring itself as a polygon with a hole.
fn track_areas(grid: &Grid, centre: (f64, f64)) -> Vec<Area> {
    let (lon, lat) = gcj02_to_wgs84(centre.0, centre.1);
    let origin = grid.local_of_geo(lon, lat);
    vec![
        local_area(grid, surface::TRACK, &capsule(origin, OUTER_RADIUS_M)),
        local_area(grid, surface::GROUND, &capsule(origin, INNER_RADIUS_M)),
    ]
}

/// Outline of a capsule in the local metre plane: two north-south straights
/// closed by semicircles of `radius_m` at each end.
///
/// The straights' end points are emitted once and the semicircle loops skip them,
/// so each arc continues the straight rather than repeating its end.
fn capsule(centre: (f64, f64), radius_m: f64) -> Vec<(f64, f64)> {
    let (cx, cy) = centre;
    let half = STRAIGHT_M * 0.5;
    let mut ring = Vec::with_capacity(2 * END_SEGMENTS + 4);
    ring.push((cx + radius_m, cy - half));
    ring.push((cx + radius_m, cy + half));
    for segment in 1..END_SEGMENTS {
        let theta = PI * segment as f64 / END_SEGMENTS as f64;
        ring.push((
            cx + radius_m * theta.cos(),
            cy + half + radius_m * theta.sin(),
        ));
    }
    ring.push((cx - radius_m, cy + half));
    ring.push((cx - radius_m, cy - half));
    for segment in 1..END_SEGMENTS {
        let theta = PI + PI * segment as f64 / END_SEGMENTS as f64;
        ring.push((
            cx + radius_m * theta.cos(),
            cy - half + radius_m * theta.sin(),
        ));
    }
    ring
}

/// A court as a clean rectangle spanning its surveyed corners.
///
/// The corners are approximate and the sides are meant to be axis-aligned, so the
/// outline is snapped to their bounding box: that smooths the survey's wobble
/// without moving the court.
fn court_area(corners: [(f64, f64); 4]) -> Area {
    let mut west = f64::INFINITY;
    let mut east = f64::NEG_INFINITY;
    let mut south = f64::INFINITY;
    let mut north = f64::NEG_INFINITY;
    for (lon, lat) in corners {
        west = west.min(lon);
        east = east.max(lon);
        south = south.min(lat);
        north = north.max(lat);
    }
    let outline = [(west, south), (east, south), (east, north), (west, north)]
        .into_iter()
        .map(|(lon, lat)| gcj02_to_wgs84(lon, lat))
        .collect();
    Area {
        kind: AreaKind::Surface(surface::GROUND),
        outline,
        height_m: None,
    }
}

/// An area from an outline in the local metre plane.
fn local_area(grid: &Grid, category: u8, outline: &[(f64, f64)]) -> Area {
    Area {
        kind: AreaKind::Surface(category),
        outline: outline
            .iter()
            .map(|(x, y)| grid.geo_of_local(*x, *y))
            .collect(),
        height_m: None,
    }
}

/// Mean of an outline's vertices: the point that decides which region an area
/// belongs to for the containment test.
fn representative_point(outline: &[(f64, f64)]) -> Option<(f64, f64)> {
    if outline.is_empty() {
        return None;
    }
    let count = outline.len() as f64;
    let (x, y) = outline
        .iter()
        .fold((0.0, 0.0), |sum, point| (sum.0 + point.0, sum.1 + point.1));
    Some((x / count, y / count))
}

/// Whether a point lies inside a closed outline, by ray casting.
///
/// The outline is treated as closed whether or not its last vertex repeats the
/// first, which matches the parser's outlines.
fn point_in_outline(point: (f64, f64), outline: &[(f64, f64)]) -> bool {
    if outline.len() < 3 {
        return false;
    }
    let mut inside = false;
    let mut previous = outline.len() - 1;
    for current in 0..outline.len() {
        let a = outline[current];
        let b = outline[previous];
        if (a.1 > point.1) != (b.1 > point.1)
            && point.0 < (b.0 - a.0) * (point.1 - a.1) / (b.1 - a.1) + a.0
        {
            inside = !inside;
        }
        previous = current;
    }
    inside
}
