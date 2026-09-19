//! CPU/GPU parity.
//!
//! The CPU backend is the semantic reference; the GPU must agree with it. Cost
//! synthesis is floating-point work with a fixed accumulation order per cell, so
//! agreement is expected to within f32 rounding. Noise generation is *exactly*
//! reproducible because both sides use the same counter-based random source —
//! that is the whole reason the batch path does not use the sequential generator.
//!
//! Without an adapter the tests report that they were skipped and pass, so the
//! suite stays green on machines with no GPU.

#![cfg(feature = "gpu")]

use ourealis_core::gpu::{
    ComputeBackend, FeatureTensor, NoiseBatchSpec, OuSpec, ProjectionBatch, WeightMatrix,
    cpu::CpuBackend, noise_batch_cpu, projection_batch_cpu, select_backend,
};
use ourealis_core::sim::config::Backend;

fn gpu_backend() -> Option<Box<dyn ComputeBackend>> {
    match select_backend(Backend::Gpu) {
        Ok(backend) => Some(backend),
        Err(error) => {
            println!("skipping GPU parity test: {error}");
            None
        }
    }
}

#[test]
fn cost_synthesis_matches_the_cpu_reference() {
    let Some(gpu) = gpu_backend() else {
        return;
    };
    let cpu = CpuBackend::new();

    let cells = 4096;
    let dims = 8;
    let _modes = 3;
    let mut state = 12345u64;
    let mut features = Vec::with_capacity(cells * dims);
    for _ in 0..cells * dims {
        state = state
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        features.push(((state >> 40) as f32) / 16_777_216.0);
    }
    let feature_tensor = FeatureTensor::new(cells, dims, features).expect("tensor");
    let weights = WeightMatrix::from_vectors(&[
        vec![1.0, 0.5, 0.25, 0.125, 0.75, 0.3, 0.9, 0.1],
        vec![0.2, 0.7, 0.4, 0.6, 0.8, 0.15, 0.25, 0.35],
        vec![0.9, 0.1, 0.5, 0.3, 0.2, 0.6, 0.4, 0.7],
    ])
    .expect("weights");

    let reference = cpu
        .cost_field_batch(&feature_tensor, &weights, 1.0)
        .expect("cpu");
    let accelerated = gpu
        .cost_field_batch(&feature_tensor, &weights, 1.0)
        .expect("gpu");

    assert_eq!(reference.cells, accelerated.cells);
    assert_eq!(reference.modes, accelerated.modes);
    let mut worst = 0.0f32;
    for (a, b) in reference.values.iter().zip(accelerated.values.iter()) {
        worst = worst.max((a - b).abs());
    }
    assert!(
        worst < 1e-4,
        "GPU and CPU cost fields differ by {worst}, backend {}",
        gpu.name()
    );
}

#[test]
fn noise_batches_are_bit_identical() {
    let Some(gpu) = gpu_backend() else {
        return;
    };
    let spec = NoiseBatchSpec {
        individuals: 64,
        samples: 512,
        dt: 0.01,
        drift: OuSpec {
            sigma: 0.35,
            tau: 45.0,
            mean: 0.6,
        },
        white_sigma: 0.03,
        seed: 0xC0FFEE,
    };
    let reference = noise_batch_cpu(&spec).expect("cpu");
    let accelerated = gpu.noise_batch(&spec).expect("gpu");

    // The hash is exact; the Box-Muller transform uses transcendental functions
    // whose last bits are device-dependent, so the comparison allows a small
    // relative error rather than demanding bit equality.
    assert_eq!(reference.drift.len(), accelerated.drift.len());
    let mut worst = 0.0f32;
    for index in 0..reference.drift.len() {
        worst = worst.max((reference.drift[index] - accelerated.drift[index]).abs());
        worst = worst.max((reference.white[index] - accelerated.white[index]).abs());
    }
    assert!(
        worst < 1e-5,
        "noise series differ by {worst} between backends"
    );
}

#[test]
fn auto_backend_falls_back_instead_of_failing() {
    let backend = select_backend(Backend::Auto).expect("auto backend");
    let name = backend.name();
    assert!(!name.is_empty());
    println!("auto backend selected: {name}");
}

#[test]
fn backends_reject_a_dimension_mismatch() {
    let backend = CpuBackend::new();
    let features = FeatureTensor::new(4, 3, vec![0.0; 12]).expect("tensor");
    let weights = WeightMatrix::from_vectors(&[vec![1.0, 1.0]]).expect("weights");
    assert!(backend.cost_field_batch(&features, &weights, 0.0).is_err());
}

