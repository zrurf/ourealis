//! Painting closed ways: lawns, walkable surfaces, building footprints and water.
//!
//! Areas are painted in three passes because an overlap has to resolve the way a
//! reader would draw it. Lawns go down first, under everything built in them.
//! Walkable surfaces come next, largest first, so a pitch overrides the oval that
//! encloses it rather than the other way round. Obstacles — buildings and water —
//! go last, so they override whatever surface a crossing way put down.
//!
//! Footprints are grown by the importer's margin before they block anything: an
//! OSM outline traces the walls, and a route that grazes one is not runnable.
//! Water is used as mapped — a shoreline is already the edge of the water.

use std::f64::consts::{FRAC_PI_2, TAU};

use ourealis_map_format::surface;

use crate::osm::class::area_channels;
use crate::osm::{Area, AreaKind};
use crate::raster::Layers;
use crate::raster::grid::Grid;

/// Strength of the direction preference the importer puts on a running track,
/// in the low byte of the packed value. High enough to steer a route round the
/// oval the right way, low enough to be overridden by a hard obstacle.
const TRACK_DIRECTION_STRENGTH: f32 = 220.0;

/// Height given to a building whose tags carry none, metres.
///
/// Two storeys plus a roof: low enough not to invent a tower, high enough that
/// the footprint reads as a block rather than as a painted patch on the ground.
const DEFAULT_BUILDING_HEIGHT_M: f32 = 9.0;

/// Paints every lawn: a park, a wood, a grass verge.
///
/// A lawn is impassable, so the pass sets the obstacle mask as well as the
/// surface. Ways are painted after this, which is what keeps the paths through a
/// park open.
pub fn paint_green(layers: &mut Layers, areas: &[Area]) {
    let grid = layers.grid.clone();
    for area in largest_first(areas, &grid) {
        if area.kind != AreaKind::Green {
            continue;
        }
        let outline = project(&grid, &area.outline);
        if outline.len() < 3 {
            continue;
        }
        paint_lawn(layers, &grid, &outline);
    }
}

/// Paints every walkable surface: a plaza, a pitch, a running track.
///
/// Largest first, so a pitch painted over the sports centre that encloses it
/// wins: an OSM oval is often one polygon for the whole venue, with the field
/// inside it mapped as a second one.
pub fn paint_surfaces(layers: &mut Layers, areas: &[Area]) {
    let grid = layers.grid.clone();
    for area in largest_first(areas, &grid) {
        let outline = project(&grid, &area.outline);
        if outline.len() < 3 {
            continue;
        }
        match area.kind {
            AreaKind::Surface(category) => paint_surface(layers, &grid, &outline, category),
            AreaKind::Building | AreaKind::Water | AreaKind::Green => {}
        }
    }
}

/// Paints the obstacles: building footprints and water bodies.
///
/// A building's outline carries the height it raises the terrain by, so the
/// extrusion and the obstacle share one mask and cannot disagree about where the
/// building stands.
pub fn apply_obstacles(layers: &mut Layers, areas: &[Area], building_margin_m: f64) {
    let grid = layers.grid.clone();
    let mut buildings = vec![0.0f32; grid.len()];
    let mut water = vec![0.0f32; grid.len()];

    for area in areas {
        let outline = project(&grid, &area.outline);
        if outline.len() < 3 {
            continue;
        }
        match area.kind {
            AreaKind::Building => mark(
                &mut buildings,
                &grid,
                &outline,
                area.height_m.unwrap_or(DEFAULT_BUILDING_HEIGHT_M),
            ),
            AreaKind::Water => mark(&mut water, &grid, &outline, 1.0),
            AreaKind::Green | AreaKind::Surface(_) => {}
        }
    }

    dilate(&mut buildings, &grid, building_margin_m);
    apply_buildings(layers, &grid, &buildings);
    apply_water(layers, &grid, &water);

    tracing::debug!(
        buildings = buildings.iter().filter(|v| **v > 0.0).count(),
        water = water.iter().filter(|v| **v > 0.0).count(),
        margin_m = building_margin_m,
        "rasterised obstacles"
    );
}

