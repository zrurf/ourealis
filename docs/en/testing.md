# Testing

The test suite is layered, and each layer answers a different question. The
layering is deliberate: a unit test asserts that an implementation matches its
specification, so a defect in something the specification leaves open — a
discretisation step, a statistical convention, a sign — passes every unit test and
only shows up as output that does not look like a human running. The realism and
randomized layers exist for that class of defect.

```bash
cargo test --workspace --all-features
```

## Layers

| Suite | Question it answers |
|---|---|
| `map-format::omf_container` (23) | Do header and footer offsets, CRCs, directory ordering and binary search, missing-chunk semantics, skeleton keys and location, writer metadata freezing and the LOD levels behave? |
| `map-format::omf_codecs` (15) | Do all codecs round-trip, does bit packing agree with the shape, and are unknown codecs and malformed payloads handled? |
| `map-format::omf_graph` (12) | Do the roadmap, connector, path-library and vector layers round-trip, do region inclusion and the spatial hash work, and do derived-layer fingerprints verify and go stale correctly? |
| `map-format::omf_patch` (6) | Does a patch apply, refuse a foreign base file, refuse to touch a derived layer, and survive a reload? |
| `map-format::prop_roundtrip` (2) | Property tests: any raster content survives encode → decode, any metadata survives write → read. |
| `core::math_terrain` (13) | Do projection, terrain, slope, the distance transform and the FFT match closed-form references? |
| `core::field_search` (25) | Do cost synthesis, hard-constraint enforcement, the sampler, Lazy Theta\*, candidate generation, the Logit model, the elastic band and corner rounding behave? |
| `core::motion` (29) | Do the speed limits, profile, stop handling, fatigue iteration, offset, attitude, bounce and turn profile behave? |
| `core::sensors` (15) | Do sample rates, reproduction, GNSS errors and correlation, the resting accelerometer reading, the step spectrum, the gyroscope's turn rate, the magnetometer's magnitude and the barometer's ripple behave? |
| `core::simulator` (13) | Does a whole run hold its invariants, replay bit-identically, honour waypoint semantics, close a loop, follow a redirect, batch reproducibly, export correctly and report metrics? |
| `core::coarse` (7) | Are coarse blocks internally passable, do they replace the fine cells they cover, do they reduce the search, and are routes feasible with them on and off? |
| `core::library` (8) | Do stored candidate libraries obey the last-mile attach contract? |
| `core::connector_lift` (4) | Do Z-axis links reach the height, the barometer and the pitch, while a route without links keeps the terrain profile? |
| `core::realism` (14) | Does one route run by one individual stay inside human physiology bands? |
| `core::randomized` (3) | Do the invariants, physical consistency and realism bands hold for randomly drawn individuals and routes? |
| `core::real_data` (4) | Does the simulator match public IMU recordings on the calibrated gait quantities? |
| `core::calibration` (5) | Do the estimator and the optimiser work, does the loss behave, and do the shipped presets match the reference table? |
| `core::gpu_cpu` (6) | Do the GPU kernels agree with the CPU reference on the cost field, the noise and the projection queries? |

Two suites depend on external data and skip themselves with a printed reason when
it is absent: `real_data` needs tables under `data/real/`, `gpu_cpu` needs a GPU
adapter.

## The realism layer

`realism.rs` turns "does this look like a person" into numbers with public ranges:
the speed distribution, the noise colour, the turn-rate distribution, the path
ratio, the lateral-offset habit, lap-time variation — and the sensor consistency
checks that only make sense jointly, such as a resting accelerometer reading
$+9.81$, the accelerometer's step harmonic anti-phase with both the bounce height
and the barometric altitude, and the GNSS vertical spread against its horizontal
error.

`randomized.rs` applies the same bands to drawn cases rather than to one fixed
route, and adds the invariants any single trajectory can be checked against:

* time strictly increases;
* the truth stays on passable ground;
* no sample moves further than a runner can in one interval;
* the along-path distance agrees with the reported speed;
* the random streams stay in step.

Its case list comes from a fixed seed, so a failure names a case that can be
regenerated exactly. It is the breadth instrument; `realism` is the tuning
instrument and prints its measurements under `--nocapture`.

## Determinism

Three properties are asserted directly:

* the same seed produces bit-identical output;
* a parallel batch produces exactly the output a serial one does;
* region events in the deterministic mode reproduce their sequence across runs.

## Real data

