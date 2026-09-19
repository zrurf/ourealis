# Usage

## Workspace

| Crate | Contents |
|---|---|
| `ourealis-map-format` | OMF: the static map container. Reader, writer, builder, codecs, fingerprints, patches. |
| `ourealis-core` | The simulator: environment, planning, motion, sensors, evaluation, compute backends. |

Dependencies point one way: `ourealis-core` reads maps through
`ourealis-map-format`, never the reverse.

## Build and verify

```bash
cargo build --workspace
cargo test  --workspace --all-features
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo fmt --all --check
cargo doc   --workspace --no-deps
```

The GPU backend is behind the `gpu` feature, which is enabled by default. Build
with `--no-default-features` for a CPU-only binary; the CPU backend is a complete
implementation of every kernel and is the semantic reference the GPU is compared
against.

## Examples

```bash
# Synthetic campus map -> OMF image -> route -> trajectory -> sensors -> files.
cargo run -p ourealis-core --example campus_run --release

# A population of individuals over one route, with the batch statistics.
cargo run -p ourealis-core --example population --release

# Convert downloaded recordings into the tables the tests read (see "Real data").
cargo run -p ourealis-core --example fetch_real_data

# Fit the gait parameters to the recordings. --write updates the reference table.
cargo run -p ourealis-core --release --example calibrate -- --verbose
cargo run -p ourealis-core --release --example calibrate -- --write

# Build a synthetic map and write it to disk without running anything.
cargo run -p ourealis-map-format --example build_synthetic_map
```

`campus_run` writes `run.json`, `csv/*.csv` and `track.geojson` under a temporary
directory and prints its path; `population` prints a per-individual summary. Neither
needs a map file: the synthetic map is generated in memory.

## A run in code

```rust
use glam::DVec2;
use ourealis_core::person::{PersonParams, Preset};
use ourealis_core::plan::StandardRequest;
use ourealis_core::sim::{MapSource, Simulator};

let output = Simulator::builder()
    .map(MapSource::omf("campus.omf"))          // or ::synthetic(spec) / ::bytes(image)
    .person(PersonParams::preset(Preset::Moderate))
    .standard(StandardRequest::new(
        DVec2::new(10.0, 10.0),
        DVec2::new(300.0, 200.0),
    ))
    .seed(42)
    .individual(0)
    .build()?
    .run()?;

println!("{:.1} s, {} GNSS fixes", output.duration_s(), output.sensors.gnss.len());
```

`build()` resolves the map, the cost weights, the compute backend and the planning
mode into a `Simulator`; `run()` produces a `SimulationOutput`:

| Field | Content |
|---|---|
| `truth` | `Vec<TruthState>`: position, velocity, acceleration, attitude, height, effective curvature, grade, and the standing and turning flags. |
| `sensors` | `Sensors { gnss, gnss_gaps, imu { accel, gyro }, mag, baro, mount, .. }`. |
| `trajectory` | The `Trajectory`: samples, path with its link channels, speed profile, bounce configuration, offsets, maneuvers and lap count. |
| `route` | `RouteSummary`: path points, length, cost, per-leg candidates and their choice probabilities. |
| `metrics` | `Option<MetricsReport>`, present when `with_metrics` is set (the default). |
| `manifest` | The reproducibility record: map, mode, individual parameters, seed, individual index, resolved backend and sample rates. |

## Planning modes

```rust
use ourealis_core::plan::{Checkpoint, LoopRequest, StandardRequest, ViaSemantics, Waypoint};

// Start -> waypoints in order -> goal. Each waypoint carries a behaviour:
//   Pass        cross at speed (the default)
//   Slow        drop to about 65% of the local limit inside its radius
//   Dwell{..}   stand still for the given time
let request = StandardRequest::new(start, goal)
    .via(Waypoint::new(mid).with_semantics(ViaSemantics::Dwell { duration_s: 15.0 }));

// A closed circuit, run a given number of laps.
let request = LoopRequest::new(start, 3);

// A redirect in mid-run.
let request = StandardRequest::new(start, goal);
let checkpoints = [Checkpoint { position: elsewhere, issued_at_s: 120.0 }];
```

