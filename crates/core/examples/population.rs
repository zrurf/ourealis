//! Simulates a population of runners and reports the choice statistics.
//!
//! Run with `cargo run -p ourealis-core --example population --release`.

use glam::DVec2;

use ourealis_core::person::{PersonSampler, Preset};
use ourealis_core::plan::StandardRequest;
use ourealis_core::sim::{BatchRunner, MapSource, Simulator};
use ourealis_map_format::synthetic::SyntheticMapSpec;

fn main() -> ourealis_core::Result<()> {
    const RUNNERS: usize = 24;

    let simulator = Simulator::builder()
        .map(MapSource::synthetic(SyntheticMapSpec::compact()))
        .standard(StandardRequest::new(
            DVec2::new(40.0, 100.0),
            DVec2::new(260.0, 180.0),
        ))
        .seed(7)
        .build()?;

    let people = PersonSampler::preset(Preset::Moderate).sample_population(7, RUNNERS)?;
    let batch = BatchRunner::new(simulator);
    let outputs = batch.run(&people)?;

    let mut durations = Vec::with_capacity(outputs.len());
    for (index, output) in outputs.iter().enumerate() {
        let speed = output
            .metrics
            .as_ref()
            .map(|metrics| metrics.speed.mean)
            .unwrap_or(0.0);
        durations.push(output.duration_s());
        println!(
            "runner {index:2}: {:.1} s, mean {:.2} m/s, route {:.1} m",
            output.duration_s(),
            speed,
            output.route.length_m
        );
    }

    let frequencies = batch.choice_frequencies(&outputs);
    println!("candidate frequencies: {frequencies:?}");
    let fastest = durations.iter().cloned().fold(f64::INFINITY, f64::min);
    let slowest = durations.iter().cloned().fold(0.0f64, f64::max);
    println!("finish spread: {fastest:.1} s to {slowest:.1} s");
    Ok(())
}
