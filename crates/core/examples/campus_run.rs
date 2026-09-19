//! Simulates one run on a synthetic campus and reports the result.
//!
//! Run with `cargo run -p ourealis-core --example campus_run --release`.

use glam::DVec2;

use ourealis_core::person::{PaceStrategy, PersonParams, Preset};
use ourealis_core::plan::{StandardRequest, Waypoint};
use ourealis_core::sim::{MapSource, Simulator};
use ourealis_map_format::synthetic::SyntheticMapSpec;

fn main() -> ourealis_core::Result<()> {
    let spec = SyntheticMapSpec::default();
    let person = PersonParams::preset(Preset::Moderate)
        .with_label("demo-runner")
        .with_pace_strategy(PaceStrategy::NegativeSplit);

    // Points on the synthetic map's road ring and crossing street.
    let request = StandardRequest::new(DVec2::new(90.0, 200.0), DVec2::new(520.0, 200.0))
        .via(Waypoint::new(DVec2::new(300.0, 60.0)))
        .via(Waypoint::new(DVec2::new(150.0, 340.0)));

    let output = Simulator::builder()
        .map(MapSource::synthetic(spec))
        .person(person)
        .standard(request)
        .seed(20_260_918)
        .individual(0)
        .build()?
        .run()?;

    println!(
        "route: {:.1} m over {:.1} s",
        output.route.length_m,
        output.duration_s()
    );
    println!(
        "samples: {} truth, {} GNSS, {} accelerometer, {} barometer",
        output.truth.len(),
        output.sensors.gnss.len(),
        output.sensors.imu.accel.len(),
        output.sensors.baro.len()
    );
    println!(
        "GNSS availability: {:.1} %",
        output.sensors.gnss_availability() * 100.0
    );
    if let Some(metrics) = &output.metrics {
        println!(
            "path ratio {:.2}, mean speed {:.2} m/s, turn rate p95 {:.3} rad/s",
            metrics.path_ratio, metrics.speed.mean, metrics.turn_rate.p95
        );
        println!(
            "barometric bounce amplitude {:.1} cm",
            metrics.baro_bounce_m * 100.0
        );
    }

    let directory = std::env::temp_dir().join("ourealis-campus-run");
    std::fs::create_dir_all(&directory).map_err(|error| ourealis_core::CoreError::Export {
        path: directory.display().to_string(),
        source: error,
    })?;
    ourealis_core::sim::export::write_json(&output, directory.join("run.json"))?;
    ourealis_core::sim::export::write_csv_dir(&output, &directory)?;
    ourealis_core::sim::export::write_geojson(&output, None, directory.join("track.geojson"))?;
    println!("wrote {}", directory.display());
    Ok(())
}