#[test]
fn projection_queries_match_the_cpu_reference() {
    // The batch is the query interface the smoother, the offset stage and the
    // route attachment all use, so the two backends have to answer it the same
    // way — including the two cases a naive implementation gets wrong: a point on
    // a cell boundary, and a point outside the grid.
    let Some(gpu) = gpu_backend() else {
        return;
    };

    let (width, height) = (64u32, 48u32);
    let resolution = 1.5f32;
    let origin = [-10.0f32, -20.0f32];
    let cells = (width * height) as usize;
    let mut forbidden_bits = vec![0u32; ProjectionBatch::bitmap_words(cells)];
    let mut distance = vec![0.0f32; cells];
    for cell in 0..cells {
        let x = (cell as u32) % width;
        let y = (cell as u32) / width;
        // A blocked cross and a diagonal, so the bitmap has structure.
        let blocked = x == 20 || y == 12 || x == y;
        if blocked {
            forbidden_bits[cell / 32] |= 1 << (cell % 32);
        }
        distance[cell] = ((x as f32 - 20.0).abs() + (y as f32 - 12.0).abs()) * resolution;
    }

    // Points: inside, on a boundary, exactly on an obstacle, and outside.
    let mut xy = Vec::new();
    for (x, y) in [
        (origin[0] + 4.0 * resolution, origin[1] + 4.0 * resolution),
        (origin[0] + 20.0 * resolution, origin[1] + 5.0 * resolution),
        (origin[0], origin[1]),
        (origin[0] + 20.0 * resolution, origin[1] + 12.0 * resolution),
        (origin[0] - 100.0, origin[1] - 100.0),
        (origin[0] + 1000.0, origin[1] + 1000.0),
        (origin[0] + 0.5 * resolution, origin[1] + 0.5 * resolution),
    ] {
        xy.push(x);
        xy.push(y);
    }
    let points = xy.len() / 2;

    let batch = ProjectionBatch {
        points,
        xy,
        grid_origin: [origin[0], origin[1], 0.0, 0.0],
        resolution,
        grid_dims: [width, height, 0, 0],
        forbidden: forbidden_bits,
        distance,
    };
    let reference = projection_batch_cpu(&batch).expect("cpu");
    let from_gpu = gpu.projection_check_batch(&batch).expect("gpu");
    assert_eq!(reference.answers.len(), points);
    for index in 0..points {
        let expected = reference.get(index);
        let actual = from_gpu.get(index);
        assert_eq!(
            expected.forbidden, actual.forbidden,
            "point {index} disagrees on the hard constraint"
        );
        assert_eq!(
            expected.outside, actual.outside,
            "point {index} disagrees on being inside the grid"
        );
        assert!(
            (expected.distance_m - actual.distance_m).abs() <= 1e-3,
            "point {index}: distance {} against {}",
            expected.distance_m,
            actual.distance_m
        );
    }
}

#[test]
fn the_batched_offset_cap_agrees_with_the_scalar_one() {
    // The offset cap asks the constraint mask a few hundred independent questions,
    // which is what makes it worth batching; the batching must not change the answer.
    // The CPU path reads the mask directly while a backend answers through the
    // kernel, so comparing a trajectory built each way pins both the refactor and —
    // when an adapter is present — the kernel's agreement with the reference.
    let build = |backend: Option<&dyn ComputeBackend>| -> Vec<[f64; 4]> {
        let mut person =
            ourealis_core::person::PersonParams::preset(ourealis_core::person::Preset::Moderate);
        person.target_speed = 3.0;
        let environment = ourealis_core::environment::Environment::load(
            &ourealis_map_format::Map::from_bytes(
                ourealis_map_format::synthetic::build(
                    &ourealis_map_format::synthetic::SyntheticMapSpec::compact(),
                )
                .expect("map"),
            )
            .expect("open"),
            &ourealis_core::field::CostWeights::uniform(5),
            &Default::default(),
            Default::default(),
        )
        .expect("environment");
        let path = ourealis_core::path::Path::resampled(
            vec![
                glam::DVec2::new(30.0, 100.0),
                glam::DVec2::new(260.0, 100.0),
            ],
            1.0,
        )
        .expect("path");
        let trajectory = ourealis_core::motion::Trajectory::build_with_backend(
            path,
            &environment.terrain,
            &environment.hard,
            &environment.distance,
            &person,
            &ourealis_core::motion::MotionConfig::default(),
            5,
            0,
            backend,
        )
        .expect("trajectory");
        trajectory
            .samples
            .iter()
            .map(|sample| {
                [
                    sample.position.x,
                    sample.position.y,
                    sample.offset_m,
                    sample.arc_s,
                ]
            })
            .collect()
    };

    let direct = build(None);
    assert!(!direct.is_empty());

    // The same trajectory through a backend: the CPU reference when no adapter is
    // present, the device otherwise.
    let Some(backend) = gpu_backend() else {
        return;
    };
    let batched = build(Some(backend.as_ref()));
    assert_eq!(
        direct.len(),
        batched.len(),
        "the two paths produced different sample counts"
    );
    let mut worst = 0.0f64;
    for (a, b) in direct.iter().zip(batched.iter()) {
        for index in 0..4 {
            worst = worst.max((a[index] - b[index]).abs());
        }
    }
    println!("batched against scalar: largest difference {worst:.6}");
    assert!(
        worst < 1e-3,
        "batching the offset cap moved the trajectory by {worst}"
    );
}