/// The areas in the order they must be painted: the largest footprint first.
///
/// Sorting by area rather than keeping the file's order is what makes a nested
/// polygon resolve correctly — the container is always painted before the thing
/// it contains — and makes the result independent of the order Overpass returned
/// the elements in.
fn largest_first<'a>(areas: &'a [Area], grid: &Grid) -> Vec<&'a Area> {
    let mut ordered: Vec<(f64, &Area)> = areas
        .iter()
        .map(|area| (polygon_area(&project(grid, &area.outline)).abs(), area))
        .collect();
    ordered.sort_by(|a, b| b.0.total_cmp(&a.0));
    ordered.into_iter().map(|(_, area)| area).collect()
}

/// Area enclosed by a projected outline, in square metres.
fn polygon_area(polygon: &[(f64, f64)]) -> f64 {
    let mut twice_area = 0.0;
    for i in 0..polygon.len() {
        let a = polygon[i];
        let b = polygon[(i + 1) % polygon.len()];
        twice_area += a.0 * b.1 - b.0 * a.1;
    }
    twice_area * 0.5
}

fn project(grid: &Grid, outline: &[(f64, f64)]) -> Vec<(f64, f64)> {
    outline
        .iter()
        .map(|(lon, lat)| grid.local_of_geo(*lon, *lat))
        .filter(|(x, y)| x.is_finite() && y.is_finite())
        .collect()
}

fn mark(mask: &mut [f32], grid: &Grid, outline: &[(f64, f64)], value: f32) {
    for (x, y) in filled_cells(grid, outline) {
        let index = grid.index(x, y);
        mask[index] = mask[index].max(value);
    }
}

/// Paints a lawn: grass, blocked, and with the channels of a patch of ground.
fn paint_lawn(layers: &mut Layers, grid: &Grid, outline: &[(f64, f64)]) {
    let (traffic, crowding, lighting) = area_channels(surface::GRASS);
    for (x, y) in filled_cells(grid, outline) {
        let index = grid.index(x, y);
        layers.surface[index] = surface::GRASS as f32;
        layers.forbidden[index] = 1.0;
        layers.traffic[index] = traffic;
        layers.crowding[index] = crowding;
        layers.lighting[index] = lighting;
        layers.direction[index] = 0.0;
    }
}

fn paint_surface(layers: &mut Layers, grid: &Grid, outline: &[(f64, f64)], category: u8) {
    let cells = filled_cells(grid, outline);
    let (traffic, crowding, lighting) = area_channels(category);
    for (x, y) in &cells {
        let index = grid.index(*x, *y);
        layers.surface[index] = category as f32;
        // A mapped surface is walked on, so it opens the cell the blocked
        // background had closed.
        layers.forbidden[index] = 0.0;
        layers.traffic[index] = layers.traffic[index].max(traffic);
        layers.crowding[index] = layers.crowding[index].max(crowding);
        layers.lighting[index] = layers.lighting[index].max(lighting);
        // Only a track steers a route, so a surface painted inside one — the
        // field within the oval — drops the ring's tangent rather than keeping
        // it: the tangent would send a route round the middle of the grass.
        layers.direction[index] = 0.0;
    }
    if category == surface::TRACK {
        paint_track_direction(layers, grid, outline, &cells);
    }
}

/// Marks a counter-clockwise direction preference on a track's cells.
///
/// Sports tracks are run counter-clockwise, and an OSM track way carries no
/// direction of its own — the vertex order of a closed way is arbitrary. The
/// tangent about the outline's centre of mass is therefore the only signal
/// available, and it is exactly right for the oval the tag describes. The centre
/// comes from the outline rather than from the cells it covers, so a
/// rasterisation that leans one cell to a side cannot tilt every heading.
fn paint_track_direction(
    layers: &mut Layers,
    grid: &Grid,
    outline: &[(f64, f64)],
    cells: &[(u32, u32)],
) {
    let Some(centroid) = centroid_of(outline) else {
        return;
    };
    for (x, y) in cells {
        let center = grid.cell_center(*x, *y);
        let offset = (center.0 - centroid.0, center.1 - centroid.1);
        if offset.0.hypot(offset.1) < grid.res_m() {
            // Too close to the centre for the tangent to mean anything.
            continue;
        }
        let angle = (offset.1.atan2(offset.0) + FRAC_PI_2).rem_euclid(TAU);
        let quantised = ((angle / TAU * 256.0) as u32) % 256;
        layers.direction[grid.index(*x, *y)] = quantised as f32 * 256.0 + TRACK_DIRECTION_STRENGTH;
    }
}

