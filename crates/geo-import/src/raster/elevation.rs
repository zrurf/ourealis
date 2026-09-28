//! Sampling terrain elevation onto the grid.
//!
//! The DEM covers the request exactly, but its tiles still carry voids — radar
//! gaps, water masks, tile edges — and a hole in the elevation layer would put a
//! cliff in the middle of the map. Voids are therefore filled from their
//! neighbours before the layer is handed to the builder.

use crate::dem::DemMosaic;
use crate::raster::grid::Grid;

/// Elevation of every cell centre in metres, with voids filled.
pub fn sample(grid: &Grid, dem: &DemMosaic) -> Vec<f32> {
    let mut values = vec![f32::NAN; grid.len()];
    let mut voids = 0usize;
    for y in 0..grid.height() {
        for x in 0..grid.width() {
            let center = grid.cell_center(x, y);
            let (lon, lat) = grid.geo_of_local(center.0, center.1);
            match dem.sample(lon, lat) {
                Some(height) if height.is_finite() => {
                    values[grid.index(x, y)] = height as f32;
                }
                _ => voids += 1,
            }
        }
    }
    if voids > 0 {
        let filled = inpaint(&mut values, grid);
        tracing::warn!(
            voids,
            filled,
            remaining = voids - filled,
            cells = grid.len(),
            "filled dem voids from neighbours"
        );
    }
    values
}

/// Replaces every `NaN` with the mean of its valid neighbours, repeatedly, until
/// nothing changes. Returns how many cells were filled.
///
/// Each pass reads the previous state and writes the next, so a pass is
/// order-independent and a hole several cells across fills from its rim inwards
/// one ring per pass.
fn inpaint(values: &mut [f32], grid: &Grid) -> usize {
    let width = grid.width() as i64;
    let height = grid.height() as i64;
    let mut filled_total = 0usize;
    loop {
        let mut updates: Vec<(usize, f32)> = Vec::new();
        for y in 0..height {
            for x in 0..width {
                let index = y as usize * width as usize + x as usize;
                if !values[index].is_nan() {
                    continue;
                }
                let mut sum = 0.0f64;
                let mut count = 0u32;
                for dy in -1..=1 {
                    for dx in -1..=1 {
                        if dx == 0 && dy == 0 {
                            continue;
                        }
                        let (nx, ny) = (x + dx, y + dy);
                        if nx < 0 || ny < 0 || nx >= width || ny >= height {
                            continue;
                        }
                        let neighbour = values[ny as usize * width as usize + nx as usize];
                        if neighbour.is_finite() {
                            sum += neighbour as f64;
                            count += 1;
                        }
                    }
                }
                if count > 0 {
                    updates.push((index, (sum / count as f64) as f32));
                }
            }
        }
        if updates.is_empty() {
            break;
        }
        filled_total += updates.len();
        for (index, value) in updates {
            values[index] = value;
        }
    }
    // A grid with no valid sample at all — an empty DEM — has nothing to
    // interpolate from, and sea level is a better answer than a hole.
    for value in values.iter_mut() {
        if value.is_nan() {
            *value = 0.0;
        }
    }
    filled_total
}
