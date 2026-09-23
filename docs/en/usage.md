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

## Service and web interface

The service is one binary in `crates/service` (crate and binary name `ourealis`).
It reads one TOML file, serves three facades and embeds the page:

```bash
pnpm --dir web install --frozen-lockfile         # once
cargo build --release -p ourealis                 # build.rs runs the Vite build and embeds web/dist
./target/release/ourealis --config dev.toml       # or with no argument: the defaults, loopback only
./target/release/ourealis --print-config          # the effective configuration, after defaults
```

| Facade | Switch | Carries |
|---|---|---|
| RPC | `server.rpc_enabled` | gRPC, package `ourealis.api.v1` |
| HTTP | `server.http_enabled` | REST under `/api/v1`, plus WebSocket and SSE |
| Web | `server.web_enabled` | the embedded page, at `/` (requires HTTP) |

### The run workspace

Open `/` and the interface is there. A run is produced on one page, `/run`, in five
stages:

| Stage | What it decides |
|---|---|
| Map | which map to plan against |
| Route | the mode and the points, drawn by clicking and dragging on the map |
| Runner | the preset, the seed and — in expert mode — every individual parameter |
| Sensors | the sample rates and, in expert mode, the noise and event switches |
| Run | the name, the metrics switch, and the button that submits |

A route is **drawn, not typed**: click the ground to place the start and the goal, drag
a handle to move it, double-click a handle to remove it. Planning is automatic — as soon
as the route is complete a preview runs — so the summary (length, path ratio, estimated
time) and the candidate table update as the route changes, and no run is needed to see
what a change did. The planner's own choice is marked in the candidate table: a run
takes that one, because path choice follows the route and the seed rather than a row in
a list.

**Simple** shows the decisions that change the result; **Expert** shows everything,
grouped, with a badge per group reporting how many fields differ from the recipe or the
defaults. Four recipes — campus jog, track intervals, phone and watch, clean truth —
fill the whole configuration in one click and stay editable.

Other pages: `/maps` (import, generate, download), `/maps/{id}` (preview with a cell
inspector and layer drapes), `/maps/{id}/studio` (draw regions and connectors, export),
`/batch` (multi-individual sweeps), `/omf` (structure tree, metadata edits, patches),
`/simulations/{id}` and its `/trajectory`, `/sensors`, `/audit` tabs, `/settings`.

### Tasks: every long operation is a ticket

Planning a route, taking a profile or generating a map takes seconds to minutes, so no
endpoint holds a request open for it. `POST /api/v1/tasks` returns `202` with a ticket
and the work runs on a blocking worker:

```jsonc
// POST /api/v1/tasks
{"kind": "route_preview", "request": { /* the same body POST /simulations takes */ }}
{"kind": "route_plan",    "request": { /* … */ }}
{"kind": "synthetic_map", "spec": { "preset": "compact", "seed": 7 }, "name": "fixture"}
```

| Call | Answers |
|---|---|
| `GET /api/v1/tasks/{id}` | `kind`, `state` (`queued`/`running`/`succeeded`/`failed`/`cancelled`), `stage`, `elapsed_s`, `error` |
| `GET /api/v1/tasks/{id}/result` | the payload, tagged by kind: `{"route": {…}}` or `{"map": {…}}`; `409` while it runs |
| `GET /api/v1/tasks/{id}/result/ref` | where the result lives, without fetching it |
| `DELETE /api/v1/tasks/{id}` | cancel; a queued task stops at once, a running one at the next boundary |
| `GET /api/v1/tasks/{id}/events` | SSE: state, stage, log, and one terminal event |
| `GET /api/v1/tasks/{id}/ws` | the same session over WebSocket |
| `GET /api/v1/tasks?kind=route_plan` | the tickets, newest first, filtered by kind |

A **run is a task too**, and `GET /tasks/{id}` answers for a run id, so one poller can
watch everything. A run's data stays on its own endpoints (`/simulations/{id}/summary`,
`/truth`, `/sensors`, `/export`) because a whole run does not fit in one response —
`/tasks/{id}/result` answers `415` for one and says so.

Requests are validated at submission when the answer does not need the map, so an
impossible individual or an unusable sensor rate is a `400` on the call rather than a
failure to poll for. Anything that needs the map (a corrupt image, no feasible path) is
a failed task with a message.

The page uses the same surface: work the interface waits on appears behind a modal
loader with its elapsed time and a cancel button; the rest is reported in the header's
task tray. Neither shows a percentage — the simulator's run is a single call, so there
is no fraction to report.

### Where a point may be

The map says where a runner can be, and the page cannot read it: the hard-forbidden mask
is a layer in the file, while the page draws only the surface. `POST
/api/v1/maps/{id}/feasibility` answers it for a list of points:

```jsonc
{"points": [{"x": 234, "y": 156}, {"x": 36, "y": 100}], "safe_radius_m": 0.75}
// { "items": [
//   {"point": …, "legal": false, "reason": "forbidden", "distance_m": 0,    "cell": [117, 78], "elevation_m": 13.8},
//   {"point": …, "legal": true,  "reason": "ok",        "distance_m": 0.75, "cell": [18, 50],  "elevation_m": 12.6}
// ]}
```

`reason` is `ok`, `outside` (beyond the map), `forbidden` (a blocked cell) or `too_close`
(passable, but nearer an obstacle than `safe_radius_m` — the rule the simulator's own
offset stage applies, so a point this call accepts is one the planner will take). The
endpoint reads the mask and the neighbouring cells only — no cost field, no graph — so it
answers in milliseconds and the workspace calls it as a point is dropped, marking a
refused point in the error colour and saying why in words.

### Front-end checks

```bash
pnpm --dir web run typecheck      # vue-tsc over src *and* tests, plus the node configs
pnpm --dir web run lint           # oxlint
pnpm --dir web run format:check   # oxfmt
pnpm --dir web run build          # the bundled page build.rs embeds
pnpm --dir web run test:unit      # pure logic, no browser
pnpm --dir web run test:e2e       # a real browser against a real service (builds it in release)
pnpm --dir web run test:fuzz      # seeded random input
pnpm --dir web run test:monkey    # seeded random operation sequences
```

The e2e lane needs a service: `test:e2e` builds it in release and expects it at
`OUREALIS_SERVICE_URL` (default `http://127.0.0.1:8080`). It fails rather than skipping
when there is none, unless `OUREALIS_ALLOW_SKIP=1` is set — a lane that silently passes
without the stack it exists to exercise is worse than one that reports the problem.

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