/// Centre of mass of a closed outline, by the shoelace formula.
///
/// `None` for a degenerate outline enclosing no area, which has no centre to
/// measure a tangent from.
fn centroid_of(polygon: &[(f64, f64)]) -> Option<(f64, f64)> {
    let (mut twice_area, mut x, mut y) = (0.0, 0.0, 0.0);
    for i in 0..polygon.len() {
        let a = polygon[i];
        let b = polygon[(i + 1) % polygon.len()];
        let cross = a.0 * b.1 - b.0 * a.1;
        twice_area += cross;
        x += (a.0 + b.0) * cross;
        y += (a.1 + b.1) * cross;
    }
    if twice_area.abs() < f64::EPSILON {
        return None;
    }
    Some((x / (3.0 * twice_area), y / (3.0 * twice_area)))
}

fn apply_buildings(layers: &mut Layers, grid: &Grid, mask: &[f32]) {
    for y in 0..grid.height() {
        for x in 0..grid.width() {
            let index = grid.index(x, y);
            let height = mask[index];
            if height <= 0.0 {
                continue;
            }
            layers.forbidden[index] = 1.0;
            layers.surface[index] = surface::BUILDING as f32;
            layers.traffic[index] = 0.0;
            layers.crowding[index] = layers.crowding[index].max(0.30);
            layers.lighting[index] = layers.lighting[index].max(0.80);
            layers.direction[index] = 0.0;
            // The mask carries the height rather than a bare flag, so the block
            // the terrain raises is exactly the block the planner refuses.
            layers.building_height[index] = layers.building_height[index].max(height);
        }
    }
}

fn apply_water(layers: &mut Layers, grid: &Grid, mask: &[f32]) {
    for y in 0..grid.height() {
        for x in 0..grid.width() {
            let index = grid.index(x, y);
            if mask[index] <= 0.0 {
                continue;
            }
            layers.forbidden[index] = 1.0;
            layers.surface[index] = surface::WATER as f32;
            layers.traffic[index] = 0.0;
            layers.crowding[index] = 0.0;
            layers.lighting[index] = 0.10;
            layers.direction[index] = 0.0;
        }
    }
}

/// Grows a mask by `radius_m` using a disc, in cells.
///
/// Cells keep the highest value they were grown from, so a mask that carries a
/// height still carries one over the whole grown footprint.
fn dilate(mask: &mut [f32], grid: &Grid, radius_m: f64) {
    if !radius_m.is_finite() || radius_m <= 0.0 {
        return;
    }
    let radius = (radius_m / grid.res_m()).ceil();
    if radius <= 0.0 {
        return;
    }
    let width = grid.width() as i64;
    let height = grid.height() as i64;
    let sources: Vec<(i64, i64, f32)> = mask
        .iter()
        .enumerate()
        .filter(|(_, value)| **value > 0.0)
        .map(|(index, value)| ((index as i64) % width, (index as i64) / width, *value))
        .collect();
    if sources.is_empty() {
        return;
    }
    let mut grown = mask.to_vec();
    let reach = radius as i64;
    for (sx, sy, value) in sources {
        for dy in -reach..=reach {
            for dx in -reach..=reach {
                if ((dx * dx + dy * dy) as f64).sqrt() > radius {
                    continue;
                }
                let (nx, ny) = (sx + dx, sy + dy);
                if nx < 0 || ny < 0 || nx >= width || ny >= height {
                    continue;
                }
                let index = ny as usize * grid.width() as usize + nx as usize;
                grown[index] = grown[index].max(value);
            }
        }
    }
    mask.copy_from_slice(&grown);
}

