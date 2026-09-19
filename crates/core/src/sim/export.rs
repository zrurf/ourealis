//! Output export.
//!
//! Three shapes are provided because they serve different consumers: one JSON
//! document with everything, a directory of CSV files that loads straight into
//! analysis tools, and a GeoJSON track for visual inspection on a map.

use std::io::Write;
use std::path::Path;

use crate::error::{CoreError, Result};

use super::output::SimulationOutput;

/// Writes the whole run as a single JSON document.
pub fn write_json(output: &SimulationOutput, path: impl AsRef<Path>) -> Result<()> {
    let path = path.as_ref();
    let document = serde_json::json!({
        "manifest": output.manifest,
        "route": output.route,
        "metrics": output.metrics,
        "truth": output.truth.iter().map(|state| serde_json::json!({
            "t": state.time_s,
            "x": state.position.x,
            "y": state.position.y,
            "z": state.z,
            "speed": state.speed,
            "heading": state.heading,
            "pitch": state.pitch,
            "roll": state.roll,
            "kappa_eff": state.kappa_eff,
            "offset_m": state.offset_m,
            "grade": state.grade,
        })).collect::<Vec<_>>(),
        "gnss": output.sensors.gnss,
        "accel": output.sensors.imu.accel,
        "gyro": output.sensors.imu.gyro,
        "mag": output.sensors.mag,
        "baro": output.sensors.baro,
        "gnss_gaps": output.sensors.gnss_gaps,
    });
    let text = serde_json::to_string_pretty(&document)
        .map_err(|e| CoreError::config(format!("failed to serialise output: {e}")))?;
    write_file(path, &text)
}

/// Writes one CSV file per stream into a directory.
pub fn write_csv_dir(output: &SimulationOutput, directory: impl AsRef<Path>) -> Result<()> {
    let directory = directory.as_ref();
    std::fs::create_dir_all(directory).map_err(|error| CoreError::Export {
        path: directory.display().to_string(),
        source: error,
    })?;

    write_file(
        directory.join("truth.csv"),
        &csv_of(
            "time_s,x,y,z,terrain_z,bounce_z,speed,heading,pitch,roll,kappa_eff,offset_m,grade,standing,turning",
            output.truth.iter().map(|state| {
                format!(
                    "{:.4},{:.4},{:.4},{:.4},{:.4},{:.4},{:.4},{:.6},{:.6},{:.6},{:.8},{:.4},{:.6},{},{}",
                    state.time_s,
                    state.position.x,
                    state.position.y,
                    state.z,
                    state.terrain_z,
                    state.z - state.terrain_z,
                    state.speed,
                    state.heading,
                    state.pitch,
                    state.roll,
                    state.kappa_eff,
                    state.offset_m,
                    state.grade,
                    state.standing as u8,
                    state.turning as u8,
                )
            }),
        ),
    )?;

    write_file(
        directory.join("gnss.csv"),
        &csv_of(
            "time_s,latitude_deg,longitude_deg,x,y,altitude_m,speed_mps,heading_rad,satellites",
            output.sensors.gnss.iter().map(|sample| {
                format!(
                    "{:.4},{},{},{:.4},{:.4},{:.4},{:.4},{:.6},{}",
                    sample.time_s,
                    sample
                        .latitude_deg
                        .map(|value| format!("{value:.7}"))
                        .unwrap_or_default(),
                    sample
                        .longitude_deg
                        .map(|value| format!("{value:.7}"))
                        .unwrap_or_default(),
                    sample.x,
                    sample.y,
                    sample.altitude_m,
                    sample.speed_mps,
                    sample.heading_rad,
                    sample.satellites,
                )
            }),
        ),
    )?;

    write_file(
        directory.join("accel.csv"),
        &csv_of(
            "time_s,x,y,z",
            output.sensors.imu.accel.iter().map(|sample| {
                format!(
                    "{:.4},{:.6},{:.6},{:.6}",
                    sample.time_s, sample.x, sample.y, sample.z
                )
            }),
        ),
    )?;

    write_file(
        directory.join("gyro.csv"),
        &csv_of(
            "time_s,x,y,z",
            output.sensors.imu.gyro.iter().map(|sample| {
                format!(
                    "{:.4},{:.6},{:.6},{:.6}",
                    sample.time_s, sample.x, sample.y, sample.z
                )
            }),
        ),
    )?;

    write_file(
        directory.join("mag.csv"),
        &csv_of(
            "time_s,x,y,z",
            output.sensors.mag.iter().map(|sample| {
                format!(
                    "{:.4},{:.4},{:.4},{:.4}",
                    sample.time_s, sample.x, sample.y, sample.z
                )
            }),
        ),
    )?;

    write_file(
        directory.join("baro.csv"),
        &csv_of(
            "time_s,pressure_pa,altitude_m",
            output.sensors.baro.iter().map(|sample| {
                format!(
                    "{:.4},{:.3},{:.4}",
                    sample.time_s, sample.pressure_pa, sample.altitude_m
                )
            }),
        ),
    )?;

    Ok(())
}

/// Writes the ground-truth track as GeoJSON.
///
/// Falls back to leaving the coordinates in local metres when the map carries no
/// geographic reference; the properties then say so instead of silently
/// labelling metre coordinates as degrees.
pub fn write_geojson(
    output: &SimulationOutput,
    frame: Option<&crate::math::LocalFrame>,
    path: impl AsRef<Path>,
) -> Result<()> {
    let coordinates: Vec<[f64; 2]> = output
        .truth
        .iter()
        .map(|state| match frame {
            Some(frame) => {
                let (lon, lat) = frame.to_geo_degrees(state.position);
                [lon, lat]
            }
            None => [state.position.x, state.position.y],
        })
        .collect();
    let document = serde_json::json!({
        "type": "FeatureCollection",
        "features": [{
            "type": "Feature",
            "geometry": { "type": "LineString", "coordinates": coordinates },
            "properties": {
                "generator": output.manifest.generator,
                "seed": output.manifest.seed,
                "individual": output.manifest.individual,
                "duration_s": output.duration_s(),
                "length_m": output.route.length_m,
                "coordinate_reference": if frame.is_some() { "epsg:4326" } else { "local metric plane" },
            },
        }],
    });
    let text = serde_json::to_string_pretty(&document)
        .map_err(|e| CoreError::config(format!("failed to serialise GeoJSON: {e}")))?;
    write_file(path.as_ref(), &text)
}

fn csv_of(header: &str, rows: impl Iterator<Item = String>) -> String {
    let mut out = String::with_capacity(header.len() + 64);
    out.push_str(header);
    out.push('\n');
    for row in rows {
        out.push_str(&row);
        out.push('\n');
    }
    out
}

fn write_file(path: impl AsRef<Path>, contents: &str) -> Result<()> {
    let path = path.as_ref();
    let mut file = std::fs::File::create(path).map_err(|error| CoreError::Export {
        path: path.display().to_string(),
        source: error,
    })?;
    file.write_all(contents.as_bytes())
        .map_err(|error| CoreError::Export {
            path: path.display().to_string(),
            source: error,
        })?;
    Ok(())
}