`crates/core/tests/real_data.rs` compares the simulator against public recordings
on the quantities that have to come from measurement rather than from a
specification: step frequency, the accelerometer's second and third harmonic
ratios, and the gyroscope's step signature. Both sides are measured with the same
estimator (`eval::calibrate::gait_of`), so a reported difference is a difference
between the simulator and the recordings rather than between two measurements.

The supported collections are WISDM 2.0 (UCI 507), MotionSense, Daily and Sports
Activities (UCI 256) and HAR (UCI 240); each contributes a different sampling rate,
device placement or pace. The recordings are not part of the repository — see
[usage.md](usage.md#real-data) for how to convert them. Without any table, the layer
reports the skip and passes.

## The service and the page

The service is tested through its own facades rather than through its internals: a
router driven by `tower`'s `oneshot`, a gRPC client over an ephemeral port, and a real
binary behind the browser lane. What each suite pins down:

| Suite | Question it answers |
|---|---|
| `service::config` (12) | Do defaults, partial files, migrations, unknown keys and impossible values behave, and does the effective configuration render back as TOML? |
| `service::facades` (5) | Do the three facades bind and stop, can each be disabled on its own, and is a page without the API refused before anything binds? |
| `service::http_api` (24) | Status codes, prefixes, error bodies, paging, the embedded page, resource exhaustion (an oversized upload is `413`, a body within the limit reaches the handler), hostile ids and names, and the input bounds that used to abort the process | 
| `service::simulations` (13) | Submit → watch → read: a summary, sample streams, exports, cancellation, and the retention bound |
| `service::tasks` (10) | The task surface: a plan's result is the candidate set, a ticket that has not finished has no result, cancellation, kind filtering, a run's ticket readable through the task endpoints, event streams ending on a terminal event, and immediate `400`s |
| `service::feasibility` (6) | Whether a point may be stood on: forbidden, passable, outside, too-close-to-an-obstacle (the same point flips with the radius), the order of a whole route, and absurd requests |
| `service::rpc_api` (3) | The same resources over gRPC, including that a ticket submitted over HTTP is readable over gRPC |
| `service::streams` (6) | NDJSON framing and paging over the sample channels |

Two conventions matter here. A lane that needs a service **fails** rather than skipping
unless `OUREALIS_ALLOW_SKIP=1` is set: a lane that silently passes without the stack it
exists to exercise is worse than one that reports the problem. And the browser lane
builds the service in **release**, because a debug build plans a route in tens of seconds,
which turns its timeouts into a measurement of the planner rather than of the page.

## The front-end lanes

Playwright runs all four, one per directory:

| Lane | Command | What it covers |
|---|---|---|
| unit (139) | `run test:unit` | Pure logic, no browser: DTO mirrors, unit conversion, colormap mapping, paging windows, SSE and NDJSON parsing, metric formatting, chart option builders, the two catalogues' key sets, the chunk crop, and the render engine's backend probe |
| e2e (20) | `run test:e2e` | A real browser against a real service: import → list → preview (with `?engine=webgl2`), the workspace (draw a route, watch it plan, pick a candidate, submit, read the result), the map menu, the touch gestures, a refused route point, the whole simulation slice (trajectory, sensors, audit against a second run, OMF round-trip, batch sweep), the shell's theme and language, and one engine across views |
| fuzz (6) | `run test:fuzz` | Seeded random input: random bytes as an image, a truncated or bit-flipped image, random job bodies, absurd path parameters, and the persisted-state guards |
| monkey (3) | `run test:monkey` | Seeded random operation sequences over the stores and the interface, asserting no console error, no unhandled rejection and no failed request outside the expected set |

The browser lanes talk to the service at `OUREALIS_SERVICE_URL` (default
`http://127.0.0.1:8080`), which they build and start themselves for `test:e2e`.

## Conventions


* Tests live under `tests/`, never in the source tree. Integration tests are
  `crates/*/tests/*.rs`; unit tests for private items are
  `crates/*/tests/unit/*.rs`, included by the module under test through
  `#[cfg(test)] #[path = "..."] mod tests;`; shared fixtures are
  `crates/*/tests/fixtures/mod.rs`.
* A test that depends on missing data prints why it skipped and passes, rather than
  failing or being ignored.
* Where a test has to compensate for a known measurement convention, the
  compensation is written next to the assertion: a comparison that silently shifts
  its own reference is worse than no comparison.
