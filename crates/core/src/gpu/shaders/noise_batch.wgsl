// Batched Ornstein-Uhlenbeck drift plus white noise.
//
// One invocation per individual: the recursion is sequential in time, so the
// batch dimension is the only one that parallelises. The random source is
// counter-based and mirrors `counter_gaussian` in Rust exactly, which is what
// makes a GPU batch bit-identical to the CPU reference.

struct Params {
    individuals: u32,
    samples: u32,
    seed_lo: u32,
    seed_hi: u32,
    dt: f32,
    sigma: f32,
    tau: f32,
    mean: f32,
    white_sigma: f32,
    padding0: u32,
    padding1: u32,
    padding2: u32,
};

@group(0) @binding(0) var<uniform> params: Params;
@group(0) @binding(1) var<storage, read_write> drift: array<f32>;
@group(0) @binding(2) var<storage, read_write> white: array<f32>;

// 32-bit finaliser, mirroring `hash32` in Rust. WGSL has no 64-bit integer type
// without an optional extension, so the whole hash stays in 32 bits.
fn hash32(z_in: u32) -> u32 {
    var z = z_in;
    z = (z ^ (z >> 16u)) * 0x7feb352du;
    z = (z ^ (z >> 15u)) * 0x846ca68bu;
    return z ^ (z >> 16u);
}

// Two hashes give two uniforms, which Box-Muller turns into a normal deviate.
fn gaussian(seed_lo: u32, seed_hi: u32, individual: u32, channel: u32, sample: u32) -> f32 {
    let base = seed_lo
        ^ hash32(individual ^ (channel * 2654435761u))
        ^ hash32(sample ^ seed_hi);
    let first = hash32(base);
    let second = hash32(first ^ 0x9e3779b9u);
    let u1 = max(f32(first >> 8u) / 16777216.0, 1e-30);
    let u2 = f32(second >> 8u) / 16777216.0;
    let radius = sqrt(-2.0 * log(u1));
    return radius * cos(6.2831855 * u2);
}

@compute @workgroup_size(64)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let individual = gid.x;
    if (individual >= params.individuals) {
        return;
    }

    let theta = select(1.0 / params.tau, 1000000.0, params.tau <= 1e-6);
    // Exact discrete O-U update, kept identical to `noise_batch_cpu`.
    let decay = exp(-theta * params.dt);
    let step_sigma = params.sigma * sqrt(max(1.0 - decay * decay, 0.0));

    var value = params.mean;
    let base = individual * params.samples;
    for (var sample = 0u; sample < params.samples; sample = sample + 1u) {
        let noise = gaussian(params.seed_lo, params.seed_hi, individual, 1u, sample);
        value = params.mean + (value - params.mean) * decay + step_sigma * noise;
        drift[base + sample] = value;
        white[base + sample] = params.white_sigma * gaussian(params.seed_lo, params.seed_hi, individual, 2u, sample);
    }
}
