// Cost field synthesis: C[cell][mode] = c0 + sum_d F[cell][d] * W[d][mode].
//
// One invocation produces one (cell, mode) pair. The feature dimension is small
// (8-16 in practice) and is traversed in a single loop: the weight column stays
// in registers, which is what a tile in shared memory would buy as well, without
// the added indexing complexity.

struct Params {
    cells: u32,
    dims: u32,
    modes: u32,
    c0: f32,
};

@group(0) @binding(0) var<uniform> params: Params;
@group(0) @binding(1) var<storage, read> features: array<f32>;
@group(0) @binding(2) var<storage, read> weights: array<f32>;
@group(0) @binding(3) var<storage, read_write> costs: array<f32>;

@compute @workgroup_size(64)
fn main(
    @builtin(global_invocation_id) gid: vec3<u32>,
    @builtin(num_workgroups) groups: vec3<u32>,
) {
    let total = params.cells * params.modes;
    // A whole map is millions of cells and a dispatch is capped at 65535 workgroups
    // per dimension, so the grid is walked in strides: the dispatch is clamped and
    // each invocation advances by the width of the whole dispatch until the field is
    // covered. A field that fits within the cap therefore still runs one iteration.
    let stride = groups.x * 64u;
    for (var index = gid.x; index < total; index = index + stride) {
        let cell = index / params.modes;
        let mode = index % params.modes;
        let feature_base = cell * params.dims;

        var sum = params.c0;
        for (var dim = 0u; dim < params.dims; dim = dim + 1u) {
            sum = sum + features[feature_base + dim] * weights[dim * params.modes + mode];
        }
        costs[index] = sum;
    }
}
