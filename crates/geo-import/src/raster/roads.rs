//! Painting the highway network onto the grid.
//!
//! A way is a polyline of projected vertices; each of its segments paints every
//! cell whose centre is within the way's half-width of it. Painting is
//! intentionally additive rather than last-write-wins: where two ways overlap —
//! a footway crossing a road, a service road meeting a street — the cell keeps
//! the highest traffic, crowding and lighting, and the surface of the busier
//! way. That makes the result independent of the order Overpass returns ways in.

use crate::osm::Road;
use crate::osm::class::RoadClass;
use crate::raster::Layers;
use crate::raster::grid::Grid;

/// Paints every way's surface and scalar channels.
pub fn paint(layers: &mut Layers, roads: &[Road]) {
    // The grid is copied out so the layer slices can be mutated while it is
    // still being addressed; it is a handful of scalars, not the raster.
    let grid = layers.grid.clone();
    for road in roads {
        let points: Vec<(f64, f64)> = road
            .points
            .iter()
            .map(|(lon, lat)| grid.local_of_geo(*lon, *lat))
            .collect();
        for segment in points.windows(2) {
            paint_segment(layers, &grid, segment[0], segment[1], &road.class);
        }
    }
}

fn paint_segment(
    layers: &mut Layers,
    grid: &Grid,
    a: (f64, f64),
    b: (f64, f64),
    class: &RoadClass,
) {
    if !a.0.is_finite() || !a.1.is_finite() || !b.0.is_finite() || !b.1.is_finite() {
        return;
    }
    // Half a cell is the floor: a way narrower than the raster would otherwise
    // slip between cell centres and paint nothing at all.
    let half = class.half_width_m.max(grid.res_m() * 0.5);
    let Some((x0, x1)) = grid.axis_cells(a.0.min(b.0) - half, a.0.max(b.0) + half, grid.width())
    else {
        return;
    };
    let Some((y0, y1)) = grid.axis_cells(a.1.min(b.1) - half, a.1.max(b.1) + half, grid.height())
    else {
        return;
    };
    let half_sq = half * half;
    for y in y0..=y1 {
        for x in x0..=x1 {
            let center = grid.cell_center(x, y);
            if point_segment_distance_sq(center, a, b) <= half_sq {
                apply(layers, grid.index(x, y), class);
            }
        }
    }
}

fn apply(layers: &mut Layers, index: usize, class: &RoadClass) {
    // A way is what makes a cell passable: the raster starts blocked, and a cell
    // the network reaches is open however the ground around it is classified.
    layers.forbidden[index] = 0.0;
    if layers.traffic[index] <= class.traffic {
        layers.surface[index] = class.surface as f32;
    }
    layers.traffic[index] = layers.traffic[index].max(class.traffic);
    layers.crowding[index] = layers.crowding[index].max(class.crowding);
    layers.lighting[index] = layers.lighting[index].max(class.lighting);
}

/// Squared distance from `point` to the segment `a -> b`.
///
/// Squared throughout: the callers only compare it against a squared radius, and
/// taking the root per cell would dominate the cost of a raster pass.
fn point_segment_distance_sq(point: (f64, f64), a: (f64, f64), b: (f64, f64)) -> f64 {
    let ab = (b.0 - a.0, b.1 - a.1);
    let length_sq = ab.0 * ab.0 + ab.1 * ab.1;
    let (dx, dy) = if length_sq <= f64::EPSILON {
        (point.0 - a.0, point.1 - a.1)
    } else {
        let t = (((point.0 - a.0) * ab.0 + (point.1 - a.1) * ab.1) / length_sq).clamp(0.0, 1.0);
        (point.0 - (a.0 + ab.0 * t), point.1 - (a.1 + ab.1 * t))
    };
    dx * dx + dy * dy
}