/// Cells a polygon covers: those whose centres fall inside it, plus those its
/// edges cross.
///
/// Filling by cell centre alone loses any polygon narrower than a cell — a wall
/// a metre thick on a two-metre grid can have no centre inside it, so a building
/// that is plainly on the ground produces an empty mask and blocks nothing.
/// Rasterising the edges as well makes the fill conservative: a cell an edge
/// passes through belongs to the polygon. For an obstacle that is the safe
/// direction to err in, and it costs one grid walk per edge.
///
/// The polygon is treated as closed whether or not its last vertex repeats the
/// first, which is what the parser hands over: it strips the duplicate so the
/// outline has no zero-length edge.
fn filled_cells(grid: &Grid, polygon: &[(f64, f64)]) -> Vec<(u32, u32)> {
    let mut bottom = f64::INFINITY;
    let mut top = f64::NEG_INFINITY;
    for vertex in polygon {
        bottom = bottom.min(vertex.1);
        top = top.max(vertex.1);
    }

    let mut cells = Vec::new();
    if let Some((y0, y1)) = grid.axis_cells(bottom, top, grid.height()) {
        let mut crossings = Vec::new();
        for y in y0..=y1 {
            let scan = (y as f64 + 0.5) * grid.res_m();
            crossings.clear();
            for i in 0..polygon.len() {
                let a = polygon[i];
                let b = polygon[(i + 1) % polygon.len()];
                if (a.1 <= scan) != (b.1 <= scan) {
                    let t = (scan - a.1) / (b.1 - a.1);
                    crossings.push(a.0 + t * (b.0 - a.0));
                }
            }
            crossings.sort_by(f64::total_cmp);
            for span in crossings.as_chunks::<2>().0 {
                let Some((x0, x1)) = grid.axis_cells(span[0], span[1], grid.width()) else {
                    continue;
                };
                for x in x0..=x1 {
                    cells.push((x, y));
                }
            }
        }
    }

    for i in 0..polygon.len() {
        edge_cells(
            grid,
            polygon[i],
            polygon[(i + 1) % polygon.len()],
            &mut cells,
        );
    }
    cells.sort_unstable();
    cells.dedup();
    cells
}

/// Appends every cell a segment passes through, by a walk between its ends.
///
/// The walk advances on the integer cell boundaries of the metre plane, so it
/// touches each cell the segment crosses exactly once and crosses none it does
/// not — the supercover of the segment, not a superset of it.
fn edge_cells(grid: &Grid, a: (f64, f64), b: (f64, f64), cells: &mut Vec<(u32, u32)>) {
    // Work in cell units so the boundaries are the integers.
    let res = grid.res_m();
    let (u0, v0, u1, v1) = (a.0 / res, a.1 / res, b.0 / res, b.1 / res);
    if ![u0, v0, u1, v1].iter().all(|value| value.is_finite()) {
        return;
    }
    let width = grid.width() as i64;
    let height = grid.height() as i64;

    let (du, dv) = (u1 - u0, v1 - v0);
    if du == 0.0 && dv == 0.0 {
        push_cell(cells, u0.floor() as i64, v0.floor() as i64, width, height);
        return;
    }

    let mut x = u0.floor() as i64;
    let mut y = v0.floor() as i64;
    let (end_x, end_y) = (u1.floor() as i64, v1.floor() as i64);
    let step_x = (du > 0.0) as i64 - (du < 0.0) as i64;
    let step_y = (dv > 0.0) as i64 - (dv < 0.0) as i64;
    // Distance along the segment, in units of its own length, to the first
    // boundary on each axis, and from one boundary to the next.
    let mut next_x = if step_x == 0 {
        f64::INFINITY
    } else {
        ((if step_x > 0 { x + 1 } else { x }) as f64 - u0) / du
    };
    let mut next_y = if step_y == 0 {
        f64::INFINITY
    } else {
        ((if step_y > 0 { y + 1 } else { y }) as f64 - v0) / dv
    };
    let delta_x = if step_x == 0 {
        f64::INFINITY
    } else {
        1.0 / du.abs()
    };
    let delta_y = if step_y == 0 {
        f64::INFINITY
    } else {
        1.0 / dv.abs()
    };

    // A straight segment crosses at most `width + height + 1` cells; the guard
    // only bounds a walk that rounding could otherwise send round in circles.
    for _ in 0..(width + height + 4) {
        push_cell(cells, x, y, width, height);
        if x == end_x && y == end_y {
            return;
        }
        if next_x < next_y {
            x += step_x;
            next_x += delta_x;
        } else {
            y += step_y;
            next_y += delta_y;
        }
    }
}

/// Appends a cell when it lies inside the grid.
fn push_cell(cells: &mut Vec<(u32, u32)>, x: i64, y: i64, width: i64, height: i64) {
    if x < 0 || y < 0 || x >= width || y >= height {
        return;
    }
    cells.push((x as u32, y as u32));
}