`run()` executes the planned route and ignores any checkpoint list;
`run_dynamic()` consumes the checkpoints in time order, re-planning each one from
the runner's state at that moment and blending the new trajectory in over the
configured switch window.

## Configuration

`SimulationConfig` is the whole configuration tree. `SimulationConfig::default()`
is a realistic consumer phone with metrics enabled; two further constructors exist
for testing:

```rust
use ourealis_core::sensor::SensorConfig;
use ourealis_core::sim::SimulationConfig;

let mut config = SimulationConfig::default();      // realistic noise, metrics on
config.sensors = SensorConfig::clean();            // no sensor noise, no events
config.sensors = SensorConfig::calibrated();       // realistic noise, deterministic events
```

Sub-configurations are grouped by stage: `cost` (cost model), `coarse` (coarse
blocks), `prm` (roadmap), `route`/`loop_route`/`dynamic` (planning), `motion`
(speed limits, profile, offset, attitude, maneuvers), `sensors`, and `backend`.

`SensorConfig::force_deterministic_events` overrides the trigger mode the map
declares for region events. Calibration and regression runs need it: with
independent draws, two runs of identical inputs would produce different sensor data
and an optimiser would chase event noise as though it were part of its objective.

Sample rates are configurable: IMU 100 Hz (the truth rate), GNSS 1 Hz,
magnetometer 50 Hz, barometer 25 Hz by default. Time stamps are `t0 + k / f` and
strictly increasing.

## Population runs

```rust
use ourealis_core::person::{PersonSampler, Preset};
use ourealis_core::sim::BatchRunner;

let people = PersonSampler::preset(Preset::Moderate).sample_population(seed, 24)?;
let batch = BatchRunner::new(simulator);
let outputs = batch.run(&people)?;                       // rayon-parallel

let frequencies = batch.choice_frequencies(&outputs);    // route-choice histogram
let fit = batch.cadence_speed_fit(&outputs);             // cadence against speed
```

Individuals are independent and the random streams are keyed by the individual
index, so a parallel batch produces exactly the output a serial one would.
`BatchRunner` loads the environment once and reuses it for every individual.

## Exports

```rust
use ourealis_core::sim::export;

export::write_json(&output, "run.json")?;         // the whole output, serde JSON
export::write_csv_dir(&output, "csv/")?;          // truth, gnss, accel, gyro, mag, baro
export::write_geojson(&output, frame, "track.geojson")?;
```

`write_csv_dir` writes one table per stream. `truth.csv` carries time, position,
height together with its terrain and bounce components, speed, heading, pitch,
roll, effective curvature, lateral offset, grade and the standing and turning flags
— the same signals the sensors are derived from, so a downstream algorithm can be
checked against the exact input it was built on. `gnss.csv` reports latitude and
longitude in fractional degrees when the map has a reference point, and local
metres in every case.

## Real data

The recordings used by the calibration and comparison layers are not part of the
repository. Place any of the supported archives in `data/raw/` and convert them:

| Archive name | Collection |
|---|---|
| `wisdm2.zip` | WISDM 2.0 smartphone and smartwatch (UCI 507) |
| `motionsense.zip` | MotionSense |
| `dasa.zip` | Daily and Sports Activities (UCI 256) |
| `har.zip` | Human Activity Recognition (UCI 240) |

```bash
cargo run -p ourealis-core --example fetch_real_data
```

The tool writes one table per channel under `data/real/` with a `.meta` sidecar
recording the sampling rate and provenance, and prints the URLs of the archives it
did not find. A collection carrying both an accelerometer and a gyroscope produces
two tables. `cargo test` reads every table present, so the comparison layer widens
as datasets are added; with none present it reports the skip and passes.

`data/real/targets.toml` holds the reference values that calibration fits, written
by `cargo run -p ourealis-core --example calibrate -- --write`. The shipped
`PersonParams` presets reproduce them to within 0.05 tolerances.

## Where to go next

* how the pieces fit together: [architecture.md](architecture.md)
* why the numbers are what they are: [design.md](design.md)
* the map container: [map-format.md](map-format.md)
* the vocabulary: [glossary.md](glossary.md)
* what the tests guarantee: [testing.md](testing.md)
